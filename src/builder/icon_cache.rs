//! Persistent icon cache: app icon rendering (squircle compositing, `.icns`
//! conversion, iOS asset-catalog generation) is one of the slower steps in a
//! build, and its output only ever changes when the source image or icon
//! settings change. Every `strudel build` starts with [`MacosBuilder::clean`]
//! wiping `build_dir`, so caching inside it is pointless; instead this caches
//! under `.strudel/icon-cache/`, which `clean` never touches.
//!
//! The cache key is a SHA-256 digest of the source file's path, size, and
//! mtime plus the icon settings that affect the output (scale, background,
//! and the output name), not a content hash: cheap to compute on every
//! build, and a false cache hit would require a source file whose size and
//! mtime are unchanged despite different content, which build tools generally
//! accept as an acceptable risk (the same assumption `make` relies on). The
//! digest is stable across Rust releases and platforms.
//!
//! Callers use [`BuilderCore::icon_cached_file`] or
//! [`BuilderCore::icon_cached_dir`], which return the cached output on a hit
//! and otherwise run the caller's generator and store its result. Entries are
//! pruned by age: a hit refreshes its entry's mtime, and storing a new entry
//! deletes entries unused for [`MAX_AGE`]. `strudel clean` removes the whole
//! cache. In dry-run, the cache is bypassed.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use clml::cformat;
use sha2::{Digest, Sha256};

use crate::builder::BuilderCore;
use crate::config::ResolvedIcon;

/// Entries not used for this long are deleted the next time an entry is
/// stored.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Feed a length-prefixed field into `hasher` so adjacent fields can't run
/// together and produce the same digest.
fn update_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// The source file and the settings string that, together with `output`,
/// identify what `icon` renders to.
fn cache_input<'a>(icon: &'a ResolvedIcon, output: &str) -> (&'a Path, String) {
    match icon {
        ResolvedIcon::Path { path, .. } => (path, output.to_string()),
        ResolvedIcon::Generated {
            src,
            scale,
            background,
            ..
        } => (
            src,
            format!("{output}|scale={scale}|background={background:?}"),
        ),
    }
}

impl BuilderCore {
    /// Hash `source`'s path/size/mtime together with `extra` into a cache
    /// key, as 32 lowercase hex characters.
    fn icon_cache_key(&self, source: &Path, extra: &str) -> Result<String> {
        let meta = fs::metadata(source)
            .with_context(|| format!("failed to read metadata for {}", source.display()))?;
        let mtime_nanos = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());

        let mut hasher = Sha256::new();
        update_field(&mut hasher, source.as_os_str().as_encoded_bytes());
        update_field(&mut hasher, &meta.len().to_le_bytes());
        update_field(&mut hasher, &mtime_nanos.to_le_bytes());
        update_field(&mut hasher, extra.as_bytes());

        let digest = hasher.finalize();
        let mut key = String::with_capacity(32);
        for byte in &digest[..16] {
            write!(key, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Ok(key)
    }

    /// Cache location of `icon`'s `output` (a file or directory name such as
    /// `AppIcon.icns`), inside an entry directory named by the cache key.
    fn icon_cache_path(&self, icon: &ResolvedIcon, output: &str) -> Result<PathBuf> {
        let (source, extra) = cache_input(icon, output);
        let key = self.icon_cache_key(source, &extra)?;
        Ok(self.icon_cache_root().join(key).join(output))
    }

    fn icon_cache_root(&self) -> PathBuf {
        self.paths.strudel_dir.join("icon-cache")
    }

    /// Mark the entry containing `cached` as recently used. Best-effort.
    fn touch_icon_cache_entry(cached: &Path) {
        let Some(entry) = cached.parent() else {
            return;
        };
        let _ = fs::File::open(entry).and_then(|f| f.set_modified(SystemTime::now()));
    }

    /// Delete entries whose mtime is older than [`MAX_AGE`]. Best-effort.
    fn sweep_icon_cache(&self) {
        let Ok(entries) = fs::read_dir(self.icon_cache_root()) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let expired = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| now.duration_since(mtime).ok())
                .is_some_and(|age| age > MAX_AGE);
            if expired {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    /// Delete the whole icon cache, for `strudel clean`.
    pub(super) fn clean_icon_cache(&self) -> Result<()> {
        let root = self.icon_cache_root();
        let prefix = if self.dry_run { "[dry-run] " } else { "" };
        self.echo(cformat!("<dim>{prefix}rm -rf {}</dim>", root.display()));
        if !self.dry_run && root.exists() {
            fs::remove_dir_all(&root)
                .with_context(|| format!("failed to remove {}", root.display()))?;
        }
        Ok(())
    }

    /// Write `icon`'s `output` file to `dest`, from the cache if present.
    /// Otherwise `generate` must write `dest`, and the result is then stored
    /// in the cache (best-effort: a failure to store doesn't fail the
    /// build). Nothing is stored if `generate` fails.
    pub(super) fn icon_cached_file(
        &self,
        icon: &ResolvedIcon,
        output: &str,
        dest: &Path,
        generate: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        if self.dry_run {
            return generate();
        }
        let cached = self.icon_cache_path(icon, output)?;
        if cached.exists() {
            Self::touch_icon_cache_entry(&cached);
            return self.copy_file(&cached, dest);
        }
        generate()?;
        if let Some(dir) = cached.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::copy(dest, &cached);
        self.sweep_icon_cache();
        Ok(())
    }

    /// Directory counterpart to [`Self::icon_cached_file`], for outputs that
    /// are a whole tree (e.g. an iOS `.xcassets`). On a miss, the tree is
    /// copied to a temporary sibling and renamed into place, so an
    /// interrupted copy never leaves a partial entry that a later build
    /// would treat as a hit.
    pub(super) fn icon_cached_dir(
        &self,
        icon: &ResolvedIcon,
        output: &str,
        dest: &Path,
        generate: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        if self.dry_run {
            return generate();
        }
        let cached = self.icon_cache_path(icon, output)?;
        if cached.is_dir() {
            Self::touch_icon_cache_entry(&cached);
            return self.copy_tree(&cached, dest);
        }
        generate()?;

        let Some(dir) = cached.parent() else {
            return Ok(());
        };
        if fs::create_dir_all(dir).is_err() {
            return Ok(());
        }
        let tmp = cached.with_file_name(format!("{output}.tmp{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        if self.copy_tree(dest, &tmp).is_err() || fs::rename(&tmp, &cached).is_err() {
            let _ = fs::remove_dir_all(&tmp);
        }
        self.sweep_icon_cache();
        Ok(())
    }
}
