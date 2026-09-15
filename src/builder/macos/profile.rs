//! Strudel-managed macOS provisioning profiles.
//!
//! A plain Developer ID-signed app generally needs no provisioning profile:
//! most entitlements, including the App Sandbox and the hardened runtime
//! exceptions, are enforced purely by the code signature. A few capabilities
//! are the exception and are checked against an embedded profile even outside
//! the Mac App Store, App Groups and Network Extensions being the two most
//! commonly hit. Those projects set `provisioning_profile = "auto"` and let
//! strudel create, cache, and refresh the profile.

use std::path::Path;

use anyhow::{Context, Result, bail};
use clml::{cformat, cprintln};

use crate::apple::appstore::AppStoreClient;
use crate::builder::MacosBuilder;
use crate::builder::profile::{CertKind, ProfileRequest};
use crate::paths::managed_profile_path;

impl MacosBuilder {
    /// Create or refresh the Developer ID provisioning profile for every
    /// bundle ID configured with `provisioning_profile = "auto"`. Profiles are
    /// cached at `.strudel/<bundle_id>.provisionprofile`. Extensions are
    /// also supported, using their own bundle IDs.
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
        self.ensure_profiles_for(force, &targets)
    }

    /// Creates or refreshes the Developer ID provisioning profile for each of
    /// `targets`, prompting for confirmation before calling the App Store
    /// Connect API.
    fn ensure_profiles_for(&mut self, force: bool, targets: &[ProfileRequest]) -> Result<()> {
        if targets.is_empty() {
            return Ok(());
        }

        if self.dry_run {
            for t in targets {
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
        // and never touches the network. An identity from APPLE_CERTIFICATE is
        // empty here because it is imported after this step.
        let identity =
            (!self.cfg.sign_identity.is_empty()).then_some(self.cfg.sign_identity.as_str());
        let stale: Vec<&ProfileRequest> = targets
            .iter()
            .filter(|t| force || !t.is_current(&self.cfg.team_id, identity))
            .collect();
        if stale.is_empty() {
            self.note(cformat!(
                "<dim>Managed provisioning profiles are current</dim>"
            ));
            return Ok(());
        }

        self.require_signing_identity()?;

        if self.ci {
            // `.strudel` is gitignored, so the cached profile cannot be committed
            // where it is. `profile fetch --out` copies it somewhere trackable.
            let files = stale
                .iter()
                .filter_map(|t| t.cache_path.file_name())
                .map(|f| format!("\n  profiles/{}", f.to_string_lossy()))
                .collect::<String>();
            bail!(
                "`provisioning_profile = \"auto\"` needs interactive confirmation and is unavailable in CI.\n\
                 Run `strudel profile fetch --out profiles` locally, commit the copied profiles, \
                 and point each `provisioning_profile` at its file:{files}\n\
                 Or create a profile manually at \
                 https://developer.apple.com/account/resources/profiles/list and point \
                 `provisioning_profile` at it."
            );
        }

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

        let client = AppStoreClient::from_config(&self.cfg)?.show_progress(!self.echo_suppressed());
        self.step("Finding Developer ID Application certificates...");
        // Listed once for the whole run rather than per profile: every macOS
        // profile here is issued against the same certificates.
        let cert_ids = CertKind::DeveloperIdApplication.list(&client)?;

        for t in &stale {
            self.step(&format!(
                "Provisioning profile for {} ({})...",
                t.label, t.bundle_id
            ));
            t.provision(&client, &cert_ids, &self.paths.strudel_dir)?;
            self.note(cformat!(
                "<green>✔</green> Profile cached at {}",
                t.cache_path.display()
            ));
        }
        Ok(())
    }

    /// Fetches Developer ID provisioning profiles for a macOS target. With no
    /// `out`, only covers bundle IDs already configured with
    /// `provisioning_profile = "auto"`. With `out`, fetches for every bundle
    /// ID (host and extensions) that isn't already pinned to an explicit
    /// profile path, and copies each fetched profile into that directory.
    /// `force` recreates profiles even when the cached copies are current.
    pub fn profile_fetch(&mut self, force: bool, out: Option<&Path>) -> Result<()> {
        let targets = if out.is_some() {
            self.all_profile_targets()
        } else {
            self.managed_profile_targets()
        };
        if targets.is_empty() {
            self.note(cformat!(
                "<dim>No managed provisioning profiles for {}: no capabilities are configured \
                 and `provisioning_profile` isn't set to \"auto\".</dim>",
                self.cfg.app_name
            ));
            return Ok(());
        }
        self.ensure_profiles_for(force, &targets)?;

        if let Some(out) = out {
            if self.dry_run {
                self.echo(cformat!(
                    "<dim>[dry-run]</dim> Would copy fetched profiles to {}",
                    out.display()
                ));
                return Ok(());
            }
            std::fs::create_dir_all(out)
                .with_context(|| format!("Failed to create {}", out.display()))?;
            for t in &targets {
                let dest = out.join(t.cache_path.file_name().unwrap());
                std::fs::copy(&t.cache_path, &dest).with_context(|| {
                    format!(
                        "Failed to copy {} to {}",
                        t.cache_path.display(),
                        dest.display()
                    )
                })?;
                self.note(cformat!("<green>✔</green> Copied to {}", dest.display()));
            }
        }
        Ok(())
    }

    /// Every bundle ID opted into strudel-managed provisioning, host first.
    fn managed_profile_targets(&self) -> Vec<ProfileRequest> {
        let mut targets = Vec::new();
        if self.cfg.manage_provisioning_profile
            && let Some(path) = &self.cfg.provisioning_profile
        {
            targets.push(ProfileRequest::macos(
                self.cfg.bundle_id.clone(),
                self.cfg.app_name.clone(),
                path.clone(),
            ));
        }
        for ext in &self.cfg.extensions {
            if ext.manage_provisioning_profile
                && let Some(path) = &ext.provisioning_profile
            {
                targets.push(ProfileRequest::macos(
                    ext.bundle_id.clone(),
                    ext.name.clone(),
                    path.clone(),
                ));
            }
        }
        targets
    }

    /// Every bundle ID (host and extensions) that could have a Developer ID
    /// profile fetched for it, skipping any that already point at an explicit
    /// (non-`"auto"`) `provisioning_profile` path. Used only by `strudel
    /// profile fetch --out`, an ad hoc fetch whose result is meant to be
    /// committed and pinned via an explicit `provisioning_profile` path
    /// rather than left on `"auto"`.
    fn all_profile_targets(&self) -> Vec<ProfileRequest> {
        let mut targets = Vec::new();
        if self.cfg.provisioning_profile.is_none() || self.cfg.manage_provisioning_profile {
            targets.push(ProfileRequest::macos(
                self.cfg.bundle_id.clone(),
                self.cfg.app_name.clone(),
                managed_profile_path(&self.cfg.source_dir, &self.cfg.bundle_id),
            ));
        }
        for ext in &self.cfg.extensions {
            if ext.provisioning_profile.is_none() || ext.manage_provisioning_profile {
                targets.push(ProfileRequest::macos(
                    ext.bundle_id.clone(),
                    ext.name.clone(),
                    managed_profile_path(&self.cfg.source_dir, &ext.bundle_id),
                ));
            }
        }
        targets
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
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::builder::OutputFlags;
    use crate::config::fixtures::resolved_macos;
    use crate::config::{ExtensionKind, ResolvedConfig, ResolvedExtension};

    fn builder(cfg: ResolvedConfig, dry_run: bool, ci: bool) -> MacosBuilder {
        let output = OutputFlags {
            dry_run,
            ..Default::default()
        };
        MacosBuilder::new(cfg, output, false, false, None, false, ci).unwrap()
    }

    fn extension(
        bundle_id: &str,
        provisioning_profile: Option<PathBuf>,
        managed: bool,
    ) -> ResolvedExtension {
        ResolvedExtension {
            kind: ExtensionKind::AppExtension,
            target_name: bundle_id.into(),
            bundle_id: bundle_id.into(),
            name: bundle_id.into(),
            info_json_path: None,
            entitlements_json_path: PathBuf::from("/x/ent.json"),
            provisioning_profile,
            manage_provisioning_profile: managed,
            resources_dir: None,
            principal_class: None,
            extension_point_identifier: None,
        }
    }

    #[test]
    fn managed_profile_targets_empty_when_not_auto() {
        let cfg = resolved_macos();
        let b = builder(cfg, true, false);
        assert!(b.managed_profile_targets().is_empty());
    }

    #[test]
    fn managed_profile_targets_includes_host_and_managed_extensions() {
        let mut cfg = resolved_macos();
        cfg.manage_provisioning_profile = true;
        cfg.provisioning_profile = Some(PathBuf::from("/x/.strudel/b.provisionprofile"));
        cfg.extensions = vec![
            extension(
                "b.ext1",
                Some(PathBuf::from("/x/.strudel/b.ext1.provisionprofile")),
                true,
            ),
            extension(
                "b.ext2",
                Some(PathBuf::from("/pinned.provisionprofile")),
                false,
            ),
        ];
        let b = builder(cfg, true, false);
        let targets = b.managed_profile_targets();
        let ids: Vec<&str> = targets.iter().map(|t| t.bundle_id.as_str()).collect();
        assert_eq!(ids, ["b", "b.ext1"]);
    }

    #[test]
    fn all_profile_targets_skips_extensions_pinned_to_explicit_path() {
        let mut cfg = resolved_macos();
        // Host has nothing configured: no pinned path, so it's still a target.
        cfg.extensions = vec![
            extension("b.auto", None, false),
            extension(
                "b.managed",
                Some(PathBuf::from("/x/.strudel/b.managed.provisionprofile")),
                true,
            ),
            extension(
                "b.pinned",
                Some(PathBuf::from("/pinned.provisionprofile")),
                false,
            ),
        ];
        let b = builder(cfg, true, false);
        let targets = b.all_profile_targets();
        let ids: Vec<&str> = targets.iter().map(|t| t.bundle_id.as_str()).collect();
        assert_eq!(ids, ["b", "b.auto", "b.managed"]);
    }

    #[test]
    fn all_profile_targets_skips_host_pinned_to_explicit_path() {
        let mut cfg = resolved_macos();
        cfg.provisioning_profile = Some(PathBuf::from("/pinned.provisionprofile"));
        cfg.manage_provisioning_profile = false;
        let b = builder(cfg, true, false);
        assert!(b.all_profile_targets().is_empty());
    }

    #[test]
    fn ensure_profiles_for_bails_in_ci() {
        let mut cfg = resolved_macos();
        cfg.sign_identity = "Developer ID Application: Me (TEAM)".into();
        cfg.provisioning_profile = Some(PathBuf::from("/x/.strudel/b.provisionprofile"));
        cfg.manage_provisioning_profile = true;
        let mut b = builder(cfg, false, true);
        let targets = b.managed_profile_targets();
        let err = b
            .ensure_profiles_for(true, &targets)
            .expect_err("CI must not attempt an interactive fetch");
        assert!(err.to_string().contains("unavailable in CI"));
    }
}
