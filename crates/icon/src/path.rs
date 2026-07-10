// Reverse-engineered constants for `UIBezierPath(roundedRect:cornerRadius:)`'s
// continuous ("squircle-like") corner, expressed relative to a single corner
// radius `r` so they work at any rect size. Kept at full precision
// even though f32 truncates it, so the constants stay traceable to source
// references (contains similar but slightly different constants):
// - https://www.paintcodeapp.com/news/code-for-ios-7-rounded-rectangles
// - https://liamrosenfeld.com/posts/apple_icon_quest/
#![allow(clippy::excessive_precision)]

use tiny_skia::{Path, PathBuilder};

const K1: f32 = 1.52866498;
const K2: f32 = 1.08849296;
const K3: f32 = 0.86840694;
const K4: f32 = 0.63149379;
const K5: f32 = 0.37282383;
const K6: f32 = 0.16905956;
const K7: f32 = 0.07491139;

/// Builds the continuous-corner rounded rect (Apple's "squircle") for a
/// `w`x`h` rect with corner radius `r` as a real cubic-Bezier path, rather
/// than a flattened polygon; `tiny_skia`'s rasterizer handles the curves
/// directly.
pub(crate) fn continuous_rounded_rect(w: f32, h: f32, r: f32) -> Path {
    let top_left = |u: f32, v: f32| (u * r, v * r);
    let top_right = |u: f32, v: f32| (w - u * r, v * r);
    let btm_right = |u: f32, v: f32| (w - u * r, h - v * r);
    let btm_left = |u: f32, v: f32| (u * r, h - v * r);

    let mut pb = PathBuilder::new();
    let (x, y) = top_left(K1, 0.0);
    pb.move_to(x, y);

    let (x, y) = top_right(K1, 0.0);
    pb.line_to(x, y);
    corner(
        &mut pb,
        top_right(K2, 0.0),
        top_right(K3, 0.0),
        top_right(K4, K7),
    );
    corner(
        &mut pb,
        top_right(K5, K6),
        top_right(K6, K5),
        top_right(K7, K4),
    );
    corner(
        &mut pb,
        top_right(0.0, K3),
        top_right(0.0, K2),
        top_right(0.0, K1),
    );

    let (x, y) = btm_right(0.0, K1);
    pb.line_to(x, y);
    corner(
        &mut pb,
        btm_right(0.0, K2),
        btm_right(0.0, K3),
        btm_right(K7, K4),
    );
    corner(
        &mut pb,
        btm_right(K6, K5),
        btm_right(K5, K6),
        btm_right(K4, K7),
    );
    corner(
        &mut pb,
        btm_right(K3, 0.0),
        btm_right(K2, 0.0),
        btm_right(K1, 0.0),
    );

    let (x, y) = btm_left(K1, 0.0);
    pb.line_to(x, y);
    corner(
        &mut pb,
        btm_left(K2, 0.0),
        btm_left(K3, 0.0),
        btm_left(K4, K7),
    );
    corner(
        &mut pb,
        btm_left(K5, K6),
        btm_left(K6, K5),
        btm_left(K7, K4),
    );
    corner(
        &mut pb,
        btm_left(0.0, K3),
        btm_left(0.0, K2),
        btm_left(0.0, K1),
    );

    let (x, y) = top_left(0.0, K1);
    pb.line_to(x, y);
    corner(
        &mut pb,
        top_left(0.0, K2),
        top_left(0.0, K3),
        top_left(K7, K4),
    );
    corner(
        &mut pb,
        top_left(K6, K5),
        top_left(K5, K6),
        top_left(K4, K7),
    );
    corner(
        &mut pb,
        top_left(K3, 0.0),
        top_left(K2, 0.0),
        top_left(K1, 0.0),
    );

    pb.close();
    pb.finish().expect("squircle path is always well-formed")
}

/// The largest corner radius (as a fraction of the shorter side) this curve
/// family supports before opposite corners start to overlap.
pub(crate) const MAX_RADIUS_RATIO: f32 = 1.0 / (2.0 * K1);

fn corner(pb: &mut PathBuilder, c1: (f32, f32), c2: (f32, f32), p: (f32, f32)) {
    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1);
}
