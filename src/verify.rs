use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::format_validation::collect_format_validation_issues;
use crate::message_store;
use crate::translations::{
    Translations, language_file_path, load_language_translations, save_language_translations,
};

pub type TranslationSnapshot = BTreeMap<String, Translations>;

#[derive(Debug, Clone)]
pub struct KeyMismatch {
    pub language: String,
    pub missing: BTreeSet<String>,
    pub extra: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct VerificationIssue {
    pub path: PathBuf,
    pub line: usize,
    pub message: String,
}

pub fn verify_language_files(root: impl AsRef<Path>, config: &TransConfig) -> Result<()> {
    let root = root.as_ref();
    let mut translations_by_language = BTreeMap::new();
    for language in &config.available_languages {
        let translations = load_language_translations(root, config, language)?;
        translations_by_language.insert(language.clone(), translations);
    }

    let base_language = &config.primary_language;
    let mismatches = key_mismatches(&translations_by_language, base_language);
    if let Some(mismatch) = mismatches.first() {
        let missing = format_key_list(&mismatch.missing);
        let extra = format_key_list(&mismatch.extra);
        return Err(TransError::VerificationFailed(format!(
            "language '{}' mismatch vs '{base_language}' (missing: {missing}, extra: {extra})",
            mismatch.language
        )));
    }

    Ok(())
}

pub fn collect_verification_issues(
    root: impl AsRef<Path>,
    config: &TransConfig,
) -> Result<Vec<VerificationIssue>> {
    let root = root.as_ref();
    let mut translations_by_language = BTreeMap::new();
    let mut issues = Vec::new();

    for language in &config.available_languages {
        let path = language_file_path(root, config, language);
        match load_language_translations(root, config, language) {
            Ok(translations) => {
                translations_by_language.insert(language.clone(), translations);
            }
            Err(TransError::MissingLanguageFile(_)) => {
                issues.push(VerificationIssue {
                    path,
                    line: 1,
                    message: "missing language file".to_string(),
                });
            }
            Err(TransError::Json(err)) => {
                issues.push(VerificationIssue {
                    path,
                    line: 1,
                    message: format!("invalid JSON: {err}"),
                });
            }
            Err(TransError::NextIntlNonStringValues(err)) => {
                issues.push(VerificationIssue {
                    path,
                    line: 1,
                    message: err,
                });
            }
            Err(err) => return Err(err),
        }
    }

    let Some(primary_translations) = translations_by_language.get(&config.primary_language) else {
        return Ok(issues);
    };

    let primary_keys: BTreeSet<String> = primary_translations
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();

    for (language, translations) in &translations_by_language {
        if language == &config.primary_language {
            continue;
        }
        let keys: BTreeSet<String> = translations.keys().cloned().collect();
        for key in primary_keys.difference(&keys) {
            let base_path = language_file_path(root, config, &config.primary_language);
            let line =
                message_store::find_key_line_number(&base_path, key, config.mode).unwrap_or(1);
            issues.push(VerificationIssue {
                path: base_path,
                line,
                message: format!(
                    "missing key '{key}' in '{language}' (present in '{}')",
                    config.primary_language
                ),
            });
        }
        for key in keys.difference(&primary_keys) {
            let path = language_file_path(root, config, language);
            let line = message_store::find_key_line_number(&path, key, config.mode).unwrap_or(1);
            issues.push(VerificationIssue {
                path,
                line,
                message: format!("extra key '{key}' not in '{}'", config.primary_language),
            });
        }
    }

    let format_issues = collect_format_validation_issues(config, &translations_by_language)?;
    for issue in format_issues {
        let path = language_file_path(root, config, &issue.language);
        let line = message_store::find_key_line_number(&path, &issue.id, config.mode).unwrap_or(1);
        issues.push(VerificationIssue {
            path,
            line,
            message: format!("{}: {}", issue.id, issue.message),
        });
    }

    Ok(issues)
}

pub fn key_mismatches(
    translations_by_language: &BTreeMap<String, Translations>,
    base_language: &str,
) -> Vec<KeyMismatch> {
    let base_keys = translations_by_language
        .get(base_language)
        .map(|translations| translations.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();

    let mut mismatches = Vec::new();
    for (language, translations) in translations_by_language {
        if language == base_language {
            continue;
        }
        let keys: BTreeSet<String> = translations.keys().cloned().collect();
        if keys != base_keys {
            let missing = base_keys.difference(&keys).cloned().collect();
            let extra = keys.difference(&base_keys).cloned().collect();
            mismatches.push(KeyMismatch {
                language: language.clone(),
                missing,
                extra,
            });
        }
    }
    mismatches
}

pub fn snapshot_translations(
    root: impl AsRef<Path>,
    config: &TransConfig,
) -> Result<TranslationSnapshot> {
    let root = root.as_ref();
    let mut snapshot = BTreeMap::new();
    for language in &config.available_languages {
        let translations = load_language_translations(root, config, language)?;
        snapshot.insert(language.clone(), translations);
    }
    Ok(snapshot)
}

pub fn restore_translations(
    root: impl AsRef<Path>,
    config: &TransConfig,
    snapshot: &TranslationSnapshot,
) -> Result<()> {
    let root = root.as_ref();
    for (language, translations) in snapshot {
        save_language_translations(root, config, language, translations)?;
    }
    Ok(())
}

fn format_key_list(keys: &BTreeSet<String>) -> String {
    if keys.is_empty() {
        return "none".to_string();
    }
    let mut list: Vec<&str> = keys.iter().map(String::as_str).collect();
    list.sort_unstable();
    let preview: Vec<&str> = list.into_iter().take(5).collect();
    if keys.len() > 5 {
        format!("{} (+{} more)", preview.join(", "), keys.len() - 5)
    } else {
        preview.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    use crate::config::TransConfig;
    use crate::translations::{Translations, save_language_translations};

    fn base_config() -> TransConfig {
        TransConfig {
            mode: crate::config::ConfigMode::ReactIntl,
            language_files_path: PathBuf::from("messages"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
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

    #[test]
    fn verify_succeeds_with_matching_keys() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let root = dir.path();

        save_language_translations(
            root,
            &config,
            "en",
            &translations(&[("app.title", "Title")]),
        )
        .expect("save en");
        save_language_translations(
            root,
            &config,
            "nb",
            &translations(&[("app.title", "Tittel")]),
        )
        .expect("save nb");

        assert!(verify_language_files(root, &config).is_ok());
    }

    #[test]
    fn verify_fails_with_mismatched_keys() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let root = dir.path();

        save_language_translations(
            root,
            &config,
            "en",
            &translations(&[("app.title", "Title")]),
        )
        .expect("save en");
        save_language_translations(
            root,
            &config,
            "nb",
            &translations(&[("app.other", "Annet")]),
        )
        .expect("save nb");

        assert!(verify_language_files(root, &config).is_err());
    }
}
