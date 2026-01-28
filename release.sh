#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./release.sh <version> [--notes <text>] [--notes-file <path>]

Examples:
  ./release.sh v0.1.1
  ./release.sh v0.1.1 --notes "Bug fixes"
  ./release.sh v0.1.1 --notes-file /path/to/notes.md
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

if [[ -n "$notes" && -n "$notes_file" ]]; then
  echo "Use either --notes or --notes-file, not both." >&2
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
