#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./release.sh <version> [--notes <text>] [--notes-file <path>]

Examples:
  ./release.sh v0.1.1
  ./release.sh v0.1.1 --notes "Bug fixes"
  ./release.sh v0.1.1 --notes-file /path/to/notes.md

Notes:
  - If the tag starts with "v", Cargo.toml is set to the version without the prefix.
  - Cargo.toml and Cargo.lock are updated and committed automatically when needed.
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

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Working tree is dirty. Commit or stash changes first." >&2
  exit 1
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

if git rev-parse "$version" >/dev/null 2>&1; then
  echo "Tag $version already exists." >&2
  exit 1
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
