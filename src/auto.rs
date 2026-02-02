use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use dialoguer::Confirm;

use crate::ai::{AiSettings, resolve_ai_settings, suggest_custom};
use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::export::load_all_languages;
use crate::format_validation::validate_message_formats;
use crate::spinner::start_spinner;
use crate::translations::{Translations, save_language_translations};
use crate::verify::{collect_verification_issues, verify_language_files};
use crate::verify_ai::verify_with_ai;

pub fn auto_translate(
    root: impl AsRef<Path>,
    config: &TransConfig,
    lang_filter: Option<Vec<String>>,
) -> Result<()> {
    let root = root.as_ref();

    loop {
        let issues = collect_verification_issues(root, config)?;
        if issues.is_empty() {
            break;
        }
        eprintln!(
            "Verification errors found. Full AI translation cannot be done while there are format errors."
        );
        if !confirm_prompt("Fix errors with AI?")? {
            return Err(TransError::InvalidInput(
                "auto translation aborted due to verification errors".to_string(),
            ));
        }
        verify_with_ai(root, config)?;
    }

    let settings = resolve_ai_settings(root, config)?.ok_or_else(|| {
        TransError::InvalidInput(
            "AI is not configured. Run `trans config ai` to set it up.".to_string(),
        )
    })?;

    let mut translations_by_language = load_all_languages(root, config)?;
    let selected_languages = resolve_target_languages(config, lang_filter)?;

    if selected_languages.is_empty() {
        println!("No target languages to translate.");
        return Ok(());
    }

    let missing_by_language =
        collect_missing_translations(&translations_by_language, config, &selected_languages);

    let total_missing: usize = missing_by_language.values().map(|ids| ids.len()).sum();
    if total_missing == 0 {
        println!("No missing translations.");
        return Ok(());
    }

    println!("Missing translations:");
    for language in &selected_languages {
        let count = missing_by_language
            .get(language)
            .map(|ids| ids.len())
            .unwrap_or(0);
        println!("{language}: {count}");
    }

    if !confirm_prompt("Proceed with AI translation?")? {
        return Ok(());
    }

    for language in &selected_languages {
        let Some(ids) = missing_by_language.get(language) else {
            continue;
        };
        for id in ids {
            let primary_value = translations_by_language
                .get(&config.primary_language)
                .and_then(|translations| translations.get(id))
                .cloned()
                .unwrap_or_default();
            if primary_value.trim().is_empty() {
                continue;
            }
            let references =
                reference_translations(config, &translations_by_language, id, language);
            let suggestion = run_translation_suggestion(
                &settings,
                &config.primary_language,
                language,
                id,
                &primary_value,
                &references,
            )?;
            if let Some(translations) = translations_by_language.get_mut(language) {
                translations.insert(id.clone(), suggestion);
            }
        }
    }

    for language in &selected_languages {
        if let Some(translations) = translations_by_language.get(language) {
            save_language_translations(root, config, language, translations)?;
        }
    }

    verify_language_files(root, config)?;
    validate_message_formats(config, &translations_by_language)?;

    Ok(())
}

fn resolve_target_languages(
    config: &TransConfig,
    lang_filter: Option<Vec<String>>,
) -> Result<Vec<String>> {
    let mut selected = match lang_filter {
        Some(list) => {
            let mut values = Vec::new();
            let mut seen = BTreeSet::new();
            for lang in list {
                if !config.available_languages.contains(&lang) {
                    return Err(TransError::InvalidInput(format!(
                        "language '{lang}' is not in available_languages"
                    )));
                }
                if seen.insert(lang.clone()) {
                    values.push(lang);
                }
            }
            values
        }
        None => config.available_languages.clone(),
    };

    if selected.contains(&config.primary_language) {
        selected.retain(|lang| lang != &config.primary_language);
        eprintln!(
            "Warning: primary language '{}' is used as the source and will be skipped.",
            config.primary_language
        );
    }

    Ok(selected)
}

fn collect_missing_translations(
    translations_by_language: &BTreeMap<String, Translations>,
    config: &TransConfig,
    target_languages: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mut missing = BTreeMap::new();
    let Some(primary) = translations_by_language.get(&config.primary_language) else {
        return missing;
    };

    for language in target_languages {
        let translations = match translations_by_language.get(language) {
            Some(translations) => translations,
            None => continue,
        };
        let mut ids = Vec::new();
        for id in primary.keys() {
            let value = translations.get(id).cloned().unwrap_or_default();
            if value == config.default_untranslated_value {
                ids.push(id.clone());
            }
        }
        missing.insert(language.clone(), ids);
    }
    missing
}

fn run_translation_suggestion(
    settings: &AiSettings,
    source_lang: &str,
    target_lang: &str,
    message_id: &str,
    source_text: &str,
    references: &[(String, String)],
) -> Result<String> {
    let system_prompt = format!(
        "You are a professional translator. Translate from {source_lang} to {target_lang}. Preserve placeholders like {{name}} and ICU plural/select syntax. Return only the translation text."
    );
    let mut user_prompt = format!("Message ID: {message_id}\nSource: {source_text}");
    if !references.is_empty() {
        user_prompt.push_str("\nOther translations:\n");
        for (language, value) in references.iter().take(5) {
            user_prompt.push_str(&format!("- {language}: {value}\n"));
        }
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|err| TransError::InvalidInput(format!("AI runtime error: {err}")))?;
    let spinner = start_spinner(format!("Consulting {}", settings.model));
    let result = runtime.block_on(suggest_custom(settings, &system_prompt, &user_prompt));
    drop(spinner);
    result
}

fn reference_translations(
    config: &TransConfig,
    translations_by_language: &BTreeMap<String, Translations>,
    message_id: &str,
    exclude_language: &str,
) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    for language in &config.available_languages {
        if language == exclude_language {
            continue;
        }
        let value = translations_by_language
            .get(language)
            .and_then(|translations| translations.get(message_id))
            .cloned()
            .unwrap_or_default();
        if value.is_empty() || value == config.default_untranslated_value {
            continue;
        }
        refs.push((language.clone(), value));
    }
    refs
}

fn confirm_prompt(prompt: &str) -> Result<bool> {
    if let Ok(value) = std::env::var("TRANS_AI_ASSUME_YES") {
        let value = value.trim().to_ascii_lowercase();
        if value == "1" || value == "true" || value == "yes" {
            return Ok(true);
        }
    }
    Ok(Confirm::new()
        .with_prompt(prompt)
        .default(true)
        .interact()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn base_config() -> TransConfig {
        TransConfig {
            language_files_path: PathBuf::from("messages"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            default_export_format: crate::config::ExportFormat::Excel,
            excel_password: "unlock".to_string(),
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
    fn collect_missing_translations_finds_defaults() {
        let config = base_config();
        let mut translations_by_language = BTreeMap::new();
        translations_by_language.insert(
            "en".to_string(),
            translations(&[("app.title", "Title"), ("app.body", "Body")]),
        );
        translations_by_language.insert(
            "nb".to_string(),
            translations(&[("app.title", ""), ("app.body", "Body")]),
        );

        let missing =
            collect_missing_translations(&translations_by_language, &config, &["nb".to_string()]);
        let nb_missing = missing.get("nb").expect("nb missing");
        assert_eq!(nb_missing, &vec!["app.title".to_string()]);
    }
}
