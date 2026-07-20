//! Bundle-layout helpers shared by the macOS and iOS pipelines.

use std::path::Path;

use anyhow::{Context, Result};

use crate::builder::{BuilderCore, step};
use crate::config::utils::is_bare_name;

/// Whether an `embed_libs` entry is a `.framework` directory bundle rather
/// than a flat dylib.
pub(crate) fn is_framework(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("framework")
}

impl BuilderCore {
    /// Copy `cfg.embed_libs` into `frameworks_dir`, rewriting `executable`'s
    /// dylib references to `@rpath/...` along the way. `.framework`
    /// directory bundles are copied as-is (they're already linked with an
    /// `@rpath` install name); flat dylibs get their install name rewritten
    /// via `install_name_tool`. No-op when `cfg.embed_libs` is empty. Bare
    /// entries (no `/`) are resolved against `bin_dir`, the current build's
    /// `.build/<triple>/release/` output; entries with a `/` are used as-is.
    pub(crate) fn embed_libraries(
        &self,
        frameworks_dir: &Path,
        executable: &Path,
        bin_dir: &Path,
    ) -> Result<()> {
        if self.cfg.embed_libs.is_empty() {
            return Ok(());
        }

        step("Embedding libraries and frameworks...");

        self.create_dir(frameworks_dir)?;

        let executable_str = executable
            .to_str()
            .context("embed: Invalid executable path.")?;

        for entry in &self.cfg.embed_libs {
            let lib_path = if is_bare_name(entry) {
                bin_dir.join(entry)
            } else {
                entry.clone()
            };
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

    use crate::builder::BuilderCore;
    use crate::config::fixtures::resolved_macos;

    /// A bare `embed_libs` entry (e.g. `"Sparkle.framework"`) is resolved
    /// against `bin_dir`, the current build's per-triple output dir, rather
    /// than the config file's directory.
    #[test]
    fn bare_embed_libs_entry_resolves_against_bin_dir() {
        let dir = tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        let vendored = bin_dir.join("Sparkle.framework");
        std::fs::create_dir_all(vendored.join("Resources")).unwrap();

        let mut cfg = resolved_macos();
        cfg.embed_libs = vec!["Sparkle.framework".into()];
        let core = BuilderCore::new(cfg, false, false);

        let frameworks_dir = dir.path().join("Frameworks");
        let executable = dir.path().join("MyApp");
        core.embed_libraries(&frameworks_dir, &executable, &bin_dir)
            .unwrap();

        assert!(frameworks_dir.join("Sparkle.framework/Resources").is_dir());
    }
}
