//! The build pipeline drivers. [`BuilderCore`] holds the resolved config,
//! paths, and a [`Shell`], plus the handful of helpers shared by every
//! platform. The two platform drivers wrap it:
//!
//! - [`MacosBuilder`] — the macOS pipeline (compile, assemble, sign, notarize,
//!   package a DMG). Entry points: `bundle`, `build`, `release`.
//! - [`IosBuilder`] — the iOS pipeline (simulator, device, provisioning).
//!
//! Both deref to [`BuilderCore`], so shared state (`self.cfg`, `self.sh`,
//! `self.paths`, …) and shared helpers read the same in either driver. The
//! work is split across submodules:
//!
//! - [`fs`] — dry-run-aware filesystem helpers (on [`BuilderCore`])
//! - [`keychain`] — signing-credential preflight and certificate import
//! - [`macos`] — the macOS pipeline stages
//! - [`ios`] — the iOS pipeline stages

mod fs;
mod ios;
pub(crate) mod keychain;
mod macos;

use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use color_print::{cformat, cprintln};
use indoc::formatdoc;

use crate::config::{
    ResolvedConfig, ResolvedIosSection, ResolvedMacOsSection, ResolvedTargetPlatform,
};
use crate::paths::Paths;
use crate::shell::Shell;

/// State and helpers shared by every platform driver.
pub struct BuilderCore {
    cfg: ResolvedConfig,
    paths: Paths,
    sh: Shell,
    dry_run: bool,
    debug: bool,
}

/// The macOS build pipeline. Wraps a [`BuilderCore`] and the resolved
/// `[macos]` config section.
pub struct MacosBuilder {
    core: BuilderCore,
    macos: ResolvedMacOsSection,
    open: bool,
    resume: Option<String>,
    skip_notarization: bool,
}

/// The iOS build pipeline. Wraps a [`BuilderCore`] and the resolved `[ios]`
/// config section.
pub struct IosBuilder {
    core: BuilderCore,
    ios: ResolvedIosSection,
}

impl Deref for MacosBuilder {
    type Target = BuilderCore;
    fn deref(&self) -> &BuilderCore {
        &self.core
    }
}

impl Deref for IosBuilder {
    type Target = BuilderCore;
    fn deref(&self) -> &BuilderCore {
        &self.core
    }
}

/// Print a green progress header for a build step.
fn step(msg: &str) {
    cprintln!("\n<green>==>> {msg}</green>");
}

impl BuilderCore {
    fn new(cfg: ResolvedConfig, dry_run: bool, debug: bool) -> Self {
        BuilderCore {
            paths: Paths::new(&cfg),
            sh: Shell::new(dry_run),
            dry_run,
            debug,
            cfg,
        }
    }

    /// Locate the binary for `target_name` in the swift build output dir. In
    /// dry-run, returns the expected path without checking the filesystem.
    /// On a real run with the binary missing, emits a hint listing the
    /// executables that *were* built, so users can fix `target_name`.
    pub fn find_binary_in(&self, bin_dir: &Path, target_name: &str) -> Result<PathBuf> {
        let binary_path = bin_dir.join(target_name);
        if self.dry_run {
            return Ok(binary_path);
        }
        if binary_path.exists() {
            return Ok(binary_path);
        }

        // The rest of this function is only for the error message when the binary is
        // missing on a real run.

        // Collect extension-free filenames (i.e. executables) for the error hint.
        let found: Vec<String> = std::fs::read_dir(bin_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                (e.file_type().ok()?.is_file() && !name.contains('.')).then_some(name)
            })
            .collect();
        let hint = if found.is_empty() {
            "No executables were found in the build directory.".to_string()
        } else {
            formatdoc! {r#"
                Executables found in the build directory: {}.
                If one of these is the right binary, set the matching `target_name` in your strudel.toml.
                "#,
                found.join(", ")
            }
        };
        bail!(formatdoc! {r#"
            Could not locate built binary at:
            {}
            strudel was looking for an executable named `{target_name}`.
            {hint}
            "#,
            binary_path.display(),
        });
    }

    /// User-facing clean: wipe the strudel output dir and run `swift package
    /// clean`. Platform-agnostic - macOS and iOS targets both build into
    /// `build_dir`, so the same cleanup applies to either.
    pub fn clean_command(&self) -> Result<()> {
        let source = self.cfg.source_dir.to_str().unwrap();
        let build_dir = &self.paths.build_dir;

        if build_dir.as_os_str().is_empty() || build_dir == Path::new("/") {
            anyhow::bail!("build_dir is empty or root, refusing to clean");
        }

        step("Cleaning strudel output...");
        let prefix = if self.dry_run { "[dry-run] " } else { "" };
        cprintln!("<dim>{prefix}rm -rf {}</dim>", build_dir.display());
        if !self.dry_run && build_dir.exists() {
            std::fs::remove_dir_all(build_dir)?;
        }

        step("Cleaning Swift build cache...");
        self.sh
            .run(&["swift", "package", "clean", "--package-path", source])?;

        println!();
        cprintln!("<green>Done!</green>");
        Ok(())
    }
}

/// Clean a single target's build output. Platform-agnostic, so `strudel
/// clean` can tidy macOS and iOS targets alike without picking a driver.
pub fn clean(cfg: ResolvedConfig, dry_run: bool) -> Result<()> {
    BuilderCore::new(cfg, dry_run, false).clean_command()
}

impl MacosBuilder {
    pub fn new(
        cfg: ResolvedConfig,
        dry_run: bool,
        open: bool,
        debug: bool,
        resume: Option<String>,
        skip_notarization: bool,
    ) -> Result<Self> {
        let ResolvedTargetPlatform::Mac(ref macos) = cfg.target_platform else {
            bail!("MacosBuilder constructed for a non-macOS target");
        };
        let macos = macos.clone();
        Ok(MacosBuilder {
            core: BuilderCore::new(cfg, dry_run, debug),
            macos,
            open,
            resume,
            skip_notarization,
        })
    }

    /// Assemble every configured app extension under
    /// `<app>.app/Contents/PlugIns/`. No-op when no extensions are configured.
    fn assemble_extensions(&self, bin_dir: &Path) -> Result<()> {
        for (ext, ext_paths) in self.cfg.extensions.iter().zip(self.paths.extensions.iter()) {
            self.assemble_appex(ext, ext_paths, bin_dir)?;
        }
        Ok(())
    }

    fn open_app(&self) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        if self.open {
            let app_bundle = self.paths.app_bundle.to_str().unwrap();
            self.sh.run(&["open", app_bundle])?;
        }
        Ok(())
    }

    /// Build bundle only (clean -> binary -> assemble).
    pub fn bundle(&self) -> Result<()> {
        self.clean()?;
        let bin_dir = self.build_binary()?;
        let host_binary = self.find_binary_in(&bin_dir, &self.cfg.target_name)?;
        let app_bundle = self.assemble_bundle(&host_binary)?;
        self.embed_libraries(&app_bundle)?;
        self.assemble_extensions(&bin_dir)?;
        println!();
        cprintln!("<green>Done! App bundle:</green>");
        cprintln!("<cyan>{}</cyan>", app_bundle.display());
        self.open_app()?;
        Ok(())
    }

    /// Local/dev pipeline: clean -> build -> assemble -> sign, stopping at a
    /// signed `.app`. Uses the configured signing identity if set, otherwise
    /// signs ad-hoc (can test entitlements and the hardened runtime
    /// without an Apple Developer account, but can't be notarized).
    pub fn build(&self) -> Result<()> {
        // No-op unless APPLE_CERTIFICATE is set; supports signing with an
        // imported Developer ID identity here too, but ad-hoc needs nothing.
        // let _keychain = self.import_certificate()?;
        self.clean()?;
        let bin_dir = self.build_binary()?;
        let host_binary = self.find_binary_in(&bin_dir, &self.cfg.target_name)?;
        let app_bundle = self.assemble_bundle(&host_binary)?;
        self.embed_libraries(&app_bundle)?;
        self.assemble_extensions(&bin_dir)?;
        self.sign(false)?;

        println!();
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> Dry run complete. Signed app bundle would be at:");
        } else {
            println!("Done! Signed app bundle:");
        };
        cprintln!("<cyan>{}</cyan>", app_bundle.display());
        self.open_app()?;
        Ok(())
    }

    /// Full release pipeline: clean -> binary -> assemble -> sign -> package
    /// DMG -> notarize. With `--resume`, skips the build and resumes a
    /// pending notarization instead. With `--skip-notarization`, stops
    /// after packaging the DMG.
    pub fn release(&self) -> Result<()> {
        if let Some(ref uuid_hint) = self.resume {
            return self.resume_notarization(uuid_hint);
        }

        if !self.skip_notarization {
            self.preflight_credentials()?;
        }
        // Held for the whole build: the imported identity must remain available
        // to both `sign` and the DMG signing in `package_dmg`. Dropped at the
        // end of this function, which tears the temporary keychain back down.
        // let _keychain = self.import_certificate()?;
        self.clean()?;
        let bin_dir = self.build_binary()?;
        let host_binary = self.find_binary_in(&bin_dir, &self.cfg.target_name)?;
        let app_bundle = self.assemble_bundle(&host_binary)?;
        self.embed_libraries(&app_bundle)?;
        self.assemble_extensions(&bin_dir)?;
        self.sign(true)?;
        self.package_dmg()?;

        if !self.skip_notarization {
            self.notarize()?;
        }

        println!();
        let app_bundle_path = cformat!("<cyan>{}</cyan>", app_bundle.display());
        let dmg_path = cformat!("<cyan>{}</cyan>", self.paths.dmg.display());
        let msg = formatdoc! {r#"
            App bundle: {app_bundle_path}
            DMG:        {dmg_path}
        "#};
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> Dry run complete. Artifacts would be at:");
            println!("{msg}");
            if !self.skip_notarization {
                let problems = self.credential_problems();
                if !problems.is_empty() {
                    println!();
                    cprintln!("<red>WARNING:</red> Credential problems:");
                    for p in &problems {
                        cprintln!("- {p}");
                    }
                }
            }
        } else if self.skip_notarization {
            cprintln!("<green>Done!</green> DMG built (notarization skipped):");
            println!("{msg}");
        } else {
            cprintln!("<green>Done!</green> Distribution artifacts:");
            println!("{msg}");
        };

        self.open_app()?;
        Ok(())
    }
}

impl IosBuilder {
    pub fn new(cfg: ResolvedConfig, dry_run: bool, debug: bool) -> Result<Self> {
        let ResolvedTargetPlatform::Ios(ref ios) = cfg.target_platform else {
            bail!("IosBuilder constructed for a non-iOS target");
        };
        let ios = ios.clone();
        Ok(IosBuilder {
            core: BuilderCore::new(cfg, dry_run, debug),
            ios,
        })
    }
}
