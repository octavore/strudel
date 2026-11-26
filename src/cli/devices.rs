use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use crate::builder::{IosBuilder, MacosBuilder, OutputFlags};
use crate::cli::helpers::for_each_selected;
use crate::config::{self, Platform, ResolvedConfig, ResolvedProject, ResolvedTargetPlatform};
use crate::status;

#[derive(clap::Args)]
pub(crate) struct DevicesCmd {
    #[command(subcommand)]
    command: Option<DevicesAction>,

    /// Select a target by id
    #[arg(long)]
    target: Option<String>,

    /// Echo the underlying commands
    #[arg(long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum DevicesAction {
    /// Register connected iOS devices on the portal (if not already) and
    /// track them in .strudel/devices.toml. This is the common workflow for
    /// adding a device you have plugged in.
    ///
    /// The device must be connected, unlocked, and trusted, with Developer
    /// Mode enabled (Settings > Privacy & Security > Developer Mode). The
    /// Developer Mode option appears only after the device is paired with
    /// this Mac.
    ///
    /// For a macOS target, this adds the current Mac for macOS development
    /// profiles. The UDID is read from `system_profiler`.
    ///
    /// With `--udid` and `--name`, a device is added by hand instead, such as
    /// a Mac other than this one. The device is registered for the platform
    /// of the selected target.
    ///
    /// With `--no-register`, devices are only tracked. A later profile fetch
    /// fails for a device that is not registered on the portal.
    Add {
        /// Device name or UDID to add (may be repeated; default: all
        /// connected devices)
        #[arg(long = "device")]
        devices: Vec<String>,

        /// UDID of a device to add by hand. Needs `--name`.
        #[arg(long, requires = "name", conflicts_with = "devices")]
        udid: Option<String>,

        /// Name of the device added with `--udid`
        #[arg(long, requires = "udid")]
        name: Option<String>,

        /// Track the devices without registering them on the portal
        #[arg(long)]
        no_register: bool,

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

/// Target that a device added by hand belongs to. The target platform decides
/// the portal platform of the device.
fn manual_add_target<'a>(
    project: &'a ResolvedProject,
    target: Option<&str>,
) -> Result<&'a ResolvedConfig> {
    if let Some(selector) = target {
        return project.resolve_target(selector);
    }
    match project.targets.as_slice() {
        [only] => Ok(only),
        targets => {
            let ids: Vec<&str> = targets.iter().map(|t| t.target_id.as_str()).collect();
            bail!(
                "Multiple targets; select one with --target. Its platform decides whether the \
                 device is registered as an iOS or macOS device. Available: {}",
                ids.join(", ")
            )
        },
    }
}

fn is_macos(cfg: &ResolvedConfig) -> bool {
    matches!(cfg.target_platform, ResolvedTargetPlatform::Mac(_))
}

/// What plain `devices add` adds.
#[derive(Debug)]
enum PlainAdd<'a> {
    /// Connected iOS devices.
    Ios,
    /// This Mac, for a macOS target.
    Mac(&'a ResolvedConfig),
    /// The project has iOS and macOS targets and none is selected. The user
    /// chooses between iOS devices and each macOS target.
    Choose(Vec<&'a ResolvedConfig>),
}

/// What plain `devices add` adds. A named target decides. Without one, a
/// project with only macOS targets adds this Mac, and a project with only iOS
/// targets adds iOS devices.
fn plain_add<'a>(project: &'a ResolvedProject, target: Option<&str>) -> Result<PlainAdd<'a>> {
    if let Some(selector) = target {
        let cfg = project.resolve_target(selector)?;
        return Ok(if is_macos(cfg) {
            PlainAdd::Mac(cfg)
        } else {
            PlainAdd::Ios
        });
    }
    let macs: Vec<&ResolvedConfig> = project.targets.iter().filter(|t| is_macos(t)).collect();
    let has_ios = macs.len() < project.targets.len();
    Ok(match (macs.as_slice(), has_ios) {
        ([], _) => PlainAdd::Ios,
        ([only], false) => PlainAdd::Mac(only),
        (_, false) => {
            let ids: Vec<&str> = macs.iter().map(|t| t.target_id.as_str()).collect();
            bail!(
                "Multiple macOS targets; select one with --target. Available: {}",
                ids.join(", ")
            )
        },
        _ => PlainAdd::Choose(macs),
    })
}

/// Ask whether to add iOS devices or this Mac. Returns the macOS target for
/// this Mac, or `None` for iOS devices.
fn choose_add<'a>(macs: &[&'a ResolvedConfig]) -> Result<Option<&'a ResolvedConfig>> {
    let mut options = vec!["Connected iOS devices".to_string()];
    options.extend(
        macs.iter()
            .map(|t| format!("This Mac (macOS target {})", t.target_id)),
    );
    let chosen = inquire::Select::new("Add devices for:", options.clone())
        .raw_prompt()
        .context("Choosing what to add needs a terminal. Select a target with --target.")?
        .index;
    Ok(chosen.checked_sub(1).map(|i| macs[i]))
}

impl DevicesCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        match self.command {
            None => status::devices_list(config, self.target.as_deref(), self.verbose),
            Some(DevicesAction::Add {
                devices,
                udid,
                name,
                no_register,
                dry_run,
                target,
            }) => {
                let register = !no_register;
                let project = config::load_config(config)?;
                let output = OutputFlags {
                    dry_run,
                    ..Default::default()
                };
                let macos_builder = |cfg: &ResolvedConfig| {
                    MacosBuilder::new(cfg.clone(), output, false, false, None, false, false)
                };
                if let (Some(udid), Some(name)) = (udid, name) {
                    let cfg = manual_add_target(&project, target.as_deref())?;
                    return match cfg.target_platform {
                        ResolvedTargetPlatform::Mac(_) => {
                            macos_builder(cfg)?.device_add_manual(&name, &udid, register)
                        },
                        _ => IosBuilder::new(cfg.clone(), output, false)?
                            .device_add_manual(&name, &udid, register),
                    };
                }
                let mac = match plain_add(&project, target.as_deref())? {
                    PlainAdd::Ios => None,
                    PlainAdd::Mac(cfg) => Some(cfg),
                    PlainAdd::Choose(macs) => choose_add(&macs)?,
                };
                if let Some(cfg) = mac {
                    if !devices.is_empty() {
                        bail!(
                            "`--device` selects connected iOS devices and does not apply to a \
                             macOS target. Run `strudel devices add` to add this Mac."
                        );
                    }
                    return macos_builder(cfg)?.device_add_current(register);
                }
                for_each_selected(&project, target.as_deref(), Platform::Ios, false, |cfg| {
                    let output = OutputFlags {
                        dry_run,
                        ..Default::default()
                    };
                    IosBuilder::new(cfg.clone(), output, false)?.device_add(&devices, register)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::fixtures::{resolved_ios, resolved_macos};

    fn project(targets: &[(&str, bool)]) -> ResolvedProject {
        ResolvedProject {
            targets: targets
                .iter()
                .map(|(id, mac)| {
                    let mut cfg = if *mac {
                        resolved_macos()
                    } else {
                        resolved_ios()
                    };
                    cfg.target_id = id.to_string();
                    cfg
                })
                .collect(),
        }
    }

    /// Summary of [`plain_add`]: `ios`, `mac:<id>`, or `choose:<ids>`.
    fn decision(project: &ResolvedProject, target: Option<&str>) -> String {
        match plain_add(project, target).unwrap() {
            PlainAdd::Ios => "ios".into(),
            PlainAdd::Mac(c) => format!("mac:{}", c.target_id),
            PlainAdd::Choose(macs) => {
                let ids: Vec<&str> = macs.iter().map(|c| c.target_id.as_str()).collect();
                format!("choose:{}", ids.join(","))
            },
        }
    }

    #[test]
    fn a_macos_only_project_adds_this_mac() {
        assert_eq!(decision(&project(&[("mac", true)]), None), "mac:mac");
    }

    #[test]
    fn an_ios_only_project_adds_ios_devices() {
        assert_eq!(decision(&project(&[("phone", false)]), None), "ios");
    }

    #[test]
    fn a_mixed_project_without_a_target_asks() {
        let p = project(&[("mac", true), ("phone", false)]);
        assert_eq!(decision(&p, None), "choose:mac");
    }

    #[test]
    fn a_named_target_decides() {
        let p = project(&[("mac", true), ("phone", false)]);
        assert_eq!(decision(&p, Some("mac")), "mac:mac");
        assert_eq!(decision(&p, Some("phone")), "ios");
    }

    #[test]
    fn several_macos_targets_without_ios_need_a_selector() {
        let p = project(&[("one", true), ("two", true)]);
        let err = plain_add(&p, None).unwrap_err().to_string();
        assert!(err.contains("Multiple macOS targets"), "{err}");
        assert_eq!(decision(&p, Some("two")), "mac:two");
    }
}
