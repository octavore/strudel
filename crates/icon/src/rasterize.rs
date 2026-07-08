/// Vertical supersampling factor; horizontal coverage is computed exactly
/// per scanline, so only the vertical axis needs supersampling for AA.
const SUBSAMPLES: usize = 4;

/// Fills a closed polygon into a `width`x`height` coverage buffer using an
/// even-odd scanline rasterizer with fractional pixel coverage at edges.
pub(crate) fn fill_polygon(points: &[(f32, f32)], width: u32, height: u32) -> Vec<f32> {
    let w = width as usize;
    let h = height as usize;
    let mut coverage = vec![0.0f32; w * h];

    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(points.len());
    for i in 0..points.len() {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % points.len()];
        if y0 != y1 {
            edges.push((x0, y0, x1, y1));
        }
    }

    let mut xs: Vec<f32> = Vec::new();
    for row in 0..h {
        let row_buf = &mut coverage[row * w..row * w + w];
        for s in 0..SUBSAMPLES {
            let y = row as f32 + (s as f32 + 0.5) / SUBSAMPLES as f32;
            xs.clear();
            for &(x0, y0, x1, y1) in &edges {
                let (ylo, yhi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if y < ylo || y >= yhi {
                    continue;
                }
                let t = (y - y0) / (y1 - y0);
                xs.push(x0 + t * (x1 - x0));
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for span in xs.chunks_exact(2) {
                accumulate_span(row_buf, span[0], span[1], 1.0 / SUBSAMPLES as f32);
            }
        }
    }

    coverage
}

fn accumulate_span(row: &mut [f32], xa: f32, xb: f32, weight: f32) {
    let len = row.len() as f32;
    let xa = xa.clamp(0.0, len);
    let xb = xb.clamp(0.0, len);
    if xb <= xa {
        return;
    }
    let xa_i = xa.floor() as usize;
    let xb_i = xb.floor() as usize;
    if xa_i == xb_i {
        if xa_i < row.len() {
            row[xa_i] += (xb - xa) * weight;
        }
        return;
    }
    if xa_i < row.len() {
        row[xa_i] += (xa_i as f32 + 1.0 - xa) * weight;
    }
    for x in (xa_i + 1)..xb_i.min(row.len()) {
        row[x] += weight;
    }
    if xb_i < row.len() {
        row[xb_i] += (xb - xb_i as f32) * weight;
    }
}
