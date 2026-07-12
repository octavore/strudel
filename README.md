# strudel

Build and ship macOS/iOS apps entirely from the command-line, without touching the Xcode IDE.

`strudel` uses the standard Apple toolchain (e.g. `swift`, `codesign`, `notarytool`) to build Swift Package Manager-based macOS and iOS apps with a config-driven, easy-to-introspect pipeline. It can produce signed `.app` bundles and notarized DMGs which can be distributed.

> [!IMPORTANT]
> **Current limitations**
> - **iOS support is still experimental.** `strudel run --sim` and `strudel run --device` work for local development, but distributing iOS apps is unsupported.
> - iOS device builds can use either a paid Apple Developer account (App Store Connect API, 1-year profiles) or any free Apple ID (`strudel login`, 7-day profiles, max 3 devices). strudel auto-registers devices and provisions development profiles for both (see [iOS device builds](#ios-device-builds)).
> - **App Store distribution is not supported yet.** strudel supports direct/notarized distribution (Developer ID) for macOS apps, but there is currently no support for submitting to the Mac App Store or iOS App Store.

- [Installation](#installation)
- [Example strudel build](#example-strudel-build)
- [Usage](#usage)
- [Config file structure](#config-file-structure)
- [Multiple targets](#multiple-targets)
- [iOS device builds](#ios-device-builds)
- [Global config](#global-config)
- [Signing \& notarization](#signing--notarization)
- [Safari Web Extensions](#safari-web-extensions)
- [App Extensions](#app-extensions)
- [Development](#development)
- [Other tips](#other-tips)
- [Acknowledgements](#acknowledgements)
- [License](#license)

## Installation

Install strudel with homebrew:

```sh
brew install octavore/tools/strudel
```

### Requirements

- macOS with the Xcode command line tools installed
- Swift Package.swift based project
- For signing/notarization: an Apple Developer account with a "Developer ID Application" certificate

## Example strudel build

```sh
# create strudel.toml and a basic Package.swift file
strudel init

# ...edit strudel.toml, info.json, entitlements.json...

# create an ad-hoc codesigned app bundle
strudel build # --dry-run

# or create a real codesigned app bundle
export APPLE_SIGNING_IDENTITY=...
strudel build

# or skip codesigning entirely
strudel build --unsigned

# produce a signed, notarized DMG
export APPLE_API_KEY_PATH=...
export APPLE_API_KEY=...
strudel release # --dry-run
```

Note that the env vars above can also be stored in the `strudel.toml` config file.

## Usage

```raw
strudel [OPTIONS] <COMMAND>

Commands:
  init       Scaffold a strudel.toml in the given directory
  login      Sign in with an Apple ID for free iOS provisioning (7-day profiles)
  build      Assemble the app bundle. Signed on macOS by default; add --unsigned to skip
  run        Build and launch locally (macOS: sign + open; iOS: simulator or device)
  release    Full distributable: build, sign, notarize, and package DMG (macOS only)
  devices    Manage tracked iOS devices; bare command lists them
  profile    Show provisioning-profile status; `profile fetch` fetches/refreshes it
  clean      Remove the strudel output directory and run `swift package clean`
  config     Show/bump the project version, or edit the global config
  skill      Install a skill file that points an AI coding agent at strudel docs or tooling
  status     Show overall status: toolchain, config, session, and per-target state
  help       Show documentation for a topic (run `strudel help` to list topics)

Options:
      --config <CONFIG>  Path to config file [default: strudel.toml]
  -h, --help             Print help
  -V, --version          Print version
```

`build`, `run`, and `release` take an optional target name as a positional
argument (`strudel build MyApp`) to select one target in a multi-target
config; other commands (`devices`, `profile`, `status`, `clean`) take
`--target <name>` instead.

`strudel help <topic>` has extended documentation for many subjects beyond the
subcommands above, including `config`, `targets`, `signing`, `notarize`,
`entitlements`, `extensions`, `ios-device`, `ios-free-provisioning`, and more.
Run `strudel help` with no argument to list every topic.

### `init`

Interactively scaffold a config file. Prompts for app name, bundle ID, version,
and build number, then writes the file into the given directory (defaults to the
current directory).

```sh
strudel init             # scaffold in the current directory
strudel init ./myapp     # scaffold in ./myapp
```

### `build`

Assemble the app bundle for one target (or all eligible targets, if no target
name is given). On macOS this cleans the old build, runs `swift build -c
release`, assembles `.app`, and codesigns it (no notarization or DMG). On iOS
it assembles a `.app` for the Simulator triple, but doesn't install or launch
it.

If a signing identity (`[apple] identity` / `APPLE_SIGNING_IDENTITY`) is set, `strudel` signs with that identity;
otherwise it signs **ad-hoc** (`codesign --sign -`), which needs no certificate
or Apple account.

Note: Signed app bundles will still fail Gatekeeper checks and cannot be distributed
easily. For that, you will need to run `strudel release` to have your app notarized.

```sh
strudel build
strudel build MyApp          # select one target by app name
strudel build --open         # open the .app after a successful build
strudel build --install      # copy the built .app into /Applications
strudel build --debug        # build with the debug configuration instead of release
strudel build --dry-run      # print commands without executing them
```

Add `--unsigned` (macOS only) to skip codesigning and leave the bundle as-is.

```sh
strudel build --unsigned
```

### `run`

Build and launch locally. On macOS this signs (same identity/ad-hoc rules as
`build`) and opens the app. On iOS it installs and launches in the Simulator
by default, or on a connected device with `--device`.

```sh
strudel run
strudel run MyApp                     # select one target by app name
strudel run --unsigned                # macOS: skip codesigning
strudel run --sim                     # iOS: launch in the Simulator (default)
strudel run --sim "iPhone 16 Pro"     # iOS: override the simulator name
strudel run --device                  # iOS: launch on a connected device
strudel run --device "iPhone 15"      # iOS: target a specific tracked device
strudel run --debug                   # build with the debug configuration instead of release
strudel run --dry-run                 # print commands without executing them
```

### `release`

Like `strudel build` but also creates a notarized DMG file so you can
distribute your app. macOS only — iOS targets error with a pointer to `run
--device`. This step requires valid signing credentials from a paid Apple
Developer membership (see below).

```sh
strudel release
strudel release --dry-run            # print commands without executing them
strudel release --open               # open the .app after a successful build
strudel release --skip-notarization  # build and package the DMG, but don't notarize
strudel release --resume             # resume the most recent pending notarization
strudel release --resume <uuid>      # resume a specific notarization submission
strudel release --ci                 # trim noisy per-second notarization progress output for captured CI logs
```

Output artifacts are saved to `build_dir`:

- `<app_name>.app` is the signed, stapled app bundle
- `<app_name>-<version>.dmg` is the notarized, stapled DMG

Notarization may take a while the first time. If it stalls or you lose the
connection, re-run with `--resume` to pick up the pending submission instead of
resubmitting. Run `strudel help notarize` for more.

### `config version` / `config increment-version`

`strudel config version` prints the current `app.version` and
`app.build_number` for each target, without modifying anything.

`strudel config increment-version` bumps `app.version` (major/minor/patch) or
`app.build_number` (build) in `strudel.toml`, printing the change and
prompting for confirmation before writing. Comments and formatting elsewhere
in the file are preserved. In a multi-target config, every target is bumped
together.

```sh
strudel config version

strudel config increment-version patch  # 1.2.3 -> 1.2.4
strudel config increment-version minor  # 1.2.3 -> 1.3.0
strudel config increment-version major  # 1.2.3 -> 2.0.0
strudel config increment-version build  # build_number 41 -> 42
```

See [Global config](#global-config) for `strudel config global edit`.

### `skill install` (experimental)

Writes a supporting skill for coding agents. There are currently two skills: `strudel` and `strudel-release-action`:

- `strudel` - a pointer to `strudel help`/`strudel help <topic>`, with the
  topic list generated from the installed strudel's own `TOPICS`, so it can't
  drift out of date.
- `release-action` - scaffolds the [`octavore/strudel-release-action`](https://github.com/octavore/strudel-release-action) GitHub
  Actions release workflow (signing, notarization, DMG packaging), including an optional `release.sh` template that bumps the version and tags a release.

```sh
strudel skill install                    # prompt for which skill(s) to install
strudel skill install release-action     # install one directly, no prompt
strudel skill install --preview          # print the SKILL.md instead of writing it
strudel skill install --force            # overwrite files that already exist
```

By default, files are installed user-globally under `~/.claude/skills/<name>/`,
since these are tools/docs for whatever you're working on, not just this one
project.

```sh
strudel skill install --project          # .claude/skills/<name>/ in this project, instead of global
strudel skill install --agents           # ~/.agents/skills/<name>/ instead of ~/.claude/skills
strudel skill install --project --agents # .agents/skills/<name>/ in this project
strudel skill install --path ./somewhere/else  # exact base dir; overrides --project/--agents
```

## Config file structure

The config file (`strudel.toml` by default) is TOML, organized into seven
sections. Relative paths are resolved **relative to the config file's directory**
unless absolute. Unknown keys are rejected, so typos surface as errors. See the HelloWorldApp
[`strudel.toml`](./examples/HelloWorldApp/strudel.toml) for an annotated template.

### `[app]` (required)

| Key            | Type   | Description                                          |
| -------------- | ------ | ---------------------------------------------------- |
| `name`         | string | Display name; also the `.app` bundle and binary name |
| `bundle_id`    | string | Bundle identifier, e.g. `com.example.myapp`          |
| `version`      | string | Marketing version (`CFBundleShortVersionString`)     |
| `build_number` | string | Build number (`CFBundleVersion`)                     |

The relationship between version and build_number is that a version ([`CFBundleShortVersionString`](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleshortversionstring))
is user-facing, and it may have multiple unique internal tracking build numbers ([`CFBundleVersion`](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleversion)).

`CFBundleShortVersionString` is three period-separated integers, such as `10.14.1`.

`CFBundleVersion` is one to three period-separated integers (e.g. `1`, `1.1`, `1.0.1`)

### `[build]` (optional)

| Key                      | Type     | Default             | Description                                                                                                                                                |
| ------------------------ | -------- | ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `source_dir`             | string   | `.`                 | Swift package directory (relative to config file)                                                                                                          |
| `build_dir`              | string   | `.build/dist`       | Output directory for artifacts (relative to `source_dir`)                                                                                                  |
| `info_json_path`         | string   | *(none)*            | JSON describing `Info.plist` keys; converted to `Info.plist` via `plutil`. If omitted, strudel starts from an empty object and injects only the keys below |
| `entitlements_json_path` | string   | *(none)*            | JSON entitlements; converted to a plist and passed to `codesign`                                                                                           |
| `icon`                   | table    | *(none)*            | Bundle icon; see below. If unset, the bundle has no icon                                                                                                   |
| `archs`                  | string[] | host architecture   | Architectures passed to `swift build --arch`. Set multiple for a universal binary, e.g. `["arm64", "x86_64"]`                                              |
| `target_name`            | string   | value of `app.name` | Swift executable target name, if it differs from the app name                                                                                              |
| `embed_libs`             | string[] | *(none)*            | Dynamic C FFI libraries to embed in `Contents/Frameworks` and sign. Paths relative to config file                                                          |
| `resources_dir`          | string   | *(none)*            | Directory whose contents are copied wholesale into `Contents/Resources/`                                                                                   |
| `resources`              | string[] | *(none)*            | Individual files to copy into `Contents/Resources/` by filename                                                                                            |
| `provisioning_profile`   | string   | *(none)*            | Provisioning profile embedded as `Contents/embedded.provisionprofile`; required for some entitlements                                                      |

#### `[build.icon]` (optional)

The bundle icon, specified one of two ways:

```toml
# a png or icns file, copied into the bundle unmodified
[build.icon]
path = "AppIcon.icns"
```

```toml
# generate an icon from a source image (png or svg)
[build.icon]
src = "art.png"
scale = 1.2               # optional
background = "#fefefe"    # optional; hex, defaults to white
```

By default, the icon is copied as-is (`path`) or as a single
composited PNG (`src`). Set `icns = true` (either form) to instead convert it into a real
multi-resolution `.icns` via `sips`/`iconutil`, which gives cleaner results at
small sizes (Finder list view, menu bar) at the cost of an extra build step:

```toml
[build.icon]
src  = "art.png"
icns = true
```

`[build.icon]` also applies to iOS targets (`icns` is ignored there). The
icon is wrapped in a minimal `.appiconset`/`.xcassets` and compiled with `xcrun actool
--include-all-app-icons`, which derives every required size/idiom from that
one image. If `path` points to an Icon Composer `.icon` bundle instead of a
raster image, it's handed to `actool` directly. If `ios.assets_dir` (below) is
also set, it takes precedence over `[build.icon]` for that target.

### `[build_env]` (optional)

Extra environment variables forwarded to `swift build` (e.g. for `pkg-config`).
Each key/value is passed through to the build environment:

```toml
[build_env]
PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig"
```

### `[ios]` (optional, experimental)

For iOS apps, this contains settings for `strudel run --sim` and `strudel run --device`. iOS support is experimental. All fields are optional except `provisioning`, which is required for device builds.

> [!NOTE]
> The flat, single-target form (a top-level `[app]`, as shown above) is always
> macOS, so `[ios]` only applies inside an iOS `[[target]]` block, or as a
> top-level fallback for iOS targets - see [Multiple targets](#multiple-targets).

> [!TIP]
> strudel can auto-manage device registration and development provisioning
> profiles via the App Store Connect API. The usual flow is `strudel devices add`
> once, then `strudel run --device` to build, install, and launch. See
> [iOS device builds](#ios-device-builds) for the full workflow, or set
> `provisioning_profile` in `[build]` to manage the profile yourself.

| Key                 | Type   | Default                        | Description                                                                                                              |
| ------------------- | ------ | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| `provisioning`      | string | *(required for device builds)* | `"app_store_connect"` (paid account, 1-year profiles) or `"free"` (any Apple ID, 7-day profiles)                         |
| `apple_id`          | string | *(none)*                       | Apple ID email; pre-fills the login prompt for the `"free"` path                                                         |
| `simulator`         | string | `"iPhone 16"`                  | Simulator name for `strudel run --sim`; override with `--sim <name>`                                                     |
| `device`            | string | *(auto)*                       | Device name or UDID for `strudel run --device`; auto-detected if unset                                                   |
| `deployment_target` | string | `"18.0"`                       | iOS deployment target, e.g. `"17.0"`                                                                                     |
| `assets_dir`        | string | *(none)*                       | `.xcassets` directory compiled into the bundle with `xcrun actool`. Takes precedence over `[build.icon]` if both are set |
| `app_icon_name`     | string | `"AppIcon"`                    | Icon set name inside `assets_dir`                                                                                        |

### `[[extensions]]` (optional)

An array of zero or more app extensions embedded under `<app>.app/Contents/PlugIns/`. Each
entry produces a separate `.appex` bundle, signed with its own entitlements,
and sealed inside the notarized host app. Two kinds are supported:

- `"safari_web_extension"` for a Safari Web Extension; see [Safari Web Extensions](#safari-web-extensions)
- `"app_extension"` for generic macOS app extensions (Share, Finder Sync, Notification Service,
  Quick Look, etc.); see [App Extensions](#app-extensions).

**Common fields** (all kinds):

| Key                      | Type   | Default                | Description                                                                       |
| ------------------------ | ------ | ---------------------- | --------------------------------------------------------------------------------- |
| `kind`                   | string | *(required)*           | `"safari_web_extension"` or `"app_extension"`                                     |
| `target_name`            | string | *(required)*           | Swift `executableTarget` in `Package.swift`                                       |
| `bundle_id`              | string | *(required)*           | `CFBundleIdentifier`; typically `<host-id>.Extension`                             |
| `name`                   | string | value of `target_name` | Display name (`CFBundleName` / `CFBundleDisplayName`) and `.appex` directory name |
| `entitlements_json_path` | string | *(required)*           | JSON entitlements (extensions have their own entitlements                         |
| `info_json_path`         | string | *(none)*               | Extra `Info.plist` keys merged with strudel's auto-injected ones                  |

**`safari_web_extension`-specific fields:**

| Key               | Type   | Default                                   | Description                                                                          |
| ----------------- | ------ | ----------------------------------------- | ------------------------------------------------------------------------------------ |
| `resources_dir`   | string | *(required)*                              | build output containing `manifest.json`; contents copied wholesale into `Resources/` |
| `principal_class` | string | `<target_name>.SafariWebExtensionHandler` | `NSExtensionPrincipalClass`                                                          |

**`app_extension`-specific fields:**

| Key                          | Type   | Default      | Description                                                                                      |
| ---------------------------- | ------ | ------------ | ------------------------------------------------------------------------------------------------ |
| `extension_point_identifier` | string | *(required)* | `NSExtensionPointIdentifier`, identifies the extension point (e.g. `"com.apple.share-services"`) |
| `principal_class`            | string | *(none)*     | `NSExtensionPrincipalClass`, required by some extension points                                   |

### `[dmg]` (optional)

Controls the Finder window layout of the DMG produced by `strudel release`. By
default (no `[dmg]` section), strudel generates a styled drag-to-install window
with the app icon on the left and an Applications symlink on the right. Add the
section to override individual fields or opt out entirely with `plain = true`.

| Key              | Type    | Default   | Description                                                                                                                       |
| ---------------- | ------- | --------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `plain`          | bool    | `false`   | Skip the styled window; produce a plain compressed DMG instead                                                                    |
| `background`     | string  | *(unset)* | `#rrggbb` hex color, or a path to a PNG/JPEG background image (relative to config file); when absent, Finder uses its own default |
| `window_width`   | integer | `660`     | Finder window width in pixels                                                                                                     |
| `window_height`  | integer | `400`     | Finder window height in pixels                                                                                                    |
| `icon_size`      | integer | `128`     | Icon size in pixels                                                                                                               |
| `app_x`          | integer | `192`     | Horizontal position of the `.app` icon                                                                                            |
| `app_y`          | integer | `192`     | Vertical position of the `.app` icon                                                                                              |
| `applications_x` | integer | `468`     | Horizontal position of the Applications symlink                                                                                   |
| `applications_y` | integer | `192`     | Vertical position of the Applications symlink                                                                                     |
| `icon_text_size` | float   | `12.0`    | Icon label point size                                                                                                             |

Example (custom background and larger icons):

```toml
[dmg]
background    = "assets/dmg-background.png"
window_width  = 800
window_height = 500
icon_size     = 160
app_x         = 200
applications_x = 600
```

To produce a plain compressed DMG with no special styling.

```toml
[dmg]
plain = true
```

### `[apple]` (optional in strudel.toml)

Apple developer identifiers, shared by signing, notarization, and
provisioning-profile management (the App Store Connect API key authenticates
all three). Required for `release`. Each identifier is resolved in priority
order: **env var > strudel.toml > [global config](#global-config)**. Secrets
are environment-only and have no config key. See
[Signing & notarization](#signing--notarization) for the full reference.

| Key                        | Type    | Env var                  | Description                                               |
| -------------------------- | ------- | ------------------------ | --------------------------------------------------------- |
| `[apple] identity`         | string  | `APPLE_SIGNING_IDENTITY` | Signing identity                                          |
| `[apple] team_id`          | string  | `APPLE_TEAM_ID`          | Apple Developer Team ID                                   |
| `[apple] api_issuer`       | string  | `APPLE_API_ISSUER`       | App Store Connect issuer UUID                             |
| `[apple] api_key`          | string  | `APPLE_API_KEY`          | App Store Connect key ID                                  |
| `[apple] api_key_path`     | string  | `APPLE_API_KEY_PATH`     | Path to the `.p8` key file                                |
| `[apple] notarize_timeout` | integer | —                        | Seconds to wait for notarization (`notarytool --timeout`) |

> Secrets (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`)
> are read from the environment only and have no config key.

### Supporting files

- **`info.json`** *(optional)*: A JSON object of `Info.plist` keys/values. `strudel`
  overrides `CFBundleShortVersionString`, `CFBundleVersion`, and `CFBundleIdentifier`
  from the config (and sets `CFBundleIconFile` when an icon is present), then converts
  the result to `Info.plist` with `plutil`. If `info_json_path` is unset, strudel starts
  from an empty object, so the generated `Info.plist` contains only those injected keys.
- **`entitlements.json`**: A JSON object of entitlement keys/values, converted to a
  plist and passed to `codesign --entitlements` during signing.

## Multiple targets

A single `strudel.toml` can declare multiple build targets using `[[target]]`
blocks instead of a top-level `[app]`. Each target is a product x platform pair
with its own `[app]`, `[build]`, `[[extensions]]`, `[dmg]`, and optional `[ios]`
settings. This is useful for shipping both a macOS and an iOS app from the same
Swift package, or for monorepos with several executables that share signing and
notarization credentials.

- `platform` is required on every `[[target]]`, either `macos` or `ios`.
- `[apple]` is always shared (top-level only).
- A top-level `[ios]` supplies defaults for iOS targets; a per-target `ios.*`
  field wins over the matching top-level field, field by field (not
  whole-section replacement).


```toml
# Shared across all targets (top-level only):
[apple]
identity     = "Developer ID Application: You (XXXXXXXXXX)"
team_id      = "XXXXXXXXXX"
api_key      = "2X9R4HXF34"
api_key_path = "AuthKey_2X9R4HXF34.p8"

# Optional top-level [ios] acts as a fallback for iOS targets.
[ios]
simulator = "iPhone 16"

[[target]]
platform         = "macos"
app.name         = "MyApp"
app.bundle_id    = "com.example.app"
app.version      = "1.0.0"
app.build_number = "1"
build.entitlements_json_path = "mac/entitlements.json"

[[target]]
platform         = "ios"
app.name         = "MyApp"
app.bundle_id    = "com.example.app" # may be the same for macos/ios
app.version      = "1.0.0"
app.build_number = "1"
ios.deployment_target = "18.0"
```

When multiple targets are eligible for a command, strudel runs them all and
prints a per-target header. Narrow to one target by name:

```sh
strudel build MyApp
strudel run   MyApp
```

`build`, `run`, and `release` take the target name as a positional argument
and dispatch per target based on its own platform (macOS or iOS). Other
commands (`devices`, `profile`, `status`, `clean`) take `--target <app name>`
instead.

With multiple targets, each gets its own build directory (`.build/dist/<name>-macos`,
`.build/dist/<name>-ios`) to avoid collisions; override per-target with
`build.build_dir`. See the [`MultiTargetApp`](./examples/MultiTargetApp/strudel.toml)
example or run `strudel help targets` for more.

## iOS device builds

`strudel run --device` builds for a connected iOS device, then installs and
launches it. strudel can auto-manage device registration and a development
provisioning profile using one of two backends, selected by `[ios]
provisioning` in `strudel.toml`:

- `"app_store_connect"` — a paid Apple Developer account, driven through the
  App Store Connect API using the same credentials as notarization (see
  [Notarization auth](#notarization-auth)). Produces 1-year profiles.
- `"free"` — any Apple ID, no paid account. Run `strudel login` once to sign
  in, then use the same device workflow below. Produces 7-day profiles, with a
  limit of 3 devices and 10 App IDs per team. Run `strudel help
  ios-free-provisioning` for the full walkthrough.

Both backends share the device and profile workflow below; the free path just
adds the one-time `strudel login`. `strudel status` shows the current session
and per-target provisioning state.

> [!IMPORTANT]
> **Admin vs Developer API keys.** Registering devices and creating bundle IDs
> and provisioning profiles modifies your App Store Connect account, so it
> requires an API key with the **Admin** role. A lower-privilege **Developer**
> key is enough for notarization (`strudel release`) but will fail with an
> "insufficient permissions" error on `strudel devices add`, `strudel run
> --device`, or `strudel profile fetch`. Either issue an Admin key for these flows, or
> register the device and create the profile manually in the
> [Developer portal](https://developer.apple.com/account/resources/) and point
> `provisioning_profile` at it.

```sh
# One-time: register connected device(s) on the portal and track them locally
strudel devices add

# Build, install, and launch on a connected device
strudel run --device
```

`strudel devices add` registers connected devices on the App Store Connect
portal and records them in `.strudel/devices.toml`. This file should be .gitignored
since devices are per-developer. `strudel devices` (no subcommand) lists the
tracked devices.

On the first `strudel run --device`, strudel looks up (or creates) the bundle
ID, finds your development certificate, creates a provisioning profile
embedding all specified devices, and caches it at
`.strudel/<bundle_id>.mobileprovision`. Subsequent runs reuse the cached
profile while it's still current, re-fetching automatically if it has expired
or no longer covers every tracked device.

Useful flags and the standalone profile command:

```sh
strudel run --device "iPhone 15"       # target specific tracked device(s)
strudel profile fetch                  # fetch/refresh the cached profile without building
strudel profile fetch --force          # recreate the profile even if current
```

To opt out of auto-management and supply your own profile, set
`provisioning_profile` under `[build]`; strudel then uses that file as-is. See
`strudel help ios-device` for the full workflow.

## Global config

`~/.config/strudel/config.toml` stores machine-wide defaults shared across all
projects. It is the lowest-priority source for each value:

```
env var  >  strudel.toml  >  ~/.config/strudel/config.toml
```

Open it in your editor (creating it with a template if it doesn't exist):

```sh
strudel config global edit
```

Only `[apple]` is supported here — `[app]`, `[build]`, `[ios]`, `[dmg]`, and
`[[extensions]]` are project-specific and belong only in `strudel.toml`.

```toml
# ~/.config/strudel/config.toml

[apple]
identity     = "Developer ID Application: Your Name (XXXXXXXXXX)"
team_id      = "XXXXXXXXXX"
api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
api_key      = "2X9R4HXF34"
api_key_path = "/Users/you/.private_keys/AuthKey_2X9R4HXF34.p8"
```

Store your credentials here once and each project's `strudel.toml` stays free
of machine-specific paths, making it safe to commit. A project `strudel.toml`
can still override any global value by setting the same key; the env var
overrides both.

## Signing & notarization

`release` signs the bundle with a certificate and notarizes it with Apple.
An **App Store Connect API key** is required for notarization.

### Signing

In development, you typically codesign with a Developer ID certificate. This is free
with Xcode but you can also create one on developer.apple.com if you have a paid
Developer membership.

After downloading and installing the certificate into the keychain on your machine, you
can verify its presence with `security find-identity -p codesigning`.

Then, either set `identity` in `strudel.toml` under `[apple]`, or pass it via the
`APPLE_SIGNING_IDENTITY` env var.

| Config key         | Environment variable     | Description                                             |
| ------------------ | ------------------------ | ------------------------------------------------------- |
| `[apple] identity` | `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (XXXXXXXXXX)` |
| `[apple] team_id`  | `APPLE_TEAM_ID`          | 10-character Apple Developer Team ID                    |

In CI, because the system keychain is not available, you should set the `APPLE_CERTIFICATE`
and `APPLE_CERTIFICATE_PASSWORD` env vars instead. These cannot be stored in `strudel.toml`.

`APPLE_CERTIFICATE` is typically an Developer ID certificate exported as a `.p12` file, then base64-encoded:

1. Open Keychain Access
2. Select the login keychain and My Certificates category
3. Find your Developer ID Application certificate. It should have a small triangle indicating there's a private key.
4. Right-click the certificate (not the key) and select Export
5. Choose format Personal Information Exchange (.p12)
6. Set a strong password (note it for `APPLE_CERTIFICATE_PASSWORD`)
7. Save the file
8. base64-encode, e.g. `base64 -i certificate.p12 | pbcopy` copies the base64 file to the clipboard.
9. Set the following secrets in your CI environment.

| Environment variable         | Description                        |
| ---------------------------- | ---------------------------------- |
| `APPLE_CERTIFICATE`          | Base64-encoded Developer ID `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Export password for that `.p12`    |

### Notarization auth

**Identifiers** are non-secret and resolved in this priority order:
env var > `strudel.toml` > `~/.config/strudel/config.toml` (see [Global config](#global-config)).

| strudel.toml key       | Environment variable     | Description                                             |
| ---------------------- | ------------------------ | ------------------------------------------------------- |
| `[apple] identity`     | `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (XXXXXXXXXX)` |
| `[apple] team_id`      | `APPLE_TEAM_ID`          | 10-character Apple Developer Team ID                    |
| `[apple] api_issuer`   | `APPLE_API_ISSUER`       | App Store Connect issuer UUID                           |
| `[apple] api_key`      | `APPLE_API_KEY`          | App Store Connect key ID                                |
| `[apple] api_key_path` | `APPLE_API_KEY_PATH`     | Path to the `AuthKey_XXXXXXYYYY.p8` file                |

`APPLE_API_ISSUER` is only present for team Apple Developer accounts.

A **Developer**-role API key is enough for notarization alone. If you also use
strudel's iOS auto-provisioning (`[ios] provisioning = "app_store_connect"`), use
an **Admin**-role key instead - device registration and profile management via
the App Store Connect API generally require Admin, and a Developer key fails
with a 403/`FORBIDDEN_ERROR`.

### Example

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (XXXXXXXXXX)"
export APPLE_API_KEY="2X9R4HXF34"
export APPLE_API_KEY_PATH="$HOME/.private_keys/AuthKey_XXXXXXYYYY.p8"
export APPLE_API_ISSUER="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee" # for team accounts

strudel release
```

## Safari Web Extensions

strudel can build and sign one or more Safari Web Extensions alongside the host
app. Each extension becomes an `.appex` bundle under
`<host>.app/Contents/PlugIns/`, signed inside-out (extension first, with its
own entitlements; then the host bundle seals over it). Notarizing the host
`.app` covers the embedded extensions.

### 1. Swift package setup

The host app and each extension are separate executable targets in the same
`Package.swift`:

```swift
let package = Package(
    name: "MyApp",
    platforms: [.macOS(.v12)],
    targets: [
        .executableTarget(name: "MyApp",          path: "Sources/App"),
        .executableTarget(name: "MyAppExtension", path: "Sources/Extension"),
    ]
)
```

App extensions are loaded by the host process (Safari) via `NSExtensionMain`,
not run as standalone programs. Xcode wires this up automatically for bundle
targets, but SPM produces a regular executable that requires a `main` symbol.
Add a one-line entry-point shim to the extension's source directory:

```swift
// Sources/Extension/main.swift
import Foundation

// NSExtensionMain is the standard C entry point exported by Foundation for
// app extensions. It sets up the XPC connection to the host process and
// instantiates the class named by NSExtensionPrincipalClass in Info.plist
// (which strudel fills in from the [[extensions]] config).
@_silgen_name("NSExtensionMain")
func NSExtensionMain() -> Never

NSExtensionMain()
```

Your principal class lives alongside it:

```swift
// Sources/Extension/SafariWebExtensionHandler.swift
import SafariServices

class SafariWebExtensionHandler: NSObject, NSExtensionRequestHandling {
    func beginRequest(with context: NSExtensionContext) {
        // Native side of messages from your extension's JS.
    }
}
```

> Don't `import SwiftUI` in the extension target unless you actually use it.
> SPM may try to link the private `SwiftUICore` framework and fail with
> "product being built is not an allowed client of it".

### 2. Web assets

`resources_dir` points at the directory whose **contents** (not the directory
itself) are copied wholesale into `<name>.appex/Contents/Resources/`. `ditto`
is used so macOS metadata and symlinks are preserved. If you're using
webpack/esbuild/etc., point `resources_dir` at the output directory and make
sure `manifest.json` ends up at its root:

```raw
extension/dist/
├── manifest.json
├── background.js
├── content.js
├── popup.html
├── popup.js
└── icons/
    └── icon-128.png
```

### 3. Config

```toml
[app]
name         = "MyApp"
bundle_id    = "com.example.myapp"
version      = "1.0.0"
build_number = "1"

[build]
entitlements_json_path = "entitlements.json"

[[extensions]]
kind                   = "safari_web_extension"
target_name            = "MyAppExtension"
bundle_id              = "com.example.myapp.Extension"
entitlements_json_path = "extension/entitlements.json"
resources_dir          = "extension/dist"
```

A minimal extension entitlements file (`extension/entitlements.json`):

```json
{ "com.apple.security.app-sandbox": true }
```

strudel auto-injects the following into the extension's `Info.plist`, on top
of any `info_json_path` you provide:

- `CFBundleName`, `CFBundleDisplayName`, `CFBundleExecutable`, `CFBundleIdentifier`
- `CFBundleShortVersionString`, `CFBundleVersion` (inherited from `[app]`)
- `CFBundleInfoDictionaryVersion = "6.0"`, `CFBundlePackageType = "XPC!"`
- `NSExtension` dict with `NSExtensionPointIdentifier = "com.apple.Safari.web-extension"`,
  `NSExtensionPrincipalClass`, and `SFSafariWebExtensionManifestPath = "Resources/manifest.json"`


### 4. Preparing Safari

You will need to enable developer mode in Safari:

1. **Enable Safari's Develop menu**. Go to Safari -> Settings -> Advanced -> "Show
   features for web developers".
2. **Allow unsigned extensions** Go to Safari -> Develop -> "Allow Unsigned
   Extensions". This resets each time Safari quits, so re-enable per session.

### 5. Testing in Safari

After any change to the extension (JS, HTML, manifest, handler), you should re-run
this step.

1. **Build and open the host app**:

   ```sh
   strudel build --open
   ```

   Ad-hoc signing is fine for local dev. Safari only discovers extensions
   whose host `.app` has been registered with Launch Services, which happens
   the first time the app is opened.
2. **Enable the extension** Go to Safari -> Settings -> Extensions and enable yours.

#### Debugging

| Target              | How                                                                                                                           |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Background script   | Safari -> Develop -> Web Extension Background Content -> `<your extension>`                                                   |
| Popup UI            | Open the popup, right-click -> Inspect Element                                                                                |
| Content scripts     | Web Inspector on the page (⌘⌥I) -> Sources tab -> "Extensions"                                                                |
| Native handler logs | `Console.app`, filter by your extension's bundle id; or `log stream --predicate 'subsystem == "com.example.myapp.Extension"'` |



#### Common gotchas

**Extension doesn't show up in Safari's list**

The host `.app` wasn't opened after the build, or Safari is remembering a different copy. Run
`open <path-to-.app>` explicitly to be sure.

**"Extension is not signed" banner**

Re-enable "Allow Unsigned Extensions" in Safari's Develop menu (it resets per session).

**Stale code keeps loading**

Safari aggressively caches extension resources. Toggle the extension off/on, or quit Safari entirely.

**Permission prompt repeats**

any change to `permissions` in `manifest.json` re-prompts the user on next enable.

**`Undefined symbols: "_main"`** at link time

You're missing the `NSExtensionMain` shim from step 1; the extension target needs a
top-level `main.swift` calling it.

## App Extensions

strudel supports generic macOS app extensions (`kind = "app_extension"`) in addition to
Safari Web Extensions. Use this for any `NSExtension`-based extension type, e.g Share
Extensions and Network Extensions.

### 1. App Extension setup in Swift

Same as for Safari Web Extensions: each extension is a separate `executableTarget`
in `Package.swift`, and needs the `NSExtensionMain` entry-point shim:

```swift
// Sources/MyShareExtension/main.swift
import Foundation

@_silgen_name("NSExtensionMain")
func NSExtensionMain() -> Never

NSExtensionMain()
```

Your principal class (the one named by `NSExtensionPrincipalClass`) lives
alongside it. For a Share Extension it would conform to `NSExtensionRequestHandling`
or a subclass specific to the extension point.

### 2. Config

```toml
[app]
name         = "MyApp"
bundle_id    = "com.example.myapp"
version      = "1.0.0"
build_number = "1"

[[extensions]]
kind                       = "app_extension"
target_name                = "MyShareExtension"
bundle_id                  = "com.example.myapp.Share"
extension_point_identifier = "com.apple.share-services"
entitlements_json_path     = "share/entitlements.json"
# principal_class          = "MyShareExtension.ShareViewController"
```

Common `extension_point_identifier` values:

| Extension type           | Identifier                            |
| ------------------------ | ------------------------------------- |
| Share Extension          | `com.apple.share-services`            |
| Finder Sync Extension    | `com.apple.FinderSync`                |
| Notification Service Ext | `com.apple.usernotifications.service` |
| Quick Look Preview       | `com.apple.quicklook.preview`         |

strudel auto-injects into the extension's `Info.plist`:

- `CFBundleName`, `CFBundleDisplayName`, `CFBundleExecutable`, `CFBundleIdentifier`
- `CFBundleShortVersionString`, `CFBundleVersion` (inherited from `[app]`)
- `CFBundleInfoDictionaryVersion = "6.0"`, `CFBundlePackageType = "XPC!"`
- `NSExtension` dict with `NSExtensionPointIdentifier` (from config) and, when
  set, `NSExtensionPrincipalClass`

Use `info_json_path` to supply any additional `Info.plist` keys required by the
extension point (e.g. `NSExtensionAttributes` for certain extension types).

## Development

`strudel` is built with a standard rust toolchain, e.g.

```sh
# check for compilation errors
cargo check

# compile and run
cargo run

# compile release build
cargo build --release

# compile and install the release build into ~/.cargo/bin
cargo install --path .
```

## Other tips

- Use `swift-format` for formatting Swift code
- `// swift-tools-version: 6.0` in `Package.swift` to use `.v15`
- IconComposer, which comes bundled with Xcode, is great for making icons.

## Acknowledgements

🍻 and 🐙 to my Spring 2 '26 batchmates and everyone else at the [Recurse Center](https://www.recurse.com), you're the best!

Additionally, many thanks to the authors of the links below, all of it was invaluable in making strudel as easy-to-use as possible.

### appleid

- https://gist.github.com/JJTech0130/049716196f5f1751b8944d93e73d3452
- https://theapplewiki.com/wiki/Grand_Slam_Authentication
- https://github.com/MathewYaldo/Apple-GSA-Protocol
- https://github.com/SideStore/SideStore/wiki/Anisette-Docs

### dmg

- https://github.com/dmgbuild
- https://github.com/appdmg
- https://metacpan.org/dist/Mac-Finder-DSStore/view/DSStoreFormat.pod

### icon

- https://www.paintcodeapp.com/news/code-for-ios-7-rounded-rectangles
- https://liamrosenfeld.com/posts/apple_icon_quest/

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.
