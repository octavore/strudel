use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use clml::cprintln;
use toml_edit::{DocumentMut, Item, Table, value};

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Component {
    Major,
    Minor,
    Patch,
    Build,
}

struct Change {
    name: Option<String>,
    old_version: String,
    old_build: String,
    new_version: String,
    new_build: String,
}

/// `strudel config version`: print the current `app.version` /
/// `app.build_number` for every target, without modifying the file. Lines
/// are labeled with the target id when targets disagree (or `force_labels`
/// is set); otherwise a single unlabeled line is printed since every target
/// shares the same version.
pub(crate) fn show(config: &Path, force_labels: bool) -> Result<()> {
    let project = crate::config::load_config(config)?;
    let differs = project
        .targets
        .windows(2)
        .any(|w| w[0].version != w[1].version || w[0].build_number != w[1].build_number);

    if force_labels || differs {
        for target in &project.targets {
            cprintln!(
                "<bold>{}</bold>: {} ({})",
                target.target_id,
                target.version,
                target.build_number
            );
        }
    } else if let Some(target) = project.targets.first() {
        cprintln!("{} ({})", target.version, target.build_number);
    }
    Ok(())
}

/// `strudel config increment-version`: bump `app.version` or
/// `app.build_number` in `strudel.toml`, after confirming the change.
pub(crate) fn increment(config: &Path, component: Component) -> Result<()> {
    let text =
        fs::read_to_string(config).with_context(|| format!("reading {}", config.display()))?;
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", config.display()))?;

    let mut changes = Vec::new();
    for app in app_tables_mut(&mut doc)? {
        changes.push(bump_app_table(app, component)?);
    }

    for c in &changes {
        let label = c.name.as_deref().unwrap_or("app");
        cprintln!(
            "<bold>{label}</bold>: {} ({}) -> <green>{} ({})</green>",
            c.old_version,
            c.old_build,
            c.new_version,
            c.new_build
        );
    }

    let confirmed = inquire::Confirm::new(&format!("Write this to {}?", config.display()))
        .with_default(false)
        .prompt()
        .context("reading confirmation")?;
    if !confirmed {
        cprintln!("<dim>Aborted; nothing was written.</dim>");
        return Ok(());
    }

    fs::write(config, doc.to_string()).with_context(|| format!("writing {}", config.display()))?;
    Ok(())
}

/// Returns the `[app]` table for a single-target config, or one per
/// `[[target]]` entry for a multi-target config.
fn app_tables_mut(doc: &mut DocumentMut) -> Result<Vec<&mut Table>> {
    if doc.contains_key("target") {
        let targets = doc
            .get_mut("target")
            .and_then(Item::as_array_of_tables_mut)
            .context("target is not an array of tables")?;
        targets
            .iter_mut()
            .map(|target| {
                target
                    .get_mut("app")
                    .and_then(Item::as_table_mut)
                    .context("[[target]] entry is missing an [app] table")
            })
            .collect()
    } else {
        let app = doc
            .get_mut("app")
            .and_then(Item::as_table_mut)
            .context("config is missing an [app] table")?;
        Ok(vec![app])
    }
}

fn bump_app_table(app: &mut Table, component: Component) -> Result<Change> {
    let name = app.get("name").and_then(Item::as_str).map(str::to_string);
    let old_version = app
        .get("version")
        .and_then(Item::as_str)
        .context("app.version is missing or not a string")?
        .to_string();
    let old_build = app
        .get("build_number")
        .and_then(Item::as_str)
        .unwrap_or("1")
        .to_string();

    let (new_version, new_build) = match component {
        Component::Major | Component::Minor | Component::Patch => {
            (bump_version(&old_version, component)?, old_build.clone())
        },
        Component::Build => (old_version.clone(), bump_build_number(&old_build)?),
    };

    if new_version != old_version {
        app["version"] = value(new_version.clone());
    }
    if new_build != old_build {
        app["build_number"] = value(new_build.clone());
    }

    Ok(Change {
        name,
        old_version,
        old_build,
        new_version,
        new_build,
    })
}

fn bump_version(version: &str, component: Component) -> Result<String> {
    let parts: Vec<&str> = version.split('.').collect();
    let [major, minor, patch] = parts[..] else {
        bail!("version {version:?} is not in x.y.z form");
    };
    let major: u64 = major
        .parse()
        .with_context(|| format!("version {version:?} is not in x.y.z form"))?;
    let minor: u64 = minor
        .parse()
        .with_context(|| format!("version {version:?} is not in x.y.z form"))?;
    let patch: u64 = patch
        .parse()
        .with_context(|| format!("version {version:?} is not in x.y.z form"))?;

    Ok(match component {
        Component::Major => format!("{}.0.0", major + 1),
        Component::Minor => format!("{major}.{}.0", minor + 1),
        Component::Patch => format!("{major}.{minor}.{}", patch + 1),
        Component::Build => unreachable!("bump_version is only called for major/minor/patch"),
    })
}

fn bump_build_number(build_number: &str) -> Result<String> {
    let n: u64 = build_number
        .parse()
        .with_context(|| format!("build_number {build_number:?} is not a plain integer"))?;
    Ok((n + 1).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_major_minor_patch() {
        assert_eq!(bump_version("1.2.3", Component::Major).unwrap(), "2.0.0");
        assert_eq!(bump_version("1.2.3", Component::Minor).unwrap(), "1.3.0");
        assert_eq!(bump_version("1.2.3", Component::Patch).unwrap(), "1.2.4");
    }

    #[test]
    fn rejects_non_semver_version() {
        assert!(bump_version("1.2", Component::Patch).is_err());
        assert!(bump_version("1.2.3-beta.1", Component::Patch).is_err());
    }

    #[test]
    fn bumps_build_number() {
        assert_eq!(bump_build_number("1").unwrap(), "2");
        assert_eq!(bump_build_number("41").unwrap(), "42");
    }

    #[test]
    fn rejects_non_numeric_build_number() {
        assert!(bump_build_number("1.0").is_err());
        assert!(bump_build_number("abc").is_err());
    }

    #[test]
    fn single_target_round_trip_preserves_comments() {
        let toml = indoc::indoc! {r#"
            # a comment worth keeping
            [app]
            name = "MyApp"
            version = "1.2.3"
            build_number = "41" # trailing comment
        "#};
        let mut doc: DocumentMut = toml.parse().unwrap();
        for app in app_tables_mut(&mut doc).unwrap() {
            bump_app_table(app, Component::Patch).unwrap();
        }
        let out = doc.to_string();
        assert!(out.contains("# a comment worth keeping"));
        assert!(out.contains("version = \"1.2.4\""));
        assert!(out.contains("build_number = \"41\" # trailing comment"));
    }

    #[test]
    fn multi_target_bumps_all_targets() {
        let toml = indoc::indoc! {r#"
            [[target]]
            app.name = "One"
            app.bundle_id = "com.example.one"
            app.version = "1.0.0"
            app.build_number = "1"
            platform = "macos"

            [[target]]
            app.name = "Two"
            app.bundle_id = "com.example.two"
            app.version = "2.5.0"
            app.build_number = "9"
            platform = "ios"
        "#};
        let mut doc: DocumentMut = toml.parse().unwrap();
        let changes: Vec<_> = app_tables_mut(&mut doc)
            .unwrap()
            .into_iter()
            .map(|app| bump_app_table(app, Component::Build).unwrap())
            .collect();
        assert_eq!(changes[0].new_build, "2");
        assert_eq!(changes[1].new_build, "10");
        let out = doc.to_string();
        assert!(out.contains("app.build_number = \"2\""));
        assert!(out.contains("app.build_number = \"10\""));
    }
}
