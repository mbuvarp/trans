use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, TransError};

fn default_untranslated_value() -> String {
    String::new()
}

const CONFIG_FILE_NAME: &str = ".trans.config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransConfig {
    pub language_files_path: PathBuf,
    pub available_languages: Vec<String>,
    pub required_languages: Vec<String>,
    pub primary_language: String,
    #[serde(default = "default_untranslated_value")]
    pub default_untranslated_value: String,
}

impl TransConfig {
    pub fn config_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(CONFIG_FILE_NAME)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(TransError::MissingConfig(path.to_path_buf()));
            }
            Err(err) => return Err(err.into()),
        };

        let config: TransConfig = serde_json::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    pub fn load_from_root(root: impl AsRef<Path>) -> Result<Self> {
        Self::load_from_path(Self::config_path(root))
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let payload = serde_json::to_string_pretty(self)?;
        fs::write(path, payload)?;
        Ok(())
    }

    pub fn save_to_root(&self, root: impl AsRef<Path>) -> Result<()> {
        self.save_to_path(Self::config_path(root))
    }

    pub fn validate(&self) -> Result<()> {
        validate_language_list("available_languages", &self.available_languages)?;
        if self.primary_language.trim().is_empty() {
            return Err(TransError::InvalidConfig(
                "primary_language must not be empty".to_string(),
            ));
        }
        if self.language_files_path.as_os_str().is_empty() {
            return Err(TransError::InvalidConfig(
                "language_files_path must not be empty".to_string(),
            ));
        }

        let available_set: HashSet<&str> = self
            .available_languages
            .iter()
            .map(String::as_str)
            .collect();

        if !available_set.contains(self.primary_language.as_str()) {
            return Err(TransError::InvalidConfig(
                "primary_language must be in available_languages".to_string(),
            ));
        }

        validate_language_list("required_languages", &self.required_languages)?;
        for lang in &self.required_languages {
            if !available_set.contains(lang.as_str()) {
                return Err(TransError::InvalidConfig(format!(
                    "required language '{lang}' is not in available_languages"
                )));
            }
        }

        Ok(())
    }
}

fn validate_language_list(name: &str, values: &[String]) -> Result<()> {
    if values.is_empty() {
        return Err(TransError::InvalidConfig(format!(
            "{name} must not be empty"
        )));
    }

    let mut seen = HashSet::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(TransError::InvalidConfig(format!(
                "{name} contains an empty value"
            )));
        }
        if !seen.insert(trimmed) {
            return Err(TransError::InvalidConfig(format!(
                "{name} contains duplicate '{trimmed}'"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> TransConfig {
        TransConfig {
            language_files_path: PathBuf::from("translations"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
        }
    }

    #[test]
    fn defaults_missing_untranslated_value() {
        let json = r#"
        {
            "language_files_path": "translations",
            "available_languages": ["en"],
            "required_languages": ["en"],
            "primary_language": "en"
        }
        "#;
        let config: TransConfig = serde_json::from_str(json).expect("valid json");
        assert_eq!(config.default_untranslated_value, "");
    }

    #[test]
    fn validate_requires_available_languages() {
        let mut config = base_config();
        config.available_languages.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_requires_required_languages() {
        let mut config = base_config();
        config.required_languages.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_primary_language_in_available() {
        let mut config = base_config();
        config.primary_language = "sv".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_required_languages_in_available() {
        let mut config = base_config();
        config.required_languages.push("sv".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_duplicate_languages() {
        let mut config = base_config();
        config.available_languages.push("en".to_string());
        assert!(config.validate().is_err());
    }
}
