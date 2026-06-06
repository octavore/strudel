use std::path::PathBuf;

use crate::config::{ResolvedConfig, ResolvedExtension};

pub struct Paths {
    pub build_dir: PathBuf,
    pub app_bundle: PathBuf,
    pub dmg: PathBuf,
    pub zip: PathBuf,
    pub info_plist: PathBuf,
    pub entitlements_plist: PathBuf,
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
            app_name,
            version,
            extensions,
            ..
        } = cfg;
        let app_bundle = build_dir.join(format!("{app_name}.app"));
        let extension_paths = extensions
            .iter()
            .map(|ext| ExtensionPaths::for_extension(&app_bundle, build_dir, ext))
            .collect();
        Paths {
            dmg: build_dir.join(format!("{app_name}-{version}.dmg")),
            zip: build_dir.join(format!("{app_name}-{version}.zip")),
            info_plist: app_bundle.join("Contents/Info.plist"),
            entitlements_plist: build_dir.join("Entitlements.plist"),
            build_dir: build_dir.clone(),
            app_bundle,
            extensions: extension_paths,
        }
    }
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
            team_id: String::new(),
            apple_id: String::new(),
            apple_api_issuer: String::new(),
            apple_api_key: String::new(),
            apple_api_key_path: None,
            apple_password: String::new(),
            apple_certificate: String::new(),
            apple_certificate_password: String::new(),
            resources_dir: None,
            resources: Vec::new(),
        }
    }

    #[test]
    fn artifact_paths_embed_app_name_and_version() {
        let p = Paths::new(&cfg("/out", "MyApp", "1.2.3"));
        assert_eq!(p.app_bundle, PathBuf::from("/out/MyApp.app"));
        assert_eq!(p.dmg, PathBuf::from("/out/MyApp-1.2.3.dmg"));
        assert_eq!(p.zip, PathBuf::from("/out/MyApp-1.2.3.zip"));
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
    fn app_name_with_spaces_is_preserved_literally() {
        // The .app bundle, DMG, and zip names embed the app name verbatim —
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
