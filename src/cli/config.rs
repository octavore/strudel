use std::os::unix::process::CommandExt;

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::config::{GLOBAL_CONFIG_TEMPLATE, GlobalConfig};

#[derive(clap::Args)]
pub(crate) struct ConfigCmd {
    #[command(subcommand)]
    command: ConfigAction,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Open the global config in $VISUAL/$EDITOR, creating it if needed
    Edit,
}

impl ConfigCmd {
    pub(crate) fn execute(self) -> Result<()> {
        match self.command {
            ConfigAction::Edit => {
                let path = GlobalConfig::xdg_path()?;
                if !path.exists() {
                    std::fs::write(&path, GLOBAL_CONFIG_TEMPLATE)?;
                }
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_else(|_| "vi".to_string());
                let err = std::process::Command::new(&editor).arg(&path).exec();
                Err(err).with_context(|| format!("Failed to exec editor '{editor}'"))
            },
        }
    }
}
