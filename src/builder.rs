use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use color_print::cprintln;
use serde_json::Value;

use crate::config::{NotaryAuth, ResolvedConfig};
use crate::paths::Paths;
use crate::shell::Shell;

pub struct Builder {
    cfg: ResolvedConfig,
    p: Paths,
    sh: Shell,
}

fn step(msg: &str) {
    cprintln!("\n<green>==>> {}</green>", msg);
}

impl Builder {
    pub fn new(cfg: ResolvedConfig, dry_run: bool) -> Self {
        let p = Paths::new(&cfg);
        Builder {
            cfg,
            p,
            sh: Shell::new(dry_run),
        }
    }

    fn dry_run(&self) -> bool {
        self.sh.dry_run
    }

    /// Create a directory (and parents), logging in dry-run instead of acting.
    fn create_dir(&self, path: &Path) -> Result<()> {
        if self.dry_run() {
            cprintln!("<dim>[dry-run]</dim> mkdir -p {}", path.display());
            return Ok(());
        }
        fs::create_dir_all(path).with_context(|| format!("Failed to create {}", path.display()))
    }

    /// Copy a file, logging source → dest in dry-run instead of acting.
    fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
        if self.dry_run() {
            cprintln!(
                "<dim>[dry-run]</dim> copy <blue>{}</blue> -> <blue>{}</blue>",
                from.display(),
                to.display()
            );
            return Ok(());
        }
        fs::copy(from, to)
            .with_context(|| format!("Failed to copy {} -> {}", from.display(), to.display()))?;
        Ok(())
    }

    /// Write a file's contents, logging dest in dry-run instead of acting.
    fn write_file(&self, path: &Path, contents: &str) -> Result<()> {
        if self.dry_run() {
            cprintln!(
                "<dim>[dry-run]</dim> write {} ({} bytes)",
                path.display(),
                contents.len()
            );
            return Ok(());
        }
        fs::write(path, contents).with_context(|| format!("Failed to write {}", path.display()))
    }

    pub fn clean(&self) -> Result<()> {
        step("Cleaning previous build...");
        if self.dry_run() {
            cprintln!("<dim>[dry-run]</dim> rm -rf {}", self.p.build_dir.display());
            cprintln!(
                "<dim>[dry-run]</dim> mkdir -p {}",
                self.p.build_dir.display()
            );
            return Ok(());
        }
        if self.p.build_dir.exists() {
            fs::remove_dir_all(&self.p.build_dir)?;
        }
        fs::create_dir_all(&self.p.build_dir)?;
        Ok(())
    }

    pub fn build_binary(&self) -> Result<PathBuf> {
        step("Building release binary...");

        let source = self.cfg.source_dir.to_str().unwrap();
        let arch_flags: Vec<String> = self
            .cfg
            .archs
            .iter()
            .flat_map(|a| ["--arch".to_string(), a.clone()])
            .collect();

        // Build base args shared between both swift invocations
        let mut base: Vec<String> = vec![
            "build".to_string(),
            "-c".to_string(),
            "release".to_string(),
            "--package-path".to_string(),
            source.to_string(),
        ];
        base.extend(arch_flags);

        let build_refs: Vec<&str> = std::iter::once("swift")
            .chain(base.iter().map(String::as_str))
            .collect();
        self.sh.run_streamed(&build_refs)?;

        let mut show_base = base.clone();
        show_base.push("--show-bin-path".to_string());
        let show_refs: Vec<&str> = std::iter::once("swift")
            .chain(show_base.iter().map(String::as_str))
            .collect();
        let bin_dir = self.sh.run(&show_refs)?;
        let bin_dir = bin_dir.trim();

        let binary_path = if bin_dir.is_empty() {
            // dry-run: fall back to expected location
            self.cfg
                .source_dir
                .join(".build/release")
                .join(&self.cfg.target_name)
        } else {
            PathBuf::from(bin_dir).join(&self.cfg.target_name)
        };

        if !bin_dir.is_empty() && !binary_path.exists() {
            let found: Vec<String> = fs::read_dir(bin_dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.contains('.'))
                .collect();

            let hint = if found.is_empty() {
                "No executables were found in the build directory.".to_string()
            } else {
                format!(
                    "Executables found in the build directory: {}.\n\
                     If one of these is the right binary, set `target_name` in your strudel.toml to its name.",
                    found.join(", ")
                )
            };

            bail!(
                "Could not locate built binary at:\n  {}\n\
                 strudel looks for an executable named `{}` (from `target_name`, which defaults to `app_name`).\n{}",
                binary_path.display(),
                self.cfg.target_name,
                hint,
            );
        }

        Ok(binary_path)
    }

    pub fn assemble_bundle(&self, binary_path: &Path) -> Result<PathBuf> {
        step("Assembling app bundle...");
        let app_bundle = &self.p.app_bundle;

        self.create_dir(&app_bundle.join("Contents/MacOS"))?;
        self.create_dir(&app_bundle.join("Contents/Resources"))?;

        self.copy_file(
            binary_path,
            &app_bundle.join("Contents/MacOS").join(&self.cfg.app_name),
        )?;

        // Read info JSON (or start from an empty object) and override version/identity fields
        let mut info: Value = match &self.cfg.info_json_path {
            Some(path) => {
                let info_str = fs::read_to_string(path).with_context(|| {
                    format!(
                        "Failed to read info JSON at {} (set in `info_json_path` in strudel.toml).",
                        path.display()
                    )
                })?;
                serde_json::from_str(&info_str)
                    .with_context(|| format!("Failed to parse info JSON at {}", path.display()))?
            }
            None => Value::Object(Default::default()),
        };
        let obj = info.as_object_mut().unwrap();
        obj.insert(
            "CFBundleExecutable".to_string(),
            Value::String(self.cfg.app_name.clone()),
        );
        obj.insert(
            "CFBundleShortVersionString".to_string(),
            Value::String(self.cfg.version.clone()),
        );
        obj.insert(
            "CFBundleVersion".to_string(),
            Value::String(self.cfg.build_number.clone()),
        );
        obj.insert(
            "CFBundleIdentifier".to_string(),
            Value::String(self.cfg.bundle_id.clone()),
        );

        if let Some(icon_path) = &self.cfg.icon_path {
            self.copy_file(
                icon_path,
                &app_bundle.join("Contents/Resources/AppIcon.icns"),
            )?;
            obj.insert(
                "CFBundleIconFile".to_string(),
                Value::String("AppIcon".to_string()),
            );
        }

        // Pipe JSON into plutil to produce Info.plist
        let json_bytes = serde_json::to_vec_pretty(&info)?;
        let plist_path = self.p.info_plist.to_str().unwrap();
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        self.write_file(&app_bundle.join("Contents/PkgInfo"), "APPL????")?;

        Ok(app_bundle.clone())
    }

    /// Build bundle only (clean → binary → assemble).
    pub fn build(&self) -> Result<()> {
        self.clean()?;
        let binary_path = self.build_binary()?;
        let app_bundle = self.assemble_bundle(&binary_path)?;
        cprintln!(
            "\n<green>Done! App bundle:</green>\n{}",
            app_bundle.display()
        );
        Ok(())
    }

    /// Local/dev pipeline: clean → build → assemble → sign, stopping at a signed
    /// `.app`. No notarization or DMG, and no notary credentials required. Uses
    /// the configured signing identity if set, otherwise signs ad-hoc — enough
    /// to test entitlements and the hardened runtime without a Developer ID
    /// certificate or an Apple account.
    pub fn sign_app(&self) -> Result<()> {
        // No-op unless APPLE_CERTIFICATE is set; supports signing with an
        // imported Developer ID identity here too, but ad-hoc needs nothing.
        let _keychain = self.import_certificate()?;
        self.clean()?;
        let binary_path = self.build_binary()?;
        self.assemble_bundle(&binary_path)?;
        self.sign()?;

        if self.dry_run() {
            cprintln!(
                "\n<dim>[dry-run]</dim> Dry run complete. Signed app bundle would be at:\n{}",
                self.p.app_bundle.display()
            );
        } else {
            cprintln!(
                "\n<green>Done! Signed app bundle:</green>\n{}",
                self.p.app_bundle.display()
            );
        }
        Ok(())
    }

    // ── Distribution steps ────────────────────────────────────────────────────

    pub fn sign(&self) -> Result<()> {
        let app_bundle = self.p.app_bundle.to_str().unwrap();
        let ent_plist = self.p.entitlements_plist.to_str().unwrap();
        let ent_json_path = &self.cfg.entitlements_json_path;
        let ent_json = ent_json_path.to_str().unwrap();

        let ent_raw = fs::read_to_string(ent_json_path).with_context(|| {
            format!("Failed to read entitlements JSON at {ent_json}")
        })?;
        serde_json::from_str::<Value>(&ent_raw).with_context(|| {
            format!("Entitlements file is not valid JSON: {ent_json}")
        })?;

        // With no identity configured, sign ad-hoc (`--sign -`): no certificate
        // or account needed, enough to exercise entitlements locally. A real
        // identity (and notarization) is required to distribute — see `release`.
        let adhoc = self.cfg.sign_identity.is_empty();
        let identity = if adhoc {
            step("Signing app bundle (ad-hoc — no signing identity configured)...");
            "-"
        } else {
            step("Signing app bundle...");
            self.cfg.sign_identity.as_str()
        };

        self.sh
            .run(&["plutil", "-convert", "xml1", "-o", ent_plist, ent_json])?;

        let mut args = vec![
            "codesign",
            "--force",
            "--entitlements",
            ent_plist,
            "--sign",
            identity,
        ];
        // The hardened runtime and a trusted timestamp both require a real
        // Developer ID certificate; skip them for ad-hoc signatures.
        if !adhoc {
            args.extend(["--options", "runtime", "--timestamp"]);
        }
        args.push(app_bundle);
        self.sh.run(&args)?;

        step("Verifying signature...");
        self.sh.run(&[
            "codesign",
            "--verify",
            "--deep",
            "--strict",
            "--verbose=2",
            app_bundle,
        ])?;
        // spctl may return non-zero for unnotarized bundles
        self.sh.try_run(&[
            "spctl",
            "--assess",
            "--verbose=4",
            "--type",
            "exec",
            app_bundle,
        ]);

        Ok(())
    }

    pub fn notarize(&self) -> Result<()> {
        step("Creating zip for notarization...");
        let app_bundle = self.p.app_bundle.to_str().unwrap();
        let zip = self.p.zip.to_str().unwrap();

        self.sh
            .run(&["ditto", "-c", "-k", "--keepParent", app_bundle, zip])?;

        step("Stapling notarization ticket...");
        self.sh.run(&["xcrun", "stapler", "staple", app_bundle])?;
        self.sh.run(&["xcrun", "stapler", "validate", app_bundle])?;

        Ok(())
    }

    pub fn package_dmg(&self) -> Result<()> {
        let app_bundle = self.p.app_bundle.to_str().unwrap();
        let dmg = self.p.dmg.to_str().unwrap();
        let vol_name = format!("{} {}", self.cfg.app_name, self.cfg.version);
        let timeout_str = self.cfg.notarize_timeout.to_string();

        step("Creating DMG...");
        self.sh.run(&[
            "hdiutil",
            "create",
            "-volname",
            &vol_name,
            "-srcfolder",
            app_bundle,
            "-ov",
            "-format",
            "UDZO",
            dmg,
        ])?;

        self.sh.run(&[
            "codesign",
            "--force",
            "--sign",
            &self.cfg.sign_identity,
            "--timestamp",
            dmg,
        ])?;

        step("Submitting DMG for notarization...");
        // Build the real args alongside a redacted display: the API key path,
        // key id, and issuer are identifiers, but the app-specific password is a
        // secret and must not reach the terminal or an error message.
        let mut args: Vec<String> = ["xcrun", "notarytool", "submit", dmg]
            .map(String::from)
            .to_vec();
        let mut display = args.clone();
        match self.cfg.notary_auth() {
            Some(NotaryAuth::ApiKey {
                key_path,
                key_id,
                issuer,
            }) => {
                let auth = [
                    "--key".into(),
                    key_path.to_string_lossy().into_owned(),
                    "--key-id".into(),
                    key_id,
                    "--issuer".into(),
                    issuer,
                ];
                display.extend(auth.clone());
                args.extend(auth);
            }
            Some(NotaryAuth::AppleId {
                apple_id,
                password,
                team_id,
            }) => {
                args.extend([
                    "--apple-id".into(),
                    apple_id.clone(),
                    "--team-id".into(),
                    team_id.clone(),
                    "--password".into(),
                    password,
                ]);
                display.extend([
                    "--apple-id".into(),
                    apple_id,
                    "--team-id".into(),
                    team_id,
                    "--password".into(),
                    "<redacted>".into(),
                ]);
            }
            // preflight_credentials guarantees a complete set before `run`.
            None => bail!("No notarization credentials available"),
        }
        let tail = ["--wait".into(), "--timeout".into(), timeout_str];
        display.extend(tail.clone());
        args.extend(tail);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.sh.run_redacted(&arg_refs, &display.join(" "))?;

        step("Stapling DMG...");
        self.sh.run(&["xcrun", "stapler", "staple", dmg])?;

        Ok(())
    }

    /// Describe any signing/notarization credentials that are missing or
    /// incomplete. Empty means a real `run` has everything it needs.
    fn credential_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.cfg.sign_identity.is_empty() {
            problems.push("SIGN_IDENTITY (signing identity) is not set".to_string());
        }
        if self.cfg.notary_auth().is_none() {
            problems.push(
                "no complete notarization credentials — provide EITHER an App \
                 Store Connect API key (APPLE_API_KEY_PATH, APPLE_API_KEY, \
                 APPLE_API_ISSUER) OR an Apple ID (APPLE_ID, APPLE_PASSWORD, \
                 TEAM_ID)"
                    .to_string(),
            );
        }
        problems
    }

    /// Verify the credentials required for signing and notarization are present.
    /// Bails early so a missing value doesn't surface deep into the pipeline (e.g.
    /// `codesign: no identity found`). In dry-run, only warns — there's nothing to sign.
    fn preflight_credentials(&self) -> Result<()> {
        let problems = self.credential_problems();
        if problems.is_empty() {
            return Ok(());
        }

        let hint = "Set identifiers in strudel.toml or the environment, and \
                    secrets (passwords, certificate) in the environment only. \
                    See the README's \"Signing & notarization\" section.";

        if self.dry_run() {
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
    fn set_user_keychains(&self, list: &[String]) -> Result<()> {
        let mut args: Vec<&str> = vec!["security", "list-keychains", "-d", "user", "-s"];
        args.extend(list.iter().map(String::as_str));
        self.sh.run(&args).map(|_| ())
    }

    /// If a signing certificate is provided via `APPLE_CERTIFICATE`, decode it
    /// into a throwaway keychain and add that keychain to the user search list
    /// so `codesign` can find the identity. The returned guard removes the
    /// keychain and restores the search list on drop, so a build leaves no
    /// credentials behind — useful on a fresh CI runner. When no certificate is
    /// configured (the common local case, where the identity already lives in
    /// the login keychain), this is a no-op returning `None`.
    fn import_certificate(&self) -> Result<Option<TempKeychain<'_>>> {
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
        let kc_pw = format!("strudel-{pid}");

        if self.dry_run() {
            cprintln!("<dim>[dry-run]</dim> security create-keychain -p <<redacted>> {keychain}");
            cprintln!(
                "<dim>[dry-run]</dim> decode $APPLE_CERTIFICATE ({} b64 chars) -> <<temp>>.p12",
                cert_b64.len()
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
        let p12 = base64::engine::general_purpose::STANDARD
            .decode(cert_b64.trim())
            .context("APPLE_CERTIFICATE is not valid base64")?;
        let p12_path = std::env::temp_dir().join(format!("strudel-{pid}.p12"));
        fs::write(&p12_path, &p12)
            .with_context(|| format!("Failed to write {}", p12_path.display()))?;
        let p12_str = p12_path.to_string_lossy().into_owned();

        // Snapshot the search list first so the guard can restore it.
        let original_list = self.user_keychains()?;

        self.sh.run_redacted(
            &["security", "create-keychain", "-p", &kc_pw, &keychain],
            &format!("security create-keychain -p <redacted> {keychain}"),
        )?;
        self.sh.run(&[
            "security",
            "set-keychain-settings",
            "-lut",
            "21600",
            &keychain,
        ])?;
        self.sh.run_redacted(
            &["security", "unlock-keychain", "-p", &kc_pw, &keychain],
            &format!("security unlock-keychain -p <redacted> {keychain}"),
        )?;
        self.sh.run_redacted(
            &[
                "security",
                "import",
                &p12_str,
                "-P",
                cert_password,
                "-A",
                "-t",
                "cert",
                "-f",
                "pkcs12",
                "-k",
                &keychain,
            ],
            &format!("security import {p12_str} -P <redacted> -A -t cert -f pkcs12 -k {keychain}"),
        )?;
        // Let codesign use the imported key without an interactive prompt.
        self.sh.run_redacted(
            &[
                "security",
                "set-key-partition-list",
                "-S",
                "apple-tool:,apple:",
                "-s",
                "-k",
                &kc_pw,
                &keychain,
            ],
            &format!(
                "security set-key-partition-list -S apple-tool:,apple: -s -k <redacted> {keychain}"
            ),
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

    /// Full release pipeline: clean → binary → assemble → sign → notarize → DMG.
    pub fn release(&self) -> Result<()> {
        self.preflight_credentials()?;
        // Held for the whole build: the imported identity must remain available
        // to both `sign` and the DMG signing in `package_dmg`. Dropped at the
        // end of this function, which tears the temporary keychain back down.
        let _keychain = self.import_certificate()?;
        self.clean()?;
        let binary_path = self.build_binary()?;
        self.assemble_bundle(&binary_path)?;
        self.sign()?;
        self.notarize()?;
        self.package_dmg()?;

        if self.dry_run() {
            cprintln!(
                "\n<dim>[dry-run]</dim> Dry run complete. Artifacts would be at:\n  App bundle: {}\n  DMG:        {}\n  Zip:        {}",
                self.p.app_bundle.display(),
                self.p.dmg.display(),
                self.p.zip.display(),
            );

            let problems = self.credential_problems();
            if !problems.is_empty() {
                cprintln!("\n<yellow>[warning]</yellow> Credentials still missing for a real run:");
                for p in &problems {
                    cprintln!("  - {p}");
                }
            }
        } else {
            cprintln!(
                "\n<green>Done!</green> Distribution artifacts:\n  App bundle: {}\n  DMG:        {}\n  Zip:        {}",
                self.p.app_bundle.display(),
                self.p.dmg.display(),
                self.p.zip.display(),
            );
        }
        Ok(())
    }
}

/// A throwaway keychain holding an imported signing identity. On drop it
/// restores the original keychain search list and deletes the keychain, so a
/// build never leaves credentials behind on the machine. Cleanup is
/// best-effort: we're tearing down, so failures are ignored.
struct TempKeychain<'a> {
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
            let _ = self.sh.run(&args);
        }
        let _ = self.sh.run(&["security", "delete-keychain", &self.path]);
    }
}
