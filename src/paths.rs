use std::path::PathBuf;

use crate::config::ResolvedConfig;

pub struct Paths {
    pub build_dir: PathBuf,
    pub app_bundle: PathBuf,
    pub dmg: PathBuf,
    pub zip: PathBuf,
    pub info_plist: PathBuf,
    pub entitlements_plist: PathBuf,
}

impl Paths {
    pub fn new(cfg: &ResolvedConfig) -> Self {
        let ResolvedConfig {
            build_dir,
            app_name,
            version,
            ..
        } = cfg;
        let app_bundle = build_dir.join(format!("{app_name}.app"));
        Paths {
            dmg: build_dir.join(format!("{app_name}-{version}.dmg")),
            zip: build_dir.join(format!("{app_name}-{version}.zip")),
            info_plist: app_bundle.join("Contents/Info.plist"),
            entitlements_plist: build_dir.join("Entitlements.plist"),
            build_dir: build_dir.clone(),
            app_bundle,
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
            entitlements_json_path: PathBuf::from("/e.json"),
            icon_path: None,
            archs: vec!["arm64".into()],
            target_name: app_name.into(),
            sign_identity: String::new(),
            notarize_timeout: 600,
            build_env: HashMap::new(),
            embed_libs: Vec::new(),
            provisioning_profile: None,
            team_id: String::new(),
            apple_id: String::new(),
            apple_api_issuer: String::new(),
            apple_api_key: String::new(),
            apple_api_key_path: None,
            apple_password: String::new(),
            apple_certificate: String::new(),
            apple_certificate_password: String::new(),
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
}
