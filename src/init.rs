use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{Result, bail};
use clml::cprintln;

use crate::apple::provisioning;
use crate::config::{
    IosProvisioningBackend, generate_initial_toml, generate_initial_toml_with_ios,
};
use crate::paths::StrudelData;

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

fn generate_package_swift(app_name: &str) -> String {
    indoc::formatdoc! {r#"
        // swift-tools-version: 6.0
        import PackageDescription

        let package = Package(
            name: "{app_name}",
            platforms: [.macOS(.v14)],
            targets: [
                .executableTarget(
                    name: "{app_name}",
                    path: "Sources/{app_name}"
                ),
            ]
        )
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
    let version = prompt("Version", Some("0.1.0"))?;

    let platforms = inquire::Select::new("Platforms:", vec!["macOS", "iOS", "both"]).prompt()?;
    let include_macos = platforms != "iOS";
    let include_ios = platforms != "macOS";

    let content = if include_ios {
        let provisioning = inquire::Select::new(
            "iOS provisioning:",
            vec![
                "app_store_connect (1-year profiles; requires paid Apple developer membership and App Store Connect API key)",
                "free (7-day profiles; any Apple ID)",
            ],
        )
        .prompt()?;
        let provisioning = if provisioning.starts_with("free") {
            IosProvisioningBackend::Free
        } else {
            IosProvisioningBackend::AppStoreConnect
        };

        let already_signed_in = StrudelData::locate()
            .map(|data| data.session_json.exists())
            .unwrap_or(false);
        if matches!(provisioning, IosProvisioningBackend::Free) && !already_signed_in {
            let sign_in_now = inquire::Confirm::new("Sign in with your Apple ID now?")
                .with_default(true)
                .with_help_message("You can also run `strudel login` later")
                .prompt()?;
            if sign_in_now && let Err(e) = provisioning::login(None) {
                eprintln!("Sign-in failed: {e:#}\nYou can run `strudel login` later.");
            }
        }

        generate_initial_toml_with_ios(&app_name, &bundle_id, &version, include_macos, provisioning)
    } else {
        generate_initial_toml(&app_name, &bundle_id, &version)
    };
    std::fs::create_dir_all(output_dir)?;
    std::fs::write(&out_path, &content)?;
    cprintln!("\nCreated <dim>{}</dim>", out_path.display());

    let pkg_path = output_dir.join("Package.swift");
    if pkg_path.exists() {
        cprintln!("Skipped <dim>{}</dim> (already exists)", pkg_path.display());
    } else {
        let pkg_content = generate_package_swift(&app_name);
        std::fs::write(&pkg_path, &pkg_content)?;
        cprintln!("Created <dim>{}</dim>", pkg_path.display());
    }

    let gitignore_path = output_dir.join(".gitignore");
    if gitignore_path.exists() {
        cprintln!(
            "Skipped <dim>{}</dim> (already exists)",
            gitignore_path.display()
        );
    } else {
        std::fs::write(
            &gitignore_path,
            indoc::indoc! {"
            # Swift package manager build artifacts
            .build/
            # strudel cache and intermediate build outputs
            .strudel/
        "},
        )?;
        cprintln!("Created <dim>{}</dim>", gitignore_path.display());
    }

    println!("\nNext steps:");
    if include_macos {
        cprintln!("  <blue>strudel build</blue>          # build + sign (ad-hoc if no identity)");
        cprintln!("  <blue>strudel run</blue>            # build + sign + open");
        cprintln!("  <blue>strudel release</blue>        # full release (sign, notarize, DMG)");
    }
    if include_ios {
        cprintln!("  <blue>strudel run --sim</blue>      # build and run in the iOS Simulator");
        cprintln!(
            "  <blue>strudel devices add</blue>    # register a device and fetch a provisioning profile"
        );
        cprintln!("  <blue>strudel run --device</blue>   # build, install, and launch on a device");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_swift_contains_app_name() {
        let pkg = generate_package_swift("Foo");
        assert!(pkg.contains("swift-tools-version: 6.0"));
        assert!(pkg.contains("name: \"Foo\""));
        assert!(pkg.contains("name: \"Foo\",\n            path: \"Sources/Foo\""));
    }

    #[test]
    fn run_init_refuses_to_overwrite() {
        // TempDir cleans up on drop, including when an assertion below panics.
        let dir = tempfile::TempDir::new().unwrap();
        let config = dir.path().join("strudel.toml");
        std::fs::write(&config, "existing").unwrap();

        let err = run_init(dir.path()).expect_err("must refuse to overwrite");
        assert!(err.to_string().contains("already exists"), "got: {err}");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "existing",
            "the refusal must leave the existing file untouched"
        );
    }
}
