/// Blurs an alpha mask in place with a separable box blur (3 passes
/// approximates a Gaussian closely enough for a drop shadow).
pub(crate) fn box_blur(mask: &mut tiny_skia::Mask, radius: usize, passes: usize) {
    let size = mask.width() as usize;
    debug_assert_eq!(mask.width(), mask.height());
    let data = mask.data_mut();
    for _ in 0..passes {
        box_blur_pass(data, size, true, radius);
        box_blur_pass(data, size, false, radius);
    }
}

fn box_blur_pass(mask: &mut [u8], size: usize, horizontal: bool, radius: usize) {
    let window = radius as u32 * 2 + 1;
    let src = mask.to_vec();
    for line in 0..size {
        let mut sum = 0u32;
        for k in 0..=radius.min(size - 1) {
            sum += sample(&src, size, line, k, horizontal) as u32;
        }
        for i in 0..size {
            // Round rather than truncate: `passes * 2` divisions each biasing
            // down would visibly lighten the shadow. `window` is always odd,
            // so `window / 2` is exactly the half-step.
            mask[index(size, line, i, horizontal)] = ((sum + window / 2) / window) as u8;

            let add = i + radius + 1;
            if add < size {
                sum += sample(&src, size, line, add, horizontal) as u32;
            }
            if i >= radius {
                sum -= sample(&src, size, line, i - radius, horizontal) as u32;
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

fn sample(src: &[u8], size: usize, line: usize, i: usize, horizontal: bool) -> u8 {
    src.get(index(size, line, i, horizontal))
        .copied()
        .unwrap_or(0)
}
