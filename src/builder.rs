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
//! - [`bundle`] bundle-layout helpers shared by both platforms
//! - [`fs`] dry-run-aware filesystem helpers (on [`BuilderCore`])
//! - [`keychain`] signing-credential preflight and certificate import
//! - [`macos`] the macOS pipeline stages (todo: move MacOSBuilder here)
//! - [`ios`] the iOS pipeline stages (todo: move IosBuilder here)

mod bundle;
mod fs;
mod ios;
pub(crate) mod keychain;
mod macos;

use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
pub(crate) use bundle::{is_framework, resolve_build_artifact};
use clml::{cformat, cprintln};
use indoc::formatdoc;
pub(crate) use ios::decode_profile;

use crate::config::{
    ResolvedConfig, ResolvedIosSection, ResolvedMacOsSection, ResolvedTargetPlatform,
};
use crate::paths::Paths;
use crate::shell::Shell;

/// Output-verbosity flags shared by every driver constructor, grouped so
/// adding one doesn't blow out the parameter count on `MacosBuilder::new`/
/// `IosBuilder::new`.
#[derive(Clone, Copy, Default)]
pub struct OutputFlags {
    /// Print commands without executing them.
    pub dry_run: bool,
    /// Suppress echoing of the underlying commands.
    pub no_echo: bool,
    /// Full silence: no progress messages or command echo, and streamed
    /// subprocess output is only shown on failure. Implies `no_echo`.
    pub quiet: bool,
}

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
    fn new(cfg: ResolvedConfig, output: OutputFlags, debug: bool) -> Self {
        BuilderCore {
            paths: Paths::new(&cfg),
            sh: Shell::new(output.dry_run, output.no_echo, output.quiet),
            dry_run: output.dry_run,
            debug,
            cfg,
        }
    }

    /// Print a green progress header for a build step. Suppressed in quiet
    /// mode. When `--no-echo` is set (but not `--quiet`), the leading blank
    /// line is dropped so consecutive step headers, now the only thing on
    /// screen, print back-to-back instead of double-spaced.
    pub(crate) fn step(&self, msg: &str) {
        if self.sh.quiet() {
            return;
        }
        if self.echo_suppressed() {
            cprintln!("<green>==>> {msg}</green>");
        } else {
            cprintln!("\n<green>==>> {msg}</green>");
        }
    }

    /// Print a dim command/filesystem echo line (e.g. a dry-run "rm -rf ..."),
    /// unless suppressed by `--no-echo` or `--quiet`. Build the (optionally
    /// colored) message with `clml::cformat!`.
    pub(crate) fn echo(&self, msg: impl AsRef<str>) {
        if !self.echo_suppressed() {
            cprintln!("{}", msg.as_ref());
        }
    }

    /// Print an informational/success line, suppressed by `--no-echo` as
    /// well as `--quiet` since it's noise once command echo and streamed
    /// subprocess output are already hidden.
    pub(crate) fn note(&self, msg: impl AsRef<str>) {
        if !self.echo_suppressed() {
            cprintln!("{}", msg.as_ref());
        }
    }

    /// Full silence: no progress headers, "Done!" banners, or streamed
    /// subprocess output.
    pub(crate) fn quiet(&self) -> bool {
        self.sh.quiet()
    }

    /// True if the dim command-echo lines (including dry-run filesystem
    /// echoes) should be skipped.
    pub(crate) fn echo_suppressed(&self) -> bool {
        self.sh.echo_suppressed()
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

        self.step("Cleaning strudel output...");
        let prefix = if self.dry_run { "[dry-run] " } else { "" };
        self.echo(cformat!(
            "<dim>{prefix}rm -rf {}</dim>",
            build_dir.display()
        ));
        if !self.dry_run && build_dir.exists() {
            std::fs::remove_dir_all(build_dir)?;
        }

        self.step("Cleaning Swift build cache...");
        self.sh
            .run(&["swift", "package", "clean", "--package-path", source])?;

        self.note(cformat!("\n<green>Done!</green>"));
        Ok(())
    }
}

/// Clean a single target's build output. Platform-agnostic, so `strudel
/// clean` can tidy macOS and iOS targets alike without picking a driver.
pub fn clean(cfg: ResolvedConfig, dry_run: bool) -> Result<()> {
    let output = OutputFlags {
        dry_run,
        ..Default::default()
    };
    BuilderCore::new(cfg, output, false).clean_command()
}

impl MacosBuilder {
    pub fn new(
        cfg: ResolvedConfig,
        output: OutputFlags,
        open: bool,
        debug: bool,
        resume: Option<String>,
        skip_notarization: bool,
        ci: bool,
    ) -> Result<Self> {
        let ResolvedTargetPlatform::Mac(ref macos) = cfg.target_platform else {
            bail!("MacosBuilder constructed for a non-macOS target");
        };
        let macos = macos.clone();
        Ok(MacosBuilder {
            core: BuilderCore::new(cfg, output, debug),
            macos,
            open,
            resume,
            skip_notarization,
            ci,
        })
    }

    /// Assemble every configured extension, under `<app>.app/Contents/PlugIns/`
    /// (app extensions) or `<app>.app/Contents/Library/SystemExtensions/`
    /// (system extensions). No-op when no extensions are configured.
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
        let app_bundle = self.assemble_bundle(&host_binary, &bin_dir)?;
        self.embed_libraries(&app_bundle, &bin_dir)?;
        self.assemble_extensions(&bin_dir)?;
        self.note(cformat!(
            "\n<green>Done! App bundle:</green>\n<cyan>{}</cyan>",
            app_bundle.display()
        ));
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
        let app_bundle = self.assemble_bundle(&host_binary, &bin_dir)?;
        self.embed_libraries(&app_bundle, &bin_dir)?;
        self.assemble_extensions(&bin_dir)?;
        // No-op unless APPLE_CERTIFICATE is set; supports signing with an
        // imported Developer ID identity here too, but ad-hoc needs nothing.
        let _keychain = self.import_certificate()?.map(|(keychain, identity)| {
            self.core.cfg.sign_identity = identity;
            keychain
        });
        self.sign()?;

        let status = if self.dry_run {
            cformat!("<dim>[dry-run]</dim> Dry run complete. Signed app bundle would be at:")
        } else {
            "Done! Signed app bundle:".to_string()
        };
        self.note(cformat!(
            "\n{status}\n<cyan>{}</cyan>",
            app_bundle.display()
        ));
        self.open_app()?;
        Ok(())
    }

    /// Copy the built `.app` into `/Applications`, replacing any existing
    /// install. Assumes the bundle has already been assembled by `bundle`,
    /// `build`, or `release`.
    pub fn install_to_applications(&self) -> Result<()> {
        let app_bundle = &self.paths.app_bundle;
        let dest = Path::new("/Applications").join(format!("{}.app", self.cfg.app_name));

        if self.dry_run {
            self.echo(cformat!(
                "\n<dim>[dry-run]</dim> rm -rf {}\n<dim>[dry-run]</dim> ditto {} {}",
                dest.display(),
                app_bundle.display(),
                dest.display()
            ));
            return Ok(());
        }

        if dest.exists() {
            std::fs::remove_dir_all(&dest)
                .with_context(|| format!("Failed to remove existing {}", dest.display()))?;
        }
        self.copy_tree(app_bundle, &dest)?;
        self.note(cformat!(
            "\n<green>Installed to</green> <cyan>{}</cyan>",
            dest.display()
        ));
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
        self.note(cformat!(
            "\n<green>DMG copied to</green> <cyan>{}</cyan>",
            dest.display()
        ));
        Ok(())
    }

    /// Full release pipeline: clean -> binary -> assemble -> sign -> package
    /// DMG -> notarize. With `--resume`, skips the build and resumes a
    /// pending notarization instead. With `--skip-notarization`, stops
    /// after packaging the DMG.
    pub fn release(&mut self) -> Result<()> {
        if let Some(ref uuid_hint) = self.resume {
            return self.resume_notarization(uuid_hint);
        }

        if !self.skip_notarization {
            self.preflight_credentials()?;
        }
        self.clean()?;
        let bin_dir = self.build_binary()?;
        let host_binary = self.find_binary_in(&bin_dir, &self.cfg.target_name)?;
        let app_bundle = self.assemble_bundle(&host_binary, &bin_dir)?;
        self.embed_libraries(&app_bundle, &bin_dir)?;
        self.assemble_extensions(&bin_dir)?;
        // Held through both `sign` and the DMG signing in `package_dmg`, the
        // only steps that need it. Dropped at the end of this function, which
        // tears the temporary keychain back down.
        let _keychain = self.import_certificate()?.map(|(keychain, identity)| {
            self.core.cfg.sign_identity = identity;
            keychain
        });
        self.sign()?;
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

        let app_bundle_path = cformat!("<cyan>{}</cyan>", app_bundle.display());
        let dmg_path = cformat!("<cyan>{}</cyan>", self.paths.dmg.display());
        let artifacts = formatdoc! {r#"
            App bundle: {app_bundle_path}
            DMG:        {dmg_path}
        "#};
        let status = if self.dry_run {
            cformat!("<dim>[dry-run]</dim> Dry run complete. Artifacts would be at:")
        } else if self.skip_notarization {
            cformat!("<green>Done!</green> DMG built (notarization skipped):")
        } else {
            cformat!("<green>Done!</green> Distribution artifacts:")
        };
        self.note(format!("\n{status}\n{artifacts}"));
        if self.dry_run && !self.skip_notarization {
            let problems = self.credential_problems();
            if !problems.is_empty() {
                println!();
                cprintln!("<red>WARNING:</red> Credential problems:");
                for p in &problems {
                    cprintln!("- {p}");
                }
            }
        }

        self.open_app()?;
        Ok(())
    }
}

impl IosBuilder {
    pub fn new(cfg: ResolvedConfig, output: OutputFlags, debug: bool) -> Result<Self> {
        let ResolvedTargetPlatform::Ios(ref ios) = cfg.target_platform else {
            bail!("IosBuilder constructed for a non-iOS target");
        };
        let ios = ios.clone();
        Ok(IosBuilder {
            core: BuilderCore::new(cfg, output, debug),
            ios,
        })
    }
}
