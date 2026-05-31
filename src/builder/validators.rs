use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use serde_json::Value;

use super::{Builder, step};

impl Builder {
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

    pub(crate) fn validate_entitlements_for_adhoc(&self, ent_value: &Value, ent_json: &str) {
        let profile_only: &[&str] = &[
            "com.apple.developer.team-identifier",
            "keychain-access-groups",
        ];
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
            cprintln!("<yellow>==> Warning:</yellow> Ad-hoc signing cannot be used with entitlements that
                    require a provisioning profile. The following keys in {ent_json} require a real provisioning profile and signing identity:");
            println!("  {}", bad_keys.join("\n  "));
            println!();
            println!(
                "The app may fail to launch. Set APPLE_SIGNING_IDENTITY env var or signing identity in strudel.toml to your Apple Development or Developer ID certificate, and ensure the corresponding provisioning profile includes these entitlements.",
            );
        }
    }
}
