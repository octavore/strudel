use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::builder::MacosBuilder;
use crate::cli::helpers::{all_or_named, run_for_targets};
use crate::config::{self, ResolvedTargetPlatform};

#[derive(clap::Args)]
pub(crate) struct ReleaseCmd {
    /// Select a target by id
    target: Option<String>,

    /// Print commands without executing them
    #[arg(long)]
    dry_run: bool,

    /// Open the app bundle after a successful build
    #[arg(long)]
    open: bool,

    /// Resume a pending notarization. Pass a UUID to resume a specific
    /// submission, or omit to auto-detect the most recent one.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    resume: Option<String>,

    /// Build and package the DMG without submitting for notarization
    #[arg(long)]
    skip_notarization: bool,

    /// Copy the built app into /Applications after a successful release
    #[arg(long)]
    install: bool,

    /// Copy the built DMG into this directory after a successful release
    #[arg(long)]
    dmg_output_dir: Option<PathBuf>,

    /// Trim interactive-only output in CI to reduce log noise.
    #[arg(long)]
    ci: bool,
}

impl ReleaseCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        let project = config::load_config(config)?;
        let targets = all_or_named(&project, self.target.as_deref())?;
        if self.resume.is_some() && targets.len() > 1 {
            let available: Vec<&str> = targets.iter().map(|t| t.target_id.as_str()).collect();
            bail!(
                "Multiple targets; select one to resume. Available: {}",
                available.join(", ")
            );
        }
        run_for_targets(targets, |cfg| match &cfg.target_platform {
            ResolvedTargetPlatform::Mac(_) => {
                let mut builder = MacosBuilder::new(
                    cfg.clone(),
                    self.dry_run,
                    self.open,
                    false,
                    self.resume.clone(),
                    self.skip_notarization,
                    self.ci,
                )?;
                builder.release()?;
                if self.install {
                    builder.install_to_applications()?;
                }
                if let Some(ref dir) = self.dmg_output_dir {
                    builder.copy_dmg_to(dir)?;
                }
                Ok(())
            },
            ResolvedTargetPlatform::Ios(_) => {
                bail!(
                    "`release` is not supported yet for iOS targets. Use \
                     `run --device` instead."
                )
            },
        })
    }
}
