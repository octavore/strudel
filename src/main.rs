mod apple;
mod builder;
mod config;
mod devices;
mod help;
mod icns;
mod init;
mod paths;
mod shell;
mod status;

use std::os::unix::process::CommandExt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use color_print::{ceprintln, cprintln};

use crate::apple::provisioning;
use crate::builder::{IosBuilder, MacosBuilder};
use crate::config::{
    GLOBAL_CONFIG_TEMPLATE, GlobalConfig, Platform, ResolvedConfig, ResolvedProject,
    ResolvedTargetPlatform,
};

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

    /// Select a target by app name (multi-target configs only)
    #[arg(long, global = true)]
    target: Option<String>,

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
                let project = config::load_config(&cli.config)?;
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Macos,
                    true,
                    |cfg| {
                        MacosBuilder::new(cfg.clone(), dry_run, open, debug, None, false)?.bundle()
                    },
                )?;
            },
            Cmd::Build {
                dry_run,
                open,
                debug,
            } => {
                let project = config::load_config(&cli.config)?;
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Macos,
                    true,
                    |cfg| {
                        MacosBuilder::new(cfg.clone(), dry_run, open, debug, None, false)?.build()
                    },
                )?;
            },
            Cmd::Release {
                dry_run,
                open,
                resume,
                skip_notarization,
            } => {
                let project = config::load_config(&cli.config)?;
                let allow_all = resume.is_none();
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Macos,
                    allow_all,
                    |cfg| {
                        MacosBuilder::new(
                            cfg.clone(),
                            dry_run,
                            open,
                            false,
                            resume.clone(),
                            skip_notarization,
                        )?
                        .release()
                    },
                )?;
            },
            Cmd::Sim {
                dry_run,
                debug,
                simulator,
            } => {
                let project = config::load_config(&cli.config)?;
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Ios,
                    false,
                    |cfg| IosBuilder::new(cfg.clone(), dry_run, debug)?.sim(simulator.as_deref()),
                )?;
            },
            Cmd::Device(DeviceArgs {
                command: None,
                dry_run,
                debug,
                device,
            }) => {
                let project = config::load_config(&cli.config)?;
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Ios,
                    false,
                    |cfg| IosBuilder::new(cfg.clone(), dry_run, debug)?.device(&device),
                )?;
            },
            Cmd::Device(DeviceArgs {
                command: Some(DeviceCmd::Add { devices, dry_run }),
                ..
            }) => {
                let project = config::load_config(&cli.config)?;
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Ios,
                    false,
                    |cfg| IosBuilder::new(cfg.clone(), dry_run, false)?.device_add(&devices),
                )?;
            },
            Cmd::Device(DeviceArgs {
                command:
                    Some(DeviceCmd::Register {
                        udid,
                        name,
                        dry_run,
                    }),
                ..
            }) => {
                let project = config::load_config(&cli.config)?;
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Ios,
                    false,
                    |cfg| {
                        IosBuilder::new(cfg.clone(), dry_run, false)?.device_register(&name, &udid)
                    },
                )?;
            },
            Cmd::Profile(ProfileArgs { command: None }) => {
                status::profile_info(&cli.config, cli.target.as_deref())?;
            },
            Cmd::Profile(ProfileArgs {
                command: Some(ProfileCmd::Fetch { dry_run, force }),
            }) => {
                let project = config::load_config(&cli.config)?;
                for_each_selected(
                    &project,
                    cli.target.as_deref(),
                    Platform::Ios,
                    false,
                    |cfg| IosBuilder::new(cfg.clone(), dry_run, false)?.profile_fetch(force),
                )?;
            },
            Cmd::Clean { dry_run } => {
                let project = config::load_config(&cli.config)?;
                // Clean isn't platform-specific (it just wipes build_dir + the
                // Swift cache), so it acts on every target rather than going
                // through the platform-scoped `select`.
                let targets = all_or_named(&project, cli.target.as_deref())?;
                run_for_targets(targets, |cfg| builder::clean(cfg.clone(), dry_run))?;
            },
            Cmd::MakeIcns { png, icns } => {
                icns::make_icns(&png, &icns, false)?;
            },
            Cmd::Login(LoginArgs {
                command: None,
                apple_id,
            }) => {
                // If no --apple-id flag, try reading it from [ios] apple_id in
                // strudel.toml (best-effort; not required for login to work).
                let apple_id_email = match apple_id.as_deref() {
                    Some(id) => Some(id.to_string()),
                    None => config::load_config(&cli.config).ok().and_then(|p| {
                        p.targets.into_iter().find_map(|t| match t.target_platform {
                            ResolvedTargetPlatform::Ios(ref ios) => ios.apple_id.clone(),
                            _ => None,
                        })
                    }),
                };
                provisioning::login(apple_id_email)?;
            },
            Cmd::Login(LoginArgs {
                command: Some(LoginCmd::Status),
                ..
            }) => {
                status::login_status()?;
            },
            Cmd::Login(LoginArgs {
                command: Some(LoginCmd::Clear),
                ..
            }) => {
                provisioning::logout()?;
            },
            Cmd::Status => {
                status::run(&cli.config, cli.target.as_deref())?;
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
    /// Scaffold a strudel.toml in the given directory
    Init {
        /// Directory to create strudel.toml in (defaults to current directory)
        output_dir: Option<PathBuf>,
    },

    /// Sign in with an Apple ID for free iOS provisioning (7-day profiles,
    /// max 3 devices). Saves the session to ~/.local/share/strudel/.
    /// Set [ios] provisioning = "free" in strudel.toml to enable.
    #[command(args_conflicts_with_subcommands = true)]
    Login(LoginArgs),

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
    /// Run `strudel device add` first to register your device(s).
    #[command(args_conflicts_with_subcommands = true)]
    Device(DeviceArgs),
    /// Show provisioning-profile status for each target. iOS profiles are
    /// auto-managed; run `strudel profile fetch` to fetch or refresh one.
    /// macOS has no auto-fetch: set `build.provisioning_profile` to pin one.
    #[command(args_conflicts_with_subcommands = true)]
    Profile(ProfileArgs),

    /// Remove the strudel output directory and run `swift package clean`
    Clean {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
    },
    /// Convert a PNG to .icns using sips + iconutil
    MakeIcns {
        /// Source PNG path (should be at least 1024x1024)
        png: PathBuf,
        /// Destination .icns path
        icns: PathBuf,
    },

    /// Manage global strudel config (~/.config/strudel/config.toml)
    Config {
        #[command(subcommand)]
        command: ConfigCmd,
    },
    /// Show the full picture: local toolchain versions, global config, the
    /// saved Apple ID session, cached dev credentials, and per-target
    /// provisioning state
    Status,
    /// Show documentation for a topic (config, targets, global-config,
    /// signing, notarize, entitlements, extensions, dylibs, universal, ci,
    /// ios-device). Run with no argument to list topics.
    Help {
        /// Topic to show docs for
        topic: Option<String>,
    },
}

#[derive(clap::Args)]
pub struct LoginArgs {
    #[command(subcommand)]
    command: Option<LoginCmd>,

    /// Apple ID email address (prompted if omitted)
    #[arg(long)]
    apple_id: Option<String>,
}

#[derive(Subcommand)]
enum LoginCmd {
    /// Show just the Apple ID session: signed-in state, apple id, and dsid.
    /// Run `strudel status` for the full picture (config, project, etc.)
    Status,
    /// Sign out and clear the saved Apple ID session and cached dev
    /// credentials
    Clear,
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
    },
    /// Register a single device on the portal by UDID, without tracking it in
    /// .strudel/devices.toml. Use this to register a device you don't have
    /// connected (e.g. a teammate's); use `device add` otherwise.
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
    },
}

#[derive(clap::Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    command: Option<ProfileCmd>,
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// Fetch (or refresh) the development provisioning profile for iOS device
    /// builds
    Fetch {
        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,
        /// Recreate the profile even if the cached one is already current
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Open the global config in $VISUAL/$EDITOR, creating it if needed
    Edit,
}

// Run the given function for each config selected by the target/platform
// criteria
fn for_each_selected(
    project: &ResolvedProject,
    target: Option<&str>,
    platform: Platform,
    allow_all: bool,
    f: impl FnMut(&ResolvedConfig) -> Result<()>,
) -> Result<()> {
    run_for_targets(project.select(target, platform, allow_all)?, f)
}

// Resolve targets for a platform-agnostic command (clean): every target, or
// the single one named by --target.
fn all_or_named<'a>(
    project: &'a ResolvedProject,
    target: Option<&str>,
) -> Result<Vec<&'a ResolvedConfig>> {
    let Some(name) = target else {
        return Ok(project.targets.iter().collect());
    };
    let matched: Vec<&ResolvedConfig> = project
        .targets
        .iter()
        .filter(|t| t.app_name == name)
        .collect();
    if matched.is_empty() {
        let available: Vec<&str> = project
            .targets
            .iter()
            .map(|t| t.app_name.as_str())
            .collect();
        bail!(
            "No target named {name:?}. Available: {}",
            available.join(", ")
        );
    }
    Ok(matched)
}

// Run the given function for each target, printing a header per target when
// there is more than one.
fn run_for_targets(
    targets: Vec<&ResolvedConfig>,
    mut f: impl FnMut(&ResolvedConfig) -> Result<()>,
) -> Result<()> {
    let multi = targets.len() > 1;
    for cfg in targets {
        if multi {
            cprintln!("\n<bold,cyan>-- {} --</bold,cyan>", cfg.app_name);
        }
        f(cfg)?;
    }
    Ok(())
}

fn main() -> ! {
    let exit_code = Cli::execute().map(|_| 0).unwrap_or_else(|e| {
        ceprintln!("<red>Error: {e:#}</red>");
        1
    });

    std::process::exit(exit_code);
}
