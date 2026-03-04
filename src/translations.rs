use std::path::{Path, PathBuf};

use crate::config::TransConfig;
use crate::error::Result;
use crate::message_store::{
    FlatTranslations, NonStringValues, coerce_non_string_values, collect_non_string_values,
    load_translations_for_mode, migrate_mode, save_translations_for_mode,
};

pub type Translations = FlatTranslations;

pub fn language_file_path(root: impl AsRef<Path>, config: &TransConfig, language: &str) -> PathBuf {
    root.as_ref()
        .join(&config.language_files_path)
        .join(format!("{language}.json"))
}

pub fn load_translations(path: impl AsRef<Path>, config: &TransConfig) -> Result<Translations> {
    load_translations_for_mode(path, config.mode)
}

pub fn save_translations(
    path: impl AsRef<Path>,
    config: &TransConfig,
    translations: &Translations,
) -> Result<()> {
    save_translations_for_mode(path, config.mode, translations)
}

pub fn load_language_translations(
    root: impl AsRef<Path>,
    config: &TransConfig,
    language: &str,
) -> Result<Translations> {
    load_translations(language_file_path(root, config, language), config)
}

pub fn save_language_translations(
    root: impl AsRef<Path>,
    config: &TransConfig,
    language: &str,
    translations: &Translations,
) -> Result<()> {
    save_translations(
        language_file_path(root, config, language),
        config,
        translations,
    )
}

pub fn migrate_language_files(
    root: impl AsRef<Path>,
    config: &TransConfig,
    target_mode: crate::config::ConfigMode,
) -> Result<()> {
    migrate_mode(root.as_ref(), config, target_mode)
}

pub fn collect_non_string_leaf_values(
    root: impl AsRef<Path>,
    config: &TransConfig,
) -> Result<NonStringValues> {
    collect_non_string_values(root.as_ref(), config)
}

pub fn coerce_non_string_leaf_values(
    root: impl AsRef<Path>,
    config: &TransConfig,
) -> Result<usize> {
    coerce_non_string_values(root.as_ref(), config)
}
