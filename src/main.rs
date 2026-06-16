mod appstore;
mod builder;
mod config;
mod devices;
mod help;
mod icns;
mod init;
mod paths;
mod shell;

use std::os::unix::process::CommandExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use color_print::ceprintln;

use crate::builder::Builder;
use crate::config::{GLOBAL_CONFIG_TEMPLATE, GlobalConfig};

#[derive(Parser)]
#[command(
    name = "strudel",
    about = "Build, sign, notarize, and package macOS/iOS Swift apps",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    /// Path to strudel.toml config file
    #[arg(long, default_value = "strudel.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Cmd,
}

impl Cli {
    fn execute() -> Result<()> {
        let cli = Self::parse();
        match cli.command {
            Cmd::Help { topic } => {
                help::run(topic.as_deref(), Cli::command());
            },
            Cmd::Init { output_dir } => {
                let dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
                init::run_init(&dir)?;
            },
            Cmd::Bundle {
                dry_run,
                open,
                debug,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, debug, None, false).bundle()?;
            },
            Cmd::Build {
                dry_run,
                open,
                debug,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, debug, None, false).build()?;
            },
            Cmd::Release {
                dry_run,
                open,
                resume,
                skip_notarization,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, false, resume, skip_notarization).release()?;
            },
            Cmd::Sim {
                dry_run,
                debug,
                simulator,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, false, debug, None, false).sim(simulator.as_deref())?;
            },
            Cmd::Device(DeviceArgs {
                command: None,
                dry_run,
                debug,
                device,
            }) => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, false, debug, None, false).device(&device)?;
            },
            Cmd::Device(DeviceArgs {
                command: Some(DeviceCmd::Register { devices, dry_run }),
                ..
            }) => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, false, false, None, false).device_register(&devices)?;
            },
            Cmd::Profile { dry_run, force } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, false, false, None, false).profile_fetch(force)?;
            },
            Cmd::MakeIcns { png, icns } => {
                icns::make_icns(&png, &icns, false)?;
            },
            Cmd::Config { command } => match command {
                ConfigCmd::Edit => {
                    let path = GlobalConfig::xdg_path()?;
                    if !path.exists() {
                        std::fs::write(&path, GLOBAL_CONFIG_TEMPLATE)?;
                    }
                    let editor = std::env::var("VISUAL")
                        .or_else(|_| std::env::var("EDITOR"))
                        .unwrap_or_else(|_| "vi".to_string());
                    let err = std::process::Command::new(&editor).arg(&path).exec();
                    return Err(err).with_context(|| format!("Failed to exec editor '{editor}'"));
                },
            },
        };
        Ok(())
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Build the app bundle only (no signing/notarization)
    Bundle {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
        /// Open the app bundle after a successful build
        #[arg(long)]
        open: bool,
        /// Build with the debug configuration instead of release
        #[arg(long)]
        debug: bool,
    },
    /// Build and sign the app bundle (no notarization or DMG); for local dev.
    /// Signs ad-hoc when no signing identity is configured.
    Build {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
        /// Open the app bundle after a successful build
        #[arg(long)]
        open: bool,
        /// Build with the debug configuration instead of release
        #[arg(long)]
        debug: bool,
    },
    /// Full release: build, sign, notarize, and package DMG
    Release {
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
    },
    /// Scaffold a strudel.toml in the given directory
    Init {
        /// Directory to create strudel.toml in (defaults to current directory)
        output_dir: Option<PathBuf>,
    },
    /// Build for the iOS Simulator and launch in Simulator.app
    Sim {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
        /// Build with the Debug configuration instead of Release
        #[arg(long)]
        debug: bool,
        /// Override the simulator name (default from [ios] config or "iPhone
        /// 16")
        #[arg(long)]
        simulator: Option<String>,
    },
    /// Build for a connected iOS device, then install and launch.
    /// Run `strudel device register` first to register your device(s).
    #[command(args_conflicts_with_subcommands = true)]
    Device(DeviceArgs),
    /// Fetch (or refresh) the development provisioning profile for iOS device
    /// builds
    Profile {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
        /// Recreate the profile even if the cached one is already current
        #[arg(long)]
        force: bool,
    },
    /// Convert a PNG to .icns using sips + iconutil
    MakeIcns {
        /// Source PNG path (should be at least 1024x1024)
        png: PathBuf,
        /// Destination .icns path
        icns: PathBuf,
    },
    /// Show documentation for a topic (config, global-config, signing,
    /// notarize, entitlements, extensions, dylibs, universal, ci). Run with
    /// no argument to list topics.
    Help {
        /// Topic to show docs for
        topic: Option<String>,
    },
    /// Manage global strudel config (~/.config/strudel/config.toml)
    Config {
        #[command(subcommand)]
        command: ConfigCmd,
    },
}

#[derive(clap::Args)]
pub struct DeviceArgs {
    #[command(subcommand)]
    command: Option<DeviceCmd>,

    /// Print commands without executing them
    #[arg(long)]
    dry_run: bool,
    /// Build with the Debug configuration instead of Release
    #[arg(long)]
    debug: bool,
    /// Device name or UDID to target (may be repeated to install on multiple
    /// devices)
    #[arg(long)]
    device: Vec<String>,
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// Register connected iOS devices on the App Store Connect portal and
    /// track them in .strudel/devices.toml
    Register {
        /// Device name or UDID to register (may be repeated; default: all
        /// connected devices)
        #[arg(long = "device")]
        devices: Vec<String>,
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Open the global config in $VISUAL/$EDITOR, creating it if needed
    Edit,
}

fn main() -> ! {
    let exit_code = Cli::execute().map(|_| 0).unwrap_or_else(|e| {
        ceprintln!("<red>Error: {e:#}</red>");
        1
    });

    std::process::exit(exit_code);
}
