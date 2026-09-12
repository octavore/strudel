use std::path::{Path, PathBuf};

use crate::config::resolved::{Sourced, ValueSource};

/// Expand a leading `~` or `~/` to the user's home directory.
pub fn expand_tilde(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(shellexpand::tilde(s.as_ref()).as_ref())
}

/// Expand tilde and resolve `p` relative to `base` if it is not absolute.
pub fn resolve_to(base: &Path, p: PathBuf) -> PathBuf {
    let p = expand_tilde(p);
    if p.is_absolute() { p } else { base.join(p) }
}

pub fn resolve_path(base: &Path, p: impl AsRef<Path>) -> PathBuf {
    resolve_to(base, p.as_ref().to_path_buf())
}

/// Select a config var by checking, in order: env, project, global.
pub fn env_or_global(
    project_val: Option<String>,
    global_val: Option<String>,
    env_key: &str,
) -> Sourced {
    if let Ok(v) = std::env::var(env_key) {
        return Sourced::new(v, ValueSource::Env);
    }
    if let Some(v) = project_val {
        return Sourced::new(v, ValueSource::Project);
    }
    if let Some(v) = global_val {
        return Sourced::new(v, ValueSource::Global);
    }
    Sourced::default()
}
