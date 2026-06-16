use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::{ResolvedConfig, ResolvedExtension};

pub struct PendingSubmission {
    pub dir: PathBuf,
    pub dmg: PathBuf,
    pub state: PathBuf,
}

pub struct Paths {
    pub build_dir: PathBuf,
    pub app_bundle: PathBuf,
    pub dmg: PathBuf,
    pub info_plist: PathBuf,
    pub entitlements_plist: PathBuf,
    pub strudel_dir: PathBuf,
    pub strudel_temp_dmg: PathBuf,
    pub dmg_staging: PathBuf,
    /// Cached provisioning profile: `.strudel/<bundle_id>.mobileprovision`.
    pub cached_profile: PathBuf,
    /// Tracked device set: `.strudel/devices.toml`.
    pub devices_toml: PathBuf,
    /// One entry per [`ResolvedExtension`], in the same order. Empty when no
    /// extensions are configured.
    pub extensions: Vec<ExtensionPaths>,
}

/// All bundle-internal paths for a single app extension. The `.appex` lives at
/// `<host>.app/Contents/PlugIns/<name>.appex/`.
pub struct ExtensionPaths {
    pub appex: PathBuf,
    pub binary: PathBuf,
    pub info_plist: PathBuf,
    pub resources: PathBuf,
    /// Generated plist for codesign — lives next to the host's
    /// `Entitlements.plist` in the build dir.
    pub entitlements_plist: PathBuf,
}

impl ExtensionPaths {
    fn for_extension(
        app_bundle: &std::path::Path,
        build_dir: &std::path::Path,
        ext: &ResolvedExtension,
    ) -> Self {
        let appex = app_bundle
            .join("Contents/PlugIns")
            .join(format!("{}.appex", ext.name));
        ExtensionPaths {
            binary: appex.join("Contents/MacOS").join(&ext.target_name),
            info_plist: appex.join("Contents/Info.plist"),
            resources: appex.join("Contents/Resources"),
            entitlements_plist: build_dir.join(format!("{}.entitlements.plist", ext.name)),
            appex,
        }
    }
}

impl Paths {
    pub fn new(cfg: &ResolvedConfig) -> Self {
        let ResolvedConfig {
            build_dir,
            source_dir,
            app_name,
            bundle_id,
            version,
            extensions,
            ..
        } = cfg;
        let app_bundle = build_dir.join(format!("{app_name}.app"));
        let dmg_name = format!("{app_name}-{version}.dmg");
        let strudel_dir = source_dir.join(".strudel");
        let extension_paths = extensions
            .iter()
            .map(|ext| ExtensionPaths::for_extension(&app_bundle, build_dir, ext))
            .collect();
        Paths {
            strudel_temp_dmg: strudel_dir.join(&dmg_name),
            dmg_staging: strudel_dir.join("dmg-staging"),
            cached_profile: strudel_dir.join(format!("{bundle_id}.mobileprovision")),
            devices_toml: strudel_dir.join("devices.toml"),
            dmg: build_dir.join(dmg_name),
            info_plist: app_bundle.join("Contents/Info.plist"),
            entitlements_plist: build_dir.join("Entitlements.plist"),
            build_dir: build_dir.clone(),
            app_bundle,
            strudel_dir,
            extensions: extension_paths,
        }
    }

    pub fn pending_submission(&self, uuid: &str) -> PendingSubmission {
        let dir = self.strudel_dir.join(uuid);
        let dmg_name = self.dmg.file_name().unwrap();
        PendingSubmission {
            dmg: dir.join(dmg_name),
            state: dir.join("pending-notarization.toml"),
            dir,
        }
    }
}

/// Create the `.strudel` directory and write a self-ignoring `.gitignore`
/// containing `*` so none of its contents are accidentally committed.
pub fn ensure_strudel_dir(strudel_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(strudel_dir)?;
    let gitignore = strudel_dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(gitignore, "*\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn cfg(build_dir: &str, app_name: &str, version: &str) -> ResolvedConfig {
        ResolvedConfig {
            app_name: app_name.into(),
            bundle_id: "x".into(),
            version: version.into(),
            build_number: "1".into(),
            source_dir: PathBuf::from("/src"),
            build_dir: PathBuf::from(build_dir),
            info_json_path: None,
            entitlements_json_path: None,
            icon_path: None,
            archs: vec!["arm64".into()],
            target_name: app_name.into(),
            sign_identity: String::new(),
            notarize_timeout: 600,
            build_env: HashMap::new(),
            embed_libs: Vec::new(),
            provisioning_profile: None,
            extensions: Vec::new(),
            ios_simulator: "iPhone 16".into(),
            ios_device: None,
            ios_deployment_target: "18.0".into(),
            ios_assets_dir: None,
            ios_app_icon_name: "AppIcon".into(),
            team_id: String::new(),
            apple_api_issuer: String::new(),
            apple_api_key: String::new(),
            apple_api_key_path: None,
            apple_certificate: String::new().into(),
            apple_certificate_password: String::new().into(),
            resources_dir: None,
            resources: Vec::new(),
            dmg: None,
        }
    }

    #[test]
    fn artifact_paths_embed_app_name_and_version() {
        let p = Paths::new(&cfg("/out", "MyApp", "1.2.3"));
        assert_eq!(p.app_bundle, PathBuf::from("/out/MyApp.app"));
        assert_eq!(p.dmg, PathBuf::from("/out/MyApp-1.2.3.dmg"));
        assert_eq!(
            p.info_plist,
            PathBuf::from("/out/MyApp.app/Contents/Info.plist")
        );
        assert_eq!(
            p.entitlements_plist,
            PathBuf::from("/out/Entitlements.plist")
        );
        assert_eq!(p.build_dir, PathBuf::from("/out"));
    }

    #[test]
    fn ios_cache_paths_are_under_strudel_dir() {
        let p = Paths::new(&cfg("/out", "MyApp", "1.0"));
        // source_dir = "/src", so strudel_dir = "/src/.strudel"
        assert_eq!(
            p.cached_profile,
            PathBuf::from("/src/.strudel/x.mobileprovision")
        );
        assert_eq!(p.devices_toml, PathBuf::from("/src/.strudel/devices.toml"));
    }

    #[test]
    fn app_name_with_spaces_is_preserved_literally() {
        // The .app bundle and DMG names embed the app name verbatim —
        // spaces and case are kept (matches Finder's display name).
        let p = Paths::new(&cfg("/out", "My App", "1.0"));
        assert_eq!(p.app_bundle, PathBuf::from("/out/My App.app"));
        assert_eq!(p.dmg, PathBuf::from("/out/My App-1.0.dmg"));
    }

    #[test]
    fn extension_paths_nest_under_plugins() {
        use crate::config::{ExtensionKind, ResolvedExtension};
        let mut c = cfg("/out", "MyApp", "1.0");
        c.extensions.push(ResolvedExtension {
            kind: ExtensionKind::SafariWebExtension,
            target_name: "MyAppExtension".into(),
            bundle_id: "com.example.myapp.Extension".into(),
            name: "MyAppExtension".into(),
            info_json_path: None,
            entitlements_json_path: PathBuf::from("/ext/e.json"),
            resources_dir: Some(PathBuf::from("/ext/dist")),
            principal_class: Some("MyAppExtension.SafariWebExtensionHandler".into()),
            extension_point_identifier: None,
        });
        let p = Paths::new(&c);
        assert_eq!(p.extensions.len(), 1);
        let e = &p.extensions[0];
        assert_eq!(
            e.appex,
            PathBuf::from("/out/MyApp.app/Contents/PlugIns/MyAppExtension.appex")
        );
        assert_eq!(
            e.binary,
            PathBuf::from(
                "/out/MyApp.app/Contents/PlugIns/MyAppExtension.appex/Contents/MacOS/MyAppExtension"
            )
        );
        assert_eq!(
            e.info_plist,
            PathBuf::from(
                "/out/MyApp.app/Contents/PlugIns/MyAppExtension.appex/Contents/Info.plist"
            )
        );
        assert_eq!(
            e.resources,
            PathBuf::from(
                "/out/MyApp.app/Contents/PlugIns/MyAppExtension.appex/Contents/Resources"
            )
        );
        assert_eq!(
            e.entitlements_plist,
            PathBuf::from("/out/MyAppExtension.entitlements.plist")
        );
    }
}
