use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use csv::Writer;
use rust_xlsxwriter::Workbook;

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::translations::{Translations, load_language_translations};
use crate::verify::verify_language_files;

pub fn export_csv(root: impl AsRef<Path>, config: &TransConfig) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;

    let translations_by_language = load_all_languages(root, config)?;
    let ids = message_ids(&translations_by_language, &config.primary_language)?;

    let output_path = root.join("translations.csv");
    let mut writer = Writer::from_path(&output_path)?;

    let mut header = Vec::with_capacity(config.available_languages.len() + 1);
    header.push("id".to_string());
    header.extend(config.available_languages.iter().cloned());
    writer.write_record(&header)?;

    for message_id in ids {
        let mut record = Vec::with_capacity(config.available_languages.len() + 1);
        record.push(message_id.clone());
        for language in &config.available_languages {
            let value = translations_by_language
                .get(language)
                .and_then(|translations| translations.get(&message_id))
                .cloned()
                .unwrap_or_default();
            record.push(value);
        }
        writer.write_record(&record)?;
    }

    writer.flush()?;
    Ok(output_path)
}

pub fn export_csv_filtered(
    root: impl AsRef<Path>,
    config: &TransConfig,
    languages: &[String],
) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;
    ensure_languages_available(config, languages)?;

    let translations_by_language = load_selected_languages(root, config, languages)?;
    let ids = message_ids(&translations_by_language, &config.primary_language)?;

    let output_path = root.join("translations.csv");
    let mut writer = Writer::from_path(&output_path)?;

    let mut header = Vec::with_capacity(languages.len() + 1);
    header.push("id".to_string());
    header.extend(languages.iter().cloned());
    writer.write_record(&header)?;

    for message_id in ids {
        let mut record = Vec::with_capacity(languages.len() + 1);
        record.push(message_id.clone());
        for language in languages {
            let value = translations_by_language
                .get(language)
                .and_then(|translations| translations.get(&message_id))
                .cloned()
                .unwrap_or_default();
            record.push(value);
        }
        writer.write_record(&record)?;
    }

    writer.flush()?;
    Ok(output_path)
}

pub fn export_excel(root: impl AsRef<Path>, config: &TransConfig) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;

    let translations_by_language = load_all_languages(root, config)?;
    let ids = message_ids(&translations_by_language, &config.primary_language)?;

    let output_path = root.join("translations.xlsx");
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();

    worksheet.write_string(0, 0, "id")?;
    for (idx, language) in config.available_languages.iter().enumerate() {
        worksheet.write_string(0, (idx + 1) as u16, language.as_str())?;
    }

    for (row_index, message_id) in ids.iter().enumerate() {
        let row = (row_index + 1) as u32;
        worksheet.write_string(row, 0, message_id.as_str())?;
        for (col_index, language) in config.available_languages.iter().enumerate() {
            let value = translations_by_language
                .get(language)
                .and_then(|translations| translations.get(message_id))
                .cloned()
                .unwrap_or_default();
            worksheet.write_string(row, (col_index + 1) as u16, value.as_str())?;
        }
    }

    workbook.save(&output_path)?;
    Ok(output_path)
}

pub fn export_excel_filtered(
    root: impl AsRef<Path>,
    config: &TransConfig,
    languages: &[String],
) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;
    ensure_languages_available(config, languages)?;

    let translations_by_language = load_selected_languages(root, config, languages)?;
    let ids = message_ids(&translations_by_language, &config.primary_language)?;

    let output_path = root.join("translations.xlsx");
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();

    worksheet.write_string(0, 0, "id")?;
    for (idx, language) in languages.iter().enumerate() {
        worksheet.write_string(0, (idx + 1) as u16, language.as_str())?;
    }

    for (row_index, message_id) in ids.iter().enumerate() {
        let row = (row_index + 1) as u32;
        worksheet.write_string(row, 0, message_id.as_str())?;
        for (col_index, language) in languages.iter().enumerate() {
            let value = translations_by_language
                .get(language)
                .and_then(|translations| translations.get(message_id))
                .cloned()
                .unwrap_or_default();
            worksheet.write_string(row, (col_index + 1) as u16, value.as_str())?;
        }
    }

    workbook.save(&output_path)?;
    Ok(output_path)
}

fn load_all_languages(root: &Path, config: &TransConfig) -> Result<BTreeMap<String, Translations>> {
    let mut map = BTreeMap::new();
    for language in &config.available_languages {
        let translations = load_language_translations(root, config, language)?;
        map.insert(language.clone(), translations);
    }
    Ok(map)
}

fn load_selected_languages(
    root: &Path,
    config: &TransConfig,
    languages: &[String],
) -> Result<BTreeMap<String, Translations>> {
    let mut map = BTreeMap::new();
    for language in languages {
        let translations = load_language_translations(root, config, language)?;
        map.insert(language.clone(), translations);
    }
    Ok(map)
}

fn ensure_languages_available(config: &TransConfig, languages: &[String]) -> Result<()> {
    for language in languages {
        if !config
            .available_languages
            .iter()
            .any(|lang| lang == language)
        {
            return Err(TransError::InvalidInput(format!(
                "language '{language}' is not in available_languages"
            )));
        }
    }

    if !languages
        .iter()
        .any(|lang| lang == &config.primary_language)
    {
        return Err(TransError::InvalidInput(format!(
            "export languages must include primary language '{}'",
            config.primary_language
        )));
    }

    Ok(())
}

fn message_ids(
    translations_by_language: &BTreeMap<String, Translations>,
    primary_language: &str,
) -> Result<Vec<String>> {
    let primary = translations_by_language
        .get(primary_language)
        .ok_or_else(|| {
            TransError::InvalidInput(format!(
                "missing primary language '{primary_language}' for export"
            ))
        })?;
    Ok(primary.keys().cloned().collect())
}
