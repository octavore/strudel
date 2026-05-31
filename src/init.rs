use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{Result, bail};

fn prompt(question: &str, default: Option<&str>) -> Result<String> {
    match default {
        Some(d) => print!("{} ({}): ", question, d),
        None => print!("{}: ", question),
    }
    io::stdout().flush()?;

    let line = io::stdin()
        .lock()
        .lines()
        .next()
        .unwrap_or(Ok(String::new()))?;
    let trimmed = line.trim().to_string();

    Ok(if trimmed.is_empty() {
        default.unwrap_or("").to_string()
    } else {
        trimmed
    })
}

fn generate_toml(app_name: &str, bundle_id: &str, version: &str, build_number: &str) -> String {
    indoc::formatdoc! {r#"
        # strudel.toml — strudel build configuration
        #
        # Commands:
        #   strudel build    — build app bundle only (no signing/notarization)
        #   strudel sign     — build and sign the app bundle (no notarization/DMG); local dev
        #   strudel release  — full release: build, sign, notarize, and package DMG
        #
        # Signing & notarization (required for `release`). Identifiers may go here or in the
        # environment; secrets are read from the environment ONLY.
        #
        # Identifiers (here or env): APPLE_SIGNING_IDENTITY, APPLE_TEAM_ID, APPLE_ID,
        #   APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH
        # Secrets (env only): APPLE_PASSWORD, APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD
        #
        # Notarization uses the App Store Connect API key ([notarize] api_*) when fully
        # set, otherwise falls back to Apple ID (apple_id + APPLE_PASSWORD + team_id).

        [app]
        name         = "{app_name}"
        bundle_id    = "{bundle_id}"
        version      = "{version}"
        build_number = "{build_number}"

        # Paths are relative to this file's directory unless absolute.
        # Uncomment and edit to override.
        [build]
        # source_dir             = "."                  # Swift package directory
        # build_dir              = ".build/dist"        # artifacts (relative to source_dir)
        # info_json_path         = "info.json"          # optional; empty object if unset
        # entitlements_json_path = "entitlements.json"
        # icon_path              = "Sources/App/Assets.xcassets/AppIcon.appiconset/AppIcon.icns"  # optional; no icon if unset
        # archs                  = ["arm64", "x86_64"]  # default: host arch only
        # target_name            = "{app_name}"         # Swift target, if it differs from the app name

        # Dynamic C FFI libraries to embed in Contents/Frameworks and sign.
        # Paths are relative to this file's directory unless absolute.
        # Build-time flags (-I, -L, -l, rpath, modulemap) belong in Package.swift
        # (cSettings / linkerSettings); static libs need nothing here.
        # embed_libs             = ["path/to/libFoo.dylib"]

        # Provisioning profile embedded as Contents/embedded.provisionprofile.
        # Required for certain entitlements (e.g. push notifications, iCloud).
        # provisioning_profile   = "{app_name}.provisionprofile"

        # Extra environment variables forwarded to `swift build` (e.g. for
        # pkg-config or library discovery). Values are passed through verbatim.
        # [build_env]
        # PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig"

        # Signing identifiers — or set via APPLE_SIGNING_IDENTITY / APPLE_TEAM_ID.
        [signing]
        # identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        # team_id  = "XXXXXXXXXX"

        # Notarization identifiers — or set via APPLE_ID / APPLE_API_*.
        [notarize]
        # apple_id     = "you@example.com"
        # api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        # api_key      = "2X9R4HXF34"
        # api_key_path = "AuthKey_2X9R4HXF34.p8"
        # timeout      = 600
    "#}
}

pub fn run_init(output_dir: &Path) -> Result<()> {
    let out_path = output_dir.join("strudel.toml");
    if out_path.exists() {
        bail!(
            "{} already exists. Remove it first or choose a different directory.",
            out_path.display()
        );
    }

    println!("Initializing strudel build config...\n");

    let app_name = prompt("App name", Some("MyApp"))?;
    let default_id = format!("com.example.{}", app_name.to_lowercase());
    let bundle_id = prompt("Bundle ID", Some(&default_id))?;
    let version = prompt("Version", Some("1.0.0"))?;
    let build_number = prompt("Build number", Some("1"))?;

    let content = generate_toml(&app_name, &bundle_id, &version, &build_number);
    std::fs::create_dir_all(output_dir)?;
    std::fs::write(&out_path, &content)?;

    println!("\nCreated {}", out_path.display());
    println!("\nNext steps:");
    println!("  strudel build    # build app bundle");
    println!("  strudel sign     # build + sign for local dev (ad-hoc if no identity)");
    println!("  strudel release  # full release (sign, notarize, DMG)");

    Ok(())
}
