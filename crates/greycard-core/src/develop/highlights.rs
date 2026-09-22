//! Highlight reconstruction before the demosaic: "inpaint opposed".
//!
//! Ported from darktable's `src/iop/hlreconstruct/opposed.c` and the
//! shared `_calc_refavg` in `segbased.c` (darktable 4.0 onwards),
//! GPL-3.0-or-later, copyright the darktable developers. The algorithm
//! was developed by Hanno Schwalm with garagecoder and Iain of the G'MIC
//! team. The method and its constants are theirs; the buffers and the
//! threading are ours.
//!
//! The observation behind it: for a clipped photosite, the mean of the
//! two other colors in its 3x3 neighborhood (taken in cube-root space,
//! the "opposed" average) is a good estimate in most images, for small
//! speculars and large areas alike. A per-channel chrominance offset,
//! measured on unclipped photosites right next to the clipped ones, then
//! removes the color cast that plain averaging would leave.
//!
//! Runs on the white-balanced mosaic, where channel `c` saturates at its
//! gain. Nothing here decides what to do with values above 1.0 afterwards;
//! that is [`super::develop`]'s ceiling.
//!
//! One departure: where every photosite in the 3x3 neighborhood is
//! clipped there is nothing to reconstruct from, and the opposed average
//! would leave a cast toward the highest-gain channel. The reference
//! leaves that to the tone mapper; this engine has none, so such
//! photosites go to neutral at the brightest channel's clip level.

use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::raw::CfaPattern;

/// The reference treats a photosite this close to its clip level as
/// clipped, to catch sensors that soften just below saturation.
pub const CLIP_MAGIC: f32 = 0.987;
/// Photosites darker than this fraction of the clip level do not vote on
/// the chrominance offset.
const LOW_CLIP: f32 = 0.2;
/// Fewer votes than this and the offset is left at zero.
const MIN_VOTES: usize = 100;

/// What reconstruction found and did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HighlightStats {
    /// Photosites at or above their channel's clip level.
    pub clipped: usize,
    /// The chrominance offset added to the opposed average, per channel.
    pub chrominance: [f32; 3],
    /// How many unclipped photosites near clipped ones voted, per channel.
    pub votes: [usize; 3],
}

/// Reconstruct clipped photosites. `samples` are one per pixel, levels
/// normalized and white balance applied; `clips` is the level at which
/// each channel is treated as saturated, normally the gains times
/// [`CLIP_MAGIC`]. Returns the new mosaic and what was measured.
pub fn inpaint_opposed(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    clips: [f32; 3],
) -> Result<(Vec<f32>, HighlightStats)> {
    if !pattern.is_rgb() {
        return Err(Error::Unsupported(format!(
            "highlight reconstruction needs an RGB pattern, got {pattern:?}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} samples",
            samples.len()
        )));
    }
    let mosaic = Mosaic {
        samples,
        width,
        height,
        pattern,
        clips,
    };
    let color = |x: usize, y: usize| mosaic.color(x, y);

    // Clipped mask per channel on a grid of 3x3 blocks.
    let mw = width.div_ceil(3);
    let mh = height.div_ceil(3);
    let block = |x: usize, y: usize| (y / 3) * mw + x / 3;
    let mut mask = vec![[false; 3]; mw * mh];
    let mut clipped = 0usize;
    for y in 0..height {
        for x in 0..width {
            let c = color(x, y);
            if samples[y * width + x] >= clips[c] {
                mask[block(x, y)][c] = true;
                clipped += 1;
            }
        }
    }
    let mut stats = HighlightStats {
        clipped,
        ..Default::default()
    };
    if clipped == 0 {
        return Ok((samples.to_vec(), stats));
    }

    // Dilate each channel's mask by about three blocks so the photosites
    // just outside the clipped area are found.
    let dilated: Vec<[bool; 3]> = (0..mw * mh)
        .into_par_iter()
        .map(|i| {
            let (bx, by) = (i % mw, i / mw);
            let safe = bx >= 3 && by >= 3 && bx < mw - 3 && by < mh - 3;
            if !safe {
                return mask[i];
            }
            let mut out = [false; 3];
            for (c, o) in out.iter_mut().enumerate() {
                *o = DILATION.iter().any(|&(dx, dy)| {
                    mask[(by as isize + dy) as usize * mw + (bx as isize + dx) as usize][c]
                });
            }
            out
        })
        .collect();

    // Chrominance offset: unclipped photosites near clipped ones, the
    // difference between what they read and what the opposed average
    // would have guessed.
    let lo = clips.map(|c| LOW_CLIP * c);
    let (sums, votes) = (0..height)
        .into_par_iter()
        .map(|y| {
            let mut sums = [0.0f64; 3];
            let mut votes = [0usize; 3];
            for x in 0..width {
                let c = color(x, y);
                let v = samples[y * width + x];
                if v > lo[c] && v < clips[c] && dilated[block(x, y)][c] {
                    let (ref_avg, _) = mosaic.refavg(x, y, c);
                    sums[c] += f64::from(v - ref_avg);
                    votes[c] += 1;
                }
            }
            (sums, votes)
        })
        .reduce(
            || ([0.0f64; 3], [0usize; 3]),
            |a, b| {
                let mut s = a.0;
                let mut v = a.1;
                for c in 0..3 {
                    s[c] += b.0[c];
                    v[c] += b.1[c];
                }
                (s, v)
            },
        );
    for c in 0..3 {
        stats.votes[c] = votes[c];
        stats.chrominance[c] = if votes[c] > MIN_VOTES {
            (sums[c] / votes[c] as f64) as f32
        } else {
            0.0
        };
    }

    // Fill clipped photosites, never below what they read. A neighborhood
    // with nothing unclipped in it goes to neutral white.
    let chrominance = stats.chrominance;
    let white = clips.iter().copied().fold(0.0f32, f32::max) / CLIP_MAGIC;
    let mut out = vec![0.0f32; width * height];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let c = color(x, y);
            let v = samples[y * width + x];
            *o = if v >= clips[c] {
                let (ref_avg, all_clipped) = mosaic.refavg(x, y, c);
                if all_clipped {
                    white
                } else {
                    v.max(ref_avg + chrominance[c])
                }
            } else {
                v
            };
        }
    });
    Ok((out, stats))
}

/// The reference's dilation footprint: the full 3x3, then a ring out to
/// three blocks with the corners cut.
const DILATION: [(isize, isize); 45] = [
    (0, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
    (-2, -3),
    (-1, -3),
    (0, -3),
    (1, -3),
    (2, -3),
    (-3, -2),
    (-2, -2),
    (-1, -2),
    (0, -2),
    (1, -2),
    (2, -2),
    (3, -2),
    (-3, -1),
    (-2, -1),
    (2, -1),
    (3, -1),
    (-3, 0),
    (-2, 0),
    (2, 0),
    (3, 0),
    (-3, 1),
    (-2, 1),
    (2, 1),
    (3, 1),
    (-3, 2),
    (-2, 2),
    (-1, 2),
    (0, 2),
    (1, 2),
    (2, 2),
    (3, 2),
    (-2, 3),
    (-1, 3),
    (0, 3),
    (1, 3),
    (2, 3),
];

/// The opposed average at photosite `(x, y)` of color `c` in cube-root
/// space: per-channel means of the 3x3 neighborhood, cube-rooted, the
/// mean of the two other channels. Shared with [`super::segments`].
pub(crate) fn opposed_root(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    x: usize,
    y: usize,
    c: usize,
) -> f32 {
    let mut sum = [0.0f32; 3];
    let mut cnt = [0u32; 3];
    for ny in y.saturating_sub(1)..(y + 2).min(height) {
        for nx in x.saturating_sub(1)..(x + 2).min(width) {
            let k = pattern
                .color_at(ny, nx)
                .rgb_index()
                .expect("RGB pattern checked by the caller");
            sum[k] += samples[ny * width + nx].max(0.0);
            cnt[k] += 1;
        }
    }
    let mean = |k: usize| {
        if cnt[k] > 0 {
            (sum[k] / cnt[k] as f32).cbrt()
        } else {
            0.0
        }
    };
    0.5 * (mean((c + 1) % 3) + mean((c + 2) % 3))
}

/// The mosaic with what the reconstruction needs to know about it.
struct Mosaic<'a> {
    samples: &'a [f32],
    width: usize,
    height: usize,
    pattern: &'a CfaPattern,
    clips: [f32; 3],
}

impl Mosaic<'_> {
    #[inline]
    fn color(&self, x: usize, y: usize) -> usize {
        self.pattern
            .color_at(y, x)
            .rgb_index()
            .expect("RGB pattern checked by the caller")
    }

    /// The opposed average at a photosite of color `c`: per-channel means
    /// of the 3x3 neighborhood in cube-root space, the mean of the two
    /// other channels, cubed back. Also reports whether every photosite in
    /// the neighborhood was clipped.
    #[inline]
    fn refavg(&self, x: usize, y: usize, c: usize) -> (f32, bool) {
        let mut sum = [0.0f32; 3];
        let mut cnt = [0u32; 3];
        let mut all_clipped = true;
        for ny in y.saturating_sub(1)..(y + 2).min(self.height) {
            for nx in x.saturating_sub(1)..(x + 2).min(self.width) {
                let k = self.color(nx, ny);
                let v = self.samples[ny * self.width + nx];
                all_clipped &= v >= self.clips[k];
                sum[k] += v.max(0.0);
                cnt[k] += 1;
            }
        }
        let mean = |k: usize| {
            if cnt[k] > 0 {
                (sum[k] / cnt[k] as f32).cbrt()
            } else {
                0.0
            }
        };
        let root = match c {
            0 => 0.5 * (mean(1) + mean(2)),
            1 => 0.5 * (mean(0) + mean(2)),
            _ => 0.5 * (mean(0) + mean(1)),
        };
        (root * root * root, all_clipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::CfaColor;

    fn mosaic(width: usize, height: usize, f: impl Fn(usize, usize, usize) -> f32) -> Vec<f32> {
        let p = CfaPattern::rggb();
        (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                f(x, y, p.color_at(y, x).rgb_index().unwrap())
            })
            .collect()
    }

    #[test]
    fn dilation_footprint_matches_the_reference_shape() {
        assert_eq!(DILATION.len(), 45);
        let mut seen = std::collections::HashSet::new();
        for d in DILATION {
            assert!(seen.insert(d), "{d:?} twice");
            assert!(d.0.abs() <= 3 && d.1.abs() <= 3);
            assert!(d.0.abs() + d.1.abs() < 6, "corner {d:?} should be cut");
        }
    }

    #[test]
    fn unclipped_images_pass_through() {
        let cfa = mosaic(30, 30, |x, _, c| {
            [0.6, 0.9, 0.4][c] * (0.5 + x as f32 / 60.0)
        });
        let (out, stats) =
            inpaint_opposed(&cfa, 30, 30, &CfaPattern::rggb(), [2.0, 1.0, 2.0]).unwrap();
        assert_eq!(out, cfa);
        assert_eq!(stats.clipped, 0);
    }

    #[test]
    fn clipped_green_takes_the_opposed_average() {
        // Gains (2, 1, 2): red and blue at 1.2 are unclipped, green sits at
        // its clip of 1.0. No unclipped green anywhere, so no offset.
        let cfa = mosaic(30, 30, |_, _, c| [1.2, 1.0, 1.2][c]);
        let (out, stats) =
            inpaint_opposed(&cfa, 30, 30, &CfaPattern::rggb(), [2.0, 1.0, 2.0]).unwrap();
        assert_eq!(stats.clipped, 450);
        assert_eq!(stats.chrominance, [0.0; 3]);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 1.2).abs() < 1e-4, "{i}: {v}");
        }
    }

    #[test]
    fn fully_clipped_neighborhoods_go_to_neutral_at_the_ceiling() {
        let gains = [2.0f32, 1.0, 4.0];
        let clips = gains.map(|g| g * CLIP_MAGIC);
        let cfa = mosaic(30, 30, |_, _, c| gains[c]);
        let (out, _) = inpaint_opposed(&cfa, 30, 30, &CfaPattern::rggb(), clips).unwrap();
        assert!(
            out.iter().all(|v| (v - 4.0).abs() < 1e-5),
            "{:?}",
            &out[..8]
        );
        let cfa = mosaic(4, 4, |_, _, c| gains[c]);
        let (out, _) = inpaint_opposed(&cfa, 4, 4, &CfaPattern::rggb(), clips).unwrap();
        assert!(out.iter().all(|v| (v - 4.0).abs() < 1e-5), "{out:?}");
    }

    #[test]
    fn chrominance_offset_is_measured_next_to_the_clipped_area() {
        // A ramp in red and blue; green is always red minus 0.1 but clips
        // at 1.0 on the right. The offset should come out near -0.1 and the
        // reconstructed green should follow the ramp.
        let (w, h) = (120, 60);
        let ramp = |x: usize| 0.6 + 0.8 * x as f32 / (w - 1) as f32;
        let cfa = mosaic(w, h, |x, _, c| match c {
            1 => (ramp(x) - 0.1).min(1.0),
            _ => ramp(x),
        });
        let (out, stats) =
            inpaint_opposed(&cfa, w, h, &CfaPattern::rggb(), [2.0, 1.0, 2.0]).unwrap();
        assert!(stats.votes[1] > MIN_VOTES, "{:?}", stats.votes);
        assert!(
            (stats.chrominance[1] + 0.1).abs() < 0.02,
            "{:?}",
            stats.chrominance
        );
        let p = CfaPattern::rggb();
        for y in 10..h - 10 {
            for x in w - 20..w - 5 {
                if p.color_at(y, x) != CfaColor::Green {
                    continue;
                }
                let want = ramp(x) - 0.1;
                let got = out[y * w + x];
                assert!((got - want).abs() < 0.03, "({x},{y}) got {got} want {want}");
            }
        }
    }
}
