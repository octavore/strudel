//! Devices of a macOS development profile.

use std::process::Command;

use anyhow::{Context, Result, bail};
use clml::cprintln;

use crate::apple::appstore::AppStoreClient;
use crate::builder::MacosBuilder;
use crate::config::Platform;
use crate::devices::{DeviceSet, confirm_add};
use crate::paths::ensure_strudel_dir;

/// Provisioning UDID in `system_profiler SPHardwareDataType -json` output.
/// Falls back to the hardware UUID, which is the UDID on Intel Macs without a
/// T2 chip.
fn parse_hardware_udid(json: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("Failed to parse `system_profiler` output")?;
    let hardware = &value["SPHardwareDataType"][0];
    ["provisioning_UDID", "platform_UUID"]
        .iter()
        .find_map(|key| hardware[key].as_str())
        .map(str::to_string)
        .context("`system_profiler` output has no provisioning UDID")
}

/// Name and UDID of this Mac.
fn current_mac() -> Result<(String, String)> {
    let output = Command::new("system_profiler")
        .args(["SPHardwareDataType", "-json"])
        .output()
        .context("Failed to run `system_profiler`")?;
    if !output.status.success() {
        bail!("`system_profiler SPHardwareDataType` failed");
    }
    let udid = parse_hardware_udid(&String::from_utf8_lossy(&output.stdout))?;
    let name = Command::new("scutil")
        .arg("--ComputerName")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "This Mac".to_string());
    Ok((name, udid))
}

impl MacosBuilder {
    /// Register this Mac on the portal and track it in
    /// `.strudel/devices.toml`.
    pub fn device_add_current(&self, register: bool) -> Result<()> {
        let (name, udid) = current_mac()?;
        cprintln!("Detected this Mac: <bold>{name}</bold> <cyan>[{udid}]</cyan>");
        self.device_add_manual(&name, &udid, register)
    }

    /// Register a device on the portal as a macOS device (if `register` is
    /// set) and track it in `.strudel/devices.toml`.
    pub fn device_add_manual(&self, name: &str, udid: &str, register: bool) -> Result<()> {
        if self.dry_run {
            if register {
                cprintln!("<dim>[dry-run]</dim> Would register {name} ({udid}) as a macOS device");
            }
            cprintln!("<dim>[dry-run]</dim> Would add to .strudel/devices.toml");
            return Ok(());
        }
        confirm_add(&[(name, udid)], Platform::Macos, register)?;

        if register {
            let client =
                AppStoreClient::from_config(&self.cfg)?.show_progress(!self.echo_suppressed());
            self.step(&format!("Registering {name} ({udid}) on portal..."));
            if !client.ensure_device_registered(name, udid, Platform::Macos)? {
                cprintln!("<dim>Already registered on portal:</dim> {name} ({udid})");
            }
        }

        let mut set = DeviceSet::load(&self.paths.devices_toml)?;
        set.upsert_on(name.to_string(), udid.to_string(), Platform::Macos);
        ensure_strudel_dir(&self.paths.strudel_dir)?;
        set.save(&self.paths.devices_toml)?;

        cprintln!(
            "\n<green>✔</green> {name} ({udid})\n\n<green>Done!</green> Tracked 1 macOS device. \
             A signing identity starting with \"Apple Development\" or \"Mac Developer\" now \
             creates a development profile for it."
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioning_udid_is_preferred() {
        let json = r#"{"SPHardwareDataType":[{"platform_UUID":"HW-UUID","provisioning_UDID":"PROV-UDID"}]}"#;
        assert_eq!(parse_hardware_udid(json).unwrap(), "PROV-UDID");
    }

    #[test]
    fn hardware_uuid_is_the_fallback() {
        let json = r#"{"SPHardwareDataType":[{"platform_UUID":"HW-UUID"}]}"#;
        assert_eq!(parse_hardware_udid(json).unwrap(), "HW-UUID");
    }

    #[test]
    fn output_without_a_udid_is_an_error() {
        assert!(parse_hardware_udid(r#"{"SPHardwareDataType":[{}]}"#).is_err());
        assert!(parse_hardware_udid("not json").is_err());
    }
}
