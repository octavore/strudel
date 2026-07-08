use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

use crate::builder::IosBuilder;
use crate::cli::helpers::for_each_selected;
use crate::config::{self, Platform};
use crate::status;

#[derive(clap::Args)]
pub(crate) struct ProfileCmd {
    #[command(subcommand)]
    command: Option<ProfileAction>,

    /// Select a target by app name (multi-target configs only)
    #[arg(long)]
    target: Option<String>,
}

#[derive(Subcommand)]
enum ProfileAction {
    /// Fetch (or refresh) the development provisioning profile for iOS device
    /// builds
    Fetch {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,

        /// Recreate the profile even if the cached one is already current
        #[arg(long)]
        force: bool,

        /// Select a target by app name (multi-target configs only)
        #[arg(long)]
        target: Option<String>,
    },
}

impl ProfileCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        match self.command {
            None => status::profile_info(config, self.target.as_deref()),
            Some(ProfileAction::Fetch {
                dry_run,
                force,
                target,
            }) => {
                let project = config::load_config(config)?;
                for_each_selected(&project, target.as_deref(), Platform::Ios, false, |cfg| {
                    IosBuilder::new(cfg.clone(), dry_run, false)?.profile_fetch(force)
                })
            },
        }
    }
}
