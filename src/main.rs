mod builder;
mod config;
mod icns;
mod init;
mod paths;
mod shell;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use color_print::ceprintln;

use crate::builder::Builder;

#[derive(Parser)]
#[command(
    name = "strudel",
    about = "Build, sign, notarize, and package macOS Swift apps",
    version
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
            Cmd::Init { output_dir } => {
                let dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
                init::run_init(&dir)?;
            },
            Cmd::Build {
                dry_run,
                open,
                debug,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, debug).build()?;
            },
            Cmd::Sign {
                dry_run,
                open,
                debug,
            } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, debug).sign_app()?;
            },
            Cmd::Release { dry_run, open } => {
                let cfg = config::load_config(&cli.config)?;
                Builder::new(cfg, dry_run, open, false).release()?;
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
    /// Build and sign the app bundle (no notarization or DMG); for local dev.
    /// Signs ad-hoc when no signing identity is configured.
    Sign {
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
    },
    /// Scaffold a strudel.toml in the given directory
    Init {
        /// Directory to create strudel.toml in (defaults to current directory)
        output_dir: Option<PathBuf>,
    },
    /// Convert a PNG to .icns using sips + iconutil
    MakeIcns {
        /// Source PNG path (should be at least 1024×1024)
        png: PathBuf,
        /// Destination .icns path
        icns: PathBuf,
    },
}

fn main() -> ! {
    let exit_code = Cli::execute().map(|_| 0).unwrap_or_else(|e| {
        ceprintln!("<red>Error: {e:#}</red>");
        1
    });

    std::process::exit(exit_code);
}
