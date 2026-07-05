use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use color_print::cprintln;

use crate::builder::{IosBuilder, step};
use crate::appstore::AppStoreClient;
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

        println!();
        cprintln!(
            "<green>Done!</green> Profile written to {}",
            cached.display()
        );
        cprintln!(
            "<dim>Tip: to pin this profile explicitly, add to strudel.toml:\n  [build]\n  provisioning_profile = \"{}\"</dim>",
            cached.display()
        );
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
            return crate::freeprov::auto_fetch_profile(&self.cfg, &self.paths);
        }

        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        if device_set.device.is_empty() {
            bail!(
                "No devices are tracked in .strudel/devices.toml.\n\
                 Run `strudel device register` first to register your device(s)."
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
                     Run `strudel device register` to re-register your devices.",
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
/// given `required_udids`, `bundle_id`, and `team_id`. Returns `false` when:
/// the profile has expired (or expires within 5 minutes), any required UDID
/// is absent from `ProvisionedDevices`, or the `application-identifier`
/// entitlement does not match `<team_id>.<bundle_id>` (when `team_id` is set).
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

    // Expiration: must not expire within 5 minutes.
    if let Some(exp) = dict.get("ExpirationDate").and_then(|v| v.as_date()) {
        let sys_time = SystemTime::from(exp);
        let cutoff = SystemTime::now()
            .checked_add(Duration::from_secs(300))
            .unwrap_or_else(SystemTime::now);
        if sys_time <= cutoff {
            return Ok(false);
        }
    } else {
        return Ok(false);
    }

    // Device coverage: every required UDID must appear in ProvisionedDevices.
    if !required_udids.is_empty() {
        let provisioned: Vec<&str> = dict
            .get("ProvisionedDevices")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_string()).collect())
            .unwrap_or_default();
        for udid in required_udids {
            if !provisioned.contains(udid) {
                return Ok(false);
            }
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
            return Ok(false);
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn profile_is_current_missing_file_returns_false() {
        let result = super::profile_is_current(
            std::path::Path::new("/nonexistent/path.mobileprovision"),
            &[],
            "com.example.app",
            "",
        )
        .unwrap();
        assert!(!result);
    }
}
