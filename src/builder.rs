//! The build pipeline driver. [`Builder`] holds the resolved config, paths, and
//! a [`Shell`], and exposes the top-level commands. The actual work is split
//! across submodules:
//!
//! - [`fs`] — dry-run-aware filesystem helpers
//! - [`steps`] — the individual pipeline stages (compile, assemble, sign, …)
//! - [`keychain`] — signing-credential preflight and certificate import

mod fs;
mod keychain;
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
    ) -> Self {
        Builder {
            paths: Paths::new(&cfg),
            sh: Shell::new(dry_run),
            dry_run,
            cfg,
            open,
            debug,
            resume,
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

    /// Build bundle only (clean → binary → assemble).
    pub fn build(&self) -> Result<()> {
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

    /// Local/dev pipeline: clean → build → assemble → sign, stopping at a
    /// signed `.app`. Uses the configured signing identity if set, otherwise
    /// signs ad-hoc (can test entitlements and the hardened runtime
    /// without a Apple Developer account, but can't be notarized).
    pub fn sign_app(&self) -> Result<()> {
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

    /// Full release pipeline: clean → binary → assemble → sign → package DMG →
    /// notarize. With `--resume`, skips the build and resumes a pending
    /// notarization instead.
    pub fn release(&self) -> Result<()> {
        if let Some(ref uuid_hint) = self.resume {
            return self.resume_notarization(uuid_hint);
        }

        self.preflight_credentials()?;
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
        self.notarize()?;

        println!();
        let app_bundle_path = cformat!("<cyan>{}</cyan>", app_bundle.display());
        let dmg_path = cformat!("<cyan>{}</cyan>", self.paths.dmg.display());
        let zip_path = cformat!("<cyan>{}</cyan>", self.paths.zip.display());
        let msg = formatdoc! {r#"
            App bundle: {app_bundle_path}
            DMG:        {dmg_path}
            Zip:        {zip_path}
        "#};
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> Dry run complete. Artifacts would be at:");
            println!("{msg}");
            let problems = self.credential_problems();
            if !problems.is_empty() {
                println!();
                cprintln!("<red>WARNING:</red> Credential problems:");
                for p in &problems {
                    cprintln!("- {p}");
                }
            }
        } else {
            cprintln!("<green>Done!</green> Distribution artifacts:");
            println!("{msg}");
        };

        self.open_app()?;
        Ok(())
    }
}
