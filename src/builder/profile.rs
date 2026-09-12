//! Provisioning-profile decoding, validity checks, and creation, shared by the
//! iOS and macOS pipelines.

use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

use crate::apple::appstore::AppStoreClient;
use crate::apple::fingerprint::parse_fingerprint;
use crate::builder::keychain::parse_identity_line;
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
}

impl ProfileRequest {
    /// A Developer ID profile for a macOS bundle ID.
    pub fn macos(bundle_id: String, label: String, cache_path: PathBuf) -> Self {
        let name = format!("strudel {label} Developer ID");
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
        let name = format!("strudel {label} Development");
        ProfileRequest {
            bundle_id,
            label,
            cache_path,
            name,
            platform: "IOS",
            profile_type: "IOS_APP_DEVELOPMENT",
            device_ids,
            required_udids,
        }
    }

    /// Whether the cached profile can be reused as-is. A profile that is
    /// absent, undecodable, expiring, missing a required device, or issued for
    /// another bundle ID or team counts as not current and is replaced.
    pub fn is_current(&self, team_id: &str) -> bool {
        let udids: Vec<&str> = self.required_udids.iter().map(String::as_str).collect();
        matches!(
            profile_is_current(&self.cache_path, &udids, &self.bundle_id, team_id),
            Ok(true)
        )
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
        let bundle_id_ref =
            client.find_or_create_bundle_id(&self.bundle_id, &self.label, self.platform)?;
        let bytes = client.create_profile(
            &self.name,
            self.profile_type,
            &bundle_id_ref,
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

/// Return `true` when `profile_path` is a valid, current profile for the
/// given `required_udids`, `bundle_id`, and `team_id`. Returns `false` when the
/// file is absent or cannot be decoded, and for the reasons listed on
/// [`dict_is_current`]. Pass an empty `required_udids` for macOS profiles,
/// which have no `ProvisionedDevices` at all.
pub fn profile_is_current(
    profile_path: &Path,
    required_udids: &[&str],
    bundle_id: &str,
    team_id: &str,
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
    Ok(dict_is_current(
        dict,
        SystemTime::now(),
        required_udids,
        bundle_id,
        team_id,
    ))
}

/// The content checks behind [`profile_is_current`], split out from the
/// `security cms` decode so they can be exercised directly. `now` is injected
/// rather than read from the clock, so the expiry window is testable.
///
/// Returns `false` when the profile has no `ExpirationDate` or expires within
/// 5 minutes of `now`, when any of `required_udids` is absent from
/// `ProvisionedDevices`, or when the application-identifier entitlement (see
/// [`application_identifier`]) is not `<team_id>.<bundle_id>`. An empty
/// `required_udids` skips the device check; an empty `team_id` skips the
/// entitlement check.
fn dict_is_current(
    dict: &plist::Dictionary,
    now: SystemTime,
    required_udids: &[&str],
    bundle_id: &str,
    team_id: &str,
) -> bool {
    // Expiration: must not expire within 5 minutes.
    let Some(exp) = dict.get("ExpirationDate").and_then(|v| v.as_date()) else {
        return false;
    };
    let cutoff = now.checked_add(Duration::from_secs(300)).unwrap_or(now);
    if SystemTime::from(exp) <= cutoff {
        return false;
    }

    // Device coverage: every required UDID must appear in ProvisionedDevices.
    if !required_udids.is_empty() {
        let provisioned: Vec<&str> = dict
            .get("ProvisionedDevices")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_string()).collect())
            .unwrap_or_default();
        if !required_udids.iter().all(|u| provisioned.contains(u)) {
            return false;
        }
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
            return false;
        }
    }

    true
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
/// how to get a profile that actually authorizes `identity` - the fix
/// differs between iOS (re-fetch via `strudel profile fetch --force`) and
/// macOS (no such command; the cached profile has to be deleted so the next
/// build re-fetches one).
pub fn check_identity_authorized(
    identity: &str,
    profile: &plist::Value,
    remedy: &str,
) -> Result<()> {
    let Some(certs) = profile
        .as_dictionary()
        .and_then(|d| d.get("DeveloperCertificates"))
        .and_then(|v| v.as_array())
    else {
        return Ok(());
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
        return Ok(());
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
            return Ok(());
        }
    }

    bail!(
        "Signing identity {identity:?} is not authorized by the provisioning profile: its \
         certificate is not in the profile's DeveloperCertificates.\n\
         Either the wrong `identity`/`team_id` is configured, or the profile was issued for a \
         different certificate. {remedy}"
    );
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use plist::{Dictionary, Value};

    use super::{dict_is_current, profile_is_current};

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
}
