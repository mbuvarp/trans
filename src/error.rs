use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("invalid message id: {0}")]
    InvalidMessageId(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("missing config file at {0}; run `trans init` first")]
    MissingConfig(String),
    #[error("missing language file at {0:?}")]
    MissingLanguageFile(PathBuf),
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    Csv(#[from] csv::Error),
    #[error(transparent)]
    Xlsx(#[from] rust_xlsxwriter::XlsxError),
    #[error(transparent)]
    Dialoguer(#[from] dialoguer::Error),
}

pub type Result<T> = std::result::Result<T, TransError>;
