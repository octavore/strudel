use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clml::{cformat, cformatdoc, cprintln};

use crate::apple::provisioning::{self, ensure_keychain_ready};
use crate::builder::ios::IosTarget;
use crate::builder::ios::bundle::ios_appex_dir;
use crate::builder::keychain::parse_identity_line;
use crate::builder::profile::{check_identity_authorized, decode_profile};
use crate::builder::{IosBuilder, is_framework};
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

        let app_bundle = self.compile_and_assemble(IosTarget::Device)?;

        // Resolve target devices (returns UDIDs).
        let target_udids = self.resolve_target_udids(device_selectors)?;

        // Resolve the provisioning profile of the host (first) and of each
        // extension. Each extension is sandboxed on its own bundle ID, so the
        // host's profile does not cover it.
        let profile_targets = self.profile_targets();
        let mut profiles = Vec::with_capacity(profile_targets.len());
        for target in &profile_targets {
            profiles.push(self.resolve_profile(target, &target_udids)?);
        }
        let (profile_path, extension_profiles) = profiles
            .split_first()
            .context("Host profile target is missing")?;

        self.step("Embedding provisioning profile...");
        self.copy_file(profile_path, &app_bundle.join("embedded.mobileprovision"))?;
        let mut appexes = Vec::new();
        for (ext, ext_profile) in self.cfg.extensions.iter().zip(extension_profiles) {
            let appex = ios_appex_dir(&app_bundle, ext);
            self.copy_file(ext_profile, &appex.join("embedded.mobileprovision"))?;
            appexes.push((appex, ext_profile.as_path()));
        }

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

        self.step("Signing device bundle...");
        let is_free = matches!(ios_settings.provisioning, IosProvisioningBackend::Free);
        let identity = if let Some(fp) = &dev_fp {
            fp.as_str()
        } else if is_free || self.cfg.sign_identity.is_empty() {
            "Apple Development"
        } else {
            &self.cfg.sign_identity
        };
        self.sign_ios_device(&app_bundle, profile_path, &appexes, identity)?;

        let app_str = app_bundle.to_str().unwrap();
        for udid in &target_udids {
            self.step(&format!("Installing on {udid}..."));
            retry_while_locked(udid, || {
                self.sh.run(&[
                    "xcrun",
                    "devicectl",
                    "device",
                    "install",
                    "app",
                    "--device",
                    udid,
                    app_str,
                ])
            })?;

            self.step("Launching app...");
            // Attach to the console and stream logs live when running on a
            // single device. With multiple target devices we'd need to
            // stream several consoles concurrently to one terminal, so fall
            // back to a plain, non-blocking launch there instead.
            if target_udids.len() == 1 {
                // Printed under --no-echo too: the console stream below still
                // runs until Ctrl+C.
                if !self.quiet() {
                    cprintln!("<dim>Streaming logs. Press Ctrl+C to stop.</dim>");
                }
                self.note(self.log_stream_hint(Some(udid)));
                retry_while_locked(udid, || {
                    self.sh
                        .run_streamed_stdout(ShellCommand::new("xcrun").args([
                            "devicectl",
                            "device",
                            "process",
                            "launch",
                            "--console",
                            "--device",
                            udid,
                            &self.cfg.bundle_id,
                        ]))
                })?;
                return Ok(());
            }
            retry_while_locked(udid, || {
                self.sh.run(&[
                    "xcrun",
                    "devicectl",
                    "device",
                    "process",
                    "launch",
                    "--device",
                    udid,
                    &self.cfg.bundle_id,
                ])
            })?;
        }

        self.note(cformat!(
            "\n<green>Done!</green> App installed and launched on {} device(s).",
            target_udids.len()
        ));
        for udid in &target_udids {
            self.note(self.log_stream_hint(Some(udid)));
        }
        Ok(())
    }

    /// Sign a device `.app` bundle and its extensions, inside-out: embedded
    /// libraries, then each `.appex` (`appexes`, with the profile it embeds),
    /// then the host. Each bundle is signed with entitlements extracted
    /// directly from its own provisioning profile. Using profile-derived
    /// entitlements (rather than a hand-edited JSON) ensures the signature
    /// matches the profile exactly. `--generate-entitlement-der` is required on
    /// modern iOS.
    fn sign_ios_device(
        &self,
        app_bundle: &Path,
        profile_path: &Path,
        appexes: &[(PathBuf, &Path)],
        identity: &str,
    ) -> Result<()> {
        if self.dry_run {
            for (appex, appex_profile) in appexes {
                self.echo(cformatdoc! {"
                    <dim>[dry-run]</dim> security cms -D -i {} | extract Entitlements
                    <dim>[dry-run]</dim> codesign --force --sign {identity} --entitlements \
                    appex-entitlements.plist --generate-entitlement-der {}",
                    appex_profile.display(),
                    appex.display()
                });
            }
            self.echo(cformatdoc! {"
                <dim>[dry-run]</dim> security cms -D -i {} | extract Entitlements
                <dim>[dry-run]</dim> codesign --force --sign {identity} --entitlements \
                ios-device-entitlements.plist --generate-entitlement-der {}",
                profile_path.display(),
                app_bundle.display()
            });
            return Ok(());
        }

        self.step("Checking signing identity...");
        self.check_signing_identity(identity)?;

        let bundle_str = app_bundle.to_str().unwrap();

        if !self.cfg.embed_libs.is_empty() {
            self.step(&format!("Signing embedded libraries ({identity})..."));
            let frameworks_dir = app_bundle.join("Frameworks");
            for lib_path in &self.cfg.embed_libs {
                if let Some(file_name) = lib_path.file_name() {
                    let dylib = frameworks_dir.join(file_name);
                    let dylib_str = dylib.to_str().unwrap();
                    let mut args = vec!["--force", "--sign", identity];
                    if is_framework(lib_path) {
                        args.push("--deep");
                    }
                    args.push(dylib_str);
                    std::process::Command::new("codesign")
                        .args(&args)
                        .status()
                        .context("Failed to run codesign")?;
                }
            }
        }

        for (appex, appex_profile) in appexes {
            self.step(&format!("Signing extension {}...", appex.display()));
            let ent_plist_path = app_bundle.parent().unwrap_or(Path::new(".")).join(format!(
                "{}.entitlements.plist",
                appex
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("appex")
            ));
            self.codesign_with_profile(appex, appex_profile, identity, &ent_plist_path)?;
        }

        let ent_plist_path = app_bundle
            .parent()
            .unwrap_or(Path::new("."))
            .join("ios-device-entitlements.plist");
        self.codesign_with_profile(app_bundle, profile_path, identity, &ent_plist_path)?;

        self.step("Verifying device signature...");
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

    /// Sign `bundle` with the entitlements of `profile_path`, written to
    /// `ent_plist_path` first. Fails if the profile does not authorize
    /// `identity`.
    fn codesign_with_profile(
        &self,
        bundle: &Path,
        profile_path: &Path,
        identity: &str,
        ent_plist_path: &Path,
    ) -> Result<()> {
        let profile_plist = decode_profile(profile_path)?;

        self.step("Checking certificate is authorized by profile...");
        check_identity_authorized(
            identity,
            &profile_plist,
            "Run: strudel profile fetch --force",
        )?;

        let entitlements = profile_plist
            .as_dictionary()
            .and_then(|d| d.get("Entitlements"))
            .context("Provisioning profile has no Entitlements key")?;
        plist::to_file_xml(ent_plist_path, entitlements)
            .context("Failed to write entitlements plist")?;

        let status = std::process::Command::new("codesign")
            .args(["--force", "--sign", identity, "--entitlements"])
            .arg(ent_plist_path)
            .arg("--generate-entitlement-der")
            .arg(bundle)
            .status()
            .context("Failed to run codesign")?;
        if !status.success() {
            bail!("codesign failed for {}", bundle.display());
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
                 Renew it at developer.apple.com under Certificates, \
                 Identifiers & Profiles > Certificates.\n\
                 Expired certificates are listed in Keychain Access under \
                 login > My Certificates."
            );
        }

        bail!(
            "Signing identity {identity:?} not found in Keychain.\n\
             Valid identities:\n{}\n\
             Set [ios] sign_identity in strudel.toml to match one of the above.",
            valid_stdout.trim()
        );
    }
}

/// Retry a `devicectl` operation for as long as it keeps failing because the
/// device is locked, prompting the user to unlock it between attempts.
fn retry_while_locked<T>(udid: &str, mut op: impl FnMut() -> Result<T>) -> Result<T> {
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) if is_device_locked_error(&err) => {
                cprintln!(
                    "<yellow>warning:</yellow> Device {udid} is locked. Unlock it and press \
                     enter to retry."
                );
                io::stdin().read_line(&mut String::new())?;
            },
            Err(err) if is_developer_mode_error(&err) => {
                return Err(err.context(format!(
                    "Developer Mode is disabled on device {udid}.\n\
                     Enable it in Settings > Privacy & Security > Developer Mode, \
                     then restart the device and confirm the prompt after unlocking."
                )));
            },
            Err(err) => return Err(err),
        }
    }
}

/// Whether a `devicectl` failure was caused by Developer Mode being disabled
/// on the device (CoreDeviceError 10005).
fn is_developer_mode_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("Developer Mode is disabled")
}

/// Whether a `devicectl` install failure was caused by the device being
/// locked (e.g. `kAMDMobileImageMounterDeviceLocked`), rather than some
/// other, non-retryable install error.
fn is_device_locked_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("kAMDMobileImageMounterDeviceLocked")
        || msg.contains("device is locked")
        || msg.contains("BSErrorCodeDescription = Locked")
        || msg.contains("could not be, unlocked")
}
