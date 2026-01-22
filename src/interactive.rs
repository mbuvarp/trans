use std::path::Path;

use dialoguer::{Input, Select};

use crate::config::TransConfig;
use crate::error::Result;
use crate::message_id::validate_message_id;
use crate::operations::{add_translation, delete_translation, update_translation, TranslationValues};
use crate::translations::load_language_translations;
use crate::verify::verify_language_files;

pub fn init_config_interactive(root: impl AsRef<Path>) -> Result<()> {
    let language_files_path = Input::<String>::new()
        .with_prompt("Location of language files (relative to project root)")
        .default("translations".to_string())
        .interact_text()?;

    let available_languages = prompt_language_list("Available languages (comma-separated)")?;

    let required_languages = loop {
        let required =
            prompt_language_list("Required languages (comma-separated)")?;
        let missing: Vec<&String> = required
            .iter()
            .filter(|lang| !available_languages.contains(lang))
            .collect();
        if missing.is_empty() {
            break required;
        }
        eprintln!(
            "Required languages must be in available languages. Missing: {}",
            missing
                .into_iter()
                .map(|lang| lang.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };

    let primary_language = loop {
        let primary = Input::<String>::new()
            .with_prompt("Primary language")
            .default(available_languages[0].clone())
            .interact_text()?;
        if available_languages.contains(&primary) {
            break primary;
        }
        eprintln!("Primary language must be in available languages.");
    };

    let default_untranslated_value = Input::<String>::new()
        .with_prompt("Default value for untranslated strings")
        .default("".to_string())
        .allow_empty(true)
        .interact_text()?;

    let config = TransConfig {
        language_files_path: language_files_path.into(),
        available_languages,
        required_languages,
        primary_language,
        default_untranslated_value,
    };

    config.validate()?;
    config.save_to_root(root)?;

    Ok(())
}

pub fn run_interactive(root: impl AsRef<Path>) -> Result<()> {
    let root = root.as_ref();
    let config = TransConfig::load_from_root(root)?;
    verify_language_files(root, &config)?;

    let message_id = prompt_message_id()?;
    let primary_translations = load_language_translations(root, &config, &config.primary_language)?;
    let existing_primary = primary_translations.get(&message_id).cloned();

    if let Some(existing) = existing_primary {
        println!("Existing translation ({}) = {}", config.primary_language, existing);
        let selection = Select::new()
            .with_prompt("Update or delete?")
            .items(&["Update", "Delete", "Cancel"])
            .default(0)
            .interact()?;
        match selection {
            0 => {
                let values = prompt_required_translations(root, &config, &message_id, true)?;
                update_translation(root, &config, &message_id, &values)
            }
            1 => delete_translation(root, &config, &message_id),
            _ => Ok(()),
        }
    } else {
        let values = prompt_required_translations(root, &config, &message_id, false)?;
        add_translation(root, &config, &message_id, &values)
    }
}

fn prompt_language_list(prompt: &str) -> Result<Vec<String>> {
    loop {
        let input = Input::<String>::new().with_prompt(prompt).interact_text()?;
        let values: Vec<String> = input
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from)
            .collect();
        if values.is_empty() {
            eprintln!("Please enter at least one language.");
            continue;
        }
        return Ok(values);
    }
}

fn prompt_message_id() -> Result<String> {
    loop {
        let input = Input::<String>::new()
            .with_prompt("Message ID (must include a namespace, e.g. app.header)")
            .interact_text()?;
        if let Err(err) = validate_message_id(&input) {
            eprintln!("{err}");
            continue;
        }
        return Ok(input);
    }
}

fn prompt_required_translations(
    root: &Path,
    config: &TransConfig,
    message_id: &str,
    use_existing_defaults: bool,
) -> Result<TranslationValues> {
    let mut values = TranslationValues::new();

    for language in &config.required_languages {
        let default_value = if use_existing_defaults {
            let translations = load_language_translations(root, config, language)?;
            translations.get(message_id).cloned()
        } else {
            None
        };

        let mut input = Input::<String>::new();
        input = input.with_prompt(format!("Translation for {language}"));
        if let Some(default) = default_value {
            input = input.default(default);
        }
        let translation = input.allow_empty(true).interact_text()?;
        values.insert(language.clone(), translation);
    }

    Ok(values)
}
