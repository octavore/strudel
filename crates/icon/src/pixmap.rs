//! Conversions across the alpha boundary between `tiny_skia` and `image`.
//!
//! `tiny_skia` composites in premultiplied alpha; `image` (and everything this
//! crate hands back to callers) works in straight alpha.

use image::{Rgba, RgbaImage};
use tiny_skia::{ColorU8, IntSize, Pixmap};

/// Converts a straight-alpha `RgbaImage` into a premultiplied-alpha `Pixmap`.
pub(crate) fn to_premultiplied(image: &RgbaImage) -> Pixmap {
    let (width, height) = image.dimensions();
    let mut data = Vec::with_capacity(image.as_raw().len());
    for px in image.pixels() {
        let c = ColorU8::from_rgba(px[0], px[1], px[2], px[3]).premultiply();
        data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Pixmap::from_vec(
        data,
        IntSize::from_wh(width, height).expect("image is nonzero"),
    )
    .expect("data length matches size")
}

/// Converts a premultiplied-alpha `Pixmap` into a straight-alpha `RgbaImage`.
pub(crate) fn to_straight(pixmap: &Pixmap) -> RgbaImage {
    let mut image = RgbaImage::new(pixmap.width(), pixmap.height());
    for (src, dst) in pixmap.pixels().iter().zip(image.pixels_mut()) {
        let c = src.demultiply();
        *dst = Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_translucent_pixel() {
        // Half-alpha red: premultiplying quantizes 255 -> 128, so the straight
        // color comes back close but not identical. Verify it stays within a
        // rounding step rather than drifting.
        let image = RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 128]));
        let back = to_straight(&to_premultiplied(&image));
        let px = back.get_pixel(0, 0);
        assert_eq!(px[3], 128);
        assert!(px[0] >= 254, "red channel survived the round trip: {px:?}");
        assert_eq!((px[1], px[2]), (0, 0));
    }

    #[test]
    fn a_transparent_pixel_stays_transparent() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([255, 255, 255, 0]));
        let back = to_straight(&to_premultiplied(&image));
        assert_eq!(back.get_pixel(0, 0)[3], 0);
    }
}
