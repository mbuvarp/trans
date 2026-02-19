use std::path::PathBuf;
use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use tempfile::tempdir;

use trans::config::{AiConfig, TransConfig};
use trans::translations::{Translations, load_language_translations, save_language_translations};

fn base_config() -> TransConfig {
    TransConfig {
        mode: trans::config::ConfigMode::ReactIntl,
        language_files_path: PathBuf::from("messages"),
        available_languages: vec!["en".to_string(), "nb".to_string()],
        required_languages: vec!["en".to_string()],
        primary_language: "en".to_string(),
        default_untranslated_value: "".to_string(),
        default_export_format: trans::config::ExportFormat::Excel,
        excel_password: "unlock".to_string(),
        ai: None,
    }
}

fn translations(values: &[(&str, &str)]) -> Translations {
    values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn setup_project(root: &std::path::Path) -> TransConfig {
    let config = base_config();
    config.save_to_root(root).expect("save config");

    save_language_translations(
        root,
        &config,
        "en",
        &translations(&[("app.title", "Title")]),
    )
    .expect("save en");
    save_language_translations(
        root,
        &config,
        "nb",
        &translations(&[("app.title", "Tittel")]),
    )
    .expect("save nb");

    config
}

fn setup_project_with_ai(root: &std::path::Path) -> TransConfig {
    let mut config = base_config();
    config.ai = Some(AiConfig {
        enabled: true,
        model: "gpt-5-mini".to_string(),
        api_key_env: "OPENAI_API_KEY".to_string(),
        max_output_tokens: 64,
        concurrency: 2,
    });
    config.save_to_root(root).expect("save config");

    save_language_translations(
        root,
        &config,
        "en",
        &translations(&[("app.title", "Title")]),
    )
    .expect("save en");
    save_language_translations(root, &config, "nb", &Translations::new()).expect("save nb");

    config
}

fn trans_cmd() -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("trans"));
    cmd.env("TRANS_AI_DISABLE", "1");
    cmd.env("TRANS_NO_UPDATE_CHECK", "1");
    cmd
}

#[test]
fn list_required_languages_outputs_expected_values() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .arg("list-required-languages")
        .assert()
        .success()
        .stdout(predicate::str::contains("en"));
}

#[test]
fn add_update_show_delete_flow() {
    let dir = tempdir().expect("tempdir");
    let config = setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["add", "--id", "app.new", "--values", "en:Hello"])
        .assert()
        .success();

    let en = load_language_translations(dir.path(), &config, "en").expect("load en");
    let nb = load_language_translations(dir.path(), &config, "nb").expect("load nb");
    assert_eq!(en.get("app.new").map(String::as_str), Some("Hello"));
    assert_eq!(nb.get("app.new").map(String::as_str), Some(""));

    trans_cmd()
        .current_dir(dir.path())
        .args(["update", "--id", "app.new", "--values", "en:Updated"])
        .assert()
        .success();

    trans_cmd()
        .current_dir(dir.path())
        .args(["show", "--id", "app.new", "--lang", "en"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated"));

    trans_cmd()
        .current_dir(dir.path())
        .args(["delete", "--id", "app.new"])
        .assert()
        .success();

    let en_after = load_language_translations(dir.path(), &config, "en").expect("load en");
    assert!(!en_after.contains_key("app.new"));
}

#[test]
fn verify_and_export_commands() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("OK"));

    trans_cmd()
        .current_dir(dir.path())
        .args(["export", "--format", "csv"])
        .assert()
        .success();

    assert!(dir.path().join("translations.csv").exists());

    trans_cmd()
        .current_dir(dir.path())
        .args(["export", "--format", "excel"])
        .assert()
        .success();

    assert!(dir.path().join("translations.xlsx").exists());
}

#[test]
fn verify_ai_applies_mock_suggestion_for_missing_id() {
    let dir = tempdir().expect("tempdir");
    let config = setup_project_with_ai(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["verify", "--ai"])
        .env("OPENAI_API_KEY", "test-key")
        .env("TRANS_AI_MOCK", "Suggested")
        .env("TRANS_AI_ASSUME_YES", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains("OK"));

    let nb = load_language_translations(dir.path(), &config, "nb").expect("load nb");
    assert_eq!(nb.get("app.title").map(String::as_str), Some("Suggested"));
}

#[test]
fn verify_ai_requires_configuration() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["verify", "--ai"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("AI is not configured"));
}

#[test]
fn export_with_lang_filter_includes_primary_and_selected() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["export", "--format", "csv", "--lang", "nb"])
        .assert()
        .success();

    let csv_path = dir.path().join("translations.csv");
    let contents = std::fs::read_to_string(csv_path).expect("read csv");
    let header = contents.lines().next().unwrap_or_default();
    assert_eq!(header, "id,en,nb");
}

#[test]
fn export_with_only_primary_lang_warns_and_skips() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["export", "--format", "csv", "--lang", "en"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Warning"));

    assert!(!dir.path().join("translations.csv").exists());
}

#[test]
fn export_with_output_without_extension_appends_format() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["export", "--format", "csv", "--output", "custom-name"])
        .assert()
        .success();

    assert!(dir.path().join("custom-name.csv").exists());
}

#[test]
fn export_missing_only_filters_rows() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["add", "--id", "app.missing", "--values", "en:Missing"])
        .assert()
        .success();

    trans_cmd()
        .current_dir(dir.path())
        .args(["export", "--format", "csv", "--missing"])
        .assert()
        .success();

    let csv_path = dir.path().join("translations.csv");
    let contents = std::fs::read_to_string(csv_path).expect("read csv");
    let rows: Vec<&str> = contents.lines().collect();
    assert_eq!(rows[0], "id,en,nb");
    assert_eq!(rows.len(), 2);
    assert!(rows[1].starts_with("app.missing,"));
}
