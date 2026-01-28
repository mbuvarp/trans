use std::env;
use std::path::Path;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::TransConfig;
use crate::error::{Result, TransError};

#[derive(Debug, Clone)]
pub struct AiSettings {
    pub model: String,
    pub api_key: String,
    pub max_output_tokens: u32,
}

pub fn resolve_ai_settings(root: &Path, config: &TransConfig) -> Result<Option<AiSettings>> {
    let ai = match &config.ai {
        Some(ai) if ai.enabled => ai,
        _ => return Ok(None),
    };

    let _ = dotenvy::from_path(root.join(".env"));
    let api_key = env::var(&ai.api_key_env).unwrap_or_default();
    if api_key.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(AiSettings {
        model: ai.model.clone(),
        api_key,
        max_output_tokens: ai.max_output_tokens,
    }))
}

pub async fn suggest_translation(
    settings: &AiSettings,
    source_lang: &str,
    target_lang: &str,
    message_id: &str,
    source_text: &str,
) -> Result<String> {
    let client = Client::new();

    let system_prompt = format!(
        "You are a professional translator. Translate from {source_lang} to {target_lang}. Preserve placeholders like {{name}} and ICU plural/select syntax. Return only the translation text."
    );

    let user_prompt = format!(
        "Message ID: {message_id}\nSource: {source_text}"
    );

    let request = ResponsesRequest {
        model: settings.model.clone(),
        input: vec![
            ResponseInputItem {
                role: "system",
                content: vec![ResponseContent {
                    kind: "input_text",
                    text: system_prompt,
                }],
            },
            ResponseInputItem {
                role: "user",
                content: vec![ResponseContent {
                    kind: "input_text",
                    text: user_prompt,
                }],
            },
        ],
        max_output_tokens: settings.max_output_tokens,
    };

    let response = client
        .post("https://api.openai.com/v1/responses")
        .bearer_auth(&settings.api_key)
        .json(&request)
        .send()
        .await
        .map_err(|err| TransError::InvalidInput(format!("AI request failed: {err}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(TransError::InvalidInput(format!(
            "AI request failed ({status}): {body}"
        )));
    }

    let payload: ResponsesResponse = response
        .json()
        .await
        .map_err(|err| TransError::InvalidInput(format!("AI response parse failed: {err}")))?;

    let text = payload
        .output
        .iter()
        .flat_map(|item| &item.content)
        .find_map(|content| content.text.as_ref())
        .map(String::from)
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err(TransError::InvalidInput(
            "AI response was empty".to_string(),
        ));
    }

    Ok(text.trim().to_string())
}

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponseInputItem>,
    max_output_tokens: u32,
}

#[derive(Debug, Serialize)]
struct ResponseInputItem {
    role: &'static str,
    content: Vec<ResponseContent>,
}

#[derive(Debug, Serialize)]
struct ResponseContent {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    output: Vec<ResponseOutputItem>,
}

#[derive(Debug, Deserialize)]
struct ResponseOutputItem {
    content: Vec<ResponseOutputContent>,
}

#[derive(Debug, Deserialize)]
struct ResponseOutputContent {
    #[serde(default)]
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiConfig;
    use std::fs;
    use tempfile::tempdir;

    fn base_config() -> TransConfig {
        TransConfig {
            language_files_path: "translations".into(),
            available_languages: vec!["en".to_string()],
            required_languages: vec!["en".to_string()],
            primary_language: "en".to_string(),
            default_untranslated_value: "".to_string(),
            ai: Some(AiConfig {
                enabled: true,
                model: "gpt-5-mini".to_string(),
                api_key_env: "OPENAI_API_KEY".to_string(),
                max_output_tokens: 128,
            }),
        }
    }

    #[test]
    fn resolve_ai_settings_reads_dotenv() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join(".env"), "OPENAI_API_KEY=test-key\n").expect("write");
        let config = base_config();
        let settings = resolve_ai_settings(root, &config).expect("settings");
        assert!(settings.is_some());
        assert_eq!(settings.unwrap().api_key, "test-key");
    }
}
