use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use clml::{cformat, cprintln};
use serde::Deserialize;

use crate::apple::appstore::AppStoreClient;
use crate::apple::provisioning;
use crate::builder::IosBuilder;
use crate::config::IosProvisioningBackend;
use crate::devices::DeviceSet;
use crate::paths::ensure_strudel_dir;

#[derive(Deserialize)]
struct DevicectlOutput {
    result: DevicectlResult,
}

#[derive(Deserialize)]
struct DevicectlResult {
    devices: Vec<DevicectlDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlDevice {
    identifier: String,
    hardware_properties: DevicectlHardwareProperties,
    connection_properties: DevicectlConnectionProperties,
    device_properties: DevicectlDeviceProperties,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlHardwareProperties {
    platform: Option<String>,
    marketing_name: Option<String>,
    udid: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlConnectionProperties {
    pairing_state: Option<String>,
    transport_type: Option<String>,
    tunnel_state: Option<String>,
}

/// Output of `xcrun devicectl device info details`. A partial result is
/// returned for devices that are unpaired or have Developer Mode off.
#[derive(Deserialize)]
struct DevicectlDetailsOutput {
    result: DevicectlDetailsResult,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlDetailsResult {
    connection_properties: DevicectlConnectionProperties,
    device_properties: DevicectlDeviceProperties,
}

/// Reduce a failed command's message to the line that carries the reason.
fn summarize_failure(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|l| l.contains("Error") || l.contains("error"))
        .or_else(|| message.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("unknown error")
        .to_string()
}

/// A device reported by `xcrun devicectl list devices`.
pub struct DeviceInfo {
    pub udid: String,
    pub name: String,
    pub model: Option<String>,
    pub pairing_state: Option<String>,
    pub transport: Option<String>,
    pub tunnel_state: Option<String>,
    pub developer_mode: Option<String>,
    /// Why `devicectl device info details` failed, if it was run and failed.
    pub detail_error: Option<String>,
}

impl DeviceInfo {
    /// Whether the device can be used for registration right now.
    fn is_connected(&self) -> bool {
        self.tunnel_state.as_deref() == Some("connected")
            || self.developer_mode.as_deref() == Some("enabled")
    }

    fn is_paired(&self) -> bool {
        self.pairing_state.as_deref() == Some("paired")
    }

    fn developer_mode_enabled(&self) -> bool {
        self.developer_mode.as_deref() == Some("enabled")
    }

    /// Print the device's name, state, and the next step to make it usable.
    /// `registered` is whether the device is on the current account's portal,
    /// or `None` when that could not be checked.
    pub fn print(&self, tracked: bool, registered: Option<bool>) {
        cprintln!("<bold>{}</bold> <cyan>[{}]</cyan>", self.name, self.udid);
        if let Some(model) = &self.model {
            cprintln!("<dim>{model}</dim>");
        }

        let connection = if self.is_connected() {
            cformat!("<green>connected</green>")
        } else {
            cformat!("<dim>not connected</dim>")
        };
        let pairing = if self.is_paired() {
            cformat!("<green>paired</green>")
        } else {
            cformat!(
                "<yellow>{}</yellow>",
                self.pairing_state.as_deref().unwrap_or("pairing unknown")
            )
        };
        let dev_mode = if self.developer_mode_enabled() {
            cformat!("<green>developer mode on</green>")
        } else {
            cformat!("<yellow>developer mode off</yellow>")
        };
        let tracked = if tracked { ", tracked" } else { "" };
        cprintln!("{connection}, {pairing}, {dev_mode}<dim>{tracked}</dim>");
        print_registration(registered);

        if let Some(error) = &self.detail_error {
            cprintln!("<dim>{error}</dim>");
        }

        if !self.is_connected() {
            return;
        }
        if !self.is_paired() {
            cprintln!(
                "<yellow>Not paired. Unlock the device and tap Trust, then run \
                 `strudel devices` again. If it still shows unpaired, disconnect and \
                 reconnect the device. Developer Mode does not appear in Settings > \
                 Privacy & Security until the device is trusted.</yellow>"
            );
        } else if !self.developer_mode_enabled() {
            cprintln!(
                "<yellow>Enable Developer Mode in Settings > Privacy & Security, then \
                 restart the device.</yellow>"
            );
        }
    }
}

/// Print whether a device is registered on the current account's portal.
fn print_registration(registered: Option<bool>) {
    match registered {
        Some(true) => cprintln!("<green>registered</green> <dim>on this account</dim>"),
        Some(false) => cprintln!(
            "<yellow>not registered</yellow> <dim>on this account, run `strudel devices add`</dim>"
        ),
        None => cprintln!("<dim>registration unknown</dim>"),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlDeviceProperties {
    name: Option<String>,
    developer_mode_status: Option<String>,
}

impl IosBuilder {
    /// Register connected iOS devices on the portal (if not already) and
    /// record them in `.strudel/devices.toml`.
    pub fn device_add(&self, device_selectors: &[String]) -> Result<()> {
        let connected = self.list_connected_devices()?;

        if connected.is_empty() {
            let known = self.list_devices_detailed().unwrap_or_default();
            if !known.is_empty() {
                cprintln!("<bold>Devices found, none ready:</bold>");
                for d in &known {
                    d.print(false, None);
                }
                bail!("Fix the items above, then run `strudel devices add` again.");
            }
            bail!(
                "No connected iOS devices found.\n\n\
                 To add a device:\n\
                 1. Connect the device to this Mac with a cable and unlock it.\n\
                 2. Tap Trust when prompted on the device, then run `strudel devices` \
                 again to confirm it is paired. If it still shows unpaired, disconnect and \
                 reconnect the device.\n\
                 3. On the device, open Settings > Privacy & Security > Developer Mode, \
                 turn it on, and restart when prompted. The option does not appear until \
                 the device is trusted.\n\
                 4. Run `strudel devices add` again."
            );
        }

        let to_register: Vec<(String, String)> = if device_selectors.is_empty() {
            if connected.len() > 1 {
                let options: Vec<String> = connected
                    .iter()
                    .map(|(udid, name)| format!("{name} ({udid})"))
                    .collect();
                let selected = inquire::MultiSelect::new("Select devices to register:", options)
                    .with_all_selected_by_default()
                    .prompt()?;
                if selected.is_empty() {
                    bail!("No devices selected.");
                }
                connected
                    .into_iter()
                    .filter(|(udid, name)| {
                        selected.iter().any(|s| s == &format!("{name} ({udid})"))
                    })
                    .collect()
            } else {
                connected
            }
        } else {
            let filtered: Vec<_> = connected
                .into_iter()
                .filter(|(udid, name)| device_selectors.iter().any(|s| s == udid || s == name))
                .collect();
            if filtered.is_empty() {
                bail!(
                    "None of the connected devices match the given selectors.\n\
                     Run `strudel devices add` without `--device` to register \
                     all connected devices."
                );
            }
            filtered
        };

        if self.dry_run {
            for (udid, name) in &to_register {
                cprintln!("<dim>[dry-run]</dim> Would register {name} ({udid}) on portal");
                cprintln!("<dim>[dry-run]</dim> Would add to .strudel/devices.toml");
            }
            return Ok(());
        }

        self.register_on_portal(&to_register)?;

        let mut device_set = DeviceSet::load(&self.paths.devices_toml)?;
        for (udid, name) in &to_register {
            device_set.upsert(name.clone(), udid.clone());
        }
        ensure_strudel_dir(&self.paths.strudel_dir)?;
        device_set.save(&self.paths.devices_toml)?;

        println!();
        for (udid, name) in &to_register {
            cprintln!("<green>✔</green> {name} ({udid})");
        }
        cprintln!(
            "\n<green>Done!</green> Registered {} device(s). \
             Run `strudel run --device` to build and install.",
            to_register.len()
        );
        Ok(())
    }

    /// Register a single device on the portal by UDID/name, without tracking
    /// it in `.strudel/devices.toml`. For registering a device you don't have
    /// connected locally (e.g. a teammate's).
    pub fn device_register(&self, name: &str, udid: &str) -> Result<()> {
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> Would register {name} ({udid}) on portal");
            return Ok(());
        }

        self.register_on_portal(&[(udid.to_string(), name.to_string())])?;

        cprintln!("\n<green>Done!</green> Registered {name} ({udid}) on the portal.");
        Ok(())
    }

    /// Print a tracked device that `devicectl` does not list.
    pub fn print_tracked_offline(name: &str, udid: &str, registered: Option<bool>) {
        cprintln!("<bold>{name}</bold> <cyan>[{udid}]</cyan>");
        cprintln!("<dim>not connected, tracked</dim>");
        print_registration(registered);
    }

    /// UDIDs registered on the portal for the current account, per
    /// `[ios] provisioning`.
    pub fn portal_udids(&self) -> Result<Vec<String>> {
        if matches!(self.ios.provisioning, IosProvisioningBackend::Free) {
            return provisioning::registered_udids(&self.cfg);
        }
        let client = AppStoreClient::from_config(&self.cfg)?.show_progress(!self.echo_suppressed());
        Ok(client.list_devices()?.into_iter().map(|d| d.udid).collect())
    }

    /// Register `(udid, name)` pairs on the portal (App Store Connect API or
    /// Apple ID, per `[ios] provisioning`), tolerating devices already
    /// registered there.
    fn register_on_portal(&self, devices: &[(String, String)]) -> Result<()> {
        let ios_settings = &self.ios;

        if matches!(ios_settings.provisioning, IosProvisioningBackend::Free) {
            cprintln!(
                "<dim>Using free provisioning (7-day profiles, max 3 devices, max 10 App IDs).</dim>"
            );
            for (udid, name) in devices {
                self.step(&format!("Registering {name} ({udid}) via Apple ID..."));
                provisioning::register_device(&self.cfg, name, udid)?;
            }
        } else {
            let client = AppStoreClient::from_config(&self.cfg)?;
            let portal_devices = client.list_devices()?;

            for (udid, name) in devices {
                let already_on_portal = portal_devices.iter().any(|d| d.udid == *udid);
                if already_on_portal {
                    cprintln!("<dim>Already registered on portal:</dim> {name} ({udid})");
                    continue;
                }
                self.step(&format!("Registering {name} ({udid}) on portal..."));
                match client.register_device(name, udid) {
                    Ok(_) => {},
                    Err(e) => {
                        // A 409 means the device is already on the portal; treat as success.
                        if !format!("{e}").contains("409") {
                            if format!("{e}").contains("403") {
                                cprintln!(
                                    "<red>error:</red> Insufficient permissions to register device {name} ({udid}). \
                                     Admin role is required to be able to register devices on the App Store Connect portal."
                                );
                            } else {
                                cprintln!(
                                    "<red>error:</red> Failed to register device {name} ({udid}): {e}"
                                );
                            }
                            return Err(e);
                        }
                        cprintln!("<dim>Already registered (portal conflict):</dim> {name}");
                    },
                }
            }
        }

        Ok(())
    }

    /// Resolve which device UDIDs to target for a `device` build.
    ///
    /// Tries `--device` selectors, then `[ios] device` config, then
    /// auto-detected connected devices. All resolved UDIDs must be tracked in
    /// `.strudel/devices.toml`.
    pub(super) fn resolve_target_udids(&self, device_selectors: &[String]) -> Result<Vec<String>> {
        let ios_settings = &self.ios;
        let device_set = DeviceSet::load(&self.paths.devices_toml)?;

        if !device_selectors.is_empty() {
            let mut udids = Vec::new();
            for selector in device_selectors {
                match device_set.resolve(selector) {
                    Some(udid) => udids.push(udid.to_string()),
                    None => bail!(
                        "Device {:?} is not tracked in .strudel/devices.toml.\n\
                         Run `strudel devices add` to register your device(s).",
                        selector
                    ),
                }
            }
            return Ok(udids);
        }

        if let Some(ref selector) = ios_settings.device {
            return match device_set.resolve(selector) {
                Some(udid) => Ok(vec![udid.to_string()]),
                None => bail!(
                    "Device {:?} (from [ios] config) is not tracked in .strudel/devices.toml.\n\
                     Run `strudel devices add` to register your device(s).",
                    selector
                ),
            };
        }

        // Fast path: a single registered device is unambiguous, so use it
        // directly without scanning. If it isn't actually connected, the
        // install step surfaces a clear error later.
        if device_set.device.len() == 1 {
            let d = &device_set.device[0];
            self.note(cformat!(
                "<green>✔</green> Using registered device: {}",
                d.name
            ));
            return Ok(vec![d.udid.clone()]);
        }

        self.step("Detecting connected iOS device...");
        let connected = self.list_connected_devices()?;

        if self.dry_run {
            return Ok(connected.into_iter().map(|(udid, _)| udid).collect());
        }

        let (resolution, unregistered) = resolve_connected(&device_set, connected)?;

        // Hint about connected devices that aren't tracked rather than failing.
        for (udid, name) in &unregistered {
            self.note(cformat!(
                "<dim>Skipping untracked device {name} ({udid}). Run \
                 `strudel devices add` to add it.</dim>"
            ));
        }

        match resolution {
            DeviceResolution::Single { udid, name } => {
                self.note(cformat!("<green>✔</green> Found device: {name}"));
                Ok(vec![udid])
            },
            DeviceResolution::Prompt(registered) => {
                cprintln!("<bold>Multiple registered devices connected. Choose:</bold>");
                cprintln!("  <bold>0</bold>. All devices");
                for (i, (_, name)) in registered.iter().enumerate() {
                    cprintln!("  <bold>{}</bold>. {name}", i + 1);
                }
                loop {
                    print!("Device [0-{}]: ", registered.len());
                    io::stdout().flush()?;
                    let mut line = String::new();
                    io::stdin().read_line(&mut line)?;
                    let n: usize = line.trim().parse().unwrap_or(usize::MAX);
                    if n == 0 {
                        return Ok(registered.into_iter().map(|(udid, _)| udid).collect());
                    }
                    if n >= 1 && n <= registered.len() {
                        let (udid, name) = &registered[n - 1];
                        cprintln!("<green>✔</green> Using device: {name}");
                        return Ok(vec![udid.clone()]);
                    }
                    cprintln!(
                        "<red>error:</red> Enter a number between 0 and {}.",
                        registered.len()
                    );
                }
            },
        }
    }

    /// Return all iOS devices known to devicectl, connected or not.
    pub fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let output = self.sh.run(&[
            "xcrun",
            "devicectl",
            "list",
            "devices",
            "--json-output",
            "-",
        ])?;
        let parsed: DevicectlOutput =
            serde_json::from_str(&output).context("Failed to parse devicectl output")?;

        Ok(parsed
            .result
            .devices
            .into_iter()
            .filter(|d| d.hardware_properties.platform.as_deref() == Some("iOS"))
            .map(|d| DeviceInfo {
                udid: d.hardware_properties.udid,
                name: d.device_properties.name.unwrap_or(d.identifier),
                model: d.hardware_properties.marketing_name,
                pairing_state: d.connection_properties.pairing_state,
                transport: d.connection_properties.transport_type,
                tunnel_state: d.connection_properties.tunnel_state,
                developer_mode: d.device_properties.developer_mode_status,
                detail_error: None,
            })
            .collect())
    }

    /// Like [`list_devices`](Self::list_devices), then query each device with
    /// `xcrun devicectl device info details`. The details call contacts the
    /// device, so its pairing and developer mode state is current, and it
    /// reports why a device is not usable. This is slower than the plain list.
    pub fn list_devices_detailed(&self) -> Result<Vec<DeviceInfo>> {
        let mut devices = self.list_devices()?;
        for d in &mut devices {
            self.refresh_device_details(d);
        }
        Ok(devices)
    }

    fn refresh_device_details(&self, device: &mut DeviceInfo) {
        let output = self.sh.run(&[
            "xcrun",
            "devicectl",
            "device",
            "info",
            "details",
            "--device",
            device.udid.as_str(),
            "--json-output",
            "-",
        ]);
        let output = match output {
            Ok(output) => output,
            Err(e) => {
                device.detail_error = Some(summarize_failure(&e.to_string()));
                return;
            },
        };
        match serde_json::from_str::<DevicectlDetailsOutput>(&output) {
            Ok(parsed) => {
                let conn = parsed.result.connection_properties;
                device.pairing_state = conn.pairing_state.or(device.pairing_state.take());
                device.transport = conn.transport_type.or(device.transport.take());
                device.tunnel_state = conn.tunnel_state.or(device.tunnel_state.take());
                device.developer_mode = parsed
                    .result
                    .device_properties
                    .developer_mode_status
                    .or(device.developer_mode.take());
            },
            Err(e) => device.detail_error = Some(format!("could not parse device details: {e}")),
        }
    }

    /// Return all connected iOS devices as `(udid, name)` pairs.
    fn list_connected_devices(&self) -> Result<Vec<(String, String)>> {
        if self.dry_run {
            return Ok(vec![(
                "<device-udid>".to_string(),
                "<device-name>".to_string(),
            )]);
        }
        Ok(self
            .list_devices()?
            .into_iter()
            .filter(DeviceInfo::is_connected)
            .map(|d| (d.udid, d.name))
            .collect())
    }
}

/// Outcome of resolving connected devices against the tracked set, short of
/// any interactive prompt.
enum DeviceResolution {
    /// Exactly one tracked device is connected; install to it directly.
    Single { udid: String, name: String },
    /// Multiple tracked devices are connected; the caller must prompt the user
    /// to choose among these `(udid, name)` pairs.
    Prompt(Vec<(String, String)>),
}

/// Partition `connected` `(udid, name)` devices into those tracked in
/// `device_set` and those that aren't, deciding how the tracked ones resolve.
///
/// Returns the resolution alongside the untracked devices (so the caller can
/// hint about them). Errors when nothing is connected, or when connected
/// devices exist but none are tracked.
fn resolve_connected(
    device_set: &DeviceSet,
    connected: Vec<(String, String)>,
) -> Result<(DeviceResolution, Vec<(String, String)>)> {
    if connected.is_empty() {
        bail!(
            "No connected iOS devices found.\n\
             Plug in your iPhone, trust this Mac, and enable Developer Mode \
             (Settings -> Privacy & Security -> Developer Mode).\n\
             List devices with: xcrun devicectl list devices"
        );
    }

    let (registered, unregistered): (Vec<_>, Vec<_>) = connected
        .into_iter()
        .partition(|(udid, _)| device_set.contains_udid(udid));

    let resolution = match registered.len() {
        0 => bail!(
            "No connected devices are tracked in .strudel/devices.toml.\n\
             Run `strudel devices add` to register your device(s)."
        ),
        1 => {
            let (udid, name) = registered.into_iter().next().unwrap();
            DeviceResolution::Single { udid, name }
        },
        _ => DeviceResolution::Prompt(registered),
    };

    Ok((resolution, unregistered))
}

#[cfg(test)]
mod tests {
    use crate::builder::ios::registration::{DeviceResolution, resolve_connected};
    use crate::devices::DeviceSet;

    fn dev(udid: &str, name: &str) -> (String, String) {
        (udid.to_string(), name.to_string())
    }

    fn device_set(udids: &[(&str, &str)]) -> DeviceSet {
        let mut set = DeviceSet::default();
        for (name, udid) in udids {
            set.upsert(name.to_string(), udid.to_string());
        }
        set
    }

    #[test]
    fn resolve_connected_errors_when_nothing_connected() {
        let set = device_set(&[("iPhone", "AAA")]);
        assert!(resolve_connected(&set, vec![]).is_err());
    }

    #[test]
    fn resolve_connected_errors_when_none_tracked() {
        let set = device_set(&[("iPhone", "AAA")]);
        let connected = vec![dev("BBB", "Someone's iPhone")];
        assert!(resolve_connected(&set, connected).is_err());
    }

    #[test]
    fn resolve_connected_single_tracked_resolves_directly() {
        let set = device_set(&[("iPhone", "AAA")]);
        let connected = vec![dev("AAA", "My iPhone"), dev("BBB", "Untracked")];
        let (resolution, unregistered) = resolve_connected(&set, connected).unwrap();
        match resolution {
            DeviceResolution::Single { udid, name } => {
                assert_eq!(udid, "AAA");
                assert_eq!(name, "My iPhone");
            },
            _ => panic!("expected a single resolved device"),
        }
        // The untracked device is reported back for a hint, not failed on.
        assert_eq!(unregistered, vec![dev("BBB", "Untracked")]);
    }

    #[test]
    fn resolve_connected_multiple_tracked_prompts() {
        let set = device_set(&[("iPhone A", "AAA"), ("iPhone B", "BBB")]);
        let connected = vec![dev("AAA", "iPhone A"), dev("BBB", "iPhone B")];
        let (resolution, unregistered) = resolve_connected(&set, connected).unwrap();
        match resolution {
            DeviceResolution::Prompt(devices) => {
                assert_eq!(devices.len(), 2);
            },
            _ => panic!("expected a prompt"),
        }
        assert!(unregistered.is_empty());
    }
}
