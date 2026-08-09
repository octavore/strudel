//! Bundle-layout helpers shared by the macOS and iOS pipelines.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::builder::BuilderCore;

/// Whether an `embed_libs` entry is a `.framework` directory bundle rather
/// than a flat dylib.
pub(crate) fn is_framework(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("framework")
}

/// Resolve a `resources`/`resources_dir` entry that may be a triple-dependent
/// build artifact (e.g. a SwiftPM-generated resource bundle) rather than a
/// source file living beside the config: `entry` is used as configured
/// (resolves like any other path) unless it doesn't exist, in which case
/// strudel falls back to `bin_dir/<file name>`, the current build's
/// `.build/<triple>/release/` output, when that exists instead.
pub(crate) fn resolve_build_artifact(entry: &Path, bin_dir: &Path) -> PathBuf {
    if entry.exists() {
        return entry.to_path_buf();
    }
    match entry.file_name() {
        Some(name) => {
            let candidate = bin_dir.join(name);
            if candidate.exists() {
                candidate
            } else {
                entry.to_path_buf()
            }
        },
        None => entry.to_path_buf(),
    }
}

impl BuilderCore {
    /// Copy `cfg.embed_libs` into `frameworks_dir`, rewriting `executable`'s
    /// dylib references to `@rpath/...` along the way. `.framework`
    /// directory bundles are copied as-is (they're already linked with an
    /// `@rpath` install name); flat dylibs get their install name rewritten
    /// via `install_name_tool`. No-op when `cfg.embed_libs` is empty. Falls
    /// back to `bin_dir`, the current build's `.build/<triple>/release/`
    /// output, when an entry doesn't exist at its configured location - so a
    /// bare name still works across build destinations (e.g. simulator to
    /// device) without listing a path.
    pub(crate) fn embed_libraries(
        &self,
        frameworks_dir: &Path,
        executable: &Path,
        bin_dir: &Path,
    ) -> Result<()> {
        if self.cfg.embed_libs.is_empty() {
            return Ok(());
        }

        self.step("Embedding libraries and frameworks...");

        self.create_dir(frameworks_dir)?;

        let executable_str = executable
            .to_str()
            .context("embed: Invalid executable path.")?;

        for entry in &self.cfg.embed_libs {
            let lib_path = resolve_build_artifact(entry, bin_dir);
            let lib_path = lib_path.as_path();
            let file_name = lib_path.file_name().with_context(|| {
                format!("embed_libs entry has no filename: {}", lib_path.display())
            })?;
            let dest = frameworks_dir.join(file_name);

            if is_framework(lib_path) {
                self.copy_tree(lib_path, &dest)?;
                continue;
            }

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

            // Update the executable (at `executable_str`): change its reference to the
            // dylib from the `orig_install_name` to `@rpath/{dylib_name}`.
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
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::resolve_build_artifact;
    use crate::builder::{BuilderCore, OutputFlags};
    use crate::config::fixtures::resolved_macos;

    #[test]
    fn resource_at_configured_path_is_used_as_is() {
        let dir = tempdir().unwrap();
        let configured = dir.path().join("Assets/logo.png");
        std::fs::create_dir_all(configured.parent().unwrap()).unwrap();
        std::fs::write(&configured, b"png").unwrap();
        let bin_dir = dir.path().join("bin");

        assert_eq!(resolve_build_artifact(&configured, &bin_dir), configured);
    }

    #[test]
    fn resource_missing_at_configured_path_falls_back_to_bin_dir() {
        let dir = tempdir().unwrap();
        let configured = dir.path().join("MyPkg_MyTarget.bundle");
        let bin_dir = dir.path().join("bin");
        let built = bin_dir.join("MyPkg_MyTarget.bundle");
        std::fs::create_dir_all(&built).unwrap();

        assert_eq!(resolve_build_artifact(&configured, &bin_dir), built);
    }

    #[test]
    fn resource_missing_everywhere_keeps_the_configured_path() {
        let dir = tempdir().unwrap();
        let configured = dir.path().join("nope.png");
        let bin_dir = dir.path().join("bin");

        assert_eq!(resolve_build_artifact(&configured, &bin_dir), configured);
    }

    /// An `embed_libs` entry that doesn't exist at its configured location
    /// falls back to `bin_dir`, the current build's per-triple output dir.
    #[test]
    fn embed_libs_entry_missing_at_configured_path_falls_back_to_bin_dir() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        let vendored = bin_dir.join("Sparkle.framework");
        std::fs::create_dir_all(vendored.join("Resources")).unwrap();

        let mut cfg = resolved_macos();
        cfg.embed_libs = vec!["Sparkle.framework".into()];
        let core = BuilderCore::new(cfg, OutputFlags::default(), false);

        let frameworks_dir = dir.path().join("Frameworks");
        let executable = dir.path().join("MyApp");
        core.embed_libraries(&frameworks_dir, &executable, &bin_dir)
            .unwrap();

        assert!(frameworks_dir.join("Sparkle.framework/Resources").is_dir());
    }
}
