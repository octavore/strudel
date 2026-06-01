use std::path::{Path, PathBuf};

pub fn resolve_path(base: &Path, p: Option<PathBuf>, default: impl AsRef<Path>) -> PathBuf {
    p.map(|p| if p.is_absolute() { p } else { base.join(&p) })
        .unwrap_or_else(|| base.join(default))
}

pub fn env_or(cfg_val: Option<String>, env_key: &str) -> String {
    cfg_val
        .or_else(|| std::env::var(env_key).ok())
        .unwrap_or_default()
}
