use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clml::{cformat, cformatdoc};

use crate::apple::appstore::AppStoreClient;
use crate::apple::capabilities::{CapabilityDef, RequiredCapabilities};
use crate::apple::{capabilities, provisioning};
use crate::builder::IosBuilder;
use crate::builder::profile::{
    CertKind, PinnedExpectations, ProfileExpectations, ProfileRequest, check_pinned_profile,
    enable_capabilities, report_pinned_findings,
};
use crate::config::{IosProvisioningBackend, Platform};
use crate::devices::DeviceSet;

impl IosBuilder {
    /// Fetch (or force-refresh) the development provisioning profile and write
    /// it to `.strudel/<bundle_id>.mobileprovision`.
    ///
    /// `out`, when given, also copies the fetched profile into that directory.
    /// `.strudel` is gitignored, so this is how a profile ends up somewhere
    /// trackable: commit the copy, then point `provisioning_profile` at it
    /// directly for environments that can't run the interactive fetch.
    pub fn profile_fetch(&self, force: bool, out: Option<&Path>) -> Result<()> {
        let cached = &self.paths.cached_profile;
        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        let caps = self.required_capabilities()?.caps;
        let request = self.profile_request(&device_set.udids(), &caps);

        if !force && request.stale_reason(&self.cfg.team_id, None).is_none() {
            self.note(cformat!(
                "<green>✔</green> Cached profile is current: {}",
                cached.display()
            ));
        } else if self.dry_run {
            self.echo(cformatdoc! {"
                <dim>[dry-run]</dim> Would fetch provisioning profile via App Store Connect API
                <dim>[dry-run]</dim> Would write to {}",
                cached.display()
            });
            return Ok(());
        } else {
            self.auto_fetch_profile(&caps)?;
        }

        if let Some(out) = out {
            self.copy_profiles(&[cached.as_path()], out)?;
        }
        Ok(())
    }

    /// Resolve the provisioning profile path for a device build.
    ///
    /// Uses the user-configured profile if set, the cached profile if
    /// current, or auto-fetches via the App Store Connect API. A configured
    /// profile is checked locally and makes no network calls.
    pub(super) fn resolve_profile(&self, target_udids: &[String]) -> Result<PathBuf> {
        let udid_refs: Vec<&str> = target_udids.iter().map(String::as_str).collect();
        let required = self.required_capabilities()?;

        if let Some(ref p) = self.cfg.provisioning_profile {
            if !self.dry_run {
                self.check_configured_profile(p, &udid_refs, &required)?;
            }
            return Ok(p.clone());
        }

        let cached = &self.paths.cached_profile;

        let request = self.profile_request(&udid_refs, &required.caps);
        if !self.dry_run && request.stale_reason(&self.cfg.team_id, None).is_none() {
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

        self.auto_fetch_profile(&required.caps)?;
        Ok(cached.clone())
    }

    /// Request for the cached development profile. The profile must cover
    /// `udids` and grant `caps` to count as current.
    fn profile_request(&self, udids: &[&str], caps: &[&'static CapabilityDef]) -> ProfileRequest {
        ProfileRequest::ios(
            self.cfg.bundle_id.clone(),
            self.cfg.app_name.clone(),
            self.paths.cached_profile.clone(),
            udids.iter().map(|u| u.to_string()).collect(),
        )
        .with_capabilities(caps.to_vec())
    }

    /// Check the configured profile without network calls. Warnings are
    /// printed. Errors fail the build.
    fn check_configured_profile(
        &self,
        path: &Path,
        udids: &[&str],
        required: &RequiredCapabilities,
    ) -> Result<()> {
        let expect = PinnedExpectations {
            platform: Platform::Ios,
            profile: ProfileExpectations {
                bundle_id: &self.cfg.bundle_id,
                team_id: &self.cfg.team_id,
                required_udids: udids,
                capabilities: &required.caps,
            },
            entitlements: required.entitlements.as_ref(),
        };
        report_pinned_findings(
            check_pinned_profile(path, &expect, None),
            "Remove `provisioning_profile` from strudel.toml to let strudel manage the profile \
             automatically.",
        )
    }

    /// Fetch (or re-create) a development profile and write it to the cache.
    /// Routes through the configured provisioning backend. `caps` are
    /// enabled on the bundle ID before the profile is created.
    fn auto_fetch_profile(&self, caps: &[&'static CapabilityDef]) -> Result<()> {
        let ios_settings = &self.ios;
        if matches!(ios_settings.provisioning, IosProvisioningBackend::Free) {
            self.note(cformat!(
                "<dim>Using free provisioning (7-day profiles, max 3 devices, max 10 App IDs).</dim>"
            ));
            return provisioning::auto_fetch_profile(&self.cfg, caps, &self.paths);
        }

        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        if device_set.device.is_empty() {
            bail!(
                "No devices are tracked in .strudel/devices.toml.\n\
                 Run `strudel devices add` first to register your device(s)."
            );
        }

        let client = AppStoreClient::from_config(&self.cfg)?.show_progress(!self.echo_suppressed());

        self.step("Finding development certificates...");
        let cert_ids = CertKind::Development.list(&client)?;
        self.note(cformat!(
            "<dim>  Found {} development certificate(s)</dim>",
            cert_ids.len()
        ));

        self.step("Matching tracked devices to portal...");
        let mut request = self.profile_request(&device_set.udids(), caps);
        request.device_ids = self.match_devices(&client, &device_set)?;

        // Register the bundle ID before creating the profile. Capabilities
        // must be enabled on the bundle ID for the profile to include them.
        self.step("Looking up bundle ID on App Store Connect...");
        let bundle_id_ref = request.ensure_bundle_id(&client)?;
        self.note(cformat!(
            "<dim>  Bundle ID: {} (portal ID: {})</dim>",
            self.cfg.bundle_id,
            bundle_id_ref
        ));

        if !caps.is_empty() {
            self.step("Enabling required capabilities on the bundle ID...");
            enable_capabilities(&client, &bundle_id_ref, caps, Platform::Ios)?;
        }

        self.step(&format!(
            "Creating provisioning profile \"{}\"...",
            request.name
        ));
        request.provision_for(&client, &bundle_id_ref, &cert_ids, &self.paths.strudel_dir)?;
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

    /// Capabilities to enable on the bundle ID, from entitlement keys found
    /// in `build.entitlements_json_path` and `[ios] capabilities` names.
    /// Prints a warning for entitlement keys that match no capability.
    fn required_capabilities(&self) -> Result<RequiredCapabilities> {
        let required = capabilities::required(
            self.cfg.entitlements_json_path.as_deref(),
            &self.ios.capabilities,
        )?;
        required.warn_unmatched();
        Ok(required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::OutputFlags;
    use crate::config::ResolvedTargetPlatform;
    use crate::config::fixtures::resolved_ios;

    #[test]
    fn pinned_profile_is_checked_without_network_calls() {
        // No App Store Connect credentials are configured, so any network
        // attempt would fail with a credentials error instead.
        let mut cfg = resolved_ios();
        cfg.provisioning_profile = Some("/nonexistent/pinned.mobileprovision".into());
        let builder = IosBuilder::new(cfg, OutputFlags::default(), false).unwrap();
        let err = builder.resolve_profile(&[]).unwrap_err().to_string();
        assert!(err.contains("Provisioning profile not found"), "{err}");
    }

    #[test]
    fn required_capabilities_merge_config_names() {
        let mut cfg = resolved_ios();
        let ResolvedTargetPlatform::Ios(ios) = &mut cfg.target_platform else {
            unreachable!("resolved_ios is an iOS target");
        };
        ios.capabilities.push("healthkit".to_string());
        let builder = IosBuilder::new(cfg, OutputFlags::default(), false).unwrap();
        let names: Vec<&str> = builder
            .required_capabilities()
            .unwrap()
            .caps
            .iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["HealthKit"]);
    }
}
