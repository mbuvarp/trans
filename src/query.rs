use std::collections::BTreeMap;
use std::path::Path;

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::message_id::validate_message_id;
use crate::translations::load_language_translations;

pub fn list_required_languages(config: &TransConfig) -> Vec<String> {
    config.required_languages.clone()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindMatchKind {
    Exact,
    Casing,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindMatch {
    pub message_id: String,
    pub kind: FindMatchKind,
}

pub fn find_translations(
    root: impl AsRef<Path>,
    config: &TransConfig,
    query: &str,
    language: Option<&str>,
    exact_only: bool,
    case_sensitive: bool,
) -> Result<Vec<FindMatch>> {
    let language = language.unwrap_or(&config.primary_language);
    if !config
        .available_languages
        .iter()
        .any(|available| available == language)
    {
        return Err(TransError::InvalidInput(format!(
            "language '{language}' is not in available_languages"
        )));
    }

    let translations = load_language_translations(root, config, language)?;
    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();

    for (message_id, value) in translations {
        if let Some(kind) =
            classify_find_match(&value, query, &query_lower, exact_only, case_sensitive)
        {
            matches.push(FindMatch { message_id, kind });
        }
    }

    matches.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.message_id.cmp(&right.message_id))
    });
    Ok(matches)
}

fn classify_find_match(
    value: &str,
    query: &str,
    query_lower: &str,
    exact_only: bool,
    case_sensitive: bool,
) -> Option<FindMatchKind> {
    if value == query {
        return Some(FindMatchKind::Exact);
    }

    if exact_only {
        return None;
    }

    if case_sensitive {
        return value.contains(query).then_some(FindMatchKind::Partial);
    }

    let value_lower = value.to_lowercase();
    if value_lower == query_lower {
        Some(FindMatchKind::Casing)
    } else {
        value_lower
            .contains(query_lower)
            .then_some(FindMatchKind::Partial)
    }
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
    translations.get(message_id).cloned().ok_or_else(|| {
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::config::{ConfigMode, ExportFormat};
    use crate::translations::{Translations, save_language_translations};

    fn base_config() -> TransConfig {
        TransConfig {
            mode: ConfigMode::ReactIntl,
            language_files_path: PathBuf::from("messages"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            default_export_format: ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            run_update_check: false,
            ai: None,
        }
    }

    fn translations(values: &[(&str, &str)]) -> Translations {
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn setup_project(root: &Path, config: &TransConfig) {
        save_language_translations(
            root,
            config,
            "en",
            &translations(&[
                ("calendar.exact", "submit"),
                ("calendar.casing", "Submit"),
                ("calendar.partial", "Submit form"),
                ("calendar.unrelated", "Cancel"),
            ]),
        )
        .expect("save en");
    }

    #[test]
    fn find_translations_groups_and_sorts_matches() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        setup_project(dir.path(), &config);

        let matches =
            find_translations(dir.path(), &config, "submit", None, false, false).expect("find");

        assert_eq!(
            matches,
            vec![
                FindMatch {
                    message_id: "calendar.exact".to_string(),
                    kind: FindMatchKind::Exact,
                },
                FindMatch {
                    message_id: "calendar.casing".to_string(),
                    kind: FindMatchKind::Casing,
                },
                FindMatch {
                    message_id: "calendar.partial".to_string(),
                    kind: FindMatchKind::Partial,
                },
            ]
        );
    }

    #[test]
    fn find_translations_exact_only_excludes_casing() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        setup_project(dir.path(), &config);

        let matches =
            find_translations(dir.path(), &config, "submit", None, true, false).expect("find");

        assert_eq!(
            matches,
            vec![FindMatch {
                message_id: "calendar.exact".to_string(),
                kind: FindMatchKind::Exact,
            }]
        );
    }

    #[test]
    fn find_translations_case_sensitive_excludes_casing() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        setup_project(dir.path(), &config);

        let matches =
            find_translations(dir.path(), &config, "Submit", None, false, true).expect("find");

        assert_eq!(
            matches,
            vec![
                FindMatch {
                    message_id: "calendar.casing".to_string(),
                    kind: FindMatchKind::Exact,
                },
                FindMatch {
                    message_id: "calendar.partial".to_string(),
                    kind: FindMatchKind::Partial,
                },
            ]
        );
    }

    #[test]
    fn find_translations_rejects_unknown_language() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        setup_project(dir.path(), &config);

        let err = find_translations(dir.path(), &config, "submit", Some("fr"), false, false)
            .expect_err("invalid language");

        assert!(err.to_string().contains("available_languages"));
    }
}
