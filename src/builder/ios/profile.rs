use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use color_print::cprintln;

use crate::apple::appstore::AppStoreClient;
use crate::apple::provisioning;
use crate::builder::{IosBuilder, step};
use crate::config::IosProvisioningBackend;
use crate::devices::DeviceSet;
use crate::paths::ensure_strudel_dir;

impl IosBuilder {
    /// Fetch (or force-refresh) the development provisioning profile and write
    /// it to `.strudel/<bundle_id>.mobileprovision`.
    pub fn profile_fetch(&self, force: bool) -> Result<()> {
        let cached = &self.paths.cached_profile;
        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        let udids = device_set.udids();

        if !force
            && cached.exists()
            && profile_is_current(cached, &udids, &self.cfg.bundle_id, &self.cfg.team_id)?
        {
            cprintln!(
                "<green>✔</green> Cached profile is current: {}",
                cached.display()
            );
            return Ok(());
        }

        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> Would fetch provisioning profile via App Store Connect API"
            );
            cprintln!("<dim>[dry-run]</dim> Would write to {}", cached.display());
            return Ok(());
        }

        self.auto_fetch_profile()?;
        Ok(())
    }

    /// Resolve the provisioning profile path for a device build.
    ///
    /// Uses the user-configured profile if set (warns if stale), the cached
    /// profile if current, or auto-fetches via the App Store Connect API.
    pub(super) fn resolve_profile(&self, target_udids: &[String]) -> Result<PathBuf> {
        let udid_refs: Vec<&str> = target_udids.iter().map(String::as_str).collect();

        if let Some(ref p) = self.cfg.provisioning_profile {
            if !self.dry_run
                && matches!(
                    profile_is_current(p, &udid_refs, &self.cfg.bundle_id, &self.cfg.team_id),
                    Ok(false)
                )
            {
                cprintln!(
                    "<yellow>warning:</yellow> Configured provisioning profile may be \
                     stale (expired or missing device UDIDs). Proceeding anyway.\n\
                     Remove `provisioning_profile` from strudel.toml to let strudel \
                     manage the profile automatically."
                );
            }
            return Ok(p.clone());
        }

        let cached = &self.paths.cached_profile;

        if !self.dry_run
            && cached.exists()
            && profile_is_current(cached, &udid_refs, &self.cfg.bundle_id, &self.cfg.team_id)?
        {
            cprintln!(
                "<green>✔</green> Using cached profile: {}",
                cached.display()
            );
            return Ok(cached.clone());
        }

        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> Would auto-fetch provisioning profile \
                 via App Store Connect API"
            );
            return Ok(cached.clone());
        }

        self.auto_fetch_profile()?;
        Ok(cached.clone())
    }

    /// Fetch (or re-create) a development profile and write it to the cache.
    /// Routes through the configured provisioning backend.
    fn auto_fetch_profile(&self) -> Result<()> {
        let ios_settings = &self.ios;
        if matches!(ios_settings.provisioning, IosProvisioningBackend::Free) {
            cprintln!(
                "<dim>Using free provisioning (7-day profiles, max 3 devices, max 10 App IDs).</dim>"
            );
            return provisioning::auto_fetch_profile(&self.cfg, &self.paths);
        }

        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        if device_set.device.is_empty() {
            bail!(
                "No devices are tracked in .strudel/devices.toml.\n\
                 Run `strudel devices add` first to register your device(s)."
            );
        }

        let client = AppStoreClient::from_config(&self.cfg)?;

        step("Looking up bundle ID on App Store Connect...");
        let bundle_id_ref =
            client.find_or_create_bundle_id(&self.cfg.bundle_id, &self.cfg.app_name)?;
        cprintln!(
            "<dim>  Bundle ID: {} (portal ID: {})</dim>",
            self.cfg.bundle_id,
            bundle_id_ref
        );

        step("Finding development certificates...");
        let certs = client.list_development_certificates()?;
        cprintln!(
            "<dim>  Found {} development certificate(s)</dim>",
            certs.len()
        );
        let cert_ids: Vec<String> = certs.iter().map(|c| c.id.clone()).collect();

        step("Matching tracked devices to portal...");
        cprintln!("<dim>  Tracked devices: {}</dim>", device_set.device.len());
        let portal_devices = client.list_devices()?;
        cprintln!("<dim>  Portal devices: {}</dim>", portal_devices.len());
        let mut device_ids = Vec::new();
        for tracked in &device_set.device {
            match portal_devices.iter().find(|d| d.udid == tracked.udid) {
                Some(pd) => {
                    cprintln!("<dim>  Matched: {} ({})</dim>", tracked.name, tracked.udid);
                    device_ids.push(pd.id.clone());
                },
                None => bail!(
                    "Device {} ({}) is in .strudel/devices.toml but not found on the \
                     App Store Connect portal.\n\
                     Run `strudel devices add` to re-register your devices.",
                    tracked.name,
                    tracked.udid
                ),
            }
        }

        let profile_name = format!("strudel {} Development", self.cfg.app_name);
        step(&format!(
            "Creating provisioning profile \"{profile_name}\"..."
        ));
        let profile_bytes = client.create_development_profile(
            &profile_name,
            &bundle_id_ref,
            &cert_ids,
            &device_ids,
        )?;

        ensure_strudel_dir(&self.paths.strudel_dir)?;
        fs::write(&self.paths.cached_profile, &profile_bytes)?;
        cprintln!(
            "<green>✔</green> Profile cached at {}",
            self.paths.cached_profile.display()
        );
        Ok(())
    }
}

/// Decode a `.mobileprovision` file's CMS envelope and return the plist value.
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
/// [`dict_is_current`].
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
/// `ProvisionedDevices`, or when the `application-identifier` entitlement is
/// not `<team_id>.<bundle_id>`. An empty `required_udids` skips the device
/// check; an empty `team_id` skips the entitlement check.
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
            .and_then(|d| d.get("application-identifier"))
            .and_then(|v| v.as_string())
            .unwrap_or("");
        if actual != expected {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use plist::{Dictionary, Value};

    use crate::builder::ios::profile::{dict_is_current, profile_is_current};

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
