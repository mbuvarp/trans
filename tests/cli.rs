use std::path::PathBuf;
use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use tempfile::tempdir;

use trans::config::TransConfig;
use trans::translations::{load_language_translations, save_language_translations, Translations};

fn base_config() -> TransConfig {
    TransConfig {
        language_files_path: PathBuf::from("messages"),
        available_languages: vec!["en".to_string(), "nb".to_string()],
        required_languages: vec!["en".to_string()],
        primary_language: "en".to_string(),
        default_untranslated_value: "".to_string(),
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

fn trans_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("trans"))
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
