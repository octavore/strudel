use color_print::cprintln;
use indoc::indoc;

const TOPICS: &[(&str, &str)] = &[
    ("config", "Full strudel.toml reference"),
    ("signing", "Code signing: Developer ID, keychain, ad-hoc"),
    ("notarize", "Notarization: Apple ID vs API key auth"),
    ("entitlements", "Entitlements and provisioning profiles"),
    (
        "extensions",
        "App extensions: safari_web_extension, app_extension",
    ),
    ("dylibs", "Embedding dynamic C libraries in the bundle"),
    ("universal", "Universal (fat) binaries for arm64 + x86_64"),
    ("ci", "CI/CD setup: GitHub Actions, secrets, keychain"),
];

pub fn run(topic: Option<&str>) {
    match topic {
        None => print_index(),
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
                    cprintln!("<red>Unknown topic: {t}</red>");
                    eprintln!();
                    print_index();
                    std::process::exit(1);
                },
            }
        },
    }
}

fn print_index() {
    println!("Available topics:");
    println!();
    for (name, desc) in TOPICS {
        cprintln!("  <bold>{name:<14}</bold> {desc}");
    }
    println!();
    println!("Usage: strudel help <topic>");
}

fn print_help(text: &str) {
    for line in text.lines() {
        if line.starts_with("# ") {
            cprintln!("<bold,cyan>{}</bold,cyan>", line);
        } else if line.starts_with("## ") {
            cprintln!("<bold,yellow>{}</bold,yellow>", line);
        } else {
            println!("{}", line);
        }
    }
}

fn print_config() {
    print_help(indoc! {r#"
        # strudel.toml reference

        Relative paths are resolved relative to the config file's directory.
        Override the config path with: strudel --config path/to/strudel.toml <cmd>

        ## [app] — required

        [app]
        name         = "MyApp"              # display name, .app bundle name, binary name
        bundle_id    = "com.example.myapp"  # CFBundleIdentifier
        version      = "1.0.0"             # CFBundleShortVersionString
        build_number = "1"                 # CFBundleVersion

        ## [build] — all optional

        [build]
        source_dir             = "."                     # Swift package root; default: config file dir
        build_dir              = ".build/dist"           # output dir; relative to source_dir
        info_json_path         = "info.json"             # JSON merged into Info.plist
        entitlements_json_path = "entitlements.json"     # JSON entitlements
        icon_path              = "AppIcon.icns"          # .icns app icon
        archs                  = ["arm64", "x86_64"]     # default: host arch only
        target_name            = "MyApp"                 # Swift executableTarget; default: app.name
        embed_libs             = ["path/to/libFoo.dylib"] # dylibs → Contents/Frameworks
        provisioning_profile   = "MyApp.provisionprofile" # required for some entitlements
        resources_dir          = "Resources"               # dir contents → Contents/Resources/
        resources              = ["Assets/logo.png"]       # individual files → Contents/Resources/

        ## [build_env] — optional

        Extra env vars forwarded to `swift build` (e.g. for pkg-config):

        [build_env]
        PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig"

        ## [signing] — optional (required for `release`)

        [signing]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"

        Both can also be set via env: APPLE_SIGNING_IDENTITY, APPLE_TEAM_ID.
        Config value wins if both are set.

        ## [notarize] — optional (required for `release`)

        [notarize]
        # API key auth (preferred)
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        api_key      = "2X9R4HXF34"
        api_key_path = "AuthKey_2X9R4HXF34.p8"

        # Apple ID auth (fallback; also needs APPLE_PASSWORD env var)
        apple_id = "you@example.com"

        timeout = 600   # seconds to wait for notarytool; default: 600

        ## [[extensions]] — optional, repeatable

        See: strudel help extensions

        ## Environment secrets (never in strudel.toml)

        APPLE_PASSWORD              app-specific password (Apple ID auth)
        APPLE_CERTIFICATE           base64-encoded Developer ID .p12 (CI use)
        APPLE_CERTIFICATE_PASSWORD  export password for the .p12
    "#});
}

fn print_signing() {
    print_help(indoc! {r#"
        # Code signing

        ## Configuring the signing identity

        Set in strudel.toml or via environment (env wins):

        [signing]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"

        Env vars: APPLE_SIGNING_IDENTITY, APPLE_TEAM_ID

        The identity string must match exactly what `security find-identity -v -p codesigning`
        shows. Copy it from there to avoid typos.

        ## Ad-hoc signing (local dev)

        When no identity is configured, `strudel sign` uses ad-hoc signing (--sign -).
        Ad-hoc signatures let you run the app locally but the app cannot be distributed
        or notarized.

        ## Certificate import for CI (APPLE_CERTIFICATE)

        When running on CI where the signing identity is not already in a keychain:

        1. Export your Developer ID certificate as a .p12 from Keychain Access
           (right-click → Export, set an export password)
        2. Base64-encode it:
               base64 -i DeveloperID.p12 | pbcopy
        3. Set CI secrets:
               APPLE_CERTIFICATE           ← the base64 string
               APPLE_CERTIFICATE_PASSWORD  ← the export password you set

        strudel will import the certificate into a temporary keychain automatically.

        ## Sign order for bundles with extensions

        strudel signs inside-out:
          1. Embedded dylibs (Contents/Frameworks)
          2. Each .appex (with the extension's entitlements)
          3. Host .app (with the host entitlements)

        Do not use --deep on the host sign — it would re-apply host entitlements
        to nested bundles, which is incorrect.

        ## See also

        strudel help notarize
        strudel help entitlements
        strudel help ci
    "#});
}

fn print_notarize() {
    print_help(indoc! {r#"
        # Notarization

        Notarization is required for distributing a signed app outside the Mac App Store.
        strudel runs `xcrun notarytool submit` and then `xcrun stapler staple` automatically
        as part of `strudel release`.

        ## Auth methods

        strudel uses App Store Connect API key auth when fully configured; falls back to
        Apple ID auth otherwise.

        ### API key auth (preferred — works in CI without 2FA headaches)

        Obtain a key at: App Store Connect → Users & Access → Integrations → App Store Connect API

        [notarize]
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"  # Issuer ID
        api_key      = "2X9R4HXF34"                             # Key ID
        api_key_path = "AuthKey_2X9R4HXF34.p8"                  # path to .p8 file

        Env equivalents: APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH
        Config value wins if both are set.

        ### Apple ID auth (fallback)

        [notarize]
        apple_id = "you@example.com"  # also set via APPLE_ID

        Required env secret (no config key):
          APPLE_PASSWORD  ← app-specific password from appleid.apple.com
                            (Account → Sign-In and Security → App-Specific Passwords)

        Also requires signing.team_id (or APPLE_TEAM_ID) to be set.

        ## Timeout

        [notarize]
        timeout = 600   # seconds; default: 600

        Notarization typically completes in under a minute, but Apple's servers can
        occasionally be slow.

        ## Troubleshooting

        - "Unable to find app-specific password": the APPLE_PASSWORD value is wrong or
          the app-specific password was revoked. Generate a new one.
        - "Team ID not found": ensure signing.team_id / APPLE_TEAM_ID is set.
        - "Invalid API key": confirm api_key_path points to the correct .p8 file and
          api_key matches the Key ID shown in App Store Connect.

        ## See also

        strudel help signing
        strudel help ci
    "#});
}

fn print_entitlements() {
    print_help(indoc! {r#"
        # Entitlements and provisioning profiles

        ## Entitlements file

        strudel reads a JSON entitlements file and converts it to a plist for `codesign`.
        Default path: entitlements.json (relative to config file).
        Override: build.entitlements_json_path = "path/to/entitlements.json"

        Minimal sandbox-only example:
          {
            "com.apple.security.app-sandbox": true
          }

        Example with network access:
          {
            "com.apple.security.app-sandbox": true,
            "com.apple.security.network.client": true,
            "com.apple.security.network.server": true
          }

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

        See: strudel help extensions
    "#});
}

fn print_extensions() {
    print_help(indoc! {r#"
        # App extensions

        App extensions are embedded as .appex bundles under Contents/PlugIns/ in the host
        app. Each extension is assembled and codesigned separately; notarizing the host .app
        covers all nested .appex bundles.

        ## Common fields (all kinds)

        [[extensions]]
        kind                   = "safari_web_extension"        # or "app_extension"
        target_name            = "MyExtension"                 # SPM executableTarget
        bundle_id              = "com.example.myapp.Extension"
        # name                 = "MyExtension"                 # defaults to target_name
        entitlements_json_path = "ext/entitlements.json"       # required
        # info_json_path       = "ext/info.json"               # optional extra Info.plist keys

        The SPM target must be an executableTarget in Package.swift.

        ## kind = "safari_web_extension"

        Embeds a Safari Web Extension. The resources_dir is copied wholesale into
        Contents/Resources/ (manifest.json, JS, HTML, icons, etc.).

        [[extensions]]
        kind          = "safari_web_extension"
        target_name   = "MyAppExtension"
        bundle_id     = "com.example.myapp.Extension"
        resources_dir = "extension/dist"                  # required; webpack output dir
        # principal_class = "MyAppExtension.SafariWebExtensionHandler"  # default shown

        strudel auto-injects NSExtension with:
          NSExtensionPointIdentifier = "com.apple.Safari.web-extension"
          NSExtensionPrincipalClass  = <principal_class>
          SFSafariWebExtensionManifestPath = "Resources/manifest.json"

        ## kind = "app_extension"

        Generic macOS app extension (Share, Finder Sync, Notification Service, Quick Look, etc.)

        [[extensions]]
        kind                       = "app_extension"
        target_name                = "MyShareExtension"
        bundle_id                  = "com.example.myapp.Share"
        entitlements_json_path     = "share/entitlements.json"
        extension_point_identifier = "com.apple.share-services"   # required
        # principal_class          = "MyShareExtension.ShareViewController"  # optional

        Common extension_point_identifier values:
          "com.apple.share-services"              Share Extension
          "com.apple.FinderSync"                  Finder Sync Extension
          "com.apple.usernotifications.service"   Notification Service Extension
          "com.apple.quicklook.preview"           Quick Look Preview Extension

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
    print_help(indoc! {r#"
        # Embedding dynamic libraries

        For C FFI dylibs that must ship inside the bundle:

          [build]
          embed_libs = ["path/to/libFoo.dylib", "path/to/libBar.dylib"]

        Paths are relative to the config file's directory unless absolute.

        ## What strudel does

        For each dylib, strudel:
          1. Copies it into Contents/Frameworks/
          2. Re-writes its install name to @rpath/libFoo.dylib
          3. Updates the executable's load command to use @rpath/libFoo.dylib
          4. Signs the dylib (before signing the outer bundle)

        strudel also injects -rpath @executable_path/../Frameworks at link time via
        `-Xlinker -rpath -Xlinker @executable_path/../Frameworks` in swift build.

        ## Build-time flags

        Compile-time flags (-I, -L, -l, module maps) and linker flags still belong in
        Package.swift (cSettings / linkerSettings). strudel's embed_libs only handles
        the bundle assembly and signing step; it does not affect how swift build finds
        or links the library.

        ## Static libraries

        Static libraries (.a) are linked directly into the binary and do not need to be
        listed in embed_libs — nothing to embed or sign.
    "#});
}

fn print_universal() {
    print_help(indoc! {r#"
        # Universal binaries

        To produce a universal (fat) binary that runs natively on both Apple Silicon
        and Intel Macs:

          [build]
          archs = ["arm64", "x86_64"]

        strudel passes --arch arm64 --arch x86_64 to `swift build`, which invokes the
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
    print_help(indoc! {r#"
        # CI/CD setup

        ## Required secrets

        Set these as CI environment secrets (never commit them):

          APPLE_SIGNING_IDENTITY      "Developer ID Application: Your Name (XXXXXXXXXX)"
          APPLE_TEAM_ID               10-character team ID
          APPLE_CERTIFICATE           base64-encoded Developer ID .p12
          APPLE_CERTIFICATE_PASSWORD  export password for the .p12

        For notarization, either:

          API key auth (preferred):
            APPLE_API_ISSUER    issuer UUID from App Store Connect
            APPLE_API_KEY       key ID (e.g. "2X9R4HXF34")
            APPLE_API_KEY_PATH  path to the .p8 file (or set inline — see below)

          Apple ID auth (fallback):
            APPLE_ID            Apple ID email
            APPLE_PASSWORD      app-specific password

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
        3. Encode: base64 -i DeveloperID.p12 | pbcopy
        4. Paste the result as the APPLE_CERTIFICATE secret

        strudel imports the certificate into a temporary keychain automatically when
        APPLE_CERTIFICATE is set.

        ## Storing the .p8 API key in CI

        Option A: store the .p8 file contents as a secret (APPLE_API_KEY_CONTENTS),
        write it to disk before running strudel, set APPLE_API_KEY_PATH to the
        written path.

        Option B: commit the .p8 file to the repo (it is not a password, but treat it
        as sensitive). Set APPLE_API_KEY_PATH to its repo-relative path.

        ## See also

        strudel help signing
        strudel help notarize
    "#});
}
