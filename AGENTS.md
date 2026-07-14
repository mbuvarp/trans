# Agent instructions

## Project overview

### Description

This will be a translation utility for react-intl translation JSON files. Have a look at the files in @spec/example-messages for the format of these JSON files. These are the things this program should be able to do:

- Read an optional `.trans.config.json` file in the project root that specifies:
    - Location of language files
    - Available languages
    - Required languages
        - These should be the languages for which translations are required
        - For languages that are in available languages but not in required languages, translations strings are set to a default value for later, manual translation
    - Primary language
        - This is the first language that is asked for in interactive mode
    - Default value for untranslated strings (default: "")
- Have a `trans init` command that asks the user for the above configuration options and creates the `.trans.config.json` file
- When `trans` is run without arguments, it should:
    - Ask user for a message ID
        - This ID _must_ have at least one "namespace", that is, it must be for example `app.header`, `app.header.title` but not just `title`
    - If the message ID exists in the primary language file, show the existing translation and ask if the user wants to update it or delete it
        - If the user wants to update it, ask for the new translation in all required languages
    - If the message ID does not exist in the primary language file, ask the user for a translation in all required languages
- Command line args for adding/updating/deleting translations without interactive mode:
    - `trans list-required-languages`: List all required languages
    - `trans add --id <message_id> --values <lang1>:<translation1>,<lang2>:<translation2>,...`
    - `trans update --id <message_id> --values <lang1>:<translation1>,<lang2>:<translation2>,...`
    - `trans delete --id <message_id>`
    - `trans show --id <message_id>`: Show translations for the given message ID
    - `trans show --id <message_id> --lang <language>`: Show translation for the given message ID in the specified language
    - `trans export [--format=csv/excel]`: Export a CSV or Excel file with all translations in all available languages
    - `trans verify`: Verify that all JSON files have the exact same message IDs
- If the config file is missing before any operation, show an error message asking the user to run `trans init` first
    - Check for the file in project root
- Verification should happen before and after every add/update/delete operation
    - If verification fails before, show an error and do not perform the operation
    - If verification fails after, revert the operation and show an error
- All command line args should be properly documented with `--help`
- JSON files should be sorted by key after every operation

# Tech stack

Written in Rust.

## Git usage

You should use git for version control. You are always allowed to use read-only commands, like `git log`, `git show`, `git status` and `git diff`. Before starting a new major feature, create a new branch with a descriptive name. You can do `git add` and `git commit` on these branches as you see fit. When you have completed a feature, create a pull request to merge your branch into `main`. Make sure to write a descriptive title and description for the pull request. After the pull request has been reviewed and approved, you can merge it into `main`. You can use the `gh` CLI tool to create pull requests.

Before making any commits, create and switch to a new feature branch (do not commit on `main`). Do not push directly to the `main` branch. Do not use destructive commands like `git reset --hard` or `git rebase` on the `main` branch. When creating PRs with `gh`, use `--body-file` (or a heredoc) so newlines render correctly in the description.

For commit messages, use the convetional commits format detailed here: https://gist.github.com/joshbuchea/6f47e86d2510bce28f8e7f42ae84c716. Be sure to mark commits created by you with "Created by codex" in the commit message body.

## Tests

I want extensive unit tests with wide coverage. Use the Rust testing framework to write tests for all major functions and modules. Make sure to test edge cases and error handling. You should also write integration tests for the CLI commands, to ensure they work as expected. Before creating apull request, make sure all tests pass and that code coverage is high.
