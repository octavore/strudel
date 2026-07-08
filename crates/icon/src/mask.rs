use crate::{path, rasterize};

/// Rasterizes the continuous-corner rounded rect (Apple's icon "squircle")
/// at `size`x`size` with corner radius `radius_ratio * size`.
pub(crate) fn build_squircle_mask(size: u32, radius_ratio: f32) -> Vec<f32> {
    let radius_ratio = radius_ratio.min(path::MAX_RADIUS_RATIO);
    let dim = size as f32;
    let polygon = path::continuous_rounded_rect(dim, dim, dim * radius_ratio);
    rasterize::fill_polygon(&polygon, size, size)
}

pub(crate) fn box_blur(mask: &mut [f32], size: u32, radius: usize, passes: usize) {
    for _ in 0..passes {
        box_blur_pass(mask, size, true, radius);
        box_blur_pass(mask, size, false, radius);
    }
}

fn box_blur_pass(mask: &mut [f32], size: u32, horizontal: bool, radius: usize) {
    let size = size as usize;
    let window = radius as f32 * 2.0 + 1.0;
    let src = mask.to_vec();
    for line in 0..size {
        let mut sum = 0.0;
        for k in 0..=radius.min(size - 1) {
            sum += sample(&src, size, line, k, horizontal);
        }
        for i in 0..size {
            mask[index(size, line, i, horizontal)] = sum / window;

            let add = i + radius + 1;
            if add < size {
                sum += sample(&src, size, line, add, horizontal);
            }
            if i >= radius {
                sum -= sample(&src, size, line, i - radius, horizontal);
            }
        }
    }
}

fn index(size: usize, line: usize, i: usize, horizontal: bool) -> usize {
    if horizontal {
        line * size + i
    } else {
        i * size + line
    }
}

fn sample(src: &[f32], size: usize, line: usize, i: usize, horizontal: bool) -> f32 {
    src.get(index(size, line, i, horizontal))
        .copied()
        .unwrap_or(0.0)
}
