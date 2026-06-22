//! The build pipeline driver. [`Builder`] holds the resolved config, paths, and
//! a [`Shell`], and exposes the top-level commands. The actual work is split
//! across submodules:
//!
//! - [`fs`] — dry-run-aware filesystem helpers
//! - [`steps`] — the individual pipeline stages (compile, assemble, sign, …)
//! - [`keychain`] — signing-credential preflight and certificate import

mod fs;
mod ios;
pub(crate) mod keychain;
mod notarize;
mod steps;
mod validators;

use std::path::Path;

use anyhow::Result;
use color_print::{cformat, cprintln};
use indoc::formatdoc;

use crate::config::ResolvedConfig;
use crate::paths::Paths;
use crate::shell::Shell;

pub struct Builder {
    cfg: ResolvedConfig,
    paths: Paths,
    sh: Shell,
    dry_run: bool,
    open: bool,
    debug: bool,
    resume: Option<String>,
    skip_notarization: bool,
}

/// Print a green progress header for a build step.
fn step(msg: &str) {
    cprintln!("\n<green>==>> {msg}</green>");
}

impl Builder {
    pub fn new(
        cfg: ResolvedConfig,
        dry_run: bool,
        open: bool,
        debug: bool,
        resume: Option<String>,
        skip_notarization: bool,
    ) -> Self {
        Builder {
            paths: Paths::new(&cfg),
            sh: Shell::new(dry_run),
            dry_run,
            cfg,
            open,
            debug,
            resume,
            skip_notarization,
        }
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

    /// User-facing clean: wipe the strudel output dir and run `swift package clean`.
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
