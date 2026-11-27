//! Edits of strudel.toml that keep comments and formatting.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use toml_edit::{Array, DocumentMut, Item, Table, TableLike, value};

use crate::config::Platform;

/// Add `capability` to the `capabilities` list of the target `platform` and
/// `app_name` in the config at `path`. Does nothing if the list has it.
pub fn add_capability(
    path: &Path,
    platform: Platform,
    app_name: &str,
    capability: &str,
) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    add_capability_to_doc(&mut doc, platform, app_name, capability)?;
    fs::write(path, doc.to_string()).with_context(|| format!("writing {}", path.display()))
}

fn add_capability_to_doc(
    doc: &mut DocumentMut,
    platform: Platform,
    app_name: &str,
    capability: &str,
) -> Result<()> {
    if !doc.contains_key("target") {
        return add_to_list(doc.as_table_mut(), "capabilities", capability);
    }

    let targets = doc
        .get_mut("target")
        .and_then(Item::as_array_of_tables_mut)
        .context("target is not an array of tables")?;
    let Some(target) = targets
        .iter_mut()
        .find(|t| is_target(t, platform, app_name))
    else {
        bail!(
            "No {} target named {app_name} in the config.",
            platform.as_str()
        );
    };
    add_to_list(target, "capabilities", capability)
}

fn is_target(target: &Table, platform: Platform, app_name: &str) -> bool {
    let platform_matches = target.get("platform").and_then(Item::as_str) == Some(platform.as_str());
    let name = target
        .get("app")
        .and_then(|app| app.get("name"))
        .and_then(Item::as_str);
    platform_matches && name == Some(app_name)
}

/// Add `capability` to the list at `key`. A missing list is created.
fn add_to_list(table: &mut dyn TableLike, key: &str, capability: &str) -> Result<()> {
    if let Some(item) = table.get_mut(key) {
        let list = item
            .as_array_mut()
            .with_context(|| format!("`{key}` is not a list"))?;
        let present = list.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(capability))
        });
        if !present {
            list.push(capability);
        }
        return Ok(());
    }
    let mut list = Array::new();
    list.push(capability);
    table.insert(key, value(list));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str, platform: Platform, app_name: &str) -> Result<String> {
        let mut doc: DocumentMut = text.parse().unwrap();
        add_capability_to_doc(&mut doc, platform, app_name, "iCloud")?;
        Ok(doc.to_string())
    }

    #[test]
    fn single_target_gets_a_top_level_list() {
        let text = "# Config\n[app]\nname = \"App\"\n";
        let got = edit(text, Platform::Macos, "App").unwrap();
        let doc: DocumentMut = got.parse().unwrap();
        assert_eq!(doc["capabilities"][0].as_str(), Some("iCloud"));
        assert!(got.contains("# Config\n[app]"), "{got}");
    }

    #[test]
    fn existing_list_keeps_its_entries_and_comments() {
        let text = "capabilities = [\"HealthKit\"] # needed\n[app]\nname = \"App\"\n";
        let got = edit(text, Platform::Macos, "App").unwrap();
        assert!(
            got.contains("\"HealthKit\", \"iCloud\"") && got.contains("# needed"),
            "{got}"
        );
    }

    #[test]
    fn present_capability_is_not_added_twice() {
        let text = "capabilities = [\"icloud\"]\n[app]\nname = \"App\"\n";
        let got = edit(text, Platform::Macos, "App").unwrap();
        assert_eq!(got, text);
    }

    #[test]
    fn multi_target_edits_only_the_named_target() {
        let text = "\
[[target]]
platform = \"macos\"
[target.app]
name = \"One\"

[[target]]
platform = \"macos\"
[target.app]
name = \"Two\"
";
        let got = edit(text, Platform::Macos, "Two").unwrap();
        let doc: DocumentMut = got.parse().unwrap();
        let targets = doc["target"].as_array_of_tables().unwrap();
        assert!(targets.get(0).unwrap().get("capabilities").is_none());
        assert_eq!(
            targets.get(1).unwrap()["capabilities"][0].as_str(),
            Some("iCloud")
        );
    }

    #[test]
    fn ios_target_gets_a_target_list() {
        let text = "\
[[target]]
platform = \"ios\"
[target.app]
name = \"App\"
";
        let got = edit(text, Platform::Ios, "App").unwrap();
        let doc: DocumentMut = got.parse().unwrap();
        assert_eq!(doc["target"][0]["capabilities"][0].as_str(), Some("iCloud"));
        assert!(doc["target"][0].get("ios").is_none());
    }

    #[test]
    fn unknown_target_is_an_error() {
        let text = "[[target]]\nplatform = \"macos\"\n[target.app]\nname = \"App\"\n";
        assert!(edit(text, Platform::Ios, "App").is_err());
        assert!(edit(text, Platform::Macos, "Other").is_err());
    }
}
