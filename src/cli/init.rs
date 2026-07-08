use std::path::PathBuf;

use anyhow::Result;

use crate::init;

#[derive(clap::Args)]
pub(crate) struct InitCmd {
    /// Directory to create strudel.toml in (defaults to current directory)
    output_dir: Option<PathBuf>,
}

impl InitCmd {
    pub(crate) fn execute(self) -> Result<()> {
        let dir = self.output_dir.unwrap_or_else(|| PathBuf::from("."));
        init::run_init(&dir)
    }
}
