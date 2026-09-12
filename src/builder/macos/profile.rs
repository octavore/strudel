//! Strudel-managed macOS provisioning profiles.
//!
//! A plain Developer ID-signed app generally needs no provisioning profile:
//! most entitlements, including the App Sandbox and the hardened runtime
//! exceptions, are enforced purely by the code signature. A few capabilities
//! are the exception and are checked against an embedded profile even outside
//! the Mac App Store, App Groups and Network Extensions being the two most
//! commonly hit. Those projects set `provisioning_profile = "auto"` and let
//! strudel create, cache, and refresh the profile.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clml::{cformat, cprintln};

use crate::apple::appstore::AppStoreClient;
use crate::builder::MacosBuilder;
use crate::builder::profile::profile_is_current;
use crate::paths::ensure_strudel_dir;

/// A bundle ID whose profile strudel manages: the host app, or one
/// `[[extensions]]` entry.
struct Target {
    /// Label for prompts and progress lines (the app or extension name).
    label: String,
    bundle_id: String,
    cached_profile: PathBuf,
}

impl MacosBuilder {
    /// Create or refresh the Developer ID provisioning profile for every
    /// bundle ID configured with `provisioning_profile = "auto"`, caching each
    /// at `.strudel/<bundle_id>.provisionprofile`. Extensions are covered
    /// alongside the host, against their own bundle IDs.
    ///
    /// No-op, with no App Store Connect calls at all, when nothing opts in or
    /// when every managed profile on disk is already current. `force`
    /// recreates every managed profile regardless, for `strudel profile fetch
    /// --force`. Changes on App Store Connect are confirmed with the user
    /// first.
    ///
    /// Must run before the bundle is assembled, which is what copies each
    /// profile into its bundle.
    pub(crate) fn ensure_profiles(&mut self, force: bool) -> Result<()> {
        let targets = self.managed_profile_targets();
        if targets.is_empty() {
            return Ok(());
        }

        if self.dry_run {
            for t in &targets {
                self.echo(cformat!(
                    "<dim>[dry-run]</dim> Would ensure a Developer ID provisioning profile for \
                     {} ({})",
                    t.label,
                    t.bundle_id
                ));
            }
            return Ok(());
        }

        // Read-only: a project whose profiles are all current never prompts
        // and never touches the network.
        let stale: Vec<&Target> = targets
            .iter()
            .filter(|t| force || !self.profile_is_usable(&t.cached_profile, &t.bundle_id))
            .collect();
        if stale.is_empty() {
            self.note(cformat!(
                "<dim>Managed provisioning profiles are current</dim>"
            ));
            return Ok(());
        }

        self.require_signing_identity()?;

        cprintln!("This needs the following on App Store Connect:");
        for t in &stale {
            cprintln!(
                "  Bundle ID <cyan>{}</cyan> ({}): create a Developer ID provisioning profile",
                t.bundle_id,
                t.label
            );
        }
        let confirmed = inquire::Confirm::new("Make these changes on App Store Connect?")
            .with_default(false)
            .prompt()
            .context("reading confirmation")?;
        if !confirmed {
            bail!(
                "Aborted: `provisioning_profile = \"auto\"` needs a profile strudel doesn't have \
                 yet. Create one manually at \
                 https://developer.apple.com/account/resources/profiles/list and point \
                 `provisioning_profile` at it, or re-run and confirm."
            );
        }

        let client = AppStoreClient::from_config(&self.cfg)?;
        self.step("Finding Developer ID Application certificates...");
        let certs = client.list_developer_id_application_certificates()?;
        let cert_ids: Vec<String> = certs.iter().map(|c| c.id.clone()).collect();

        for t in &stale {
            self.create_profile(&client, t, &cert_ids)?;
        }
        Ok(())
    }

    /// `strudel profile fetch` for a macOS target: do the App Store Connect
    /// work a build would do, up front. Enables configured capabilities (which
    /// is what opts a bundle ID into managed provisioning in the first place),
    /// then creates any profile that's missing or stale. `force` recreates
    /// them even when the cached copies are current.
    ///
    /// Reports when a target has nothing to manage, since that otherwise looks
    /// identical to a successful no-op.
    pub fn profile_fetch(&mut self, force: bool) -> Result<()> {
        if self.managed_profile_targets().is_empty() {
            self.note(cformat!(
                "<dim>No managed provisioning profiles for {}: no capabilities are configured \
                 and `provisioning_profile` isn't set to \"auto\".</dim>",
                self.cfg.app_name
            ));
            return Ok(());
        }
        self.ensure_profiles(force)
    }

    /// Every bundle ID opted into strudel-managed provisioning, host first.
    fn managed_profile_targets(&self) -> Vec<Target> {
        let mut targets = Vec::new();
        if self.cfg.manage_provisioning_profile
            && let Some(path) = &self.cfg.provisioning_profile
        {
            targets.push(Target {
                label: self.cfg.app_name.clone(),
                bundle_id: self.cfg.bundle_id.clone(),
                cached_profile: path.clone(),
            });
        }
        for ext in &self.cfg.extensions {
            if ext.manage_provisioning_profile
                && let Some(path) = &ext.provisioning_profile
            {
                targets.push(Target {
                    label: ext.name.clone(),
                    bundle_id: ext.bundle_id.clone(),
                    cached_profile: path.clone(),
                });
            }
        }
        targets
    }

    /// Whether the cached profile can be reused as-is. macOS profiles carry no
    /// `ProvisionedDevices`, so no UDIDs are required. A profile that can't be
    /// decoded counts as unusable and is replaced.
    fn profile_is_usable(&self, path: &Path, bundle_id: &str) -> bool {
        matches!(
            profile_is_current(path, &[], bundle_id, &self.cfg.team_id),
            Ok(true)
        )
    }

    /// A Developer ID profile is issued against a Developer ID certificate, so
    /// an ad-hoc build has nothing to bind one to. Catch that here rather than
    /// let a profile authorizing no usable identity get embedded and fail at
    /// launch.
    fn require_signing_identity(&self) -> Result<()> {
        if self.cfg.sign_identity.is_empty() && self.cfg.signing_cert().is_none() {
            bail!(
                "`provisioning_profile = \"auto\"` needs a Developer ID signing identity, but \
                 none is configured, so this build would be signed ad-hoc.\n\
                 Set `apple.identity` in strudel.toml (or APPLE_SIGNING_IDENTITY), or remove \
                 `provisioning_profile` for an unsigned local build. See `strudel help signing`."
            );
        }
        Ok(())
    }

    fn create_profile(
        &self,
        client: &AppStoreClient,
        target: &Target,
        cert_ids: &[String],
    ) -> Result<()> {
        self.step(&format!(
            "Provisioning profile for {} ({})...",
            target.label, target.bundle_id
        ));
        let bundle_id_ref =
            client.find_or_create_bundle_id(&target.bundle_id, &target.label, "MAC_OS")?;

        let profile_name = format!("strudel {} Developer ID", target.label);
        // "MAC_APP_DIRECT" is the ASC ProfileType for Developer ID (direct,
        // non-App-Store) distribution profiles. A wrong value surfaces as the
        // API's own error rather than failing silently.
        let profile_bytes = client.create_profile(
            &profile_name,
            "MAC_APP_DIRECT",
            &bundle_id_ref,
            cert_ids,
            &[],
        )?;

        ensure_strudel_dir(&self.paths.strudel_dir)?;
        fs::write(&target.cached_profile, &profile_bytes).with_context(|| {
            format!(
                "Failed to write provisioning profile to {}",
                target.cached_profile.display()
            )
        })?;
        self.note(cformat!(
            "<green>✔</green> Profile cached at {}",
            target.cached_profile.display()
        ));
        Ok(())
    }
}
