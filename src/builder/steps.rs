//! The individual stages of the build pipeline: compiling the binary,
//! assembling the `.app` bundle, embedding/signing libraries, signing,
//! notarizing, and packaging the DMG.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use indoc::formatdoc;
use serde_json::{Value, json};

use super::{Builder, step};
use crate::config::{ExtensionKind, NotaryAuth, ResolvedExtension};
use crate::paths::ExtensionPaths;
use crate::shell::ShellCommand;

impl Builder {
    pub fn clean(&self) -> Result<()> {
        step("Cleaning previous build...");
        if self.dry_run() {
            cprintln!(
                "<dim>[dry-run]</dim> rm -rf {}",
                self.paths.build_dir.display()
            );
            cprintln!(
                "<dim>[dry-run]</dim> mkdir -p {}",
                self.paths.build_dir.display()
            );
            return Ok(());
        }
        if self.paths.build_dir.exists() {
            fs::remove_dir_all(&self.paths.build_dir)?;
        }
        fs::create_dir_all(&self.paths.build_dir)?;
        Ok(())
    }

    /// At a high-level, this wraps `swift build`. Returns the swift build bin
    /// directory, which contains the host binary and any extension binaries.
    /// Use [`Self::find_binary_in`] to locate a specific target's binary.
    pub fn build_binary(&self) -> Result<PathBuf> {
        let (config_flag, config_name) = if self.debug {
            ("debug", "debug")
        } else {
            ("release", "release")
        };
        step(&format!("Building {config_name} binary..."));

        // Build base args shared between both swift invocations
        let source = self.cfg.source_dir.to_str().unwrap();
        let mut build_cmd = ShellCommand::new("swift")
            .args(&["build", "-c", config_flag, "--package-path", source])
            .envs(&self.cfg.build_env);

        // add archs from cfg
        for arch in &self.cfg.archs {
            build_cmd = build_cmd.args(&["--arch", arch]);
        }

        // embed the Frameworks rpath at link time if we're embedding libraries
        if !self.cfg.embed_libs.is_empty() {
            build_cmd = build_cmd.arg_group(&[
                "-Xlinker",
                "-rpath",
                "-Xlinker",
                "@executable_path/../Frameworks",
            ]);
        }

        // clone of the build command with --show-bin-path to find the binary after
        // building
        let show_bin_cmd = build_cmd.clone().arg("--show-bin-path").hide_dry_run();

        // run the build_cmd
        self.sh.run_streamed_env(build_cmd)?;

        // run show_bin_cmd to find the swift build output directory
        let bin_dir = show_bin_cmd.run(&self.sh)?;
        let bin_dir = bin_dir.trim();
        let bin_dir = if bin_dir.is_empty() {
            // dry-run: fall back to expected location
            self.cfg.source_dir.join(format!(".build/{config_name}"))
        } else {
            PathBuf::from(bin_dir)
        };
        Ok(bin_dir)
    }

    /// Locate the binary for `target_name` in the swift build output. In
    /// dry-run, returns the expected path without checking the filesystem.
    /// On a real run with the binary missing, emits a hint listing the
    /// executables that *were* built, so users can fix `target_name`.
    pub fn find_binary_in(&self, bin_dir: &Path, target_name: &str) -> Result<PathBuf> {
        let binary_path = bin_dir.join(target_name);
        if self.dry_run() {
            return Ok(binary_path);
        }
        if binary_path.exists() {
            return Ok(binary_path);
        }

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
            formatdoc! {r#"
                Executables found in the build directory: {}.
                If one of these is the right binary, set the matching `target_name` in your strudel.toml.
                "#,
                found.join(", ")
            }
        };
        bail!(formatdoc! {r#"
            Could not locate built binary at:
            {}
            strudel was looking for an executable named `{target_name}`.
            {hint}
            "#,
            binary_path.display(),
        });
    }

    // assemble the .app bundle by creating the bundle structure:
    // 1. copy in the binary and other resources
    // 2. generate the Info.plist from the info JSON
    pub fn assemble_bundle(&self, binary_path: &Path) -> Result<PathBuf> {
        step("Assembling app bundle...");
        let app_bundle = &self.paths.app_bundle;

        // create structure
        self.create_dir(&app_bundle.join("Contents/MacOS"))?;
        self.create_dir(&app_bundle.join("Contents/Resources"))?;

        // copy the binary into the bundle
        self.copy_file(
            binary_path,
            &app_bundle.join("Contents/MacOS").join(&self.cfg.app_name),
        )?;

        // Read info JSON for Info.plist
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
            },
            None => Value::Object(Default::default()),
        };

        // set/override version and identifier keys
        let obj = info
            .as_object_mut()
            .context("Info JSON must be a JSON object at the top level.")?;
        obj.insert(
            "CFBundleExecutable".to_string(),
            json!(self.cfg.app_name.clone()),
        );
        obj.insert(
            "CFBundleShortVersionString".to_string(),
            json!(self.cfg.version.clone()),
        );
        obj.insert(
            "CFBundleVersion".to_string(),
            json!(self.cfg.build_number.clone()),
        );
        obj.insert(
            "CFBundleIdentifier".to_string(),
            json!(self.cfg.bundle_id.clone()),
        );

        // set CFBundleIconFile if bundle icon_path is provided
        // todo: also support CFBundleIconName, the newer format.
        // see https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/CoreFoundationKeys.html
        if let Some(icon_path) = &self.cfg.icon_path {
            self.copy_file(
                icon_path,
                &app_bundle.join("Contents/Resources/AppIcon.icns"),
            )?;
            obj.insert("CFBundleIconFile".to_string(), json!("AppIcon.icns"));
        }

        // copy provisioning profile if provided
        if let Some(profile_path) = &self.cfg.provisioning_profile {
            self.copy_file(
                profile_path,
                &app_bundle.join("Contents/embedded.provisionprofile"),
            )?;
        }

        // pipe JSON into plutil to produce Info.plist
        let json_bytes = serde_json::to_vec_pretty(&info)?;
        let plist_path = self
            .paths
            .info_plist
            .to_str()
            .context("Invalid Info.plist path.")?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        let resources_dir = app_bundle.join("Contents/Resources");

        // Copy user-configured resources_dir contents into Contents/Resources/.
        if let Some(rdir) = &self.cfg.resources_dir {
            step("Copying resource directory...");
            self.copy_tree(rdir, &resources_dir)?;
        }

        // Copy individual user-configured resource files into Contents/Resources/.
        if !self.cfg.resources.is_empty() {
            step("Copying resources...");
            for resource in &self.cfg.resources {
                let name = resource.file_name().with_context(|| {
                    format!(
                        "Resource path has no filename: {}",
                        resource.display()
                    )
                })?;
                self.copy_file(resource, &resources_dir.join(name))?;
            }
        }

        // note: we don't really need PkgInfo, it's a legacy file
        // self.write_file(&app_bundle.join("Contents/PkgInfo"), "APPL????")?;
        Ok(app_bundle.clone())
    }

    /// Embed dynamic libraries into `Contents/Frameworks`, fix their install
    /// names and the executable's rpath, so the bundle is self-contained.
    /// No-op when `cfg.embed_libs` is empty.
    pub fn embed_libraries(&self, app_bundle: &Path) -> Result<()> {
        if self.cfg.embed_libs.is_empty() {
            return Ok(());
        }

        step("Embedding dynamic libraries...");

        let frameworks_dir = app_bundle.join("Contents/Frameworks");
        self.create_dir(&frameworks_dir)?;

        let executable = app_bundle.join("Contents/MacOS").join(&self.cfg.app_name);
        let executable_str = executable
            .to_str()
            .context("embed: Invalid executable path.")?;

        for lib_path in &self.cfg.embed_libs {
            let file_name = lib_path.file_name().with_context(|| {
                format!("embed_libs entry has no filename: {}", lib_path.display())
            })?;
            let dest = frameworks_dir.join(file_name);
            let file_name_str = file_name.to_str().context("embed: Invalid file name.")?;
            let rpath_entry = format!("@rpath/{file_name_str}");
            self.copy_file(lib_path, &dest)?;

            // Find the original install name as seen by the executable.
            let otool_out = self.sh.run(&["otool", "-L", executable_str])?;
            let orig_install_name = if self.dry_run() {
                // In dry-run we can't run otool; use the filename as a stand-in.
                format!("<otool:{file_name_str}>")
            } else {
                otool_out
                    .lines()
                    .skip(1)
                    .map(|l| l.split_whitespace().next().unwrap_or(""))
                    .find(|name| {
                        Path::new(name)
                            .file_name()
                            .map(|n| n == file_name)
                            .unwrap_or(false)
                    })
                    .map(|s| s.to_string())
                    .with_context(|| {
                        format!(
                            "Could not find {file_name_str} in `otool -L {executable_str}`.\n\
                             Ensure your Package.swift links this library."
                        )
                    })?
            };

            // Update the dylib (at `dest_str`): change install name to @rpath/{dylib_name}
            let dest_str = dest.to_str().context("embed: Invalid destination path.")?;
            self.sh
                .run(&["install_name_tool", "-id", &rpath_entry, dest_str])?;

            // Updated the executable (at `executable_str`): change its reference to the
            // dylib from the `orig_install_name` to `@rpath/{dylib_name}``.
            self.sh.run(&[
                "install_name_tool",
                "-change",
                &orig_install_name,
                &rpath_entry,
                executable_str,
            ])?;
        }

        Ok(())
    }

    /// Assemble one app extension `.appex` bundle under the host's
    /// `Contents/PlugIns/`. Builds the extension `Info.plist` (injecting
    /// kind-specific `NSExtension` keys), copies the binary, and copies the
    /// kind-specific resource payload (for Safari Web Extensions, the entire
    /// webpack output directory).
    pub fn assemble_appex(
        &self,
        ext: &ResolvedExtension,
        paths: &ExtensionPaths,
        bin_dir: &Path,
    ) -> Result<()> {
        step(&format!("Assembling extension `{}`...", ext.name));

        // Locate the extension binary that `swift build` produced. Building
        // all package targets is intentional: a Package.swift that declares
        // both the host and the extension as executableTargets produces both
        // binaries from a single `swift build` invocation.
        let binary_path = self.find_binary_in(bin_dir, &ext.target_name)?;

        self.create_dir(&paths.appex.join("Contents/MacOS"))?;
        self.create_dir(&paths.resources)?;
        self.copy_file(&binary_path, &paths.binary)?;

        // Start from any user-supplied JSON, then layer required keys on top.
        let mut info: Value = match &ext.info_json_path {
            Some(p) => {
                let s = fs::read_to_string(p).with_context(|| {
                    format!(
                        "Failed to read extension info JSON at {} (set in \
                         `info_json_path` for extension `{}` in strudel.toml).",
                        p.display(),
                        ext.name
                    )
                })?;
                serde_json::from_str(&s).with_context(|| {
                    format!("Failed to parse extension info JSON at {}", p.display())
                })?
            },
            None => Value::Object(Default::default()),
        };
        let obj = info.as_object_mut().with_context(|| {
            format!(
                "Extension info JSON for `{}` must be a JSON object at the top level.",
                ext.name
            )
        })?;
        obj.insert("CFBundleName".into(), json!(ext.name.clone()));
        obj.insert("CFBundleDisplayName".into(), json!(ext.name.clone()));
        obj.insert("CFBundleExecutable".into(), json!(ext.target_name.clone()));
        obj.insert("CFBundleIdentifier".into(), json!(ext.bundle_id.clone()));
        obj.insert(
            "CFBundleShortVersionString".into(),
            json!(self.cfg.version.clone()),
        );
        obj.insert(
            "CFBundleVersion".into(),
            json!(self.cfg.build_number.clone()),
        );
        obj.insert("CFBundleInfoDictionaryVersion".into(), json!("6.0"));
        // App extensions use the XPC bundle package type.
        obj.insert("CFBundlePackageType".into(), json!("XPC!"));

        match ext.kind {
            ExtensionKind::SafariWebExtension => {
                // `NSExtensionPrincipalClass` was filled in during resolve.
                let principal = ext.principal_class.as_deref().unwrap_or("");
                obj.insert(
                    "NSExtension".into(),
                    json!({
                        "NSExtensionPointIdentifier": "com.apple.Safari.web-extension",
                        "NSExtensionPrincipalClass": principal,
                        "SFSafariWebExtensionManifestPath": "Resources/manifest.json",
                    }),
                );
            },
            ExtensionKind::AppExtension => {
                let ident = ext.extension_point_identifier.as_deref().unwrap_or("");
                let mut ns_ext = serde_json::Map::new();
                ns_ext.insert("NSExtensionPointIdentifier".into(), json!(ident));
                if let Some(class) = ext.principal_class.as_deref() {
                    ns_ext.insert("NSExtensionPrincipalClass".into(), json!(class));
                }
                obj.insert("NSExtension".into(), Value::Object(ns_ext));
            },
        }

        let json_bytes = serde_json::to_vec_pretty(&info)?;
        let plist_path = paths
            .info_plist
            .to_str()
            .context("Invalid extension Info.plist path.")?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        // Copy the kind-specific resource payload. For Safari Web Extensions
        // this is the entire webpack output (manifest.json, JS, HTML, icons).
        if let Some(src_resources) = &ext.resources_dir {
            self.copy_tree(src_resources, &paths.resources)?;
        }

        Ok(())
    }

    pub fn sign(&self, spctl: bool) -> Result<()> {
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

        // With no identity configured, sign ad-hoc (`--sign -`): no certificate
        // or account needed, enough to exercise entitlements locally. A real
        // identity (and notarization) is required to distribute — see `release`.
        let adhoc = self.cfg.sign_identity.is_empty();

        // Create the base codesign command. The hardened runtime and a trusted
        // timestamp both require a real Developer ID certificate; skip them for
        // ad-hoc signatures.
        //
        // This is necessary to prevent crashes when the app attempts to load embedded
        // frameworks that do not share the same Team ID.
        let (identity, msg) = if adhoc {
            ("-", " (ad-hoc — no signing identity configured)...")
        } else {
            (self.cfg.sign_identity.as_str(), "")
        };

        if !adhoc && self.validate_sign_identity()? {
            self.validate_entitlements_for_adhoc(&ent_value);
        }

        let mut codesign_cmd = ShellCommand::new("codesign").args(&["--force", "--sign", identity]);
        if !adhoc {
            codesign_cmd = codesign_cmd.args(&["--options", "runtime", "--timestamp"]);
        }

        // Sign each embedded dylib individually before signing the bundle.
        // codesign --verify --deep --strict and notarization both require nested
        // Mach-O files to carry valid signatures.
        if !self.cfg.embed_libs.is_empty() {
            step(&format!("Signing embedded libraries...{msg}"));
            let frameworks_dir = self.paths.app_bundle.join("Contents/Frameworks");
            for lib_path in &self.cfg.embed_libs {
                if let Some(file_name) = lib_path.file_name() {
                    let mut lib_codesign_cmd = codesign_cmd.clone();
                    let dylib = frameworks_dir.join(file_name);
                    let dylib_str = dylib.to_str().unwrap();
                    lib_codesign_cmd = lib_codesign_cmd.arg(dylib_str);
                    lib_codesign_cmd.run(&self.sh)?;
                }
            }
        }

        // Sign extensions inside-out: each `.appex` must be signed with its
        // own entitlements before the host bundle is sealed. A single
        // `codesign --deep` pass over the host would re-use the host's
        // entitlements for the nested bundle, which is wrong — the
        // extension is sandboxed independently and typically needs a
        // different set.
        for (ext, ext_paths) in self.cfg.extensions.iter().zip(self.paths.extensions.iter()) {
            self.sign_extension(ext, ext_paths, &codesign_cmd, adhoc, msg)?;
        }

        // Run bundle codesign with entitlements
        step(&format!("Signing app bundle...{msg}"));

        // Some entitlements only work when the signature is backed by a
        // provisioning profile. Ad-hoc signatures carry no profile, so the system
        // (launchd) refuses to spawn the process with a cryptic "Launchd job
        // spawn failed" error. This helps the user to debug
        if adhoc {
            self.validate_entitlements_for_adhoc(&ent_value);
        }

        codesign_cmd = codesign_cmd.arg_group(&["--entitlements", ent_plist_path]);
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

        if spctl {
            // spctl may return non-zero for unnotarized bundles, warn but allow build to
            // continue. We only run this for release builds to debug notarization issues;
            // for dev builds the signature often fails, even with a Apple Developer
            // certificate.
            let _ = self
                .sh
                .run(&["spctl", "--assess", "-vv", "--type", "exec", app_bundle])
                .inspect_err(|e| {
                    cprintln!("<yellow>warning:</yellow> spctl assessment failed: {e}")
                });
        }

        Ok(())
    }

    /// Sign one nested `.appex` with its own entitlements. Called by [`sign`]
    /// for each configured extension, after embedded dylibs are signed and
    /// before the host bundle is sealed.
    fn sign_extension(
        &self,
        ext: &ResolvedExtension,
        paths: &ExtensionPaths,
        base_cmd: &ShellCommand,
        adhoc: bool,
        msg: &str,
    ) -> Result<()> {
        let appex_str = paths
            .appex
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
            .arg_group(&["--entitlements", ent_plist_str])
            .arg(appex_str);
        appex_cmd.run(&self.sh)?;
        Ok(())
    }

    pub fn notarize(&self) -> Result<()> {
        step("Creating zip for notarization...");
        let app_bundle = self.paths.app_bundle.to_str().unwrap();
        let zip = self.paths.zip.to_str().unwrap();

        ShellCommand::new("ditto")
            // -c: create an archive
            // -k: use zip format
            // --keepParent: include the parent directory in the archive, so the .app bundle
            // structure is preserved.
            .args(&["-c", "-k", "--keepParent", app_bundle, zip])
            .run(&self.sh)?;

        step("Stapling notarization ticket...");
        self.sh.run(&["xcrun", "stapler", "staple", app_bundle])?;
        self.sh.run(&["xcrun", "stapler", "validate", app_bundle])?;

        Ok(())
    }

    pub fn package_dmg(&self) -> Result<()> {
        let app_bundle = self.paths.app_bundle.to_str().unwrap();
        let dmg = self.paths.dmg.to_str().unwrap();
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
            },
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
            },
            None => {
                if self.dry_run() {
                    cprintln!("<red>Error: No notarization credentials configured.</red>");
                    let auth = [
                        "--key".into(),
                        "MISSING!".into(),
                        "--key-id".into(),
                        "MISSING!".into(),
                        "--issuer".into(),
                        "MISSING!".into(),
                    ];
                    display.extend(auth.clone());
                    args.extend(auth);
                } else {
                    // preflight_credentials should guarantee a complete set before `run`.
                    bail!("No notarization credentials configured");
                }
            },
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
}
