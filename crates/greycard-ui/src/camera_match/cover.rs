//! The coverage pick: a group's frames chosen for the colors they
//! show the fit, from the camera's JPEG alone, before anything is
//! developed.
//!
//! The table is a lattice over the encoded color, and a node no block
//! reaches is held by the smoothness and the pull, not by data. So a
//! frame is worth developing for the cells of that lattice it fills
//! that the frames already picked have not. Each candidate's camera
//! JPEG is cut into the fit's blocks at about the develop's size, the
//! flat, unclipped ones (the cuts the pairs make, on the camera's side
//! alone) counted into the lattice's cells, coarsened four to one; and
//! the pick is greedy: the frame that fills most of what is still
//! under-filled, with the cells weighted by how often the candidates
//! show them or evenly.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use greycard_match::color::luminance;
use greycard_match::pairs::{CLIP_HIGH, CLIP_LOW, TEXTURE_MAX, block_stats};
use greycard_match::{BLOCK, LUT_SIZE, Picture};
use serde::{Deserialize, Serialize};

use super::{Frame, LONG_EDGE};

/// Cells per axis: the table's lattice, its 32 cells a side taken four
/// to one.
pub(crate) const CELLS: usize = (LUT_SIZE - 1) / 4;
/// Blocks a cell holds before it counts as filled: about one a node of
/// the 4³ the coarse cell spans.
pub(crate) const FILL: f32 = 64.0;
/// A frame with more of its pixels clipped than this is passed over
/// while any other is left.
pub(crate) const CLIPPED_MAX: f32 = 0.05;
/// A lens with this many candidates gets at least one frame picked.
pub(crate) const LENS_MIN: usize = 3;

/// What a frame's camera JPEG shows the fit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Seen {
    /// Flat, unclipped blocks per cell, the cell's index red fastest.
    pub(crate) cells: Vec<(u16, u32)>,
    /// The share of pixels at or over [`CLIP_HIGH`] in any channel.
    pub(crate) clipped: f32,
}

/// The cell an encoded color falls in.
pub(crate) fn cell_of(v: [f32; 3]) -> u16 {
    let i = v.map(|c| ((c.clamp(0.0, 1.0) * CELLS as f32) as usize).min(CELLS - 1));
    (i[0] + CELLS * (i[1] + CELLS * i[2])) as u16
}

/// [`Seen`] of a picture already at about the develop's size.
pub(crate) fn seen_of_picture(p: &Picture) -> Seen {
    let clipped = p
        .data
        .iter()
        .filter(|v| v[0].max(v[1]).max(v[2]) >= CLIP_HIGH)
        .count() as f32
        / p.data.len().max(1) as f32;
    let (means, stds, _, _) = block_stats(p, BLOCK);
    let mut cells: BTreeMap<u16, u32> = BTreeMap::new();
    for (m, s) in means.iter().zip(&stds) {
        let max = m[0].max(m[1]).max(m[2]);
        let min = m[0].min(m[1]).min(m[2]);
        if luminance(*s) < TEXTURE_MAX && max < CLIP_HIGH && min > CLIP_LOW {
            *cells.entry(cell_of(*m)).or_default() += 1;
        }
    }
    Seen {
        cells: cells.into_iter().collect(),
        clipped,
    }
}

/// [`Seen`] of the camera's JPEG in a raw, box-averaged by a whole
/// factor to about [`LONG_EDGE`], so its blocks are the develop's.
pub(crate) fn seen_of(path: &Path) -> Result<Seen, String> {
    let (rgb, _) = greycard_core::decode::preview_path(path)
        .map_err(|e| e.to_string())?
        .ok_or("the file carries no camera JPEG")?;
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let k = ((w.max(h) as f32 / LONG_EDGE as f32).round() as usize).max(1);
    let (sw, sh) = (w / k, h / k);
    let raw = rgb.as_raw();
    let scale = 1.0 / (255.0 * (k * k) as f32);
    let mut data = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let mut sum = [0u32; 3];
            for yy in y * k..(y + 1) * k {
                let row = &raw[(yy * w + x * k) * 3..(yy * w + (x + 1) * k) * 3];
                for px in row.as_chunks::<3>().0 {
                    for (acc, &v) in sum.iter_mut().zip(px) {
                        *acc += v as u32;
                    }
                }
            }
            data.push(sum.map(|s| s as f32 * scale));
        }
    }
    Ok(seen_of_picture(&Picture::new(sw, sh, data)))
}

/// One frame of a coverage pick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Step {
    /// Its index in the candidates.
    pub(crate) frame: usize,
    /// The share of the whole coverage it added.
    pub(crate) gain: f32,
    /// Taken for its lens rather than its gain.
    pub(crate) forced: bool,
}

/// A coverage pick, in the order picked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Covered {
    pub(crate) steps: Vec<Step>,
    /// Candidates passed over for clipping.
    pub(crate) clipped: usize,
    /// Lenses with [`LENS_MIN`] candidates or more.
    pub(crate) lenses: usize,
}

/// Which candidates the pick takes while any is left: a JPEG that
/// read, clipped no more than [`CLIPPED_MAX`].
fn eligible(seen: &[Option<&Seen>]) -> Vec<bool> {
    seen.iter()
        .map(|s| s.is_some_and(|s| s.clipped <= CLIPPED_MAX))
        .collect()
}

/// Each cell's weight and the blocks that fill it, over the eligible
/// candidates, and the weights' sum, which is the whole coverage.
struct Weights {
    weight: Vec<f32>,
    cap: Vec<f32>,
    total: f32,
}

fn weights(seen: &[Option<&Seen>], eligible: &[bool], even: bool) -> Weights {
    let cells = CELLS * CELLS * CELLS;
    let (mut demand, mut shown) = (vec![0f32; cells], vec![0u32; cells]);
    for s in seen
        .iter()
        .zip(eligible)
        .filter(|(_, e)| **e)
        .filter_map(|(s, _)| *s)
    {
        for &(c, n) in &s.cells {
            demand[c as usize] += n as f32;
            shown[c as usize] += 1;
        }
    }
    let count = eligible.iter().filter(|e| **e).count().max(1) as f32;
    let weight: Vec<f32> = (0..cells)
        .map(|c| match (shown[c] >= 2, even) {
            (false, _) => 0.0,
            (true, true) => 1.0,
            (true, false) => (demand[c] / count).sqrt(),
        })
        .collect();
    let cap = demand.iter().map(|d| d.min(FILL)).collect();
    let total = weight.iter().sum();
    Weights { weight, cap, total }
}

/// The share of the whole coverage (as [`cover`] weighs it) that the
/// frames `picked` reach, however they were picked.
pub(crate) fn reach(seen: &[Option<&Seen>], picked: &[usize], even: bool) -> f32 {
    let Weights { weight, cap, total } = weights(seen, &eligible(seen), even);
    let mut filled = vec![0f32; weight.len()];
    for s in picked.iter().filter_map(|&i| seen[i]) {
        for &(c, n) in &s.cells {
            filled[c as usize] += n as f32;
        }
    }
    let got: f32 = (0..weight.len())
        .filter(|&c| weight[c] > 0.0)
        .map(|c| weight[c] * filled[c].min(cap[c]) / cap[c])
        .sum();
    if total > 0.0 { got / total } else { 0.0 }
}

/// Up to `want` of `frames`, greedily for coverage ([`Seen`] for each,
/// `None` where the JPEG would not read): each step takes the frame
/// that fills the most of what is still under-filled, a cell's worth
/// its weight once it holds [`FILL`] blocks (or every block the
/// candidates have of it); the weight is the square root of how often
/// the candidates show the cell, or one for every cell (`even`), a
/// cell only one frame shows counting for nothing. A frame clipped
/// over [`CLIPPED_MAX`] is passed over while another is left, and when
/// the picks left are no more than the lenses with [`LENS_MIN`]
/// candidates not picked yet, the pick is from those lenses' frames.
/// Ties go to the frame from the folder picked least, then the
/// earliest.
pub(crate) fn cover(frames: &[Frame], seen: &[Option<&Seen>], want: usize, even: bool) -> Covered {
    assert_eq!(frames.len(), seen.len());
    let eligible = eligible(seen);
    let clipped = seen
        .iter()
        .filter(|s| s.is_some_and(|s| s.clipped > CLIPPED_MAX))
        .count();
    let Weights { weight, cap, total } = weights(seen, &eligible, even);
    let lens_of = |i: usize| frames[i].lens.clone().unwrap_or_default();
    let mut per_lens: HashMap<String, usize> = HashMap::new();
    for i in (0..frames.len()).filter(|&i| eligible[i]) {
        *per_lens.entry(lens_of(i)).or_default() += 1;
    }
    let mut owed: BTreeSet<String> = per_lens
        .into_iter()
        .filter(|(l, n)| *n >= LENS_MIN && !l.is_empty())
        .map(|(l, _)| l)
        .collect();
    let lenses = owed.len();
    let mut filled = vec![0f32; weight.len()];
    let mut picked = vec![false; frames.len()];
    let mut per_folder: HashMap<&Path, usize> = HashMap::new();
    let mut steps = Vec::new();
    let gain_of = |i: usize, filled: &[f32]| -> f32 {
        let Some(s) = seen[i] else { return 0.0 };
        s.cells
            .iter()
            .map(|&(c, n)| {
                let c = c as usize;
                if weight[c] == 0.0 {
                    return 0.0;
                }
                let before = filled[c].min(cap[c]);
                let after = (filled[c] + n as f32).min(cap[c]);
                weight[c] * (after - before) / cap[c]
            })
            .sum()
    };
    while steps.len() < want.min(frames.len()) {
        let left = want - steps.len();
        let forced = !owed.is_empty() && owed.len() >= left;
        let open = |i: &usize| !picked[*i];
        let mut pool: Vec<usize> = (0..frames.len())
            .filter(open)
            .filter(|&i| eligible[i])
            .filter(|&i| !forced || owed.contains(&lens_of(i)))
            .collect();
        if pool.is_empty() {
            pool = (0..frames.len()).filter(open).collect();
        }
        let best = pool
            .into_iter()
            .map(|i| (i, gain_of(i, &filled)))
            .min_by(|a, b| {
                b.1.total_cmp(&a.1)
                    .then_with(|| {
                        let fa = per_folder.get(frames[a.0].folder()).unwrap_or(&0);
                        let fb = per_folder.get(frames[b.0].folder()).unwrap_or(&0);
                        fa.cmp(fb)
                    })
                    .then(a.0.cmp(&b.0))
            });
        let Some((i, gain)) = best else { break };
        picked[i] = true;
        if let Some(s) = seen[i] {
            for &(c, n) in &s.cells {
                filled[c as usize] += n as f32;
            }
        }
        owed.remove(&lens_of(i));
        *per_folder.entry(frames[i].folder()).or_default() += 1;
        steps.push(Step {
            frame: i,
            gain: if total > 0.0 { gain / total } else { 0.0 },
            forced,
        });
    }
    Covered {
        steps,
        clipped,
        lenses,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn frame(folder: &str, i: usize, lens: &str) -> Frame {
        Frame {
            path: PathBuf::from(format!("/photos/{folder}/IMG_{i:04}.CR3")),
            taken: Some(format!("2026-01-01 00:00:{i:02}")),
            lens: Some(lens.into()),
            fixed: true,
        }
    }

    fn seen(cells: &[(u16, u32)], clipped: f32) -> Seen {
        Seen {
            cells: cells.to_vec(),
            clipped,
        }
    }

    fn picked(c: &Covered) -> Vec<usize> {
        c.steps.iter().map(|s| s.frame).collect()
    }

    #[test]
    fn the_pick_fills_the_cells_the_others_left_empty() {
        let frames: Vec<Frame> = (0..4).map(|i| frame("a", i, "L")).collect();
        let s = [
            seen(&[(1, 100)], 0.0),
            seen(&[(1, 100)], 0.0),
            seen(&[(2, 30)], 0.0),
            seen(&[(1, 10), (2, 40)], 0.0),
        ];
        let refs: Vec<Option<&Seen>> = s.iter().map(Some).collect();
        // The first frame fills cell 1; the second is for cell 2, not
        // the twin of the first.
        for even in [false, true] {
            let c = cover(&frames, &refs, 2, even);
            assert_eq!(picked(&c), vec![0, 3], "{c:?}");
            assert!(c.steps[0].gain >= c.steps[1].gain, "{c:?}");
        }
    }

    #[test]
    fn a_frame_adding_nothing_gains_nothing() {
        let frames: Vec<Frame> = (0..3).map(|i| frame("a", i, "L")).collect();
        let s = [
            seen(&[(5, 200)], 0.0),
            seen(&[(5, 200)], 0.0),
            seen(&[(5, 200)], 0.0),
        ];
        let refs: Vec<Option<&Seen>> = s.iter().map(Some).collect();
        let c = cover(&frames, &refs, 3, false);
        assert!((c.steps[0].gain - 1.0).abs() < 1e-6, "{c:?}");
        assert_eq!(c.steps[1].gain, 0.0);
        assert_eq!(c.steps[2].gain, 0.0);
    }

    #[test]
    fn every_lens_with_three_frames_gets_one() {
        let mut frames: Vec<Frame> = (0..5).map(|i| frame("a", i, "Wide")).collect();
        frames.extend((5..8).map(|i| frame("a", i, "Long")));
        frames.push(frame("a", 8, "Odd"));
        let s: Vec<Seen> = (0..9)
            .map(|i| {
                if i < 5 {
                    seen(&[(i as u16 % 2, 100), (40, 100)], 0.0)
                } else {
                    seen(&[(40, 100)], 0.0)
                }
            })
            .collect();
        let refs: Vec<Option<&Seen>> = s.iter().map(Some).collect();
        let c = cover(&frames, &refs, 3, false);
        assert_eq!(c.lenses, 2, "the one-frame lens is owed nothing");
        let lenses: Vec<&str> = picked(&c)
            .iter()
            .map(|&i| frames[i].lens.as_deref().unwrap())
            .collect();
        assert!(lenses.contains(&"Long"), "{lenses:?}");
        assert!(!lenses.contains(&"Odd"), "{lenses:?}");
        assert!(c.steps.last().unwrap().forced);
        assert!(!c.steps[0].forced);
    }

    #[test]
    fn a_clipped_frame_waits_until_nothing_else_is_left() {
        let frames: Vec<Frame> = (0..3).map(|i| frame("a", i, "L")).collect();
        let s = [
            seen(&[(1, 100), (2, 100), (3, 100)], 0.2),
            seen(&[(1, 100), (2, 100), (3, 100)], 0.0),
            seen(&[(1, 20), (2, 20), (3, 20)], 0.01),
        ];
        let refs: Vec<Option<&Seen>> = s.iter().map(Some).collect();
        let c = cover(&frames, &refs, 3, false);
        assert_eq!(c.clipped, 1);
        assert_eq!(picked(&c), vec![1, 2, 0]);
    }

    #[test]
    fn a_tie_goes_to_the_folder_picked_least() {
        let frames = vec![frame("a", 0, "L"), frame("a", 1, "L"), frame("b", 2, "L")];
        let s = [
            seen(&[(1, 100)], 0.0),
            seen(&[(1, 100)], 0.0),
            seen(&[(1, 100)], 0.0),
        ];
        let refs: Vec<Option<&Seen>> = s.iter().map(Some).collect();
        let c = cover(&frames, &refs, 2, false);
        assert_eq!(picked(&c), vec![0, 2]);
    }

    #[test]
    fn a_picture_is_binned_by_its_flat_unclipped_blocks() {
        // Four blocks: flat mid grey, flat red, white (clipped) and a
        // checkerboard (texture).
        let (w, h) = (BLOCK * 4, BLOCK);
        let mut p = Picture::filled(w, h, [0.5; 3]);
        for y in 0..h {
            for x in 0..w {
                p.data[y * w + x] = match x / BLOCK {
                    0 => [0.5; 3],
                    1 => [0.8, 0.2, 0.2],
                    2 => [1.0; 3],
                    _ => [if (x + y) % 2 == 0 { 0.1 } else { 0.9 }; 3],
                };
            }
        }
        let s = seen_of_picture(&p);
        assert!((s.clipped - 0.25).abs() < 1e-6, "{s:?}");
        let mut want = vec![(cell_of([0.5; 3]), 1), (cell_of([0.8, 0.2, 0.2]), 1)];
        want.sort();
        assert_eq!(s.cells, want);
    }
}
