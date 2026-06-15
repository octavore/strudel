#!/usr/bin/env bash
set -euo pipefail

ARG="${1:?usage: axo release <major|minor|patch|x.y.z>}"

CURRENT=$(cargo pkgid | grep -oE '[0-9]+\.[0-9]+\.[0-9]+$')

if [[ "$ARG" =~ ^(major|minor|patch)$ ]]; then
  IFS='.' read -r MAJ MIN PAT <<< "$CURRENT"
  case "$ARG" in
    major) VERSION="$((MAJ+1)).0.0" ;;
    minor) VERSION="${MAJ}.$((MIN+1)).0" ;;
    patch) VERSION="${MAJ}.${MIN}.$((PAT+1))" ;;
  esac
else
  VERSION="$ARG"
fi

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: invalid semver: ${VERSION}"; exit 1; }

read -r -p "Release v${VERSION} (current: v${CURRENT})? [y/N] " CONFIRM
[[ "$CONFIRM" =~ ^[Yy]$ ]] || exit 1

echo "Releasing v${VERSION}"

set -x
cargo set-version "$VERSION"
git add Cargo.toml Cargo.lock crates/dmg/Cargo.toml
git commit -m "chore: release v${VERSION}"
git tag "v${VERSION}"
set +x

read -r -p "Push v${VERSION} to remote? [y/N] " CONFIRM
[[ "$CONFIRM" =~ ^[Yy]$ ]] || exit 1

git push origin "v${VERSION}"
