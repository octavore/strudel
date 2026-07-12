---
name: strudel-release-action
description: Set up the `octavore/strudel-release-action` GitHub Action so a macOS strudel app builds, signs, notarizes, and packages a DMG in CI on every tag push, and gets uploaded as a draft GitHub release. Use this whenever the user wants CI/CD, automated releases, or a release workflow for a strudel-based macOS app, or mentions "release action", "GitHub Actions release", "sign and notarize in CI", "DMG in CI", or asks to automate what `strudel release` does locally. Also use it if the user is troubleshooting an existing `strudel-release-action` workflow (missing secrets, failed notarization, wrong strudel-version). Trigger even if they don't name the action explicitly — e.g. "how do I get GitHub to build a signed DMG when I push a tag" should trigger this.
---

# strudel-release-action

`octavore/strudel-release-action` is a composite GitHub Action that wraps `strudel release --ci`: it installs strudel, signs and notarizes the app, and packages a DMG. It does not create a GitHub release itself — that's a second step in the workflow, typically `softprops/action-gh-release`. This skill scaffolds that whole workflow and walks the user through the Apple credentials it needs.

Setting this up has two independent halves that are easy to conflate: **the workflow file** (mechanical, safe to write directly) and **the Apple credentials** (sensitive, involve external services and repo secrets — never invent or guess these, and confirm before pushing anything to GitHub on the user's behalf).

## 1. Check the repo is ready

The project needs a working `strudel.toml` for a macOS target before CI can build it — if there's no `strudel.toml`, or the user hasn't run `strudel release` locally yet, point them at the `strudel` skill to get that working first. CI should mirror a release that already succeeds locally; don't debug local signing problems through GitHub Actions.

## 2. Write the workflow

Copy `assets/release.yml.template` to `.github/workflows/release.yml` (adjust the path if the repo already has a release workflow to fold this into). Replace `__STRUDEL_VERSION__`:

- Pin it to a specific strudel release tag (e.g. `v0.5.0-beta.3`) that matches the version the user builds with locally — run `strudel --version` to check. Pinning avoids CI silently picking up a new strudel release that changes behavior.
- Or leave it as an empty string (`''`) to always install the latest strudel release. Only recommend this if the user explicitly wants to float.

The template triggers on `v*.*.*` tags plus `workflow_dispatch`, runs on `macos-latest`, and creates a **draft** GitHub release (the user reviews and publishes it manually — don't change this to a non-draft release without asking, since publishing is user-visible and hard to walk back).

If the repo is a multi-target strudel config (see `strudel help targets`), ask the user which target(s) need a release build — `strudel release` without a target argument builds all of them, which may not be what CI should do.

## 3. Walk through Apple credentials

The action needs three repo **variables** and three repo **secrets**. These are config, not something this skill should fabricate — collect real values from the user or from their Apple Developer account, one at a time, and explain where each comes from:

### Repo variables (Settings > Secrets and variables > Actions > Variables)

| Variable           | Where it comes from                                                                                             |
| ------------------ | --------------------------------------------------------------------------------------------------------------- |
| `APPLE_TEAM_ID`    | Apple Developer account membership page (10-character ID)                                                       |
| `APPLE_API_ISSUER` | Top of [App Store Connect > Integrations > API Keys](https://appstoreconnect.apple.com/access/integrations/api) |
| `APPLE_API_KEY`    | The Key ID column on the same API Keys page                                                                     |

### Repo secrets (Settings > Secrets and variables > Actions > Secrets)

| Secret                       | Where it comes from                                                                                                                      |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `APPLE_API_KEY_CONTENTS`     | Full contents of the `.p8` file downloaded when generating the API key above (only downloadable once — if it's lost, generate a new key) |
| `APPLE_CERTIFICATE`          | Base64-encoded `Developer ID Application` `.p12` certificate                                                                             |
| `APPLE_CERTIFICATE_PASSWORD` | The password chosen when exporting that `.p12`                                                                                           |

If the user doesn't have a `Developer ID Application` certificate yet, walk them through generating one (requires a paid Apple Developer Program membership and Account Holder/Admin access):

1. Keychain Access > Certificate Assistant > Request a Certificate from a Certificate Authority. Leave "CA Email Address" blank, choose "Saved to disk" to get a `.csr`.
2. In [Certificates, Identifiers & Profiles](https://developer.apple.com/account/resources/certificates/list), click **+**, choose **Developer ID Application** under Software, upload the `.csr`.
3. Download the resulting `.cer` and double-click to install it into the login keychain.
4. In Keychain Access, find the cert, right-click > Export, save as `.p12`, set the export password (this is `APPLE_CERTIFICATE_PASSWORD`).
5. `base64 -i /path/to/certificate.p12 | pbcopy` gets the value for `APPLE_CERTIFICATE`.

### Setting the values

Default to giving the user the exact `gh` commands to run themselves, since these are secrets touching their GitHub repo — don't run `gh secret set` / `gh variable set` on their behalf unless they explicitly ask you to and confirm the value first (pasting a certificate password into a shell command you execute is also easy to leak into shell history/logs):

```sh
gh variable set APPLE_TEAM_ID --body "XXXXXXXXXX"
gh variable set APPLE_API_ISSUER --body "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
gh variable set APPLE_API_KEY --body "2X9R4HXF34"

gh secret set APPLE_API_KEY_CONTENTS < AuthKey_XXXXXXXXXX.p8
gh secret set APPLE_CERTIFICATE < <(base64 -i certificate.p12)
gh secret set APPLE_CERTIFICATE_PASSWORD --body "the export password"
```

## 4. Add a release script (ask first)

The workflow only fires on a `v*.*.*` tag push, and that tag needs to match `app.version` in `strudel.toml` — bumping both by hand invites drift (forgetting to tag, or tagging a version that doesn't match what's in the TOML). Ask the user whether they want a `scripts/release.sh` that automates this, rather than adding it by default — some repos already have their own versioning/changelog tooling, and a second competing script is worse than none. If they want it, scaffold `scripts/release.sh` from `assets/release.sh.template` (make it executable: `chmod +x scripts/release.sh`) to do this in one step: it bumps `app.version`/`app.build_number` in `strudel.toml` via `strudel config increment-version`, commits, tags `vX.Y.Z`, and prompts before pushing.

- Usage is `scripts/release.sh [major|minor|patch]` (defaults to `patch`) — releasing is then just running the script and confirming the push prompt, which is what actually kicks off the CI workflow from step 2.
- The script pauses for a `y/N` confirmation before pushing (a shared, hard-to-reverse action — it's creating a commit and tag on origin). Don't remove that prompt or add a flag that skips it without the user asking.
- If the repo already has its own release/versioning script (changelog generation, monorepo version bumps, etc.), fold the tag-and-push logic into that instead of adding a second competing script — don't ask the user to remember which one to run.

## 5. Sanity-check before the user pushes a tag

- `dmg-path` output from the action is what gets uploaded — if the user is inserting another step after the release (notarization stapling check, uploading elsewhere), it needs `${{ steps.strudel.outputs.dmg-path }}`, matching whatever `id:` was given to the strudel-release-action step in the template.
- The workflow only runs on tag pushes matching `v*.*.*` (or manual dispatch) — a plain `git push` to a branch won't trigger it.
- The first real run is the actual test. Don't fabricate a fake tag/release just to "verify" the workflow works — the user should push a real version tag when they're ready and watch the Actions run.

## Bumping the action or strudel version later

- To pick up strudel fixes: bump `strudel-version` in the workflow to a newer tag.
- To pick up `strudel-release-action` improvements: the action is consumed by major version tag (`@v1`); check its README/releases for what's new before bumping, since a new major tag (`@v2`) may change required inputs.
