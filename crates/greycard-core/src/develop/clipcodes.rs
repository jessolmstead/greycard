//! Where the mosaic clipped, cell by cell: what a learned highlight
//! fill reads beside the classical fill.
//!
//! The unit is the cell: a 2x2 quad of a Bayer mosaic, a 3x3 block of
//! an X-Trans one, laid from the crop's origin. A whole Bayer cell
//! holds each color, green twice. A whole X-Trans cell holds each
//! color too, at every one of the pattern's 36 phases, but in counts
//! the phase sets: five greens, and one to three each of red and blue.
//! A cell's code is how many of its colors (0 to 3) have a clipped
//! photosite. Cells at the right and bottom edges of a frame
//! whose side is not a multiple of the cell's are partial, and count
//! what they hold. This is the cell and its count as the highlight
//! benchmark's `ClipMap` has them, which is what the fill was trained
//! on.
//!
//! A photosite is clipped where inpaint opposed takes it as clipped:
//! at or over its gain times the frame's headroom times
//! [`CLIP_MAGIC`], on the white-balanced mosaic before opposed runs.
//! The white balance does not move the codes: a photosite clips at its
//! gain whatever the gain.

use rayon::prelude::*;

use super::highlights::CLIP_MAGIC;
use crate::raw::CfaPattern;

/// The clipped colors of every cell of a mosaic, and each channel's
/// clip level as the fill reads it.
///
/// The cells are the prepared mosaic's: full size, cells laid from the
/// crop's origin, read before the CA correction, the demosaic, any bin,
/// any warp, and the orientation. A consumer after a bin, a warp, or an
/// orientation maps its coordinates back into this frame itself;
/// [`ClipCodes::at`] takes them only in it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipCodes {
    /// The mosaic's width and height in pixels, the frame the cells
    /// cover.
    pub width: usize,
    pub height: usize,
    /// The cell's side: 2 for Bayer, 3 for X-Trans.
    pub cell: usize,
    /// Cells across: the width over the cell's side, rounded up, so the
    /// last column of cells may be partial.
    pub cells_wide: usize,
    /// Cells down: the height over the cell's side, rounded up, so the
    /// last row of cells may be partial.
    pub cells_high: usize,
    /// Per cell, row-major: the count of colors with a clipped
    /// photosite, 0 to 3.
    pub codes: Vec<u8>,
    /// Each channel's clip level over the ceiling (the largest gain,
    /// and at least one): `gain · CLIP_MAGIC / ceiling`. The fill's
    /// picture is read divided by the ceiling times the headroom; the
    /// clips, made at the gains times the headroom, read the same in
    /// those units with the headroom taken out of both, so they carry
    /// none.
    pub clips: [f32; 3],
}

impl ClipCodes {
    /// The code of the cell holding pixel (`x`, `y`) of the full-size
    /// mosaic.
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> u8 {
        debug_assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is outside the {}x{} mosaic the codes cover",
            self.width,
            self.height
        );
        self.codes[(y / self.cell) * self.cells_wide + x / self.cell]
    }
}

/// The codes of a white-balanced mosaic (one sample a pixel, `pattern`
/// from its origin, `gains` applied, levels normalized up to
/// `headroom` over the white), read before any highlight
/// reconstruction. `pattern` is a 2x2 Bayer or an X-Trans one, what
/// the develop demosaics; a photosite of no RGB color counts for none.
pub fn clip_codes(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    gains: [f32; 3],
    headroom: f32,
) -> ClipCodes {
    debug_assert_eq!(samples.len(), width * height);
    let cell = if pattern.is_xtrans() { 3 } else { 2 };
    let cw = width.div_ceil(cell);
    let ch = height.div_ceil(cell);
    // Opposed's own clips, so the codes and what it rebuilds agree to
    // the photosite.
    let levels = gains.map(|g| g * headroom * CLIP_MAGIC);
    let colors: Vec<Option<usize>> = (0..pattern.height)
        .flat_map(|r| (0..pattern.width).map(move |c| (r, c)))
        .map(|(r, c)| pattern.color_at(r, c).rgb_index())
        .collect();
    let mut codes = vec![0u8; cw * ch];
    if cw > 0 {
        codes.par_chunks_mut(cw).enumerate().for_each(|(cy, row)| {
            let mut bits = vec![0u8; cw];
            for y in cy * cell..((cy + 1) * cell).min(height) {
                let line = &samples[y * width..(y + 1) * width];
                let base = (y % pattern.height) * pattern.width;
                for (x, &v) in line.iter().enumerate() {
                    if let Some(c) = colors[base + x % pattern.width]
                        && v >= levels[c]
                    {
                        bits[x / cell] |= 1 << c;
                    }
                }
            }
            for (code, b) in row.iter_mut().zip(bits) {
                *code = b.count_ones() as u8;
            }
        });
    }
    let ceiling = gains.iter().copied().fold(1.0f32, f32::max);
    ClipCodes {
        width,
        height,
        cell,
        cells_wide: cw,
        cells_high: ch,
        codes,
        clips: gains.map(|g| g * CLIP_MAGIC / ceiling),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::CfaColor;

    const GAINS: [f32; 3] = [2.0, 1.0, 1.5];

    /// A mosaic of `fill` everywhere with `set` written in.
    fn mosaic(w: usize, h: usize, fill: f32, set: &[(usize, usize, f32)]) -> Vec<f32> {
        let mut s = vec![fill; w * h];
        for &(x, y, v) in set {
            s[y * w + x] = v;
        }
        s
    }

    fn color(p: &CfaPattern, x: usize, y: usize) -> usize {
        p.color_at(y, x).rgb_index().unwrap()
    }

    /// The count of colors among the pixels `x0..x1` by `y0..y1`.
    fn colors_in(p: &CfaPattern, (x0, x1): (usize, usize), (y0, y1): (usize, usize)) -> u8 {
        let mut bits = 0u8;
        for y in y0..y1 {
            for x in x0..x1 {
                bits |= 1 << color(p, x, y);
            }
        }
        bits.count_ones() as u8
    }

    /// Every channel of every photosite of one cell clipped gives 3,
    /// and the cells round it stay 0, in each of Bayer's four phases.
    #[test]
    fn bayer_cells_count_their_colors_in_every_phase() {
        let (w, h) = (8, 6);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let p = CfaPattern::rggb().shifted(dx, dy);
            // Cell (1, 1): pixels 2..4 by 2..4.
            let mut s = vec![0.1; w * h];
            for y in 2..4 {
                for x in 2..4 {
                    s[y * w + x] = GAINS[color(&p, x, y)];
                }
            }
            let codes = clip_codes(&s, w, h, &p, GAINS, 1.0);
            assert_eq!((codes.cell, codes.cells_wide, codes.cells_high), (2, 4, 3));
            for cy in 0..3 {
                for cx in 0..4 {
                    let want = if (cx, cy) == (1, 1) { 3 } else { 0 };
                    assert_eq!(codes.codes[cy * 4 + cx], want, "phase {dx},{dy}");
                }
            }
            // One color alone: the cell's red, wherever the phase puts it.
            let (rx, ry) = (0..2)
                .flat_map(|y| (0..2).map(move |x| (x, y)))
                .find(|&(x, y)| color(&p, x, y) == 0)
                .unwrap();
            let s = mosaic(w, h, 0.1, &[(rx, ry, 2.0)]);
            let codes = clip_codes(&s, w, h, &p, GAINS, 1.0);
            assert_eq!(codes.at(0, 0), 1);
            assert_eq!(codes.codes.iter().map(|&c| c as usize).sum::<usize>(), 1);
        }
    }

    /// Both greens of a quad clipped are one color, not two.
    #[test]
    fn two_greens_are_one_color() {
        let p = CfaPattern::rggb();
        let s = mosaic(2, 2, 0.1, &[(1, 0, 1.0), (0, 1, 1.0)]);
        assert_eq!(clip_codes(&s, 2, 2, &p, GAINS, 1.0).codes, vec![1]);
        let s = mosaic(2, 2, 0.1, &[(1, 0, 1.0), (0, 1, 1.0), (1, 1, 1.5)]);
        assert_eq!(clip_codes(&s, 2, 2, &p, GAINS, 1.0).codes, vec![2]);
    }

    /// The threshold is opposed's: at 0.987 times the gain a photosite
    /// is clipped, a step under it is not, and over it is.
    #[test]
    fn the_threshold_is_the_gain_times_the_magic() {
        let p = CfaPattern::rggb();
        for (c, (x, y)) in [(0, (0, 0)), (1, (1, 0)), (2, (1, 1))] {
            let at = GAINS[c] * CLIP_MAGIC;
            for (v, want) in [(at, 1), (at.next_down(), 0), (at.next_up(), 1)] {
                let s = mosaic(2, 2, 0.0, &[(x, y, v)]);
                let codes = clip_codes(&s, 2, 2, &p, GAINS, 1.0);
                assert_eq!(codes.codes, vec![want], "channel {c} at {v}");
            }
        }
    }

    /// With headroom the clip moves up with it: what clipped at the
    /// white is now light, and only a photosite near the saturation
    /// clips. The clips over the ceiling do not move.
    #[test]
    fn headroom_raises_the_threshold_and_leaves_the_clips() {
        let p = CfaPattern::rggb();
        let headroom = 1.25;
        let at = GAINS[0] * headroom * CLIP_MAGIC;
        let cases = [
            (GAINS[0], 0),
            (at.next_down(), 0),
            (at, 1),
            (GAINS[0] * headroom, 1),
        ];
        for (v, want) in cases {
            let s = mosaic(2, 2, 0.0, &[(0, 0, v)]);
            let codes = clip_codes(&s, 2, 2, &p, GAINS, headroom);
            assert_eq!(codes.codes, vec![want], "red at {v}");
            assert_eq!(codes.clips, clip_codes(&s, 2, 2, &p, GAINS, 1.0).clips);
        }
        // In the fill's units, the picture over the ceiling times the
        // headroom, opposed's clip reads as the clips say.
        let ceiling = 2.0;
        let codes = clip_codes(&[0.0; 4], 2, 2, &p, GAINS, headroom);
        for c in 0..3 {
            let read = GAINS[c] * headroom * CLIP_MAGIC / (ceiling * headroom);
            assert!((codes.clips[c] - read).abs() < 1e-6);
        }
        assert_eq!(
            codes.clips,
            [CLIP_MAGIC, 0.5 * CLIP_MAGIC, 0.75 * CLIP_MAGIC]
        );
    }

    /// Gains under one take a ceiling of one, as the develop's does.
    #[test]
    fn the_ceiling_is_at_least_one() {
        let codes = clip_codes(&[0.0; 4], 2, 2, &CfaPattern::rggb(), [0.5, 0.25, 0.5], 1.0);
        assert_eq!(
            codes.clips,
            [0.5 * CLIP_MAGIC, 0.25 * CLIP_MAGIC, 0.5 * CLIP_MAGIC]
        );
    }

    /// X-Trans cells are 3x3 from the crop's origin, at any phase: one
    /// clipped through counts the colors it holds, and a lone green is
    /// 1.
    #[test]
    fn xtrans_cells_are_three_by_three() {
        // At every phase a cell holds each color, in counts the phase
        // sets: at (1, 0) one red and three blues.
        let mut counts = std::collections::BTreeSet::new();
        for dy in 0..6 {
            for dx in 0..6 {
                let p = CfaPattern::xtrans().shifted(dx, dy);
                let mut n = [0; 3];
                for y in 0..3 {
                    for x in 0..3 {
                        n[color(&p, x, y)] += 1;
                    }
                }
                counts.insert(n);
            }
        }
        let want = [[1, 5, 3], [2, 5, 2], [3, 5, 1]];
        assert_eq!(counts.into_iter().collect::<Vec<_>>(), want);
        // The lone red of a cell at (1, 0) clipped makes it a 1; its
        // three blues clipped as well, a 2.
        let p = CfaPattern::xtrans().shifted(1, 0);
        let sites: Vec<(usize, usize)> = (0..3).flat_map(|y| (0..3).map(move |x| (x, y))).collect();
        let reds: Vec<_> = sites
            .iter()
            .filter(|&&(x, y)| color(&p, x, y) == 0)
            .collect();
        assert_eq!(reds.len(), 1);
        let mut s = vec![0.1; 9];
        s[reds[0].1 * 3 + reds[0].0] = GAINS[0];
        assert_eq!(clip_codes(&s, 3, 3, &p, GAINS, 1.0).codes, vec![1]);
        for &(x, y) in sites.iter().filter(|&&(x, y)| color(&p, x, y) == 2) {
            s[y * 3 + x] = GAINS[2];
        }
        assert_eq!(clip_codes(&s, 3, 3, &p, GAINS, 1.0).codes, vec![2]);

        let (w, h) = (12, 9);
        for (dx, dy) in [(0, 0), (1, 2), (4, 5)] {
            let p = CfaPattern::xtrans().shifted(dx, dy);
            let mut s = vec![0.1; w * h];
            for y in 3..6 {
                for x in 6..9 {
                    s[y * w + x] = GAINS[color(&p, x, y)];
                }
            }
            let g = (0..3)
                .flat_map(|y| (0..3).map(move |x| (x, y)))
                .find(|&(x, y)| color(&p, x, y) == 1)
                .unwrap();
            s[g.1 * w + g.0] = 1.0;
            let codes = clip_codes(&s, w, h, &p, GAINS, 1.0);
            assert_eq!((codes.cell, codes.cells_wide, codes.cells_high), (3, 4, 3));
            let mut want = vec![0u8; 12];
            want[0] = 1;
            want[4 + 2] = colors_in(&p, (6, 9), (3, 6));
            if (dx, dy) == (0, 0) {
                assert_eq!(want[6], 3);
            }
            assert_eq!(codes.codes, want, "phase {dx},{dy}");
        }
    }

    /// A frame whose sides are not a multiple of the cell's has partial
    /// cells at its right and bottom, which count what they hold.
    #[test]
    fn partial_edge_cells_count_what_they_hold() {
        let p = CfaPattern::rggb();
        let (w, h) = (5, 3);
        // Column 4 is red and green; row 2 is red and green.
        let s = mosaic(
            w,
            h,
            0.1,
            &[(4, 0, 2.0), (4, 1, 1.0), (2, 2, 2.0), (4, 2, 2.0)],
        );
        let codes = clip_codes(&s, w, h, &p, GAINS, 1.0);
        assert_eq!((codes.cells_wide, codes.cells_high), (3, 2));
        assert_eq!(codes.codes, vec![0, 0, 2, 0, 1, 1]);
        assert_eq!(codes.at(4, 2), 1);

        let p = CfaPattern::xtrans();
        let (w, h) = (7, 4);
        let s: Vec<f32> = (0..w * h).map(|i| GAINS[color(&p, i % w, i / w)]).collect();
        let codes = clip_codes(&s, w, h, &p, GAINS, 1.0);
        assert_eq!((codes.cells_wide, codes.cells_high), (3, 2));
        // The one-photosite column and the one-photosite row are partial
        // cells, the corner a single photosite.
        let want: Vec<u8> = (0..2)
            .flat_map(|y| (0..3).map(move |x| (x, y)))
            .map(|(x, y)| colors_in(&p, (x * 3, (x * 3 + 3).min(w)), (y * 3, (y * 3 + 3).min(h))))
            .collect();
        assert_eq!(codes.codes, want);
        assert_eq!(codes.at(6, 3), 1);
    }

    /// Reading a pixel outside the mosaic the codes cover (a binned
    /// picture's, say) is caught in a debug build.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "outside the 5x3 mosaic")]
    fn reading_outside_the_mosaic_is_caught() {
        let codes = clip_codes(&[0.0; 15], 5, 3, &CfaPattern::rggb(), GAINS, 1.0);
        assert_eq!(codes.at(4, 2), 0);
        // In the last cell's span, past the frame.
        codes.at(5, 2);
    }

    /// An empty frame has no cells; a filter of no RGB color counts
    /// for none.
    #[test]
    fn empty_frames_and_other_filters() {
        let codes = clip_codes(&[], 0, 0, &CfaPattern::rggb(), GAINS, 1.0);
        assert!(codes.codes.is_empty());
        let p = CfaPattern::new(
            2,
            2,
            vec![
                CfaColor::Red,
                CfaColor::Other(4),
                CfaColor::Other(4),
                CfaColor::Blue,
            ],
        )
        .unwrap();
        assert_eq!(clip_codes(&[9.0; 4], 2, 2, &p, GAINS, 1.0).codes, vec![2]);
    }
}
