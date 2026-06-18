use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{Result, bail};

use crate::config::generate_initial_toml;

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
    let build_number = prompt("Build number", Some("1"))?;

    let content = generate_initial_toml(&app_name, &bundle_id, &version, &build_number);
    std::fs::create_dir_all(output_dir)?;
    std::fs::write(&out_path, &content)?;
    println!("\nCreated {}", out_path.display());

    let pkg_path = output_dir.join("Package.swift");
    if pkg_path.exists() {
        println!("Skipped {} (already exists)", pkg_path.display());
    } else {
        let pkg_content = generate_package_swift(&app_name);
        std::fs::write(&pkg_path, &pkg_content)?;
        println!("Created {}", pkg_path.display());
    }

    let gitignore_path = output_dir.join(".gitignore");
    if gitignore_path.exists() {
        println!("Skipped {} (already exists)", gitignore_path.display());
    } else {
        std::fs::write(&gitignore_path, indoc::indoc! {"
            .build/    # Swift package manager build artifacts
            .strudel/  # strudel cache and intermediate build outputs
        "})?;
        println!("Created {}", gitignore_path.display());
    }

    println!("\nNext steps:");
    println!("  strudel bundle   # build app bundle (unsigned)");
    println!("  strudel build    # build + sign for local dev (ad-hoc if no identity)");
    println!("  strudel release  # full release (sign, notarize, DMG)");

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
        let dir = std::env::temp_dir().join(format!("strudel-init-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("strudel.toml"), "existing").unwrap();
        let err = run_init(&dir).expect_err("must refuse to overwrite");
        assert!(err.to_string().contains("already exists"), "got: {err}",);
        std::fs::remove_dir_all(&dir).ok();
    }
}
