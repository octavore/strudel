mod extension;
mod resolved;
mod user;
mod utils;

#[cfg(test)]
mod fixtures;

use std::path::Path;

use anyhow::{Context, Result};

pub use crate::config::extension::ExtensionKind;
pub use crate::config::resolved::{ResolvedConfig, ResolvedExtension};
use crate::config::user::BuildConfig;
pub use crate::config::user::{NotaryAuth, generate_initial_toml};

pub fn load_config(config_path: &Path) -> Result<ResolvedConfig> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
    let cfg: BuildConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config: {}", config_path.display()))?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    cfg.resolve(config_dir)
}
