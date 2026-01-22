use std::collections::BTreeMap;

use clap::{Parser, Subcommand, ValueEnum};

use crate::error::{Result, TransError};
use crate::operations::TranslationValues;

#[derive(Parser, Debug)]
#[command(name = "trans")]
#[command(about = "Translation utility for react-intl JSON files")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Init,
    ListRequiredLanguages,
    Add {
        #[arg(long)]
        id: String,
        #[arg(long)]
        values: String,
    },
    Update {
        #[arg(long)]
        id: String,
        #[arg(long)]
        values: String,
    },
    Delete {
        #[arg(long)]
        id: String,
    },
    Show {
        #[arg(long)]
        id: String,
        #[arg(long)]
        lang: Option<String>,
    },
    Export {
        #[arg(long, value_enum, default_value = "csv")]
        format: ExportFormat,
    },
    Verify,
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
