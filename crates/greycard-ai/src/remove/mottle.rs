//! The fill's chroma at the latents' scale: the four-step remove leaves
//! a green-magenta mottle 8 to 16 px across in its fills, a chroma the
//! picture round the hole does not have, and 25 steps leave a little of
//! it on a sky. Here the fill's two chroma axes (the denoiser's
//! orthonormal luma and chroma, in linear light) are smoothed by a
//! guided filter with the fill's luma as the guide, [`RADIUS`] wide,
//! over the hole's own pixels alone: chroma that follows the luma (a
//! red leaf against the sea, a sign's edge) is kept, chroma that does
//! not is averaged out. How much of the smoothed chroma is taken is
//! solved for per axis, so that the fill's chroma, read at the
//! distances of [`DISTANCES`] after a small box ([`PRE`]), comes down
//! to the picture's round the hole times [`TOLERANCE`] and no further.
//! A fill whose chroma already matches is left exactly as it was; the
//! luma is never touched.
//!
//! The guided filter is a reimplementation from the paper: "Guided
//! Image Filtering", Kaiming He, Jian Sun, and Xiaoou Tang, ECCV 2010,
//! with its box means taken over the hole's pixels only, each sum
//! beside a count, so nothing of the picture next to the hole (the rim
//! a removed object leaves) is carried into the fill.
//!
//! What it cannot tell apart: the guide is the plain sum of the linear
//! channels, (r + g + b) / √3, not a perceptual luma, so two colors of
//! the same sum meet with no edge in the guide; such an edge, or a
//! chroma texture with no luma in it, is smoothed like a mottle, across
//! a triangle about 2 × [`RADIUS`] wide on each side. And the filter's
//! regularization is read from the ring's luma ([`EPS_SCALE`]), so round
//! a textured surround (foliage) it is the texture's size, and luma
//! steps up to that size do not hold the chroma either.

use super::scale::{to_linear, to_srgb};
use crate::image::Rgbf;

/// The guided filter's radius, in the model's pixels: the latents'
/// cell is 8, and the mottle runs one to two cells across, which a
/// window three cells wide averages out. 8 left the wall's two-cell
/// blotches; 16 measured the same as 12.
pub const RADIUS: usize = 12;

/// The distances the chroma residual is read at, after [`PRE`].
pub const DISTANCES: [usize; 3] = [4, 8, 16];

/// The box radius the chroma is taken through before it is read, so
/// the picture's fine grain, which a raw residual at any distance is
/// mostly made of, does not hide a mottle at the latents' scale and
/// above.
pub const PRE: usize = 2;

/// How far over the picture's residual the fill's may sit before any
/// smoothing is taken: the picture's own.
pub const TOLERANCE: f64 = 1.0;

/// The band of the picture the residual is read in, out from the
/// hole's edge: set back past a removed object's rim, as the level
/// across the edge is.
const RING: (usize, usize) = (8, 48);

/// How far past the hole's box the work reaches: the ring, read at the
/// farthest distance after the box.
const PAD: usize = RING.1 + DISTANCES[DISTANCES.len() - 1] + PRE;

/// The fewest pixels either side must hold at a distance for it to
/// count.
const LEAST: usize = 256;

/// The guided filter's regularization, as a multiple of the luma's
/// residual at distance two round the hole, squared: luma at the
/// grain's own size is not taken for an edge. Twice that kept 84
/// percent of a chroma edge's step two pixels from it; once keeps it.
const EPS_SCALE: f32 = 1.0;

/// How much of the smoothed chroma was taken on each chroma axis.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Smoothed {
    pub amount: [f32; 2],
}

const S2: f32 = std::f32::consts::SQRT_2;

fn to_axes([r, g, b]: [f32; 3]) -> [f32; 3] {
    let s3 = 3f32.sqrt();
    let s6 = 6f32.sqrt();
    [(r + g + b) / s3, (r - b) / S2, (r - 2.0 * g + b) / s6]
}

fn from_axes([y, u, v]: [f32; 3]) -> [f32; 3] {
    let s3 = 3f32.sqrt();
    let s6 = 6f32.sqrt();
    [
        y / s3 + u / S2 + v / s6,
        y / s3 - 2.0 * v / s6,
        y / s3 - u / S2 + v / s6,
    ]
}

/// The mean of `p` in a square of `r` round each pixel, clipped to the
/// window.
#[cfg(test)]
fn box_mean(p: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let all = vec![true; w * h];
    masked_mean(p, &all, w, h, r)
}

/// The mean of `p` over the pixels of `mask` in a square of `r` round
/// each pixel, clipped to the window: a summed-area table of the values
/// and one of the count. Zero where the square holds none.
fn masked_mean(p: &[f32], mask: &[bool], w: usize, h: usize, r: usize) -> Vec<f32> {
    let stride = w + 1;
    let mut sum = vec![[0.0f64; 2]; stride * (h + 1)];
    for y in 0..h {
        let mut row = [0.0f64; 2];
        for x in 0..w {
            let i = y * w + x;
            if mask[i] {
                row[0] += f64::from(p[i]);
                row[1] += 1.0;
            }
            let above = sum[y * stride + x + 1];
            sum[(y + 1) * stride + x + 1] = [above[0] + row[0], above[1] + row[1]];
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s: [f64; 2] = std::array::from_fn(|k| {
                sum[y1 * stride + x1][k] - sum[y0 * stride + x1][k] - sum[y1 * stride + x0][k]
                    + sum[y0 * stride + x0][k]
            });
            if s[1] > 0.0 {
                out[y * w + x] = (s[0] / s[1]) as f32;
            }
        }
    }
    out
}

/// [`PRE`]'s box over the hole's pixels for a pixel in the hole and
/// over the picture's for one outside it, so neither side's reading
/// takes in the other's.
fn split_mean(p: &[f32], inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    let outside: Vec<bool> = inside.iter().map(|&m| !m).collect();
    let (a, b) = (
        masked_mean(p, inside, w, h, PRE),
        masked_mean(p, &outside, w, h, PRE),
    );
    (0..w * h)
        .map(|i| if inside[i] { a[i] } else { b[i] })
        .collect()
}

/// The guided filter's view of its guide over `mask`: the guide's mean
/// and variance round each pixel, the same for both chroma axes.
struct Guide<'a> {
    guide: &'a [f32],
    mask: &'a [bool],
    w: usize,
    h: usize,
    r: usize,
    mean: Vec<f32>,
    var: Vec<f32>,
}

impl<'a> Guide<'a> {
    fn new(guide: &'a [f32], mask: &'a [bool], w: usize, h: usize, r: usize) -> Self {
        let mean = masked_mean(guide, mask, w, h, r);
        let sq: Vec<f32> = guide.iter().map(|v| v * v).collect();
        let var = masked_mean(&sq, mask, w, h, r)
            .iter()
            .zip(&mean)
            .map(|(s, m)| (s - m * m).max(0.0))
            .collect();
        Self {
            guide,
            mask,
            w,
            h,
            r,
            mean,
            var,
        }
    }

    /// `p` filtered by the guide over the mask, `p` itself off it.
    fn filter(&self, p: &[f32], eps: f32) -> Vec<f32> {
        let (w, h, r, mask) = (self.w, self.h, self.r, self.mask);
        let mp = masked_mean(p, mask, w, h, r);
        let ip: Vec<f32> = self.guide.iter().zip(p).map(|(a, b)| a * b).collect();
        let mip = masked_mean(&ip, mask, w, h, r);
        let mut a = vec![0.0f32; w * h];
        let mut b = vec![0.0f32; w * h];
        for i in (0..w * h).filter(|&i| mask[i]) {
            let cov = mip[i] - self.mean[i] * mp[i];
            a[i] = cov / (self.var[i] + eps);
            b[i] = mp[i] - a[i] * self.mean[i];
        }
        let ma = masked_mean(&a, mask, w, h, r);
        let mb = masked_mean(&b, mask, w, h, r);
        (0..w * h)
            .map(|i| {
                if mask[i] {
                    ma[i] * self.guide[i] + mb[i]
                } else {
                    p[i]
                }
            })
            .collect()
    }
}

/// The residual at distance `d`: a pixel less the mean of its four
/// neighbors `d` away.
fn residual(p: &[f32], w: usize, i: usize, d: usize) -> f32 {
    p[i] - (p[i - d] + p[i + d] + p[i - d * w] + p[i + d * w]) * 0.25
}

/// The pixels whose neighbors at `d` are all in `region`, `d` in from
/// the window's edge.
fn whole(region: &[bool], w: usize, h: usize, d: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for y in d..h.saturating_sub(d) {
        for x in d..w.saturating_sub(d) {
            let i = y * w + x;
            if region[i] && region[i - d] && region[i + d] && region[i - d * w] && region[i + d * w]
            {
                out.push(i);
            }
        }
    }
    out
}

fn rms(p: &[f32], w: usize, d: usize, at: &[usize]) -> f64 {
    let s: f64 = at
        .iter()
        .map(|&i| f64::from(residual(p, w, i, d)).powi(2))
        .sum();
    (s / at.len() as f64).sqrt()
}

/// The share of `q` to take into `p`, given each distance's mean
/// squares (`s00` of `p`'s residual, `s01` of its product with the
/// change, `s11` of the change) and its target.
fn share(reads: &[(f64, f64, f64, f64)]) -> f64 {
    let mut take = 0.0f64;
    for &(s00, s01, s11, t2) in reads {
        if s00 <= t2 || s11 <= 0.0 {
            continue;
        }
        // The residual of (1 − t) p + t q is r0 + t (rq − r0): its mean
        // square s11 t² + 2 s01 t + s00, which falls to the target at
        // the smaller root; where it never does, its least, at the
        // vertex.
        let disc = s01 * s01 - s11 * (s00 - t2);
        let t = if disc < 0.0 {
            -s01 / s11
        } else {
            (-s01 - disc.sqrt()) / s11
        };
        take = take.max(t.clamp(0.0, 1.0));
    }
    take
}

/// `fill` (sRGB-encoded, in 0 to 1, like `image`) with its chroma under
/// `hole` (nonzero is in it) smoothed toward the picture's round it,
/// and how much was taken; `fill` as it was and `None` where there is
/// too little of the hole or the ring to read, or no axis reads over
/// the picture's by more than [`TOLERANCE`].
pub fn smoothed(image: &Rgbf, hole: &[f32], fill: &Rgbf) -> (Rgbf, Option<Smoothed>) {
    let (fw, fh) = (image.width, image.height);
    if fill.width != fw || fill.height != fh || hole.len() != fw * fh {
        return (fill.clone(), None);
    }
    // The hole's box, out by the ring and its reach.
    let (mut x0, mut x1, mut y0, mut y1) = (fw, 0, fh, 0);
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
        let (x, y) = (i % fw, i / fw);
        (x0, x1, y0, y1) = (x0.min(x), x1.max(x + 1), y0.min(y), y1.max(y + 1));
    }
    if x0 >= x1 {
        return (fill.clone(), None);
    }
    let (x0, y0) = (x0.saturating_sub(PAD), y0.saturating_sub(PAD));
    let (x1, y1) = ((x1 + PAD).min(fw), (y1 + PAD).min(fh));
    let (w, h) = (x1 - x0, y1 - y0);
    let at = |i: usize| (y0 + i / w) * fw + x0 + i % w;

    let cut: Vec<f32> = (0..w * h).map(|i| hole[at(i)]).collect();
    let inside: Vec<bool> = cut.iter().map(|&m| m > 0.0).collect();
    let near = super::ring(&cut, w, h, RING.0);
    let ring: Vec<bool> = super::ring(&cut, w, h, RING.1)
        .into_iter()
        .zip(&near)
        .map(|(o, n)| o && !n)
        .collect();

    // The fill under the hole and the picture round it, on the axes.
    let mut axes = [
        vec![0.0f32; w * h],
        vec![0.0f32; w * h],
        vec![0.0f32; w * h],
    ];
    for (i, &is_in) in inside.iter().enumerate() {
        let src = if is_in { &fill.data } else { &image.data };
        let j = at(i);
        let a = to_axes([0, 1, 2].map(|c| to_linear(src[j * 3 + c])));
        for (axis, v) in axes.iter_mut().zip(a) {
            axis[i] = v;
        }
    }

    let luma_ring = {
        let at = whole(&ring, w, h, 2);
        if at.len() < LEAST {
            return (fill.clone(), None);
        }
        rms(&axes[0], w, 2, &at)
    };
    let eps = ((EPS_SCALE as f64 * luma_ring).powi(2) as f32).max(1e-10);
    let guide = Guide::new(&axes[0], &inside, w, h, RADIUS);

    let mut amount = [0.0f32; 2];
    let mut smooth: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    for k in 0..2 {
        let p = &axes[k + 1];
        let q = guide.filter(p, eps);
        let (lp, lq) = (split_mean(p, &inside, w, h), split_mean(&q, &inside, w, h));
        let mut reads = Vec::new();
        for d in DISTANCES {
            let (hin, rin) = (whole(&inside, w, h, d), whole(&ring, w, h, d));
            if hin.len() < LEAST || rin.len() < LEAST {
                continue;
            }
            let target = TOLERANCE * rms(&lp, w, d, &rin);
            let (mut s00, mut s01, mut s11) = (0.0f64, 0.0f64, 0.0f64);
            for &i in &hin {
                let r0 = f64::from(residual(&lp, w, i, d));
                let dr = f64::from(residual(&lq, w, i, d)) - r0;
                s00 += r0 * r0;
                s01 += r0 * dr;
                s11 += dr * dr;
            }
            let n = hin.len() as f64;
            reads.push((s00 / n, s01 / n, s11 / n, target * target));
        }
        amount[k] = share(&reads) as f32;
        smooth[k] = q;
    }
    if amount.iter().all(|&t| t <= 0.0) {
        return (fill.clone(), None);
    }

    let mut data = fill.data.clone();
    for i in (0..w * h).filter(|&i| inside[i]) {
        let mut a = [axes[0][i], axes[1][i], axes[2][i]];
        for k in 0..2 {
            a[k + 1] += amount[k] * (smooth[k][i] - a[k + 1]);
        }
        let j = at(i);
        for (c, v) in from_axes(a).into_iter().enumerate() {
            data[j * 3 + c] = to_srgb(v);
        }
    }
    (Rgbf::new(fw, fh, data), Some(Smoothed { amount }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 256;

    /// A square hole of side 120 in the middle.
    fn hole() -> Vec<f32> {
        (0..N * N)
            .map(|i| {
                let (x, y) = (i % N, i / N);
                f32::from(u8::from((68..188).contains(&x) && (68..188).contains(&y)))
            })
            .collect()
    }

    /// Linear `base(x, y)` with grain of `sigma` on the luma axis and
    /// `chroma` on both chroma axes, sRGB-encoded.
    fn picture(
        base: impl Fn(usize, usize) -> [f32; 3],
        sigma: f32,
        chroma: f32,
        seed: u64,
    ) -> Rgbf {
        let n = crate::remove::normals(3 * N * N, seed);
        let mut data = Vec::with_capacity(3 * N * N);
        for i in 0..N * N {
            let mut a = to_axes(base(i % N, i / N));
            a[0] += sigma * n[3 * i];
            a[1] += chroma * n[3 * i + 1];
            a[2] += chroma * n[3 * i + 2];
            data.extend(from_axes(a).map(to_srgb));
        }
        Rgbf::new(N, N, data)
    }

    /// The picture outside `hole`, `fill` inside it.
    fn under(image: &Rgbf, hole: &[f32], fill: &Rgbf) -> Rgbf {
        let mut data = image.data.clone();
        for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m > 0.0) {
            data[i * 3..i * 3 + 3].copy_from_slice(&fill.data[i * 3..i * 3 + 3]);
        }
        Rgbf::new(N, N, data)
    }

    /// `fill` with a chroma mottle of `amount` on both chroma axes, in
    /// 8 px cells softened by a box of 2, as the decoder leaves it.
    fn mottled(fill: &Rgbf, amount: f32, seed: u64) -> Rgbf {
        let cells = crate::remove::normals(2 * (N / 8) * (N / 8), seed);
        let field = |k: usize| {
            let raw: Vec<f32> = (0..N * N)
                .map(|i| cells[k * (N / 8) * (N / 8) + (i / N / 8) * (N / 8) + i % N / 8])
                .collect();
            box_mean(&raw, N, N, 2)
        };
        let (u, v) = (field(0), field(1));
        let mut data = fill.data.clone();
        for i in 0..N * N {
            let mut a = to_axes([0, 1, 2].map(|c| to_linear(data[i * 3 + c])));
            a[1] += amount * u[i];
            a[2] += amount * v[i];
            let rgb = from_axes(a);
            for c in 0..3 {
                data[i * 3 + c] = to_srgb(rgb[c]);
            }
        }
        Rgbf::new(N, N, data)
    }

    /// The V axis's residual at `d` in the hole's middle over the
    /// picture's outside it.
    fn ratio(image: &Rgbf, hole: &[f32], d: usize) -> f64 {
        let v: Vec<f32> = (0..N * N)
            .map(|i| to_axes([0, 1, 2].map(|c| to_linear(image.data[i * 3 + c])))[2])
            .collect();
        let inside: Vec<bool> = (0..N * N)
            .map(|i| {
                let (x, y) = (i % N, i / N);
                (78..178).contains(&x) && (78..178).contains(&y)
            })
            .collect();
        let outside: Vec<bool> = hole.iter().map(|&m| m <= 0.0).collect();
        let v = box_mean(&v, N, N, PRE);
        rms(&v, N, d, &whole(&inside, N, N, d)) / rms(&v, N, d, &whole(&outside, N, N, d))
    }

    const GREY: fn(usize, usize) -> [f32; 3] = |_, _| [0.18, 0.18, 0.18];

    #[test]
    fn a_mottled_fill_is_smoothed_to_its_surround() {
        let hole = hole();
        let image = picture(GREY, 0.004, 0.002, 1);
        let fill = mottled(&picture(GREY, 0.004, 0.002, 2), 0.001, 3);
        let before = under(&image, &hole, &fill);
        assert!(ratio(&before, &hole, 8) > 1.5);
        let (after, s) = smoothed(&image, &hole, &before);
        let s = s.expect("a mottled fill is smoothed");
        assert!(s.amount.iter().all(|&t| t > 0.5), "{s:?}");
        for d in DISTANCES {
            let r = ratio(&after, &hole, d);
            assert!(r < TOLERANCE + 0.15, "at {d}: {r}");
        }
        // The luma is the fill's own.
        let luma =
            |img: &Rgbf, i: usize| to_axes([0, 1, 2].map(|c| to_linear(img.data[i * 3 + c])))[0];
        for i in (0..N * N).step_by(97) {
            assert!((luma(&before, i) - luma(&after, i)).abs() < 1e-4);
        }
    }

    #[test]
    fn a_matching_fill_is_untouched() {
        let hole = hole();
        let image = picture(GREY, 0.004, 0.002, 1);
        let before = under(&image, &hole, &picture(GREY, 0.004, 0.002, 2));
        let (after, s) = smoothed(&image, &hole, &before);
        assert_eq!(s, None);
        assert_eq!(after, before);
    }

    #[test]
    fn a_chroma_edge_with_its_luma_is_kept() {
        // Grey-blue above, a brighter green below, the edge across the
        // hole; the fill carries it and a mottle.
        let edge = |_: usize, y: usize| {
            if y < N / 2 {
                [0.10, 0.12, 0.16]
            } else {
                [0.12, 0.30, 0.06]
            }
        };
        let hole = hole();
        let image = picture(edge, 0.004, 0.002, 1);
        let fill = mottled(&picture(edge, 0.004, 0.002, 2), 0.008, 3);
        let before = under(&image, &hole, &fill);
        let (after, s) = smoothed(&image, &hole, &before);
        let s = s.expect("the mottle is smoothed");
        assert!(s.amount.iter().all(|&t| t > 0.5), "{s:?}");
        let mean = |img: &Rgbf, ys: std::ops::Range<usize>| {
            let mut s = [0.0f64; 3];
            let mut n = 0.0;
            for y in ys {
                for x in 90..166 {
                    let a = to_axes([0, 1, 2].map(|c| to_linear(img.data[(y * N + x) * 3 + c])));
                    for k in 0..3 {
                        s[k] += f64::from(a[k]);
                    }
                    n += 1.0;
                }
            }
            s.map(|v| v / n)
        };
        // Two pixels either side of the edge.
        for (above, below) in [(126..127, 129..130), (124..126, 130..132)] {
            let (a0, b0) = (mean(&before, above.clone()), mean(&before, below.clone()));
            let (a1, b1) = (mean(&after, above), mean(&after, below));
            for k in 1..3 {
                let (s0, s1) = (b0[k] - a0[k], b1[k] - a1[k]);
                assert!((s1 / s0) > 0.9, "axis {k}: {s1} against {s0}");
            }
        }
    }

    #[test]
    fn chroma_texture_that_follows_the_luma_is_kept() {
        // Bright yellow-green dots, 6 px across, on a darker green: a
        // flower bed in the fill, a plain grey path round it.
        let dots = crate::remove::normals((N / 12 + 1) * (N / 12 + 1), 7);
        let bed = |x: usize, y: usize| {
            let (cx, cy) = (x % 12, y % 12);
            let on = dots[(y / 12) * (N / 12 + 1) + x / 12] > 0.0 && cx < 6 && cy < 6;
            if on {
                [0.40, 0.38, 0.05]
            } else {
                [0.06, 0.14, 0.05]
            }
        };
        let hole = hole();
        let image = picture(GREY, 0.004, 0.002, 1);
        let before = under(&image, &hole, &picture(bed, 0.004, 0.002, 2));
        let (after, s) = smoothed(&image, &hole, &before);
        let s = s.expect("the fill is smoothed");
        assert!(s.amount.iter().all(|&t| t > 0.5), "{s:?}");
        // The chroma's spread about its mean in the hole's middle.
        let spread = |img: &Rgbf, k: usize| {
            let v: Vec<f64> = (0..N * N)
                .filter(|i| (78..178).contains(&(i % N)) && (78..178).contains(&(i / N)))
                .map(|i| f64::from(to_axes([0, 1, 2].map(|c| to_linear(img.data[i * 3 + c])))[k]))
                .collect();
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        for k in 1..3 {
            let (s0, s1) = (spread(&before, k), spread(&after, k));
            assert!(s1 / s0 > 0.9, "axis {k}: {s1} against {s0}");
        }
    }

    /// Pinned as it is: a chroma step with no step in the guide is the
    /// guided filter's two boxes, a triangle 2 × RADIUS wide each side,
    /// half the step at the edge and an eighth of it 12 px away.
    #[test]
    fn an_equal_luminance_chroma_edge_is_smoothed_by_a_triangle() {
        let (w, h) = (128, 64);
        let guide = vec![0.3f32; w * h];
        let p: Vec<f32> = (0..w * h)
            .map(|i| if i % w < w / 2 { 0.0 } else { 1.0 })
            .collect();
        let mask = vec![true; w * h];
        let q = Guide::new(&guide, &mask, w, h, RADIUS).filter(&p, 1e-6);
        let row = &q[(h / 2) * w..(h / 2 + 1) * w];
        let edge = w / 2;
        // The pixel just past the edge, and the pixel 12 px before it.
        assert!((row[edge] - 0.54).abs() < 0.03, "{}", row[edge]);
        assert!((row[edge - 12] - 0.13).abs() < 0.03, "{}", row[edge - 12]);
        assert!(row[edge - 2 * RADIUS - 1] < 1e-6);
    }

    #[test]
    fn the_share_where_the_target_is_out_of_reach_is_the_least() {
        // Mean square 4 − t + t², never down to 1: least at t = 0.5.
        assert!((share(&[(4.0, -0.5, 1.0, 1.0)]) - 0.5).abs() < 1e-12);
        // Mean square 4 − 6 t + 4 t² reaches 2 at t = 0.5.
        assert!((share(&[(4.0, -3.0, 4.0, 2.0)]) - 0.5).abs() < 1e-12);
        // Already under its target.
        assert_eq!(share(&[(1.0, -1.0, 1.0, 2.0)]), 0.0);
    }

    #[test]
    fn the_picture_next_to_the_hole_does_not_reach_into_the_fill() {
        // A red rim 6 px wide round the hole, which the fill does not
        // carry: the fill's chroma stays the fill's.
        let hole = hole();
        let rim = |x: usize, y: usize| {
            let (dx, dy) = (
                x.abs_diff(127).saturating_sub(60),
                y.abs_diff(127).saturating_sub(60),
            );
            if dx.max(dy) < 7 && !(68..188).contains(&x) | !(68..188).contains(&y) {
                [0.40, 0.05, 0.05]
            } else {
                [0.18, 0.18, 0.18]
            }
        };
        let image = picture(rim, 0.004, 0.002, 1);
        let fill = mottled(&picture(GREY, 0.004, 0.002, 2), 0.005, 3);
        let before = under(&image, &hole, &fill);
        let (after, s) = smoothed(&image, &hole, &before);
        let s = s.expect("the fill is smoothed");
        assert!(s.amount.iter().all(|&t| t > 0.5), "{s:?}");
        let u =
            |img: &Rgbf, i: usize| to_axes([0, 1, 2].map(|c| to_linear(img.data[i * 3 + c])))[1];
        // Just inside the hole's left edge, down its middle.
        let mean = |img: &Rgbf| (100..156).map(|y| u(img, y * N + 69)).sum::<f32>() / 56.0;
        let (m0, m1) = (mean(&before), mean(&after));
        assert!((m1 - m0).abs() < 0.003, "{m1} against {m0}");
    }
}
