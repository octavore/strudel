//! Local checks of a profile pinned by an explicit `provisioning_profile`
//! path. These checks make no network calls.

use std::path::Path;
use std::time::SystemTime;

use anyhow::{Result, bail};
use clml::cprintln;

use super::checks::{ProfileExpectations, ProfileProblem, profile_problems};
use super::{decode_profile, identity_authorized};
use crate::apple::capabilities::{CapabilityDef, Severity};
use crate::config::Platform;

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
    /// On macOS the caller passes only capabilities that may require a
    /// profile.
    pub profile: ProfileExpectations<'a>,
    /// Parsed entitlements JSON of the app, if any.
    pub entitlements: Option<&'a serde_json::Value>,
}

/// Check a pinned profile file. A file that is missing or unreadable is an
/// error finding. If `identity` is given, a certificate that the profile does
/// not list is a warning.
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
/// Missing devices are warnings. A missing capability entitlement key is a
/// warning, or an error on macOS if the capability is verified to need a
/// profile. String array values the profile does not cover are warnings.
/// Other problems from [`profile_problems`] are errors. Entitlements that are
/// not in the capability table are not checked, since Developer ID profiles
/// do not list keys that the signature enforces.
pub fn check_pinned_dict(
    dict: &plist::Dictionary,
    expect: &PinnedExpectations,
    now: SystemTime,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = profile_problems(dict, &expect.profile, now)
        .into_iter()
        .filter_map(|problem| finding(problem, expect.platform))
        .collect();

    let entitlements = dict.get("Entitlements").and_then(|v| v.as_dictionary());
    if let (Some(profile_ents), Some(app_ents)) = (entitlements, expect.entitlements) {
        for cap in expect.profile.capabilities {
            if let Some(finding) = value_mismatch(cap, profile_ents, app_ents) {
                findings.push(finding);
            }
        }
    }

    findings
}

/// Finding for `problem` on `platform`. `None` if the problem does not apply.
fn finding(problem: ProfileProblem, platform: Platform) -> Option<Finding> {
    let severity = match &problem {
        ProfileProblem::MissingDevices(_) => Severity::Warning,
        ProfileProblem::MissingCapability(cap) => match platform {
            Platform::Macos if !cap.may_require_profile() => return None,
            Platform::Macos => cap.requires_profile.severity(),
            Platform::Ios => Severity::Warning,
        },
        _ => Severity::Error,
    };
    let mut message = format!("Pinned {problem}.");
    if let ProfileProblem::MissingCapability(cap) = &problem
        && severity == Severity::Warning
        && platform == Platform::Macos
    {
        message.push_str(&format!(
            " strudel has not verified whether {} requires a profile on macOS.",
            cap.name
        ));
    }
    Some(Finding { severity, message })
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

/// Print the warnings in `findings`. Fail if any finding is an error. The
/// error lists every error finding, followed by `remedy`.
pub fn report_pinned_findings(findings: Vec<Finding>, remedy: &str) -> Result<()> {
    let (errors, warnings): (Vec<Finding>, Vec<Finding>) = findings
        .into_iter()
        .partition(|f| f.severity == Severity::Error);
    for finding in &warnings {
        cprintln!("<yellow>warning:</yellow> {}", finding.message);
    }
    if errors.is_empty() {
        return Ok(());
    }
    let list: Vec<String> = errors.iter().map(|f| format!("- {}", f.message)).collect();
    bail!(
        "Configured provisioning profile cannot be used:\n{}\n{remedy}",
        list.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use plist::{Dictionary, Value};

    use super::super::checks::tests::{BUNDLE_ID, TEAM_ID, cap, now, profile_with, valid_profile};
    use super::*;

    fn expectations<'a>(
        platform: Platform,
        caps: &'a [&'static CapabilityDef],
        entitlements: Option<&'a serde_json::Value>,
    ) -> PinnedExpectations<'a> {
        PinnedExpectations {
            platform,
            profile: ProfileExpectations {
                bundle_id: BUNDLE_ID,
                team_id: TEAM_ID,
                required_udids: &[],
                capabilities: caps,
            },
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
        assert!(findings[0].message.starts_with("Pinned profile expired"));
    }

    #[test]
    fn pinned_profile_for_another_team_or_bundle_is_an_error() {
        let mut expect = expectations(Platform::Macos, &[], None);
        expect.profile.team_id = "OTHERTEAM";
        assert_eq!(
            severities(&pinned(&valid_profile(), &expect)),
            [Severity::Error]
        );

        let mut expect = expectations(Platform::Macos, &[], None);
        expect.profile.bundle_id = "com.example.other";
        assert_eq!(
            severities(&pinned(&valid_profile(), &expect)),
            [Severity::Error]
        );
    }

    #[test]
    fn pinned_profile_missing_devices_is_a_warning() {
        let mut expect = expectations(Platform::Ios, &[], None);
        expect.profile.required_udids = &["AAA", "CCC"];
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

    #[test]
    fn report_pinned_findings_fails_on_errors_and_lists_them_all() {
        let warning = Finding::warning("lacks a key".into());
        assert!(report_pinned_findings(vec![warning.clone()], "Fix it.").is_ok());

        let findings = vec![
            Finding::error("expired".into()),
            warning,
            Finding::error("wrong team".into()),
        ];
        let err = report_pinned_findings(findings, "Fix it.")
            .unwrap_err()
            .to_string();
        assert!(err.contains("- expired"), "{err}");
        assert!(err.contains("- wrong team"), "{err}");
        assert!(!err.contains("lacks a key"), "{err}");
        assert!(err.ends_with("Fix it."), "{err}");
    }
}
