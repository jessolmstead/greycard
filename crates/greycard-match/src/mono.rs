//! Monochrome sets: frames whose camera JPEGs carry no color, a
//! maker's Monochrome or Monotone style. Told from the data, not the
//! style's name, since the names differ by maker and a user may fit
//! any folder: a set is monochrome when nearly every block of its
//! camera side is grey while its develop side shows color.
//!
//! A monochrome set says nothing about what a saturated color should
//! become, only that it becomes a grey. The color fit's priors (the
//! matrix's ridge toward identity, the table's pull toward the matrix
//! and curves) would fill that silence with the develop's own color,
//! so a red no frame showed came out red. A monochrome set is fitted
//! as a grey instead ([`crate::Model::monochrome`]).

use crate::color::{decode3, oklab};

/// The share of a set's camera blocks under [`MONO_CHROMA`] for the set
/// to be monochrome, as a percentile: a few stray blocks (a registration
/// edge, a speck of chroma noise) do not make a color set of a grey one,
/// nor a grey set of a color one.
pub const MONO_PERCENTILE: usize = 99;

/// The Oklab chroma the camera side's [`MONO_PERCENTILE`]th percentile
/// must be under for a set to be monochrome. Measured on the blocks of
/// five color frames of five makers, their camera JPEGs made grey and
/// encoded again as JPEG at quality 92: a neutral grey scores 0.0000, a grey
/// one level of Cb and Cr off neutral 0.0062 to 0.0094, two levels
/// 0.013 to 0.019; a weak sepia (four levels at mid-grey) 0.0117 to
/// 0.0120, a strong one 0.032 to 0.033; the frames in color 0.045 to
/// 0.10. It sits between the one-level cast and the weak sepia, and
/// under a just-visible step (0.02): what a monochrome fit drops is
/// never more than that at the percentile.
pub const MONO_CHROMA: f32 = 0.0105;

/// The [`MONO_PERCENTILE`]th percentile of the Oklab chroma of encoded
/// colors, nearest rank; none for none.
pub fn chroma_percentile(y_enc: &[[f32; 3]]) -> Option<f32> {
    if y_enc.is_empty() {
        return None;
    }
    let mut c: Vec<f32> = y_enc
        .iter()
        .map(|y| {
            let l = oklab(decode3(*y));
            l[1].hypot(l[2])
        })
        .collect();
    c.sort_by(f32::total_cmp);
    let rank = (c.len() * MONO_PERCENTILE).div_ceil(100).max(1);
    Some(c[rank - 1])
}

/// The Oklab chroma the develop side's [`MONO_PERCENTILE`]th percentile
/// must be over for a set to be monochrome: the develop shows color the
/// camera dropped. A color style shot on scenes with next to no color
/// (fog, snow, copy work of prints) is grey on both sides, and fitted as
/// a grey it would grey every later frame of that style; such a set
/// takes the color fit. The develop sides measured: 0.044 to 0.106 on
/// five frames of five bodies (0.106 a portrait), 0.16 on the OM-1's
/// Monotone scene, 0.066 on a synthetic portrait shoot of skin and
/// colors muted to a fifth. The bound sits under the least of them with
/// margin, and over twice [`MONO_CHROMA`].
pub const DEVELOP_CHROMA: f32 = 0.025;

/// What [`is_monochrome`] reads of a set: the [`MONO_PERCENTILE`]th
/// percentile of the camera side's chroma and of the develop side's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub camera: Option<f32>,
    pub develop: Option<f32>,
}

impl Reading {
    /// The two sides of a set, encoded.
    pub fn of(x_enc: &[[f32; 3]], y_enc: &[[f32; 3]]) -> Reading {
        Reading {
            camera: chroma_percentile(y_enc),
            develop: chroma_percentile(x_enc),
        }
    }

    /// Grey on the camera side, in color on the develop side.
    pub fn monochrome(&self) -> bool {
        self.camera.is_some_and(|c| c < MONO_CHROMA)
            && self.develop.is_some_and(|d| d > DEVELOP_CHROMA)
    }

    /// Either side within a factor of two of its bound, where a
    /// misreading would start: worth a line in a log.
    pub fn near(&self) -> bool {
        let close =
            |v: Option<f32>, bound: f32| v.is_some_and(|v| v > bound / 2.0 && v < bound * 2.0);
        close(self.camera, MONO_CHROMA) || close(self.develop, DEVELOP_CHROMA)
    }
}

/// Whether a set is monochrome: its camera side, encoded, grey, and
/// its develop side not ([`Reading::monochrome`]).
pub fn is_monochrome(x_enc: &[[f32; 3]], y_enc: &[[f32; 3]]) -> bool {
    Reading::of(x_enc, y_enc).monochrome()
}

/// The linear grey whose Oklab lightness is `l`. Oklab's lightness of
/// a grey `g` is `cbrt(g)` times white's, so this is exact, and its
/// output is R = G = B.
pub fn grey(l: f32) -> f32 {
    let white = oklab([1.0; 3])[0];
    let t = (l / white).max(0.0);
    t * t * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::encode3;

    /// The camera side alone, with a develop side in full color.
    fn camera_grey(y: &[[f32; 3]]) -> bool {
        Reading {
            camera: chroma_percentile(y),
            develop: Some(0.2),
        }
        .monochrome()
    }

    /// The guard: a set grey on both sides (fog, snow, copy work) is
    /// not monochrome; grey on the camera side and in color on the
    /// develop side is; the bound's edges, and nothing on either side.
    #[test]
    fn a_set_grey_on_both_sides_is_not_monochrome() {
        let reading = |camera, develop| Reading {
            camera: Some(camera),
            develop: Some(develop),
        };
        assert!(reading(0.0, 0.16).monochrome());
        assert!(reading(0.0, DEVELOP_CHROMA + 0.001).monochrome());
        assert!(!reading(0.0, DEVELOP_CHROMA).monochrome());
        assert!(!reading(0.0, 0.012).monochrome());
        assert!(!reading(0.02, 0.16).monochrome());
        assert!(
            !Reading {
                camera: Some(0.0),
                develop: None
            }
            .monochrome()
        );
        // The same grey blocks on both sides: a foggy color shoot.
        let fog: Vec<[f32; 3]> = (0..400).map(|i| [0.4 + i as f32 / 1000.0; 3]).collect();
        assert!(!is_monochrome(&fog, &fog));
        // Near: within a factor of two of either bound.
        assert!(reading(0.008, 0.16).near());
        assert!(reading(0.0, 0.03).near());
        assert!(!reading(0.0, 0.16).near());
        assert!(!reading(0.05, 0.16).near());
    }

    /// A JPEG's grey: luma `y` (0 to 255) with `cb` and `cr` levels
    /// off neutral, BT.601 full range as JFIF has it, encoded sRGB out.
    fn ycc(y: f32, cb: f32, cr: f32) -> [f32; 3] {
        [
            y + 1.402 * cr,
            y - 0.344_136 * cb - 0.714_136 * cr,
            y + 1.772 * cb,
        ]
        .map(|v| (v / 255.0).clamp(0.0, 1.0))
    }

    #[test]
    fn grey_inverts_oklab_lightness_and_is_neutral() {
        for g in [0.0, 0.001, 0.05, 0.18, 0.5, 1.0] {
            let l = oklab([g; 3])[0];
            assert!((grey(l) - g).abs() < 1e-5, "{g}: {}", grey(l));
        }
        assert_eq!(grey(-0.1), 0.0);
    }

    #[test]
    fn nothing_is_not_monochrome() {
        assert_eq!(chroma_percentile(&[]), None);
        assert!(!is_monochrome(&[], &[]));
    }

    /// The percentile is the nearest rank: one block in a hundred may
    /// be anything, two may not.
    #[test]
    fn the_percentile_leaves_one_block_in_a_hundred() {
        let grey = [0.5; 3];
        let red = [0.9, 0.2, 0.2];
        let mut set = vec![grey; 99];
        set.push(red);
        assert!(camera_grey(&set));
        set[0] = red;
        assert!(!camera_grey(&set));
    }

    /// The threshold's edges: neutral greys and greys a level or two
    /// of Cb and Cr off neutral are monochrome; a cast of three levels,
    /// a weak sepia and a strong one are not; nor is a muted color set.
    #[test]
    fn the_threshold_sits_between_a_greys_cast_and_a_sepia() {
        let ramp = |cb: f32, cr: f32| -> Vec<[f32; 3]> {
            (8..248).map(|y| ycc(y as f32, cb, cr)).collect()
        };
        let p = |s: &[[f32; 3]]| chroma_percentile(s).unwrap();
        assert!(p(&ramp(0.0, 0.0)) < 1e-5);
        assert!(camera_grey(&ramp(0.0, 0.0)));
        for (cb, cr) in [(1.0, 1.0), (-1.0, 1.0), (1.0, 0.0), (0.0, -1.0)] {
            assert!(
                camera_grey(&ramp(cb, cr)),
                "{cb} {cr}: {}",
                p(&ramp(cb, cr))
            );
        }
        for (cb, cr) in [(2.0, 2.0), (3.0, 3.0), (-3.0, 3.0)] {
            assert!(
                !camera_grey(&ramp(cb, cr)),
                "{cb} {cr}: {}",
                p(&ramp(cb, cr))
            );
        }
        // Sepia: Cb and Cr off neutral in proportion to how far a
        // grey is from both black and white, warm at the middle.
        let sepia = |k: f32| -> Vec<[f32; 3]> {
            (8..248)
                .map(|y| {
                    let t = y as f32 / 255.0;
                    let s = 4.0 * t * (1.0 - t) * k;
                    ycc(y as f32, -s, 0.6 * s)
                })
                .collect()
        };
        assert!(!camera_grey(&sepia(4.0)), "weak sepia {}", p(&sepia(4.0)));
        assert!(!camera_grey(&sepia(12.0)), "strong {}", p(&sepia(12.0)));
        // A muted color set: every hue at a tenth of full chroma.
        let muted: Vec<[f32; 3]> = (0..600)
            .map(|i| {
                let h = i as f32 * 0.618_034 * std::f32::consts::TAU;
                let l = 0.3 + 0.6 * ((i * 7) % 100) as f32 / 100.0;
                encode3(crate::color::from_oklab([
                    l,
                    0.03 * h.cos(),
                    0.03 * h.sin(),
                ]))
            })
            .collect();
        assert!(!camera_grey(&muted), "muted {}", p(&muted));
    }
}
