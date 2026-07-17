//! App and extension bundle assembly: [`MacosBuilder::assemble_bundle`],
//! [`MacosBuilder::assemble_appex`], and associated helpers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use color_print::cprintln;
use serde_json::{Value, json};

use crate::builder::{MacosBuilder, step};
use crate::config::{ExtensionKind, ResolvedExtension, ResolvedIcon};
use crate::icon::icns;
use crate::icon::render::render_to_png;
use crate::paths::ExtensionPaths;
use crate::shell::ShellCommand;

impl MacosBuilder {
    // assemble the .app bundle by creating the bundle structure:
    // 1. copy in the binary and other resources
    // 2. generate the Info.plist from the info JSON
    pub fn assemble_bundle(&self, binary_path: &Path) -> Result<PathBuf> {
        step("Assembling app bundle...");
        let app_bundle = &self.paths.app_bundle;
        let app_bundle_resources_dir = app_bundle.join("Contents/Resources");

        // create structure
        self.create_dir(&app_bundle.join("Contents/MacOS"))?;
        self.create_dir(&app_bundle_resources_dir)?;

        // copy the binary into the bundle
        self.copy_file(
            binary_path,
            &app_bundle.join("Contents/MacOS").join(&self.cfg.app_name),
        )?;

        // Read info JSON for Info.plist and extract the top-level object for inserting
        // keys.
        let mut info_json = self.build_info_json(
            self.cfg.info_json_path.clone(),
            HashMap::from([
                ("CFBundleExecutable".into(), self.cfg.app_name.clone()),
                ("CFBundleIdentifier".into(), self.cfg.bundle_id.clone()),
                ("CFBundlePackageType".into(), "APPL".into()),
            ]),
        )?;

        // set CFBundleIconFile if a bundle icon is configured
        // todo: also support CFBundleIconName, the newer format.
        // see https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/CoreFoundationKeys.html
        if let Some(icon) = &self.cfg.icon {
            self.write_icon(icon, &app_bundle_resources_dir.join("AppIcon.icns"))?;
            info_json.insert("CFBundleIconFile".to_string(), json!("AppIcon.icns"));
        }

        // pipe JSON into plutil to produce Info.plist
        let json_bytes = serde_json::to_vec_pretty(&info_json)?;
        let plist_path = self
            .paths
            .info_plist
            .to_str()
            .context("Invalid Info.plist path.")?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        // copy provisioning profile if provided (for non-app-store macos apps)
        // macOS expects MyApp.app/Contents/embedded.provisionprofile
        // see: https://developer.apple.com/Documentation/technotes/tn3125-inside-code-signing-provisioning-profiles
        if let Some(profile_path) = &self.cfg.provisioning_profile {
            self.copy_file(
                profile_path,
                &app_bundle.join("Contents/embedded.provisionprofile"),
            )?;
        }

        // Copy user-configured resources_dir contents into Contents/Resources/.
        if let Some(rdir) = &self.cfg.resources_dir {
            step("Copying resource directory...");
            self.copy_tree(rdir, &app_bundle_resources_dir)?;
        }

        // Copy individual user-configured resource files and folders into
        // Contents/Resources/.
        if !self.cfg.resources.is_empty() {
            step("Copying resources...");
            for resource in &self.cfg.resources {
                let name = resource.file_name().with_context(|| {
                    format!("Resource path has no filename: {}", resource.display())
                })?;
                let dest = app_bundle_resources_dir.join(name);
                if resource.is_dir() {
                    self.copy_tree(resource, &dest)?;
                } else {
                    self.copy_file(resource, &dest)?;
                }
            }
        }

        // Compile a user-configured `.xcassets` catalog into
        // `Contents/Resources/Assets.car`.
        if let Some(assets_dir) = &self.macos.assets_dir {
            self.compile_macos_assets(assets_dir, &app_bundle_resources_dir)?;
        }

        Ok(app_bundle.clone())
    }

    /// Compile `assets_dir` (a `.xcassets` catalog) into
    /// `Assets.car` inside `resources_dir` via `xcrun actool`.
    fn compile_macos_assets(&self, assets_dir: &Path, resources_dir: &Path) -> Result<()> {
        step("Compiling asset catalog...");

        let deployment_target = self.macos_deployment_target()?;
        let assets_str = assets_dir
            .to_str()
            .with_context(|| format!("Invalid assets_dir path: {}", assets_dir.display()))?;
        let resources_str = resources_dir.to_str().context(
            "Contents/Resources path is not valid UTF-8, cannot pass to `actool --compile`",
        )?;
        let partial_plist_path = resources_dir
            .parent()
            .unwrap_or(Path::new("."))
            .join("actool-partial-info.plist");
        let partial_plist_str = partial_plist_path.to_str().unwrap();

        self.sh.run(ShellCommand::new("xcrun").args([
            "actool",
            assets_str,
            "--compile",
            resources_str,
            "--platform",
            "macosx",
            "--minimum-deployment-target",
            &deployment_target,
            "--output-partial-info-plist",
            partial_plist_str,
        ]))?;

        let _ = fs::remove_file(&partial_plist_path);
        Ok(())
    }

    /// The macOS deployment target `actool` should compile assets for, read
    /// from `Package.swift`'s `platforms: [.macOS(.vXX)]` declaration via
    /// `swift package dump-package`. Falls back to `"14.0"` when the
    /// manifest declares no macOS platform minimum (a valid, common case).
    fn macos_deployment_target(&self) -> Result<String> {
        if self.dry_run {
            return Ok("<package.swift:macos>".to_string());
        }

        let source = self
            .cfg
            .source_dir
            .to_str()
            .context("source_dir is not valid UTF-8")?;
        let out = ShellCommand::new("swift")
            .args(["package", "dump-package", "--package-path", source])
            .hide_dry_run()
            .run(&self.sh)?;
        parse_macos_deployment_target(&out)
    }

    /// Produce the bundle's `AppIcon.icns` at `dest`.
    ///
    /// For [`ResolvedIcon::Path`], copied unmodified unless `icns` is set, in
    /// which case it's converted via [`icns::make_icns`].
    ///
    /// For [`ResolvedIcon::Generated`], composited from source image into a
    /// squircle via the `icon` crate and written out as a single PNG,
    /// unless `icns` is set, in which case the composited image is
    /// converted into a multi-resolution `.icns`.
    fn write_icon(&self, icon: &ResolvedIcon, dest: &Path) -> Result<()> {
        match icon {
            ResolvedIcon::Path {
                path,
                icns: to_icns,
            } => {
                if !to_icns {
                    return self.copy_file(path, dest);
                }
                if self.dry_run {
                    cprintln!(
                        "<dim>[dry-run]</dim> convert icon <blue>{}</blue> -> <blue>{}</blue> (.icns)",
                        path.display(),
                        dest.display()
                    );
                    return Ok(());
                }
                step("Converting app icon to .icns...");
                icns::make_icns(path, dest)
            },
            ResolvedIcon::Generated {
                src, icns: to_icns, ..
            } => {
                if self.dry_run {
                    cprintln!(
                        "<dim>[dry-run]</dim> generate icon <blue>{}</blue> -> <blue>{}</blue>",
                        src.display(),
                        dest.display()
                    );
                    return Ok(());
                }

                step("Generating app icon...");

                if !to_icns {
                    return render_to_png(icon, dest);
                }

                let tmp_png = dest
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("AppIcon-generated.png");
                render_to_png(icon, &tmp_png)?;
                let result = icns::make_icns(&tmp_png, dest);
                let _ = fs::remove_file(&tmp_png);
                result
            },
        }
    }

    /// Assemble one app extension `.appex` bundle under the host's
    /// `Contents/PlugIns/`:
    /// 1. Builds the extension `Info.plist` (injecting kind-specific
    ///    `NSExtension` keys)
    /// 2. Copies the binary
    /// 3. Copies the kind-specific resource payload (e.g. for Safari Web
    ///    Extensions, this might be the entire webpack output directory).
    pub fn assemble_appex(
        &self,
        ext: &ResolvedExtension,
        paths: &ExtensionPaths,
        bin_dir: &Path,
    ) -> Result<()> {
        step(&format!("Assembling extension `{}`...", ext.name));

        // Locate the extension binary that `swift build` produced. Building
        // all package targets is intentional: a Package.swift that declares
        // both the host and the extension as executableTargets produces both
        // binaries from a single `swift build` invocation.
        let binary_path = self.find_binary_in(bin_dir, &ext.target_name)?;

        self.create_dir(&paths.appex.join("Contents/MacOS"))?;
        self.create_dir(&paths.resources)?;
        self.copy_file(&binary_path, &paths.binary)?;

        let mut info_json = self.build_info_json(
            ext.info_json_path.clone(),
            HashMap::from([
                ("CFBundleExecutable".into(), ext.target_name.clone()),
                ("CFBundleIdentifier".into(), ext.bundle_id.clone()),
                ("CFBundleName".into(), ext.name.clone()),
                ("CFBundleDisplayName".into(), ext.name.clone()),
                ("CFBundleInfoDictionaryVersion".into(), "6.0".into()),
                // App extensions use the XPC bundle package type.
                ("CFBundlePackageType".into(), "XPC!".into()),
            ]),
        )?;

        match ext.kind {
            ExtensionKind::SafariWebExtension => {
                // `NSExtensionPrincipalClass` was filled in during resolve (todo: clean up, a
                // little spooky?)
                let principal = ext.principal_class.as_deref().unwrap_or("");
                info_json.insert(
                    "NSExtension".into(),
                    json!({
                        "NSExtensionPointIdentifier": "com.apple.Safari.web-extension",
                        "NSExtensionPrincipalClass": principal,
                        "SFSafariWebExtensionManifestPath": "Resources/manifest.json",
                    }),
                );
            },
            ExtensionKind::AppExtension => {
                let ident = ext.extension_point_identifier.as_deref().unwrap_or("");
                let mut ns_ext = serde_json::Map::new();
                ns_ext.insert("NSExtensionPointIdentifier".into(), json!(ident));
                if let Some(class) = ext.principal_class.as_deref() {
                    ns_ext.insert("NSExtensionPrincipalClass".into(), json!(class));
                } else {
                    cprintln!(
                        "<yellow>warning:</yellow> App Extension `{}` is missing `principal_class`, which may be required depending on the extension point.",
                        ext.name
                    );
                }
                info_json.insert("NSExtension".into(), Value::Object(ns_ext));
            },
        }

        let json_bytes = serde_json::to_vec_pretty(&info_json)?;
        let plist_path = paths
            .info_plist
            .to_str()
            .context("Invalid extension Info.plist path.")?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        // Copy the kind-specific resource payload. For Safari Web Extensions
        // this is the extension source code (manifest.json, JS, HTML, icons).
        if let Some(src_resources) = &ext.resources_dir {
            self.copy_tree(src_resources, &paths.resources)?;
        }

        Ok(())
    }

    pub(super) fn build_info_json(
        &self,
        path: Option<PathBuf>,
        additional_data: HashMap<String, String>,
    ) -> Result<serde_json::Map<String, Value>> {
        let mut info_json = match path {
            Some(path) => {
                let path_str = path.display();
                fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read {path_str}."))
                    .and_then(|s| {
                        serde_json::from_str::<serde_json::Map<String, Value>>(&s)
                            .with_context(|| format!("Failed to parse {path_str}, must be a map."))
                    })?
            },
            None => serde_json::Map::new(),
        };
        for (k, v) in additional_data {
            info_json.insert(k, json!(v));
        }
        info_json.insert(
            "CFBundleShortVersionString".to_string(),
            json!(self.cfg.version.clone()),
        );
        info_json.insert(
            "CFBundleVersion".to_string(),
            json!(self.cfg.build_number.clone()),
        );
        Ok(info_json)
    }
}

/// Pull the macOS deployment target out of `swift package dump-package`'s
/// JSON output (its `platforms: [{platformName, version}]` array). Falls
/// back to `"14.0"` when the manifest declares no macOS platform minimum.
fn parse_macos_deployment_target(dump_package_json: &str) -> Result<String> {
    let manifest: Value = serde_json::from_str(dump_package_json)
        .context("Failed to parse `swift package dump-package` output as JSON")?;
    let version = manifest["platforms"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["platformName"] == "macos")
        .and_then(|p| p["version"].as_str());
    Ok(version.unwrap_or("14.0").to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_macos_deployment_target;

    #[test]
    fn deployment_target_reads_macos_platform_version() {
        let json = r#"{"platforms":[{"platformName":"macos","version":"13.0","options":[]}]}"#;
        assert_eq!(parse_macos_deployment_target(json).unwrap(), "13.0");
    }

    #[test]
    fn deployment_target_ignores_other_platforms() {
        let json = r#"{"platforms":[{"platformName":"ios","version":"17.0","options":[]}]}"#;
        assert_eq!(parse_macos_deployment_target(json).unwrap(), "11.0");
    }

    #[test]
    fn deployment_target_falls_back_when_platforms_absent() {
        let json = r#"{"platforms":[]}"#;
        assert_eq!(parse_macos_deployment_target(json).unwrap(), "11.0");
    }

    #[test]
    fn deployment_target_rejects_invalid_json() {
        assert!(parse_macos_deployment_target("not json").is_err());
    }
}
