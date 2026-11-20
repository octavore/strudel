//! Capability planning for macOS builds.
//!
//! Required capabilities come from the entitlements files and the
//! `capabilities` config key. A signed build with a capability that may need a
//! provisioning profile also opts its bundle ID into strudel-managed
//! provisioning (see [`super::profile`]). A bundle ID with a pinned profile is
//! only checked locally.

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clml::cprintln;

use crate::apple::appstore::AppStoreClient;
use crate::apple::capabilities;
use crate::apple::capabilities::{CapabilityDef, CapabilityType, Severity};
use crate::builder::MacosBuilder;
use crate::builder::profile::{
    Finding, PinnedExpectations, ProfileRequest, ProvisioningPlan, check_pinned_profile,
    load_entitlements, missing_capabilities,
};
use crate::config::Platform;
use crate::paths::managed_profile_path;

pub(super) const IDENTIFIERS_URL: &str =
    "https://developer.apple.com/account/resources/identifiers/list";

/// Bundle ID with required capabilities. Either the host app or one
/// `[[extensions]]` entry.
#[derive(Debug)]
pub(super) struct Target {
    /// App or extension name, shown in prompts and progress lines.
    pub label: String,
    pub bundle_id: String,
    /// Required capabilities that apply to macOS.
    pub required: Vec<&'static CapabilityDef>,
    pub entitlements_path: Option<PathBuf>,
    /// Config key and files that list the capabilities.
    pub config_location: String,
    /// Profile pinned by an explicit `provisioning_profile` path.
    pub pinned: Option<PathBuf>,
}

impl Target {
    /// Required capabilities that may be checked against a profile.
    pub fn profile_capabilities(&self) -> Vec<&'static CapabilityDef> {
        self.required
            .iter()
            .copied()
            .filter(|c| c.may_require_profile())
            .collect()
    }
}

/// State of a bundle ID on App Store Connect.
pub(super) struct PortalState {
    /// Portal resource ID. `None` if the bundle ID is not registered.
    pub bundle_ref: Option<String>,
    pub enabled: HashSet<CapabilityType>,
}

impl PortalState {
    /// Read-only lookup.
    pub fn lookup(client: &AppStoreClient, bundle_id: &str) -> Result<Self> {
        let bundle_ref = client.find_bundle_id(bundle_id)?;
        let enabled = match &bundle_ref {
            Some(id) => client.enabled_capability_types(id)?,
            None => HashSet::new(),
        };
        Ok(PortalState {
            bundle_ref,
            enabled,
        })
    }
}

impl MacosBuilder {
    /// Opt bundle IDs into managed provisioning and check pinned profiles.
    /// Returns the bundle IDs with required capabilities.
    ///
    /// Capabilities come from the top-level `capabilities` key and the
    /// entitlements files (the host's `build.entitlements_json_path` and each
    /// extension's own). See [`MacosBuilder::profile_opt_ins`] for the bundle
    /// IDs that are opted in.
    ///
    /// Makes no App Store Connect calls. Called by `ensure_profiles` and
    /// `profile_fetch`, so both act on the same bundle IDs.
    pub(super) fn prepare_capabilities(&mut self) -> Result<Vec<Target>> {
        let interactive = !self.ci && std::io::stdin().is_terminal();
        self.prepare_capabilities_with(interactive)
    }

    fn prepare_capabilities_with(&mut self, interactive: bool) -> Result<Vec<Target>> {
        let targets = self.capability_targets()?;
        self.warn_if_unsigned(&targets);
        self.check_pinned_profiles(&targets)?;
        for bundle_id in self.profile_opt_ins(&targets, interactive) {
            self.manage_profile_for(&bundle_id);
        }
        Ok(targets)
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

    /// Warn that an ad-hoc build has capabilities that may need a profile.
    fn warn_if_unsigned(&self, targets: &[Target]) {
        if self.is_signed() {
            return;
        }
        let lines: String = targets
            .iter()
            .filter(|t| !t.profile_capabilities().is_empty())
            .map(|t| {
                let names: Vec<&str> = t.profile_capabilities().iter().map(|c| c.name).collect();
                format!("\n  {} ({}): {}", t.bundle_id, t.label, names.join(", "))
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
            let entitlements = load_entitlements(t.entitlements_path.as_deref());
            let caps = t.profile_capabilities();
            let expect = PinnedExpectations {
                platform: Platform::Macos,
                bundle_id: &t.bundle_id,
                team_id: &self.cfg.team_id,
                required_udids: &[],
                capabilities: &caps,
                entitlements: entitlements.as_ref(),
            };
            for finding in check_pinned_profile(path, &expect, identity) {
                findings.push((format!("{} ({})", t.label, t.bundle_id), finding));
            }
        }
        report_pinned_findings(findings)
    }

    /// Bundle IDs to opt into strudel-managed provisioning.
    ///
    /// Requires a signed build, API credentials, and a capability that may
    /// need a profile. A capability verified to need one opts in
    /// immediately. Other capabilities opt in only if `interactive`, since
    /// the profile can only be created after a prompt.
    fn profile_opt_ins(&self, targets: &[Target], interactive: bool) -> Vec<String> {
        if !self.is_signed() || !self.has_api_credentials() {
            return Vec::new();
        }
        targets
            .iter()
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

    /// Warn that App Store Connect credentials are missing, so `targets` were
    /// not checked.
    pub(super) fn warn_capabilities_unchecked(&self, targets: &[&Target]) {
        cprintln!(
            "<yellow>warning:</yellow> App Store Connect API credentials are not set, so \
             capabilities were not checked or enabled. Make sure these are enabled on the \
             bundle IDs at {IDENTIFIERS_URL}:"
        );
        for t in targets {
            let names: Vec<&str> = t.required.iter().map(|c| c.name).collect();
            cprintln!(
                "  <cyan>{}</cyan> ({}): {}",
                t.bundle_id,
                t.label,
                names.join(", ")
            );
        }
    }

    /// Plan for one bundle ID. `state` is the portal state, or `None` if it
    /// was not looked up. Then the bundle ID is assumed to be registered
    /// with no capabilities enabled.
    pub(super) fn build_plan(
        &self,
        request: &ProfileRequest,
        target: Option<&Target>,
        state: Option<&PortalState>,
        creates_profile: bool,
    ) -> ProvisioningPlan {
        let empty = HashSet::new();
        let (registered, enabled) = match state {
            Some(s) => (s.bundle_ref.is_some(), &s.enabled),
            None => (true, &empty),
        };
        let caps = target.map(|t| t.required.as_slice()).unwrap_or(&[]);
        let entitlements = target.and_then(|t| load_entitlements(t.entitlements_path.as_deref()));
        let mut plan = ProvisioningPlan::new(
            request,
            registered,
            enabled,
            caps,
            entitlements.as_ref(),
            None,
        );
        plan.create_profile = creates_profile;
        if creates_profile && request.cache_path.exists() {
            plan.missing_entitlements = self.missing_profile_entitlements(request);
        }
        if let Some(t) = target {
            plan.config_location = t.config_location.clone();
        }
        plan.opt_in_reason = self.opt_in_reason(&request.bundle_id, target);
        plan
    }

    /// Primary entitlement key of each required capability that the cached
    /// profile does not grant.
    fn missing_profile_entitlements(&self, request: &ProfileRequest) -> Vec<&'static str> {
        let Ok(profile) = crate::builder::profile::decode_profile(&request.cache_path) else {
            return Vec::new();
        };
        let Some(dict) = profile.as_dictionary() else {
            return Vec::new();
        };
        missing_capabilities(dict, &request.required_capabilities)
            .into_iter()
            .map(|c| c.entitlement_keys[0])
            .collect()
    }

    /// Why `bundle_id` is managed, or `None` if it is not.
    fn opt_in_reason(&self, bundle_id: &str, target: Option<&Target>) -> Option<String> {
        if self.capability_managed.contains(bundle_id) {
            let names: Vec<&str> = target
                .map(|t| t.profile_capabilities())
                .unwrap_or_default()
                .iter()
                .map(|c| c.name)
                .collect();
            return Some(format!(
                "{} may need an embedded profile on macOS",
                names.join(", ")
            ));
        }
        self.has_auto_profile(bundle_id)
            .then(|| "`provisioning_profile = \"auto\"` is set".to_string())
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

    /// Enable `caps` on the bundle ID. A capability that App Store Connect
    /// rejects produces a warning unless the table verifies it for macOS.
    pub(super) fn enable_capabilities(
        &self,
        client: &AppStoreClient,
        bundle_ref: &str,
        caps: &[&'static CapabilityDef],
    ) -> Result<()> {
        for cap in caps {
            let Err(e) = client.ensure_capabilities(bundle_ref, &[cap.capability_type]) else {
                continue;
            };
            let permissions = e.to_string().contains("Insufficient permissions");
            if cap.macos.severity() == Severity::Error || permissions {
                return Err(e);
            }
            cprintln!(
                "<yellow>warning:</yellow> App Store Connect did not enable {} for this bundle \
                 ID: {e:#}\n\
                 Enable it by hand at {IDENTIFIERS_URL} if the app needs it.",
                cap.name
            );
        }
        Ok(())
    }

    /// Bundle IDs with required capabilities, host first. Bundle IDs that
    /// require nothing are omitted. Capabilities that exist only on iOS are
    /// dropped with a warning.
    fn capability_targets(&self) -> Result<Vec<Target>> {
        let host_pinned = self
            .cfg
            .provisioning_profile
            .clone()
            .filter(|_| !self.cfg.manage_provisioning_profile);
        let mut targets = vec![self.target(
            &self.cfg.app_name,
            &self.cfg.bundle_id,
            self.cfg.entitlements_json_path.clone(),
            &self.macos.capabilities,
            host_pinned,
        )?];
        for ext in &self.cfg.extensions {
            let pinned = ext
                .provisioning_profile
                .clone()
                .filter(|_| !ext.manage_provisioning_profile);
            targets.push(self.target(
                &ext.name,
                &ext.bundle_id,
                ext.entitlements_json_path.clone(),
                &[],
                pinned,
            )?);
        }
        targets.retain(|t| !t.required.is_empty());
        Ok(targets)
    }

    fn target(
        &self,
        label: &str,
        bundle_id: &str,
        entitlements_path: Option<PathBuf>,
        names: &[String],
        pinned: Option<PathBuf>,
    ) -> Result<Target> {
        let required = capabilities::required(entitlements_path.as_deref(), names)?;
        let (required, ios_only) = drop_ios_only(required);
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
            entitlements_path,
            config_location: places.join(" and "),
            pinned,
        })
    }

    /// Opt `bundle_id` into strudel-managed provisioning, unless a profile is
    /// already configured for it. An opted-in bundle ID is recorded in
    /// `capability_managed`.
    fn manage_profile_for(&mut self, bundle_id: &str) {
        let cached = managed_profile_path(&self.core.cfg.source_dir, bundle_id);
        if self.core.cfg.bundle_id == bundle_id {
            if self.core.cfg.provisioning_profile.is_none() {
                self.core.cfg.provisioning_profile = Some(cached);
                self.core.cfg.manage_provisioning_profile = true;
                self.capability_managed.insert(bundle_id.to_string());
            }
            return;
        }
        if let Some(ext) = self
            .core
            .cfg
            .extensions
            .iter_mut()
            .find(|e| e.bundle_id == bundle_id)
            && ext.provisioning_profile.is_none()
        {
            ext.provisioning_profile = Some(cached);
            ext.manage_provisioning_profile = true;
            self.capability_managed.insert(bundle_id.to_string());
        }
    }

    /// Undo [`MacosBuilder::manage_profile_for`] for a bundle ID that was
    /// opted in by a capability. Bundle IDs with an explicit profile setting
    /// are not changed.
    pub(super) fn unmanage_profile_for(&mut self, bundle_id: &str) {
        if !self.capability_managed.remove(bundle_id) {
            return;
        }
        if self.core.cfg.bundle_id == bundle_id {
            self.core.cfg.provisioning_profile = None;
            self.core.cfg.manage_provisioning_profile = false;
        } else if let Some(ext) = self
            .core
            .cfg
            .extensions
            .iter_mut()
            .find(|e| e.bundle_id == bundle_id)
        {
            ext.provisioning_profile = None;
            ext.manage_provisioning_profile = false;
        }
    }
}

/// Split `caps` into those that apply to macOS and those that exist only on
/// iOS.
fn drop_ios_only(
    caps: Vec<&'static CapabilityDef>,
) -> (Vec<&'static CapabilityDef>, Vec<&'static CapabilityDef>) {
    caps.into_iter().partition(|c| !c.ios_only())
}

/// Targets that only need capabilities enabled. They have no managed profile
/// and no pinned profile.
pub(super) fn enable_only_targets<'a>(
    targets: &'a [Target],
    profiles: &[ProfileRequest],
) -> Vec<&'a Target> {
    targets
        .iter()
        .filter(|t| t.pinned.is_none())
        .filter(|t| !profiles.iter().any(|p| p.bundle_id == t.bundle_id))
        .collect()
}

/// Print the warnings in `findings`. Fail if any finding is an error. Each
/// entry holds the target it belongs to.
fn report_pinned_findings(findings: Vec<(String, Finding)>) -> Result<()> {
    let (errors, warnings): (Vec<_>, Vec<_>) = findings
        .into_iter()
        .partition(|(_, f)| f.severity == Severity::Error);
    for (target, finding) in &warnings {
        cprintln!("<yellow>warning:</yellow> {target}: {}", finding.message);
    }
    if errors.is_empty() {
        return Ok(());
    }
    let list: Vec<String> = errors
        .iter()
        .map(|(target, f)| format!("- {target}: {}", f.message))
        .collect();
    bail!(
        "Configured provisioning profile cannot be used:\n{}\n\
         Point `provisioning_profile` at a valid file, or set it to \"auto\" to let strudel \
         manage the profile.",
        list.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::apple::capabilities::{FreeFeature, Known};
    use crate::builder::OutputFlags;
    use crate::config::ResolvedConfig;
    use crate::config::fixtures::resolved_macos;

    const APP_GROUPS: &str = r#"{"com.apple.security.application-groups": ["group.x"]}"#;
    const HOTSPOT: &str = r#"{"com.apple.developer.networking.HotspotConfiguration": true}"#;
    const NETWORK_EXTENSIONS: &str =
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

    /// Config with an entitlements file holding `entitlements`. The returned
    /// directory deletes the file when dropped.
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

    fn builder(cfg: ResolvedConfig, dry_run: bool, ci: bool) -> MacosBuilder {
        let output = OutputFlags {
            dry_run,
            ..Default::default()
        };
        MacosBuilder::new(cfg, output, false, false, None, false, ci).unwrap()
    }

    fn is_managed(b: &MacosBuilder) -> bool {
        b.cfg.manage_provisioning_profile
            && b.cfg.provisioning_profile.is_some()
            && b.capability_managed.contains(&b.cfg.bundle_id)
    }

    #[test]
    fn adhoc_build_with_app_groups_does_not_opt_in() {
        let (mut cfg, _dir) = config(APP_GROUPS);
        set_credentials(&mut cfg);
        let mut b = builder(cfg, true, false);
        b.prepare_capabilities_with(true).unwrap();
        assert!(!b.cfg.manage_provisioning_profile);
        assert!(b.cfg.provisioning_profile.is_none());
        assert!(b.capability_managed.is_empty());
    }

    #[test]
    fn signed_build_with_only_ios_capability_does_not_opt_in() {
        let (mut cfg, _dir) = config(HOTSPOT);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        let mut b = builder(cfg, true, false);
        b.prepare_capabilities_with(true).unwrap();
        assert!(b.cfg.provisioning_profile.is_none());
        assert!(b.capability_managed.is_empty());
    }

    #[test]
    fn signed_build_with_app_groups_opts_in() {
        let (mut cfg, _dir) = config(APP_GROUPS);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        let mut b = builder(cfg, true, false);
        b.prepare_capabilities_with(true).unwrap();
        assert!(is_managed(&b));
    }

    #[test]
    fn unverified_capability_needs_an_interactive_terminal_to_opt_in() {
        let (mut cfg, _dir) = config(APP_GROUPS);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        let mut b = builder(cfg, true, false);
        b.prepare_capabilities_with(false).unwrap();
        assert!(b.cfg.provisioning_profile.is_none());
    }

    #[test]
    fn verified_capability_opts_in_without_a_terminal() {
        let (mut cfg, _dir) = config(NETWORK_EXTENSIONS);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        let mut b = builder(cfg, true, true);
        b.prepare_capabilities_with(false).unwrap();
        assert!(is_managed(&b));
    }

    #[test]
    fn missing_api_credentials_do_not_opt_in() {
        let (mut cfg, _dir) = config(NETWORK_EXTENSIONS);
        sign(&mut cfg);
        let mut b = builder(cfg, false, false);
        b.prepare_capabilities_with(true).unwrap();
        assert!(b.cfg.provisioning_profile.is_none());
        assert!(b.capability_managed.is_empty());
    }

    #[test]
    fn explicit_provisioning_profile_is_never_replaced() {
        let pinned = Path::new("/pinned.provisionprofile");
        let (mut cfg, _dir) = config(APP_GROUPS);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        cfg.provisioning_profile = Some(pinned.to_path_buf());
        let mut b = builder(cfg, true, false);
        b.prepare_capabilities_with(true).unwrap();
        assert_eq!(b.cfg.provisioning_profile.as_deref(), Some(pinned));
        assert!(!b.cfg.manage_provisioning_profile);
        assert!(b.capability_managed.is_empty());
    }

    #[test]
    fn explicit_auto_is_not_recorded_as_capability_managed() {
        let cached = PathBuf::from("/x/.strudel/b.provisionprofile");
        let (mut cfg, _dir) = config(APP_GROUPS);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        cfg.provisioning_profile = Some(cached.clone());
        cfg.manage_provisioning_profile = true;
        let mut b = builder(cfg, true, false);
        b.prepare_capabilities_with(true).unwrap();
        assert_eq!(b.cfg.provisioning_profile, Some(cached));
        assert!(b.capability_managed.is_empty());
    }

    #[test]
    fn pinned_profile_is_checked_locally_and_never_sent_to_app_store_connect() {
        let pinned = PathBuf::from("/missing/pinned.provisionprofile");
        let (mut cfg, _dir) = config(NETWORK_EXTENSIONS);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        cfg.provisioning_profile = Some(pinned);
        let b = builder(cfg, false, true);
        let targets = b.capability_targets().unwrap();
        assert!(targets[0].pinned.is_some());
        assert!(enable_only_targets(&targets, &[]).is_empty());

        // The missing file is reported before any client is created. The
        // credentials above point at a key file that does not exist.
        let mut b = b;
        let err = b.prepare_capabilities_with(false).unwrap_err().to_string();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn unpinned_targets_without_a_profile_only_enable_capabilities() {
        let (cfg, _dir) = config(APP_GROUPS);
        let b = builder(cfg, false, true);
        let targets = b.capability_targets().unwrap();
        assert_eq!(enable_only_targets(&targets, &[]).len(), 1);
        let profile = ProfileRequest::macos(
            b.cfg.bundle_id.clone(),
            b.cfg.app_name.clone(),
            PathBuf::from("/x/.strudel/b.provisionprofile"),
        );
        assert!(enable_only_targets(&targets, &[profile]).is_empty());
    }

    #[test]
    fn capabilities_and_profile_share_one_plan() {
        let (mut cfg, _dir) = config(APP_GROUPS);
        sign(&mut cfg);
        set_credentials(&mut cfg);
        let mut b = builder(cfg, true, false);
        let targets = b.prepare_capabilities_with(true).unwrap();
        let request = ProfileRequest::macos(
            b.cfg.bundle_id.clone(),
            b.cfg.app_name.clone(),
            PathBuf::from("/x/.strudel/b.provisionprofile"),
        )
        .with_capabilities(targets[0].profile_capabilities());
        let state = PortalState {
            bundle_ref: None,
            enabled: HashSet::new(),
        };
        let plan = b.build_plan(&request, targets.first(), Some(&state), true);
        let text = plan.render();
        assert!(text.contains("Register the bundle ID"), "{text}");
        assert!(text.contains("Enable capabilities: App Groups"), "{text}");
        assert!(text.contains("Create a MAC_APP_DIRECT profile"), "{text}");
        assert!(text.contains("group.x"), "{text}");
        assert!(text.contains("entitlements.json"), "{text}");
        assert!(
            text.contains("App Groups may need an embedded profile"),
            "{text}"
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

    #[test]
    fn pinned_errors_fail_and_warnings_pass() {
        let warning = Finding {
            severity: Severity::Warning,
            message: "w".into(),
        };
        let error = Finding {
            severity: Severity::Error,
            message: "e".into(),
        };
        assert!(report_pinned_findings(vec![("a".into(), warning.clone())]).is_ok());
        let err = report_pinned_findings(vec![("a".into(), warning), ("b".into(), error)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("- b: e"), "{err}");
        assert!(!err.contains("a: w"), "{err}");
    }
}
