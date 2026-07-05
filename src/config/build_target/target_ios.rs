use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum IosProvisioningBackend {
    Free,
    AppStoreConnect,
}

/// `[ios]` — optional settings for iOS simulator and device workflows.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct IosSection {
    /// iOS Simulator name for `strudel sim`. Default: `"iPhone 16"`.
    pub simulator: Option<String>,

    /// Connected device name or UDID for `strudel device`.
    /// If unset, strudel auto-detects the first connected device.
    pub device: Option<String>,

    /// iOS deployment target (e.g. `"18.0"`). Default: `"18.0"`.
    pub deployment_target: Option<String>,

    /// Path to a `.xcassets` directory to compile into the bundle with
    /// `xcrun actool`. Optional; skipped when unset.
    pub assets_dir: Option<PathBuf>,

    /// Name of the app icon set inside `assets_dir`. Default: `"AppIcon"`.
    pub app_icon_name: Option<String>,

    /// Provisioning backend: `"free"` (Apple ID, 7-day profiles) or
    /// `"app_store_connect"` (paid account, default).
    pub provisioning: Option<IosProvisioningBackend>,

    /// Apple ID email for `provisioning = "free"`. Pre-fills the login prompt.
    pub apple_id: Option<String>,
}
