use clap::Command;
use clml::{cformatdoc, cprintln};

pub(crate) const TOPICS: &[(&str, &str)] = &[
    ("config", "Full strudel.toml reference"),
    (
        "targets",
        "Multiple targets: product x platform in one strudel.toml",
    ),
    (
        "global-config",
        "Machine-wide defaults in ~/.config/strudel/config.toml",
    ),
    ("signing", "Code signing: Developer ID, keychain, ad-hoc"),
    ("notarize", "Notarization: App Store Connect API key auth"),
    ("entitlements", "Entitlements and provisioning profiles"),
    (
        "extensions",
        "App and system extensions: safari_web_extension, app_extension, system_extension",
    ),
    (
        "dylibs",
        "Embedding dynamic C libraries and .framework bundles in the bundle",
    ),
    (
        "copy",
        "Copying arbitrary files/directories into the bundle, optionally signed",
    ),
    ("universal", "Universal (fat) binaries for arm64 + x86_64"),
    ("ci", "CI/CD setup: GitHub Actions, secrets, keychain"),
    (
        "ios-device",
        "iOS device builds: register, profile, auto-fetch",
    ),
    (
        "ios-free-provisioning",
        "Free Apple ID provisioning: login, 7-day profiles, no paid account needed",
    ),
    (
        "ios-app",
        "Quick checklist for iOS app setup: launch screen, permission strings, icons, dark mode",
    ),
    (
        "macos-app",
        "Quick checklist for macOS app niceties: settings window, menu bar icon, sandboxing, activation policy",
    ),
];

pub fn run(topic: Option<&str>, mut app: Command) {
    match topic {
        None => print_index(&app),
        Some(t) => {
            let key = t.to_lowercase();
            match key.as_str() {
                "config" => print_config(),
                "targets" | "target" => print_targets(),
                "global-config" | "global_config" => print_global_config(),
                "signing" => print_signing(),
                "notarize" | "notarization" => print_notarize(),
                "entitlements" => print_entitlements(),
                "extensions" | "extension" => print_extensions(),
                "dylibs" | "dylib" | "frameworks" | "framework" => print_dylibs(),
                "copy" => print_copy(),
                "universal" => print_universal(),
                "ci" => print_ci(),
                "ios-device" | "ios_device" => print_ios_device(),
                "ios-free-provisioning"
                | "ios_free_provisioning"
                | "free-provisioning"
                | "free_provisioning" => print_ios_free_provisioning(),
                "ios-app" | "ios_app" | "ios-guidelines" | "ios_guidelines" => print_ios_app(),
                "macos-app" | "macos_app" | "macos-guidelines" | "macos_guidelines" => {
                    print_macos_app()
                },
                _ => {
                    if let Some(sub) = app.find_subcommand_mut(&key) {
                        sub.print_long_help().unwrap();
                        println!();
                    } else {
                        cprintln!("<red>Unknown topic: {t}</red>");
                        eprintln!();
                        print_index(&app);
                        std::process::exit(1);
                    }
                },
            }
        },
    }
}

/// Top-level subcommands (name, short about), in clap's declared order,
/// excluding `help` itself. Shared by `strudel help` and `strudel skill
/// install`, so neither can drift from the actual command list.
pub(crate) fn commands(app: &Command) -> Vec<(String, String)> {
    app.get_subcommands()
        .filter(|sub| sub.get_name() != "help")
        .map(|sub| {
            let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
            (sub.get_name().to_string(), about)
        })
        .collect()
}

fn print_index(app: &Command) {
    println!("Available commands:");
    println!();
    for (name, about) in commands(app) {
        cprintln!("  <bold,green>{name:<14}</bold,green> {about}");
    }
    println!();
    println!("Available topics:");
    println!();
    for (name, desc) in TOPICS {
        cprintln!("  <bold,green>{name:<14}</bold,green> {desc}");
    }
    println!();
    println!("Usage: strudel help <topic/command>");
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

fn print_targets() {
    print_help(&cformatdoc! {r#"
        # Multiple targets (product x platform)

        A single `strudel.toml` can declare multiple build targets using `[[target]]`
        blocks. Each target is a product x platform pair with its own [app], [build],
        [[extensions]], [dmg], and optional [ios] settings.

        ## When to use [[target]]

        - Cross-platform projects shipping both a macOS app and an iOS app from the
          same Swift package.
        - Monorepos with multiple executables that share signing/notarization creds.

        ## Example: macOS + iOS from one strudel.toml
        <magenta>
        # Shared across all targets:
        [apple]
        identity     = "Developer ID Application: You (XXXXXXXXXX)"
        team_id      = "XXXXXXXXXX"
        api_key      = "2X9R4HXF34"
        api_key_path = "AuthKey_2X9R4HXF34.p8"

        # Optional top-level [ios] supplies defaults for iOS targets.
        # A per-target ios.* field wins over the matching top-level field.
        [ios]
        simulator = "iPhone 16"

        [[target]]
        platform = "macos"
        app.name         = "MyApp"
        app.bundle_id    = "com.example.app"
        app.version      = "1.0.0"
        app.build_number = "1"
        build.entitlements_json_path = "mac/entitlements.json"

        [[target.extensions]]
        kind          = "safari_web_extension"
        target_name   = "MyAppExtension"
        bundle_id     = "com.example.app.Extension"
        resources_dir = "extension/dist"
        entitlements_json_path = "extension/entitlements.json"

        [[target]]
        platform = "ios"
        app.name         = "MyApp"
        app.bundle_id    = "com.example.app"
        app.version      = "1.0.0"
        app.build_number = "1"
        ios.deployment_target = "18.0"
        </>
        ## Rules

        - A config defines EITHER a top-level [app] OR one or more [[target]] blocks,
          never both. Mixing them is an error.
        - `platform` is required on every [[target]] block. Must be `"macos"` or `"ios"`.
        - `[apple]` is always shared (top-level only).
        - `[ios]` at the top level supplies defaults for iOS targets; a per-target
          `ios.*` field wins over the matching top-level field, field by field.

        ## Selecting targets at runtime

        Every target has an id of `<<platform>>/<<app.name>>`, e.g. `macos/MyApp` and
        `ios/MyApp`. Two targets on the same platform may not share an app name.

        When multiple targets are eligible for a command, strudel runs them all and
        prints a per-target header. To narrow to a single target, give any substring
        of its id, eg:
        <blue>
        strudel build ios/MyApp    # the whole id
        strudel build mac          # a prefix of the platform
        strudel build MyApp        # the app name
        </>
        A selector must select exactly one target. `MyApp` above works only if a
        single target carries that app name; when both a macOS and an iOS target do,
        strudel will report an error. Exact id matches always works, eg `ios/App`
        selects that target even alongside `ios/AppPro`.

        `build`, `run`, and `release` take the selector as a positional argument and
        dispatch per target based on its own platform (macOS or iOS). Other commands
        (`devices`, `profile`, `status`, `clean`) take `--target` instead.

        `strudel run` with no selector, `--sim`, or `--device` is an exception: it
        only runs macOS targets, since launching iOS needs `--sim` or `--device` to
        say where. Pass `--sim`/`--device` to run iOS targets instead, or a selector
        to target the app explicitly regardless of platform.

        ## Build directories

        With multiple targets, each gets its own build directory, named for its target
        id so they cannot collide:
          .build/dist/macos/<<name>>
          .build/dist/ios/<<name>>

        Override per-target with <magenta>build.build_dir</>.

        ## iOS extension caveat

        Assembling and signing [[extensions]] inside an iOS bundle is not yet
        supported. A warning is printed and the extensions list is ignored for
        iOS `strudel build` and `strudel run`.

        ## See also

        <blue>strudel help config</>
        <blue>strudel help extensions</>
    "#});
}

fn print_config() {
    print_help(&cformatdoc! {r##"
        # strudel.toml reference

        Relative paths are resolved relative to the config file's directory.
        Override the config path with: <blue>strudel --config path/to/strudel.toml <<cmd>></>

        ## [app] required
        <magenta>
        [app]
        name         = "MyApp"              # display name, .app bundle name, binary name
        bundle_id    = "com.example.myapp"  # CFBundleIdentifier
        version      = "1.0.0"              # CFBundleShortVersionString
        build_number = "1"                  # CFBundleVersion; default: "1"
        </>
        ## [build] optional
        <magenta>
        [build]
        source_dir             = "."                       # Swift package root; default: config file dir
        build_dir              = ".build/dist"             # output dir; relative to source_dir

        info_json_path         = "info.json"               # JSON merged into Info.plist
        entitlements_json_path = "entitlements.json"       # JSON entitlements
        archs                  = ["arm64", "x86_64"]       # default: host arch only
        target_name            = "MyApp"                   # Swift executableTarget; default: app.name
        embed_libs             = ["libFoo.dylib"]           # dylibs/.frameworks; see `strudel help dylibs`
        provisioning_profile   = "MyApp.provisionprofile"  # required for some entitlements

        resources_dir          = "Resources"               # all files here copied into Contents/Resources/
        resources              = ["Assets/logo.png"]       # individual files/folders to copy into Contents/Resources/
        </>
        Both resolve relative to the config file's directory unless absolute, same as
        any other path. If the resolved location doesn't exist, strudel falls back to
        the current build's `.build/<<triple>>/release/` output dir instead - so a bare
        name (e.g. a SwiftPM-generated resource bundle) still works without listing a
        path.

        ## [[build.copy]] optional, repeatable
        # Arbitrary files/directories copied to a caller-chosen destination inside the
        # bundle (e.g. a helper binary), optionally signed. See <blue>strudel help copy</>.
        <magenta>
        [[build.copy]]
        src      = "path/to/helper"
        dest_dir = "Contents/MacOS"
        sign     = true
        </>
        ## assets_dir optional; top-level key (like [dmg]), not under [build]
        # xcassets catalog compiled into Contents/Resources/Assets.car via actool.
        # Deployment target is read from Package.swift's platforms entry; defaults to 14.0 if none.
        <magenta>
        assets_dir = "Sources/App/Assets.xcassets"
        </>
        ## [build.icon] optional; no icon if unset
        # Either a png or icns file copied in unmodified (set icon.path), or
        # generate an icon from a png or svg at build time (icon.src). For iOS
        # targets, ios.assets_dir takes precedence if both are set.
        <magenta>
        icon.src        = "art.png"
        icon.scale      = 1.2               # optional
        icon.background = "#fefefe"       # optional; hex, defaults to white
        </>
        ## [build.build_env] optional

        Extra env vars forwarded to <blue>swift build</> (e.g. for pkg-config):
        <magenta>
        [build.build_env]
        PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig"
        </>
        ## [apple] optional, but required for `release`

        Apple developer identifiers, shared by signing, notarization, and
        provisioning-profile management (the App Store Connect API key
        authenticates all three).
        <magenta>
        [apple]
        identity     = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id      = "XXXXXXXXXX"

        api_issuer       = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        api_key          = "2X9R4HXF34"
        api_key_path     = "AuthKey_2X9R4HXF34.p8"
        notarize_timeout = 600   # seconds to wait for notarytool; default: 600
        </>
        Precedence: env var > strudel.toml > ~/.config/strudel/config.toml.
        See: <blue>strudel help global-config</>

        ## [[target]] optional, repeatable (multi-target configs)

        Replace [app] with one or more [[target]] blocks to build multiple
        products or platforms (e.g. macOS + iOS) from the same strudel.toml.
        See: <blue>strudel help targets</>

        ## [[extensions]] optional, repeatable

        See: <blue>strudel help extensions</>

        ## [ios] optional

        Only valid inside an iOS [[target]] block, or as a top-level fallback for
        iOS targets in a multi-target config - the flat single-app form above is
        always macOS. See: <blue>strudel help targets</>
        <magenta>
        [ios]
        simulator         = "iPhone 16"  # default; override with --simulator
        device            = "My iPhone"  # name or UDID; auto-detected if unset
        deployment_target = "18.0"       # iOS deployment target
        assets_dir        = "Sources/App/Assets.xcassets"  # xcassets for actool; takes precedence over [build.icon]
        app_icon_name     = "AppIcon"    # icon set name inside assets_dir

        # Provisioning backend, required for device builds. Choose one:
        #   "app_store_connect"  paid account + App Store Connect API key; 1-year profiles
        #   "free"               any Apple ID, no paid account; 7-day profiles, max 3 devices
        provisioning = "app_store_connect"  # or "free"
        apple_id     = "you@example.com"    # pre-fills the login prompt (free path only)
        </>
        See <blue>strudel help ios-device</> and <blue>strudel help ios-free-provisioning</>.

        ## [dmg] optional overrides for macOS DMG window layout
        <magenta>
        [dmg]
        plain          = false                         # set true for a plain UDZO DMG
        background     = "assets/dmg-background.png"  # PNG/JPEG image or "#rrggbb" color; optional
        window_width   = 660                           # Finder window width (default shown)
        window_height  = 400                           # Finder window height (default shown)
        icon_size      = 128                           # icon size in pixels (default shown)
        app_x          = 192                           # .app icon X position (default shown)
        app_y          = 192                           # .app icon Y position (default shown)
        applications_x = 468                           # Applications symlink X (default shown)
        applications_y = 192                           # Applications symlink Y (default shown)
        icon_text_size = 12.0                         # icon label point size (default shown)
        </>
        By default (even with no `[dmg]` section), strudel stages the app, an
        Applications symlink, and a generated `.DS_Store` that lays out the Finder
        window (icon positions, size, background), then builds the compressed DMG
        directly from that folder.

        To skip window configuration and produce a plain compressed DMG directly:
        <magenta>
        [dmg]
        plain = true
        </>
        All other fields are optional overrides; omit `[dmg]` entirely to use defaults.

        ## Environment secrets (never in strudel.toml)

        <green>APPLE_CERTIFICATE</>           base64-encoded Developer ID .p12 (CI use)
        <green>APPLE_CERTIFICATE_PASSWORD</>  export password for the .p12
    "##});
}

fn print_signing() {
    print_help(&cformatdoc! {r#"
        # Code signing

        ## Configuring the signing identity

        Three ways to set the identity (highest to lowest priority):

        1. Environment: <green>APPLE_SIGNING_IDENTITY</>, <green>APPLE_TEAM_ID</>
        2. Project config (strudel.toml):
        <magenta>
        [apple]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"
        </>
        3. Global config (~/.config/strudel/config.toml) is shared across all projects:
        <magenta>
        [apple]
        identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id  = "XXXXXXXXXX"
        </>
        Edit the global config: <blue>strudel config global edit</>
        See: <blue>strudel help global-config</>

        The identity string must match exactly what <blue>security find-identity -v -p codesigning</>
        shows. Copy it from there to avoid typos.

        ## Ad-hoc signing (local dev)

        When no identity is configured, <blue>strudel build</> uses ad-hoc signing (--sign -).
        Ad-hoc signatures let you run the app locally but the app cannot be distributed
        or notarized.

        ## Certificate import for CI (APPLE_CERTIFICATE)

        When running on CI where the signing identity is not already in a keychain:

        1. Export your Developer ID certificate as a .p12 from Keychain Access
           (right-click -> Export, set an export password)
        2. Base64-encode it:
               <blue>base64 -i DeveloperID.p12 | pbcopy</>
        3. Set CI secrets:
               <green>APPLE_CERTIFICATE</>          (the base64 string)
               <green>APPLE_CERTIFICATE_PASSWORD</> (the export password you set)

        ## Sign order for bundles with extensions

        strudel signs inside-out:
          1. Embedded dylibs (Contents/Frameworks)
          2. Each extension bundle - .appex or .systemextension (with the extension's
             entitlements)
          3. Host .app (with the host entitlements)

        We do not use --deep on the host sign as it would incorrectly re-apply host entitlements
        to nested bundles.

        ## See also

        <blue>strudel help notarize</>
        <blue>strudel help entitlements</>
        <blue>strudel help ci</>
    "#});
}

fn print_notarize() {
    print_help(&cformatdoc! {r#"
        # Notarization

        Notarization is required for distributing a signed app outside the Mac App Store.
        strudel runs <blue>xcrun notarytool submit</> and then <blue>xcrun stapler staple</> automatically
        as part of <blue>strudel release</>.

        ## Auth

        strudel uses the App Store Connect API key for notarization.

        Obtain a key at: App Store Connect -> Users & Access -> Integrations -> App Store Connect API

        Three ways to set credentials (highest to lowest priority):

        1. Environment: <green>APPLE_API_ISSUER</>, <green>APPLE_API_KEY</>, <green>APPLE_API_KEY_PATH</>
        2. Project config (strudel.toml):
        <magenta>
        [apple]
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"  # Issuer ID
        api_key      = "2X9R4HXF34"                             # Key ID
        api_key_path = "AuthKey_2X9R4HXF34.p8"                  # path to .p8 file
        </>
        3. Global config (~/.config/strudel/config.toml) is shared across all projects.
           api_key_path here is typically an absolute path:
        <magenta>
        [apple]
        api_key_path = "/Users/you/.private_keys/AuthKey_2X9R4HXF34.p8"
        </>
        Edit the global config: <blue>strudel config global edit</>
        See: <blue>strudel help global-config</>

        ## Key role

        A "Developer" role key is enough for macOS app notarization, whether locally or in CI.
        If you also use strudel's iOS auto-provisioning (<magenta>"app_store_connect"</>),
        use an "Admin" role key instead - device registration and profile management via the
        App Store Connect API require additional permissions.

        ## Timeout
        <magenta>
        [apple]
        notarize_timeout = 600   # seconds; default: 600
        </>
        Notarization typically completes in under a minute, but Apple's servers can
        occasionally be slow.

        ## Troubleshooting

        - "Invalid API key": confirm api_key_path points to the correct .p8 file and
          api_key matches the Key ID shown in App Store Connect.

        ## See also

        <blue>strudel help signing</>
        <blue>strudel help ci</>
    "#});
}

fn print_entitlements() {
    print_help(&cformatdoc! {r#"
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
               strudel devices add

          2. Run fetches the profile and caches it automatically:
               strudel run --device

        The profile is cached at .strudel/<<bundle_id>>.mobileprovision (gitignored). On every
        build strudel checks whether the cached profile is still current (not expired, includes
        all tracked devices); if not, it re-fetches automatically.

        To manage the profile manually instead, set:
        <magenta>
          [build]
          provisioning_profile = "path/to/MyApp.mobileprovision"
        </>
        When provisioning_profile is set strudel uses that file as-is and warns if it looks
        stale, but does not overwrite it. See <blue>strudel help ios-device</> for the full workflow.

        ## Ad-hoc + entitlements

        Ad-hoc signatures (no signing identity configured) won't work with entitlements
        that require a provisioning profile. strudel will warn when this is detected.

        ## Extensions

        Each extension gets its own entitlements file (required, extensions are sandboxed
        independently of the host app):

          [[extensions]]
          entitlements_json_path = "extension/entitlements.json"

        See: <blue>strudel help extensions</>
    "#});
}

fn print_extensions() {
    print_help(&cformatdoc! {r#"
        # App and system extensions

        App extensions (safari_web_extension, app_extension) are embedded as .appex bundles
        under Contents/PlugIns/ in the host app. System extensions (system_extension) are
        embedded as .systemextension bundles under Contents/Library/SystemExtensions/. Each
        extension is assembled and codesigned separately; notarizing the host .app covers
        all nested bundles.

        ## Common fields (all kinds)
        <magenta>
        [[extensions]]
        kind                   = "safari_web_extension"  # or "app_extension", "system_extension"
        target_name            = "MyExtension"                 # SPM executableTarget
        bundle_id              = "com.example.myapp.Extension"
        # name                 = "MyExtension"                 # defaults to target_name
        entitlements_json_path = "ext/entitlements.json"       # required
        # info_json_path       = "ext/info.json"               # optional extra Info.plist keys
        </>
        The SPM target must be an executableTarget in Package.swift.

        ## kind = "safari_web_extension"

        Embeds a Safari Web Extension. The resources_dir is copied wholesale into
        Contents/Resources/ (manifest.json, JS, HTML, icons, etc.).
        <magenta>
        [[extensions]]
        kind          = "safari_web_extension"
        target_name   = "MyAppExtension"
        bundle_id     = "com.example.myapp.Extension"
        resources_dir = "extension/dist"                  # required; webpack output dir
        # principal_class = "MyAppExtension.SafariWebExtensionHandler"  # default shown
        </>
        strudel auto-injects NSExtension with:
          NSExtensionPointIdentifier = "com.apple.Safari.web-extension"
          NSExtensionPrincipalClass  = <<principal_class>>
          SFSafariWebExtensionManifestPath = "Resources/manifest.json"

        ## kind = "app_extension"

        Generic macOS app extension (Share, Finder Sync, Notification Service, Quick Look, etc.)
        <magenta>
        [[extensions]]
        kind                       = "app_extension"
        target_name                = "MyShareExtension"
        bundle_id                  = "com.example.myapp.Share"
        entitlements_json_path     = "share/entitlements.json"
        extension_point_identifier = "com.apple.share-services"   # required
        # principal_class          = "MyShareExtension.ShareViewController"  # optional
        </>
        Common extension_point_identifier values:
          "com.apple.share-services"              Share Extension
          "com.apple.FinderSync"                  Finder Sync Extension
          "com.apple.usernotifications.service"   Notification Service Extension
          "com.apple.quicklook.preview"           Quick Look Preview Extension

        See https://developer.apple.com/documentation/bundleresources/information-property-list/nsextension/nsextensionpointidentifier

        ## kind = "system_extension"

        A macOS System Extension (Network Extension or Endpoint Security), embedded under
        Contents/Library/SystemExtensions/. Unlike app extensions, a system extension is not
        sandboxed alongside the host app: once installed it runs as its own long-lived,
        OS-managed process, activated at runtime by the host app via `SystemExtensions.framework` (`OSSystemExtensionRequest`). strudel only assembles and signs the bundle, it does not
        call that API for you.
        <magenta>
        [[extensions]]
        kind                  = "system_extension"
        target_name           = "MyNetworkExtension"
        bundle_id             = "com.example.myapp.NetworkExtension"
        entitlements_json_path = "netext/entitlements.json"
        system_extension_type = "network_extension"   # or "endpoint_security"
        # principal_class     = "MyNetworkExtension.FilterDataProvider"  # rarely needed
        </>
        system_extension_type values and their NSExtensionPointIdentifier:
          "network_extension"    com.apple.system_extension.network_extension
          "endpoint_security"    com.apple.system_extension.endpoint_security

        DriverKit drivers (.dext) are not supported: they use IOKitPersonalities instead
        of NSExtension and are outside this mechanism entirely.

        The host app's own entitlements (its top-level `entitlements_json_path`) need
        `com.apple.developer.system-extension.install`; the extension's entitlements
        need whatever the extension type requires (e.g.
        `com.apple.developer.networking.networkextension` for a Network Extension,
        `com.apple.developer.endpoint-security.client` for Endpoint Security).

        ## Auto-injected Info.plist keys

        All extensions get: CFBundleExecutable, CFBundleIdentifier, CFBundleName,
        CFBundleDisplayName, CFBundleVersion, CFBundleShortVersionString.
        CFBundlePackageType = "XPC!" for safari_web_extension/app_extension, "SYSX" for
        system_extension.

        User-supplied info_json_path provides additional keys; auto-injected ones win
        on conflict.

        ## Sign order

        Inside-out: embedded dylibs -> each extension bundle (.appex or .systemextension) ->
        host .app. `--deep` is not used on the host as it would apply host entitlements to
        nested bundles incorrectly.
    "#});
}

fn print_ios_device() {
    print_help(&cformatdoc! {r#"
        # iOS device builds

        ## Provisioning backends

        strudel supports two provisioning backends:

          "app_store_connect"  Requires a paid Apple Developer account and an App Store
                               Connect API key (Admin role). Produces 1-year profiles.
                               See "Credentials required" below.

          "free"               Sign in with any Apple ID (no paid account). Produces
                               7-day profiles; max 3 devices and 10 App IDs per team.
                               Run `strudel login` first, then the normal device workflow.

        Set the backend in strudel.toml (required):
        <magenta>
          [ios]
          provisioning = "app_store_connect"  # or "free"
        </>
        For the free path, see: <blue>strudel help ios-free-provisioning</>

        ## One-time setup (App Store Connect path)

        Register your device on the App Store Connect portal and track it locally:
        <blue>
          strudel devices add
        </>
        With a device connected and Developer Mode enabled, this registers it on the portal
        (if not already) and adds it to .strudel/devices.toml (gitignored). Repeat whenever
        you add a device.

        To add a specific subset of connected devices:
        <blue>
          strudel devices add --device "iPhone 15" --device "iPad Air"
        </>
        To register a device on the portal only, without a connection or local tracking
        (e.g. a teammate's device):
        <blue>
          strudel devices register --udid <<UDID>> --name "Their iPhone"
        </>
        ## Building and installing
        <blue>
          strudel run --device
        </>
        On first run, strudel calls the App Store Connect API to:
          1. Look up (or create) the bundle ID
          2. Find your development certificate(s)
          3. Create a development profile embedding all tracked devices
          4. Cache the profile at .strudel/<<bundle_id>>.mobileprovision

        On subsequent runs the cached profile is reused if it is still current. A profile
        is considered stale when it has expired (within 5 minutes), is missing a device
        UDID, or the application-identifier no longer matches. Stale profiles are silently
        re-fetched.

        To target specific devices for one build (all must be in devices.toml):
        <blue>
          strudel run --device "iPhone 15" --device "iPhone 16 Pro"
        </>
        ## Managing the profile manually

        To check the cached profile's status without building:
        <blue>
          strudel profile
        </>
        To fetch or force-refresh the cached profile without building:
        <blue>
          strudel profile fetch
          strudel profile fetch --force
        </>
        To opt out of auto-management and use your own profile, set in strudel.toml:
        <magenta>
          [build]
          provisioning_profile = "path/to/MyApp.mobileprovision"
        </>
        ## Credentials required (App Store Connect path only)

        Profile auto-fetch uses the same App Store Connect API key as notarization:
        <green>
          APPLE_API_KEY_PATH   path to your .p8 key file
          APPLE_API_KEY        key ID (shown in App Store Connect)
          APPLE_API_ISSUER     issuer ID (shown in App Store Connect)
        </>
        See <blue>strudel help notarize</> for how to configure these credentials.

        Note: registering devices and creating bundle IDs and profiles requires an API key
        with the <green>Admin</> role. A <green>Developer</> key
        is fine for notarization but will fail with "insufficient permissions" on
        `strudel devices add`, `strudel run --device`, and `strudel profile fetch`. Either use
        an Admin key or register the device and create the profile manually and set
        build.provisioning_profile.

        ## .strudel/devices.toml

        Tracked devices are stored in .strudel/devices.toml. This file is gitignored
        automatically by .strudel/.gitignore (written by strudel). Do not commit it.

        Example:
        <magenta>
          [[device]]
          name = "My iPhone"
          udid = "00008101-001234AB3456001E"
        </>
    "#});
}

fn print_dylibs() {
    print_help(&cformatdoc! {r#"
        # Embedding dynamic libraries and frameworks

        For C FFI dylibs, or .framework bundles (e.g. Sparkle, vendored as a SwiftPM
        binaryTarget) that must ship inside the app bundle:
        <magenta>
        [build]
        embed_libs = ["libFoo.dylib", "Sparkle.framework", "vendor/libBar.dylib"]
        </>
        strudel tells dylibs and frameworks apart by extension (.framework vs.
        everything else).

        ## Resolving entries

        Every entry resolves relative to the config file's directory unless absolute,
        same as any other path - including a bare name like `libFoo.dylib`. If the
        resolved location doesn't exist, strudel falls back to whichever
        `.build/<<triple>>/release/` directory it just built for this invocation.

        The fallback is what makes a bare name work for anything swift build produces
        or links per-platform: it stays correct across build destinations (e.g.
        switching between iOS simulator and device) without editing the list. A
        vendored dylib or framework that lives outside the build output and is the
        same for every triple just needs a real path (e.g. `vendor/libBar.dylib`) and
        the fallback never triggers.

        ## dylibs

        For each dylib, strudel:
          1. Copies it into Contents/Frameworks/
          2. Re-writes its install name to @rpath/libFoo.dylib
          3. Updates the executable's load command to use @rpath/libFoo.dylib
          4. Signs the dylib (before signing the outer bundle)

        ## .framework bundles

        For each framework, strudel:
          1. Copies the whole bundle into Contents/Frameworks/ (preserving the
             Versions/... symlink structure)
          2. Signs it with --deep (before signing the outer bundle), so any nested
             code (e.g. Sparkle's bundled Autoupdate.app / XPC services) carries your
             own Team ID instead of the vendor's

        No install-name rewrite is done for frameworks: SwiftPM binaryTargets are
        already linked with an @rpath install name.

        strudel injects -rpath @executable_path/../Frameworks at link time via
        <blue>-Xlinker -rpath -Xlinker @executable_path/../Frameworks</> in <blue>swift build</>
        whenever embed_libs is non-empty.

        ## Build-time flags

        Compile-time flags (-I, -L, -l, module maps) and linker flags still belong in
        Package.swift (cSettings / linkerSettings, or a binaryTarget declaration for a
        vendored .framework/.xcframework). strudel's embed_libs only handles the bundle
        assembly and signing step; it does not affect how <blue>swift build</> finds or links
        the library.

        ## Static libraries

        Static libraries (.a) are linked directly into the binary and do not need to be
        listed in embed_libs, nothing to embed or sign.
    "#});
}

fn print_copy() {
    print_help(&cformatdoc! {r#"
        # Copying arbitrary files into the bundle

        `[[build.copy]]` copies a file or directory, under its own file name, into a
        caller-chosen destination directory inside the bundle, distinct from
        `embed_libs` (Contents/Frameworks) and `resources`/`resources_dir`
        (Contents/Resources). Use it for things like a helper binary, a
        command-line tool, or any other file that needs to land somewhere else in
        the bundle:
        <magenta>
        [[build.copy]]
        src      = "path/to/helper"     # relative to the config file's directory
        dest_dir = "Contents/MacOS"     # relative to the bundle root
        sign     = true                  # codesign after copying; default: false
        entitlements_json_path = "helper-entitlements.json"  # optional, requires sign = true
        </>
        This copies `path/to/helper` to `Contents/MacOS/helper` - same file name,
        just relocated. `dest_dir` is created (including parent directories) if it
        doesn't already exist. Repeat `[[build.copy]]` for multiple entries.

        ## Signing

        `sign = true` codesigns the copied item before the outer bundle is sealed,
        the same way `embed_libs` entries are signed - required for any Mach-O
        executable or nested bundle placed outside Contents/Frameworks, since nested
        code must carry its own valid signature for `codesign --verify --deep
        --strict` and notarization to pass.

        strudel picks the codesign flags automatically based on `src`:
          - a directory (e.g. a nested helper `.app` or plugin bundle) is signed with
            `--deep`, since it may contain nested code of its own
          - a plain file (e.g. a flat helper binary) is signed directly, no `--deep`

        Files with `sign = false` (the default) are copied as-is and rely on the
        outer bundle's `codesign --deep` (if any) or are exempt from signing
        entirely (e.g. plain data files, scripts).

        `entitlements_json_path`, like extensions' entitlements, gives the copied
        item its own entitlements at signing time instead of inheriting the host
        app's - useful for a nested helper `.app` or plugin that needs a different
        sandbox/capability set. Resolved relative to the config file's directory.
        Ignored unless `sign = true`.
    "#});
}

fn print_universal() {
    print_help(&cformatdoc! {r#"
        # Universal binaries

        To produce a universal (fat) binary that runs natively on both Apple Silicon
        and Intel Macs:
        <magenta>
        [build]
        archs = ["arm64", "x86_64"]
        </>
        strudel passes --arch arm64 --arch x86_64 to <blue>swift build</>, which invokes the
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
    print_help(&cformatdoc! {r#"
        # Global config

        <blue>~/.config/strudel/config.toml</> stores machine-wide defaults shared across all
        projects on this machine. It is the lowest-priority source for each value:

          env var  >  strudel.toml  >  ~/.config/strudel/config.toml

        ## Editing
        <blue>
        strudel config global edit
        </>
        Opens the file in <green>$VISUAL</> / <green>$EDITOR</>, creating it with a template if it doesn't
        exist yet. The XDG_CONFIG_HOME env var overrides the default location.

        ## Supported keys
        <magenta>
        [apple]
        identity     = "Developer ID Application: Your Name (XXXXXXXXXX)"
        team_id      = "XXXXXXXXXX"
        api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        api_key      = "2X9R4HXF34"
        api_key_path = "/Users/you/.private_keys/AuthKey_2X9R4HXF34.p8"
        </>
        Only <magenta>[apple]</> is supported here. <magenta>[app]</>, <magenta>[build]</>, <magenta>[ios]</>, <magenta>[dmg]</>, and
        <magenta>[[extensions]]</> are project-specific and belong only in strudel.toml.

        ## Typical use

        Store your signing identity and notarize credentials once in the global
        config, and leave each project's strudel.toml clean of machine-specific
        paths. This keeps strudel.toml committable to version control without
        embedding your developer identity or API key paths.

        A project strudel.toml can still override any global value by setting
        the same key; the env var overrides both.

        ## See also

        <blue>strudel help signing</>
        <blue>strudel help notarize</>
    "#});
}

fn print_ci() {
    print_help(&cformatdoc! {r#"
        # CI/CD setup

        ## Required secrets

        Set these as CI environment secrets (never commit them):

          <green>APPLE_SIGNING_IDENTITY</>      "Developer ID Application: Your Name (XXXXXXXXXX)"
          <green>APPLE_TEAM_ID</>               10-character team ID
          <green>APPLE_CERTIFICATE</>           base64-encoded Developer ID .p12
          <green>APPLE_CERTIFICATE_PASSWORD</>  export password for the .p12

        For notarization (App Store Connect API key):

          <green>APPLE_API_ISSUER</>    issuer UUID from App Store Connect
          <green>APPLE_API_KEY</>       key ID (e.g. "2X9R4HXF34")
          <green>APPLE_API_KEY_PATH</>  path to the .p8 file (or set inline, see below)

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
                  strudel release --ci

        Pass <blue>--ci</> to trim the per-second notarization countdown, which
        otherwise spams captured CI logs with a line for every tick. Other output
        (steps, errors, submission IDs) is unaffected, so it's safe to leave on
        while debugging a CI run.

        ## Preparing APPLE_CERTIFICATE

        1. Open Keychain Access -> find your Developer ID Application certificate
        2. Right-click -> Export -> save as DeveloperID.p12, set an export password
        3. Encode: <blue>base64 -i DeveloperID.p12 | pbcopy</>
        4. Paste the result as the APPLE_CERTIFICATE secret

        ## Storing the .p8 API key in CI

        Option A: store the .p8 file contents as a secret (APPLE_API_KEY_CONTENTS),
        write it to disk before running strudel, set APPLE_API_KEY_PATH to the
        written path.

        Option B: commit the .p8 file to the repo (it is not a password, but treat it
        as sensitive). Set APPLE_API_KEY_PATH to its repo-relative path.

        ## See also

        <blue>strudel help signing</>
        <blue>strudel help notarize</>
    "#});
}

fn print_ios_free_provisioning() {
    print_help(&cformatdoc! {r#"
        # Free Apple ID provisioning

        strudel can provision iOS device builds using any Apple ID - no paid Apple
        Developer account required. This mirrors what Xcode does when you sign in with
        a plain Apple ID: 7-day development profiles, max 3 devices and 10 App IDs per
        Personal Team.

        ## Limits vs paid provisioning

          Free (Apple ID)          Paid (App Store Connect)
          ─────────────────────    ────────────────────────
          7-day profiles           1-year profiles
          Max 3 devices            Unlimited devices
          Max 10 App IDs           Unlimited App IDs
          No Admin API key needed  Requires Admin API key
          strudel login required   App Store Connect creds

        ## Setup

        1. Enable the free backend in strudel.toml:
        <magenta>
           [ios]
           provisioning = "free"
           apple_id     = "you@example.com"   # optional; pre-fills login prompt
        </>
        2. Sign in with your Apple ID:
        <blue>
           strudel login
        </>
           Prompts for your Apple ID, password, and a 2FA code if your account has
           two-factor authentication enabled. The session token (never the password)
           is saved to <green>~/.local/share/strudel/session.json</>.

        3. Register your device and build as usual:
        <blue>
           strudel devices add
           strudel run --device
        </>
        ## Session management
        <blue>
        strudel login                        # interactive sign-in
        strudel login --apple-id you@ex.com  # pre-fill the email
        strudel login clear                  # clear session and cached credentials
        strudel login status                 # show just the Apple ID session
        strudel status                       # show full config + session + project state
        </>
        The session token expires. If a `strudel run --device` run fails with an auth
        error, re-run `strudel login`.

        ## What strudel stores

        All data lives in <green>~/.local/share/strudel/</> (per-machine, not per-project):

          session.json             GSA token + DSID (no password stored)
          dev-cert.der             Cached DER-encoded developer certificate
          dev-key.pem              Cached private key (generated fresh each cert rotation)
          strudel-dev.keychain-db  Persistent keychain holding the signing identity

        The keypair and certificate are regenerated on each profile refresh (every
        7 days). The keychain is created once and reused across rotations.

        ## Profile refresh

        A fresh profile is fetched whenever the cached one is stale (expired or
        missing a device). `strudel run --device` does this automatically. To trigger
        manually:
        <blue>
        strudel profile fetch           # fetch if stale
        strudel profile fetch --force   # force-refresh
        </>
        Each refresh revokes the previous dev certificate for this machine (to stay
        within the 2-cert limit per team) and generates a new RSA keypair + CSR.

        ## Under the hood

        strudel implements the Apple developer-services provisioning protocol natively:
          - Authenticates via Apple's GrandSlam SRP-6a flow (SHA-256, custom pre-hash)
          - Generates anisette headers via <green>AOSKit.framework</> (no Docker required)
          - Calls <green>developerservices2.apple.com</> endpoints used by Xcode itself
          - Shells <green>openssl</> to generate the RSA keypair + CSR

        Known issue: <green>AOSKit.retrieveOTPHeadersForDSID:</> returned -45070 in early
        macOS 27 betas. If you see anisette errors, file an issue.

        ## See also

        <blue>strudel help ios-device</>
        <blue>strudel help entitlements</>
    "#});
}

fn print_ios_app() {
    print_help(&cformatdoc! {r#"
        # iOS app checklist

        Things that are easy to skip, don't fail the build, but are visibly wrong
        (or cause a rejection) at runtime.

        ## Info.plist data are set in info_json_path

        strudel generates Info.plist itself from the JSON file at <magenta>build.info_json_path</>.
        Note that the CFBundle* identity keys (identifier, name, version, build number)
        are always written by strudel and override anything in that file. With no
        info_json_path at all, the app ships with only strudel's defaults.
        See <blue>strudel help config</>.

        ## UILaunchScreen

        Required, or the app falls back to a letterboxed compatibility mode at
        launch instead of using the full screen. strudel does not inject a
        default, so a target with no info_json_path has no launch screen.
        Minimal working value:
        <magenta>
        {{ "UILaunchScreen": {{}} }}
        </>
        ## Permission usage strings

        Any API that needs user consent (camera, photos, microphone, location,
        contacts, etc.) needs its NSXxxUsageDescription key in info_json_path with
        a real, human-readable reason. Missing the key crashes the app on first
        access instead of showing a denial prompt.

        ## App icon

        With <magenta>[build.icon]</>, strudel renders the complete iPhone appiconset
        (every size/scale plus the 1024x1024 ios-marketing rendition) from your
        single source image. Use <blue>strudel icon</> to render that source image to
        a PNG (written to <blue>--out</>, default the current directory) to preview.

        ## Dark mode

        Verify custom colors are asset-catalog colors (or adapt via
        Color(uiColor:)/semantic colors), not hardcoded RGB, so the app doesn't
        look broken in dark mode.

        ## Dynamic Type

        Use .font(.body)-style semantic text styles rather than fixed point sizes
        for user-facing prose, so accessibility text-size settings take effect.

        ## Orientation / size classes

        strudel builds iPhone-only by default: UIDeviceFamily defaults to [1] and
        icons are compiled with --target-device iphone, so the generated appiconset
        carries no iPad renditions. For a universal app, set UIDeviceFamily
        yourself in info_json_path:
        <magenta>
        {{ "UIDeviceFamily": [1, 2] }}
        </>
        and supply a hand-authored <magenta>ios.assets_dir</> with ipad idiom icons, since
        [build.icon] won't generate them. Then check layout at iPad multitasking
        widths, e.g:
        <blue>
        strudel run --sim "iPad Pro 13-inch (M4)"
        </>

        ## See also

        <blue>strudel help config</>
        <blue>strudel help entitlements</>
        <blue>strudel help ios-device</>
    "#});
}

fn print_macos_app() {
    print_help(&cformatdoc! {r#"
        # macOS app checklist

        Things that don't fail the build but read as unpolished, or break
        sandboxed/notarized builds.

        ## Info.plist data are set in info_json_path

        strudel generates Info.plist itself from the JSON file at <magenta>build.info_json_path</>.
        It always writes CFBundleExecutable, CFBundleIdentifier, CFBundlePackageType
        and the version keys, overriding anything in that file.
        See <blue>strudel help config</>.

        ## App display name

        Unlike the iOS builder, this one writes no CFBundleName/CFBundleDisplayName,
        so the menu bar and About box fall back to the executable name. If the name
        users should see differs (spaces, capitalization), set it yourself:
        <magenta>
        {{ "CFBundleName": "My App" }}
        </>
        ## Settings/Preferences window

        Use SwiftUI's Settings {{ }} scene with a TabView shell: one Label per
        tab, a fixed .frame(width:height:), top-aligned content.
        <magenta>
        Settings {{
            TabView {{
                GeneralSettingsView()
                    .tabItem {{ Label("General", systemImage: "gearshape") }}
            }}
            .padding(20)
            .frame(width: 400, height: 300, alignment: .top)
            .navigationTitle("My App")
        }}
        </>
        Persist values with @AppStorage directly in the views, or a shared
        wrapper class reading the same UserDefaults keys for code that isn't a
        SwiftUI view (e.g. a background service reading preferences at runtime).

        ## Menu bar icon

        A MenuBarExtra label built from Image(nsImage:) needs isTemplate = true
        set on the underlying NSImage (plus manual resizing to ~18pt height,
        preserving aspect ratio) or it won't tint correctly for light/dark mode
        and click-highlight the way native status-item icons do.

        ## Menu-bar-only apps opening a window

        With LSUIElement set, opening any window (Settings, About, a document)
        needs an explicit switch to .regular activation policy first, or the
        window can open behind other apps or not come to front at all. Switching
        alone isn't enough: the policy change needs a beat to take effect, so
        give it ~100ms before opening, or the window still opens behind.

        ## App Sandbox entitlements

        Decide sandboxing up front; retrofitting it onto an app that assumed
        unrestricted file access is the most common late-stage rework. See
        <blue>strudel help entitlements</>.

        ## See also

        <blue>strudel help entitlements</>
        <blue>strudel help signing</>
        <blue>strudel help notarize</>
    "#});
}
