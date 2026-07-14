use std::collections::BTreeMap;
use std::path::Path;

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::translations::{Translations, load_language_translations, save_language_translations};

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub total_missing: usize,
    pub missing_by_language: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct SyncPlan {
    missing_by_language: BTreeMap<String, Vec<String>>,
}

impl SyncPlan {
    pub fn total_missing(&self) -> usize {
        self.missing_by_language
            .values()
            .map(|items| items.len())
            .sum()
    }

    pub fn report(&self) -> SyncReport {
        let mut missing_by_language = BTreeMap::new();
        let mut total_missing = 0usize;
        for (language, missing) in &self.missing_by_language {
            let count = missing.len();
            if count == 0 {
                continue;
            }
            total_missing += count;
            missing_by_language.insert(language.clone(), count);
        }
        SyncReport {
            total_missing,
            missing_by_language,
        }
    }
}

pub fn collect_missing_ids(root: &Path, config: &TransConfig) -> Result<SyncPlan> {
    let primary = load_language_translations(root, config, &config.primary_language)?;
    let primary_keys: Vec<String> = primary.keys().cloned().collect();
    if primary_keys.is_empty() {
        return Ok(SyncPlan {
            missing_by_language: BTreeMap::new(),
        });
    }

    let mut missing_by_language = BTreeMap::new();

    for language in &config.available_languages {
        if language == &config.primary_language {
            continue;
        }
        let translations = load_language_translations(root, config, language)?;
        let missing = collect_missing_keys(&translations, &primary_keys);
        if !missing.is_empty() {
            missing_by_language.insert(language.clone(), missing);
        }
    }

    Ok(SyncPlan {
        missing_by_language,
    })
}

pub fn apply_sync_plan(root: &Path, config: &TransConfig, plan: &SyncPlan) -> Result<SyncReport> {
    let mut total_missing = 0usize;
    let mut missing_by_language = BTreeMap::new();
    for (language, missing_keys) in &plan.missing_by_language {
        if missing_keys.is_empty() {
            continue;
        }
        let mut translations = load_language_translations(root, config, language)?;
        let added = add_missing_keys(
            &mut translations,
            missing_keys,
            &config.default_untranslated_value,
        );
        if added > 0 {
            save_language_translations(root, config, language, &translations)?;
        }
        total_missing += added;
        missing_by_language.insert(language.clone(), added);
    }
    Ok(SyncReport {
        total_missing,
        missing_by_language,
    })
}

fn collect_missing_keys(translations: &Translations, primary_keys: &[String]) -> Vec<String> {
    let mut missing = Vec::new();
    for key in primary_keys {
        if !translations.contains_key(key) {
            missing.push(key.clone());
        }
    }
    missing
}

fn add_missing_keys(
    translations: &mut Translations,
    missing_keys: &[String],
    default_value: &str,
) -> usize {
    let mut added = 0usize;
    for key in missing_keys {
        if !translations.contains_key(key) {
            translations.insert(key.clone(), default_value.to_string());
            added += 1;
        }
    }
    added
}

pub fn maybe_prompt_sync(root: &Path, config: &TransConfig, err: &TransError) -> Result<bool> {
    let TransError::VerificationFailed(_) = err else {
        return Ok(false);
    };
    let plan = collect_missing_ids(root, config)?;
    if plan.total_missing() == 0 {
        return Ok(false);
    }
    let confirmed = dialoguer::Confirm::new()
        .with_prompt("ID inconsistencies found. Do you want to sync IDs?")
        .default(true)
        .interact()?;
    if !confirmed {
        return Ok(false);
    }
    let _ = apply_sync_plan(root, config, &plan)?;
    println!("Synced missing IDs. Please re-run the command.");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ExportFormat, TransConfig};
    use crate::translations::{
        Translations, load_language_translations, save_language_translations,
    };
    use tempfile::tempdir;

    fn config() -> TransConfig {
        TransConfig {
            mode: crate::config::ConfigMode::ReactIntl,
            language_files_path: "messages".into(),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            newline_at_end_of_file: false,
            default_export_format: ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            run_update_check: false,
            ai: None,
        }
    }

    #[test]
    fn collect_missing_ids_reports_missing() {
        let dir = tempdir().expect("tempdir");
        let config = config();
        let mut en = Translations::new();
        en.insert("app.title".to_string(), "Title".to_string());
        en.insert("app.subtitle".to_string(), "Subtitle".to_string());
        save_language_translations(dir.path(), &config, "en", &en).expect("save en");
        let mut nb = Translations::new();
        nb.insert("app.title".to_string(), "Tittel".to_string());
        save_language_translations(dir.path(), &config, "nb", &nb).expect("save nb");

        let plan = collect_missing_ids(dir.path(), &config).expect("plan");
        assert_eq!(plan.total_missing(), 1);
        assert_eq!(
            plan.missing_by_language["nb"],
            vec!["app.subtitle".to_string()]
        );
    }

    #[test]
    fn apply_sync_plan_adds_missing_ids() {
        let dir = tempdir().expect("tempdir");
        let config = config();
        let mut en = Translations::new();
        en.insert("app.title".to_string(), "Title".to_string());
        en.insert("app.subtitle".to_string(), "Subtitle".to_string());
        save_language_translations(dir.path(), &config, "en", &en).expect("save en");
        let mut nb = Translations::new();
        nb.insert("app.title".to_string(), "Tittel".to_string());
        save_language_translations(dir.path(), &config, "nb", &nb).expect("save nb");

        let plan = collect_missing_ids(dir.path(), &config).expect("plan");
        let report = apply_sync_plan(dir.path(), &config, &plan).expect("apply");
        assert_eq!(report.total_missing, 1);
        let nb_after = load_language_translations(dir.path(), &config, "nb").expect("load nb");
        assert_eq!(nb_after.get("app.subtitle"), Some(&"".to_string()));
    }
}
