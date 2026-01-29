use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use calamine::{Data, Reader, open_workbook_auto};
use clap::ValueEnum;
use console::style;
use dialoguer::Select;

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::export::load_selected_languages;
use crate::format_validation::validate_message_formats;
use crate::translations::save_language_translations;
use crate::verify::verify_language_files;
use crate::verify_ai::apply_format_fixes_with_ai;

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraLangsStrategy {
    #[value(name = "ignore")]
    Ignore,
    #[value(name = "create")]
    Create,
    #[value(name = "abort")]
    Abort,
}

pub fn import_translations(
    root: &Path,
    config: &TransConfig,
    path: &Path,
    lang_filter: Option<Vec<String>>,
    extra_langs: Option<ExtraLangsStrategy>,
    trim: bool,
) -> Result<()> {
    import_translations_with_ai(root, config, path, lang_filter, extra_langs, trim, false)
}

pub fn import_translations_with_ai(
    root: &Path,
    config: &TransConfig,
    path: &Path,
    lang_filter: Option<Vec<String>>,
    extra_langs: Option<ExtraLangsStrategy>,
    trim: bool,
    use_ai: bool,
) -> Result<()> {
    verify_language_files(root, config)?;
    let mut config = config.clone();
    let import_path = resolve_import_path(root, path);
    let import_data = read_import_file(&import_path, trim)?;
    let header_languages = import_data.languages.clone();

    let existing_languages = config.available_languages.clone();
    let mut config_changed = false;
    let extra_languages: Vec<String> = header_languages
        .iter()
        .filter(|lang| !config.available_languages.contains(lang))
        .cloned()
        .collect();

    let strategy = if extra_languages.is_empty() {
        None
    } else {
        Some(match extra_langs {
            Some(choice) => choice,
            None => prompt_extra_languages(&extra_languages)?,
        })
    };

    if let Some(strategy) = strategy {
        match strategy {
            ExtraLangsStrategy::Abort => {
                return Err(TransError::InvalidInput(
                    "import aborted due to extra languages".to_string(),
                ));
            }
            ExtraLangsStrategy::Ignore => {
                println!(
                    "Warning: ignoring extra languages: {}",
                    extra_languages.join(", ")
                );
            }
            ExtraLangsStrategy::Create => {
                config_changed = true;
            }
        }
    }

    let ignore_extra = matches!(strategy, Some(ExtraLangsStrategy::Ignore));
    let mut languages_to_import = if ignore_extra {
        header_languages
            .iter()
            .filter(|lang| !extra_languages.contains(*lang))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        header_languages.clone()
    };

    if let Some(filter) = lang_filter {
        let filter_set: HashSet<String> = filter.iter().cloned().collect();
        let mut missing_from_import = Vec::new();
        for lang in &filter {
            if !languages_to_import.contains(lang) {
                missing_from_import.push(lang.clone());
            }
        }
        if !missing_from_import.is_empty() {
            println!(
                "Warning: requested languages not in import file: {}",
                missing_from_import.join(", ")
            );
        }
        languages_to_import.retain(|lang| filter_set.contains(lang));
    }

    languages_to_import.retain(|lang| lang != &config.primary_language);
    if languages_to_import.is_empty() {
        println!(
            "Warning: no non-primary languages to import for '{}'.",
            config.primary_language
        );
        return Ok(());
    }

    let mut translations_by_language = load_selected_languages(root, &config, &existing_languages)?;
    let primary_ids = translations_by_language
        .get(&config.primary_language)
        .ok_or_else(|| {
            TransError::InvalidInput(format!(
                "missing primary language '{}' in translations",
                config.primary_language
            ))
        })?
        .keys()
        .cloned()
        .collect::<Vec<_>>();

    let mut missing_by_language: HashMap<String, Vec<String>> = HashMap::new();
    for row in &import_data.rows {
        for language in &existing_languages {
            let translations = translations_by_language.get(language);
            let exists = translations
                .and_then(|translations| translations.get(&row.id))
                .is_some();
            if !exists {
                missing_by_language
                    .entry(language.clone())
                    .or_default()
                    .push(row.id.clone());
            }
        }
    }

    if !missing_by_language.is_empty() {
        let mut lines = Vec::new();
        for (language, ids) in missing_by_language {
            lines.push(format!("{language}: {}", ids.join(", ")));
        }
        return Err(TransError::InvalidInput(format!(
            "import ids missing in language files:\n- {}",
            lines.join("\n- ")
        )));
    }

    let mut languages_to_save: HashSet<String> = HashSet::new();
    if matches!(strategy, Some(ExtraLangsStrategy::Create)) {
        for language in &extra_languages {
            if translations_by_language.contains_key(language) {
                continue;
            }
            let mut translations = BTreeMap::new();
            for id in &primary_ids {
                translations.insert(id.clone(), config.default_untranslated_value.clone());
            }
            translations_by_language.insert(language.clone(), translations);
            languages_to_save.insert(language.clone());
        }
        for language in &extra_languages {
            if !config.available_languages.contains(language) {
                config.available_languages.push(language.clone());
            }
        }
    }

    let language_index = build_language_index(&import_data.languages)?;
    for row in &import_data.rows {
        for language in &languages_to_import {
            let idx = match language_index.get(language) {
                Some(idx) => *idx,
                None => continue,
            };
            let value = row.values.get(idx).cloned().unwrap_or_default();
            if value == config.default_untranslated_value {
                continue;
            }
            if let Some(translations) = translations_by_language.get_mut(language) {
                if let Some(existing) = translations.get_mut(&row.id) {
                    if existing != &value {
                        *existing = value;
                        languages_to_save.insert(language.clone());
                    }
                }
            }
        }
    }

    if languages_to_save.is_empty() && !config_changed {
        println!("No translations updated.");
        return Ok(());
    }

    if use_ai {
        apply_format_fixes_with_ai(
            root,
            &config,
            &mut translations_by_language,
            &mut languages_to_save,
        )?;
    } else {
        validate_message_formats(&config, &translations_by_language)?;
    }

    for language in &languages_to_save {
        if let Some(translations) = translations_by_language.get(language) {
            save_language_translations(root, &config, language, translations)?;
        }
    }

    if config_changed {
        config.save_to_root(root)?;
        println!("Added languages to config: {}", extra_languages.join(", "));
    }

    println!("Import complete.");
    Ok(())
}

struct ImportData {
    languages: Vec<String>,
    rows: Vec<ImportRow>,
}

struct ImportRow {
    id: String,
    values: Vec<String>,
}

fn resolve_import_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn read_import_file(path: &Path, trim: bool) -> Result<ImportData> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "csv" => read_csv(path, trim),
        "xlsx" => read_xlsx(path, trim),
        _ => Err(TransError::InvalidInput(
            "import file must be a .csv or .xlsx file".to_string(),
        )),
    }
}

fn read_csv(path: &Path, trim: bool) -> Result<ImportData> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_path(path)?;
    let headers = reader
        .headers()
        .map_err(|err| TransError::InvalidInput(format!("failed to read csv headers: {err}")))?;
    if headers.is_empty() {
        return Err(TransError::InvalidInput("csv file is empty".to_string()));
    }
    let (languages, header_len) = parse_header(headers.iter().map(|value| value.to_string()))?;
    let mut rows = Vec::new();
    let mut duplicates = HashSet::new();
    let mut seen = HashSet::new();

    for record in reader.records() {
        let record = record?;
        if record.len() > header_len {
            return Err(TransError::InvalidInput(
                "csv row has more columns than header".to_string(),
            ));
        }
        let mut cells = Vec::with_capacity(header_len);
        for idx in 0..header_len {
            cells.push(record.get(idx).unwrap_or("").to_string());
        }
        if cells.iter().all(|value| value.trim().is_empty()) {
            continue;
        }
        let id = cells[0].trim().to_string();
        if id.is_empty() {
            return Err(TransError::InvalidInput(
                "csv row is missing an id value".to_string(),
            ));
        }
        if !seen.insert(id.clone()) {
            duplicates.insert(id.clone());
        }
        let values = cells[1..]
            .iter()
            .map(|value| {
                if trim {
                    value.trim().to_string()
                } else {
                    value.clone()
                }
            })
            .collect::<Vec<_>>();
        rows.push(ImportRow { id, values });
    }

    if !duplicates.is_empty() {
        let mut list: Vec<String> = duplicates.into_iter().collect();
        list.sort();
        return Err(TransError::InvalidInput(format!(
            "duplicate ids found in import file: {}",
            list.join(", ")
        )));
    }

    Ok(ImportData { languages, rows })
}

fn read_xlsx(path: &Path, trim: bool) -> Result<ImportData> {
    let mut workbook = open_workbook_auto(path)
        .map_err(|err| TransError::InvalidInput(format!("failed to open xlsx: {err}")))?;
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| TransError::InvalidInput("xlsx file has no sheets".to_string()))?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|err| TransError::InvalidInput(format!("failed to read worksheet: {err}")))?;

    let mut rows_iter = range.rows();
    let header_row = rows_iter
        .next()
        .ok_or_else(|| TransError::InvalidInput("xlsx file is empty".to_string()))?;
    let mut header_cells = header_row.iter().map(cell_to_string).collect::<Vec<_>>();
    while header_cells
        .last()
        .map(|value| value.trim().is_empty())
        .unwrap_or(false)
    {
        header_cells.pop();
    }
    let (languages, header_len) = parse_header(header_cells.into_iter())?;

    let mut rows = Vec::new();
    let mut duplicates = HashSet::new();
    let mut seen = HashSet::new();

    for row in rows_iter {
        if row.len() > header_len {
            let extra_has_content = row[header_len..]
                .iter()
                .any(|value| !cell_to_string(value).trim().is_empty());
            if extra_has_content {
                return Err(TransError::InvalidInput(
                    "xlsx row has more columns than header".to_string(),
                ));
            }
        }

        let mut cells = Vec::with_capacity(header_len);
        for idx in 0..header_len {
            cells.push(row.get(idx).map(cell_to_string).unwrap_or_default());
        }
        if cells.iter().all(|value| value.trim().is_empty()) {
            continue;
        }
        let id = cells[0].trim().to_string();
        if id.is_empty() {
            return Err(TransError::InvalidInput(
                "xlsx row is missing an id value".to_string(),
            ));
        }
        if !seen.insert(id.clone()) {
            duplicates.insert(id.clone());
        }
        let values = cells[1..]
            .iter()
            .map(|value| {
                if trim {
                    value.trim().to_string()
                } else {
                    value.clone()
                }
            })
            .collect::<Vec<_>>();
        rows.push(ImportRow { id, values });
    }

    if !duplicates.is_empty() {
        let mut list: Vec<String> = duplicates.into_iter().collect();
        list.sort();
        return Err(TransError::InvalidInput(format!(
            "duplicate ids found in import file: {}",
            list.join(", ")
        )));
    }

    Ok(ImportData { languages, rows })
}

fn parse_header<I>(values: I) -> Result<(Vec<String>, usize)>
where
    I: IntoIterator<Item = String>,
{
    let mut headers: Vec<String> = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .collect();
    if headers.is_empty() {
        return Err(TransError::InvalidInput("missing header row".to_string()));
    }
    if headers[0] != "id" {
        return Err(TransError::InvalidInput(
            "first column must be 'id'".to_string(),
        ));
    }
    if headers.len() < 2 {
        return Err(TransError::InvalidInput(
            "import file must include at least one language column".to_string(),
        ));
    }

    headers.remove(0);
    let mut seen = HashSet::new();
    for lang in &headers {
        if lang.is_empty() {
            return Err(TransError::InvalidInput(
                "language header cannot be empty".to_string(),
            ));
        }
        if !seen.insert(lang.clone()) {
            return Err(TransError::InvalidInput(format!(
                "duplicate language '{lang}' in header"
            )));
        }
    }
    let header_len = headers.len();
    Ok((headers, header_len + 1))
}

fn build_language_index(languages: &[String]) -> Result<HashMap<String, usize>> {
    let mut map = HashMap::new();
    for (idx, language) in languages.iter().enumerate() {
        if map.insert(language.clone(), idx).is_some() {
            return Err(TransError::InvalidInput(format!(
                "duplicate language '{language}' in header"
            )));
        }
    }
    Ok(map)
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(value) => value.clone(),
        Data::Float(value) => value.to_string(),
        Data::Int(value) => value.to_string(),
        Data::Bool(value) => value.to_string(),
        Data::Error(value) => format!("{value:?}"),
        Data::DateTime(value) => value.to_string(),
        Data::DateTimeIso(value) => value.clone(),
        Data::DurationIso(value) => value.clone(),
    }
}

fn prompt_extra_languages(languages: &[String]) -> Result<ExtraLangsStrategy> {
    println!("{}", style("Additional languages found in import:").bold());
    println!("{}", languages.join(", "));
    let options = [
        "Ignore extra languages",
        "Create extra languages",
        "Abort import",
    ];
    let selection = Select::new()
        .with_prompt(">")
        .items(&options)
        .default(0)
        .interact()?;
    println!();
    Ok(match selection {
        0 => ExtraLangsStrategy::Ignore,
        1 => ExtraLangsStrategy::Create,
        _ => ExtraLangsStrategy::Abort,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    use crate::config::ExportFormat;
    use crate::translations::{
        Translations, load_language_translations, save_language_translations,
    };

    fn base_config() -> TransConfig {
        TransConfig {
            language_files_path: std::path::PathBuf::from("lang"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            default_export_format: ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            ai: Default::default(),
        }
    }

    #[test]
    fn imports_csv_updates_non_primary_languages() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let config = base_config();

        let mut en = Translations::new();
        en.insert("app.title".to_string(), "Title".to_string());
        let mut nb = Translations::new();
        nb.insert("app.title".to_string(), "Tittel".to_string());
        save_language_translations(root, &config, "en", &en).expect("save en");
        save_language_translations(root, &config, "nb", &nb).expect("save nb");

        let csv_path = root.join("import.csv");
        let contents = "id,en,nb\napp.title,Title,Oppdatert\n";
        std::fs::write(&csv_path, contents).expect("write csv");

        import_translations(
            root,
            &config,
            Path::new("import.csv"),
            None,
            Some(ExtraLangsStrategy::Ignore),
            false,
        )
        .expect("import");

        let nb_updated = load_language_translations(root, &config, "nb").expect("load nb");
        assert_eq!(nb_updated.get("app.title"), Some(&"Oppdatert".to_string()));
    }
}
