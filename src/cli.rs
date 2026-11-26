mod build;
mod clean;
mod config;
mod devices;
mod help;
pub(crate) mod helpers;
mod icloud_containers;
mod icon;
mod increment_version;
mod init;
mod login;
mod profile;
mod release;
mod run;
mod skill;
mod status;

use std::path::PathBuf;

use anyhow::Result;
use clap::builder::styling;
use clap::{Parser, Subcommand};

use crate::cli::build::BuildCmd;
use crate::cli::clean::CleanCmd;
use crate::cli::config::ConfigCmd;
use crate::cli::devices::DevicesCmd;
use crate::cli::help::HelpCmd;
use crate::cli::icloud_containers::IcloudContainersCmd;
use crate::cli::icon::IconCmd;
use crate::cli::init::InitCmd;
use crate::cli::login::LoginCmd;
use crate::cli::profile::ProfileCmd;
use crate::cli::release::ReleaseCmd;
use crate::cli::run::RunCmd;
use crate::cli::skill::SkillCmd;
use crate::cli::status::StatusCmd;

const STYLES: styling::Styles = styling::Styles::styled()
    .header(styling::AnsiColor::Green.on_default().bold())
    .usage(styling::AnsiColor::Green.on_default().bold())
    .literal(styling::AnsiColor::Blue.on_default().bold())
    .placeholder(styling::AnsiColor::Cyan.on_default());

#[derive(Parser)]
#[command(
    name = "strudel",
    about = "Build, sign, notarize, and package macOS/iOS Swift apps",
    version,
    styles = STYLES,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Path to strudel.toml config file
    #[arg(long, global = true, default_value = "strudel.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a strudel.toml in the given directory
    Init(InitCmd),

    /// Sign in with an Apple ID for free iOS provisioning (7-day profiles,
    /// max 3 devices). Saves the session to ~/.local/share/strudel/. Run it
    /// again to add another Apple ID. Set [ios] provisioning = "free" in
    /// strudel.toml to enable. Paid teams use a session only for
    /// `icloud-containers`.
    #[command(args_conflicts_with_subcommands = true)]
    Login(LoginCmd),

    /* primary commands */
    /// Assemble the app bundle.
    /// macOS: signed .app (using configured identity, ad-hoc fallback; skip
    /// signing with --unsigned).
    /// iOS: unsigned .app, no install/launch.
    Build(BuildCmd),
    /// Build and launch locally.
    /// macOS: runs build, then opens the app. With no flags, only macOS
    /// targets run; pass --sim/--device for iOS.
    /// iOS: install and launch on a simulator or device.
    Run(RunCmd),
    /// (macOS) Create a full distributable DMG: signed, notarized, and
    /// packaged. iOS is not supported yet.
    Release(ReleaseCmd),

    /* configuration commands */
    /// Manage tracked devices. Run with no subcommand to list iOS devices
    /// tracked in .strudel/devices.toml. `devices add` on a macOS target adds
    /// this Mac for macOS development profiles.
    #[command(args_conflicts_with_subcommands = true)]
    Devices(DevicesCmd),
    /// Show provisioning-profile status for each target. Run `strudel profile
    /// fetch` to create or refresh the strudel-managed profiles.
    #[command(args_conflicts_with_subcommands = true)]
    Profile(ProfileCmd),
    /// Create and list iCloud containers on the team, and link them to App
    /// IDs. Only `icloud-containers create` creates containers, and only
    /// `icloud-containers link` links them. Builds do not do either.
    IcloudContainers(IcloudContainersCmd),

    /* helper commands */
    /// Remove the strudel output directory and run `swift package clean`
    Clean(CleanCmd),

    /// Show or bump the project version (`config version`,
    /// `config increment-version`); `config global edit` manages the global
    /// config (~/.config/strudel/config.toml).
    Config(ConfigCmd),
    /// Render each target's configured app icon to a plain PNG (or copy it,
    /// for a path-based icon), for inspecting generated artwork without
    /// running a full build.
    Icon(IconCmd),
    /// Install a skill file that points an AI coding agent at strudel docs
    /// or tooling for this project. `skill install` for usage.
    Skill(SkillCmd),
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
            Cmd::IcloudContainers(c) => c.execute(&config),
            Cmd::Clean(c) => c.execute(&config),
            Cmd::Config(c) => c.execute(&config),
            Cmd::Icon(c) => c.execute(&config),
            Cmd::Skill(c) => c.execute(),
            Cmd::Status(c) => c.execute(&config),
            Cmd::Help(c) => c.execute(),
        }
    }
}
