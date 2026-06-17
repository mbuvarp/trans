use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::{ConfigMode, ExportFormat};
use crate::error::{Result, TransError};
use crate::import::ExtraLangsStrategy;
use crate::operations::TranslationValues;

#[derive(Parser, Debug)]
#[command(name = "trans", version, disable_version_flag = true)]
#[command(about = "Translation utility for translation JSON files")]
#[command(
    long_about = "Translation utility for translation JSON files (react-intl or next-intl).\n\nRun without a subcommand to enter interactive mode, or pass a MESSAGE_ID to use the interactive add/edit flow for that id."
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
        short = 'C',
        long = "cwd",
        global = true,
        value_name = "DIR",
        help = "Run as if trans was started in DIR"
    )]
    pub cwd: Option<PathBuf>,
    #[arg(long = "all", help = "Prompt for all languages in interactive mode")]
    pub all: bool,
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
        values: Option<String>,
        #[arg(long = "all", help = "Prompt for all languages interactively")]
        all: bool,
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
        values: Option<String>,
        #[arg(long = "all", help = "Prompt for all languages interactively")]
        all: bool,
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
    #[command(about = "Check whether a message id exists in all languages")]
    Has {
        #[arg(value_name = "MESSAGE_ID", help = "Message id with namespace to check")]
        id: String,
    },
    #[command(about = "Find message ids by searching translation values")]
    Find {
        #[arg(value_name = "QUERY", help = "Translation string to search for")]
        query: String,
        #[arg(
            short = 'e',
            long = "exact-only",
            help = "Only include exact same-case string matches"
        )]
        exact_only: bool,
        #[arg(
            short = 'c',
            long = "case-sensitive",
            help = "Match using exact casing"
        )]
        case_sensitive: bool,
        #[arg(
            short = 'l',
            long = "language",
            value_name = "LANG",
            help = "Language file to search (defaults to primaryLanguage)"
        )]
        language: Option<String>,
    },
    #[command(
        about = "Update config values interactively",
        long_about = "Update config values interactively.\n\nRoot config options:\n- mode: translation library mode (react-intl or next-intl)\n- languageFilesPath: location of language files\n- availableLanguages: all known languages\n- requiredLanguages: languages required for input\n- primaryLanguage: first language in interactive mode\n- defaultUntranslatedValue: default for non-required languages\n- defaultExportFormat: csv or excel\n- excelPassword: password for Excel sheet protection\n- runUpdateCheck: enable brew update check prompt after successful commands\n\nUse `trans config ai` to edit AI settings:\n- enabled\n- model\n- apiKeyEnv\n- maxOutputTokens\n- concurrency\n\nUse `trans config show` to print current configuration values.\nUse `trans config edit [key]` to edit all values or a single key.\nUse `trans config --format json|yaml` to convert the config file format."
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
    #[command(
        about = "Convert translation files between react-intl and next-intl and update config mode",
        long_about = "Convert translation files between react-intl and next-intl formats and update config mode when migration succeeds.\n\nBy default files are converted in place under languageFilesPath.\nUse -o/--out-dir to write converted files to another directory.\nWhen -o is used, languageFilesPath is updated unless --no-update-language-files-path is provided.\nUse -b/--backup to copy languageFilesPath to a sibling directory named <languageFilesPath>__backup before migration; migration fails if that backup directory already exists.\nUse -c/--check to only validate migration compatibility (no writes to files or config). In check mode, --backup is ignored and --out-dir is not created or validated."
    )]
    Migrate {
        #[arg(
            value_enum,
            value_name = "MODE",
            help = "Target mode: react-intl or next-intl"
        )]
        mode: ConfigMode,
        #[arg(
            short = 'o',
            long = "out-dir",
            value_name = "DIR",
            help = "Output directory for converted files (created if missing)"
        )]
        out_dir: Option<String>,
        #[arg(
            long = "no-update-language-files-path",
            help = "With --out-dir, keep existing languageFilesPath in config"
        )]
        no_update_language_files_path: bool,
        #[arg(
            short = 'b',
            long = "backup",
            help = "Create a backup directory at <languageFilesPath>__backup before migration"
        )]
        backup: bool,
        #[arg(
            short = 'c',
            long = "check",
            help = "Only validate migration compatibility; do not modify files or config"
        )]
        check: bool,
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
    #[command(about = "Sync missing ids from the primary language into all languages")]
    Sync,
    #[command(about = "Translate all missing values with AI")]
    Auto {
        #[arg(
            short = 'l',
            long = "lang",
            value_name = "LANGS",
            help = "Comma-separated locales to translate (primary language is the source)"
        )]
        lang: Option<String>,
        #[arg(
            short = 'c',
            long = "concurrency",
            value_name = "N",
            help = "Number of AI requests to run in parallel"
        )]
        concurrency: Option<usize>,
    },
    #[command(about = "Add a new language based on the primary language keys")]
    AddLang {
        #[arg(value_name = "LANG", help = "Language code to add (e.g. nb)")]
        lang: String,
    },
    #[command(about = "Remove a language file and update config")]
    DelLang {
        #[arg(value_name = "LANG", help = "Language code to remove (e.g. nb)")]
        lang: String,
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
    #[value(name = "mode")]
    Mode,
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
    #[value(name = "runUpdateCheck")]
    RunUpdateCheck,
    #[value(name = "ai.enabled")]
    AiEnabled,
    #[value(name = "ai.model")]
    AiModel,
    #[value(name = "ai.apiKeyEnv")]
    AiApiKeyEnv,
    #[value(name = "ai.maxOutputTokens")]
    AiMaxOutputTokens,
    #[value(name = "ai.concurrency")]
    AiConcurrency,
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
