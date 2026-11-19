use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum IosProvisioningBackend {
    Free,
    AppStoreConnect,
}

/// A device family an iOS app supports.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum IosDeviceFamily {
    Iphone,
    Ipad,
}

impl IosDeviceFamily {
    /// The `UIDeviceFamily` Info.plist value.
    pub fn ui_device_family(self) -> u8 {
        match self {
            IosDeviceFamily::Iphone => 1,
            IosDeviceFamily::Ipad => 2,
        }
    }

    /// The `actool --target-device` value.
    pub fn actool_name(self) -> &'static str {
        match self {
            IosDeviceFamily::Iphone => "iphone",
            IosDeviceFamily::Ipad => "ipad",
        }
    }
}

/// IosSection `[ios]` optional settings for iOS simulator and device workflows.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct IosSection {
    /// iOS Simulator name for `strudel run --sim`. Default: `"iPhone 16"`.
    pub simulator: Option<String>,

    /// Connected device name or UDID for `strudel run --device`.
    /// If unset, strudel auto-detects the first connected device.
    pub device: Option<String>,

    /// iOS deployment target (e.g. `"18.0"`). Default: `"18.0"`.
    pub deployment_target: Option<String>,

    /// Device families the app supports: `"iphone"`, `"ipad"`, or both.
    /// Default: `["iphone"]`.
    pub device_family: Option<Vec<IosDeviceFamily>>,

    /// Path to a `.xcassets` directory to compile into the bundle with
    /// `xcrun actool`. Optional; skipped when unset.
    pub assets_dir: Option<PathBuf>,

    /// Name of the app icon set inside `assets_dir`. Default: `"AppIcon"`.
    pub app_icon_name: Option<String>,

    /// Provisioning backend: `"free"` (Apple ID, 7-day profiles) or
    /// `"app_store_connect"` (paid account, default). The
    /// `"app_store_connect"` backend authenticates with the `[apple]` API key
    /// (`api_issuer`/`api_key`/`api_key_path`).
    pub provisioning: Option<IosProvisioningBackend>,

    /// Apple ID email for `provisioning = "free"`. Pre-fills the login prompt.
    pub apple_id: Option<String>,

    /// Capability display names (e.g. `"Push Notifications"`, `"HealthKit"`)
    /// to auto-enable on the bundle ID via the App Store Connect API, in
    /// addition to any inferred from `build.entitlements_json_path`. Only
    /// capabilities strudel knows the ASC `CapabilityType` for can be listed
    /// here; see `strudel help entitlements` for the supported set.
    pub capabilities: Option<Vec<String>>,
}

impl IosSection {
    /// Fills in fields left unset here from the shared `[ios]` defaults.
    /// Per-target fields take precedence.
    pub fn merge(self, shared: Option<&IosSection>) -> IosSection {
        let shared = shared.cloned().unwrap_or_default();
        IosSection {
            simulator: self.simulator.or(shared.simulator),
            device: self.device.or(shared.device),
            deployment_target: self.deployment_target.or(shared.deployment_target),
            device_family: self.device_family.or(shared.device_family),
            assets_dir: self.assets_dir.or(shared.assets_dir),
            app_icon_name: self.app_icon_name.or(shared.app_icon_name),
            provisioning: self.provisioning.or(shared.provisioning),
            apple_id: self.apple_id.or(shared.apple_id),
            capabilities: self.capabilities.or(shared.capabilities),
        }
    }
}
