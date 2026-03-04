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
        run_update_check: false,
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

fn setup_next_intl_project(root: &std::path::Path) -> TransConfig {
    let mut config = base_config();
    config.mode = trans::config::ConfigMode::NextIntl;
    config.save_to_root(root).expect("save config");

    std::fs::create_dir_all(root.join("messages")).expect("mkdir");
    std::fs::write(
        root.join("messages/en.json"),
        "{\n  \"app\": {\n    \"title\": \"Title\"\n  }\n}\n",
    )
    .expect("write en");
    std::fs::write(
        root.join("messages/nb.json"),
        "{\n  \"app\": {\n    \"title\": \"Tittel\"\n  }\n}\n",
    )
    .expect("write nb");

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

#[test]
fn next_intl_add_update_show_delete_flow() {
    let dir = tempdir().expect("tempdir");
    let config = setup_next_intl_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["add", "--id", "app.new", "--values", "en:Hello"])
        .assert()
        .success();

    let en = load_language_translations(dir.path(), &config, "en").expect("load en");
    let nb = load_language_translations(dir.path(), &config, "nb").expect("load nb");
    assert_eq!(en.get("app.new").map(String::as_str), Some("Hello"));
    assert_eq!(nb.get("app.new").map(String::as_str), Some(""));
    let en_file = std::fs::read_to_string(dir.path().join("messages/en.json")).expect("read en");
    assert!(en_file.contains("\"app\""));
    assert!(en_file.contains("\"new\""));
    assert!(!en_file.contains("\"app.new\""));

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
fn next_intl_verify_reports_nested_line_numbers() {
    let dir = tempdir().expect("tempdir");
    let mut config = setup_next_intl_project(dir.path());
    config.available_languages = vec!["en".to_string(), "nb".to_string()];
    config.save_to_root(dir.path()).expect("save config");

    std::fs::write(
        dir.path().join("messages/nb.json"),
        "{\n  \"app\": {\n    \"other\": \"Annet\"\n  }\n}\n",
    )
    .expect("write nb");

    trans_cmd()
        .current_dir(dir.path())
        .arg("verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("messages/en.json:3"));
}

#[test]
fn next_intl_import_updates_nested_files() {
    let dir = tempdir().expect("tempdir");
    let config = setup_next_intl_project(dir.path());

    let csv = "id,en,nb\napp.title,Title,Oppdatert\n";
    std::fs::write(dir.path().join("import.csv"), csv).expect("write csv");

    trans_cmd()
        .current_dir(dir.path())
        .args(["import", "import.csv"])
        .assert()
        .success();

    let nb = load_language_translations(dir.path(), &config, "nb").expect("load nb");
    assert_eq!(nb.get("app.title").map(String::as_str), Some("Oppdatert"));
    let raw = std::fs::read_to_string(dir.path().join("messages/nb.json")).expect("read nb");
    assert!(raw.contains("\"app\""));
    assert!(raw.contains("\"title\""));
}

#[test]
fn next_intl_non_string_values_fail_in_non_interactive_commands() {
    let dir = tempdir().expect("tempdir");
    let config = setup_next_intl_project(dir.path());
    std::fs::write(
        dir.path().join("messages/nb.json"),
        "{\n  \"app\": {\n    \"title\": 1\n  }\n}\n",
    )
    .expect("write nb");

    trans_cmd()
        .current_dir(dir.path())
        .args(["show", "--id", "app.title", "--lang", "nb"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("next-intl non-string values"));

    let _ = config;
}

#[test]
fn migrate_in_place_updates_mode_and_rewrites_files() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl"])
        .assert()
        .success();

    let config = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config.mode, trans::config::ConfigMode::NextIntl);

    let en_raw = std::fs::read_to_string(dir.path().join("messages/en.json")).expect("read en");
    assert!(en_raw.contains("\"app\""));
    assert!(!en_raw.contains("\"app.title\""));
}

#[test]
fn migrate_out_dir_updates_language_files_path_to_relative() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "-o", "converted/messages"])
        .assert()
        .success();

    let config = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config.mode, trans::config::ConfigMode::NextIntl);
    assert_eq!(
        config.language_files_path,
        PathBuf::from("converted/messages")
    );
    assert!(dir.path().join("converted/messages/en.json").exists());
}

#[test]
fn migrate_out_dir_with_no_update_keeps_language_files_path() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args([
            "migrate",
            "next-intl",
            "-o",
            "converted/messages",
            "--no-update-language-files-path",
        ])
        .assert()
        .success();

    let config = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config.mode, trans::config::ConfigMode::NextIntl);
    assert_eq!(config.language_files_path, PathBuf::from("messages"));
    assert!(dir.path().join("converted/messages/en.json").exists());
}

#[test]
fn migrate_rejects_no_update_without_out_dir() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "--no-update-language-files-path"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--no-update-language-files-path requires --out-dir",
        ));
}

#[test]
fn migrate_conflict_failure_keeps_config_and_source_files() {
    let dir = tempdir().expect("tempdir");
    let config = base_config();
    config.save_to_root(dir.path()).expect("save config");

    std::fs::create_dir_all(dir.path().join("messages")).expect("mkdir");
    std::fs::write(
        dir.path().join("messages/en.json"),
        "{\n  \"app\": \"A\",\n  \"app.header\": \"B\"\n}\n",
    )
    .expect("write en");
    std::fs::write(
        dir.path().join("messages/nb.json"),
        "{\n  \"app\": \"A\",\n  \"app.header\": \"B\"\n}\n",
    )
    .expect("write nb");

    let before = std::fs::read_to_string(dir.path().join("messages/en.json")).expect("read before");
    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "mode migration aborted due to key conflicts",
        ));

    let after = std::fs::read_to_string(dir.path().join("messages/en.json")).expect("read after");
    assert_eq!(before, after);
    let config_after = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config_after.mode, trans::config::ConfigMode::ReactIntl);
}

#[test]
fn migrate_out_dir_overwrites_existing_files() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());
    std::fs::create_dir_all(dir.path().join("converted/messages")).expect("mkdir");
    std::fs::write(
        dir.path().join("converted/messages/en.json"),
        "{\n  \"old\": \"value\"\n}\n",
    )
    .expect("write old");

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "-o", "converted/messages"])
        .assert()
        .success();

    let converted =
        std::fs::read_to_string(dir.path().join("converted/messages/en.json")).expect("read");
    assert!(converted.contains("\"app\""));
    assert!(!converted.contains("\"old\""));
}

#[test]
fn migrate_check_valid_does_not_modify_files_or_config() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    let before = std::fs::read_to_string(dir.path().join("messages/en.json")).expect("read before");
    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Check OK"));

    let after = std::fs::read_to_string(dir.path().join("messages/en.json")).expect("read after");
    assert_eq!(before, after);
    let config = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config.mode, trans::config::ConfigMode::ReactIntl);
}

#[test]
fn migrate_check_conflict_fails_without_modifying_config() {
    let dir = tempdir().expect("tempdir");
    let config = base_config();
    config.save_to_root(dir.path()).expect("save config");
    std::fs::create_dir_all(dir.path().join("messages")).expect("mkdir");
    std::fs::write(
        dir.path().join("messages/en.json"),
        "{\n  \"app\": \"A\",\n  \"app.header\": \"B\"\n}\n",
    )
    .expect("write en");
    std::fs::write(
        dir.path().join("messages/nb.json"),
        "{\n  \"app\": \"A\",\n  \"app.header\": \"B\"\n}\n",
    )
    .expect("write nb");

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "--check"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "mode migration aborted due to key conflicts",
        ));

    let config_after = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config_after.mode, trans::config::ConfigMode::ReactIntl);
}

#[test]
fn migrate_check_with_out_dir_does_not_create_output_dir() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args([
            "migrate",
            "next-intl",
            "--check",
            "-o",
            "converted/messages",
        ])
        .assert()
        .success();

    assert!(!dir.path().join("converted/messages").exists());
}

#[test]
fn migrate_check_ignores_backup_flag() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "--check", "--backup"])
        .assert()
        .success();

    assert!(!dir.path().join("messages__backup").exists());
}

#[test]
fn migrate_backup_in_place_creates_backup_then_migrates() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "--backup"])
        .assert()
        .success();

    let config = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config.mode, trans::config::ConfigMode::NextIntl);

    let backup = dir.path().join("messages__backup/en.json");
    let backup_raw = std::fs::read_to_string(backup).expect("read backup");
    assert!(backup_raw.contains("\"app.title\""));

    let migrated_raw =
        std::fs::read_to_string(dir.path().join("messages/en.json")).expect("read migrated");
    assert!(migrated_raw.contains("\"app\""));
    assert!(!migrated_raw.contains("\"app.title\""));
}

#[test]
fn migrate_backup_fails_when_backup_exists() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());
    std::fs::create_dir_all(dir.path().join("messages__backup")).expect("mkdir backup");

    trans_cmd()
        .current_dir(dir.path())
        .args(["migrate", "next-intl", "--backup"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("backup directory already exists"));

    let config = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config.mode, trans::config::ConfigMode::ReactIntl);
}

#[test]
fn migrate_backup_with_out_dir_creates_backup_and_migrates_to_out_dir() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args([
            "migrate",
            "next-intl",
            "--backup",
            "-o",
            "converted/messages",
        ])
        .assert()
        .success();

    assert!(dir.path().join("messages__backup/en.json").exists());
    assert!(dir.path().join("converted/messages/en.json").exists());

    let config = TransConfig::load_from_root(dir.path()).expect("load config");
    assert_eq!(config.mode, trans::config::ConfigMode::NextIntl);
    assert_eq!(
        config.language_files_path,
        PathBuf::from("converted/messages")
    );
}
