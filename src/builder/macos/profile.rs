//! Strudel-managed macOS provisioning profiles.
//!
//! A plain Developer ID-signed app generally needs no provisioning profile:
//! most entitlements, including the App Sandbox and the hardened runtime
//! exceptions, are enforced purely by the code signature. A few capabilities
//! are the exception and are checked against an embedded profile even outside
//! the Mac App Store, App Groups and Network Extensions being the two most
//! commonly hit. Those projects set `provisioning_profile = "auto"` and let
//! strudel create, cache, and refresh the profile.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clml::{cformat, cprintln};

use super::capabilities::{IDENTIFIERS_URL, PortalState, Target, enable_only_targets};
use crate::apple::appstore::AppStoreClient;
use crate::builder::MacosBuilder;
use crate::builder::profile::{CertKind, ProfileRequest, ProvisioningPlan};
use crate::paths::managed_profile_path;

const PROFILES_URL: &str = "https://developer.apple.com/account/resources/profiles/list";

/// Changes for one bundle ID on App Store Connect.
struct Job<'a> {
    request: &'a ProfileRequest,
    plan: ProvisioningPlan,
    /// Portal resource ID. `None` if the bundle ID is not registered.
    bundle_ref: Option<String>,
}

impl MacosBuilder {
    /// Create or refresh the Developer ID provisioning profile for every
    /// bundle ID configured with `provisioning_profile = "auto"` or opted in
    /// by a capability. Profiles are cached at
    /// `.strudel/<bundle_id>.provisionprofile`. Extensions are also
    /// supported, using their own bundle IDs.
    ///
    /// Required capabilities are enabled in the same step, and both changes
    /// share one confirmation. A pinned profile is checked locally and makes
    /// no App Store Connect calls.
    ///
    /// No-op, with no App Store Connect calls at all, when nothing opts in or
    /// when every managed profile on disk is already current and grants the
    /// required entitlements. `force` recreates every managed profile
    /// regardless, for `strudel profile fetch --force`.
    ///
    /// Must run before the bundle is assembled, which is what copies each
    /// profile into its bundle.
    pub(crate) fn ensure_profiles(&mut self, force: bool) -> Result<()> {
        let targets = self.prepare_capabilities()?;
        let profiles = self.managed_profile_targets(&targets);
        self.provision(force, &targets, &profiles)
    }

    /// Create or refresh `profiles` and enable the capabilities of `targets`,
    /// prompting for one confirmation before changing App Store Connect.
    fn provision(
        &mut self,
        force: bool,
        targets: &[Target],
        profiles: &[ProfileRequest],
    ) -> Result<()> {
        let enable_only = enable_only_targets(targets, profiles);

        if self.dry_run {
            for t in profiles {
                self.echo(cformat!(
                    "<dim>[dry-run]</dim> Would ensure a Developer ID provisioning profile for \
                     {} ({})",
                    t.label,
                    t.bundle_id
                ));
            }
            for t in &enable_only {
                let names: Vec<&str> = t.required.iter().map(|c| c.name).collect();
                self.echo(cformat!(
                    "<dim>[dry-run]</dim> Would enable capabilities on App Store Connect for \
                     {}: {}",
                    t.label,
                    names.join(", ")
                ));
            }
            return Ok(());
        }

        // Read-only: a project whose profiles are all current never prompts
        // and never touches the network. An identity from APPLE_CERTIFICATE is
        // empty here because it is imported after this step.
        let identity =
            (!self.cfg.sign_identity.is_empty()).then_some(self.cfg.sign_identity.as_str());
        let stale: Vec<&ProfileRequest> = profiles
            .iter()
            .filter(|t| force || !t.is_current(&self.cfg.team_id, identity))
            .collect();
        if stale.is_empty() && enable_only.is_empty() {
            if !profiles.is_empty() {
                self.note(cformat!(
                    "<dim>Managed provisioning profiles are current</dim>"
                ));
            }
            return Ok(());
        }
        if stale.is_empty() && !self.has_api_credentials() {
            self.warn_capabilities_unchecked(&enable_only);
            return Ok(());
        }

        if !stale.is_empty() {
            self.require_signing_identity()?;
        }

        // Profiles for bundle IDs opted in by a capability warn instead of
        // failing. Only an explicit `provisioning_profile = "auto"` is strict.
        let strict = stale
            .iter()
            .any(|t| !self.capability_managed.contains(&t.bundle_id));

        // The confirmation prompt needs a terminal. Without one (e.g. CI without
        // `--ci`), fail with instructions instead of a raw prompt error.
        let interactive = !self.ci && std::io::stdin().is_terminal();
        // `.strudel` is gitignored, so the cached profile cannot be committed
        // where it is. `profile fetch --out` copies it somewhere trackable.
        let files = stale
            .iter()
            .filter_map(|t| t.cache_path.file_name())
            .map(|f| format!("\n  profiles/{}", f.to_string_lossy()))
            .collect::<String>();
        if !interactive && strict {
            bail!(
                "`provisioning_profile = \"auto\"` needs interactive confirmation and is unavailable in CI \
                 or without a terminal.\n\
                 Run `strudel profile fetch --out profiles` locally, commit the copied profiles, \
                 and point each `provisioning_profile` at its file:{files}\n\
                 Or create a profile manually at {PROFILES_URL} and point \
                 `provisioning_profile` at it."
            );
        }

        let client = if self.has_api_credentials() || (interactive && !stale.is_empty()) {
            Some(
                AppStoreClient::from_config(&self.cfg, false)?
                    .show_progress(!self.echo_suppressed()),
            )
        } else {
            None
        };

        // Read-only lookups. A bundle ID that is already set up does not
        // prompt.
        let enable_requests: Vec<ProfileRequest> = enable_only
            .iter()
            .map(|t| {
                ProfileRequest::macos(
                    t.bundle_id.clone(),
                    t.label.clone(),
                    managed_profile_path(&self.cfg.source_dir, &t.bundle_id),
                )
            })
            .collect();
        let mut jobs: Vec<Job> = Vec::new();
        let requests = stale
            .iter()
            .copied()
            .map(|r| (r, true))
            .chain(enable_requests.iter().map(|r| (r, false)));
        for (request, creates_profile) in requests {
            let target = targets.iter().find(|t| t.bundle_id == request.bundle_id);
            let state = match &client {
                Some(c) => Some(PortalState::lookup(c, &request.bundle_id)?),
                None => None,
            };
            let plan = self.build_plan(request, target, state.as_ref(), creates_profile);
            if creates_profile || plan.has_portal_changes() {
                jobs.push(Job {
                    request,
                    plan,
                    bundle_ref: state.and_then(|s| s.bundle_ref),
                });
            }
        }
        if jobs.is_empty() {
            self.note(cformat!(
                "<dim>All required capabilities are already enabled</dim>"
            ));
            return Ok(());
        }

        if !interactive {
            let advice = if stale.is_empty() {
                format!(
                    "Changes need interactive confirmation, which is unavailable in CI or \
                     without a terminal.\n\
                     Make them at {IDENTIFIERS_URL} and run strudel again."
                )
            } else {
                format!(
                    "Changes need interactive confirmation, which is unavailable in CI or \
                     without a terminal.\n\
                     Run `strudel profile fetch --out profiles` locally, commit the copied \
                     profiles, and point each `provisioning_profile` at its file:{files}"
                )
            };
            self.skip_unconfirmed(&jobs, &stale, &advice);
            return Ok(());
        }

        cprintln!("This needs the following on App Store Connect:");
        for job in &jobs {
            cprintln!("{}", job.plan.render());
        }
        let confirmed = inquire::Confirm::new("Make these changes on App Store Connect?")
            .with_default(false)
            .prompt()
            .context("reading confirmation")?;
        if !confirmed {
            if strict {
                bail!(
                    "Aborted: `provisioning_profile = \"auto\"` needs a profile strudel doesn't have \
                     yet. Create one manually at {PROFILES_URL} and point \
                     `provisioning_profile` at it, or re-run and confirm."
                );
            }
            let advice = format!(
                "Make the changes manually at {IDENTIFIERS_URL} and {PROFILES_URL}, or re-run \
                 and confirm."
            );
            self.skip_unconfirmed(&jobs, &stale, &advice);
            return Ok(());
        }

        let client = match client {
            Some(client) => client,
            None => AppStoreClient::from_config(&self.cfg, false)?
                .show_progress(!self.echo_suppressed()),
        };
        let cert_ids = if stale.is_empty() {
            Vec::new()
        } else {
            self.step("Finding Developer ID Application certificates...");
            // Listed once for the whole run rather than per profile: every
            // macOS profile here is issued against the same certificates.
            CertKind::DeveloperIdApplication.list(&client)?
        };

        for job in &jobs {
            let request = job.request;
            let bundle_ref = match &job.bundle_ref {
                Some(id) => id.clone(),
                None => {
                    self.step(&format!(
                        "Registering bundle ID {} on App Store Connect...",
                        request.bundle_id
                    ));
                    request.ensure_bundle_id(&client)?
                },
            };
            if !job.plan.capabilities_to_enable.is_empty() {
                self.step(&format!(
                    "Enabling required capabilities on bundle ID {}...",
                    request.bundle_id
                ));
                self.enable_capabilities(&client, &bundle_ref, &job.plan.capabilities_to_enable)?;
            }
            if job.plan.create_profile {
                self.step(&format!(
                    "Provisioning profile for {} ({})...",
                    request.label, request.bundle_id
                ));
                request.provision_for(&client, &bundle_ref, &cert_ids, &self.paths.strudel_dir)?;
                self.note(cformat!(
                    "<green>✔</green> Profile cached at {}",
                    request.cache_path.display()
                ));
            }
        }
        Ok(())
    }

    /// Warn that the changes in `jobs` were not made and build without them.
    /// Bundle IDs in `stale` that were opted in by a capability are no longer
    /// managed. `advice` follows the warning.
    fn skip_unconfirmed(&mut self, jobs: &[Job], stale: &[&ProfileRequest], advice: &str) {
        let plans: Vec<String> = jobs.iter().map(|j| j.plan.render()).collect();
        cprintln!(
            "<yellow>warning:</yellow> These changes were not made on App Store Connect. The \
             build continues without them, so signing may fail or the app may misbehave at \
             runtime.\n{}\n{advice}",
            plans.join("\n")
        );
        for t in stale {
            self.unmanage_profile_for(&t.bundle_id);
        }
    }

    /// Fetches Developer ID provisioning profiles for a macOS target. With no
    /// `out`, only covers bundle IDs configured with
    /// `provisioning_profile = "auto"` or opted in by a capability. With
    /// `out`, fetches for every bundle ID (host and extensions) that isn't
    /// already pinned to an explicit profile path, and copies each fetched
    /// profile into that directory. `force` recreates profiles even when the
    /// cached copies are current.
    ///
    /// Runs the same capability step as a build, so both act on the same
    /// bundle IDs.
    pub fn profile_fetch(&mut self, force: bool, out: Option<&Path>) -> Result<()> {
        let targets = self.prepare_capabilities()?;
        let profiles = if out.is_some() {
            self.all_profile_targets(&targets)
        } else {
            self.managed_profile_targets(&targets)
        };
        if profiles.is_empty() && enable_only_targets(&targets, &profiles).is_empty() {
            self.note(cformat!(
                "<dim>No managed provisioning profiles for {}: `provisioning_profile` isn't set \
                 to \"auto\" on the app or any extension.</dim>",
                self.cfg.app_name
            ));
            return Ok(());
        }
        self.provision(force, &targets, &profiles)?;

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
            // A profile that was skipped with a warning has no cached file.
            for t in profiles.iter().filter(|t| t.cache_path.exists()) {
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
    fn managed_profile_targets(&self, targets: &[Target]) -> Vec<ProfileRequest> {
        let mut profiles = Vec::new();
        if self.cfg.manage_provisioning_profile
            && let Some(path) = &self.cfg.provisioning_profile
        {
            profiles.push(profile_request(
                &self.cfg.bundle_id,
                &self.cfg.app_name,
                path,
                targets,
            ));
        }
        for ext in &self.cfg.extensions {
            if ext.manage_provisioning_profile
                && let Some(path) = &ext.provisioning_profile
            {
                profiles.push(profile_request(&ext.bundle_id, &ext.name, path, targets));
            }
        }
        profiles
    }

    /// Every bundle ID (host and extensions) that could have a Developer ID
    /// profile fetched for it, skipping any that already point at an explicit
    /// (non-`"auto"`) `provisioning_profile` path. Used only by `strudel
    /// profile fetch --out`, an ad hoc fetch whose result is meant to be
    /// committed and pinned via an explicit `provisioning_profile` path
    /// rather than left on `"auto"`.
    fn all_profile_targets(&self, targets: &[Target]) -> Vec<ProfileRequest> {
        let mut profiles = Vec::new();
        if self.cfg.provisioning_profile.is_none() || self.cfg.manage_provisioning_profile {
            profiles.push(profile_request(
                &self.cfg.bundle_id,
                &self.cfg.app_name,
                &managed_profile_path(&self.cfg.source_dir, &self.cfg.bundle_id),
                targets,
            ));
        }
        for ext in &self.cfg.extensions {
            if ext.provisioning_profile.is_none() || ext.manage_provisioning_profile {
                profiles.push(profile_request(
                    &ext.bundle_id,
                    &ext.name,
                    &managed_profile_path(&self.cfg.source_dir, &ext.bundle_id),
                    targets,
                ));
            }
        }
        profiles
    }

    /// A Developer ID profile is issued against a Developer ID certificate, so
    /// an ad-hoc build has nothing to bind one to. Catch that here rather than
    /// let a profile authorizing no usable identity get embedded and fail at
    /// launch.
    fn require_signing_identity(&self) -> Result<()> {
        if !self.is_signed() {
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

/// Profile request for a bundle ID. The cached profile must grant the
/// entitlements of the capabilities required by the matching target.
fn profile_request(
    bundle_id: &str,
    label: &str,
    cache_path: &Path,
    targets: &[Target],
) -> ProfileRequest {
    let request = ProfileRequest::macos(
        bundle_id.to_string(),
        label.to_string(),
        cache_path.to_path_buf(),
    );
    match targets.iter().find(|t| t.bundle_id == bundle_id) {
        Some(t) => request.with_capabilities(t.profile_capabilities()),
        None => request,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::builder::OutputFlags;
    use crate::config::fixtures::resolved_macos;
    use crate::config::{ExtensionKind, ResolvedConfig, ResolvedExtension};

    const NETWORK_EXTENSIONS: &str =
        r#"{"com.apple.developer.networking.networkextension": ["packet-tunnel-provider"]}"#;

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
            entitlements_json_path: Some(PathBuf::from("/x/ent.json")),
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
        assert!(b.managed_profile_targets(&[]).is_empty());
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
        let targets = b.managed_profile_targets(&[]);
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
        let targets = b.all_profile_targets(&[]);
        let ids: Vec<&str> = targets.iter().map(|t| t.bundle_id.as_str()).collect();
        assert_eq!(ids, ["b", "b.auto", "b.managed"]);
    }

    #[test]
    fn all_profile_targets_skips_host_pinned_to_explicit_path() {
        let mut cfg = resolved_macos();
        cfg.provisioning_profile = Some(PathBuf::from("/pinned.provisionprofile"));
        cfg.manage_provisioning_profile = false;
        let b = builder(cfg, true, false);
        assert!(b.all_profile_targets(&[]).is_empty());
    }

    #[test]
    fn ensure_profiles_for_bails_in_ci() {
        let mut cfg = resolved_macos();
        cfg.sign_identity = "Developer ID Application: Me (TEAM)".into();
        cfg.provisioning_profile = Some(PathBuf::from("/x/.strudel/b.provisionprofile"));
        cfg.manage_provisioning_profile = true;
        let mut b = builder(cfg, false, true);
        let profiles = b.managed_profile_targets(&[]);
        let err = b
            .provision(true, &[], &profiles)
            .expect_err("CI must not attempt an interactive fetch");
        assert!(err.to_string().contains("unavailable in CI"));
    }

    #[test]
    fn ensure_profiles_for_warns_in_ci_for_capability_managed_profiles() {
        let mut cfg = resolved_macos();
        cfg.sign_identity = "Developer ID Application: Me (TEAM)".into();
        cfg.provisioning_profile = Some(PathBuf::from("/x/.strudel/b.provisionprofile"));
        cfg.manage_provisioning_profile = true;
        let mut b = builder(cfg, false, true);
        b.capability_managed.insert("b".into());
        let profiles = b.managed_profile_targets(&[]);
        b.provision(true, &[], &profiles).unwrap();
        assert!(b.cfg.provisioning_profile.is_none());
        assert!(!b.cfg.manage_provisioning_profile);
        assert!(b.capability_managed.is_empty());
    }

    /// Builder in CI whose entitlements file needs Network Extensions, which
    /// opts the host into managed provisioning without a terminal.
    fn network_extension_builder(dry_run: bool) -> (MacosBuilder, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entitlements.json");
        std::fs::write(&path, NETWORK_EXTENSIONS).unwrap();
        let mut cfg = resolved_macos();
        cfg.entitlements_json_path = Some(path);
        cfg.sign_identity = "Developer ID Application: Me (TEAM)".into();
        cfg.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        cfg.apple_api_key = "KID".into();
        cfg.apple_api_issuer = "ISS".into();
        (builder(cfg, dry_run, true), dir)
    }

    #[test]
    fn profile_fetch_opts_in_the_same_bundle_ids_as_build() {
        let (mut build, _dir) = network_extension_builder(true);
        build.ensure_profiles(false).unwrap();
        let (mut fetch, _dir) = network_extension_builder(true);
        fetch.profile_fetch(false, None).unwrap();
        assert!(!build.capability_managed.is_empty());
        assert_eq!(build.capability_managed, fetch.capability_managed);
        assert_eq!(
            build.cfg.provisioning_profile,
            fetch.cfg.provisioning_profile
        );
    }
}
