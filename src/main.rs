mod builder;
mod config;
mod icns;
mod init;
mod paths;
mod shell;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

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

#[derive(Subcommand)]
enum Cmd {
    /// Build the app bundle only (no signing/notarization)
    Build {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
    },
    /// Build and sign the app bundle (no notarization or DMG); for local dev.
    /// Signs ad-hoc when no signing identity is configured.
    Sign {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
    },
    /// Full release: build, sign, notarize, and package DMG
    Release {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Cmd::Init { output_dir } => {
            let dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
            init::run_init(&dir)?;
        }
        Cmd::Build { dry_run } => {
            let cfg = config::load_config(&cli.config)?;
            builder::Builder::new(cfg, dry_run).build()?;
        }
        Cmd::Sign { dry_run } => {
            let cfg = config::load_config(&cli.config)?;
            builder::Builder::new(cfg, dry_run).sign_app()?;
        }
        Cmd::Release { dry_run } => {
            let cfg = config::load_config(&cli.config)?;
            builder::Builder::new(cfg, dry_run).release()?;
        }
        Cmd::MakeIcns { png, icns } => {
            icns::make_icns(&png, &icns, false)?;
        }
    }

    Ok(())
}
