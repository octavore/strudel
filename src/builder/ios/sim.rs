use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use serde::Deserialize;

use crate::builder::ios::IosTarget;
use crate::builder::{IosBuilder, step};
use crate::shell::ShellCommand;

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

impl IosBuilder {
    /// Compile the iOS Simulator triple and assemble a `.app` bundle at
    /// `<build_dir>/ios-sim/<target>.app`. Shared by `strudel build` (assembly
    /// only) and `strudel run --sim` (assembly, then ad-hoc sign, install, and
    /// launch).
    fn build_sim_bundle(&self) -> Result<PathBuf> {
        let ios_settings = &self.ios;
        if !self.cfg.extensions.is_empty() {
            cprintln!(
                "<yellow>warning:</yellow> iOS extension bundling is not yet supported; \
                 [[extensions]] in this target will be ignored."
            );
        }
        let target = &self.cfg.target_name;
        let config_flag = if self.debug { "debug" } else { "release" };
        let deployment = &ios_settings.deployment_target;

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

        step("Building for iOS Simulator...");
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
        self.assemble_ios_bundle(&binary, &app_bundle, IosTarget::Simulator)?;

        Ok(app_bundle)
    }

    /// `strudel build` for an iOS target: assemble a `.app` for the Simulator
    /// triple. No signing, install, or launch — see `run --sim` for that.
    pub fn build(&self) -> Result<()> {
        let app_bundle = self.build_sim_bundle()?;
        println!();
        cprintln!("<green>Done! App bundle:</green>");
        cprintln!("<cyan>{}</cyan>", app_bundle.display());
        Ok(())
    }

    /// Build for the iOS Simulator and launch in Simulator.app.
    pub fn sim(&self, sim_override: Option<&str>) -> Result<()> {
        let ios_settings = &self.ios;
        let sim_name = sim_override.unwrap_or(&ios_settings.simulator);
        let app_bundle = self.build_sim_bundle()?;

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
}
