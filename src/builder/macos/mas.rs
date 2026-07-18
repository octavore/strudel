//! Mac App Store channel packaging and upload: [`MacosBuilder::package_pkg`]
//! and [`MacosBuilder::upload_pkg`]. Selected by `strudel release --mas`.

use std::fs;

use anyhow::{Context, Result, anyhow, bail};
use color_print::cprintln;

use crate::builder::{MacosBuilder, step};
use crate::shell::ShellCommand;

impl MacosBuilder {
    pub fn package_pkg(&self) -> Result<()> {
        step("Packaging pkg...");
        self.validate_installer_identity()?;

        let app_bundle_str = self.paths.app_bundle.to_str().unwrap();
        let pkg_str = self.paths.pkg.to_str().unwrap();
        let installer_identity = self
            .cfg
            .mas_installer_identity
            .as_deref()
            .unwrap_or("MISSING!");

        if !self.dry_run
            && let Some(parent) = self.paths.pkg.parent()
        {
            fs::create_dir_all(parent)?;
        }

        self.sh.run(&[
            "productbuild",
            "--component",
            app_bundle_str,
            "/Applications",
            "--sign",
            installer_identity,
            pkg_str,
        ])?;
        Ok(())
    }

    pub fn upload_pkg(&self) -> Result<()> {
        step("Uploading pkg to App Store Connect...");

        let apple_id = self.cfg.mas_app_apple_id.as_deref().unwrap_or("MISSING!");
        let pkg_str = self.paths.pkg.to_str().unwrap();

        let auth = match self.cfg.notary_auth() {
            Some(auth) => auth,
            None if self.dry_run => {
                cprintln!(
                    "<red>Error: No App Store Connect API key configured (used for altool \
                     upload too).</red>"
                );
                crate::config::NotaryAuth {
                    key_path: "MISSING!".into(),
                    key_id: "MISSING!".into(),
                    issuer: Some("MISSING!".into()),
                }
            },
            None => bail!("No App Store Connect API key configured (used for altool upload too)"),
        };
        let issuer = auth.issuer.clone().ok_or_else(|| {
            anyhow!(
                "altool upload requires an API issuer (APPLE_API_ISSUER / apple.api_issuer), in \
                 addition to the key id and key path."
            )
        })?;

        // altool has no flag to point at an arbitrary .p8 file directly: it
        // discovers `AuthKey_<key_id>.p8` by searching a handful of fixed
        // directories (or `API_PRIVATE_KEYS_DIR`). Stage a correctly-named
        // copy so a configured key file at any name/location still works.
        let keys_dir = self.paths.strudel_dir.join("altool-keys");
        let staged_key = keys_dir.join(format!("AuthKey_{}.p8", auth.key_id));
        if !self.dry_run {
            fs::create_dir_all(&keys_dir)?;
            fs::copy(&auth.key_path, &staged_key).with_context(|| {
                format!(
                    "Failed to stage API key {} -> {}",
                    auth.key_path.display(),
                    staged_key.display()
                )
            })?;
        }

        let cmd = ShellCommand::new("xcrun")
            .args([
                "altool",
                "--upload-package",
                pkg_str,
                "-t",
                "macos",
                "--apple-id",
                apple_id,
                "--bundle-id",
                self.cfg.bundle_id.as_str(),
                "--bundle-version",
                self.cfg.build_number.as_str(),
                "--bundle-short-version-string",
                self.cfg.version.as_str(),
                "--api-key",
                auth.key_id.as_str(),
                "--api-issuer",
                issuer.as_str(),
                "--output-format",
                "json",
            ])
            .envs(
                &[(
                    "API_PRIVATE_KEYS_DIR".to_string(),
                    keys_dir.to_string_lossy().into_owned(),
                )]
                .into_iter()
                .collect(),
            );

        let out = self.sh.run(cmd)?;
        if !out.trim().is_empty() {
            println!("{out}");
        }
        Ok(())
    }
}
