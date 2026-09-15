//! Code signing: [`MacosBuilder::sign`] and [`MacosBuilder::sign_extension`].

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::builder::profile::application_identifier;
use crate::builder::{MacosBuilder, is_framework};
use crate::config::ResolvedExtension;
use crate::paths::ExtensionPaths;
use crate::shell::ShellCommand;

impl MacosBuilder {
    /// Signs the app bundle (and any extensions). Returns whether the
    /// signature ended up ad-hoc (no signing identity configured), so
    /// callers can warn about it once the pipeline finishes.
    pub fn sign(&self) -> Result<bool> {
        let app_bundle = self.paths.app_bundle.to_str().unwrap();
        let ent_plist_path = self.paths.entitlements_plist.to_str().unwrap();

        let mut ent_value: Value = match self.cfg.entitlements_json_path {
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
        // With no identity configured, sign ad-hoc (`--sign -`): no certificate or
        // account needed, enough to exercise entitlements locally. A real
        // identity (and notarization) is required to distribute. See `release`.
        let adhoc = self.cfg.sign_identity.is_empty();

        let profile = self.embedded_profile(
            &self.paths.app_bundle,
            self.cfg.provisioning_profile.as_deref(),
            &self.cfg.bundle_id,
        )?;
        if let Some(profile) = &profile
            && !adhoc
        {
            inject_identifier_entitlements(&mut ent_value, profile);
        }

        let ent_bytes = serde_json::to_vec_pretty(&ent_value)?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", ent_plist_path, "-"],
            &ent_bytes,
        )?;

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
            self.step(&format!("Signing embedded libraries...{msg}"));
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
            self.step(&format!("Signing copied files...{msg}"));
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
        self.step(&format!("Signing app bundle...{msg}"));

        // Some entitlements only work when the signature is backed by a provisioning
        // profile. Ad-hoc signatures carry no profile, so the system (launchd)
        // refuses to spawn the process with a cryptic "Launchd job spawn
        // failed" error. This helps the user to debug.
        if adhoc {
            self.validate_entitlements_for_adhoc(&ent_value);
        }

        codesign_cmd = codesign_cmd.arg_group(["--entitlements", ent_plist_path]);
        self.sh.run(codesign_cmd.arg(app_bundle))?;

        self.step("Verifying signature...");
        self.sh.run(&[
            "codesign",
            "--verify",
            "--deep",
            "--strict",
            "--verbose=2",
            app_bundle,
        ])?;

        Ok(adhoc)
    }

    /// Validate and decode the provisioning profile embedded in `bundle`.
    /// Returns `None` when `profile_path` is unset or the bundle has no
    /// `Contents/embedded.provisionprofile`, e.g. a managed profile that was
    /// never fetched, or a dry run where nothing was copied.
    fn embedded_profile(
        &self,
        bundle: &Path,
        profile_path: Option<&Path>,
        bundle_id: &str,
    ) -> Result<Option<plist::Value>> {
        let Some(profile_path) = profile_path else {
            return Ok(None);
        };
        if !bundle.join("Contents/embedded.provisionprofile").exists() {
            return Ok(None);
        }
        self.validate_provisioning_profile(profile_path, bundle_id)
            .map(Some)
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
        let mut ent_value: Value = serde_json::from_str(&ent_raw).with_context(|| {
            format!("Extension entitlements file is not valid JSON: {ent_json_str}")
        })?;
        let profile = self.embedded_profile(
            &paths.bundle,
            ext.provisioning_profile.as_deref(),
            &ext.bundle_id,
        )?;
        if let Some(profile) = &profile
            && !adhoc
        {
            inject_identifier_entitlements(&mut ent_value, profile);
        }
        let ent_bytes = serde_json::to_vec_pretty(&ent_value)?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", ent_plist_str, "-"],
            &ent_bytes,
        )?;

        self.step(&format!("Signing extension `{}`...{msg}", ext.name));
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

/// Copy the application identifier and team identifier granted by `profile`
/// into `ent_value`, as Xcode does when signing with a profile.
///
/// Callers must only use this for a bundle that embeds `profile`. These
/// entitlements are restricted: a bundle that carries them without a profile
/// granting them is refused by launchd ("Launchd job spawn failed").
///
/// Values come from the profile rather than `team_id` in the config, so they
/// match the profile even when `team_id` is unset or differs. Keys already
/// present in `ent_value` are preserved.
fn inject_identifier_entitlements(ent_value: &mut Value, profile: &plist::Value) {
    let Value::Object(map) = ent_value else {
        return;
    };
    let Some(dict) = profile.as_dictionary() else {
        return;
    };
    let profile_ents = dict
        .get("Entitlements")
        .and_then(plist::Value::as_dictionary);

    if let Some(app_id) = profile_ents.and_then(application_identifier) {
        map.entry("com.apple.application-identifier")
            .or_insert_with(|| app_id.into());
    }

    let team_id = profile_ents
        .and_then(|e| e.get("com.apple.developer.team-identifier"))
        .and_then(plist::Value::as_string)
        .or_else(|| {
            dict.get("TeamIdentifier")
                .and_then(plist::Value::as_array)
                .and_then(|a| a.first())
                .and_then(plist::Value::as_string)
        });
    if let Some(team_id) = team_id {
        map.entry("com.apple.developer.team-identifier")
            .or_insert_with(|| team_id.into());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use plist::Dictionary;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::builder::OutputFlags;
    use crate::config::fixtures::resolved_macos;

    fn profile(entitlements: Option<Dictionary>, team_ids: &[&str]) -> plist::Value {
        let mut dict = Dictionary::new();
        if let Some(ents) = entitlements {
            dict.insert("Entitlements".into(), plist::Value::Dictionary(ents));
        }
        dict.insert(
            "TeamIdentifier".into(),
            plist::Value::Array(team_ids.iter().map(|t| (*t).into()).collect()),
        );
        plist::Value::Dictionary(dict)
    }

    fn developer_id_entitlements() -> Dictionary {
        let mut ents = Dictionary::new();
        ents.insert(
            "com.apple.application-identifier".into(),
            "PROFTEAM01.com.example.app".into(),
        );
        ents.insert(
            "com.apple.developer.team-identifier".into(),
            "PROFTEAM01".into(),
        );
        ents
    }

    #[test]
    fn identifiers_are_copied_from_the_profile() {
        let mut ents = json!({ "com.apple.security.app-sandbox": true });
        inject_identifier_entitlements(
            &mut ents,
            &profile(Some(developer_id_entitlements()), &["PROFTEAM01"]),
        );
        assert_eq!(
            ents,
            json!({
                "com.apple.security.app-sandbox": true,
                "com.apple.application-identifier": "PROFTEAM01.com.example.app",
                "com.apple.developer.team-identifier": "PROFTEAM01",
            })
        );
    }

    #[test]
    fn unprefixed_application_identifier_is_written_under_the_prefixed_key() {
        let mut profile_ents = Dictionary::new();
        profile_ents.insert(
            "application-identifier".into(),
            "PROFTEAM01.com.example.app".into(),
        );
        let mut ents = json!({});
        inject_identifier_entitlements(&mut ents, &profile(Some(profile_ents), &["PROFTEAM01"]));
        assert_eq!(
            ents,
            json!({
                "com.apple.application-identifier": "PROFTEAM01.com.example.app",
                "com.apple.developer.team-identifier": "PROFTEAM01",
            })
        );
    }

    #[test]
    fn team_identifier_falls_back_to_the_profile_team() {
        let mut ents = json!({});
        inject_identifier_entitlements(&mut ents, &profile(None, &["PROFTEAM01"]));
        assert_eq!(
            ents,
            json!({ "com.apple.developer.team-identifier": "PROFTEAM01" })
        );
    }

    #[test]
    fn user_entitlements_are_preserved() {
        let mut ents = json!({
            "com.apple.application-identifier": "USER.value",
            "com.apple.developer.team-identifier": "USER",
        });
        let before = ents.clone();
        inject_identifier_entitlements(
            &mut ents,
            &profile(Some(developer_id_entitlements()), &["PROFTEAM01"]),
        );
        assert_eq!(ents, before);
    }

    #[test]
    fn no_embedded_profile_when_the_bundle_has_none() {
        // A configured profile path is not enough: without a profile inside
        // the bundle, nothing is validated or injected.
        let dir = tempdir().unwrap();
        let mut cfg = resolved_macos();
        cfg.sign_identity = "Developer ID Application: Me (TEAM123456)".into();
        cfg.team_id = "TEAM123456".into();
        let b = MacosBuilder::new(
            cfg,
            OutputFlags::default(),
            false,
            false,
            None,
            false,
            false,
        )
        .unwrap();
        let configured = PathBuf::from("/x/.strudel/com.example.app.provisionprofile");
        let profile = b
            .embedded_profile(dir.path(), Some(configured.as_path()), "com.example.app")
            .unwrap();
        assert!(profile.is_none());
        assert!(
            b.embedded_profile(dir.path(), None, "com.example.app")
                .unwrap()
                .is_none()
        );
    }
}
