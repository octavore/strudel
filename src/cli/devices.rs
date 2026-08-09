use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

use crate::builder::{IosBuilder, OutputFlags};
use crate::cli::helpers::for_each_selected;
use crate::config::{self, Platform};
use crate::status;

#[derive(clap::Args)]
pub(crate) struct DevicesCmd {
    #[command(subcommand)]
    command: Option<DevicesAction>,

    /// Select a target by id
    #[arg(long)]
    target: Option<String>,
}

#[derive(Subcommand)]
enum DevicesAction {
    /// Register connected iOS devices on the portal (if not already) and
    /// track them in .strudel/devices.toml. This is the common workflow for
    /// adding a device you have plugged in.
    Add {
        /// Device name or UDID to add (may be repeated; default: all
        /// connected devices)
        #[arg(long = "device")]
        devices: Vec<String>,

        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,

        /// Select a target by id
        #[arg(long)]
        target: Option<String>,
    },
    /// Register a single device on the portal by UDID, without tracking it in
    /// .strudel/devices.toml. Use this to register a device you don't have
    /// connected (e.g. a teammate's); use `devices add` otherwise.
    Register {
        /// Device UDID
        #[arg(long)]
        udid: String,

        /// Device name
        #[arg(long)]
        name: String,

        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,

        /// Select a target by id
        #[arg(long)]
        target: Option<String>,
    },
}

impl DevicesCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        match self.command {
            None => status::devices_list(config, self.target.as_deref()),
            Some(DevicesAction::Add {
                devices,
                dry_run,
                target,
            }) => {
                let project = config::load_config(config)?;
                for_each_selected(&project, target.as_deref(), Platform::Ios, false, |cfg| {
                    let output = OutputFlags {
                        dry_run,
                        ..Default::default()
                    };
                    IosBuilder::new(cfg.clone(), output, false)?.device_add(&devices)
                })
            },
            Some(DevicesAction::Register {
                udid,
                name,
                dry_run,
                target,
            }) => {
                let project = config::load_config(config)?;
                for_each_selected(&project, target.as_deref(), Platform::Ios, false, |cfg| {
                    let output = OutputFlags {
                        dry_run,
                        ..Default::default()
                    };
                    IosBuilder::new(cfg.clone(), output, false)?.device_register(&name, &udid)
                })
            },
        }
    }
}
