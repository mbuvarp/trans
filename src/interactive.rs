use std::fs;
use std::path::{Path, PathBuf};

use console::style;
use dialoguer::{Completion, Confirm, FuzzySelect, Input, Select};

use crate::ai::{resolve_ai_settings, suggest_translation, AiSettings};
use crate::config::{AiConfig, TransConfig};
use crate::error::Result;
use crate::message_id::validate_message_id;
use crate::operations::{add_translation, delete_translation, update_translation, TranslationValues};
use crate::translations::load_language_translations;
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

pub fn init_config_interactive(root: impl AsRef<Path>) -> Result<()> {
    let root = root.as_ref();
    let language_files_path = prompt_language_files_path(root)?;

    let default_languages = discover_languages(root, &language_files_path)?;
    print_label("Available languages (comma-separated)");
    let available_languages = prompt_language_list(
        if default_languages.is_empty() {
            None
        } else {
            Some(&default_languages)
        },
    )?;
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

    let config = TransConfig {
        language_files_path: language_files_path.into(),
        available_languages,
        required_languages,
        primary_language,
        default_untranslated_value,
        ai: None,
    };

    config.validate()?;
    config.save_to_root(root)?;

    Ok(())
}

pub fn configure_root_interactive(root: impl AsRef<Path>) -> Result<()> {
    let root = root.as_ref();
    let mut config = TransConfig::load_from_root(root)?;

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

    config.language_files_path = language_files_path.into();
    config.available_languages = available_languages;
    config.required_languages = required_languages;
    config.primary_language = primary_language;
    config.default_untranslated_value = default_untranslated_value;

    config.validate()?;
    config.save_to_root(root)?;
    Ok(())
}

pub fn configure_ai_interactive(root: impl AsRef<Path>) -> Result<()> {
    let root = root.as_ref();
    let mut config = TransConfig::load_from_root(root)?;
    let defaults = config.ai.clone().unwrap_or_default();

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

    config.ai = Some(AiConfig {
        enabled,
        model,
        api_key_env,
        max_output_tokens,
    });

    config.save_to_root(root)?;
    Ok(())
}

pub fn run_interactive(root: impl AsRef<Path>, message_id: Option<String>) -> Result<()> {
    let root = root.as_ref();
    let config = TransConfig::load_from_root(root)?;
    verify_language_files(root, &config)?;
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
            .items(&["Update", "Delete", "Cancel"])
            .default(0)
            .interact()?;
        print_spacer();
        match selection {
            0 => {
                let values = prompt_required_translations(root, &config, &message_id, &ai_settings, true)?;
                update_translation(root, &config, &message_id, &values)
            }
            1 => delete_translation(root, &config, &message_id),
            _ => Ok(()),
        }
    } else {
        let values = prompt_required_translations(root, &config, &message_id, &ai_settings, false)?;
        add_translation(root, &config, &message_id, &values)
    }
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
                return Ok(input);
            }
            eprintln!("Path exists but is not a directory: {}", candidate.display());
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
        if let Some(default) = default {
            if !default.is_empty() {
                input_prompt = input_prompt.default(default.join(","));
            }
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
        languages.push(stem.to_string());
    }

    languages.sort();
    languages.dedup();
    Ok(languages)
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

fn prompt_required_translations(
    root: &Path,
    config: &TransConfig,
    message_id: &str,
    ai_settings: &Option<AiSettings>,
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

        print_label(&format!("Translation for {language}"));
        let mut input = Input::<String>::new().with_prompt(">");
        if let Some(default) = default_value {
            input = input.default(default);
        }
        let mut translation = input.allow_empty(true).interact_text()?;
        if translation.trim() == "/ai" {
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
                    translation = loop {
                        match suggest_translation_blocking(
                            settings,
                            &config.primary_language,
                            language,
                            message_id,
                            &source_text,
                        ) {
                            Ok(suggestion) => {
                                print_label("AI suggestion");
                                println!("{suggestion}");
                                let reviewed = Input::<String>::new()
                                    .with_prompt(">")
                                    .allow_empty(true)
                                    .default(suggestion.clone())
                                    .interact_text()?;
                                if reviewed.trim() == "/ai" {
                                    continue;
                                }
                                break reviewed;
                            }
                            Err(err) => {
                                eprintln!("AI error: {err}");
                                let manual = Input::<String>::new()
                                    .with_prompt(">")
                                    .allow_empty(true)
                                    .interact_text()?;
                                break manual;
                            }
                        }
                    };
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

fn suggest_translation_blocking(
    settings: &AiSettings,
    source_lang: &str,
    target_lang: &str,
    message_id: &str,
    source_text: &str,
) -> Result<String> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|err| crate::error::TransError::InvalidInput(format!("AI runtime error: {err}")))?;
    runtime.block_on(suggest_translation(
        settings,
        source_lang,
        target_lang,
        message_id,
        source_text,
    ))
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
