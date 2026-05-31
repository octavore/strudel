//! Filesystem helpers that respect dry-run: in dry-run they log the operation
//! they would perform instead of touching the disk.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use color_print::cprintln;

use super::Builder;

impl Builder {
    /// Create a directory (and parents), logging in dry-run instead of acting.
    pub(super) fn create_dir(&self, path: &Path) -> Result<()> {
        if self.dry_run() {
            cprintln!("<dim>[dry-run]</dim> mkdir -p {}", path.display());
            return Ok(());
        }
        fs::create_dir_all(path).with_context(|| format!("Failed to create {}", path.display()))
    }

    /// Copy a file, logging source → dest in dry-run instead of acting.
    pub(super) fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
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
    #[allow(dead_code)]
    pub(super) fn write_file(&self, path: &Path, contents: &str) -> Result<()> {
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
}
