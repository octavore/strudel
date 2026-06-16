use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedDevice {
    pub name: String,
    pub udid: String,
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

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn upsert(&mut self, name: String, udid: String) {
        if let Some(d) = self.device.iter_mut().find(|d| d.udid == udid) {
            d.name = name;
        } else {
            self.device.push(TrackedDevice { name, udid });
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
}
