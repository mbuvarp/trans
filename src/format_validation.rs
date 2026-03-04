use std::collections::{BTreeMap, HashMap};

use formatjs_icu_messageformat_parser::{MessageFormatElement, Parser, ParserOptions};

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::translations::Translations;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderKind {
    Argument,
    Number,
    Date,
    Time,
    Select,
    Plural,
    Tag,
}

#[derive(Debug, Clone)]
pub struct FormatValidationIssue {
    pub id: String,
    pub language: String,
    pub message: String,
    pub value: String,
    pub primary_value: String,
}

pub fn placeholders_for_message(
    message: &str,
) -> std::result::Result<HashMap<String, PlaceholderKind>, String> {
    let options = ParserOptions {
        requires_other_clause: true,
        ..ParserOptions::default()
    };
    let parser = Parser::new(message, options);
    let ast = parser
        .parse()
        .map_err(|err| format!("format parse error: {err}"))?;
    let mut placeholders = HashMap::new();
    collect_placeholders(&ast, &mut placeholders)?;
    Ok(placeholders)
}

pub fn compare_placeholders(
    primary_placeholders: &HashMap<String, PlaceholderKind>,
    translation: &str,
) -> std::result::Result<(), String> {
    let translation_placeholders = placeholders_for_message(translation)?;

    let mut missing = Vec::new();
    let mut extra = Vec::new();
    let mut mismatched = Vec::new();

    for (name, kind) in primary_placeholders {
        match translation_placeholders.get(name) {
            None => missing.push(name.clone()),
            Some(other_kind) if other_kind != kind => {
                mismatched.push(format!(
                    "{name} (expected {:?}, got {:?})",
                    kind, other_kind
                ));
            }
            _ => {}
        }
    }

    for name in translation_placeholders.keys() {
        if !primary_placeholders.contains_key(name) {
            extra.push(name.clone());
        }
    }

    if missing.is_empty() && extra.is_empty() && mismatched.is_empty() {
        return Ok(());
    }

    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!("missing placeholders: {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        parts.push(format!("extra placeholders: {}", extra.join(", ")));
    }
    if !mismatched.is_empty() {
        parts.push(format!("type mismatches: {}", mismatched.join(", ")));
    }

    Err(parts.join("; "))
}

pub fn validate_message_formats(
    config: &TransConfig,
    translations_by_language: &BTreeMap<String, Translations>,
) -> Result<()> {
    let issues = collect_format_validation_issues(config, translations_by_language)?;
    if issues.is_empty() {
        return Ok(());
    }

    let errors = issues
        .iter()
        .map(|issue| format!("{} ({}) - {}", issue.id, issue.language, issue.message))
        .collect::<Vec<_>>();

    Err(TransError::InvalidInput(format!(
        "format validation failed:\n- {}",
        errors.join("\n- ")
    )))
}

pub fn collect_format_validation_issues(
    config: &TransConfig,
    translations_by_language: &BTreeMap<String, Translations>,
) -> Result<Vec<FormatValidationIssue>> {
    let primary_translations = translations_by_language
        .get(&config.primary_language)
        .ok_or_else(|| {
            TransError::InvalidInput(format!(
                "missing primary language '{}' in translations",
                config.primary_language
            ))
        })?;

    let mut issues = Vec::new();

    for (id, primary_value) in primary_translations {
        let primary_placeholders = match placeholders_for_message(primary_value) {
            Ok(placeholders) => placeholders,
            Err(err) => {
                issues.push(FormatValidationIssue {
                    id: id.clone(),
                    language: config.primary_language.clone(),
                    message: err,
                    value: primary_value.clone(),
                    primary_value: primary_value.clone(),
                });
                continue;
            }
        };

        for language in &config.available_languages {
            if language == &config.primary_language {
                continue;
            }
            let translations = match translations_by_language.get(language) {
                Some(translations) => translations,
                None => continue,
            };
            let value = match translations.get(id) {
                Some(value) => value,
                None => continue,
            };
            if value == &config.default_untranslated_value {
                continue;
            }
            if let Err(err) = compare_placeholders(&primary_placeholders, value) {
                issues.push(FormatValidationIssue {
                    id: id.clone(),
                    language: language.clone(),
                    message: err,
                    value: value.clone(),
                    primary_value: primary_value.clone(),
                });
            }
        }
    }

    Ok(issues)
}

fn collect_placeholders(
    elements: &[MessageFormatElement],
    placeholders: &mut HashMap<String, PlaceholderKind>,
) -> std::result::Result<(), String> {
    for element in elements {
        match element {
            MessageFormatElement::Literal(_) | MessageFormatElement::Pound(_) => {}
            MessageFormatElement::Argument(arg) => {
                insert_placeholder(placeholders, &arg.value, PlaceholderKind::Argument)?;
            }
            MessageFormatElement::Number(arg) => {
                insert_placeholder(placeholders, &arg.value, PlaceholderKind::Number)?;
            }
            MessageFormatElement::Date(arg) => {
                insert_placeholder(placeholders, &arg.value, PlaceholderKind::Date)?;
            }
            MessageFormatElement::Time(arg) => {
                insert_placeholder(placeholders, &arg.value, PlaceholderKind::Time)?;
            }
            MessageFormatElement::Select(arg) => {
                insert_placeholder(placeholders, &arg.value, PlaceholderKind::Select)?;
                for option in arg.options.values() {
                    collect_placeholders(&option.value, placeholders)?;
                }
            }
            MessageFormatElement::Plural(arg) => {
                insert_placeholder(placeholders, &arg.value, PlaceholderKind::Plural)?;
                for option in arg.options.values() {
                    collect_placeholders(&option.value, placeholders)?;
                }
            }
            MessageFormatElement::Tag(tag) => {
                insert_placeholder(placeholders, &tag.value, PlaceholderKind::Tag)?;
                collect_placeholders(&tag.children, placeholders)?;
            }
        }
    }
    Ok(())
}

fn insert_placeholder(
    placeholders: &mut HashMap<String, PlaceholderKind>,
    name: &str,
    kind: PlaceholderKind,
) -> std::result::Result<(), String> {
    if let Some(existing) = placeholders.get(name) {
        if existing != &kind {
            return Err(format!(
                "placeholder '{name}' has conflicting types ({existing:?} vs {kind:?})"
            ));
        }
        return Ok(());
    }
    placeholders.insert(name.to_string(), kind);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> TransConfig {
        TransConfig {
            mode: crate::config::ConfigMode::ReactIntl,
            language_files_path: std::path::PathBuf::from("lang"),
            available_languages: vec!["en".to_string(), "nb".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            default_export_format: crate::config::ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            run_update_check: false,
            ai: Default::default(),
        }
    }

    #[test]
    fn parses_placeholders() {
        let placeholders =
            placeholders_for_message("{name} has {count, number} items").expect("placeholders");
        assert_eq!(placeholders.get("name"), Some(&PlaceholderKind::Argument));
        assert_eq!(placeholders.get("count"), Some(&PlaceholderKind::Number));
    }

    #[test]
    fn detects_placeholder_mismatch() {
        let primary = placeholders_for_message("{count, plural, one {# item} other {# items}}")
            .expect("primary");
        let err = compare_placeholders(&primary, "{count} item").expect_err("mismatch");
        assert!(err.contains("type mismatches"));
    }

    #[test]
    fn skips_default_untranslated_values() {
        let config = base_config();
        let mut translations = BTreeMap::new();
        let mut en = Translations::new();
        en.insert("app.items".to_string(), "{count, number} items".to_string());
        let mut nb = Translations::new();
        nb.insert(
            "app.items".to_string(),
            config.default_untranslated_value.clone(),
        );
        translations.insert("en".to_string(), en);
        translations.insert("nb".to_string(), nb);

        validate_message_formats(&config, &translations).expect("valid");
    }
}
