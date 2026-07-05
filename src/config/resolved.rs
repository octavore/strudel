use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use secrecy::{ExposeSecret, SecretString};

use crate::config::build_target::{IosProvisioningBackend, Platform};
use crate::config::extension::ExtensionKind;
use crate::config::user::NotaryAuth;

/// Resolved `[dmg]` customization. `None` in `ResolvedConfig` only when the
/// user explicitly sets `plain = true` in their `[dmg]` section; the plain UDZO
/// path is used in that case. When absent entirely from the config, defaults
/// are applied and the styled window (a generated `.DS_Store`) is produced.
#[derive(Debug, Clone)]
pub struct ResolvedDmg {
    pub background: Option<PathBuf>,
    pub window_width: u32,
    pub window_height: u32,
    pub icon_size: u32,
    pub app_x: u32,
    pub app_y: u32,
    pub applications_x: u32,
    pub applications_y: u32,
}

impl Default for ResolvedDmg {
    fn default() -> Self {
        ResolvedDmg {
            background: None,
            window_width: 660,
            window_height: 400,
            icon_size: 128,
            app_x: 192,
            app_y: 162,
            applications_x: 468,
            applications_y: 162,
        }
    }
}

/// All resolved targets from a `strudel.toml`. Single-target configs produce
/// exactly one entry; `[[target]]` configs produce one per block.
#[derive(Debug)]
pub struct ResolvedProject {
    pub targets: Vec<ResolvedConfig>,
}

impl ResolvedProject {
    /// Return the subset of targets eligible for `platform`, optionally
    /// filtered by `name` (matched against `app_name`).
    ///
    /// - Agnostic targets (`platform: None`) are eligible for every platform.
    /// - `allow_all = true` returns all eligible when no name is given.
    /// - `allow_all = false` with >1 eligible target (and no name) is an error.
    pub fn select(
        &self,
        name: Option<&str>,
        platform: Platform,
        allow_all: bool,
    ) -> Result<Vec<&ResolvedConfig>> {
        let eligible: Vec<&ResolvedConfig> = self
            .targets
            .iter()
            .filter(|t| t.platform.is_none_or(|p| p == platform))
            .collect();

        if eligible.is_empty() {
            bail!("No {} targets in strudel.toml", platform.label());
        }

        if let Some(name) = name {
            let matched: Vec<&ResolvedConfig> = eligible
                .iter()
                .filter(|t| t.app_name == name)
                .copied()
                .collect();
            if matched.is_empty() {
                let available: Vec<&str> = eligible.iter().map(|t| t.app_name.as_str()).collect();
                bail!(
                    "No {} target named {:?}. Available: {}",
                    platform.label(),
                    name,
                    available.join(", ")
                );
            }
            return Ok(matched);
        }

        if allow_all {
            return Ok(eligible);
        }

        if eligible.len() == 1 {
            return Ok(eligible);
        }

        let available: Vec<&str> = eligible.iter().map(|t| t.app_name.as_str()).collect();
        bail!(
            "Multiple {} targets; select one with --target. Available: {}",
            platform.label(),
            available.join(", ")
        );
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub platform: Option<Platform>,
    pub app_name: String,
    pub bundle_id: String,
    pub version: String,
    pub build_number: String,
    pub source_dir: PathBuf,
    pub build_dir: PathBuf,
    pub info_json_path: Option<PathBuf>,
    pub entitlements_json_path: Option<PathBuf>,
    pub icon_path: Option<PathBuf>,
    pub archs: Vec<String>,
    pub target_name: String,
    pub sign_identity: String,
    pub notarize_timeout: u64,
    /// Extra environment variables forwarded to `swift build`.
    pub build_env: HashMap<String, String>,
    /// Dynamic libraries resolved and ready to embed in `Contents/Frameworks`.
    pub embed_libs: Vec<PathBuf>,
    /// Provisioning profile to embed in the bundle, if configured.
    pub provisioning_profile: Option<PathBuf>,
    /// App extensions to assemble and sign inside `Contents/PlugIns/`.
    pub extensions: Vec<ResolvedExtension>,
    /// Directory whose contents are merged into `Contents/Resources/`.
    pub resources_dir: Option<PathBuf>,
    /// Individual files to copy into `Contents/Resources/`.
    pub resources: Vec<PathBuf>,

    pub target_platform: ResolvedTargetPlatform,

    // Notarization identifiers (from strudel.toml or the environment).
    pub team_id: String,
    pub apple_api_issuer: String,
    pub apple_api_key: String,
    pub apple_api_key_path: Option<PathBuf>,

    // Secrets — read from the environment only, never from strudel.toml.
    pub apple_certificate: SecretString,
    pub apple_certificate_password: SecretString,
}

impl ResolvedConfig {
    /// The notarization credentials, if a complete set is available. Returns
    /// `None` when the API key set is not fully specified.
    pub fn notary_auth(&self) -> Option<NotaryAuth> {
        if let Some(key_path) = &self.apple_api_key_path
            && !self.apple_api_key.is_empty()
        {
            return Some(NotaryAuth {
                key_path: key_path.clone(),
                key_id: self.apple_api_key.clone(),
                issuer: (!self.apple_api_issuer.is_empty()).then(|| self.apple_api_issuer.clone()),
            });
        }
        None
    }

    /// The signing certificate to import, if supplied via the environment, as
    /// `(base64 PKCS#12 data, export password)`. When `None`, the identity is
    /// assumed to be present in an existing keychain (the common local case).
    pub fn signing_cert(&self) -> Option<(SecretString, SecretString)> {
        if self.apple_certificate.expose_secret().is_empty() {
            None
        } else {
            Some((
                self.apple_certificate.clone(),
                self.apple_certificate_password.clone(),
            ))
        }
    }
}

/// An [`ExtensionSection`] after path resolution and kind-specific validation.
/// All paths are absolute (resolved relative to the config file's directory).
#[derive(Debug, Clone)]
pub struct ResolvedExtension {
    pub kind: ExtensionKind,
    pub target_name: String,
    pub bundle_id: String,
    /// Resolved display name (defaults to `target_name`).
    pub name: String,
    pub info_json_path: Option<PathBuf>,
    pub entitlements_json_path: PathBuf,
    /// Required for [`ExtensionKind::SafariWebExtension`]; the directory whose
    /// contents become the extension's `Resources/`.
    pub resources_dir: Option<PathBuf>,
    /// Resolved `NSExtensionPrincipalClass`. Used by both
    /// [`ExtensionKind::SafariWebExtension`] and
    /// [`ExtensionKind::AppExtension`].
    pub principal_class: Option<String>,
    /// `NSExtensionPointIdentifier`. Required for
    /// [`ExtensionKind::AppExtension`]; always `None` for other kinds (the
    /// identifier is hardcoded per-kind).
    pub extension_point_identifier: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ResolvedTargetPlatform {
    Mac(ResolvedMacOsSection),
    Ios(ResolvedIosSection),
}

#[derive(Debug, Clone)]
pub struct ResolvedMacOsSection {
    pub dmg: Option<ResolvedDmg>,
}

impl From<ResolvedMacOsSection> for ResolvedTargetPlatform {
    fn from(macos: ResolvedMacOsSection) -> Self {
        ResolvedTargetPlatform::Mac(macos)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedIosSection {
    pub simulator: String,
    pub device: Option<String>,
    pub deployment_target: String,
    pub assets_dir: Option<PathBuf>,
    pub app_icon_name: String,
    pub provisioning: IosProvisioningBackend,
    pub apple_id: Option<String>,
}

impl From<ResolvedIosSection> for ResolvedTargetPlatform {
    fn from(ios: ResolvedIosSection) -> Self {
        ResolvedTargetPlatform::Ios(ios)
    }
}

#[cfg(test)]
mod select_tests {
    use crate::config::build_target::Platform;
    use crate::config::fixtures::MULTI;
    use crate::config::resolved::ResolvedProject;
    use crate::config::user::BuildConfig;

    fn two_target_project() -> ResolvedProject {
        use std::path::Path;
        let cfg: BuildConfig = toml::from_str(MULTI).unwrap();
        cfg.resolve_project(Path::new("/cfg"), None).unwrap()
    }

    #[test]
    fn select_filters_by_platform() {
        let proj = two_target_project();
        let macos = proj.select(None, Platform::Macos, true).unwrap();
        assert_eq!(macos.len(), 1);
        assert_eq!(macos[0].platform, Some(Platform::Macos));

        let ios = proj.select(None, Platform::Ios, true).unwrap();
        assert_eq!(ios.len(), 1);
        assert_eq!(ios[0].platform, Some(Platform::Ios));
    }

    #[test]
    fn no_matching_platform_is_error() {
        // Build a macos-only project, then ask for iOS.
        use std::path::Path;

        use indoc::indoc;
        let cfg: BuildConfig = toml::from_str(indoc! { r#"
            [[target]]
            platform = "macos"
            app.name = "A"
            app.bundle_id = "com.a"
            app.version = "1"
            app.build_number = "1"
        "#})
        .unwrap();
        let proj = cfg.resolve_project(Path::new("/cfg"), None).unwrap();
        let err = proj.select(None, Platform::Ios, false).unwrap_err();
        assert!(format!("{err}").contains("No iOS targets"), "got: {err}");
    }

    #[test]
    fn name_not_found_is_error() {
        let proj = two_target_project();
        let err = proj
            .select(Some("DoesNotExist"), Platform::Macos, true)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("DoesNotExist"), "got: {msg}");
    }

    #[test]
    fn ambiguous_without_target_flag_is_error() {
        use std::path::Path;

        use indoc::indoc;
        let cfg: BuildConfig = toml::from_str(indoc! { r#"
            [[target]]
            platform = "macos"
            app.name = "AppA"
            app.bundle_id = "com.a"
            app.version = "1"
            app.build_number = "1"

            [[target]]
            platform = "macos"
            app.name = "AppB"
            app.bundle_id = "com.b"
            app.version = "1"
            app.build_number = "1"
        "#})
        .unwrap();
        let proj = cfg.resolve_project(Path::new("/cfg"), None).unwrap();
        // allow_all = false, no name -> error when >1 eligible
        let err = proj.select(None, Platform::Macos, false).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("--target"), "got: {msg}");
    }

    #[test]
    fn allow_all_returns_all_eligible() {
        let proj = two_target_project();
        // MULTI has one macos and one ios target; asking for macos with allow_all=true
        // returns the single macos one.
        let targets = proj.select(None, Platform::Macos, true).unwrap();
        assert_eq!(targets.len(), 1);
    }
}

#[cfg(test)]
pub mod fixtures {
    use std::path::PathBuf;

    use secrecy::ExposeSecret;

    use crate::config::fixtures::RESOLVED;

    #[test]
    fn notary_auth_returns_api_key_when_complete() {
        let mut r = RESOLVED.clone();
        r.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        r.apple_api_key = "KID".into();
        r.apple_api_issuer = "ISS".into();

        let auth = r.notary_auth().expect("expected Some(NotaryAuth)");
        assert_eq!(auth.key_id, "KID");
        assert_eq!(auth.issuer, Some("ISS".into()));
        assert_eq!(auth.key_path, PathBuf::from("/k.p8"));
    }

    #[test]
    fn notary_auth_none_when_nothing_set() {
        assert!(RESOLVED.notary_auth().is_none());
    }

    #[test]
    fn notary_auth_none_when_api_key_incomplete() {
        let mut r = RESOLVED.clone();
        // Key path present but key id missing -> incomplete API set.
        r.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        assert!(r.notary_auth().is_none());
    }

    #[test]
    fn signing_cert_present_and_absent() {
        assert!(RESOLVED.signing_cert().is_none());
        let mut r = RESOLVED.clone();
        r.apple_certificate = "BASE64".into();
        r.apple_certificate_password = "pw".into();
        let (cert, pw) = r.signing_cert().unwrap();
        assert_eq!(cert.expose_secret(), "BASE64");
        assert_eq!(pw.expose_secret(), "pw");
    }
}
