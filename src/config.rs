mod build_target;
mod extension;
mod global;
mod resolved;
mod user;
mod utils;

#[cfg(test)]
mod fixtures;

use std::path::Path;

use anyhow::{Context, Result};
pub use build_target::{IosProvisioningBackend, Platform};

pub use crate::config::extension::ExtensionKind;
pub use crate::config::global::{GLOBAL_CONFIG_TEMPLATE, GlobalConfig};
pub use crate::config::resolved::{
    ResolvedConfig, ResolvedExtension, ResolvedIosSection, ResolvedProject, ResolvedTargetPlatform,
};
use crate::config::user::BuildConfig;
pub use crate::config::user::generate_initial_toml;

pub fn load_config(config_path: &Path) -> Result<ResolvedProject> {
    let global = GlobalConfig::load()?;
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
    let cfg: BuildConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config: {}", config_path.display()))?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    cfg.resolve_project(config_dir, Some(&global))
}
