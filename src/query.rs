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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HasMessageIdResult {
    pub found: Vec<String>,
    pub not_found: Vec<String>,
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

pub fn has_message_id(
    root: impl AsRef<Path>,
    config: &TransConfig,
    message_id: &str,
) -> Result<HasMessageIdResult> {
    validate_message_id(message_id)?;

    let mut found = Vec::new();
    let mut not_found = Vec::new();
    for language in &config.available_languages {
        let translations = load_language_translations(&root, config, language)?;
        if translations.contains_key(message_id) {
            found.push(language.clone());
        } else {
            not_found.push(language.clone());
        }
    }

    Ok(HasMessageIdResult { found, not_found })
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

    #[test]
    fn has_message_id_reports_all_languages_found() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        save_language_translations(
            dir.path(),
            &config,
            "en",
            &translations(&[("app.title", "Title")]),
        )
        .expect("save en");
        save_language_translations(
            dir.path(),
            &config,
            "nb",
            &translations(&[("app.title", "Tittel")]),
        )
        .expect("save nb");

        let result = has_message_id(dir.path(), &config, "app.title").expect("has");

        assert_eq!(
            result,
            HasMessageIdResult {
                found: vec!["en".to_string(), "nb".to_string()],
                not_found: vec![],
            }
        );
    }

    #[test]
    fn has_message_id_reports_no_languages_found() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        save_language_translations(
            dir.path(),
            &config,
            "en",
            &translations(&[("app.other", "Other")]),
        )
        .expect("save en");
        save_language_translations(
            dir.path(),
            &config,
            "nb",
            &translations(&[("app.other", "Annet")]),
        )
        .expect("save nb");

        let result = has_message_id(dir.path(), &config, "app.title").expect("has");

        assert_eq!(
            result,
            HasMessageIdResult {
                found: vec![],
                not_found: vec!["en".to_string(), "nb".to_string()],
            }
        );
    }

    #[test]
    fn has_message_id_reports_partial_found_in_config_order() {
        let dir = tempdir().expect("tempdir");
        let mut config = base_config();
        config.available_languages = vec![
            "nb".to_string(),
            "en".to_string(),
            "pl".to_string(),
            "se".to_string(),
        ];
        save_language_translations(
            dir.path(),
            &config,
            "nb",
            &translations(&[("app.title", "Tittel")]),
        )
        .expect("save nb");
        save_language_translations(
            dir.path(),
            &config,
            "en",
            &translations(&[("app.title", "Title")]),
        )
        .expect("save en");
        save_language_translations(
            dir.path(),
            &config,
            "pl",
            &translations(&[("app.other", "Inne")]),
        )
        .expect("save pl");
        save_language_translations(
            dir.path(),
            &config,
            "se",
            &translations(&[("app.other", "Other")]),
        )
        .expect("save se");

        let result = has_message_id(dir.path(), &config, "app.title").expect("has");

        assert_eq!(
            result,
            HasMessageIdResult {
                found: vec!["nb".to_string(), "en".to_string()],
                not_found: vec!["pl".to_string(), "se".to_string()],
            }
        );
    }

    #[test]
    fn has_message_id_counts_empty_values_as_found() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        save_language_translations(
            dir.path(),
            &config,
            "en",
            &translations(&[("app.title", "")]),
        )
        .expect("save en");
        save_language_translations(
            dir.path(),
            &config,
            "nb",
            &translations(&[("app.title", "")]),
        )
        .expect("save nb");

        let result = has_message_id(dir.path(), &config, "app.title").expect("has");

        assert_eq!(
            result,
            HasMessageIdResult {
                found: vec!["en".to_string(), "nb".to_string()],
                not_found: vec![],
            }
        );
    }
}
