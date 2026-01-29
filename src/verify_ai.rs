use std::collections::HashSet;
use std::path::Path;

use console::style;
use dialoguer::Confirm;

use crate::ai::{AiSettings, resolve_ai_settings, suggest_custom, suggest_translation};
use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::export::load_all_languages;
use crate::format_validation::{
    FormatValidationIssue, collect_format_validation_issues, validate_message_formats,
};
use crate::translations::save_language_translations;
use crate::verify::{KeyMismatch, key_mismatches, verify_language_files};

pub fn verify_with_ai(root: &Path, config: &TransConfig) -> Result<()> {
    let settings = resolve_ai_settings(root, config)?.ok_or_else(|| {
        TransError::InvalidInput(
            "AI is not configured. Run `trans config ai` to set it up.".to_string(),
        )
    })?;

    let mut translations_by_language = load_all_languages(root, config)?;
    let mismatches = key_mismatches(&translations_by_language, &config.primary_language);
    let format_issues = collect_format_validation_issues(config, &translations_by_language)?;

    if mismatches.is_empty() && format_issues.is_empty() {
        println!("OK");
        return Ok(());
    }

    let mut languages_changed = HashSet::new();

    for mismatch in mismatches {
        handle_key_mismatch(
            &settings,
            config,
            &mut translations_by_language,
            &mut languages_changed,
            mismatch,
        )?;
    }

    apply_format_fixes_with_ai(
        root,
        config,
        &mut translations_by_language,
        &mut languages_changed,
    )?;

    for language in &languages_changed {
        if let Some(translations) = translations_by_language.get(language) {
            save_language_translations(root, config, language, translations)?;
        }
    }

    if languages_changed.is_empty() {
        println!("No changes applied.");
    }

    verify_language_files(root, config)?;
    validate_message_formats(config, &translations_by_language)?;
    println!("OK");
    Ok(())
}

pub fn apply_format_fixes_with_ai(
    root: &Path,
    config: &TransConfig,
    translations_by_language: &mut std::collections::BTreeMap<
        String,
        crate::translations::Translations,
    >,
    languages_changed: &mut HashSet<String>,
) -> Result<()> {
    let settings = resolve_ai_settings(root, config)?.ok_or_else(|| {
        TransError::InvalidInput(
            "AI is not configured. Run `trans config ai` to set it up.".to_string(),
        )
    })?;
    let format_issues = collect_format_validation_issues(config, translations_by_language)?;
    for issue in format_issues {
        handle_format_issue(
            &settings,
            config,
            translations_by_language,
            languages_changed,
            issue,
        )?;
    }
    Ok(())
}

fn handle_key_mismatch(
    settings: &AiSettings,
    config: &TransConfig,
    translations_by_language: &mut std::collections::BTreeMap<
        String,
        crate::translations::Translations,
    >,
    languages_changed: &mut HashSet<String>,
    mismatch: KeyMismatch,
) -> Result<()> {
    if !mismatch.missing.is_empty() {
        for id in mismatch.missing {
            let primary_value = translations_by_language
                .get(&config.primary_language)
                .and_then(|translations| translations.get(&id))
                .cloned()
                .unwrap_or_default();
            if primary_value.is_empty() {
                continue;
            }
            print_issue_header(&format!("Missing id in '{}': {}", mismatch.language, id));
            let suggestion = run_translation_suggestion(
                settings,
                &config.primary_language,
                &mismatch.language,
                &id,
                &primary_value,
            )?;
            print_suggestion("<missing>", &suggestion);
            if confirm_apply()? {
                if let Some(translations) = translations_by_language.get_mut(&mismatch.language) {
                    translations.insert(id.clone(), suggestion);
                    languages_changed.insert(mismatch.language.clone());
                }
            }
        }
    }

    if !mismatch.extra.is_empty() {
        for id in mismatch.extra {
            let value = translations_by_language
                .get(&mismatch.language)
                .and_then(|translations| translations.get(&id))
                .cloned()
                .unwrap_or_default();
            print_issue_header(&format!("Extra id in '{}': {}", mismatch.language, id));
            let suggestion = run_extra_id_suggestion(settings, &mismatch.language, &id, &value)?;
            print_suggestion(&value, &suggestion);
            if confirm_apply()? {
                if suggestion.trim().to_uppercase().starts_with("DELETE") {
                    if let Some(translations) = translations_by_language.get_mut(&mismatch.language)
                    {
                        translations.remove(&id);
                        languages_changed.insert(mismatch.language.clone());
                    }
                }
            }
        }
    }

    Ok(())
}

fn handle_format_issue(
    settings: &AiSettings,
    config: &TransConfig,
    translations_by_language: &mut std::collections::BTreeMap<
        String,
        crate::translations::Translations,
    >,
    languages_changed: &mut HashSet<String>,
    issue: FormatValidationIssue,
) -> Result<()> {
    print_issue_header(&format!(
        "Format issue in '{}': {} ({})",
        issue.language, issue.id, issue.message
    ));
    let suggestion = run_format_suggestion(
        settings,
        &issue.id,
        &issue.primary_value,
        &issue.value,
        &issue.message,
    )?;
    print_suggestion(&issue.value, &suggestion);
    if confirm_apply()? {
        if let Some(translations) = translations_by_language.get_mut(&issue.language) {
            translations.insert(issue.id.clone(), suggestion);
            languages_changed.insert(issue.language.clone());
        }
    }
    if issue.language == config.primary_language {
        println!(
            "{}",
            style("Note: Updating the primary language may require reviewing other languages.")
                .dim()
        );
    }
    Ok(())
}

fn run_translation_suggestion(
    settings: &AiSettings,
    source_lang: &str,
    target_lang: &str,
    message_id: &str,
    source_text: &str,
) -> Result<String> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|err| TransError::InvalidInput(format!("AI runtime error: {err}")))?;
    runtime.block_on(suggest_translation(
        settings,
        source_lang,
        target_lang,
        message_id,
        source_text,
    ))
}

fn run_format_suggestion(
    settings: &AiSettings,
    message_id: &str,
    primary_value: &str,
    current_value: &str,
    error: &str,
) -> Result<String> {
    let system_prompt = "You are a professional translator. Fix the translation so it matches ICU MessageFormat placeholders and syntax from the primary language. Return only the corrected translation text.";
    let user_prompt = format!(
        "Message ID: {message_id}\nPrimary: {primary_value}\nCurrent: {current_value}\nIssue: {error}"
    );
    run_custom_suggestion(settings, system_prompt, &user_prompt)
}

fn run_extra_id_suggestion(
    settings: &AiSettings,
    language: &str,
    message_id: &str,
    value: &str,
) -> Result<String> {
    let system_prompt = "You are maintaining translation files. Decide if the extra key should be removed. Reply with DELETE to remove it, or KEEP to keep it. Return only DELETE or KEEP.";
    let user_prompt = format!(
        "Language: {language}\nMessage ID: {message_id}\nValue: {value}\nThe primary language does not contain this key."
    );
    run_custom_suggestion(settings, system_prompt, &user_prompt)
}

fn run_custom_suggestion(
    settings: &AiSettings,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|err| TransError::InvalidInput(format!("AI runtime error: {err}")))?;
    runtime.block_on(suggest_custom(settings, system_prompt, user_prompt))
}

fn print_issue_header(text: &str) {
    println!("{}", style(text).bold());
}

fn print_suggestion(old_value: &str, suggestion: &str) {
    println!("Old: {old_value}");
    println!("Suggested: {suggestion}");
}

fn confirm_apply() -> Result<bool> {
    if let Ok(value) = std::env::var("TRANS_AI_ASSUME_YES") {
        let value = value.trim().to_ascii_lowercase();
        if value == "1" || value == "true" || value == "yes" {
            return Ok(true);
        }
    }
    Ok(Confirm::new()
        .with_prompt("Apply suggestion?")
        .default(true)
        .interact()?)
}
