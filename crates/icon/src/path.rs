// Reverse-engineered constants for `UIBezierPath(roundedRect:cornerRadius:)`'s
// continuous ("squircle-like") corner, expressed relative to a single corner
// radius `r` so they work at any rect size. Kept at full precision
// even though f32 truncates it, so the constants stay traceable to source
// references (contains similar but slightly different constants):
// - https://www.paintcodeapp.com/news/code-for-ios-7-rounded-rectangles
// - https://liamrosenfeld.com/posts/apple_icon_quest/
#![allow(clippy::excessive_precision)]

const K1: f32 = 1.52866498;
const K2: f32 = 1.08849296;
const K3: f32 = 0.86840694;
const K4: f32 = 0.63149379;
const K5: f32 = 0.37282383;
const K6: f32 = 0.16905956;
const K7: f32 = 0.07491139;

const CURVE_SEGMENTS: usize = 24;

/// Flattens the continuous-corner rounded rect (Apple's "squircle") for a
/// `w`x`h` rect with corner radius `r` into a closed polygon.
pub(crate) fn continuous_rounded_rect(w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    let top_left = |u: f32, v: f32| (u * r, v * r);
    let top_right = |u: f32, v: f32| (w - u * r, v * r);
    let btm_right = |u: f32, v: f32| (w - u * r, h - v * r);
    let btm_left = |u: f32, v: f32| (u * r, h - v * r);

    let mut pts = Vec::with_capacity(4 + CURVE_SEGMENTS * 12);
    pts.push(top_left(K1, 0.0));

    pts.push(top_right(K1, 0.0));
    curve(
        &mut pts,
        top_right(K1, 0.0),
        top_right(K2, 0.0),
        top_right(K3, 0.0),
        top_right(K4, K7),
    );
    curve(
        &mut pts,
        top_right(K4, K7),
        top_right(K5, K6),
        top_right(K6, K5),
        top_right(K7, K4),
    );
    curve(
        &mut pts,
        top_right(K7, K4),
        top_right(0.0, K3),
        top_right(0.0, K2),
        top_right(0.0, K1),
    );

    pts.push(btm_right(0.0, K1));
    curve(
        &mut pts,
        btm_right(0.0, K1),
        btm_right(0.0, K2),
        btm_right(0.0, K3),
        btm_right(K7, K4),
    );
    curve(
        &mut pts,
        btm_right(K7, K4),
        btm_right(K6, K5),
        btm_right(K5, K6),
        btm_right(K4, K7),
    );
    curve(
        &mut pts,
        btm_right(K4, K7),
        btm_right(K3, 0.0),
        btm_right(K2, 0.0),
        btm_right(K1, 0.0),
    );

    pts.push(btm_left(K1, 0.0));
    curve(
        &mut pts,
        btm_left(K1, 0.0),
        btm_left(K2, 0.0),
        btm_left(K3, 0.0),
        btm_left(K4, K7),
    );
    curve(
        &mut pts,
        btm_left(K4, K7),
        btm_left(K5, K6),
        btm_left(K6, K5),
        btm_left(K7, K4),
    );
    curve(
        &mut pts,
        btm_left(K7, K4),
        btm_left(0.0, K3),
        btm_left(0.0, K2),
        btm_left(0.0, K1),
    );

    pts.push(top_left(0.0, K1));
    curve(
        &mut pts,
        top_left(0.0, K1),
        top_left(0.0, K2),
        top_left(0.0, K3),
        top_left(K7, K4),
    );
    curve(
        &mut pts,
        top_left(K7, K4),
        top_left(K6, K5),
        top_left(K5, K6),
        top_left(K4, K7),
    );
    curve(
        &mut pts,
        top_left(K4, K7),
        top_left(K3, 0.0),
        top_left(K2, 0.0),
        top_left(K1, 0.0),
    );

    pts
}

/// The largest corner radius (as a fraction of the shorter side) this curve
/// family supports before opposite corners start to overlap.
pub(crate) const MAX_RADIUS_RATIO: f32 = 1.0 / (2.0 * K1);

fn curve(
    pts: &mut Vec<(f32, f32)>,
    p0: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    p3: (f32, f32),
) {
    for i in 1..=CURVE_SEGMENTS {
        let t = i as f32 / CURVE_SEGMENTS as f32;
        pts.push(cubic_point(p0, c1, c2, p3, t));
    }
}

fn cubic_point(
    p0: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * c1.0 + c * c2.0 + d * p3.0,
        a * p0.1 + b * c1.1 + c * c2.1 + d * p3.1,
    )
}
