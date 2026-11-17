mod alias;
mod ds_store;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// DMG Finder-window background: a solid color, an image, or left unset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DmgBackground {
    /// Solid RGB color.
    Color(u8, u8, u8),
    /// Path to a background image (PNG or TIFF).
    Path(PathBuf),
    /// No background configuration is written; Finder uses its own default.
    #[default]
    None,
}

pub struct DmgSpec {
    pub vol_name: String,
    pub app_name: String,
    /// Fully-resolved path to the signed `.app` bundle to embed.
    pub source_app: PathBuf,
    /// DMG Finder-window background. Defaults to unconfigured (Finder's own
    /// default).
    pub background: DmgBackground,
    pub window_width: u32,
    pub window_height: u32,
    pub icon_size: u32,
    pub app_x: u32,
    pub app_y: u32,
    pub applications_x: u32,
    pub applications_y: u32,
    pub icon_text_size: f64,
    /// Print each `hdiutil` command, dimmed, before it runs.
    pub echo: bool,
}

/// Create a compressed, styled DMG at `output`.
///
/// We pre-generate the `.DS_Store` that carries the Finder window layout, to
/// avoid driving Finder/AppleScript. The image is built from a staging folder
/// with `hdiutil makehybrid` and then compressed with `hdiutil convert`.
///
/// `makehybrid` does not mount the volume. `hdiutil create -srcfolder` mounts
/// at `/Volumes/<vol_name>`, where macOS can deny access to the app bundle.
pub fn create(spec: &DmgSpec, output: &Path) -> Result<()> {
    let tmp = tempfile::tempdir().context("creating temp dir for DMG build")?;
    let staging = tmp.path().join("staging");
    fs::create_dir_all(&staging).context("creating DMG staging dir")?;

    populate(spec, &staging).context("staging DMG contents")?;

    if output.exists() {
        fs::remove_file(output).context("removing stale output DMG")?;
    }

    let hybrid_dmg = tmp.path().join("hybrid.dmg");
    run(
        spec.echo,
        &[
            "hdiutil",
            "makehybrid",
            "-hfs",
            "-hfs-volume-name",
            &spec.vol_name,
            "-ov",
            "-o",
            hybrid_dmg.to_str().unwrap(),
            staging.to_str().unwrap(),
        ],
    )
    .context("building image")?;

    run(
        spec.echo,
        &[
            "hdiutil",
            "convert",
            hybrid_dmg.to_str().unwrap(),
            "-format",
            "UDZO",
            "-ov",
            "-o",
            output.to_str().unwrap(),
        ],
    )
    .context("converting image to UDZO")?;

    Ok(())
}

/// Lay out the DMG contents in `staging`: the `.app`, the `Applications`
/// symlink, an optional background image, and the `.DS_Store` that styles the
/// window. `hdiutil makehybrid` later turns this into the volume.
fn populate(spec: &DmgSpec, staging: &Path) -> Result<()> {
    // Copy .app bundle
    let dest_app = staging.join(format!("{}.app", spec.app_name));
    run(
        false,
        &[
            "cp",
            "-rp",
            spec.source_app.to_str().unwrap(),
            dest_app.to_str().unwrap(),
        ],
    )
    .context("copying .app into staging")?;

    // Applications symlink
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))
        .context("creating Applications symlink")?;

    // Background image + alias record, or a solid color.
    let background = match &spec.background {
        DmgBackground::Path(bg_src) => {
            let bg_dir = staging.join(".background");
            fs::create_dir_all(&bg_dir).context("creating .background dir")?;

            let bg_name = bg_src
                .file_name()
                .context("background image has no filename")?;
            let bg_dest = bg_dir.join(bg_name);
            fs::copy(bg_src, &bg_dest).context("copying background image")?;

            // The alias is built from the staging paths, so its CNIDs won't
            // match the final volume. Finder resolves it by name/relative
            // path instead, which is stable (`<vol>/.background/<name>`).
            let alias_bytes =
                alias::build(&spec.vol_name, staging, &bg_dest).context("building alias record")?;
            ds_store::Background::Image(alias_bytes)
        },
        DmgBackground::Color(r, g, b) => ds_store::Background::Color(*r, *g, *b),
        DmgBackground::None => ds_store::Background::None,
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
        background,
        icon_text_size: spec.icon_text_size,
    };
    let ds_bytes = ds_store::build(&ds_spec).context("building .DS_Store")?;
    fs::write(staging.join(".DS_Store"), &ds_bytes).context("writing .DS_Store")?;

    Ok(())
}

fn run(echo: bool, args: &[&str]) -> Result<()> {
    let (prog, rest) = args.split_first().context("empty command")?;
    if echo {
        let line: Vec<String> = args
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a.to_string()
                }
            })
            .collect();
        println!("\x1b[2m{}\x1b[0m", line.join(" "));
    }
    let output = Command::new(prog)
        .args(rest)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("spawning {prog}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{prog} failed with exit code {:?}\n{}{}",
            output.status.code(),
            stdout,
            stderr,
        );
    }
    Ok(())
}
