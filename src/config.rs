use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use console::style;
use serde::{Deserialize, Serialize};

use crate::error::{Result, TransError};

fn default_untranslated_value() -> String {
    String::new()
}

fn default_ai_enabled() -> bool {
    true
}

fn default_ai_model() -> String {
    "gpt-5-mini".to_string()
}

fn default_ai_api_key_env() -> String {
    "OPENAI_API_KEY".to_string()
}

fn default_ai_max_output_tokens() -> u32 {
    128
}

fn default_ai_concurrency() -> usize {
    2
}

fn default_export_format() -> ExportFormat {
    ExportFormat::Excel
}

fn default_mode() -> ConfigMode {
    ConfigMode::ReactIntl
}

fn default_excel_password() -> String {
    "unlock".to_string()
}

fn default_run_update_check() -> bool {
    false
}

const CONFIG_JSON_FILE_NAME: &str = ".trans.config.json";
const CONFIG_YAML_FILE_NAME: &str = ".trans.config.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Yaml,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Excel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigMode {
    ReactIntl,
    NextIntl,
}

impl ConfigMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigMode::ReactIntl => "react-intl",
            ConfigMode::NextIntl => "next-intl",
        }
    }
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Excel => "excel",
        }
    }
}

impl ConfigFormat {
    pub fn file_name(self) -> &'static str {
        match self {
            ConfigFormat::Json => CONFIG_JSON_FILE_NAME,
            ConfigFormat::Yaml => CONFIG_YAML_FILE_NAME,
        }
    }

    pub fn other(self) -> Self {
        match self {
            ConfigFormat::Json => ConfigFormat::Yaml,
            ConfigFormat::Yaml => ConfigFormat::Json,
        }
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("json") => Ok(ConfigFormat::Json),
            Some("yaml") | Some("yml") => Ok(ConfigFormat::Yaml),
            _ => Err(TransError::InvalidConfig(format!(
                "unsupported config format for path {}",
                path.display()
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransConfig {
    #[serde(default = "default_mode")]
    pub mode: ConfigMode,
    pub language_files_path: PathBuf,
    pub available_languages: Vec<String>,
    pub required_languages: Vec<String>,
    pub primary_language: String,
    #[serde(default = "default_untranslated_value")]
    pub default_untranslated_value: String,
    #[serde(default = "default_export_format")]
    pub default_export_format: ExportFormat,
    #[serde(default = "default_excel_password")]
    pub excel_password: String,
    #[serde(default = "default_run_update_check")]
    pub run_update_check: bool,
    #[serde(default)]
    pub ai: Option<AiConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    #[serde(default = "default_ai_enabled")]
    pub enabled: bool,
    #[serde(default = "default_ai_model")]
    pub model: String,
    #[serde(default = "default_ai_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_ai_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_ai_concurrency")]
    pub concurrency: usize,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: default_ai_enabled(),
            model: default_ai_model(),
            api_key_env: default_ai_api_key_env(),
            max_output_tokens: default_ai_max_output_tokens(),
            concurrency: default_ai_concurrency(),
        }
    }
}

impl TransConfig {
    pub fn config_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(CONFIG_JSON_FILE_NAME)
    }

    pub fn config_path_for_format(root: impl AsRef<Path>, format: ConfigFormat) -> PathBuf {
        root.as_ref().join(format.file_name())
    }

    pub fn config_paths(root: impl AsRef<Path>) -> (PathBuf, PathBuf) {
        (
            Self::config_path_for_format(&root, ConfigFormat::Json),
            Self::config_path_for_format(root, ConfigFormat::Yaml),
        )
    }

    pub fn find_root(start: impl AsRef<Path>) -> Result<PathBuf> {
        let start = start.as_ref();
        for root in start.ancestors() {
            let (json_path, yaml_path) = Self::config_paths(root);
            match (json_path.exists(), yaml_path.exists()) {
                (true, true) => {
                    return Err(TransError::InvalidConfig(
                        "both .trans.config.json and .trans.config.yaml exist; keep only one"
                            .to_string(),
                    ));
                }
                (true, false) | (false, true) => return Ok(root.to_path_buf()),
                (false, false) => {}
            }
        }

        Err(TransError::MissingConfig(
            ".trans.config.json or .trans.config.yaml".to_string(),
        ))
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let format = ConfigFormat::from_path(path)?;
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(TransError::MissingConfig(path.display().to_string()));
            }
            Err(err) => return Err(err.into()),
        };

        let config: TransConfig = match format {
            ConfigFormat::Json => serde_json::from_str(&contents)?,
            ConfigFormat::Yaml => serde_yaml::from_str(&contents)?,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn load_from_root(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::load_from_root_with_path(root)?.0)
    }

    pub fn load_from_root_with_path(root: impl AsRef<Path>) -> Result<(Self, PathBuf)> {
        let (json_path, yaml_path) = Self::config_paths(root);
        let json_exists = json_path.exists();
        let yaml_exists = yaml_path.exists();
        match (json_exists, yaml_exists) {
            (true, true) => Err(TransError::InvalidConfig(
                "both .trans.config.json and .trans.config.yaml exist; keep only one".to_string(),
            )),
            (true, false) => Ok((Self::load_from_path(&json_path)?, json_path)),
            (false, true) => Ok((Self::load_from_path(&yaml_path)?, yaml_path)),
            (false, false) => Err(TransError::MissingConfig(format!(
                "{} or {}",
                json_path.display(),
                yaml_path.display()
            ))),
        }
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let format = ConfigFormat::from_path(path)?;
        match format {
            ConfigFormat::Json => {
                let payload = serde_json::to_string_pretty(self)?;
                fs::write(path, payload)?;
            }
            ConfigFormat::Yaml => {
                let payload = serde_yaml::to_string(self)?;
                fs::write(path, payload)?;
            }
        }
        Ok(())
    }

    pub fn save_to_root(&self, root: impl AsRef<Path>) -> Result<()> {
        let (json_path, yaml_path) = Self::config_paths(root.as_ref());
        let json_exists = json_path.exists();
        let yaml_exists = yaml_path.exists();
        let target = match (json_exists, yaml_exists) {
            (true, true) => {
                return Err(TransError::InvalidConfig(
                    "both .trans.config.json and .trans.config.yaml exist; keep only one"
                        .to_string(),
                ));
            }
            (false, true) => yaml_path,
            _ => json_path,
        };
        self.save_to_path(target)
    }

    pub fn save_to_root_format(
        &self,
        root: impl AsRef<Path>,
        format: ConfigFormat,
        remove_other: bool,
    ) -> Result<PathBuf> {
        let target = Self::config_path_for_format(&root, format);
        self.save_to_path(&target)?;
        if remove_other {
            let other = Self::config_path_for_format(root, format.other());
            if other.exists() {
                fs::remove_file(other)?;
            }
        }
        Ok(target)
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

        if let Some(ai) = &self.ai {
            if ai.model.trim().is_empty() {
                return Err(TransError::InvalidConfig(
                    "ai.model must not be empty".to_string(),
                ));
            }
            if ai.api_key_env.trim().is_empty() {
                return Err(TransError::InvalidConfig(
                    "ai.apiKeyEnv must not be empty".to_string(),
                ));
            }
            validate_ai_concurrency(ai.concurrency)?;
        }

        Ok(())
    }
}

pub fn format_config_list(config: &TransConfig, config_path: Option<&Path>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(path) = config_path {
        lines.push(format_label_value(
            "configPath",
            &path.display().to_string(),
        ));
    }
    lines.push(format_label_value("mode", config.mode.as_str()));
    lines.push(format_label_value(
        "languageFilesPath",
        &config.language_files_path.display().to_string(),
    ));
    lines.push(format_label_value(
        "availableLanguages",
        &config.available_languages.join(", "),
    ));
    lines.push(format_label_value(
        "requiredLanguages",
        &config.required_languages.join(", "),
    ));
    lines.push(format_label_value(
        "primaryLanguage",
        &config.primary_language,
    ));
    lines.push(format_label_value(
        "defaultUntranslatedValue",
        &format_value(&config.default_untranslated_value),
    ));
    lines.push(format_label_value(
        "defaultExportFormat",
        config.default_export_format.as_str(),
    ));
    lines.push(format_label_value(
        "excelPassword",
        &format_value(&config.excel_password),
    ));
    lines.push(format_label_value(
        "runUpdateCheck",
        &config.run_update_check.to_string(),
    ));

    let ai = config.ai.clone().unwrap_or_default();
    lines.push(format_label_value("ai.enabled", &ai.enabled.to_string()));
    lines.push(format_label_value("ai.model", &ai.model));
    lines.push(format_label_value("ai.apiKeyEnv", &ai.api_key_env));
    lines.push(format_label_value(
        "ai.maxOutputTokens",
        &ai.max_output_tokens.to_string(),
    ));
    lines.push(format_label_value(
        "ai.concurrency",
        &ai.concurrency.to_string(),
    ));
    lines
}

#[derive(Debug, Clone, Copy)]
pub enum ConfigField {
    Mode,
    LanguageFilesPath,
    AvailableLanguages,
    RequiredLanguages,
    PrimaryLanguage,
    DefaultUntranslatedValue,
    DefaultExportFormat,
    ExcelPassword,
    RunUpdateCheck,
    AiEnabled,
    AiModel,
    AiApiKeyEnv,
    AiMaxOutputTokens,
    AiConcurrency,
}

fn format_value(value: &str) -> String {
    if value.is_empty() {
        "<empty>".to_string()
    } else {
        value.to_string()
    }
}

fn format_label_value(label: &str, value: &str) -> String {
    format!("{}: {}", style(label).bold(), value)
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

fn validate_ai_concurrency(value: usize) -> Result<()> {
    if value == 0 {
        return Err(TransError::InvalidConfig(
            "ai.concurrency must be at least 1".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn base_config() -> TransConfig {
        TransConfig {
            mode: ConfigMode::ReactIntl,
            language_files_path: PathBuf::from("translations"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            default_export_format: ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            run_update_check: false,
            ai: None,
        }
    }

    #[test]
    fn defaults_missing_untranslated_value() {
        let json = r#"
        {
            "languageFilesPath": "translations",
            "availableLanguages": ["en"],
            "requiredLanguages": ["en"],
            "primaryLanguage": "en"
        }
        "#;
        let config: TransConfig = serde_json::from_str(json).expect("valid json");
        assert_eq!(config.mode, ConfigMode::ReactIntl);
        assert_eq!(config.default_untranslated_value, "");
        assert_eq!(config.default_export_format, ExportFormat::Excel);
        assert_eq!(config.excel_password, "unlock");
        assert!(!config.run_update_check);
        assert!(config.ai.is_none());
    }

    #[test]
    fn save_to_path_places_mode_first_in_json() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let path = dir.path().join(".trans.config.json");
        config.save_to_path(&path).expect("save");
        let payload = fs::read_to_string(path).expect("read");
        assert!(
            payload
                .trim_start()
                .starts_with("{\n  \"mode\": \"react-intl\"")
        );
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

    #[test]
    fn load_from_root_prefers_yaml_when_present() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let yaml_path = TransConfig::config_path_for_format(dir.path(), ConfigFormat::Yaml);
        config.save_to_path(&yaml_path).expect("save yaml");

        let loaded = TransConfig::load_from_root(dir.path()).expect("load");
        assert_eq!(loaded.primary_language, "en");
    }

    #[test]
    fn find_root_finds_json_in_starting_directory() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        config.save_to_root(dir.path()).expect("save config");

        let root = TransConfig::find_root(dir.path()).expect("find root");
        assert_eq!(root, dir.path());
    }

    #[test]
    fn find_root_finds_yaml_in_ancestor_directory() {
        let dir = tempdir().expect("tempdir");
        let child = dir.path().join("src/components");
        fs::create_dir_all(&child).expect("mkdir");
        let config = base_config();
        let yaml_path = TransConfig::config_path_for_format(dir.path(), ConfigFormat::Yaml);
        config.save_to_path(&yaml_path).expect("save yaml");

        let root = TransConfig::find_root(&child).expect("find root");
        assert_eq!(root, dir.path());
    }

    #[test]
    fn find_root_errors_when_no_config_exists() {
        let dir = tempdir().expect("tempdir");
        let err = TransConfig::find_root(dir.path()).expect_err("missing config");

        assert!(matches!(err, TransError::MissingConfig(_)));
    }

    #[test]
    fn find_root_errors_when_both_config_formats_exist() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        config
            .save_to_path(TransConfig::config_path_for_format(
                dir.path(),
                ConfigFormat::Json,
            ))
            .expect("save json");
        config
            .save_to_path(TransConfig::config_path_for_format(
                dir.path(),
                ConfigFormat::Yaml,
            ))
            .expect("save yaml");

        let err = TransConfig::find_root(dir.path()).expect_err("duplicate config");
        assert!(matches!(err, TransError::InvalidConfig(_)));
    }

    #[test]
    fn save_to_root_format_removes_other_file() {
        let dir = tempdir().expect("tempdir");
        let config = base_config();
        let json_path = TransConfig::config_path_for_format(dir.path(), ConfigFormat::Json);
        config.save_to_path(&json_path).expect("save json");

        let yaml_path = config
            .save_to_root_format(dir.path(), ConfigFormat::Yaml, true)
            .expect("save yaml");
        assert!(yaml_path.exists());
        assert!(!json_path.exists());
    }
}
