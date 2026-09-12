use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

use crate::builder::{IosBuilder, MacosBuilder, OutputFlags};
use crate::cli::helpers::{all_or_named, run_for_targets};
use crate::config::{self, ResolvedTargetPlatform};
use crate::status;

#[derive(clap::Args)]
pub(crate) struct ProfileCmd {
    #[command(subcommand)]
    command: Option<ProfileAction>,

    /// Select a target by id
    #[arg(long)]
    target: Option<String>,
}

#[derive(Subcommand)]
enum ProfileAction {
    /// Create or refresh the provisioning profiles strudel manages: the
    /// development profile for iOS device builds, and the Developer ID
    /// profiles for macOS bundle IDs that need one
    Fetch {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,

        /// Recreate the profile even if the cached one is already current
        #[arg(long)]
        force: bool,

        /// Select a target by id
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
                let targets = all_or_named(&project, target.as_deref())?;
                run_for_targets(targets, |cfg| {
                    let output = OutputFlags {
                        dry_run,
                        ..Default::default()
                    };
                    match &cfg.target_platform {
                        ResolvedTargetPlatform::Mac(_) => MacosBuilder::new(
                            cfg.clone(),
                            output,
                            false,
                            false,
                            None,
                            false,
                            false,
                        )?
                        .profile_fetch(force),
                        ResolvedTargetPlatform::Ios(_) => {
                            IosBuilder::new(cfg.clone(), output, false)?.profile_fetch(force)
                        },
                    }
                })
            },
        }
    }
}
