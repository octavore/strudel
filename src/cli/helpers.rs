use anyhow::Result;
use clml::cprintln;

use crate::config::{Platform, ResolvedConfig, ResolvedProject};

// Run the given function for each config selected by the target/platform
// criteria
pub(crate) fn for_each_selected(
    project: &ResolvedProject,
    target: Option<&str>,
    platform: Platform,
    allow_all: bool,
    f: impl FnMut(&ResolvedConfig) -> Result<()>,
) -> Result<()> {
    run_for_targets(project.select(target, platform, allow_all)?, f)
}

// Resolve targets for a platform-agnostic command (build/run/release/clean):
// every target, or the single one named by the target positional/flag.
pub(crate) fn all_or_named<'a>(
    project: &'a ResolvedProject,
    target: Option<&str>,
) -> Result<Vec<&'a ResolvedConfig>> {
    match target {
        None => Ok(project.targets.iter().collect()),
        Some(selector) => Ok(vec![project.resolve_target(selector)?]),
    }
}

// Run the given function for each target, printing a header per target when
// there is more than one.
pub(crate) fn run_for_targets(
    targets: Vec<&ResolvedConfig>,
    mut f: impl FnMut(&ResolvedConfig) -> Result<()>,
) -> Result<()> {
    let multi = targets.len() > 1;
    for cfg in targets {
        if multi {
            cprintln!("\n<bold,cyan>-- {} --</bold,cyan>", cfg.target_id);
        }
        f(cfg)?;
    }
    Ok(())
}
