use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::utils::resolve_to;

/// Parsed and path-resolved contents of `~/.config/strudel/config.toml`.
/// All `PathBuf` fields are absolute. Fields absent from the file are `None`.
#[derive(Debug, Default, Clone)]
pub struct GlobalConfig {
    pub signing_identity: Option<String>,
    pub signing_team_id: Option<String>,
    pub notarize_api_issuer: Option<String>,
    pub notarize_api_key: Option<String>,
    pub notarize_api_key_path: Option<PathBuf>,
}

fn base_dirs() -> xdg::BaseDirectories {
    xdg::BaseDirectories::with_prefix("strudel")
}

impl GlobalConfig {
    /// Loads from the XDG config location
    /// (`$XDG_CONFIG_HOME/strudel/config.toml`, defaults to
    /// `~/.config/strudel/config.toml`). Returns a default empty
    /// config when the file does not exist.
    pub fn load() -> Result<Self> {
        let Some(path) = base_dirs().find_config_file("config.toml") else {
            return Ok(Self::default());
        };
        Self::load_from(&path)
    }

    /// Returns the XDG path for the global config, creating the parent
    /// directory if needed. The file itself may not exist yet.
    pub fn xdg_path() -> Result<PathBuf> {
        base_dirs()
            .place_config_file("config.toml")
            .context("Failed to determine global config path")
    }

    fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read global config: {}", path.display()))?;
        let file: GlobalConfigFile = toml::from_str(&content)
            .with_context(|| format!("Failed to parse global config: {}", path.display()))?;
        // relative paths are resolved relative to the config file's directory
        let dir = path.parent().unwrap_or(Path::new("."));
        Ok(Self {
            signing_identity: file.apple.identity,
            signing_team_id: file.apple.team_id,
            notarize_api_issuer: file.apple.api_issuer,
            notarize_api_key: file.apple.api_key,
            notarize_api_key_path: file.apple.api_key_path.map(|p| resolve_to(dir, p)),
        })
    }
}

pub const GLOBAL_CONFIG_TEMPLATE: &str = indoc::indoc! {r#"
    # ~/.config/strudel/config.toml — strudel global config
    #
    # Values here apply to every project on this machine. Each can be overridden
    # per-project in strudel.toml, or via the matching environment variable.
    #
    # Apple developer identifiers, shared by signing, notarization, and
    # provisioning-profile management — or set via the matching env var
    # (APPLE_SIGNING_IDENTITY, APPLE_TEAM_ID, APPLE_API_*).
    [apple]
    # identity     = "Developer ID Application: Your Name (XXXXXXXXXX)"
    # team_id      = "XXXXXXXXXX"
    # api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    # api_key      = "YYYYYYYY"
    # api_key_path = "/Users/you/.private_keys/AuthKey_YYYYYYYY.p8"
"#};

/// The on-disk `~/.config/strudel/config.toml`. Private; callers use
/// [`GlobalConfig`] after path resolution.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobalConfigFile {
    #[serde(default)]
    apple: GlobalAppleSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct GlobalAppleSection {
    identity: Option<String>,
    team_id: Option<String>,
    api_issuer: Option<String>,
    api_key: Option<String>,
    api_key_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_temp(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        (dir, path)
    }

    #[test]
    fn empty_file_gives_all_none() {
        let (_dir, path) = write_temp("");
        let g = GlobalConfig::load_from(&path).unwrap();
        assert!(g.signing_identity.is_none());
        assert!(g.signing_team_id.is_none());
        assert!(g.notarize_api_issuer.is_none());
        assert!(g.notarize_api_key.is_none());
        assert!(g.notarize_api_key_path.is_none());
    }

    #[test]
    fn parses_signing_and_notarize() {
        let (_dir, path) = write_temp(indoc::indoc! {r#"
            [apple]
            identity     = "Developer ID Application: Me (ABC123)"
            team_id      = "ABC123"
            api_issuer   = "iss-uuid"
            api_key      = "KEY123"
            api_key_path = "/abs/AuthKey.p8"
        "#});
        let g = GlobalConfig::load_from(&path).unwrap();
        assert_eq!(
            g.signing_identity.as_deref(),
            Some("Developer ID Application: Me (ABC123)")
        );
        assert_eq!(g.signing_team_id.as_deref(), Some("ABC123"));
        assert_eq!(g.notarize_api_issuer.as_deref(), Some("iss-uuid"));
        assert_eq!(g.notarize_api_key.as_deref(), Some("KEY123"));
        assert_eq!(
            g.notarize_api_key_path,
            Some(PathBuf::from("/abs/AuthKey.p8"))
        );
    }

    #[test]
    fn relative_api_key_path_resolved_to_config_dir() {
        let (_dir, path) = write_temp(indoc::indoc! {r#"
            [apple]
            api_key_path = "AuthKey.p8"
        "#});
        let g = GlobalConfig::load_from(&path).unwrap();
        let expected = path.parent().unwrap().join("AuthKey.p8");
        assert_eq!(g.notarize_api_key_path, Some(expected));
    }

    #[test]
    fn tilde_in_api_key_path_is_expanded() {
        let (_dir, path) = write_temp(indoc::indoc! {r#"
            [apple]
            api_key_path = "~/my_keys/AuthKey.p8"
        "#});
        let g = GlobalConfig::load_from(&path).unwrap();
        let expanded = g.notarize_api_key_path.unwrap();
        assert!(
            expanded.is_absolute(),
            "tilde should expand to an absolute path"
        );
        assert!(expanded.ends_with("my_keys/AuthKey.p8"));
    }

    #[test]
    fn unknown_key_is_rejected() {
        let (_dir, path) = write_temp(indoc::indoc! {r#"
            [apple]
            typo_key = "oops"
        "#});
        assert!(GlobalConfig::load_from(&path).is_err());
    }

    #[test]
    fn missing_file_gives_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.toml");
        // load_from errors on missing file; the missing-file fast-path is in load()
        assert!(GlobalConfig::load_from(&path).is_err());
    }
}
