use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::TransConfig;
use crate::error::{Result, TransError};

pub type Translations = BTreeMap<String, String>;

pub fn language_file_path(
    root: impl AsRef<Path>,
    config: &TransConfig,
    language: &str,
) -> PathBuf {
    root.as_ref()
        .join(&config.language_files_path)
        .join(format!("{language}.json"))
}

pub fn load_translations(path: impl AsRef<Path>) -> Result<Translations> {
    let path = path.as_ref();
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(TransError::MissingLanguageFile(path.to_path_buf()));
        }
        Err(err) => return Err(err.into()),
    };

    let translations: Translations = serde_json::from_str(&contents)?;
    Ok(translations)
}

pub fn save_translations(path: impl AsRef<Path>, translations: &Translations) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(translations)?;
    fs::write(path, payload)?;
    Ok(())
}

pub fn load_language_translations(
    root: impl AsRef<Path>,
    config: &TransConfig,
    language: &str,
) -> Result<Translations> {
    load_translations(language_file_path(root, config, language))
}

pub fn save_language_translations(
    root: impl AsRef<Path>,
    config: &TransConfig,
    language: &str,
    translations: &Translations,
) -> Result<()> {
    save_translations(language_file_path(root, config, language), translations)
}
