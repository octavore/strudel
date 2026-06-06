use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

use crate::config::ResolvedConfig;
use crate::config::extension::ExtensionSection;
use crate::config::utils::{env_or, resolve_path};

/// The on-disk `strudel.toml`. Organized into sections; `deny_unknown_fields`
/// turns typos and stale flat keys into clear errors instead of silent no-ops.
#[derive(Debug, Deserialize, Clone)]
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
    #[serde(default)]
    pub ios: IosSection,
}

impl BuildConfig {
    pub fn resolve(self, config_dir: &Path) -> Result<ResolvedConfig> {
        let BuildConfig {
            app,
            build,
            signing,
            notarize,
            extensions,
            ios,
        } = self;

        let source_dir = resolve_path(config_dir, build.source_dir, ".");
        let build_dir = resolve_path(&source_dir, build.build_dir, ".build/dist");
        let target_name = build.target_name.unwrap_or_else(|| app.name.clone());
        let ios_simulator = ios.simulator.unwrap_or_else(|| "iPhone 16".to_string());
        let ios_device = ios.device;
        let ios_deployment_target = ios.deployment_target.unwrap_or_else(|| "18.0".to_string());
        let ios_assets_dir = ios.assets_dir.map(|p| {
            if p.is_absolute() {
                p
            } else {
                config_dir.join(p)
            }
        });
        let ios_app_icon_name = ios.app_icon_name.unwrap_or_else(|| "AppIcon".to_string());

        let extensions = extensions
            .into_iter()
            .map(|ext| ext.resolve(config_dir))
            .collect::<Result<Vec<_>>>()?;

        Ok(ResolvedConfig {
            // User-supplied input paths are resolved relative to the config file's
            // directory (the one fixed anchor the user reasons about), independent of
            // `source_dir`. info_json_path and icon_path are optional with no default.
            info_json_path: build.info_json_path.map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    // default is always ignored here.
                    resolve_path(config_dir, Some(p), "info.json")
                }
            }),
            entitlements_json_path: build.entitlements_json_path.map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    // default is always ignored here.
                    resolve_path(config_dir, Some(p), "entitlements.json")
                }
            }),
            icon_path: build.icon_path.map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    // default is always ignored here.
                    resolve_path(config_dir, Some(p), "icon.png")
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
            apple_api_issuer: env_or(notarize.api_issuer, "APPLE_API_ISSUER"),
            apple_api_key: env_or(notarize.api_key, "APPLE_API_KEY"),
            // Like other input paths, resolved relative to the config file directory.
            apple_api_key_path: std::env::var("APPLE_API_KEY_PATH")
                .ok()
                .map(PathBuf::from)
                .or(notarize.api_key_path)
                .map(|p| {
                    if p.is_absolute() {
                        p
                    } else {
                        config_dir.join(&p)
                    }
                }),
            // Secrets: environment only — these are never deserialized from the file.
            apple_certificate: std::env::var("APPLE_CERTIFICATE")
                .unwrap_or_default()
                .into(),
            apple_certificate_password: std::env::var("APPLE_CERTIFICATE_PASSWORD")
                .unwrap_or_default()
                .into(),
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
            resources_dir: build.resources_dir.map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    config_dir.join(&p)
                }
            }),
            resources: build
                .resources
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
            app_name: app.name,
            bundle_id: app.bundle_id,
            version: app.version,
            build_number: app.build_number,
            source_dir,
            build_dir,
            target_name,
            extensions,
            ios_simulator,
            ios_device,
            ios_deployment_target,
            ios_assets_dir,
            ios_app_icon_name,
        })
    }
}

/// `[app]` — required application metadata.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AppSection {
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub build_number: String,
}

/// `[build]` — inputs and outputs. All optional with defaults.
#[derive(Debug, Default, Deserialize, Clone)]
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
    /// Directory whose contents are merged into `Contents/Resources/`.
    pub resources_dir: Option<PathBuf>,
    /// Individual files to copy into `Contents/Resources/`.
    pub resources: Option<Vec<PathBuf>>,
}

/// `[signing]` — non-secret signing identifiers. Each may also come from the
/// matching env var (`APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`); the config
/// value wins.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SigningSection {
    pub identity: Option<String>,
    pub team_id: Option<String>,
}

/// `[notarize]` — non-secret notarization identifiers (env vars
/// `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`). Secrets
/// (`APPLE_CERTIFICATE*`) are read from the environment only.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NotarizeSection {
    pub api_issuer: Option<String>,
    pub api_key: Option<String>,
    pub api_key_path: Option<PathBuf>,
    pub timeout: Option<u64>,
}

/// `[ios]` — optional settings for iOS simulator and device workflows.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct IosSection {
    /// iOS Simulator name for `strudel sim`. Default: `"iPhone 16"`.
    pub simulator: Option<String>,
    /// Connected device name or UDID for `strudel device`.
    /// If unset, strudel auto-detects the first connected device.
    pub device: Option<String>,
    /// iOS deployment target (e.g. `"18.0"`). Default: `"18.0"`.
    pub deployment_target: Option<String>,
    /// Path to a `.xcassets` directory to compile into the bundle with
    /// `xcrun actool`. Optional; skipped when unset.
    pub assets_dir: Option<PathBuf>,
    /// Name of the app icon set inside `assets_dir`. Default: `"AppIcon"`.
    pub app_icon_name: Option<String>,
}

/// App Store Connect API key credentials for `notarytool`.
#[derive(Debug, Clone)]
pub struct NotaryAuth {
    pub key_path: PathBuf,
    pub key_id: String,
    pub issuer: String,
}

pub fn generate_initial_toml(
    app_name: &str,
    bundle_id: &str,
    version: &str,
    build_number: &str,
) -> String {
    indoc::formatdoc! {r#"
        # strudel.toml — strudel build configuration
        #
        # Commands:
        #   strudel bundle   — build app bundle only (no signing/notarization)
        #   strudel build    — build and sign the app bundle (no notarization/DMG); local dev
        #   strudel release  — full release: build, sign, notarize, and package DMG
        #
        # Signing & notarization (required for `release`). Identifiers may go here or in the
        # environment; secrets are read from the environment ONLY.
        #
        # Identifiers (here or env): APPLE_SIGNING_IDENTITY, APPLE_TEAM_ID,
        #   APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH
        # Secrets (env only): APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD

        [app]
        name         = "{app_name}"
        bundle_id    = "{bundle_id}"
        version      = "{version}"
        build_number = "{build_number}"

        # Paths are relative to this file's directory unless absolute.
        # Uncomment and edit to override.
        [build]
        # source_dir             = "."                  # Swift package directory
        # build_dir              = ".build/dist"        # artifacts (relative to source_dir)
        # info_json_path         = "info.json"          # optional; empty object if unset
        # entitlements_json_path = "entitlements.json"
        # icon_path              = "Sources/App/Assets.xcassets/AppIcon.appiconset/AppIcon.icns"  # optional; no icon if unset
        # archs                  = ["arm64", "x86_64"]  # default: host arch only
        # target_name            = "{app_name}"         # Swift target, if it differs from the app name

        # Dynamic C FFI libraries to embed in Contents/Frameworks and sign.
        # Paths are relative to this file's directory unless absolute.
        # Build-time flags (-I, -L, -l, rpath, modulemap) belong in Package.swift
        # (cSettings / linkerSettings); static libs need nothing here.
        # embed_libs             = ["path/to/libFoo.dylib"]

        # Resources copied into Contents/Resources/ during bundle assembly.
        # resources_dir = "Resources"               # directory; contents merged into Contents/Resources/
        # resources     = ["Assets/logo.png"]       # individual files copied by name

        # Provisioning profile embedded as Contents/embedded.provisionprofile.
        # Required for certain entitlements (e.g. push notifications, iCloud).
        # provisioning_profile   = "{app_name}.provisionprofile"

        # Extra environment variables forwarded to `swift build` (e.g. for
        # pkg-config or library discovery). Values are passed through verbatim.
        # [build_env]
        # PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig"

        # Signing identifiers — or set via APPLE_SIGNING_IDENTITY / APPLE_TEAM_ID.
        [signing]
        # identity = "Developer ID Application: Your Name (XXXXXXXXXX)"
        # team_id  = "XXXXXXXXXX"

        # Notarization identifiers — or set via APPLE_API_*.
        [notarize]
        # api_issuer   = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        # api_key      = "2X9R4HXF34"
        # api_key_path = "AuthKey_2X9R4HXF34.p8"
        # timeout      = 600

        # iOS simulator and device settings for `strudel sim` and `strudel device`.
        [ios]
        # simulator         = "iPhone 16"         # simulator name; default shown
        # device            = "My iPhone"          # name or UDID; auto-detected if unset
        # deployment_target = "18.0"               # iOS deployment target; default shown
        # assets_dir        = "Sources/{app_name}/Assets.xcassets"  # xcassets for actool
        # app_icon_name     = "AppIcon"            # icon set name inside assets_dir
    "#}
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use indoc::indoc;

    use super::*;
    use crate::config::BuildConfig;
    use crate::config::fixtures::*;

    #[test]
    fn generated_toml_parses_into_build_config() {
        // The scaffolded file must be valid input to the config loader —
        // otherwise `strudel init` produces a file `strudel build` rejects.
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.2.3", "42");
        let cfg: BuildConfig = toml::from_str(&t).expect("scaffolded TOML must parse");
        assert_eq!(cfg.app.name, "MyApp");
        assert_eq!(cfg.app.bundle_id, "com.example.myapp");
        assert_eq!(cfg.app.version, "1.2.3");
        assert_eq!(cfg.app.build_number, "42");
    }

    #[test]
    fn generated_toml_resolves_with_defaults() {
        // After parsing it must also resolve cleanly — i.e. every key the
        // template emits round-trips through resolve_config (no missing
        // required derived fields, no path resolution panics).
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.0", "1");
        let cfg: BuildConfig = toml::from_str(&t).unwrap();
        let r = cfg.resolve(Path::new("/cfg")).unwrap();
        assert_eq!(r.app_name, "MyApp");
        assert_eq!(r.target_name, "MyApp"); // default = app.name
        assert_eq!(r.notarize_timeout, 600); // default
    }

    #[test]
    fn parses_full_nested_config() {
        let cfg = parse_build_config(FULL).expect("should parse");
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
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
        "#})
        .expect("only [app] is required");
        assert!(cfg.build.source_dir.is_none());
        assert!(cfg.signing.identity.is_none());
        assert!(cfg.notarize.timeout.is_none());
    }

    #[test]
    fn missing_required_app_field_is_error() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
        "#});
        assert!(err.is_err(), "missing build_number should fail");
    }

    #[test]
    fn unknown_key_in_section_is_rejected() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [signing]
            sign_identity = "z"
        "#});
        assert!(err.is_err(), "typo'd key should be rejected");
    }

    #[test]
    fn resolves_paths_relative_to_config_dir() {
        let cfg = parse_build_config(FULL).unwrap();
        let r = cfg.resolve(Path::new("/cfg")).unwrap();
        // source_dir is relative to the config dir; build_dir is relative to
        // source_dir.
        assert_eq!(r.source_dir, PathBuf::from("/cfg/src"));
        assert_eq!(r.build_dir, PathBuf::from("/cfg/src/out"));
        // Input paths anchor on the config dir regardless of source_dir.
        assert_eq!(
            r.entitlements_json_path,
            Some(PathBuf::from("/cfg/ent.json"))
        );
        assert_eq!(r.apple_api_key_path, Some(PathBuf::from("/cfg/AuthKey.p8")));
    }

    #[test]
    fn absolute_paths_are_left_untouched() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [build]
            entitlements_json_path = "/abs/ent.json"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg")).unwrap();
        assert_eq!(
            r.entitlements_json_path,
            Some(PathBuf::from("/abs/ent.json"))
        );
    }

    #[test]
    fn applies_defaults() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "Defaulted"
            bundle_id = "y"
            version = "1"
            build_number = "1"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg")).unwrap();
        assert_eq!(r.build_dir, PathBuf::from("/cfg/.build/dist"));
        assert_eq!(r.entitlements_json_path, None);
        assert_eq!(r.target_name, "Defaulted"); // defaults to app.name
        assert_eq!(r.notarize_timeout, 600);
        assert_eq!(r.archs.len(), 1); // host arch
        assert!(r.info_json_path.is_none());
        assert!(r.icon_path.is_none());
    }

    #[test]
    fn ios_defaults_when_absent() {
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.0", "1");
        let cfg: BuildConfig = toml::from_str(&t).unwrap();
        let r = cfg.resolve(Path::new("/cfg")).unwrap();
        assert_eq!(r.ios_simulator, "iPhone 16");
        assert_eq!(r.ios_deployment_target, "18.0");
        assert_eq!(r.ios_app_icon_name, "AppIcon");
        assert!(r.ios_device.is_none());
        assert!(r.ios_assets_dir.is_none());
    }

    #[test]
    fn ios_section_overrides() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [ios]
            simulator = "iPhone 15 Pro"
            device = "00000000-0000-0000-0000-000000000000"
            deployment_target = "17.0"
            assets_dir = "Sources/Assets.xcassets"
            app_icon_name = "MyIcon"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg")).unwrap();
        assert_eq!(r.ios_simulator, "iPhone 15 Pro");
        assert_eq!(r.ios_deployment_target, "17.0");
        assert_eq!(r.ios_app_icon_name, "MyIcon");
        assert_eq!(
            r.ios_device.as_deref(),
            Some("00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(
            r.ios_assets_dir,
            Some(PathBuf::from("/cfg/Sources/Assets.xcassets"))
        );
    }

    #[test]
    fn config_value_wins_over_environment() {
        // A value present in the file is used verbatim, independent of any
        // ambient APPLE_SIGNING_IDENTITY in the environment (config takes precedence).
        let cfg = parse_build_config(FULL).unwrap();
        let r = cfg.resolve(Path::new("/cfg")).unwrap();
        assert_eq!(r.sign_identity, "Developer ID Application: Me (TEAM123456)");
        assert_eq!(r.team_id, "TEAM123456");
    }
}
