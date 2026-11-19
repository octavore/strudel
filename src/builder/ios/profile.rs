use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clml::{cformat, cformatdoc, cprintln};

use crate::apple::appstore::AppStoreClient;
use crate::apple::capabilities::{CapabilityDef, CapabilityType, Severity};
use crate::apple::{capabilities, provisioning};
use crate::builder::IosBuilder;
use crate::builder::profile::{
    CertKind, Finding, PinnedExpectations, ProfileRequest, check_pinned_profile, load_entitlements,
    profile_is_current,
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
        let udids = device_set.udids();
        let caps = self.required_capabilities()?;

        let current = !force
            && cached.exists()
            && profile_is_current(
                cached,
                &udids,
                &self.cfg.bundle_id,
                &self.cfg.team_id,
                &caps,
            )?;
        if current {
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
            if self.dry_run {
                self.echo(cformat!(
                    "<dim>[dry-run]</dim> Would copy fetched profile to {}",
                    out.display()
                ));
                return Ok(());
            }
            std::fs::create_dir_all(out)
                .with_context(|| format!("Failed to create {}", out.display()))?;
            let dest = out.join(cached.file_name().unwrap());
            std::fs::copy(cached, &dest).with_context(|| {
                format!("Failed to copy {} to {}", cached.display(), dest.display())
            })?;
            self.note(cformat!("<green>✔</green> Copied to {}", dest.display()));
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
        let caps = self.required_capabilities()?;

        if let Some(ref p) = self.cfg.provisioning_profile {
            if !self.dry_run {
                self.check_configured_profile(p, &udid_refs, &caps)?;
            }
            return Ok(p.clone());
        }

        let cached = &self.paths.cached_profile;

        if !self.dry_run
            && cached.exists()
            && profile_is_current(
                cached,
                &udid_refs,
                &self.cfg.bundle_id,
                &self.cfg.team_id,
                &caps,
            )?
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

        self.auto_fetch_profile(&caps)?;
        Ok(cached.clone())
    }

    /// Check the configured profile without network calls. Warnings are
    /// printed. Errors fail the build.
    fn check_configured_profile(
        &self,
        path: &Path,
        udids: &[&str],
        caps: &[&'static CapabilityDef],
    ) -> Result<()> {
        let entitlements = load_entitlements(self.cfg.entitlements_json_path.as_deref());
        let expect = PinnedExpectations {
            platform: Platform::Ios,
            bundle_id: &self.cfg.bundle_id,
            team_id: &self.cfg.team_id,
            required_udids: udids,
            capabilities: caps,
            entitlements: entitlements.as_ref(),
        };
        report_pinned_findings(check_pinned_profile(path, &expect, None))
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
            return provisioning::auto_fetch_profile(&self.cfg, &self.ios, &self.paths);
        }

        let device_set = DeviceSet::load(&self.paths.devices_toml)?;
        if device_set.device.is_empty() {
            bail!(
                "No devices are tracked in .strudel/devices.toml.\n\
                 Run `strudel devices add` first to register your device(s)."
            );
        }

        let client =
            AppStoreClient::from_config(&self.cfg, true)?.show_progress(!self.echo_suppressed());

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

        // Register the bundle ID before creating the profile. Capabilities
        // must be enabled on the bundle ID for the profile to include them.
        self.step("Looking up bundle ID on App Store Connect...");
        let bundle_id_ref = request.ensure_bundle_id(&client)?;
        self.note(cformat!(
            "<dim>  Bundle ID: {} (portal ID: {})</dim>",
            self.cfg.bundle_id,
            bundle_id_ref
        ));

        let capability_types: Vec<CapabilityType> =
            caps.iter().map(|c| c.capability_type).collect();
        if !capability_types.is_empty() {
            self.step("Enabling required capabilities on the bundle ID...");
            client.ensure_capabilities(&bundle_id_ref, &capability_types)?;
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
    fn required_capabilities(&self) -> Result<Vec<&'static CapabilityDef>> {
        capabilities::required(
            self.cfg.entitlements_json_path.as_deref(),
            &self.ios.capabilities,
        )
    }
}

/// Print the warnings in `findings`. Fail if any finding is an error.
fn report_pinned_findings(findings: Vec<Finding>) -> Result<()> {
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
        "Configured provisioning profile cannot be used:\n{}\n\
         Remove `provisioning_profile` from strudel.toml to let strudel manage the \
         profile automatically.",
        list.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::OutputFlags;
    use crate::config::ResolvedTargetPlatform;
    use crate::config::fixtures::resolved_ios;

    fn finding(severity: Severity, message: &str) -> Finding {
        Finding {
            severity,
            message: message.to_string(),
        }
    }

    #[test]
    fn warnings_do_not_fail() {
        let findings = vec![finding(Severity::Warning, "lacks a key")];
        assert!(report_pinned_findings(findings).is_ok());
    }

    #[test]
    fn errors_fail_and_are_all_listed() {
        let findings = vec![
            finding(Severity::Error, "expired"),
            finding(Severity::Warning, "lacks a key"),
            finding(Severity::Error, "wrong team"),
        ];
        let err = report_pinned_findings(findings).unwrap_err().to_string();
        assert!(err.contains("- expired"));
        assert!(err.contains("- wrong team"));
        assert!(!err.contains("lacks a key"));
        assert!(err.contains("Remove `provisioning_profile`"));
    }

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
            .iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["HealthKit"]);
    }
}
