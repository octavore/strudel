use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use color_print::cprintln;

use crate::apple::provisioning::{self, ensure_keychain_ready};
use crate::builder::ios::IosTarget;
use crate::builder::ios::profile::decode_profile;
use crate::builder::keychain::parse_identity_line;
use crate::builder::{IosBuilder, step};
use crate::config::IosProvisioningBackend;
use crate::shell::ShellCommand;

impl IosBuilder {
    /// Build for one or more connected iOS devices, then install and launch.
    ///
    /// Requires devices to be registered via `strudel devices add`. Auto-
    /// fetches and caches a development provisioning profile via the App Store
    /// Connect API when one is not already current.
    pub fn device(&self, device_selectors: &[String]) -> Result<()> {
        let ios_settings = &self.ios;

        if !self.cfg.extensions.is_empty() {
            cprintln!(
                "<yellow>warning:</yellow> iOS extension bundling is not yet supported; \
                 [[extensions]] in this target will be ignored."
            );
        }
        let target = &self.cfg.target_name;
        let config_flag = if self.debug { "debug" } else { "release" };
        let deployment = &ios_settings.deployment_target;
        let triple = format!("arm64-apple-ios{deployment}");

        let sdk_path = self
            .sh
            .run(&["xcrun", "--sdk", "iphoneos", "--show-sdk-path"])
            .map(|s| {
                if s.is_empty() {
                    "<iphoneos-sdk>".into()
                } else {
                    s
                }
            })?;
        let swift = self
            .sh
            .run(&["xcrun", "-f", "swift"])
            .map(|s| if s.is_empty() { "swift".into() } else { s })?;

        step("Building for iOS device...");
        let source = self.cfg.source_dir.to_str().unwrap();
        self.sh.run_streamed_env(
            ShellCommand::new(&swift)
                .args([
                    "build",
                    "-c",
                    config_flag,
                    "--triple",
                    &triple,
                    "--sdk",
                    &sdk_path,
                    "--package-path",
                    source,
                ])
                .envs(&self.cfg.build_env),
        )?;

        let bin_dir = self.ios_bin_dir(&swift, config_flag, &triple, &sdk_path)?;
        let binary = self.find_binary_in(&bin_dir, target)?;

        step("Assembling iOS device bundle...");
        let bundle_dir = self.paths.build_dir.join("ios-device");
        let app_bundle = bundle_dir.join(format!("{target}.app"));
        self.assemble_ios_bundle(&binary, &app_bundle, IosTarget::Device)?;

        // Resolve target devices (returns UDIDs).
        let target_udids = self.resolve_target_udids(device_selectors)?;

        // Resolve provisioning profile.
        let profile_path = self.resolve_profile(&target_udids)?;

        step("Embedding provisioning profile...");
        self.copy_file(&profile_path, &app_bundle.join("embedded.mobileprovision"))?;

        // For free provisioning, always sign with the exact certificate we
        // issued, identified by SHA-1, ignoring any configured
        // `sign_identity`: free provisioning mints and manages its own
        // certificate, so a `sign_identity` set for other purposes (e.g. a
        // macOS Developer ID default inherited from the global config) does
        // not apply here and would otherwise be picked instead, only to be
        // rejected by the profile.
        let mut dev_fp = None;
        if matches!(ios_settings.provisioning, IosProvisioningBackend::Free) && !self.dry_run {
            ensure_keychain_ready()?;
            dev_fp = provisioning::dev_cert_sha1()?;
        }

        step("Signing device bundle...");
        let is_free = matches!(ios_settings.provisioning, IosProvisioningBackend::Free);
        let identity = if let Some(fp) = &dev_fp {
            fp.as_str()
        } else if is_free || self.cfg.sign_identity.is_empty() {
            "Apple Development"
        } else {
            &self.cfg.sign_identity
        };
        self.sign_ios_device(&app_bundle, &profile_path, identity)?;

        let app_str = app_bundle.to_str().unwrap();
        for udid in &target_udids {
            step(&format!("Installing on {udid}..."));
            self.sh.run(&[
                "xcrun",
                "devicectl",
                "device",
                "install",
                "app",
                "--device",
                udid,
                app_str,
            ])?;

            step("Launching app...");
            self.sh.run(&[
                "xcrun",
                "devicectl",
                "device",
                "process",
                "launch",
                "--device",
                udid,
                &self.cfg.bundle_id,
            ])?;
        }

        println!();
        cprintln!(
            "<green>Done!</green> App installed and launched on {} device(s).",
            target_udids.len()
        );
        Ok(())
    }

    /// Sign a device `.app` bundle with entitlements extracted directly from
    /// the provisioning profile. Using profile-derived entitlements (rather
    /// than a hand-edited JSON) ensures the signature matches the profile
    /// exactly. `--generate-entitlement-der` is required on modern iOS.
    fn sign_ios_device(
        &self,
        app_bundle: &Path,
        profile_path: &Path,
        identity: &str,
    ) -> Result<()> {
        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> security cms -D -i {} | extract Entitlements",
                profile_path.display()
            );
            cprintln!(
                "<dim>[dry-run]</dim> codesign --force --sign {} --entitlements \
                 ios-device-entitlements.plist --generate-entitlement-der {}",
                identity,
                app_bundle.display()
            );
            return Ok(());
        }

        step("Checking signing identity...");
        self.check_signing_identity(identity)?;

        let profile_plist = decode_profile(profile_path)?;

        step("Checking certificate is authorized by profile...");
        self.check_identity_in_profile(identity, &profile_plist)?;

        let entitlements = profile_plist
            .as_dictionary()
            .and_then(|d| d.get("Entitlements"))
            .context("Provisioning profile has no Entitlements key")?;

        let ent_plist_path = app_bundle
            .parent()
            .unwrap_or(Path::new("."))
            .join("ios-device-entitlements.plist");
        plist::to_file_xml(&ent_plist_path, entitlements)
            .context("Failed to write entitlements plist")?;
        let ent_str = ent_plist_path.to_str().unwrap();
        let bundle_str = app_bundle.to_str().unwrap();

        std::process::Command::new("codesign")
            .args([
                "--force",
                "--sign",
                identity,
                "--entitlements",
                ent_str,
                "--generate-entitlement-der",
                bundle_str,
            ])
            .status()
            .context("Failed to run codesign")?;

        step("Verifying device signature...");
        let verify_status = std::process::Command::new("codesign")
            .args(["--verify", "--deep", "--strict", "--verbose=2", bundle_str])
            .status()
            .context("Failed to run codesign --verify")?;
        if !verify_status.success() {
            // Show the identity actually embedded in the bundle for diagnosis.
            let _ = std::process::Command::new("codesign")
                .args(["-dvvv", bundle_str])
                .status();
            bail!(
                "Signature verification failed - the app will be rejected at install time.\n\
                 The signing certificate may have expired since the bundle was built.\n\
                 Check: security find-identity -v -p codesigning"
            );
        }

        Ok(())
    }

    fn check_signing_identity(&self, identity: &str) -> Result<()> {
        let valid_out = std::process::Command::new("security")
            .args(["find-identity", "-v", "-p", "codesigning"])
            .output()
            .context("Failed to run `security find-identity`")?;
        let valid_stdout = String::from_utf8_lossy(&valid_out.stdout);

        if let Some(line) = valid_stdout.lines().find(|l| l.contains(identity)) {
            let cert_name = parse_identity_line(line).map_or("", |(_, name)| name);
            if cert_name.starts_with("Apple Distribution")
                || cert_name.starts_with("iPhone Distribution")
            {
                bail!(
                    "Signing identity {identity:?} is a distribution certificate \
                     and cannot be used for development device installs.\n\
                     Use an \"Apple Development\" certificate instead."
                );
            }
            return Ok(());
        }

        // Not in the valid list - check if it exists but is expired/revoked.
        let all_out = std::process::Command::new("security")
            .args(["find-identity", "-p", "codesigning"])
            .output()
            .context("Failed to run `security find-identity`")?;
        let all_stdout = String::from_utf8_lossy(&all_out.stdout);

        if all_stdout.contains(identity) {
            bail!(
                "Signing identity {identity:?} is expired or revoked.\n\
                 Renew in Xcode (Settings > Accounts > Manage Certificates) \
                 or at developer.apple.com."
            );
        }

        bail!(
            "Signing identity {identity:?} not found in Keychain.\n\
             Valid identities:\n{}\n\
             Set [ios] sign_identity in strudel.toml to match one of the above.",
            valid_stdout.trim()
        );
    }

    /// Verify that the signing identity's certificate is listed in the
    /// profile's DeveloperCertificates. Mismatches cause iOS to reject the
    /// app at install time even when the local signature verifies cleanly.
    fn check_identity_in_profile(&self, identity: &str, profile: &plist::Value) -> Result<()> {
        let Some(certs) = profile
            .as_dictionary()
            .and_then(|d| d.get("DeveloperCertificates"))
            .and_then(|v| v.as_array())
        else {
            return Ok(());
        };

        // Extract the SHA1 fingerprint for our identity from the keychain.
        let id_out = std::process::Command::new("security")
            .args(["find-identity", "-v", "-p", "codesigning"])
            .output()
            .context("Failed to run `security find-identity`")?;
        let id_stdout = String::from_utf8_lossy(&id_out.stdout);

        let Some(signing_fp) = id_stdout
            .lines()
            .find(|l| l.contains(identity))
            .and_then(|l| parse_identity_line(l).map(|(hash, _)| hash.to_ascii_uppercase()))
        else {
            return Ok(());
        };

        for cert_val in certs {
            let Some(cert_data) = cert_val.as_data() else {
                continue;
            };

            let mut child = std::process::Command::new("openssl")
                .args(["x509", "-inform", "DER", "-noout", "-fingerprint", "-sha1"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .context("Failed to run `openssl x509`")?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(cert_data);
            }
            let fp_out = child.wait_with_output().context("openssl x509 failed")?;
            let fp_str = String::from_utf8_lossy(&fp_out.stdout);
            // Output: "SHA1 Fingerprint=AA:BB:CC:..."
            if let Some(fp) = fp_str.split('=').nth(1) {
                let fp_clean: String = fp.trim().replace(':', "").to_ascii_uppercase();
                if fp_clean == signing_fp {
                    return Ok(());
                }
            }
        }

        bail!(
            "Signing identity {identity:?} is not authorized by the provisioning profile.\n\
             The profile was created with an older certificate.\n\
             Run: strudel profile fetch --force"
        );
    }
}
