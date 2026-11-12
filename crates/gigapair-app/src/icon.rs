//! The tray icon, drawn in code: the Gigapair mark (a G whose bar ends in a
//! block) in paper on an ink tile, so it reads on light and dark taskbars
//! alike. Idle, the block is hollow; while transfers run, it fills with
//! signal yellow, like a verified block in the progress strip.
//!
//! Geometry follows `docs/design/gigapair-visual.html` on a 64-unit grid.

const INK: [u8; 3] = [0x17, 0x18, 0x1C];
const PAPER: [u8; 3] = [0xF1, 0xF2, 0xF4];
const SIGNAL: [u8; 3] = [0xFF, 0xC9, 0x29];

/// The icon as RGBA bytes, `size` × `size`, row by row.
pub fn rgba(size: u32, busy: bool) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Supersample each pixel for smooth edges.
            let mut acc = [0f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let u = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    if let Some(c) = sample(u * 64.0, v * 64.0, busy) {
                        acc[0] += c[0] as f32;
                        acc[1] += c[1] as f32;
                        acc[2] += c[2] as f32;
                        acc[3] += 1.0;
                    }
                }
            }
            let n = acc[3];
            if n == 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                // Straight (not premultiplied) alpha.
                let total = (SAMPLES * SAMPLES) as f32;
                out.push((acc[0] / n).round() as u8);
                out.push((acc[1] / n).round() as u8);
                out.push((acc[2] / n).round() as u8);
                out.push((n / total * 255.0).round() as u8);
            }
        }
    }
    out
}

/// Colour at a point of the 64-unit grid, or `None` where it's transparent.
fn sample(x: f32, y: f32, busy: bool) -> Option<[u8; 3]> {
    if !rounded_rect(x, y, (0.0, 0.0, 64.0, 64.0), 14.0) {
        return None;
    }
    // The block at the end of the bar.
    let block = (31.0, 26.5, 44.5, 37.5);
    if inside(x, y, block) {
        if busy {
            return Some(SIGNAL);
        }
        // Idle: a hollow block, 3 units thick.
        let hole = (block.0 + 3.0, block.1 + 3.0, block.2 - 3.0, block.3 - 3.0);
        return Some(if inside(x, y, hole) { INK } else { PAPER });
    }
    // The G: a rounded ring, open at the top right where the bar turns in.
    let ring = rounded_rect(x, y, (8.5, 8.5, 55.5, 55.5), 15.5)
        && !rounded_rect(x, y, (19.5, 19.5, 44.5, 44.5), 4.5);
    let opening = x > 44.0 && y < 26.5;
    Some(if ring && !opening { PAPER } else { INK })
}

fn inside(x: f32, y: f32, (x0, y0, x1, y1): (f32, f32, f32, f32)) -> bool {
    x >= x0 && x <= x1 && y >= y0 && y <= y1
}

fn rounded_rect(x: f32, y: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> bool {
    if !inside(x, y, (x0, y0, x1, y1)) {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// RGBA to ARGB (the D-Bus StatusNotifierItem format).
#[cfg(any(target_os = "linux", test))]
pub fn to_argb(rgba: &[u8]) -> Vec<u8> {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(px: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * size + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn sizes_and_shapes() {
        let px = rgba(64, false);
        assert_eq!(px.len(), 64 * 64 * 4);
        // Tile corners are transparent.
        assert_eq!(pixel(&px, 64, 0, 0)[3], 0);
        // Left side of the G is paper; the middle of the ring is ink.
        assert_eq!(pixel(&px, 64, 13, 32), [PAPER[0], PAPER[1], PAPER[2], 255]);
        assert_eq!(pixel(&px, 64, 24, 32), [INK[0], INK[1], INK[2], 255]);
        // The opening at the top right is ink, not ring.
        assert_eq!(pixel(&px, 64, 50, 15), [INK[0], INK[1], INK[2], 255]);
        // Idle: the block is hollow.
        assert_eq!(pixel(&px, 64, 32, 32), [PAPER[0], PAPER[1], PAPER[2], 255]);
        assert_eq!(pixel(&px, 64, 37, 32), [INK[0], INK[1], INK[2], 255]);
    }

    #[test]
    fn busy_fills_the_block() {
        let px = rgba(64, true);
        assert_eq!(
            pixel(&px, 64, 37, 32),
            [SIGNAL[0], SIGNAL[1], SIGNAL[2], 255]
        );
        assert_ne!(rgba(32, true), rgba(32, false));
    }

    #[test]
    fn argb_order() {
        assert_eq!(to_argb(&[1, 2, 3, 4]), vec![4, 1, 2, 3]);
    }
}
