//! Ephemeral keychain for a supplied `APPLE_CERTIFICATE` (the paid / CI path).
//! The keychain and its search-list entry are removed when the build finishes,
//! so a build leaves no credentials behind on the machine.

use std::fs;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as b64;
use color_print::cprintln;
use secrecy::{ExposeSecret, SecretString};

use crate::builder::{MacosBuilder, step};
use crate::shell::{Shell, ShellCommand};

#[allow(dead_code)]
impl MacosBuilder {
    /// The user's current keychain search list (absolute paths).
    fn user_keychains(&self) -> Result<Vec<String>> {
        let out = self.sh.run(&["security", "list-keychains", "-d", "user"])?;
        Ok(out
            .lines()
            .map(|l| l.trim().trim_matches('"').to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    /// Replace the user keychain search list.
    fn set_user_keychains(&self, keychains: &[String]) -> Result<()> {
        let mut cmd = ShellCommand::new("security").args(["list-keychains", "-d", "user", "-s"]);
        for keychain in keychains {
            cmd = cmd.arg(keychain.as_str());
        }
        match self.sh.run(cmd) {
            Ok(_) => Ok(()),
            Err(e) => Err(e).context("Failed to update user keychain search list"),
        }
    }

    /// If a signing certificate is provided via `APPLE_CERTIFICATE`, decode it
    /// into a throwaway keychain and add that keychain to the user search list
    /// so `codesign` can find the identity. The returned guard removes the
    /// keychain and restores the search list on drop, so a build leaves no
    /// credentials behind - useful on a fresh CI runner. When no certificate is
    /// configured (the common local case, where the identity already lives in
    /// the login keychain), this is a no-op returning `None`.
    pub(in crate::builder) fn import_certificate(&self) -> Result<Option<TempKeychain>> {
        let Some((cert_b64, cert_password)) = self.cfg.signing_cert() else {
            return Ok(None);
        };

        step("Importing signing certificate into a temporary keychain...");

        let pid = std::process::id();
        let temp_dir = tempfile::Builder::new()
            .prefix("strudel-keychain-")
            .tempdir()
            .context("Failed to create temporary directory for keychain")?;
        let keychain = temp_dir
            .path()
            .join("strudel.keychain-db")
            .to_string_lossy()
            .into_owned();
        // Locks the throwaway keychain; never leaves this process, and the
        // keychain is deleted on drop.
        let kc_pw: SecretString = format!("strudel-{pid}").into();

        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> security create-keychain -p <<redacted>> {keychain}");
            cprintln!(
                "<dim>[dry-run]</dim> decode $APPLE_CERTIFICATE ({} b64 chars) -> <<temp>>.p12",
                cert_b64.expose_secret().len()
            );
            cprintln!(
                "<dim>[dry-run]</dim> security import <<temp>>.p12 -P <<redacted>> -k {keychain}"
            );
            cprintln!(
                "<dim>[dry-run]</dim> security set-key-partition-list -S apple-tool:,apple: -k <<redacted>> {keychain}"
            );
            cprintln!(
                "<dim>[dry-run]</dim> security list-keychains -d user -s {keychain} <<existing...>>"
            );
            return Ok(Some(TempKeychain {
                sh: self.sh,
                path: keychain,
                original_list: Vec::new(),
                dry_run: true,
                _temp_dir: temp_dir,
            }));
        }

        // Decode the PKCS#12 bundle to a temp file for `security import`.
        let p12 = b64
            .decode(cert_b64.expose_secret().trim())
            .context("APPLE_CERTIFICATE is not valid base64")?;

        let p12_path = temp_dir.path().join(format!("strudel-{pid}.p12"));
        fs::write(&p12_path, &p12)
            .with_context(|| format!("Failed to write {}", p12_path.display()))?;
        let p12_str = p12_path.to_string_lossy().into_owned();

        // Snapshot the search list first so the guard can restore it.
        let original_list = self.user_keychains()?;

        self.sh.run(
            ShellCommand::new("security")
                .arg("create-keychain")
                .arg_with_secret("-p", kc_pw.clone())
                .arg(&keychain),
        )?;

        self.sh.run(&[
            "security",
            "set-keychain-settings",
            "-lut",
            "21600",
            &keychain,
        ])?;

        self.sh.run(
            ShellCommand::new("security")
                .arg("unlock-keychain")
                .arg_with_secret("-p", kc_pw.clone())
                .arg(&keychain),
        )?;

        self.sh.run(
            ShellCommand::new("security")
                .args(["import", &p12_str])
                .arg_with_secret("-P", cert_password)
                .args(["-A", "-t", "cert", "-f", "pkcs12", "-k", &keychain]),
        )?;

        // Let codesign use the imported key without an interactive prompt.
        self.sh.run(
            ShellCommand::new("security")
                .args(["set-key-partition-list", "-S", "apple-tool:,apple:", "-s"])
                .arg_with_secret("-k", kc_pw.clone())
                .arg(&keychain),
        )?;

        // Put the new keychain at the front of the search list.
        let mut list = vec![keychain.clone()];
        list.extend(original_list.iter().cloned());
        self.set_user_keychains(&list)?;

        // The decoded cert has been imported; don't leave it on disk.
        let _ = fs::remove_file(&p12_path);

        Ok(Some(TempKeychain {
            sh: self.sh,
            path: keychain,
            original_list,
            dry_run: false,
            _temp_dir: temp_dir,
        }))
    }
}

/// A throwaway keychain holding an imported signing identity. On drop it
/// restores the original keychain search list and deletes the keychain, so a
/// build never leaves credentials behind on the machine. Cleanup is
/// best-effort: we're tearing down, so failures are ignored.
pub(in crate::builder) struct TempKeychain {
    sh: Shell,
    path: String,
    original_list: Vec<String>,
    dry_run: bool,
    // Held only to keep the directory (and its cleanup on drop) alive.
    _temp_dir: tempfile::TempDir,
}

impl Drop for TempKeychain {
    fn drop(&mut self) {
        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> security delete-keychain {}",
                self.path
            );
            return;
        }
        if !self.original_list.is_empty() {
            let mut args: Vec<&str> = vec!["security", "list-keychains", "-d", "user", "-s"];
            args.extend(self.original_list.iter().map(String::as_str));
            let _ = self.sh.run(args.as_slice());
        }
        let _ = self.sh.run(&["security", "delete-keychain", &self.path]);
    }
}
