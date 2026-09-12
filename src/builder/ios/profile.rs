use std::path::PathBuf;

use anyhow::{Result, bail};
use clml::{cformat, cformatdoc, cprintln};

use crate::apple::appstore::AppStoreClient;
use crate::apple::provisioning;
use crate::builder::IosBuilder;
use crate::builder::profile::{CertKind, ProfileRequest, profile_is_current};
use crate::config::IosProvisioningBackend;
use crate::devices::DeviceSet;

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
            self.note(cformat!(
                "<green>✔</green> Cached profile is current: {}",
                cached.display()
            ));
            return Ok(());
        }

        if self.dry_run {
            self.echo(cformatdoc! {"
                <dim>[dry-run]</dim> Would fetch provisioning profile via App Store Connect API
                <dim>[dry-run]</dim> Would write to {}",
                cached.display()
            });
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
            self.note(cformat!(
                "<green>✔</green> Using cached profile: {}",
                cached.display()
            ));
            return Ok(cached.clone());
        }

        if self.dry_run {
            self.echo(cformat!(
                "<dim>[dry-run]</dim> Would auto-fetch provisioning profile \
                 via App Store Connect API"
            ));
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
            self.note(cformat!(
                "<dim>Using free provisioning (7-day profiles, max 3 devices, max 10 App IDs).</dim>"
            ));
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

        self.step("Finding development certificates...");
        let cert_ids = CertKind::Development.list(&client)?;
        self.note(cformat!(
            "<dim>  Found {} development certificate(s)</dim>",
            cert_ids.len()
        ));

        self.step("Matching tracked devices to portal...");
        let device_ids = self.match_devices(&client, &device_set)?;

        let request = ProfileRequest::ios(
            self.cfg.bundle_id.clone(),
            self.cfg.app_name.clone(),
            self.paths.cached_profile.clone(),
            device_ids,
            device_set.udids().into_iter().map(str::to_string).collect(),
        );

        self.step(&format!(
            "Creating provisioning profile \"{}\"...",
            request.name
        ));
        request.provision(&client, &cert_ids, &self.paths.strudel_dir)?;
        self.note(cformat!(
            "<green>✔</green> Profile cached at {}",
            request.cache_path.display()
        ));
        Ok(())
    }

    /// Map each tracked device to its portal resource ID. Every tracked device
    /// must be registered on the portal: a profile silently missing one fails
    /// only at install time, on that device.
    fn match_devices(
        &self,
        client: &AppStoreClient,
        device_set: &DeviceSet,
    ) -> Result<Vec<String>> {
        self.note(cformat!(
            "<dim>  Tracked devices: {}</dim>",
            device_set.device.len()
        ));
        let portal_devices = client.list_devices()?;
        self.note(cformat!(
            "<dim>  Portal devices: {}</dim>",
            portal_devices.len()
        ));

        let mut device_ids = Vec::new();
        for tracked in &device_set.device {
            match portal_devices.iter().find(|d| d.udid == tracked.udid) {
                Some(pd) => {
                    self.note(cformat!(
                        "<dim>  Matched: {} ({})</dim>",
                        tracked.name,
                        tracked.udid
                    ));
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
        Ok(device_ids)
    }
}
