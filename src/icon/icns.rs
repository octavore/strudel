use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const SIZES: &[u32] = &[16, 32, 64, 128, 256, 512];

/// Convert a PNG to an .icns file using macOS's built-in tools (`sips` +
/// `iconutil`). The source PNG should be at least 1024x1024.
///
/// Callers are expected to check dry-run themselves before calling this -
/// there's no way to "dry run" a real `.icns` conversion meaningfully, so
/// this always does the real work.
pub fn make_icns(png_path: &Path, icns_path: &Path) -> Result<()> {
    if !png_path.exists() {
        bail!("Source PNG not found: {}", png_path.display());
    }

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
            run(&["sips", "-z", &s, &s, png, "--out", &out1])?;
            run(&["sips", "-z", &s2, &s2, png, "--out", &out2])?;
        }

        if let Some(parent) = icns_path.parent() {
            fs::create_dir_all(parent)?;
        }
        run(&["iconutil", "-c", "icns", &iconset, "-o", icns])?;
        Ok(())
    })();

    // Always clean up the temporary iconset, even on failure
    let _ = fs::remove_dir_all(&iconset_dir);
    result
}

fn run(args: &[&str]) -> Result<()> {
    let status = Command::new(args[0])
        .args(&args[1..])
        .status()
        .with_context(|| format!("Failed to run {}", args.join(" ")))?;
    if !status.success() {
        bail!("{} exited with {}", args.join(" "), status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn missing_source_png_errors_before_any_work() {
        // Guard the user against `sips`/`iconutil` producing a cryptic error;
        // we should fail fast with a clear message that names the missing file.
        let missing = PathBuf::from("/definitely/does/not/exist.png");
        let dir = tempfile::tempdir().expect("create temp dir");
        let dest = dir.path().join("strudel-icns-test.icns");
        let err = make_icns(&missing, &dest).expect_err("must error on missing PNG");
        let msg = err.to_string();
        assert!(msg.contains("Source PNG not found"), "got: {msg}");
        assert!(msg.contains("exist.png"), "msg should name the path: {msg}");
    }
}
