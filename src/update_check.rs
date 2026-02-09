use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;

const CACHE_FILE: &str = "trans_update_check.json";
const CACHE_TTL_SECONDS: u64 = 60 * 60 * 24;
const NO_UPDATE_ENV: &str = "TRANS_NO_UPDATE_CHECK";
const NO_UPDATE_ENV_ALT: &str = "TRANS_SKIP_UPDATE_CHECK";

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdateCache {
    checked_at: u64,
    latest: String,
}

pub fn spawn_update_check(current: &str) -> Option<Receiver<UpdateInfo>> {
    if std::env::var_os(NO_UPDATE_ENV).is_some() || std::env::var_os(NO_UPDATE_ENV_ALT).is_some() {
        return None;
    }
    if let Some(info) = cached_update(current) {
        let (tx, rx) = channel();
        let _ = tx.send(info);
        return Some(rx);
    }

    let current = current.to_string();
    let (tx, rx) = channel();
    thread::spawn(move || {
        if let Ok(Some(latest)) = check_brew_latest() {
            let _ = write_cache(&latest);
            if is_newer(&latest, &current) {
                let _ = tx.send(UpdateInfo { current, latest });
            }
        }
    });
    Some(rx)
}

fn cached_update(current: &str) -> Option<UpdateInfo> {
    let cache = read_cache()?;
    if cache.checked_at + CACHE_TTL_SECONDS < now_seconds() {
        return None;
    }
    if is_newer(&cache.latest, current) {
        return Some(UpdateInfo {
            current: current.to_string(),
            latest: cache.latest,
        });
    }
    None
}

fn cache_path() -> PathBuf {
    std::env::temp_dir().join(CACHE_FILE)
}

fn read_cache() -> Option<UpdateCache> {
    let path = cache_path();
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn write_cache(latest: &str) -> Result<()> {
    let cache = UpdateCache {
        checked_at: now_seconds(),
        latest: latest.to_string(),
    };
    let payload = serde_json::to_string(&cache)?;
    fs::write(cache_path(), payload)?;
    Ok(())
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

fn check_brew_latest() -> Result<Option<String>> {
    let output = Command::new("brew")
        .args(["livecheck", "trans", "--json"])
        .output();
    let output = match output {
        Ok(output) if output.status.success() => output,
        _ => return Ok(None),
    };
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let latest = extract_livecheck_latest(&payload);
    Ok(latest)
}

fn extract_livecheck_latest(payload: &serde_json::Value) -> Option<String> {
    payload
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| {
            item.get("latest")
                .and_then(|value| value.as_str())
                .or_else(|| {
                    item.get("version")
                        .and_then(|version| version.get("latest"))
                        .and_then(|value| value.as_str())
                })
                .or_else(|| item.get("version").and_then(|value| value.as_str()))
        })
        .map(|value| value.trim_start_matches('v').to_string())
}

fn parse_version(value: &str) -> Option<Vec<u64>> {
    let mut parts = Vec::new();
    for segment in value.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        if segment.is_empty() {
            continue;
        }
        for part in segment.split('.') {
            if part.is_empty() {
                continue;
            }
            parts.push(part.parse::<u64>().ok()?);
        }
        break;
    }
    if parts.is_empty() { None } else { Some(parts) }
}

fn is_newer(latest: &str, current: &str) -> bool {
    let latest_parts = match parse_version(latest) {
        Some(parts) => parts,
        None => return false,
    };
    let current_parts = match parse_version(current) {
        Some(parts) => parts,
        None => return false,
    };
    let max_len = latest_parts.len().max(current_parts.len());
    for idx in 0..max_len {
        let latest_value = *latest_parts.get(idx).unwrap_or(&0);
        let current_value = *current_parts.get(idx).unwrap_or(&0);
        if latest_value > current_value {
            return true;
        }
        if latest_value < current_value {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::extract_livecheck_latest;

    #[test]
    fn extract_livecheck_latest_from_nested_version_object() {
        let payload = serde_json::json!([
            {
                "formula": "trans",
                "version": {
                    "current": "0.1.6",
                    "latest": "0.1.7",
                    "outdated": true
                }
            }
        ]);
        assert_eq!(
            extract_livecheck_latest(&payload),
            Some("0.1.7".to_string())
        );
    }

    #[test]
    fn extract_livecheck_latest_from_flat_latest() {
        let payload = serde_json::json!([
            {
                "formula": "trans",
                "latest": "v0.2.0"
            }
        ]);
        assert_eq!(
            extract_livecheck_latest(&payload),
            Some("0.2.0".to_string())
        );
    }
}
