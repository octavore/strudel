use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use indoc::formatdoc;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::{self, Deserializer};

use crate::config::build_target::{
    AppSection, BuildSection, BuildTarget, DmgSection, IosSection, TargetPlatform,
};
use crate::config::extension::ExtensionSection;
use crate::config::global::GlobalConfig;
use crate::config::resolved::{
    ResolvedCopy, ResolvedDmg, ResolvedIosSection, ResolvedMacOsSection, ResolvedProject,
};
use crate::config::utils::{env_or_global, resolve_path, resolve_to};
use crate::config::{IosProvisioningBackend, ResolvedConfig};

/// The on-disk `strudel.toml`. Single-target configs use the flat form
/// (`[app]`, `[build]`, `[dmg]`) and are always macOS; multiple or non-macOS
/// targets use `[[target]]`, each tagged with its own `platform`.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum BuildConfig {
    Single(SingleBuildConfig),
    Multi(MultiBuildConfig),
}

// Custom deserializer so we can provide better error messages.
impl<'de> Deserialize<'de> for BuildConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = toml::Value::deserialize(deserializer)?;
        let is_multi = value.as_table().is_some_and(|t| t.contains_key("target"));
        if is_multi {
            MultiBuildConfig::deserialize(value)
                .map(BuildConfig::Multi)
                .map_err(de::Error::custom)
        } else {
            SingleBuildConfig::deserialize(value)
                .map(BuildConfig::Single)
                .map_err(de::Error::custom)
        }
    }
}

/// Flat, single-target form; only macOS. Use `[[target]]` for iOS or for
/// more than one target.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SingleBuildConfig {
    pub app: AppSection,

    #[serde(default)]
    pub build: BuildSection,

    #[serde(default)]
    pub extensions: Vec<ExtensionSection>,

    pub dmg: Option<DmgSection>,

    /// Path to a `.xcassets` directory to compile into
    /// `Contents/Resources/Assets.car` with `xcrun actool`.
    pub assets_dir: Option<PathBuf>,

    #[serde(default)]
    pub apple: AppleSection,
}

/// Multi-target form: one `[[target]]` per app/platform pair.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MultiBuildConfig {
    pub target: Vec<BuildTarget>,

    /// Shared iOS defaults inherited by targets that don't set their own
    /// `[ios]` field; per-target fields take precedence over this.
    pub ios: Option<IosSection>,

    #[serde(default)]
    pub apple: AppleSection,
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

        let targets = match self {
            BuildConfig::Single(single) => {
                let SingleBuildConfig {
                    app,
                    build,
                    extensions,
                    dmg,
                    assets_dir,
                    apple,
                } = single;
                let target = BuildTarget {
                    app,
                    build,
                    extensions,
                    platform: TargetPlatform::Macos { dmg, assets_dir },
                };
                vec![resolve_target(
                    target, &apple, None, true, config_dir, global,
                )?]
            },
            BuildConfig::Multi(multi) => {
                let MultiBuildConfig { target, ios, apple } = multi;
                target
                    .into_iter()
                    .map(|t| resolve_target(t, &apple, ios.as_ref(), false, config_dir, global))
                    .collect::<Result<Vec<_>>>()?
            },
        };

        ensure_unique_target_ids(&targets)?;
        Ok(ResolvedProject { targets })
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

/// Target ids are derived as `{platform}/{app.name}`, so two `[[target]]`
/// blocks sharing a platform must not share an app name. Such targets would be
/// indistinguishable to `--target` and would resolve to the same default build
/// directory, silently clobbering each other's artifacts.
fn ensure_unique_target_ids(targets: &[ResolvedConfig]) -> Result<()> {
    let mut seen = HashSet::new();
    for target in targets {
        if !seen.insert(target.target_id.as_str()) {
            bail!(
                "Duplicate target {:?}: two [[target]] blocks share a platform and an \
                 app.name. Give them distinct app.name values.",
                target.target_id
            );
        }
    }
    Ok(())
}

/// `APPLE_SIGNING_IDENTITY` and `APPLE_CERTIFICATE` are mutually exclusive:
/// when a certificate is supplied, strudel imports it into a temporary
/// keychain and derives the signing identity from it, so an explicit
/// identity env var alongside it is an ambiguous, likely-accidental input.
fn reject_conflicting_signing_inputs(
    has_certificate: bool,
    identity_env_is_set: bool,
) -> Result<()> {
    if has_certificate && identity_env_is_set {
        bail!(
            "APPLE_SIGNING_IDENTITY and APPLE_CERTIFICATE are mutually exclusive: when \
             APPLE_CERTIFICATE is set, strudel imports it into a temporary keychain and \
             derives the signing identity from it automatically. Unset APPLE_SIGNING_IDENTITY \
             to sign with the imported certificate, or unset APPLE_CERTIFICATE to sign with an \
             identity already present in your keychain."
        );
    }
    Ok(())
}

fn resolve_target(
    target: BuildTarget,
    apple: &AppleSection,
    shared_ios: Option<&IosSection>,
    is_single: bool,
    config_dir: &Path,
    global: &GlobalConfig,
) -> Result<ResolvedConfig> {
    let platform = target.platform();
    let BuildTarget {
        app,
        build,
        extensions,
        ..
    } = target;

    let target_id = format!("{}/{}", platform.as_str(), app.name);
    let source_dir = resolve_path(config_dir, build.source_dir.unwrap_or(".".into()));
    // The single-target form has exactly one target, so its build dir doesn't
    // need the target id to stay unique.
    let build_dir_default = if is_single {
        ".build/dist".to_string()
    } else {
        format!(".build/dist/{target_id}")
    };
    let build_dir = resolve_path(
        &source_dir,
        build.build_dir.unwrap_or(build_dir_default.into()),
    );

    let target_name = build.target_name.unwrap_or_else(|| app.name.clone());
    let target_platform = match target.platform {
        TargetPlatform::Macos { dmg, assets_dir } => ResolvedMacOsSection {
            dmg: match dmg {
                Some(d) => d.resolve(config_dir)?,
                None => Some(ResolvedDmg::default()),
            },
            assets_dir: assets_dir.map(|p| resolve_to(config_dir, p)),
        }
        .into(),
        TargetPlatform::Ios { ios } => ResolvedIosSection {
            simulator: ios
                .simulator
                .or_else(|| shared_ios.and_then(|s| s.simulator.clone()))
                .unwrap_or_else(|| "iPhone 16".to_string()),
            device: ios
                .device
                .or_else(|| shared_ios.and_then(|s| s.device.clone())),
            deployment_target: ios
                .deployment_target
                .or_else(|| shared_ios.and_then(|s| s.deployment_target.clone()))
                .unwrap_or_else(|| "18.0".to_string()),
            assets_dir: ios
                .assets_dir
                .or_else(|| shared_ios.and_then(|s| s.assets_dir.clone()))
                .map(|p| resolve_to(config_dir, p)),
            app_icon_name: ios
                .app_icon_name
                .or_else(|| shared_ios.and_then(|s| s.app_icon_name.clone()))
                .unwrap_or_else(|| "AppIcon".to_string()),
            provisioning: ios
                .provisioning
                .or_else(|| shared_ios.and_then(|s| s.provisioning.clone()))
                .ok_or_else(|| {
                    anyhow!("[ios] `provisioning` must be \"free\" or \"app_store_connect\"")
                })?,
            apple_id: ios
                .apple_id
                .or_else(|| shared_ios.and_then(|s| s.apple_id.clone())),
        }
        .into(),
    };

    let extensions = extensions
        .into_iter()
        .map(|ext| ext.resolve(config_dir))
        .collect::<Result<Vec<_>>>()?;

    // Secrets: environment only. These are never deserialized from the file.
    let apple_certificate: SecretString = std::env::var("APPLE_CERTIFICATE")
        .unwrap_or_default()
        .into();
    let apple_certificate_password: SecretString = std::env::var("APPLE_CERTIFICATE_PASSWORD")
        .unwrap_or_default()
        .into();
    let has_certificate = !apple_certificate.expose_secret().is_empty();

    // Only an *explicit* env var conflicts with APPLE_CERTIFICATE - a
    // project/global `identity` default is meant for local builds without a
    // certificate, and is silently overridden (not fought over) once a
    // certificate is supplied.
    reject_conflicting_signing_inputs(
        has_certificate,
        std::env::var("APPLE_SIGNING_IDENTITY").is_ok(),
    )?;

    // Identifiers: env var > strudel.toml > global config. A certificate
    // (env-only) always wins over any project/global identity default: the
    // real identity is derived from the imported certificate at build time.
    let sign_identity = if has_certificate {
        String::new()
    } else {
        env_or_global(
            apple.identity.clone(),
            global.signing_identity.clone(),
            "APPLE_SIGNING_IDENTITY",
        )
    };

    Ok(ResolvedConfig {
        platform: Some(platform),
        target_id,
        // User-supplied input paths are resolved relative to the config file's
        // directory (the one fixed anchor the user reasons about), independent of
        // `source_dir`. info_json_path and icon are optional with no default.
        info_json_path: build.info_json_path.map(|p| resolve_path(config_dir, p)),
        entitlements_json_path: build
            .entitlements_json_path
            .map(|p| resolve_path(config_dir, p)),
        icon: build.icon.map(|icon| icon.resolve(config_dir)),
        archs: build.archs.unwrap_or_else(|| {
            let arch = match std::env::consts::ARCH {
                "aarch64" => "arm64",
                other => other,
            };
            vec![arch.to_string()]
        }),
        sign_identity,
        team_id: env_or_global(
            apple.team_id.clone(),
            global.signing_team_id.clone(),
            "APPLE_TEAM_ID",
        ),
        apple_api_issuer: env_or_global(
            apple.api_issuer.clone(),
            global.notarize_api_issuer.clone(),
            "APPLE_API_ISSUER",
        ),
        apple_api_key: env_or_global(
            apple.api_key.clone(),
            global.notarize_api_key.clone(),
            "APPLE_API_KEY",
        ),
        // Like other input paths, resolved relative to the config file directory.
        // Global config path is already absolute (resolved at load time).
        apple_api_key_path: std::env::var("APPLE_API_KEY_PATH")
            .ok()
            .map(PathBuf::from)
            .or_else(|| apple.api_key_path.clone())
            .map(|p| resolve_to(config_dir, p))
            .or_else(|| global.notarize_api_key_path.clone()),
        apple_certificate,
        apple_certificate_password,
        notarize_timeout: apple.notarize_timeout.unwrap_or(600),
        build_env: build.build_env.unwrap_or_default(),
        // Resolves like any other path (relative to the config file's
        // directory), whether bare or not. If the resolved location doesn't
        // exist, `BuilderCore::embed_libraries` falls back to the current
        // build's `.build/<triple>/release/` output dir at build time - so a
        // bare name still works across build destinations (e.g. simulator to
        // device) without listing a path.
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
        copy: build
            .copy
            .unwrap_or_default()
            .into_iter()
            .map(|c| ResolvedCopy {
                src: resolve_to(config_dir, c.src),
                dest_dir: c.dest_dir,
                sign: c.sign,
                entitlements_json_path: c
                    .entitlements_json_path
                    .map(|p| resolve_path(config_dir, p)),
            })
            .collect(),
        app_name: app.name,
        bundle_id: app.bundle_id,
        version: app.version,
        build_number: app.build_number.unwrap_or_else(|| "1".to_string()),
        source_dir,
        build_dir,
        target_name,
        extensions,
        target_platform,
    })
}

/// `[apple]` Non-secret Apple developer identifiers, shared by signing,
/// notarization, and provisioning-profile management (the App Store Connect
/// API key authenticates all three). Each may also come from the matching
/// env var (`APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`, `APPLE_API_ISSUER`,
/// `APPLE_API_KEY`, `APPLE_API_KEY_PATH`); the env var takes precedence when
/// both are set. Secrets (`APPLE_CERTIFICATE*`) are read from the
/// environment only.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AppleSection {
    pub identity: Option<String>,
    pub team_id: Option<String>,
    pub api_issuer: Option<String>,
    pub api_key: Option<String>,
    pub api_key_path: Option<PathBuf>,
    pub notarize_timeout: Option<u64>,
}

/// App Store Connect API key credentials for `notarytool`.
#[derive(Debug, Clone)]
pub struct NotaryAuth {
    pub key_path: PathBuf,
    pub key_id: String,
    pub issuer: Option<String>,
}

// todo: optionally generate ios/ios+macos configs
pub fn generate_initial_toml(app_name: &str, bundle_id: &str, version: &str) -> String {
    formatdoc! {r##"
        # strudel build configuration
        # See `strudel help config` for the full list of options.

        [app]
        name         = "{app_name}"
        bundle_id    = "{bundle_id}"
        version      = "{version}"
        # build_number = "1"  # optional, defaults to "1"

        # Paths are relative to this file's directory unless absolute.
        # [build]
        # source_dir             = "."                  # default: current dir
        # entitlements_json_path = "entitlements.json"  # default: none
        # Bundle icon; either a png or icns file copied in unmodified (set icon.path),
        # or generate an icon from a png/svg at build time:
        # icon.src               = "art.png"
        # icon.scale             = 1.2         # optional
        # icon.background        = "#fefefe" # optional; hex, defaults to white

        # Apple developer identifiers, used for signing and notarization.
        # Can also be set via env vars.
        # [apple]
        # identity     = "Developer ID Application: Your Name (XXXXXXXXXX)"
        # team_id      = "XXXXXXXXXX"
        # api_key      = "2X9R4HXF34"
        # api_key_path = "AuthKey_2X9R4HXF34.p8"

        # A DMG with a styled Finder window is created by default for `strudel release`.
        # See help for customizing the DMG window.
        # Set plain = true for a plain unstyled DMG instead.
        # [dmg]
        # plain = true
    "##}
}

pub fn generate_initial_toml_with_ios(
    app_name: &str,
    bundle_id: &str,
    version: &str,
    include_macos: bool,
    ios_provisioning: IosProvisioningBackend,
) -> String {
    let provisioning = match ios_provisioning {
        IosProvisioningBackend::Free => "free",
        IosProvisioningBackend::AppStoreConnect => "app_store_connect",
    };
    let macos_target = if include_macos {
        formatdoc! {r#"

            [[target]]
            platform         = "macos"
            app.name         = "{app_name}"
            app.bundle_id    = "{bundle_id}"
            app.version      = "{version}"
            # app.build_number = "1"  # optional, defaults to "1"

            # build.entitlements_json_path = "entitlements.json"
            # build.icon.src               = "art.png"  # or build.icon.path = "AppIcon.icns"
            # dmg.plain                    = true
        "#}
    } else {
        String::new()
    };

    formatdoc! {r#"
        # strudel build configuration
        # See `strudel help config` for the full list of options.

        # Apple developer identifiers, used for signing and notarization.
        # Can also be set via env vars.
        # [apple]
        # identity     = "Developer ID Application: Your Name (XXXXXXXXXX)"
        # team_id      = "XXXXXXXXXX"
        # api_key      = "2X9R4HXF34"
        # api_key_path = "AuthKey_2X9R4HXF34.p8"

        [ios]
        # Provisioning backend - required for device builds. Choose one:
        #   "app_store_connect"  paid account + App Store Connect API key; 1-year profiles
        #   "free"               any Apple ID; 7-day profiles, no paid account needed
        provisioning = "{provisioning}"

        {macos_target}
        [[target]]
        platform         = "ios"
        app.name         = "{app_name}"
        app.bundle_id    = "{bundle_id}"
        app.version      = "{version}"
        # app.build_number = "1"  # optional, defaults to "1"

        # ios.simulator          = "iPhone 16"
        # ios.deployment_target  = "18.0"
        # ios.assets_dir         = "Sources/App/Assets.xcassets"
    "#}
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::path::Path;

    use dmg::DmgBackground;
    use indoc::indoc;

    use super::*;
    use crate::config::fixtures::*;
    use crate::config::resolved::{ResolvedIcon, ResolvedTargetPlatform};
    use crate::config::{BuildConfig, Platform};

    #[test]
    fn generated_toml_parses_and_resolves() {
        // The scaffolded file must be valid input to the config loader, and it
        // must resolve cleanly: otherwise `strudel init` produces a file that
        // `strudel build` rejects. Resolving subsumes parsing, so one test
        // covers both halves.
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.2.3");
        let cfg: BuildConfig = toml::from_str(&t).expect("scaffolded TOML must parse");
        let BuildConfig::Single(single) = &cfg else {
            panic!("scaffolded TOML should parse as a single-target config");
        };
        assert_eq!(single.app.bundle_id, "com.example.myapp");
        assert_eq!(single.app.build_number, None);

        let r = cfg
            .resolve(Path::new("/cfg"), None)
            .expect("scaffolded TOML must resolve");
        assert_eq!(r.app_name, "MyApp");
        assert_eq!(r.version, "1.2.3");
        assert_eq!(r.build_number, "1");
    }

    #[test]
    fn generated_macos_ios_toml_resolves_both_targets() {
        let t = generate_initial_toml_with_ios(
            "MyApp",
            "com.example.myapp",
            "1.0",
            true,
            IosProvisioningBackend::AppStoreConnect,
        );
        let cfg: BuildConfig = toml::from_str(&t).expect("scaffolded TOML must parse");
        let project = cfg
            .resolve_project(Path::new("/cfg"), None)
            .expect("scaffolded multi-target TOML must resolve");
        assert_eq!(project.targets.len(), 2);

        assert_matches!(
            project.targets[0].target_platform,
            ResolvedTargetPlatform::Mac(_)
        );
        assert_matches!(
            project.targets[1].target_platform,
            ResolvedTargetPlatform::Ios(_)
        );
    }

    #[test]
    fn generated_ios_only_toml_resolves_single_target() {
        let t = generate_initial_toml_with_ios(
            "MyApp",
            "com.example.myapp",
            "1.0",
            false,
            IosProvisioningBackend::Free,
        );
        let cfg: BuildConfig = toml::from_str(&t).expect("scaffolded TOML must parse");
        let project = cfg
            .resolve_project(Path::new("/cfg"), None)
            .expect("scaffolded iOS-only TOML must resolve");
        assert_eq!(project.targets.len(), 1);
        assert_matches!(
            project.targets[0].target_platform,
            ResolvedTargetPlatform::Ios(_)
        );
    }

    #[test]
    fn parses_full_nested_config() {
        let cfg = parse_build_config(FULL).expect("should parse");
        let BuildConfig::Single(single) = cfg else {
            panic!("FULL fixture should parse as a single-target config");
        };
        assert_eq!(single.app.name, "MyApp");
        assert_eq!(single.app.build_number.as_deref(), Some("42"));
        assert_eq!(
            single.build.archs.as_deref(),
            Some(&["arm64".into(), "x86_64".into()][..])
        );
        assert_eq!(
            single.apple.identity.as_deref(),
            Some("Developer ID Application: Me (TEAM123456)")
        );
        assert_eq!(single.apple.api_key.as_deref(), Some("KEYID123"));
        assert_eq!(single.apple.notarize_timeout, Some(1200));
        assert!(single.dmg.is_some(), "FULL fixture includes [dmg]");
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
        let BuildConfig::Single(single) = cfg else {
            panic!("expected single-target config");
        };
        assert!(single.build.source_dir.is_none());
        assert!(single.apple.identity.is_none());
        assert!(single.apple.notarize_timeout.is_none());
    }

    #[test]
    fn build_env_nests_under_build() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"

            [build.build_env]
            PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert_eq!(
            r.build_env.get("PKG_CONFIG_PATH").map(String::as_str),
            Some("/opt/homebrew/lib/pkgconfig")
        );
    }

    #[test]
    fn embed_libs_entries_resolve_relative_to_config_dir_like_any_other_path() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"

            [build]
            embed_libs = ["libFoo.dylib", "Sparkle.framework", "vendor/libBar.dylib"]
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert_eq!(
            r.embed_libs,
            vec![
                PathBuf::from("/cfg/libFoo.dylib"),
                PathBuf::from("/cfg/Sparkle.framework"),
                PathBuf::from("/cfg/vendor/libBar.dylib"),
            ]
        );
    }

    #[test]
    fn missing_required_app_field_is_error() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
        "#});
        assert!(err.is_err(), "missing version should fail");
    }

    #[test]
    fn missing_build_number_defaults_to_one() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
        "#})
        .expect("build_number should default to \"1\"");
        let BuildConfig::Single(single) = &cfg else {
            panic!("expected single-target config");
        };
        assert_eq!(single.app.build_number, None);

        let resolved = cfg
            .resolve(Path::new("/cfg"), None)
            .expect("should resolve with defaulted build_number");
        assert_eq!(resolved.build_number, "1");
    }

    #[test]
    fn unknown_key_in_section_is_rejected() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [apple]
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
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("FULL fixture is a macOS target");
        };
        let dmg = macos.dmg.as_ref().expect("FULL fixture has [dmg]");
        assert_eq!(
            dmg.background,
            DmgBackground::Path(PathBuf::from("/cfg/dmg-bg.png"))
        );
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

            [apple]
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

            [dmg]
            background = "/abs/bg.png"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        assert_eq!(
            r.entitlements_json_path,
            Some(PathBuf::from("/abs/ent.json"))
        );
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        let dmg = macos.dmg.as_ref().expect("[dmg] section is present");
        assert_eq!(
            dmg.background,
            DmgBackground::Path(PathBuf::from("/abs/bg.png"))
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
        assert!(r.icon.is_none());
    }

    #[test]
    fn ios_defaults_when_absent() {
        let cfg = parse_build_config(indoc! { r#"
            [[target]]
            platform = "ios"
            app.name = "MyApp"
            app.bundle_id = "com.example.myapp"
            app.version = "1.0"
            app.build_number = "1"
            ios.provisioning = "app_store_connect"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ResolvedTargetPlatform::Ios(ios) = &r.target_platform else {
            panic!("expected an iOS target");
        };
        assert_eq!(ios.simulator, "iPhone 16");
        assert_eq!(ios.deployment_target, "18.0");
        assert_eq!(ios.app_icon_name, "AppIcon");
        assert!(ios.device.is_none());
        assert!(ios.assets_dir.is_none());
    }

    #[test]
    fn ios_section_overrides() {
        let cfg = parse_build_config(indoc! { r#"
            [[target]]
            platform = "ios"
            app.name = "MyApp"
            app.bundle_id = "com.example.myapp"
            app.version = "1.0.0"
            app.build_number = "1"
            ios.simulator = "iPhone 15 Pro"
            ios.device = "00000000-0000-0000-0000-000000000000"
            ios.deployment_target = "17.0"
            ios.assets_dir = "Sources/Assets.xcassets"
            ios.app_icon_name = "MyIcon"
            ios.provisioning = "app_store_connect"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ResolvedTargetPlatform::Ios(ios) = &r.target_platform else {
            panic!("expected an iOS target");
        };
        assert_eq!(ios.simulator, "iPhone 15 Pro");
        assert_eq!(ios.deployment_target, "17.0");
        assert_eq!(ios.app_icon_name, "MyIcon");
        assert_eq!(
            ios.device.as_deref(),
            Some("00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(
            ios.assets_dir,
            Some(PathBuf::from("/cfg/Sources/Assets.xcassets"))
        );
    }

    #[test]
    fn dmg_absent_uses_defaults() {
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.0");
        let cfg: BuildConfig = toml::from_str(&t).unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        let dmg = macos
            .dmg
            .as_ref()
            .expect("absent [dmg] section should use defaults, not None");
        assert_eq!(dmg.window_width, 660);
        assert_eq!(dmg.window_height, 400);
        assert_eq!(dmg.icon_size, 128);
        assert_eq!(dmg.background, DmgBackground::Color(255, 255, 255));
    }

    #[test]
    fn macos_assets_dir_absent_is_none() {
        let t = generate_initial_toml("MyApp", "com.example.myapp", "1.0");
        let cfg: BuildConfig = toml::from_str(&t).unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        assert!(macos.assets_dir.is_none());
    }

    #[test]
    fn macos_assets_dir_resolves_relative_to_config_dir() {
        let cfg = parse_build_config(indoc! { r#"
            assets_dir = "Sources/Assets.xcassets"

            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        assert_eq!(
            macos.assets_dir,
            Some(PathBuf::from("/cfg/Sources/Assets.xcassets"))
        );
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
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        assert!(
            macos.dmg.is_none(),
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
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        let dmg = macos.dmg.as_ref().expect("dmg should be Some");
        assert_eq!(
            dmg.background,
            DmgBackground::Path(PathBuf::from("/cfg/assets/bg.png"))
        );
        assert_eq!(dmg.window_width, 800);
        assert_eq!(dmg.window_height, 500);
        assert_eq!(dmg.icon_size, 100);
        assert_eq!(dmg.app_x, 200);
        assert_eq!(dmg.app_y, 200);
        assert_eq!(dmg.applications_x, 600);
        assert_eq!(dmg.applications_y, 200);
    }

    #[test]
    fn dmg_background_hex_color_resolves_to_color_variant() {
        let cfg = parse_build_config(indoc! { r##"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [dmg]
            background = "#fefefe"
        "##})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        let dmg = macos.dmg.as_ref().expect("dmg should be Some");
        assert_eq!(dmg.background, DmgBackground::Color(0xfe, 0xfe, 0xfe));
    }

    #[test]
    fn dmg_background_invalid_hex_color_is_rejected() {
        let cfg = parse_build_config(indoc! { r##"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [dmg]
            background = "#zzzzzz"
        "##})
        .unwrap();
        assert!(cfg.resolve(Path::new("/cfg"), None).is_err());
    }

    #[test]
    fn dmg_background_absent_resolves_to_white() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "MyApp"
            bundle_id = "com.example.myapp"
            version = "1.0.0"
            build_number = "1"

            [dmg]
            window_width = 800
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        let dmg = macos.dmg.as_ref().expect("dmg should be Some");
        // An unset background must still resolve to an explicit color (not
        // `DmgBackground::None`) so the icvp record stays complete - an
        // incomplete record can make Finder discard the whole view-options
        // blob, including iconSize.
        assert_eq!(dmg.background, DmgBackground::Color(255, 255, 255));
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
        let ResolvedTargetPlatform::Mac(macos) = &r.target_platform else {
            panic!("expected a macOS target");
        };
        let dmg = macos
            .dmg
            .as_ref()
            .expect("empty [dmg] section should use defaults");
        assert_eq!(dmg.background, DmgBackground::Color(255, 255, 255));
        assert_eq!(dmg.window_width, 660);
        assert_eq!(dmg.window_height, 400);
        assert_eq!(dmg.icon_size, 128);
        assert_eq!(dmg.app_x, 192);
        assert_eq!(dmg.app_y, 192);
        assert_eq!(dmg.applications_x, 468);
        assert_eq!(dmg.applications_y, 192);
    }

    /// A single-target config with an empty `[apple]` section, so every
    /// identifier has to come from the environment or the global config.
    const NO_APPLE_SECTION: &str = indoc! { r#"
        [app]
        name = "X"
        bundle_id = "y"
        version = "1"
        build_number = "1"
    "#};

    #[test]
    fn environment_wins_over_project_and_global() {
        // Env beats both lower layers, even when all three are populated.
        // temp_env keeps the vars from leaking into other tests.
        temp_env::with_vars(
            [
                ("APPLE_SIGNING_IDENTITY", Some("env-identity")),
                ("APPLE_TEAM_ID", Some("env-team")),
                ("APPLE_API_ISSUER", Some("env-issuer")),
                ("APPLE_API_KEY", Some("env-key")),
                ("APPLE_API_KEY_PATH", Some("/env/AuthKey.p8")),
            ],
            || {
                let cfg = parse_build_config(FULL).unwrap();
                let r = cfg
                    .resolve(Path::new("/cfg"), Some(&global_config()))
                    .unwrap();
                assert_eq!(r.sign_identity, "env-identity");
                assert_eq!(r.team_id, "env-team");
                assert_eq!(r.apple_api_issuer, "env-issuer");
                assert_eq!(r.apple_api_key, "env-key");
                assert_eq!(r.apple_api_key_path, Some(PathBuf::from("/env/AuthKey.p8")));
            },
        );
    }

    // Exercises the pure conflict check directly rather than through
    // `resolve()` + real env vars: ~40 other tests in this module call
    // `resolve()` unguarded (no `temp_env`), and process env vars are
    // process-global, not thread-local - setting `APPLE_SIGNING_IDENTITY`
    // and `APPLE_CERTIFICATE` for real would leak into those tests and
    // intermittently fail them, since a `bail!` (unlike a silently-wrong
    // assertion) is a hard failure however briefly the window overlaps.
    #[test]
    fn rejects_identity_env_var_alongside_certificate() {
        let err = reject_conflicting_signing_inputs(true, true)
            .expect_err("identity env var + certificate should be rejected");
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn non_conflicting_signing_inputs_are_fine() {
        reject_conflicting_signing_inputs(true, false).unwrap();
        reject_conflicting_signing_inputs(false, true).unwrap();
        reject_conflicting_signing_inputs(false, false).unwrap();
    }

    #[test]
    fn project_value_wins_over_global() {
        // FULL populates every [apple] key, so nothing should fall through to
        // the global config.
        temp_env::with_vars(APPLE_ENV_UNSET, || {
            let cfg = parse_build_config(FULL).unwrap();
            let r = cfg
                .resolve(Path::new("/cfg"), Some(&global_config()))
                .unwrap();
            assert_eq!(r.sign_identity, "Developer ID Application: Me (TEAM123456)");
            assert_eq!(r.team_id, "TEAM123456");
            assert_eq!(r.apple_api_issuer, "issuer-uuid");
            assert_eq!(r.apple_api_key, "KEYID123");
            // A project-relative key path anchors on the config dir, and must not
            // be shadowed by the global config's absolute path.
            assert_eq!(r.apple_api_key_path, Some(PathBuf::from("/cfg/AuthKey.p8")));
        });
    }

    #[test]
    fn global_fills_in_values_the_project_omits() {
        temp_env::with_vars(APPLE_ENV_UNSET, || {
            let cfg = parse_build_config(NO_APPLE_SECTION).unwrap();
            let r = cfg
                .resolve(Path::new("/cfg"), Some(&global_config()))
                .unwrap();
            assert_eq!(r.sign_identity, "global-identity");
            assert_eq!(r.team_id, "global-team");
            assert_eq!(r.apple_api_issuer, "global-issuer");
            assert_eq!(r.apple_api_key, "global-key");
            // The global path was made absolute at load time, so it is used
            // as-is rather than joined onto the project's config dir.
            assert_eq!(
                r.apple_api_key_path,
                Some(PathBuf::from("/global/AuthKey.p8"))
            );
        });
    }

    #[test]
    fn absent_everywhere_leaves_identifiers_empty() {
        // No env, no [apple] section, no global config: resolve must still
        // succeed. Missing credentials are reported later by preflight, not here.
        temp_env::with_vars(APPLE_ENV_UNSET, || {
            let cfg = parse_build_config(NO_APPLE_SECTION).unwrap();
            let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
            assert_eq!(r.sign_identity, "");
            assert_eq!(r.team_id, "");
            assert_eq!(r.apple_api_issuer, "");
            assert_eq!(r.apple_api_key, "");
            assert_eq!(r.apple_api_key_path, None);
        });
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
            ios.provisioning = "app_store_connect"
        "#})
        .unwrap();
        let BuildConfig::Multi(multi) = cfg else {
            panic!("[[target]] should parse as a multi-target config");
        };
        assert_matches!(multi.target[0].platform, TargetPlatform::Macos { .. });
        assert_matches!(multi.target[1].platform, TargetPlatform::Ios { .. });
    }

    #[test]
    fn multi_target_parses_to_n_targets_with_platforms() {
        // MULTI shares one app name across two platforms, so this also covers
        // the rule that the platform segment keeps such target ids distinct.
        let project = parse_build_config(MULTI)
            .unwrap()
            .resolve_project(Path::new("/cfg"), None)
            .unwrap();
        assert_eq!(project.targets.len(), 2);
        assert_eq!(project.targets[0].platform, Some(Platform::Macos));
        assert_eq!(project.targets[0].app_name, "MyApp");
        assert_eq!(project.targets[0].target_id, "macos/MyApp");
        assert_eq!(project.targets[1].platform, Some(Platform::Ios));
        assert_eq!(project.targets[1].app_name, "MyApp");
        assert_eq!(project.targets[1].target_id, "ios/MyApp");
    }

    #[test]
    fn same_platform_and_app_name_is_a_duplicate_target() {
        // Two iOS targets sharing an app name (e.g. an iPhone and an iPad app)
        // would collide on both --target and the default build directory.
        let err = parse_build_config(indoc! { r#"
            [[target]]
            platform = "ios"
            app.name = "MyApp"
            app.bundle_id = "com.example.myapp.phone"
            app.version = "1"
            app.build_number = "1"
            ios.provisioning = "free"

            [[target]]
            platform = "ios"
            app.name = "MyApp"
            app.bundle_id = "com.example.myapp.pad"
            app.version = "1"
            app.build_number = "1"
            ios.provisioning = "free"
        "#})
        .unwrap()
        .resolve_project(Path::new("/cfg"), None)
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Duplicate target \"ios/MyApp\""), "got: {msg}");
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
        "#});
        // Neither the single-target form (extra `target` key) nor the
        // multi-target form (extra `app` key) accepts this, so it's rejected
        // at parse time.
        assert!(err.is_err(), "mixing [app] with [[target]] should fail");
    }

    #[test]
    fn target_without_platform_is_error() {
        // platform is the serde tag on TargetPlatform.
        let err = parse_build_config(indoc! { r#"
            [[target]]
            app.name = "A"
            app.bundle_id = "com.a"
            app.version = "1"
            app.build_number = "1"
        "#});
        assert!(
            err.is_err(),
            "[[target]] without platform should fail to parse"
        );
    }

    #[test]
    fn neither_app_nor_target_is_error() {
        let err = parse_build_config(indoc! { r#"
            [apple]
            identity = "x"
        "#});
        assert!(
            err.is_err(),
            "config with neither [app] nor [[target]] should fail to parse"
        );
    }

    #[test]
    fn per_target_ios_overrides_top_level() {
        let project = parse_build_config(indoc! { r#"
            [ios]
            simulator = "top-level-sim"
            deployment_target = "17.0"
            provisioning = "app_store_connect"

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

        // Target A has a per-target ios override.
        let ResolvedTargetPlatform::Ios(ios_a) = &project.targets[0].target_platform else {
            panic!("expected an iOS target");
        };
        assert_eq!(ios_a.deployment_target, "18.0");

        // Target B inherits the top-level [ios] section.
        let ResolvedTargetPlatform::Ios(ios_b) = &project.targets[1].target_platform else {
            panic!("expected an iOS target");
        };
        assert_eq!(ios_b.simulator, "top-level-sim");
        assert_eq!(ios_b.deployment_target, "17.0");
    }

    #[test]
    fn multi_build_dir_gets_platform_subdir() {
        let project = parse_build_config(MULTI)
            .unwrap()
            .resolve_project(Path::new("/cfg"), None)
            .unwrap();
        // Multi-target: each target gets .build/dist/<target-id>.
        assert_eq!(
            project.targets[0].build_dir,
            PathBuf::from("/cfg/.build/dist/macos/MyApp")
        );
        assert_eq!(
            project.targets[1].build_dir,
            PathBuf::from("/cfg/.build/dist/ios/MyApp")
        );
    }

    #[test]
    fn icon_path_form_resolves_to_path_variant() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [build]
            icon.path = "AppIcon.icns"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        match r.icon {
            Some(ResolvedIcon::Path { path, icns }) => {
                assert_eq!(path, PathBuf::from("/cfg/AppIcon.icns"));
                assert!(!icns, "icns conversion should default to false");
            },
            other => panic!("expected ResolvedIcon::Path, got {other:?}"),
        }
    }

    #[test]
    fn icon_path_form_can_opt_into_icns_conversion() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [build.icon]
            path = "art.png"
            icns = true
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        match r.icon {
            Some(ResolvedIcon::Path { icns, .. }) => assert!(icns),
            other => panic!("expected ResolvedIcon::Path, got {other:?}"),
        }
    }

    #[test]
    fn icon_generated_form_resolves_with_defaults() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [build.icon]
            src = "art.png"
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        match r.icon {
            Some(ResolvedIcon::Generated {
                src,
                scale,
                background,
                icns,
            }) => {
                assert_eq!(src, PathBuf::from("/cfg/art.png"));
                assert_eq!(scale, 1.0);
                assert!(background.is_none());
                assert!(!icns, "icns conversion should default to false");
            },
            other => panic!("expected ResolvedIcon::Generated, got {other:?}"),
        }
    }

    #[test]
    fn icon_generated_form_can_opt_into_icns_conversion() {
        let cfg = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [build.icon]
            src = "art.png"
            icns = true
        "#})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        match r.icon {
            Some(ResolvedIcon::Generated { icns, .. }) => assert!(icns),
            other => panic!("expected ResolvedIcon::Generated, got {other:?}"),
        }
    }

    #[test]
    fn icon_generated_form_parses_scale_and_background() {
        let cfg = parse_build_config(indoc! { r##"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [build.icon]
            src = "art.png"
            scale = 1.2
            background = "#fefefe"
        "##})
        .unwrap();
        let r = cfg.resolve(Path::new("/cfg"), None).unwrap();
        match r.icon {
            Some(ResolvedIcon::Generated {
                scale, background, ..
            }) => {
                assert_eq!(scale, 1.2);
                assert_eq!(background.as_deref(), Some("#fefefe"));
            },
            other => panic!("expected ResolvedIcon::Generated, got {other:?}"),
        }
    }

    #[test]
    fn icon_mixing_path_and_src_is_rejected() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [build.icon]
            path = "AppIcon.icns"
            src = "art.png"
        "#});
        assert!(err.is_err(), "icon can't be both a path and generated");
    }

    #[test]
    fn icon_unknown_field_is_rejected() {
        let err = parse_build_config(indoc! { r#"
            [app]
            name = "X"
            bundle_id = "y"
            version = "1"
            build_number = "1"

            [build.icon]
            path = "AppIcon.icns"
            scale = 1.2
        "#});
        assert!(err.is_err(), "typo'd icon key should be rejected");
    }
}
