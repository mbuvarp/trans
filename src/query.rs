use std::collections::BTreeMap;
use std::path::Path;

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::message_id::validate_message_id;
use crate::translations::load_language_translations;

pub fn list_required_languages(config: &TransConfig) -> Vec<String> {
    config.required_languages.clone()
}

pub fn get_translation(
    root: impl AsRef<Path>,
    config: &TransConfig,
    message_id: &str,
    language: &str,
) -> Result<String> {
    validate_message_id(message_id)?;

    if !config
        .available_languages
        .iter()
        .any(|lang| lang == language)
    {
        return Err(TransError::InvalidInput(format!(
            "language '{language}' is not in available_languages"
        )));
    }

    let translations = load_language_translations(root, config, language)?;
    translations
        .get(message_id)
        .cloned()
        .ok_or_else(|| {
            TransError::InvalidInput(format!(
                "message id '{message_id}' not found for language '{language}'"
            ))
        })
}

pub fn get_translations_all(
    root: impl AsRef<Path>,
    config: &TransConfig,
    message_id: &str,
) -> Result<BTreeMap<String, String>> {
    validate_message_id(message_id)?;

    let mut results = BTreeMap::new();
    for language in &config.available_languages {
        let translations = load_language_translations(&root, config, language)?;
        let value = translations.get(message_id).cloned().ok_or_else(|| {
            TransError::InvalidInput(format!(
                "message id '{message_id}' not found for language '{language}'"
            ))
        })?;
        results.insert(language.clone(), value);
    }

    Ok(results)
}
