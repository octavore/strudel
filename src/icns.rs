use std::fs;
use std::path::Path;

use anyhow::{Result, bail};

use crate::shell::Shell;

const SIZES: &[u32] = &[16, 32, 64, 128, 256, 512];

/// Convert a PNG to an .icns file using macOS built-in tools (sips + iconutil).
/// The source PNG should be at least 1024×1024.
pub fn make_icns(png_path: &Path, icns_path: &Path, dry_run: bool) -> Result<()> {
    if !png_path.exists() {
        bail!("Source PNG not found: {}", png_path.display());
    }

    let sh = Shell::new(dry_run);
    let png = png_path.to_str().unwrap();
    let icns = icns_path.to_str().unwrap();

    let iconset_dir = icns_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("_iconset_tmp");
    let iconset = iconset_dir.to_str().unwrap().to_string();

    fs::create_dir_all(&iconset_dir)?;

    let result = (|| -> Result<()> {
        for &size in SIZES {
            let s = size.to_string();
            let s2 = (size * 2).to_string();
            let out1 = format!("{iconset}/icon_{size}x{size}.png");
            let out2 = format!("{iconset}/icon_{size}x{size}@2x.png");
            sh.run(&["sips", "-z", &s, &s, png, "--out", &out1])?;
            sh.run(&["sips", "-z", &s2, &s2, png, "--out", &out2])?;
        }

        if let Some(parent) = icns_path.parent() {
            fs::create_dir_all(parent)?;
        }
        sh.run(&["iconutil", "-c", "icns", &iconset, "-o", icns])?;
        Ok(())
    })();

    // Always clean up the temporary iconset, even on failure
    let _ = fs::remove_dir_all(&iconset_dir);
    result
}
