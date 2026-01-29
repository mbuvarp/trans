use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;

use trans::cli::{
    Cli, Command, ConfigFormat, ConfigKey, ConfigSection, parse_lang_list, parse_values,
};
use trans::config::{
    ConfigField, ConfigFormat as ConfigFileFormat, ExportFormat, TransConfig, format_config_list,
};
use trans::error::{Result, TransError};
use trans::export::{export_csv, export_csv_with_options, export_excel, export_excel_with_options};
use trans::format_validation::validate_message_formats;
use trans::interactive::{
    configure_ai_interactive, configure_edit_interactive, configure_root_interactive,
    init_config_interactive, run_interactive,
};
use trans::operations::{
    add_translation, change_message_id, delete_translation, update_translation,
};
use trans::query::{get_translation, get_translations_all, list_required_languages};
use trans::verify::verify_language_files;
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
    match cli.command {
        None => {
            let root = env::current_dir()?;
            run_interactive(&root, cli.message_id)
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
            let selected = if let Some(lang) = lang {
                let langs = parse_lang_list(&lang)?;
                let selected = resolve_export_languages(&config, &langs)?;
                if langs.iter().all(|value| value == &config.primary_language) {
                    eprintln!(
                        "Warning: only the primary language '{}' was provided; nothing to export.",
                        config.primary_language
                    );
                    return Ok(());
                }
                Some(selected)
            } else {
                None
            };
            if use_custom {
                verify_language_files(&root, &config)?;
            }
            let output_path = build_export_path(&root, output.as_deref(), format);
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
        Some(Command::Add { id, values }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            let values = parse_values(&values)?;
            add_translation(&root, &config, &id, &values)
        }
        Some(Command::Update { id, values }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            let values = parse_values(&values)?;
            update_translation(&root, &config, &id, &values)
        }
        Some(Command::Delete { id }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            delete_translation(&root, &config, &id)
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
                return Ok(());
            }
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
        Some(Command::ChangeId { old_id, new_id }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            change_message_id(&root, &config, &old_id, &new_id)
        }
        Some(Command::Verify { ai }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            if ai {
                verify_with_ai(&root, &config)
            } else {
                verify_language_files(&root, &config)?;
                let translations = trans::export::load_all_languages(&root, &config)?;
                validate_message_formats(&config, &translations)?;
                println!("OK");
                Ok(())
            }
        }
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
