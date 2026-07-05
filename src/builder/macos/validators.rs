use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use serde_json::Value;

use crate::builder::{MacosBuilder, step};

impl MacosBuilder {
    /// Describe any signing/notarization credentials that are missing or
    /// incomplete. Empty means a real `run` has everything it needs.
    pub(in crate::builder) fn credential_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.cfg.sign_identity.is_empty() {
            problems.push("APPLE_SIGNING_IDENTITY (signing identity) is not set".to_string());
        }
        if self.cfg.notary_auth().is_none() {
            problems.push(
                "no complete notarization credentials. Provide the App Store Connect API key \
                    (APPLE_API_KEY_PATH, APPLE_API_KEY, APPLE_API_ISSUER)."
                    .to_string(),
            );
        }
        problems
    }

    /// Verify the credentials required for signing and notarization are
    /// present. Bails early so a missing value doesn't surface deep into
    /// the pipeline (e.g. `codesign: no identity found`). In dry-run, only
    /// warns - there's nothing to sign.
    pub(in crate::builder) fn preflight_credentials(&self) -> Result<()> {
        let problems = self.credential_problems();
        if problems.is_empty() {
            return Ok(());
        }

        let hint = "Set identifiers in strudel.toml or the environment, and \
                    secrets (passwords, certificate) in the environment only. \
                    See the README's \"Signing & notarization\" section.";

        if self.dry_run {
            for p in &problems {
                cprintln!("<yellow>[warning]</yellow> {p}");
            }
            cprintln!("<yellow>[warning]</yellow> {hint}");
            Ok(())
        } else {
            let mut msg = String::from("Cannot run signing/notarization:");
            for p in &problems {
                msg.push_str(&format!("\n  - {p}"));
            }
            msg.push_str(&format!("\n{hint}"));
            bail!(msg);
        }
    }

    /// Check that the configured signing identity exists in the keychain by
    /// running `security find-identity -v -p codesigning` and looking for
    /// the identity name in the output. Skipped in dry-run.
    /// Returns `true` if the identity is self-signed (present without `-v`).
    pub(crate) fn validate_sign_identity(&self) -> Result<bool> {
        if self.dry_run {
            return Ok(false);
        }
        step("Validating signing identity...");
        let identity = &self.cfg.sign_identity;

        let identity_in = |args: &[&str]| -> Result<bool> {
            let out = Command::new("security")
                .args(args)
                .output()
                .context("Failed to run `security find-identity`")?;
            Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.contains(&format!("\"{identity}\""))))
        };

        if identity_in(&["find-identity", "-v", "-p", "codesigning"])? {
            cprintln!("<green>✔</green> Signing identity found");
            Ok(false)
        } else if identity_in(&["find-identity", "-p", "codesigning"])? {
            cprintln!(
                "<yellow>warning:</yellow> Signing identity \"{identity}\" is self-signed \
                 and will not be trusted by other machines."
            );
            Ok(true)
        } else {
            bail!(
                "Signing identity \"{identity}\" was not found in the keychain.\n\
                 Run `security find-identity -v -p codesigning` to see available identities."
            );
        }
    }

    /// Decode a provisioning profile with `security cms` and warn about
    /// expiry, team ID mismatches, and bundle ID mismatches.
    pub(crate) fn validate_provisioning_profile(&self, profile_path: &Path) -> Result<()> {
        step("Validating provisioning profile...");
        let profile_str = profile_path.to_str().unwrap();

        // 1. Decode the CMS envelope in memory and parse with the `plist` crate.
        // Capture raw bytes via Command so binary plist data is never corrupted
        // by a UTF-8 String conversion.
        let output = Command::new("security")
            .args(["cms", "-D", "-i", profile_str])
            .output()
            .context("Failed to run `security cms`")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to decode provisioning profile: {stderr}");
        }
        let profile = plist::Value::from_reader(Cursor::new(output.stdout))
            .context("Failed to parse provisioning profile")?;
        let dict = profile
            .as_dictionary()
            .context("Provisioning profile is not a dictionary")?;

        // 2. Check and warn if the profile is expired.
        if let Some(exp) = dict.get("ExpirationDate").and_then(plist::Value::as_date)
            && SystemTime::from(exp) < SystemTime::now()
        {
            cprintln!(
                "<yellow>warning:</yellow> Provisioning profile expired on {:?}",
                SystemTime::from(exp)
            );
        }

        // 3. Check and warn if the config team_id is not in the provisioning profile's
        //    TeamIdentifier array.
        if !self.cfg.team_id.is_empty() {
            let profile_teams: Vec<&str> = dict
                .get("TeamIdentifier")
                .and_then(plist::Value::as_array)
                .map(|a| a.iter().filter_map(plist::Value::as_string).collect())
                .unwrap_or_default();
            if !profile_teams.is_empty() && !profile_teams.contains(&self.cfg.team_id.as_str()) {
                cprintln!(
                    "<yellow>warning:</yellow> Provisioning profile team {profile_teams:?} does not \
                     match configured team_id \"{}\".",
                    self.cfg.team_id
                );
            }
        }

        // 4. Check and warn if the config bundle ID does not match the provisioning
        //    profile's application-identifier entitlement (which is required for the
        //    profile to apply to the app). app_id is "TEAMID.com.example.app" or e.g.
        //    "TEAMID.com.example.*" (wildcard). todo: correctly handle wildcard app_id
        if let Some(app_id) = dict
            .get("Entitlements")
            .and_then(plist::Value::as_dictionary)
            .and_then(|e| e.get("application-identifier"))
            .and_then(plist::Value::as_string)
        {
            let matches = app_id.ends_with(&format!(".{}", self.cfg.bundle_id))
                || app_id == self.cfg.bundle_id.as_str();
            if !matches {
                cprintln!(
                    "<yellow>warning:</yellow> Provisioning profile app identifier \
                     \"{app_id}\" does not match bundle ID \"{}\".",
                    self.cfg.bundle_id
                );
            }
        }

        cprintln!("<green>✔</green> Provisioning profile validated");
        Ok(())
    }

    pub(crate) fn validate_entitlements_for_adhoc(&self, ent_value: &Value) {
        let profile_only: &[&str] = &["keychain-access-groups"];
        let bad_keys: Vec<&str> = profile_only
            .iter()
            .copied()
            .filter(|k| ent_value.get(k).is_some())
            .chain(
                ent_value
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.keys())
                    .filter(|k| k.starts_with("com.apple.developer."))
                    .map(|k| k.as_str()),
            )
            .collect();
        if !bad_keys.is_empty() {
            println!();
            cprintln!(
                "<yellow>warning:</yellow> Ad-hoc/self-signed certificates cannot be used with entitlements that require a provisioning profile. The following entitlement keys require a real signing identity:"
            );
            println!("  {}", bad_keys.join("\n  "));
            println!();
            println!(
                "The app may fail to launch. Set APPLE_SIGNING_IDENTITY env var or signing identity in strudel.toml to your Apple Development or Developer ID certificate, and ensure the corresponding provisioning profile includes these entitlements.",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::config::{ResolvedConfig, ResolvedMacOsSection};

    fn empty_cfg() -> ResolvedConfig {
        ResolvedConfig {
            platform: None,
            app_name: "A".into(),
            bundle_id: "b".into(),
            version: "1".into(),
            build_number: "1".into(),
            source_dir: PathBuf::from("/x"),
            build_dir: PathBuf::from("/x"),
            info_json_path: None,
            entitlements_json_path: None,
            icon_path: None,
            archs: vec!["arm64".into()],
            target_name: "A".into(),
            sign_identity: String::new(),
            notarize_timeout: 600,
            build_env: HashMap::new(),
            embed_libs: Vec::new(),
            provisioning_profile: None,
            extensions: Vec::new(),
            target_platform: ResolvedMacOsSection { dmg: None }.into(),
            team_id: String::new(),
            apple_api_issuer: String::new(),
            apple_api_key: String::new(),
            apple_api_key_path: None,
            apple_certificate: String::new().into(),
            apple_certificate_password: String::new().into(),
            resources_dir: None,
            resources: Vec::new(),
        }
    }

    fn builder(cfg: ResolvedConfig) -> MacosBuilder {
        MacosBuilder::new(cfg, true, false, false, None, false).unwrap()
    }

    #[test]
    fn problems_reports_missing_identity_and_notary() {
        let b = builder(empty_cfg());
        let problems = b.credential_problems();
        assert_eq!(problems.len(), 2);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("APPLE_SIGNING_IDENTITY"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("notarization credentials"))
        );
    }

    #[test]
    fn problems_empty_when_api_key_set() {
        let mut cfg = empty_cfg();
        cfg.sign_identity = "Developer ID Application: X (TEAM)".into();
        cfg.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        cfg.apple_api_key = "KID".into();
        cfg.apple_api_issuer = "ISS".into();
        assert!(builder(cfg).credential_problems().is_empty());
    }

    #[test]
    fn preflight_warns_but_passes_in_dry_run() {
        // Dry-run must not bail: a missing-credential dry-run is the user
        // explicitly checking what `release` would do - they shouldn't have to
        // populate every secret just to preview the pipeline.
        let b = builder(empty_cfg());
        b.preflight_credentials()
            .expect("dry-run preflight must succeed even without credentials");
    }
}
