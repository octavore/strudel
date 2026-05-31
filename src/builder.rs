//! The build pipeline driver. [`Builder`] holds the resolved config, paths, and
//! a [`Shell`], and exposes the top-level commands. The actual work is split
//! across submodules:
//!
//! - [`fs`] — dry-run-aware filesystem helpers
//! - [`steps`] — the individual pipeline stages (compile, assemble, sign, …)
//! - [`keychain`] — signing-credential preflight and certificate import

mod fs;
mod keychain;
mod steps;
mod validators;

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
    open: bool,
}

/// Print a green progress header for a build step.
fn step(msg: &str) {
    cprintln!("\n<green>==>> {msg}</green>");
}

impl Builder {
    pub fn new(cfg: ResolvedConfig, dry_run: bool, open: bool) -> Self {
        Builder {
            paths: Paths::new(&cfg),
            sh: Shell::new(dry_run),
            cfg,
            open,
        }
    }

    fn dry_run(&self) -> bool {
        self.sh.dry_run
    }

    fn open_app(&self) -> Result<()> {
        if self.dry_run() {
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
        let binary_path = self.build_binary()?;
        let app_bundle = self.assemble_bundle(&binary_path)?;
        self.embed_libraries(&app_bundle)?;
        println!();
        cprintln!("<green>Done! App bundle:</green>");
        cprintln!("<cyan>{}</cyan>", app_bundle.display());
        self.open_app()?;
        Ok(())
    }

    /// Local/dev pipeline: clean → build → assemble → sign, stopping at a
    /// signed `.app`. No notarization or DMG, and no notary credentials
    /// required. Uses the configured signing identity if set, otherwise
    /// signs ad-hoc — enough to test entitlements and the hardened runtime
    /// without a Developer ID certificate or an Apple account.
    pub fn sign_app(&self) -> Result<()> {
        // No-op unless APPLE_CERTIFICATE is set; supports signing with an
        // imported Developer ID identity here too, but ad-hoc needs nothing.
        // let _keychain = self.import_certificate()?;
        self.clean()?;
        let binary_path = self.build_binary()?;
        let app_bundle = self.assemble_bundle(&binary_path)?;
        self.embed_libraries(&app_bundle)?;
        self.sign()?;

        println!();
        if self.dry_run() {
            cprintln!("<dim>[dry-run]</dim> Dry run complete. Signed app bundle would be at:");
        } else {
            println!("Done! Signed app bundle:");
        };
        cprintln!("<cyan>{}</cyan>", app_bundle.display());
        self.open_app()?;
        Ok(())
    }

    /// Full release pipeline: clean → binary → assemble → sign → notarize →
    /// DMG.
    pub fn release(&self) -> Result<()> {
        self.preflight_credentials()?;
        // Held for the whole build: the imported identity must remain available
        // to both `sign` and the DMG signing in `package_dmg`. Dropped at the
        // end of this function, which tears the temporary keychain back down.
        // let _keychain = self.import_certificate()?;
        self.clean()?;
        let binary_path = self.build_binary()?;
        let app_bundle = self.assemble_bundle(&binary_path)?;
        self.embed_libraries(&app_bundle)?;
        self.sign()?;
        self.notarize()?;
        self.package_dmg()?;

        println!();
        let app_bundle_path = cformat!("<cyan>{}</cyan>", app_bundle.display());
        let dmg_path = cformat!("<cyan>{}</cyan>", self.paths.dmg.display());
        let zip_path = cformat!("<cyan>{}</cyan>", self.paths.zip.display());
        let msg = formatdoc! {r#"
            App bundle: {app_bundle_path}
            DMG:        {dmg_path}
            Zip:        {zip_path}
        "#};
        if self.dry_run() {
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
