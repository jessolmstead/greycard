//! The dominant straight line of a picture, for a one-click level:
//! the angle of the longest straight edges within 45° of horizontal or
//! vertical, which a horizon or a wall gives and a forest does not.
//!
//! The edges are found as line segments, the way LSD (von Gioi,
//! Jakubowicz, Morel and Randall, 2012) grows them: pixels whose
//! gradient points the same way, joined while they stay within a few
//! degrees of their region's direction. Each region's angle is its
//! principal axis, which for an edge hundreds of pixels long is good
//! to a few hundredths of a degree, aliased stairs and all. Texture
//! grows only short regions. The segments vote their length into a
//! histogram of angle modulo 90° — a horizontal and a vertical of the
//! same picture agree — and the answer is the length-weighted mean of
//! the segments at its peak.

use crate::guided::box_mean;

/// The long edge the picture is brought down to before it is read.
pub const LONG_EDGE: usize = 1024;
/// Histogram bins a degree.
const BINS_PER_DEGREE: usize = 10;
const BINS: usize = 90 * BINS_PER_DEGREE;
/// The smoothing of the histogram before its peak is taken, bins.
const SMOOTH: f32 = 3.0;
/// The half width of the peak, degrees: the segments within it are
/// the line's.
const PEAK: f32 = 0.5;
/// How far a pixel's gradient may turn from its region's and still
/// join it, degrees.
const TOLERANCE: f32 = 10.0;
/// The least gradient an edge pixel has, a pixel, on the gray scaled
/// so its bright end is one.
const FLOOR: f32 = 0.01;
/// The shortest segment that votes, a fraction of the long edge, and
/// the widest, pixels: a crest an edge's own is a pixel or two wide,
/// and a region wider has joined edges that are not one line.
const SHORTEST: f32 = 0.03;
const WIDEST: f32 = 3.0;
/// For a line to count as dominant: the length its segments must add
/// to, and the longest of them reach, as fractions of the long edge,
/// and the least share of all the segments' length they must hold. A
/// horizon or a wall has one long edge at least; ridges, rocks and
/// wires in the sky add up to a length only in many short pieces.
const LONG_ENOUGH: f32 = 0.25;
const LONGEST: f32 = 0.2;
const SHARE: f32 = 0.10;

/// The dominant line of a `w` by `h` gray picture: its angle in
/// degrees, counter-clockwise on the screen (y runs down), folded to
/// (-45, 45], and where it lies. A horizontal or a vertical reads 0, a
/// horizon turned 3° counter-clockwise reads 3, and a wall turned 3°
/// the same. `None` when no direction stands out (texture, noise, a
/// blank frame).
///
/// The gray wants a perceptual scale (a cube root of luminance, say),
/// so a shadowed horizon weighs what it looks like.
pub fn horizon(gray: &[f32], w: usize, h: usize) -> Option<Line> {
    assert_eq!(gray.len(), w * h);
    let factor = w.max(h).div_ceil(LONG_EDGE).max(1) as f32;
    let (gray, w, h) = shrink(gray, w, h);
    if w < 8 || h < 8 {
        log::debug!("horizon: picture too small ({w}x{h})");
        return None;
    }
    let mut segments = segments(&gray, w, h);
    segments.sort_by(|a, b| b.length.total_cmp(&a.length));
    for s in segments.iter().take(3) {
        log::debug!(
            "horizon: a segment of {:.0} px at {:.2}° about {:.0?}",
            s.length,
            s.angle,
            s.at
        );
    }
    let long = w.max(h) as f32;
    let total: f32 = segments.iter().map(|s| s.length).sum();
    let mut hist = vec![0.0f64; BINS];
    for s in &segments {
        let bin = ((fold(s.angle) + 45.0) * BINS_PER_DEGREE as f32) as usize;
        hist[bin.min(BINS - 1)] += f64::from(s.length);
    }
    let smooth = smoothed(&hist);
    let (peak, _) = smooth
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    let peak = (peak as f32 + 0.5) / BINS_PER_DEGREE as f32 - 45.0;
    // The line's segments: the length-weighted mean of their angles,
    // about the peak, around the circle.
    let (mut sum, mut length, mut longest) = (0.0f32, 0.0f32, 0.0f32);
    let mut middle = (0.0f32, 0.0f32);
    for s in &segments {
        let d = fold(s.angle - peak);
        if d.abs() <= PEAK {
            sum += d * s.length;
            length += s.length;
            longest = longest.max(s.length);
            middle.0 += s.at.0 * s.length;
            middle.1 += s.at.1 * s.length;
        }
    }
    if length < LONG_ENOUGH * long || longest < LONGEST * long || length < SHARE * total {
        log::debug!(
            "horizon: no dominant line ({length:.0} px at {peak:.1}°, the longest {longest:.0}, \
             of {total:.0} px in {} segments)",
            segments.len()
        );
        return None;
    }
    let t = fold(peak + sum / length);
    log::debug!(
        "horizon: {t:.2}° ({length:.0} px of line, of {total:.0} px in {} segments)",
        segments.len()
    );
    // Where the line is, back in the picture's own pixels: a shrunk
    // pixel's center is the middle of the block it averaged.
    let at = |v: f32| (v / length + 0.5) * factor - 0.5;
    Some(Line {
        angle: t,
        at: (at(middle.0), at(middle.1)),
    })
}

/// The dominant line of a picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    /// Degrees counter-clockwise on the screen (y down), folded to
    /// (-45, 45].
    pub angle: f32,
    /// Where it lies: the middle of its segments, weighted by their
    /// lengths, in the picture's pixels. With a keystone in the edit a
    /// line's angle on the screen depends on where it is.
    pub at: (f32, f32),
}

/// A straight edge: its angle counter-clockwise on the screen modulo
/// 180, and its length, pixels.
#[derive(Debug)]
struct Segment {
    angle: f32,
    length: f32,
    /// Its middle, in pixels of the picture shrunk.
    at: (f32, f32),
}

/// The picture's straight edges, grown from its strongest pixels out.
fn segments(gray: &[f32], w: usize, h: usize) -> Vec<Segment> {
    // The gray scaled so its bright end is one, whatever the exposure.
    let mut sorted: Vec<f32> = gray.iter().copied().step_by(7).collect();
    sorted.sort_by(f32::total_cmp);
    let bright = sorted[sorted.len() * 99 / 100].max(1e-6);
    let scaled: Vec<f32> = gray.iter().map(|v| v / bright).collect();
    // A pixel's blur first, so a hard, aliased step's gradients point
    // across the line rather than across its stairs.
    let g = box_mean(&box_mean(&scaled, w, h, 1), w, h, 1);
    // Scharr's gradient, the most nearly isotropic of the 3x3s.
    let at = |x: usize, y: usize| g[y * w + x];
    // Each pixel's gradient as its strength and its direction, a unit
    // vector.
    let mut unit = vec![(0.0f32, 0.0f32); w * h];
    let mut mag = vec![0.0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = (3.0 * at(x + 1, y - 1) + 10.0 * at(x + 1, y) + 3.0 * at(x + 1, y + 1))
                - (3.0 * at(x - 1, y - 1) + 10.0 * at(x - 1, y) + 3.0 * at(x - 1, y + 1));
            let gy = (3.0 * at(x - 1, y + 1) + 10.0 * at(x, y + 1) + 3.0 * at(x + 1, y + 1))
                - (3.0 * at(x - 1, y - 1) + 10.0 * at(x, y - 1) + 3.0 * at(x + 1, y - 1));
            let (gx, gy) = (gx / 32.0, gy / 32.0);
            let m = (gx * gx + gy * gy).sqrt();
            mag[y * w + x] = m;
            if m > 0.0 {
                unit[y * w + x] = (gx / m, gy / m);
            }
        }
    }
    // The edges thinned to their crests (Canny's non-maximum
    // suppression): a pixel is an edge's only where its gradient peaks
    // across the edge. Two parallel edges a little apart then stay two
    // regions, rather than joining through the slope between them.
    let mut edge = vec![false; w * h];
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let i = y * w + x;
            if mag[i] <= FLOOR {
                continue;
            }
            let (ux, uy) = unit[i];
            let (dx, dy) = (ux.round() as i64, uy.round() as i64);
            let at = |k: i64| mag[((y as i64 + k * dy) * w as i64 + x as i64 + k * dx) as usize];
            edge[i] = mag[i] >= at(1) && mag[i] > at(-1);
        }
    }
    let mut seeds: Vec<usize> = (0..w * h).filter(|&i| edge[i]).collect();
    seeds.sort_by(|&a, &b| mag[b].total_cmp(&mag[a]));
    let mut used = vec![false; w * h];
    let tolerance = TOLERANCE.to_radians().cos();
    let long = w.max(h) as f32;
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for seed in seeds {
        if used[seed] {
            continue;
        }
        used[seed] = true;
        stack.clear();
        stack.push(seed);
        // The region's direction, a sum of unit gradients.
        let (mut cx, mut cy) = unit[seed];
        let mut i = 0;
        while i < stack.len() {
            let p = stack[i];
            i += 1;
            let (x, y) = (p % w, p / w);
            for (dx, dy) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 1 || ny < 1 || nx >= w as i64 - 1 || ny >= h as i64 - 1 {
                    continue;
                }
                let q = ny as usize * w + nx as usize;
                if used[q] || !edge[q] {
                    continue;
                }
                let (qx, qy) = unit[q];
                let norm = (cx * cx + cy * cy).sqrt();
                if (qx * cx + qy * cy) / norm < tolerance {
                    continue;
                }
                used[q] = true;
                cx += qx;
                cy += qy;
                stack.push(q);
            }
        }
        if (stack.len() as f32) < SHORTEST * long {
            continue;
        }
        if let Some(segment) = fit(&stack, &mag, w)
            && segment.length >= SHORTEST * long
        {
            out.push(segment);
        }
    }
    out
}

/// The line through a region's pixels, weighted by their gradients:
/// the principal axis, refit twice on the pixels within a pixel of it,
/// so a region that has run on into a neighboring edge a few pixels
/// off is read along the edge that holds most of it. `None` when what
/// is left is wider than an edge's crest.
fn fit(pixels: &[usize], mag: &[f32], w: usize) -> Option<Segment> {
    let mut keep = vec![true; pixels.len()];
    let mut line = None;
    for round in 0..3 {
        let mut m = [0.0f64; 6];
        for (&p, _) in pixels.iter().zip(&keep).filter(|(_, k)| **k) {
            let (x, y, wt) = ((p % w) as f64, (p / w) as f64, f64::from(mag[p]));
            m[0] += wt;
            m[1] += wt * x;
            m[2] += wt * y;
            m[3] += wt * x * x;
            m[4] += wt * x * y;
            m[5] += wt * y * y;
        }
        if m[0] <= 0.0 {
            return None;
        }
        // The spread along and across the axis, a uniform run's length
        // being the root of twelve times its variance.
        let n = m[0];
        let (mx, my) = (m[1] / n, m[2] / n);
        let (sxx, sxy, syy) = (m[3] / n - mx * mx, m[4] / n - mx * my, m[5] / n - my * my);
        let half = 0.5 * (sxx + syy);
        let root = (0.25 * (sxx - syy) * (sxx - syy) + sxy * sxy).sqrt();
        let length = (12.0 * (half + root)).sqrt() as f32;
        let width = (12.0 * (half - root).max(0.0)).sqrt() as f32;
        // The axis in y-down coordinates; counter-clockwise on the
        // screen is its negative.
        let axis = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        line = Some((axis, mx, my, length, width));
        if round == 2 {
            break;
        }
        let (s, c) = axis.sin_cos();
        for (&p, k) in pixels.iter().zip(keep.iter_mut()) {
            let (x, y) = ((p % w) as f64 - mx, (p / w) as f64 - my);
            *k = (y * c - x * s).abs() <= 1.0;
        }
    }
    let (axis, mx, my, length, width) = line?;
    (width <= WIDEST).then(|| Segment {
        angle: (-axis.to_degrees() as f32).rem_euclid(180.0),
        length,
        at: (mx as f32, my as f32),
    })
}

/// An angle folded to (-45, 45] modulo 90.
fn fold(t: f32) -> f32 {
    let t = t.rem_euclid(90.0);
    if t > 45.0 { t - 90.0 } else { t }
}

/// The histogram blurred by a gaussian, around the circle.
fn smoothed(hist: &[f64]) -> Vec<f64> {
    let r = (3.0 * SMOOTH).ceil() as i32;
    let kernel: Vec<f64> = (-r..=r)
        .map(|d| (-(d * d) as f64 / (2.0 * f64::from(SMOOTH * SMOOTH))).exp())
        .collect();
    let n = hist.len() as i32;
    (0..n)
        .map(|i| {
            (-r..=r)
                .zip(&kernel)
                .map(|(d, k)| k * hist[(i + d).rem_euclid(n) as usize])
                .sum()
        })
        .collect()
}

/// The picture box-averaged by a whole factor to [`LONG_EDGE`] or
/// under; as it is when already that small.
fn shrink(gray: &[f32], w: usize, h: usize) -> (std::borrow::Cow<'_, [f32]>, usize, usize) {
    let f = w.max(h).div_ceil(LONG_EDGE);
    if f <= 1 {
        return (gray.into(), w, h);
    }
    let (sw, sh) = (w / f, h / f);
    let mut out = vec![0.0f32; sw * sh];
    for y in 0..sh {
        for x in 0..sw {
            let mut s = 0.0f32;
            for yy in y * f..(y + 1) * f {
                s += gray[yy * w + x * f..yy * w + (x + 1) * f]
                    .iter()
                    .sum::<f32>();
            }
            out[y * sw + x] = s / (f * f) as f32;
        }
    }
    (out.into(), sw, sh)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A horizon: dark below a line through the center at `degrees`
    /// counter-clockwise on the screen, light above, its edge a pixel
    /// soft, plus a little grain.
    fn edge(degrees: f32, w: usize, h: usize) -> Vec<f32> {
        edge_of(degrees, w, h, 0.7)
    }

    fn edge_of(degrees: f32, w: usize, h: usize, soft: f32) -> Vec<f32> {
        let (s, c) = degrees.to_radians().sin_cos();
        let mut seed = 1u32;
        (0..w * h)
            .map(|i| {
                let (x, y) = (
                    (i % w) as f32 - w as f32 / 2.0,
                    (i / w) as f32 - h as f32 / 2.0,
                );
                // The signed distance from the line along (c, -s), y down.
                let d = x * s + y * c;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let grain = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                0.3 + 0.4 / (1.0 + (-d / soft).exp()) + 0.01 * grain
            })
            .collect()
    }

    fn noise(w: usize, h: usize) -> Vec<f32> {
        let mut seed = 7u32;
        (0..w * h)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32
            })
            .collect()
    }

    #[test]
    fn a_turned_horizon_reads_its_angle() {
        for degrees in [3.0f32, 0.7, -12.0, 40.0, 0.0] {
            let got = horizon(&edge(degrees, 600, 400), 600, 400)
                .expect("a line")
                .angle;
            assert!((got - degrees).abs() < 0.1, "{degrees}: {got}");
        }
    }

    /// A wall: the same line nearer vertical reads its tilt from the
    /// vertical, the angle the level tool takes for it.
    #[test]
    fn a_turned_wall_reads_its_tilt_from_the_vertical() {
        for (degrees, tilt) in [(93.0f32, 3.0f32), (130.0, 40.0), (78.0, -12.0)] {
            let got = horizon(&edge(degrees, 400, 600), 400, 600)
                .expect("a line")
                .angle;
            assert!((got - tilt).abs() < 0.1, "{degrees}: {got}");
        }
    }

    /// The angle is read the same at any size, the large one shrunk.
    #[test]
    fn a_large_picture_is_read_shrunk() {
        let line = horizon(&edge(-5.0, 3000, 2000), 3000, 2000).expect("a line");
        assert!((line.angle + 5.0).abs() < 0.1, "{line:?}");
        // Where it lies, in the picture's own pixels: through the middle.
        let (x, y) = line.at;
        assert!(
            (x - 1500.0).abs() < 30.0 && (y - 1000.0).abs() < 10.0,
            "{line:?}"
        );
    }

    /// A hard, aliased edge reads locally as the angle of its stairs;
    /// the line through them is what comes back.
    #[test]
    fn a_hard_edge_reads_its_line_not_its_stairs() {
        for degrees in [0.53f32, 2.7, 5.9, -8.3, 24.6, 41.8] {
            let got = horizon(&edge_of(degrees, 1024, 683, 0.05), 1024, 683)
                .expect("a line")
                .angle;
            assert!((got - degrees).abs() < 0.1, "{degrees}: {got}");
        }
    }

    /// The gray of a picture with steps along several lines: each a
    /// line at `degrees` through `through` (y down), where it runs
    /// (from x0 to x1), and how far it steps, a pixel soft.
    /// A step along a line: its angle, a point on it, the run of x it
    /// covers, and how far it steps.
    type Step = (f32, (f32, f32), (f32, f32), f32);

    fn steps(w: usize, h: usize, lines: &[Step]) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let mut v = 0.3;
                for &(degrees, (px, py), (x0, x1), step) in lines {
                    if x < x0 || x > x1 {
                        continue;
                    }
                    let (s, c) = degrees.to_radians().sin_cos();
                    let d = (x - px) * s + (y - py) * c;
                    v += step / (1.0 + (-d / 0.7).exp());
                }
                v
            })
            .collect()
    }

    /// Two edges of one direction a few pixels apart, each across the
    /// frame: their angle, not one between them and the gap.
    #[test]
    fn parallel_edges_close_together_read_their_angle() {
        let (w, h) = (900, 600);
        for gap in [4.0f32, 8.0, 20.0] {
            let lines = [
                (1.5, (450.0, 300.0), (0.0, 900.0), 0.2),
                (1.5, (450.0, 300.0 + gap), (0.0, 900.0), 0.2),
            ];
            let got = horizon(&steps(w, h, &lines), w, h).expect("a line").angle;
            assert!((got - 1.5).abs() < 0.1, "{gap}: {got}");
        }
    }

    /// A horizon with a cliff's edge 20 pixels above it, near parallel
    /// and shorter: a strip of land on the right standing over the sea.
    /// The horizon is what is read.
    #[test]
    fn a_horizon_beside_a_cliff_edge_reads_the_horizon() {
        let (w, h) = (900, 600);
        let side = |degrees: f32, (px, py): (f32, f32), x: f32, y: f32| {
            let (s, c) = degrees.to_radians().sin_cos();
            1.0 / (1.0 + (-((x - px) * s + (y - py) * c) / 0.7).exp())
        };
        let gray: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                // Sky over sea at the horizon; the land a darker strip
                // from its edge down to the horizon, where it begins.
                let sea = side(1.5, (450.0, 300.0), x, y);
                let land =
                    side(3.0, (675.0, 280.0), x, y) * (1.0 - sea) * side(90.0, (450.0, 0.0), x, y);
                0.8 - 0.3 * sea - 0.4 * land
            })
            .collect();
        let got = horizon(&gray, w, h).expect("a line").angle;
        assert!((got - 1.5).abs() < 0.1, "{got}");
    }

    /// A horizon whose edge steps by a few pixels part way along, as a
    /// far shore does where it meets the sea: one angle, not the step's.
    #[test]
    fn an_offset_in_an_edge_does_not_tilt_it() {
        let (w, h) = (900, 600);
        let lines = [
            (0.8, (300.0, 300.0), (0.0, 520.0), 0.3),
            (0.8, (300.0, 297.0), (520.0, 900.0), 0.3),
        ];
        let got = horizon(&steps(w, h, &lines), w, h).expect("a line").angle;
        assert!((got - 0.8).abs() < 0.1, "{got}");
    }

    #[test]
    fn noise_has_no_line() {
        assert_eq!(horizon(&noise(600, 400), 600, 400), None);
    }

    #[test]
    fn a_flat_frame_has_no_line() {
        assert_eq!(horizon(&vec![0.5; 600 * 400], 600, 400), None);
    }

    /// Horizontal and vertical edges in equal measure agree modulo 90:
    /// level, never a compromise between them.
    #[test]
    fn a_checkerboard_reads_level_or_nothing() {
        let (w, h) = (600, 400);
        let board: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w / 50, i / w / 50);
                if (x + y) % 2 == 0 { 0.2 } else { 0.8 }
            })
            .collect();
        if let Some(got) = horizon(&board, w, h).map(|l| l.angle) {
            assert!(got.abs() < 0.1, "{got}");
        }
    }
}
