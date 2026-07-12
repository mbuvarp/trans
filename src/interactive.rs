use std::fs;
use std::path::{Path, PathBuf};

use console::style;
use dialoguer::{Completion, Confirm, FuzzySelect, Input, Select};

use crate::ai::{
    AiSettings, SuggestTranslationContext, resolve_ai_settings, suggest_translation_with_context,
};
use crate::config::{AiConfig, ConfigField, ConfigMode, ExportFormat, TransConfig};
use crate::error::Result;
use crate::language::is_valid_language_code;
use crate::message_id::validate_message_id;
use crate::operations::{
    TranslationValues, add_translation, delete_translation, replace_default_untranslated_value,
    update_translation,
};
use crate::spinner::start_spinner;
use crate::translations::{
    coerce_non_string_leaf_values, collect_non_string_leaf_values, load_language_translations,
    migrate_language_files,
};
use crate::verify::verify_language_files;

struct PathCompletion {
    root: PathBuf,
}

impl PathCompletion {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    fn candidate_from_input(&self, input: &str) -> Option<(PathBuf, String, String)> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }

        if input.ends_with('/') {
            let dir = self.root.join(input);
            return Some((dir, String::new(), input.to_string()));
        }

        let path = Path::new(input);
        let prefix = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        let dir = path.parent().unwrap_or_else(|| Path::new(""));
        let base = if dir.as_os_str().is_empty() {
            String::new()
        } else {
            format!("{}/", dir.display())
        };

        Some((self.root.join(dir), prefix, base))
    }
}

impl Completion for PathCompletion {
    fn get(&self, input: &str) -> Option<String> {
        let (dir, prefix, base) = self.candidate_from_input(input)?;
        let entries = fs::read_dir(dir).ok()?;
        let mut matches: Vec<String> = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                if !entry.file_type().ok()?.is_dir() {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        matches.sort();
        matches.first().map(|name| format!("{base}{name}"))
    }
}

pub fn init_config_interactive(
    root: impl AsRef<Path>,
    format: crate::config::ConfigFormat,
) -> Result<()> {
    let root = root.as_ref();
    let (json_path, yaml_path) = TransConfig::config_paths(root);
    if json_path.exists() || yaml_path.exists() {
        print_label("Config file already exists, do you wish to overwrite?");
        println!(
            "Found {}{}",
            json_path.display(),
            if yaml_path.exists() {
                format!(", {}", yaml_path.display())
            } else {
                "".to_string()
            }
        );
        let overwrite = Confirm::new().with_prompt(">").default(false).interact()?;
        print_spacer();
        if !overwrite {
            println!("Aborted.");
            return Ok(());
        }
    }
    let mode = prompt_mode(ConfigMode::ReactIntl)?;
    let language_files_path = prompt_language_files_path(root)?;

    let default_languages = discover_languages(root, &language_files_path)?;
    print_label("Available languages (comma-separated)");
    let available_languages = prompt_language_list(if default_languages.is_empty() {
        None
    } else {
        Some(&default_languages)
    })?;
    print_spacer();

    let required_languages = loop {
        print_label("Required languages (comma-separated)");
        print_description(
            "These are the languages required to be input each time a new translation is added.",
        );
        let required = prompt_language_list(None)?;
        print_spacer();
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
        print_label("Primary language");
        let primary = Input::<String>::new()
            .with_prompt(">")
            .default(available_languages[0].clone())
            .interact_text()?;
        print_spacer();
        if available_languages.contains(&primary) {
            break primary;
        }
        eprintln!("Primary language must be in available languages.");
    };

    print_label("Default value for untranslated strings");
    let default_untranslated_value = Input::<String>::new()
        .with_prompt(">")
        .default("".to_string())
        .allow_empty(true)
        .interact_text()?;
    print_spacer();

    let default_export_format = prompt_default_export_format(ExportFormat::Excel)?;

    print_label("Run update check after successful commands");
    let run_update_check = Confirm::new().with_prompt(">").default(false).interact()?;
    print_spacer();

    print_label("Do you want to set up AI?");
    let setup_ai = Confirm::new().with_prompt(">").default(true).interact()?;
    print_spacer();

    let ai = if setup_ai {
        Some(prompt_ai_config(&AiConfig::default())?)
    } else {
        None
    };

    let config = TransConfig {
        mode,
        language_files_path: language_files_path.into(),
        available_languages,
        required_languages,
        primary_language,
        default_untranslated_value,
        default_export_format,
        excel_password: "unlock".to_string(),
        run_update_check,
        ai,
    };

    config.validate()?;
    config.save_to_root_format(root, format, true)?;

    Ok(())
}

pub fn configure_root_interactive(root: impl AsRef<Path>) -> Result<()> {
    let root = root.as_ref();
    let mut config = TransConfig::load_from_root(root)?;
    let original_config = config.clone();

    let mode = prompt_mode(config.mode)?;
    let language_files_path = prompt_language_files_path_with_default(
        root,
        &config.language_files_path.to_string_lossy(),
    )?;

    let available_languages = {
        print_label("Available languages (comma-separated)");
        let defaults = if config.available_languages.is_empty() {
            None
        } else {
            Some(config.available_languages.as_slice())
        };
        let values = prompt_language_list(defaults)?;
        print_spacer();
        values
    };

    let required_languages = loop {
        print_label("Required languages (comma-separated)");
        print_description(
            "These are the languages required to be input each time a new translation is added.",
        );
        let defaults = if config.required_languages.is_empty() {
            None
        } else {
            Some(config.required_languages.as_slice())
        };
        let required = prompt_language_list(defaults)?;
        print_spacer();
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
        print_label("Primary language");
        let primary = Input::<String>::new()
            .with_prompt(">")
            .default(config.primary_language.clone())
            .interact_text()?;
        print_spacer();
        if available_languages.contains(&primary) {
            break primary;
        }
        eprintln!("Primary language must be in available languages.");
    };

    print_label("Default value for untranslated strings");
    let default_untranslated_value = Input::<String>::new()
        .with_prompt(">")
        .default(config.default_untranslated_value.clone())
        .allow_empty(true)
        .interact_text()?;
    print_spacer();

    let default_export_format = prompt_default_export_format(config.default_export_format)?;

    print_label("Run update check after successful commands");
    let run_update_check = Confirm::new()
        .with_prompt(">")
        .default(config.run_update_check)
        .interact()?;
    print_spacer();

    config.mode = mode;
    config.language_files_path = language_files_path.into();
    config.available_languages = available_languages;
    config.required_languages = required_languages;
    config.primary_language = primary_language;
    config.default_untranslated_value = default_untranslated_value;
    config.default_export_format = default_export_format;
    config.run_update_check = run_update_check;

    config.validate()?;
    maybe_migrate_mode(root, &original_config, config.mode)?;
    config.save_to_root(root)?;
    Ok(())
}

pub fn configure_ai_interactive(root: impl AsRef<Path>) -> Result<()> {
    let root = root.as_ref();
    let mut config = TransConfig::load_from_root(root)?;
    let defaults = config.ai.clone().unwrap_or_default();
    config.ai = Some(prompt_ai_config(&defaults)?);

    config.save_to_root(root)?;
    Ok(())
}

fn prompt_ai_config(defaults: &AiConfig) -> Result<AiConfig> {
    print_label("AI enabled");
    let enabled = Confirm::new()
        .with_prompt(">")
        .default(defaults.enabled)
        .interact()?;
    print_spacer();

    print_label("AI model");
    let model = Input::<String>::new()
        .with_prompt(">")
        .default(defaults.model.clone())
        .interact_text()?;
    print_spacer();

    print_label("API key environment variable");
    let api_key_env = Input::<String>::new()
        .with_prompt(">")
        .default(defaults.api_key_env.clone())
        .interact_text()?;
    print_spacer();

    print_label("Max output tokens");
    let max_output_tokens = Input::<u32>::new()
        .with_prompt(">")
        .default(defaults.max_output_tokens)
        .interact_text()?;
    print_spacer();

    print_label("AI concurrency (max simultaneous AI requests)");
    let concurrency = Input::<usize>::new()
        .with_prompt(">")
        .default(defaults.concurrency)
        .interact_text()?;
    print_spacer();

    Ok(AiConfig {
        enabled,
        model,
        api_key_env,
        max_output_tokens,
        concurrency,
    })
}

pub fn run_interactive(
    root: impl AsRef<Path>,
    message_id: Option<String>,
    prompt_all: bool,
) -> Result<()> {
    let root = root.as_ref();
    let config = TransConfig::load_from_root(root)?;
    ensure_next_intl_strings(root, &config)?;
    if let Err(err) = verify_language_files(root, &config) {
        if crate::sync::maybe_prompt_sync(root, &config, &err)? {
            verify_language_files(root, &config)?;
        } else {
            return Err(err);
        }
    }
    let ai_settings = resolve_ai_settings(root, &config)?;

    let message_id = match message_id {
        Some(candidate) => {
            validate_message_id(&candidate)?;
            candidate
        }
        None => prompt_message_id()?,
    };
    let primary_translations = load_language_translations(root, &config, &config.primary_language)?;
    let existing_primary = primary_translations.get(&message_id).cloned();

    if let Some(existing) = existing_primary {
        print_label(&format!(
            "Existing translation ({})",
            config.primary_language
        ));
        println!("{existing}");
        let selection = Select::new()
            .with_prompt(">")
            .items(["Update", "Delete", "Cancel"])
            .default(0)
            .interact()?;
        print_spacer();
        match selection {
            0 => {
                let values = if prompt_all {
                    prompt_translations_for_languages(
                        root,
                        &config,
                        &message_id,
                        &ai_settings,
                        &languages_for_all(&config),
                        true,
                    )?
                } else {
                    prompt_translations_for_languages(
                        root,
                        &config,
                        &message_id,
                        &ai_settings,
                        &config.required_languages,
                        true,
                    )?
                };
                update_translation(root, &config, &message_id, &values)
            }
            1 => delete_translation(root, &config, &message_id),
            _ => Ok(()),
        }
    } else {
        let values = if prompt_all {
            prompt_translations_for_languages(
                root,
                &config,
                &message_id,
                &ai_settings,
                &languages_for_all(&config),
                false,
            )?
        } else {
            prompt_translations_for_languages(
                root,
                &config,
                &message_id,
                &ai_settings,
                &config.required_languages,
                false,
            )?
        };
        add_translation(root, &config, &message_id, &values)
    }
}

pub fn configure_edit_interactive(
    root: impl AsRef<Path>,
    field: Option<ConfigField>,
) -> Result<()> {
    let root = root.as_ref();
    let mut config = TransConfig::load_from_root(root)?;
    let original_config = config.clone();

    match field {
        None => {
            let mode = prompt_mode(config.mode)?;
            let language_files_path = prompt_language_files_path_with_default(
                root,
                &config.language_files_path.to_string_lossy(),
            )?;

            let available_languages = {
                print_label("Available languages (comma-separated)");
                let defaults = if config.available_languages.is_empty() {
                    None
                } else {
                    Some(config.available_languages.as_slice())
                };
                let values = prompt_language_list(defaults)?;
                print_spacer();
                values
            };

            let required_languages = loop {
                print_label("Required languages (comma-separated)");
                print_description(
                    "These are the languages required to be input each time a new translation is added.",
                );
                let defaults = if config.required_languages.is_empty() {
                    None
                } else {
                    Some(config.required_languages.as_slice())
                };
                let required = prompt_language_list(defaults)?;
                print_spacer();
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
                print_label("Primary language");
                let primary = Input::<String>::new()
                    .with_prompt(">")
                    .default(config.primary_language.clone())
                    .interact_text()?;
                print_spacer();
                if available_languages.contains(&primary) {
                    break primary;
                }
                eprintln!("Primary language must be in available languages.");
            };

            print_label("Default value for untranslated strings");
            let old_default_value = config.default_untranslated_value.clone();
            let default_untranslated_value = Input::<String>::new()
                .with_prompt(">")
                .default(old_default_value.clone())
                .allow_empty(true)
                .interact_text()?;
            print_spacer();

            let replace_default = if default_untranslated_value != old_default_value {
                print_label("Replace existing default values in translations?");
                let confirmed = Confirm::new().with_prompt(">").default(true).interact()?;
                print_spacer();
                confirmed
            } else {
                false
            };

            let default_export_format = prompt_default_export_format(config.default_export_format)?;

            print_label("Run update check after successful commands");
            let run_update_check = Confirm::new()
                .with_prompt(">")
                .default(config.run_update_check)
                .interact()?;
            print_spacer();

            print_label("Do you want to set up AI?");
            let setup_ai = Confirm::new()
                .with_prompt(">")
                .default(config.ai.is_some())
                .interact()?;
            print_spacer();

            let ai = if setup_ai {
                Some(prompt_ai_config(&config.ai.clone().unwrap_or_default())?)
            } else {
                None
            };

            config.mode = mode;
            config.language_files_path = language_files_path.into();
            config.available_languages = available_languages;
            config.required_languages = required_languages;
            config.primary_language = primary_language;
            config.default_untranslated_value = default_untranslated_value;
            config.default_export_format = default_export_format;
            config.run_update_check = run_update_check;
            config.ai = ai;

            config.validate()?;
            maybe_migrate_mode(root, &original_config, config.mode)?;
            if replace_default {
                let replaced = replace_default_untranslated_value(
                    root,
                    &config,
                    &old_default_value,
                    &config.default_untranslated_value,
                )?;
                println!("Replaced {replaced} values.");
            }
            config.save_to_root(root)?;
            return Ok(());
        }
        Some(ConfigField::Mode) => {
            config.mode = prompt_mode(config.mode)?;
        }
        Some(ConfigField::LanguageFilesPath) => {
            let language_files_path = prompt_language_files_path_with_default(
                root,
                &config.language_files_path.to_string_lossy(),
            )?;
            config.language_files_path = language_files_path.into();
        }
        Some(ConfigField::AvailableLanguages) => {
            let available_languages = loop {
                print_label("Available languages (comma-separated)");
                let defaults = if config.available_languages.is_empty() {
                    None
                } else {
                    Some(config.available_languages.as_slice())
                };
                let values = prompt_language_list(defaults)?;
                print_spacer();
                if values.contains(&config.primary_language)
                    && config
                        .required_languages
                        .iter()
                        .all(|lang| values.contains(lang))
                {
                    break values;
                }
                eprintln!(
                    "Available languages must include the primary language and all required languages."
                );
            };
            config.available_languages = available_languages;
        }
        Some(ConfigField::RequiredLanguages) => {
            let required_languages = loop {
                print_label("Required languages (comma-separated)");
                print_description(
                    "These are the languages required to be input each time a new translation is added.",
                );
                let defaults = if config.required_languages.is_empty() {
                    None
                } else {
                    Some(config.required_languages.as_slice())
                };
                let required = prompt_language_list(defaults)?;
                print_spacer();
                let missing: Vec<&String> = required
                    .iter()
                    .filter(|lang| !config.available_languages.contains(lang))
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
            config.required_languages = required_languages;
        }
        Some(ConfigField::PrimaryLanguage) => {
            let primary_language = loop {
                print_label("Primary language");
                let primary = Input::<String>::new()
                    .with_prompt(">")
                    .default(config.primary_language.clone())
                    .interact_text()?;
                print_spacer();
                if config.available_languages.contains(&primary) {
                    break primary;
                }
                eprintln!("Primary language must be in available languages.");
            };
            config.primary_language = primary_language;
        }
        Some(ConfigField::DefaultUntranslatedValue) => {
            print_label("Default value for untranslated strings");
            let old_default_value = config.default_untranslated_value.clone();
            let default_untranslated_value = Input::<String>::new()
                .with_prompt(">")
                .default(old_default_value.clone())
                .allow_empty(true)
                .interact_text()?;
            print_spacer();
            let replace_default = if default_untranslated_value != old_default_value {
                print_label("Replace existing default values in translations?");
                let confirmed = Confirm::new().with_prompt(">").default(true).interact()?;
                print_spacer();
                confirmed
            } else {
                false
            };
            config.default_untranslated_value = default_untranslated_value;
            if replace_default {
                let replaced = replace_default_untranslated_value(
                    root,
                    &config,
                    &old_default_value,
                    &config.default_untranslated_value,
                )?;
                println!("Replaced {replaced} values.");
            }
        }
        Some(ConfigField::DefaultExportFormat) => {
            config.default_export_format =
                prompt_default_export_format(config.default_export_format)?;
        }
        Some(ConfigField::ExcelPassword) => {
            print_label("Excel password");
            let excel_password = Input::<String>::new()
                .with_prompt(">")
                .default(config.excel_password.clone())
                .allow_empty(true)
                .interact_text()?;
            print_spacer();
            config.excel_password = excel_password;
        }
        Some(ConfigField::RunUpdateCheck) => {
            print_label("Run update check after successful commands");
            let run_update_check = Confirm::new()
                .with_prompt(">")
                .default(config.run_update_check)
                .interact()?;
            print_spacer();
            config.run_update_check = run_update_check;
        }
        Some(ConfigField::AiEnabled) => {
            let defaults = config.ai.clone().unwrap_or_default();
            print_label("AI enabled");
            let enabled = Confirm::new()
                .with_prompt(">")
                .default(defaults.enabled)
                .interact()?;
            print_spacer();
            config.ai = Some(AiConfig {
                enabled,
                ..defaults
            });
        }
        Some(ConfigField::AiModel) => {
            let defaults = config.ai.clone().unwrap_or_default();
            print_label("AI model");
            let model = Input::<String>::new()
                .with_prompt(">")
                .default(defaults.model.clone())
                .interact_text()?;
            print_spacer();
            config.ai = Some(AiConfig { model, ..defaults });
        }
        Some(ConfigField::AiApiKeyEnv) => {
            let defaults = config.ai.clone().unwrap_or_default();
            print_label("API key environment variable");
            let api_key_env = Input::<String>::new()
                .with_prompt(">")
                .default(defaults.api_key_env.clone())
                .interact_text()?;
            print_spacer();
            config.ai = Some(AiConfig {
                api_key_env,
                ..defaults
            });
        }
        Some(ConfigField::AiMaxOutputTokens) => {
            let defaults = config.ai.clone().unwrap_or_default();
            print_label("Max output tokens");
            let max_output_tokens = Input::<u32>::new()
                .with_prompt(">")
                .default(defaults.max_output_tokens)
                .interact_text()?;
            print_spacer();
            config.ai = Some(AiConfig {
                max_output_tokens,
                ..defaults
            });
        }
        Some(ConfigField::AiConcurrency) => {
            let defaults = config.ai.clone().unwrap_or_default();
            print_label("AI concurrency (max simultaneous AI requests)");
            let concurrency = Input::<usize>::new()
                .with_prompt(">")
                .default(defaults.concurrency)
                .interact_text()?;
            print_spacer();
            config.ai = Some(AiConfig {
                concurrency,
                ..defaults
            });
        }
    }

    config.validate()?;
    maybe_migrate_mode(root, &original_config, config.mode)?;
    config.save_to_root(root)?;
    Ok(())
}

fn prompt_language_files_path(root: &Path) -> Result<String> {
    prompt_language_files_path_with_default(root, "translations")
}

fn prompt_language_files_path_with_default(root: &Path, default_value: &str) -> Result<String> {
    let path_completion = PathCompletion::new(root);
    let choices = build_directory_choices(root)?;
    print_label("Location of language files (relative to project root)");

    if choices.len() < 7 {
        let mut labels: Vec<String> = Vec::with_capacity(choices.len());
        let mut values: Vec<Option<String>> = Vec::with_capacity(choices.len());
        for choice in &choices {
            labels.push(choice.label.clone());
            values.push(choice.value.clone());
        }

        let default_idx = choices
            .iter()
            .position(|choice| choice.value.as_deref() == Some(default_value))
            .unwrap_or(0);

        let selection = FuzzySelect::new()
            .with_prompt(">")
            .items(&labels)
            .default(default_idx)
            .interact()?;
        print_spacer();

        if let Some(value) = &values[selection] {
            warn_invalid_language_files(root, value)?;
            return Ok(value.clone());
        }
    }

    loop {
        let input = Input::<String>::new()
            .with_prompt(">")
            .default(default_value.to_string())
            .completion_with(&path_completion)
            .interact_text()?;
        let candidate = root.join(&input);
        if candidate.exists() {
            if candidate.is_dir() {
                warn_invalid_language_files(root, &input)?;
                return Ok(input);
            }
            eprintln!(
                "Path exists but is not a directory: {}",
                candidate.display()
            );
            continue;
        }

        print_label(&format!(
            "Directory does not exist at {}",
            candidate.display()
        ));
        let create = Confirm::new()
            .with_prompt("Create it? (y/n)")
            .default(true)
            .interact()?;
        print_spacer();
        if create {
            fs::create_dir_all(&candidate)?;
            warn_invalid_language_files(root, &input)?;
            return Ok(input);
        }
    }
}

struct DirectoryChoice {
    label: String,
    value: Option<String>,
}

fn build_directory_choices(root: &Path) -> Result<Vec<DirectoryChoice>> {
    let mut choices = Vec::new();
    choices.push(DirectoryChoice {
        label: "Project root (.)".to_string(),
        value: Some(".".to_string()),
    });

    let mut dirs = collect_directories(root)?;
    dirs.sort();
    for dir in dirs {
        choices.push(DirectoryChoice {
            label: dir.clone(),
            value: Some(dir),
        });
    }

    choices.push(DirectoryChoice {
        label: "Create new directory…".to_string(),
        value: None,
    });

    Ok(choices)
}

fn collect_directories(root: &Path) -> Result<Vec<String>> {
    let mut dirs = Vec::new();
    let mut stack = vec![PathBuf::new()];

    while let Some(relative) = stack.pop() {
        let dir_path = root.join(&relative);
        let entries = match fs::read_dir(&dir_path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }

            let name = entry.file_name();
            let name = match name.to_str() {
                Some(name) => name,
                None => continue,
            };
            if should_skip_dir(name) {
                continue;
            }

            let child = relative.join(name);
            let display = child.to_string_lossy().replace('\\', "/");
            dirs.push(display.clone());
            stack.push(child);
        }
    }

    Ok(dirs)
}

fn should_skip_dir(name: &str) -> bool {
    matches!(name, ".git" | "target" | "node_modules" | "dist" | "build")
}

fn prompt_language_list(default: Option<&[String]>) -> Result<Vec<String>> {
    loop {
        let mut input_prompt = Input::<String>::new().with_prompt(">");
        if let Some(default) = default
            && !default.is_empty()
        {
            input_prompt = input_prompt.default(default.join(","));
        }
        let input = input_prompt.interact_text()?;
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

fn prompt_default_export_format(current: ExportFormat) -> Result<ExportFormat> {
    print_label("Default export format");
    let choices = ["excel", "csv"];
    let default_index = match current {
        ExportFormat::Excel => 0,
        ExportFormat::Csv => 1,
    };
    let selection = Select::new()
        .with_prompt(">")
        .items(choices)
        .default(default_index)
        .interact()?;
    print_spacer();
    Ok(match selection {
        0 => ExportFormat::Excel,
        _ => ExportFormat::Csv,
    })
}

fn prompt_mode(current: ConfigMode) -> Result<ConfigMode> {
    print_label("Mode");
    let choices = [
        ConfigMode::ReactIntl.as_str(),
        ConfigMode::NextIntl.as_str(),
    ];
    let default_index = match current {
        ConfigMode::ReactIntl => 0,
        ConfigMode::NextIntl => 1,
    };
    let selection = Select::new()
        .with_prompt(">")
        .items(choices)
        .default(default_index)
        .interact()?;
    print_spacer();
    Ok(match selection {
        0 => ConfigMode::ReactIntl,
        _ => ConfigMode::NextIntl,
    })
}

fn discover_languages(root: &Path, relative_path: &str) -> Result<Vec<String>> {
    let dir = root.join(relative_path);
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };

    let mut languages = Vec::new();
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) if !stem.is_empty() => stem,
            _ => continue,
        };
        if is_valid_language_code(stem) {
            languages.push(stem.to_string());
        }
    }

    languages.sort();
    languages.dedup();
    Ok(languages)
}

fn warn_invalid_language_files(root: &Path, relative_path: &str) -> Result<()> {
    let dir = root.join(relative_path);
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    let mut invalid = Vec::new();
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) if !stem.is_empty() => stem,
            _ => continue,
        };
        if !is_valid_language_code(stem) {
            invalid.push(stem.to_string());
        }
    }
    if !invalid.is_empty() {
        invalid.sort();
        invalid.dedup();
        eprintln!(
            "Warning: found language files with invalid codes: {}",
            invalid.join(", ")
        );
    }
    Ok(())
}

fn prompt_message_id() -> Result<String> {
    loop {
        print_label("Message ID (must include a namespace, e.g. app.header)");
        let input = Input::<String>::new().with_prompt(">").interact_text()?;
        print_spacer();
        if let Err(err) = validate_message_id(&input) {
            eprintln!("{err}");
            continue;
        }
        return Ok(input);
    }
}

pub fn prompt_translations_for_languages(
    root: &Path,
    config: &TransConfig,
    message_id: &str,
    ai_settings: &Option<AiSettings>,
    languages: &[String],
    use_existing_defaults: bool,
) -> Result<TranslationValues> {
    let mut values = TranslationValues::new();

    for language in languages {
        let default_value = if use_existing_defaults {
            let translations = load_language_translations(root, config, language)?;
            translations.get(message_id).cloned()
        } else {
            None
        };

        print_label(&format!("Translation for {language}"));
        let mut input = Input::<String>::new().with_prompt(">");
        if let Some(default) = default_value {
            input = input.default(default);
        }
        let mut translation = input.allow_empty(true).interact_text()?;
        if let Some(initial_feedback) = parse_ai_command(&translation) {
            if language == &config.primary_language {
                eprintln!("AI mode requires a manual entry for the primary language first.");
                translation = Input::<String>::new()
                    .with_prompt(">")
                    .allow_empty(true)
                    .interact_text()?;
            } else {
                let source_text = values
                    .get(&config.primary_language)
                    .cloned()
                    .unwrap_or_default();
                if source_text.trim().is_empty() {
                    eprintln!("Primary language value is required before using /ai.");
                    translation = Input::<String>::new()
                        .with_prompt(">")
                        .allow_empty(true)
                        .interact_text()?;
                } else if let Some(settings) = ai_settings {
                    let reference_translations =
                        gather_reference_translations(root, config, message_id, language, &values)?;
                    translation = prompt_translation_with_ai(
                        settings,
                        &config.primary_language,
                        language,
                        message_id,
                        &source_text,
                        &reference_translations,
                        initial_feedback,
                    )?;
                } else {
                    eprintln!("AI is not configured. Enter translation manually.");
                    translation = Input::<String>::new()
                        .with_prompt(">")
                        .allow_empty(true)
                        .interact_text()?;
                }
            }
        }
        print_spacer();
        values.insert(language.clone(), translation);
    }

    Ok(values)
}

pub fn languages_for_all(config: &TransConfig) -> Vec<String> {
    let mut languages = Vec::with_capacity(config.available_languages.len());
    languages.push(config.primary_language.clone());
    for language in &config.available_languages {
        if language != &config.primary_language {
            languages.push(language.clone());
        }
    }
    languages
}

fn suggest_translation_blocking(
    settings: &AiSettings,
    source_lang: &str,
    target_lang: &str,
    message_id: &str,
    source_text: &str,
    context: &SuggestTranslationContext,
) -> Result<String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|err| {
        crate::error::TransError::InvalidInput(format!("AI runtime error: {err}"))
    })?;
    let spinner = start_spinner(format!("Consulting {}", settings.model));
    let result = runtime.block_on(suggest_translation_with_context(
        settings,
        source_lang,
        target_lang,
        message_id,
        source_text,
        context,
    ));
    drop(spinner);
    result
}

fn gather_reference_translations(
    root: &Path,
    config: &TransConfig,
    message_id: &str,
    target_language: &str,
    values: &TranslationValues,
) -> Result<Vec<(String, String)>> {
    let mut references = Vec::new();
    for language in &config.available_languages {
        if language == target_language || language == &config.primary_language {
            continue;
        }
        let value = if let Some(value) = values.get(language) {
            Some(value.clone())
        } else {
            let translations = load_language_translations(root, config, language)?;
            translations.get(message_id).cloned()
        };
        if let Some(value) = value
            && !value.trim().is_empty()
            && value != config.default_untranslated_value
        {
            references.push((language.clone(), value));
        }
    }
    Ok(references)
}

fn prompt_translation_with_ai(
    settings: &AiSettings,
    source_language: &str,
    target_language: &str,
    message_id: &str,
    source_text: &str,
    reference_translations: &[(String, String)],
    initial_feedback: Option<String>,
) -> Result<String> {
    let mut suggestions: Vec<String> = Vec::new();
    let mut feedback_history: Vec<String> = Vec::new();
    let mut latest_feedback = initial_feedback;

    loop {
        let context = SuggestTranslationContext {
            reference_translations: reference_translations.to_vec(),
            previous_suggestions: suggestions.clone(),
            feedback_history: feedback_history.clone(),
            latest_feedback: latest_feedback.clone(),
            request_alternative: latest_feedback.is_none() && !suggestions.is_empty(),
        };
        match suggest_translation_blocking(
            settings,
            source_language,
            target_language,
            message_id,
            source_text,
            &context,
        ) {
            Ok(suggestion) => {
                if !suggestions.iter().any(|value| value == &suggestion) {
                    suggestions.push(suggestion.clone());
                }
                if let Some(feedback) = latest_feedback.take() {
                    feedback_history.push(feedback);
                }
                match select_translation_action(&suggestions)? {
                    AiSelection::UseSuggestion(selection) => return Ok(selection),
                    AiSelection::Instruct(feedback) => {
                        latest_feedback = feedback;
                        continue;
                    }
                    AiSelection::WriteCustom(custom) => return Ok(custom),
                }
            }
            Err(err) => {
                eprintln!("AI error: {err}");
                match select_translation_action(&suggestions)? {
                    AiSelection::UseSuggestion(selection) => return Ok(selection),
                    AiSelection::Instruct(feedback) => {
                        latest_feedback = feedback;
                        continue;
                    }
                    AiSelection::WriteCustom(custom) => return Ok(custom),
                }
            }
        }
    }
}

enum AiSelection {
    UseSuggestion(String),
    Instruct(Option<String>),
    WriteCustom(String),
}

fn select_translation_action(suggestions: &[String]) -> Result<AiSelection> {
    if !suggestions.is_empty() {
        print_label("AI suggestions");
        let mut items: Vec<String> = suggestions.to_vec();
        let instruct_label = style("Instruct the AI").bright().cyan().to_string();
        let custom_label = style("Write custom translation")
            .bright()
            .cyan()
            .to_string();
        items.push(instruct_label);
        items.push(custom_label);
        let selection = Select::new()
            .with_prompt(">")
            .items(&items)
            .default(suggestions.len().saturating_sub(1))
            .interact()?;
        if selection < suggestions.len() {
            return Ok(AiSelection::UseSuggestion(suggestions[selection].clone()));
        }
        if selection == suggestions.len() {
            show_previous_suggestion(suggestions);
            let feedback = Input::<String>::new()
                .with_prompt(">")
                .allow_empty(true)
                .interact_text()?;
            let feedback = if feedback.trim().is_empty() {
                None
            } else {
                Some(feedback)
            };
            return Ok(AiSelection::Instruct(feedback));
        }
        show_previous_suggestion(suggestions);
        let mut prompt = Input::<String>::new().with_prompt(">").allow_empty(true);
        if let Some(last) = suggestions.last() {
            prompt = prompt.default(last.clone());
        }
        return Ok(AiSelection::WriteCustom(prompt.interact_text()?));
    }

    let command_label = style("Instruct the AI").bright().cyan();
    let manual_label = style("Write custom translation").bright().cyan();
    print_label(&format!("{} or {}", command_label, manual_label));
    let selection = Select::new()
        .with_prompt(">")
        .items(&[command_label.to_string(), manual_label.to_string()])
        .default(0)
        .interact()?;
    if selection == 0 {
        let feedback = Input::<String>::new()
            .with_prompt(">")
            .allow_empty(true)
            .interact_text()?;
        let feedback = if feedback.trim().is_empty() {
            None
        } else {
            Some(feedback)
        };
        return Ok(AiSelection::Instruct(feedback));
    }
    let prompt = Input::<String>::new().with_prompt(">").allow_empty(true);
    Ok(AiSelection::WriteCustom(prompt.interact_text()?))
}

fn show_previous_suggestion(suggestions: &[String]) {
    if let Some(last) = suggestions.last() {
        print_label("Previous suggestion");
        println!("{last}");
        print_spacer();
    }
}

fn parse_ai_command(input: &str) -> Option<Option<String>> {
    let trimmed = input.trim();
    if !trimmed.starts_with("/ai") {
        return None;
    }
    if trimmed == "/ai" {
        return Some(None);
    }
    let rest = trimmed.trim_start_matches("/ai");
    if rest.is_empty() {
        return Some(None);
    }
    let first = rest.chars().next()?;
    if !first.is_whitespace() {
        return None;
    }
    let feedback = rest.trim();
    if feedback.is_empty() {
        Some(None)
    } else {
        Some(Some(feedback.to_string()))
    }
}

pub fn ensure_next_intl_strings(root: &Path, config: &TransConfig) -> Result<()> {
    if config.mode != ConfigMode::NextIntl {
        return Ok(());
    }

    let non_string = collect_non_string_leaf_values(root, config)?;
    if non_string.is_empty() {
        return Ok(());
    }

    print_label("Non-string values found in next-intl files");
    println!("{}", non_string.format_for_display());
    let coerce = Confirm::new()
        .with_prompt("Coerce these values to strings now?")
        .default(true)
        .interact()?;
    print_spacer();
    if !coerce {
        return Err(crate::error::TransError::NextIntlNonStringValues(
            non_string.format_for_display(),
        ));
    }
    let changed = coerce_non_string_leaf_values(root, config)?;
    println!("Coerced non-string values in {changed} file(s).");
    print_spacer();
    Ok(())
}

fn maybe_migrate_mode(root: &Path, original: &TransConfig, target_mode: ConfigMode) -> Result<()> {
    if original.mode == target_mode {
        return Ok(());
    }

    print_label(&format!(
        "Mode changed from {} to {}.",
        original.mode.as_str(),
        target_mode.as_str()
    ));
    let migrate = Confirm::new()
        .with_prompt("Migrate language files now?")
        .default(true)
        .interact()?;
    print_spacer();
    if !migrate {
        return Ok(());
    }

    if original.mode == ConfigMode::NextIntl {
        ensure_next_intl_strings(root, original)?;
    }

    migrate_language_files(root, original, target_mode)?;
    println!("Migrated language files to {} mode.", target_mode.as_str());
    print_spacer();
    Ok(())
}

fn print_label(text: &str) {
    println!("{}", style(text).bold());
}

fn print_description(text: &str) {
    println!("{text}");
}

fn print_spacer() {
    println!();
}

#[cfg(test)]
mod tests {
    use super::parse_ai_command;

    #[test]
    fn parse_ai_command_without_feedback() {
        assert_eq!(parse_ai_command("/ai"), Some(None));
        assert_eq!(parse_ai_command(" /ai  "), Some(None));
    }

    #[test]
    fn parse_ai_command_with_feedback() {
        assert_eq!(
            parse_ai_command("/ai bruk ordet las"),
            Some(Some("bruk ordet las".to_string()))
        );
    }

    #[test]
    fn parse_ai_command_rejects_non_ai_input() {
        assert_eq!(parse_ai_command("hello"), None);
        assert_eq!(parse_ai_command("/air"), None);
    }
}
