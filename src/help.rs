use anstream::println;
use clap::Command;
use color_print::cformat;
use indoc::formatdoc;

const ANSI_GREEN: &str = "\x1b[32m"; // env vars
const ANSI_BLUE: &str = "\x1b[34m"; // code
const ANSI_PURPLE: &str = "\x1b[35m"; // toml
const ANSI_RESET: &str = "\x1b[0m";

const TOPICS: &[(&str, &str)] = &[
    ("config", "Full strudel.toml reference"),
    ("signing", "Code signing: Developer ID, keychain, ad-hoc"),
    ("notarize", "Notarization: App Store Connect API key auth"),
    ("entitlements", "Entitlements and provisioning profiles"),
    (
        "extensions",
        "App extensions: safari_web_extension, app_extension",
    ),
    ("dylibs", "Embedding dynamic C libraries in the bundle"),
    ("universal", "Universal (fat) binaries for arm64 + x86_64"),
    ("ci", "CI/CD setup: GitHub Actions, secrets, keychain"),
];

pub fn run(topic: Option<&str>, mut app: Command) {
    match topic {
        None => print_index(&app),
        Some(t) => {
            let key = t.to_lowercase();
            match key.as_str() {
                "config" => print_config(),
                "signing" => print_signing(),
                "notarize" | "notarization" => print_notarize(),
                "entitlements" => print_entitlements(),
                "extensions" | "extension" => print_extensions(),
                "dylibs" | "dylib" => print_dylibs(),
                "universal" => print_universal(),
                "ci" => print_ci(),
                _ => {
                    if let Some(sub) = app.find_subcommand_mut(&key) {
                        sub.print_long_help().unwrap();
                        println!();
                    } else {
                        println!("{}", cformat!("<red>Unknown topic: {t}</red>"));
                        eprintln!();
                        print_index(&app);
                        std::process::exit(1);
                    }
                },
            }
        },
    }
}

fn print_index(app: &Command) {
    println!("Available commands:");
    println!();
    for sub in app.get_subcommands() {
        let name = sub.get_name();
        if name == "help" {
            continue;
        }
        let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
        println!(
            "{}",
            cformat!("  <bold,green>{name:<14}</bold,green> {about}")
        );
    }
    println!();
    println!("Available topics:");
    println!();
    for (name, desc) in TOPICS {
        println!(
            "{}",
            cformat!("  <bold,green>{name:<14}</bold,green> {desc}")
        );
    }
    println!();
    println!("Usage: strudel help <topic/command>");
}

fn print_help(text: &str) {
    for line in text.lines() {
        if line.starts_with("# ") {
            println!("{}", cformat!("<bold,cyan>{}</bold,cyan>", line));
        } else if line.starts_with("## ") {
            println!("{}", cformat!("<bold,yellow>{}</bold,yellow>", line));
        } else {
            println!("{}", line);
        }
    }
}

fn print_config() {
    print_help(&formatdoc! {r#"
        # strudel.toml reference

        Relative paths are resolved relative to the config file's directory.
        Override the config path with: {ANSI_BLUE}strudel --config path/to/strudel.toml <cmd>{ANSI_RESET}

        ## [app] — required
        {ANSI_PURPLE}
        [app]
        name         = "MyApp"              # display name, .app bundle name, binary name
        bundle_id    = "com.example.myapp"  # CFBundleIdentifier
        version      = "1.0.0"              # CFBundleShortVersionString
        build_number = "1"                  # CFBundleVersion
        {ANSI_RESET}
        ## [build] — optional
        {ANSI_PURPLE}
        [build]
        source_dir             = "."                       # Swift package root; default: config file dir
        build_dir              = ".build/dist"             # output dir; relative to source_dir

        info_json_path         = "info.json"               # JSON merged into Info.plist
        entitlements_json_path = "entitlements.json"       # JSON entitlements
        icon_path              = "AppIcon.icns"            # path to .icns or .png app icon
        archs                  = ["arm64", "x86_64"]       # default: host arch only
        target_name            = "MyApp"                   # Swift executableTarget; default: app.name
        embed_libs             = ["path/to/libFoo.dylib"]  # dylibs copied into Contents/Frameworks
        provisioning_profile   = "MyApp.provisionprofile"  # required for some entitlements

        resources_dir          = "Resources"               # all files here copied into Contents/Resources/
        resources              = ["Assets/logo.png"]       # individual files to copy into Contents/Resources/
        {ANSI_RESET}
        ## [build_env] — optional

        Extra env vars forwarded to {ANSI_BLUE}swift build{ANSI_RESET} (e.g. for pkg-config):
        {ANSI_PURPLE}
        [build_env]
        PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig"
        {ANSI_RESET}
        ## [signing] — optional (required for `release`)
        {ANSI_PURPLE}
        [signing]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"
        {ANSI_RESET}
        Both can also be set via env: {ANSI_GREEN}APPLE_SIGNING_IDENTITY{ANSI_RESET}, {ANSI_GREEN}APPLE_TEAM_ID{ANSI_RESET}.
        Env var takes precedence if both are set.

        ## [notarize] — optional (required for `release`)
        {ANSI_PURPLE}
        [notarize]
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        api_key      = "2X9R4HXF34"
        api_key_path = "AuthKey_2X9R4HXF34.p8"

        timeout = 600   # seconds to wait for notarytool; default: 600
        {ANSI_RESET}
        ## [[extensions]] — optional, repeatable

        See: {ANSI_BLUE}strudel help extensions{ANSI_RESET}

        ## [dmg] — optional overrides (styled Finder window is the default)
        {ANSI_PURPLE}
        [dmg]
        plain          = false                         # set true for a plain UDZO DMG
        background     = "assets/dmg-background.png"  # PNG/JPEG background image; optional
        window_width   = 660                           # Finder window width (default shown)
        window_height  = 400                           # Finder window height (default shown)
        icon_size      = 128                           # icon size in pixels (default shown)
        app_x          = 192                           # .app icon X position (default shown)
        app_y          = 192                           # .app icon Y position (default shown)
        applications_x = 468                           # Applications symlink X (default shown)
        applications_y = 192                           # Applications symlink Y (default shown)
        {ANSI_RESET}
        By default (even with no `[dmg]` section), strudel stages the app, an
        Applications symlink, and a generated `.DS_Store` that lays out the Finder
        window (icon positions, size, background), then builds the compressed DMG
        directly from that folder. This is fully headless — no mounting, Finder,
        or AppleScript required.

        To skip window configuration and produce a plain compressed DMG directly:
        {ANSI_PURPLE}
        [dmg]
        plain = true
        {ANSI_RESET}
        All other fields are optional overrides; omit `[dmg]` entirely to use defaults.

        ## Environment secrets (never in strudel.toml)

        {ANSI_GREEN}APPLE_CERTIFICATE{ANSI_RESET}           base64-encoded Developer ID .p12 (CI use)
        {ANSI_GREEN}APPLE_CERTIFICATE_PASSWORD{ANSI_RESET}  export password for the .p12
    "#});
}

fn print_signing() {
    print_help(&formatdoc! {r#"
        # Code signing

        ## Configuring the signing identity

        Set in strudel.toml or via environment (env vars take precedence):
        {ANSI_PURPLE}
        [signing]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"
        {ANSI_RESET}
        - or -

        Env vars: {ANSI_GREEN}APPLE_SIGNING_IDENTITY{ANSI_RESET}, {ANSI_GREEN}APPLE_TEAM_ID{ANSI_RESET}

        The identity string must match exactly what {ANSI_BLUE}security find-identity -v -p codesigning{ANSI_RESET}
        shows. Copy it from there to avoid typos.

        ## Ad-hoc signing (local dev)

        When no identity is configured, {ANSI_BLUE}strudel build{ANSI_RESET} uses ad-hoc signing (--sign -).
        Ad-hoc signatures let you run the app locally but the app cannot be distributed
        or notarized.

        ## Certificate import for CI (APPLE_CERTIFICATE)

        When running on CI where the signing identity is not already in a keychain:

        1. Export your Developer ID certificate as a .p12 from Keychain Access
           (right-click → Export, set an export password)
        2. Base64-encode it:
               {ANSI_BLUE}base64 -i DeveloperID.p12 | pbcopy{ANSI_RESET}
        3. Set CI secrets:
               {ANSI_GREEN}APPLE_CERTIFICATE{ANSI_RESET}          (the base64 string)
               {ANSI_GREEN}APPLE_CERTIFICATE_PASSWORD{ANSI_RESET} (the export password you set)

        ## Sign order for bundles with extensions

        strudel signs inside-out:
          1. Embedded dylibs (Contents/Frameworks)
          2. Each .appex (with the extension's entitlements)
          3. Host .app (with the host entitlements)

        Do not use --deep on the host sign — it would re-apply host entitlements
        to nested bundles, which is incorrect.

        ## See also

        {ANSI_BLUE}strudel help notarize{ANSI_RESET}
        {ANSI_BLUE}strudel help entitlements{ANSI_RESET}
        {ANSI_BLUE}strudel help ci{ANSI_RESET}
    "#});
}

fn print_notarize() {
    print_help(&formatdoc! {r#"
        # Notarization

        Notarization is required for distributing a signed app outside the Mac App Store.
        strudel runs {ANSI_BLUE}xcrun notarytool submit{ANSI_RESET} and then {ANSI_BLUE}xcrun stapler staple{ANSI_RESET} automatically
        as part of {ANSI_BLUE}strudel release{ANSI_RESET}.

        ## Auth

        strudel uses the App Store Connect API key for notarization.

        Obtain a key at: App Store Connect → Users & Access → Integrations → App Store Connect API
        {ANSI_PURPLE}
        [notarize]
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"  # Issuer ID
        api_key      = "2X9R4HXF34"                             # Key ID
        api_key_path = "AuthKey_2X9R4HXF34.p8"                  # path to .p8 file
        {ANSI_RESET}
        - or -

        Env: {ANSI_GREEN}APPLE_API_ISSUER{ANSI_RESET}, {ANSI_GREEN}APPLE_API_KEY{ANSI_RESET}, {ANSI_GREEN}APPLE_API_KEY_PATH{ANSI_RESET}

        Env var takes precedence if both are set.

        ## Timeout
        {ANSI_PURPLE}
        [notarize]
        timeout = 600   # seconds; default: 600
        {ANSI_RESET}
        Notarization typically completes in under a minute, but Apple's servers can
        occasionally be slow.

        ## Troubleshooting

        - "Invalid API key": confirm api_key_path points to the correct .p8 file and
          api_key matches the Key ID shown in App Store Connect.

        ## See also

        {ANSI_BLUE}strudel help signing{ANSI_RESET}
        {ANSI_BLUE}strudel help ci{ANSI_RESET}
    "#});
}

fn print_entitlements() {
    print_help(&formatdoc! {r#"
        # Entitlements and provisioning profiles

        ## Entitlements file

        strudel reads a JSON entitlements file and converts it to a plist for `codesign`.
        Default path: entitlements.json (relative to config file).
        Override: build.entitlements_json_path = "path/to/entitlements.json"

        Minimal sandbox-only example:
          {{
            "com.apple.security.app-sandbox": true
          }}

        Example with network access:
          {{
            "com.apple.security.app-sandbox": true,
            "com.apple.security.network.client": true,
            "com.apple.security.network.server": true
          }}

        The entitlements file is optional. If the default path (entitlements.json) does
        not exist and no path is configured, strudel signs without entitlements.

        ## Provisioning profiles

        Some entitlements require a provisioning profile (e.g. push notifications, iCloud,
        HealthKit). To embed one:

          [build]
          provisioning_profile = "MyApp.provisionprofile"

        The profile is copied into Contents/embedded.provisionprofile inside the bundle.

        Provisioning profiles are created in the Apple Developer portal (Certificates,
        Identifiers & Profiles → Profiles).

        ## Ad-hoc + entitlements

        Ad-hoc signatures (no signing identity configured) won't work with entitlements
        that require a provisioning profile. strudel will warn when this is detected.

        ## Extensions

        Each extension gets its own entitlements file (required — extensions are sandboxed
        independently of the host app):

          [[extensions]]
          entitlements_json_path = "extension/entitlements.json"

        See: {ANSI_BLUE}strudel help extensions{ANSI_RESET}
    "#});
}

fn print_extensions() {
    print_help(&formatdoc! {r#"
        # App extensions

        App extensions are embedded as .appex bundles under Contents/PlugIns/ in the host
        app. Each extension is assembled and codesigned separately; notarizing the host .app
        covers all nested .appex bundles.

        ## Common fields (all kinds)
        {ANSI_PURPLE}
        [[extensions]]
        kind                   = "safari_web_extension"        # or "app_extension"
        target_name            = "MyExtension"                 # SPM executableTarget
        bundle_id              = "com.example.myapp.Extension"
        # name                 = "MyExtension"                 # defaults to target_name
        entitlements_json_path = "ext/entitlements.json"       # required
        # info_json_path       = "ext/info.json"               # optional extra Info.plist keys
        {ANSI_RESET}
        The SPM target must be an executableTarget in Package.swift.

        ## kind = "safari_web_extension"

        Embeds a Safari Web Extension. The resources_dir is copied wholesale into
        Contents/Resources/ (manifest.json, JS, HTML, icons, etc.).
        {ANSI_PURPLE}
        [[extensions]]
        kind          = "safari_web_extension"
        target_name   = "MyAppExtension"
        bundle_id     = "com.example.myapp.Extension"
        resources_dir = "extension/dist"                  # required; webpack output dir
        # principal_class = "MyAppExtension.SafariWebExtensionHandler"  # default shown
        {ANSI_RESET}
        strudel auto-injects NSExtension with:
          NSExtensionPointIdentifier = "com.apple.Safari.web-extension"
          NSExtensionPrincipalClass  = <principal_class>
          SFSafariWebExtensionManifestPath = "Resources/manifest.json"

        ## kind = "app_extension"

        Generic macOS app extension (Share, Finder Sync, Notification Service, Quick Look, etc.)
        {ANSI_PURPLE}
        [[extensions]]
        kind                       = "app_extension"
        target_name                = "MyShareExtension"
        bundle_id                  = "com.example.myapp.Share"
        entitlements_json_path     = "share/entitlements.json"
        extension_point_identifier = "com.apple.share-services"   # required
        # principal_class          = "MyShareExtension.ShareViewController"  # optional
        {ANSI_RESET}
        Common extension_point_identifier values:
          "com.apple.share-services"              Share Extension
          "com.apple.FinderSync"                  Finder Sync Extension
          "com.apple.usernotifications.service"   Notification Service Extension
          "com.apple.quicklook.preview"           Quick Look Preview Extension

        See https://developer.apple.com/documentation/bundleresources/information-property-list/nsextension/nsextensionpointidentifier

        ## Auto-injected Info.plist keys

        All extensions get: CFBundleExecutable, CFBundleIdentifier, CFBundleName,
        CFBundleDisplayName, CFBundleVersion, CFBundleShortVersionString,
        CFBundlePackageType = "XPC!"

        User-supplied info_json_path provides additional keys; auto-injected ones win
        on conflict.

        ## Sign order

        Inside-out: embedded dylibs → each .appex → host .app. Never use --deep on the
        host — it would apply host entitlements to nested bundles incorrectly.
    "#});
}

fn print_dylibs() {
    print_help(&formatdoc! {r#"
        # Embedding dynamic libraries

        For C FFI dylibs that must ship inside the bundle:
        {ANSI_PURPLE}
        [build]
        embed_libs = ["path/to/libFoo.dylib", "path/to/libBar.dylib"]
        {ANSI_RESET}
        Paths are relative to the config file's directory unless absolute.

        ## What strudel does

        For each dylib, strudel:
          1. Copies it into Contents/Frameworks/
          2. Re-writes its install name to @rpath/libFoo.dylib
          3. Updates the executable's load command to use @rpath/libFoo.dylib
          4. Signs the dylib (before signing the outer bundle)

        strudel also injects -rpath @executable_path/../Frameworks at link time via
        {ANSI_BLUE}-Xlinker -rpath -Xlinker @executable_path/../Frameworks{ANSI_RESET} in {ANSI_BLUE}swift build{ANSI_RESET}.

        ## Build-time flags

        Compile-time flags (-I, -L, -l, module maps) and linker flags still belong in
        Package.swift (cSettings / linkerSettings). strudel's embed_libs only handles
        the bundle assembly and signing step; it does not affect how {ANSI_BLUE}swift build{ANSI_RESET} finds
        or links the library.

        ## Static libraries

        Static libraries (.a) are linked directly into the binary and do not need to be
        listed in embed_libs — nothing to embed or sign.
    "#});
}

fn print_universal() {
    print_help(&formatdoc! {r#"
        # Universal binaries

        To produce a universal (fat) binary that runs natively on both Apple Silicon
        and Intel Macs:
        {ANSI_PURPLE}
        [build]
        archs = ["arm64", "x86_64"]
        {ANSI_RESET}
        strudel passes --arch arm64 --arch x86_64 to {ANSI_BLUE}swift build{ANSI_RESET}, which invokes the
        compiler twice and uses lipo to merge the outputs.

        ## Default behavior

        When archs is not set, strudel builds for the host architecture only (arm64 on
        Apple Silicon, x86_64 on Intel).

        ## Build time

        Universal builds take roughly twice as long and produce larger binaries. For local
        development, omit archs and only set it when building for distribution.

        ## Extensions

        When archs is set, all embedded extensions are also built as universal binaries
        using the same arch list.
    "#});
}

fn print_ci() {
    print_help(&formatdoc! {r#"
        # CI/CD setup

        ## Required secrets

        Set these as CI environment secrets (never commit them):

          {ANSI_GREEN}APPLE_SIGNING_IDENTITY{ANSI_RESET}      "Developer ID Application: Your Name (XXXXXXXXXX)"
          {ANSI_GREEN}APPLE_TEAM_ID{ANSI_RESET}               10-character team ID
          {ANSI_GREEN}APPLE_CERTIFICATE{ANSI_RESET}           base64-encoded Developer ID .p12
          {ANSI_GREEN}APPLE_CERTIFICATE_PASSWORD{ANSI_RESET}  export password for the .p12

        For notarization (App Store Connect API key):

          {ANSI_GREEN}APPLE_API_ISSUER{ANSI_RESET}    issuer UUID from App Store Connect
          {ANSI_GREEN}APPLE_API_KEY{ANSI_RESET}       key ID (e.g. "2X9R4HXF34")
          {ANSI_GREEN}APPLE_API_KEY_PATH{ANSI_RESET}  path to the .p8 file (or set inline — see below)

        ## GitHub Actions example

        jobs:
          release:
            runs-on: macos-latest
            steps:
              - uses: actions/checkout@v4

              - name: Install strudel
                run: cargo install strudel   # or use cargo-dist / axo

              - name: Release
                env:
                  APPLE_SIGNING_IDENTITY:     ${{ secrets.APPLE_SIGNING_IDENTITY }}
                  APPLE_TEAM_ID:              ${{ secrets.APPLE_TEAM_ID }}
                  APPLE_CERTIFICATE:          ${{ secrets.APPLE_CERTIFICATE }}
                  APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
                  APPLE_API_ISSUER:           ${{ secrets.APPLE_API_ISSUER }}
                  APPLE_API_KEY:              ${{ secrets.APPLE_API_KEY }}
                  APPLE_API_KEY_PATH:         AuthKey.p8
                run: |
                  echo "$APPLE_API_KEY_CONTENTS" > AuthKey.p8
                  strudel release

        ## Preparing APPLE_CERTIFICATE

        1. Open Keychain Access → find your Developer ID Application certificate
        2. Right-click → Export → save as DeveloperID.p12, set an export password
        3. Encode: {ANSI_BLUE}base64 -i DeveloperID.p12 | pbcopy{ANSI_RESET}
        4. Paste the result as the APPLE_CERTIFICATE secret

        ## Storing the .p8 API key in CI

        Option A: store the .p8 file contents as a secret (APPLE_API_KEY_CONTENTS),
        write it to disk before running strudel, set APPLE_API_KEY_PATH to the
        written path.

        Option B: commit the .p8 file to the repo (it is not a password, but treat it
        as sensitive). Set APPLE_API_KEY_PATH to its repo-relative path.

        ## See also

        {ANSI_BLUE}strudel help signing{ANSI_RESET}
        {ANSI_BLUE}strudel help notarize{ANSI_RESET}
    "#});
}
