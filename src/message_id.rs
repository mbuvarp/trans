use crate::error::{Result, TransError};

pub fn validate_message_id(message_id: &str) -> Result<()> {
    let trimmed = message_id.trim();
    if trimmed.is_empty() {
        return Err(TransError::InvalidMessageId(
            "message id must not be empty".to_string(),
        ));
    }
    if !trimmed.contains('.') {
        return Err(TransError::InvalidMessageId(
            "message id must include a namespace (e.g. app.header)".to_string(),
        ));
    }
    if trimmed.starts_with('.') || trimmed.ends_with('.') {
        return Err(TransError::InvalidMessageId(
            "message id must not start or end with '.'".to_string(),
        ));
    }
    if trimmed.split('.').any(|segment| segment.trim().is_empty()) {
        return Err(TransError::InvalidMessageId(
            "message id must not contain empty segments".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_namespaced_ids() {
        assert!(validate_message_id("app.header").is_ok());
        assert!(validate_message_id("app.header.title").is_ok());
    }

    #[test]
    fn rejects_empty_or_non_namespaced() {
        assert!(validate_message_id("").is_err());
        assert!(validate_message_id("title").is_err());
    }

    #[test]
    fn rejects_leading_or_trailing_dots() {
        assert!(validate_message_id(".title").is_err());
        assert!(validate_message_id("title.").is_err());
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(validate_message_id("app..title").is_err());
    }
}
