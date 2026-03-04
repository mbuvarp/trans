use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::config::{ConfigMode, TransConfig};
use crate::error::{Result, TransError};

pub type FlatTranslations = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct NonStringValues {
    pub by_language: BTreeMap<String, Vec<String>>,
}

impl NonStringValues {
    pub fn is_empty(&self) -> bool {
        self.by_language.is_empty()
    }

    pub fn format_for_display(&self) -> String {
        let mut lines = Vec::new();
        for (language, paths) in &self.by_language {
            lines.push(format!("{language}: {}", paths.join(", ")));
        }
        lines.join("\n")
    }
}

pub fn load_translations_for_mode(
    path: impl AsRef<Path>,
    mode: ConfigMode,
) -> Result<FlatTranslations> {
    let path = path.as_ref();
    let contents = read_file(path)?;
    match mode {
        ConfigMode::ReactIntl => Ok(serde_json::from_str(&contents)?),
        ConfigMode::NextIntl => {
            let value: Value = serde_json::from_str(&contents)?;
            flatten_next_intl_json(path, &value)
        }
    }
}

pub fn save_translations_for_mode(
    path: impl AsRef<Path>,
    mode: ConfigMode,
    translations: &FlatTranslations,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload = match mode {
        ConfigMode::ReactIntl => serde_json::to_string_pretty(translations)?,
        ConfigMode::NextIntl => {
            let nested = unflatten_to_next_intl(translations)?;
            serde_json::to_string_pretty(&nested)?
        }
    };
    fs::write(path, payload)?;
    Ok(())
}

pub fn collect_non_string_values(root: &Path, config: &TransConfig) -> Result<NonStringValues> {
    if config.mode != ConfigMode::NextIntl {
        return Ok(NonStringValues {
            by_language: BTreeMap::new(),
        });
    }

    let mut by_language = BTreeMap::new();
    for language in &config.available_languages {
        let path = root
            .join(&config.language_files_path)
            .join(format!("{language}.json"));
        let contents = read_file(&path)?;
        let value: Value = serde_json::from_str(&contents)?;
        let issues = next_intl_non_string_paths(&value)?;
        if !issues.is_empty() {
            by_language.insert(language.clone(), issues);
        }
    }

    Ok(NonStringValues { by_language })
}

pub fn coerce_non_string_values(root: &Path, config: &TransConfig) -> Result<usize> {
    if config.mode != ConfigMode::NextIntl {
        return Ok(0);
    }

    let mut total = 0usize;
    for language in &config.available_languages {
        let path = root
            .join(&config.language_files_path)
            .join(format!("{language}.json"));
        let contents = read_file(&path)?;
        let mut value: Value = serde_json::from_str(&contents)?;
        let mut changed = false;
        coerce_next_intl_value(&mut value, &mut Vec::new(), &mut changed)?;
        if changed {
            fs::write(&path, serde_json::to_string_pretty(&value)?)?;
            total += 1;
        }
    }

    Ok(total)
}

pub fn migrate_mode(root: &Path, config: &TransConfig, target_mode: ConfigMode) -> Result<()> {
    if config.mode == target_mode {
        return Ok(());
    }

    let mut snapshots: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut by_language: BTreeMap<String, FlatTranslations> = BTreeMap::new();

    for language in &config.available_languages {
        let path = root
            .join(&config.language_files_path)
            .join(format!("{language}.json"));
        let bytes = fs::read(&path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                TransError::MissingLanguageFile(path.clone())
            } else {
                TransError::Io(err)
            }
        })?;
        snapshots.insert(language.clone(), bytes);

        let translations = load_translations_for_mode(&path, config.mode)?;
        by_language.insert(language.clone(), translations);
    }

    if target_mode == ConfigMode::NextIntl {
        let conflicts = detect_migration_conflicts(&by_language);
        if !conflicts.is_empty() {
            return Err(TransError::InvalidInput(format!(
                "mode migration aborted due to key conflicts:\n{}",
                conflicts.join("\n")
            )));
        }
    }

    for language in &config.available_languages {
        let path = root
            .join(&config.language_files_path)
            .join(format!("{language}.json"));
        let Some(translations) = by_language.get(language) else {
            continue;
        };
        if let Err(err) = save_translations_for_mode(&path, target_mode, translations) {
            restore_snapshots(root, config, &snapshots);
            return Err(err);
        }
    }

    Ok(())
}

fn restore_snapshots(root: &Path, config: &TransConfig, snapshots: &BTreeMap<String, Vec<u8>>) {
    for (language, bytes) in snapshots {
        let path = root
            .join(&config.language_files_path)
            .join(format!("{language}.json"));
        let _ = fs::write(path, bytes);
    }
}

fn detect_migration_conflicts(by_language: &BTreeMap<String, FlatTranslations>) -> Vec<String> {
    let mut conflicts = Vec::new();

    for (language, translations) in by_language {
        for key in translations.keys() {
            if has_empty_segments(key) {
                conflicts.push(format!(
                    "{language}: invalid message id '{key}' (contains empty segment)"
                ));
                continue;
            }
            let parts: Vec<&str> = key.split('.').collect();
            for idx in 1..parts.len() {
                let prefix = parts[..idx].join(".");
                if translations.contains_key(&prefix) {
                    conflicts.push(format!(
                        "{language}: '{prefix}' conflicts with descendant '{key}'"
                    ));
                }
            }
        }
    }

    conflicts.sort();
    conflicts.dedup();
    conflicts
}

fn read_file(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Err(TransError::MissingLanguageFile(path.to_path_buf()))
        }
        Err(err) => Err(err.into()),
    }
}

fn flatten_next_intl_json(path: &Path, value: &Value) -> Result<FlatTranslations> {
    let object = value.as_object().ok_or_else(|| {
        TransError::InvalidInput(format!(
            "next-intl file '{}' must contain a JSON object at the root",
            path.display()
        ))
    })?;

    let mut flat = BTreeMap::new();
    let mut non_string = Vec::new();
    let mut stack = Vec::new();
    flatten_object(path, object, &mut stack, &mut flat, &mut non_string)?;

    if !non_string.is_empty() {
        return Err(TransError::NextIntlNonStringValues(format!(
            "{}: {}",
            path.display(),
            non_string.join(", ")
        )));
    }

    Ok(flat)
}

fn flatten_object(
    path: &Path,
    object: &Map<String, Value>,
    stack: &mut Vec<String>,
    flat: &mut FlatTranslations,
    non_string: &mut Vec<String>,
) -> Result<()> {
    if object.is_empty() && !stack.is_empty() {
        non_string.push(stack.join("."));
        return Ok(());
    }

    for (key, value) in object {
        validate_next_intl_segment(path, key)?;
        stack.push(key.to_string());
        flatten_value(path, value, stack, flat, non_string)?;
        stack.pop();
    }

    Ok(())
}

fn flatten_value(
    path: &Path,
    value: &Value,
    stack: &mut Vec<String>,
    flat: &mut FlatTranslations,
    non_string: &mut Vec<String>,
) -> Result<()> {
    match value {
        Value::String(text) => {
            flat.insert(stack.join("."), text.clone());
        }
        Value::Object(map) => flatten_object(path, map, stack, flat, non_string)?,
        _ => {
            non_string.push(stack.join("."));
        }
    }

    Ok(())
}

fn next_intl_non_string_paths(value: &Value) -> Result<Vec<String>> {
    let object = value.as_object().ok_or_else(|| {
        TransError::InvalidInput(
            "next-intl file must contain a JSON object at the root".to_string(),
        )
    })?;

    let mut stack = Vec::new();
    let mut paths = Vec::new();
    collect_non_string_paths(object, &mut stack, &mut paths)?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn collect_non_string_paths(
    object: &Map<String, Value>,
    stack: &mut Vec<String>,
    paths: &mut Vec<String>,
) -> Result<()> {
    if object.is_empty() && !stack.is_empty() {
        paths.push(stack.join("."));
        return Ok(());
    }

    for (key, value) in object {
        if key.contains('.') {
            return Err(TransError::InvalidInput(format!(
                "next-intl key segment '{}' must not contain '.'",
                key
            )));
        }
        if key.trim().is_empty() {
            return Err(TransError::InvalidInput(
                "next-intl key segment must not be empty".to_string(),
            ));
        }

        stack.push(key.to_string());
        match value {
            Value::String(_) => {}
            Value::Object(map) => collect_non_string_paths(map, stack, paths)?,
            _ => paths.push(stack.join(".")),
        }
        stack.pop();
    }

    Ok(())
}

fn coerce_next_intl_value(
    value: &mut Value,
    stack: &mut Vec<String>,
    changed: &mut bool,
) -> Result<()> {
    let object = value.as_object_mut().ok_or_else(|| {
        TransError::InvalidInput(
            "next-intl file must contain a JSON object at the root".to_string(),
        )
    })?;

    coerce_object(object, stack, changed)
}

fn coerce_object(
    object: &mut Map<String, Value>,
    stack: &mut Vec<String>,
    changed: &mut bool,
) -> Result<()> {
    if object.is_empty() && !stack.is_empty() {
        *changed = true;
    }

    for (key, value) in object {
        if key.contains('.') {
            return Err(TransError::InvalidInput(format!(
                "next-intl key segment '{}' must not contain '.'",
                key
            )));
        }
        if key.trim().is_empty() {
            return Err(TransError::InvalidInput(
                "next-intl key segment must not be empty".to_string(),
            ));
        }

        stack.push(key.to_string());
        match value {
            Value::String(_) => {}
            Value::Object(map) => {
                if map.is_empty() {
                    *value = Value::String("{}".to_string());
                    *changed = true;
                } else {
                    coerce_object(map, stack, changed)?;
                }
            }
            Value::Number(number) => {
                *value = Value::String(number.to_string());
                *changed = true;
            }
            Value::Bool(flag) => {
                *value = Value::String(flag.to_string());
                *changed = true;
            }
            Value::Null => {
                *value = Value::String("null".to_string());
                *changed = true;
            }
            Value::Array(_) => {
                let serialized = serde_json::to_string(value)?;
                *value = Value::String(serialized);
                *changed = true;
            }
        }
        stack.pop();
    }

    Ok(())
}

fn validate_next_intl_segment(path: &Path, segment: &str) -> Result<()> {
    if segment.contains('.') {
        return Err(TransError::InvalidInput(format!(
            "next-intl file '{}' has key segment '{}' containing '.'",
            path.display(),
            segment
        )));
    }
    if segment.trim().is_empty() {
        return Err(TransError::InvalidInput(format!(
            "next-intl file '{}' contains an empty key segment",
            path.display()
        )));
    }
    Ok(())
}

fn has_empty_segments(id: &str) -> bool {
    id.split('.').any(|segment| segment.trim().is_empty())
}

fn unflatten_to_next_intl(translations: &FlatTranslations) -> Result<Value> {
    let mut root = Node::Object(BTreeMap::new());

    for (id, value) in translations {
        let segments = parse_message_segments(id)?;
        insert_message(&mut root, &segments, value)?;
    }

    to_json_value(root)
}

fn parse_message_segments(id: &str) -> Result<Vec<&str>> {
    let mut segments = Vec::new();
    for segment in id.split('.') {
        if segment.trim().is_empty() {
            return Err(TransError::InvalidInput(format!(
                "invalid message id '{id}' (empty segment)"
            )));
        }
        if segment.contains('.') {
            return Err(TransError::InvalidInput(format!(
                "invalid message id '{id}' (segment contains '.')"
            )));
        }
        segments.push(segment);
    }
    Ok(segments)
}

fn insert_message(root: &mut Node, segments: &[&str], value: &str) -> Result<()> {
    let mut current = root;

    for (index, segment) in segments.iter().enumerate() {
        let is_last = index + 1 == segments.len();
        match current {
            Node::Leaf(existing) => {
                let parent = segments[..index].join(".");
                return Err(TransError::InvalidInput(format!(
                    "cannot nest under '{parent}' because it is already a leaf ('{existing}')"
                )));
            }
            Node::Object(children) => {
                if is_last {
                    if let Some(Node::Object(_)) = children.get(*segment) {
                        let id = segments.join(".");
                        return Err(TransError::InvalidInput(format!(
                            "cannot write leaf '{id}' because it conflicts with existing namespace"
                        )));
                    }
                    children.insert(segment.to_string(), Node::Leaf(value.to_string()));
                    return Ok(());
                }

                let entry = children
                    .entry(segment.to_string())
                    .or_insert_with(|| Node::Object(BTreeMap::new()));

                if matches!(entry, Node::Leaf(_)) {
                    let prefix = segments[..=index].join(".");
                    let id = segments.join(".");
                    return Err(TransError::InvalidInput(format!(
                        "cannot write '{id}' because '{prefix}' is already a leaf"
                    )));
                }

                current = entry;
            }
        }
    }

    Ok(())
}

fn to_json_value(node: Node) -> Result<Value> {
    match node {
        Node::Leaf(value) => Ok(Value::String(value)),
        Node::Object(children) => {
            let mut out = Map::new();
            for (key, child) in children {
                out.insert(key, to_json_value(child)?);
            }
            Ok(Value::Object(out))
        }
    }
}

enum Node {
    Leaf(String),
    Object(BTreeMap<String, Node>),
}

pub fn find_key_line_number(path: &Path, key: &str, mode: ConfigMode) -> Option<usize> {
    match mode {
        ConfigMode::ReactIntl => find_flat_key_line_number(path, key),
        ConfigMode::NextIntl => find_nested_key_line_number(path, key),
    }
}

fn find_flat_key_line_number(path: &Path, key: &str) -> Option<usize> {
    let contents = std::fs::read_to_string(path).ok()?;
    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix('"') else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(key) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        if rest.trim_start().starts_with(':') {
            return Some(index + 1);
        }
    }
    None
}

fn find_nested_key_line_number(path: &Path, key: &str) -> Option<usize> {
    let contents = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = contents.lines().collect();
    let segments: Vec<&str> = key.split('.').collect();

    let mut start = 0usize;
    let mut last_line = None;
    for segment in segments {
        let token = format!("\"{segment}\"");
        let mut found = None;
        for (offset, line) in lines[start..].iter().enumerate() {
            if line.contains(&token) {
                found = Some(start + offset + 1);
                start += offset + 1;
                break;
            }
        }
        let line = found?;
        last_line = Some(line);
    }

    last_line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(values: &[(&str, &str)]) -> FlatTranslations {
        values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn unflatten_then_flatten_round_trip() {
        let flat = map(&[("app.header.title", "Title"), ("app.footer.help", "Help")]);
        let value = unflatten_to_next_intl(&flat).expect("nested");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("en.json");
        fs::write(&path, serde_json::to_string_pretty(&value).expect("json")).expect("write");

        let loaded = load_translations_for_mode(&path, ConfigMode::NextIntl).expect("load");
        assert_eq!(loaded, flat);
    }

    #[test]
    fn detects_conflicts_for_unflatten() {
        let flat = map(&[("app", "x"), ("app.header", "y")]);
        assert!(unflatten_to_next_intl(&flat).is_err());
    }

    #[test]
    fn rejects_dot_in_next_intl_segment() {
        let value = serde_json::json!({"app.header": "Title"});
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("en.json");
        fs::write(&path, serde_json::to_string_pretty(&value).expect("json")).expect("write");

        assert!(load_translations_for_mode(&path, ConfigMode::NextIntl).is_err());
    }

    #[test]
    fn reports_non_string_values() {
        let value = serde_json::json!({"app": {"header": 1}});
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("en.json");
        fs::write(&path, serde_json::to_string_pretty(&value).expect("json")).expect("write");

        let err = load_translations_for_mode(&path, ConfigMode::NextIntl).expect_err("err");
        assert!(err.to_string().contains("next-intl non-string values"));
    }

    #[test]
    fn coerce_non_strings_updates_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let config = TransConfig {
            mode: ConfigMode::NextIntl,
            language_files_path: "messages".into(),
            available_languages: vec!["en".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: String::new(),
            default_export_format: crate::config::ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            ai: None,
        };

        fs::create_dir_all(root.join("messages")).expect("mkdir");
        let path = root.join("messages/en.json");
        fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "app": {
                    "n": 3,
                    "arr": [1, 2],
                    "nested": {"ok": "x"}
                }
            }))
            .expect("json"),
        )
        .expect("write");

        let updated = coerce_non_string_values(root, &config).expect("coerce");
        assert_eq!(updated, 1);

        let loaded = load_translations_for_mode(&path, ConfigMode::NextIntl).expect("load");
        assert_eq!(loaded.get("app.n"), Some(&"3".to_string()));
        assert_eq!(loaded.get("app.arr"), Some(&"[1,2]".to_string()));
        assert_eq!(loaded.get("app.nested.ok"), Some(&"x".to_string()));
    }

    #[test]
    fn detects_migration_conflicts_by_language() {
        let mut by_language = BTreeMap::new();
        by_language.insert("en".to_string(), map(&[("app", "x"), ("app.header", "y")]));

        let conflicts = detect_migration_conflicts(&by_language);
        assert!(conflicts.iter().any(|line| line.contains("conflicts")));
    }

    #[test]
    fn nested_line_lookup_best_effort() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("en.json");
        fs::write(
            &path,
            "{\n  \"app\": {\n    \"header\": {\n      \"title\": \"Title\"\n    }\n  }\n}\n",
        )
        .expect("write");

        let line = find_key_line_number(&path, "app.header.title", ConfigMode::NextIntl);
        assert_eq!(line, Some(4));
    }

    #[test]
    fn migration_rewrites_files_when_mode_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("messages")).expect("mkdir");
        fs::write(
            root.join("messages/en.json"),
            "{\n  \"app.header.title\": \"Title\"\n}\n",
        )
        .expect("write");

        let config = TransConfig {
            mode: ConfigMode::ReactIntl,
            language_files_path: "messages".into(),
            available_languages: vec!["en".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: String::new(),
            default_export_format: crate::config::ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            ai: None,
        };

        migrate_mode(root, &config, ConfigMode::NextIntl).expect("migrate");
        let text = fs::read_to_string(root.join("messages/en.json")).expect("read");
        assert!(text.contains("\"app\""));
        assert!(text.contains("\"header\""));
    }
}
