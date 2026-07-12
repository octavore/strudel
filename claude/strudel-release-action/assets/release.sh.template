#!/usr/bin/env bash
# Bumps app.version and app.build_number in strudel.toml, commits, and tags the release.
# Usage: scripts/release.sh [major|minor|patch]  (default: patch)
set -euo pipefail

cd "$(dirname "$0")/.."

bump="${1:-patch}"

read_version() {
  strudel config version | grep -oE '^[0-9]+\.[0-9]+\.[0-9]+'
}

current="$(read_version)"
if [[ -z "$current" ]]; then
  echo "error: could not find app.version in strudel.toml" >&2
  exit 1
fi

strudel config increment-version "$bump"
strudel config increment-version build

new="$(read_version)"
tag="v$new"

if git rev-parse "$tag" >/dev/null 2>&1; then
  echo "error: tag $tag already exists; reverting strudel.toml" >&2
  git checkout -- strudel.toml
  exit 1
fi

git add strudel.toml
git commit -m "release: $tag"
git tag "$tag"

echo "Bumped $current -> $new, tagged $tag."

read -r -p "Push commit and tag $tag to origin? [y/N] " confirm
if [[ "$confirm" =~ ^[Yy]$ ]]; then
  git push
  git push origin "$tag"
else
  echo "Not pushed. Push later with: git push && git push origin $tag"
fi
