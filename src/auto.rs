use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use dialoguer::Confirm;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{Duration, sleep};

use crate::ai::{AiSettings, resolve_ai_settings, suggest_custom};
use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::export::load_all_languages;
use crate::format_validation::validate_message_formats;
use crate::translations::{Translations, save_language_translations};
use crate::verify::{collect_verification_issues, verify_language_files};
use crate::verify_ai::verify_with_ai;

const MAX_RETRIES: usize = 4;
const RETRY_BASE_DELAY_MS: u64 = 500;

#[derive(Debug, Clone)]
struct TranslationTask {
    source_language: String,
    language: String,
    id: String,
    source_text: String,
    references: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct TranslationResult {
    language: String,
    id: String,
    value: String,
}

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

    let tasks = build_translation_tasks(
        config,
        &translations_by_language,
        &missing_by_language,
        &selected_languages,
    );
    if tasks.is_empty() {
        println!("No missing translations.");
        return Ok(());
    }

    let mut counts_by_language: BTreeMap<String, usize> = BTreeMap::new();
    for task in &tasks {
        *counts_by_language.entry(task.language.clone()).or_insert(0) += 1;
    }

    let progress = MultiProgress::new();
    let style = ProgressStyle::with_template("{prefix} {bar:40.cyan/blue} {pos}/{len} {msg}")
        .map_err(|err| TransError::InvalidInput(format!("progress style error: {err}")))?;
    let mut bars: BTreeMap<String, ProgressBar> = BTreeMap::new();
    for language in &selected_languages {
        let count = counts_by_language.get(language).cloned().unwrap_or(0);
        if count == 0 {
            continue;
        }
        let bar = progress.add(ProgressBar::new(count as u64));
        bar.set_style(style.clone());
        bar.set_prefix(language.clone());
        bars.insert(language.clone(), bar);
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|err| TransError::InvalidInput(format!("AI runtime error: {err}")))?;
    let results = runtime.block_on(run_translation_tasks(&settings, tasks, &bars))?;

    for result in results {
        if let Some(translations) = translations_by_language.get_mut(&result.language) {
            translations.insert(result.id, result.value);
        }
    }

    for bar in bars.values() {
        bar.finish_with_message("done");
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

fn build_translation_tasks(
    config: &TransConfig,
    translations_by_language: &BTreeMap<String, Translations>,
    missing_by_language: &BTreeMap<String, Vec<String>>,
    target_languages: &[String],
) -> Vec<TranslationTask> {
    let mut tasks = Vec::new();
    let primary = match translations_by_language.get(&config.primary_language) {
        Some(primary) => primary,
        None => return tasks,
    };

    for language in target_languages {
        let Some(ids) = missing_by_language.get(language) else {
            continue;
        };
        for id in ids {
            let primary_value = primary.get(id).cloned().unwrap_or_default();
            if primary_value.trim().is_empty() {
                continue;
            }
            let references = reference_translations(config, translations_by_language, id, language);
            tasks.push(TranslationTask {
                source_language: config.primary_language.clone(),
                language: language.clone(),
                id: id.clone(),
                source_text: primary_value,
                references,
            });
        }
    }

    tasks
}

async fn run_translation_tasks(
    settings: &AiSettings,
    tasks: Vec<TranslationTask>,
    bars: &BTreeMap<String, ProgressBar>,
) -> Result<Vec<TranslationResult>> {
    let semaphore = Arc::new(Semaphore::new(settings.concurrency.max(1)));
    let mut join_set = JoinSet::new();

    for task in tasks {
        let permit = semaphore.clone().acquire_owned().await.map_err(|_| {
            TransError::InvalidInput("failed to acquire AI concurrency permit".to_string())
        })?;
        let settings = settings.clone();
        let bar = bars.get(&task.language).cloned();
        join_set.spawn(async move {
            let _permit = permit;
            if let Some(bar) = &bar {
                bar.set_message(format!("consulting {}", settings.model));
            }
            let result = suggest_with_retries(&settings, &task).await;
            if let Some(bar) = &bar {
                bar.inc(1);
                bar.set_message("");
            }
            result.map(|value| TranslationResult {
                language: task.language,
                id: task.id,
                value,
            })
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = join_set.join_next().await {
        let output =
            joined.map_err(|err| TransError::InvalidInput(format!("AI task failed: {err}")))?;
        let value = output?;
        results.push(value);
    }

    Ok(results)
}

async fn suggest_with_retries(settings: &AiSettings, task: &TranslationTask) -> Result<String> {
    let (system_prompt, user_prompt) = build_prompts(task);
    let mut attempt = 0usize;
    loop {
        match suggest_custom(settings, &system_prompt, &user_prompt).await {
            Ok(value) => return Ok(value),
            Err(TransError::InvalidInput(message))
                if is_rate_limit_error(&message) && attempt < MAX_RETRIES =>
            {
                let delay_ms =
                    RETRY_BASE_DELAY_MS.saturating_mul(2u64.saturating_pow(attempt as u32));
                sleep(Duration::from_millis(delay_ms)).await;
                attempt += 1;
                continue;
            }
            Err(err) => return Err(err),
        }
    }
}

fn build_prompts(task: &TranslationTask) -> (String, String) {
    let system_prompt = format!(
        "You are a professional translator. Translate from {} to {}. Preserve placeholders like {{name}} and ICU plural/select syntax. Return only the translation text.",
        task.source_language, task.language
    );
    let mut user_prompt = format!("Message ID: {}\nSource: {}", task.id, task.source_text);
    if !task.references.is_empty() {
        user_prompt.push_str("\nOther translations:\n");
        for (language, value) in task.references.iter().take(5) {
            user_prompt.push_str(&format!("- {language}: {value}\n"));
        }
    }
    (system_prompt, user_prompt)
}

fn is_rate_limit_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if lower.contains("quota exceeded") || lower.contains("insufficient_quota") {
        return false;
    }
    lower.contains("429") || lower.contains("too many requests") || lower.contains("rate limit")
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
