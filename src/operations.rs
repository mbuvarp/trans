use std::collections::BTreeMap;
use std::path::Path;

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::message_id::validate_message_id;
use crate::translations::save_language_translations;
use crate::verify::{restore_translations, snapshot_translations, verify_language_files, TranslationSnapshot};

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

    let primary = updated
        .get(&config.primary_language)
        .ok_or_else(|| {
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

    let primary = updated
        .get(&config.primary_language)
        .ok_or_else(|| {
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

    let primary = updated
        .get(&config.primary_language)
        .ok_or_else(|| {
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
        if !config.available_languages.iter().any(|lang| lang == language) {
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

fn write_snapshot(
    root: &Path,
    config: &TransConfig,
    snapshot: &TranslationSnapshot,
) -> Result<()> {
    for language in &config.available_languages {
        let translations = snapshot
            .get(language)
            .ok_or_else(|| missing_language_in_snapshot(language))?;
        save_language_translations(root, config, language, translations)?;
    }
    Ok(())
}

fn missing_language_in_snapshot(language: &str) -> TransError {
    TransError::InvalidInput(format!(
        "missing translations for language '{language}'"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    use crate::config::TransConfig;
    use crate::translations::{load_language_translations, save_language_translations, Translations};

    fn base_config() -> TransConfig {
        TransConfig {
            language_files_path: PathBuf::from("messages"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
        }
    }

    fn translations(values: &[(&str, &str)]) -> Translations {
        values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn setup_files(root: &Path, config: &TransConfig) {
        save_language_translations(
            root,
            config,
            "en",
            &translations(&[("app.title", "Title")]),
        )
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
}
