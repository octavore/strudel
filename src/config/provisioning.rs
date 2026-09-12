//! The `provisioning_profile` config setting, shared by `[build]` and each
//! `[[extensions]]` entry.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use crate::config::utils::resolve_to;
use crate::paths::managed_profile_path;

/// The literal value that opts a bundle ID into strudel-managed provisioning.
const AUTO: &str = "auto";

/// A `provisioning_profile` value: either a path to a profile strudel embeds
/// unchanged, or the literal `"auto"`, which makes strudel fetch a Developer ID
/// profile for the bundle ID and cache it under `.strudel`.
///
/// A file actually named `auto` can still be pinned by writing a path that
/// isn't the bare word, e.g. `"./auto"`.
#[derive(Debug, Clone)]
pub enum ProvisioningProfileSetting {
    /// strudel creates, caches, and refreshes the profile.
    Auto,
    /// A user-supplied profile, used as-is.
    Path(PathBuf),
}

impl ProvisioningProfileSetting {
    /// Resolve to the profile path and whether strudel manages it. `Auto` maps
    /// to the cache path for `bundle_id`, which may not exist yet; an explicit
    /// path resolves relative to the config file's directory, like every other
    /// user-supplied input path.
    pub fn resolve(self, config_dir: &Path, source_dir: &Path, bundle_id: &str) -> (PathBuf, bool) {
        match self {
            ProvisioningProfileSetting::Auto => (managed_profile_path(source_dir, bundle_id), true),
            ProvisioningProfileSetting::Path(p) => (resolve_to(config_dir, p), false),
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, ProvisioningProfileSetting::Auto)
    }
}

impl<'de> Deserialize<'de> for ProvisioningProfileSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(if s == AUTO {
            ProvisioningProfileSetting::Auto
        } else {
            ProvisioningProfileSetting::Path(PathBuf::from(s))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(s: &str) -> ProvisioningProfileSetting {
        toml::from_str::<toml::Value>(&format!("v = {s:?}"))
            .unwrap()
            .get("v")
            .unwrap()
            .clone()
            .try_into()
            .unwrap()
    }

    #[test]
    fn bare_auto_is_managed() {
        let (path, managed) =
            setting("auto").resolve(Path::new("/cfg"), Path::new("/src"), "com.example.app");
        assert!(managed);
        assert_eq!(
            path,
            PathBuf::from("/src/.strudel/com.example.app.provisionprofile")
        );
    }

    #[test]
    fn a_path_is_resolved_against_the_config_dir_and_not_managed() {
        let (path, managed) = setting("profiles/MyApp.provisionprofile").resolve(
            Path::new("/cfg"),
            Path::new("/src"),
            "com.example.app",
        );
        assert!(!managed);
        assert_eq!(path, PathBuf::from("/cfg/profiles/MyApp.provisionprofile"));
    }

    #[test]
    fn a_file_named_auto_can_be_pinned_as_a_path() {
        // Only the bare word is the opt-in, so a profile that happens to be
        // named `auto` is still reachable.
        let (path, managed) =
            setting("./auto").resolve(Path::new("/cfg"), Path::new("/src"), "com.example.app");
        assert!(!managed);
        assert_eq!(path, PathBuf::from("/cfg/./auto"));
    }

    #[test]
    fn an_absolute_path_is_left_alone() {
        let (path, managed) = setting("/profiles/MyApp.provisionprofile").resolve(
            Path::new("/cfg"),
            Path::new("/src"),
            "com.example.app",
        );
        assert!(!managed);
        assert_eq!(path, PathBuf::from("/profiles/MyApp.provisionprofile"));
    }
}
