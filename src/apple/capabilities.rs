#![allow(dead_code)]

//! Known capabilities and their App Store Connect API `capabilityType`.
//!
//! Apple's Developer Portal exposes more capabilities than its
//! `bundleIdCapabilities` API handles; e.g. DriverKit, Matter, and several
//! newer entitlements have no API-visible `CapabilityType` at all and must be
//! enabled by hand in the portal. This table only lists capabilities that
//! `CapabilityType` actually supports, so anything absent here simply isn't
//! auto-requestable yet.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use clml::cprintln;
use serde::{Deserialize, Serialize};

/// Fact about a capability, with its verification state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known<T> {
    /// Confirmed against Apple documentation or a real build.
    Verified(T),
    /// Believed to be correct but not confirmed.
    Unverified(T),
    /// Not known.
    Unknown,
}

/// How a failed check is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl<T> Known<T> {
    /// Severity of a failure that depends on this fact. Only a verified fact
    /// fails the build.
    pub fn severity(&self) -> Severity {
        match self {
            Known::Verified(_) => Severity::Error,
            Known::Unverified(_) | Known::Unknown => Severity::Warning,
        }
    }

    /// Value of the fact, verified or not. `None` if unknown.
    pub fn value(&self) -> Option<&T> {
        match self {
            Known::Verified(v) | Known::Unverified(v) => Some(v),
            Known::Unknown => None,
        }
    }
}

/// App Store Connect API `capabilityType` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CapabilityType {
    AccessWifiInformation,
    AppGroups,
    ApplePay,
    AssociatedDomains,
    AutofillCredentialProvider,
    Classkit,
    NetworkCustomProtocol,
    DataProtection,
    Healthkit,
    Homekit,
    #[serde(rename = "HOT_SPOT")]
    Hotspot,
    Icloud,
    InterAppAudio,
    #[serde(rename = "COREMEDIA_HLS_LOW_LATENCY")]
    CoremediaHlsLowLatency,
    Maps,
    Multipath,
    NetworkExtensions,
    NfcTagReading,
    PersonalVpn,
    PushNotifications,
    AppleIdAuth,
    Sirikit,
    SystemExtensionInstall,
    UserManagement,
    Wallet,
    WirelessAccessoryConfiguration,
}

impl CapabilityType {
    /// Value as sent to and returned by the App Store Connect API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AccessWifiInformation => "ACCESS_WIFI_INFORMATION",
            Self::AppGroups => "APP_GROUPS",
            Self::ApplePay => "APPLE_PAY",
            Self::AssociatedDomains => "ASSOCIATED_DOMAINS",
            Self::AutofillCredentialProvider => "AUTOFILL_CREDENTIAL_PROVIDER",
            Self::Classkit => "CLASSKIT",
            Self::NetworkCustomProtocol => "NETWORK_CUSTOM_PROTOCOL",
            Self::DataProtection => "DATA_PROTECTION",
            Self::Healthkit => "HEALTHKIT",
            Self::Homekit => "HOMEKIT",
            Self::Hotspot => "HOT_SPOT",
            Self::Icloud => "ICLOUD",
            Self::InterAppAudio => "INTER_APP_AUDIO",
            Self::CoremediaHlsLowLatency => "COREMEDIA_HLS_LOW_LATENCY",
            Self::Maps => "MAPS",
            Self::Multipath => "MULTIPATH",
            Self::NetworkExtensions => "NETWORK_EXTENSIONS",
            Self::NfcTagReading => "NFC_TAG_READING",
            Self::PersonalVpn => "PERSONAL_VPN",
            Self::PushNotifications => "PUSH_NOTIFICATIONS",
            Self::AppleIdAuth => "APPLE_ID_AUTH",
            Self::Sirikit => "SIRIKIT",
            Self::SystemExtensionInstall => "SYSTEM_EXTENSION_INSTALL",
            Self::UserManagement => "USER_MANAGEMENT",
            Self::Wallet => "WALLET",
            Self::WirelessAccessoryConfiguration => "WIRELESS_ACCESSORY_CONFIGURATION",
        }
    }

    /// Parse an API value. A value this enum does not list gives `None`.
    pub fn from_api(value: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
    }
}

/// Feature code and enabled value for the developer-services protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeFeature {
    pub code: &'static str,
    pub value: &'static str,
}

/// Fields `macos`, `identifiers`, and `source` are not read by build logic yet.
#[derive(Debug)]
#[allow(dead_code)]
pub struct CapabilityDef {
    /// Display name as shown in the Developer Portal / Xcode capabilities UI.
    pub name: &'static str,
    /// Entitlement keys this capability grants, as they would appear in an
    /// entitlements plist/JSON.
    pub entitlement_keys: &'static [&'static str],
    /// The App Store Connect API `CapabilityType`.
    pub capability_type: CapabilityType,
    /// Availability on macOS.
    pub macos: Known<bool>,
    /// Capability checked against an embedded provisioning profile on macOS
    /// outside the Mac App Store. iOS builds always use a profile and ignore
    /// this field.
    pub requires_profile: Known<bool>,
    /// Entitlement values are portal identifiers that must be assigned.
    pub identifiers: bool,
    /// `ios/updateAppId` feature used by free provisioning. Set only for
    /// capabilities that free accounts can use.
    ///
    /// Experimental. Apple does not document this scheme.
    pub free_feature: Known<FreeFeature>,
    /// Apple documentation URL or test that verified the entry. Required if
    /// any field is `Verified`.
    pub source: Option<&'static str>,
}

impl CapabilityDef {
    /// Capability may be checked against a provisioning profile on macOS.
    pub fn may_require_profile(&self) -> bool {
        !matches!(
            self.requires_profile,
            Known::Verified(false) | Known::Unverified(false)
        )
    }
}

pub const CAPABILITIES: &[CapabilityDef] = &[
    CapabilityDef {
        name: "Access Wi-Fi Information",
        entitlement_keys: &["com.apple.developer.networking.wifi-info"],
        capability_type: CapabilityType::AccessWifiInformation,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "App Groups",
        entitlement_keys: &["com.apple.security.application-groups"],
        capability_type: CapabilityType::AppGroups,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(true),
        identifiers: true,
        free_feature: Known::Unverified(FreeFeature {
            code: "APG3427HIY",
            value: "on",
        }),
        source: None,
    },
    CapabilityDef {
        name: "Apple Pay Payment Processing",
        entitlement_keys: &["com.apple.developer.in-app-payments"],
        capability_type: CapabilityType::ApplePay,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: true,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Associated Domains",
        entitlement_keys: &["com.apple.developer.associated-domains"],
        capability_type: CapabilityType::AssociatedDomains,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "AutoFill Credential Provider",
        entitlement_keys: &[
            "com.apple.developer.authentication-services.autofill-credential-provider",
        ],
        capability_type: CapabilityType::AutofillCredentialProvider,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "ClassKit",
        entitlement_keys: &["com.apple.developer.ClassKit-environment"],
        capability_type: CapabilityType::Classkit,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Custom Network Protocol",
        entitlement_keys: &["com.apple.developer.networking.custom-protocol"],
        capability_type: CapabilityType::NetworkCustomProtocol,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Data Protection",
        entitlement_keys: &["com.apple.developer.default-data-protection"],
        capability_type: CapabilityType::DataProtection,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unverified(FreeFeature {
            code: "dataProtection",
            value: "complete",
        }),
        source: None,
    },
    CapabilityDef {
        name: "HealthKit",
        entitlement_keys: &["com.apple.developer.healthkit"],
        capability_type: CapabilityType::Healthkit,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "HomeKit",
        entitlement_keys: &["com.apple.developer.homekit"],
        capability_type: CapabilityType::Homekit,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Hotspot",
        entitlement_keys: &["com.apple.developer.networking.HotspotConfiguration"],
        capability_type: CapabilityType::Hotspot,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "iCloud",
        entitlement_keys: &["com.apple.developer.icloud-container-identifiers"],
        capability_type: CapabilityType::Icloud,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: true,
        free_feature: Known::Unverified(FreeFeature {
            code: "iCloud",
            value: "on",
        }),
        source: None,
    },
    CapabilityDef {
        name: "Inter-App Audio",
        entitlement_keys: &["inter-app-audio"],
        capability_type: CapabilityType::InterAppAudio,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Low Latency HLS",
        entitlement_keys: &["com.apple.developer.low-latency-streaming"],
        capability_type: CapabilityType::CoremediaHlsLowLatency,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Maps",
        entitlement_keys: &["com.apple.developer.maps"],
        capability_type: CapabilityType::Maps,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Multipath",
        entitlement_keys: &["com.apple.developer.networking.multipath"],
        capability_type: CapabilityType::Multipath,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Network Extensions",
        entitlement_keys: &["com.apple.developer.networking.networkextension"],
        capability_type: CapabilityType::NetworkExtensions,
        macos: Known::Unknown,
        requires_profile: Known::Verified(true),
        identifiers: false,
        free_feature: Known::Unknown,
        source: Some("https://developer.apple.com/forums/thread/664126"),
    },
    CapabilityDef {
        name: "NFC Tag Reading",
        entitlement_keys: &["com.apple.developer.nfc.readersession.formats"],
        capability_type: CapabilityType::NfcTagReading,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Personal VPN",
        entitlement_keys: &["com.apple.developer.networking.vpn.api"],
        capability_type: CapabilityType::PersonalVpn,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Push Notifications",
        entitlement_keys: &["aps-environment", "com.apple.developer.aps-environment"],
        capability_type: CapabilityType::PushNotifications,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Sign In with Apple",
        entitlement_keys: &["com.apple.developer.applesignin"],
        capability_type: CapabilityType::AppleIdAuth,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "SiriKit",
        entitlement_keys: &["com.apple.developer.siri"],
        capability_type: CapabilityType::Sirikit,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "System Extension",
        entitlement_keys: &["com.apple.developer.system-extension.install"],
        capability_type: CapabilityType::SystemExtensionInstall,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(true),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "TV Services",
        entitlement_keys: &["com.apple.developer.user-management"],
        capability_type: CapabilityType::UserManagement,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Wallet",
        entitlement_keys: &["com.apple.developer.pass-type-identifiers"],
        capability_type: CapabilityType::Wallet,
        macos: Known::Unknown,
        requires_profile: Known::Unknown,
        identifiers: true,
        free_feature: Known::Unknown,
        source: None,
    },
    CapabilityDef {
        name: "Wireless Accessory Configuration",
        entitlement_keys: &["com.apple.external-accessory.wireless-configuration"],
        capability_type: CapabilityType::WirelessAccessoryConfiguration,
        macos: Known::Unknown,
        requires_profile: Known::Unverified(false),
        identifiers: false,
        free_feature: Known::Unknown,
        source: None,
    },
];

/// Capabilities required for a build, sorted by name without duplicates.
///
/// Includes capabilities found in `entitlements_json_path`, matched by
/// entitlement key. Also includes those in `capability_names`, matched by
/// display name and ignoring case. Entitlement keys with no known capability
/// are skipped, for example `application-identifier`. An unknown name in
/// `capability_names` is an error.
pub fn required(
    entitlements_json_path: Option<&Path>,
    capability_names: &[String],
) -> Result<Vec<&'static CapabilityDef>> {
    let mut caps: Vec<&'static CapabilityDef> = Vec::new();

    if let Some(path) = entitlements_json_path {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("Failed to read entitlements JSON at {}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("Entitlements file is not valid JSON: {}", path.display()))?;
        if let Some(obj) = value.as_object() {
            for key in obj.keys() {
                if let Some(cap) = by_entitlement_key(key) {
                    caps.push(cap);
                }
            }
            let unmatched = unmatched_developer_keys(obj.keys().map(String::as_str));
            if !unmatched.is_empty() {
                cprintln!(
                    "<yellow>warning:</yellow> No known capability for entitlement {}. \
                     Enable the capability by hand if the app needs it: \
                     https://developer.apple.com/account/resources/identifiers/list",
                    unmatched.join(", ")
                );
            }
        }
    }

    for name in capability_names {
        caps.push(lookup_name(name)?);
    }

    caps.sort_by_key(|c| c.name);
    caps.dedup_by_key(|c| c.name);
    Ok(caps)
}

/// Look up a capability by display name. An unknown name is an error.
pub fn lookup_name(name: &str) -> Result<&'static CapabilityDef> {
    by_name(name).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown capability {name:?} in `capabilities`. \
             See `strudel help entitlements` for the supported set."
        )
    })
}

/// Entitlement key that is set by the build and needs no capability.
const IGNORED_DEVELOPER_KEY: &str = "com.apple.developer.team-identifier";

/// Sorted `com.apple.developer.*` keys that match no table entry.
fn unmatched_developer_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut unmatched: Vec<&str> = keys
        .into_iter()
        .filter(|k| k.starts_with("com.apple.developer."))
        .filter(|k| *k != IGNORED_DEVELOPER_KEY)
        .filter(|k| by_entitlement_key(k).is_none())
        .collect();
    unmatched.sort_unstable();
    unmatched
}

/// Look up a capability by its entitlement key (as found in an entitlements
/// plist/JSON file).
pub fn by_entitlement_key(key: &str) -> Option<&'static CapabilityDef> {
    CAPABILITIES
        .iter()
        .find(|c| c.entitlement_keys.contains(&key))
}

/// Look up a capability by its display name (case-insensitive), as it would
/// be written in `[ios] capabilities` in strudel.toml.
pub fn by_name(name: &str) -> Option<&'static CapabilityDef> {
    CAPABILITIES
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_entitlement_key_finds_known_key() {
        let cap = by_entitlement_key("aps-environment").unwrap();
        assert_eq!(cap.name, "Push Notifications");
        assert_eq!(cap.capability_type, CapabilityType::PushNotifications);
    }

    #[test]
    fn both_push_keys_map_to_push_notifications() {
        for key in ["aps-environment", "com.apple.developer.aps-environment"] {
            assert_eq!(by_entitlement_key(key).unwrap().name, "Push Notifications");
        }
    }

    #[test]
    fn unmatched_developer_keys_lists_unknown_developer_keys_only() {
        let keys = [
            "com.apple.developer.zeta",
            "com.apple.developer.team-identifier",
            "com.apple.developer.healthkit",
            "com.apple.developer.alpha",
            "com.apple.security.app-sandbox",
            "application-identifier",
        ];
        assert_eq!(
            unmatched_developer_keys(keys),
            ["com.apple.developer.alpha", "com.apple.developer.zeta"]
        );
    }

    #[test]
    fn by_entitlement_key_unknown_is_none() {
        assert!(by_entitlement_key("com.apple.developer.does-not-exist").is_none());
    }

    #[test]
    fn by_name_is_case_insensitive() {
        assert_eq!(by_name("healthkit").unwrap().name, "HealthKit");
        assert_eq!(by_name("HEALTHKIT").unwrap().name, "HealthKit");
    }

    fn names(caps: &[&CapabilityDef]) -> Vec<&'static str> {
        caps.iter().map(|c| c.name).collect()
    }

    #[test]
    fn required_reads_entitlement_keys_and_skips_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entitlements.json");
        fs::write(
            &path,
            r#"{"aps-environment": "development", "application-identifier": "x"}"#,
        )
        .unwrap();
        let caps = required(Some(&path), &[]).unwrap();
        assert_eq!(names(&caps), ["Push Notifications"]);
    }

    #[test]
    fn required_merges_names_and_dedups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entitlements.json");
        fs::write(&path, r#"{"aps-environment": "development"}"#).unwrap();
        let extra = ["healthkit".to_string(), "Push Notifications".to_string()];
        let caps = required(Some(&path), &extra).unwrap();
        assert_eq!(names(&caps), ["HealthKit", "Push Notifications"]);
    }

    #[test]
    fn required_rejects_unknown_name() {
        let err = required(None, &["Nope".to_string()]).unwrap_err();
        assert!(err.to_string().contains("Unknown capability"));
    }

    #[test]
    fn required_rejects_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entitlements.json");
        fs::write(&path, "not json").unwrap();
        assert!(required(Some(&path), &[]).is_err());
    }

    #[test]
    fn names_and_entitlement_keys_are_unique() {
        let mut names: Vec<&str> = CAPABILITIES.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CAPABILITIES.len(), "duplicate capability name");

        let mut keys: Vec<&str> = CAPABILITIES
            .iter()
            .flat_map(|c| c.entitlement_keys.iter().copied())
            .collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), total, "duplicate entitlement key");
    }

    #[test]
    fn severity_maps_verified_to_error() {
        assert_eq!(Known::Verified(true).severity(), Severity::Error);
        assert_eq!(Known::Verified(false).severity(), Severity::Error);
        assert_eq!(Known::Unverified(true).severity(), Severity::Warning);
        assert_eq!(Known::<bool>::Unknown.severity(), Severity::Warning);
    }

    #[test]
    fn verified_entries_have_a_source() {
        for c in CAPABILITIES {
            let verified = matches!(c.macos, Known::Verified(_))
                || matches!(c.requires_profile, Known::Verified(_))
                || matches!(c.free_feature, Known::Verified(_));
            assert!(
                !verified || c.source.is_some(),
                "{} has a Verified field and no source",
                c.name
            );
        }
    }

    #[test]
    fn capability_type_as_str_matches_serde_name() {
        for c in CAPABILITIES {
            let json = serde_json::to_value(c.capability_type).unwrap();
            assert_eq!(json, c.capability_type.as_str(), "{}", c.name);
            assert_eq!(
                CapabilityType::from_api(c.capability_type.as_str()),
                Some(c.capability_type)
            );
        }
        assert_eq!(CapabilityType::from_api("NOT_A_TYPE"), None);
    }
}
