//! Provisioning-profile decoding, validity checks, and creation, shared by the
//! iOS and macOS pipelines.

use std::collections::HashSet;
use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

use crate::apple::appstore::AppStoreClient;
use crate::apple::capabilities::{CapabilityDef, CapabilityType, Severity};
use crate::apple::fingerprint::parse_fingerprint;
use crate::builder::keychain::parse_identity_line;
use crate::config::Platform;
use crate::paths::ensure_strudel_dir;

/// The App Store Connect certificate type a profile is issued against. A
/// profile only authorizes signatures made with a certificate it embeds, so
/// this has to match how the bundle is actually signed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CertKind {
    /// Development certificates, for iOS device builds.
    Development,
    /// Developer ID Application certificates, for directly-distributed macOS
    /// apps. Covers both CA generations Apple issues under.
    DeveloperIdApplication,
}

impl CertKind {
    /// Resource IDs of every certificate of this kind in the account. Errors
    /// when the account has none, since a profile embedding no certificate
    /// authorizes no signature.
    pub fn list(self, client: &AppStoreClient) -> Result<Vec<String>> {
        let certs = match self {
            CertKind::Development => client.list_development_certificates()?,
            CertKind::DeveloperIdApplication => {
                client.list_developer_id_application_certificates()?
            },
        };
        Ok(certs.into_iter().map(|c| c.id).collect())
    }
}

/// One provisioning profile strudel manages, for the host app or for a single
/// extension. Both platforms describe their profiles this way and share
/// [`ProfileRequest::is_current`] and [`ProfileRequest::provision`]; the
/// platform differences live entirely in these fields.
pub struct ProfileRequest {
    pub bundle_id: String,
    /// App or extension name. Registers the bundle ID and labels progress
    /// output.
    pub label: String,
    /// Where the fetched profile is cached, and what gets embedded.
    pub cache_path: PathBuf,
    /// Profile name on the portal. Reused across fetches so refreshing
    /// replaces the profile rather than accumulating copies.
    pub name: String,
    /// ASC `BundleIdPlatform`: `"IOS"` or `"MAC_OS"`.
    pub platform: &'static str,
    /// ASC `ProfileType`, e.g. `"IOS_APP_DEVELOPMENT"` or `"MAC_APP_DIRECT"`.
    pub profile_type: &'static str,
    /// Portal device resource IDs to embed. Empty for profile types with no
    /// device relationship, which is every macOS type.
    pub device_ids: Vec<String>,
    /// Device UDIDs the cached profile must already cover to count as current.
    /// Empty on macOS, whose profiles carry no `ProvisionedDevices` at all.
    pub required_udids: Vec<String>,
    /// Capabilities whose entitlement keys the cached profile must grant to
    /// count as current. Set with [`ProfileRequest::with_capabilities`].
    pub required_capabilities: Vec<&'static CapabilityDef>,
}

impl ProfileRequest {
    /// Require the cached profile to grant the entitlement keys of `caps`.
    #[allow(dead_code)]
    pub fn with_capabilities(mut self, caps: Vec<&'static CapabilityDef>) -> Self {
        self.required_capabilities = caps;
        self
    }

    /// A Developer ID profile for a macOS bundle ID.
    pub fn macos(bundle_id: String, label: String, cache_path: PathBuf) -> Self {
        let name = format!("[strudel] {label} Developer ID");
        ProfileRequest {
            bundle_id,
            label,
            cache_path,
            name,
            platform: "MAC_OS",
            // "MAC_APP_DIRECT" is the ASC ProfileType for Developer ID
            // (direct, non-App-Store) distribution. A wrong value surfaces as
            // the API's own error rather than failing silently.
            profile_type: "MAC_APP_DIRECT",
            device_ids: Vec::new(),
            required_udids: Vec::new(),
            required_capabilities: Vec::new(),
        }
    }

    /// A development profile for an iOS bundle ID, embedding `device_ids`.
    pub fn ios(
        bundle_id: String,
        label: String,
        cache_path: PathBuf,
        device_ids: Vec<String>,
        required_udids: Vec<String>,
    ) -> Self {
        let name = format!("[strudel] {label} Development");
        ProfileRequest {
            bundle_id,
            label,
            cache_path,
            name,
            platform: "IOS",
            profile_type: "IOS_APP_DEVELOPMENT",
            device_ids,
            required_udids,
            required_capabilities: Vec::new(),
        }
    }

    /// Whether the cached profile can be reused as-is. A profile that is
    /// absent, undecodable, expiring, missing a required device or
    /// capability entitlement, or issued for another bundle ID or team counts
    /// as not current and is replaced. See [`ProfileRequest::stale_reason`].
    ///
    /// When `identity` is given, a profile that does not list that identity's
    /// certificate also counts as not current. A certificate belongs to one
    /// team, so this also covers the team check when `team_id` is unset. Pass
    /// `None` when the identity is not known yet (e.g. it comes from
    /// `APPLE_CERTIFICATE`, which is imported later). If the certificate
    /// comparison cannot be made, the profile is kept and the sign-time check
    /// in [`check_identity_authorized`] reports any mismatch.
    pub fn is_current(&self, team_id: &str, identity: Option<&str>) -> bool {
        self.stale_reason(team_id, identity).is_none()
    }

    /// Why the cached profile cannot be reused, or `None` if it is current.
    /// Arguments are as for [`ProfileRequest::is_current`].
    pub fn stale_reason(&self, team_id: &str, identity: Option<&str>) -> Option<StaleReason> {
        if !self.cache_path.exists() {
            return Some(StaleReason::Missing);
        }
        let Ok(profile) = decode_profile(&self.cache_path) else {
            return Some(StaleReason::Unreadable);
        };
        let Some(dict) = profile.as_dictionary() else {
            return Some(StaleReason::Unreadable);
        };
        let udids: Vec<&str> = self.required_udids.iter().map(String::as_str).collect();
        if let Some(reason) = stale_reason(
            dict,
            SystemTime::now(),
            &udids,
            &self.bundle_id,
            team_id,
            &self.required_capabilities,
        ) {
            return Some(reason);
        }
        match identity {
            Some(identity)
                if matches!(identity_authorized(identity, &profile), Ok(Some(false))) =>
            {
                Some(StaleReason::IdentityNotAuthorized)
            },
            _ => None,
        }
    }

    /// Find or create the bundle ID on the portal and return its resource ID.
    /// Callers that need the resource ID before creating the profile, such as
    /// to enable capabilities, use this with [`Self::provision_for`].
    pub fn ensure_bundle_id(&self, client: &AppStoreClient) -> Result<String> {
        client.find_or_create_bundle_id(&self.bundle_id, &self.label, self.platform)
    }

    /// Find or create the bundle ID, create the profile against `cert_ids`,
    /// and write it to the cache path. `cert_ids` must be of the [`CertKind`]
    /// this profile type is issued against; callers list them once per run
    /// rather than per profile. Prints nothing: the App Store Connect client
    /// narrates its own lookups, and callers frame this with their own
    /// progress lines.
    pub fn provision(
        &self,
        client: &AppStoreClient,
        cert_ids: &[String],
        strudel_dir: &Path,
    ) -> Result<()> {
        let bundle_id_ref = self.ensure_bundle_id(client)?;
        self.provision_for(client, &bundle_id_ref, cert_ids, strudel_dir)
    }

    /// Like [`Self::provision`], with a bundle ID resource already resolved
    /// through [`Self::ensure_bundle_id`].
    pub fn provision_for(
        &self,
        client: &AppStoreClient,
        bundle_id_ref: &str,
        cert_ids: &[String],
        strudel_dir: &Path,
    ) -> Result<()> {
        let bytes = client.create_profile(
            &self.name,
            self.profile_type,
            bundle_id_ref,
            cert_ids,
            &self.device_ids,
        )?;
        ensure_strudel_dir(strudel_dir)?;
        std::fs::write(&self.cache_path, &bytes).with_context(|| {
            format!(
                "Failed to write provisioning profile to {}",
                self.cache_path.display()
            )
        })
    }
}

/// Decode a provisioning profile's (`.mobileprovision`/`.provisionprofile`)
/// CMS envelope and return the plist value.
pub fn decode_profile(profile_path: &Path) -> Result<plist::Value> {
    let profile_str = profile_path
        .to_str()
        .context("Invalid provisioning profile path")?;
    let output = std::process::Command::new("security")
        .args(["cms", "-D", "-i", profile_str])
        .output()
        .context("Failed to run `security cms`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to decode provisioning profile: {stderr}");
    }
    plist::Value::from_reader(Cursor::new(&output.stdout))
        .context("Failed to parse provisioning profile plist")
}

/// Why a cached profile cannot be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    /// No file at the cache path.
    Missing,
    /// File could not be decoded.
    Unreadable,
    /// Profile has no `ExpirationDate`.
    NoExpiration,
    /// Profile expired, or expires within 5 minutes. Holds the expiry date.
    Expiring(String),
    /// Required device UDIDs absent from `ProvisionedDevices`.
    MissingDevices(Vec<String>),
    /// Application identifier is not `<team_id>.<bundle_id>`.
    ApplicationIdentifier { expected: String, actual: String },
    /// Entitlement keys of required capabilities that the profile lacks.
    MissingEntitlements(Vec<&'static str>),
    /// Signing identity's certificate is not in `DeveloperCertificates`.
    IdentityNotAuthorized,
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "no profile is cached"),
            Self::Unreadable => write!(f, "the cached profile cannot be read"),
            Self::NoExpiration => write!(f, "the profile has no expiration date"),
            Self::Expiring(at) => write!(f, "the profile expires at {at}"),
            Self::MissingDevices(udids) => {
                write!(f, "the profile lacks devices {}", udids.join(", "))
            },
            Self::ApplicationIdentifier { expected, actual } => {
                write!(f, "the profile is for {actual:?}, expected {expected:?}")
            },
            Self::MissingEntitlements(keys) => {
                write!(f, "the profile lacks entitlements {}", keys.join(", "))
            },
            Self::IdentityNotAuthorized => {
                write!(f, "the profile does not list the signing certificate")
            },
        }
    }
}

/// Return `true` when `profile_path` is a valid, current profile for the
/// given `required_udids`, `bundle_id`, and `team_id`. Returns `false` when the
/// file is absent or cannot be decoded, and for the reasons listed on
/// [`stale_reason`]. Pass an empty `required_udids` for macOS profiles,
/// which have no `ProvisionedDevices` at all. The profile must also grant the
/// entitlement keys of `capabilities`.
pub fn profile_is_current(
    profile_path: &Path,
    required_udids: &[&str],
    bundle_id: &str,
    team_id: &str,
    capabilities: &[&CapabilityDef],
) -> Result<bool> {
    if !profile_path.exists() {
        return Ok(false);
    }
    let profile = match decode_profile(profile_path) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let dict = match profile.as_dictionary() {
        Some(d) => d,
        None => return Ok(false),
    };
    Ok(stale_reason(
        dict,
        SystemTime::now(),
        required_udids,
        bundle_id,
        team_id,
        capabilities,
    )
    .is_none())
}

/// The content checks behind [`profile_is_current`], split out from the
/// `security cms` decode so they can be exercised directly. `now` is injected
/// rather than read from the clock, so the expiry window is testable.
///
/// Returns a reason when the profile has no `ExpirationDate` or expires within
/// 5 minutes of `now`, when any of `required_udids` is absent from
/// `ProvisionedDevices`, when the application-identifier entitlement (see
/// [`application_identifier`]) is not `<team_id>.<bundle_id>`, or when the
/// entitlements lack a key of a capability in `capabilities`. An empty
/// `required_udids` skips the device check. An empty `team_id` skips the
/// application identifier check.
pub fn stale_reason(
    dict: &plist::Dictionary,
    now: SystemTime,
    required_udids: &[&str],
    bundle_id: &str,
    team_id: &str,
    capabilities: &[&CapabilityDef],
) -> Option<StaleReason> {
    // Expiration: must not expire within 5 minutes.
    let Some(exp) = dict.get("ExpirationDate").and_then(|v| v.as_date()) else {
        return Some(StaleReason::NoExpiration);
    };
    let cutoff = now.checked_add(Duration::from_secs(300)).unwrap_or(now);
    if SystemTime::from(exp) <= cutoff {
        return Some(StaleReason::Expiring(exp.to_xml_format()));
    }

    // Device coverage: every required UDID must appear in ProvisionedDevices.
    let missing_devices = missing_udids(dict, required_udids);
    if !missing_devices.is_empty() {
        return Some(StaleReason::MissingDevices(missing_devices));
    }

    // application-identifier entitlement match (when team_id is set).
    if !team_id.is_empty() {
        let expected = format!("{team_id}.{bundle_id}");
        let actual = dict
            .get("Entitlements")
            .and_then(|v| v.as_dictionary())
            .and_then(application_identifier)
            .unwrap_or("");
        if actual != expected {
            return Some(StaleReason::ApplicationIdentifier {
                expected,
                actual: actual.to_string(),
            });
        }
    }

    let missing = missing_entitlement_keys(dict, capabilities);
    if !missing.is_empty() {
        return Some(StaleReason::MissingEntitlements(missing));
    }

    None
}

/// Entries of `required_udids` absent from the profile's `ProvisionedDevices`.
fn missing_udids(dict: &plist::Dictionary, required_udids: &[&str]) -> Vec<String> {
    if required_udids.is_empty() {
        return Vec::new();
    }
    let provisioned: Vec<&str> = dict
        .get("ProvisionedDevices")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_string()).collect())
        .unwrap_or_default();
    required_udids
        .iter()
        .filter(|u| !provisioned.contains(u))
        .map(|u| u.to_string())
        .collect()
}

/// Capabilities in `capabilities` for which the profile's `Entitlements` has
/// none of the capability's entitlement keys.
pub fn missing_capabilities<'a>(
    dict: &plist::Dictionary,
    capabilities: &[&'a CapabilityDef],
) -> Vec<&'a CapabilityDef> {
    let entitlements = dict.get("Entitlements").and_then(|v| v.as_dictionary());
    capabilities
        .iter()
        .copied()
        .filter(|cap| {
            !cap.entitlement_keys
                .iter()
                .any(|key| entitlements.is_some_and(|e| e.contains_key(key)))
        })
        .collect()
}

/// Primary entitlement key of each capability the profile does not grant.
fn missing_entitlement_keys(
    dict: &plist::Dictionary,
    capabilities: &[&CapabilityDef],
) -> Vec<&'static str> {
    missing_capabilities(dict, capabilities)
        .into_iter()
        .map(|cap| cap.entitlement_keys[0])
        .collect()
}

/// Parsed entitlements JSON at `path`. `None` if there is no path or the
/// file cannot be read or parsed.
pub fn load_entitlements(path: Option<&Path>) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(path?).ok()?;
    serde_json::from_str(&raw).ok()
}

/// One result of checking a pinned profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    fn error(message: String) -> Self {
        Finding {
            severity: Severity::Error,
            message,
        }
    }

    fn warning(message: String) -> Self {
        Finding {
            severity: Severity::Warning,
            message,
        }
    }
}

/// What a pinned profile must satisfy.
pub struct PinnedExpectations<'a> {
    pub platform: Platform,
    pub bundle_id: &'a str,
    /// Empty if no team is configured.
    pub team_id: &'a str,
    /// Empty on macOS.
    pub required_udids: &'a [&'a str],
    /// Capabilities the build needs. On macOS the caller passes only those
    /// that may require a profile.
    pub capabilities: &'a [&'static CapabilityDef],
    /// Parsed entitlements JSON of the app, if any.
    pub entitlements: Option<&'a serde_json::Value>,
}

/// Check a pinned profile file without network calls. A file that is missing
/// or unreadable is an error finding. If `identity` is given, a certificate
/// that the profile does not list is a warning.
pub fn check_pinned_profile(
    path: &Path,
    expect: &PinnedExpectations,
    identity: Option<&str>,
) -> Vec<Finding> {
    if !path.exists() {
        return vec![Finding::error(format!(
            "Provisioning profile not found at {}.",
            path.display()
        ))];
    }
    let profile = match decode_profile(path) {
        Ok(p) => p,
        Err(e) => {
            return vec![Finding::error(format!(
                "Provisioning profile at {} cannot be read: {e}",
                path.display()
            ))];
        },
    };
    let Some(dict) = profile.as_dictionary() else {
        return vec![Finding::error(format!(
            "Provisioning profile at {} is not a property list dictionary.",
            path.display()
        ))];
    };
    let mut findings = check_pinned_dict(dict, expect, SystemTime::now());
    if let Some(identity) = identity
        && matches!(identity_authorized(identity, &profile), Ok(Some(false)))
    {
        findings.push(Finding::warning(format!(
            "Signing identity {identity:?} is not listed in the profile's \
             DeveloperCertificates."
        )));
    }
    findings
}

/// Content checks behind [`check_pinned_profile`], split out from the
/// `security cms` decode. `now` is injected so expiry is testable.
///
/// Errors are an expired profile and an application identifier for another
/// team or bundle ID. Warnings are missing devices, missing capability
/// entitlement keys, and string array values the profile does not cover.
/// Entitlements that are not in the capability table are not checked, since
/// Developer ID profiles do not list keys that the signature enforces.
pub fn check_pinned_dict(
    dict: &plist::Dictionary,
    expect: &PinnedExpectations,
    now: SystemTime,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    match dict.get("ExpirationDate").and_then(|v| v.as_date()) {
        None => findings.push(Finding::error(
            "Pinned profile has no expiration date.".to_string(),
        )),
        Some(exp) => {
            let cutoff = now.checked_add(Duration::from_secs(300)).unwrap_or(now);
            if SystemTime::from(exp) <= cutoff {
                findings.push(Finding::error(format!(
                    "Pinned profile expired or expires within 5 minutes (expiration {}).",
                    exp.to_xml_format()
                )));
            }
        },
    }

    let entitlements = dict.get("Entitlements").and_then(|v| v.as_dictionary());
    match entitlements.and_then(application_identifier) {
        None => findings.push(Finding::error(
            "Pinned profile has no application-identifier entitlement.".to_string(),
        )),
        Some(actual) => {
            let (actual_team, actual_bundle) = actual.split_once('.').unwrap_or(("", actual));
            if !expect.team_id.is_empty() && actual_team != expect.team_id {
                findings.push(Finding::error(format!(
                    "Pinned profile is for team {actual_team:?}, expected {:?}.",
                    expect.team_id
                )));
            }
            if !bundle_id_matches(actual_bundle, expect.bundle_id) {
                findings.push(Finding::error(format!(
                    "Pinned profile is for bundle ID {actual_bundle:?}, expected {:?}.",
                    expect.bundle_id
                )));
            }
        },
    }

    let devices = missing_udids(dict, expect.required_udids);
    if !devices.is_empty() {
        findings.push(Finding::warning(format!(
            "Pinned profile does not include devices {}.",
            devices.join(", ")
        )));
    }

    for cap in missing_capabilities(dict, expect.capabilities) {
        let severity = match expect.platform {
            Platform::Macos if !cap.may_require_profile() => continue,
            Platform::Macos => cap.requires_profile.severity(),
            Platform::Ios => Severity::Warning,
        };
        let mut message = format!(
            "Pinned profile does not grant {} ({}).",
            cap.entitlement_keys[0], cap.name
        );
        if severity == Severity::Warning && expect.platform == Platform::Macos {
            message.push_str(&format!(
                " strudel has not verified whether {} requires a profile on macOS.",
                cap.name
            ));
        }
        findings.push(Finding { severity, message });
    }

    if let (Some(profile_ents), Some(app_ents)) = (entitlements, expect.entitlements) {
        for cap in expect.capabilities {
            if let Some(finding) = value_mismatch(cap, profile_ents, app_ents) {
                findings.push(finding);
            }
        }
    }

    findings
}

/// A profile for `<team>.*` or `*` covers any bundle ID under its prefix.
fn bundle_id_matches(profile_bundle: &str, bundle_id: &str) -> bool {
    match profile_bundle.strip_suffix('*') {
        Some(prefix) => bundle_id.starts_with(prefix),
        None => profile_bundle == bundle_id,
    }
}

/// Warning for string array values that the app lists and the profile does
/// not. A `*` entry in the profile covers any value. Values of any other
/// type are not compared.
fn value_mismatch(
    cap: &CapabilityDef,
    profile_ents: &plist::Dictionary,
    app_ents: &serde_json::Value,
) -> Option<Finding> {
    let key = cap
        .entitlement_keys
        .iter()
        .find(|k| profile_ents.contains_key(k) && app_ents.get(k).is_some())?;
    let profile_values: Vec<&str> = profile_ents
        .get(key)?
        .as_array()?
        .iter()
        .map(|v| v.as_string())
        .collect::<Option<_>>()?;
    let app_values: Vec<&str> = app_ents
        .get(key)?
        .as_array()?
        .iter()
        .map(|v| v.as_str())
        .collect::<Option<_>>()?;
    if profile_values.contains(&"*") {
        return None;
    }
    let missing: Vec<&str> = app_values
        .into_iter()
        .filter(|v| !profile_values.contains(v))
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(Finding::warning(format!(
        "Pinned profile does not list {} for {key}.",
        missing.join(", ")
    )))
}

/// Portal identifiers that must be assigned to a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct IdentifierAssignment {
    pub capability: &'static str,
    pub values: Vec<String>,
}

/// Changes strudel makes for one bundle ID, shown before they are made.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProvisioningPlan {
    pub bundle_id: String,
    /// `"IOS"` or `"MAC_OS"`.
    pub platform: &'static str,
    /// Bundle ID is not registered on the portal.
    pub register_bundle_id: bool,
    /// Required capabilities, for verification notes.
    pub capabilities: Vec<&'static CapabilityDef>,
    /// Capabilities not yet enabled on the bundle ID.
    pub capabilities_to_enable: Vec<&'static CapabilityDef>,
    /// Identifiers to assign, read from the entitlements JSON.
    pub identifiers: Vec<IdentifierAssignment>,
    pub profile_type: &'static str,
    /// Entitlement keys the current profile lacks.
    pub missing_entitlements: Vec<&'static str>,
    /// Config key or file that lists the capabilities.
    pub config_location: String,
    /// Why this bundle ID is managed, if it was opted in implicitly.
    pub opt_in_reason: Option<String>,
}

#[allow(dead_code)]
impl ProvisioningPlan {
    /// Plan for `request`. `registered` says whether the bundle ID exists on
    /// the portal, and `enabled` lists capabilities already enabled on it.
    /// `current_profile` is the decoded cached profile, if any.
    pub fn new(
        request: &ProfileRequest,
        registered: bool,
        enabled: &HashSet<CapabilityType>,
        capabilities: &[&'static CapabilityDef],
        entitlements: Option<&serde_json::Value>,
        current_profile: Option<&plist::Dictionary>,
    ) -> Self {
        let capabilities_to_enable = capabilities
            .iter()
            .copied()
            .filter(|c| !registered || !enabled.contains(&c.capability_type))
            .collect();
        let identifiers = capabilities
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
        let missing_entitlements = current_profile
            .map(|dict| missing_entitlement_keys(dict, capabilities))
            .unwrap_or_default();
        ProvisioningPlan {
            bundle_id: request.bundle_id.clone(),
            platform: request.platform,
            register_bundle_id: !registered,
            capabilities: capabilities.to_vec(),
            capabilities_to_enable,
            identifiers,
            profile_type: request.profile_type,
            missing_entitlements,
            config_location: "strudel.toml".to_string(),
            opt_in_reason: None,
        }
    }

    /// Whether the plan changes anything on the portal.
    pub fn has_portal_changes(&self) -> bool {
        self.register_bundle_id || !self.capabilities_to_enable.is_empty()
    }

    /// Notes about table facts that are not verified for this platform.
    pub fn verification_notes(&self) -> Vec<String> {
        if self.platform != "MAC_OS" {
            return Vec::new();
        }
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
        let mut lines = vec![format!("Bundle ID {}:", self.bundle_id)];
        if self.register_bundle_id {
            lines.push("  - Register the bundle ID.".to_string());
        }
        if !self.capabilities_to_enable.is_empty() {
            let names: Vec<&str> = self.capabilities_to_enable.iter().map(|c| c.name).collect();
            lines.push(format!("  - Enable capabilities: {}.", names.join(", ")));
        }
        if self.missing_entitlements.is_empty() {
            lines.push(format!("  - Create a {} profile.", self.profile_type));
        } else {
            lines.push(format!(
                "  - Create a {} profile. The current profile lacks {}.",
                self.profile_type,
                self.missing_entitlements.join(", ")
            ));
        }
        if let Some(reason) = &self.opt_in_reason {
            lines.push(format!("  Managed because {reason}."));
        }
        lines.push(format!(
            "  Capabilities are listed in {}.",
            self.config_location
        ));
        if !self.identifiers.is_empty() {
            lines.push("Assign identifiers on the portal:".to_string());
            lines.push(format!(
                "  1. Open https://developer.apple.com/account/resources/identifiers/list and \
                 select {}.",
                self.bundle_id
            ));
            for (i, id) in self.identifiers.iter().enumerate() {
                lines.push(format!(
                    "  {}. Configure {} with {}.",
                    i + 2,
                    id.capability,
                    id.values.join(", ")
                ));
            }
        }
        for note in self.verification_notes() {
            lines.push(format!("Note: {note}"));
        }
        lines.join("\n")
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

/// Read a provisioning profile's `application-identifier` entitlement,
/// checking both key spellings Apple uses for it: `com.apple.` prefixed (seen
/// in a macOS Developer ID / `MAC_APP_DIRECT` profile) and unprefixed (seen
/// elsewhere, e.g. iOS profiles). Prefers the prefixed form when a profile
/// somehow carries both.
pub fn application_identifier(entitlements: &plist::Dictionary) -> Option<&str> {
    entitlements
        .get("com.apple.application-identifier")
        .or_else(|| entitlements.get("application-identifier"))
        .and_then(|v| v.as_string())
}

/// Verify that `identity`'s certificate is listed in the profile's
/// `DeveloperCertificates`. Signing and embedding a profile independently
/// chosen from the identity produces a bundle that passes local
/// verification (`codesign --verify --deep --strict`) yet is silently
/// refused by the OS at launch/install time, since the profile can't be
/// bound to a signature it doesn't authorize - see the strudel README's
/// "Signing & notarization" section. Shared by the iOS device and macOS
/// signing pipelines.
///
/// Skips the check (returns `Ok`) when the profile carries no
/// `DeveloperCertificates` array, or when `identity`'s fingerprint can't be
/// resolved from the keychain (e.g. an empty/ad-hoc identity) - those cases
/// are reported by other, earlier checks.
///
/// `remedy` is appended to the error and should tell the caller's platform
/// how to get a profile that actually authorizes `identity`: `strudel profile
/// fetch --force` for a strudel-managed profile, or a new profile from the
/// Apple Developer portal for a pinned one.
pub fn check_identity_authorized(
    identity: &str,
    profile: &plist::Value,
    remedy: &str,
) -> Result<()> {
    if identity_authorized(identity, profile)? != Some(false) {
        return Ok(());
    }
    bail!(
        "Signing identity {identity:?} is not authorized by the provisioning profile: its \
         certificate is not in the profile's DeveloperCertificates.\n\
         Either the wrong `identity`/`team_id` is configured, or the profile was issued for a \
         different certificate. {remedy}"
    );
}

/// Whether `profile`'s `DeveloperCertificates` lists `identity`'s
/// certificate. Returns `None` when this cannot be determined: the profile
/// has no `DeveloperCertificates` array, or `identity`'s fingerprint is not
/// found in the keychain (e.g. an empty/ad-hoc identity).
pub fn identity_authorized(identity: &str, profile: &plist::Value) -> Result<Option<bool>> {
    let Some(certs) = profile
        .as_dictionary()
        .and_then(|d| d.get("DeveloperCertificates"))
        .and_then(|v| v.as_array())
    else {
        return Ok(None);
    };

    let id_out = std::process::Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .context("Failed to run `security find-identity`")?;
    let id_stdout = String::from_utf8_lossy(&id_out.stdout);

    let Some(signing_fp) = id_stdout
        .lines()
        .find(|l| l.contains(identity))
        .and_then(|l| parse_identity_line(l).map(|(hash, _)| hash.to_ascii_uppercase()))
    else {
        return Ok(None);
    };

    for cert_val in certs {
        let Some(cert_data) = cert_val.as_data() else {
            continue;
        };

        let mut child = std::process::Command::new("openssl")
            .args(["x509", "-inform", "DER", "-noout", "-fingerprint", "-sha1"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run `openssl x509`")?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(cert_data);
        }
        let fp_out = child.wait_with_output().context("openssl x509 failed")?;
        let fp_str = String::from_utf8_lossy(&fp_out.stdout);
        if parse_fingerprint(&fp_str).as_deref() == Some(&signing_fp) {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::{Duration, SystemTime};

    use plist::{Dictionary, Value};

    use super::{
        Finding, IdentifierAssignment, PinnedExpectations, ProfileRequest, ProvisioningPlan,
        StaleReason, check_pinned_dict, profile_is_current, stale_reason,
    };
    use crate::apple::capabilities::{self, CapabilityDef, CapabilityType, Severity};
    use crate::config::Platform;

    fn cap(name: &str) -> &'static CapabilityDef {
        capabilities::by_name(name).unwrap()
    }

    /// `dict_is_current` as it was before capability checks.
    fn dict_is_current(
        dict: &Dictionary,
        now: SystemTime,
        required_udids: &[&str],
        bundle_id: &str,
        team_id: &str,
    ) -> bool {
        stale_reason(dict, now, required_udids, bundle_id, team_id, &[]).is_none()
    }

    const BUNDLE_ID: &str = "com.example.app";
    const TEAM_ID: &str = "TEAM123456";

    /// A fixed "now" well clear of the epoch, so tests can subtract from it
    /// without `SystemTime` underflowing.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    /// A profile that passes every check: expires in an hour, provisions both
    /// devices, and carries the matching `application-identifier`.
    fn valid_profile() -> Dictionary {
        let mut entitlements = Dictionary::new();
        entitlements.insert(
            "application-identifier".into(),
            format!("{TEAM_ID}.{BUNDLE_ID}").into(),
        );

        let mut dict = Dictionary::new();
        dict.insert("ExpirationDate".into(), Value::Date(expires_in(3600)));
        dict.insert(
            "ProvisionedDevices".into(),
            Value::Array(vec!["AAA".into(), "BBB".into()]),
        );
        dict.insert("Entitlements".into(), Value::Dictionary(entitlements));
        dict
    }

    fn expires_in(secs: u64) -> plist::Date {
        (now() + Duration::from_secs(secs)).into()
    }

    /// `dict_is_current` with the fixture's bundle and team, so each test only
    /// varies the thing it is about.
    fn is_current(dict: &Dictionary, required_udids: &[&str]) -> bool {
        dict_is_current(dict, now(), required_udids, BUNDLE_ID, TEAM_ID)
    }

    #[test]
    fn missing_file_returns_false() {
        let result = profile_is_current(
            std::path::Path::new("/nonexistent/path.mobileprovision"),
            &[],
            BUNDLE_ID,
            "",
            &[],
        )
        .unwrap();
        assert!(!result);
    }

    #[test]
    fn valid_profile_is_current() {
        assert!(is_current(&valid_profile(), &["AAA", "BBB"]));
    }

    #[test]
    fn expiry_inside_the_five_minute_window_is_not_current() {
        // A profile about to expire is treated as stale: signing with it would
        // produce a build that stops launching minutes later.
        let mut dict = valid_profile();
        dict.insert("ExpirationDate".into(), Value::Date(expires_in(299)));
        assert!(!is_current(&dict, &[]), "299s out is inside the window");

        dict.insert("ExpirationDate".into(), Value::Date(expires_in(300)));
        assert!(!is_current(&dict, &[]), "the boundary is exclusive");

        dict.insert("ExpirationDate".into(), Value::Date(expires_in(301)));
        assert!(is_current(&dict, &[]), "301s out is outside the window");
    }

    #[test]
    fn already_expired_is_not_current() {
        let mut dict = valid_profile();
        dict.insert(
            "ExpirationDate".into(),
            Value::Date((now() - Duration::from_secs(1)).into()),
        );
        assert!(!is_current(&dict, &[]));
    }

    #[test]
    fn missing_expiration_date_is_not_current() {
        let mut dict = valid_profile();
        dict.remove("ExpirationDate");
        assert!(!is_current(&dict, &[]));
    }

    #[test]
    fn a_required_udid_missing_from_the_profile_is_not_current() {
        // This is what triggers a re-fetch after `strudel device register`.
        let dict = valid_profile();
        assert!(!is_current(&dict, &["AAA", "CCC"]));
        assert!(!is_current(&dict, &["CCC"]));
    }

    #[test]
    fn no_required_udids_skips_the_device_check() {
        // A macOS-style profile has no ProvisionedDevices at all.
        let mut dict = valid_profile();
        dict.remove("ProvisionedDevices");
        assert!(is_current(&dict, &[]));
        assert!(!is_current(&dict, &["AAA"]));
    }

    #[test]
    fn application_identifier_mismatch_is_not_current() {
        // A profile for a different app, or issued under a different team, must
        // not be reused just because it happens to be cached at this path.
        let mut dict = valid_profile();
        let mut entitlements = Dictionary::new();
        entitlements.insert(
            "application-identifier".into(),
            format!("{TEAM_ID}.com.example.other").into(),
        );
        dict.insert("Entitlements".into(), Value::Dictionary(entitlements));
        assert!(!is_current(&dict, &[]));

        assert!(
            !dict_is_current(&valid_profile(), now(), &[], BUNDLE_ID, "OTHERTEAM"),
            "same bundle id under another team must not match"
        );
    }

    #[test]
    fn missing_entitlements_is_not_current_when_team_id_is_set() {
        let mut dict = valid_profile();
        dict.remove("Entitlements");
        assert!(!is_current(&dict, &[]));
    }

    #[test]
    fn empty_team_id_skips_the_entitlement_check() {
        // team_id is unset until the user configures one, and an unconfigured
        // project should still be able to reuse a cached profile.
        let mut dict = valid_profile();
        dict.remove("Entitlements");
        assert!(dict_is_current(&dict, now(), &[], BUNDLE_ID, ""));
    }

    /// `valid_profile` with one extra entitlement.
    fn profile_with(key: &str, value: Value) -> Dictionary {
        let mut dict = valid_profile();
        let mut entitlements = dict
            .get("Entitlements")
            .and_then(|v| v.as_dictionary())
            .cloned()
            .unwrap();
        entitlements.insert(key.into(), value);
        dict.insert("Entitlements".into(), Value::Dictionary(entitlements));
        dict
    }

    fn reason(dict: &Dictionary, caps: &[&CapabilityDef]) -> Option<StaleReason> {
        stale_reason(dict, now(), &[], BUNDLE_ID, TEAM_ID, caps)
    }

    #[test]
    fn profile_missing_a_required_entitlement_is_stale() {
        let caps = [cap("Network Extensions")];
        assert_eq!(
            reason(&valid_profile(), &caps),
            Some(StaleReason::MissingEntitlements(vec![
                "com.apple.developer.networking.networkextension"
            ]))
        );
    }

    #[test]
    fn profile_with_the_entitlement_is_current() {
        let dict = profile_with(
            "com.apple.developer.networking.networkextension",
            Value::Array(vec!["packet-tunnel-provider".into()]),
        );
        assert_eq!(reason(&dict, &[cap("Network Extensions")]), None);
    }

    #[test]
    fn either_push_key_satisfies_push_notifications() {
        let caps = [cap("Push Notifications")];
        for key in ["aps-environment", "com.apple.developer.aps-environment"] {
            let dict = profile_with(key, "development".into());
            assert_eq!(reason(&dict, &caps), None, "{key}");
        }
    }

    #[test]
    fn stale_reasons_are_specific() {
        let mut dict = valid_profile();
        let reason = stale_reason(&dict, now(), &["CCC"], BUNDLE_ID, TEAM_ID, &[]);
        assert_eq!(
            reason,
            Some(StaleReason::MissingDevices(vec!["CCC".into()]))
        );

        dict.insert("ExpirationDate".into(), Value::Date(expires_in(10)));
        let reason = stale_reason(&dict, now(), &[], BUNDLE_ID, TEAM_ID, &[]).unwrap();
        assert!(matches!(reason, StaleReason::Expiring(_)));
        assert!(reason.to_string().starts_with("the profile expires at "));
    }

    fn expectations<'a>(
        platform: Platform,
        caps: &'a [&'static CapabilityDef],
        entitlements: Option<&'a serde_json::Value>,
    ) -> PinnedExpectations<'a> {
        PinnedExpectations {
            platform,
            bundle_id: BUNDLE_ID,
            team_id: TEAM_ID,
            required_udids: &[],
            capabilities: caps,
            entitlements,
        }
    }

    fn pinned(dict: &Dictionary, expect: &PinnedExpectations) -> Vec<Finding> {
        check_pinned_dict(dict, expect, now())
    }

    fn severities(findings: &[Finding]) -> Vec<Severity> {
        findings.iter().map(|f| f.severity).collect()
    }

    #[test]
    fn pinned_profile_missing_a_verified_key_is_an_error() {
        let caps = [cap("Network Extensions")];
        let findings = pinned(
            &valid_profile(),
            &expectations(Platform::Macos, &caps, None),
        );
        assert_eq!(severities(&findings), [Severity::Error]);
        assert!(findings[0].message.contains("networkextension"));
    }

    #[test]
    fn pinned_profile_missing_an_unverified_key_is_a_warning() {
        let caps = [cap("App Groups")];
        let findings = pinned(
            &valid_profile(),
            &expectations(Platform::Macos, &caps, None),
        );
        assert_eq!(severities(&findings), [Severity::Warning]);
        assert!(findings[0].message.contains("has not verified"));
    }

    #[test]
    fn pinned_profile_missing_a_key_for_an_unknown_capability_is_a_warning() {
        let caps = [cap("iCloud")];
        let findings = pinned(
            &valid_profile(),
            &expectations(Platform::Macos, &caps, None),
        );
        assert_eq!(severities(&findings), [Severity::Warning]);
    }

    #[test]
    fn pinned_macos_profile_skips_capabilities_that_need_no_profile() {
        let caps = [cap("Hotspot")];
        let findings = pinned(
            &valid_profile(),
            &expectations(Platform::Macos, &caps, None),
        );
        assert_eq!(findings, []);
    }

    #[test]
    fn pinned_ios_profile_missing_a_key_is_a_warning() {
        let caps = [cap("Hotspot"), cap("Network Extensions")];
        let findings = pinned(&valid_profile(), &expectations(Platform::Ios, &caps, None));
        assert_eq!(
            severities(&findings),
            [Severity::Warning, Severity::Warning]
        );
    }

    #[test]
    fn expired_pinned_profile_is_an_error() {
        let mut dict = valid_profile();
        dict.insert(
            "ExpirationDate".into(),
            Value::Date((now() - Duration::from_secs(1)).into()),
        );
        let findings = pinned(&dict, &expectations(Platform::Macos, &[], None));
        assert_eq!(severities(&findings), [Severity::Error]);
        assert!(findings[0].message.contains("expired"));
    }

    #[test]
    fn pinned_profile_for_another_team_or_bundle_is_an_error() {
        let mut expect = expectations(Platform::Macos, &[], None);
        expect.team_id = "OTHERTEAM";
        assert_eq!(
            severities(&pinned(&valid_profile(), &expect)),
            [Severity::Error]
        );

        let mut expect = expectations(Platform::Macos, &[], None);
        expect.bundle_id = "com.example.other";
        assert_eq!(
            severities(&pinned(&valid_profile(), &expect)),
            [Severity::Error]
        );
    }

    #[test]
    fn wildcard_pinned_profile_matches_bundle_ids_under_its_prefix() {
        let dict = profile_with(
            "application-identifier",
            format!("{TEAM_ID}.com.example.*").into(),
        );
        let findings = pinned(&dict, &expectations(Platform::Macos, &[], None));
        assert_eq!(findings, []);
    }

    #[test]
    fn pinned_profile_missing_devices_is_a_warning() {
        let mut expect = expectations(Platform::Ios, &[], None);
        expect.required_udids = &["AAA", "CCC"];
        let findings = pinned(&valid_profile(), &expect);
        assert_eq!(severities(&findings), [Severity::Warning]);
        assert!(findings[0].message.contains("CCC"));
        assert!(!findings[0].message.contains("AAA"));
    }

    #[test]
    fn signature_enforced_keys_are_not_reported() {
        // The profile carries none of these, and the app declares them.
        let app: serde_json::Value = serde_json::json!({
            "com.apple.security.app-sandbox": true,
            "com.apple.security.network.client": true,
            "com.apple.developer.team-identifier": "X",
        });
        let findings = pinned(
            &valid_profile(),
            &expectations(Platform::Macos, &[], Some(&app)),
        );
        assert_eq!(findings, []);
    }

    #[test]
    fn pinned_profile_value_mismatch_is_a_warning() {
        let caps = [cap("App Groups")];
        let key = "com.apple.security.application-groups";
        let app = serde_json::json!({ key: ["group.a", "group.b"] });
        let dict = profile_with(key, Value::Array(vec!["group.a".into()]));
        let findings = pinned(&dict, &expectations(Platform::Macos, &caps, Some(&app)));
        assert_eq!(severities(&findings), [Severity::Warning]);
        assert!(findings[0].message.contains("group.b"));
        assert!(!findings[0].message.contains("group.a"));
    }

    #[test]
    fn pinned_profile_wildcard_value_covers_the_app_value() {
        let caps = [cap("iCloud")];
        let key = "com.apple.developer.icloud-container-identifiers";
        let app = serde_json::json!({ key: ["iCloud.com.example"] });
        let dict = profile_with(key, Value::Array(vec!["*".into()]));
        let findings = pinned(&dict, &expectations(Platform::Macos, &caps, Some(&app)));
        assert_eq!(findings, []);
    }

    fn plan_for(
        platform: &'static str,
        registered: bool,
        enabled: &[CapabilityType],
        caps: &[&'static CapabilityDef],
        entitlements: Option<&serde_json::Value>,
    ) -> ProvisioningPlan {
        let request = if platform == "MAC_OS" {
            ProfileRequest::macos(BUNDLE_ID.into(), "App".into(), "/tmp/p".into())
        } else {
            ProfileRequest::ios(
                BUNDLE_ID.into(),
                "App".into(),
                "/tmp/p".into(),
                vec![],
                vec![],
            )
        };
        let enabled: HashSet<CapabilityType> = enabled.iter().copied().collect();
        ProvisioningPlan::new(&request, registered, &enabled, caps, entitlements, None)
    }

    #[test]
    fn plan_enables_only_missing_capabilities() {
        let caps = [cap("HealthKit"), cap("Push Notifications")];
        let plan = plan_for("IOS", true, &[CapabilityType::Healthkit], &caps, None);
        let names: Vec<&str> = plan.capabilities_to_enable.iter().map(|c| c.name).collect();
        assert_eq!(names, ["Push Notifications"]);
        assert!(!plan.register_bundle_id);
        assert!(plan.has_portal_changes());

        let plan = plan_for("IOS", true, &[CapabilityType::Healthkit], &caps[..1], None);
        assert!(!plan.has_portal_changes());
    }

    #[test]
    fn plan_for_an_unregistered_bundle_id_enables_everything() {
        let caps = [cap("HealthKit")];
        let plan = plan_for("IOS", false, &[CapabilityType::Healthkit], &caps, None);
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
        let caps = [cap("App Groups"), cap("HealthKit"), cap("iCloud")];
        let plan = plan_for("IOS", false, &[], &caps, Some(&ents));
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
        let text = plan.render();
        assert!(text.contains("Register the bundle ID."));
        assert!(text.contains("Enable capabilities: App Groups, HealthKit, iCloud."));
        assert!(
            text.contains("1. Open https://developer.apple.com/account/resources/identifiers/list")
        );
        assert!(
            text.contains("2. Configure App Groups with group.com.example.a, group.com.example.b.")
        );
        assert!(text.contains("3. Configure iCloud with iCloud.com.example."));
    }

    #[test]
    fn plan_notes_unverified_profile_requirements_on_macos_only() {
        let caps = [cap("Push Notifications"), cap("Network Extensions")];
        let plan = plan_for("MAC_OS", true, &[], &caps, None);
        assert_eq!(
            plan.verification_notes(),
            ["strudel has not verified whether Push Notifications requires a profile on macOS."]
        );
        assert!(plan.render().contains("Note: strudel has not verified"));

        let plan = plan_for("IOS", true, &[], &caps, None);
        assert!(plan.verification_notes().is_empty());
    }

    #[test]
    fn plan_reports_entitlements_missing_from_the_current_profile() {
        let request = ProfileRequest::macos(BUNDLE_ID.into(), "App".into(), "/tmp/p".into());
        let caps = [cap("Network Extensions")];
        let profile = valid_profile();
        let plan =
            ProvisioningPlan::new(&request, true, &HashSet::new(), &caps, None, Some(&profile));
        assert_eq!(
            plan.missing_entitlements,
            ["com.apple.developer.networking.networkextension"]
        );
        assert!(plan.render().contains("The current profile lacks"));
    }
}
