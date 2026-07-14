use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::message_id::validate_message_id;
use crate::message_store::validate_no_duplicate_json_keys;
use crate::translations::{
    Translations, language_file_path, load_language_translations, save_language_translations,
};
use crate::verify::{
    TranslationSnapshot, restore_translations, snapshot_translations, verify_language_files,
};

pub type TranslationValues = BTreeMap<String, String>;

pub fn add_translation(
    root: impl AsRef<Path>,
    config: &TransConfig,
    message_id: &str,
    values: &TranslationValues,
) -> Result<()> {
    validate_message_id(message_id)?;
    verify_language_files(&root, config)?;

    let snapshot = snapshot_translations(&root, config)?;
    let mut updated = snapshot.clone();

    let primary = updated.get(&config.primary_language).ok_or_else(|| {
        TransError::InvalidInput(format!(
            "missing primary language '{}' in snapshot",
            config.primary_language
        ))
    })?;
    if primary.contains_key(message_id) {
        return Err(TransError::InvalidInput(format!(
            "message id '{message_id}' already exists"
        )));
    }

    validate_values(config, values, true)?;

    for language in &config.available_languages {
        let entry = updated
            .get_mut(language)
            .ok_or_else(|| missing_language_in_snapshot(language))?;
        let value = values
            .get(language)
            .cloned()
            .unwrap_or_else(|| config.default_untranslated_value.clone());
        entry.insert(message_id.to_string(), value);
    }

    persist_with_rollback(&root, config, &snapshot, &updated)
}

pub fn update_translation(
    root: impl AsRef<Path>,
    config: &TransConfig,
    message_id: &str,
    values: &TranslationValues,
) -> Result<()> {
    validate_message_id(message_id)?;
    verify_language_files(&root, config)?;

    let snapshot = snapshot_translations(&root, config)?;
    let mut updated = snapshot.clone();

    let primary = updated.get(&config.primary_language).ok_or_else(|| {
        TransError::InvalidInput(format!(
            "missing primary language '{}' in snapshot",
            config.primary_language
        ))
    })?;
    if !primary.contains_key(message_id) {
        return Err(TransError::InvalidInput(format!(
            "message id '{message_id}' does not exist"
        )));
    }

    validate_values(config, values, true)?;

    for language in &config.available_languages {
        if let Some(value) = values.get(language) {
            let entry = updated
                .get_mut(language)
                .ok_or_else(|| missing_language_in_snapshot(language))?;
            entry.insert(message_id.to_string(), value.clone());
        }
    }

    persist_with_rollback(&root, config, &snapshot, &updated)
}

pub fn delete_translation(
    root: impl AsRef<Path>,
    config: &TransConfig,
    message_id: &str,
) -> Result<()> {
    validate_message_id(message_id)?;
    verify_language_files(&root, config)?;

    let snapshot = snapshot_translations(&root, config)?;
    let mut updated = snapshot.clone();

    let primary = updated.get(&config.primary_language).ok_or_else(|| {
        TransError::InvalidInput(format!(
            "missing primary language '{}' in snapshot",
            config.primary_language
        ))
    })?;
    if !primary.contains_key(message_id) {
        return Err(TransError::InvalidInput(format!(
            "message id '{message_id}' does not exist"
        )));
    }

    for language in &config.available_languages {
        let entry = updated
            .get_mut(language)
            .ok_or_else(|| missing_language_in_snapshot(language))?;
        entry.remove(message_id);
    }

    persist_with_rollback(&root, config, &snapshot, &updated)
}

pub fn change_message_id(
    root: impl AsRef<Path>,
    config: &TransConfig,
    old_id: &str,
    new_id: &str,
) -> Result<()> {
    validate_message_id(old_id)?;
    validate_message_id(new_id)?;
    if old_id == new_id {
        return Err(TransError::InvalidInput(
            "old and new message ids must differ".to_string(),
        ));
    }

    verify_language_files(&root, config)?;

    let snapshot = snapshot_translations(&root, config)?;
    let mut updated = snapshot.clone();

    let primary = updated.get(&config.primary_language).ok_or_else(|| {
        TransError::InvalidInput(format!(
            "missing primary language '{}' in snapshot",
            config.primary_language
        ))
    })?;
    if !primary.contains_key(old_id) {
        return Err(TransError::InvalidInput(format!(
            "message id '{old_id}' does not exist"
        )));
    }
    if primary.contains_key(new_id) {
        return Err(TransError::InvalidInput(format!(
            "message id '{new_id}' already exists"
        )));
    }

    for language in &config.available_languages {
        let entry = updated
            .get_mut(language)
            .ok_or_else(|| missing_language_in_snapshot(language))?;
        if let Some(value) = entry.remove(old_id) {
            entry.insert(new_id.to_string(), value);
        }
    }

    persist_with_rollback(&root, config, &snapshot, &updated)
}

pub fn add_language(root: impl AsRef<Path>, config: &TransConfig, language: &str) -> Result<()> {
    let language = language.trim();
    if language.is_empty() {
        return Err(TransError::InvalidInput(
            "language must not be empty".to_string(),
        ));
    }
    if config
        .available_languages
        .iter()
        .any(|lang| lang == language)
    {
        return Err(TransError::InvalidInput(format!(
            "language '{language}' already exists"
        )));
    }

    verify_language_files(&root, config)?;

    let primary_translations = load_language_translations(&root, config, &config.primary_language)?;

    let path = language_file_path(&root, config, language);
    if path.exists() {
        return Err(TransError::InvalidInput(format!(
            "language file already exists at {}",
            path.display()
        )));
    }

    let mut new_translations = Translations::new();
    for key in primary_translations.keys() {
        new_translations.insert(key.clone(), config.default_untranslated_value.clone());
    }

    save_language_translations(&root, config, language, &new_translations)?;

    let mut updated_config = config.clone();
    updated_config
        .available_languages
        .push(language.to_string());
    if let Err(err) = updated_config.save_to_root(&root) {
        let _ = std::fs::remove_file(&path);
        return Err(err);
    }

    if let Err(err) = verify_language_files(&root, &updated_config) {
        let _ = std::fs::remove_file(&path);
        let _ = config.save_to_root(&root);
        return Err(err);
    }

    Ok(())
}

pub fn delete_language(root: impl AsRef<Path>, config: &TransConfig, language: &str) -> Result<()> {
    let language = language.trim();
    if language.is_empty() {
        return Err(TransError::InvalidInput(
            "language must not be empty".to_string(),
        ));
    }
    if language == config.primary_language {
        return Err(TransError::InvalidInput(
            "cannot delete the primary language".to_string(),
        ));
    }
    if !config
        .available_languages
        .iter()
        .any(|lang| lang == language)
    {
        return Err(TransError::InvalidInput(format!(
            "language '{language}' does not exist"
        )));
    }

    verify_language_files(&root, config)?;

    let translations = load_language_translations(&root, config, language)?;
    let path = language_file_path(&root, config, language);

    if !path.exists() {
        return Err(TransError::InvalidInput(format!(
            "language file does not exist at {}",
            path.display()
        )));
    }

    std::fs::remove_file(&path)?;

    let mut updated_config = config.clone();
    updated_config
        .available_languages
        .retain(|lang| lang != language);
    updated_config
        .required_languages
        .retain(|lang| lang != language);

    if let Err(err) = updated_config.save_to_root(&root) {
        let _ = save_language_translations(&root, config, language, &translations);
        return Err(err);
    }

    if let Err(err) = verify_language_files(&root, &updated_config) {
        let _ = save_language_translations(&root, config, language, &translations);
        let _ = config.save_to_root(&root);
        return Err(err);
    }

    Ok(())
}

pub fn replace_default_untranslated_value(
    root: impl AsRef<Path>,
    config: &TransConfig,
    old_value: &str,
    new_value: &str,
) -> Result<usize> {
    verify_language_files(&root, config)?;

    let snapshot = snapshot_translations(&root, config)?;
    let mut updated = snapshot.clone();
    let mut replaced = 0usize;

    for language in &config.available_languages {
        let entry = updated
            .get_mut(language)
            .ok_or_else(|| missing_language_in_snapshot(language))?;
        for value in entry.values_mut() {
            if value == old_value {
                *value = new_value.to_string();
                replaced += 1;
            }
        }
    }

    persist_with_rollback(&root, config, &snapshot, &updated)?;
    Ok(replaced)
}

pub fn sort_translation_files(root: impl AsRef<Path>, config: &TransConfig) -> Result<usize> {
    let root = root.as_ref();
    let raw_snapshot = snapshot_raw_translation_files(root, config)?;
    let snapshot = snapshot_translations(root, config)?;
    write_sort_snapshot(
        config,
        &snapshot,
        &raw_snapshot,
        |language, translations| save_language_translations(root, config, language, translations),
    )?;
    Ok(snapshot.len())
}

fn snapshot_raw_translation_files(
    root: &Path,
    config: &TransConfig,
) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut snapshot = BTreeMap::new();
    for language in &config.available_languages {
        let path = language_file_path(root, config, language);
        validate_no_duplicate_json_keys(&path)?;
        snapshot.insert(path.clone(), std::fs::read(path)?);
    }
    Ok(snapshot)
}

fn write_sort_snapshot<F>(
    config: &TransConfig,
    snapshot: &TranslationSnapshot,
    raw_snapshot: &BTreeMap<PathBuf, Vec<u8>>,
    mut save: F,
) -> Result<()>
where
    F: FnMut(&str, &Translations) -> Result<()>,
{
    for language in &config.available_languages {
        let Some(translations) = snapshot.get(language) else {
            restore_raw_translation_files(raw_snapshot);
            return Err(missing_language_in_snapshot(language));
        };
        if let Err(err) = save(language, translations) {
            restore_raw_translation_files(raw_snapshot);
            return Err(err);
        }
    }
    Ok(())
}

fn restore_raw_translation_files(snapshot: &BTreeMap<PathBuf, Vec<u8>>) {
    for (path, contents) in snapshot {
        let _ = std::fs::write(path, contents);
    }
}

fn validate_values(
    config: &TransConfig,
    values: &TranslationValues,
    require_required: bool,
) -> Result<()> {
    if values.is_empty() {
        return Err(TransError::InvalidInput(
            "values must not be empty".to_string(),
        ));
    }

    for language in values.keys() {
        if !config
            .available_languages
            .iter()
            .any(|lang| lang == language)
        {
            return Err(TransError::InvalidInput(format!(
                "language '{language}' is not in available_languages"
            )));
        }
    }

    if require_required {
        for language in &config.required_languages {
            if !values.contains_key(language) {
                return Err(TransError::InvalidInput(format!(
                    "missing value for required language '{language}'"
                )));
            }
        }
    }

    Ok(())
}

fn persist_with_rollback(
    root: impl AsRef<Path>,
    config: &TransConfig,
    snapshot: &TranslationSnapshot,
    updated: &TranslationSnapshot,
) -> Result<()> {
    let root = root.as_ref();

    if let Err(err) = write_snapshot(root, config, updated) {
        let _ = restore_translations(root, config, snapshot);
        return Err(err);
    }

    if let Err(err) = verify_language_files(root, config) {
        let _ = restore_translations(root, config, snapshot);
        return Err(err);
    }

    Ok(())
}

fn write_snapshot(root: &Path, config: &TransConfig, snapshot: &TranslationSnapshot) -> Result<()> {
    for language in &config.available_languages {
        let translations = snapshot
            .get(language)
            .ok_or_else(|| missing_language_in_snapshot(language))?;
        save_language_translations(root, config, language, translations)?;
    }
    Ok(())
}

fn missing_language_in_snapshot(language: &str) -> TransError {
    TransError::InvalidInput(format!("missing translations for language '{language}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    use crate::config::TransConfig;
    use crate::translations::{
        Translations, load_language_translations, save_language_translations,
    };

    fn base_config() -> TransConfig {
        TransConfig {
            mode: crate::config::ConfigMode::ReactIntl,
            language_files_path: PathBuf::from("messages"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            newline_at_end_of_file: false,
            default_export_format: crate::config::ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            run_update_check: false,
            ai: None,
        }
    }

    fn translations(values: &[(&str, &str)]) -> Translations {
        values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn setup_files(root: &Path, config: &TransConfig) {
        save_language_translations(root, config, "en", &translations(&[("app.title", "Title")]))
            .expect("save en");
        save_language_translations(
            root,
            config,
            "nb",
            &translations(&[("app.title", "Tittel")]),
        )
        .expect("save nb");
    }

    #[test]
    fn add_translation_inserts_defaults_for_optional_languages() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let root = dir.path();
        setup_files(root, &config);

        let mut values = TranslationValues::new();
        values.insert("en".to_string(), "New".to_string());

        add_translation(root, &config, "app.new", &values).expect("add");

        let en = load_language_translations(root, &config, "en").expect("load en");
        let nb = load_language_translations(root, &config, "nb").expect("load nb");

        assert_eq!(en.get("app.new").map(String::as_str), Some("New"));
        assert_eq!(nb.get("app.new").map(String::as_str), Some(""));
    }

    #[test]
    fn update_translation_overwrites_required_languages() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let root = dir.path();
        setup_files(root, &config);

        let mut values = TranslationValues::new();
        values.insert("en".to_string(), "Updated".to_string());

        update_translation(root, &config, "app.title", &values).expect("update");

        let en = load_language_translations(root, &config, "en").expect("load en");
        let nb = load_language_translations(root, &config, "nb").expect("load nb");

        assert_eq!(en.get("app.title").map(String::as_str), Some("Updated"));
        assert_eq!(nb.get("app.title").map(String::as_str), Some("Tittel"));
    }

    #[test]
    fn delete_translation_removes_from_all_languages() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let root = dir.path();
        setup_files(root, &config);

        delete_translation(root, &config, "app.title").expect("delete");

        let en = load_language_translations(root, &config, "en").expect("load en");
        let nb = load_language_translations(root, &config, "nb").expect("load nb");

        assert!(!en.contains_key("app.title"));
        assert!(!nb.contains_key("app.title"));
    }

    #[test]
    fn change_message_id_renames_key() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let root = dir.path();
        setup_files(root, &config);

        change_message_id(root, &config, "app.title", "app.title.new").expect("change id");

        let en = load_language_translations(root, &config, "en").expect("load en");
        let nb = load_language_translations(root, &config, "nb").expect("load nb");

        assert!(!en.contains_key("app.title"));
        assert!(!nb.contains_key("app.title"));
        assert_eq!(en.get("app.title.new").map(String::as_str), Some("Title"));
        assert_eq!(nb.get("app.title.new").map(String::as_str), Some("Tittel"));
    }

    #[test]
    fn replace_default_untranslated_value_updates_matching_entries() {
        let dir = tempdir().expect("tempdir");
        let mut config = base_config();
        config.default_untranslated_value = "".to_string();
        let root = dir.path();

        save_language_translations(
            root,
            &config,
            "en",
            &translations(&[("app.title", "Title")]),
        )
        .expect("save en");
        save_language_translations(root, &config, "nb", &translations(&[("app.title", "")]))
            .expect("save nb");

        let replaced =
            replace_default_untranslated_value(root, &config, "", "TODO").expect("replace");
        assert_eq!(replaced, 1);

        let nb = load_language_translations(root, &config, "nb").expect("load nb");
        assert_eq!(nb.get("app.title").map(String::as_str), Some("TODO"));
    }

    #[test]
    fn sort_translation_files_sorts_mismatched_react_intl_files() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let messages = dir.path().join("messages");
        std::fs::create_dir_all(&messages).expect("mkdir messages");
        std::fs::write(
            messages.join("en.json"),
            "{\n  \"app.zeta\": \"Zeta\",\n  \"app.alpha\": \"Alpha\"\n}\n",
        )
        .expect("write en");
        std::fs::write(
            messages.join("nb.json"),
            "{\n  \"other.zeta\": \"Siste\",\n  \"other.alpha\": \"Første\"\n}\n",
        )
        .expect("write nb");

        let sorted = sort_translation_files(dir.path(), &config).expect("sort");

        assert_eq!(sorted, 2);
        assert_eq!(
            std::fs::read_to_string(messages.join("en.json")).expect("read en"),
            "{\n  \"app.alpha\": \"Alpha\",\n  \"app.zeta\": \"Zeta\"\n}"
        );
        assert_eq!(
            std::fs::read_to_string(messages.join("nb.json")).expect("read nb"),
            "{\n  \"other.alpha\": \"Første\",\n  \"other.zeta\": \"Siste\"\n}"
        );
    }

    #[test]
    fn sort_translation_files_loads_every_file_before_writing() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let messages = dir.path().join("messages");
        std::fs::create_dir_all(&messages).expect("mkdir messages");
        let original_en = "{\n  \"app.zeta\": \"Zeta\",\n  \"app.alpha\": \"Alpha\"\n}\n";
        std::fs::write(messages.join("en.json"), original_en).expect("write en");
        std::fs::write(messages.join("nb.json"), "{ invalid json").expect("write nb");

        sort_translation_files(dir.path(), &config).expect_err("sort should fail");

        assert_eq!(
            std::fs::read_to_string(messages.join("en.json")).expect("read en"),
            original_en
        );
    }

    #[test]
    fn sort_translation_files_rejects_duplicate_keys_without_writing() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let messages = dir.path().join("messages");
        std::fs::create_dir_all(&messages).expect("mkdir messages");
        let original_en = "{\n  \"app.title\": \"First\",\n  \"app.title\": \"Second\"\n}\n";
        let original_nb = "{\n  \"app.zeta\": \"Siste\",\n  \"app.alpha\": \"Første\"\n}\n";
        std::fs::write(messages.join("en.json"), original_en).expect("write en");
        std::fs::write(messages.join("nb.json"), original_nb).expect("write nb");

        let err = sort_translation_files(dir.path(), &config).expect_err("sort should fail");

        assert!(err.to_string().contains("duplicate JSON key 'app.title'"));
        assert_eq!(
            std::fs::read_to_string(messages.join("en.json")).expect("read en"),
            original_en
        );
        assert_eq!(
            std::fs::read_to_string(messages.join("nb.json")).expect("read nb"),
            original_nb
        );
    }

    #[test]
    fn sort_write_failure_restores_original_file_bytes() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let messages = dir.path().join("messages");
        std::fs::create_dir_all(&messages).expect("mkdir messages");
        let original_en = "{\n    \"app.zeta\": \"Zeta\",\n    \"app.alpha\": \"Alpha\"\n}\n";
        let original_nb = "{\n    \"app.zeta\": \"Siste\",\n    \"app.alpha\": \"Første\"\n}\n";
        std::fs::write(messages.join("en.json"), original_en).expect("write en");
        std::fs::write(messages.join("nb.json"), original_nb).expect("write nb");
        let raw_snapshot =
            snapshot_raw_translation_files(dir.path(), &config).expect("raw snapshot");
        let snapshot = snapshot_translations(dir.path(), &config).expect("snapshot");
        let mut writes = 0usize;

        write_sort_snapshot(
            &config,
            &snapshot,
            &raw_snapshot,
            |language, translations| {
                writes += 1;
                if writes == 2 {
                    std::fs::write(language_file_path(dir.path(), &config, language), "")
                        .expect("truncate failing file");
                    return Err(std::io::Error::other("simulated write failure").into());
                }
                save_language_translations(dir.path(), &config, language, translations)
            },
        )
        .expect_err("write should fail");

        assert_eq!(
            std::fs::read_to_string(messages.join("en.json")).expect("read en"),
            original_en
        );
        assert_eq!(
            std::fs::read_to_string(messages.join("nb.json")).expect("read nb"),
            original_nb
        );
    }

    #[test]
    fn add_language_creates_new_language_file_and_updates_config() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let root = dir.path();
        setup_files(root, &config);

        add_language(root, &config, "fr").expect("add language");

        let fr = load_language_translations(root, &config, "fr").expect("load fr");
        assert_eq!(fr.get("app.title").map(String::as_str), Some(""));

        let updated = TransConfig::load_from_root(root).expect("load config");
        assert!(updated.available_languages.contains(&"fr".to_string()));
    }

    #[test]
    fn delete_language_removes_file_and_updates_config() {
        let dir = tempdir().expect("tempdir");
        let mut config = base_config();
        config.required_languages.push("nb".to_string());
        let root = dir.path();
        setup_files(root, &config);

        delete_language(root, &config, "nb").expect("delete language");

        let path = crate::translations::language_file_path(root, &config, "nb");
        assert!(!path.exists());

        let updated = TransConfig::load_from_root(root).expect("load config");
        assert!(!updated.available_languages.contains(&"nb".to_string()));
        assert!(!updated.required_languages.contains(&"nb".to_string()));
    }
}
