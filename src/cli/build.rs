use std::path::Path;

use anyhow::Result;

use crate::builder::{IosBuilder, MacosBuilder, OutputFlags};
use crate::cli::helpers::{all_or_named, run_for_targets};
use crate::config::{self, ResolvedTargetPlatform};

#[derive(clap::Args)]
pub(crate) struct BuildCmd {
    /// Select a target by id
    target: Option<String>,

    /// macOS only: skip codesigning and leave the bundle unsigned.
    #[arg(long)]
    unsigned: bool,

    /// Print commands without executing them
    #[arg(long)]
    dry_run: bool,

    /// Suppress echoing of the underlying commands
    #[arg(long)]
    no_echo: bool,

    /// Full silence: no progress messages or command echo, and streamed
    /// subprocess output (e.g. `swift build`) is only shown on failure.
    /// Implies --no-echo.
    #[arg(long)]
    quiet: bool,

    /// (macOS) Open the app bundle after a successful build
    #[arg(long)]
    open: bool,

    /// (macOS) Copy the built app into /Applications after a successful
    /// build
    #[arg(long)]
    install: bool,

    /// Build with the debug configuration instead of release
    #[arg(long)]
    debug: bool,
}

impl BuildCmd {
    fn output_flags(&self) -> OutputFlags {
        OutputFlags {
            dry_run: self.dry_run,
            no_echo: self.no_echo,
            quiet: self.quiet,
        }
    }

    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        let project = config::load_config(config)?;
        let targets = all_or_named(&project, self.target.as_deref())?;
        run_for_targets(targets, |cfg| match &cfg.target_platform {
            ResolvedTargetPlatform::Mac(_) => {
                let mut builder = MacosBuilder::new(
                    cfg.clone(),
                    self.output_flags(),
                    self.open,
                    self.debug,
                    None,
                    false,
                    false,
                )?;
                if self.unsigned {
                    builder.bundle()?;
                } else {
                    builder.build()?;
                }
                if self.install {
                    builder.install_to_applications()?;
                }
                Ok(())
            },
            ResolvedTargetPlatform::Ios(_) => {
                if self.install {
                    anyhow::bail!("--install is only supported for macOS targets");
                }
                IosBuilder::new(cfg.clone(), self.output_flags(), self.debug)?.build()
            },
        })
    }
}
