use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BuildConfig {
    pub app_name: String,
    pub bundle_id: String,
    pub version: String,
    pub build_number: String,
    pub source_dir: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,
    pub info_json_path: Option<PathBuf>,
    pub entitlements_json_path: Option<PathBuf>,
    pub icon_path: Option<PathBuf>,
    pub archs: Option<Vec<String>>,
    /// Swift executable target name. Defaults to app_name.
    pub target_name: Option<String>,
    // Signing — optional, overridden by env vars if absent
    pub team_id: Option<String>,
    pub sign_identity: Option<String>,
    pub apple_id: Option<String>,
    pub apple_password: Option<String>,
    pub notarize_timeout: Option<u64>,
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
    pub team_id: String,
    pub sign_identity: String,
    pub apple_id: String,
    pub apple_password: String,
    pub notarize_timeout: u64,
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

pub fn resolve_config(cfg: BuildConfig, config_dir: &Path) -> ResolvedConfig {
    let source_dir = resolve_path(config_dir, cfg.source_dir, ".");
    let build_dir = resolve_path(&source_dir, cfg.build_dir, ".build/dist");
    let target_name = cfg.target_name.unwrap_or_else(|| cfg.app_name.clone());

    ResolvedConfig {
        // User-supplied input paths are resolved relative to the config file's
        // directory (the one fixed anchor the user reasons about), independent of
        // `source_dir`. info_json_path and icon_path are optional with no default.
        info_json_path: cfg
            .info_json_path
            .map(|p| if p.is_absolute() { p } else { config_dir.join(&p) }),
        entitlements_json_path: resolve_path(
            config_dir,
            cfg.entitlements_json_path,
            "entitlements.json",
        ),
        icon_path: cfg
            .icon_path
            .map(|p| if p.is_absolute() { p } else { config_dir.join(&p) }),
        archs: cfg.archs.unwrap_or_else(|| {
            let arch = match std::env::consts::ARCH {
                "aarch64" => "arm64",
                other => other,
            };
            vec![arch.to_string()]
        }),
        team_id: env_or(cfg.team_id, "TEAM_ID"),
        sign_identity: env_or(cfg.sign_identity, "SIGN_IDENTITY"),
        apple_id: env_or(cfg.apple_id, "APPLE_ID"),
        apple_password: env_or(cfg.apple_password, "APPLE_PASSWORD"),
        notarize_timeout: cfg.notarize_timeout.unwrap_or(600),
        app_name: cfg.app_name,
        bundle_id: cfg.bundle_id,
        version: cfg.version,
        build_number: cfg.build_number,
        source_dir,
        build_dir,
        target_name,
    }
}

pub fn load_config(config_path: &Path) -> Result<ResolvedConfig> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
    let cfg: BuildConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config: {}", config_path.display()))?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    Ok(resolve_config(cfg, config_dir))
}
