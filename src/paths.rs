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
