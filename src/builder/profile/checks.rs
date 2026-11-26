//! Content checks of a decoded provisioning profile. Shared by the staleness
//! check of managed profiles and the pinned-profile check.

use std::path::Path;
use std::time::{Duration, SystemTime};

use super::{application_identifier, decode_profile, identity_authorized};
use crate::apple::capabilities::CapabilityDef;

/// What a profile must satisfy.
#[derive(Debug, Clone, Copy)]
pub struct ProfileExpectations<'a> {
    pub bundle_id: &'a str,
    /// Empty if no team is configured. Then the team is not checked.
    pub team_id: &'a str,
    /// Device UDIDs the profile must list. Empty for a profile without
    /// devices, such as Developer ID.
    pub required_udids: &'a [&'a str],
    /// Capabilities whose entitlement keys the profile must grant.
    pub capabilities: &'a [&'static CapabilityDef],
}

/// Content check failure of a profile. Display text starts with `profile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileProblem {
    NoExpiration,
    /// Profile expired, or expires within 5 minutes. Holds the expiry date.
    Expiring(String),
    NoApplicationIdentifier,
    WrongTeam {
        expected: String,
        actual: String,
    },
    WrongBundleId {
        expected: String,
        actual: String,
    },
    /// Required device UDIDs absent from `ProvisionedDevices`.
    MissingDevices(Vec<String>),
    /// Capability with none of its entitlement keys in `Entitlements`.
    MissingCapability(&'static CapabilityDef),
}

impl std::fmt::Display for ProfileProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoExpiration => write!(f, "profile has no expiration date"),
            Self::Expiring(at) => write!(
                f,
                "profile expired or expires within 5 minutes (expiration {at})"
            ),
            Self::NoApplicationIdentifier => {
                write!(f, "profile has no application-identifier entitlement")
            },
            Self::WrongTeam { expected, actual } => {
                write!(f, "profile is for team {actual:?}, expected {expected:?}")
            },
            Self::WrongBundleId { expected, actual } => {
                write!(
                    f,
                    "profile is for bundle ID {actual:?}, expected {expected:?}"
                )
            },
            Self::MissingDevices(udids) => {
                write!(f, "profile does not include devices {}", udids.join(", "))
            },
            Self::MissingCapability(cap) => write!(
                f,
                "profile does not grant {} ({})",
                cap.entitlement_keys[0], cap.name
            ),
        }
    }
}

/// Content checks of a decoded profile. `now` is injected so the expiry
/// window is testable.
///
/// Reports a profile with no `ExpirationDate` or one that expires within 5
/// minutes of `now`. Reports an application identifier (see
/// [`application_identifier`]) that is missing or for another team or bundle
/// ID. A wildcard identifier such as `<team>.com.example.*` covers any bundle
/// ID under its prefix. Also reports absent `required_udids` and each
/// capability in `capabilities` with none of its entitlement keys.
pub fn profile_problems(
    dict: &plist::Dictionary,
    expect: &ProfileExpectations,
    now: SystemTime,
) -> Vec<ProfileProblem> {
    let mut problems = Vec::new();

    match dict.get("ExpirationDate").and_then(|v| v.as_date()) {
        None => problems.push(ProfileProblem::NoExpiration),
        Some(exp) => {
            let cutoff = now.checked_add(Duration::from_secs(300)).unwrap_or(now);
            if SystemTime::from(exp) <= cutoff {
                problems.push(ProfileProblem::Expiring(exp.to_xml_format()));
            }
        },
    }

    let entitlements = dict.get("Entitlements").and_then(|v| v.as_dictionary());
    match entitlements.and_then(application_identifier) {
        None => problems.push(ProfileProblem::NoApplicationIdentifier),
        Some(actual) => {
            let (actual_team, actual_bundle) = actual.split_once('.').unwrap_or(("", actual));
            if !expect.team_id.is_empty() && actual_team != expect.team_id {
                problems.push(ProfileProblem::WrongTeam {
                    expected: expect.team_id.to_string(),
                    actual: actual_team.to_string(),
                });
            }
            if !bundle_id_matches(actual_bundle, expect.bundle_id) {
                problems.push(ProfileProblem::WrongBundleId {
                    expected: expect.bundle_id.to_string(),
                    actual: actual_bundle.to_string(),
                });
            }
        },
    }

    let devices = missing_udids(dict, expect.required_udids);
    if !devices.is_empty() {
        problems.push(ProfileProblem::MissingDevices(devices));
    }

    problems.extend(
        missing_capabilities(dict, expect.capabilities)
            .into_iter()
            .map(ProfileProblem::MissingCapability),
    );

    problems
}

/// Why a cached profile cannot be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    /// No file at the cache path.
    Missing,
    /// File could not be decoded.
    Unreadable,
    /// Content checks failed. See [`profile_problems`].
    Problems(Vec<ProfileProblem>),
    /// Signing identity certificate is not in `DeveloperCertificates`.
    IdentityNotAuthorized,
}

impl StaleReason {
    /// Primary entitlement key of each required capability the profile does
    /// not grant.
    pub fn missing_entitlement_keys(&self) -> Vec<&'static str> {
        let Self::Problems(problems) = self else {
            return Vec::new();
        };
        problems
            .iter()
            .filter_map(|p| match p {
                ProfileProblem::MissingCapability(cap) => Some(cap.entitlement_keys[0]),
                _ => None,
            })
            .collect()
    }
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "no profile is cached"),
            Self::Unreadable => write!(f, "the cached profile cannot be read"),
            Self::Problems(problems) => {
                let text: Vec<String> = problems.iter().map(ToString::to_string).collect();
                write!(f, "{}", text.join("; "))
            },
            Self::IdentityNotAuthorized => {
                write!(f, "the profile does not list the signing certificate")
            },
        }
    }
}

/// Why the profile at `path` cannot be reused, or `None` if it is current.
/// A profile that is absent, undecodable, or fails [`profile_problems`] is
/// stale.
///
/// If `identity` is given, a profile that does not list that identity's
/// certificate is also stale. A certificate belongs to one team, so this also
/// covers the team check if `team_id` is unset. Pass `None` if the identity is
/// not known yet, for example if it comes from `APPLE_CERTIFICATE`, which is
/// imported later. If the certificate comparison cannot be made, the profile
/// is kept and the sign-time check in [`super::check_identity_authorized`]
/// reports any mismatch.
pub fn stale_reason(
    path: &Path,
    expect: &ProfileExpectations,
    identity: Option<&str>,
) -> Option<StaleReason> {
    if !path.exists() {
        return Some(StaleReason::Missing);
    }
    let Ok(profile) = decode_profile(path) else {
        return Some(StaleReason::Unreadable);
    };
    let Some(dict) = profile.as_dictionary() else {
        return Some(StaleReason::Unreadable);
    };
    let problems = profile_problems(dict, expect, SystemTime::now());
    if !problems.is_empty() {
        return Some(StaleReason::Problems(problems));
    }
    match identity {
        Some(identity) if matches!(identity_authorized(identity, &profile), Ok(Some(false))) => {
            Some(StaleReason::IdentityNotAuthorized)
        },
        _ => None,
    }
}

/// A profile for `<team>.*` or `*` covers any bundle ID under its prefix.
fn bundle_id_matches(profile_bundle: &str, bundle_id: &str) -> bool {
    match profile_bundle.strip_suffix('*') {
        Some(prefix) => bundle_id.starts_with(prefix),
        None => profile_bundle == bundle_id,
    }
}

/// Entries of `required_udids` absent from the profile `ProvisionedDevices`.
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

/// Capabilities in `capabilities` for which the profile `Entitlements` has
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

#[cfg(test)]
pub(super) mod tests {
    use std::time::{Duration, SystemTime};

    use plist::{Dictionary, Value};

    use super::*;
    use crate::apple::capabilities;

    pub const BUNDLE_ID: &str = "com.example.app";
    pub const TEAM_ID: &str = "TEAM123456";

    pub fn cap(name: &str) -> &'static CapabilityDef {
        capabilities::by_name(name).unwrap()
    }

    /// A fixed "now" well clear of the epoch, so tests can subtract from it
    /// without `SystemTime` underflowing.
    pub fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    /// A profile that passes every check: expires in an hour, provisions both
    /// devices, and carries the matching `application-identifier`.
    pub fn valid_profile() -> Dictionary {
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

    pub fn expires_in(secs: u64) -> plist::Date {
        (now() + Duration::from_secs(secs)).into()
    }

    /// `valid_profile` with one extra entitlement.
    pub fn profile_with(key: &str, value: Value) -> Dictionary {
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

    fn problems(
        dict: &Dictionary,
        required_udids: &[&str],
        team_id: &str,
        caps: &[&'static CapabilityDef],
    ) -> Vec<ProfileProblem> {
        let expect = ProfileExpectations {
            bundle_id: BUNDLE_ID,
            team_id,
            required_udids,
            capabilities: caps,
        };
        profile_problems(dict, &expect, now())
    }

    /// No problems with the fixture's bundle and team, so each test only
    /// varies the thing it is about.
    fn is_current(dict: &Dictionary, required_udids: &[&str]) -> bool {
        problems(dict, required_udids, TEAM_ID, &[]).is_empty()
    }

    #[test]
    fn missing_file_is_stale() {
        let expect = ProfileExpectations {
            bundle_id: BUNDLE_ID,
            team_id: "",
            required_udids: &[],
            capabilities: &[],
        };
        let reason = stale_reason(
            Path::new("/nonexistent/path.mobileprovision"),
            &expect,
            None,
        );
        assert_eq!(reason, Some(StaleReason::Missing));
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
        let found = problems(&dict, &[], TEAM_ID, &[]);
        assert!(matches!(found[..], [ProfileProblem::Expiring(_)]));
        assert!(found[0].to_string().starts_with("profile expired"));
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
        assert_eq!(
            problems(&dict, &["AAA", "CCC"], TEAM_ID, &[]),
            [ProfileProblem::MissingDevices(vec!["CCC".into()])]
        );
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
        let dict = profile_with(
            "application-identifier",
            format!("{TEAM_ID}.com.example.other").into(),
        );
        assert!(!is_current(&dict, &[]));

        assert_eq!(
            problems(&valid_profile(), &[], "OTHERTEAM", &[]),
            [ProfileProblem::WrongTeam {
                expected: "OTHERTEAM".into(),
                actual: TEAM_ID.into(),
            }],
            "same bundle id under another team must not match"
        );
    }

    #[test]
    fn missing_application_identifier_is_not_current() {
        let mut dict = valid_profile();
        dict.remove("Entitlements");
        assert_eq!(
            problems(&dict, &[], "", &[]),
            [ProfileProblem::NoApplicationIdentifier]
        );
    }

    #[test]
    fn empty_team_id_skips_the_team_check() {
        // team_id is unset until the user configures one, and an unconfigured
        // project should still be able to reuse a cached profile.
        assert!(problems(&valid_profile(), &[], "", &[]).is_empty());

        let other = profile_with(
            "application-identifier",
            format!("{TEAM_ID}.com.example.other").into(),
        );
        assert!(!problems(&other, &[], "", &[]).is_empty());
    }

    #[test]
    fn wildcard_application_identifier_covers_bundle_ids_under_its_prefix() {
        let dict = profile_with(
            "application-identifier",
            format!("{TEAM_ID}.com.example.*").into(),
        );
        assert!(is_current(&dict, &[]));
    }

    #[test]
    fn profile_missing_a_required_entitlement_is_stale() {
        let caps = [cap("Network Extensions")];
        let found = problems(&valid_profile(), &[], TEAM_ID, &caps);
        assert_eq!(found, [ProfileProblem::MissingCapability(caps[0])]);
        assert_eq!(
            StaleReason::Problems(found).missing_entitlement_keys(),
            ["com.apple.developer.networking.networkextension"]
        );
    }

    #[test]
    fn profile_with_the_entitlement_is_current() {
        let dict = profile_with(
            "com.apple.developer.networking.networkextension",
            Value::Array(vec!["packet-tunnel-provider".into()]),
        );
        assert!(problems(&dict, &[], TEAM_ID, &[cap("Network Extensions")]).is_empty());
    }

    #[test]
    fn either_push_key_satisfies_push_notifications() {
        let caps = [cap("Push Notifications")];
        for key in ["aps-environment", "com.apple.developer.aps-environment"] {
            let dict = profile_with(key, "development".into());
            assert!(problems(&dict, &[], TEAM_ID, &caps).is_empty(), "{key}");
        }
    }

    #[test]
    fn stale_reason_lists_every_problem() {
        let mut dict = valid_profile();
        dict.insert("ExpirationDate".into(), Value::Date(expires_in(10)));
        let reason = StaleReason::Problems(problems(&dict, &["CCC"], TEAM_ID, &[]));
        let text = reason.to_string();
        assert!(text.starts_with("profile expired"), "{text}");
        assert!(
            text.ends_with("; profile does not include devices CCC"),
            "{text}"
        );
    }
}
