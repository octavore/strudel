//! Code signing: [`MacosBuilder::sign`] and [`MacosBuilder::sign_extension`].

use std::fs;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::builder::{MacosBuilder, is_framework, step};
use crate::config::ResolvedExtension;
use crate::paths::ExtensionPaths;
use crate::shell::ShellCommand;

impl MacosBuilder {
    pub fn sign(&self) -> Result<()> {
        let app_bundle = self.paths.app_bundle.to_str().unwrap();
        let ent_plist_path = self.paths.entitlements_plist.to_str().unwrap();

        let ent_value: Value = match self.cfg.entitlements_json_path {
            Some(ref path) => {
                let ent_raw = fs::read_to_string(path).with_context(|| {
                    format!("Failed to read entitlements JSON at {}", path.display())
                })?;
                serde_json::from_str(&ent_raw).with_context(|| {
                    format!("Entitlements file is not valid JSON: {}", path.display())
                })?
            },
            None => Value::Object(Default::default()),
        };

        let ent_bytes = serde_json::to_vec_pretty(&ent_value)?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", ent_plist_path, "-"],
            &ent_bytes,
        )?;

        if let Some(profile_path) = &self.cfg.provisioning_profile {
            self.validate_provisioning_profile(profile_path)?;
        }

        // With no identity configured, sign ad-hoc (`--sign -`): no certificate or
        // account needed, enough to exercise entitlements locally. A real
        // identity (and notarization) is required to distribute. See `release`.
        let adhoc = self.cfg.sign_identity.is_empty();

        // Create the base codesign command. The hardened runtime and a trusted
        // timestamp both require a real Developer ID certificate; skip them for
        // ad-hoc signatures.
        //
        // This is necessary to prevent crashes when the app attempts to load embedded
        // frameworks that do not share the same Team ID.
        let (identity, msg) = if adhoc {
            ("-", " (ad-hoc: no signing identity configured)...")
        } else {
            (self.cfg.sign_identity.as_str(), "")
        };

        if !adhoc && self.validate_sign_identity()? {
            self.validate_entitlements_for_adhoc(&ent_value);
        }

        let mut codesign_cmd = ShellCommand::new("codesign").args(["--force", "--sign", identity]);
        if !adhoc {
            codesign_cmd = codesign_cmd.args(["--options", "runtime", "--timestamp"]);
        }

        // Sign each embedded dylib/framework individually before signing the
        // bundle. codesign --verify --deep --strict and notarization both
        // require nested Mach-O files to carry valid signatures.
        if !self.cfg.embed_libs.is_empty() {
            step(&format!("Signing embedded libraries...{msg}"));
            let frameworks_dir = self.paths.app_bundle.join("Contents/Frameworks");
            for lib_path in &self.cfg.embed_libs {
                if let Some(file_name) = lib_path.file_name() {
                    let mut lib_codesign_cmd = codesign_cmd.clone();
                    if is_framework(lib_path) {
                        // Re-sign nested code (e.g. Sparkle's bundled Autoupdate.app / XPC
                        // services) with our own identity so it shares the outer app's Team ID;
                        // vendor signatures alone would fail Gatekeeper.
                        lib_codesign_cmd = lib_codesign_cmd.arg("--deep");
                    }
                    let dylib = frameworks_dir.join(file_name);
                    let dylib_str = dylib.to_str().unwrap();
                    lib_codesign_cmd = lib_codesign_cmd.arg(dylib_str);
                    lib_codesign_cmd.run(&self.sh)?;
                }
            }
        }

        // Sign user-configured `[[build.copy]]` entries marked `sign = true`.
        // Like embed_libs, these must be signed before the outer bundle is
        // sealed: directories may contain nested code (hence `--deep`), flat
        // files (e.g. a helper binary) are signed directly.
        if self.cfg.copy.iter().any(|c| c.sign) {
            step(&format!("Signing copied files...{msg}"));
            for item in self.cfg.copy.iter().filter(|c| c.sign) {
                let name = item.src.file_name().with_context(|| {
                    format!("copy entry has no filename: {}", item.src.display())
                })?;
                let dest = self.paths.app_bundle.join(&item.dest_dir).join(name);
                let dest_str = dest.to_str().with_context(|| {
                    format!("Invalid copy destination: {}/{:?}", item.dest_dir, name)
                })?;
                let mut item_codesign_cmd = codesign_cmd.clone();
                if item.src.is_dir() {
                    item_codesign_cmd = item_codesign_cmd.arg("--deep");
                }
                if let Some(ent_json_path) = &item.entitlements_json_path {
                    let ent_json_str = ent_json_path
                        .to_str()
                        .context("Invalid copy entitlements path.")?;
                    let ent_raw = fs::read_to_string(ent_json_path).with_context(|| {
                        format!(
                            "Failed to read entitlements JSON for copy entry `{}` at {ent_json_str}",
                            name.to_string_lossy()
                        )
                    })?;
                    let item_ent_value: Value =
                        serde_json::from_str(&ent_raw).with_context(|| {
                            format!(
                                "Copy entry entitlements file is not valid JSON: {ent_json_str}"
                            )
                        })?;
                    let ent_plist = self.paths.build_dir.join(format!(
                        "{}.copy-entitlements.plist",
                        name.to_string_lossy()
                    ));
                    let ent_plist_str = ent_plist
                        .to_str()
                        .context("Invalid copy entitlements plist path.")?;
                    self.sh.run(&[
                        "plutil",
                        "-convert",
                        "xml1",
                        ent_json_str,
                        "-o",
                        ent_plist_str,
                    ])?;
                    if adhoc {
                        self.validate_entitlements_for_adhoc(&item_ent_value);
                    }
                    item_codesign_cmd =
                        item_codesign_cmd.arg_group(["--entitlements", ent_plist_str]);
                }
                item_codesign_cmd = item_codesign_cmd.arg(dest_str);
                item_codesign_cmd.run(&self.sh)?;
            }
        }

        // Sign extensions inside-out: each extension bundle (.appex or
        // .systemextension) must be signed with its own entitlements before
        // the host bundle is sealed. A single `codesign --deep` pass over the
        // host would re-use the host's entitlements for the nested bundle,
        // which is wrong: the extension is sandboxed independently and
        // typically needs a different set.
        for (ext, ext_paths) in self.cfg.extensions.iter().zip(self.paths.extensions.iter()) {
            self.sign_extension(ext, ext_paths, &codesign_cmd, adhoc, msg)?;
        }

        // Run bundle codesign with entitlements
        step(&format!("Signing app bundle...{msg}"));

        // Some entitlements only work when the signature is backed by a provisioning
        // profile. Ad-hoc signatures carry no profile, so the system (launchd)
        // refuses to spawn the process with a cryptic "Launchd job spawn
        // failed" error. This helps the user to debug.
        if adhoc {
            self.validate_entitlements_for_adhoc(&ent_value);
        }

        codesign_cmd = codesign_cmd.arg_group(["--entitlements", ent_plist_path]);
        self.sh.run(codesign_cmd.arg(app_bundle))?;

        step("Verifying signature...");
        self.sh.run(&[
            "codesign",
            "--verify",
            "--deep",
            "--strict",
            "--verbose=2",
            app_bundle,
        ])?;

        Ok(())
    }

    /// Sign one nested extension bundle (`.appex` or `.systemextension`) with
    /// its own entitlements. Called by [`sign`] for each configured
    /// extension, after embedded dylibs are signed and before the host
    /// bundle is sealed.
    fn sign_extension(
        &self,
        ext: &ResolvedExtension,
        paths: &ExtensionPaths,
        base_cmd: &ShellCommand,
        adhoc: bool,
        msg: &str,
    ) -> Result<()> {
        let appex_str = paths
            .bundle
            .to_str()
            .context("Invalid extension bundle path.")?;
        let ent_json_path = &ext.entitlements_json_path;
        let ent_json_str = ent_json_path
            .to_str()
            .context("Invalid extension entitlements path.")?;
        let ent_plist_str = paths
            .entitlements_plist
            .to_str()
            .context("Invalid extension entitlements plist path.")?;

        let ent_raw = fs::read_to_string(ent_json_path).with_context(|| {
            format!(
                "Failed to read entitlements JSON for extension `{}` at {ent_json_str}",
                ext.name
            )
        })?;
        let ent_value: Value = serde_json::from_str(&ent_raw).with_context(|| {
            format!("Extension entitlements file is not valid JSON: {ent_json_str}")
        })?;
        self.sh.run(&[
            "plutil",
            "-convert",
            "xml1",
            ent_json_str,
            "-o",
            ent_plist_str,
        ])?;

        step(&format!("Signing extension `{}`...{msg}", ext.name));
        if adhoc {
            self.validate_entitlements_for_adhoc(&ent_value);
        }
        let appex_cmd = base_cmd
            .clone()
            .arg_group(["--entitlements", ent_plist_str])
            .arg(appex_str);
        appex_cmd.run(&self.sh)?;
        Ok(())
    }
}
