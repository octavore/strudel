use std::path::Path;

use anyhow::Result;

use crate::builder::{IosBuilder, MacosBuilder};
use crate::cli::helpers::{all_or_named, run_for_targets};
use crate::config::{self, Platform, ResolvedTargetPlatform};

#[derive(clap::Args)]
pub(crate) struct RunCmd {
    /// Select a target by id
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
    /// Optionally give a device name or UDID; may be repeated to install on
    /// multiple devices. With no name, auto-selects the sole registered
    /// device (or the sole connected device if none are registered). Run
    /// `strudel devices add` first to register your device(s).
    #[arg(long = "device", num_args = 0..=1, default_missing_value = "")]
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
        // --sim / --device only apply to iOS targets, so if either is given
        // restrict selection to iOS instead of also running the macOS target.
        // With no flags at all, only run macOS targets.
        let targets = if self.sim.is_some() || !self.device.is_empty() {
            project.select(self.target.as_deref(), Platform::Ios, true)?
        } else if self.target.is_some() {
            all_or_named(&project, self.target.as_deref())?
        } else {
            project.select(None, Platform::Macos, true)?
        };
        run_for_targets(targets, |cfg| match &cfg.target_platform {
            ResolvedTargetPlatform::Mac(_) => {
                let mut builder =
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
                    let selectors: Vec<String> = self
                        .device
                        .iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect();
                    builder.device(&selectors)
                } else {
                    let sim_name = self.sim.as_deref().filter(|s| !s.is_empty());
                    builder.sim(sim_name)
                }
            },
        })
    }
}
