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

fn setup_next_intl_project_with_keys(root: &std::path::Path, keys: &[(&str, &str)]) -> TransConfig {
    let mut config = base_config();
    config.mode = trans::config::ConfigMode::NextIntl;
    config.save_to_root(root).expect("save config");

    save_language_translations(root, &config, "en", &translations(keys)).expect("save en");
    save_language_translations(root, &config, "nb", &translations(keys)).expect("save nb");

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

fn setup_find_project(root: &std::path::Path) -> TransConfig {
    let config = base_config();
    config.save_to_root(root).expect("save config");

    save_language_translations(
        root,
        &config,
        "en",
        &translations(&[
            ("calendar.this-week", "This week"),
            ("calendar.submit", "submit"),
            ("calendar.submit-title", "Submit"),
            ("calendar.submit-help", "Submit form"),
            ("calendar.other", "Cancel"),
        ]),
    )
    .expect("save en");
    save_language_translations(
        root,
        &config,
        "nb",
        &translations(&[
            ("calendar.this-week", "Denne uken"),
            ("calendar.submit", "send inn"),
            ("calendar.submit-title", "Send inn"),
            ("calendar.submit-help", "Send inn skjema"),
            ("calendar.other", "Avbryt"),
        ]),
    )
    .expect("save nb");

    config
}

fn setup_partial_has_project(root: &std::path::Path) -> TransConfig {
    let mut config = base_config();
    config.available_languages = vec![
        "en".to_string(),
        "nb".to_string(),
        "pl".to_string(),
        "se".to_string(),
    ];
    config.save_to_root(root).expect("save config");

    save_language_translations(
        root,
        &config,
        "en",
        &translations(&[("common.help", "Help")]),
    )
    .expect("save en");
    save_language_translations(
        root,
        &config,
        "nb",
        &translations(&[("common.help", "Hjelp")]),
    )
    .expect("save nb");
    save_language_translations(
        root,
        &config,
        "pl",
        &translations(&[("common.other", "Other")]),
    )
    .expect("save pl");
    save_language_translations(
        root,
        &config,
        "se",
        &translations(&[("common.other", "Other")]),
    )
    .expect("save se");

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
fn list_required_languages_works_from_child_directory() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());
    let child = dir.path().join("src/components");
    std::fs::create_dir_all(&child).expect("mkdir child");

    trans_cmd()
        .current_dir(&child)
        .arg("list-required-languages")
        .assert()
        .success()
        .stdout(predicate::str::contains("en"));
}

#[test]
fn mutation_and_verify_from_child_use_config_directory_paths() {
    let dir = tempdir().expect("tempdir");
    let config = setup_project(dir.path());
    let child = dir.path().join("src/components");
    std::fs::create_dir_all(&child).expect("mkdir child");

    trans_cmd()
        .current_dir(&child)
        .args(["add", "--id", "app.child", "--values", "en:Child"])
        .assert()
        .success();

    trans_cmd()
        .current_dir(&child)
        .args(["show", "--id", "app.child", "--lang", "en"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Child"));

    trans_cmd()
        .current_dir(&child)
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("OK"));

    let en = load_language_translations(dir.path(), &config, "en").expect("load en");
    assert_eq!(en.get("app.child").map(String::as_str), Some("Child"));
    assert!(!child.join("messages").exists());
}

#[test]
fn cwd_flag_discovers_config_from_child_directory() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());
    let child = dir.path().join("src/components");
    std::fs::create_dir_all(&child).expect("mkdir child");
    let outside = tempdir().expect("outside tempdir");

    trans_cmd()
        .current_dir(outside.path())
        .args([
            "-C",
            child.to_str().expect("utf-8 path"),
            "show",
            "--id",
            "app.title",
            "--lang",
            "en",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Title"));
}

#[test]
fn cwd_flag_works_after_subcommand_and_resolves_relative_paths() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());
    let child = dir.path().join("src/components");
    std::fs::create_dir_all(&child).expect("mkdir child");

    trans_cmd()
        .current_dir(dir.path())
        .args([
            "show",
            "-C",
            "src/components",
            "--id",
            "app.title",
            "--lang",
            "en",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Title"));
}

#[test]
fn cwd_flag_export_writes_to_config_directory() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());
    let child = dir.path().join("src/components");
    std::fs::create_dir_all(&child).expect("mkdir child");
    let outside = tempdir().expect("outside tempdir");

    trans_cmd()
        .current_dir(outside.path())
        .args([
            "-C",
            child.to_str().expect("utf-8 path"),
            "export",
            "--format",
            "csv",
        ])
        .assert()
        .success();

    assert!(dir.path().join("translations.csv").exists());
    assert!(!child.join("translations.csv").exists());
    assert!(!outside.path().join("translations.csv").exists());
}

#[test]
fn help_documents_cwd_flag() {
    trans_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("-C, --cwd <DIR>"));
}

#[test]
fn find_uses_primary_language_by_default() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["find", "This week"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("calendar.this-week")
                .and(predicate::str::contains("exact"))
                .and(predicate::str::contains("Denne uken").not()),
        );
}

#[test]
fn has_outputs_found_and_exits_zero_when_all_languages_contain_id() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["has", "app.title"])
        .assert()
        .success()
        .stdout("found\n");
}

#[test]
fn has_outputs_not_found_and_exits_one_when_no_languages_contain_id() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["has", "app.missing"])
        .assert()
        .code(1)
        .stdout("not found\n")
        .stderr("");
}

#[test]
fn has_outputs_partial_language_lists_and_exits_two() {
    let dir = tempdir().expect("tempdir");
    setup_partial_has_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["has", "common.help"])
        .assert()
        .code(2)
        .stdout("found: en, nb\nnot found: pl, se\n")
        .stderr("");
}

#[test]
fn has_searches_next_intl_nested_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["has", "app.title"])
        .assert()
        .success()
        .stdout("found\n");
}

#[test]
fn has_rejects_invalid_message_id() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["has", "title"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("namespace"));
}

#[test]
fn find_searches_explicit_language() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["find", "--language", "nb", "Denne uken"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("calendar.this-week").and(predicate::str::contains("exact")),
        );
}

#[test]
fn find_outputs_exact_casing_and_partial_matches_in_order() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    let assert = trans_cmd()
        .current_dir(dir.path())
        .args(["find", "submit"])
        .assert()
        .success();
    let output = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let lines: Vec<&str> = output.lines().collect();

    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("calendar.submit"));
    assert!(lines[0].contains("exact"));
    assert!(lines[1].contains("calendar.submit-title"));
    assert!(lines[1].contains("casing"));
    assert!(lines[2].contains("calendar.submit-help"));
    assert!(lines[2].contains("partial"));
}

#[test]
fn find_exact_only_excludes_casing_and_partial_matches() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    let assert = trans_cmd()
        .current_dir(dir.path())
        .args(["find", "--exact-only", "submit"])
        .assert()
        .success();
    let output = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");

    assert!(output.contains("calendar.submit"));
    assert!(output.contains("exact"));
    assert!(!output.contains("calendar.submit-title"));
    assert!(!output.contains("calendar.submit-help"));
}

#[test]
fn find_case_sensitive_excludes_casing_matches() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    let assert = trans_cmd()
        .current_dir(dir.path())
        .args(["find", "--case-sensitive", "Submit"])
        .assert()
        .success();
    let output = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");

    assert!(output.contains("calendar.submit-title"));
    assert!(output.contains("exact"));
    assert!(output.contains("calendar.submit-help"));
    assert!(output.contains("partial"));
    assert!(!output.contains("calendar.submit  "));
    assert!(!output.contains("casing"));
}

#[test]
fn find_exact_only_and_case_sensitive_returns_same_case_exact_matches() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    let assert = trans_cmd()
        .current_dir(dir.path())
        .args(["find", "--exact-only", "--case-sensitive", "submit"])
        .assert()
        .success();
    let output = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");

    assert!(output.contains("calendar.submit"));
    assert!(output.contains("exact"));
    assert!(!output.contains("calendar.submit-title"));
    assert!(!output.contains("calendar.submit-help"));
}

#[test]
fn find_no_matches_exits_successfully_with_empty_stdout() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["find", "Missing"])
        .assert()
        .success()
        .stdout("");
}

#[test]
fn find_rejects_invalid_language() {
    let dir = tempdir().expect("tempdir");
    setup_find_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .args(["find", "--language", "fr", "submit"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("available_languages"));
}

#[test]
fn find_searches_next_intl_nested_values() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project(dir.path());
    std::fs::write(
        dir.path().join("messages/en.json"),
        "{\n  \"calendar\": {\n    \"thisWeek\": \"This week\"\n  }\n}\n",
    )
    .expect("write en");

    trans_cmd()
        .current_dir(dir.path())
        .args(["find", "This week"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("calendar.thisWeek").and(predicate::str::contains("exact")),
        );
}

#[test]
fn unused_help_documents_no_ts_checker_flag() {
    trans_cmd()
        .args(["unused", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-ts-checker"));
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
fn unused_lists_unused_next_intl_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("app.title", "Title"), ("app.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt('title');\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .arg("unused")
        .assert()
        .success()
        .stdout(predicate::str::contains("Unused keys: 1"))
        .stdout(predicate::str::contains("app.unused").not())
        .stdout(predicate::str::contains("app.title").not());
}

#[test]
fn unused_keys_flag_outputs_only_unused_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("app.title", "Title"), ("app.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt('title');\nt(key);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("app.unused\n")
        .stderr("");
}

#[test]
fn unused_respects_gitignored_files() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(dir.path(), &[("app.ignored", "Ignored")]);
    std::fs::write(dir.path().join(".gitignore"), "ignored.tsx\n").expect("write gitignore");
    std::fs::write(
        dir.path().join("ignored.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt('ignored');\n",
    )
    .expect("write ignored source");
    Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    trans_cmd()
        .current_dir(dir.path())
        .arg("unused")
        .assert()
        .success()
        .stdout(predicate::str::contains("Unused keys: 1"));
}

#[test]
fn unused_fallback_scanner_excludes_generated_directories() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(dir.path(), &[("app.generated", "Generated")]);
    std::fs::create_dir_all(dir.path().join(".next")).expect("mkdir");
    std::fs::write(
        dir.path().join(".next/generated.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt('generated');\n",
    )
    .expect("write generated source");

    trans_cmd()
        .current_dir(dir.path())
        .arg("unused")
        .assert()
        .success()
        .stdout(predicate::str::contains("Unused keys: 1"));
}

#[test]
fn unused_reports_dynamic_usage_locations() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("app.title", "Title"), ("other.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt(key);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .arg("unused")
        .assert()
        .success()
        .stdout(predicate::str::contains("Unused keys: 1"))
        .stdout(predicate::str::contains(
            "Warning: dynamic translation key usage detected in 1 place(s):",
        ))
        .stdout(predicate::str::contains("./page.tsx:3"));
}

#[test]
fn unused_resolves_finite_conditional_key_variables() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("files.folder-count", "Folders"),
            ("files.subfolder-count", "Subfolders"),
            ("files.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('files');\nconst folderCountKey = searchAllFolders ? 'folder-count' : 'subfolder-count';\nt(folderCountKey);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("files.unused\n");
}

#[test]
fn unused_resolves_finite_object_map_lookup_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("notifications.errors.blocked", "Blocked"),
            ("notifications.errors.denied", "Denied"),
            (
                "notifications.errors.registrationFailed",
                "Registration failed",
            ),
            ("notifications.errors.unsupported", "Unsupported"),
            ("notifications.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations();\nconst messageKeyByReason = {\n  blocked: 'notifications.errors.blocked',\n  denied: 'notifications.errors.denied',\n  registrationFailed: 'notifications.errors.registrationFailed',\n  unsupported: 'notifications.errors.unsupported',\n};\nt(messageKeyByReason[error.reason]);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("notifications.unused\n");
}

#[test]
fn unused_resolves_typed_finite_domain_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("settings.timeTypes.categories.REGULAR", "Regular"),
            ("settings.timeTypes.categories.ABSENCE", "Absence"),
            ("settings.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\ntype Category = 'REGULAR' | 'ABSENCE';\ntype FormValues = { category: Category };\nconst t = useTranslations('settings');\nconst form = useForm<FormValues>();\nconst category = form.watch('category');\nt(`timeTypes.categories.${category}`);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_resolves_zod_inferred_property_domain_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("users.relations.parent", "Parent"),
            ("users.relations.partner", "Partner"),
            ("users.relations.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("emergency-contact-shared.ts"),
        "export const EMERGENCY_CONTACT_RELATION_VALUES = ['parent', 'partner'] as const;\n",
    )
    .expect("write relation values");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {UseFormReturn, useWatch} from 'react-hook-form';\nimport {z} from 'zod';\nimport {EMERGENCY_CONTACT_RELATION_VALUES} from './emergency-contact-shared';\nconst EmergencyContactFormSchema = z.object({\n  relation: z.enum(EMERGENCY_CONTACT_RELATION_VALUES),\n});\ntype EmergencyContactFormValues = z.infer<typeof EmergencyContactFormSchema>;\nfunction Fields({form}: {form: UseFormReturn<EmergencyContactFormValues>}) {\n  const watchedValues = useWatch({ control: form.control }) as EmergencyContactFormValues | undefined;\n  const values = watchedValues ?? form.getValues();\n  const tRelation = useTranslations('users.relations');\n  tRelation(values.relation);\n}\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("users.relations.unused\n");
}

#[test]
fn unused_resolves_finite_map_callback_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("template.tabs.overview.label", "Overview"),
            ("template.tabs.variables.label", "Variables"),
            ("template.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('template');\nconst TEMPLATE_TABS = ['overview', 'variables'] as const;\nTEMPLATE_TABS.map(tab => t(`tabs.${tab}.label`));\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("template.unused\n");
}

#[test]
fn unused_resolves_finite_iterated_string_transforms() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("template.variables.types.number", "Number"),
            ("template.variables.types.text", "Text"),
            ("template.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('template');\nconst VARIABLE_TYPES = ['NUMBER', 'TEXT'] as const;\nVARIABLE_TYPES.map(type => t(`variables.types.${type.toLowerCase()}`));\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("template.unused\n");
}

#[test]
fn unused_resolves_imported_finite_iterable_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("relation.parent", "Parent"),
            ("relation.child", "Child"),
            ("relation.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("relations.ts"),
        "export const RELATIONS = ['parent', 'child'] as const;\n",
    )
    .expect("write values");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {RELATIONS} from './relations';\nconst t = useTranslations('relation');\nRELATIONS.map(relation => t(relation));\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("relation.unused\n");
}

#[test]
fn unused_resolves_member_keys_from_imported_enum_value_constants() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("users.user-types.ADMIN", "Admin"),
            ("users.user-types.PROJECT_MANAGER", "Project manager"),
            ("users.user-types.EXTENDED_USER", "Extended user"),
            ("users.user-types.USER", "User"),
            ("users.user-types.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("users-presenter.ts"),
        "import {TenantUserType} from '@digitech/db/types';\nexport const TENANT_USER_TYPE_VALUES = [\n  TenantUserType.ADMIN,\n  TenantUserType.PROJECT_MANAGER,\n  TenantUserType.EXTENDED_USER,\n  TenantUserType.USER,\n] as const;\n",
    )
    .expect("write values");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {TENANT_USER_TYPE_VALUES} from './users-presenter';\nconst tUsers = useTranslations('users');\ntUsers(`user-types.${row.original.userType}`);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("users.user-types.unused\n");
}

#[test]
fn unused_resolves_export_specifier_finite_iterable_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("relation.parent", "Parent"),
            ("relation.child", "Child"),
            ("relation.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("relations.ts"),
        "const RELATIONS = ['parent', 'child'] as const;\nexport { RELATIONS };\n",
    )
    .expect("write values");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {RELATIONS} from './relations';\nconst t = useTranslations('relation');\nfor (const relation of RELATIONS) { t(relation); }\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("relation.unused\n");
}

#[test]
fn unused_resolves_imported_finite_return_helper_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("common.customers", "Customers"),
            ("common.projects", "Projects"),
            ("common.my-page", "My page"),
            ("common.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("top-bar-state.ts"),
        "export function resolveTitleKey(section) {\n  switch (section) {\n    case 'customers':\n      return 'customers';\n    case 'projects':\n      return 'projects';\n    default:\n      return 'my-page';\n  }\n}\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {resolveTitleKey} from './top-bar-state';\nconst t = useTranslations('common');\nt(resolveTitleKey(section));\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("common.unused\n");
}

#[test]
fn unused_resolves_imported_finite_record_return_helper_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("settings.navigation.profile", "Profile"),
            ("settings.navigation.members", "Members"),
            ("settings.navigation.auditLog", "Audit log"),
            ("settings.navigation.sections.account", "Account"),
            ("settings.navigation.sections.system", "System"),
            ("settings.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("settings-sidebar-state.ts"),
        "const ITEMS = [\n  {labelKey: 'navigation.profile'},\n  {labelKey: 'navigation.members'},\n  {labelKey: 'navigation.auditLog'},\n] as const;\nexport function getSettingsSidebarSections(isDeveloper) {\n  const sections = [\n    {labelKey: 'navigation.sections.account', items: ITEMS.slice(0, 2)},\n  ];\n  if (isDeveloper) {\n    sections.push({labelKey: 'navigation.sections.system', items: ITEMS.slice(2)});\n  }\n  return sections;\n}\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getSettingsSidebarSections} from './settings-sidebar-state';\nconst t = useTranslations('settings');\nconst sections = getSettingsSidebarSections(isDeveloper);\nsections.map(section => {\n  t(section.labelKey);\n  section.items.forEach(item => t(item.labelKey));\n});\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_resolves_imported_filtered_record_iterable_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("common.home", "Home"),
            ("common.offers", "Offers"),
            ("common.projects", "Projects"),
            ("common.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("footer-navigation.ts"),
        "export const FOOTER_NAV_ITEMS: {id: string; labelKey: string}[] = [\n  {id: 'home', labelKey: 'common.home'},\n  {id: 'offers', labelKey: 'common.offers'},\n  {id: 'projects', labelKey: 'common.projects'},\n];\n",
    )
    .expect("write nav items");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {FOOTER_NAV_ITEMS} from './footer-navigation';\nconst t = useTranslations();\nFOOTER_NAV_ITEMS.filter(item => item.id !== 'home').map(item => t(item.labelKey));\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("common.unused\n");
}

#[test]
fn unused_resolves_imported_record_indexed_return_helper_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("offers.history.event-created", "Created"),
            ("offers.history.event-sent", "Sent"),
            ("offers.history.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("event-config.ts"),
        "const EVENT_TYPE_CONFIG: Record<string, {titleKey: string}> = {\n  CREATED: {titleKey: 'offers.history.event-created'},\n  SENT: {titleKey: 'offers.history.event-sent'},\n};\nexport function getEventConfig(eventType: string) {\n  return EVENT_TYPE_CONFIG[eventType];\n}\n",
    )
    .expect("write event config");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getEventConfig} from './event-config';\nconst t = useTranslations();\nconst config = getEventConfig(eventType);\nt(config.titleKey);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("offers.history.unused\n");
}

#[test]
fn unused_resolves_imported_finite_record_map_get_helper_keys() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("settings.navigation.profile", "Profile"),
            ("settings.navigation.members", "Members"),
            ("settings.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("settings-sidebar-state.ts"),
        "const ITEMS = [\n  {id: 'profile', labelKey: 'navigation.profile'},\n  {id: 'members', labelKey: 'navigation.members'},\n] as const;\nconst ITEM_BY_ID = new Map(ITEMS.map(item => [item.id, item]));\nexport function getSettingsSidebarItemById(id) {\n  return ITEM_BY_ID.get(id) ?? null;\n}\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getSettingsSidebarItemById} from './settings-sidebar-state';\nconst t = useTranslations('settings');\nconst item = getSettingsSidebarItemById(id);\nif (item) {\n  t(item.labelKey);\n}\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_traces_named_helper_import_from_relative_file() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("settings.title", "Title"), ("settings.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("settings-helper.ts"),
        "export function getTitle(tSettings) { return tSettings('title'); }\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getTitle} from './settings-helper';\nconst t = useTranslations('settings');\ngetTitle(t);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_traces_named_helper_import_from_parent_relative_file() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("projects.validation.customerRequired", "Required"),
            ("projects.unused", "Unused"),
        ],
    );
    std::fs::create_dir_all(dir.path().join("src/projects")).expect("mkdir projects");
    std::fs::create_dir_all(dir.path().join("src/offers")).expect("mkdir offers");
    std::fs::write(
        dir.path().join("src/projects/project-form-shared.tsx"),
        "export function createProjectFormSchema(tProjects) { return tProjects('validation.customerRequired'); }\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("src/offers/offer-create-dialog.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {createProjectFormSchema} from '../projects/project-form-shared';\nconst tProjects = useTranslations('projects');\ncreateProjectFormSchema(tProjects);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("projects.unused\n");
}

#[test]
fn unused_traces_named_helper_import_with_dotted_file_stem() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[
            ("checklists.stage-ongoing", "Ongoing"),
            ("checklists.unused", "Unused"),
        ],
    );
    std::fs::write(
        dir.path().join("checklist-detail-page.shared.ts"),
        "export function getChecklistStatusLabel(status, tChecklists) { return tChecklists('stage-ongoing'); }\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getChecklistStatusLabel} from './checklist-detail-page.shared';\nconst tChecklists = useTranslations('checklists');\ngetChecklistStatusLabel('ONGOING', tChecklists);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("checklists.unused\n");
}

#[test]
fn unused_traces_default_helper_import_from_relative_file() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("settings.title", "Title"), ("settings.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("settings-helper.ts"),
        "export default function getTitle(tSettings) { return tSettings('title'); }\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport getTitle from './settings-helper';\nconst t = useTranslations('settings');\ngetTitle(t);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_traces_export_specifier_helper() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("settings.title", "Title"), ("settings.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("settings-helper.ts"),
        "function getTitle(tSettings) { return tSettings('title'); }\nexport { getTitle };\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getTitle} from './settings-helper';\nconst t = useTranslations('settings');\ngetTitle(t);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_traces_helper_import_through_tsconfig_path_alias() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("settings.title", "Title"), ("settings.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("tsconfig.json"),
        r#"{
          "compilerOptions": {
            "baseUrl": ".",
            "paths": {
              "@/*": ["src/*"]
            }
          }
        }"#,
    )
    .expect("write tsconfig");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(
        dir.path().join("src/settings-helper.ts"),
        "export function getTitle(tSettings) { return tSettings('title'); }\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getTitle} from '@/settings-helper';\nconst t = useTranslations('settings');\ngetTitle(t);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_traces_helper_import_via_index_file() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("settings.title", "Title"), ("settings.unused", "Unused")],
    );
    std::fs::create_dir_all(dir.path().join("helpers")).expect("mkdir helpers");
    std::fs::write(
        dir.path().join("helpers/index.ts"),
        "export function getTitle(tSettings) { return tSettings('title'); }\n",
    )
    .expect("write helper");
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getTitle} from './helpers';\nconst t = useTranslations('settings');\ngetTitle(t);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "--keys"])
        .assert()
        .success()
        .stdout("settings.unused\n");
}

#[test]
fn unused_unresolved_import_with_translator_argument_is_dynamic() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(dir.path(), &[("settings.title", "Title")]);
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nimport {getTitle} from './missing';\nconst t = useTranslations('settings');\ngetTitle(t);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .arg("unused")
        .assert()
        .success()
        .stdout(predicate::str::contains("Unused keys: 0"))
        .stdout(predicate::str::contains(
            "Warning: dynamic translation key usage detected in 1 place(s):",
        ));
}

#[test]
fn unused_remove_removes_safe_unused_keys() {
    let dir = tempdir().expect("tempdir");
    let config = setup_next_intl_project_with_keys(
        dir.path(),
        &[("app.title", "Title"), ("app.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt('title');\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "remove"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Removed 1 unused translation ids.",
        ));

    let en = load_language_translations(dir.path(), &config, "en").expect("load en");
    let nb = load_language_translations(dir.path(), &config, "nb").expect("load nb");
    assert!(en.contains_key("app.title"));
    assert!(!en.contains_key("app.unused"));
    assert!(!nb.contains_key("app.unused"));
}

#[test]
fn unused_remove_refuses_dynamic_usage_without_force() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(
        dir.path(),
        &[("app.title", "Title"), ("app.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt(key);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "remove"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "dynamic translation key usage detected",
        ));
}

#[test]
fn unused_remove_force_allows_dynamic_usage() {
    let dir = tempdir().expect("tempdir");
    let config = setup_next_intl_project_with_keys(
        dir.path(),
        &[("app.title", "Title"), ("app.unused", "Unused")],
    );
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useTranslations} from 'next-intl';\nconst t = useTranslations('app');\nt('title');\nt(key);\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "remove", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Removed 1 unused translation ids.",
        ));

    let en = load_language_translations(dir.path(), &config, "en").expect("load en");
    assert!(en.contains_key("app.title"));
    assert!(!en.contains_key("app.unused"));
}

#[test]
fn unused_remove_refuses_extraction_usage_even_with_force() {
    let dir = tempdir().expect("tempdir");
    setup_next_intl_project_with_keys(dir.path(), &[("app.unused", "Unused")]);
    std::fs::write(
        dir.path().join("page.tsx"),
        "import {useExtracted} from 'next-intl';\nconst t = useExtracted();\nt('Close');\n",
    )
    .expect("write source");

    trans_cmd()
        .current_dir(dir.path())
        .args(["unused", "remove", "--force"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "cannot remove unused keys while next-intl extraction usage is detected",
        ));
}

#[test]
fn unused_reports_react_intl_as_unsupported() {
    let dir = tempdir().expect("tempdir");
    setup_project(dir.path());

    trans_cmd()
        .current_dir(dir.path())
        .arg("unused")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "trans unused currently supports next-intl mode only",
        ));
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
