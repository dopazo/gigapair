//! Placeholder tray icon, drawn in code until Gigapair has a real icon: a
//! blue rounded square with two white dots (a pair). While transfers run it
//! gets a green badge in the corner.

const BLUE: [u8; 3] = [0x25, 0x63, 0xEB];
const GREEN: [u8; 3] = [0x22, 0xC5, 0x5E];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

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
                    if let Some(c) = sample(u, v, busy) {
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

/// Colour at a point of the unit square, or `None` where it's transparent.
fn sample(u: f32, v: f32, busy: bool) -> Option<[u8; 3]> {
    let in_circle = |cx: f32, cy: f32, r: f32| (u - cx).powi(2) + (v - cy).powi(2) <= r * r;
    if busy && in_circle(0.8, 0.8, 0.2) {
        return Some(if in_circle(0.8, 0.8, 0.14) {
            GREEN
        } else {
            WHITE
        });
    }
    if in_circle(0.36, 0.5, 0.15) || in_circle(0.64, 0.5, 0.15) {
        return Some(WHITE);
    }
    rounded_square(u, v, 0.04, 0.22).then_some(BLUE)
}

fn rounded_square(u: f32, v: f32, margin: f32, radius: f32) -> bool {
    let (lo, hi) = (margin, 1.0 - margin);
    if u < lo || u > hi || v < lo || v > hi {
        return false;
    }
    let cx = u.clamp(lo + radius, hi - radius);
    let cy = v.clamp(lo + radius, hi - radius);
    (u - cx).powi(2) + (v - cy).powi(2) <= radius * radius
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

    #[test]
    fn sizes_and_transparency() {
        let px = rgba(32, false);
        assert_eq!(px.len(), 32 * 32 * 4);
        // Corner is transparent, centre of a dot is white, between is blue.
        assert_eq!(px[3], 0);
        let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(at(11, 16), &[255, 255, 255, 255]);
        assert_eq!(at(16, 6), &[BLUE[0], BLUE[1], BLUE[2], 255]);
        let busy = rgba(32, true);
        let at = |x: usize, y: usize| &busy[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(&at(25, 25)[..3], &GREEN);
    }

    #[test]
    fn argb_order() {
        assert_eq!(to_argb(&[1, 2, 3, 4]), vec![4, 1, 2, 3]);
    }
}
