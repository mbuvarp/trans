#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./release.sh <version> [--notes <text>] [--notes-file <path>] [--dry-run]

Examples:
  ./release.sh v0.1.1
  ./release.sh v0.1.1 --notes "Bug fixes"
  ./release.sh v0.1.1 --notes-file /path/to/notes.md
  ./release.sh v0.1.1 --dry-run

Notes:
  - If the tag starts with "v", Cargo.toml is set to the version without the prefix.
  - Cargo.toml and Cargo.lock are updated and committed automatically when needed.
  - If no notes are provided, a changelog draft is generated and you will be prompted to approve/edit it.
  - If the `codex` CLI is available, it is used to draft the changelog; otherwise a local heuristic is used.
USAGE
}

if [[ ${1:-} == "" || ${1:-} == "-h" || ${1:-} == "--help" ]]; then
  usage
  exit 0
fi

version="$1"
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

if ! command -v gh >/dev/null 2>&1; then
  echo "gh CLI is required. Install from https://cli.github.com" >&2
  exit 1
fi

if ! command -v git >/dev/null 2>&1; then
  echo "git is required." >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to update Cargo.toml." >&2
  exit 1
fi

if [[ -n "$notes" && -n "$notes_file" ]]; then
  echo "Use either --notes or --notes-file, not both." >&2
  exit 1
fi

if [[ "$dry_run" == "true" && (-n "$notes" || -n "$notes_file") ]]; then
  echo "--dry-run cannot be used with --notes or --notes-file." >&2
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Working tree is dirty. Commit or stash changes first." >&2
  exit 1
fi

if git rev-parse "$version" >/dev/null 2>&1; then
  echo "Tag $version already exists." >&2
  exit 1
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
  commits="$(git log "$range" --pretty=%s --no-merges)"

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
      cleaned="$(printf "%s\n" "$ai_output" | python3 - <<'PY'
import sys

lines = sys.stdin.read().splitlines()
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
        printf "%s\n" "$cleaned" >"$tmp_file"
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
  tmp_notes="$(mktemp -t trans-release-notes.XXXXXX)"
  trap 'rm -f "$tmp_notes"' EXIT
  prepare_changelog "$tmp_notes"
  approve_changelog "$tmp_notes"
  notes_file="$tmp_notes"
fi

if [[ "$dry_run" == "true" ]]; then
  echo "Dry run complete. No version bump, tag, or release created."
  exit 0
fi

cargo_version="${version#v}"

current_version="$(python3 - <<'PY'
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
)"

if [[ "$current_version" != "$cargo_version" ]]; then
  echo "Updating Cargo.toml version $current_version -> $cargo_version"
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
  cargo check -q
  git add Cargo.toml Cargo.lock
  git commit -m "chore: release ${version}"
fi

git tag "$version"
git push origin "$version"

if [[ -n "$notes_file" ]]; then
  gh release create "$version" --title "$version" --notes-file "$notes_file"
elif [[ -n "$notes" ]]; then
  gh release create "$version" --title "$version" --notes "$notes"
else
  gh release create "$version" --title "$version" --generate-notes
fi

echo "Release $version created."
