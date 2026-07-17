//! Core pipeline steps: [`MacosBuilder::clean`],
//! [`MacosBuilder::build_binary`], [`MacosBuilder::embed_libraries`],
//! [`MacosBuilder::package_dmg`], and [`MacosBuilder::notarize`].

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use dmg::DmgSpec;
use serde_json::Value;

use crate::builder::macos::notarize::NotarizationState;
use crate::builder::{MacosBuilder, step};
use crate::shell::ShellCommand;

impl MacosBuilder {
    pub fn clean(&self) -> Result<()> {
        step("Cleaning previous build...");
        let build_dir = &self.paths.build_dir;
        if build_dir.as_os_str().is_empty() || build_dir == Path::new("/") {
            bail!("build_dir is empty or root, refusing to clean");
        }

        if self.dry_run {
            let build_dir = build_dir.display();
            cprintln!("<dim>[dry-run]</dim> rm -rf {build_dir}");
            cprintln!("<dim>[dry-run]</dim> mkdir -p {build_dir}");
            return Ok(());
        }

        if build_dir.exists() {
            fs::remove_dir_all(build_dir)?;
        }
        fs::create_dir_all(build_dir)?;
        Ok(())
    }

    /// `swift build` wrapper, which also adds rpath if there are embedded
    /// libraries. Returns the swift build bin directory which contains the host
    /// binary and any extension binaries. Use [`Self::find_binary_in`] to
    /// locate a specific target's binary.
    pub fn build_binary(&self) -> Result<PathBuf> {
        let config_flag = if self.debug { "debug" } else { "release" };
        step(&format!("Building {config_flag} binary..."));

        // Build base args shared between both swift invocations
        let source = self.cfg.source_dir.to_str().unwrap();
        let mut build_cmd = ShellCommand::new("swift")
            .args(["build", "-c", config_flag, "--package-path", source])
            .envs(&self.cfg.build_env);

        // add archs from cfg
        for arch in &self.cfg.archs {
            build_cmd = build_cmd.args(["--arch", arch]);
        }

        // embed the Frameworks rpath at link time if we're embedding libraries
        if !self.cfg.embed_libs.is_empty() {
            build_cmd = build_cmd.arg_group([
                "-Xlinker",
                "-rpath",
                "-Xlinker",
                "@executable_path/../Frameworks",
            ]);
        }

        // clone of the build command with --show-bin-path to find the binary after
        // building
        let show_bin_cmd = build_cmd.clone().arg("--show-bin-path").hide_dry_run();

        // run the build_cmd
        self.sh.run_streamed_env(build_cmd)?;

        // run show_bin_cmd to find the swift build output directory
        let bin_dir = show_bin_cmd.run(&self.sh)?;
        let bin_dir = bin_dir.trim();
        let bin_dir = if bin_dir.is_empty() {
            // dry-run: fall back to default swift location
            self.cfg.source_dir.join(format!(".build/{config_flag}"))
        } else {
            PathBuf::from(bin_dir)
        };
        Ok(bin_dir)
    }

    /// Embed dynamic libraries and `.framework` bundles into
    /// `Contents/Frameworks`, fix dylib install names and the executable's
    /// rpath, so the bundle is self-contained. No-op when `cfg.embed_libs`
    /// is empty.
    pub fn embed_libraries(&self, app_bundle: &Path) -> Result<()> {
        if self.cfg.embed_libs.is_empty() {
            return Ok(());
        }

        step("Embedding libraries and frameworks...");

        let frameworks_dir = app_bundle.join("Contents/Frameworks");
        self.create_dir(&frameworks_dir)?;

        let executable = app_bundle.join("Contents/MacOS").join(&self.cfg.app_name);
        let executable_str = executable
            .to_str()
            .context("embed: Invalid executable path.")?;

        for lib_path in &self.cfg.embed_libs {
            let file_name = lib_path.file_name().with_context(|| {
                format!("embed_libs entry has no filename: {}", lib_path.display())
            })?;
            let dest = frameworks_dir.join(file_name);

            if is_framework(lib_path) {
                // `.framework` bundles (e.g. SwiftPM binaryTargets like
                // Sparkle) are directory bundles already linked with an
                // @rpath install name; copy the tree (via `ditto`, which
                // preserves the Versions/... symlink structure) and skip
                // the install-name rewrite done below for flat dylibs.
                self.copy_tree(lib_path, &dest)?;
                continue;
            }

            let file_name_str = file_name.to_str().context("embed: Invalid file name.")?;
            let rpath_entry = format!("@rpath/{file_name_str}");
            self.copy_file(lib_path, &dest)?;

            // Find the original install name as seen by the executable.
            let otool_out = self.sh.run(&["otool", "-L", executable_str])?;
            let orig_install_name = if self.dry_run {
                // In dry-run we can't run otool; use the filename as a stand-in.
                format!("<otool:{file_name_str}>")
            } else {
                otool_out
                    .lines()
                    .skip(1)
                    .map(|l| l.split_whitespace().next().unwrap_or(""))
                    .find(|name| {
                        Path::new(name)
                            .file_name()
                            .map(|n| n == file_name)
                            .unwrap_or(false)
                    })
                    .map(|s| s.to_string())
                    .with_context(|| {
                        format!(
                            "Could not find {file_name_str} in `otool -L {executable_str}`.\n\
                             Ensure your Package.swift links this library."
                        )
                    })?
            };

            // Update the dylib (at `dest_str`): change install name to @rpath/{dylib_name}
            let dest_str = dest.to_str().context("embed: Invalid destination path.")?;
            self.sh
                .run(&["install_name_tool", "-id", &rpath_entry, dest_str])?;

            // Updated the executable (at `executable_str`): change its reference to the
            // dylib from the `orig_install_name` to `@rpath/{dylib_name}``.
            self.sh.run(&[
                "install_name_tool",
                "-change",
                &orig_install_name,
                &rpath_entry,
                executable_str,
            ])?;
        }

        Ok(())
    }

    pub fn package_dmg(&self) -> Result<()> {
        let macos_settings = &self.macos;

        let app_name = &self.cfg.app_name;
        let vol_name = format!("{app_name} {}", self.cfg.version);
        let temp_dmg = &self.paths.strudel_temp_dmg;
        let temp_dmg_str = temp_dmg.to_str().unwrap();

        step("Creating DMG...");

        if let Some(dmg_cfg) = &macos_settings.dmg {
            if self.dry_run {
                cprintln!(
                    "<dim>[dry-run]</dim> hdiutil create -volname {:?} -format UDZO {}",
                    vol_name,
                    temp_dmg.display()
                );
                return Ok(());
            }
            fs::create_dir_all(&self.paths.strudel_dir)?;
            step("Configuring DMG window...");
            dmg::create(
                &DmgSpec {
                    vol_name,
                    app_name: app_name.clone(),
                    source_app: self.paths.app_bundle.clone(),
                    background: dmg_cfg.background.clone(),
                    window_width: dmg_cfg.window_width,
                    window_height: dmg_cfg.window_height,
                    icon_size: dmg_cfg.icon_size,
                    app_x: dmg_cfg.app_x,
                    app_y: dmg_cfg.app_y,
                    applications_x: dmg_cfg.applications_x,
                    applications_y: dmg_cfg.applications_y,
                    icon_text_size: dmg_cfg.icon_text_size,
                },
                temp_dmg,
            )?;
        } else {
            // Plain UDZO: no custom window layout, use staging folder approach.
            let staging = &self.paths.dmg_staging;
            let staging_str = staging.to_str().unwrap();
            let staging_app = staging.join(format!("{app_name}.app"));
            let staging_applications = staging.join("Applications");

            if self.dry_run {
                cprintln!("<dim>[dry-run]</dim> rm -rf {staging_str}");
                cprintln!("<dim>[dry-run]</dim> mkdir -p {staging_str}");
                cprintln!(
                    "<dim>[dry-run]</dim> ln -s /Applications {}",
                    staging_applications.display()
                );
            } else {
                fs::create_dir_all(&self.paths.strudel_dir)?;
                if staging.exists() {
                    fs::remove_dir_all(staging)?;
                }
                fs::create_dir_all(staging)?;
                std::os::unix::fs::symlink("/Applications", &staging_applications)?;
            }

            self.sh.run(&[
                "cp",
                "-rp",
                self.paths.app_bundle.to_str().unwrap(),
                staging_app.to_str().unwrap(),
            ])?;

            self.sh.run(&[
                "hdiutil",
                "create",
                "-volname",
                &vol_name,
                "-srcfolder",
                staging_str,
                "-ov",
                "-format",
                "UDZO",
                temp_dmg_str,
            ])?;

            if !self.dry_run {
                fs::remove_dir_all(staging)?;
            }
        }

        Ok(())
    }

    pub fn notarize(&self) -> Result<()> {
        let temp_dmg = &self.paths.strudel_temp_dmg;
        let temp_dmg_str = temp_dmg.to_str().unwrap();

        step("Submitting DMG for notarization...");
        cprintln!(
            "<dim>Note: first-time notarization can take several hours. \
             Press Ctrl-C to stop: run `strudel release --resume` to continue later.</dim>"
        );

        let auth_args = self.notary_auth_args()?;
        let notarize_cmd = ShellCommand::new("xcrun")
            .args(["notarytool", "submit", temp_dmg_str])
            .args(auth_args.iter().cloned())
            .args(["--output-format", "json"]);

        let submit_out = self.sh.run(notarize_cmd)?;

        let uuid = if self.dry_run {
            "dry-run-uuid-0000".to_string()
        } else {
            let v: Value = serde_json::from_str(&submit_out)
                .context("Failed to parse notarytool submit output as JSON")?;
            v["id"]
                .as_str()
                .context("notarytool submit output missing 'id' field")?
                .to_string()
        };

        cprintln!("  <dim>Submission ID: {uuid}</dim>");

        let pending = self.paths.pending_submission(&uuid);
        if !self.dry_run {
            fs::create_dir_all(&pending.dir)?;
            fs::rename(temp_dmg, &pending.dmg)?;
        } else {
            cprintln!("<dim>[dry-run]</dim> mkdir -p {}", pending.dir.display());
            cprintln!(
                "<dim>[dry-run]</dim> mv {} {}",
                temp_dmg.display(),
                pending.dmg.display()
            );
        }

        let state = NotarizationState {
            submitted_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            dmg_dest: self.paths.dmg.display().to_string(),
        };
        if !self.dry_run {
            fs::write(&pending.state, toml::to_string(&state)?)?;
        } else {
            cprintln!("<dim>[dry-run]</dim> write {}", pending.state.display());
        }

        self.poll_notarization(&uuid, &pending, &PathBuf::from(&state.dmg_dest), &auth_args)
    }
}

/// Whether an `embed_libs` entry is a `.framework` directory bundle rather
/// than a flat dylib.
pub(super) fn is_framework(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("framework")
}
