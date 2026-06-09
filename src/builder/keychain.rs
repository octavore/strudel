//! Signing-credential handling: preflight checks for required credentials, and
//! importing an `APPLE_CERTIFICATE` into a throwaway keychain that is torn down
//! when the build finishes, so nothing is left on the machine.

use std::fs;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as b64;
use color_print::cprintln;
use secrecy::{ExposeSecret, SecretString};

use super::{Builder, step};
use crate::shell::{Shell, ShellCommand};

impl Builder {
    /// Describe any signing/notarization credentials that are missing or
    /// incomplete. Empty means a real `run` has everything it needs.
    pub(super) fn credential_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.cfg.sign_identity.is_empty() {
            problems.push("APPLE_SIGNING_IDENTITY (signing identity) is not set".to_string());
        }
        if self.cfg.notary_auth().is_none() {
            problems.push(
                "no complete notarization credentials. Provide the App Store Connect API key \
                    (APPLE_API_KEY_PATH, APPLE_API_KEY, APPLE_API_ISSUER)."
                    .to_string(),
            );
        }
        problems
    }

    /// Verify the credentials required for signing and notarization are
    /// present. Bails early so a missing value doesn't surface deep into
    /// the pipeline (e.g. `codesign: no identity found`). In dry-run, only
    /// warns — there's nothing to sign.
    pub(super) fn preflight_credentials(&self) -> Result<()> {
        let problems = self.credential_problems();
        if problems.is_empty() {
            return Ok(());
        }

        let hint = "Set identifiers in strudel.toml or the environment, and \
                    secrets (passwords, certificate) in the environment only. \
                    See the README's \"Signing & notarization\" section.";

        if self.dry_run {
            for p in &problems {
                cprintln!("<yellow>[warning]</yellow> {p}");
            }
            cprintln!("<yellow>[warning]</yellow> {hint}");
            Ok(())
        } else {
            let mut msg = String::from("Cannot run signing/notarization:");
            for p in &problems {
                msg.push_str(&format!("\n  - {p}"));
            }
            msg.push_str(&format!("\n{hint}"));
            bail!(msg);
        }
    }

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

    #[allow(dead_code)]
    /// If a signing certificate is provided via `APPLE_CERTIFICATE`, decode it
    /// into a throwaway keychain and add that keychain to the user search list
    /// so `codesign` can find the identity. The returned guard removes the
    /// keychain and restores the search list on drop, so a build leaves no
    /// credentials behind — useful on a fresh CI runner. When no certificate is
    /// configured (the common local case, where the identity already lives in
    /// the login keychain), this is a no-op returning `None`.
    pub(super) fn import_certificate(&self) -> Result<Option<TempKeychain<'_>>> {
        let Some((cert_b64, cert_password)) = self.cfg.signing_cert() else {
            return Ok(None);
        };

        step("Importing signing certificate into a temporary keychain...");

        let pid = std::process::id();
        let keychain = std::env::temp_dir()
            .join(format!("strudel-{pid}.keychain-db"))
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
                sh: &self.sh,
                path: keychain,
                original_list: Vec::new(),
                dry_run: true,
            }));
        }

        // Decode the PKCS#12 bundle to a temp file for `security import`.
        let p12 = b64
            .decode(cert_b64.expose_secret().trim())
            .context("APPLE_CERTIFICATE is not valid base64")?;

        let p12_path = std::env::temp_dir().join(format!("strudel-{pid}.p12"));
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
            sh: &self.sh,
            path: keychain,
            original_list,
            dry_run: false,
        }))
    }
}

/// A throwaway keychain holding an imported signing identity. On drop it
/// restores the original keychain search list and deletes the keychain, so a
/// build never leaves credentials behind on the machine. Cleanup is
/// best-effort: we're tearing down, so failures are ignored.
pub(super) struct TempKeychain<'a> {
    sh: &'a Shell,
    path: String,
    original_list: Vec<String>,
    dry_run: bool,
}

impl Drop for TempKeychain<'_> {
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::config::ResolvedConfig;

    fn empty_cfg() -> ResolvedConfig {
        ResolvedConfig {
            app_name: "A".into(),
            bundle_id: "b".into(),
            version: "1".into(),
            build_number: "1".into(),
            source_dir: PathBuf::from("/x"),
            build_dir: PathBuf::from("/x"),
            info_json_path: None,
            entitlements_json_path: None,
            icon_path: None,
            archs: vec!["arm64".into()],
            target_name: "A".into(),
            sign_identity: String::new(),
            notarize_timeout: 600,
            build_env: HashMap::new(),
            embed_libs: Vec::new(),
            provisioning_profile: None,
            extensions: Vec::new(),
            team_id: String::new(),
            apple_api_issuer: String::new(),
            apple_api_key: String::new(),
            apple_api_key_path: None,
            apple_certificate: String::new().into(),
            apple_certificate_password: String::new().into(),
            resources_dir: None,
            resources: Vec::new(),
        }
    }

    fn builder(cfg: ResolvedConfig) -> Builder {
        Builder::new(cfg, true, false, false, None)
    }

    #[test]
    fn problems_reports_missing_identity_and_notary() {
        let b = builder(empty_cfg());
        let problems = b.credential_problems();
        assert_eq!(problems.len(), 2);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("APPLE_SIGNING_IDENTITY"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("notarization credentials"))
        );
    }

    #[test]
    fn problems_empty_when_api_key_set() {
        let mut cfg = empty_cfg();
        cfg.sign_identity = "Developer ID Application: X (TEAM)".into();
        cfg.apple_api_key_path = Some(PathBuf::from("/k.p8"));
        cfg.apple_api_key = "KID".into();
        cfg.apple_api_issuer = "ISS".into();
        assert!(builder(cfg).credential_problems().is_empty());
    }

    #[test]
    fn preflight_warns_but_passes_in_dry_run() {
        // Dry-run must not bail: a missing-credential dry-run is the user
        // explicitly checking what `release` would do — they shouldn't have to
        // populate every secret just to preview the pipeline.
        let b = builder(empty_cfg());
        b.preflight_credentials()
            .expect("dry-run preflight must succeed even without credentials");
    }
}
