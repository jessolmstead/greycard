//! Clone and heal: a region of the picture replaced from another
//! part of it. Clone copies the source under a soft edge. Heal keeps
//! the source's texture and gives it the destination's color and
//! light, the classic frequency separation, with the destination's
//! low frequencies taken from outside the region so the blemish being
//! removed does not color its own replacement.

use crate::image::WorkingImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Clone,
    Heal,
}

/// A destination: a window of the picture, each pixel's coverage (0
/// to 1, the region itself with its soft edge) and the ring just
/// outside it that says what the region's surroundings look like.
#[derive(Debug, Clone)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub cover: Vec<f32>,
    pub ring: Vec<f32>,
}

impl Region {
    /// The region's own size, in pixels: the extent of its coverage.
    fn radius(&self) -> f32 {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.cover[y * self.width + x] > 0.0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == usize::MAX {
            return 1.0;
        }
        ((x1 - x0).max(y1 - y0) as f32 / 2.0).max(1.0)
    }
}

/// Where to take the source from: the offset whose surroundings match
/// the region's ring best, from candidates on two rings around it.
pub fn find_source(image: &WorkingImage, region: &Region) -> (i32, i32) {
    let r = region.radius();
    let mut best = ((0, 0), f32::INFINITY);
    for &scale in &[2.2f32, 3.2] {
        for k in 0..16 {
            let angle = k as f32 * std::f32::consts::TAU / 16.0;
            let offset = (
                (angle.cos() * r * scale).round() as i32,
                (angle.sin() * r * scale).round() as i32,
            );
            if !inside(image, region, offset) {
                continue;
            }
            let score = ring_score(image, region, offset);
            if score < best.1 {
                best = (offset, score);
            }
        }
    }
    best.0
}

fn inside(image: &WorkingImage, region: &Region, offset: (i32, i32)) -> bool {
    let x0 = region.x as i32 + offset.0;
    let y0 = region.y as i32 + offset.1;
    x0 >= 0
        && y0 >= 0
        && x0 + region.width as i32 <= image.width as i32
        && y0 + region.height as i32 <= image.height as i32
}

/// The mean squared difference over the ring between the picture and
/// the picture shifted by `offset`.
fn ring_score(image: &WorkingImage, region: &Region, offset: (i32, i32)) -> f32 {
    let (mut sum, mut weight) = (0.0f32, 0.0f32);
    for y in 0..region.height {
        for x in 0..region.width {
            let w = region.ring[y * region.width + x];
            if w <= 0.0 {
                continue;
            }
            let d = pixel(image, region.x + x, region.y + y);
            let s = pixel(
                image,
                (region.x as i32 + x as i32 + offset.0) as usize,
                (region.y as i32 + y as i32 + offset.1) as usize,
            );
            sum += w * ((d[0] - s[0]).powi(2) + (d[1] - s[1]).powi(2) + (d[2] - s[2]).powi(2));
            weight += w;
        }
    }
    if weight > 0.0 {
        sum / weight
    } else {
        f32::INFINITY
    }
}

fn pixel(image: &WorkingImage, x: usize, y: usize) -> [f32; 3] {
    let x = x.min(image.width - 1);
    let y = y.min(image.height - 1);
    let i = (y * image.width + x) * 3;
    [image.data[i], image.data[i + 1], image.data[i + 2]]
}

/// Replace the region from the picture `offset` away, by `method`, at
/// `opacity`.
pub fn apply(
    image: &mut WorkingImage,
    region: &Region,
    offset: (i32, i32),
    method: Method,
    opacity: f32,
) {
    let (w, h) = (region.width, region.height);
    // The source window, clamped at the picture's edge.
    let mut src = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let sx = (region.x as i32 + x as i32 + offset.0).max(0) as usize;
            let sy = (region.y as i32 + y as i32 + offset.1).max(0) as usize;
            src[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&pixel(image, sx, sy));
        }
    }
    let replacement = match method {
        Method::Clone => src,
        Method::Heal => {
            let mut dst = vec![0.0f32; w * h * 3];
            for y in 0..h {
                for x in 0..w {
                    dst[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&pixel(
                        image,
                        region.x + x,
                        region.y + y,
                    ));
                }
            }
            let sigma = (region.radius() / 2.0).max(1.5);
            let src_low = blur(&src, w, h, sigma, None);
            let outside: Vec<f32> = region.cover.iter().map(|c| 1.0 - c).collect();
            let dst_low = blur(&dst, w, h, sigma, Some(&outside));
            src.iter()
                .zip(&src_low)
                .zip(&dst_low)
                .map(|((s, sl), dl)| s - sl + dl)
                .collect()
        }
    };
    apply_replacement(image, region, &replacement, opacity);
}

/// Blend `replacement`, the region's window in the picture's own
/// values, into the picture under the coverage at `opacity`.
pub fn apply_replacement(
    image: &mut WorkingImage,
    region: &Region,
    replacement: &[f32],
    opacity: f32,
) {
    let w = region.width;
    for y in 0..region.height {
        for x in 0..w {
            let a = (region.cover[y * w + x] * opacity).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let i = ((region.y + y) * image.width + region.x + x) * 3;
            let j = (y * w + x) * 3;
            for c in 0..3 {
                image.data[i + c] += (replacement[j + c] - image.data[i + c]) * a;
            }
        }
    }
}

/// A Gaussian blur of three-channel `data`, as three box blurs. With
/// `weight` the blur is normalized by the blurred weight, so pixels
/// weighted nothing take their value from those around them.
fn blur(data: &[f32], w: usize, h: usize, sigma: f32, weight: Option<&[f32]>) -> Vec<f32> {
    let r = (((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0)
        .round()
        .max(1.0) as usize;
    let mut v: Vec<f32> = match weight {
        Some(wt) => data
            .iter()
            .enumerate()
            .map(|(i, d)| d * wt[i / 3])
            .collect(),
        None => data.to_vec(),
    };
    let mut wt: Option<Vec<f32>> = weight.map(|w| w.to_vec());
    for _ in 0..3 {
        v = box_pass(&v, w, h, 3, r);
        if let Some(m) = wt.take() {
            wt = Some(box_pass(&m, w, h, 1, r));
        }
    }
    match wt {
        Some(m) => v
            .iter()
            .enumerate()
            .map(|(i, x)| x / m[i / 3].max(1e-6))
            .collect(),
        None => v,
    }
}

/// One box blur, rows then columns, `ch` channels, window clamped at
/// the edges.
fn box_pass(src: &[f32], w: usize, h: usize, ch: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; src.len()];
    for y in 0..h {
        for c in 0..ch {
            let mut sum = vec![0.0f32; w + 1];
            for x in 0..w {
                sum[x + 1] = sum[x] + src[(y * w + x) * ch + c];
            }
            for x in 0..w {
                let a = x.saturating_sub(r);
                let b = (x + r + 1).min(w);
                rows[(y * w + x) * ch + c] = (sum[b] - sum[a]) / (b - a) as f32;
            }
        }
    }
    let mut out = vec![0.0f32; src.len()];
    for x in 0..w {
        for c in 0..ch {
            let mut sum = vec![0.0f32; h + 1];
            for y in 0..h {
                sum[y + 1] = sum[y] + rows[(y * w + x) * ch + c];
            }
            for y in 0..h {
                let a = y.saturating_sub(r);
                let b = (y + r + 1).min(h);
                out[(y * w + x) * ch + c] = (sum[b] - sum[a]) / (b - a) as f32;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64×64 picture: a horizontal gradient, a dark blemish at
    /// (20, 32) of radius 4, and a region over the blemish.
    fn scene() -> (WorkingImage, Region) {
        let (w, h) = (64usize, 64usize);
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let base = 0.2 + 0.6 * x as f32 / w as f32;
                let d = ((x as f32 - 20.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt();
                let v = if d < 4.0 { 0.02 } else { base };
                data.extend([v, v * 0.9, v * 0.8]);
            }
        }
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let (r, ring) = (6.0f32, 10.0f32);
        let (x0, y0) = (20 - 10, 32 - 10);
        let (rw, rh) = (21usize, 21usize);
        let mut cover = Vec::new();
        let mut ringw = Vec::new();
        for y in 0..rh {
            for x in 0..rw {
                let d =
                    (((x0 + x) as f32 - 20.0).powi(2) + ((y0 + y) as f32 - 32.0).powi(2)).sqrt();
                cover.push(if d <= r { 1.0 } else { 0.0 });
                ringw.push(if d > r && d <= ring { 1.0 } else { 0.0 });
            }
        }
        let region = Region {
            x: x0,
            y: y0,
            width: rw,
            height: rh,
            cover,
            ring: ringw,
        };
        (image, region)
    }

    #[test]
    fn a_clone_copies_the_source_under_the_cover() {
        let (mut image, region) = scene();
        let before = image.clone();
        apply(&mut image, &region, (0, 20), Method::Clone, 1.0);
        // Inside: the source's value; outside: untouched.
        assert_eq!(pixel(&image, 20, 32), pixel(&before, 20, 52));
        assert_eq!(pixel(&image, 5, 5), pixel(&before, 5, 5));
        assert_eq!(pixel(&image, 20, 45), pixel(&before, 20, 45));
    }

    #[test]
    fn a_heal_takes_the_blemish_out_and_keeps_the_gradient() {
        let (mut image, region) = scene();
        let before = image.clone();
        // A source above: clean gradient, same columns.
        apply(&mut image, &region, (0, -20), Method::Heal, 1.0);
        for x in 16..=24 {
            let healed = pixel(&image, x, 32)[0];
            let expected = 0.2 + 0.6 * x as f32 / 64.0;
            assert!(
                (healed - expected).abs() < 0.03,
                "x {x}: healed {healed} expected {expected}"
            );
        }
        assert_eq!(pixel(&image, 5, 5), pixel(&before, 5, 5));
    }

    #[test]
    fn opacity_halves_the_change() {
        let (mut full, region) = scene();
        let mut half = full.clone();
        let before = full.clone();
        apply(&mut full, &region, (0, 20), Method::Clone, 1.0);
        apply(&mut half, &region, (0, 20), Method::Clone, 0.5);
        let (b, f, hf) = (
            pixel(&before, 20, 32)[0],
            pixel(&full, 20, 32)[0],
            pixel(&half, 20, 32)[0],
        );
        assert!((hf - (b + f) / 2.0).abs() < 1e-5);
    }

    #[test]
    fn the_source_finder_prefers_matching_surroundings() {
        // The gradient runs along x, so a source straight above or
        // below matches the ring; one to the side does not.
        let (image, region) = scene();
        let (dx, dy) = find_source(&image, &region);
        assert!(dx.abs() <= 2, "offset ({dx}, {dy}) should be vertical");
        assert!(dy.abs() >= 10);
    }

    #[test]
    fn a_weighted_blur_ignores_what_is_weighted_out() {
        let (w, h) = (16usize, 16usize);
        let mut data = vec![0.5f32; w * h * 3];
        let mut weight = vec![1.0f32; w * h];
        // A hole of nonsense, weighted out.
        for y in 6..10 {
            for x in 6..10 {
                for c in 0..3 {
                    data[(y * w + x) * 3 + c] = 100.0;
                }
                weight[y * w + x] = 0.0;
            }
        }
        let out = blur(&data, w, h, 2.0, Some(&weight));
        assert!(out.iter().all(|v| (v - 0.5).abs() < 1e-4));
    }
}
