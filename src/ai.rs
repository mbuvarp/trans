use std::env;
use std::path::Path;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::TransConfig;
use crate::error::{Result, TransError};
use crate::language::language_display_name;

#[derive(Debug, Clone)]
pub struct AiSettings {
    pub model: String,
    pub api_key: String,
    pub max_output_tokens: u32,
    pub concurrency: usize,
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
        concurrency: ai.concurrency.max(1),
    }))
}

pub async fn suggest_translation(
    settings: &AiSettings,
    source_lang: &str,
    target_lang: &str,
    message_id: &str,
    source_text: &str,
) -> Result<String> {
    let source_name = language_display_name(source_lang);
    let target_name = language_display_name(target_lang);
    let system_prompt = format!(
        "You are a professional translator. Translate from {source_name} to {target_name}. Preserve placeholders like {{name}} and ICU plural/select syntax. Do not add XML/HTML tags unless they appear in the source. Return only the translation text."
    );

    let user_prompt = format!("Message ID: {message_id}\nSource: {source_text}");
    suggest_custom(settings, &system_prompt, &user_prompt).await
}

pub async fn suggest_custom(
    settings: &AiSettings,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String> {
    if let Ok(mock) = env::var("TRANS_AI_MOCK") {
        let trimmed = mock.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(value) = env::var("TRANS_AI_DISABLE") {
        let value = value.trim().to_ascii_lowercase();
        if value == "1" || value == "true" || value == "yes" {
            return Err(TransError::InvalidInput(
                "AI is disabled via TRANS_AI_DISABLE".to_string(),
            ));
        }
    }
    let client = Client::new();

    let request = ResponsesRequest {
        model: settings.model.clone(),
        input: vec![
            ResponseInputItem {
                role: "system",
                content: vec![ResponseContent {
                    kind: "input_text",
                    text: system_prompt.to_string(),
                }],
            },
            ResponseInputItem {
                role: "user",
                content: vec![ResponseContent {
                    kind: "input_text",
                    text: user_prompt.to_string(),
                }],
            },
        ],
        max_output_tokens: settings.max_output_tokens,
        reasoning: reasoning_for_model(&settings.model).map(|effort| ReasoningConfig { effort }),
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
        if let Ok(api_error) = serde_json::from_str::<OpenAiErrorResponse>(&body) {
            if api_error.error.code.as_deref() == Some("insufficient_quota") {
                return Err(TransError::InvalidInput(
                    "AI quota exceeded. Update your OpenAI plan or API key.".to_string(),
                ));
            }
            return Err(TransError::InvalidInput(format!(
                "AI request failed ({status}): {}",
                api_error.error.message
            )));
        }
        return Err(TransError::InvalidInput(format!(
            "AI request failed ({status}): {body}"
        )));
    }

    let body = response
        .text()
        .await
        .map_err(|err| TransError::InvalidInput(format!("AI response read failed: {err}")))?;
    if body.trim().is_empty() {
        return Err(TransError::InvalidInput(
            "AI response was empty".to_string(),
        ));
    }

    let text = match serde_json::from_str::<ResponsesResponse>(&body) {
        Ok(payload) => payload
            .output
            .iter()
            .flat_map(|item| &item.content)
            .find_map(|content| content.text.as_ref())
            .map(String::from),
        Err(_) => extract_text_from_value(&body),
    }
    .unwrap_or_default();

    if text.trim().is_empty() {
        let preview = body.chars().take(800).collect::<String>();
        return Err(TransError::InvalidInput(format!(
            "AI response was empty. Raw response (truncated): {preview}"
        )));
    }

    Ok(text.trim().to_string())
}

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponseInputItem>,
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
}

#[derive(Debug, Serialize)]
struct ReasoningConfig {
    effort: &'static str,
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

fn extract_text_from_value(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let output = value.get("output")?.as_array()?;
    for item in output {
        let contents = match item.get("content").and_then(|c| c.as_array()) {
            Some(contents) => contents,
            None => continue,
        };
        for content in contents {
            if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                if !text.trim().is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

fn reasoning_for_model(model: &str) -> Option<&'static str> {
    if model.starts_with("gpt-5.1") {
        Some("none")
    } else if model.starts_with("gpt-5") {
        Some("minimal")
    } else {
        None
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorResponse {
    error: OpenAiErrorBody,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorBody {
    message: String,
    #[serde(default)]
    code: Option<String>,
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
            default_export_format: crate::config::ExportFormat::Excel,
            excel_password: "unlock".to_string(),
            ai: Some(AiConfig {
                enabled: true,
                model: "gpt-5-mini".to_string(),
                api_key_env: "OPENAI_API_KEY".to_string(),
                max_output_tokens: 128,
                concurrency: 2,
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
