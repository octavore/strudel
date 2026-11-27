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
    /// Connect API when one is not already current. With `console`, attach to
    /// the app console after launch and stream it until the app exits.
    pub fn device(&self, device_selectors: &[String], console: bool) -> Result<()> {
        let ios_settings = &self.ios;

        let app_bundle = self.compile_and_assemble(IosTarget::Device)?;

        // Resolve target devices (returns UDIDs).
        let target_udids = self.resolve_target_udids(device_selectors)?;
        if console && target_udids.len() > 1 {
            bail!(
                "--console supports a single device; {} devices selected",
                target_udids.len()
            );
        }

        // Resolve the provisioning profile of the host (first) and of each
        // extension. Each extension is sandboxed on its own bundle ID, so the
        // host's profile does not cover it.
        let profile_targets = self.profile_targets();
        self.step("Preparing provisioning profiles...");
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
            let account = provisioning::free_account(&self.cfg.team_id)?;
            ensure_keychain_ready(&account)?;
            dev_fp = provisioning::dev_cert_sha1(&account)?;
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
            if console {
                // Printed under --no-echo too. The console stream below runs
                // until the app exits or Ctrl+C.
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

        if self.dry_run {
            self.note(cformat!(
                "\n<dim>[dry-run]</dim> Would install and launch the app on {} device(s).",
                target_udids.len()
            ));
            return Ok(());
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
            self.codesign_with_profile(appex, appex_profile, None, identity, &ent_plist_path)?;
        }

        let ent_plist_path = app_bundle
            .parent()
            .unwrap_or(Path::new("."))
            .join("ios-device-entitlements.plist");
        self.codesign_with_profile(
            app_bundle,
            profile_path,
            self.cfg.entitlements_json_path.as_deref(),
            identity,
            &ent_plist_path,
        )?;

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

    /// Entitlements JSON of the app, if one is configured.
    fn read_app_entitlements(&self, path: Option<&Path>) -> Result<Option<serde_json::Value>> {
        let Some(path) = path else {
            return Ok(None);
        };
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read entitlements JSON at {}", path.display()))?;
        let value = serde_json::from_str(&raw)
            .with_context(|| format!("Entitlements file is not valid JSON: {}", path.display()))?;
        Ok(Some(value))
    }

    /// Sign `bundle` with the entitlements of `profile_path`, written to
    /// `ent_plist_path` first. Wildcard values in the profile are replaced
    /// with values from `app_entitlements_path`. Fails if the profile does not
    /// authorize `identity`.
    fn codesign_with_profile(
        &self,
        bundle: &Path,
        profile_path: &Path,
        app_entitlements_path: Option<&Path>,
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

        let mut entitlements = profile_plist
            .as_dictionary()
            .and_then(|d| d.get("Entitlements"))
            .and_then(|v| v.as_dictionary())
            .cloned()
            .context("Provisioning profile has no Entitlements key")?;
        let app_entitlements = self.read_app_entitlements(app_entitlements_path)?;
        resolve_wildcard_entitlements(&mut entitlements, app_entitlements.as_ref())
            .with_context(|| format!("Cannot sign {}", bundle.display()))?;
        plist::to_file_xml(ent_plist_path, &plist::Value::Dictionary(entitlements))
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

/// Entitlement that a profile grants as a list of environments. A signature
/// carries one environment.
const ICLOUD_ENVIRONMENT_KEY: &str = "com.apple.developer.icloud-container-environment";

/// Profile entitlement that holds a grant instead of a value a signature can
/// carry. This is `"*"`, or a list of environments for iCloud.
fn is_unsignable_grant(key: &str, value: &plist::Value) -> bool {
    value.as_string() == Some("*") || (key == ICLOUD_ENVIRONMENT_KEY && value.as_array().is_some())
}

/// Replace profile entitlements that hold a grant with the value from `app`.
/// Fails and lists every key that `app` does not declare.
fn resolve_wildcard_entitlements(
    entitlements: &mut plist::Dictionary,
    app: Option<&serde_json::Value>,
) -> Result<()> {
    let wildcards: Vec<String> = entitlements
        .iter()
        .filter(|(k, v)| is_unsignable_grant(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    let mut missing = Vec::new();
    for key in wildcards {
        let from_app = app
            .and_then(|a| a.get(&key))
            .and_then(|v| plist::to_value(v).ok());
        match from_app {
            Some(value) => {
                entitlements.insert(key, value);
            },
            None => missing.push(key),
        }
    }
    if !missing.is_empty() {
        bail!(
            "The provisioning profile grants these entitlements as a wildcard or a list, \
             which cannot be signed: {}.\nDeclare each one in the entitlements JSON, for \
             example \"com.apple.developer.icloud-services\": [\"CloudKit\"] or \
             \"com.apple.developer.icloud-container-environment\": \"Development\".",
            missing.join(", ")
        );
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ICLOUD_SERVICES_KEY: &str = "com.apple.developer.icloud-services";

    fn ents(pairs: &[(&str, plist::Value)]) -> plist::Dictionary {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn star() -> plist::Value {
        plist::Value::String("*".into())
    }

    #[test]
    fn wildcard_takes_the_app_value() {
        let mut e = ents(&[(ICLOUD_SERVICES_KEY, star())]);
        let app = json!({ ICLOUD_SERVICES_KEY: ["CloudKit", "CloudDocuments"] });
        resolve_wildcard_entitlements(&mut e, Some(&app)).unwrap();
        let got = e.get(ICLOUD_SERVICES_KEY).unwrap().as_array().unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn undeclared_wildcards_fail_and_are_all_listed() {
        let mut e = ents(&[
            (ICLOUD_SERVICES_KEY, star()),
            ("com.apple.developer.other", star()),
        ]);
        let err = resolve_wildcard_entitlements(&mut e, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains(ICLOUD_SERVICES_KEY), "{err}");
        assert!(err.contains("com.apple.developer.other"), "{err}");
    }

    #[test]
    fn environment_list_needs_a_declared_value() {
        let list = plist::Value::Array(vec!["Production".into(), "Development".into()]);
        let mut e = ents(&[(ICLOUD_ENVIRONMENT_KEY, list.clone())]);
        let err = resolve_wildcard_entitlements(&mut e, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains(ICLOUD_ENVIRONMENT_KEY), "{err}");

        let mut e = ents(&[(ICLOUD_ENVIRONMENT_KEY, list)]);
        let app = json!({ ICLOUD_ENVIRONMENT_KEY: "Development" });
        resolve_wildcard_entitlements(&mut e, Some(&app)).unwrap();
        let got = e.get(ICLOUD_ENVIRONMENT_KEY).unwrap();
        assert_eq!(got.as_string(), Some("Development"));
    }

    #[test]
    fn plain_values_are_kept() {
        let mut e = ents(&[("get-task-allow", plist::Value::Boolean(true))]);
        resolve_wildcard_entitlements(&mut e, None).unwrap();
        assert!(e.get("get-task-allow").is_some());
    }
}
