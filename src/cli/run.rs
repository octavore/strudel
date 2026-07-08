use std::path::Path;

use anyhow::Result;

use crate::builder::{IosBuilder, MacosBuilder};
use crate::cli::helpers::{all_or_named, run_for_targets};
use crate::config::{self, ResolvedTargetPlatform};

#[derive(clap::Args)]
pub(crate) struct RunCmd {
    /// Select a target by app name (multi-target configs only)
    target: Option<String>,

    /// macOS only: skip codesigning and leave the bundle unsigned.
    #[arg(long)]
    unsigned: bool,

    /// iOS only: run in the Simulator (default destination). Optionally
    /// override the simulator name (default from [ios] config or
    /// "iPhone 16").
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    sim: Option<String>,

    /// iOS only: run on a connected device instead of the Simulator.
    /// Device name or UDID; may be repeated to install on multiple
    /// devices. Run `strudel devices add` first to register your
    /// device(s).
    #[arg(long = "device")]
    device: Vec<String>,

    /// Print commands without executing them
    #[arg(long)]
    dry_run: bool,

    /// Build with the debug configuration instead of release
    #[arg(long)]
    debug: bool,
}

impl RunCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        let project = config::load_config(config)?;
        let targets = all_or_named(&project, self.target.as_deref())?;
        run_for_targets(targets, |cfg| match &cfg.target_platform {
            ResolvedTargetPlatform::Mac(_) => {
                let builder =
                    MacosBuilder::new(cfg.clone(), self.dry_run, true, self.debug, None, false)?;
                if self.unsigned {
                    builder.bundle()
                } else {
                    builder.build()
                }
            },
            ResolvedTargetPlatform::Ios(_) => {
                let builder = IosBuilder::new(cfg.clone(), self.dry_run, self.debug)?;
                if !self.device.is_empty() {
                    builder.device(&self.device)
                } else {
                    let sim_name = self.sim.as_deref().filter(|s| !s.is_empty());
                    builder.sim(sim_name)
                }
            },
        })
    }
}
