use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// The on-disk `strudel.toml`. Organized into sections; `deny_unknown_fields`
/// turns typos and stale flat keys into clear errors instead of silent no-ops.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    pub app: AppSection,
    #[serde(default)]
    pub build: BuildSection,
    #[serde(default)]
    pub signing: SigningSection,
    #[serde(default)]
    pub notarize: NotarizeSection,
    /// Zero or more app extensions embedded under `Contents/PlugIns/` in the
    /// host bundle. See [`ExtensionSection`].
    #[serde(default, rename = "extensions")]
    pub extensions: Vec<ExtensionSection>,
}

/// `[app]` — required application metadata.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AppSection {
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub build_number: String,
}

/// `[build]` — inputs and outputs. All optional with defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct BuildSection {
    pub source_dir: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,
    pub info_json_path: Option<PathBuf>,
    pub entitlements_json_path: Option<PathBuf>,
    pub icon_path: Option<PathBuf>,
    pub archs: Option<Vec<String>>,
    /// Swift executable target name. Defaults to the app name.
    pub target_name: Option<String>,
    /// Extra environment variables forwarded to `swift build`.
    pub build_env: Option<HashMap<String, String>>,
    /// Dynamic libraries to embed in `Contents/Frameworks` and sign.
    pub embed_libs: Option<Vec<PathBuf>>,
    /// Provisioning profile to embed as `Contents/embedded.provisionprofile`.
    pub provisioning_profile: Option<PathBuf>,
}

/// `[signing]` — non-secret signing identifiers. Each may also come from the
/// matching env var (`APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`); the config
/// value wins.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SigningSection {
    pub identity: Option<String>,
    pub team_id: Option<String>,
}

/// `[[extensions]]` — one entry per app extension embedded in the host bundle.
/// Fields shared by all extension kinds live at the top level; kind-specific
/// fields are carried by the flattened, `kind`-tagged [`ExtensionKindConfig`].
// note: not deny_unknown_fields because enum is internally tagged and deny_unknown_fields would
// reject the tag
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExtensionSection {
    /// Swift executable target name in `Package.swift`. The compiled binary
    /// is placed at `<name>.appex/Contents/MacOS/<target_name>`.
    pub target_name: String,
    /// `CFBundleIdentifier` of the extension — typically a child of the host
    /// app's bundle id (e.g. `com.example.myapp.Extension`).
    pub bundle_id: String,
    /// Display/bundle name. Defaults to `target_name`. Used for both the
    /// `.appex` directory name and `CFBundleName` / `CFBundleDisplayName`.
    pub name: Option<String>,
    /// JSON describing extra `Info.plist` keys for the extension. strudel
    /// always injects `CFBundle*` identity keys and the kind-specific
    /// `NSExtension` dict on top of this. Optional; defaults to an empty
    /// object.
    pub info_json_path: Option<PathBuf>,
    /// JSON entitlements for the extension — required, since extensions are
    /// sandboxed independently of the host app.
    pub entitlements_json_path: Option<PathBuf>,
    /// Discriminator (`kind = "..."`) plus the kind-specific fields. Internally
    /// tagged so the variant's fields sit flat alongside the common ones.
    #[serde(flatten)]
    pub kind: ExtensionKindConfig,
}

/// Kind-specific deserialized fields for an [`ExtensionSection`]. Internally
/// tagged on `kind`: each variant's fields appear at the same level as the
/// common extension fields in TOML.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionKindConfig {
    SafariWebExtension {
        /// Directory whose contents (manifest.json, JS, HTML, icons, …) are
        /// copied wholesale into `<name>.appex/Contents/Resources/`.
        resources_dir: PathBuf,
        /// `NSExtensionPrincipalClass`. Defaults to
        /// `"<target_name>.SafariWebExtensionHandler"` — the Apple Xcode
        /// template convention.
        principal_class: Option<String>,
    },
    AppExtension {
        /// `NSExtensionPointIdentifier` — identifies the extension point this
        /// extension targets (e.g. `"com.apple.share-services"` for a Share
        /// Extension, `"com.apple.FinderSync"` for a Finder Sync Extension).
        extension_point_identifier: String,
        /// `NSExtensionPrincipalClass`. Optional — some extension points require
        /// it, others do not.
        principal_class: Option<String>,
    },
}

/// The kind of app extension after resolution. A flat discriminator used by
/// downstream build steps; the kind-specific data has already been unpacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionKind {
    SafariWebExtension,
    AppExtension,
}

/// `[notarize]` — non-secret notarization identifiers (env vars `APPLE_ID`,
/// `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`). Secrets
/// (`APPLE_PASSWORD`, `APPLE_CERTIFICATE*`) are read from the environment only.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NotarizeSection {
    pub apple_id: Option<String>,
    pub api_issuer: Option<String>,
    pub api_key: Option<String>,
    pub api_key_path: Option<PathBuf>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub app_name: String,
    pub bundle_id: String,
    pub version: String,
    pub build_number: String,
    pub source_dir: PathBuf,
    pub build_dir: PathBuf,
    pub info_json_path: Option<PathBuf>,
    pub entitlements_json_path: PathBuf,
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

    // Notarization identifiers (from strudel.toml or the environment).
    pub team_id: String,
    pub apple_id: String,
    pub apple_api_issuer: String,
    pub apple_api_key: String,
    pub apple_api_key_path: Option<PathBuf>,

    // Secrets — read from the environment only, never from strudel.toml.
    pub apple_password: String,
    pub apple_certificate: String,
    pub apple_certificate_password: String,
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
    /// [`ExtensionKind::SafariWebExtension`] and [`ExtensionKind::AppExtension`].
    pub principal_class: Option<String>,
    /// `NSExtensionPointIdentifier`. Required for [`ExtensionKind::AppExtension`];
    /// always `None` for other kinds (the identifier is hardcoded per-kind).
    pub extension_point_identifier: Option<String>,
}

/// How `notarytool` authenticates with Apple's notary service. The App Store
/// Connect API key is preferred when fully configured; otherwise we fall back
/// to Apple ID + app-specific password.
#[derive(Debug, Clone)]
pub enum NotaryAuth {
    ApiKey {
        key_path: PathBuf,
        key_id: String,
        issuer: String,
    },
    AppleId {
        apple_id: String,
        password: String,
        team_id: String,
    },
}

impl ResolvedConfig {
    /// The notarization credentials, if a complete set is available. Prefers
    /// the API key; returns `None` when neither set is fully specified.
    pub fn notary_auth(&self) -> Option<NotaryAuth> {
        if let Some(key_path) = &self.apple_api_key_path
            && !self.apple_api_key.is_empty()
            && !self.apple_api_issuer.is_empty()
        {
            return Some(NotaryAuth::ApiKey {
                key_path: key_path.clone(),
                key_id: self.apple_api_key.clone(),
                issuer: self.apple_api_issuer.clone(),
            });
        }
        if !self.apple_id.is_empty() && !self.apple_password.is_empty() && !self.team_id.is_empty()
        {
            return Some(NotaryAuth::AppleId {
                apple_id: self.apple_id.clone(),
                password: self.apple_password.clone(),
                team_id: self.team_id.clone(),
            });
        }
        None
    }

    /// The signing certificate to import, if supplied via the environment, as
    /// `(base64 PKCS#12 data, export password)`. When `None`, the identity is
    /// assumed to be present in an existing keychain (the common local case).
    pub fn signing_cert(&self) -> Option<(&str, &str)> {
        if self.apple_certificate.is_empty() {
            None
        } else {
            Some((&self.apple_certificate, &self.apple_certificate_password))
        }
    }
}

fn resolve_path(base: &Path, p: Option<PathBuf>, default: impl AsRef<Path>) -> PathBuf {
    p.map(|p| if p.is_absolute() { p } else { base.join(&p) })
        .unwrap_or_else(|| base.join(default))
}

fn env_or(cfg_val: Option<String>, env_key: &str) -> String {
    cfg_val
        .or_else(|| std::env::var(env_key).ok())
        .unwrap_or_default()
}

/// Resolve a parsed extension entry against the config directory, applying
/// defaults and validating kind-specific required fields.
fn resolve_extension(ext: ExtensionSection, config_dir: &Path) -> Result<ResolvedExtension> {
    let ExtensionSection {
        target_name,
        bundle_id,
        name,
        info_json_path,
        entitlements_json_path,
        kind,
    } = ext;

    let resolved_name = name.unwrap_or_else(|| target_name.clone());
    let resolve = |p: PathBuf| {
        if p.is_absolute() {
            p
        } else {
            config_dir.join(p)
        }
    };

    let entitlements_json_path = entitlements_json_path.map(resolve).with_context(|| {
        format!(
            "extension `{target_name}` is missing required field `entitlements_json_path` \
             (extensions are sandboxed independently of the host app)"
        )
    })?;

    let info_json_path = info_json_path.map(resolve);

    let (kind, resources_dir, principal_class, extension_point_identifier) = match kind {
        ExtensionKindConfig::SafariWebExtension {
            resources_dir,
            principal_class,
        } => {
            let principal_class = principal_class
                .unwrap_or_else(|| format!("{target_name}.SafariWebExtensionHandler"));
            (
                ExtensionKind::SafariWebExtension,
                Some(resolve(resources_dir)),
                Some(principal_class),
                None,
            )
        },
        ExtensionKindConfig::AppExtension {
            extension_point_identifier,
            principal_class,
        } => (
            ExtensionKind::AppExtension,
            None,
            principal_class,
            Some(extension_point_identifier),
        ),
    };

    Ok(ResolvedExtension {
        kind,
        target_name,
        bundle_id,
        name: resolved_name,
        info_json_path,
        entitlements_json_path,
        resources_dir,
        principal_class,
        extension_point_identifier,
    })
}

pub fn resolve_config(cfg: BuildConfig, config_dir: &Path) -> Result<ResolvedConfig> {
    let BuildConfig {
        app,
        build,
        signing,
        notarize,
        extensions,
    } = cfg;

    let source_dir = resolve_path(config_dir, build.source_dir, ".");
    let build_dir = resolve_path(&source_dir, build.build_dir, ".build/dist");
    let target_name = build.target_name.unwrap_or_else(|| app.name.clone());

    let extensions = extensions
        .into_iter()
        .map(|ext| resolve_extension(ext, config_dir))
        .collect::<Result<Vec<_>>>()?;

    Ok(ResolvedConfig {
        // User-supplied input paths are resolved relative to the config file's
        // directory (the one fixed anchor the user reasons about), independent of
        // `source_dir`. info_json_path and icon_path are optional with no default.
        info_json_path: build.info_json_path.map(|p| {
            if p.is_absolute() {
                p
            } else {
                config_dir.join(&p)
            }
        }),
        entitlements_json_path: resolve_path(
            config_dir,
            build.entitlements_json_path,
            "entitlements.json",
        ),
        icon_path: build.icon_path.map(|p| {
            if p.is_absolute() {
                p
            } else {
                config_dir.join(&p)
            }
        }),
        archs: build.archs.unwrap_or_else(|| {
            let arch = match std::env::consts::ARCH {
                "aarch64" => "arm64",
                other => other,
            };
            vec![arch.to_string()]
        }),
        // Identifiers: strudel.toml value wins, else the matching env var.
        sign_identity: env_or(signing.identity, "APPLE_SIGNING_IDENTITY"),
        team_id: env_or(signing.team_id, "APPLE_TEAM_ID"),
        apple_id: env_or(notarize.apple_id, "APPLE_ID"),
        apple_api_issuer: env_or(notarize.api_issuer, "APPLE_API_ISSUER"),
        apple_api_key: env_or(notarize.api_key, "APPLE_API_KEY"),
        // Like other input paths, resolved relative to the config file directory.
        apple_api_key_path: notarize
            .api_key_path
            .or_else(|| std::env::var("APPLE_API_KEY_PATH").ok().map(PathBuf::from))
            .map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    config_dir.join(&p)
                }
            }),
        // Secrets: environment only — these are never deserialized from the file.
        apple_password: std::env::var("APPLE_PASSWORD").unwrap_or_default(),
        apple_certificate: std::env::var("APPLE_CERTIFICATE").unwrap_or_default(),
        apple_certificate_password: std::env::var("APPLE_CERTIFICATE_PASSWORD").unwrap_or_default(),
        notarize_timeout: notarize.timeout.unwrap_or(600),
        build_env: build.build_env.unwrap_or_default(),
        embed_libs: build
            .embed_libs
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    config_dir.join(&p)
                }
            })
            .collect(),
        provisioning_profile: build.provisioning_profile.map(|p| {
            if p.is_absolute() {
                p
            } else {
                config_dir.join(&p)
            }
        }),
        app_name: app.name,
        bundle_id: app.bundle_id,
        version: app.version,
        build_number: app.build_number,
        source_dir,
        build_dir,
        target_name,
        extensions,
    })
}

pub fn load_config(config_path: &Path) -> Result<ResolvedConfig> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
    let cfg: BuildConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config: {}", config_path.display()))?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    resolve_config(cfg, config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
        [app]
        name = "MyApp"
        bundle_id = "com.example.myapp"
        version = "1.2.3"
        build_number = "42"

        [build]
        source_dir = "src"
        build_dir = "out"
        entitlements_json_path = "ent.json"
        archs = ["arm64", "x86_64"]
        target_name = "MyAppBin"

        [signing]
        identity = "Developer ID Application: Me (TEAM123456)"
        team_id = "TEAM123456"

        [notarize]
        apple_id = "me@example.com"
        api_issuer = "issuer-uuid"
        api_key = "KEYID123"
        api_key_path = "AuthKey.p8"
        timeout = 1200
    "#;

    fn parse(s: &str) -> Result<BuildConfig, toml::de::Error> {
        toml::from_str(s)
    }

    // ── Parsing & the nested layout ─────────────────────────────────────────

    #[test]
    fn parses_full_nested_config() {
        let cfg = parse(FULL).expect("should parse");
        assert_eq!(cfg.app.name, "MyApp");
        assert_eq!(cfg.app.build_number, "42");
        assert_eq!(
            cfg.build.archs.as_deref(),
            Some(&["arm64".into(), "x86_64".into()][..])
        );
        assert_eq!(
            cfg.signing.identity.as_deref(),
            Some("Developer ID Application: Me (TEAM123456)")
        );
        assert_eq!(cfg.notarize.api_key.as_deref(), Some("KEYID123"));
        assert_eq!(cfg.notarize.timeout, Some(1200));
    }

    #[test]
    fn optional_sections_default_when_absent() {
        let cfg = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
        "#,
        )
        .expect("only [app] is required");
        assert!(cfg.build.source_dir.is_none());
        assert!(cfg.signing.identity.is_none());
        assert!(cfg.notarize.timeout.is_none());
    }

    #[test]
    fn missing_required_app_field_is_error() {
        let err = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
        "#,
        );
        assert!(err.is_err(), "missing build_number should fail");
    }

    #[test]
    fn stale_flat_layout_is_rejected() {
        // The old pre-nesting format must not silently parse.
        let err = parse(
            r#"
            app_name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
        "#,
        );
        assert!(
            err.is_err(),
            "flat keys should be rejected by deny_unknown_fields"
        );
    }

    #[test]
    fn unknown_key_in_section_is_rejected() {
        // `sign_identity` is the old name; the field is now `identity`.
        let err = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [signing]
            sign_identity = "z"
        "#,
        );
        assert!(err.is_err(), "typo'd key should be rejected");
    }

    // ── resolve_config (env-independent fields) ─────────────────────────────

    #[test]
    fn resolves_paths_relative_to_config_dir() {
        let cfg = parse(FULL).unwrap();
        let r = resolve_config(cfg, Path::new("/cfg")).unwrap();
        // source_dir is relative to the config dir; build_dir is relative to
        // source_dir.
        assert_eq!(r.source_dir, PathBuf::from("/cfg/src"));
        assert_eq!(r.build_dir, PathBuf::from("/cfg/src/out"));
        // Input paths anchor on the config dir regardless of source_dir.
        assert_eq!(r.entitlements_json_path, PathBuf::from("/cfg/ent.json"));
        assert_eq!(r.apple_api_key_path, Some(PathBuf::from("/cfg/AuthKey.p8")));
    }

    #[test]
    fn absolute_paths_are_left_untouched() {
        let cfg = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [build]
            entitlements_json_path = "/abs/ent.json"
        "#,
        )
        .unwrap();
        let r = resolve_config(cfg, Path::new("/cfg")).unwrap();
        assert_eq!(r.entitlements_json_path, PathBuf::from("/abs/ent.json"));
    }

    #[test]
    fn applies_defaults() {
        let cfg = parse(
            r#"
            [app]
            name = "Defaulted"
            bundle_id = "y"
            version = "1"
            build_number = "1"
        "#,
        )
        .unwrap();
        let r = resolve_config(cfg, Path::new("/cfg")).unwrap();
        assert_eq!(r.build_dir, PathBuf::from("/cfg/.build/dist"));
        assert_eq!(
            r.entitlements_json_path,
            PathBuf::from("/cfg/entitlements.json")
        );
        assert_eq!(r.target_name, "Defaulted"); // defaults to app.name
        assert_eq!(r.notarize_timeout, 600);
        assert_eq!(r.archs.len(), 1); // host arch
        assert!(r.info_json_path.is_none());
        assert!(r.icon_path.is_none());
    }

    #[test]
    fn config_value_wins_over_environment() {
        // A value present in the file is used verbatim, independent of any
        // ambient APPLE_SIGNING_IDENTITY in the environment (config takes precedence).
        let cfg = parse(FULL).unwrap();
        let r = resolve_config(cfg, Path::new("/cfg")).unwrap();
        assert_eq!(r.sign_identity, "Developer ID Application: Me (TEAM123456)");
        assert_eq!(r.team_id, "TEAM123456");
    }

    // ── NotaryAuth / signing_cert helpers (pure) ────────────────────────────

    fn resolved() -> ResolvedConfig {
        ResolvedConfig {
            app_name: "A".into(),
            bundle_id: "b".into(),
            version: "1".into(),
            build_number: "1".into(),
            source_dir: PathBuf::from("/x"),
            build_dir: PathBuf::from("/x"),
            info_json_path: None,
            entitlements_json_path: PathBuf::from("/x/e.json"),
            icon_path: None,
            archs: vec!["arm64".into()],
            target_name: "A".into(),
            sign_identity: String::new(),
            notarize_timeout: 600,
            build_env: HashMap::new(),
            embed_libs: Vec::new(),
            provisioning_profile: None,
            extensions: Vec::new(),
            team_id: String::new(),
            apple_id: String::new(),
            apple_api_issuer: String::new(),
            apple_api_key: String::new(),
            apple_api_key_path: None,
            apple_password: String::new(),
            apple_certificate: String::new(),
            apple_certificate_password: String::new(),
        }
    }


    #[test]
    fn notary_auth_prefers_complete_api_key() {
        let mut r = resolved();
        r.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        r.apple_api_key = "KID".into();
        r.apple_api_issuer = "ISS".into();
        // Apple ID is also fully present; the API key must still win.
        r.apple_id = "me@example.com".into();
        r.apple_password = "pw".into();
        r.team_id = "TID".into();
        match r.notary_auth() {
            Some(NotaryAuth::ApiKey {
                key_id,
                issuer,
                key_path,
            }) => {
                assert_eq!(key_id, "KID");
                assert_eq!(issuer, "ISS");
                assert_eq!(key_path, PathBuf::from("/k.p8"));
            },
            other => panic!("expected ApiKey, got {other:?}"),
        }
    }

    #[test]
    fn notary_auth_falls_back_to_apple_id_when_api_incomplete() {
        let mut r = resolved();
        // Key path present but key id missing → incomplete API set.
        r.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        r.apple_id = "me@example.com".into();
        r.apple_password = "pw".into();
        r.team_id = "TID".into();
        assert!(matches!(r.notary_auth(), Some(NotaryAuth::AppleId { .. })));
    }

    #[test]
    fn notary_auth_none_when_nothing_set() {
        assert!(resolved().notary_auth().is_none());
    }

    #[test]
    fn notary_auth_none_when_apple_id_missing_password() {
        let mut r = resolved();
        r.apple_id = "me@example.com".into();
        r.team_id = "TID".into();
        // apple_password is empty (env-only secret, unset) → not a complete set.
        assert!(r.notary_auth().is_none());
    }

    #[test]
    fn signing_cert_present_and_absent() {
        assert!(resolved().signing_cert().is_none());
        let mut r = resolved();
        r.apple_certificate = "BASE64".into();
        r.apple_certificate_password = "pw".into();
        assert_eq!(r.signing_cert(), Some(("BASE64", "pw")));
    }

    // ── Extensions ──────────────────────────────────────────────────────────

    const WITH_EXTENSION: &str = r#"
        [app]
        name = "MyApp"
        bundle_id = "com.example.myapp"
        version = "1.0.0"
        build_number = "1"

        [[extensions]]
        kind = "safari_web_extension"
        target_name = "MyAppExtension"
        bundle_id = "com.example.myapp.Extension"
        resources_dir = "ext/dist"
        entitlements_json_path = "ext/entitlements.json"
    "#;

    #[test]
    fn parses_and_resolves_safari_web_extension() {
        let cfg = parse(WITH_EXTENSION).unwrap();
        let r = resolve_config(cfg, Path::new("/cfg")).unwrap();
        assert_eq!(r.extensions.len(), 1);
        let ext = &r.extensions[0];
        assert_eq!(ext.kind, ExtensionKind::SafariWebExtension);
        assert_eq!(ext.target_name, "MyAppExtension");
        // name defaults to target_name
        assert_eq!(ext.name, "MyAppExtension");
        assert_eq!(ext.bundle_id, "com.example.myapp.Extension");
        assert_eq!(ext.resources_dir, Some(PathBuf::from("/cfg/ext/dist")));
        assert_eq!(
            ext.entitlements_json_path,
            PathBuf::from("/cfg/ext/entitlements.json")
        );
        // principal_class defaults to "<target_name>.SafariWebExtensionHandler"
        assert_eq!(
            ext.principal_class.as_deref(),
            Some("MyAppExtension.SafariWebExtensionHandler")
        );
    }

    #[test]
    fn safari_web_extension_requires_resources_dir() {
        let err = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [[extensions]]
            kind = "safari_web_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            entitlements_json_path = "e.json"
        "#,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("resources_dir"), "got: {msg}");
    }

    #[test]
    fn extension_requires_entitlements() {
        let cfg = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [[extensions]]
            kind = "safari_web_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            resources_dir = "ext"
        "#,
        )
        .unwrap();
        let err = resolve_config(cfg, Path::new("/cfg")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("entitlements_json_path"), "got: {msg}");
    }

    #[test]
    fn extension_unknown_field_is_rejected() {
        let err = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [[extensions]]
            kind = "safari_web_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            resources_dir = "ext"
            entitlements_json_path = "e.json"
            unknown_field = "boom"
        "#,
        );
        assert!(err.is_err(), "typo'd extension key should be rejected");
    }

    #[test]
    fn unknown_extension_kind_is_rejected() {
        let err = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [[extensions]]
            kind = "share_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            entitlements_json_path = "e.json"
        "#,
        );
        assert!(err.is_err(), "unknown extension kind should be rejected");
    }

    const WITH_APP_EXTENSION: &str = r#"
        [app]
        name = "MyApp"
        bundle_id = "com.example.myapp"
        version = "1.0.0"
        build_number = "1"

        [[extensions]]
        kind = "app_extension"
        target_name = "MyShareExtension"
        bundle_id = "com.example.myapp.Share"
        extension_point_identifier = "com.apple.share-services"
        entitlements_json_path = "share/entitlements.json"
    "#;

    #[test]
    fn parses_and_resolves_app_extension() {
        let cfg = parse(WITH_APP_EXTENSION).unwrap();
        let r = resolve_config(cfg, Path::new("/cfg")).unwrap();
        assert_eq!(r.extensions.len(), 1);
        let ext = &r.extensions[0];
        assert_eq!(ext.kind, ExtensionKind::AppExtension);
        assert_eq!(ext.target_name, "MyShareExtension");
        assert_eq!(ext.name, "MyShareExtension");
        assert_eq!(ext.bundle_id, "com.example.myapp.Share");
        assert_eq!(
            ext.extension_point_identifier.as_deref(),
            Some("com.apple.share-services")
        );
        assert!(ext.principal_class.is_none());
        assert!(ext.resources_dir.is_none());
        assert_eq!(
            ext.entitlements_json_path,
            PathBuf::from("/cfg/share/entitlements.json")
        );
    }

    #[test]
    fn app_extension_accepts_optional_principal_class() {
        let cfg = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [[extensions]]
            kind = "app_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            extension_point_identifier = "com.apple.FinderSync"
            principal_class = "Ext.FinderSyncController"
            entitlements_json_path = "e.json"
        "#,
        )
        .unwrap();
        let r = resolve_config(cfg, Path::new("/cfg")).unwrap();
        let ext = &r.extensions[0];
        assert_eq!(ext.principal_class.as_deref(), Some("Ext.FinderSyncController"));
    }

    #[test]
    fn app_extension_requires_extension_point_identifier() {
        let err = parse(
            r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [[extensions]]
            kind = "app_extension"
            target_name = "Ext"
            bundle_id = "y.Ext"
            entitlements_json_path = "e.json"
        "#,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("extension_point_identifier"), "got: {msg}");
    }
}
