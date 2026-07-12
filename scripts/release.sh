#!/usr/bin/env bash
# release.sh — Bump version, build, commit, tag, and push a new release.
#
# Usage: axo release <major|minor|patch|beta [major|minor|patch]|x.y.z>
#
#   major/minor/patch       — increment the corresponding semver component
#   beta [major|minor|patch] — cut a pre-release: bumps the given component
#                         (patch by default) + "-beta.1" from a stable
#                         version, or increments "-beta.N" if the current
#                         version is already a beta (component arg not
#                         allowed in that case — bump was already done)
#   x.y.z[-beta.N]      — set an explicit version number
#
# Steps:
#   1. Validates you are on the main branch
#   2. Computes the new version from the current Cargo workspace version
#   3. Prompts for confirmation, then runs `cargo set-version`
#   4. Builds a release binary for aarch64-apple-darwin (to update Cargo.* files)
#   5. Commits Cargo.toml, Cargo.lock, and crates/dmg/Cargo.toml
#   6. Creates a git tag (e.g. v1.2.3 or v1.2.4-beta.1)
#   7. Prompts again, then pushes the commit and tag to origin/main
#
# Publishing the resulting GitHub release as a pre-release (tags with a
# "-beta.N" suffix are marked pre-release automatically, see release.yml)
# updates the `strudel-beta` Homebrew formula instead of the stable
# `strudel` formula — see homebrew.yml.
set -euo pipefail

ARG="${1:?usage: axo release <major|minor|patch|beta|x.y.z>}"
BUMP="${2:-patch}"

BRANCH=$(git rev-parse --abbrev-ref HEAD)
[[ "$BRANCH" == "main" ]] || { echo "error: must be on main branch (currently on ${BRANCH})"; exit 1; }

CURRENT=$(cargo pkgid | grep -oE '[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$')
CURRENT_BASE="${CURRENT%%-*}"

if [[ "$ARG" =~ ^(major|minor|patch)$ ]]; then
  IFS='.' read -r MAJ MIN PAT <<< "$CURRENT_BASE"
  case "$ARG" in
    major) VERSION="$((MAJ+1)).0.0" ;;
    minor) VERSION="${MAJ}.$((MIN+1)).0" ;;
    patch) VERSION="${MAJ}.${MIN}.$((PAT+1))" ;;
  esac
elif [[ "$ARG" == "beta" ]]; then
  if [[ "$CURRENT" == *-beta.* ]]; then
    [[ -z "${2:-}" ]] || { echo "error: v${CURRENT} is already a beta; omit the component arg, just run 'axo release beta'"; exit 1; }
    N="${CURRENT##*-beta.}"
    VERSION="${CURRENT_BASE}-beta.$((N+1))"
  else
    [[ "$BUMP" =~ ^(major|minor|patch)$ ]] || { echo "error: unknown component '${BUMP}', expected major|minor|patch"; exit 1; }
    IFS='.' read -r MAJ MIN PAT <<< "$CURRENT_BASE"
    case "$BUMP" in
      major) VERSION="$((MAJ+1)).0.0-beta.1" ;;
      minor) VERSION="${MAJ}.$((MIN+1)).0-beta.1" ;;
      patch) VERSION="${MAJ}.${MIN}.$((PAT+1))-beta.1" ;;
    esac
  fi
else
  VERSION="$ARG"
fi

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]] || { echo "error: invalid semver: ${VERSION}"; exit 1; }

read -r -p "Release v${VERSION} (current: v${CURRENT})? [y/N] " CONFIRM
[[ "$CONFIRM" =~ ^[Yy]$ ]] || exit 1

echo "Releasing v${VERSION}"

set -x
cargo set-version "$VERSION"
cargo build --release --target aarch64-apple-darwin
git add Cargo.toml Cargo.lock
git commit -m "release: v${VERSION}"
git tag "v${VERSION}"
set +x

read -r -p "Push v${VERSION} to remote? [y/N] " CONFIRM
[[ "$CONFIRM" =~ ^[Yy]$ ]] || exit 1

git push origin main
git push origin "v${VERSION}"
