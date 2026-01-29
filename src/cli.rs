use std::collections::BTreeMap;

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::ExportFormat;
use crate::error::{Result, TransError};
use crate::import::ExtraLangsStrategy;
use crate::operations::TranslationValues;

#[derive(Parser, Debug)]
#[command(name = "trans", version, disable_version_flag = true)]
#[command(about = "Translation utility for react-intl JSON files")]
#[command(
    long_about = "Translation utility for react-intl JSON files.\n\nRun without a subcommand to enter interactive mode, or pass a MESSAGE_ID to use the interactive add/edit flow for that id."
)]
pub struct Cli {
    #[arg(
        short = 'v',
        long = "version",
        action = clap::ArgAction::SetTrue,
        help = "Print version information"
    )]
    pub version: bool,
    #[arg(
        value_name = "MESSAGE_ID",
        help = "Optional message id (e.g. app.header.title) to use in interactive mode"
    )]
    pub message_id: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(about = "Create a .trans.config.json or .trans.config.yaml via interactive prompts")]
    Init {
        #[arg(
            long,
            value_enum,
            default_value = "json",
            value_name = "FORMAT",
            help = "Config format to write: json or yaml"
        )]
        format: ConfigFormat,
    },
    #[command(about = "List required languages from the config")]
    ListRequiredLanguages,
    #[command(about = "Add a new translation id with required language values")]
    Add {
        #[arg(
            long,
            value_name = "MESSAGE_ID",
            help = "Message id with namespace, e.g. app.header.title"
        )]
        id: String,
        #[arg(
            long,
            value_name = "LANG:VALUE,...",
            help = "Comma-separated translations, e.g. en:Hello,nb:Hei"
        )]
        values: String,
    },
    #[command(about = "Update an existing translation id")]
    Update {
        #[arg(
            long,
            value_name = "MESSAGE_ID",
            help = "Message id with namespace, e.g. app.header.title"
        )]
        id: String,
        #[arg(
            long,
            value_name = "LANG:VALUE,...",
            help = "Comma-separated translations, e.g. en:Hello,nb:Hei"
        )]
        values: String,
    },
    #[command(about = "Delete a translation id from all languages")]
    Delete {
        #[arg(
            long,
            value_name = "MESSAGE_ID",
            help = "Message id with namespace to delete"
        )]
        id: String,
    },
    #[command(about = "Show translations for a message id")]
    Show {
        #[arg(
            long,
            value_name = "MESSAGE_ID",
            help = "Message id with namespace to display"
        )]
        id: String,
        #[arg(
            long,
            value_name = "LANG",
            help = "Optional language code to show a single translation"
        )]
        lang: Option<String>,
    },
    #[command(
        about = "Update config values interactively",
        long_about = "Update config values interactively.\n\nRoot config options:\n- languageFilesPath: location of language files\n- availableLanguages: all known languages\n- requiredLanguages: languages required for input\n- primaryLanguage: first language in interactive mode\n- defaultUntranslatedValue: default for non-required languages\n- defaultExportFormat: csv or excel\n- excelPassword: password for Excel sheet protection\n\nUse `trans config ai` to edit AI settings:\n- enabled\n- model\n- apiKeyEnv\n- maxOutputTokens\n\nUse `trans config show` to print current configuration values.\nUse `trans config edit [key]` to edit all values or a single key.\nUse `trans config --format json|yaml` to convert the config file format."
    )]
    Config {
        #[arg(
            long,
            value_enum,
            value_name = "FORMAT",
            help = "Convert config file to format: json or yaml"
        )]
        format: Option<ConfigFormat>,
        #[command(subcommand)]
        section: Option<ConfigSection>,
    },
    #[command(about = "Rename a message id across all languages")]
    ChangeId {
        #[arg(value_name = "OLD_ID", help = "Existing message id to rename")]
        old_id: String,
        #[arg(value_name = "NEW_ID", help = "New message id to replace it with")]
        new_id: String,
    },
    #[command(about = "Export translations to CSV or Excel")]
    Export {
        #[arg(
            short = 'f',
            long,
            value_enum,
            value_name = "FORMAT",
            help = "Export format: csv or excel"
        )]
        format: Option<ExportFormat>,
        #[arg(
            short = 'l',
            long = "lang",
            value_name = "LANGS",
            help = "Comma-separated locales to include (primary language is always included)"
        )]
        lang: Option<String>,
        #[arg(
            short = 'o',
            long = "output",
            value_name = "FILE",
            help = "Output filename (defaults to translations)"
        )]
        output: Option<String>,
        #[arg(
            short = 'm',
            long = "missing",
            help = "Only export rows with missing values"
        )]
        missing: bool,
        #[arg(
            long = "no-lock",
            help = "Disable worksheet protection in Excel exports"
        )]
        no_lock: bool,
    },
    #[command(about = "Import translations from a CSV or Excel file")]
    Import {
        #[arg(value_name = "FILE", help = "Path to a .csv or .xlsx file to import")]
        path: String,
        #[arg(
            short = 'l',
            long = "lang",
            value_name = "LANGS",
            help = "Comma-separated locales to import (primary language is ignored)"
        )]
        lang: Option<String>,
        #[arg(
            long = "extra-langs",
            value_enum,
            value_name = "STRATEGY",
            help = "How to handle extra languages in the import file: ignore, create, or abort"
        )]
        extra_langs: Option<ExtraLangsStrategy>,
        #[arg(long = "trim", help = "Trim whitespace around imported values")]
        trim: bool,
        #[arg(
            long = "ai",
            help = "Use AI to suggest fixes for format/placeholder issues"
        )]
        ai: bool,
    },
    #[command(about = "Verify that all language files contain the same message ids")]
    Verify {
        #[arg(long = "ai", help = "Use AI to suggest fixes for verification errors")]
        ai: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigSection {
    #[command(about = "Update AI configuration values interactively")]
    Ai,
    #[command(about = "Show current configuration values")]
    Show,
    #[command(about = "Edit all config values or a single key")]
    Edit {
        #[arg(value_enum, value_name = "KEY", help = "Config key to edit")]
        key: Option<ConfigKey>,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum ConfigFormat {
    Json,
    Yaml,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum ConfigKey {
    #[value(name = "languageFilesPath")]
    LanguageFilesPath,
    #[value(name = "availableLanguages")]
    AvailableLanguages,
    #[value(name = "requiredLanguages")]
    RequiredLanguages,
    #[value(name = "primaryLanguage")]
    PrimaryLanguage,
    #[value(name = "defaultUntranslatedValue")]
    DefaultUntranslatedValue,
    #[value(name = "defaultExportFormat")]
    DefaultExportFormat,
    #[value(name = "excelPassword")]
    ExcelPassword,
    #[value(name = "ai.enabled")]
    AiEnabled,
    #[value(name = "ai.model")]
    AiModel,
    #[value(name = "ai.apiKeyEnv")]
    AiApiKeyEnv,
    #[value(name = "ai.maxOutputTokens")]
    AiMaxOutputTokens,
}

pub fn parse_values(input: &str) -> Result<TranslationValues> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(TransError::InvalidInput(
            "values must not be empty".to_string(),
        ));
    }

    for pair in trimmed.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (lang, value) = pair.split_once(':').ok_or_else(|| {
            TransError::InvalidInput(format!(
                "value pair '{pair}' must be in <lang>:<translation> format"
            ))
        })?;
        let lang = lang.trim();
        if lang.is_empty() {
            return Err(TransError::InvalidInput(
                "language code must not be empty".to_string(),
            ));
        }
        if map.contains_key(lang) {
            return Err(TransError::InvalidInput(format!(
                "duplicate language '{lang}' in values"
            )));
        }
        map.insert(lang.to_string(), value.to_string());
    }

    if map.is_empty() {
        return Err(TransError::InvalidInput(
            "values must include at least one language".to_string(),
        ));
    }

    Ok(map)
}

pub fn parse_lang_list(input: &str) -> Result<Vec<String>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(TransError::InvalidInput(
            "languages must not be empty".to_string(),
        ));
    }

    let mut values = Vec::new();
    for value in trimmed.split(',') {
        let value = value.trim();
        if value.is_empty() {
            return Err(TransError::InvalidInput(
                "language value must not be empty".to_string(),
            ));
        }
        values.push(value.to_string());
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_message_id_positional() {
        let cli = Cli::try_parse_from(["trans", "app.header.title"]).expect("parse");
        assert_eq!(cli.message_id.as_deref(), Some("app.header.title"));
        assert!(cli.command.is_none());
    }
}
