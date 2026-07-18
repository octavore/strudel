//! [`BuilderCore`] holds the resolved config, paths, and a [`Shell`], plus the
//! helpers shared by every platform. This is wrapped by platform-specific
//! drivers:
//!
//! - [`MacosBuilder`] for macOS apps (compile, assemble, sign, notarize,
//!   package a DMG).
//! - [`IosBuilder`] for iOS apps (simulator, device, provisioning).
//!
//! Both deref to [`BuilderCore`], so shared state (`self.cfg`, `self.sh`,
//! `self.paths`, etc) and shared helpers are accessible from both. The
//! work is split across submodules:
//!
//! - [`fs`] dry-run-aware filesystem helpers (on [`BuilderCore`])
//! - [`keychain`] signing-credential preflight and certificate import
//! - [`macos`] the macOS pipeline stages (todo: move MacOSBuilder here)
//! - [`ios`] the iOS pipeline stages (todo: move IosBuilder here)

mod fs;
mod ios;
pub(crate) mod keychain;
mod macos;

use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use color_print::{cformat, cprintln};
use indoc::formatdoc;
pub(crate) use ios::decode_profile;

use crate::config::{
    ResolvedConfig, ResolvedIosSection, ResolvedMacOsSection, ResolvedTargetPlatform,
};
use crate::paths::Paths;
use crate::shell::Shell;

/// State and helpers shared by every platform driver.
pub struct BuilderCore {
    cfg: ResolvedConfig,
    paths: Paths,
    sh: Shell,
    dry_run: bool,
    debug: bool,
}

/// The macOS build pipeline. Wraps a [`BuilderCore`] and the resolved
/// `[macos]` config section.
pub struct MacosBuilder {
    core: BuilderCore,
    macos: ResolvedMacOsSection,
    open: bool,
    resume: Option<String>,
    skip_notarization: bool,
    /// Mac App Store channel (`strudel release --mas`): sign with Apple
    /// Distribution + Installer certs, package a `.pkg`, upload via `altool`
    /// instead of DMG + notarization.
    mas: bool,
    /// Trims interactive-only output (e.g. the per-second notarization
    /// countdown) that's noisy in captured CI logs but harmless on a real
    /// terminal.
    ci: bool,
}

/// The iOS build pipeline. Wraps a [`BuilderCore`] and the resolved `[ios]`
/// config section.
pub struct IosBuilder {
    core: BuilderCore,
    ios: ResolvedIosSection,
}

impl Deref for MacosBuilder {
    type Target = BuilderCore;
    fn deref(&self) -> &BuilderCore {
        &self.core
    }
}

impl Deref for IosBuilder {
    type Target = BuilderCore;
    fn deref(&self) -> &BuilderCore {
        &self.core
    }
}

/// Print a green progress header for a build step.
fn step(msg: &str) {
    cprintln!("\n<green>==>> {msg}</green>");
}

/// Read `actool`'s `--output-partial-info-plist` output (an XML plist
/// fragment, typically just `CFBundleIconName`/`CFBundleIcons`) and decode
/// it straight into JSON via `plist`'s serde support, so it can be merged
/// into the bundle's Info.plist JSON object - consistent with how the rest
/// of the codebase reads plists (`plist::Value::from_reader` in
/// `builder::macos::validators`/`builder::ios::profile`) rather than
/// shelling out to `plutil`. Shared by the macOS and iOS bundlers, both of
/// which compile icons/asset catalogs via `actool`.
pub(crate) fn read_partial_info_plist(
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let value: serde_json::Value = plist::from_file(path)
        .with_context(|| format!("Failed to read partial Info.plist at {}", path.display()))?;
    match value {
        serde_json::Value::Object(map) => Ok(map),
        _ => bail!(
            "Partial Info.plist at {} is not a dictionary",
            path.display()
        ),
    }
}

impl BuilderCore {
    fn new(cfg: ResolvedConfig, dry_run: bool, debug: bool) -> Self {
        BuilderCore {
            paths: Paths::new(&cfg),
            sh: Shell::new(dry_run),
            dry_run,
            debug,
            cfg,
        }
    }

    /// Locate the binary for `target_name` in the swift build output dir. In
    /// dry-run, returns the expected path without checking the filesystem.
    /// On a real run with the binary missing, emits a hint listing the
    /// executables that *were* built, so users can fix `target_name`.
    pub fn find_binary_in(&self, bin_dir: &Path, target_name: &str) -> Result<PathBuf> {
        let binary_path = bin_dir.join(target_name);
        if self.dry_run {
            return Ok(binary_path);
        }
        if binary_path.exists() {
            return Ok(binary_path);
        }

        // The rest of this function is only for the error message when the binary is
        // missing on a real run.

        // Collect extension-free filenames (i.e. executables) for the error hint.
        let found: Vec<String> = std::fs::read_dir(bin_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                (e.file_type().ok()?.is_file() && !name.contains('.')).then_some(name)
            })
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

    /// User-facing clean: wipe the strudel output dir and run `swift package
    /// clean`. Platform-agnostic - macOS and iOS targets both build into
    /// `build_dir`, so the same cleanup applies to either.
    pub fn clean_command(&self) -> Result<()> {
        let source = self.cfg.source_dir.to_str().unwrap();
        let build_dir = &self.paths.build_dir;

        if build_dir.as_os_str().is_empty() || build_dir == Path::new("/") {
            anyhow::bail!("build_dir is empty or root, refusing to clean");
        }

        step("Cleaning strudel output...");
        let prefix = if self.dry_run { "[dry-run] " } else { "" };
        cprintln!("<dim>{prefix}rm -rf {}</dim>", build_dir.display());
        if !self.dry_run && build_dir.exists() {
            std::fs::remove_dir_all(build_dir)?;
        }

        step("Cleaning Swift build cache...");
        self.sh
            .run(&["swift", "package", "clean", "--package-path", source])?;

        println!();
        cprintln!("<green>Done!</green>");
        Ok(())
    }
}

/// Clean a single target's build output. Platform-agnostic, so `strudel
/// clean` can tidy macOS and iOS targets alike without picking a driver.
pub fn clean(cfg: ResolvedConfig, dry_run: bool) -> Result<()> {
    BuilderCore::new(cfg, dry_run, false).clean_command()
}

impl MacosBuilder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: ResolvedConfig,
        dry_run: bool,
        open: bool,
        debug: bool,
        resume: Option<String>,
        skip_notarization: bool,
        mas: bool,
        ci: bool,
    ) -> Result<Self> {
        let ResolvedTargetPlatform::Mac(ref macos) = cfg.target_platform else {
            bail!("MacosBuilder constructed for a non-macOS target");
        };
        let macos = macos.clone();
        Ok(MacosBuilder {
            core: BuilderCore::new(cfg, dry_run, debug),
            macos,
            open,
            resume,
            skip_notarization,
            mas,
            ci,
        })
    }

    /// Swap in the MAS signing identity, provisioning profile, and
    /// entitlements ahead of `sign()`, so the shared signing pipeline signs
    /// with Apple Distribution instead of Developer ID. `sign()` itself needs
    /// no MAS-specific logic: it always signs whatever is in
    /// `self.cfg.sign_identity`/`provisioning_profile`/
    /// `entitlements_json_path`.
    fn apply_mas_overrides(&mut self) -> Result<()> {
        let sign_identity = self.cfg.mas_sign_identity.clone().ok_or_else(|| {
            anyhow!(
                "`--mas` requires [apple.mas] identity (an \"Apple Distribution: ...\" \
                 identity). See `strudel help app-store`."
            )
        })?;
        if self.cfg.mas_installer_identity.is_none() {
            bail!(
                "`--mas` requires [apple.mas] installer_identity (a \"3rd Party Mac Developer \
                 Installer: ...\" or \"Apple Distribution Installer: ...\" identity). See \
                 `strudel help app-store`."
            );
        }
        self.core.cfg.entitlements_json_path = self.cfg.mas_entitlements_json_path.clone();
        self.warn_if_mas_entitlements_missing_sandbox();
        self.core.cfg.sign_identity = sign_identity;
        self.core.cfg.provisioning_profile = self.cfg.mas_provisioning_profile.clone();
        Ok(())
    }

    /// Assemble every configured app extension under
    /// `<app>.app/Contents/PlugIns/`. No-op when no extensions are configured.
    fn assemble_extensions(&self, bin_dir: &Path) -> Result<()> {
        for (ext, ext_paths) in self.cfg.extensions.iter().zip(self.paths.extensions.iter()) {
            self.assemble_appex(ext, ext_paths, bin_dir)?;
        }
        Ok(())
    }

    fn open_app(&self) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        if self.open {
            let app_bundle = self.paths.app_bundle.to_str().unwrap();
            self.sh.run(&["open", app_bundle])?;
        }
        Ok(())
    }

    /// Build bundle only (clean -> binary -> assemble).
    pub fn bundle(&self) -> Result<()> {
        self.clean()?;
        let bin_dir = self.build_binary()?;
        let host_binary = self.find_binary_in(&bin_dir, &self.cfg.target_name)?;
        let app_bundle = self.assemble_bundle(&host_binary)?;
        self.embed_libraries(&app_bundle)?;
        self.assemble_extensions(&bin_dir)?;
        println!();
        cprintln!("<green>Done! App bundle:</green>");
        cprintln!("<cyan>{}</cyan>", app_bundle.display());
        self.open_app()?;
        Ok(())
    }

    /// Local/dev pipeline: clean -> build -> assemble -> sign, stopping at a
    /// signed `.app`. Uses the configured signing identity if set, otherwise
    /// signs ad-hoc (can test entitlements and the hardened runtime
    /// without an Apple Developer account, but can't be notarized).
    pub fn build(&mut self) -> Result<()> {
        self.clean()?;
        let bin_dir = self.build_binary()?;
        let host_binary = self.find_binary_in(&bin_dir, &self.cfg.target_name)?;
        let app_bundle = self.assemble_bundle(&host_binary)?;
        self.embed_libraries(&app_bundle)?;
        self.assemble_extensions(&bin_dir)?;
        // No-op unless APPLE_CERTIFICATE is set; supports signing with an
        // imported Developer ID identity here too, but ad-hoc needs nothing.
        let _keychain = self.import_certificate()?.map(|(keychain, identity)| {
            self.core.cfg.sign_identity = identity;
            keychain
        });
        self.sign()?;

        println!();
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> Dry run complete. Signed app bundle would be at:");
        } else {
            println!("Done! Signed app bundle:");
        };
        cprintln!("<cyan>{}</cyan>", app_bundle.display());
        self.open_app()?;
        Ok(())
    }

    /// Copy the built `.app` into `/Applications`, replacing any existing
    /// install. Assumes the bundle has already been assembled by `bundle`,
    /// `build`, or `release`.
    pub fn install_to_applications(&self) -> Result<()> {
        let app_bundle = &self.paths.app_bundle;
        let dest = Path::new("/Applications").join(format!("{}.app", self.cfg.app_name));

        println!();
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> rm -rf {}", dest.display());
            cprintln!(
                "<dim>[dry-run]</dim> ditto {} {}",
                app_bundle.display(),
                dest.display()
            );
            return Ok(());
        }

        if dest.exists() {
            std::fs::remove_dir_all(&dest)
                .with_context(|| format!("Failed to remove existing {}", dest.display()))?;
        }
        self.copy_tree(app_bundle, &dest)?;
        cprintln!(
            "<green>Installed to</green> <cyan>{}</cyan>",
            dest.display()
        );
        Ok(())
    }

    /// Copy the built DMG into `dir`, replacing any existing file of the same
    /// name. Assumes the DMG has already been packaged by `release`.
    pub fn copy_dmg_to(&self, dir: &Path) -> Result<()> {
        let dmg = &self.paths.dmg;
        let dmg_name = dmg
            .file_name()
            .with_context(|| format!("DMG path has no file name: {}", dmg.display()))?;
        let dest = dir.join(dmg_name);

        if !self.dry_run {
            self.create_dir(dir)?;
            if dest.exists() {
                std::fs::remove_file(&dest)
                    .with_context(|| format!("Failed to remove existing {}", dest.display()))?;
            }
        }
        self.copy_file(dmg, &dest)?;
        println!();
        cprintln!(
            "<green>DMG copied to</green> <cyan>{}</cyan>",
            dest.display()
        );
        Ok(())
    }

    /// Full release pipeline: clean -> binary -> assemble -> sign -> package
    /// DMG -> notarize (or, with `--mas`, package pkg -> upload). With
    /// `--resume`, skips the build and resumes a pending notarization
    /// instead (Developer-ID only). With `--skip-notarization`, stops after
    /// packaging the DMG/pkg, before notarization/upload.
    pub fn release(&mut self) -> Result<()> {
        if let Some(ref uuid_hint) = self.resume {
            return self.resume_notarization(uuid_hint);
        }

        if !self.skip_notarization {
            if self.mas {
                self.preflight_mas_credentials()?;
            } else {
                self.preflight_credentials()?;
            }
        }
        self.clean()?;
        let bin_dir = self.build_binary()?;
        let host_binary = self.find_binary_in(&bin_dir, &self.cfg.target_name)?;
        let app_bundle = self.assemble_bundle(&host_binary)?;
        self.embed_libraries(&app_bundle)?;
        self.assemble_extensions(&bin_dir)?;
        if self.mas {
            self.apply_mas_overrides()?;
        }
        // Held through both `sign` and the DMG signing in `package_dmg`, the
        // only steps that need it. Dropped at the end of this function, which
        // tears the temporary keychain back down.
        let _keychain = self.import_certificate()?.map(|(keychain, identity)| {
            self.core.cfg.sign_identity = identity;
            keychain
        });
        self.sign()?;

        let artifact_path = if self.mas {
            self.package_pkg()?;
            if !self.skip_notarization {
                self.upload_pkg()?;
            }
            self.paths.pkg.clone()
        } else {
            self.package_dmg()?;
            if self.skip_notarization {
                if !self.dry_run {
                    if let Some(parent) = self.paths.dmg.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::rename(&self.paths.strudel_temp_dmg, &self.paths.dmg)?;
                }
            } else {
                self.notarize()?;
            }
            self.paths.dmg.clone()
        };

        println!();
        let app_bundle_path = cformat!("<cyan>{}</cyan>", app_bundle.display());
        let artifact_label = if self.mas {
            "Pkg:        "
        } else {
            "DMG:        "
        };
        let artifact_path_str = cformat!("<cyan>{}</cyan>", artifact_path.display());
        let msg = formatdoc! {"
            App bundle: {app_bundle_path}
            {artifact_label}{artifact_path_str}
        "};
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> Dry run complete. Artifacts would be at:");
            println!("{msg}");
            if !self.skip_notarization {
                let problems = if self.mas {
                    self.mas_credential_problems()
                } else {
                    self.credential_problems()
                };
                if !problems.is_empty() {
                    println!();
                    cprintln!("<red>WARNING:</red> Credential problems:");
                    for p in &problems {
                        cprintln!("- {p}");
                    }
                }
            }
        } else if self.skip_notarization {
            let artifact_kind = if self.mas { "pkg" } else { "DMG" };
            let verb = if self.mas { "upload" } else { "notarization" };
            cprintln!("<green>Done!</green> {artifact_kind} built ({verb} skipped):");
            println!("{msg}");
        } else {
            cprintln!("<green>Done!</green> Distribution artifacts:");
            println!("{msg}");
        };

        self.open_app()?;
        Ok(())
    }
}

impl IosBuilder {
    pub fn new(cfg: ResolvedConfig, dry_run: bool, debug: bool) -> Result<Self> {
        let ResolvedTargetPlatform::Ios(ref ios) = cfg.target_platform else {
            bail!("IosBuilder constructed for a non-iOS target");
        };
        let ios = ios.clone();
        Ok(IosBuilder {
            core: BuilderCore::new(cfg, dry_run, debug),
            ios,
        })
    }
}
