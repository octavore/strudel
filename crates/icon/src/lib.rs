mod color;
mod mask;
mod path;
mod rasterize;

use image::{Rgba, RgbaImage};
use thiserror::Error;

pub use crate::color::parse_hex_color;

#[derive(Debug, Error)]
pub enum IconError {
    #[error("invalid color {0:?}: expected #RRGGBB")]
    InvalidColor(String),
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
    /// `UIBezierPath(roundedRect:cornerRadius:)`. ~0.181 matches Apple's
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
            squircle_scale: 0.84,
            foreground_scale: 0.64,
            corner_radius_ratio: 0.1811,
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
    let mask = mask::build_squircle_mask(squircle_size, options.corner_radius_ratio);

    let mut shadow_mask = mask.clone();
    mask::box_blur(&mut shadow_mask, squircle_size, options.shadow_blur, 3);

    let mut canvas =
        RgbaImage::from_pixel(options.canvas_size, options.canvas_size, Rgba([0, 0, 0, 0]));
    let inset = (options.canvas_size - squircle_size) / 2;

    paint_shadow(&mut canvas, &shadow_mask, squircle_size, inset, options);
    paint_background(&mut canvas, &mask, squircle_size, inset, options);
    paint_foreground(
        &mut canvas,
        &mask,
        squircle_size,
        inset,
        foreground,
        options,
    );
    paint_gloss(&mut canvas, &mask, squircle_size, inset, options);

    Ok(canvas)
}

/// Standard "over" alpha compositing in straight (non-premultiplied) alpha.
/// Coverage/edge pixels must go through this rather than a hardcoded
/// alpha=255 write, otherwise antialiased squircle edges pick up a dark
/// halo from blending against the transparent-black canvas background.
fn blend(dst: Rgba<u8>, src_rgb: [u8; 3], src_alpha: f32) -> Rgba<u8> {
    let src_a = src_alpha.clamp(0.0, 1.0);
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let mix = |d: u8, s: u8| {
        let d = d as f32 / 255.0;
        let s = s as f32 / 255.0;
        let out = (s * src_a + d * dst_a * (1.0 - src_a)) / out_a;
        (out * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Rgba([
        mix(dst[0], src_rgb[0]),
        mix(dst[1], src_rgb[1]),
        mix(dst[2], src_rgb[2]),
        (out_a * 255.0).round() as u8,
    ])
}

fn paint_shadow(
    canvas: &mut RgbaImage,
    shadow_mask: &[f32],
    size: u32,
    inset: u32,
    options: &IconOptions,
) {
    let offset = options.shadow_offset_y as i64;
    for y in 0..size {
        for x in 0..size {
            let a = shadow_mask[(y * size + x) as usize];
            if a <= 0.0 {
                continue;
            }
            let cx = inset as i64 + x as i64;
            let cy = inset as i64 + y as i64 + offset;
            if cx < 0 || cy < 0 || cx >= canvas.width() as i64 || cy >= canvas.height() as i64 {
                continue;
            }
            let px = canvas.get_pixel_mut(cx as u32, cy as u32);
            *px = blend(*px, [0, 0, 0], a * options.shadow_alpha);
        }
    }
}

fn paint_background(
    canvas: &mut RgbaImage,
    mask: &[f32],
    size: u32,
    inset: u32,
    options: &IconOptions,
) {
    let bg = [
        options.background[0],
        options.background[1],
        options.background[2],
    ];
    let bg_alpha = options.background[3] as f32 / 255.0;
    for y in 0..size {
        for x in 0..size {
            let coverage = mask[(y * size + x) as usize];
            if coverage <= 0.0 {
                continue;
            }
            let px = canvas.get_pixel_mut(x + inset, y + inset);
            *px = blend(*px, bg, coverage * bg_alpha);
        }
    }
}

fn paint_foreground(
    canvas: &mut RgbaImage,
    mask: &[f32],
    squircle_size: u32,
    inset: u32,
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
    // (bleeds off the edge, cropped by the mask) rather than being inset.
    let margin_x = (squircle_size as i64 - art_w as i64) / 2;
    let margin_y = (squircle_size as i64 - art_h as i64) / 2;

    for y in 0..art_h {
        for x in 0..art_w {
            let mx = margin_x + x as i64;
            let my = margin_y + y as i64;
            if mx < 0 || my < 0 || mx >= squircle_size as i64 || my >= squircle_size as i64 {
                continue;
            }
            let src = *art.get_pixel(x, y);
            if src[3] == 0 {
                continue;
            }
            let coverage = mask[(my as u32 * squircle_size + mx as u32) as usize];
            if coverage <= 0.0 {
                continue;
            }
            let px = canvas.get_pixel_mut((inset as i64 + mx) as u32, (inset as i64 + my) as u32);
            let alpha = (src[3] as f32 / 255.0) * coverage;
            *px = blend(*px, [src[0], src[1], src[2]], alpha);
        }
    }
}

fn paint_gloss(canvas: &mut RgbaImage, mask: &[f32], size: u32, inset: u32, options: &IconOptions) {
    for y in 0..size {
        let t = y as f32 / size as f32;
        let gloss = (options.gloss_alpha * (1.0 - t * 1.6)).clamp(0.0, options.gloss_alpha);
        if gloss <= 0.0 {
            continue;
        }
        for x in 0..size {
            let coverage = mask[(y * size + x) as usize];
            if coverage <= 0.0 {
                continue;
            }
            let px = canvas.get_pixel_mut(x + inset, y + inset);
            *px = blend(*px, [255, 255, 255], gloss * coverage);
        }
    }
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
