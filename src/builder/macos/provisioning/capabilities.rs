//! Capability discovery and profile opt-in for macOS builds. Nothing here
//! calls App Store Connect.
//!
//! Required capabilities come from the entitlements files and the
//! `capabilities` config key. A signed build with a capability that may need a
//! provisioning profile also opts its bundle ID into strudel-managed
//! provisioning (see [`super`]). A bundle ID with a pinned profile is
//! only checked locally.

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use clml::cprintln;

use crate::apple::capabilities::{self, CapabilityDef, RequiredCapabilities, Severity};
use crate::builder::MacosBuilder;
use crate::builder::profile::{
    Finding, PinnedExpectations, ProfileExpectations, check_pinned_profile, report_pinned_findings,
};
use crate::config::Platform;

/// Bundle ID with required capabilities. Either the host app or one
/// `[[extensions]]` entry.
#[derive(Debug)]
pub(super) struct Target {
    /// App or extension name, shown in prompts and progress lines.
    pub label: String,
    pub bundle_id: String,
    /// Required capabilities that apply to macOS, and the entitlements they
    /// were read from.
    pub required: RequiredCapabilities,
    /// Config key and files that list the capabilities.
    pub config_location: String,
    /// Profile pinned by an explicit `provisioning_profile` path.
    pub pinned: Option<PathBuf>,
}

impl Target {
    /// Required capabilities that may be checked against a profile.
    pub fn profile_capabilities(&self) -> Vec<&'static CapabilityDef> {
        self.required.profile_capabilities(Platform::Macos)
    }

    fn capability_names(&self, caps: &[&CapabilityDef]) -> String {
        let names: Vec<&str> = caps.iter().map(|c| c.name).collect();
        names.join(", ")
    }
}

impl MacosBuilder {
    /// Bundle IDs with required capabilities, after local checks.
    ///
    /// Capabilities come from the top-level `capabilities` key and the
    /// entitlements files (the host's `build.entitlements_json_path` and each
    /// extension's own). Warns if an ad-hoc build has capabilities that may
    /// need a profile. Checks each pinned profile and fails on an error
    /// finding. Called by `ensure_profiles` and `profile_fetch`, so both act
    /// on the same bundle IDs.
    pub(super) fn prepare_capabilities(&self) -> Result<Vec<Target>> {
        let targets = self.capability_targets()?;
        self.warn_if_unsigned(&targets);
        self.check_pinned_profiles(&targets)?;
        Ok(targets)
    }

    /// Confirmation prompts can be shown.
    pub(super) fn interactive(&self) -> bool {
        !self.ci && std::io::stdin().is_terminal()
    }

    /// Build is signed with a Developer ID identity, either named in the
    /// config or imported from `APPLE_CERTIFICATE`.
    pub(super) fn is_signed(&self) -> bool {
        !self.cfg.sign_identity.is_empty() || self.cfg.signing_cert().is_some()
    }

    pub(super) fn has_api_credentials(&self) -> bool {
        self.cfg.apple_api_key_path.is_some()
            && !self.cfg.apple_api_key.is_empty()
            && !self.cfg.apple_api_issuer.is_empty()
    }

    /// Bundle IDs that a capability opts into strudel-managed provisioning.
    ///
    /// Requires a signed build, API credentials, and a capability that may
    /// need a profile. A bundle ID with a pinned profile or
    /// `provisioning_profile = "auto"` is not opted in. A capability verified
    /// to need a profile opts in immediately. Other capabilities opt in only
    /// if `interactive`, since the profile can only be created after a prompt.
    pub(super) fn profile_opt_ins(&self, targets: &[Target], interactive: bool) -> HashSet<String> {
        if !self.is_signed() || !self.has_api_credentials() {
            return HashSet::new();
        }
        targets
            .iter()
            .filter(|t| t.pinned.is_none() && !self.has_auto_profile(&t.bundle_id))
            .filter(|t| {
                let caps = t.profile_capabilities();
                let strict = caps
                    .iter()
                    .any(|c| c.requires_profile.severity() == Severity::Error);
                !caps.is_empty() && (strict || interactive)
            })
            .map(|t| t.bundle_id.clone())
            .collect()
    }

    /// `bundle_id` is configured with `provisioning_profile = "auto"`.
    fn has_auto_profile(&self, bundle_id: &str) -> bool {
        if self.cfg.bundle_id == bundle_id {
            return self.cfg.manage_provisioning_profile;
        }
        self.cfg
            .extensions
            .iter()
            .any(|e| e.bundle_id == bundle_id && e.manage_provisioning_profile)
    }

    /// Warn that an ad-hoc build has capabilities that may need a profile.
    fn warn_if_unsigned(&self, targets: &[Target]) {
        if self.is_signed() {
            return;
        }
        let lines: String = targets
            .iter()
            .filter_map(|t| {
                let caps = t.profile_capabilities();
                (!caps.is_empty()).then(|| {
                    format!(
                        "\n  {} ({}): {}",
                        t.bundle_id,
                        t.label,
                        t.capability_names(&caps)
                    )
                })
            })
            .collect();
        if lines.is_empty() {
            return;
        }
        cprintln!(
            "<yellow>warning:</yellow> This build is signed ad-hoc, so no provisioning profile \
             is created. The app may fail at launch without one for these capabilities. Set \
             `apple.identity` in strudel.toml (or APPLE_SIGNING_IDENTITY) to sign with a \
             Developer ID identity:{lines}"
        );
    }

    /// Check the pinned profile of each target without network calls.
    /// Warnings are printed. Errors fail the build.
    fn check_pinned_profiles(&self, targets: &[Target]) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        let identity =
            (!self.cfg.sign_identity.is_empty()).then_some(self.cfg.sign_identity.as_str());
        let mut findings = Vec::new();
        for t in targets {
            let Some(path) = &t.pinned else {
                continue;
            };
            let caps = t.profile_capabilities();
            let expect = PinnedExpectations {
                platform: Platform::Macos,
                profile: ProfileExpectations {
                    bundle_id: &t.bundle_id,
                    team_id: &self.cfg.team_id,
                    required_udids: &[],
                    capabilities: &caps,
                },
                entitlements: t.required.entitlements.as_ref(),
            };
            for finding in check_pinned_profile(path, &expect, identity) {
                findings.push(Finding {
                    severity: finding.severity,
                    message: format!("{} ({}): {}", t.label, t.bundle_id, finding.message),
                });
            }
        }
        report_pinned_findings(
            findings,
            "Point `provisioning_profile` at a valid file, or set it to \"auto\" to let strudel \
             manage the profile.",
        )
    }

    /// Bundle IDs with required capabilities, host first. Bundle IDs that
    /// require nothing are omitted.
    fn capability_targets(&self) -> Result<Vec<Target>> {
        let host_pinned = self
            .cfg
            .provisioning_profile
            .clone()
            .filter(|_| !self.cfg.manage_provisioning_profile);
        let host = target(
            &self.cfg.app_name,
            &self.cfg.bundle_id,
            self.cfg.entitlements_json_path.clone(),
            &self.macos.capabilities,
            host_pinned,
        )?;
        let mut targets = vec![host];
        for ext in &self.cfg.extensions {
            let pinned = ext
                .provisioning_profile
                .clone()
                .filter(|_| !ext.manage_provisioning_profile);
            targets.push(target(
                &ext.name,
                &ext.bundle_id,
                ext.entitlements_json_path.clone(),
                &[],
                pinned,
            )?);
        }
        targets.retain(|t| !t.required.caps.is_empty());
        Ok(targets)
    }
}

/// Target for one bundle ID. Prints a warning for entitlement keys that match
/// no capability. Capabilities that exist only on iOS are dropped with a
/// warning.
fn target(
    label: &str,
    bundle_id: &str,
    entitlements_path: Option<PathBuf>,
    names: &[String],
    pinned: Option<PathBuf>,
) -> Result<Target> {
    let mut required = capabilities::required(entitlements_path.as_deref(), names)?;
    required.warn_unmatched();
    let (caps, ios_only) = drop_ios_only(std::mem::take(&mut required.caps));
    required.caps = caps;
    if !ios_only.is_empty() {
        let names: Vec<&str> = ios_only.iter().map(|c| c.name).collect();
        cprintln!(
            "<yellow>warning:</yellow> {} is only available on iOS, so it is skipped for \
             {bundle_id} ({label}).",
            names.join(", ")
        );
    }
    let mut places = Vec::new();
    if !names.is_empty() {
        places.push("`capabilities` in strudel.toml".to_string());
    }
    if let Some(path) = &entitlements_path {
        places.push(format!("the entitlements file {}", path.display()));
    }
    Ok(Target {
        label: label.to_string(),
        bundle_id: bundle_id.to_string(),
        required,
        config_location: places.join(" and "),
        pinned,
    })
}

/// Split `caps` into those that apply to macOS and those that exist only on
/// iOS.
fn drop_ios_only(
    caps: Vec<&'static CapabilityDef>,
) -> (Vec<&'static CapabilityDef>, Vec<&'static CapabilityDef>) {
    caps.into_iter().partition(|c| !c.ios_only())
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::apple::capabilities::{CapabilityType, FreeFeature, Known};
    use crate::builder::OutputFlags;
    use crate::config::ResolvedConfig;
    use crate::config::fixtures::resolved_macos;

    pub const APP_GROUPS: &str = r#"{"com.apple.security.application-groups": ["group.x"]}"#;
    const HOTSPOT: &str = r#"{"com.apple.developer.networking.HotspotConfiguration": true}"#;
    pub const NETWORK_EXTENSIONS: &str =
        r#"{"com.apple.developer.networking.networkextension": ["packet-tunnel-provider"]}"#;

    /// Capability that exists only on iOS.
    static IOS_ONLY: CapabilityDef = CapabilityDef {
        name: "Only iOS",
        entitlement_keys: &["com.apple.developer.only-ios"],
        capability_type: CapabilityType::Classkit,
        macos: Known::Unverified(false),
        requires_profile: Known::Unknown,
        identifiers: false,
        free_feature: Known::<FreeFeature>::Unknown,
        source: None,
    };

    /// Signed config with API credentials and an entitlements file holding
    /// `entitlements`. The returned directory deletes the file when dropped.
    pub fn signed_config(entitlements: &str) -> (ResolvedConfig, tempfile::TempDir) {
        let (mut cfg, dir) = config(entitlements);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        (cfg, dir)
    }

    fn config(entitlements: &str) -> (ResolvedConfig, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entitlements.json");
        std::fs::write(&path, entitlements).unwrap();
        let mut cfg = resolved_macos();
        cfg.entitlements_json_path = Some(path);
        (cfg, dir)
    }

    fn sign(cfg: &mut ResolvedConfig) {
        cfg.sign_identity = "Developer ID Application: Me (TEAM)".into();
    }

    fn set_credentials(cfg: &mut ResolvedConfig) {
        cfg.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        cfg.apple_api_key = "KID".into();
        cfg.apple_api_issuer = "ISS".into();
    }

    pub fn builder(cfg: ResolvedConfig, dry_run: bool, ci: bool) -> MacosBuilder {
        let output = OutputFlags {
            dry_run,
            ..Default::default()
        };
        MacosBuilder::new(cfg, output, false, false, None, false, ci).unwrap()
    }

    fn opt_ins(b: &MacosBuilder, interactive: bool) -> HashSet<String> {
        let targets = b.prepare_capabilities().unwrap();
        b.profile_opt_ins(&targets, interactive)
    }

    #[test]
    fn adhoc_build_with_app_groups_does_not_opt_in() {
        let (mut cfg, _dir) = config(APP_GROUPS);
        set_credentials(&mut cfg);
        let b = builder(cfg, true, false);
        assert!(opt_ins(&b, true).is_empty());
    }

    #[test]
    fn signed_build_with_only_ios_capability_does_not_opt_in() {
        let (cfg, _dir) = signed_config(HOTSPOT);
        let b = builder(cfg, true, false);
        assert!(opt_ins(&b, true).is_empty());
    }

    #[test]
    fn signed_build_with_app_groups_opts_in() {
        let (cfg, _dir) = signed_config(APP_GROUPS);
        let b = builder(cfg, true, false);
        assert_eq!(opt_ins(&b, true), HashSet::from(["b".to_string()]));
    }

    #[test]
    fn unverified_capability_needs_an_interactive_terminal_to_opt_in() {
        let (cfg, _dir) = signed_config(APP_GROUPS);
        let b = builder(cfg, true, false);
        assert!(opt_ins(&b, false).is_empty());
    }

    #[test]
    fn verified_capability_opts_in_without_a_terminal() {
        let (cfg, _dir) = signed_config(NETWORK_EXTENSIONS);
        let b = builder(cfg, true, true);
        assert_eq!(opt_ins(&b, false).len(), 1);
    }

    #[test]
    fn missing_api_credentials_do_not_opt_in() {
        let (mut cfg, _dir) = config(NETWORK_EXTENSIONS);
        sign(&mut cfg);
        let b = builder(cfg, false, false);
        assert!(opt_ins(&b, true).is_empty());
    }

    #[test]
    fn explicit_provisioning_profile_is_never_opted_in() {
        let (mut cfg, _dir) = signed_config(APP_GROUPS);
        cfg.provisioning_profile = Some(PathBuf::from("/pinned.provisionprofile"));
        let b = builder(cfg, true, false);
        assert!(opt_ins(&b, true).is_empty());
    }

    #[test]
    fn explicit_auto_is_not_opted_in_by_a_capability() {
        let (mut cfg, _dir) = signed_config(APP_GROUPS);
        cfg.provisioning_profile = Some(PathBuf::from("/x/.strudel/b.provisionprofile"));
        cfg.manage_provisioning_profile = true;
        let b = builder(cfg, true, false);
        assert!(opt_ins(&b, true).is_empty());
    }

    #[test]
    fn pinned_profile_is_checked_locally() {
        let (mut cfg, _dir) = signed_config(NETWORK_EXTENSIONS);
        cfg.provisioning_profile = Some(PathBuf::from("/missing/pinned.provisionprofile"));
        let b = builder(cfg, false, true);
        let targets = b.capability_targets().unwrap();
        assert_eq!(
            targets[0].pinned.as_deref(),
            Some(Path::new("/missing/pinned.provisionprofile"))
        );

        // The missing file is reported before any client is created. The
        // credentials above point at a key file that does not exist.
        let err = b.prepare_capabilities().unwrap_err().to_string();
        assert!(
            err.contains("A (b): Provisioning profile not found"),
            "{err}"
        );
    }

    #[test]
    fn ios_only_capability_is_skipped_on_macos() {
        let hotspot = capabilities::by_name("Hotspot").unwrap();
        let (kept, skipped) = drop_ios_only(vec![&IOS_ONLY, hotspot]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "Hotspot");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "Only iOS");
    }
}
