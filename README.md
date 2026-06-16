# strudel

Build and ship macOS/iOS apps entirely from the command-line, without touching the Xcode IDE.

`strudel` uses the standard Apple toolchain (e.g. `swift`, `codesign`, `notarytool`) to build Swift Package Manager-based macOS and iOS apps with a config-driven, easy-to-introspect pipeline. It can produce signed `.app` bundles and notarized DMGs which can be distributed.

> **Current limitations**
> - **iOS support is still experimental.** `strudel sim` and `strudel device` work for local development, but distributing iOS apps is unsupported.
> - iOS provisioning profiles and devices need to be manually registered with Apple (this has only been tested with a paid Apple Developer account).
> - **App Store distribution is not supported yet.** strudel supports direct/notarized distribution (Developer ID) for macOS apps, but there is currently no support for submitting to the Mac App Store or iOS App Store.

- [Installation](#installation)
- [Example strudel build](#example-strudel-build)
- [Usage](#usage)
- [Config file structure](#config-file-structure)
- [Global config](#global-config)
- [Signing \& notarization](#signing--notarization)
- [Safari Web Extensions](#safari-web-extensions)
- [App Extensions](#app-extensions)
- [Development](#development)
- [Other tips](#other-tips)
- [Acknowledgements](#acknowledgements)

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

# create an unsigned app bundle
strudel bundle # --dry-run

# or create an ad-hoc codesigned app bundle
strudel build # --dry-run

# or create a real codesigned app bundel
export APPLE_SIGNING_IDENTITY=...
strudel build

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
  bundle     Build app bundle only (no signing/notarization)
  build      Build and sign the app bundle (no notarization or DMG); for local dev
  release    Full release: build, sign, notarize, and package DMG
  sim        Build for the iOS Simulator and launch in Simulator.app
  device     Build for a connected iOS device, then install and launch
  init       Scaffold a config file in the given directory
  config     Manage global strudel config (~/.config/strudel/config.toml)
  make-icns  Convert a PNG to .icns using sips + iconutil
  help       Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>  Path to config file [default: strudel.toml]
  -h, --help             Print help
  -V, --version          Print version
```

### `init`

Interactively scaffold a config file. Prompts for app name, bundle ID, version,
and build number, then writes the file into the given directory (defaults to the
current directory).

```sh
strudel init             # scaffold in the current directory
strudel init ./myapp     # scaffold in ./myapp
```

### `bundle`

Build the app bundle, without codesigning or notarization. Useful for local testing.
This cleans the old build, runs `swift build -c release`, and then assembles `.app`.

```sh
strudel bundle
strudel bundle --debug       # build with the debug configuration instead of release
strudel bundle --dry-run     # print commands without executing them
```

### `build`

Build and sign the app bundle, stopping at a signed `.app` (no notarization and no
DMG). This is the same as `strudel bundle` but also runs `codesign` at the end.

Meant for local development - hardened runtime and some entitlements only
take effect on a signed binary.

If `sign_identity` (`APPLE_SIGNING_IDENTITY`) is set, `strudel` signs with that identity;
otherwise it signs **ad-hoc** (`codesign --sign -`), which needs no certificate
or Apple account.

Note: Signed app bundles will still fail Gatekeeper checks and cannot be distributed
easily. For that, you will need to run `strudel release` to have your app notarized.

```sh
strudel build
strudel build --debug        # build with the debug configuration instead of release
strudel build --dry-run      # print commands without executing them
```

### `release`

Like `strudel build` but also creates a notarized DMG file so you can distribute your app.
This step requires valid signing credentials from a paid Apple Developer membership (see below).

```sh
strudel release
strudel release --dry-run    # print commands without executing them
```

Output artifacts are saved to `build_dir`:

- `<app_name>.app` is the signed, stapled app bundle
- `<app_name>-<version>.dmg` is the notarized, stapled DMG

Notarization may take a while the first time. Run `strudel help notarize` for more.

## Config file structure

The config file (`strudel.toml` by default) is TOML, organized into six
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
| `icon_path`              | string   | *(none)*            | `.icns` icon copied into the bundle. If unset, the bundle has no icon; if set, the file must exist                                                         |
| `archs`                  | string[] | host architecture   | Architectures passed to `swift build --arch`. Set multiple for a universal binary, e.g. `["arm64", "x86_64"]`                                              |
| `target_name`            | string   | value of `app.name` | Swift executable target name, if it differs from the app name                                                                                              |
| `embed_libs`             | string[] | *(none)*            | Dynamic C FFI libraries to embed in `Contents/Frameworks` and sign. Paths relative to config file                                                          |
| `resources_dir`          | string   | *(none)*            | Directory whose contents are copied wholesale into `Contents/Resources/`                                                                                   |
| `resources`              | string[] | *(none)*            | Individual files to copy into `Contents/Resources/` by filename                                                                                            |
| `provisioning_profile`   | string   | *(none)*            | Provisioning profile embedded as `Contents/embedded.provisionprofile`; required for some entitlements                                                      |

### `[ios]` (optional, experimental)

For iOS apps, this contains settings for `strudel sim` and `strudel device`. iOS support is experimental. All fields are optional.

> **Note:** strudel does not manage provisioning profiles or device registration. To use `strudel device`:
> 1. Register the device's UDID on the [Apple Developer portal](https://developer.apple.com/account/resources/devices/list).
> 2. Create a provisioning profile that includes that device on the [profiles page](https://developer.apple.com/account/resources/profiles/list).
> 3. Download the profile and point `provisioning_profile` in `[build]` at it.

| Key                 | Type   | Default       | Description                                                        |
| ------------------- | ------ | ------------- | ------------------------------------------------------------------ |
| `simulator`         | string | `"iPhone 16"` | Simulator name for `strudel sim`; override with `--simulator`      |
| `device`            | string | *(auto)*      | Device name or UDID for `strudel device`; auto-detected if unset   |
| `deployment_target` | string | `"18.0"`      | iOS deployment target, e.g. `"17.0"`                               |
| `assets_dir`        | string | *(none)*      | `.xcassets` directory compiled into the bundle with `xcrun actool` |
| `app_icon_name`     | string | `"AppIcon"`   | Icon set name inside `assets_dir`                                  |

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

| Key               | Type    | Default | Description                                                              |
| ----------------- | ------- | ------- | ------------------------------------------------------------------------ |
| `plain`           | bool    | `false` | Skip the styled window; produce a plain compressed DMG instead           |
| `background`      | string  | *(none)*| Path to a PNG or JPEG background image (relative to config file)         |
| `window_width`    | integer | `660`   | Finder window width in pixels                                            |
| `window_height`   | integer | `400`   | Finder window height in pixels                                           |
| `icon_size`       | integer | `128`   | Icon size in pixels                                                      |
| `app_x`           | integer | `192`   | Horizontal position of the `.app` icon                                   |
| `app_y`           | integer | `192`   | Vertical position of the `.app` icon                                     |
| `applications_x`  | integer | `468`   | Horizontal position of the Applications symlink                          |
| `applications_y`  | integer | `192`   | Vertical position of the Applications symlink                            |

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

### `[signing]` and `[notarize]` (optional in strudel.toml)

Required for `release`. Each identifier is resolved in priority order:
**env var > strudel.toml > [global config](#global-config)**. Secrets are
environment-only and have no config key. See [Signing & notarization](#signing--notarization)
for the full reference.

| Key                       | Type    | Env var                  | Description                                               |
| ------------------------- | ------- | ------------------------ | --------------------------------------------------------- |
| `[signing] identity`      | string  | `APPLE_SIGNING_IDENTITY` | Signing identity                                          |
| `[signing] team_id`       | string  | `APPLE_TEAM_ID`          | Apple Developer Team ID                                   |
| `[notarize] api_issuer`   | string  | `APPLE_API_ISSUER`       | App Store Connect issuer UUID                             |
| `[notarize] api_key`      | string  | `APPLE_API_KEY`          | App Store Connect key ID                                  |
| `[notarize] api_key_path` | string  | `APPLE_API_KEY_PATH`     | Path to the `.p8` key file                                |
| `[notarize] timeout`      | integer | —                        | Seconds to wait for notarization (`notarytool --timeout`) |

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

## Global config

`~/.config/strudel/config.toml` stores machine-wide defaults shared across all
projects. It is the lowest-priority source for each value:

```
env var  >  strudel.toml  >  ~/.config/strudel/config.toml
```

Open it in your editor (creating it with a template if it doesn't exist):

```sh
strudel config edit
```

Only `[signing]` and `[notarize]` are supported here — `[app]`, `[build]`,
`[ios]`, `[dmg]`, and `[[extensions]]` are project-specific and belong only
in `strudel.toml`.

```toml
# ~/.config/strudel/config.toml

[signing]
identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
team_id  = "XXXXXXXXXX"

[notarize]
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

Then, either set `identity` in `strudel.toml` under `[signing]`, or pass it via the
`APPLE_SIGNING_IDENTITY` env var.

| Config key           | Environment variable     | Description                                             |
| -------------------- | ------------------------ | ------------------------------------------------------- |
| `[signing] identity` | `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (XXXXXXXXXX)` |
| `[signing] team_id`  | `APPLE_TEAM_ID`          | 10-character Apple Developer Team ID                    |

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

| strudel.toml key          | Environment variable     | Description                                             |
| ------------------------- | ------------------------ | ------------------------------------------------------- |
| `[signing] identity`      | `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (XXXXXXXXXX)` |
| `[signing] team_id`       | `APPLE_TEAM_ID`          | 10-character Apple Developer Team ID                    |
| `[notarize] api_issuer`   | `APPLE_API_ISSUER`       | App Store Connect issuer UUID                           |
| `[notarize] api_key`      | `APPLE_API_KEY`          | App Store Connect key ID                                |
| `[notarize] api_key_path` | `APPLE_API_KEY_PATH`     | Path to the `AuthKey_XXXXXXYYYY.p8` file                |

`APPLE_API_ISSUER` is only present for team Apple Developer accounts.

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
| Background script   | Safari -> Develop -> Web Extension Background Content -> `<your extension>`                                                      |
| Popup UI            | Open the popup, right-click -> Inspect Element                                                                                 |
| Content scripts     | Web Inspector on the page (⌘⌥I) -> Sources tab -> "Extensions"                                                                  |
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

## Acknowledgements

🍻 and 🐙 to my Spring 2 '26 batchmates and everyone else at the [Recurse Center](https://www.recurse.com), you're the best!