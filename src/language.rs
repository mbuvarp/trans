use std::str::FromStr;

use codes_iso_15924::ScriptCode;
use iso3166::Country;
use isolang::Language;

#[derive(Debug, Clone)]
pub struct LanguageTag {
    pub language: String,
    pub region: Option<String>,
    pub script: Option<String>,
}

pub fn is_valid_language_code(code: &str) -> bool {
    parse_language_tag(code).is_some()
}

pub fn language_display_name(code: &str) -> String {
    let Some(tag) = parse_language_tag(code) else {
        return code.to_string();
    };
    let Some(language) = Language::from_639_1(&tag.language) else {
        return code.to_string();
    };
    let mut name = language.to_name().to_string();
    if let Some(script) = tag.script.as_deref()
        && let Ok(script_code) = ScriptCode::from_str(script)
    {
        name.push_str(" (");
        name.push_str(script_code.name());
        name.push(')');
        return name;
    }
    if let Some(region) = tag.region.as_deref()
        && let Some(country) = Country::from_alpha2_ignore_case(region)
    {
        name.push_str(" (");
        name.push_str(country.name);
        name.push(')');
    }
    name
}

fn parse_language_tag(code: &str) -> Option<LanguageTag> {
    let trimmed = code.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() > 2 {
        return None;
    }
    let lang = parts[0];
    if lang.len() != 2 || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let language = lang.to_ascii_lowercase();
    Language::from_639_1(&language)?;

    let mut region = None;
    let mut script = None;
    if parts.len() == 2 {
        let subtag = parts[1];
        if subtag.len() == 2 && subtag.chars().all(|c| c.is_ascii_alphabetic()) {
            region = Some(subtag.to_ascii_uppercase());
            Country::from_alpha2_ignore_case(subtag)?;
        } else if subtag.len() == 4 && subtag.chars().all(|c| c.is_ascii_alphabetic()) {
            let normalized = normalize_script(subtag);
            if ScriptCode::from_str(&normalized).is_err() {
                return None;
            }
            script = Some(normalized);
        } else {
            return None;
        }
    }

    Some(LanguageTag {
        language,
        region,
        script,
    })
}

fn normalize_script(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut output = String::new();
    output.push(first.to_ascii_uppercase());
    for ch in chars {
        output.push(ch.to_ascii_lowercase());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_iso_639_1_codes() {
        assert!(is_valid_language_code("en"));
        assert!(is_valid_language_code("nb"));
        assert!(!is_valid_language_code("eng"));
    }

    #[test]
    fn accepts_language_region() {
        assert!(is_valid_language_code("en-GB"));
        assert!(is_valid_language_code("pt-br"));
        assert!(!is_valid_language_code("en-999"));
    }

    #[test]
    fn accepts_language_script() {
        assert!(is_valid_language_code("zh-Hans"));
        assert!(is_valid_language_code("sr-Latn"));
        assert!(!is_valid_language_code("zh-Han"));
    }
}
