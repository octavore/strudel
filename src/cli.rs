mod build;
mod clean;
mod config;
mod devices;
mod help;
mod helpers;
mod icon;
mod init;
mod login;
mod profile;
mod release;
mod run;
mod status;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::cli::build::BuildCmd;
use crate::cli::clean::CleanCmd;
use crate::cli::config::ConfigCmd;
use crate::cli::devices::DevicesCmd;
use crate::cli::help::HelpCmd;
use crate::cli::icon::IconCmd;
use crate::cli::init::InitCmd;
use crate::cli::login::LoginCmd;
use crate::cli::profile::ProfileCmd;
use crate::cli::release::ReleaseCmd;
use crate::cli::run::RunCmd;
use crate::cli::status::StatusCmd;

#[derive(Parser)]
#[command(
    name = "strudel",
    about = "Build, sign, notarize, and package macOS/iOS Swift apps",
    version,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Path to strudel.toml config file
    #[arg(long, default_value = "strudel.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a strudel.toml in the given directory
    Init(InitCmd),

    /// Sign in with an Apple ID for free iOS provisioning (7-day profiles,
    /// max 3 devices). Saves the session to ~/.local/share/strudel/.
    /// Set [ios] provisioning = "free" in strudel.toml to enable.
    #[command(args_conflicts_with_subcommands = true)]
    Login(LoginCmd),

    /* primary commands */
    /// Assemble the app bundle.
    /// macOS: signed .app (using configured identity, ad-hoc fallback; skip
    /// signing with --unsigned).
    /// iOS: unsigned .app, no install/launch.
    Build(BuildCmd),
    /// Build and launch locally.
    /// macOS: runs build, then opens the app.
    /// iOS: install and launch on a simulator or device.
    Run(RunCmd),
    /// (macOS) Create a full distributable DMG: signed, notarized, and
    /// packaged. iOS is not supported yet.
    Release(ReleaseCmd),

    /* configuration commands */
    /// (iOS) Manage tracked iOS devices. Run with no subcommand to list devices
    /// tracked in .strudel/devices.toml.
    #[command(args_conflicts_with_subcommands = true)]
    Devices(DevicesCmd),
    /// Show provisioning-profile status for each target. iOS profiles are
    /// auto-managed; run `strudel profile fetch` to fetch or refresh one.
    /// macOS has no auto-fetch: set `build.provisioning_profile` to pin one.
    #[command(args_conflicts_with_subcommands = true)]
    Profile(ProfileCmd),

    /* helper commands */
    /// Remove the strudel output directory and run `swift package clean`
    Clean(CleanCmd),

    /// Manage global strudel config (~/.config/strudel/config.toml)
    Config(ConfigCmd),
    /// Render each target's configured app icon to a plain PNG (or copy it,
    /// for a path-based icon), for inspecting generated artwork without
    /// running a full build.
    Icon(IconCmd),
    /// Show overall status: local toolchain versions, global config, the
    /// logged-in session (for free provisioning, if any), cached dev
    /// credentials, and per-target provisioning state.
    Status(StatusCmd),

    /// Show documentation for commands and topics (including: config, targets,
    /// global-config, signing, notarize, entitlements, extensions, dylibs,
    /// universal, ci, ios-device, ios-free-provisioning). Run with no argument
    /// to list topics.
    Help(HelpCmd),
}

impl Cli {
    pub fn execute() -> Result<()> {
        let cli = Self::parse();
        let config = cli.config;
        match cli.command {
            Cmd::Init(c) => c.execute(),
            Cmd::Login(c) => c.execute(&config),
            Cmd::Build(c) => c.execute(&config),
            Cmd::Run(c) => c.execute(&config),
            Cmd::Release(c) => c.execute(&config),
            Cmd::Devices(c) => c.execute(&config),
            Cmd::Profile(c) => c.execute(&config),
            Cmd::Clean(c) => c.execute(&config),
            Cmd::Config(c) => c.execute(),
            Cmd::Icon(c) => c.execute(&config),
            Cmd::Status(c) => c.execute(&config),
            Cmd::Help(c) => c.execute(),
        }
    }
}
