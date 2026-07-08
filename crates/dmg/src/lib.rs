mod alias;
mod ds_store;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub struct DmgSpec {
    pub vol_name: String,
    pub app_name: String,
    /// Fully-resolved path to the signed `.app` bundle to embed.
    pub source_app: PathBuf,
    /// Optional background image (PNG or TIFF).  When `None` the window gets
    /// a plain white background.
    pub background: Option<PathBuf>,
    pub window_width: u32,
    pub window_height: u32,
    pub icon_size: u32,
    pub app_x: u32,
    pub app_y: u32,
    pub applications_x: u32,
    pub applications_y: u32,
}

/// Create a compressed, styled DMG at `output`.
///
/// We pre-generate the `.DS_Store` that carries the Finder window layout, to
/// avoid having to mount a read-write image and drive Finder/AppleScript.
/// Instead the volume is built from a staging folder in a single
/// `hdiutil create -srcfolder` call.
pub fn create(spec: &DmgSpec, output: &Path) -> Result<()> {
    let tmp = tempfile::tempdir().context("creating temp dir for DMG build")?;
    let staging = tmp.path().join("staging");
    fs::create_dir_all(&staging).context("creating DMG staging dir")?;

    populate(spec, &staging).context("staging DMG contents")?;

    // `-srcfolder` builds (and auto-sizes) the HFS+ volume directly from the
    // staging directory, then `-format UDZO` compresses it. `-ov` overwrites any
    // stale image at `output`.
    if output.exists() {
        fs::remove_file(output).context("removing stale output DMG")?;
    }
    run(&[
        "hdiutil",
        "create",
        "-volname",
        &spec.vol_name,
        "-srcfolder",
        staging.to_str().unwrap(),
        "-ov",
        "-format",
        "UDZO",
        output.to_str().unwrap(),
    ])
    .context("hdiutil create")?;

    Ok(())
}

/// Lay out the DMG contents in `staging`: the `.app`, the `Applications`
/// symlink, an optional background image, and the `.DS_Store` that styles the
/// window. `hdiutil create -srcfolder` later turns this into the volume.
fn populate(spec: &DmgSpec, staging: &Path) -> Result<()> {
    // Copy .app bundle
    let dest_app = staging.join(format!("{}.app", spec.app_name));
    run(&[
        "cp",
        "-rp",
        spec.source_app.to_str().unwrap(),
        dest_app.to_str().unwrap(),
    ])
    .context("copying .app into staging")?;

    // Applications symlink
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))
        .context("creating Applications symlink")?;

    // Background image + alias record
    let background_alias = if let Some(bg_src) = &spec.background {
        let bg_dir = staging.join(".background");
        fs::create_dir_all(&bg_dir).context("creating .background dir")?;

        let bg_name = bg_src
            .file_name()
            .context("background image has no filename")?;
        let bg_dest = bg_dir.join(bg_name);
        fs::copy(bg_src, &bg_dest).context("copying background image")?;

        // The alias is built from the staging paths, so its CNIDs won't match
        // the final volume. Finder resolves it by name/relative path instead,
        // which is stable (`<vol>/.background/<name>`).
        let alias_bytes =
            alias::build(&spec.vol_name, staging, &bg_dest).context("building alias record")?;
        Some(alias_bytes)
    } else {
        None
    };

    // Write .DS_Store
    let ds_spec = ds_store::DsStoreSpec {
        window_width: spec.window_width,
        window_height: spec.window_height,
        icon_size: spec.icon_size,
        app_name: &spec.app_name,
        app_x: spec.app_x,
        app_y: spec.app_y,
        applications_x: spec.applications_x,
        applications_y: spec.applications_y,
        background_alias,
    };
    let ds_bytes = ds_store::build(&ds_spec).context("building .DS_Store")?;
    fs::write(staging.join(".DS_Store"), &ds_bytes).context("writing .DS_Store")?;

    Ok(())
}

fn run(args: &[&str]) -> Result<()> {
    let (prog, rest) = args.split_first().context("empty command")?;
    let status = Command::new(prog)
        .args(rest)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("spawning {prog}"))?;
    if !status.success() {
        bail!("{prog} failed with exit code {:?}", status.code());
    }
    Ok(())
}
