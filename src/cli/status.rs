use std::path::Path;

use anyhow::Result;

use crate::status;

#[derive(clap::Args)]
pub(crate) struct StatusCmd {
    /// Select a target by id
    #[arg(long)]
    target: Option<String>,
}

impl StatusCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        status::run(config, self.target.as_deref())
    }
}
