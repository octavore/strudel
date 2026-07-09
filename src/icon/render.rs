//! Renders a [`ResolvedIcon`] to a flat PNG, regardless of platform. Shared
//! between the macOS bundler (which may further convert the result to
//! `.icns`) and the iOS bundler (which feeds it to `actool`). The actual
//! squircle compositing lives in the `icon` crate; this module just maps
//! [`ResolvedIcon`]'s config-facing shape onto it (and, for iOS, onto
//! [`crate::icon::ios`]).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use image::RgbaImage;

use crate::config::ResolvedIcon;
use crate::icon::ios::pad_to_square;

/// Write `icon`'s artwork to `dest` as a plain PNG (or, for
/// [`ResolvedIcon::Path`], simply copied as-is). Ignores the `icns` option on
/// both variants; callers that want a real `.icns` convert the result
/// themselves.
pub fn render_to_png(icon: &ResolvedIcon, dest: &Path) -> Result<()> {
    match icon {
        ResolvedIcon::Path { path, .. } => {
            fs::copy(path, dest)
                .with_context(|| format!("Failed to copy icon: {}", path.display()))?;
            Ok(())
        },
        ResolvedIcon::Generated {
            src,
            scale,
            background,
            ..
        } => {
            let default_options = icon::IconOptions::default();
            let foreground = icon::load_foreground(src, default_options.canvas_size)
                .with_context(|| format!("Failed to read icon source image: {}", src.display()))?;

            let background = match background {
                Some(hex) => icon::parse_hex_color(hex)
                    .with_context(|| format!("Invalid icon background color {hex:?}"))?,
                None => default_options.background,
            };
            let options = icon::IconOptions {
                foreground_scale: default_options.foreground_scale * scale,
                background,
                ..default_options
            };
            let canvas =
                icon::generate(&foreground, &options).context("Failed to generate app icon")?;
            // `dest` may be named `AppIcon.icns` even though these are always
            // PNG bytes (macOS sniffs content, not the extension). `save()`
            // picks its encoder from the extension, and `image` doesn't know
            // `.icns` at all, so it would bail before encoding.
            canvas
                .save_with_format(dest, image::ImageFormat::Png)
                .with_context(|| format!("Failed to write generated icon: {}", dest.display()))
        },
    }
}

/// Render `icon`'s artwork as a padded, fully opaque square, suitable as the
/// single source image an iOS `.appiconset` is derived from (see
/// [`crate::icon::ios::write_appiconset`]). Unlike the macOS squircle
/// compositor (shadow/gloss/rounded mask - the OS applies its own mask on
/// iOS), this just centers the artwork over a flat background color, scaled
/// to leave the same proportional padding as the macOS default.
pub fn render_ios_icon(icon: &ResolvedIcon, canvas_size: u32) -> Result<RgbaImage> {
    let (path, scale, background) = match icon {
        ResolvedIcon::Path { path, .. } => (path, 1.0, None),
        ResolvedIcon::Generated {
            src,
            scale,
            background,
            ..
        } => (src, *scale, background.as_deref()),
    };

    let foreground = icon::load_foreground(path, canvas_size)
        .with_context(|| format!("Failed to read icon source image: {}", path.display()))?;

    let default_options = icon::IconOptions::default();
    let background = match background {
        Some(hex) => icon::parse_hex_color(hex)
            .with_context(|| format!("Invalid icon background color {hex:?}"))?,
        None => default_options.background,
    };

    Ok(pad_to_square(
        &foreground,
        canvas_size,
        default_options.foreground_scale * scale,
        background,
    ))
}
