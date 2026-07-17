use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::build_config::BuildConfig;
use crate::config::build_target::IosProvisioningBackend;
use crate::config::{
    GlobalConfig, ResolvedConfig, ResolvedIosSection, ResolvedMacOsSection, ResolvedTargetPlatform,
};

pub const FULL: &str = indoc::indoc! {r#"
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

  [apple]
  identity = "Developer ID Application: Me (TEAM123456)"
  team_id = "TEAM123456"
  api_issuer = "issuer-uuid"
  api_key = "KEYID123"
  api_key_path = "AuthKey.p8"
  notarize_timeout = 1200

  [dmg]
  background = "dmg-bg.png"
  window_width = 800
  window_height = 500
  icon_size = 100
  app_x = 200
  app_y = 200
  applications_x = 600
  applications_y = 200
"#};

pub const MULTI: &str = indoc::indoc! {r#"
  [[target]]
  platform = "macos"
  app.name = "MyApp"
  app.bundle_id = "com.example.myapp"
  app.version = "1.2.3"
  app.build_number = "42"

  [[target]]
  platform = "ios"
  app.name = "MyApp"
  app.bundle_id = "com.example.myapp"
  app.version = "1.2.3"
  app.build_number = "42"
  ios.provisioning = "app_store_connect"
"#};

/// A minimal, credential-free [`ResolvedConfig`] for tests to clone and
/// override. Every field is set explicitly, so adding a field to
/// [`ResolvedConfig`] surfaces here once rather than in each test module.
fn resolved(target_platform: ResolvedTargetPlatform) -> ResolvedConfig {
    ResolvedConfig {
        platform: None,
        target_id: "A".into(),
        app_name: "A".into(),
        bundle_id: "b".into(),
        version: "1".into(),
        build_number: "1".into(),
        source_dir: PathBuf::from("/x"),
        build_dir: PathBuf::from("/x"),
        info_json_path: None,
        entitlements_json_path: None,
        icon: None,
        archs: vec!["arm64".into()],
        target_name: "A".into(),
        sign_identity: String::new(),
        notarize_timeout: 600,
        build_env: HashMap::new(),
        embed_libs: Vec::new(),
        provisioning_profile: None,
        extensions: Vec::new(),
        resources_dir: None,
        resources: Vec::new(),
        target_platform,
        team_id: String::new(),
        apple_api_issuer: String::new(),
        apple_api_key: String::new(),
        apple_api_key_path: None,
        apple_certificate: String::new().into(),
        apple_certificate_password: String::new().into(),
    }
}

/// [`resolved`] with an iOS target platform and stock `[ios]` defaults.
pub fn resolved_ios() -> ResolvedConfig {
    resolved(
        ResolvedIosSection {
            simulator: "iPhone 16".into(),
            device: None,
            deployment_target: "18.0".into(),
            assets_dir: None,
            app_icon_name: "AppIcon".into(),
            provisioning: IosProvisioningBackend::AppStoreConnect,
            apple_id: None,
        }
        .into(),
    )
}

/// [`resolved`] with a macOS target platform and no DMG styling.
pub fn resolved_macos() -> ResolvedConfig {
    resolved(
        ResolvedMacOsSection {
            dmg: None,
            assets_dir: None,
        }
        .into(),
    )
}

pub fn parse_build_config(s: &str) -> Result<BuildConfig, toml::de::Error> {
    toml::from_str(s)
}

/// A [`GlobalConfig`] with every field populated, as if
/// `~/.config/strudel/config.toml` set all of them. Values are prefixed
/// `global-` so precedence tests can see which layer a value came from.
/// `notarize_api_key_path` is absolute, matching `GlobalConfig::load_from`,
/// which resolves it at load time.
pub fn global_config() -> GlobalConfig {
    GlobalConfig {
        signing_identity: Some("global-identity".into()),
        signing_team_id: Some("global-team".into()),
        notarize_api_issuer: Some("global-issuer".into()),
        notarize_api_key: Some("global-key".into()),
        notarize_api_key_path: Some(PathBuf::from("/global/AuthKey.p8")),
    }
}

/// The environment variables [`env_or_global`] consults, all cleared.
/// Precedence tests must run with a known environment or a developer's real
/// `APPLE_*` exports would decide the result.
///
/// [`env_or_global`]: crate::config::utils::env_or_global
pub const APPLE_ENV_UNSET: [(&str, Option<&str>); 5] = [
    ("APPLE_SIGNING_IDENTITY", None),
    ("APPLE_TEAM_ID", None),
    ("APPLE_API_ISSUER", None),
    ("APPLE_API_KEY", None),
    ("APPLE_API_KEY_PATH", None),
];
