use anyhow::Result;
use clap::CommandFactory;

use crate::cli::Cli;
use crate::help;

#[derive(clap::Args)]
pub(crate) struct HelpCmd {
    /// Topic to show docs for
    topic: Option<String>,
}

impl HelpCmd {
    pub(crate) fn execute(self) -> Result<()> {
        help::run(self.topic.as_deref(), Cli::command());
        Ok(())
    }
}
