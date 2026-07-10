use std::sync::{Arc, OnceLock};

use image::RgbaImage;
use resvg::usvg;
use resvg::usvg::fontdb;

use crate::{IconError, pixmap};

/// The system font database, used to resolve `<text>` in icon artwork.
/// `load_system_fonts` scans the OS font directories, so it's built once and
/// shared - `usvg::Options` only ever needs a handle to it.
fn system_fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Rasterizes SVG `data` to a straight-alpha RGBA image, scaled (preserving
/// aspect ratio) so its longer side is `target_size` pixels. Callers that
/// need an exact canvas size (e.g. to `contain`-fit into a square) should
/// resize the result themselves, same as they already do for raster sources.
///
/// `<text>` resolves against the system fonts. A `font-family` that isn't
/// installed drops the text rather than failing, which is usvg's behavior and
/// not something this crate can detect.
pub(crate) fn rasterize(data: &[u8], target_size: u32) -> Result<RgbaImage, IconError> {
    let options = usvg::Options {
        fontdb: system_fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(data, &options)
        .map_err(|err| IconError::InvalidSvg(err.to_string()))?;

    let size = tree.size();
    let scale = (target_size as f32 / size.width()).min(target_size as f32 / size.height());
    let width = ((size.width() * scale).round() as u32).max(1);
    let height = ((size.height() * scale).round() as u32).max(1);

    let mut target = tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
        IconError::InvalidSvg(format!("cannot allocate a {width}x{height} pixel buffer"))
    })?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut target.as_mut(),
    );

    Ok(pixmap::to_straight(&target))
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    const SQUARE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
        <rect width="100" height="100" fill="#ff0000"/>
    </svg>"##;

    const WIDE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
        <rect width="200" height="100" fill="#00ff00"/>
    </svg>"##;

    #[test]
    fn rasterizes_to_the_target_size() {
        let image = rasterize(SQUARE_SVG.as_bytes(), 64).unwrap();
        assert_eq!((image.width(), image.height()), (64, 64));
        assert_eq!(*image.get_pixel(32, 32), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn preserves_aspect_ratio() {
        let image = rasterize(WIDE_SVG.as_bytes(), 64).unwrap();
        assert_eq!((image.width(), image.height()), (64, 32));
    }

    #[test]
    fn invalid_svg_is_rejected() {
        assert!(rasterize(b"not an svg", 64).is_err());
    }

    /// The name of a font that's actually installed, or `None` on a machine
    /// with no fonts at all. Hardcoding a family would make this test depend
    /// on which fonts the host happens to ship.
    fn an_installed_family() -> Option<String> {
        let fonts = system_fonts();
        let face = fonts.faces().next()?;
        Some(face.families.first()?.0.clone())
    }

    #[test]
    fn renders_text() {
        let Some(family) = an_installed_family() else {
            return;
        };
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <text x="10" y="80" font-family="{family}" font-size="80" fill="#0000ff">A</text>
            </svg>"##
        );

        let image = rasterize(svg.as_bytes(), 64).unwrap();
        let painted = image.pixels().filter(|px| px[3] > 0).count();
        assert!(
            painted > 0,
            "text should rasterize to visible pixels; an empty fontdb silently drops it"
        );
    }
}
