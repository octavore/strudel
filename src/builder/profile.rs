//! Provisioning-profile decoding, validity checks, and creation, shared by the
//! iOS and macOS pipelines.
//!
//! - [`checks`] content checks and staleness of a cached profile
//! - [`pinned`] local checks of a profile pinned by an explicit path

pub mod checks;
mod pinned;

use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub use self::checks::{
    ProfileExpectations, StaleReason, missing_capabilities, profile_problems, stale_reason,
};
pub use self::pinned::{
    Finding, PinnedExpectations, check_pinned_dict, check_pinned_profile, report_pinned_findings,
};
use crate::apple::appstore::AppStoreClient;
use crate::apple::capabilities::CapabilityDef;
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
/// [`ProfileRequest::stale_reason`] and [`ProfileRequest::provision_for`]; the
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
    /// Platform the bundle ID is registered for.
    pub platform: Platform,
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
            platform: Platform::Macos,
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
            platform: Platform::Ios,
            profile_type: "IOS_APP_DEVELOPMENT",
            device_ids,
            required_udids,
            required_capabilities: Vec::new(),
        }
    }

    /// Why the cached profile cannot be reused, or `None` if it is current.
    /// See [`stale_reason`] for the checks and the use of `identity`.
    pub fn stale_reason(&self, team_id: &str, identity: Option<&str>) -> Option<StaleReason> {
        let udids: Vec<&str> = self.required_udids.iter().map(String::as_str).collect();
        let expect = ProfileExpectations {
            bundle_id: &self.bundle_id,
            team_id,
            required_udids: &udids,
            capabilities: &self.required_capabilities,
        };
        checks::stale_reason(&self.cache_path, &expect, identity)
    }

    /// Find or create the bundle ID on the portal and return its resource ID.
    /// Callers that need the resource ID before creating the profile, such as
    /// to enable capabilities, use this with [`Self::provision_for`].
    pub fn ensure_bundle_id(&self, client: &AppStoreClient) -> Result<String> {
        client.find_or_create_bundle_id(&self.bundle_id, &self.label, self.platform)
    }

    /// Create the profile against `cert_ids` for a bundle ID resource resolved
    /// through [`Self::ensure_bundle_id`], and write it to the cache path.
    /// `cert_ids` must be of the [`CertKind`] this profile type is issued
    /// against; callers list them once per run rather than per profile. Prints
    /// nothing: the App Store Connect client narrates its own lookups, and
    /// callers frame this with their own progress lines.
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
