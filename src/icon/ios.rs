//! iOS icon support: padding artwork onto a flat, opaque square (iOS applies
//! its own corner mask, unlike the macOS squircle compositor in the `icon`
//! crate), and synthesizing a classic `.appiconset` that `actool` can compile
//! without further auto-derivation.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use serde_json::json;

/// `(idiom, point size, scale)` for every iPhone app-icon rendition `actool`
/// expects when compiling a synthesized `.appiconset` (matches
/// `--target-device iphone`). The `ios-marketing` 1024x1024@1x rendition is
/// handled separately.
const IOS_ICON_SIZES: &[(&str, u32, u32)] = &[
    ("iphone", 20, 2),
    ("iphone", 20, 3),
    ("iphone", 29, 2),
    ("iphone", 29, 3),
    ("iphone", 40, 2),
    ("iphone", 40, 3),
    ("iphone", 60, 2),
    ("iphone", 60, 3),
];

/// Center `source` on a `canvas_size` square filled with `background`,
/// scaled (preserving aspect ratio, "contain"-fit) to occupy `scale` of the
/// canvas. iOS icons must be fully opaque, so `background` fills the whole
/// canvas rather than leaving transparent padding.
pub fn pad_to_square(
    source: &RgbaImage,
    canvas_size: u32,
    scale: f32,
    background: Rgba<u8>,
) -> RgbaImage {
    let mut canvas = RgbaImage::from_pixel(canvas_size, canvas_size, background);

    let art_box = ((canvas_size as f32 * scale).round() as u32).max(1);
    let (sw, sh) = (source.width() as f32, source.height() as f32);
    let fit_scale = (art_box as f32 / sw).min(art_box as f32 / sh);
    let art_w = ((sw * fit_scale).round() as u32).max(1);
    let art_h = ((sh * fit_scale).round() as u32).max(1);
    let art = image::imageops::resize(source, art_w, art_h, FilterType::Lanczos3);

    let x = (canvas_size as i64 - art_w as i64) / 2;
    let y = (canvas_size as i64 - art_h as i64) / 2;
    image::imageops::overlay(&mut canvas, &art, x, y);

    canvas
}

/// Write a full classic `.appiconset` (every iPhone size/scale rendition
/// plus the `ios-marketing` 1024x1024, each as its own PNG, plus
/// `Contents.json`) derived from a single `source` image.
///
/// `actool` doesn't derive per-size renditions from a single "universal"
/// idiom image the way Xcode's own icon-generation build phase does - it
/// needs every idiom/scale spelled out with a correctly-sized image already
/// on disk. This renders each one from `source` so `actool` can compile the
/// result directly.
pub fn write_appiconset(source: &RgbaImage, appiconset_dir: &Path) -> Result<()> {
    fs::create_dir_all(appiconset_dir)?;

    let mut images = Vec::new();
    for &(idiom, size, scale) in IOS_ICON_SIZES {
        let px = size * scale;
        let filename = format!("icon-{size}x{size}@{scale}x.png");
        image::imageops::resize(source, px, px, FilterType::Lanczos3)
            .save(appiconset_dir.join(&filename))
            .with_context(|| format!("Failed to write icon rendition: {filename}"))?;
        images.push(json!({
            "filename": filename,
            "idiom": idiom,
            "scale": format!("{scale}x"),
            "size": format!("{size}x{size}"),
        }));
    }

    let marketing_filename = "icon-1024x1024@1x.png";
    image::imageops::resize(source, 1024, 1024, FilterType::Lanczos3)
        .save(appiconset_dir.join(marketing_filename))
        .with_context(|| format!("Failed to write icon rendition: {marketing_filename}"))?;
    images.push(json!({
        "filename": marketing_filename,
        "idiom": "ios-marketing",
        "scale": "1x",
        "size": "1024x1024",
    }));

    fs::write(
        appiconset_dir.join("Contents.json"),
        serde_json::to_vec_pretty(&json!({
            "images": images,
            "info": { "author": "xcode", "version": 1 },
        }))?,
    )?;

    Ok(())
}
