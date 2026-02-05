use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;
use console::style;
use dialoguer::Confirm;

use trans::cli::{
    Cli, Command, ConfigFormat, ConfigKey, ConfigSection, parse_lang_list, parse_values,
};
use trans::config::{
    ConfigField, ConfigFormat as ConfigFileFormat, ExportFormat, TransConfig, format_config_list,
};
use trans::error::{Result, TransError};
use trans::export::{export_csv, export_csv_with_options, export_excel, export_excel_with_options};
use trans::interactive::{
    configure_ai_interactive, configure_edit_interactive, configure_root_interactive,
    init_config_interactive, run_interactive,
};
use trans::operations::{
    add_language, add_translation, change_message_id, delete_language, delete_translation,
    update_translation,
};
use trans::query::{get_translation, get_translations_all, list_required_languages};
use trans::sync::{apply_sync_plan, collect_missing_ids, maybe_prompt_sync};
use trans::update_check::{UpdateInfo, spawn_update_check};
use trans::verify::{collect_verification_issues, verify_language_files};
use trans::verify_ai::verify_with_ai;

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.version {
        println!("trans {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let update_check = spawn_update_check(env!("CARGO_PKG_VERSION"));
    let result = match cli.command {
        None => {
            let root = env::current_dir()?;
            run_interactive(&root, cli.message_id, cli.all)
        }
        Some(Command::Init { format }) => {
            let root = env::current_dir()?;
            init_config_interactive(
                &root,
                match format {
                    ConfigFormat::Json => ConfigFileFormat::Json,
                    ConfigFormat::Yaml => ConfigFileFormat::Yaml,
                },
            )
        }
        Some(Command::Export {
            format,
            lang,
            output,
            missing,
            no_lock,
        }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            let format = format.unwrap_or(config.default_export_format);
            let use_custom = lang.is_some() || output.is_some() || missing || no_lock;
            let selected = if let Some(lang) = lang.as_ref() {
                let langs = parse_lang_list(lang)?;
                let selected = resolve_export_languages(&config, &langs)?;
                let only_primary = langs.iter().all(|value| value == &config.primary_language);
                if only_primary {
                    eprintln!(
                        "Warning: only the primary language '{}' was provided; nothing to export.",
                        config.primary_language
                    );
                    None
                } else {
                    Some(selected)
                }
            } else {
                None
            };
            if use_custom && selected.is_some() {
                verify_language_files(&root, &config)?;
            }
            let output_path = build_export_path(&root, output.as_deref(), format);
            if lang.is_some() && selected.is_none() {
                Ok(())
            } else {
                match format {
                    ExportFormat::Csv => {
                    if let Some(langs) = selected.as_ref() {
                        let translations =
                            trans::export::load_selected_languages(&root, &config, langs)?;
                        export_csv_with_options(
                            &config,
                            &translations,
                            langs,
                            &output_path,
                            missing,
                        )?;
                        println!("Exported CSV to {}", output_path.display());
                        Ok(())
                    } else {
                        if use_custom {
                            let translations = trans::export::load_all_languages(&root, &config)?;
                            export_csv_with_options(
                                &config,
                                &translations,
                                &config.available_languages,
                                &output_path,
                                missing,
                            )?;
                            println!("Exported CSV to {}", output_path.display());
                            Ok(())
                        } else {
                            let path = export_csv(&root, &config)?;
                            println!("Exported CSV to {}", path.display());
                            Ok(())
                        }
                    }
                }
                    ExportFormat::Excel => {
                    if let Some(langs) = selected.as_ref() {
                        let translations =
                            trans::export::load_selected_languages(&root, &config, langs)?;
                        export_excel_with_options(
                            &config,
                            &translations,
                            langs,
                            &output_path,
                            missing,
                            !no_lock,
                        )?;
                        println!("Exported Excel to {}", output_path.display());
                        Ok(())
                    } else {
                        if use_custom {
                            let translations = trans::export::load_all_languages(&root, &config)?;
                            export_excel_with_options(
                                &config,
                                &translations,
                                &config.available_languages,
                                &output_path,
                                missing,
                                !no_lock,
                            )?;
                            println!("Exported Excel to {}", output_path.display());
                            Ok(())
                        } else {
                            let path = export_excel(&root, &config)?;
                            println!("Exported Excel to {}", path.display());
                            Ok(())
                        }
                    }
                    }
                }
            }
        }
        Some(Command::Import {
            path,
            lang,
            extra_langs,
            trim,
            ai,
        }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            let lang_filter = match lang {
                Some(lang) => Some(parse_lang_list(&lang)?),
                None => None,
            };
            trans::import::import_translations_with_ai(
                &root,
                &config,
                Path::new(&path),
                lang_filter,
                extra_langs,
                trim,
                ai,
            )
        }
        Some(Command::ListRequiredLanguages) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            for language in list_required_languages(&config) {
                println!("{language}");
            }
            Ok(())
        }
        Some(Command::Add { id, values, all }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            if all && values.is_some() {
                return Err(TransError::InvalidInput(
                    "cannot use --all together with --values".to_string(),
                ));
            }
            if all {
                let ai_settings = trans::ai::resolve_ai_settings(&root, &config)?;
                let values = trans::interactive::prompt_translations_for_languages(
                    &root,
                    &config,
                    &id,
                    &ai_settings,
                    &trans::interactive::languages_for_all(&config),
                    false,
                )?;
                add_translation(&root, &config, &id, &values)
            } else {
                let values = values.ok_or_else(|| {
                    TransError::InvalidInput("missing --values or --all".to_string())
                })?;
                let values = parse_values(&values)?;
                match add_translation(&root, &config, &id, &values) {
                    Err(err) => {
                        if maybe_prompt_sync(&root, &config, &err)? {
                            Ok(())
                        } else {
                            Err(err)
                        }
                    }
                    Ok(()) => Ok(()),
                }
            }
        }
        Some(Command::Update { id, values, all }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            if all && values.is_some() {
                return Err(TransError::InvalidInput(
                    "cannot use --all together with --values".to_string(),
                ));
            }
            if all {
                let ai_settings = trans::ai::resolve_ai_settings(&root, &config)?;
                let values = trans::interactive::prompt_translations_for_languages(
                    &root,
                    &config,
                    &id,
                    &ai_settings,
                    &trans::interactive::languages_for_all(&config),
                    true,
                )?;
                update_translation(&root, &config, &id, &values)
            } else {
                let values = values.ok_or_else(|| {
                    TransError::InvalidInput("missing --values or --all".to_string())
                })?;
                let values = parse_values(&values)?;
                match update_translation(&root, &config, &id, &values) {
                    Err(err) => {
                        if maybe_prompt_sync(&root, &config, &err)? {
                            Ok(())
                        } else {
                            Err(err)
                        }
                    }
                    Ok(()) => Ok(()),
                }
            }
        }
        Some(Command::Delete { id }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            match delete_translation(&root, &config, &id) {
                Err(err) => {
                    if maybe_prompt_sync(&root, &config, &err)? {
                        Ok(())
                    } else {
                        Err(err)
                    }
                }
                Ok(()) => Ok(()),
            }
        }
        Some(Command::Show { id, lang }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            if let Some(lang) = lang {
                let value = get_translation(&root, &config, &id, &lang)?;
                println!("{value}");
            } else {
                let results = get_translations_all(&root, &config, &id)?;
                for (language, value) in results {
                    println!("{language}: {value}");
                }
            }
            Ok(())
        }
        Some(Command::Config { section, format }) => {
            let root = env::current_dir()?;
            if let Some(format) = format {
                if section.is_some() {
                    return Err(TransError::InvalidInput(
                        "--format cannot be used with a config section".to_string(),
                    ));
                }
                let config = TransConfig::load_from_root(&root)?;
                let target = config.save_to_root_format(
                    &root,
                    match format {
                        ConfigFormat::Json => ConfigFileFormat::Json,
                        ConfigFormat::Yaml => ConfigFileFormat::Yaml,
                    },
                    true,
                )?;
                println!("Wrote config to {}", target.display());
                Ok(())
            } else {
                match section {
                    Some(ConfigSection::Ai) => configure_ai_interactive(&root),
                    Some(ConfigSection::Show) => {
                        let (config, path) = TransConfig::load_from_root_with_path(&root)?;
                        for line in format_config_list(&config, Some(&path)) {
                            println!("{line}");
                        }
                        Ok(())
                    }
                    Some(ConfigSection::Edit { key }) => {
                        let field = key.map(map_config_key);
                        configure_edit_interactive(&root, field)
                    }
                    None => configure_root_interactive(&root),
                }
            }
        }
        Some(Command::ChangeId { old_id, new_id }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            match change_message_id(&root, &config, &old_id, &new_id) {
                Err(err) => {
                    if maybe_prompt_sync(&root, &config, &err)? {
                        Ok(())
                    } else {
                        Err(err)
                    }
                }
                Ok(()) => Ok(()),
            }
        }
        Some(Command::Verify { ai }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            if ai {
                verify_with_ai(&root, &config)
            } else {
                let issues = collect_verification_issues(&root, &config)?;
                if issues.is_empty() {
                    println!("OK");
                    Ok(())
                } else {
                    println!("Found {} errors in translation files:\n", issues.len());
                    for (index, issue) in issues.iter().enumerate() {
                        let relative = issue.path.strip_prefix(&root).unwrap_or(&issue.path);
                        let display_path = format!("{}:{}", relative.display(), issue.line);
                        println!("{}", style(display_path).bold());
                        println!("{}", issue.message);
                        if index + 1 < issues.len() {
                            println!();
                        }
                    }
                    process::exit(1);
                }
            }
        }
        Some(Command::Sync) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            handle_sync(&root, &config)
        }
        Some(Command::Auto { lang, concurrency }) => {
            let root = env::current_dir()?;
            let mut config = TransConfig::load_from_root(&root)?;
            if let Some(concurrency) = concurrency {
                if concurrency == 0 {
                    return Err(TransError::InvalidInput(
                        "concurrency must be at least 1".to_string(),
                    ));
                }
                let ai = config.ai.clone().unwrap_or_default();
                config.ai = Some(trans::config::AiConfig { concurrency, ..ai });
            }
            let lang_filter = match lang {
                Some(lang) => Some(parse_lang_list(&lang)?),
                None => None,
            };
            trans::auto::auto_translate(&root, &config, lang_filter)
        }
        Some(Command::AddLang { lang }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            add_language(&root, &config, &lang)
        }
        Some(Command::DelLang { lang }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            if lang == config.primary_language {
                return Err(TransError::InvalidInput(
                    "cannot delete the primary language".to_string(),
                ));
            }
            let confirmed = Confirm::new()
                .with_prompt(format!(
                    "Delete language '{lang}'? This will remove the language file and update config."
                ))
                .default(false)
                .interact()?;
            if confirmed {
                delete_language(&root, &config, &lang)
            } else {
                Ok(())
            }
        }
    };
    if result.is_ok() {
        maybe_prompt_update(update_check)?;
    }
    result
}

fn handle_sync(root: &Path, config: &TransConfig) -> Result<()> {
    let plan = collect_missing_ids(root, config)?;
    let report = plan.report();
    if report.total_missing == 0 {
        println!("No missing IDs.");
        return Ok(());
    }

    println!("Found missing IDs:");
    for (language, count) in report.missing_by_language {
        println!("{language}: {count}");
    }
    println!();
    let default_message = if config.default_untranslated_value.is_empty() {
        "<empty>".to_string()
    } else {
        config.default_untranslated_value.clone()
    };
    let prompt = format!(
        "Do you want to add the missing IDs with message \"{default_message}\"?"
    );
    let confirmed = dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(true)
        .interact()?;
    if !confirmed {
        return Ok(());
    }

    let applied = apply_sync_plan(root, config, &plan)?.total_missing;
    if applied == 0 {
        println!("No missing IDs to add.");
    } else {
        println!("Added {applied} missing IDs.");
    }
    Ok(())
}

fn maybe_prompt_update(receiver: Option<std::sync::mpsc::Receiver<UpdateInfo>>) -> Result<()> {
    let receiver = match receiver {
        Some(receiver) => receiver,
        None => return Ok(()),
    };
    let info = match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
        Ok(info) => info,
        Err(_) => return Ok(()),
    };
    println!(
        "You are using trans {}, but {} is available. Update now?",
        style(format!("v{}", info.current)).bold(),
        style(format!("v{}", info.latest)).bold()
    );
    let confirmed = Confirm::new().default(true).interact()?;
    if !confirmed {
        return Ok(());
    }
    let status = std::process::Command::new("brew")
        .args(["upgrade", "trans"])
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(TransError::InvalidInput(
            "brew upgrade trans failed".to_string(),
        )),
        Err(err) => Err(TransError::InvalidInput(format!(
            "failed to run brew upgrade trans: {err}"
        ))),
    }
}

fn map_config_key(key: ConfigKey) -> ConfigField {
    match key {
        ConfigKey::LanguageFilesPath => ConfigField::LanguageFilesPath,
        ConfigKey::AvailableLanguages => ConfigField::AvailableLanguages,
        ConfigKey::RequiredLanguages => ConfigField::RequiredLanguages,
        ConfigKey::PrimaryLanguage => ConfigField::PrimaryLanguage,
        ConfigKey::DefaultUntranslatedValue => ConfigField::DefaultUntranslatedValue,
        ConfigKey::DefaultExportFormat => ConfigField::DefaultExportFormat,
        ConfigKey::ExcelPassword => ConfigField::ExcelPassword,
        ConfigKey::AiEnabled => ConfigField::AiEnabled,
        ConfigKey::AiModel => ConfigField::AiModel,
        ConfigKey::AiApiKeyEnv => ConfigField::AiApiKeyEnv,
        ConfigKey::AiMaxOutputTokens => ConfigField::AiMaxOutputTokens,
        ConfigKey::AiConcurrency => ConfigField::AiConcurrency,
    }
}

fn resolve_export_languages(config: &TransConfig, requested: &[String]) -> Result<Vec<String>> {
    if requested.is_empty() {
        return Err(TransError::InvalidInput(
            "languages must not be empty".to_string(),
        ));
    }

    let mut seen = HashSet::new();
    let mut selected = Vec::new();

    seen.insert(config.primary_language.clone());
    selected.push(config.primary_language.clone());

    for language in requested {
        if !config
            .available_languages
            .iter()
            .any(|lang| lang == language)
        {
            return Err(TransError::InvalidInput(format!(
                "language '{language}' is not in available_languages"
            )));
        }
        if seen.insert(language.clone()) {
            selected.push(language.clone());
        }
    }

    Ok(selected)
}

fn build_export_path(root: &Path, output: Option<&str>, format: ExportFormat) -> PathBuf {
    let base = output.unwrap_or("translations");
    let mut path = PathBuf::from(base);
    if path.extension().is_none() {
        let ext = match format {
            ExportFormat::Csv => "csv",
            ExportFormat::Excel => "xlsx",
        };
        path.set_extension(ext);
    }
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}
