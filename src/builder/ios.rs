use std::collections::HashMap;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Builder, step};
use crate::appstore::AppStoreClient;
use crate::devices::DeviceSet;
use crate::paths::ensure_strudel_dir;
use crate::shell::ShellCommand;

/// Simulator or device flavor — selects the SDK, triple suffix, and
/// platform keys that go into the iOS `Info.plist`.
enum IosFlavor {
    Simulator,
    Device,
}

#[derive(Deserialize)]
struct SimctlDevicesOutput {
    devices: HashMap<String, Vec<SimctlDevice>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimctlDevice {
    name: String,
    udid: String,
    is_available: bool,
}

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
struct DevicectlHardwareProperties {
    platform: Option<String>,
    udid: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlConnectionProperties {
    tunnel_state: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicectlDeviceProperties {
    name: Option<String>,
    developer_mode_status: Option<String>,
}

impl Builder {
    /// Build for the iOS Simulator and launch in Simulator.app.
    pub fn sim(&self, sim_override: Option<&str>) -> Result<()> {
        if !self.cfg.extensions.is_empty() {
            cprintln!(
                "<yellow>warning:</yellow> iOS extension bundling is not yet supported; \
                 [[extensions]] in this target will be ignored."
            );
        }
        let sim_name = sim_override.unwrap_or(&self.cfg.ios_simulator);
        let target = &self.cfg.target_name;
        let config_flag = if self.debug { "debug" } else { "release" };
        let deployment = &self.cfg.ios_deployment_target;

        let host_arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            other => other,
        };
        let triple = format!("{host_arch}-apple-ios{deployment}-simulator");

        let sdk_path = self
            .sh
            .run(&["xcrun", "--sdk", "iphonesimulator", "--show-sdk-path"])
            .map(|s| {
                if s.is_empty() {
                    "<iphonesimulator-sdk>".into()
                } else {
                    s
                }
            })?;
        let swift = self
            .sh
            .run(&["xcrun", "-f", "swift"])
            .map(|s| if s.is_empty() { "swift".into() } else { s })?;

        step(&format!("Building for iOS Simulator ({sim_name})..."));
        let source = self.cfg.source_dir.to_str().unwrap();
        self.sh.run_streamed_env(
            ShellCommand::new(&swift)
                .args([
                    "build",
                    "-c",
                    config_flag,
                    "--triple",
                    &triple,
                    "--sdk",
                    &sdk_path,
                    "--package-path",
                    source,
                ])
                .envs(&self.cfg.build_env),
        )?;

        let bin_dir = self.ios_bin_dir(&swift, config_flag, &triple, &sdk_path)?;
        let binary = self.find_binary_in(&bin_dir, target)?;

        step("Assembling iOS Simulator bundle...");
        let bundle_dir = self.paths.build_dir.join("ios-sim");
        let app_bundle = bundle_dir.join(format!("{target}.app"));
        self.assemble_ios_bundle(&binary, &app_bundle, IosFlavor::Simulator)?;

        step("Ad-hoc signing simulator bundle...");
        self.sh.run(
            ShellCommand::new("codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(app_bundle.to_str().unwrap()),
        )?;

        let sim_udid = self.find_simulator(sim_name)?;

        step("Booting iOS Simulator...");
        let _ = self.sh.run(&["xcrun", "simctl", "boot", &sim_udid]);
        self.sh.run(&["open", "-a", "Simulator"])?;

        step("Installing app on simulator...");
        let app_str = app_bundle.to_str().unwrap();
        self.sh
            .run(&["xcrun", "simctl", "install", &sim_udid, app_str])?;

        step("Launching app...");
        self.sh.run(ShellCommand::new("xcrun").args([
            "simctl",
            "launch",
            "--console-pty",
            &sim_udid,
            &self.cfg.bundle_id,
        ]))?;

        Ok(())
    }

    /// Build for one or more connected iOS devices, then install and launch.
    ///
    /// Requires devices to be registered via `strudel device register`. Auto-
    /// fetches and caches a development provisioning profile via the App Store
    /// Connect API when one is not already current.
    pub fn device(&self, device_selectors: &[String]) -> Result<()> {
        if !self.cfg.extensions.is_empty() {
            cprintln!(
                "<yellow>warning:</yellow> iOS extension bundling is not yet supported; \
                 [[extensions]] in this target will be ignored."
            );
        }
        let target = &self.cfg.target_name;
        let config_flag = if self.debug { "debug" } else { "release" };
        let deployment = &self.cfg.ios_deployment_target;
        let triple = format!("arm64-apple-ios{deployment}");

        let sdk_path = self
            .sh
            .run(&["xcrun", "--sdk", "iphoneos", "--show-sdk-path"])
            .map(|s| {
                if s.is_empty() {
                    "<iphoneos-sdk>".into()
                } else {
                    s
                }
            })?;
        let swift = self
            .sh
            .run(&["xcrun", "-f", "swift"])
            .map(|s| if s.is_empty() { "swift".into() } else { s })?;

        step("Building for iOS device...");
        let source = self.cfg.source_dir.to_str().unwrap();
        self.sh.run_streamed_env(
            ShellCommand::new(&swift)
                .args([
                    "build",
                    "-c",
                    config_flag,
                    "--triple",
                    &triple,
                    "--sdk",
                    &sdk_path,
                    "--package-path",
                    source,
                ])
                .envs(&self.cfg.build_env),
        )?;

        let bin_dir = self.ios_bin_dir(&swift, config_flag, &triple, &sdk_path)?;
        let binary = self.find_binary_in(&bin_dir, target)?;

        step("Assembling iOS device bundle...");
        let bundle_dir = self.paths.build_dir.join("ios-device");
        let app_bundle = bundle_dir.join(format!("{target}.app"));
        self.assemble_ios_bundle(&binary, &app_bundle, IosFlavor::Device)?;

        // Resolve target devices (returns UDIDs).
        let target_udids = self.resolve_target_udids(device_selectors)?;

        // Resolve provisioning profile.
        let profile_path = self.resolve_profile(&target_udids)?;

        step("Embedding provisioning profile...");
        self.copy_file(&profile_path, &app_bundle.join("embedded.mobileprovision"))?;

        step("Signing device bundle...");
        let identity = if self.cfg.sign_identity.is_empty() {
            "Apple Development"
        } else {
            &self.cfg.sign_identity
        };
        self.sign_ios_device(&app_bundle, &profile_path, identity)?;

        let app_str = app_bundle.to_str().unwrap();
        for udid in &target_udids {
            step(&format!("Installing on {udid}..."));
            self.sh.run(&[
                "xcrun",
                "devicectl",
                "device",
                "install",
                "app",
                "--device",
                udid,
                app_str,
            ])?;

            step("Launching app...");
            self.sh.run(&[
                "xcrun",
                "devicectl",
                "device",
                "process",
                "launch",
                "--device",
                udid,
                &self.cfg.bundle_id,
            ])?;
        }

        println!();
        cprintln!(
            "<green>Done!</green> App installed and launched on {} device(s).",
            target_udids.len()
        );
        Ok(())
    }

    /// Fetch (or force-refresh) the development provisioning profile and write
    /// it to `.strudel/<bundle_id>.mobileprovision`.
    pub fn profile_fetch(&self, force: bool) -> Result<()> {
        let cached = &self.paths.cached_profile;
        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        let udids = device_set.udids();

        if !force
            && cached.exists()
            && profile_is_current(cached, &udids, &self.cfg.bundle_id, &self.cfg.team_id)?
        {
            cprintln!(
                "<green>✔</green> Cached profile is current: {}",
                cached.display()
            );
            return Ok(());
        }

        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> Would fetch provisioning profile via App Store Connect API"
            );
            cprintln!("<dim>[dry-run]</dim> Would write to {}", cached.display());
            return Ok(());
        }

        self.auto_fetch_profile()?;

        println!();
        cprintln!(
            "<green>Done!</green> Profile written to {}",
            cached.display()
        );
        cprintln!(
            "<dim>Tip: to pin this profile explicitly, add to strudel.toml:\n  [build]\n  provisioning_profile = \"{}\"</dim>",
            cached.display()
        );
        Ok(())
    }

    /// Register connected iOS devices on the portal and record them in
    /// `.strudel/devices.toml`.
    pub fn device_register(&self, device_selectors: &[String]) -> Result<()> {
        let connected = self.list_connected_devices()?;

        if connected.is_empty() {
            bail!(
                "No connected iOS devices found.\n\
                 Plug in your iPhone, trust this Mac, and enable Developer Mode \
                 (Settings -> Privacy & Security -> Developer Mode)."
            );
        }

        let to_register: Vec<(String, String)> = if device_selectors.is_empty() {
            connected
        } else {
            let filtered: Vec<_> = connected
                .into_iter()
                .filter(|(udid, name)| device_selectors.iter().any(|s| s == udid || s == name))
                .collect();
            if filtered.is_empty() {
                bail!(
                    "None of the connected devices match the given selectors.\n\
                     Run `strudel device register` without `--device` to register \
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

        let mut device_set = DeviceSet::load(&self.paths.devices_toml)?;
        let client = AppStoreClient::from_config(&self.cfg)?;
        let portal_devices = client.list_devices()?;

        for (udid, name) in &to_register {
            let already_on_portal = portal_devices.iter().any(|d| d.udid == *udid);
            if already_on_portal {
                cprintln!("<dim>Already registered on portal:</dim> {name} ({udid})");
            } else {
                step(&format!("Registering {name} ({udid}) on portal..."));
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
             Run `strudel device` to build and install.",
            to_register.len()
        );
        Ok(())
    }

    /// Resolve which device UDIDs to target for a `device` build.
    ///
    /// Tries `--device` selectors, then `[ios] device` config, then
    /// auto-detected connected devices. All resolved UDIDs must be tracked in
    /// `.strudel/devices.toml`.
    fn resolve_target_udids(&self, device_selectors: &[String]) -> Result<Vec<String>> {
        let device_set = DeviceSet::load(&self.paths.devices_toml)?;

        if !device_selectors.is_empty() {
            let mut udids = Vec::new();
            for selector in device_selectors {
                match device_set.resolve(selector) {
                    Some(udid) => udids.push(udid.to_string()),
                    None => bail!(
                        "Device {:?} is not tracked in .strudel/devices.toml.\n\
                         Run `strudel device register` to register your device(s).",
                        selector
                    ),
                }
            }
            return Ok(udids);
        }

        if let Some(ref selector) = self.cfg.ios_device {
            return match device_set.resolve(selector) {
                Some(udid) => Ok(vec![udid.to_string()]),
                None => bail!(
                    "Device {:?} (from [ios] config) is not tracked in .strudel/devices.toml.\n\
                     Run `strudel device register` to register your device(s).",
                    selector
                ),
            };
        }

        // Fast path: a single registered device is unambiguous, so use it
        // directly without scanning. If it isn't actually connected, the
        // install step surfaces a clear error later.
        if device_set.device.len() == 1 {
            let d = &device_set.device[0];
            cprintln!("<green>✔</green> Using registered device: {}", d.name);
            return Ok(vec![d.udid.clone()]);
        }

        step("Detecting connected iOS device...");
        let connected = self.list_connected_devices()?;

        if self.dry_run {
            return Ok(connected.into_iter().map(|(udid, _)| udid).collect());
        }

        let (resolution, unregistered) = resolve_connected(&device_set, connected)?;

        // Hint about connected devices that aren't tracked rather than failing.
        for (udid, name) in &unregistered {
            cprintln!(
                "<dim>Skipping untracked device {name} ({udid}) — run \
                 `strudel device register` to add it.</dim>"
            );
        }

        match resolution {
            DeviceResolution::Single { udid, name } => {
                cprintln!("<green>✔</green> Found device: {name}");
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

    /// Resolve the provisioning profile path for a device build.
    ///
    /// Uses the user-configured profile if set (warns if stale), the cached
    /// profile if current, or auto-fetches via the App Store Connect API.
    fn resolve_profile(&self, target_udids: &[String]) -> Result<PathBuf> {
        let udid_refs: Vec<&str> = target_udids.iter().map(String::as_str).collect();

        if let Some(ref p) = self.cfg.provisioning_profile {
            if !self.dry_run
                && matches!(
                    profile_is_current(p, &udid_refs, &self.cfg.bundle_id, &self.cfg.team_id),
                    Ok(false)
                )
            {
                cprintln!(
                    "<yellow>warning:</yellow> Configured provisioning profile may be \
                     stale (expired or missing device UDIDs). Proceeding anyway.\n\
                     Remove `provisioning_profile` from strudel.toml to let strudel \
                     manage the profile automatically."
                );
            }
            return Ok(p.clone());
        }

        let cached = &self.paths.cached_profile;

        if !self.dry_run
            && cached.exists()
            && profile_is_current(cached, &udid_refs, &self.cfg.bundle_id, &self.cfg.team_id)?
        {
            cprintln!(
                "<green>✔</green> Using cached profile: {}",
                cached.display()
            );
            return Ok(cached.clone());
        }

        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> Would auto-fetch provisioning profile \
                 via App Store Connect API"
            );
            return Ok(cached.clone());
        }

        self.auto_fetch_profile()?;
        Ok(cached.clone())
    }

    /// Call the App Store Connect API to create a development profile and write
    /// it to the cache. Uses the full tracked device set from `devices.toml`.
    fn auto_fetch_profile(&self) -> Result<()> {
        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        if device_set.device.is_empty() {
            bail!(
                "No devices are tracked in .strudel/devices.toml.\n\
                 Run `strudel device register` first to register your device(s)."
            );
        }

        let client = AppStoreClient::from_config(&self.cfg)?;

        step("Looking up bundle ID on App Store Connect...");
        let bundle_id_ref =
            client.find_or_create_bundle_id(&self.cfg.bundle_id, &self.cfg.app_name)?;
        cprintln!("<dim>  Bundle ID: {} (portal ID: {})</dim>", self.cfg.bundle_id, bundle_id_ref);

        step("Finding development certificates...");
        let certs = client.list_development_certificates()?;
        cprintln!("<dim>  Found {} development certificate(s)</dim>", certs.len());
        let cert_ids: Vec<String> = certs.iter().map(|c| c.id.clone()).collect();

        step("Matching tracked devices to portal...");
        cprintln!("<dim>  Tracked devices: {}</dim>", device_set.device.len());
        let portal_devices = client.list_devices()?;
        cprintln!("<dim>  Portal devices: {}</dim>", portal_devices.len());
        let mut device_ids = Vec::new();
        for tracked in &device_set.device {
            match portal_devices.iter().find(|d| d.udid == tracked.udid) {
                Some(pd) => {
                    cprintln!("<dim>  Matched: {} ({})</dim>", tracked.name, tracked.udid);
                    device_ids.push(pd.id.clone());
                },
                None => bail!(
                    "Device {} ({}) is in .strudel/devices.toml but not found on the \
                     App Store Connect portal.\n\
                     Run `strudel device register` to re-register your devices.",
                    tracked.name,
                    tracked.udid
                ),
            }
        }

        let profile_name = format!("strudel {} Development", self.cfg.app_name);
        step(&format!(
            "Creating provisioning profile \"{profile_name}\"..."
        ));
        let profile_bytes = client.create_development_profile(
            &profile_name,
            &bundle_id_ref,
            &cert_ids,
            &device_ids,
        )?;

        ensure_strudel_dir(&self.paths.strudel_dir)?;
        fs::write(&self.paths.cached_profile, &profile_bytes)?;
        cprintln!(
            "<green>✔</green> Profile cached at {}",
            self.paths.cached_profile.display()
        );
        Ok(())
    }

    /// Assemble a flat iOS `.app` bundle (no `Contents/` subdirectory).
    /// Generates `Info.plist` from `info_json_path` (if set) merged with
    /// required iOS keys. Optionally compiles the asset catalog.
    fn assemble_ios_bundle(
        &self,
        binary: &Path,
        app_bundle: &Path,
        flavor: IosFlavor,
    ) -> Result<()> {
        if !self.dry_run {
            if app_bundle.exists() {
                fs::remove_dir_all(app_bundle)?;
            }
            fs::create_dir_all(app_bundle)?;
        }

        self.copy_file(binary, &app_bundle.join(&self.cfg.target_name))?;

        let mut info: Value = match &self.cfg.info_json_path {
            Some(path) => {
                let s = fs::read_to_string(path)
                    .with_context(|| format!("Failed to read info JSON at {}", path.display()))?;
                serde_json::from_str(&s)
                    .with_context(|| format!("Failed to parse info JSON at {}", path.display()))?
            },
            None => Value::Object(Default::default()),
        };
        let obj = info
            .as_object_mut()
            .context("Info JSON must be a JSON object at the top level.")?;

        obj.entry("CFBundleExecutable")
            .or_insert_with(|| json!(&self.cfg.target_name));
        obj.insert("CFBundleIdentifier".into(), json!(&self.cfg.bundle_id));
        obj.insert("CFBundleName".into(), json!(&self.cfg.app_name));
        obj.insert("CFBundleDisplayName".into(), json!(&self.cfg.app_name));
        obj.insert(
            "CFBundleShortVersionString".into(),
            json!(&self.cfg.version),
        );
        obj.insert("CFBundleVersion".into(), json!(&self.cfg.build_number));
        obj.insert("CFBundlePackageType".into(), json!("APPL"));
        obj.entry("UIDeviceFamily").or_insert_with(|| json!([1]));
        obj.insert(
            "MinimumOSVersion".into(),
            json!(&self.cfg.ios_deployment_target),
        );

        match flavor {
            IosFlavor::Simulator => {
                obj.insert(
                    "CFBundleSupportedPlatforms".into(),
                    json!(["iPhoneSimulator"]),
                );
                obj.insert("DTPlatformName".into(), json!("iphonesimulator"));
            },
            IosFlavor::Device => {
                obj.insert("CFBundleSupportedPlatforms".into(), json!(["iPhoneOS"]));
                obj.insert("DTPlatformName".into(), json!("iphoneos"));
                obj.entry("UIRequiredDeviceCapabilities")
                    .or_insert_with(|| json!(["arm64"]));
            },
        }

        let json_bytes = serde_json::to_vec_pretty(&info)?;
        let plist_path = app_bundle.join("Info.plist");
        self.sh.run_stdin(
            &[
                "plutil",
                "-convert",
                "xml1",
                "-o",
                plist_path.to_str().unwrap(),
                "-",
            ],
            &json_bytes,
        )?;

        if let Some(assets_dir) = &self.cfg.ios_assets_dir {
            let platform = match flavor {
                IosFlavor::Simulator => "iphonesimulator",
                IosFlavor::Device => "iphoneos",
            };
            self.compile_ios_assets(assets_dir, app_bundle, platform)?;
        }

        Ok(())
    }

    fn compile_ios_assets(
        &self,
        assets_dir: &Path,
        app_bundle: &Path,
        platform: &str,
    ) -> Result<()> {
        step("Compiling asset catalog...");
        let assets_str = assets_dir.to_str().unwrap();
        let bundle_str = app_bundle.to_str().unwrap();
        let deployment = &self.cfg.ios_deployment_target;
        let icon_name = &self.cfg.ios_app_icon_name;

        self.sh.run(ShellCommand::new("xcrun").args([
            "actool",
            assets_str,
            "--compile",
            bundle_str,
            "--platform",
            platform,
            "--minimum-deployment-target",
            deployment,
            "--target-device",
            "iphone",
            "--app-icon",
            icon_name,
            "--bundle-identifier",
            &self.cfg.bundle_id,
            "--product-type",
            "com.apple.product-type.application",
            "--output-partial-info-plist",
            "/dev/null",
        ]))?;
        Ok(())
    }

    /// Sign a device `.app` bundle with entitlements extracted directly from
    /// the provisioning profile. Using profile-derived entitlements (rather
    /// than a hand-edited JSON) ensures the signature matches the profile
    /// exactly. `--generate-entitlement-der` is required on modern iOS.
    fn sign_ios_device(
        &self,
        app_bundle: &Path,
        profile_path: &Path,
        identity: &str,
    ) -> Result<()> {
        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> security cms -D -i {} | extract Entitlements",
                profile_path.display()
            );
            cprintln!(
                "<dim>[dry-run]</dim> codesign --force --sign {} --entitlements \
                 ios-device-entitlements.plist --generate-entitlement-der {}",
                identity,
                app_bundle.display()
            );
            return Ok(());
        }

        step("Checking signing identity...");
        self.check_signing_identity(identity)?;

        let profile_plist = decode_profile(profile_path)?;

        step("Checking certificate is authorized by profile...");
        self.check_identity_in_profile(identity, &profile_plist)?;

        let entitlements = profile_plist
            .as_dictionary()
            .and_then(|d| d.get("Entitlements"))
            .context("Provisioning profile has no Entitlements key")?;

        let ent_plist_path = app_bundle
            .parent()
            .unwrap_or(Path::new("."))
            .join("ios-device-entitlements.plist");
        plist::to_file_xml(&ent_plist_path, entitlements)
            .context("Failed to write entitlements plist")?;
        let ent_str = ent_plist_path.to_str().unwrap();
        let bundle_str = app_bundle.to_str().unwrap();

        std::process::Command::new("codesign")
            .args([
                "--force",
                "--sign",
                identity,
                "--entitlements",
                ent_str,
                "--generate-entitlement-der",
                bundle_str,
            ])
            .status()
            .context("Failed to run codesign")?;

        step("Verifying device signature...");
        let verify_status = std::process::Command::new("codesign")
            .args(["--verify", "--deep", "--strict", "--verbose=2", bundle_str])
            .status()
            .context("Failed to run codesign --verify")?;
        if !verify_status.success() {
            // Show the identity actually embedded in the bundle for diagnosis.
            let _ = std::process::Command::new("codesign")
                .args(["-dvvv", bundle_str])
                .status();
            bail!(
                "Signature verification failed - the app will be rejected at install time.\n\
                 The signing certificate may have expired since the bundle was built.\n\
                 Check: security find-identity -v -p codesigning"
            );
        }

        Ok(())
    }

    fn check_signing_identity(&self, identity: &str) -> Result<()> {
        let valid_out = std::process::Command::new("security")
            .args(["find-identity", "-v", "-p", "codesigning"])
            .output()
            .context("Failed to run `security find-identity`")?;
        let valid_stdout = String::from_utf8_lossy(&valid_out.stdout);

        if let Some(line) = valid_stdout.lines().find(|l| l.contains(identity)) {
            // Extract cert name between quotes: `  N) HASH "Cert Name"`
            let cert_name = line
                .find('"')
                .and_then(|s| line.rfind('"').filter(|&e| e > s).map(|e| &line[s + 1..e]))
                .unwrap_or("");
            if cert_name.starts_with("Apple Distribution")
                || cert_name.starts_with("iPhone Distribution")
            {
                bail!(
                    "Signing identity {identity:?} is a distribution certificate \
                     and cannot be used for development device installs.\n\
                     Use an \"Apple Development\" certificate instead."
                );
            }
            return Ok(());
        }

        // Not in the valid list - check if it exists but is expired/revoked.
        let all_out = std::process::Command::new("security")
            .args(["find-identity", "-p", "codesigning"])
            .output()
            .context("Failed to run `security find-identity`")?;
        let all_stdout = String::from_utf8_lossy(&all_out.stdout);

        if all_stdout.contains(identity) {
            bail!(
                "Signing identity {identity:?} is expired or revoked.\n\
                 Renew in Xcode (Settings > Accounts > Manage Certificates) \
                 or at developer.apple.com."
            );
        }

        bail!(
            "Signing identity {identity:?} not found in Keychain.\n\
             Valid identities:\n{}\n\
             Set [ios] sign_identity in strudel.toml to match one of the above.",
            valid_stdout.trim()
        );
    }

    /// Verify that the signing identity's certificate is listed in the
    /// profile's DeveloperCertificates. Mismatches cause iOS to reject the
    /// app at install time even when the local signature verifies cleanly.
    fn check_identity_in_profile(&self, identity: &str, profile: &plist::Value) -> Result<()> {
        let Some(certs) = profile
            .as_dictionary()
            .and_then(|d| d.get("DeveloperCertificates"))
            .and_then(|v| v.as_array())
        else {
            return Ok(());
        };

        // Extract the SHA1 fingerprint for our identity from the keychain.
        let id_out = std::process::Command::new("security")
            .args(["find-identity", "-v", "-p", "codesigning"])
            .output()
            .context("Failed to run `security find-identity`")?;
        let id_stdout = String::from_utf8_lossy(&id_out.stdout);

        // Lines look like: `  1) AABB...EE "Apple Development: Name (TEAM)"`
        let Some(signing_fp) = id_stdout
            .lines()
            .find(|l| l.contains(identity))
            .and_then(|l| {
                l.split_whitespace()
                    .find(|t| t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit()))
            })
            .map(str::to_ascii_uppercase)
        else {
            return Ok(());
        };

        for cert_val in certs {
            let Some(cert_data) = cert_val.as_data() else { continue };

            let mut child = std::process::Command::new("openssl")
                .args(["x509", "-inform", "DER", "-noout", "-fingerprint", "-sha1"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .context("Failed to run `openssl x509`")?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(cert_data);
            }
            let fp_out = child.wait_with_output().context("openssl x509 failed")?;
            let fp_str = String::from_utf8_lossy(&fp_out.stdout);
            // Output: "SHA1 Fingerprint=AA:BB:CC:..."
            if let Some(fp) = fp_str.split('=').nth(1) {
                let fp_clean: String = fp.trim().replace(':', "").to_ascii_uppercase();
                if fp_clean == signing_fp {
                    return Ok(());
                }
            }
        }

        bail!(
            "Signing identity {identity:?} is not authorized by the provisioning profile.\n\
             The profile was created with an older certificate.\n\
             Run: strudel profile fetch --force"
        );
    }

    fn find_simulator(&self, name: &str) -> Result<String> {
        if self.dry_run {
            return Ok(name.to_string());
        }
        let output = self
            .sh
            .run(&["xcrun", "simctl", "list", "devices", "available", "-j"])?;
        let parsed: SimctlDevicesOutput = serde_json::from_str(&output)
            .context("Failed to parse `xcrun simctl list devices` output")?;

        let mut exact: Option<String> = None;
        let mut fallback: Option<(String, String)> = None;

        for (runtime, devices) in &parsed.devices {
            if !runtime.contains("iOS") {
                continue;
            }
            for d in devices {
                if !d.is_available {
                    continue;
                }
                if d.name == name {
                    exact = Some(d.udid.clone());
                }
                if fallback.is_none() && d.name.starts_with("iPhone") {
                    fallback = Some((d.udid.clone(), d.name.clone()));
                }
            }
        }

        if let Some(udid) = exact {
            return Ok(udid);
        }
        if let Some((udid, found_name)) = fallback {
            cprintln!(
                "<yellow>warning:</yellow> Simulator \"{name}\" not found; using \"{found_name}\" instead."
            );
            return Ok(udid);
        }
        bail!(
            "No iOS simulators are available.\n\
             Install an iOS runtime with: xcodebuild -downloadPlatform iOS\n\
             List available simulators with: xcrun simctl list devices available"
        );
    }

    /// Return all connected iOS devices as `(udid, name)` pairs.
    fn list_connected_devices(&self) -> Result<Vec<(String, String)>> {
        if self.dry_run {
            return Ok(vec![(
                "<device-udid>".to_string(),
                "<device-name>".to_string(),
            )]);
        }
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
            .filter(|d| {
                let platform = d.hardware_properties.platform.as_deref().unwrap_or("");
                let tunnel = d
                    .connection_properties
                    .tunnel_state
                    .as_deref()
                    .unwrap_or("");
                let dev_mode = d
                    .device_properties
                    .developer_mode_status
                    .as_deref()
                    .unwrap_or("");
                platform == "iOS" && (tunnel == "connected" || dev_mode == "enabled")
            })
            .map(|d| {
                let udid = d.hardware_properties.udid;
                let name = d
                    .device_properties
                    .name
                    .unwrap_or_else(|| d.identifier.clone());
                (udid, name)
            })
            .collect())
    }

    fn ios_bin_dir(
        &self,
        swift: &str,
        config_flag: &str,
        triple: &str,
        sdk_path: &str,
    ) -> Result<PathBuf> {
        let source = self.cfg.source_dir.to_str().unwrap();
        let out = ShellCommand::new(swift)
            .args([
                "build",
                "-c",
                config_flag,
                "--triple",
                triple,
                "--sdk",
                sdk_path,
                "--package-path",
                source,
                "--show-bin-path",
            ])
            .hide_dry_run()
            .run(&self.sh)?;
        let dir = if out.trim().is_empty() {
            self.cfg
                .source_dir
                .join(format!(".build/{triple}/{config_flag}"))
        } else {
            PathBuf::from(out.trim())
        };
        Ok(dir)
    }
}

/// Decode a `.mobileprovision` file's CMS envelope and return the plist value.
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
             Run `strudel device register` to register your device(s)."
        ),
        1 => {
            let (udid, name) = registered.into_iter().next().unwrap();
            DeviceResolution::Single { udid, name }
        },
        _ => DeviceResolution::Prompt(registered),
    };

    Ok((resolution, unregistered))
}

pub fn decode_profile(profile_path: &Path) -> Result<plist::Value> {
    let profile_str = profile_path
        .to_str()
        .context("Invalid provisioning profile path")?;
    let output = std::process::Command::new("security")
        .args(["cms", "-D", "-i", profile_str])
        .output()
        .context("Failed to run `security cms`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to decode provisioning profile: {stderr}");
    }
    plist::Value::from_reader(Cursor::new(&output.stdout))
        .context("Failed to parse provisioning profile plist")
}

/// Return `true` when `profile_path` is a valid, current profile for the
/// given `required_udids`, `bundle_id`, and `team_id`. Returns `false` when:
/// the profile has expired (or expires within 5 minutes), any required UDID
/// is absent from `ProvisionedDevices`, or the `application-identifier`
/// entitlement does not match `<team_id>.<bundle_id>` (when `team_id` is set).
pub fn profile_is_current(
    profile_path: &Path,
    required_udids: &[&str],
    bundle_id: &str,
    team_id: &str,
) -> Result<bool> {
    if !profile_path.exists() {
        return Ok(false);
    }
    let profile = match decode_profile(profile_path) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let dict = match profile.as_dictionary() {
        Some(d) => d,
        None => return Ok(false),
    };

    // Expiration: must not expire within 5 minutes.
    if let Some(exp) = dict.get("ExpirationDate").and_then(|v| v.as_date()) {
        let sys_time = SystemTime::from(exp);
        let cutoff = SystemTime::now()
            .checked_add(Duration::from_secs(300))
            .unwrap_or_else(SystemTime::now);
        if sys_time <= cutoff {
            return Ok(false);
        }
    } else {
        return Ok(false);
    }

    // Device coverage: every required UDID must appear in ProvisionedDevices.
    if !required_udids.is_empty() {
        let provisioned: Vec<&str> = dict
            .get("ProvisionedDevices")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_string()).collect())
            .unwrap_or_default();
        for udid in required_udids {
            if !provisioned.contains(udid) {
                return Ok(false);
            }
        }
    }

    // application-identifier entitlement match (when team_id is set).
    if !team_id.is_empty() {
        let expected = format!("{team_id}.{bundle_id}");
        let actual = dict
            .get("Entitlements")
            .and_then(|v| v.as_dictionary())
            .and_then(|d| d.get("application-identifier"))
            .and_then(|v| v.as_string())
            .unwrap_or("");
        if actual != expected {
            return Ok(false);
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::devices::DeviceSet;

    use super::{DeviceResolution, resolve_connected};

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

    #[test]
    fn ios_app_bundle_path_is_flat() {
        let bundle = PathBuf::from("/out/ios-sim").join("MyApp.app");
        assert_eq!(bundle, PathBuf::from("/out/ios-sim/MyApp.app"));
        assert!(!bundle.to_str().unwrap().contains("Contents"));
    }

    #[test]
    fn profile_is_current_missing_file_returns_false() {
        let result = super::profile_is_current(
            std::path::Path::new("/nonexistent/path.mobileprovision"),
            &[],
            "com.example.app",
            "",
        )
        .unwrap();
        assert!(!result);
    }
}
