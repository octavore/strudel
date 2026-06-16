use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::ResolvedConfig;
use crate::config::extension::ExtensionSection;
use crate::config::global::GlobalConfig;
use crate::config::resolved::{ResolvedDmg, ResolvedProject};
use crate::config::utils::{env_or_global, resolve_path, resolve_to};

/// The target platform for a build target.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Ios,
}

impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Platform::Macos => "macOS",
            Platform::Ios => "iOS",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Ios => "ios",
        }
    }
}

/// One entry in a `[[target]]` array — a product × platform pair.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TargetSection {
    pub platform: Option<Platform>,
    pub app: AppSection,
    #[serde(default)]
    pub build: BuildSection,
    #[serde(default, rename = "extensions")]
    pub extensions: Vec<ExtensionSection>,
    pub dmg: Option<DmgSection>,
    pub ios: Option<IosSection>,
}

/// The on-disk `strudel.toml`. Organized into sections; `deny_unknown_fields`
/// turns typos and stale flat keys into clear errors instead of silent no-ops.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    #[serde(default)]
    pub app: Option<AppSection>,
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
    #[serde(default)]
    pub dmg: Option<DmgSection>,
    /// Multi-target declarations — one per `[[target]]` block.
    #[serde(default)]
    pub target: Vec<TargetSection>,
}

impl BuildConfig {
    pub fn resolve_project(
        self,
        config_dir: &Path,
        global: Option<&GlobalConfig>,
    ) -> Result<ResolvedProject> {
        let global_default;
        let global = match global {
            Some(g) => g,
            None => {
                global_default = GlobalConfig::default();
                &global_default
            },
        };

        let BuildConfig {
            app,
            build,
            signing,
            notarize,
            extensions,
            ios,
            dmg,
            target,
        } = self;

        let targets = if !target.is_empty() {
            if app.is_some() || !extensions.is_empty() || dmg.is_some() {
                bail!(
                    "strudel.toml cannot mix top-level [app] / [[extensions]] / [dmg] \
                     with [[target]] sections"
                );
            }
            for (i, t) in target.iter().enumerate() {
                if t.platform.is_none() {
                    bail!(
                        "[[target]] #{} (app.name = {:?}) is missing `platform`. \
                         Set `platform = \"macos\"` or `platform = \"ios\"`.",
                        i + 1,
                        t.app.name
                    );
                }
            }
            target
        } else {
            let app = app.context(
                "strudel.toml must contain either [app] or one or more [[target]] sections",
            )?;
            vec![TargetSection {
                platform: None,
                app,
                build,
                extensions,
                dmg,
                ios: None,
            }]
        };

        let multi = targets.len() > 1;
        let resolved = targets
            .into_iter()
            .map(|t| resolve_target(t, &signing, &notarize, &ios, config_dir, global, multi))
            .collect::<Result<Vec<_>>>()?;

        Ok(ResolvedProject { targets: resolved })
    }

    #[cfg(test)]
    pub fn resolve(
        self,
        config_dir: &Path,
        global: Option<&GlobalConfig>,
    ) -> Result<ResolvedConfig> {
        Ok(self.resolve_project(config_dir, global)?.targets.remove(0))
    }
}

fn resolve_target(
    target: TargetSection,
    signing: &SigningSection,
    notarize: &NotarizeSection,
    top_ios: &IosSection,
    config_dir: &Path,
    global: &GlobalConfig,
    multi: bool,
) -> Result<ResolvedConfig> {
    let TargetSection {
        platform,
        app,
        build,
        extensions,
        dmg,
        ios,
    } = target;
    let ios = ios.unwrap_or_else(|| top_ios.clone());

    let source_dir = resolve_path(config_dir, build.source_dir, ".");
    let build_dir_default = if multi {
        match platform {
            Some(p) => format!(".build/dist/{}-{}", app.name, p.as_str()),
            None => format!(".build/dist/{}", app.name),
        }
    } else {
        ".build/dist".to_string()
    };
    let build_dir = resolve_path(&source_dir, build.build_dir, &build_dir_default);
    let target_name = build.target_name.unwrap_or_else(|| app.name.clone());
    let ios_simulator = ios.simulator.unwrap_or_else(|| "iPhone 16".to_string());
    let ios_device = ios.device;
    let ios_deployment_target = ios.deployment_target.unwrap_or_else(|| "18.0".to_string());
    let ios_assets_dir = ios.assets_dir.map(|p| resolve_to(config_dir, p));
    let ios_app_icon_name = ios.app_icon_name.unwrap_or_else(|| "AppIcon".to_string());

    let extensions = extensions
        .into_iter()
        .map(|ext| ext.resolve(config_dir))
        .collect::<Result<Vec<_>>>()?;

    let dmg = match dmg {
        None => Some(ResolvedDmg::default()),
        Some(d) => d.resolve(config_dir),
    };

    Ok(ResolvedConfig {
        platform,
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
        // Identifiers: env var > strudel.toml > global config.
        sign_identity: env_or_global(
            signing.identity.clone(),
            global.signing_identity.clone(),
            "APPLE_SIGNING_IDENTITY",
        ),
        team_id: env_or_global(
            signing.team_id.clone(),
            global.signing_team_id.clone(),
            "APPLE_TEAM_ID",
        ),
        apple_api_issuer: env_or_global(
            notarize.api_issuer.clone(),
            global.notarize_api_issuer.clone(),
            "APPLE_API_ISSUER",
        ),
        apple_api_key: env_or_global(
            notarize.api_key.clone(),
            global.notarize_api_key.clone(),
            "APPLE_API_KEY",
        ),
        // Like other input paths, resolved relative to the config file directory.
        // Global config path is already absolute (resolved at load time).
        apple_api_key_path: std::env::var("APPLE_API_KEY_PATH")
            .ok()
            .map(PathBuf::from)
            .or_else(|| notarize.api_key_path.clone())
            .map(|p| resolve_to(config_dir, p))
            .or_else(|| global.notarize_api_key_path.clone()),
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
            .map(|p| resolve_to(config_dir, p))
            .collect(),
        provisioning_profile: build
            .provisioning_profile
            .map(|p| resolve_to(config_dir, p)),
        resources_dir: build.resources_dir.map(|p| resolve_to(config_dir, p)),
        resources: build
            .resources
            .unwrap_or_default()
            .into_iter()
            .map(|p| resolve_to(config_dir, p))
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
        dmg,
    })
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
/// matching env var (`APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`); the env var
/// takes precedence when both are set.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SigningSection {
    pub identity: Option<String>,
    pub team_id: Option<String>,
}

/// `[notarize]` — non-secret notarization identifiers. Each may also come from
/// the matching env var (`APPLE_API_ISSUER`, `APPLE_API_KEY`,
/// `APPLE_API_KEY_PATH`); the env var takes precedence when both are set.
/// Secrets (`APPLE_CERTIFICATE*`) are read from the environment only.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NotarizeSection {
    pub api_issuer: Option<String>,
    pub api_key: Option<String>,
    pub api_key_path: Option<PathBuf>,
    pub timeout: Option<u64>,
}

/// `[dmg]` — DMG window customization for `strudel release`.
///
/// The styled Finder window (a generated `.DS_Store`, applied headlessly by the
/// `dmg` crate) is the default even when this section is absent. Add the
/// section to override individual fields or opt out with `plain = true`.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DmgSection {
    /// Set to `true` to skip the styled window and produce a plain compressed
    /// DMG directly (no Finder window configuration). Default: `false`.
    #[serde(default)]
    pub plain: bool,
    /// Path to a PNG or JPEG background image for the DMG Finder window.
    pub background: Option<PathBuf>,
    /// Finder window width in pixels. Default: 660.
    pub window_width: Option<u32>,
    /// Finder window height in pixels. Default: 400.
    pub window_height: Option<u32>,
    /// Icon size in pixels. Default: 128.
    pub icon_size: Option<u32>,
    /// Horizontal position of the .app icon. Default: 192.
    pub app_x: Option<u32>,
    /// Vertical position of the .app icon. Default: 192.
    pub app_y: Option<u32>,
    /// Horizontal position of the Applications symlink. Default: 468.
    pub applications_x: Option<u32>,
    /// Vertical position of the Applications symlink. Default: 192.
    pub applications_y: Option<u32>,
}

impl DmgSection {
    /// Returns `None` when `plain = true` (simple UDZO path); otherwise
    /// returns the resolved config with defaults filled in.
    fn resolve(self, config_dir: &Path) -> Option<ResolvedDmg> {
        if self.plain {
            return None;
        }
        Some(ResolvedDmg {
            background: self.background.map(|p| resolve_to(config_dir, p)),
            window_width: self.window_width.unwrap_or(660),
            window_height: self.window_height.unwrap_or(400),
            icon_size: self.icon_size.unwrap_or(128),
            app_x: self.app_x.unwrap_or(192),
            app_y: self.app_y.unwrap_or(192),
            applications_x: self.applications_x.unwrap_or(468),
            applications_y: self.applications_y.unwrap_or(192),
        })
    }
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
    pub issuer: Option<String>,
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

        # DMG window configuration for `strudel release`.
        # A styled Finder window (via a generated .DS_Store) is applied by default.
        # Uncomment [dmg] to override specific fields, or set plain = true for a
        # plain compressed DMG.
        # [dmg]
        # plain             = true                          # skip the styled window
        # background        = "assets/dmg-background.png"  # PNG/JPEG background image
        # window_width      = 660                           # default shown
        # window_height     = 400                           # default shown
        # icon_size         = 128                           # default shown
        # app_x             = 192                           # .app icon X position
        # app_y             = 192                           # .app icon Y position
        # applications_x    = 468                           # Applications symlink X position
        # applications_y    = 192                           # Applications symlink Y position
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
        let app = cfg.app.as_ref().unwrap();
        assert_eq!(app.name, "MyApp");
        assert_eq!(app.bundle_id, "com.example.myapp");
        assert_eq!(app.version, "1.2.3");
        assert_eq!(app.build_number, "42");
    }

    #[test]
    fn generated_toml_resolves_with_defaults() {
        // After parsing it must also resolve cleanly — i.e. every key the
        // template emits round-trips through resolve_config (no missing
        // required derived fields, no path resolution panics).
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.0", "1");
        let cfg: BuildConfig = toml::from_str(&t).unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert_eq!(r.app_name, "MyApp");
        assert_eq!(r.target_name, "MyApp"); // default = app.name
        assert_eq!(r.notarize_timeout, 600); // default
    }

    #[test]
    fn parses_full_nested_config() {
        let cfg = parse_build_config(FULL).expect("should parse");
        assert_eq!(cfg.app.as_ref().unwrap().name, "MyApp");
        assert_eq!(cfg.app.as_ref().unwrap().build_number, "42");
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
        let dmg = cfg.dmg.as_ref().expect("FULL fixture includes [dmg]");
        assert_eq!(dmg.window_width, Some(800));
        assert_eq!(dmg.icon_size, Some(100));
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
        .expect("[app] is sufficient for the single-target form");
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
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
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
        let dmg = r.dmg.expect("FULL fixture has [dmg]");
        assert_eq!(dmg.background, Some(PathBuf::from("/cfg/dmg-bg.png")));
    }

    #[test]
    fn tilde_paths_are_expanded() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
            [build]
            entitlements_json_path = "~/my/ent.json"
            [notarize]
            api_key_path = "~/my/AuthKey.p8"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ent = r.entitlements_json_path.unwrap();
        assert!(
            ent.is_absolute(),
            "~ in entitlements_json_path should expand"
        );
        assert!(ent.ends_with("my/ent.json"));
        let key = r.apple_api_key_path.unwrap();
        assert!(key.is_absolute(), "~ in api_key_path should expand");
        assert!(key.ends_with("my/AuthKey.p8"));
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
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
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
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
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
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
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
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
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
    fn dmg_absent_uses_defaults() {
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.0", "1");
        let cfg: BuildConfig = toml::from_str(&t).unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let dmg = r
            .dmg
            .expect("absent [dmg] section should use defaults, not None");
        assert_eq!(dmg.window_width, 660);
        assert_eq!(dmg.window_height, 400);
        assert_eq!(dmg.icon_size, 128);
        assert!(dmg.background.is_none());
    }

    #[test]
    fn dmg_plain_true_gives_none() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [dmg]
            plain = true
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert!(
            r.dmg.is_none(),
            "plain = true should skip the styled window path"
        );
    }

    #[test]
    fn dmg_section_parses_and_resolves() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [dmg]
            background = "assets/bg.png"
            window_width = 800
            window_height = 500
            icon_size = 100
            app_x = 200
            app_y = 200
            applications_x = 600
            applications_y = 200
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let dmg = r.dmg.expect("dmg should be Some");
        assert_eq!(dmg.background, Some(PathBuf::from("/cfg/assets/bg.png")));
        assert_eq!(dmg.window_width, 800);
        assert_eq!(dmg.window_height, 500);
        assert_eq!(dmg.icon_size, 100);
        assert_eq!(dmg.app_x, 200);
        assert_eq!(dmg.app_y, 200);
        assert_eq!(dmg.applications_x, 600);
        assert_eq!(dmg.applications_y, 200);
    }

    #[test]
    fn dmg_empty_section_uses_defaults() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [dmg]
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let dmg = r.dmg.expect("empty [dmg] section should use defaults");
        assert!(dmg.background.is_none());
        assert_eq!(dmg.window_width, 660);
        assert_eq!(dmg.window_height, 400);
        assert_eq!(dmg.icon_size, 128);
        assert_eq!(dmg.app_x, 192);
        assert_eq!(dmg.app_y, 192);
        assert_eq!(dmg.applications_x, 468);
        assert_eq!(dmg.applications_y, 192);
    }

    #[test]
    fn dmg_background_absolute_path_untouched() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [dmg]
            background = "/abs/bg.png"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let dmg = r.dmg.unwrap();
        assert_eq!(dmg.background, Some(PathBuf::from("/abs/bg.png")));
    }

    #[test]
    fn environment_wins_over_config_value() {
        // When both an env var and a config value are present, the env var takes
        // precedence. Use temp_env to avoid polluting other tests.
        temp_env::with_vars(
            [
                ("APPLE_SIGNING_IDENTITY", Some("env-identity")),
                ("APPLE_TEAM_ID", Some("env-team")),
            ],
            || {
                let cfg = parse_build_config(FULL).unwrap();
                let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
                assert_eq!(r.sign_identity, "env-identity");
                assert_eq!(r.team_id, "env-team");
            },
        );
    }

    #[test]
    fn config_value_used_when_no_env_var() {
        // When the env var is absent, the config file value is used.
        temp_env::with_vars(
            [
                ("APPLE_SIGNING_IDENTITY", None::<&str>),
                ("APPLE_TEAM_ID", None::<&str>),
            ],
            || {
                let cfg = parse_build_config(FULL).unwrap();
                let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
                assert_eq!(r.sign_identity, "Developer ID Application: Me (TEAM123456)");
                assert_eq!(r.team_id, "TEAM123456");
            },
        );
    }

    #[test]
    fn platform_deserializes_from_strings() {
        let cfg = parse_build_config(indoc! { r#"
            [[target]]
            platform = "macos"
            app.name = "A"
            app.bundle_id = "com.a"
            app.version = "1"
            app.build_number = "1"

            [[target]]
            platform = "ios"
            app.name = "B"
            app.bundle_id = "com.b"
            app.version = "1"
            app.build_number = "1"
        "#})
        .unwrap();
        assert_eq!(cfg.target[0].platform, Some(Platform::Macos));
        assert_eq!(cfg.target[1].platform, Some(Platform::Ios));
    }

    #[test]
    fn multi_target_parses_to_n_targets_with_platforms() {
        let project = parse_build_config(MULTI)
            .unwrap()
            .resolve_project(Path::new("/cfg"), None)
            .unwrap();
        assert_eq!(project.targets.len(), 2);
        assert_eq!(project.targets[0].platform, Some(Platform::Macos));
        assert_eq!(project.targets[0].app_name, "MyApp");
        assert_eq!(project.targets[1].platform, Some(Platform::Ios));
        assert_eq!(project.targets[1].app_name, "MyApp");
    }

    #[test]
    fn mixed_top_level_app_and_target_is_error() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [[target]]
            platform = "macos"
            app.name = "X"
            app.bundle_id = "y"
            app.version = "1"
            app.build_number = "1"
        "#})
        .unwrap()
        .resolve_project(Path::new("/cfg"), None);
        assert!(err.is_err(), "mixing [app] with [[target]] should fail");
        let msg = format!("{:#}", err.unwrap_err());
        assert!(msg.contains("cannot mix"), "got: {msg}");
    }

    #[test]
    fn target_without_platform_is_error() {
        let err = parse_build_config(indoc! { r#"
            [[target]]
            app.name = "A"
            app.bundle_id = "com.a"
            app.version = "1"
            app.build_number = "1"
        "#})
        .unwrap()
        .resolve_project(Path::new("/cfg"), None);
        assert!(err.is_err(), "[[target]] without platform should fail");
        let msg = format!("{:#}", err.unwrap_err());
        assert!(msg.contains("missing `platform`"), "got: {msg}");
    }

    #[test]
    fn neither_app_nor_target_is_error() {
        let err = parse_build_config(indoc! { r#"
            [signing]
            identity = "x"
        "#})
        .unwrap()
        .resolve_project(Path::new("/cfg"), None);
        assert!(
            err.is_err(),
            "config with neither [app] nor [[target]] should fail"
        );
    }

    #[test]
    fn per_target_ios_overrides_top_level() {
        let project = parse_build_config(indoc! { r#"
            [ios]
            simulator = "top-level-sim"
            deployment_target = "17.0"

            [[target]]
            platform = "ios"
            app.name = "A"
            app.bundle_id = "com.a"
            app.version = "1"
            app.build_number = "1"
            ios.deployment_target = "18.0"

            [[target]]
            platform = "ios"
            app.name = "B"
            app.bundle_id = "com.b"
            app.version = "1"
            app.build_number = "1"
        "#})
        .unwrap()
        .resolve_project(Path::new("/cfg"), None)
        .unwrap();
        // Target A has per-target ios override.
        assert_eq!(project.targets[0].ios_deployment_target, "18.0");
        // Target B inherits top-level ios.
        assert_eq!(project.targets[1].ios_simulator, "top-level-sim");
        assert_eq!(project.targets[1].ios_deployment_target, "17.0");
    }

    #[test]
    fn multi_build_dir_gets_platform_subdir() {
        let project = parse_build_config(MULTI)
            .unwrap()
            .resolve_project(Path::new("/cfg"), None)
            .unwrap();
        // Multi-target: each target gets .build/dist/<name>-<platform>.
        assert_eq!(
            project.targets[0].build_dir,
            PathBuf::from("/cfg/.build/dist/MyApp-macos")
        );
        assert_eq!(
            project.targets[1].build_dir,
            PathBuf::from("/cfg/.build/dist/MyApp-ios")
        );
    }

    #[test]
    fn single_target_keeps_default_build_dir() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert_eq!(r.build_dir, PathBuf::from("/cfg/.build/dist"));
    }
}
