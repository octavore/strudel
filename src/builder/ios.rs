use std::collections::HashMap;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Builder, step};
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
    ///
    /// Uses `swift build --triple --sdk` (via `xcrun -f swift` to avoid the
    /// `Wincompatible-sysroot` warning), assembles a flat `.app` bundle, ad-hoc
    /// signs it, and installs/launches via `xcrun simctl`.
    pub fn sim(&self, sim_override: Option<&str>) -> Result<()> {
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

    /// Build for a connected iOS device, then install and launch it.
    ///
    /// Requires a provisioning profile (set `provisioning_profile` in
    /// `[build]`). Extracts entitlements directly from the profile so the
    /// signature matches exactly. Requires Xcode 15+ for `xcrun devicectl`.
    pub fn device(&self, device_override: Option<&str>) -> Result<()> {
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

        // Provisioning profile — required for device signing.
        let profile_path = self.cfg.provisioning_profile.as_ref().context(
            "A provisioning profile is required for device builds.\n\
             Set `provisioning_profile` in the `[build]` section of strudel.toml.",
        )?;

        step("Embedding provisioning profile...");
        self.copy_file(profile_path, &app_bundle.join("embedded.mobileprovision"))?;

        step("Signing device bundle...");
        let identity = if self.cfg.sign_identity.is_empty() {
            "Apple Development"
        } else {
            &self.cfg.sign_identity
        };
        self.sign_ios_device(&app_bundle, profile_path, identity)?;

        // Resolve target device.
        let device_id = match device_override
            .map(str::to_string)
            .or_else(|| self.cfg.ios_device.clone())
        {
            Some(d) => d,
            None => self.find_connected_device()?,
        };

        step(&format!("Installing on {device_id}..."));
        let app_str = app_bundle.to_str().unwrap();
        self.sh.run(&[
            "xcrun",
            "devicectl",
            "device",
            "install",
            "app",
            "--device",
            &device_id,
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
            &device_id,
            &self.cfg.bundle_id,
        ])?;

        println!();
        cprintln!("<green>Done!</green> App installed and launched on device.");
        Ok(())
    }

    // ── Bundle assembly ────────────────────────────────────────────────────────

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

        // Copy the binary flat into the bundle root.
        self.copy_file(binary, &app_bundle.join(&self.cfg.target_name))?;

        // Build Info.plist from user JSON (if any) plus auto-injected iOS keys.
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

        // Compile asset catalog if configured.
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

        // `xcrun actool` is noisy on success; capture its output silently and
        // only surface it when the command fails.
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

    // ── Device signing ─────────────────────────────────────────────────────────

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
        // Decode the provisioning profile's CMS envelope and extract the
        // Entitlements plist. The same approach used by build-device.sh.
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

        let profile_str = profile_path
            .to_str()
            .context("Invalid provisioning profile path.")?;
        let output = std::process::Command::new("security")
            .args(["cms", "-D", "-i", profile_str])
            .output()
            .context("Failed to run `security cms`")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to decode provisioning profile: {stderr}");
        }

        let profile_plist = plist::Value::from_reader(Cursor::new(&output.stdout))
            .context("Failed to parse provisioning profile")?;
        let entitlements = profile_plist
            .as_dictionary()
            .and_then(|d| d.get("Entitlements"))
            .context("Provisioning profile has no Entitlements key")?;

        // Write the extracted entitlements plist next to the bundle.
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
        std::process::Command::new("codesign")
            .args(["--verify", "--deep", "--strict", "--verbose=2", bundle_str])
            .status()
            .context("Failed to run codesign --verify")?;

        Ok(())
    }

    /// Find the UDID of an available iOS simulator matching `name`.
    /// Falls back to the first available iPhone simulator on any iOS runtime
    /// when the named device isn't found, with a warning.
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
        let mut fallback: Option<(String, String)> = None; // (udid, name)

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

    /// Find the identifier of a connected iOS device (filtered by
    /// `platform=iOS` and tunnel/developer-mode state). Used when `--device`
    /// and `[ios] device` are both unset. Prompts the user to choose when
    /// multiple devices are connected.
    fn find_connected_device(&self) -> Result<String> {
        if self.dry_run {
            return Ok("<device-udid>".to_string());
        }
        step("Detecting connected iOS device...");
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

        let devices: Vec<(String, String)> = parsed
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
                let name = d
                    .device_properties
                    .name
                    .unwrap_or_else(|| d.identifier.clone());
                (d.identifier, name)
            })
            .collect();

        match devices.as_slice() {
            [] => bail!(
                "No connected iOS devices found.\n\
                 Plug in your iPhone, trust this Mac, and enable Developer Mode \
                 (Settings → Privacy & Security → Developer Mode).\n\
                 List devices with: xcrun devicectl list devices"
            ),
            [(id, name)] => {
                cprintln!("<green>✔</green> Found device: {name}");
                Ok(id.clone())
            },
            _ => {
                cprintln!("<bold>Multiple devices connected. Choose one:</bold>");
                for (i, (_, name)) in devices.iter().enumerate() {
                    cprintln!("  <bold>{}</bold>. {name}", i + 1);
                }
                loop {
                    print!("Device [1-{}]: ", devices.len());
                    io::stdout().flush()?;
                    let mut line = String::new();
                    io::stdin().read_line(&mut line)?;
                    let n: usize = line.trim().parse().unwrap_or(0);
                    if n >= 1 && n <= devices.len() {
                        let (id, name) = &devices[n - 1];
                        cprintln!("<green>✔</green> Using device: {name}");
                        return Ok(id.clone());
                    }
                    cprintln!(
                        "<red>error:</red> Enter a number between 1 and {}.",
                        devices.len()
                    );
                }
            },
        }
    }

    /// Run `swift build --show-bin-path` with the same flags used for the real
    /// build to locate the output directory.
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn ios_app_bundle_path_is_flat() {
        // iOS bundles live directly in <build_dir>/ios-sim/<target>.app —
        // no Contents/ subdirectory (unlike macOS).
        let bundle = PathBuf::from("/out/ios-sim").join("MyApp.app");
        assert_eq!(bundle, PathBuf::from("/out/ios-sim/MyApp.app"));
        assert!(!bundle.to_str().unwrap().contains("Contents"));
    }
}
