//! Confirmation text for the App Store Connect changes on one macOS bundle
//! ID.

use clml::cformat;

use super::ManagedBy;
use super::capabilities::Target;
use super::provision::{PortalState, StaleProfile};
use crate::apple::capabilities::{CapabilityDef, Severity};
use crate::builder::profile::{IDENTIFIERS_URL, MacosProfileKind};

/// Portal identifiers that must be assigned to a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IdentifierAssignment {
    pub capability: &'static str,
    pub values: Vec<String>,
}

/// Changes strudel makes for one bundle ID, shown before they are made.
#[derive(Debug, Clone)]
pub(super) struct ProvisioningPlan {
    pub bundle_id: String,
    /// Bundle ID is not registered on the portal.
    pub register_bundle_id: bool,
    /// Required capabilities, for verification notes.
    pub capabilities: Vec<&'static CapabilityDef>,
    /// Capabilities not yet enabled on the bundle ID.
    pub capabilities_to_enable: Vec<&'static CapabilityDef>,
    /// Identifiers to assign, read from the entitlements JSON.
    pub identifiers: Vec<IdentifierAssignment>,
    /// ASC `ProfileType` of the profile to create. `None` if the plan only
    /// enables capabilities.
    pub profile_type: Option<&'static str>,
    /// Kind of the profile to create.
    pub profile_kind: Option<MacosProfileKind>,
    /// UDIDs the profile must list. Empty unless the kind needs devices.
    pub devices: Vec<String>,
    /// Entitlement keys the current profile lacks.
    pub missing_entitlements: Vec<&'static str>,
    /// Config key or file that lists the capabilities.
    pub config_location: String,
    /// Why strudel manages the profile.
    pub opt_in_reason: Option<String>,
}

impl ProvisioningPlan {
    /// Plan for `bundle_id`. `target` holds its required capabilities, if
    /// any. `state` is its portal state. `stale` is the profile to create, or
    /// `None` if the plan only enables capabilities.
    pub fn new(
        bundle_id: &str,
        target: Option<&Target>,
        state: &PortalState,
        stale: Option<&StaleProfile>,
    ) -> Self {
        let caps = target.map(|t| t.required.caps.as_slice()).unwrap_or(&[]);
        let registered = state.bundle_ref.is_some();
        let capabilities_to_enable = caps
            .iter()
            .copied()
            .filter(|c| !registered || !state.enabled.contains(&c.capability_type))
            .collect();
        let entitlements = target.and_then(|t| t.required.entitlements.as_ref());
        let identifiers = caps
            .iter()
            .filter(|c| c.identifiers)
            .filter_map(|c| {
                let values = identifier_values(c, entitlements);
                (!values.is_empty()).then_some(IdentifierAssignment {
                    capability: c.name,
                    values,
                })
            })
            .collect();
        let missing_entitlements = stale
            .and_then(|s| s.reason.as_ref())
            .map(|r| r.missing_entitlement_keys())
            .unwrap_or_default();
        ProvisioningPlan {
            bundle_id: bundle_id.to_string(),
            register_bundle_id: !registered,
            capabilities: caps.to_vec(),
            capabilities_to_enable,
            identifiers,
            profile_type: stale.map(|s| s.profile.request.profile_type),
            profile_kind: stale.and_then(|s| s.profile.request.macos_kind),
            devices: stale
                .map(|s| s.profile.request.required_udids.clone())
                .unwrap_or_default(),
            missing_entitlements,
            config_location: target
                .map(|t| t.config_location.clone())
                .unwrap_or_else(|| "strudel.toml".to_string()),
            opt_in_reason: stale.and_then(|s| opt_in_reason(s.profile.managed_by)),
        }
    }

    /// Whether the plan changes anything on the portal.
    pub fn has_portal_changes(&self) -> bool {
        self.register_bundle_id || !self.capabilities_to_enable.is_empty()
    }

    /// Notes about capabilities not verified to need a profile on macOS.
    pub fn verification_notes(&self) -> Vec<String> {
        self.capabilities
            .iter()
            .filter(|c| {
                c.may_require_profile() && c.requires_profile.severity() == Severity::Warning
            })
            .map(|c| {
                format!(
                    "strudel has not verified whether {} requires a profile on macOS.",
                    c.name
                )
            })
            .collect()
    }

    /// Text for the confirmation prompt.
    pub fn render(&self) -> String {
        let mut lines = vec![cformat!("Bundle ID <bold>{}</bold>:", self.bundle_id)];
        if self.register_bundle_id {
            lines.push(cformat!("  <green>-</green> Register the bundle ID."));
        }
        if !self.capabilities_to_enable.is_empty() {
            let names: Vec<&str> = self.capabilities_to_enable.iter().map(|c| c.name).collect();
            lines.push(cformat!(
                "  <green>-</green> Enable capabilities: <cyan>{}</cyan>.",
                names.join(", ")
            ));
        }
        if let Some(profile_type) = self.profile_type {
            let kind = self.profile_kind.unwrap_or(MacosProfileKind::DeveloperId);
            lines.push(cformat!(
                "  <green>-</green> Create a {} provisioning profile \
                 (<cyan>{profile_type}</cyan>) and cache it locally.",
                kind.description()
            ));
            if kind.needs_devices() {
                lines.push(cformat!(
                    "    Devices, from .strudel/devices.toml: <cyan>{}</cyan>.",
                    self.devices.join(", ")
                ));
            }
            lines.push(cformat!(
                "    <dim>The signing identity decides the type. A name starting with {} uses \
                 {profile_type}.</dim>",
                kind.identity_prefixes()
            ));
            if !self.missing_entitlements.is_empty() {
                lines.push(cformat!(
                    "    <yellow>The current profile lacks {}.</yellow>",
                    self.missing_entitlements.join(", ")
                ));
            }
        }
        if !self.has_portal_changes() {
            lines.push(cformat!(
                "  <dim>The bundle ID is registered and has every required capability \
                 enabled.</dim>"
            ));
        }
        if let Some(reason) = &self.opt_in_reason {
            lines.push(cformat!(
                "  <dim>The profile is managed because {reason}.</dim>"
            ));
        }
        if !self.capabilities.is_empty() {
            let names: Vec<&str> = self.capabilities.iter().map(|c| c.name).collect();
            lines.push(cformat!(
                "  <dim>Required capabilities, from {}: {}.</dim>",
                self.config_location,
                names.join(", ")
            ));
        }
        if !self.identifiers.is_empty() {
            lines.push(cformat!("<bold>Assign identifiers on the portal:</bold>"));
            lines.push(format!(
                "  1. Open {IDENTIFIERS_URL} and select {}.",
                self.bundle_id
            ));
            for (i, id) in self.identifiers.iter().enumerate() {
                lines.push(cformat!(
                    "  {}. Configure <cyan>{}</cyan> with {}.",
                    i + 2,
                    id.capability,
                    id.values.join(", ")
                ));
            }
        }
        for note in self.verification_notes() {
            lines.push(cformat!("<yellow>Note:</yellow> {note}"));
        }
        lines.join("\n")
    }
}

/// Why strudel manages a profile, for the confirmation prompt.
fn opt_in_reason(managed_by: ManagedBy) -> Option<String> {
    match managed_by {
        ManagedBy::Config => Some("`provisioning_profile = \"auto\"` is set".to_string()),
        ManagedBy::Requested => None,
    }
}

/// String values of a capability's entitlement, as a string or string array.
fn identifier_values(cap: &CapabilityDef, entitlements: Option<&serde_json::Value>) -> Vec<String> {
    let Some(ents) = entitlements else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for key in cap.entitlement_keys {
        match ents.get(key) {
            Some(serde_json::Value::String(s)) => values.push(s.clone()),
            Some(serde_json::Value::Array(items)) => {
                values.extend(items.iter().filter_map(|v| v.as_str().map(str::to_string)));
            },
            _ => {},
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::super::ManagedProfile;
    use super::*;
    use crate::apple::capabilities::{self, CapabilityType, RequiredCapabilities};
    use crate::builder::profile::checks::ProfileProblem;
    use crate::builder::profile::{MacosProfileKind, ProfileRequest, StaleReason};

    fn cap(name: &str) -> &'static CapabilityDef {
        capabilities::by_name(name).unwrap()
    }

    fn target(caps: &[&'static CapabilityDef], entitlements: Option<serde_json::Value>) -> Target {
        Target {
            label: "App".into(),
            bundle_id: "com.example.app".into(),
            required: RequiredCapabilities {
                caps: caps.to_vec(),
                entitlements,
                unmatched_keys: Vec::new(),
            },
            config_location: "the entitlements file entitlements.json".into(),
            pinned: None,
        }
    }

    fn state(registered: bool, enabled: &[CapabilityType]) -> PortalState {
        PortalState {
            bundle_ref: registered.then(|| "REF".to_string()),
            enabled: enabled.iter().copied().collect::<HashSet<_>>(),
        }
    }

    fn managed(managed_by: ManagedBy) -> ManagedProfile {
        ManagedProfile {
            request: ProfileRequest::macos(
                "com.example.app".into(),
                "App".into(),
                PathBuf::from("/tmp/p"),
            ),
            managed_by,
        }
    }

    /// Rendered plan without color codes.
    fn plain(plan: &ProvisioningPlan) -> String {
        anstream::adapter::strip_str(&plan.render()).to_string()
    }

    fn enable_plan(target: &Target, state: &PortalState) -> ProvisioningPlan {
        ProvisioningPlan::new(&target.bundle_id, Some(target), state, None)
    }

    #[test]
    fn plan_enables_only_missing_capabilities() {
        let t = target(&[cap("HealthKit"), cap("Push Notifications")], None);
        let plan = enable_plan(&t, &state(true, &[CapabilityType::Healthkit]));
        let names: Vec<&str> = plan.capabilities_to_enable.iter().map(|c| c.name).collect();
        assert_eq!(names, ["Push Notifications"]);
        assert!(!plan.register_bundle_id);
        assert!(plan.has_portal_changes());

        let t = target(&[cap("HealthKit")], None);
        let plan = enable_plan(&t, &state(true, &[CapabilityType::Healthkit]));
        assert!(!plan.has_portal_changes());
    }

    #[test]
    fn plan_for_an_unregistered_bundle_id_enables_everything() {
        let t = target(&[cap("HealthKit")], None);
        let plan = enable_plan(&t, &state(false, &[CapabilityType::Healthkit]));
        assert!(plan.register_bundle_id);
        assert_eq!(plan.capabilities_to_enable.len(), 1);
    }

    #[test]
    fn plan_rendering_lists_identifiers_from_the_entitlements_json() {
        let ents = serde_json::json!({
            "com.apple.security.application-groups": ["group.com.example.a", "group.com.example.b"],
            "com.apple.developer.icloud-container-identifiers": "iCloud.com.example",
            "com.apple.developer.healthkit": true,
        });
        let t = target(
            &[cap("App Groups"), cap("HealthKit"), cap("iCloud")],
            Some(ents),
        );
        let plan = enable_plan(&t, &state(false, &[]));
        assert_eq!(
            plan.identifiers,
            [
                IdentifierAssignment {
                    capability: "App Groups",
                    values: vec!["group.com.example.a".into(), "group.com.example.b".into()],
                },
                IdentifierAssignment {
                    capability: "iCloud",
                    values: vec!["iCloud.com.example".into()],
                },
            ]
        );
        let text = plain(&plan);
        assert!(text.contains("Register the bundle ID."));
        assert!(text.contains("Enable capabilities: App Groups, HealthKit, iCloud."));
        assert!(text.contains(&format!("1. Open {IDENTIFIERS_URL}")));
        assert!(
            text.contains("2. Configure App Groups with group.com.example.a, group.com.example.b.")
        );
        assert!(text.contains("3. Configure iCloud with iCloud.com.example."));
    }

    #[test]
    fn plan_notes_unverified_profile_requirements() {
        let t = target(
            &[cap("Push Notifications"), cap("Network Extensions")],
            None,
        );
        let plan = enable_plan(&t, &state(true, &[]));
        assert_eq!(
            plan.verification_notes(),
            ["strudel has not verified whether Push Notifications requires a profile on macOS."]
        );
        assert!(plain(&plan).contains("Note: strudel has not verified"));
    }

    #[test]
    fn plan_without_a_profile_renders_no_profile_line() {
        let t = target(&[cap("HealthKit")], None);
        let text = plain(&enable_plan(&t, &state(true, &[])));
        assert!(!text.contains("profile."), "{text}");
        assert!(text.contains("Enable capabilities: HealthKit."));
    }

    #[test]
    fn managed_profile_plan_lists_every_change() {
        let ents = serde_json::json!({ "com.apple.security.application-groups": ["group.x"] });
        let t = target(&[cap("App Groups")], Some(ents));
        let profile = managed(ManagedBy::Config);
        let stale = StaleProfile {
            profile: &profile,
            reason: Some(StaleReason::Missing),
        };
        let plan = ProvisioningPlan::new(&t.bundle_id, Some(&t), &state(false, &[]), Some(&stale));
        let text = plain(&plan);
        assert!(text.contains("Register the bundle ID"), "{text}");
        assert!(text.contains("Enable capabilities: App Groups"), "{text}");
        assert!(
            text.contains(
                "Create a Developer ID provisioning profile (MAC_APP_DIRECT) and cache it locally."
            ),
            "{text}"
        );
        assert!(text.contains("group.x"), "{text}");
        assert!(text.contains("entitlements.json"), "{text}");
        assert!(
            text.contains(
                "The profile is managed because `provisioning_profile = \"auto\"` is set."
            ),
            "{text}"
        );
    }

    fn development_plan(udids: &[&str]) -> ProvisioningPlan {
        let t = target(&[cap("App Groups")], None);
        let profile = ManagedProfile {
            request: ProfileRequest::macos(
                "com.example.app".into(),
                "App".into(),
                PathBuf::from("/tmp/p"),
            )
            .with_macos_kind(
                MacosProfileKind::Development,
                udids.iter().map(|u| u.to_string()).collect(),
            ),
            managed_by: ManagedBy::Config,
        };
        let stale = StaleProfile {
            profile: &profile,
            reason: Some(StaleReason::Missing),
        };
        ProvisioningPlan::new(&t.bundle_id, Some(&t), &state(true, &[]), Some(&stale))
    }

    #[test]
    fn development_plan_lists_the_type_and_devices() {
        let plan = development_plan(&["AAA-111", "BBB-222"]);
        assert_eq!(plan.profile_type, Some("MAC_APP_DEVELOPMENT"));
        assert_eq!(plan.devices, ["AAA-111", "BBB-222"]);
        let text = plain(&plan);
        assert!(
            text.contains(
                "Create a Mac development provisioning profile (MAC_APP_DEVELOPMENT) and cache \
                 it locally."
            ),
            "{text}"
        );
        assert!(
            text.contains("Devices, from .strudel/devices.toml: AAA-111, BBB-222."),
            "{text}"
        );
        assert!(
            text.contains(
                "The signing identity decides the type. A name starting with \"Apple \
                 Development\" or \"Mac Developer\" uses MAC_APP_DEVELOPMENT."
            ),
            "{text}"
        );
        assert!(!text.contains("Developer ID"), "{text}");
    }

    #[test]
    fn development_plan_renders_in_full() {
        let text = plain(&development_plan(&["AAA-111"]));
        assert_eq!(
            text,
            indoc::indoc! {"
                Bundle ID com.example.app:
                  - Enable capabilities: App Groups.
                  - Create a Mac development provisioning profile (MAC_APP_DEVELOPMENT) and cache it locally.
                    Devices, from .strudel/devices.toml: AAA-111.
                    The signing identity decides the type. A name starting with \"Apple Development\" or \"Mac Developer\" uses MAC_APP_DEVELOPMENT.
                  The profile is managed because `provisioning_profile = \"auto\"` is set.
                  Required capabilities, from the entitlements file entitlements.json: App Groups.
                Note: strudel has not verified whether App Groups requires a profile on macOS."
            }
        );
    }

    #[test]
    fn developer_id_plan_lists_no_devices() {
        let t = target(&[cap("App Groups")], None);
        let profile = managed(ManagedBy::Config);
        let stale = StaleProfile {
            profile: &profile,
            reason: Some(StaleReason::Missing),
        };
        let plan = ProvisioningPlan::new(&t.bundle_id, Some(&t), &state(true, &[]), Some(&stale));
        assert!(plan.devices.is_empty());
        let text = plain(&plan);
        assert!(!text.contains("Devices, from"), "{text}");
        assert!(
            text.contains("A name starting with \"Developer ID Application\" uses MAC_APP_DIRECT."),
            "{text}"
        );
    }

    #[test]
    fn plan_reports_entitlements_missing_from_the_current_profile() {
        let t = target(&[cap("Network Extensions")], None);
        let profile = managed(ManagedBy::Config);
        let reason = StaleReason::Problems(vec![ProfileProblem::MissingCapability(cap(
            "Network Extensions",
        ))]);
        let stale = StaleProfile {
            profile: &profile,
            reason: Some(reason),
        };
        let plan = ProvisioningPlan::new(&t.bundle_id, Some(&t), &state(true, &[]), Some(&stale));
        assert_eq!(
            plan.missing_entitlements,
            ["com.apple.developer.networking.networkextension"]
        );
        let text = plain(&plan);
        assert!(text.contains("The current profile lacks"), "{text}");
        assert!(
            text.contains("`provisioning_profile = \"auto\"` is set"),
            "{text}"
        );
    }
}
