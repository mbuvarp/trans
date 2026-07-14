#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./release.sh <version|patch|minor|major> [--notes <text>] [--notes-file <path>] [--dry-run]

Examples:
  ./release.sh v0.1.1
  ./release.sh patch
  ./release.sh minor
  ./release.sh major
  ./release.sh v0.1.1 --notes "Bug fixes"
  ./release.sh v0.1.1 --notes-file /path/to/notes.md
  ./release.sh v0.1.1 --dry-run

Notes:
  - Releases must be created from the main branch.
  - If the tag starts with "v", Cargo.toml is set to the version without the prefix.
  - Cargo.toml and Cargo.lock are updated, committed, and pushed to origin/main when needed.
  - If no notes are provided, a changelog draft is generated and you will be prompted to approve/edit it.
  - If the `codex` CLI is available, it is used to draft the changelog; otherwise a local heuristic is used.
  - Successful command output is hidden; captured output is shown when a step fails.
  - After publishing, the script waits up to 120 seconds for the Homebrew update PR.
  - When the Homebrew PR is available, you can wait for its bottle checks and publish it automatically.
  - Bottle checks are polled every 10 seconds for up to 30 minutes by default.
  - Do not merge the Homebrew PR directly; publish it with the tap's brew pr-pull workflow.
USAGE
}

progress_tmp_dir=""
spinner_pid=""
cursor_hidden="false"
step_index=0
release_url=""
homebrew_pr_url=""
homebrew_pr_number=""
homebrew_pr_head_sha=""
homebrew_pr_head_branch=""
homebrew_publish_url=""
homebrew_publish_dispatched="false"
homebrew_tap_repository="${HOMEBREW_TAP_REPOSITORY:-mbuvarp/homebrew-trans}"

if [[ -t 2 && "${TERM:-}" != "dumb" ]]; then
  progress_is_tty="true"
else
  progress_is_tty="false"
fi

if [[ "$progress_is_tty" == "true" && -z "${NO_COLOR:-}" ]]; then
  color_green=$'\033[32m'
  color_red=$'\033[31m'
  color_yellow=$'\033[33m'
  color_cyan=$'\033[36m'
  color_reset=$'\033[0m'
else
  color_green=""
  color_red=""
  color_yellow=""
  color_cyan=""
  color_reset=""
fi

cleanup_progress() {
  if [[ -n "$spinner_pid" ]]; then
    kill "$spinner_pid" >/dev/null 2>&1 || true
    wait "$spinner_pid" 2>/dev/null || true
    spinner_pid=""
  fi
  if [[ "$cursor_hidden" == "true" ]]; then
    printf '\033[?25h' >&2
    cursor_hidden="false"
  fi
  if [[ -n "$progress_tmp_dir" && -d "$progress_tmp_dir" ]]; then
    rm -rf "$progress_tmp_dir"
  fi
}

trap cleanup_progress EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

status_line() {
  local status="$1"
  local color="$2"
  local label="$3"
  printf "  %s%s%s  %s\n" "$color" "$status" "$color_reset" "$label" >&2
}

print_ok() {
  status_line "ok" "$color_green" "$1"
}

print_warning() {
  status_line "--" "$color_yellow" "$1"
}

print_failure() {
  status_line "failed" "$color_red" "$1"
}

spinner_loop() {
  local label="$1"
  local frames=("⠋" "⠙" "⠹" "⠸" "⠼" "⠴" "⠦" "⠧" "⠇" "⠏")
  local index=0
  trap 'exit 0' INT TERM
  while true; do
    printf "\r\033[2K  %s%s%s  %s" \
      "$color_cyan" "${frames[$index]}" "$color_reset" "$label" >&2
    index=$(((index + 1) % ${#frames[@]}))
    sleep 0.08
  done
}

start_step() {
  local label="$1"
  if [[ "$progress_is_tty" == "true" ]]; then
    printf '\033[?25l' >&2
    cursor_hidden="true"
    spinner_loop "$label" &
    spinner_pid=$!
  else
    status_line ".." "" "$label"
  fi
}

finish_step() {
  local status="$1"
  local label="$2"
  if [[ -n "$spinner_pid" ]]; then
    kill "$spinner_pid" >/dev/null 2>&1 || true
    wait "$spinner_pid" 2>/dev/null || true
    spinner_pid=""
  fi
  if [[ "$progress_is_tty" == "true" ]]; then
    printf '\r\033[2K\033[?25h' >&2
    cursor_hidden="false"
  fi
  case "$status" in
    ok) print_ok "$label" ;;
    warning) print_warning "$label" ;;
    failed) print_failure "$label" ;;
  esac
}

show_step_log() {
  local log_file="$1"
  if [[ -s "$log_file" ]]; then
    printf "\nStep output:\n" >&2
    sed 's/^/  /' "$log_file" >&2
  fi
}

run_step() {
  local label="$1"
  shift
  step_index=$((step_index + 1))
  local log_file="${progress_tmp_dir}/step-${step_index}.log"
  start_step "$label"
  if "$@" >"$log_file" 2>&1; then
    finish_step ok "$label"
    return 0
  else
    local status=$?
    finish_step failed "$label"
    show_step_log "$log_file"
    return "$status"
  fi
}

if [[ ${1:-} == "" || ${1:-} == "-h" || ${1:-} == "--help" ]]; then
  usage
  exit 0
fi

requested_version="$1"
version="$requested_version"
shift || true

notes=""
notes_file=""
dry_run="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --notes)
      notes="$2"
      shift 2
      ;;
    --notes-file)
      notes_file="$2"
      shift 2
      ;;
    --dry-run)
      dry_run="true"
      shift 1
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

progress_tmp_dir="$(mktemp -d -t trans-release.XXXXXX)"

validate_tools() {
  if ! command -v gh >/dev/null 2>&1; then
    echo "gh CLI is required. Install from https://cli.github.com" >&2
    return 1
  fi
  if ! command -v git >/dev/null 2>&1; then
    echo "git is required." >&2
    return 1
  fi
  if ! command -v python3 >/dev/null 2>&1; then
    echo "python3 is required to update Cargo.toml." >&2
    return 1
  fi
}

run_step "Check release tools" validate_tools

current_cargo_version() {
  python3 - <<'PY'
from pathlib import Path

lines = Path("Cargo.toml").read_text(encoding="utf-8").splitlines()
in_pkg = False
for line in lines:
  stripped = line.strip()
  if stripped == "[package]":
    in_pkg = True
    continue
  if in_pkg and stripped.startswith("[") and stripped.endswith("]"):
    break
  if in_pkg and stripped.startswith("version"):
    value = stripped.split("=", 1)[1].strip().strip('"')
    print(value)
    break
else:
  raise SystemExit("version not found in Cargo.toml")
PY
}

bump_cargo_version() {
  local current="$1"
  local mode="$2"
  python3 - "$current" "$mode" <<'PY'
import re
import sys

version = sys.argv[1]
mode = sys.argv[2]
match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version)
if not match:
    raise SystemExit(f"unsupported Cargo.toml version '{version}', expected x.y.z")
major, minor, patch = map(int, match.groups())
if mode == "patch":
    patch += 1
elif mode == "minor":
    minor += 1
    patch = 0
elif mode == "major":
    major += 1
    minor = 0
    patch = 0
else:
    raise SystemExit(f"unsupported bump mode '{mode}'")
print(f"{major}.{minor}.{patch}")
PY
}

if [[ -n "$notes" && -n "$notes_file" ]]; then
  echo "Use either --notes or --notes-file, not both." >&2
  exit 1
fi

if [[ "$dry_run" == "true" && (-n "$notes" || -n "$notes_file") ]]; then
  echo "--dry-run cannot be used with --notes or --notes-file." >&2
  exit 1
fi

resolve_release_version() {
  current_version="$(current_cargo_version)" || return $?
  if [[ "$requested_version" == "patch" || "$requested_version" == "minor" || "$requested_version" == "major" ]]; then
    next_cargo_version="$(bump_cargo_version "$current_version" "$requested_version")" || return $?
    version="v${next_cargo_version}"
  fi
}

validate_release_state() {
  local current_branch
  current_branch="$(git branch --show-current)" || return $?
  if [[ "$current_branch" != "main" ]]; then
    if [[ -z "$current_branch" ]]; then
      current_branch="detached HEAD"
    fi
    echo "Releases must be created from the main branch (current: $current_branch)." >&2
    return 1
  fi
  if [[ -n "$notes_file" && ! -f "$notes_file" ]]; then
    echo "Release notes file does not exist: $notes_file" >&2
    return 1
  fi
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "Working tree is dirty. Commit or stash changes first." >&2
    return 1
  fi
  if git rev-parse --verify "refs/tags/${version}" >/dev/null 2>&1; then
    echo "Tag $version already exists." >&2
    return 1
  fi

  local remote_tag_status=0
  git ls-remote --exit-code --tags origin "refs/tags/${version}" >/dev/null 2>&1 \
    || remote_tag_status=$?
  case "$remote_tag_status" in
    0)
      echo "Tag $version already exists on origin." >&2
      return 1
      ;;
    2) ;;
    *)
      echo "Could not check whether tag $version exists on origin." >&2
      return "$remote_tag_status"
      ;;
  esac
}

run_step "Resolve release version" resolve_release_version
run_step "Validate release state" validate_release_state

if [[ "$requested_version" == "patch" || "$requested_version" == "minor" || "$requested_version" == "major" ]]; then
  echo
  echo "Bump mode: $requested_version"
  echo "Current version: v${current_version}"
  echo "Version to release: ${version}"
  read -r -p "Proceed with ${version}? [Y/n] " reply
  reply="${reply:-Y}"
  if [[ ! "$reply" =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 1
  fi
fi

prepare_changelog() {
  local tmp_file="$1"
  local last_tag
  last_tag="$(git describe --tags --abbrev=0 2>/dev/null || true)"
  local range="HEAD"
  if [[ -n "$last_tag" ]]; then
    range="${last_tag}..HEAD"
  fi

  local commits
  commits="$(git log "$range" --pretty=%s --no-merges)" || return $?

  if command -v codex >/dev/null 2>&1; then
    local prompt
    prompt="$(
      cat <<PROMPT
You are drafting release notes for the trans CLI. Summarize changes since ${last_tag:-the last release}.
Group related changes into sections when it improves readability. Use concise, user-facing descriptions (not raw commit prefixes).
Keep the output in markdown and do not include any extra commentary.
Do not create tasks or modify git. You may only run read-only git commands (e.g. log, show, diff, status). Only return the changelog text.

Commit subjects:
PROMPT
    )"
    local ai_output
    ai_output="$(codex exec "${prompt}"$'\n'"${commits}")" || true
    if [[ -n "${ai_output// }" ]]; then
      local cleaned
      cleaned="$(python3 - "$ai_output" <<'PY'
import sys

lines = sys.argv[1].splitlines()
start = None
for idx, line in enumerate(lines):
  if line.startswith("#"):
    start = idx
    break
if start is None:
  sys.exit(0)

stop_patterns = (
  "tokens used",
  "OpenAI Codex",
  "workdir:",
  "model:",
  "provider:",
  "approval:",
  "sandbox:",
  "reasoning effort:",
  "reasoning summaries:",
  "session id:",
  "----",
  "user",
  "thinking",
)
out = []
for line in lines[start:]:
  if any(line.startswith(p) for p in stop_patterns):
    break
  out.append(line)

while out and not out[-1].strip():
  out.pop()

print("\n".join(out))
PY
)"
      if [[ -n "${cleaned// }" ]]; then
        printf "%s\n" "$cleaned" >"$tmp_file" || return $?
        return 0
      fi
    fi
  fi

  python3 - "$commits" <<'PY' >"$tmp_file"
import sys

raw = sys.argv[1]
lines = [line.strip() for line in raw.splitlines() if line.strip()]

sections = {
    "Highlights": [],
    "Translation workflows": [],
    "AI + validation": [],
    "Release/CI": [],
    "Other": [],
}

def add(section, text):
    if text not in sections[section]:
        sections[section].append(text)

for line in lines:
    lower = line.lower()
    if lower.startswith("feat:"):
        text = line.split(":", 1)[1].strip()
        if "import" in lower:
            add("Translation workflows", text)
        elif "ai" in lower or "verify" in lower:
            add("AI + validation", text)
        else:
            add("Highlights", text)
    elif lower.startswith("fix:"):
        add("Highlights", line.split(":", 1)[1].strip())
    elif lower.startswith(("ci:", "chore:")) and ("brew" in lower or "release" in lower or "workflow" in lower):
        add("Release/CI", line.split(":", 1)[1].strip())
    elif lower.startswith(("ci:", "chore:", "docs:", "refactor:", "test:")):
        add("Other", line.split(":", 1)[1].strip())
    else:
        add("Other", line)

print("## Changes")
for section, items in sections.items():
    if not items:
        continue
    print()
    print(f"### {section}")
    for item in items:
        print(f"- {item}")
PY
}

approve_changelog() {
  local file="$1"
  echo
  echo "Draft changelog:"
  echo "----------------------------------------"
  cat "$file"
  echo "----------------------------------------"
  echo
  read -r -p "Use this changelog? [Y/n] " reply
  reply="${reply:-Y}"
  if [[ "$reply" =~ ^[Yy]$ ]]; then
    return 0
  fi

  local editor="${EDITOR:-vi}"
  "$editor" "$file"
  read -r -p "Use the edited changelog? [Y/n] " reply2
  reply2="${reply2:-Y}"
  if [[ "$reply2" =~ ^[Yy]$ ]]; then
    return 0
  fi

  echo "Aborted." >&2
  return 1
}

if [[ -z "$notes" && -z "$notes_file" ]]; then
  tmp_notes="${progress_tmp_dir}/release-notes.md"
  run_step "Prepare changelog" prepare_changelog "$tmp_notes"
  approve_changelog "$tmp_notes"
  notes_file="$tmp_notes"
elif [[ -n "$notes_file" ]]; then
  print_ok "Use release notes from $notes_file"
else
  print_ok "Use supplied release notes"
fi

if [[ "$dry_run" == "true" ]]; then
  print_ok "Complete dry run"
  echo
  echo "Dry run complete. No version bump, tag, or release created."
  exit 0
fi

cargo_version="${version#v}"

update_cargo_version() {
  python3 - <<PY
from pathlib import Path

new_version = "${cargo_version}"
path = Path("Cargo.toml")
lines = path.read_text(encoding="utf-8").splitlines()
out = []
in_pkg = False
updated = False
for line in lines:
  stripped = line.strip()
  if stripped == "[package]":
    in_pkg = True
    out.append(line)
    continue
  if in_pkg and stripped.startswith("[") and stripped.endswith("]"):
    in_pkg = False
  if in_pkg and stripped.startswith("version"):
    prefix = line.split("=", 1)[0]
    out.append(f"{prefix}= \"{new_version}\"")
    updated = True
    continue
  out.append(line)
if not updated:
  raise SystemExit("version not found in Cargo.toml")
path.write_text("\n".join(out) + "\n", encoding="utf-8")
PY
}

commit_release_version() {
  git add Cargo.toml Cargo.lock || return $?
  git commit -m "chore: release ${version}"
}

create_github_release() {
  local output
  if [[ -n "$notes_file" ]]; then
    output="$(gh release create "$version" --title "$version" --notes-file "$notes_file")" || return $?
  elif [[ -n "$notes" ]]; then
    output="$(gh release create "$version" --title "$version" --notes "$notes")" || return $?
  else
    output="$(gh release create "$version" --title "$version" --generate-notes)" || return $?
  fi
  release_url="${output##*$'\n'}"
  if [[ "$release_url" != http://* && "$release_url" != https://* ]]; then
    release_url="$(gh release view "$version" --json url --jq .url)" || return $?
  fi
}

wait_for_homebrew_pr() {
  local wait_seconds="${HOMEBREW_PR_WAIT_SECONDS:-120}"
  local poll_seconds="${HOMEBREW_PR_POLL_SECONDS:-5}"
  local branch="update-trans-${version}"
  local result

  if [[ ! "$wait_seconds" =~ ^[0-9]+$ || ! "$poll_seconds" =~ ^[0-9]+$ || "$poll_seconds" == "0" ]]; then
    echo "Homebrew PR polling intervals must be positive integers." >&2
    return 1
  fi

  local deadline=$((SECONDS + wait_seconds))
  while ((SECONDS <= deadline)); do
    if ! result="$(
      gh pr list \
        --repo "$homebrew_tap_repository" \
        --head "$branch" \
        --state all \
        --json number,url,headRefOid,headRefName \
        --jq '.[0] | select(. != null) | [.number, .url, .headRefOid, .headRefName] | @tsv'
    )"; then
      return 1
    fi
    if [[ -n "$result" ]]; then
      IFS=$'\t' read -r \
        homebrew_pr_number \
        homebrew_pr_url \
        homebrew_pr_head_sha \
        homebrew_pr_head_branch <<<"$result"
      if [[ ! "$homebrew_pr_number" =~ ^[0-9]+$ \
        || ! "$homebrew_pr_head_sha" =~ ^[0-9a-fA-F]{40}$ \
        || -z "$homebrew_pr_url" \
        || -z "$homebrew_pr_head_branch" ]]; then
        echo "Homebrew PR metadata was incomplete: $result" >&2
        return 1
      fi
      return 0
    fi
    if ((SECONDS >= deadline)); then
      break
    fi
    sleep "$poll_seconds"
  done
  return 2
}

evaluate_homebrew_checks() {
  local checks_json="$1"
  python3 - "$checks_json" <<'PY'
import json
import sys

expected = {
    "test-bot-macos (macos-15-intel)",
    "test-bot-macos (macos-26)",
    "test-bot-linux",
}

try:
    checks = json.loads(sys.argv[1])
except json.JSONDecodeError as error:
    raise SystemExit(f"Could not parse Homebrew check results: {error}")

by_name = {check.get("name", ""): check.get("bucket", "") for check in checks}
missing = sorted(expected - by_name.keys())
failed = sorted(
    name for name, bucket in by_name.items()
    if bucket in {"fail", "cancel", "skipping"}
)
pending = sorted(name for name, bucket in by_name.items() if bucket != "pass")

if failed:
    print("failed\tHomebrew checks did not succeed: " + ", ".join(failed))
elif missing:
    print("pending\tWaiting for Homebrew checks to appear: " + ", ".join(missing))
elif pending:
    print("pending\tWaiting for Homebrew checks: " + ", ".join(pending))
else:
    print("passed")
PY
}

wait_for_homebrew_checks() {
  local wait_seconds="${HOMEBREW_CHECK_WAIT_SECONDS:-1800}"
  local poll_seconds="${HOMEBREW_CHECK_POLL_SECONDS:-10}"
  local checks=""
  local evaluation=""
  local last_message="Waiting for Homebrew checks to appear."

  if [[ ! "$wait_seconds" =~ ^[0-9]+$ \
    || ! "$poll_seconds" =~ ^[0-9]+$ \
    || "$poll_seconds" == "0" ]]; then
    echo "Homebrew check polling intervals must be positive integers." >&2
    return 1
  fi

  local deadline=$((SECONDS + wait_seconds))
  while ((SECONDS <= deadline)); do
    local check_status=0
    checks="$(
      gh pr checks "$homebrew_pr_number" \
        --repo "$homebrew_tap_repository" \
        --json name,bucket,state 2>&1
    )" || check_status=$?

    if [[ "$check_status" -ne 0 && "$check_status" -ne 8 ]]; then
      if [[ "$checks" == *"no checks reported"* ]]; then
        checks="[]"
      elif [[ "$checks" == \[* ]]; then
        : # Failed checks still produce usable JSON with a non-zero exit status.
      else
        printf '%s\n' "$checks" >&2
        return "$check_status"
      fi
    fi

    evaluation="$(evaluate_homebrew_checks "$checks")" || return $?
    case "${evaluation%%$'\t'*}" in
      passed)
        return 0
        ;;
      failed)
        echo "${evaluation#*$'\t'}" >&2
        return 1
        ;;
      pending)
        last_message="${evaluation#*$'\t'}"
        ;;
      *)
        echo "Unexpected Homebrew check evaluation: $evaluation" >&2
        return 1
        ;;
    esac

    if ((SECONDS >= deadline)); then
      break
    fi
    sleep "$poll_seconds"
  done

  echo "Timed out after ${wait_seconds} seconds. $last_message" >&2
  return 2
}

verify_homebrew_pr_head() {
  local result
  result="$(
    gh pr view "$homebrew_pr_number" \
      --repo "$homebrew_tap_repository" \
      --json state,headRefOid,headRefName,headRepository \
      --jq '[.state, .headRefOid, .headRefName, .headRepository.nameWithOwner] | @tsv'
  )" || return $?

  local state current_sha current_branch current_repository
  IFS=$'\t' read -r state current_sha current_branch current_repository <<<"$result"
  if [[ "$state" != "OPEN" ]]; then
    echo "Homebrew PR #${homebrew_pr_number} is no longer open." >&2
    return 1
  fi
  if [[ "$current_repository" != "$homebrew_tap_repository" ]]; then
    echo "Homebrew PR #${homebrew_pr_number} no longer comes from ${homebrew_tap_repository}." >&2
    return 1
  fi
  if [[ "$current_branch" != "$homebrew_pr_head_branch" ]]; then
    echo "Homebrew PR #${homebrew_pr_number} changed head branch while checks were running." >&2
    return 1
  fi
  if [[ "$current_sha" != "$homebrew_pr_head_sha" ]]; then
    echo "Homebrew PR #${homebrew_pr_number} changed head SHA while checks were running." >&2
    return 1
  fi
}

dispatch_homebrew_publish() {
  local actor=""
  local existing_run_ids=""
  local run_snapshot_available="false"
  if actor="$(gh api user --jq .login 2>/dev/null)" \
    && existing_run_ids="$(
      gh run list \
        --repo "$homebrew_tap_repository" \
        --workflow publish.yml \
        --event workflow_dispatch \
        --branch main \
        --user "$actor" \
        --limit 20 \
        --json databaseId \
        --jq '.[].databaseId' 2>/dev/null
    )"; then
    run_snapshot_available="true"
  fi

  local output
  output="$(
    gh workflow run publish.yml \
      --repo "$homebrew_tap_repository" \
      --ref main \
      --raw-field "pull_request=${homebrew_pr_number}" \
      --raw-field "head_sha=${homebrew_pr_head_sha}"
  )" || return $?
  homebrew_publish_dispatched="true"

  homebrew_publish_url="$(printf '%s\n' "$output" | sed -nE 's#.*(https://github.com/[^[:space:]]+/actions/runs/[0-9]+).*#\1#p' | tail -n 1)"
  if [[ -z "$homebrew_publish_url" && "$run_snapshot_available" == "true" ]]; then
    find_dispatched_homebrew_run "$actor" "$existing_run_ids" || true
  fi
}

find_dispatched_homebrew_run() {
  local actor="$1"
  local existing_run_ids="$2"
  local wait_seconds="${HOMEBREW_RUN_LOOKUP_WAIT_SECONDS:-30}"
  local poll_seconds="${HOMEBREW_RUN_LOOKUP_POLL_SECONDS:-2}"

  if [[ ! "$wait_seconds" =~ ^[0-9]+$ \
    || ! "$poll_seconds" =~ ^[0-9]+$ \
    || "$poll_seconds" == "0" ]]; then
    return 1
  fi

  local deadline=$((SECONDS + wait_seconds))
  while ((SECONDS <= deadline)); do
    local runs
    runs="$(
      gh run list \
        --repo "$homebrew_tap_repository" \
        --workflow publish.yml \
        --event workflow_dispatch \
        --branch main \
        --user "$actor" \
        --limit 20 \
        --json databaseId,url \
        --jq '.[] | [.databaseId, .url] | @tsv'
    )" || return $?

    local new_run_count=0
    local new_run_url=""
    local run_id run_url
    while IFS=$'\t' read -r run_id run_url; do
      if [[ -z "$run_id" ]]; then
        continue
      fi
      if ! printf '%s\n' "$existing_run_ids" | grep -Fxq "$run_id"; then
        new_run_count=$((new_run_count + 1))
        new_run_url="$run_url"
      fi
    done <<<"$runs"

    if [[ "$new_run_count" -eq 1 && -n "$new_run_url" ]]; then
      homebrew_publish_url="$new_run_url"
      return 0
    fi
    if [[ "$new_run_count" -gt 1 ]]; then
      return 2
    fi
    if ((SECONDS >= deadline)); then
      break
    fi
    sleep "$poll_seconds"
  done

  return 3
}

watch_homebrew_publish() {
  local poll_seconds="${HOMEBREW_CHECK_POLL_SECONDS:-10}"
  local run_id="${homebrew_publish_url##*/}"
  gh run watch "$run_id" \
    --repo "$homebrew_tap_repository" \
    --exit-status \
    --compact \
    --interval "$poll_seconds"
}

offer_homebrew_publish() {
  local reply=""
  echo
  printf "Wait for bottle checks and publish with brew pr-pull? [y/N] " >&2
  read -r reply || reply=""
  if [[ ! "$reply" =~ ^[Yy]$ ]]; then
    print_warning "Leave Homebrew bottle publishing for manual completion"
    return 0
  fi

  run_step "Wait for Homebrew bottle checks" wait_for_homebrew_checks
  run_step "Verify reviewed Homebrew PR head" verify_homebrew_pr_head
  run_step "Start brew pr-pull workflow" dispatch_homebrew_publish
  if [[ -n "$homebrew_publish_url" ]]; then
    run_step "Wait for brew pr-pull workflow" watch_homebrew_publish
  else
    print_warning "brew pr-pull started; run URL is not available"
  fi
}

find_homebrew_pr_step() {
  local label="Find Homebrew update PR"
  step_index=$((step_index + 1))
  local log_file="${progress_tmp_dir}/step-${step_index}.log"
  start_step "$label"
  if wait_for_homebrew_pr >"$log_file" 2>&1; then
    finish_step ok "$label"
    return 0
  else
    local status=$?
    if [[ "$status" -eq 2 ]]; then
      finish_step warning "Homebrew PR is not available yet"
    else
      finish_step warning "Could not query the Homebrew PR"
      show_step_log "$log_file"
    fi
    return 0
  fi
}

if [[ "$current_version" != "$cargo_version" ]]; then
  run_step "Update Cargo version to $cargo_version" update_cargo_version
  run_step "Check updated Cargo package" cargo check -q
  run_step "Commit release version" commit_release_version
else
  print_ok "Cargo version is already $cargo_version"
fi

run_step "Create tag $version" git tag "$version"
run_step "Push release commit and tag" git push --atomic origin main "$version"
run_step "Create GitHub release" create_github_release
find_homebrew_pr_step

echo
echo "Release $version created."
if [[ -n "$release_url" ]]; then
  echo "Release: $release_url"
fi
if [[ -n "$homebrew_pr_url" ]]; then
  echo "Homebrew PR: $homebrew_pr_url"
else
  echo "Homebrew PR: not available yet"
fi

if [[ -n "$homebrew_pr_url" ]]; then
  offer_homebrew_publish
fi

echo
echo "Homebrew bottle publishing:"
echo "  Do not merge the Homebrew PR directly."
if [[ -n "$homebrew_publish_url" ]]; then
  echo "  Published with brew pr-pull: $homebrew_publish_url"
elif [[ "$homebrew_publish_dispatched" == "true" ]]; then
  echo "  The brew pr-pull workflow was dispatched, but its run URL is unavailable."
  echo "  Check its status at https://github.com/${homebrew_tap_repository}/actions/workflows/publish.yml"
else
  echo "  After all bottle checks pass, run the brew pr-pull workflow:"
  echo "  https://github.com/${homebrew_tap_repository}/actions/workflows/publish.yml"
  echo "  Enter the Homebrew PR number and its reviewed head SHA."
fi
