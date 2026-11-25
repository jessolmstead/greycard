//! A synthetic focus stack with a ground truth, for measuring what the
//! merge keeps of fine, low-contrast texture.
//!
//! The scene is a surface whose depth runs across `x` and bows a little
//! down `y`, seen through a stack of frames each focused at its own
//! depth. Three regions run the width of the frame, so each crosses
//! every depth: a hard-edged checker (the strong edges every merge
//! gets right), a faint fine texture on a gently shaded ground (the
//! skin of a fruit, a spot inside a translucent egg), and a flat field
//! (where a merge must average the noise down, not select it). Every
//! frame is defocused by the distance of the surface from its focus,
//! taken through a small magnification that grows down the stack
//! (focus breathing), and given shot and read noise of its own.

use rayon::prelude::*;

use crate::image::WorkingImage;
use crate::register::Transform;

/// A lattice value at `(ix, iy)` in `seed`'s sequence, 0 to 1.
fn hash(ix: i64, iy: i64, seed: u64) -> f64 {
    let mut h = (ix as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93));
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Value noise on a lattice of `cell` pixels, smoothstepped, -0.5 to
/// 0.5.
fn octave(x: f64, y: f64, cell: f64, seed: u64) -> f64 {
    let (gx, gy) = (x / cell, y / cell);
    let (i, j) = (gx.floor(), gy.floor());
    let (fx, fy) = (gx - i, gy - j);
    let ease = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (ease(fx), ease(fy));
    let (i, j) = (i as i64, j as i64);
    let l = |a, b| hash(a, b, seed);
    let a = l(i, j) + sx * (l(i + 1, j) - l(i, j));
    let b = l(i, j + 1) + sx * (l(i + 1, j + 1) - l(i, j + 1));
    a + sy * (b - a) - 0.5
}

/// What a Gaussian of deviation `sigma` leaves of an octave of `cell`.
fn response(cell: f64, sigma: f64) -> f64 {
    (-2.0 * std::f64::consts::PI.powi(2) * sigma * sigma / (cell * cell)).exp()
}

/// The standard normal's cumulative distribution (Abramowitz and
/// Stegun 7.1.26 for the error function).
fn phi(t: f64) -> f64 {
    let z = t / std::f64::consts::SQRT_2;
    let s = z.signum();
    let z = z.abs();
    let u = 1.0 / (1.0 + 0.327_591_1 * z);
    let y = 1.0
        - (((((1.061_405_429 * u - 1.453_152_027) * u) + 1.421_413_741) * u - 0.284_496_736) * u
            + 0.254_829_592)
            * u
            * (-z * z).exp();
    0.5 * (1.0 + s * y)
}

/// A square wave of period `2 * half`, -1 then 1, blurred by a Gaussian
/// of `sigma`: exactly, as a sum over the intervals in reach.
fn square(x: f64, half: f64, sigma: f64) -> f64 {
    let j0 = (x / half).floor() as i64;
    let sign = |j: i64| if j.rem_euclid(2) == 0 { -1.0 } else { 1.0 };
    if sigma < 1e-3 {
        return sign(j0);
    }
    let reach = (6.0 * sigma / half).ceil() as i64 + 1;
    let mut v = 0.0;
    for j in j0 - reach..=j0 + reach {
        let a = j as f64 * half;
        v += sign(j) * (phi((x - a) / sigma) - phi((x - a - half) / sigma));
    }
    v
}

/// The regions, as rows: strong edges, faint texture, flat.
const STRONG: (usize, usize) = (0, 120);
const FAINT: (usize, usize) = (120, 240);
const FLAT: (usize, usize) = (240, 360);

/// The scene at a point of its own coordinates, defocused by `sigma`.
fn scene(u: f64, v: f64, sigma: f64) -> f64 {
    if v < STRONG.1 as f64 {
        // A checker of 18 px squares, a fourfold contrast.
        0.375 + 0.225 * square(u, 18.0, sigma) * square(v, 18.0, sigma)
    } else if v < FAINT.1 as f64 {
        faint(u, v, sigma)
    } else {
        0.3
    }
}

/// A gently shaded ground with a faint fine texture on it, a percent
/// or two of its level.
fn faint(u: f64, v: f64, sigma: f64) -> f64 {
    let ground = 0.35 + 0.06 * response(90.0, sigma) * octave(u, v, 90.0, 1);
    let mut fine = 0.0;
    for (cell, amplitude) in [(6.0, 0.016), (4.0, 0.016), (3.0, 0.014)] {
        fine += amplitude * response(cell, sigma) * octave(u, v, cell, 2 + cell as u64);
    }
    ground * (1.0 + fine * 2.0)
}

/// A strong texture at five scales, a third of its level and more.
fn strong(u: f64, v: f64, sigma: f64, seed: u64) -> f64 {
    let mut value = 0.6;
    let mut amplitude = 0.35;
    for cell in [40.0, 20.0, 10.0, 5.0, 3.0] {
        value += amplitude * response(cell, sigma) * octave(u, v, cell, seed + cell as u64);
        amplitude *= 0.85;
    }
    value
}

/// How a stack of the scene is shot.
#[derive(Debug, Clone, Copy)]
pub(super) struct Shot {
    pub width: usize,
    pub height: usize,
    pub frames: usize,
    /// Defocus, in pixels of deviation, per unit of depth away from
    /// focus. The depth runs about 0.1 to 0.9 across the frame and the
    /// frames' focus 0 to 1, so the step between frames is this over
    /// `frames - 1`.
    pub blur: f64,
    /// Shot noise's variance per unit of signal; 0 for none.
    pub shot: f64,
    /// Read noise's deviation.
    pub read: f64,
    /// The magnification each step of focus adds.
    pub breathing: f64,
    /// A frame shot a stop under the rest: half the light, and the
    /// noise of half the light.
    pub under: Option<usize>,
}

impl Default for Shot {
    fn default() -> Self {
        Self {
            width: 480,
            height: 360,
            frames: 11,
            blur: 16.0,
            // About 20 000 electrons at 0.4 of full scale: a base ISO.
            shot: 2e-5,
            read: 5e-4,
            breathing: 0.0015,
            under: None,
        }
    }
}

impl Shot {
    /// The surface's depth at a point of the scene.
    pub fn depth(&self, u: f64, v: f64) -> f64 {
        let (w, h) = (self.width as f64, self.height as f64);
        0.1 + 0.8 * u / w + 0.06 * (std::f64::consts::PI * v / h).sin()
    }

    /// Frame `k`'s focus depth.
    pub fn focus(&self, k: usize) -> f64 {
        k as f64 / (self.frames - 1).max(1) as f64
    }

    fn reference(&self) -> usize {
        self.frames / 2
    }

    /// How frame `k` sees the scene: its pixel `(x, y)` is the scene at
    /// `apply(x, y)`.
    pub(super) fn view(&self, k: usize) -> Transform {
        let step = k as f64 - self.reference() as f64;
        Transform::similarity(
            1.0 + self.breathing * step,
            0.0,
            [
                (self.width - 1) as f64 / 2.0,
                (self.height - 1) as f64 / 2.0,
            ],
            [0.0, 0.0],
        )
    }

    /// One sample of noise for frame `k` at a pixel with signal `v`.
    fn noise(&self, k: usize, x: usize, y: usize, v: f64) -> f64 {
        if self.shot == 0.0 && self.read == 0.0 {
            return 0.0;
        }
        let seed = 1000 + k as u64;
        let a = hash(x as i64, y as i64, seed).max(1e-12);
        let b = hash(x as i64, y as i64, seed + 7919);
        let g = (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos();
        g * (self.shot * v.max(0.0) + self.read * self.read).sqrt()
    }

    fn image(&self, f: impl Fn(usize, usize) -> f32 + Sync) -> WorkingImage {
        let w = self.width;
        let data: Vec<f32> = (0..w * self.height)
            .into_par_iter()
            .flat_map_iter(|i| {
                let v = f(i % w, i / w);
                [v, v, v]
            })
            .collect();
        WorkingImage::from_data(w, self.height, data).expect("a frame")
    }

    /// The stack, in focus order.
    pub fn frames(&self) -> Vec<WorkingImage> {
        (0..self.frames)
            .map(|k| {
                let view = self.view(k);
                let focus = self.focus(k);
                self.image(|x, y| {
                    let (u, v) = view.apply(x as f64, y as f64);
                    let sigma = self.blur * (self.depth(u, v) - focus).abs();
                    let s = scene(u, v, sigma) * if self.under == Some(k) { 0.5 } else { 1.0 };
                    (s + self.noise(k, x, y, s)) as f32
                })
            })
            .collect()
    }

    /// The scene sharp everywhere and without noise, in the reference
    /// frame's geometry.
    pub fn truth(&self) -> WorkingImage {
        self.image(|x, y| scene(x as f64, y as f64, 0.0) as f32)
    }

    /// The best one frame could do at each pixel: the scene at the
    /// defocus of the frame focused nearest, with one frame's noise.
    pub fn nearest(&self) -> WorkingImage {
        self.image(|x, y| {
            let (u, v) = (x as f64, y as f64);
            let d = self.depth(u, v);
            let k = (0..self.frames)
                .min_by(|&a, &b| {
                    (self.focus(a) - d)
                        .abs()
                        .total_cmp(&(self.focus(b) - d).abs())
                })
                .unwrap_or(0);
            let s = scene(u, v, self.blur * (d - self.focus(k)).abs());
            (s + self.noise(k, x, y, s)) as f32
        })
    }
}

/// A Gaussian blur of a plane, separable, the taps clamped at the edge.
fn gauss(src: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    let r = (3.0 * sigma).ceil() as isize;
    let taps: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32)
        .collect();
    let total: f32 = taps.iter().sum();
    let line = |get: &dyn Fn(isize) -> f32| -> f32 {
        taps.iter()
            .enumerate()
            .map(|(t, k)| k * get(t as isize - r))
            .sum::<f32>()
            / total
    };
    let mut rows = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            rows[y * w + x] =
                line(&|d| src[y * w + (x as isize + d).clamp(0, w as isize - 1) as usize]);
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] =
                line(&|d| rows[(y as isize + d).clamp(0, h as isize - 1) as usize * w + x]);
        }
    }
    out
}

/// The fine band of an image's green: what a blur of 0.7 px keeps and
/// one of 2.5 px does not, which is where the faint texture lives.
fn band(image: &WorkingImage) -> Vec<f32> {
    let (w, h) = (image.width, image.height);
    let g: Vec<f32> = image.data.chunks(3).map(|p| p[1]).collect();
    let a = gauss(&g, w, h, 0.7);
    let b = gauss(&g, w, h, 2.5);
    a.iter().zip(&b).map(|(a, b)| a - b).collect()
}

/// What a merge kept of the truth's fine band over some rows: the
/// share of its amplitude kept (the projection of the merge's band onto
/// the truth's, over the truth's own energy: noise, being uncorrelated
/// with it, does not count), and the error left (the band's difference
/// from the truth's, over the truth's).
#[derive(Debug, Clone, Copy)]
struct Detail {
    pub kept: f64,
    pub error: f64,
}

/// The margin kept off the frame's sides and the regions' edges.
const MARGIN: usize = 24;

fn detail(merged: &[f32], truth: &[f32], w: usize, rows: (usize, usize)) -> Detail {
    let (mut mt, mut tt, mut ee) = (0.0f64, 0.0f64, 0.0f64);
    for y in rows.0 + MARGIN / 2..rows.1 - MARGIN / 2 {
        for x in MARGIN..w - MARGIN {
            let (m, t) = (merged[y * w + x] as f64, truth[y * w + x] as f64);
            mt += m * t;
            tt += t * t;
            ee += (m - t) * (m - t);
        }
    }
    Detail {
        kept: mt / tt,
        error: (ee / tt).sqrt(),
    }
}

/// The deviation of a band over some rows: on the flat field, its
/// noise.
fn deviation(band: &[f32], w: usize, rows: (usize, usize)) -> f64 {
    let (mut s, mut n) = (0.0f64, 0usize);
    for y in rows.0 + MARGIN / 2..rows.1 - MARGIN / 2 {
        for x in MARGIN..w - MARGIN {
            s += (band[y * w + x] as f64).powi(2);
            n += 1;
        }
    }
    (s / n as f64).sqrt()
}

/// A depth edge: a near bar, its texture strong, over a far ground,
/// shot in `frames` steps of focus from the bar (frame 0) to the ground
/// (the last), each frame laying the bar's defocused disc over the
/// ground with the coverage a Gaussian gives it, as the halo fixture in
/// `stack`'s tests does for two frames.
struct DepthEdge {
    pub shot: Shot,
    /// The ground's texture: the faint one, or a strong one.
    pub faint_ground: bool,
}

impl DepthEdge {
    /// The bar's columns.
    pub fn bar(&self) -> (usize, usize) {
        (self.shot.width / 3, 2 * self.shot.width / 3)
    }

    fn ground(&self, u: f64, v: f64, sigma: f64) -> f64 {
        if self.faint_ground {
            faint(u, v, sigma)
        } else {
            0.4 * strong(u, v, sigma, 40)
        }
    }

    fn near(u: f64, v: f64, sigma: f64) -> f64 {
        1.1 * strong(u + 3000.0, v, sigma, 80)
    }

    /// The frames, focused from the bar to the ground; no breathing.
    pub fn frames(&self) -> Vec<WorkingImage> {
        let (x0, x1) = self.bar();
        let shot = self.shot;
        (0..shot.frames)
            .map(|k| {
                let f = shot.focus(k);
                let (near_sigma, far_sigma) = (shot.blur * f, shot.blur * (1.0 - f));
                shot.image(|x, y| {
                    let (u, v) = (x as f64, y as f64);
                    let a = if near_sigma < 1e-3 {
                        if x >= x0 && x < x1 { 1.0 } else { 0.0 }
                    } else {
                        (phi((x1 as f64 - 0.5 - u) / near_sigma)
                            - phi((x0 as f64 - 0.5 - u) / near_sigma))
                        .clamp(0.0, 1.0)
                    };
                    let s =
                        a * Self::near(u, v, near_sigma) + (1.0 - a) * self.ground(u, v, far_sigma);
                    (s + shot.noise(k, x, y, s)) as f32
                })
            })
            .collect()
    }

    pub fn truth(&self) -> WorkingImage {
        let (x0, x1) = self.bar();
        self.shot.image(|x, y| {
            let (u, v) = (x as f64, y as f64);
            (if x >= x0 && x < x1 {
                Self::near(u, v, 0.0)
            } else {
                self.ground(u, v, 0.0)
            }) as f32
        })
    }
}

/// The halo beside the bar's right edge, as a fraction of the step
/// across it: the merge's departure from the truth in the thirty pixels
/// beyond the edge, column-averaged, over its departure further out,
/// as `stack`'s own halo test measures it.
fn halo(merged: &WorkingImage, truth: &WorkingImage, edge: usize) -> f32 {
    let (width, height) = (merged.width, merged.height);
    let profile: Vec<f32> = (edge..width)
        .map(|x| {
            let mut sum = 0.0f64;
            for y in 8..height - 8 {
                sum += (merged.pixel(x, y)[1] - truth.pixel(x, y)[1]) as f64;
            }
            (sum / (height - 16) as f64) as f32
        })
        .collect();
    let far = &profile[40..70];
    let baseline = far.iter().sum::<f32>() / far.len() as f32;
    let worst = profile[6..36]
        .iter()
        .map(|v| v - baseline)
        .fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a });
    let mean = |x0: usize, x1: usize| {
        let mut sum = 0.0f64;
        for y in 8..height - 8 {
            for x in x0..x1 {
                sum += truth.pixel(x, y)[1] as f64;
            }
        }
        sum / ((height - 16) * (x1 - x0)) as f64
    };
    let step = mean(edge - 40, edge - 10) - mean(edge + 40, edge + 70);
    (worst as f64 / step) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stack::{self, Options};

    /// The merge as it was before the stack's prior, kept in
    /// `in_memory`: the baseline.
    fn old(frames: &[WorkingImage]) -> WorkingImage {
        let samples: Vec<&[f32]> = frames.iter().map(|f| f.data.as_slice()).collect();
        let (w, h) = (frames[0].width, frames[0].height);
        let (data, _) =
            stack::in_memory::merge(&samples, w, h, stack::WORKING_LUMA, &Options::default())
                .expect("the old merge");
        WorkingImage::from_data(w, h, data).expect("a frame")
    }

    fn new(frames: &[WorkingImage]) -> WorkingImage {
        stack::stack(frames, &Options::default())
            .expect("a stack")
            .0
    }

    /// What a merge kept in each region, and the flat field's noise as
    /// a share of one frame's.
    struct Measured {
        strong: Detail,
        faint: Detail,
        flat: f64,
    }

    fn measured(merged: &WorkingImage, truth: &[f32], single: f64) -> Measured {
        let w = merged.width;
        let b = band(merged);
        Measured {
            strong: detail(&b, truth, w, STRONG),
            faint: detail(&b, truth, w, FAINT),
            flat: deviation(&b, w, FLAT) / single,
        }
    }

    fn line(name: &str, m: &Measured) {
        println!(
            "{name:<10} strong kept {:.3} error {:.3} | faint kept {:.3} error {:.3} | flat noise {:.3} of a frame's",
            m.strong.kept, m.strong.error, m.faint.kept, m.faint.error, m.flat
        );
    }

    /// The old merge and the new on one shot, with the nearest frame's
    /// for a ceiling.
    fn compare(shot: &Shot) -> (Measured, Measured) {
        let frames = shot.frames();
        let truth = band(&shot.truth());
        let single = deviation(&band(&frames[0]), shot.width, FLAT).max(1e-12);
        println!("{shot:?}");
        line("nearest", &measured(&shot.nearest(), &truth, single));
        let before = measured(&old(&frames), &truth, single);
        line("before", &before);
        let after = measured(&new(&frames), &truth, single);
        line("after", &after);
        (before, after)
    }

    /// What the prior must not cost: the strong edges no softer, and
    /// the flat field's noise no less averaged.
    fn holds(before: &Measured, after: &Measured) {
        assert!(
            after.strong.kept >= before.strong.kept - 0.005,
            "the strong edges went from {:.3} to {:.3}",
            before.strong.kept,
            after.strong.kept
        );
        assert!(
            after.flat <= before.flat * 1.05,
            "the flat field's noise went from {:.3} to {:.3} of a frame's",
            before.flat,
            after.flat
        );
    }

    /// The tester's complaint, on the bed: eleven frames at base ISO,
    /// a faint fine texture beside strong edges. The old merge keeps a
    /// quarter of the texture (0.244) where the nearest frame keeps
    /// 0.77; the prior brings it to 0.57.
    #[test]
    fn a_faint_texture_comes_through_the_stack() {
        let (before, after) = compare(&Shot::default());
        assert!(
            before.faint.kept < 0.35,
            "the fixture has no loss to take out: {:.3}",
            before.faint.kept
        );
        assert!(
            after.faint.kept > 0.5 && after.faint.error < before.faint.error * 0.85,
            "the faint texture kept {:.3} (error {:.3}) against {:.3} ({:.3}) before",
            after.faint.kept,
            after.faint.error,
            before.faint.kept,
            before.faint.error
        );
        holds(&before, &after);
    }

    /// Thirty-one frames, a step of 1.3 px of defocus apart: the more
    /// frames share a region's score, the further the old merge spread
    /// its weights (0.12 of the texture kept); the prior keeps 0.46.
    #[test]
    fn a_deep_stack_keeps_its_faint_texture() {
        let (before, after) = compare(&Shot {
            frames: 31,
            blur: 40.0,
            ..Shot::default()
        });
        assert!(before.faint.kept < 0.2, "{:.3}", before.faint.kept);
        assert!(
            after.faint.kept > 0.4 && after.faint.error < before.faint.error * 0.9,
            "the faint texture kept {:.3} (error {:.3}) against {:.3} ({:.3}) before",
            after.faint.kept,
            after.faint.error,
            before.faint.kept,
            before.faint.error
        );
        holds(&before, &after);
    }

    /// Four times the noise's variance and four times its read noise:
    /// the evidence is thinner and the prior, held to the null, trusts
    /// little of it (0.16 of the texture kept before, 0.20 after), but
    /// what it trusts it gets right, and the error, noise and all,
    /// still falls.
    #[test]
    fn a_noisier_stack_still_gains() {
        let (before, after) = compare(&Shot {
            shot: 8e-5,
            read: 2e-3,
            ..Shot::default()
        });
        assert!(before.faint.kept < 0.25, "{:.3}", before.faint.kept);
        assert!(
            after.faint.kept > before.faint.kept + 0.025 && after.faint.error < before.faint.error,
            "the faint texture kept {:.3} (error {:.3}) against {:.3} ({:.3}) before",
            after.faint.kept,
            after.faint.error,
            before.faint.kept,
            before.faint.error
        );
        holds(&before, &after);
    }

    /// The halo the pyramid keeps down must stay down with the prior,
    /// over a strong ground and over a faint one: on a faint ground the
    /// bar's own blocks shout loudest, and a prior read wrongly would
    /// hand the ground beside it to a frame carrying the bar's disc.
    /// Nine frames, enough for the prior to speak.
    #[test]
    fn a_depth_edge_leaves_no_more_halo() {
        for faint_ground in [false, true] {
            let edge = DepthEdge {
                shot: Shot {
                    width: 400,
                    height: 300,
                    frames: 9,
                    blur: 8.0,
                    breathing: 0.0,
                    ..Shot::default()
                },
                faint_ground,
            };
            let frames = edge.frames();
            let truth = edge.truth();
            let x1 = edge.bar().1;
            let before = halo(&old(&frames), &truth, x1);
            let after = halo(&new(&frames), &truth, x1);
            // The prior does speak here (the frames are not moved, so
            // the placing pass's blocks are each frame's own).
            let said = gather(&frames).trusted();
            println!(
                "a depth edge over a {} ground: halo {:+.2}% of the step before, {:+.2}% after; \
                 the prior has a say in {:.0}% of it",
                if faint_ground { "faint" } else { "strong" },
                before * 100.0,
                after * 100.0,
                said * 100.0
            );
            assert!(said > 0.2, "the prior has no say: {said}");
            assert!(
                after.abs() < 0.05 && after.abs() <= before.abs() + 0.005,
                "halo {:.2}% against {:.2}% before",
                after * 100.0,
                before * 100.0
            );
        }
    }

    /// How the full-resolution weights spread: the nearest-focused
    /// frame's share and whether it wins. A measurement:
    /// `cargo test -p greycard-core bed_spread -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn bed_spread() {
        for shot in [
            Shot::default(),
            Shot {
                shot: 0.0,
                read: 0.0,
                ..Shot::default()
            },
        ] {
            println!("{shot:?}");
            let frames = shot.frames();
            let w = shot.width;
            let options = Options::default();
            let maps: Vec<Vec<f32>> = frames
                .iter()
                .map(|f| {
                    let lum = stack::luminance_of(&f.data, stack::WORKING_LUMA);
                    crate::sharpness::map(
                        &lum,
                        w,
                        shot.height,
                        options.register.floor_stops,
                        options.sharpness_radius,
                    )
                })
                .collect();
            let floors: Vec<f32> = maps
                .iter()
                .map(|m| m.iter().sum::<f32>() / m.len() as f32 * options.weight_floor)
                .collect();
            for (name, rows) in [("strong", STRONG), ("faint", FAINT), ("flat", FLAT)] {
                let (mut share, mut wins, mut over, mut n) = (0.0f64, 0usize, 0.0f64, 0usize);
                for y in rows.0 + 12..rows.1 - 12 {
                    for x in 24..w - 24 {
                        let d = shot.depth(x as f64, y as f64);
                        let best = (d * (shot.frames - 1) as f64).round() as usize;
                        let ws: Vec<f64> = (0..frames.len())
                            .map(|k| (maps[k][y * w + x] + floors[k]) as f64)
                            .collect();
                        share += ws[best] / ws.iter().sum::<f64>();
                        let arg = (0..ws.len())
                            .max_by(|&a, &b| ws[a].total_cmp(&ws[b]))
                            .unwrap_or(0);
                        wins += usize::from(arg == best);
                        let mut sorted = ws.clone();
                        sorted.sort_by(f64::total_cmp);
                        over += ws[best] / sorted[sorted.len() / 2];
                        n += 1;
                    }
                }
                println!(
                    "  {name}: the nearest frame's share {:.3}, wins {:.1}%, over the median frame {:.2}",
                    share / n as f64,
                    100.0 * wins as f64 / n as f64,
                    over / n as f64
                );
            }
        }
    }

    /// The faint region's block energies, frame by frame, at the full
    /// resolution and at the levels the prior reads. A measurement:
    /// `cargo test -p greycard-core bed_curves -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn bed_curves() {
        let shot = Shot::default();
        let frames = shot.frames();
        let (w, h) = (shot.width, shot.height);
        let options = Options::default();
        let logs: Vec<Vec<f32>> = frames
            .iter()
            .map(|f| {
                let lum = stack::luminance_of(&f.data, stack::WORKING_LUMA);
                crate::register::log_plane(&lum, options.register.floor_stops)
            })
            .collect();
        use super::super::prior;
        // Per scale, per frame, the block means.
        let full: Vec<Vec<f32>> = logs
            .iter()
            .map(|log| {
                let energy = crate::sharpness::laplacian_energy(log, w, h);
                prior::Blocks::of(&energy, (w, h), 0).as_placed().to_vec()
            })
            .collect();
        let (mut half, mut quarter) = (Vec::new(), Vec::new());
        for log in &logs {
            let finest = crate::sharpness::laplacian_energy(log, w, h);
            let levels = prior::blocks(log, &finest, w, h);
            half.push(levels[0].as_placed().to_vec());
            quarter.push(levels[1].as_placed().to_vec());
        }
        let cols = w.div_ceil(prior::SIDE);
        let curves = [
            ("full resolution", full),
            ("half size", half),
            ("quarter size", quarter),
        ];
        for (name, curve) in curves {
            println!("{name}");
            for bx in [8usize, 20, 30, 45] {
                let by = 21;
                let b = by * cols + bx;
                let values: Vec<f64> = curve.iter().map(|f| f[b] as f64).collect();
                let least = values.iter().copied().fold(f64::MAX, f64::min);
                let depth =
                    shot.depth((bx * 8 + 4) as f64, (by * 8 + 4) as f64) * (shot.frames - 1) as f64;
                let said: Vec<String> =
                    values.iter().map(|v| format!("{:.2}", v / least)).collect();
                println!("  focused at frame {depth:.1}: {}", said.join(" "));
            }
        }
    }

    /// How far the chain places each frame from where the bed put it,
    /// at the corners. A measurement.
    #[test]
    #[ignore = "a measurement"]
    fn bed_registration() {
        for shot in [
            Shot::default(),
            Shot {
                frames: 31,
                blur: 40.0,
                ..Shot::default()
            },
        ] {
            let (_, report) = stack::stack(&shot.frames(), &Options::default()).expect("a stack");
            let errors: Vec<String> = report
                .frames
                .iter()
                .map(|f| {
                    let truth = shot.view(f.index).inverse().expect("a similarity");
                    format!(
                        "{:.2}",
                        f.transform.corner_distance(&truth, shot.width, shot.height)
                    )
                })
                .collect();
            println!(
                "{} frames: px off at the corners {}",
                shot.frames,
                errors.join(" ")
            );
        }
    }

    /// The faint region at 100%, its departure from the truth's own
    /// shading stretched twelve times so the texture shows: the truth,
    /// the nearest frame, and the merge before and after, as PNGs into
    /// `GREYCARD_BED_OUT`.
    #[test]
    #[ignore = "writes pictures"]
    fn bed_crops() {
        let Some(out) = std::env::var_os("GREYCARD_BED_OUT") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        let shot = Shot::default();
        let frames = shot.frames();
        let (x0, y0, w, h) = (120usize, FAINT.0 + 10, 240usize, 100usize);
        let pictures = [
            ("truth", shot.truth()),
            ("nearest-frame", shot.nearest()),
            ("before", old(&frames)),
            ("after", new(&frames)),
        ];
        // The truth's own shading, blurred by 6 px, is taken off every
        // picture alike, so what shows is the fine texture and noise.
        let ground = {
            let t = &pictures[0].1;
            let g: Vec<f32> = t.data.chunks(3).map(|p| p[1]).collect();
            gauss(&g, t.width, t.height, 6.0)
        };
        let mut strip = image::GrayImage::new((w * pictures.len() + 8 * 3) as u32, h as u32);
        for (i, (name, picture)) in pictures.iter().enumerate() {
            let mut crop = image::GrayImage::new(w as u32, h as u32);
            for y in 0..h {
                for x in 0..w {
                    let v = picture.pixel(x0 + x, y0 + y)[1] as f64;
                    let m = ground[(y0 + y) * picture.width + x0 + x] as f64;
                    let shown = ((0.5 + (v - m) / m * 12.0).clamp(0.0, 1.0) * 255.0).round();
                    crop.put_pixel(x as u32, y as u32, image::Luma([shown as u8]));
                    strip.put_pixel(
                        (i * (w + 8) + x) as u32,
                        y as u32,
                        image::Luma([shown as u8]),
                    );
                }
            }
            crop.save(out.join(format!("synthetic-faint-{name}.png")))
                .expect("a crop written");
        }
        strip
            .save(out.join("synthetic-faint-truth-nearest-before-after.png"))
            .expect("the strip written");
    }

    /// Each unmoved frame's blocks, as both passes take them.
    fn blocks_of(frames: &[WorkingImage]) -> Vec<Vec<super::super::prior::Blocks>> {
        frames
            .iter()
            .map(|f| {
                let (w, h) = (f.width, f.height);
                let lum = stack::luminance_of(&f.data, stack::WORKING_LUMA);
                let (_, blocks) = stack::measure(&lum, w, h, &Options::default(), true);
                blocks.expect("gathered")
            })
            .collect()
    }

    /// The prior the placing pass gathers from unmoved frames.
    fn gather(frames: &[WorkingImage]) -> super::super::prior::Prior {
        gathered(&blocks_of(frames), frames[0].width, frames[0].height)
    }

    fn gathered(
        blocks: &[Vec<super::super::prior::Blocks>],
        w: usize,
        h: usize,
    ) -> super::super::prior::Prior {
        let mut gathered = super::super::prior::Gathered::new(w, h);
        for b in blocks {
            gathered.add(b, &Transform::IDENTITY);
        }
        gathered.finish()
    }

    /// A null: `n` frames of `size` square, frame `k` a flat field at
    /// `level(k)` (or the faint texture, sharp, scaled by it, when
    /// `textured`) under the noise of `shot` and `read`.
    struct Null {
        n: usize,
        shot: f64,
        read: f64,
        textured: bool,
        level: fn(usize) -> f64,
    }

    impl Null {
        fn frames(&self, size: usize, seed: usize) -> Vec<WorkingImage> {
            let shot = Shot {
                width: size,
                height: size,
                frames: self.n,
                shot: self.shot,
                read: self.read,
                ..Shot::default()
            };
            (0..self.n)
                .map(|k| {
                    let level = (self.level)(k);
                    shot.image(|x, y| {
                        let v = if self.textured {
                            level / 0.35 * faint(x as f64, y as f64, 0.0)
                        } else {
                            level
                        };
                        (v + shot.noise(k + 1000 * seed, x, y, v)) as f32
                    })
                })
                .collect()
        }
    }

    /// Per level, the share of blocks given any trust, and the largest
    /// ratio over its threshold.
    fn null_trust(prior: &super::super::prior::Prior, n: usize) -> Vec<(f64, f32)> {
        prior
            .ratios()
            .iter()
            .enumerate()
            .map(|(l, level)| {
                let trusted = level.iter().filter(|(_, t)| *t).count() as f64;
                let threshold = super::super::prior::ratio_from(l + 1, n);
                let worst = level
                    .iter()
                    .map(|(r, _)| r / threshold)
                    .fold(0.0f32, f32::max);
                (trusted / level.len().max(1) as f64, worst)
            })
            .collect()
    }

    const BASE: (f64, f64) = (2e-5, 5e-4);
    const HIGH_ISO: (f64, f64) = (3.2e-4, 8e-3);

    /// The cases the null is held to: names, counts, and the noise.
    fn nulls() -> Vec<(&'static str, Null)> {
        let null = |n, (shot, read): (f64, f64), level: fn(usize) -> f64| Null {
            n,
            shot,
            read,
            textured: false,
            level,
        };
        vec![
            ("flat, base ISO", null(8, BASE, |_| 0.3)),
            ("flat, base ISO", null(31, BASE, |_| 0.3)),
            (
                "one a stop under",
                null(11, BASE, |k| if k == 1 { 0.15 } else { 0.3 }),
            ),
            (
                "one a stop over",
                null(11, BASE, |k| if k == 1 { 0.6 } else { 0.3 }),
            ),
            ("shadow, read noise", null(11, BASE, |_| 0.003)),
            (
                "shadow, one a third under",
                null(11, BASE, |k| {
                    0.003 * if k == 1 { 2f64.powf(-1.0 / 3.0) } else { 1.0 }
                }),
            ),
            (
                "shadow, one a stop under",
                null(11, BASE, |k| if k == 1 { 0.0015 } else { 0.003 }),
            ),
            (
                "shadow, one a stop under",
                null(31, BASE, |k| if k == 1 { 0.0015 } else { 0.003 }),
            ),
            (
                "dim (SNR 33), one a stop under",
                null(11, BASE, |k| if k == 1 { 0.015 } else { 0.03 }),
            ),
            (
                "dim (SNR 17), one a stop under",
                null(11, BASE, |k| if k == 1 { 0.006 } else { 0.012 }),
            ),
            (
                "high ISO shadow, one a stop under",
                null(11, HIGH_ISO, |k| if k == 1 { 0.0075 } else { 0.015 }),
            ),
            (
                "textured alike, one a third over",
                Null {
                    textured: true,
                    ..null(11, BASE, |k| {
                        0.3 * if k == 1 { 2f64.powf(1.0 / 3.0) } else { 1.0 }
                    })
                },
            ),
            (
                "textured alike, one a stop over",
                Null {
                    textured: true,
                    ..null(11, BASE, |k| if k == 1 { 0.6 } else { 0.3 })
                },
            ),
            (
                "textured alike, one two stops over",
                Null {
                    textured: true,
                    ..null(11, BASE, |k| if k == 1 { 1.2 } else { 0.3 })
                },
            ),
        ]
    }

    /// The null the trust is set from: noise alone believes no block,
    /// whatever the count, with a frame a stop under or over the rest,
    /// and in a deep shadow where read noise rules and one frame is a
    /// third of a stop or a stop under (where every block falls under
    /// the prior's exclusion of noisy evidence), or dim enough (a
    /// signal to noise of 33) to be measured. A small sample (144
    /// blocks a case, so a single block trusted fails it); `bed_null`
    /// is the large one.
    ///
    /// And where every frame holds the same texture and one is two
    /// stops brighter: there the brighter frame's lower noise does lift
    /// its ratio past the threshold, but its share is read from its raw
    /// energy, which the texture fills alike in every frame, so the
    /// frames' multipliers stay within a few percent of each other and
    /// nothing is chosen.
    #[test]
    fn noise_alone_is_not_believed() {
        for (name, null) in nulls() {
            let frames = null.frames(192, 0);
            let blocks = blocks_of(&frames);
            let prior = gathered(&blocks, 192, 192);
            let said = null_trust(&prior, null.n);
            let spread = prior.spread(&blocks);
            println!(
                "{name}, {} frames: {said:.4?}, multipliers within {spread:.3}x",
                null.n
            );
            if null.textured {
                // Up to a stop over the rest, nothing is chosen; at two
                // stops the cleaner frame is weighed up by about its
                // inverse variance (four times), and no further.
                let most = if name.contains("two stops") { 4.5 } else { 1.1 };
                assert!(spread < most, "{name}: multipliers {spread}x apart");
            } else {
                assert!(
                    said.iter().all(|&(share, _)| share < 0.001),
                    "{name}, {} frames: {said:?}",
                    null.n
                );
            }
        }
    }

    /// The null at scale, what the thresholds are set from: at least
    /// 100,000 blocks a case, as many as a 24 MP picture has. A
    /// measurement, in a release build (minutes in a debug one):
    /// `cargo test --release -p greycard-core bed_null -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn bed_null() {
        let mut cases: Vec<(&str, Null)> = [8usize, 11, 31, 100, 250]
            .iter()
            .map(|&n| {
                (
                    "flat, base ISO",
                    Null {
                        n,
                        shot: BASE.0,
                        read: BASE.1,
                        textured: false,
                        level: |_| 0.3,
                    },
                )
            })
            .collect();
        cases.extend(nulls().into_iter().skip(2));
        for (name, null) in cases {
            let size = 1024usize;
            let reps = 100_000usize.div_ceil((size / 16) * (size / 16));
            let mut trusted = [0.0f64; 2];
            let mut worst = [0.0f32; 2];
            let mut ratios: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
            for rep in 0..reps {
                let prior = gather(&null.frames(size, rep));
                for (l, (share, w)) in null_trust(&prior, null.n).into_iter().enumerate() {
                    trusted[l] += share / reps as f64;
                    worst[l] = worst[l].max(w);
                }
                for (l, level) in prior.ratios().into_iter().enumerate() {
                    ratios[l].extend(level.iter().map(|(r, _)| *r));
                }
            }
            for (l, r) in ratios.iter_mut().enumerate() {
                if r.is_empty() {
                    println!(
                        "{name}, {} frames, level {}: every block excluded",
                        null.n,
                        l + 1
                    );
                    continue;
                }
                r.sort_by(f32::total_cmp);
                let q = |p: f64| r[((r.len() - 1) as f64 * p) as usize];
                println!(
                    "{name}, {} frames, level {}, {} blocks: trusted {:.5}, ratio 99.9% {:.2} 99.99% {:.2} most {:.2}, threshold {:.2} (most over it {:.2})",
                    null.n,
                    l + 1,
                    r.len(),
                    trusted[l],
                    q(0.999),
                    q(0.9999),
                    r[r.len() - 1],
                    super::super::prior::ratio_from(l + 1, null.n),
                    worst[l]
                );
            }
        }
    }

    /// Flat noise is the old merge's at a large count and with a frame
    /// a stop under the rest. (Under the prior's least count the merge
    /// is the old one to the bit: `the_merge_in_passes_is_the_merge_that_held_every_frame`.)
    #[test]
    fn the_prior_does_not_select_noise() {
        for shot in [
            Shot {
                frames: 41,
                blur: 50.0,
                width: 240,
                ..Shot::default()
            },
            Shot {
                under: Some(3),
                width: 240,
                ..Shot::default()
            },
        ] {
            let (before, after) = compare(&shot);
            assert!(
                after.flat <= before.flat * 1.05,
                "{shot:?}: flat noise {:.3} against {:.3} of a frame's",
                after.flat,
                before.flat
            );
        }
    }

    /// Every shot the bed knows, before and after. A measurement:
    /// `cargo test -p greycard-core bed_measure -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn bed_measure() {
        let deep = Shot {
            frames: 31,
            blur: 40.0,
            ..Shot::default()
        };
        for shot in [
            Shot::default(),
            Shot {
                shot: 0.0,
                read: 0.0,
                ..Shot::default()
            },
            deep,
            Shot {
                shot: 8e-5,
                read: 2e-3,
                ..Shot::default()
            },
            Shot {
                breathing: 0.0,
                ..Shot::default()
            },
            Shot {
                breathing: 0.0,
                ..deep
            },
        ] {
            let frames = shot.frames();
            let t = std::time::Instant::now();
            let _ = old(&frames);
            let before = t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let _ = new(&frames);
            let after = t.elapsed().as_secs_f64();
            compare(&shot);
            println!("  merge {before:.2} s before, {after:.2} s after");
        }
    }

    /// A measurement: what the stack's prior says where a blown
    /// specular sits. The bed's stack with a disc of 8 px at six times
    /// the ground, focused with frame `k0`, seen by frame `k` through a
    /// pillbox of 2.5 px a step from it. The evidence is gathered from
    /// the frames held to 1, as the placing pass's short develop holds
    /// them; the multipliers from the frames over 1, as a walk that
    /// reconstructs hands them to the blend. Prints the most trust any
    /// block near the disc has at each level, and the spread of the
    /// frames' multipliers there.
    #[test]
    #[ignore = "a measurement: the prior at a blown specular"]
    fn bed_blown_specular_prior() {
        let shot = Shot::default();
        let (w, h) = (shot.width, shot.height);
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let depth = shot.depth(cx, cy);
        let k0 = (0..shot.frames)
            .min_by(|&a, &b| {
                (shot.focus(a) - depth)
                    .abs()
                    .total_cmp(&(shot.focus(b) - depth).abs())
            })
            .unwrap();
        let disc = |d: f64, r1: f64, r2: f64| {
            use std::f64::consts::PI;
            if d >= r1 + r2 {
                return 0.0;
            }
            if d <= (r1 - r2).abs() {
                return PI * r1.min(r2).powi(2);
            }
            let a = ((d * d + r1 * r1 - r2 * r2) / (2.0 * d * r1)).clamp(-1.0, 1.0);
            let b = ((d * d + r2 * r2 - r1 * r1) / (2.0 * d * r2)).clamp(-1.0, 1.0);
            let k = ((-d + r1 + r2) * (d + r1 - r2) * (d - r1 + r2) * (d + r1 + r2)).max(0.0);
            r1 * r1 * a.acos() + r2 * r2 * b.acos() - 0.5 * k.sqrt()
        };
        let lit = |held: bool| -> Vec<WorkingImage> {
            shot.frames()
                .into_iter()
                .enumerate()
                .map(|(k, mut f)| {
                    let b = (2.5 * (k as f64 - k0 as f64).abs()).max(0.35);
                    for y in 0..h {
                        for x in 0..w {
                            let r = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                            let spot = 6.0 * 0.4 * disc(r, 8.0, b) / (std::f64::consts::PI * b * b);
                            let i = (y * w + x) * 3;
                            for v in &mut f.data[i..i + 3] {
                                *v += spot as f32;
                                if held {
                                    *v = v.min(1.0);
                                }
                            }
                        }
                    }
                    f
                })
                .collect()
        };
        let plain = shot.frames();
        let held = lit(true);
        let over = lit(false);
        let side = super::super::prior::SIDE as f64;
        let cols = w.div_ceil(super::super::prior::SIDE);
        let centre = |b: usize| {
            (
                ((b % cols) as f64 + 0.5) * side,
                ((b / cols) as f64 + 0.5) * side,
            )
        };
        let away = |b: usize| {
            let (x, y) = centre(b);
            ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
        };
        let near: Vec<usize> = (0..cols * h.div_ceil(super::super::prior::SIDE))
            .filter(|&b| away(b) <= 48.0)
            .collect();
        println!(
            "focused frame {k0}; blocks within 48 px of the disc: {}",
            near.len()
        );
        for (name, evidence, blend) in [
            ("no specular", &plain, &plain),
            ("specular, blended held to 1 (before)", &held, &held),
            ("specular, blended over 1 (now)", &held, &over),
        ] {
            let prior = gather(evidence);
            let trust: Vec<String> = [16.0, 32.0, 48.0]
                .iter()
                .map(|&r| format!("{r}: {:?}", prior.trust_near((cx, cy), r)))
                .collect();
            let multipliers: Vec<Vec<f32>> = blocks_of(blend)
                .iter()
                .map(|b| prior.multiplier(b, &Transform::IDENTITY))
                .collect();
            // Each block's frame with the largest multiplier, where it
            // picks one at all.
            let picks: Vec<String> = near
                .iter()
                .filter_map(|&b| {
                    let (lo, hi, k) = multipliers.iter().enumerate().fold(
                        (f32::MAX, 0.0f32, 0usize),
                        |(lo, hi, k), (i, m)| {
                            (lo.min(m[b]), hi.max(m[b]), if m[b] > hi { i } else { k })
                        },
                    );
                    (hi / lo > 1.5).then(|| format!("{:.0}px:{k}", away(b)))
                })
                .collect();
            println!(
                "{name}: trust (most, blocks) a level within {}",
                trust.join("; ")
            );
            println!(
                "  blocks picking a frame (distance: frame): {}",
                picks.join(" ")
            );
        }
    }
}
