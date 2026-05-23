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
        let app_bundle = cfg.build_dir.join(format!("{}.app", cfg.app_name));
        Paths {
            dmg: cfg
                .build_dir
                .join(format!("{}-{}.dmg", cfg.app_name, cfg.version)),
            zip: cfg
                .build_dir
                .join(format!("{}-{}.zip", cfg.app_name, cfg.version)),
            info_plist: app_bundle.join("Contents/Info.plist"),
            entitlements_plist: cfg.build_dir.join("entitlements.plist"),
            build_dir: cfg.build_dir.clone(),
            app_bundle,
        }
    }
}
