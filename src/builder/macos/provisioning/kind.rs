//! Profile type selection from the signing identity, and the devices of a
//! development profile.

use std::process::Command;

use anyhow::{Context, Result, bail};
use clml::cformat;

use super::ManagedProfile;
use crate::apple::appstore::{AppStoreClient, PortalDevice};
use crate::builder::MacosBuilder;
use crate::builder::keychain::parse_identity_line;
use crate::builder::profile::MacosProfileKind;
use crate::config::Platform;
use crate::devices::DeviceSet;

/// Whether `s` is a SHA-1 certificate hash, as `codesign` accepts for an
/// identity.
fn is_sha1_hash(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Name of the identity with `hash` in `security find-identity` output.
fn identity_name_for_hash(listing: &str, hash: &str) -> Option<String> {
    listing
        .lines()
        .filter_map(parse_identity_line)
        .find(|(h, _)| h.eq_ignore_ascii_case(hash))
        .map(|(_, name)| name.to_string())
}

/// UDIDs of the tracked macOS devices. Fails if there are none, since a
/// development profile without devices authorizes no machine.
fn require_devices(set: &DeviceSet) -> Result<Vec<String>> {
    if set.device.is_empty() {
        bail!(
            "No macOS devices are tracked in .strudel/devices.toml.\n\
             A development profile lists the devices it can run on. Add this Mac with:\n  \
             strudel devices add\n\
             Add another Mac with:\n  \
             strudel devices add --udid <udid> --name <name> --platform macos"
        );
    }
    Ok(set.udids().into_iter().map(str::to_string).collect())
}

/// Portal resource IDs of the tracked devices. Every tracked device must be
/// registered on the portal, since a profile without it fails only when run
/// on that device.
pub(super) fn match_portal_devices(
    set: &DeviceSet,
    portal: &[PortalDevice],
) -> Result<Vec<String>> {
    set.device
        .iter()
        .map(|tracked| {
            let Some(found) = portal.iter().find(|d| d.udid == tracked.udid) else {
                bail!(
                    "Device {} ({}) is in .strudel/devices.toml but is not registered as a \
                     macOS device on the App Store Connect portal.\n\
                     Run `strudel devices add --udid {} --name {:?} --platform macos` to \
                     register it.",
                    tracked.name,
                    tracked.udid,
                    tracked.udid,
                    tracked.name
                );
            };
            Ok(found.id.clone())
        })
        .collect()
}

impl MacosBuilder {
    /// Name of the signing identity. `None` for an ad-hoc build, or if the
    /// name is unknown because the build only imports `APPLE_CERTIFICATE` when
    /// it signs.
    fn signing_identity_name(&self) -> Result<Option<String>> {
        let configured = self.cfg.sign_identity.as_str();
        if is_sha1_hash(configured) {
            return self.keychain_identity_name(configured).map(Some);
        }
        if !configured.is_empty() {
            return Ok(Some(configured.to_string()));
        }
        if self.cfg.signing_cert().is_none() || self.dry_run {
            return Ok(None);
        }
        // The keychain is removed when the guard drops, and the build imports
        // the certificate again when it signs.
        Ok(self.import_certificate()?.map(|(_keychain, name)| name))
    }

    /// Name of the keychain identity with SHA-1 `hash`.
    fn keychain_identity_name(&self, hash: &str) -> Result<String> {
        let output = Command::new("security")
            .args(["find-identity", "-v", "-p", "codesigning"])
            .output()
            .context("Failed to run `security find-identity`")?;
        identity_name_for_hash(&String::from_utf8_lossy(&output.stdout), hash).with_context(|| {
            format!(
                "Signing identity {hash} is not listed by `security find-identity -v -p \
                 codesigning`."
            )
        })
    }

    /// Set the profile type of `profiles` from the signing identity. Prints
    /// the chosen type and the reason. A development profile takes the
    /// tracked macOS devices. An ad-hoc build keeps the Developer ID type and
    /// fails later, when a profile is created.
    pub(super) fn apply_profile_kind(
        &self,
        profiles: Vec<ManagedProfile>,
    ) -> Result<Vec<ManagedProfile>> {
        if profiles.is_empty() {
            return Ok(profiles);
        }
        let Some(identity) = self.signing_identity_name()? else {
            return Ok(profiles);
        };
        let kind = MacosProfileKind::from_identity_name(&identity)?;
        self.note(cformat!(
            "Signing identity <cyan>{identity}</cyan> starts with {}, so strudel creates \
             <cyan>{}</cyan> profiles.",
            kind.identity_prefixes(),
            kind.profile_type()
        ));
        let udids = if kind.needs_devices() {
            let set = DeviceSet::load_for(&self.paths.devices_toml, Platform::Macos)?;
            // A dry run reports the plan without devices.
            if self.dry_run && set.device.is_empty() {
                Vec::new()
            } else {
                require_devices(&set)?
            }
        } else {
            Vec::new()
        };
        Ok(profiles
            .into_iter()
            .map(
                |ManagedProfile {
                     request,
                     managed_by,
                 }| ManagedProfile {
                    request: request.with_macos_kind(kind, udids.clone()),
                    managed_by,
                },
            )
            .collect())
    }

    /// Portal resource IDs of the tracked macOS devices.
    pub(super) fn portal_device_ids(&self, client: &AppStoreClient) -> Result<Vec<String>> {
        let set = DeviceSet::load_for(&self.paths.devices_toml, Platform::Macos)?;
        require_devices(&set)?;
        self.step("Matching tracked devices to portal...");
        let portal = client.list_devices_on(Platform::Macos)?;
        match_portal_devices(&set, &portal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Apple Development: Jo Doe (TEAMID)"
  2) 89ABCDEF0123456789ABCDEF0123456789ABCDEF "Developer ID Application: Acme Inc (TEAMID)"
     2 valid identities found"#;

    fn kind(name: &str) -> Result<MacosProfileKind> {
        MacosProfileKind::from_identity_name(name)
    }

    #[test]
    fn developer_id_identity_selects_the_direct_profile() {
        let k = kind("Developer ID Application: Acme Inc (TEAMID)").unwrap();
        assert_eq!(k, MacosProfileKind::DeveloperId);
        assert_eq!(k.profile_type(), "MAC_APP_DIRECT");
        assert!(!k.needs_devices());
    }

    #[test]
    fn apple_development_and_mac_developer_select_the_development_profile() {
        for name in [
            "Apple Development: Jo Doe (TEAMID)",
            "Mac Developer: Jo Doe (TEAMID)",
        ] {
            let k = kind(name).unwrap();
            assert_eq!(k, MacosProfileKind::Development, "{name}");
            assert_eq!(k.profile_type(), "MAC_APP_DEVELOPMENT");
            assert!(k.needs_devices());
        }
    }

    #[test]
    fn other_identities_are_an_error() {
        for name in [
            "Apple Distribution: Acme Inc (TEAMID)",
            "3rd Party Mac Developer Application: Acme (TEAMID)",
            "Developer ID Installer: Acme Inc (TEAMID)",
            "my identity",
            "",
        ] {
            let err = kind(name).unwrap_err().to_string();
            assert!(
                err.contains("Cannot choose a provisioning profile type"),
                "{err}"
            );
        }
    }

    #[test]
    fn sha1_hash_detection() {
        assert!(is_sha1_hash("0123456789abcdef0123456789ABCDEF01234567"));
        assert!(!is_sha1_hash("0123456789abcdef"));
        assert!(!is_sha1_hash("Apple Development: Jo Doe (TEAMID)"));
        assert!(!is_sha1_hash(&"g".repeat(40)));
    }

    #[test]
    fn hash_resolves_to_its_identity_name_in_any_case() {
        let name = identity_name_for_hash(LISTING, "89abcdef0123456789abcdef0123456789abcdef");
        assert_eq!(
            name.as_deref(),
            Some("Developer ID Application: Acme Inc (TEAMID)")
        );
        assert_eq!(identity_name_for_hash(LISTING, &"0".repeat(40)), None);
    }

    #[test]
    fn a_hash_identity_selects_the_type_of_its_resolved_name() {
        let name =
            identity_name_for_hash(LISTING, "0123456789ABCDEF0123456789ABCDEF01234567").unwrap();
        assert_eq!(kind(&name).unwrap(), MacosProfileKind::Development);
    }

    #[test]
    fn no_tracked_macos_devices_is_an_error() {
        let mut set = DeviceSet::default();
        set.upsert("iPhone".into(), "AAA".into());
        let err = require_devices(&set.only(Platform::Macos))
            .unwrap_err()
            .to_string();
        assert!(err.contains("No macOS devices are tracked"), "{err}");
        assert!(err.contains("strudel devices add\n"), "{err}");
        assert!(
            err.contains("strudel devices add --udid <udid> --name <name>"),
            "{err}"
        );
    }

    #[test]
    fn tracked_macos_devices_are_required_by_udid() {
        let mut set = DeviceSet::default();
        set.upsert("iPhone".into(), "AAA".into());
        set.upsert_on("Mac".into(), "BBB".into(), Platform::Macos);
        assert_eq!(
            require_devices(&set.only(Platform::Macos)).unwrap(),
            ["BBB"]
        );
    }

    fn portal(udid: &str, id: &str) -> PortalDevice {
        PortalDevice {
            id: id.into(),
            name: "n".into(),
            udid: udid.into(),
        }
    }

    #[test]
    fn tracked_devices_map_to_portal_resource_ids() {
        let mut set = DeviceSet::default();
        set.upsert_on("Mac".into(), "BBB".into(), Platform::Macos);
        let ids = match_portal_devices(&set, &[portal("AAA", "1"), portal("BBB", "2")]).unwrap();
        assert_eq!(ids, ["2"]);
    }

    #[test]
    fn a_tracked_device_missing_from_the_portal_is_an_error() {
        let mut set = DeviceSet::default();
        set.upsert_on("Mac".into(), "BBB".into(), Platform::Macos);
        let err = match_portal_devices(&set, &[portal("AAA", "1")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("Mac (BBB)"), "{err}");
        assert!(err.contains("not registered as a macOS device"), "{err}");
    }
}
