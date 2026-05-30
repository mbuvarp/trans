use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;
use console::style;
use dialoguer::Confirm;

use trans::cli::{
    Cli, Command, ConfigFormat, ConfigKey, ConfigSection, UnusedCommand, parse_lang_list,
    parse_values,
};
use trans::config::{
    ConfigField, ConfigFormat as ConfigFileFormat, ConfigMode, ExportFormat, TransConfig,
    format_config_list,
};
use trans::error::{Result, TransError};
use trans::export::{export_csv, export_csv_with_options, export_excel, export_excel_with_options};
use trans::interactive::{
    configure_ai_interactive, configure_edit_interactive, configure_root_interactive,
    ensure_next_intl_strings, init_config_interactive, run_interactive,
};
use trans::operations::{
    add_language, add_translation, change_message_id, delete_language, delete_translation,
    update_translation,
};
use trans::query::{
    FindMatchKind, find_translations, get_translation, get_translations_all, has_message_id,
    list_required_languages,
};
use trans::sync::{apply_sync_plan, collect_missing_ids, maybe_prompt_sync};
use trans::translations::{migrate_language_files_to_dir, validate_language_file_migration};
use trans::update_check::{UpdateInfo, spawn_update_check};
use trans::verify::{collect_verification_issues, verify_language_files};
use trans::verify_ai::verify_with_ai;

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err}");
        process::exit(1);
    }
}

fn resolve_effective_cwd(cwd: Option<&Path>) -> Result<PathBuf> {
    let process_cwd = env::current_dir()?;
    let path = match cwd {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => process_cwd.join(path),
        None => process_cwd,
    };

    let path = std::fs::canonicalize(&path)?;
    if !path.is_dir() {
        return Err(TransError::InvalidInput(format!(
            "cwd is not a directory: {}",
            path.display()
        )));
    }

    Ok(path)
}

fn config_root(effective_cwd: &Path) -> Result<PathBuf> {
    TransConfig::find_root(effective_cwd)
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.version {
        println!("trans {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let effective_cwd = resolve_effective_cwd(cli.cwd.as_deref())?;
    let update_check = maybe_start_update_check(&cli, &effective_cwd)?;
    let result = match cli.command {
        None => {
            let root = config_root(&effective_cwd)?;
            run_interactive(&root, cli.message_id, cli.all)
        }
        Some(Command::Init { format }) => {
            let root = effective_cwd;
            init_config_interactive(
                &root,
                match format {
                    ConfigFormat::Json => ConfigFileFormat::Json,
                    ConfigFormat::Yaml => ConfigFileFormat::Yaml,
                },
            )
        }
        Some(Command::Export {
            format,
            lang,
            output,
            missing,
            no_lock,
        }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            let format = format.unwrap_or(config.default_export_format);
            let use_custom = lang.is_some() || output.is_some() || missing || no_lock;
            let selected = if let Some(lang) = lang.as_ref() {
                let langs = parse_lang_list(lang)?;
                let selected = resolve_export_languages(&config, &langs)?;
                let only_primary = langs.iter().all(|value| value == &config.primary_language);
                if only_primary {
                    eprintln!(
                        "Warning: only the primary language '{}' was provided; nothing to export.",
                        config.primary_language
                    );
                    None
                } else {
                    Some(selected)
                }
            } else {
                None
            };
            if use_custom && selected.is_some() {
                verify_language_files(&root, &config)?;
            }
            let output_path = build_export_path(&root, output.as_deref(), format);
            if lang.is_some() && selected.is_none() {
                Ok(())
            } else {
                match format {
                    ExportFormat::Csv => {
                        if let Some(langs) = selected.as_ref() {
                            let translations =
                                trans::export::load_selected_languages(&root, &config, langs)?;
                            export_csv_with_options(
                                &config,
                                &translations,
                                langs,
                                &output_path,
                                missing,
                            )?;
                            println!("Exported CSV to {}", output_path.display());
                            Ok(())
                        } else {
                            if use_custom {
                                let translations =
                                    trans::export::load_all_languages(&root, &config)?;
                                export_csv_with_options(
                                    &config,
                                    &translations,
                                    &config.available_languages,
                                    &output_path,
                                    missing,
                                )?;
                                println!("Exported CSV to {}", output_path.display());
                                Ok(())
                            } else {
                                let path = export_csv(&root, &config)?;
                                println!("Exported CSV to {}", path.display());
                                Ok(())
                            }
                        }
                    }
                    ExportFormat::Excel => {
                        if let Some(langs) = selected.as_ref() {
                            let translations =
                                trans::export::load_selected_languages(&root, &config, langs)?;
                            export_excel_with_options(
                                &config,
                                &translations,
                                langs,
                                &output_path,
                                missing,
                                !no_lock,
                            )?;
                            println!("Exported Excel to {}", output_path.display());
                            Ok(())
                        } else {
                            if use_custom {
                                let translations =
                                    trans::export::load_all_languages(&root, &config)?;
                                export_excel_with_options(
                                    &config,
                                    &translations,
                                    &config.available_languages,
                                    &output_path,
                                    missing,
                                    !no_lock,
                                )?;
                                println!("Exported Excel to {}", output_path.display());
                                Ok(())
                            } else {
                                let path = export_excel(&root, &config)?;
                                println!("Exported Excel to {}", path.display());
                                Ok(())
                            }
                        }
                    }
                }
            }
        }
        Some(Command::Import {
            path,
            lang,
            extra_langs,
            trim,
            ai,
        }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            let lang_filter = match lang {
                Some(lang) => Some(parse_lang_list(&lang)?),
                None => None,
            };
            trans::import::import_translations_with_ai(
                &root,
                &config,
                Path::new(&path),
                lang_filter,
                extra_langs,
                trim,
                ai,
            )
        }
        Some(Command::ListRequiredLanguages) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            for language in list_required_languages(&config) {
                println!("{language}");
            }
            Ok(())
        }
        Some(Command::Add { id, values, all }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            if all && values.is_some() {
                return Err(TransError::InvalidInput(
                    "cannot use --all together with --values".to_string(),
                ));
            }
            if all {
                ensure_next_intl_strings(&root, &config)?;
                let ai_settings = trans::ai::resolve_ai_settings(&root, &config)?;
                let values = trans::interactive::prompt_translations_for_languages(
                    &root,
                    &config,
                    &id,
                    &ai_settings,
                    &trans::interactive::languages_for_all(&config),
                    false,
                )?;
                add_translation(&root, &config, &id, &values)
            } else {
                let values = values.ok_or_else(|| {
                    TransError::InvalidInput("missing --values or --all".to_string())
                })?;
                let values = parse_values(&values)?;
                match add_translation(&root, &config, &id, &values) {
                    Err(err) => {
                        if maybe_prompt_sync(&root, &config, &err)? {
                            Ok(())
                        } else {
                            Err(err)
                        }
                    }
                    Ok(()) => Ok(()),
                }
            }
        }
        Some(Command::Update { id, values, all }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            if all && values.is_some() {
                return Err(TransError::InvalidInput(
                    "cannot use --all together with --values".to_string(),
                ));
            }
            if all {
                ensure_next_intl_strings(&root, &config)?;
                let ai_settings = trans::ai::resolve_ai_settings(&root, &config)?;
                let values = trans::interactive::prompt_translations_for_languages(
                    &root,
                    &config,
                    &id,
                    &ai_settings,
                    &trans::interactive::languages_for_all(&config),
                    true,
                )?;
                update_translation(&root, &config, &id, &values)
            } else {
                let values = values.ok_or_else(|| {
                    TransError::InvalidInput("missing --values or --all".to_string())
                })?;
                let values = parse_values(&values)?;
                match update_translation(&root, &config, &id, &values) {
                    Err(err) => {
                        if maybe_prompt_sync(&root, &config, &err)? {
                            Ok(())
                        } else {
                            Err(err)
                        }
                    }
                    Ok(()) => Ok(()),
                }
            }
        }
        Some(Command::Delete { id }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            match delete_translation(&root, &config, &id) {
                Err(err) => {
                    if maybe_prompt_sync(&root, &config, &err)? {
                        Ok(())
                    } else {
                        Err(err)
                    }
                }
                Ok(()) => Ok(()),
            }
        }
        Some(Command::Show { id, lang }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            if let Some(lang) = lang {
                let value = get_translation(&root, &config, &id, &lang)?;
                println!("{value}");
            } else {
                let results = get_translations_all(&root, &config, &id)?;
                for (language, value) in results {
                    println!("{language}: {value}");
                }
            }
            Ok(())
        }
        Some(Command::Has { id }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            let result = has_message_id(&root, &config, &id)?;
            if result.not_found.is_empty() {
                println!("found");
                Ok(())
            } else if result.found.is_empty() {
                println!("not found");
                process::exit(1);
            } else {
                println!("found: {}", result.found.join(", "));
                println!("not found: {}", result.not_found.join(", "));
                process::exit(2);
            }
        }
        Some(Command::Find {
            query,
            exact_only,
            case_sensitive,
            language,
        }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            let matches = find_translations(
                &root,
                &config,
                &query,
                language.as_deref(),
                exact_only,
                case_sensitive,
            )?;
            print_find_matches(&matches);
            Ok(())
        }
        Some(Command::Config { section, format }) => {
            let root = config_root(&effective_cwd)?;
            if let Some(format) = format {
                if section.is_some() {
                    return Err(TransError::InvalidInput(
                        "--format cannot be used with a config section".to_string(),
                    ));
                }
                let config = TransConfig::load_from_root(&root)?;
                let target = config.save_to_root_format(
                    &root,
                    match format {
                        ConfigFormat::Json => ConfigFileFormat::Json,
                        ConfigFormat::Yaml => ConfigFileFormat::Yaml,
                    },
                    true,
                )?;
                println!("Wrote config to {}", target.display());
                Ok(())
            } else {
                match section {
                    Some(ConfigSection::Ai) => configure_ai_interactive(&root),
                    Some(ConfigSection::Show) => {
                        let (config, path) = TransConfig::load_from_root_with_path(&root)?;
                        for line in format_config_list(&config, Some(&path)) {
                            println!("{line}");
                        }
                        Ok(())
                    }
                    Some(ConfigSection::Edit { key }) => {
                        let field = key.map(map_config_key);
                        configure_edit_interactive(&root, field)
                    }
                    None => configure_root_interactive(&root),
                }
            }
        }
        Some(Command::ChangeId { old_id, new_id }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            match change_message_id(&root, &config, &old_id, &new_id) {
                Err(err) => {
                    if maybe_prompt_sync(&root, &config, &err)? {
                        Ok(())
                    } else {
                        Err(err)
                    }
                }
                Ok(()) => Ok(()),
            }
        }
        Some(Command::Migrate {
            mode,
            out_dir,
            no_update_language_files_path,
            backup,
            check,
        }) => {
            let root = config_root(&effective_cwd)?;
            handle_migrate(
                &root,
                mode,
                out_dir,
                no_update_language_files_path,
                backup,
                check,
            )
        }
        Some(Command::Verify { ai }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            if ai {
                verify_with_ai(&root, &config)
            } else {
                let issues = collect_verification_issues(&root, &config)?;
                if issues.is_empty() {
                    println!("OK");
                    Ok(())
                } else {
                    println!("Found {} errors in translation files:\n", issues.len());
                    for (index, issue) in issues.iter().enumerate() {
                        let relative = issue.path.strip_prefix(&root).unwrap_or(&issue.path);
                        let display_path = format!("{}:{}", relative.display(), issue.line);
                        println!("{}", style(display_path).bold());
                        println!("{}", issue.message);
                        if index + 1 < issues.len() {
                            println!();
                        }
                    }
                    process::exit(1);
                }
            }
        }
        Some(Command::Unused { keys, command }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            match command {
                None => {
                    if keys {
                        let ids = trans::unused::find_unused_keys(&root, &config)?;
                        for id in &ids {
                            println!("{id}");
                        }
                    } else {
                        let report = trans::unused::find_unused(&root, &config)?;
                        println!("Unused keys: {}", style(report.unused_ids.len()).bold());
                        if !report.dynamic_usage_locations.is_empty() {
                            println!();
                            println!(
                                "{}",
                                style(format!(
                                    "Warning: dynamic translation key usage detected in {} place(s):",
                                    report.dynamic_usage_locations.len()
                                ))
                                .yellow()
                            );
                            for location in &report.dynamic_usage_locations {
                                println!(
                                    "{}",
                                    format_terminal_link(&location.display, &location.url)
                                );
                            }
                        }
                    }
                    Ok(())
                }
                Some(UnusedCommand::Remove { force }) => {
                    let report = trans::unused::remove_unused(&root, &config, force)?;
                    for warning in &report.warnings {
                        eprintln!("Warning: {warning}");
                    }
                    println!(
                        "Removed {} unused translation ids.",
                        report.unused_ids.len()
                    );
                    Ok(())
                }
            }
        }
        Some(Command::Sync) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            handle_sync(&root, &config)
        }
        Some(Command::Auto { lang, concurrency }) => {
            let root = config_root(&effective_cwd)?;
            let mut config = TransConfig::load_from_root(&root)?;
            if let Some(concurrency) = concurrency {
                if concurrency == 0 {
                    return Err(TransError::InvalidInput(
                        "concurrency must be at least 1".to_string(),
                    ));
                }
                let ai = config.ai.clone().unwrap_or_default();
                config.ai = Some(trans::config::AiConfig { concurrency, ..ai });
            }
            let lang_filter = match lang {
                Some(lang) => Some(parse_lang_list(&lang)?),
                None => None,
            };
            trans::auto::auto_translate(&root, &config, lang_filter)
        }
        Some(Command::AddLang { lang }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            add_language(&root, &config, &lang)
        }
        Some(Command::DelLang { lang }) => {
            let root = config_root(&effective_cwd)?;
            let config = TransConfig::load_from_root(&root)?;
            if lang == config.primary_language {
                return Err(TransError::InvalidInput(
                    "cannot delete the primary language".to_string(),
                ));
            }
            let confirmed = Confirm::new()
                .with_prompt(format!(
                    "Delete language '{lang}'? This will remove the language file and update config."
                ))
                .default(false)
                .interact()?;
            if confirmed {
                delete_language(&root, &config, &lang)
            } else {
                Ok(())
            }
        }
    };
    if result.is_ok() {
        maybe_prompt_update(update_check)?;
    }
    result
}

fn print_find_matches(matches: &[trans::query::FindMatch]) {
    let width = matches
        .iter()
        .map(|find_match| find_match.message_id.len())
        .max()
        .unwrap_or(0);

    for find_match in matches {
        println!(
            "{:<width$}  {}",
            find_match.message_id,
            format_find_match_kind(find_match.kind),
            width = width
        );
    }
}

fn format_find_match_kind(kind: FindMatchKind) -> String {
    match kind {
        FindMatchKind::Exact => style("exact").green().to_string(),
        FindMatchKind::Casing => style("casing").yellow().to_string(),
        FindMatchKind::Partial => style("partial").color256(208).to_string(),
    }
}

fn format_terminal_link(display: &str, url: &str) -> String {
    if std::io::stdout().is_terminal() {
        format!("\x1b]8;;{url}\x1b\\{display}\x1b]8;;\x1b\\")
    } else {
        display.to_string()
    }
}

fn handle_sync(root: &Path, config: &TransConfig) -> Result<()> {
    let plan = collect_missing_ids(root, config)?;
    let report = plan.report();
    if report.total_missing == 0 {
        println!("No missing IDs.");
        return Ok(());
    }

    println!("Found missing IDs:");
    for (language, count) in report.missing_by_language {
        println!("{language}: {count}");
    }
    println!();
    let default_message = if config.default_untranslated_value.is_empty() {
        "<empty>".to_string()
    } else {
        config.default_untranslated_value.clone()
    };
    let prompt = format!("Do you want to add the missing IDs with message \"{default_message}\"?");
    let confirmed = dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(true)
        .interact()?;
    if !confirmed {
        return Ok(());
    }

    let applied = apply_sync_plan(root, config, &plan)?.total_missing;
    if applied == 0 {
        println!("No missing IDs to add.");
    } else {
        println!("Added {applied} missing IDs.");
    }
    Ok(())
}

fn handle_migrate(
    root: &Path,
    target_mode: ConfigMode,
    out_dir: Option<String>,
    no_update_language_files_path: bool,
    backup: bool,
    check: bool,
) -> Result<()> {
    if no_update_language_files_path && out_dir.is_none() {
        return Err(TransError::InvalidInput(
            "--no-update-language-files-path requires --out-dir".to_string(),
        ));
    }

    let mut config = TransConfig::load_from_root(root)?;
    if config.mode == target_mode {
        if check {
            println!(
                "Check OK: config is already in '{}' mode.",
                target_mode.as_str()
            );
        } else {
            println!(
                "Config is already in '{}' mode. Nothing to migrate.",
                target_mode.as_str()
            );
        }
        return Ok(());
    }

    if check {
        validate_language_file_migration(root, &config, target_mode)?;
        println!(
            "Check OK: translations are valid for migration to '{}'.",
            target_mode.as_str()
        );
        return Ok(());
    }

    if backup {
        create_language_files_backup(root, &config)?;
    }

    let out_dir_abs = out_dir
        .as_deref()
        .map(|value| resolve_out_dir(root, value))
        .transpose()?;
    let out_dir_ref = out_dir_abs.as_deref();

    migrate_language_files_to_dir(root, &config, target_mode, out_dir_ref)?;

    config.mode = target_mode;
    if let Some(path) = out_dir_abs {
        if !no_update_language_files_path {
            config.language_files_path = normalize_config_path(root, &path);
        }
    }
    config.save_to_root(root)?;

    if let Some(out_dir) = out_dir {
        if no_update_language_files_path {
            println!(
                "Migrated language files to '{}' mode in {} (languageFilesPath unchanged).",
                target_mode.as_str(),
                out_dir
            );
        } else {
            println!(
                "Migrated language files to '{}' mode in {}.",
                target_mode.as_str(),
                config.language_files_path.display()
            );
        }
    } else {
        println!(
            "Migrated language files in place to '{}' mode.",
            target_mode.as_str()
        );
    }

    Ok(())
}

fn maybe_prompt_update(receiver: Option<std::sync::mpsc::Receiver<UpdateInfo>>) -> Result<()> {
    let receiver = match receiver {
        Some(receiver) => receiver,
        None => return Ok(()),
    };
    let info = match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
        Ok(info) => info,
        Err(_) => return Ok(()),
    };
    println!(
        "You are using trans {}, but {} is available. Update now?",
        style(format!("v{}", info.current)).bold(),
        style(format!("v{}", info.latest)).bold()
    );
    let confirmed = Confirm::new().default(true).interact()?;
    if !confirmed {
        return Ok(());
    }
    let status = std::process::Command::new("brew")
        .args(["upgrade", "trans"])
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(TransError::InvalidInput(
            "brew upgrade trans failed".to_string(),
        )),
        Err(err) => Err(TransError::InvalidInput(format!(
            "failed to run brew upgrade trans: {err}"
        ))),
    }
}

fn map_config_key(key: ConfigKey) -> ConfigField {
    match key {
        ConfigKey::Mode => ConfigField::Mode,
        ConfigKey::LanguageFilesPath => ConfigField::LanguageFilesPath,
        ConfigKey::AvailableLanguages => ConfigField::AvailableLanguages,
        ConfigKey::RequiredLanguages => ConfigField::RequiredLanguages,
        ConfigKey::PrimaryLanguage => ConfigField::PrimaryLanguage,
        ConfigKey::DefaultUntranslatedValue => ConfigField::DefaultUntranslatedValue,
        ConfigKey::DefaultExportFormat => ConfigField::DefaultExportFormat,
        ConfigKey::ExcelPassword => ConfigField::ExcelPassword,
        ConfigKey::RunUpdateCheck => ConfigField::RunUpdateCheck,
        ConfigKey::AiEnabled => ConfigField::AiEnabled,
        ConfigKey::AiModel => ConfigField::AiModel,
        ConfigKey::AiApiKeyEnv => ConfigField::AiApiKeyEnv,
        ConfigKey::AiMaxOutputTokens => ConfigField::AiMaxOutputTokens,
        ConfigKey::AiConcurrency => ConfigField::AiConcurrency,
    }
}

fn maybe_start_update_check(
    cli: &Cli,
    effective_cwd: &Path,
) -> Result<Option<std::sync::mpsc::Receiver<UpdateInfo>>> {
    if matches!(cli.command, Some(Command::Init { .. })) {
        return Ok(None);
    }
    let root = match TransConfig::find_root(effective_cwd) {
        Ok(root) => root,
        Err(_) => return Ok(None),
    };
    let config = match TransConfig::load_from_root(&root) {
        Ok(config) => config,
        Err(_) => return Ok(None),
    };
    if !config.run_update_check {
        return Ok(None);
    }
    Ok(spawn_update_check(env!("CARGO_PKG_VERSION")))
}

fn resolve_export_languages(config: &TransConfig, requested: &[String]) -> Result<Vec<String>> {
    if requested.is_empty() {
        return Err(TransError::InvalidInput(
            "languages must not be empty".to_string(),
        ));
    }

    let mut seen = HashSet::new();
    let mut selected = Vec::new();

    seen.insert(config.primary_language.clone());
    selected.push(config.primary_language.clone());

    for language in requested {
        if !config
            .available_languages
            .iter()
            .any(|lang| lang == language)
        {
            return Err(TransError::InvalidInput(format!(
                "language '{language}' is not in available_languages"
            )));
        }
        if seen.insert(language.clone()) {
            selected.push(language.clone());
        }
    }

    Ok(selected)
}

fn build_export_path(root: &Path, output: Option<&str>, format: ExportFormat) -> PathBuf {
    let base = output.unwrap_or("translations");
    let mut path = PathBuf::from(base);
    if path.extension().is_none() {
        let ext = match format {
            ExportFormat::Csv => "csv",
            ExportFormat::Excel => "xlsx",
        };
        path.set_extension(ext);
    }
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn resolve_out_dir(root: &Path, out_dir: &str) -> Result<PathBuf> {
    let path = PathBuf::from(out_dir);
    let resolved = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    std::fs::create_dir_all(&resolved)?;
    Ok(resolved)
}

fn normalize_config_path(root: &Path, out_dir: &Path) -> PathBuf {
    let root_abs = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let out_abs = std::fs::canonicalize(out_dir).unwrap_or_else(|_| out_dir.to_path_buf());

    match out_abs.strip_prefix(&root_abs) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_path_buf(),
        Ok(_) => PathBuf::from("."),
        Err(_) => out_abs,
    }
}

fn create_language_files_backup(root: &Path, config: &TransConfig) -> Result<()> {
    let source_dir = root.join(&config.language_files_path);
    let metadata = fs::metadata(&source_dir).map_err(|err| {
        TransError::InvalidInput(format!(
            "failed to read languageFilesPath '{}': {err}",
            source_dir.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(TransError::InvalidInput(format!(
            "languageFilesPath '{}' is not a directory",
            source_dir.display()
        )));
    }

    let backup_dir = backup_dir_path(&source_dir)?;
    if backup_dir.exists() {
        return Err(TransError::InvalidInput(format!(
            "backup directory already exists: {}",
            backup_dir.display()
        )));
    }

    copy_dir_recursive(&source_dir, &backup_dir)?;
    println!("Created backup at {}", backup_dir.display());
    Ok(())
}

fn backup_dir_path(source_dir: &Path) -> Result<PathBuf> {
    let parent = source_dir.parent().ok_or_else(|| {
        TransError::InvalidInput(format!(
            "failed to resolve backup path for '{}'",
            source_dir.display()
        ))
    })?;
    let name = source_dir
        .file_name()
        .ok_or_else(|| {
            TransError::InvalidInput(format!(
                "failed to resolve backup path for '{}'",
                source_dir.display()
            ))
        })?
        .to_string_lossy();
    Ok(parent.join(format!("{name}__backup")))
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let mut entries = fs::read_dir(source)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
