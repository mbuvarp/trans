use std::collections::HashSet;
use std::env;
use std::process;

use clap::Parser;

use trans::cli::{
    Cli, Command, ConfigFormat, ConfigKey, ConfigSection, parse_lang_list, parse_values,
};
use trans::config::{
    ConfigField, ConfigFormat as ConfigFileFormat, TransConfig, format_config_list,
};
use trans::error::{Result, TransError};
use trans::export::{export_csv, export_csv_filtered, export_excel, export_excel_filtered};
use trans::interactive::{
    configure_ai_interactive, configure_edit_interactive, configure_root_interactive,
    init_config_interactive, run_interactive,
};
use trans::operations::{
    add_translation, change_message_id, delete_translation, update_translation,
};
use trans::query::{get_translation, get_translations_all, list_required_languages};
use trans::verify::verify_language_files;

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
        Some(Command::Export { format, lang }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
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
            match format {
                trans::cli::ExportFormat::Csv => {
                    let path = if let Some(langs) = selected.as_ref() {
                        export_csv_filtered(&root, &config, langs)?
                    } else {
                        export_csv(&root, &config)?
                    };
                    println!("Exported CSV to {}", path.display());
                    Ok(())
                }
                trans::cli::ExportFormat::Excel => {
                    let path = if let Some(langs) = selected.as_ref() {
                        export_excel_filtered(&root, &config, langs)?
                    } else {
                        export_excel(&root, &config)?
                    };
                    println!("Exported Excel to {}", path.display());
                    Ok(())
                }
            }
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
        Some(Command::Verify) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            verify_language_files(&root, &config)?;
            println!("OK");
            Ok(())
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
