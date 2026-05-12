# trans

`trans` is a Rust CLI for managing application translations stored as JSON files.

It supports both:

- `react-intl` style flat message files
- `next-intl` style nested message files

The tool covers interactive editing, non-interactive batch updates, verification, import/export, mode migration, language management, and optional AI-assisted translation workflows.

## Features

- Interactive setup with `trans init`
- Interactive add/edit/delete flow when running `trans` with no subcommand
- Support for `.trans.config.json` and `.trans.config.yaml`
- `react-intl` and `next-intl` storage modes
- Add, update, delete, show, rename, sync, import, export, and verify commands
- CSV and Excel export
- CSV and Excel import
- Add and remove language files
- AI suggestions for interactive translation, verification fixes, import fixes, and auto-translation
- Verification before and after mutations, with rollback on failure
- Stable sorted output via `BTreeMap`-backed JSON writes

## Install

Install with Homebrew:

```bash
brew tap mbuvarp/trans
brew install trans
```

Build or run from source:

```bash
cargo run -- --help
```

Install locally:

```bash
cargo install --path .
```

Then use:

```bash
trans --help
```

## Quick Start

Initialize config in the project root:

```bash
trans init
```

Run the interactive flow:

```bash
trans
```

Open the interactive flow for a specific message ID:

```bash
trans app.header.title
```

Add a translation non-interactively:

```bash
trans add --id app.header.title --values en:Hello,nb:Hallo
```

Verify all language files:

```bash
trans verify
```

## Configuration

`trans` looks for exactly one config file in the current directory, then walks up parent directories until it finds one:

- `.trans.config.json`
- `.trans.config.yaml`

If neither file exists, most commands fail and tell you to run `trans init`.
Paths in the config, such as `languageFilesPath`, are resolved relative to the directory containing the discovered config file.

Example JSON config:

```json
{
  "mode": "react-intl",
  "languageFilesPath": "messages",
  "availableLanguages": ["en", "nb", "de"],
  "requiredLanguages": ["en", "nb"],
  "primaryLanguage": "en",
  "defaultUntranslatedValue": "",
  "defaultExportFormat": "excel",
  "excelPassword": "unlock",
  "runUpdateCheck": false,
  "ai": {
    "enabled": true,
    "model": "gpt-5-mini",
    "apiKeyEnv": "OPENAI_API_KEY",
    "maxOutputTokens": 128,
    "concurrency": 2
  }
}
```

Key fields:

- `mode`: `react-intl` or `next-intl`
- `languageFilesPath`: directory containing `<lang>.json`
- `availableLanguages`: all managed locales
- `requiredLanguages`: languages prompted for during add/update flows
- `primaryLanguage`: source language and reference set for verification
- `defaultUntranslatedValue`: value written for non-required or missing translations
- `defaultExportFormat`: `csv` or `excel`
- `excelPassword`: password used for Excel sheet protection
- `runUpdateCheck`: if `true`, successful commands may prompt for `brew upgrade trans`
- `ai`: optional AI configuration

Use `trans config --help` for interactive config editing and format conversion between JSON and YAML.

## Translation File Modes

`react-intl` mode uses flat keys:

```json
{
  "app.header.title": "Hello"
}
```

`next-intl` mode stores the same message as nested JSON:

```json
{
  "app": {
    "header": {
      "title": "Hello"
    }
  }
}
```

In both modes, the CLI works with dotted message IDs such as `app.header.title`.

Message IDs must include at least one namespace segment. `title` is invalid, while `app.title` is valid.

## Commands

Main commands:

- `trans`: interactive add/edit/delete flow
- `trans init`: create config interactively
- `trans list-required-languages`: print required languages
- `trans add --id <id> --values <lang:value,...>`: add a new message
- `trans update --id <id> --values <lang:value,...>`: update an existing message
- `trans delete --id <id>`: remove a message from all languages
- `trans show --id <id> [--lang <lang>]`: show one message in all or one language
- `trans change-id <old_id> <new_id>`: rename a message across all languages
- `trans verify [--ai]`: check for key mismatches and format issues
- `trans sync`: add missing IDs from the primary language into other languages
- `trans export [--format csv|excel]`: export all translations
- `trans import <file>`: import translations from CSV or Excel
- `trans migrate <mode>`: convert between `react-intl` and `next-intl`
- `trans auto`: fill missing translations with AI
- `trans add-lang <lang>`: add a new language file
- `trans del-lang <lang>`: remove a language file
- `trans config`: inspect or edit config values

Run `trans --help` or `trans <command> --help` for the full option set.

Global options:

- `-C, --cwd <DIR>`: run as if `trans` was started in `DIR`; config discovery starts there, and relative project paths still resolve from the discovered config directory.

## Interactive Mode

Running `trans` without a subcommand starts an interactive workflow:

- prompts for a message ID unless one was passed positionally
- shows the existing primary-language value if the ID already exists
- lets you update or delete an existing message
- prompts for translations in required languages
- supports `--all` to prompt for every available language

Examples:

```bash
trans
trans --all
trans app.header.title
```

### `/ai` in Add and Update Flows

During interactive translation prompts, you can type `/ai` instead of a translation to ask for an AI suggestion.

This applies to:

- `trans`
- `trans <message_id>`
- `trans add --id <id> --all`
- `trans update --id <id> --all`

You can also provide guidance inline:

```text
/ai
/ai keep the tone formal
/ai use the word "shipment" instead of "delivery"
```

Behavior:

- the primary language must be entered manually first
- `/ai` is only available for non-primary languages
- if AI is configured, `trans` asks the model for a suggestion using the primary-language text as source
- existing translations in other languages can be used as extra reference context
- after a suggestion is returned, you can accept it, ask the AI for another version or instruction, or write a custom translation manually

If AI is not configured, the prompt falls back to manual input.

## Import and Export

Export:

```bash
trans export --format csv
trans export --format excel --output translations-review
trans export --lang nb,de --missing
```

Notes:

- `export --lang` always includes the primary language automatically
- `--missing` exports only rows containing untranslated values
- Excel exports are protected by default; use `--no-lock` to disable protection

Import:

```bash
trans import translations.csv
trans import translations.xlsx --lang nb,de
trans import translations.xlsx --extra-langs create --trim
trans import translations.csv --ai
```

The import/export tabular format uses `id` as the first column and one column per language, for example:

```csv
id,en,nb
app.header.title,Hello,Hallo
```

## Verification and Safety

Mutation commands such as `add`, `update`, `delete`, and `change-id` verify the language files before making changes.

They also verify again after writing files. If post-write verification fails, the operation is rolled back to the previous snapshot.

Other safety-related behavior:

- JSON output is written in sorted key order
- `verify` reports missing keys, extra keys, invalid JSON, and message format problems
- `sync` fills missing IDs in non-primary languages using `defaultUntranslatedValue`
- `del-lang` refuses to delete the primary language
- `next-intl` mode requires string leaf values

## AI Features

AI support is optional and configured under the `ai` section in the config file.

Supported AI-assisted flows:

- interactive translation suggestions
- `trans verify --ai`
- `trans import --ai`
- `trans auto`

The API key is read from the environment variable named by `ai.apiKeyEnv`. `.env` in the project root is loaded automatically.

Example:

```dotenv
OPENAI_API_KEY=your-key-here
```

## Migration

Convert translation files between storage modes:

```bash
trans migrate next-intl
trans migrate react-intl --check
trans migrate next-intl --backup
trans migrate next-intl --out-dir converted/messages
```

Useful flags:

- `--check`: validate compatibility without writing files
- `--backup`: create `<languageFilesPath>__backup` before migration
- `--out-dir`: write converted files to a different directory
- `--no-update-language-files-path`: keep the existing config path when using `--out-dir`

Migration to `next-intl` fails if dotted keys would conflict with nested object paths.

## Development

Run tests:

```bash
cargo test
```

The repository includes unit tests and CLI integration tests under `tests/cli.rs`.
