use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clml::{cformat, cformatdoc};

use crate::apple::appstore::AppStoreClient;
use crate::apple::capabilities::{CapabilityDef, RequiredCapabilities};
use crate::apple::provisioning::FreeProfileTarget;
use crate::apple::{capabilities, provisioning};
use crate::builder::IosBuilder;
use crate::builder::profile::{
    CertKind, PinnedExpectations, ProfileExpectations, ProfileRequest, check_pinned_profile,
    enable_capabilities, report_pinned_findings,
};
use crate::config::{IosProvisioningBackend, Platform};
use crate::devices::DeviceSet;

/// Bundle ID that gets its own development profile. This is the host app or
/// one `[[extensions]]` entry.
pub(super) struct ProfileTarget {
    /// App or extension name, shown in progress lines.
    pub label: String,
    pub bundle_id: String,
    /// Where the managed profile is cached.
    pub cache_path: PathBuf,
    /// Profile pinned by an explicit `provisioning_profile` path.
    pub pinned: Option<PathBuf>,
    pub entitlements_json_path: Option<PathBuf>,
    /// `[ios] capabilities` names. Only the host has these.
    pub capability_names: Vec<String>,
}

impl IosBuilder {
    /// Host app first, then each extension.
    pub(super) fn profile_targets(&self) -> Vec<ProfileTarget> {
        let host = ProfileTarget {
            label: self.cfg.app_name.clone(),
            bundle_id: self.cfg.bundle_id.clone(),
            cache_path: self.paths.cached_profile.clone(),
            pinned: self.cfg.provisioning_profile.clone(),
            entitlements_json_path: self.cfg.entitlements_json_path.clone(),
            capability_names: self.ios.capabilities.clone(),
        };
        let extensions = self.cfg.extensions.iter().map(|ext| ProfileTarget {
            label: ext.name.clone(),
            bundle_id: ext.bundle_id.clone(),
            cache_path: self
                .paths
                .strudel_dir
                .join(format!("{}.mobileprovision", ext.bundle_id)),
            pinned: ext.provisioning_profile.clone(),
            entitlements_json_path: ext.entitlements_json_path.clone(),
            capability_names: Vec::new(),
        });
        std::iter::once(host).chain(extensions).collect()
    }

    /// Fetch (or force-refresh) the development provisioning profile of the
    /// host and each extension, and write them to
    /// `.strudel/<bundle_id>.mobileprovision`.
    ///
    /// `out`, when given, also copies the fetched profiles into that
    /// directory. `.strudel` is gitignored, so this is how a profile ends up
    /// somewhere trackable: commit the copy, then point `provisioning_profile`
    /// at it directly for environments that can't run the interactive fetch.
    pub fn profile_fetch(&self, force: bool, out: Option<&Path>) -> Result<()> {
        let device_set = DeviceSet::load_for(&self.paths.devices_toml, Platform::Ios)?;
        let udids = device_set.udids();
        let mut fetched = Vec::new();

        for target in self.profile_targets() {
            if let Some(pinned) = &target.pinned {
                self.note(cformat!(
                    "<dim>{} ({}) uses a pinned profile: {}</dim>",
                    target.label,
                    target.bundle_id,
                    pinned.display()
                ));
                continue;
            }
            let caps = self.required_capabilities(&target)?.caps;
            let request = self.profile_request(&target, &udids, &caps);

            if !force && request.stale_reason(&self.cfg.team_id, None).is_none() {
                self.note(cformat!(
                    "<green>✔</green> Cached profile for {} is current: {}",
                    target.label,
                    target.cache_path.display()
                ));
            } else if self.dry_run {
                self.echo(cformatdoc! {"
                    <dim>[dry-run]</dim> Would fetch provisioning profile for {} via App Store Connect API
                    <dim>[dry-run]</dim> Would write to {}",
                    target.bundle_id,
                    target.cache_path.display()
                });
                continue;
            } else {
                self.auto_fetch_profile(&target, &caps)?;
            }
            fetched.push(target.cache_path);
        }

        if let Some(out) = out {
            let paths: Vec<&Path> = fetched.iter().map(PathBuf::as_path).collect();
            self.copy_profiles(&paths, out)?;
        }
        Ok(())
    }

    /// Resolve the provisioning profile path of `target` for a device build.
    ///
    /// Uses the user-configured profile if set, the cached profile if
    /// current, or auto-fetches via the App Store Connect API. A configured
    /// profile is checked locally and makes no network calls.
    pub(super) fn resolve_profile(
        &self,
        target: &ProfileTarget,
        target_udids: &[String],
    ) -> Result<PathBuf> {
        let udid_refs: Vec<&str> = target_udids.iter().map(String::as_str).collect();
        let required = self.required_capabilities(target)?;

        if let Some(ref p) = target.pinned {
            if !self.dry_run {
                self.check_configured_profile(target, p, &udid_refs, &required)?;
            }
            return Ok(p.clone());
        }

        let cached = &target.cache_path;

        let request = self.profile_request(target, &udid_refs, &required.caps);
        if !self.dry_run && request.stale_reason(&self.cfg.team_id, None).is_none() {
            self.note(cformat!(
                "<green>✔</green> Using cached profile: {}",
                cached.display()
            ));
            return Ok(cached.clone());
        }

        if self.dry_run {
            self.echo(cformat!(
                "<dim>[dry-run]</dim> Would auto-fetch provisioning profile for {} \
                 via App Store Connect API",
                target.bundle_id
            ));
            return Ok(cached.clone());
        }

        self.auto_fetch_profile(target, &required.caps)?;
        Ok(cached.clone())
    }

    /// Request for the cached development profile of `target`. The profile
    /// must cover `udids` and grant `caps` to count as current.
    fn profile_request(
        &self,
        target: &ProfileTarget,
        udids: &[&str],
        caps: &[&'static CapabilityDef],
    ) -> ProfileRequest {
        ProfileRequest::ios(
            target.bundle_id.clone(),
            target.label.clone(),
            target.cache_path.clone(),
            udids.iter().map(|u| u.to_string()).collect(),
        )
        .with_capabilities(caps.to_vec())
    }

    /// Check the configured profile without network calls. Warnings are
    /// printed. Errors fail the build.
    fn check_configured_profile(
        &self,
        target: &ProfileTarget,
        path: &Path,
        udids: &[&str],
        required: &RequiredCapabilities,
    ) -> Result<()> {
        let expect = PinnedExpectations {
            platform: Platform::Ios,
            profile: ProfileExpectations {
                bundle_id: &target.bundle_id,
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

    /// Fetch (or re-create) the development profile of `target` and write it
    /// to the cache. Routes through the configured provisioning backend.
    /// `caps` are enabled on the bundle ID before the profile is created.
    fn auto_fetch_profile(
        &self,
        target: &ProfileTarget,
        caps: &[&'static CapabilityDef],
    ) -> Result<()> {
        let ios_settings = &self.ios;
        if matches!(ios_settings.provisioning, IosProvisioningBackend::Free) {
            self.note(cformat!(
                "<dim>Using free provisioning (7-day profiles, max 3 devices, max 10 App IDs).</dim>"
            ));
            return provisioning::auto_fetch_profile(
                &FreeProfileTarget {
                    bundle_id: &target.bundle_id,
                    label: &target.label,
                    cache_path: &target.cache_path,
                    team_id: &self.cfg.team_id,
                },
                caps,
                &self.paths,
            );
        }

        let device_set = DeviceSet::load_for(&self.paths.devices_toml, Platform::Ios)?;
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
        let mut request = self.profile_request(target, &device_set.udids(), caps);
        request.device_ids = self.match_devices(&client, &device_set)?;

        // Register the bundle ID before creating the profile. Capabilities
        // must be enabled on the bundle ID for the profile to include them.
        self.step(&format!(
            "Looking up bundle ID {} on App Store Connect...",
            target.bundle_id
        ));
        let bundle_id_ref = request.ensure_bundle_id(&client)?;
        self.note(cformat!(
            "<dim>  Bundle ID: {} (portal ID: {})</dim>",
            target.bundle_id,
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

    /// Capabilities to enable on the bundle ID of `target`, from entitlement
    /// keys found in its entitlements JSON and `[ios] capabilities` names.
    /// Prints a warning for entitlement keys that match no capability.
    fn required_capabilities(&self, target: &ProfileTarget) -> Result<RequiredCapabilities> {
        let required = capabilities::required(
            target.entitlements_json_path.as_deref(),
            &target.capability_names,
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
    use crate::config::fixtures::{resolved_ios, resolved_ios_extension};

    fn host_target(builder: &IosBuilder) -> ProfileTarget {
        builder.profile_targets().remove(0)
    }

    #[test]
    fn pinned_profile_is_checked_without_network_calls() {
        // No App Store Connect credentials are configured, so any network
        // attempt would fail with a credentials error instead.
        let mut cfg = resolved_ios();
        cfg.provisioning_profile = Some("/nonexistent/pinned.mobileprovision".into());
        let builder = IosBuilder::new(cfg, OutputFlags::default(), false).unwrap();
        let err = builder
            .resolve_profile(&host_target(&builder), &[])
            .unwrap_err()
            .to_string();
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
            .required_capabilities(&host_target(&builder))
            .unwrap()
            .caps
            .iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["HealthKit"]);
    }

    #[test]
    fn profile_targets_list_host_then_extensions_with_own_cache_paths() {
        let mut cfg = resolved_ios();
        cfg.extensions = vec![resolved_ios_extension(
            "AutofillExt",
            "com.example.app.Autofill",
        )];
        let builder = IosBuilder::new(cfg, OutputFlags::default(), false).unwrap();
        let targets = builder.profile_targets();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].bundle_id, builder.cfg.bundle_id);
        assert_eq!(targets[0].cache_path, builder.paths.cached_profile);
        assert_eq!(targets[1].label, "AutofillExt");
        assert_eq!(targets[1].bundle_id, "com.example.app.Autofill");
        assert_eq!(
            targets[1].cache_path,
            builder
                .paths
                .strudel_dir
                .join("com.example.app.Autofill.mobileprovision")
        );
    }

    #[test]
    fn pinned_extension_profile_is_checked_locally() {
        let mut cfg = resolved_ios();
        let mut ext = resolved_ios_extension("AutofillExt", "com.example.app.Autofill");
        ext.provisioning_profile = Some("/nonexistent/ext.mobileprovision".into());
        cfg.extensions = vec![ext];
        let builder = IosBuilder::new(cfg, OutputFlags::default(), false).unwrap();
        let target = builder.profile_targets().remove(1);
        let err = builder
            .resolve_profile(&target, &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("Provisioning profile not found"), "{err}");
    }
}
