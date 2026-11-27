//! App Store Connect changes for macOS managed profiles and capabilities.
//!
//! Changes are made in three phases. Read-only lookups build one
//! [`ProvisioningPlan`] per bundle ID. One prompt confirms every plan. Then
//! the bundle IDs are registered, capabilities are enabled, and profiles are
//! created.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use clml::{cformat, cprintln};

use super::ManagedProfile;
use super::capabilities::Target;
use super::plan::ProvisioningPlan;
use crate::apple::appstore::AppStoreClient;
use crate::apple::capabilities::CapabilityType;
use crate::builder::MacosBuilder;
use crate::builder::profile::{
    IDENTIFIERS_URL, MacosProfileKind, ProfileRequest, StaleReason, enable_capabilities,
};
use crate::config::Platform;

const PROFILES_URL: &str = "https://developer.apple.com/account/resources/profiles/list";

/// State of a bundle ID on App Store Connect.
pub(super) struct PortalState {
    /// Portal resource ID. `None` if the bundle ID is not registered.
    pub bundle_ref: Option<String>,
    pub enabled: HashSet<CapabilityType>,
}

impl PortalState {
    /// Read-only lookup.
    fn lookup(client: &AppStoreClient, bundle_id: &str) -> Result<Self> {
        let bundle_ref = client.find_bundle_id(bundle_id)?;
        let enabled = match &bundle_ref {
            Some(id) => client.enabled_capability_types(id)?,
            None => HashSet::new(),
        };
        Ok(PortalState {
            bundle_ref,
            enabled,
        })
    }
}

/// Managed profile whose cached copy cannot be used.
pub(super) struct StaleProfile<'a> {
    pub profile: &'a ManagedProfile,
    /// `None` if the profile is recreated because of `--force`.
    pub reason: Option<StaleReason>,
}

/// Changes for one bundle ID on App Store Connect.
struct Job<'a> {
    bundle_id: &'a str,
    label: &'a str,
    /// Profile to create. `None` if the job only enables capabilities.
    profile: Option<&'a ProfileRequest>,
    plan: ProvisioningPlan,
    /// Portal resource ID. `None` if the bundle ID is not registered.
    bundle_ref: Option<String>,
}

impl MacosBuilder {
    /// Create or refresh `profiles` and enable the capabilities of `targets`,
    /// prompting for one confirmation before changing App Store Connect.
    ///
    /// Returns the bundle IDs whose stale profile was not created.
    pub(super) fn provision(
        &self,
        force: bool,
        targets: &[Target],
        profiles: &[ManagedProfile],
    ) -> Result<HashSet<String>> {
        let enable_only = enable_only_targets(targets, profiles);
        if self.dry_run {
            self.echo_dry_run(profiles, &enable_only);
            return Ok(HashSet::new());
        }

        let stale = self.stale_profiles(force, profiles);
        let all_skipped = || {
            stale
                .iter()
                .map(|s| s.profile.request.bundle_id.clone())
                .collect()
        };
        if stale.is_empty() && enable_only.is_empty() {
            for p in profiles {
                self.note(cformat!(
                    "<green>✔</green> Profile is current: {}",
                    p.request.cache_path.display()
                ));
            }
            return Ok(HashSet::new());
        }

        // Failing to create a profile is an error. Failing to enable
        // capabilities alone only warns.
        let strict = !stale.is_empty();
        if !strict && !self.has_api_credentials() {
            let unchecked: Vec<&Target> = targets
                .iter()
                .filter(|t| {
                    enable_only.iter().any(|e| e.bundle_id == t.bundle_id)
                        || stale
                            .iter()
                            .any(|s| s.profile.request.bundle_id == t.bundle_id)
                })
                .collect();
            self.warn_capabilities_unchecked(&unchecked);
            return Ok(all_skipped());
        }
        if !stale.is_empty() {
            self.require_signing_identity()?;
        }

        // The confirmation prompt needs a terminal. Without one (e.g. CI
        // without `--ci`), fail with instructions instead of a raw prompt
        // error.
        let interactive = self.interactive();
        if !interactive && strict {
            bail!(
                "`provisioning_profile = \"auto\"` needs interactive confirmation and is unavailable in CI \
                 or without a terminal.\n\
                 Run `strudel profile fetch --out profiles` locally, commit the copied profiles, \
                 and point each `provisioning_profile` at its file:{}\n\
                 Or create a profile manually at {PROFILES_URL} and point \
                 `provisioning_profile` at it.",
                committed_profile_files(&stale)
            );
        }

        let client = AppStoreClient::from_config(&self.cfg)?.show_progress(!self.echo_suppressed());
        let jobs = collect_jobs(&client, targets, &stale, &enable_only)?;
        if jobs.is_empty() {
            self.note(cformat!(
                "<dim>All required capabilities are already enabled</dim>"
            ));
            return Ok(HashSet::new());
        }
        if !self.confirm(&jobs, &stale, interactive, strict)? {
            return Ok(all_skipped());
        }
        self.execute(&client, &jobs)?;
        Ok(HashSet::new())
    }

    fn echo_dry_run(&self, profiles: &[ManagedProfile], enable_only: &[&Target]) {
        for p in profiles {
            self.echo(cformat!(
                "<dim>[dry-run]</dim> Would ensure a {} provisioning profile for {} ({})",
                p.request
                    .macos_kind
                    .unwrap_or(MacosProfileKind::DeveloperId)
                    .description(),
                p.request.label,
                p.request.bundle_id
            ));
        }
        for t in enable_only {
            let names: Vec<&str> = t.required.caps.iter().map(|c| c.name).collect();
            self.echo(cformat!(
                "<dim>[dry-run]</dim> Would enable capabilities on App Store Connect for {}: {}",
                t.label,
                names.join(", ")
            ));
        }
    }

    /// Profiles in `profiles` that must be created. `force` includes every
    /// profile.
    fn stale_profiles<'a>(
        &self,
        force: bool,
        profiles: &'a [ManagedProfile],
    ) -> Vec<StaleProfile<'a>> {
        // Only checked against an identity named in the config. One from
        // `APPLE_CERTIFICATE` is empty here because it is imported after this
        // step.
        let identity =
            (!self.cfg.sign_identity.is_empty()).then_some(self.cfg.sign_identity.as_str());
        profiles
            .iter()
            .filter_map(|profile| {
                if force {
                    return Some(StaleProfile {
                        profile,
                        reason: None,
                    });
                }
                let reason = profile.request.stale_reason(&self.cfg.team_id, identity)?;
                Some(StaleProfile {
                    profile,
                    reason: Some(reason),
                })
            })
            .collect()
    }

    /// Ask once for every change in `jobs`. Returns `false` if the changes
    /// are skipped with a warning. Fails if a declined change is `strict`.
    fn confirm(
        &self,
        jobs: &[Job],
        stale: &[StaleProfile],
        interactive: bool,
        strict: bool,
    ) -> Result<bool> {
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
                     profiles, and point each `provisioning_profile` at its file:{}",
                    committed_profile_files(stale)
                )
            };
            warn_skipped(jobs, &advice);
            return Ok(false);
        }

        cprintln!("This needs the following on App Store Connect:");
        for job in jobs {
            cprintln!("{}", job.plan.render());
        }
        let confirmed = inquire::Confirm::new("Make these changes on App Store Connect?")
            .with_default(false)
            .prompt()
            .context("reading confirmation")?;
        if confirmed {
            return Ok(true);
        }
        if strict {
            bail!(
                "Aborted: `provisioning_profile = \"auto\"` needs a profile strudel doesn't have \
                 yet. Create one manually at {PROFILES_URL} and point \
                 `provisioning_profile` at it, or re-run and confirm."
            );
        }
        warn_skipped(
            jobs,
            &format!(
                "Make the changes manually at {IDENTIFIERS_URL} and {PROFILES_URL}, or re-run \
                 and confirm."
            ),
        );
        Ok(false)
    }

    /// Register bundle IDs, enable capabilities, and create profiles.
    fn execute(&self, client: &AppStoreClient, jobs: &[Job]) -> Result<()> {
        // Every profile in a run has the same kind, since the signing
        // identity decides it.
        let kind = jobs
            .iter()
            .find_map(|j| j.profile)
            .map(|p| p.macos_kind.unwrap_or(MacosProfileKind::DeveloperId));
        let cert_ids = match kind {
            Some(kind) => {
                self.step(&format!(
                    "Finding {} certificates...",
                    kind.cert_kind().description()
                ));
                // Listed once for the whole run rather than per profile: every
                // macOS profile here is issued against the same certificates.
                kind.cert_kind().list(client)?
            },
            None => Vec::new(),
        };
        let device_ids = match kind {
            Some(kind) if kind.needs_devices() => self.portal_device_ids(client)?,
            _ => Vec::new(),
        };

        for job in jobs {
            let bundle_ref = match &job.bundle_ref {
                Some(id) => id.clone(),
                None => {
                    self.step(&format!(
                        "Registering bundle ID {} on App Store Connect...",
                        job.bundle_id
                    ));
                    client.find_or_create_bundle_id(job.bundle_id, job.label, Platform::Macos)?
                },
            };
            if !job.plan.capabilities_to_enable.is_empty() {
                self.step(&format!(
                    "Enabling required capabilities on bundle ID {}...",
                    job.bundle_id
                ));
                enable_capabilities(
                    client,
                    &bundle_ref,
                    &job.plan.capabilities_to_enable,
                    Platform::Macos,
                )?;
            }
            if let Some(request) = job.profile {
                self.step(&format!(
                    "Provisioning profile for {} ({})...",
                    request.label, request.bundle_id
                ));
                let request = ProfileRequest {
                    device_ids: device_ids.clone(),
                    ..request.clone()
                };
                request.provision_for(client, &bundle_ref, &cert_ids, &self.paths.strudel_dir)?;
                self.note(cformat!(
                    "<green>✔</green> Profile cached at {}",
                    request.cache_path.display()
                ));
            }
        }
        Ok(())
    }

    /// Warn that App Store Connect credentials are missing, so `targets` were
    /// not checked.
    fn warn_capabilities_unchecked(&self, targets: &[&Target]) {
        cprintln!(
            "<yellow>warning:</yellow> App Store Connect API credentials are not set, so \
             capabilities were not checked or enabled. Make sure these are enabled on the \
             bundle IDs at {IDENTIFIERS_URL}:"
        );
        for t in targets {
            let names: Vec<&str> = t.required.caps.iter().map(|c| c.name).collect();
            cprintln!(
                "  <cyan>{}</cyan> ({}): {}",
                t.bundle_id,
                t.label,
                names.join(", ")
            );
        }
    }
}

/// Targets that only need capabilities enabled. They have no managed profile
/// and no pinned profile.
pub(super) fn enable_only_targets<'a>(
    targets: &'a [Target],
    profiles: &[ManagedProfile],
) -> Vec<&'a Target> {
    targets
        .iter()
        .filter(|t| t.pinned.is_none())
        .filter(|t| !profiles.iter().any(|p| p.request.bundle_id == t.bundle_id))
        .collect()
}

/// Read-only lookups and one plan per bundle ID. Every stale profile has a
/// job. An enable-only target has a job if it changes the portal.
fn collect_jobs<'a>(
    client: &AppStoreClient,
    targets: &'a [Target],
    stale: &'a [StaleProfile<'a>],
    enable_only: &[&'a Target],
) -> Result<Vec<Job<'a>>> {
    let mut jobs = Vec::new();
    for s in stale {
        let request = &s.profile.request;
        let target = targets.iter().find(|t| t.bundle_id == request.bundle_id);
        let state = PortalState::lookup(client, &request.bundle_id)?;
        jobs.push(Job {
            bundle_id: &request.bundle_id,
            label: &request.label,
            profile: Some(request),
            plan: ProvisioningPlan::new(&request.bundle_id, target, &state, Some(s)),
            bundle_ref: state.bundle_ref,
        });
    }
    for &t in enable_only {
        let state = PortalState::lookup(client, &t.bundle_id)?;
        let plan = ProvisioningPlan::new(&t.bundle_id, Some(t), &state, None);
        if plan.has_portal_changes() {
            jobs.push(Job {
                bundle_id: &t.bundle_id,
                label: &t.label,
                profile: None,
                plan,
                bundle_ref: state.bundle_ref,
            });
        }
    }
    Ok(jobs)
}

/// Suggested committed path of each stale profile, one per line.
fn committed_profile_files(stale: &[StaleProfile]) -> String {
    // `.strudel` is gitignored, so the cached profile cannot be committed
    // where it is. `profile fetch --out` copies it somewhere trackable.
    stale
        .iter()
        .filter_map(|s| s.profile.request.cache_path.file_name())
        .map(|f| format!("\n  profiles/{}", f.to_string_lossy()))
        .collect()
}

/// Warn that the changes in `jobs` were not made and the build continues
/// without them. `advice` follows the warning.
fn warn_skipped(jobs: &[Job], advice: &str) {
    let plans: Vec<String> = jobs.iter().map(|j| j.plan.render()).collect();
    cprintln!(
        "<yellow>warning:</yellow> These changes were not made on App Store Connect. The build \
         continues without them, so signing may fail or the app may misbehave at runtime.\n{}\n\
         {advice}",
        plans.join("\n")
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::ManagedBy;
    use super::super::capabilities::tests::{APP_GROUPS, builder, signed_config};
    use super::*;
    use crate::config::fixtures::resolved_macos;

    fn managed(bundle_id: &str, managed_by: ManagedBy) -> ManagedProfile {
        ManagedProfile {
            request: ProfileRequest::macos(
                bundle_id.into(),
                "A".into(),
                PathBuf::from(format!("/x/.strudel/{bundle_id}.provisionprofile")),
            ),
            managed_by,
        }
    }

    #[test]
    fn auto_profile_bails_in_ci() {
        let mut cfg = resolved_macos();
        cfg.sign_identity = "Developer ID Application: Me (TEAM)".into();
        let b = builder(cfg, false, true);
        let err = b
            .provision(true, &[], &[managed("b", ManagedBy::Config)])
            .expect_err("CI must not attempt an interactive fetch");
        assert!(err.to_string().contains("unavailable in CI"));
    }

    #[test]
    fn unpinned_targets_without_a_profile_only_enable_capabilities() {
        let (cfg, _dir) = signed_config(APP_GROUPS);
        let b = builder(cfg, false, true);
        let targets = b.prepare_capabilities().unwrap();
        assert_eq!(enable_only_targets(&targets, &[]).len(), 1);
        let profiles = [managed("b", ManagedBy::Config)];
        assert!(enable_only_targets(&targets, &profiles).is_empty());
    }
}
