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
  - Do not merge the Homebrew PR directly; publish it with the tap's brew pr-pull workflow.
USAGE
}

progress_tmp_dir=""
spinner_pid=""
cursor_hidden="false"
step_index=0
release_url=""
homebrew_pr_url=""
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
        --json url \
        --jq '.[0].url'
    )"; then
      return 1
    fi
    if [[ -n "$result" ]]; then
      homebrew_pr_url="$result"
      return 0
    fi
    if ((SECONDS >= deadline)); then
      break
    fi
    sleep "$poll_seconds"
  done
  return 2
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

run_step "Push release commit to main" git push origin main
run_step "Create tag $version" git tag "$version"
run_step "Push tag $version" git push origin "$version"
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
echo
echo "Homebrew bottle publishing:"
echo "  Do not merge the Homebrew PR directly."
echo "  After all bottle checks pass, run the brew pr-pull workflow:"
echo "  https://github.com/${homebrew_tap_repository}/actions/workflows/publish.yml"
echo "  Enter the Homebrew PR number and its reviewed head SHA."
