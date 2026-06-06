mod builder;
mod config;
mod help;
mod icns;
mod init;
mod paths;
mod shell;

use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use color_print::ceprintln;

use crate::builder::Builder;

#[derive(Parser)]
#[command(
    name = "strudel",
    about = "Build, sign, notarize, and package macOS Swift apps",
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
                Builder::new(cfg, dry_run, open, debug, None).build()?;
            },
            Cmd::Build {
                dry_run,
                open,
                debug,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, debug, None).sign_app()?;
            },
            Cmd::Release {
                dry_run,
                open,
                resume,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, false, resume).release()?;
            },
            Cmd::Sim {
                dry_run,
                debug,
                simulator,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, false, debug, None).sim(simulator.as_deref())?;
            },
            Cmd::Device {
                dry_run,
                debug,
                device,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, false, debug, None).device(device.as_deref())?;
            },
            Cmd::MakeIcns { png, icns } => {
                icns::make_icns(&png, &icns, false)?;
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
    /// Build for a connected iOS device, then install and launch
    Device {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
        /// Build with the Debug configuration instead of Release
        #[arg(long)]
        debug: bool,
        /// Device name or UDID (default from [ios] config or auto-detected)
        #[arg(long)]
        device: Option<String>,
    },
    /// Convert a PNG to .icns using sips + iconutil
    MakeIcns {
        /// Source PNG path (should be at least 1024×1024)
        png: PathBuf,
        /// Destination .icns path
        icns: PathBuf,
    },
    /// Show documentation for a topic (config, signing, notarize, entitlements,
    /// extensions, dylibs, universal, ci). Run with no argument to list topics.
    Help {
        /// Topic to show docs for
        topic: Option<String>,
    },
}

fn main() -> ! {
    let exit_code = Cli::execute().map(|_| 0).unwrap_or_else(|e| {
        ceprintln!("<red>Error: {e:#}</red>");
        1
    });

    std::process::exit(exit_code);
}
