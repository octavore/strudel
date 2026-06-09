use std::collections::HashMap;
use std::path::PathBuf;

use secrecy::{ExposeSecret, SecretString};

use crate::config::extension::ExtensionKind;
use crate::config::user::NotaryAuth;

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
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
            && !self.apple_api_issuer.is_empty()
        {
            return Some(NotaryAuth {
                key_path: key_path.clone(),
                key_id: self.apple_api_key.clone(),
                issuer: self.apple_api_issuer.clone(),
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
        assert_eq!(auth.issuer, "ISS");
        assert_eq!(auth.key_path, PathBuf::from("/k.p8"));
    }

    #[test]
    fn notary_auth_none_when_nothing_set() {
        assert!(RESOLVED.notary_auth().is_none());
    }

    #[test]
    fn notary_auth_none_when_api_key_incomplete() {
        let mut r = RESOLVED.clone();
        // Key path present but key id missing → incomplete API set.
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
