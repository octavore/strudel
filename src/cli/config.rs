use std::os::unix::process::CommandExt;
use std::path::Path;

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::cli::increment_version::{self, Component};
use crate::config::{GLOBAL_CONFIG_TEMPLATE, GlobalConfig};

#[derive(clap::Args)]
pub(crate) struct ConfigCmd {
    #[command(subcommand)]
    command: ConfigAction,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the current app.version / app.build_number for each target
    Version {
        /// Label each line with its target id, even if every target shares
        /// the same version and build number
        #[arg(long)]
        labels: bool,
    },
    /// Bump app.version (major/minor/patch) or app.build_number (build) in
    /// strudel.toml, after confirming the change
    IncrementVersion { component: Component },
    /// Manage the global strudel config (~/.config/strudel/config.toml)
    Global(GlobalCmd),
}

#[derive(clap::Args)]
pub(crate) struct GlobalCmd {
    #[command(subcommand)]
    command: GlobalAction,
}

#[derive(Subcommand)]
enum GlobalAction {
    /// Open the global config in $VISUAL/$EDITOR, creating it if needed
    Edit,
}

impl ConfigCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        match self.command {
            ConfigAction::Version { labels } => increment_version::show(config, labels),
            ConfigAction::IncrementVersion { component } => {
                increment_version::increment(config, component)
            },
            ConfigAction::Global(GlobalCmd {
                command: GlobalAction::Edit,
            }) => {
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
