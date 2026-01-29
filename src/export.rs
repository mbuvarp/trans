use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use csv::Writer;
use rust_xlsxwriter::{Color, Format, Workbook};

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::translations::{Translations, load_language_translations};
use crate::verify::verify_language_files;

pub fn export_csv(root: impl AsRef<Path>, config: &TransConfig) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;

    let translations_by_language = load_all_languages(root, config)?;

    let output_path = root.join("translations.csv");
    export_csv_with_options(
        config,
        &translations_by_language,
        &config.available_languages,
        &output_path,
        false,
    )?;
    Ok(output_path)
}

pub fn export_csv_with_options(
    config: &TransConfig,
    translations_by_language: &BTreeMap<String, Translations>,
    languages: &[String],
    output_path: &Path,
    missing_only: bool,
) -> Result<()> {
    let mut writer = Writer::from_path(output_path)?;

    let mut header = Vec::with_capacity(languages.len() + 1);
    header.push("id".to_string());
    header.extend(languages.iter().cloned());
    writer.write_record(&header)?;

    let ids = message_ids(translations_by_language, &config.primary_language)?;
    for message_id in ids {
        let mut record = Vec::with_capacity(languages.len() + 1);
        record.push(message_id.clone());
        let mut has_missing = false;
        for language in languages {
            let value = translations_by_language
                .get(language)
                .and_then(|translations| translations.get(&message_id))
                .cloned()
                .unwrap_or_else(|| config.default_untranslated_value.clone());
            if value == config.default_untranslated_value {
                has_missing = true;
            }
            record.push(value);
        }
        if !missing_only || has_missing {
            writer.write_record(&record)?;
        }
    }

    writer.flush()?;
    Ok(())
}

pub fn export_csv_filtered(
    root: impl AsRef<Path>,
    config: &TransConfig,
    languages: &[String],
) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;

    let translations_by_language = load_selected_languages(root, config, languages)?;

    let output_path = root.join("translations.csv");
    export_csv_with_options(
        config,
        &translations_by_language,
        languages,
        &output_path,
        false,
    )?;
    Ok(output_path)
}

pub fn export_excel(root: impl AsRef<Path>, config: &TransConfig) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;

    let translations_by_language = load_all_languages(root, config)?;

    let output_path = root.join("translations.xlsx");
    export_excel_with_options(
        config,
        &translations_by_language,
        &config.available_languages,
        &output_path,
        false,
    )?;
    Ok(output_path)
}

pub fn export_excel_with_options(
    config: &TransConfig,
    translations_by_language: &BTreeMap<String, Translations>,
    languages: &[String],
    output_path: &Path,
    missing_only: bool,
) -> Result<()> {
    let ids = message_ids(translations_by_language, &config.primary_language)?;
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    let missing_format = Format::new().set_background_color(Color::RGB(0xFFC7CE));

    worksheet.write_string(0, 0, "id")?;
    for (idx, language) in languages.iter().enumerate() {
        worksheet.write_string(0, (idx + 1) as u16, language.as_str())?;
    }

    let mut row_index = 0usize;
    for message_id in ids {
        let mut values = Vec::with_capacity(languages.len());
        let mut has_missing = false;
        for language in languages {
            let value = translations_by_language
                .get(language)
                .and_then(|translations| translations.get(&message_id))
                .cloned()
                .unwrap_or_else(|| config.default_untranslated_value.clone());
            if value == config.default_untranslated_value {
                has_missing = true;
            }
            values.push(value);
        }
        if !missing_only || has_missing {
            let row = (row_index + 1) as u32;
            worksheet.write_string(row, 0, message_id.as_str())?;
            for (col_index, value) in values.iter().enumerate() {
                let col = (col_index + 1) as u16;
                if value == &config.default_untranslated_value {
                    worksheet.write_string_with_format(
                        row,
                        col,
                        value.as_str(),
                        &missing_format,
                    )?;
                } else {
                    worksheet.write_string(row, col, value.as_str())?;
                }
            }
            row_index += 1;
        }
    }

    workbook.save(output_path)?;
    Ok(())
}

pub fn export_excel_filtered(
    root: impl AsRef<Path>,
    config: &TransConfig,
    languages: &[String],
) -> Result<PathBuf> {
    let root = root.as_ref();
    verify_language_files(root, config)?;

    let translations_by_language = load_selected_languages(root, config, languages)?;

    let output_path = root.join("translations.xlsx");
    export_excel_with_options(
        config,
        &translations_by_language,
        languages,
        &output_path,
        false,
    )?;
    Ok(output_path)
}

pub fn load_all_languages(
    root: &Path,
    config: &TransConfig,
) -> Result<BTreeMap<String, Translations>> {
    let mut map = BTreeMap::new();
    for language in &config.available_languages {
        let translations = load_language_translations(root, config, language)?;
        map.insert(language.clone(), translations);
    }
    Ok(map)
}

pub fn load_selected_languages(
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
