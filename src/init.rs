use anyhow::{Result, bail};
use std::io::{self, BufRead, Write};
use std::path::Path;

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
    format!(
        r#"# strudel.toml — strudel build configuration
#
# Commands:
#   strudel bundle   — build app bundle only (no signing/notarization)
#   strudel run      — full build: bundle, sign, notarize, and package DMG
#
# Signing credentials are read from environment variables (required for `run`):
#   TEAM_ID        — 10-character Apple Developer Team ID
#   SIGN_IDENTITY  — e.g. "Developer ID Application: Your Name (XXXXXXXXXX)"
#   APPLE_ID       — Your Apple ID email address
#   APPLE_PASSWORD — App-specific password

app_name     = "{app_name}"
bundle_id    = "{bundle_id}"
version      = "{version}"
build_number = "{build_number}"

# Paths are relative to this file's directory unless absolute.
# Uncomment and edit to override.
# source_dir             = "."                  # Swift package directory
# build_dir              = ".build/dist"        # artifacts (relative to source_dir)
# info_json_path         = "info.json"          # optional; empty object if unset
# entitlements_json_path = "entitlements.json"
# icon_path              = "Sources/App/Assets.xcassets/AppIcon.appiconset/AppIcon.icns"  # optional; no icon if unset
# archs                  = ["arm64", "x86_64"]  # default: host arch only
# target_name            = "{app_name}"         # Swift target, if it differs from app_name
"#,
        app_name = app_name,
        bundle_id = bundle_id,
        version = version,
        build_number = build_number,
    )
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
    println!("  strudel bundle   # build app bundle");
    println!("  strudel run      # full build (sign, notarize, DMG)");

    Ok(())
}
