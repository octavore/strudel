use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use serde_json::{Map, Value, json};

use crate::builder::ios::IosTarget;
use crate::builder::{IosBuilder, step};
use crate::config::ResolvedIcon;
use crate::icon::{ios, render};
use crate::shell::ShellCommand;

impl IosBuilder {
    /// Assemble a flat iOS `.app` bundle (no `Contents/` subdirectory).
    /// Generates `Info.plist` from `info_json_path` (if set) merged with
    /// required iOS keys. Optionally compiles the asset catalog.
    pub(super) fn assemble_ios_bundle(
        &self,
        binary: &Path,
        app_bundle: &Path,
        target: IosTarget,
    ) -> Result<()> {
        let ios_settings = &self.ios;
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
            json!(&ios_settings.deployment_target),
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

        // `assets_dir` is a full hand-authored catalog, so it takes
        // precedence over a generated/copied `icon` if both are set. Either
        // path emits a partial Info.plist fragment (`CFBundleIconName` /
        // `CFBundleIcons`) that must be merged in *before* Info.plist is
        // written below, or iOS won't know the compiled icon exists.
        let icon_plist_keys = if let Some(assets_dir) = &ios_settings.assets_dir {
            self.compile_ios_assets(assets_dir, app_bundle, target)?
        } else if let Some(icon) = &self.cfg.icon {
            self.compile_ios_icon(icon, app_bundle, target)?
        } else {
            Map::new()
        };
        obj.extend(icon_plist_keys);

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

        Ok(())
    }

    fn compile_ios_assets(
        &self,
        assets_dir: &Path,
        app_bundle: &Path,
        target: IosTarget,
    ) -> Result<Map<String, Value>> {
        step("Compiling asset catalog...");
        let icon_name = self.ios.app_icon_name.clone();
        let partial_plist_path = app_bundle
            .parent()
            .unwrap_or(Path::new("."))
            .join("actool-partial-info.plist");
        let result = self.run_actool(
            assets_dir,
            &icon_name,
            &[],
            app_bundle,
            target,
            &partial_plist_path,
        );
        let _ = fs::remove_file(&partial_plist_path);
        result
    }

    /// Compile a bundle icon configured via `[build.icon]` into the iOS
    /// asset catalog. If `icon` already points to an Icon Composer `.icon`
    /// bundle, it's handed to `actool` directly; otherwise a flat raster
    /// image is rendered (or copied) and wrapped in a minimal
    /// `.appiconset`/`.xcassets` so `actool` can derive the full icon set
    /// from that single source image via `--include-all-app-icons`.
    fn compile_ios_icon(
        &self,
        icon: &ResolvedIcon,
        app_bundle: &Path,
        target: IosTarget,
    ) -> Result<Map<String, Value>> {
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> generate app icon and compile via actool");
            return Ok(Map::new());
        }
        step("Generating app icon...");

        let work_dir = app_bundle
            .parent()
            .unwrap_or(Path::new("."))
            .join("AppIcon-actool-input");
        if work_dir.exists() {
            fs::remove_dir_all(&work_dir)?;
        }

        let (input_dir, icon_name) = self.prepare_ios_icon_input(icon, &work_dir)?;
        let partial_plist_path = work_dir.join("partial-info.plist");
        let result = self.run_actool(
            &input_dir,
            &icon_name,
            &["--include-all-app-icons"],
            app_bundle,
            target,
            &partial_plist_path,
        );
        let _ = fs::remove_dir_all(&work_dir);
        result
    }

    /// Resolve `icon` to an `actool`-ready input directory and the
    /// `--app-icon` name to compile from it. Icon Composer `.icon` bundles
    /// are used in place; anything else is rendered to a flat PNG and
    /// wrapped in a synthesized `.xcassets`/`.appiconset` under `work_dir`.
    fn prepare_ios_icon_input(
        &self,
        icon: &ResolvedIcon,
        work_dir: &Path,
    ) -> Result<(PathBuf, String)> {
        if let ResolvedIcon::Path { path, .. } = icon
            && path.extension().and_then(|e| e.to_str()) == Some("icon")
        {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .with_context(|| {
                    format!("Icon Composer bundle has no file name: {}", path.display())
                })?
                .to_string();
            return Ok((path.clone(), name));
        }

        let source_image = render::render_ios_icon(icon, 1024)?;

        let icon_name = "AppIcon";
        let xcassets_dir = work_dir.join("Assets.xcassets");
        let appiconset_dir = xcassets_dir.join(format!("{icon_name}.appiconset"));
        fs::create_dir_all(&xcassets_dir)?;
        fs::write(
            xcassets_dir.join("Contents.json"),
            serde_json::to_vec_pretty(&json!({ "info": { "author": "xcode", "version": 1 } }))?,
        )?;

        ios::write_appiconset(&source_image, &appiconset_dir)?;

        Ok((xcassets_dir, icon_name.to_string()))
    }

    /// Shared `xcrun actool` invocation used to compile an asset catalog
    /// (whether a user-provided `.xcassets` or one synthesized from
    /// `[build.icon]`) into `app_bundle`. Returns the keys from `actool`'s
    /// partial Info.plist output (`CFBundleIconName`/`CFBundleIcons`), which
    /// the caller must merge into the bundle's real Info.plist for the icon
    /// to actually be picked up.
    fn run_actool(
        &self,
        assets_dir: &Path,
        icon_name: &str,
        extra_args: &[&str],
        app_bundle: &Path,
        target: IosTarget,
        partial_plist_path: &Path,
    ) -> Result<Map<String, Value>> {
        let ios_settings = &self.ios;
        let assets_str = assets_dir.to_str().unwrap();
        let bundle_str = app_bundle.to_str().unwrap();
        let deployment = &ios_settings.deployment_target;
        let partial_plist_str = partial_plist_path.to_str().unwrap();

        let platform = match target {
            IosTarget::Simulator => "iphonesimulator",
            IosTarget::Device => "iphoneos",
        };

        self.sh.run(
            ShellCommand::new("xcrun")
                .args([
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
                    partial_plist_str,
                ])
                .args(extra_args.iter().copied()),
        )?;

        if self.dry_run {
            return Ok(Map::new());
        }
        read_partial_info_plist(partial_plist_path)
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

/// Read `actool`'s `--output-partial-info-plist` output (an XML plist
/// fragment, typically just `CFBundleIconName`/`CFBundleIcons`) and decode
/// it straight into JSON via `plist`'s serde support, so it can be merged
/// into the bundle's Info.plist JSON object - consistent with how the rest
/// of the codebase reads plists (`plist::Value::from_reader` in
/// `builder::macos::validators`/`builder::ios::profile`) rather than
/// shelling out to `plutil`.
fn read_partial_info_plist(path: &Path) -> Result<Map<String, Value>> {
    let value: Value = plist::from_file(path)
        .with_context(|| format!("Failed to read partial Info.plist at {}", path.display()))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => bail!(
            "Partial Info.plist at {} is not a dictionary",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::builder::IosBuilder;
    use crate::config::fixtures::RESOLVED;

    #[test]
    fn ios_app_bundle_path_is_flat() {
        let bundle = PathBuf::from("/out/ios-sim").join("MyApp.app");
        assert_eq!(bundle, PathBuf::from("/out/ios-sim/MyApp.app"));
        assert!(!bundle.to_str().unwrap().contains("Contents"));
    }

    fn builder() -> IosBuilder {
        IosBuilder::new(RESOLVED.clone(), true, false).unwrap()
    }

    #[test]
    fn prepare_icon_composer_bundle_used_in_place() {
        let dir = tempdir().unwrap();
        let icon_bundle = dir.path().join("AppIcon.icon");
        std::fs::create_dir_all(&icon_bundle).unwrap();

        let icon = ResolvedIcon::Path {
            path: icon_bundle.clone(),
            icns: false,
        };
        let work_dir = dir.path().join("work");
        let (input_dir, icon_name) = builder().prepare_ios_icon_input(&icon, &work_dir).unwrap();
        assert_eq!(input_dir, icon_bundle);
        assert_eq!(icon_name, "AppIcon");
        assert!(!work_dir.exists());
    }

    #[test]
    fn prepare_plain_image_synthesizes_appiconset() {
        let dir = tempdir().unwrap();
        let src_png = dir.path().join("icon.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&src_png)
            .unwrap();

        let icon = ResolvedIcon::Path {
            path: src_png,
            icns: false,
        };
        let work_dir = dir.path().join("work");
        let (input_dir, icon_name) = builder().prepare_ios_icon_input(&icon, &work_dir).unwrap();
        assert_eq!(icon_name, "AppIcon");
        assert_eq!(input_dir, work_dir.join("Assets.xcassets"));
        assert!(input_dir.join("Contents.json").is_file());
        assert!(input_dir.join("AppIcon.appiconset/Contents.json").is_file());
        assert!(
            input_dir
                .join("AppIcon.appiconset/icon-60x60@3x.png")
                .is_file()
        );
        assert!(
            input_dir
                .join("AppIcon.appiconset/icon-1024x1024@1x.png")
                .is_file()
        );

        let contents: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(input_dir.join("AppIcon.appiconset/Contents.json")).unwrap(),
        )
        .unwrap();
        let images = contents["images"].as_array().unwrap();
        // 8 iPhone size/scale renditions + the ios-marketing 1024x1024.
        assert_eq!(images.len(), 9);
    }
}
