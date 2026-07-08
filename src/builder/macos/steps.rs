//! The individual stages of the build pipeline: compiling the binary,
//! assembling the `.app` bundle, embedding/signing libraries, signing,
//! notarizing, and packaging the DMG.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use color_print::cprintln;
use dmg::DmgSpec;
use serde_json::{Value, json};

use crate::builder::macos::notarize::NotarizationState;
use crate::builder::{MacosBuilder, step};
use crate::config::{ExtensionKind, ResolvedExtension};
use crate::paths::ExtensionPaths;
use crate::shell::ShellCommand;

impl MacosBuilder {
    pub fn clean(&self) -> Result<()> {
        step("Cleaning previous build...");
        let build_dir = &self.paths.build_dir;
        if build_dir.as_os_str().is_empty() || build_dir == Path::new("/") {
            bail!("build_dir is empty or root, refusing to clean");
        }

        if self.dry_run {
            let build_dir = build_dir.display();
            cprintln!("<dim>[dry-run]</dim> rm -rf {build_dir}");
            cprintln!("<dim>[dry-run]</dim> mkdir -p {build_dir}");
            return Ok(());
        }

        if build_dir.exists() {
            fs::remove_dir_all(build_dir)?;
        }
        fs::create_dir_all(build_dir)?;
        Ok(())
    }

    /// `swift build` wrapper, which also adds rpath if there are embedded
    /// libraries. Returns the swift build bin directory which contains the host
    /// binary and any extension binaries. Use [`Self::find_binary_in`] to
    /// locate a specific target's binary.
    pub fn build_binary(&self) -> Result<PathBuf> {
        let config_flag = if self.debug { "debug" } else { "release" };
        step(&format!("Building {config_flag} binary..."));

        // Build base args shared between both swift invocations
        let source = self.cfg.source_dir.to_str().unwrap();
        let mut build_cmd = ShellCommand::new("swift")
            .args(["build", "-c", config_flag, "--package-path", source])
            .envs(&self.cfg.build_env);

        // add archs from cfg
        for arch in &self.cfg.archs {
            build_cmd = build_cmd.args(["--arch", arch]);
        }

        // embed the Frameworks rpath at link time if we're embedding libraries
        if !self.cfg.embed_libs.is_empty() {
            build_cmd = build_cmd.arg_group([
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
            // dry-run: fall back to default swift location
            self.cfg.source_dir.join(format!(".build/{config_flag}"))
        } else {
            PathBuf::from(bin_dir)
        };
        Ok(bin_dir)
    }

    // assemble the .app bundle by creating the bundle structure:
    // 1. copy in the binary and other resources
    // 2. generate the Info.plist from the info JSON
    pub fn assemble_bundle(&self, binary_path: &Path) -> Result<PathBuf> {
        step("Assembling app bundle...");
        let app_bundle = &self.paths.app_bundle;
        let app_bundle_resources_dir = app_bundle.join("Contents/Resources");

        // create structure
        self.create_dir(&app_bundle.join("Contents/MacOS"))?;
        self.create_dir(&app_bundle_resources_dir)?;

        // copy the binary into the bundle
        self.copy_file(
            binary_path,
            &app_bundle.join("Contents/MacOS").join(&self.cfg.app_name),
        )?;

        // Read info JSON for Info.plist and extract the top-level object for inserting
        // keys.
        let mut info_json = self.build_info_json(
            self.cfg.info_json_path.clone(),
            HashMap::from([
                ("CFBundleExecutable".into(), self.cfg.app_name.clone()),
                ("CFBundleIdentifier".into(), self.cfg.bundle_id.clone()),
                ("CFBundlePackageType".into(), "APPL".into()),
            ]),
        )?;

        // set CFBundleIconFile if bundle icon_path is provided
        // todo: also support CFBundleIconName, the newer format.
        // see https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/CoreFoundationKeys.html
        if let Some(icon_path) = &self.cfg.icon_path {
            self.copy_file(icon_path, &app_bundle_resources_dir.join("AppIcon.icns"))?;
            info_json.insert("CFBundleIconFile".to_string(), json!("AppIcon.icns"));
        }

        // pipe JSON into plutil to produce Info.plist
        let json_bytes = serde_json::to_vec_pretty(&info_json)?;
        let plist_path = self
            .paths
            .info_plist
            .to_str()
            .context("Invalid Info.plist path.")?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        // copy provisioning profile if provided (for non-app-store macos apps)
        //
        // from apple: https://developer.apple.com/Documentation/technotes/tn3125-inside-code-signing-provisioning-profiles
        // In the early days of iOS development it was common to install a provisioning
        // profile on the device as a whole (in the Settings app). That’s still
        // possible, but current best practice is to embed the profile within the app
        // itself:
        //
        // - macOS expects to find the profile at
        //   MyApp.app/Contents/embedded.provisionprofile.
        // - Other Apple platforms expect to find the profile at
        //   MyApp.app/embedded.mobileprovision.
        //
        if let Some(profile_path) = &self.cfg.provisioning_profile {
            self.copy_file(
                profile_path,
                &app_bundle.join("Contents/embedded.provisionprofile"),
            )?;
        }

        // Copy user-configured resources_dir contents into Contents/Resources/.
        if let Some(rdir) = &self.cfg.resources_dir {
            step("Copying resource directory...");
            self.copy_tree(rdir, &app_bundle_resources_dir)?;
        }

        // Copy individual user-configured resource files into Contents/Resources/.
        if !self.cfg.resources.is_empty() {
            step("Copying resources...");
            for resource in &self.cfg.resources {
                let name = resource.file_name().with_context(|| {
                    format!("Resource path has no filename: {}", resource.display())
                })?;
                self.copy_file(resource, &app_bundle_resources_dir.join(name))?;
            }
        }

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
            let orig_install_name = if self.dry_run {
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
    /// `Contents/PlugIns/`:
    /// 1. Builds the extension `Info.plist` (injecting kind-specific
    ///    `NSExtension` keys)
    /// 2. Copies the binary
    /// 3. Copies the kind-specific resource payload (e.g. for Safari Web
    ///    Extensions, this might be the entire webpack output directory).
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

        let mut info_json = self.build_info_json(
            ext.info_json_path.clone(),
            HashMap::from([
                ("CFBundleExecutable".into(), ext.target_name.clone()),
                ("CFBundleIdentifier".into(), ext.bundle_id.clone()),
                ("CFBundleName".into(), ext.name.clone()),
                ("CFBundleDisplayName".into(), ext.name.clone()),
                ("CFBundleInfoDictionaryVersion".into(), "6.0".into()),
                // App extensions use the XPC bundle package type.
                ("CFBundlePackageType".into(), "XPC!".into()),
            ]),
        )?;

        match ext.kind {
            ExtensionKind::SafariWebExtension => {
                // `NSExtensionPrincipalClass` was filled in during resolve (todo: clean up, a
                // little spooky?)
                let principal = ext.principal_class.as_deref().unwrap_or("");
                info_json.insert(
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
                } else {
                    cprintln!(
                        "<yellow>warning:</yellow> App Extension `{}` is missing `principal_class`, which may be required depending on the extension point.",
                        ext.name
                    );
                }
                info_json.insert("NSExtension".into(), Value::Object(ns_ext));
            },
        }

        let json_bytes = serde_json::to_vec_pretty(&info_json)?;
        let plist_path = paths
            .info_plist
            .to_str()
            .context("Invalid extension Info.plist path.")?;
        self.sh.run_stdin(
            &["plutil", "-convert", "xml1", "-o", plist_path, "-"],
            &json_bytes,
        )?;

        // Copy the kind-specific resource payload. For Safari Web Extensions
        // this is the extension source code (manifest.json, JS, HTML, icons).
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
        // entitlements for the nested bundle, which is wrong: the
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
        // spawn failed" error. This helps the user to debug.
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

        // spctl may return non-zero for unnotarized bundles, warn but allow build to
        // continue. We only run this for release builds to debug notarization issues;
        // for dev builds the signature often fails, even with a Apple Developer
        // certificate.
        if spctl {
            let _ = self
                .sh
                .run(&[
                    "spctl",
                    "-a",
                    "-t",
                    "open",
                    "--context",
                    "context:primary-signature",
                    "-v",
                    app_bundle,
                ])
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
            .arg_group(["--entitlements", ent_plist_str])
            .arg(appex_str);
        appex_cmd.run(&self.sh)?;
        Ok(())
    }

    pub fn package_dmg(&self) -> Result<()> {
        let macos_settings = &self.macos;

        let app_name = &self.cfg.app_name;
        let vol_name = format!("{app_name} {}", self.cfg.version);
        let temp_dmg = &self.paths.strudel_temp_dmg;
        let temp_dmg_str = temp_dmg.to_str().unwrap();

        step("Creating DMG...");

        if let Some(dmg_cfg) = &macos_settings.dmg {
            if self.dry_run {
                cprintln!(
                    "<dim>[dry-run]</dim> hdiutil create -volname {:?} -format UDZO {}",
                    vol_name,
                    temp_dmg.display()
                );
                return Ok(());
            }
            fs::create_dir_all(&self.paths.strudel_dir)?;
            step("Configuring DMG window...");
            dmg::create(
                &DmgSpec {
                    vol_name,
                    app_name: app_name.clone(),
                    source_app: self.paths.app_bundle.clone(),
                    background: dmg_cfg.background.clone(),
                    window_width: dmg_cfg.window_width,
                    window_height: dmg_cfg.window_height,
                    icon_size: dmg_cfg.icon_size,
                    app_x: dmg_cfg.app_x,
                    app_y: dmg_cfg.app_y,
                    applications_x: dmg_cfg.applications_x,
                    applications_y: dmg_cfg.applications_y,
                },
                temp_dmg,
            )?;
        } else {
            // Plain UDZO: no custom window layout, use staging folder approach.
            let staging = &self.paths.dmg_staging;
            let staging_str = staging.to_str().unwrap();
            let staging_app = staging.join(format!("{app_name}.app"));
            let staging_applications = staging.join("Applications");

            if self.dry_run {
                cprintln!("<dim>[dry-run]</dim> rm -rf {staging_str}");
                cprintln!("<dim>[dry-run]</dim> mkdir -p {staging_str}");
                cprintln!(
                    "<dim>[dry-run]</dim> ln -s /Applications {}",
                    staging_applications.display()
                );
            } else {
                fs::create_dir_all(&self.paths.strudel_dir)?;
                if staging.exists() {
                    fs::remove_dir_all(staging)?;
                }
                fs::create_dir_all(staging)?;
                std::os::unix::fs::symlink("/Applications", &staging_applications)?;
            }

            self.sh.run(&[
                "cp",
                "-rp",
                self.paths.app_bundle.to_str().unwrap(),
                staging_app.to_str().unwrap(),
            ])?;

            self.sh.run(&[
                "hdiutil",
                "create",
                "-volname",
                &vol_name,
                "-srcfolder",
                staging_str,
                "-ov",
                "-format",
                "UDZO",
                temp_dmg_str,
            ])?;

            if !self.dry_run {
                fs::remove_dir_all(staging)?;
            }
        }

        Ok(())
    }

    pub fn notarize(&self) -> Result<()> {
        let temp_dmg = &self.paths.strudel_temp_dmg;
        let temp_dmg_str = temp_dmg.to_str().unwrap();

        step("Submitting DMG for notarization...");
        cprintln!(
            "<dim>Note: first-time notarization can take several hours. \
             Press Ctrl-C to stop: run `strudel release --resume` to continue later.</dim>"
        );

        let auth_args = self.notary_auth_args()?;
        let notarize_cmd = ShellCommand::new("xcrun")
            .args(["notarytool", "submit", temp_dmg_str])
            .args(auth_args.iter().cloned())
            .args(["--output-format", "json"]);

        let submit_out = self.sh.run(notarize_cmd)?;

        let uuid = if self.dry_run {
            "dry-run-uuid-0000".to_string()
        } else {
            let v: serde_json::Value = serde_json::from_str(&submit_out)
                .context("Failed to parse notarytool submit output as JSON")?;
            v["id"]
                .as_str()
                .context("notarytool submit output missing 'id' field")?
                .to_string()
        };

        cprintln!("  <dim>Submission ID: {uuid}</dim>");

        let pending = self.paths.pending_submission(&uuid);
        if !self.dry_run {
            fs::create_dir_all(&pending.dir)?;
            fs::rename(temp_dmg, &pending.dmg)?;
        } else {
            cprintln!("<dim>[dry-run]</dim> mkdir -p {}", pending.dir.display());
            cprintln!(
                "<dim>[dry-run]</dim> mv {} {}",
                temp_dmg.display(),
                pending.dmg.display()
            );
        }

        let state = NotarizationState {
            submitted_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            dmg_dest: self.paths.dmg.display().to_string(),
        };
        if !self.dry_run {
            fs::write(&pending.state, toml::to_string(&state)?)?;
        } else {
            cprintln!("<dim>[dry-run]</dim> write {}", pending.state.display());
        }

        self.poll_notarization(&uuid, &pending, &PathBuf::from(&state.dmg_dest), &auth_args)
    }

    fn build_info_json(
        &self,
        path: Option<PathBuf>,
        additional_data: HashMap<String, String>,
    ) -> Result<serde_json::Map<String, Value>> {
        let mut info_json = match path {
            Some(path) => {
                let path_str = path.display();
                fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read {path_str}."))
                    .and_then(|s| {
                        serde_json::from_str::<serde_json::Map<String, Value>>(&s)
                            .with_context(|| format!("Failed to parse {path_str}, must be a map."))
                    })?
            },
            None => serde_json::Map::new(),
        };
        for (k, v) in additional_data {
            info_json.insert(k, json!(v));
        }
        info_json.insert(
            "CFBundleShortVersionString".to_string(),
            json!(self.cfg.version.clone()),
        );
        info_json.insert(
            "CFBundleVersion".to_string(),
            json!(self.cfg.build_number.clone()),
        );
        Ok(info_json)
    }
}
