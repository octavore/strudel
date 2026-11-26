use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clml::cprintln;
use serde::{Deserialize, Serialize};

use crate::config::Platform;

/// Device registered on the portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalEntry {
    pub name: String,
    pub udid: String,
}

/// Print a heading and the portal devices in `portal` that `is_known` does
/// not accept. Prints nothing if every device is known.
pub fn print_untracked(portal: &[PortalEntry], is_known: impl Fn(&str) -> bool) {
    let untracked: Vec<&PortalEntry> = portal.iter().filter(|d| !is_known(&d.udid)).collect();
    if untracked.is_empty() {
        return;
    }
    println!();
    cprintln!(
        "<bold>Registered on the portal, not tracked:</bold> <dim>(track one with `strudel \
         devices add --udid <<udid>> --name <<name>>`)</dim>"
    );
    for d in untracked {
        cprintln!("  {} <cyan>[{}]</cyan>", d.name, d.udid);
    }
}

/// Print whether a device is registered on the current account's portal.
pub fn print_registration(registered: Option<bool>) {
    match registered {
        Some(true) => cprintln!("  <green>registered</green> <dim>on this account</dim>"),
        Some(false) => cprintln!(
            "  <yellow>not registered</yellow> <dim>on this account, run `strudel devices add`</dim>"
        ),
        None => cprintln!("  <dim>registration unknown</dim>"),
    }
}

/// Ask before tracking `devices` (name and UDID), and registering them on the
/// portal if `register` is set. A registered device counts against the
/// portal's yearly device limit. Fails if the answer is no or there is no
/// terminal.
pub fn confirm_add(devices: &[(&str, &str)], platform: Platform, register: bool) -> Result<()> {
    if register {
        cprintln!(
            "This registers the following {} device(s) on App Store Connect, if not already \
             registered, and tracks them in .strudel/devices.toml:",
            platform.label()
        );
    } else {
        cprintln!(
            "This tracks the following {} device(s) in .strudel/devices.toml without \
             registering them on App Store Connect:",
            platform.label()
        );
    }
    for (name, udid) in devices {
        cprintln!("  <bold>{name}</bold> <cyan>[{udid}]</cyan>");
    }
    let confirmed = inquire::Confirm::new("Add these devices?")
        .with_default(false)
        .prompt()
        .context("Adding devices needs interactive confirmation and a terminal")?;
    if !confirmed {
        bail!("Aborted: no devices were added.");
    }
    Ok(())
}

fn default_platform() -> Platform {
    Platform::Ios
}

fn is_ios(platform: &Platform) -> bool {
    *platform == Platform::Ios
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedDevice {
    pub name: String,
    pub udid: String,
    /// Platform the device is registered under on the portal. Entries
    /// without a platform are iOS devices.
    #[serde(default = "default_platform", skip_serializing_if = "is_ios")]
    pub platform: Platform,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DeviceSet {
    #[serde(default)]
    pub device: Vec<TrackedDevice>,
}

impl DeviceSet {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = fs::read_to_string(path)?;
        Ok(toml::from_str(&s)?)
    }

    /// Load the devices tracked for `platform`.
    pub fn load_for(path: &Path, platform: Platform) -> Result<Self> {
        Ok(Self::load(path)?.only(platform))
    }

    /// Keep the devices tracked for `platform`.
    pub fn only(mut self, platform: Platform) -> Self {
        self.device.retain(|d| d.platform == platform);
        self
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Add or rename an iOS device.
    pub fn upsert(&mut self, name: String, udid: String) {
        self.upsert_on(name, udid, Platform::Ios);
    }

    /// Add or rename a device. A known UDID is updated in place, including
    /// its platform.
    pub fn upsert_on(&mut self, name: String, udid: String, platform: Platform) {
        if let Some(d) = self.device.iter_mut().find(|d| d.udid == udid) {
            d.name = name;
            d.platform = platform;
        } else {
            self.device.push(TrackedDevice {
                name,
                udid,
                platform,
            });
        }
    }

    pub fn contains_udid(&self, udid: &str) -> bool {
        self.device.iter().any(|d| d.udid == udid)
    }

    pub fn udids(&self) -> Vec<&str> {
        self.device.iter().map(|d| d.udid.as_str()).collect()
    }

    /// Find a tracked device by UDID or name. Returns the UDID.
    pub fn resolve(&self, selector: &str) -> Option<&str> {
        self.device
            .iter()
            .find(|d| d.udid == selector || d.name == selector)
            .map(|d| d.udid.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_adds_new_device() {
        let mut set = DeviceSet::default();
        set.upsert("iPhone".into(), "UDID-1".into());
        assert_eq!(set.device.len(), 1);
        assert_eq!(set.device[0].udid, "UDID-1");
    }

    #[test]
    fn upsert_updates_existing_by_udid() {
        let mut set = DeviceSet::default();
        set.upsert("Old Name".into(), "UDID-1".into());
        set.upsert("New Name".into(), "UDID-1".into());
        assert_eq!(set.device.len(), 1);
        assert_eq!(set.device[0].name, "New Name");
    }

    #[test]
    fn resolve_by_udid_and_name() {
        let mut set = DeviceSet::default();
        set.upsert("My iPhone".into(), "UDID-123".into());
        assert_eq!(set.resolve("UDID-123"), Some("UDID-123"));
        assert_eq!(set.resolve("My iPhone"), Some("UDID-123"));
        assert_eq!(set.resolve("Unknown"), None);
    }

    #[test]
    fn roundtrip_toml() {
        let mut set = DeviceSet::default();
        set.upsert("iPhone A".into(), "AAA".into());
        set.upsert("iPhone B".into(), "BBB".into());
        let s = toml::to_string_pretty(&set).unwrap();
        let loaded: DeviceSet = toml::from_str(&s).unwrap();
        assert_eq!(loaded.device.len(), 2);
        assert_eq!(loaded.device[0].udid, "AAA");
    }

    #[test]
    fn entries_without_a_platform_are_ios() {
        let set: DeviceSet =
            toml::from_str("[[device]]\nname = \"iPhone\"\nudid = \"AAA\"\n").unwrap();
        assert_eq!(set.device[0].platform, Platform::Ios);
    }

    #[test]
    fn only_keeps_devices_of_one_platform() {
        let mut set = DeviceSet::default();
        set.upsert("iPhone".into(), "AAA".into());
        set.upsert_on("Mac".into(), "BBB".into(), Platform::Macos);
        assert_eq!(set.only(Platform::Macos).udids(), ["BBB"]);
    }

    #[test]
    fn macos_platform_roundtrips_and_ios_is_omitted() {
        let mut set = DeviceSet::default();
        set.upsert("iPhone".into(), "AAA".into());
        set.upsert_on("Mac".into(), "BBB".into(), Platform::Macos);
        let s = toml::to_string_pretty(&set).unwrap();
        assert_eq!(s.matches("platform").count(), 1, "{s}");
        let loaded: DeviceSet = toml::from_str(&s).unwrap();
        assert_eq!(loaded.device[0].platform, Platform::Ios);
        assert_eq!(loaded.device[1].platform, Platform::Macos);
    }

    #[test]
    fn upsert_on_moves_a_known_udid_to_the_new_platform() {
        let mut set = DeviceSet::default();
        set.upsert("Old".into(), "AAA".into());
        set.upsert_on("New".into(), "AAA".into(), Platform::Macos);
        assert_eq!(set.device.len(), 1);
        assert_eq!(set.device[0].platform, Platform::Macos);
        assert_eq!(set.device[0].name, "New");
    }
}
