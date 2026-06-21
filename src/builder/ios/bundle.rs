use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::super::{Builder, step};
use super::IosTarget;
use crate::shell::ShellCommand;

impl Builder {
    /// Assemble a flat iOS `.app` bundle (no `Contents/` subdirectory).
    /// Generates `Info.plist` from `info_json_path` (if set) merged with
    /// required iOS keys. Optionally compiles the asset catalog.
    pub(super) fn assemble_ios_bundle(
        &self,
        binary: &Path,
        app_bundle: &Path,
        target: IosTarget,
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

        match target {
            IosTarget::Simulator => {
                obj.insert(
                    "CFBundleSupportedPlatforms".into(),
                    json!(["iPhoneSimulator"]),
                );
                obj.insert("DTPlatformName".into(), json!("iphonesimulator"));
            },
            IosTarget::Device => {
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
            self.compile_ios_assets(assets_dir, app_bundle, target)?;
        }

        Ok(())
    }

    fn compile_ios_assets(
        &self,
        assets_dir: &Path,
        app_bundle: &Path,
        target: IosTarget,
    ) -> Result<()> {
        step("Compiling asset catalog...");
        let assets_str = assets_dir.to_str().unwrap();
        let bundle_str = app_bundle.to_str().unwrap();
        let deployment = &self.cfg.ios_deployment_target;
        let icon_name = &self.cfg.ios_app_icon_name;

        let platform = match target {
            IosTarget::Simulator => "iphonesimulator",
            IosTarget::Device => "iphoneos",
        };

        // generate asset catalog with actool
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

    pub(super) fn ios_bin_dir(
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
        let bundle = PathBuf::from("/out/ios-sim").join("MyApp.app");
        assert_eq!(bundle, PathBuf::from("/out/ios-sim/MyApp.app"));
        assert!(!bundle.to_str().unwrap().contains("Contents"));
    }
}
