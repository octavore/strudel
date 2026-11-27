use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use crate::apple::appstore::AppStoreClient;
use crate::builder::{IosBuilder, MacosBuilder, OutputFlags};
use crate::config::{self, Platform, ResolvedConfig, ResolvedProject, ResolvedTargetPlatform};
use crate::status;

#[derive(clap::Args)]
pub(crate) struct DevicesCmd {
    #[command(subcommand)]
    command: Option<DevicesAction>,

    /// Select a target by id
    #[arg(long)]
    target: Option<String>,

    /// Also list connected devices and portal devices that are not tracked
    #[arg(long)]
    all: bool,

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
    /// With `--udid`, a device is added by hand instead, such as a Mac other
    /// than this one. Without `--name`, the name is read from the portal.
    ///
    /// A project with both iOS and macOS targets needs `--platform` for
    /// `--udid`, and asks which platform to add otherwise. The first target of
    /// the platform supplies the portal account.
    ///
    /// With `--no-register`, devices are only tracked. A later profile fetch
    /// fails for a device that is not registered on the portal.
    Add {
        /// Device name or UDID to add (may be repeated; default: all
        /// connected devices)
        #[arg(long = "device")]
        devices: Vec<String>,

        /// UDID of a device to add by hand
        #[arg(long, conflicts_with = "devices")]
        udid: Option<String>,

        /// Name of the device added with `--udid`. Default: the name on the
        /// portal.
        #[arg(long, requires = "udid")]
        name: Option<String>,

        /// Track the devices without registering them on the portal
        #[arg(long)]
        no_register: bool,

        /// Print commands without executing them
        #[arg(long)]
        dry_run: bool,

        /// Platform to add devices for
        #[arg(long, value_enum)]
        platform: Option<Platform>,
    },
    /// Register a single device on the portal by UDID, without tracking it in
    /// .strudel/devices.toml. Use this to register a device you don't have
    /// connected (e.g. a teammate's); use `devices add` otherwise.
    ///
    /// A project with both iOS and macOS targets needs `--platform`. The first
    /// target of the platform supplies the portal account.
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

        /// Platform to register the device for
        #[arg(long, value_enum)]
        platform: Option<Platform>,
    },
}

fn macos_builder(cfg: &ResolvedConfig, output: OutputFlags) -> Result<MacosBuilder> {
    MacosBuilder::new(cfg.clone(), output, false, false, None, false, false)
}

/// First target of `platform`. It supplies the portal account.
fn first_target(project: &ResolvedProject, platform: Platform) -> Option<&ResolvedConfig> {
    project
        .targets
        .iter()
        .find(|t| t.target_platform.platform() == platform)
}

/// Name of the device with `udid` on the portal of `cfg`'s platform.
fn portal_device_name(cfg: &ResolvedConfig, udid: &str) -> Result<String> {
    let devices: Vec<(String, String)> = if is_macos(cfg) {
        AppStoreClient::from_config(cfg)?
            .list_devices_on(Platform::Macos)?
            .into_iter()
            .map(|d| (d.name, d.udid))
            .collect()
    } else {
        IosBuilder::new(cfg.clone(), OutputFlags::default(), false)?
            .portal_devices()?
            .into_iter()
            .map(|d| (d.name, d.udid))
            .collect()
    };
    devices
        .into_iter()
        .find(|(_, u)| u == udid)
        .map(|(name, _)| name)
        .with_context(|| format!("Device {udid} is not on the portal. Pass --name."))
}

fn is_macos(cfg: &ResolvedConfig) -> bool {
    matches!(cfg.target_platform, ResolvedTargetPlatform::Mac(_))
}

/// Platform that `devices add` adds devices for. `platform` decides. Without
/// it, the only platform in the project decides. Returns `None` if the project
/// has iOS and macOS targets, so the platform is ambiguous.
fn add_platform(project: &ResolvedProject, platform: Option<Platform>) -> Result<Option<Platform>> {
    if platform.is_some() {
        return Ok(platform);
    }
    let present: Vec<Platform> = [Platform::Ios, Platform::Macos]
        .into_iter()
        .filter(|p| first_target(project, *p).is_some())
        .collect();
    match present.as_slice() {
        [] => bail!("No targets in strudel.toml"),
        [only] => Ok(Some(*only)),
        _ => Ok(None),
    }
}

/// Platform of a device given by UDID. Fails if the platform is ambiguous.
fn manual_platform(project: &ResolvedProject, platform: Option<Platform>) -> Result<Platform> {
    add_platform(project, platform)?
        .context("The project has iOS and macOS targets. Select one with --platform.")
}

/// Ask whether to add connected iOS devices or this Mac.
fn choose_platform() -> Result<Platform> {
    let options = vec!["Connected iOS devices", "This Mac"];
    let chosen = inquire::Select::new("Add devices for:", options)
        .raw_prompt()
        .context("Choosing what to add needs a terminal. Select a platform with --platform.")?
        .index;
    Ok(if chosen == 0 {
        Platform::Ios
    } else {
        Platform::Macos
    })
}

impl DevicesCmd {
    pub(crate) fn execute(self, config: &Path) -> Result<()> {
        match self.command {
            None => status::devices_list(config, self.target.as_deref(), self.all, self.verbose),
            Some(DevicesAction::Add {
                devices,
                udid,
                name,
                no_register,
                dry_run,
                platform,
            }) => {
                let register = !no_register;
                let project = config::load_config(config)?;
                let output = OutputFlags {
                    dry_run,
                    ..Default::default()
                };
                let platform = if udid.is_some() {
                    manual_platform(&project, platform)?
                } else {
                    match add_platform(&project, platform)? {
                        Some(platform) => platform,
                        None => choose_platform()?,
                    }
                };
                let cfg = first_target(&project, platform)
                    .with_context(|| format!("No {} targets in strudel.toml", platform.label()))?;
                if let Some(udid) = udid {
                    let name = match name {
                        Some(name) => name,
                        None => portal_device_name(cfg, &udid)?,
                    };
                    return match platform {
                        Platform::Macos => {
                            macos_builder(cfg, output)?.device_add_manual(&name, &udid, register)
                        },
                        Platform::Ios => IosBuilder::new(cfg.clone(), output, false)?
                            .device_add_manual(&name, &udid, register),
                    };
                }
                match platform {
                    Platform::Macos => {
                        if !devices.is_empty() {
                            bail!(
                                "`--device` selects connected iOS devices and does not apply to \
                                 macOS. Run `strudel devices add` to add this Mac."
                            );
                        }
                        macos_builder(cfg, output)?.device_add_current(register)
                    },
                    Platform::Ios => {
                        IosBuilder::new(cfg.clone(), output, false)?.device_add(&devices, register)
                    },
                }
            },
            Some(DevicesAction::Register {
                udid,
                name,
                dry_run,
                platform,
            }) => {
                let project = config::load_config(config)?;
                let output = OutputFlags {
                    dry_run,
                    ..Default::default()
                };
                let platform = manual_platform(&project, platform)?;
                let cfg = first_target(&project, platform)
                    .with_context(|| format!("No {} targets in strudel.toml", platform.label()))?;
                match platform {
                    Platform::Macos => macos_builder(cfg, output)?.device_register(&name, &udid),
                    Platform::Ios => {
                        IosBuilder::new(cfg.clone(), output, false)?.device_register(&name, &udid)
                    },
                }
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

    #[test]
    fn a_macos_only_project_adds_macos() {
        let p = project(&[("mac", true)]);
        assert_eq!(add_platform(&p, None).unwrap(), Some(Platform::Macos));
    }

    #[test]
    fn an_ios_only_project_adds_ios() {
        let p = project(&[("phone", false)]);
        assert_eq!(add_platform(&p, None).unwrap(), Some(Platform::Ios));
    }

    #[test]
    fn several_targets_of_one_platform_are_not_ambiguous() {
        let p = project(&[("one", true), ("two", true)]);
        assert_eq!(add_platform(&p, None).unwrap(), Some(Platform::Macos));
        assert_eq!(first_target(&p, Platform::Macos).unwrap().target_id, "one");
    }

    #[test]
    fn a_mixed_project_without_a_platform_is_ambiguous() {
        let p = project(&[("mac", true), ("phone", false)]);
        assert_eq!(add_platform(&p, None).unwrap(), None);
    }

    #[test]
    fn a_given_platform_decides() {
        let p = project(&[("mac", true), ("phone", false)]);
        assert_eq!(
            add_platform(&p, Some(Platform::Ios)).unwrap(),
            Some(Platform::Ios)
        );
        assert_eq!(first_target(&p, Platform::Ios).unwrap().target_id, "phone");
    }
}
