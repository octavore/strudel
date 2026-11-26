//! Strudel-managed macOS provisioning profiles.
//!
//! A plain Developer ID-signed app generally needs no provisioning profile:
//! most entitlements, including the App Sandbox and the hardened runtime
//! exceptions, are enforced purely by the code signature. A few capabilities
//! are the exception and are checked against an embedded profile even outside
//! the Mac App Store, App Groups and Network Extensions being the two most
//! commonly hit. Those projects set `provisioning_profile = "auto"` and let
//! strudel create, cache, and refresh the profile. A capability that may need
//! a profile also opts its bundle ID in (see [`capabilities`]).
//!
//! The signing identity decides the profile type. A Developer ID identity gets
//! a `MAC_APP_DIRECT` profile. An Apple Development or Mac Developer identity
//! gets a `MAC_APP_DEVELOPMENT` profile for the devices in
//! `.strudel/devices.toml`.
//!
//! - [`capabilities`] required capabilities, pinned-profile checks, and profile
//!   opt-in, without network calls
//! - [`kind`] profile type from the signing identity, and development devices
//! - [`provision`] App Store Connect changes for profiles and capabilities
//! - [`plan`] confirmation text for those changes

mod capabilities;
mod kind;
mod plan;
mod provision;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clml::cformat;

use self::capabilities::Target;
use self::provision::enable_only_targets;
use crate::builder::MacosBuilder;
use crate::builder::profile::ProfileRequest;
use crate::paths::managed_profile_path;

/// Why strudel manages the profile of a bundle ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedBy {
    /// `provisioning_profile = "auto"`. Failures are errors.
    Config,
    /// `strudel profile fetch --out` for a bundle ID with no profile
    /// configured. Failures are errors.
    Requested,
    /// Capability that may need a profile. Failures are warnings. The config
    /// points at the profile only after it exists.
    Capability,
}

/// Profile strudel creates or refreshes for one bundle ID.
struct ManagedProfile {
    pub request: ProfileRequest,
    pub managed_by: ManagedBy,
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
        let opt_ins = self.profile_opt_ins(&targets, self.interactive());
        let profiles = self.apply_profile_kind(self.managed_profiles(&targets, &opt_ins))?;
        self.provision(force, &targets, &profiles)?;
        Ok(())
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
        let opt_ins = self.profile_opt_ins(&targets, self.interactive());
        let profiles = if out.is_some() {
            self.all_profiles(&targets, &opt_ins)
        } else {
            self.managed_profiles(&targets, &opt_ins)
        };
        let profiles = self.apply_profile_kind(profiles)?;
        if profiles.is_empty() && enable_only_targets(&targets, &profiles).is_empty() {
            self.note(cformat!(
                "<dim>No managed provisioning profiles for {}: `provisioning_profile` isn't set \
                 to \"auto\" on the app or any extension.</dim>",
                self.cfg.app_name
            ));
            return Ok(());
        }
        let skipped = self.provision(force, &targets, &profiles)?;

        if let Some(out) = out {
            let fetched: Vec<&Path> = profiles
                .iter()
                .map(|p| &p.request)
                .filter(|r| !skipped.contains(&r.bundle_id) && r.cache_path.exists())
                .map(|r| r.cache_path.as_path())
                .collect();
            self.copy_profiles(&fetched, out)?;
        }
        Ok(())
    }

    /// Profile of every bundle ID configured with
    /// `provisioning_profile = "auto"` or in `opt_ins`, host first.
    fn managed_profiles(
        &self,
        targets: &[Target],
        opt_ins: &HashSet<String>,
    ) -> Vec<ManagedProfile> {
        let managed_by = |bundle_id: &str, configured: Option<&Path>, manage: bool| match configured
        {
            Some(path) if manage => Some((path.to_path_buf(), ManagedBy::Config)),
            None if opt_ins.contains(bundle_id) => Some((
                managed_profile_path(&self.cfg.source_dir, bundle_id),
                ManagedBy::Capability,
            )),
            _ => None,
        };
        self.profiles_where(targets, managed_by)
    }

    /// Profile of every bundle ID (host and extensions) that could have a
    /// Developer ID profile fetched for it, skipping any that already point at
    /// an explicit (non-`"auto"`) `provisioning_profile` path. Used only by
    /// `strudel profile fetch --out`, an ad hoc fetch whose result is meant to
    /// be committed and pinned via an explicit `provisioning_profile` path
    /// rather than left on `"auto"`.
    fn all_profiles(&self, targets: &[Target], opt_ins: &HashSet<String>) -> Vec<ManagedProfile> {
        let managed_by = |bundle_id: &str, configured: Option<&Path>, manage: bool| {
            let managed_by = if manage {
                ManagedBy::Config
            } else if configured.is_some() {
                return None;
            } else if opt_ins.contains(bundle_id) {
                ManagedBy::Capability
            } else {
                ManagedBy::Requested
            };
            Some((
                managed_profile_path(&self.cfg.source_dir, bundle_id),
                managed_by,
            ))
        };
        self.profiles_where(targets, managed_by)
    }

    /// Profile of the host and each extension, host first. `select` receives
    /// the bundle ID, the configured profile path, and whether it is
    /// `"auto"`. It returns the cache path and [`ManagedBy`], or `None` to skip
    /// the bundle ID.
    fn profiles_where(
        &self,
        targets: &[Target],
        select: impl Fn(&str, Option<&Path>, bool) -> Option<(PathBuf, ManagedBy)>,
    ) -> Vec<ManagedProfile> {
        let host = (
            &self.cfg.bundle_id,
            &self.cfg.app_name,
            self.cfg.provisioning_profile.as_deref(),
            self.cfg.manage_provisioning_profile,
        );
        let extensions = self.cfg.extensions.iter().map(|e| {
            (
                &e.bundle_id,
                &e.name,
                e.provisioning_profile.as_deref(),
                e.manage_provisioning_profile,
            )
        });
        std::iter::once(host)
            .chain(extensions)
            .filter_map(|(bundle_id, label, configured, manage)| {
                let (cache_path, managed_by) = select(bundle_id, configured, manage)?;
                Some(ManagedProfile {
                    request: profile_request(bundle_id, label, cache_path, targets),
                    managed_by,
                })
            })
            .collect()
    }

    /// A profile is issued against a certificate, so an ad-hoc build has
    /// nothing to bind one to. Catch that here rather than let a profile
    /// authorizing no usable identity get embedded and fail at launch.
    fn require_signing_identity(&self) -> Result<()> {
        if !self.is_signed() {
            bail!(
                "`provisioning_profile = \"auto\"` needs a signing identity (Developer ID \
                 Application, Apple Development, or Mac Developer), but none is configured, so \
                 this build would be signed ad-hoc.\n\
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
    cache_path: PathBuf,
    targets: &[Target],
) -> ProfileRequest {
    let request = ProfileRequest::macos(bundle_id.to_string(), label.to_string(), cache_path);
    match targets.iter().find(|t| t.bundle_id == bundle_id) {
        Some(t) => request.with_capabilities(t.profile_capabilities()),
        None => request,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::capabilities::tests::{NETWORK_EXTENSIONS, builder, signed_config};
    use super::*;
    use crate::config::fixtures::resolved_macos;
    use crate::config::{ExtensionKind, ResolvedExtension};

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

    fn summary(profiles: &[ManagedProfile]) -> Vec<(&str, ManagedBy)> {
        profiles
            .iter()
            .map(|p| (p.request.bundle_id.as_str(), p.managed_by))
            .collect()
    }

    #[test]
    fn managed_profiles_empty_when_not_auto() {
        let b = builder(resolved_macos(), true, false);
        assert!(b.managed_profiles(&[], &HashSet::new()).is_empty());
    }

    #[test]
    fn managed_profiles_includes_host_and_managed_extensions() {
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
            extension("b.ext3", None, false),
        ];
        let b = builder(cfg, true, false);
        let opt_ins = HashSet::from(["b.ext2".to_string(), "b.ext3".to_string()]);
        assert_eq!(
            summary(&b.managed_profiles(&[], &opt_ins)),
            [
                ("b", ManagedBy::Config),
                ("b.ext1", ManagedBy::Config),
                ("b.ext3", ManagedBy::Capability),
            ]
        );
    }

    #[test]
    fn all_profiles_skips_extensions_pinned_to_explicit_path() {
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
        let opt_ins = HashSet::from(["b.auto".to_string()]);
        assert_eq!(
            summary(&b.all_profiles(&[], &opt_ins)),
            [
                ("b", ManagedBy::Requested),
                ("b.auto", ManagedBy::Capability),
                ("b.managed", ManagedBy::Config),
            ]
        );
    }

    #[test]
    fn all_profiles_skips_host_pinned_to_explicit_path() {
        let mut cfg = resolved_macos();
        cfg.provisioning_profile = Some(PathBuf::from("/pinned.provisionprofile"));
        cfg.manage_provisioning_profile = false;
        let b = builder(cfg, true, false);
        assert!(b.all_profiles(&[], &HashSet::new()).is_empty());
    }

    #[test]
    fn profile_fetch_opts_in_the_same_bundle_ids_as_build() {
        let (cfg, _dir) = signed_config(NETWORK_EXTENSIONS);
        let mut build = builder(cfg.clone(), true, true);
        build.ensure_profiles(false).unwrap();
        let mut fetch = builder(cfg, true, true);
        fetch.profile_fetch(false, None).unwrap();
        assert!(build.cfg.provisioning_profile.is_some());
        assert!(build.cfg.manage_provisioning_profile);
        assert_eq!(
            build.cfg.provisioning_profile,
            fetch.cfg.provisioning_profile
        );
    }
}
