use std::collections::BTreeMap;

use clap::{Parser, Subcommand, ValueEnum};

use crate::error::{Result, TransError};
use crate::operations::TranslationValues;

#[derive(Parser, Debug)]
#[command(name = "trans")]
#[command(about = "Translation utility for react-intl JSON files")]
#[command(long_about = "Translation utility for react-intl JSON files.\n\nRun without a subcommand to enter interactive mode, or pass a MESSAGE_ID to use the interactive add/edit flow for that id.")]
pub struct Cli {
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
    #[command(about = "Create a .trans.config.json via interactive prompts")]
    Init,
    #[command(about = "List required languages from the config")]
    ListRequiredLanguages,
    #[command(about = "Add a new translation id with required language values")]
    Add {
        #[arg(long, value_name = "MESSAGE_ID", help = "Message id with namespace, e.g. app.header.title")]
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
        #[arg(long, value_name = "MESSAGE_ID", help = "Message id with namespace, e.g. app.header.title")]
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
        #[arg(long, value_name = "MESSAGE_ID", help = "Message id with namespace to delete")]
        id: String,
    },
    #[command(about = "Show translations for a message id")]
    Show {
        #[arg(long, value_name = "MESSAGE_ID", help = "Message id with namespace to display")]
        id: String,
        #[arg(long, value_name = "LANG", help = "Optional language code to show a single translation")]
        lang: Option<String>,
    },
    #[command(
        about = "Update config values interactively",
        long_about = "Update config values interactively.\n\nRoot config options:\n- languageFilesPath: location of language files\n- availableLanguages: all known languages\n- requiredLanguages: languages required for input\n- primaryLanguage: first language in interactive mode\n- defaultUntranslatedValue: default for non-required languages\n\nUse `trans config ai` to edit AI settings:\n- enabled\n- model\n- apiKeyEnv\n- temperature\n- maxOutputTokens"
    )]
    Config {
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
            long,
            value_enum,
            default_value = "csv",
            value_name = "FORMAT",
            help = "Export format: csv or excel"
        )]
        format: ExportFormat,
    },
    #[command(about = "Verify that all language files contain the same message ids")]
    Verify,
}

#[derive(Subcommand, Debug)]
pub enum ConfigSection {
    #[command(about = "Update AI configuration values interactively")]
    Ai,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum ExportFormat {
    Csv,
    Excel,
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
