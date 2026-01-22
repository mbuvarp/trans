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

### Commands

- `sam install`: Initialize global config and create the database schema at `~/.config/sam/`.

- `sam create org [--name <name>]`: Create a new organization.
- `sam create project [--name <name>] [--prefix <prefix>]`: Create a new project.
- `sam show project <project_id>`: Show detailed information about a project.
- `sam list project`: List all projects.
- `sam delete project <project_id>`: Delete a project by ID.

- `sam create epic [--name <name>]`: Create a new epic in the current project.
- `sam show epic <epic_id>`: Show detailed information about an epic.
- `sam list epic`: List all epics in the current project.
- `sam update epic <epic_id> [--name <name>]`: Update an epic's name.
- `sam delete epic <epic_id>`: Delete an epic by ID.

- `sam create task --title <title> [--description <description>] [--depends <task_ids>] --prio <priority>`: Create a new task in the current project, with status "IDLE".
- `sam show task <task_id>`: Show detailed information about a task.
- `sam list task [--status <status>] [--prio <priority>] [--epic <epic_id>]`: List all tasks in the current project, optionally filtered by status, priority, or epic.
- `sam update task <task_id> [--title <title>] [--description <description>] [--status <status>] [--prio <priority>] [--depends <task_ids>] [--reserve-files <file_paths>] [--epic <epic_id>]`: Update a task's fields.
- `sam delete task <task_id>`: Delete a task by ID.

- `sam create user`: Create a new user.
- `sam update user --add-org <org_id> [--user <user_id>]`: Add a user to an organization.
- `sam update user --remove-org <org_id> [--user <user_id>]`: Remove a user from an organization.

## Git usage

You should use git for version control. You are always allowed to use read-only commands, like `git log`, `git show`, `git status` and `git diff`. Before starting a new major feature, create a new branch with a descriptive name. You can do `git add` and `git commit` on these branches as you see fit. When you have completed a feature, create a pull request to merge your branch into `main`. Make sure to write a descriptive title and description for the pull request. After the pull request has been reviewed and approved, you can merge it into `main`. You can use the `gh` CLI tool to create pull requests.

Do not push directly to the `main` branch. Do not use destructive commands like `git reset --hard` or `git rebase` on the `main` branch.

For commit messages, use the convetional commits format detailed here: https://gist.github.com/joshbuchea/6f47e86d2510bce28f8e7f42ae84c716. Be sure to mark commits created by you with "Created by codex" in the commit message body.

## Task management workflow

Use `sam` for all new work in this repo.

- Check current project context via `.sam.config.json`, then run `sam list task` before starting new work.
- Create a task for each new unit of work: `sam create task --title "<title>" --prio <LO|MD|HI|UR> [--description "..."] [--depends "..."] [--epic <epic_id>]`.
- Move a task to `PROG` before doing code changes: `sam update task <task_id> --status PROG`.
- If reserving files, only do so in `PROG` and clear them on status change: `sam update task <task_id> --reserve-files "path1,path2"`.
- Update status as work progresses (`REVU`, `DONE`) and keep titles/descriptions current.
- Use `sam show task <task_id>` to verify details and history.

## Tests

I want extensive unit tests with wide coverage. Use the Rust testing framework to write tests for all major functions and modules. Make sure to test edge cases and error handling. You should also write integration tests for the CLI commands, to ensure they work as expected. Before creating apull request, make sure all tests pass and that code coverage is high.