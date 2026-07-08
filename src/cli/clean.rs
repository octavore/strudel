use std::path::Path;

use anyhow::Result;

use crate::cli::helpers::{all_or_named, run_for_targets};
use crate::config;

#[derive(clap::Args)]
pub(crate) struct CleanCmd {
    /// Select a target by app name (multi-target configs only)
    #[arg(long)]
    target: Option<String>,

    /// Print commands without executing them
    #[arg(long)]
    dry_run: bool,
}

impl CleanCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        let project = config::load_config(config)?;
        // Clean isn't platform-specific (it just wipes build_dir + the Swift
        // cache), so it acts on every target rather than going through the
        // platform-scoped `select`.
        let targets = all_or_named(&project, self.target.as_deref())?;
        run_for_targets(targets, |cfg| {
            crate::builder::clean(cfg.clone(), self.dry_run)
        })
    }
}
