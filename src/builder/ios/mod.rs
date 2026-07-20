//! The iOS build pipeline. The [`IosBuilder`](super::IosBuilder) methods are
//! split across submodules by concern:
//!
//! - [`sim`] Simulator builds: compile, install, and launch in Simulator.app
//! - [`device`] device builds, code-signing, and identity/profile checks
//! - [`profile`] provisioning-profile fetch, caching, and validity checks
//! - [`registration`] device discovery, registration, and target resolution
//! - [`bundle`] `.app` bundle assembly, asset compilation, and bin paths

mod bundle;
mod device;
mod profile;
mod registration;
mod sim;

use std::path::PathBuf;

use anyhow::Result;
use color_print::cprintln;
pub use profile::decode_profile;

use crate::builder::{IosBuilder, step};
use crate::shell::ShellCommand;

/// Simulator or device target. This selects the SDK, triple suffix, and
/// platform keys that go into the iOS `Info.plist`.
enum IosTarget {
    Simulator,
    Device,
}

impl IosBuilder {
    /// Compile the Simulator or device triple and assemble a `.app` bundle
    /// at `<build_dir>/ios-sim|ios-device/<target>.app`. Shared by device
    /// builds, `strudel build` (Simulator assembly only), and `run --sim`
    /// (Simulator assembly, then ad-hoc sign, install, and launch).
    fn compile_and_assemble(&self, target: IosTarget) -> Result<PathBuf> {
        if !self.cfg.extensions.is_empty() {
            cprintln!(
                "<yellow>warning:</yellow> iOS extension bundling is not yet supported; \
                 [[extensions]] in this target will be ignored."
            );
        }

        let target_name = &self.cfg.target_name;
        let config_flag = if self.debug { "debug" } else { "release" };
        let deployment = &self.ios.deployment_target;

        let (sdk, triple, bundle_dir_name, build_label, assemble_label) = match target {
            IosTarget::Device => (
                "iphoneos",
                format!("arm64-apple-ios{deployment}"),
                "ios-device",
                "Building for iOS device...",
                "Assembling iOS device bundle...",
            ),
            IosTarget::Simulator => {
                let host_arch = match std::env::consts::ARCH {
                    "aarch64" => "arm64",
                    other => other,
                };
                (
                    "iphonesimulator",
                    format!("{host_arch}-apple-ios{deployment}-simulator"),
                    "ios-sim",
                    "Building for iOS Simulator...",
                    "Assembling iOS Simulator bundle...",
                )
            },
        };

        let sdk_path = self
            .sh
            .run(&["xcrun", "--sdk", sdk, "--show-sdk-path"])
            .map(|s| {
                if s.is_empty() {
                    format!("<{sdk}-sdk>")
                } else {
                    s
                }
            })?;
        let swift = self
            .sh
            .run(&["xcrun", "-f", "swift"])
            .map(|s| if s.is_empty() { "swift".into() } else { s })?;

        step(build_label);
        let source = self.cfg.source_dir.to_str().unwrap();
        let mut build_cmd = ShellCommand::new(&swift)
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
            .envs(&self.cfg.build_env);
        if !self.cfg.embed_libs.is_empty() {
            build_cmd = build_cmd.arg_group([
                "-Xlinker",
                "-rpath",
                "-Xlinker",
                "@executable_path/Frameworks",
            ]);
        }
        self.sh.run_streamed_env(build_cmd)?;

        let bin_dir = self.ios_bin_dir(&swift, config_flag, &triple, &sdk_path)?;
        let binary = self.find_binary_in(&bin_dir, target_name)?;

        step(assemble_label);
        let bundle_dir = self.paths.build_dir.join(bundle_dir_name);
        let app_bundle = bundle_dir.join(format!("{target_name}.app"));
        self.assemble_ios_bundle(&binary, &app_bundle, target)?;

        Ok(app_bundle)
    }
}
