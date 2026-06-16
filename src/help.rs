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
    (
        "global-config",
        "Machine-wide defaults in ~/.config/strudel/config.toml",
    ),
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
    (
        "ios-device",
        "iOS device builds: register, profile, auto-fetch",
    ),
];

pub fn run(topic: Option<&str>, mut app: Command) {
    match topic {
        None => print_index(&app),
        Some(t) => {
            let key = t.to_lowercase();
            match key.as_str() {
                "config" => print_config(),
                "global-config" | "global_config" => print_global_config(),
                "signing" => print_signing(),
                "notarize" | "notarization" => print_notarize(),
                "entitlements" => print_entitlements(),
                "extensions" | "extension" => print_extensions(),
                "dylibs" | "dylib" => print_dylibs(),
                "universal" => print_universal(),
                "ci" => print_ci(),
                "ios-device" | "ios_device" => print_ios_device(),
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
        Precedence: env var > strudel.toml > ~/.config/strudel/config.toml.
        See: {ANSI_BLUE}strudel help global-config{ANSI_RESET}

        ## [notarize] — optional (required for `release`)
        {ANSI_PURPLE}
        [notarize]
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        api_key      = "2X9R4HXF34"
        api_key_path = "AuthKey_2X9R4HXF34.p8"

        timeout = 600   # seconds to wait for notarytool; default: 600
        {ANSI_RESET}
        Precedence: env var > strudel.toml > ~/.config/strudel/config.toml.
        See: {ANSI_BLUE}strudel help global-config{ANSI_RESET}
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

        Three ways to set the identity (highest to lowest priority):

        1. Environment: {ANSI_GREEN}APPLE_SIGNING_IDENTITY{ANSI_RESET}, {ANSI_GREEN}APPLE_TEAM_ID{ANSI_RESET}
        2. Project config (strudel.toml):
        {ANSI_PURPLE}
        [signing]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"
        {ANSI_RESET}
        3. Global config (~/.config/strudel/config.toml) — shared across all projects:
        {ANSI_PURPLE}
        [signing]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"
        {ANSI_RESET}
        Edit the global config: {ANSI_BLUE}strudel config edit{ANSI_RESET}
        See: {ANSI_BLUE}strudel help global-config{ANSI_RESET}

        The identity string must match exactly what {ANSI_BLUE}security find-identity -v -p codesigning{ANSI_RESET}
        shows. Copy it from there to avoid typos.

        ## Ad-hoc signing (local dev)

        When no identity is configured, {ANSI_BLUE}strudel build{ANSI_RESET} uses ad-hoc signing (--sign -).
        Ad-hoc signatures let you run the app locally but the app cannot be distributed
        or notarized.

        ## Certificate import for CI (APPLE_CERTIFICATE)

        When running on CI where the signing identity is not already in a keychain:

        1. Export your Developer ID certificate as a .p12 from Keychain Access
           (right-click -> Export, set an export password)
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

        Obtain a key at: App Store Connect -> Users & Access -> Integrations -> App Store Connect API

        Three ways to set credentials (highest to lowest priority):

        1. Environment: {ANSI_GREEN}APPLE_API_ISSUER{ANSI_RESET}, {ANSI_GREEN}APPLE_API_KEY{ANSI_RESET}, {ANSI_GREEN}APPLE_API_KEY_PATH{ANSI_RESET}
        2. Project config (strudel.toml):
        {ANSI_PURPLE}
        [notarize]
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"  # Issuer ID
        api_key      = "2X9R4HXF34"                             # Key ID
        api_key_path = "AuthKey_2X9R4HXF34.p8"                  # path to .p8 file
        {ANSI_RESET}
        3. Global config (~/.config/strudel/config.toml) — shared across all projects.
           api_key_path here is typically an absolute path:
        {ANSI_PURPLE}
        [notarize]
        api_key_path = "/Users/you/.private_keys/AuthKey_2X9R4HXF34.p8"
        {ANSI_RESET}
        Edit the global config: {ANSI_BLUE}strudel config edit{ANSI_RESET}
        See: {ANSI_BLUE}strudel help global-config{ANSI_RESET}

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
        Identifiers & Profiles -> Profiles).

        ## iOS device builds

        strudel can auto-manage development provisioning profiles via the App Store Connect
        API (the same credentials used for notarization). The recommended workflow is:

          1. Run once to register your device on the portal and track it locally:
               strudel device register

          2. Then just run strudel device — it fetches and caches the profile automatically:
               strudel device

        The profile is cached at .strudel/<bundle_id>.mobileprovision (gitignored). On every
        build strudel checks whether the cached profile is still current (not expired, includes
        all tracked devices); if not, it re-fetches automatically.

        To manage the profile manually instead, set:
        {ANSI_PURPLE}
          [build]
          provisioning_profile = "path/to/MyApp.mobileprovision"
        {ANSI_RESET}
        When provisioning_profile is set strudel uses that file as-is and warns if it looks
        stale, but does not overwrite it. See {ANSI_BLUE}strudel help ios-device{ANSI_RESET} for the full workflow.

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

        Inside-out: embedded dylibs -> each .appex -> host .app. Never use --deep on the
        host — it would apply host entitlements to nested bundles incorrectly.
    "#});
}

fn print_ios_device() {
    print_help(&formatdoc! {r#"
        # iOS device builds

        ## One-time setup

        Register your device on the App Store Connect portal and track it locally:
        {ANSI_BLUE}
          strudel device register
        {ANSI_RESET}
        With a device connected and Developer Mode enabled, this registers it on the portal
        and adds it to .strudel/devices.toml (gitignored). Repeat whenever you add a device.

        To register a specific subset of connected devices:
        {ANSI_BLUE}
          strudel device register --device "iPhone 15" --device "iPad Air"
        {ANSI_RESET}
        ## Building and installing
        {ANSI_BLUE}
          strudel device
        {ANSI_RESET}
        On first run, strudel calls the App Store Connect API to:
          1. Look up (or create) the bundle ID
          2. Find your development certificate(s)
          3. Create a development profile embedding all tracked devices
          4. Cache the profile at .strudel/<bundle_id>.mobileprovision

        On subsequent runs the cached profile is reused if it is still current. A profile
        is considered stale when it has expired (within 5 minutes), is missing a device
        UDID, or the application-identifier no longer matches. Stale profiles are silently
        re-fetched.

        To target specific devices for one build (all must be in devices.toml):
        {ANSI_BLUE}
          strudel device --device "iPhone 15" --device "iPhone 16 Pro"
        {ANSI_RESET}
        ## Managing the profile manually

        To fetch or force-refresh the cached profile without building:
        {ANSI_BLUE}
          strudel profile
          strudel profile --force
        {ANSI_RESET}
        To opt out of auto-management and use your own profile, set in strudel.toml:
        {ANSI_PURPLE}
          [build]
          provisioning_profile = "path/to/MyApp.mobileprovision"
        {ANSI_RESET}
        ## Credentials required

        Profile auto-fetch uses the same App Store Connect API key as notarization:
        {ANSI_GREEN}
          APPLE_API_KEY_PATH   path to your .p8 key file
          APPLE_API_KEY        key ID (shown in App Store Connect)
          APPLE_API_ISSUER     issuer ID (shown in App Store Connect)
        {ANSI_RESET}
        See {ANSI_BLUE}strudel help notarize{ANSI_RESET} for how to configure these credentials.

        ## .strudel/devices.toml

        Tracked devices are stored in .strudel/devices.toml. This file is gitignored
        automatically by .strudel/.gitignore (written by strudel). Do not commit it.

        Example:
        {ANSI_PURPLE}
          [[device]]
          name = "My iPhone"
          udid = "00008101-001234AB3456001E"
        {ANSI_RESET}
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

fn print_global_config() {
    print_help(&formatdoc! {r#"
        # Global config

        {ANSI_BLUE}~/.config/strudel/config.toml{ANSI_RESET} stores machine-wide defaults shared across all
        projects on this machine. It is the lowest-priority source for each value:

          env var  >  strudel.toml  >  ~/.config/strudel/config.toml

        ## Editing
        {ANSI_BLUE}
        strudel config edit
        {ANSI_RESET}
        Opens the file in {ANSI_GREEN}$VISUAL{ANSI_RESET} / {ANSI_GREEN}$EDITOR{ANSI_RESET}, creating it with a template if it doesn't
        exist yet. The XDG_CONFIG_HOME env var overrides the default location.

        ## Supported keys
        {ANSI_PURPLE}
        [signing]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"

        [notarize]
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        api_key      = "2X9R4HXF34"
        api_key_path = "/Users/you/.private_keys/AuthKey_2X9R4HXF34.p8"
        {ANSI_RESET}
        Only {ANSI_PURPLE}[signing]{ANSI_RESET} and {ANSI_PURPLE}[notarize]{ANSI_RESET} are supported here. {ANSI_PURPLE}[app]{ANSI_RESET}, {ANSI_PURPLE}[build]{ANSI_RESET}, {ANSI_PURPLE}[ios]{ANSI_RESET}, {ANSI_PURPLE}[dmg]{ANSI_RESET}, and
        {ANSI_PURPLE}[[extensions]]{ANSI_RESET} are project-specific and belong only in strudel.toml.

        ## Typical use

        Store your signing identity and notarize credentials once in the global
        config, and leave each project's strudel.toml clean of machine-specific
        paths. This keeps strudel.toml committable to version control without
        embedding your developer identity or API key paths.

        A project strudel.toml can still override any global value by setting
        the same key; the env var overrides both.

        ## See also

        {ANSI_BLUE}strudel help signing{ANSI_RESET}
        {ANSI_BLUE}strudel help notarize{ANSI_RESET}
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

        1. Open Keychain Access -> find your Developer ID Application certificate
        2. Right-click -> Export -> save as DeveloperID.p12, set an export password
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
