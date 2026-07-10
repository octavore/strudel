mod color;
mod mask;
mod path;
mod pixmap;
mod svg;

use std::path::Path;

use image::{Rgba, RgbaImage};
use thiserror::Error;
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Mask, Paint, Pixmap, PixmapPaint, Point, Rect,
    SpreadMode, Transform,
};

pub use crate::color::parse_hex_color;

#[derive(Debug, Error)]
pub enum IconError {
    #[error("invalid color {0:?}: expected #RRGGBB")]
    InvalidColor(String),
    #[error("invalid SVG: {0}")]
    InvalidSvg(String),
    #[error("failed to read icon source {path:?}: {source}")]
    ReadForeground {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode icon source {path:?}: {source}")]
    DecodeForeground {
        path: std::path::PathBuf,
        #[source]
        source: image::ImageError,
    },
}

/// Loads `path` as foreground artwork, rasterizing it if it's an SVG (`.svg`
/// extension, case-insensitive) or decoding it as a raster image otherwise.
/// `target_size` bounds the SVG rasterization resolution (its longer side);
/// it's ignored for raster sources, which are decoded at their native size.
pub fn load_foreground(path: &Path, target_size: u32) -> Result<RgbaImage, IconError> {
    let is_svg = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));

    if is_svg {
        let data = std::fs::read(path).map_err(|source| IconError::ReadForeground {
            path: path.to_path_buf(),
            source,
        })?;
        svg::rasterize(&data, target_size)
    } else {
        Ok(image::open(path)
            .map_err(|source| IconError::DecodeForeground {
                path: path.to_path_buf(),
                source,
            })?
            .to_rgba8())
    }
}

/// Tunable parameters for [`generate`]. Defaults approximate the macOS
/// Big Sur+ squircle icon style.
#[derive(Debug, Clone)]
pub struct IconOptions {
    pub canvas_size: u32,
    pub background: Rgba<u8>,
    /// Fraction of the canvas the squircle itself occupies.
    pub squircle_scale: f32,
    /// Fraction of the squircle the foreground artwork occupies.
    pub foreground_scale: f32,
    /// Corner radius as a fraction of the squircle's size, fed into the
    /// same continuous-corner curve as
    /// `UIBezierPath(roundedRect:cornerRadius:)`. ~0.245 matches Apple's
    /// 1024pt macOS icon template.
    pub corner_radius_ratio: f32,
    pub shadow_blur: usize,
    pub shadow_offset_y: f32,
    pub shadow_alpha: f32,
    pub gloss_alpha: f32,
}

impl Default for IconOptions {
    fn default() -> Self {
        IconOptions {
            canvas_size: 1024,
            background: Rgba([255, 255, 255, 255]),
            squircle_scale: 0.81,
            foreground_scale: 0.64,
            corner_radius_ratio: 0.245,
            shadow_blur: 24,
            shadow_offset_y: 10.0,
            shadow_alpha: 0.35,
            gloss_alpha: 0.22,
        }
    }
}

/// Composites `foreground` onto a squircle of `options.background`, with a
/// drop shadow and a top gloss highlight, centered on a transparent canvas.
pub fn generate(foreground: &RgbaImage, options: &IconOptions) -> Result<RgbaImage, IconError> {
    let squircle_size = (options.canvas_size as f32 * options.squircle_scale) as u32;
    let dim = squircle_size as f32;
    let radius = dim * options.corner_radius_ratio.min(path::MAX_RADIUS_RATIO);
    let squircle_path = path::continuous_rounded_rect(dim, dim, radius);

    let inset = ((options.canvas_size - squircle_size) / 2) as f32;
    let squircle_transform = Transform::from_translate(inset, inset);

    let mut canvas = Pixmap::new(options.canvas_size, options.canvas_size)
        .expect("canvas_size is always nonzero");

    // The squircle's exact silhouette, at canvas position, reused to clip the
    // foreground artwork (whose own draw call can't take a path directly).
    let mut squircle_mask =
        Mask::new(options.canvas_size, options.canvas_size).expect("canvas_size is always nonzero");
    squircle_mask.fill_path(&squircle_path, FillRule::Winding, true, squircle_transform);

    paint_shadow(&mut canvas, &squircle_path, inset, options);
    paint_background(&mut canvas, &squircle_path, squircle_transform, options);
    paint_foreground(
        &mut canvas,
        &squircle_mask,
        squircle_size,
        inset,
        foreground,
        options,
    );
    paint_gloss(
        &mut canvas,
        &squircle_path,
        squircle_transform,
        dim,
        options,
    );

    Ok(pixmap::to_straight(&canvas))
}

fn paint_shadow(
    canvas: &mut Pixmap,
    squircle_path: &tiny_skia::Path,
    inset: f32,
    options: &IconOptions,
) {
    let mut shadow_mask = Mask::new(canvas.width(), canvas.height()).expect("canvas is nonzero");
    let shadow_transform = Transform::from_translate(inset, inset + options.shadow_offset_y);
    shadow_mask.fill_path(squircle_path, FillRule::Winding, true, shadow_transform);
    mask::box_blur(&mut shadow_mask, options.shadow_blur, 3);

    let full_canvas = Rect::from_xywh(0.0, 0.0, canvas.width() as f32, canvas.height() as f32)
        .expect("canvas is nonzero");
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, to_alpha_u8(options.shadow_alpha));
    canvas.fill_rect(
        full_canvas,
        &paint,
        Transform::identity(),
        Some(&shadow_mask),
    );
}

fn paint_background(
    canvas: &mut Pixmap,
    squircle_path: &tiny_skia::Path,
    squircle_transform: Transform,
    options: &IconOptions,
) {
    let bg = options.background;
    let mut paint = Paint::default();
    paint.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
    canvas.fill_path(
        squircle_path,
        &paint,
        FillRule::Winding,
        squircle_transform,
        None,
    );
}

fn paint_foreground(
    canvas: &mut Pixmap,
    squircle_mask: &Mask,
    squircle_size: u32,
    inset: f32,
    foreground: &RgbaImage,
    options: &IconOptions,
) {
    let art_box = ((squircle_size as f32 * options.foreground_scale) as u32).max(1);

    // Fit `foreground` inside the `art_box` x `art_box` area, preserving its
    // aspect ratio ("contain") rather than requiring square input and
    // stretching it. The shorter axis ends up with less coverage, centered.
    let (fw, fh) = (foreground.width() as f32, foreground.height() as f32);
    let fit_scale = (art_box as f32 / fw).min(art_box as f32 / fh);
    let art_w = ((fw * fit_scale).round() as u32).max(1);
    let art_h = ((fh * fit_scale).round() as u32).max(1);
    let art = image::imageops::resize(
        foreground,
        art_w,
        art_h,
        image::imageops::FilterType::Lanczos3,
    );

    // Signed: foreground_scale > 1.0 means the art overscans the squircle
    // (bleeds off the edge, cropped by `squircle_mask`) rather than being inset.
    let margin_x = (squircle_size as i64 - art_w as i64) / 2;
    let margin_y = (squircle_size as i64 - art_h as i64) / 2;

    let art_pixmap = pixmap::to_premultiplied(&art);
    canvas.draw_pixmap(
        (inset as i64 + margin_x) as i32,
        (inset as i64 + margin_y) as i32,
        art_pixmap.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        Some(squircle_mask),
    );
}

fn paint_gloss(
    canvas: &mut Pixmap,
    squircle_path: &tiny_skia::Path,
    squircle_transform: Transform,
    squircle_dim: f32,
    options: &IconOptions,
) {
    if options.gloss_alpha <= 0.0 {
        return;
    }
    // A vertical ramp from `gloss_alpha` at the top, fading to 0 by 62.5% of
    // the way down (`1.0 / 1.6`); `SpreadMode::Pad` clamps the rest to
    // transparent, matching the original `(1.0 - t * 1.6).clamp(0.0, alpha)`.
    let top = Color::from_rgba8(255, 255, 255, to_alpha_u8(options.gloss_alpha));
    let bottom = Color::from_rgba8(255, 255, 255, 0);
    let Some(shader) = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, squircle_dim / 1.6),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    ) else {
        return;
    };
    let paint = Paint {
        shader,
        ..Paint::default()
    };
    canvas.fill_path(
        squircle_path,
        &paint,
        FillRule::Winding,
        squircle_transform,
        None,
    );
}

fn to_alpha_u8(alpha: f32) -> u8 {
    (alpha.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const FG: [u8; 3] = [10, 20, 30];

    /// A wide (2.5:1) foreground, and options with the gloss switched off so
    /// painted pixels keep the exact `FG` color. Tests about placement should
    /// not have to model the gloss ramp; `gloss_lightens_the_artwork` covers
    /// that separately.
    fn wide_foreground() -> (RgbaImage, IconOptions) {
        let fg = RgbaImage::from_pixel(200, 80, Rgba([FG[0], FG[1], FG[2], 255]));
        let options = IconOptions {
            gloss_alpha: 0.0,
            ..IconOptions::default()
        };
        (fg, options)
    }

    fn rgb(canvas: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
        let px = canvas.get_pixel(x, y);
        [px[0], px[1], px[2]]
    }

    #[test]
    fn generate_accepts_non_square_foreground() {
        // A wide, non-square foreground should be fit inside the artwork
        // area (letterboxed) rather than rejected or stretched.
        let (fg, options) = wide_foreground();
        let canvas = generate(&fg, &options).expect("non-square foreground should be accepted");
        assert_eq!(canvas.width(), options.canvas_size);
        assert_eq!(canvas.height(), options.canvas_size);

        // The center picked up the foreground, so the art was really painted
        // rather than skipped or cropped away.
        let mid = canvas.width() / 2;
        assert_eq!(rgb(&canvas, mid, mid), FG);

        // 200px above center is outside the letterboxed art but still well
        // inside the squircle, so it must show the background. If the art were
        // stretched to fill instead of fit, this would be FG too.
        assert_eq!(
            rgb(&canvas, mid, mid - 200),
            [255, 255, 255],
            "a 2.5:1 foreground must be letterboxed, not stretched"
        );
    }

    #[test]
    fn gloss_lightens_the_artwork() {
        // The gloss is a white ramp over the whole squircle, strongest at the
        // top. It tints the artwork, so a pixel of painted art comes back
        // lighter than the source color under default options.
        let (fg, _) = wide_foreground();
        let canvas = generate(&fg, &IconOptions::default()).unwrap();
        let mid = canvas.width() / 2;
        let center = rgb(&canvas, mid, mid);
        assert_ne!(center, FG, "default options apply gloss");
        for (channel, source) in center.iter().zip(FG) {
            assert!(
                *channel > source,
                "gloss blends toward white: {center:?} vs {FG:?}"
            );
        }
    }
}
