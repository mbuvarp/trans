use std::env;
use std::process;

use clap::Parser;

use trans::cli::{parse_values, Cli, Command};
use trans::config::TransConfig;
use trans::error::Result;
use trans::export::{export_csv, export_excel};
use trans::interactive::{init_config_interactive, run_interactive};
use trans::operations::{add_translation, change_message_id, delete_translation, update_translation};
use trans::query::{get_translation, get_translations_all, list_required_languages};
use trans::verify::verify_language_files;

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            let root = env::current_dir()?;
            run_interactive(&root, cli.message_id)
        }
        Some(Command::Init) => {
            let root = env::current_dir()?;
            init_config_interactive(&root)
        }
        Some(Command::Export { format }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            match format {
                trans::cli::ExportFormat::Csv => {
                    let path = export_csv(&root, &config)?;
                    println!("Exported CSV to {}", path.display());
                    Ok(())
                }
                trans::cli::ExportFormat::Excel => {
                    let path = export_excel(&root, &config)?;
                    println!("Exported Excel to {}", path.display());
                    Ok(())
                }
            }
        }
        Some(Command::ListRequiredLanguages) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            for language in list_required_languages(&config) {
                println!("{language}");
            }
            Ok(())
        }
        Some(Command::Add { id, values }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            let values = parse_values(&values)?;
            add_translation(&root, &config, &id, &values)
        }
        Some(Command::Update { id, values }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            let values = parse_values(&values)?;
            update_translation(&root, &config, &id, &values)
        }
        Some(Command::Delete { id }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            delete_translation(&root, &config, &id)
        }
        Some(Command::Show { id, lang }) => {
            let root = env::current_dir()?;
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
        Some(Command::ChangeId { old_id, new_id }) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            change_message_id(&root, &config, &old_id, &new_id)
        }
        Some(Command::Verify) => {
            let root = env::current_dir()?;
            let config = TransConfig::load_from_root(&root)?;
            verify_language_files(&root, &config)?;
            println!("OK");
            Ok(())
        }
    }
}
