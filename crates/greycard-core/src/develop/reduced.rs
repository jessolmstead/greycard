//! The reduced develop: the mosaic binned into a linear picture at half
//! the size, in place of the demosaic.
//!
//! A 2x2 bin of a Bayer mosaic holds one sample of each color (two of
//! green), so a cell is one RGB pixel with no demosaic at all: no
//! direction to choose, no aliasing to blend away, a quarter of the
//! pixels. A 24 MP frame comes out about 3000 wide, which is enough for
//! a 2048 picture and every thumbnail shrunk from it, for a tenth of
//! the full develop's time past the decode.
//!
//! Everything before the demosaic runs on the mosaic at full size, as
//! [`super::prepare`] has it: the levels, the noise measured, the hot
//! pixels repaired, the gains, the highlights reconstructed, the
//! chromatic aberration corrected. Binning first would read a clipped
//! green with an unclipped one as a value under the clip point, and the
//! CA correction reads the mosaic's own channels, so neither can go
//! after the bin. The matrix, the profile's map and the orientation
//! ([`super::finish`]) are per pixel and run on the binned picture as
//! they would on the full one.
//!
//! What this cannot stand in for is said by the caller: the capture
//! sharpen and the denoisers are measured on the mosaic's own pixels and
//! have no meaning at half size, so a consumer leaves them out of the
//! settings and out of its reference alike.
//!
//! An X-Trans mosaic is binned three photosites each way
//! ([`XTRANS_BIN`]): its 2x2 cells include ones of four greens, and its
//! 3x3 cells hold every color at any phase of the pattern. Its picture
//! is a third of the size, which a frame of 26 MP and up still takes to
//! 2048 on its long edge.

use rayon::prelude::*;

use super::{CaCorrector, DevelopSettings, Developed, Prepared, demosaic_prepared, finish};
use crate::error::Result;
use crate::raw::{CfaPattern, Orientation, RawFrame};

/// The bin: two photosites each way, one Bayer cell.
pub const BIN: usize = 2;

/// The bin of an X-Trans mosaic: three photosites each way. Every 3x3
/// window of X-Trans holds every color, five greens and two each of red
/// and blue where it sits on one of the pattern's 3x3 blocks, one and
/// three where it does not, so a cell is one RGB pixel at any phase of
/// the crop. A cell off the blocks has its red and blue a photosite off
/// its center, a third of a binned pixel; at two photosites a cell can
/// be all green.
pub const XTRANS_BIN: usize = 3;

/// A frame whose binned long edge would fall under this is not binned
/// at all: the pictures made from the reduced develop are 2048 on the
/// long edge, and a bin that lands under that would have to be
/// enlarged.
pub const MIN_BINNED_EDGE: usize = 2048;

/// A frame developed reduced: the picture, and what the full develop
/// would have been.
#[derive(Debug)]
pub struct Reduced {
    pub developed: Developed,
    /// The full develop's size, oriented as the picture is: what the
    /// masks, the guide plane and the vignette of a finish are placed
    /// by when they were placed on the full picture.
    pub full: (usize, usize),
    /// The factor the picture is smaller than the full develop each
    /// way: [`BIN`], or 1 for a frame too small to bin.
    pub bin: usize,
}

/// The bin for a Bayer mosaic of this size: [`BIN`] when its binned
/// long edge is at least [`MIN_BINNED_EDGE`], else 1.
pub fn bin_for(width: usize, height: usize) -> usize {
    bin_for_cell(BIN, width, height)
}

/// The smallest bin of `pattern`: [`XTRANS_BIN`] for X-Trans, [`BIN`]
/// for anything else.
pub fn cell_of(pattern: &CfaPattern) -> usize {
    if pattern.is_xtrans() { XTRANS_BIN } else { BIN }
}

/// The bin `cell` for a mosaic of this size when its binned long edge
/// is at least [`MIN_BINNED_EDGE`], else 1.
pub fn bin_for_cell(cell: usize, width: usize, height: usize) -> usize {
    if width.max(height) / cell >= MIN_BINNED_EDGE {
        cell
    } else {
        1
    }
}

/// A bin `pattern` can take: an X-Trans bin past one goes up to a
/// multiple of [`XTRANS_BIN`], so no cell misses a color.
fn fit_bin(pattern: &CfaPattern, bin: usize) -> usize {
    if bin > 1 && pattern.is_xtrans() {
        bin.div_ceil(XTRANS_BIN) * XTRANS_BIN
    } else {
        bin.max(1)
    }
}

/// [`super::develop`] with the mosaic binned by [`bin_for`] in place of
/// the demosaic; `ca` as [`super::develop_with`] takes it. A linear
/// frame, and a mosaic too small to bin, develop as they would in full.
pub fn develop_reduced(
    frame: &RawFrame,
    settings: &DevelopSettings,
    ca: Option<CaCorrector<'_>>,
) -> Result<Reduced> {
    let prepared = super::prepare_with(frame, settings, false, ca)?;
    let bin = match &prepared.pattern {
        Some(p) => bin_for_cell(cell_of(p), prepared.width, prepared.height),
        None => 1,
    };
    develop_prepared_binned(prepared, settings, bin)
}

/// [`develop_reduced`] with the bin given, for a frame of any size: the
/// tests', and a consumer that has decided for itself. An X-Trans
/// mosaic's bin is raised to a multiple of three; [`Reduced::bin`] says
/// the bin it took.
pub fn develop_reduced_by(
    frame: &RawFrame,
    settings: &DevelopSettings,
    ca: Option<CaCorrector<'_>>,
    bin: usize,
) -> Result<Reduced> {
    let prepared = super::prepare_with(frame, settings, false, ca)?;
    let bin = match &prepared.pattern {
        Some(p) => fit_bin(p, bin),
        None => 1,
    };
    develop_prepared_binned(prepared, settings, bin)
}

fn develop_prepared_binned(
    prepared: Prepared,
    settings: &DevelopSettings,
    bin: usize,
) -> Result<Reduced> {
    let full = oriented((prepared.width, prepared.height), prepared.orientation);
    if bin == 1 {
        let (rgb, dual, denoise) = demosaic_prepared(&prepared, settings)?;
        let developed = finish(prepared, rgb, dual, denoise, settings)?;
        return Ok(Reduced {
            developed,
            full,
            bin: 1,
        });
    }
    let pattern = prepared
        .pattern
        .as_ref()
        .expect("a bin past one is a mosaic's");
    let (rgb, width, height) = bin_mosaic(
        &prepared.samples,
        prepared.width,
        prepared.height,
        pattern,
        bin,
    );
    let binned = Prepared {
        samples: Vec::new(),
        width,
        height,
        pattern: None,
        ..prepared
    };
    let developed = finish(binned, rgb, None, None, settings)?;
    Ok(Reduced {
        developed,
        full,
        bin,
    })
}

/// A mosaic's size as its orientation shows it.
fn oriented((w, h): (usize, usize), orientation: Orientation) -> (usize, usize) {
    if matches!(
        orientation,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    ) {
        (h, w)
    } else {
        (w, h)
    }
}

/// A mosaic (one sample a pixel, `pattern` from its origin) binned
/// `bin` photosites each way into interleaved RGB: each output pixel is
/// the mean of the cell's samples of each color. The rows and columns
/// past the last whole cell are left out. Answers the RGB and its size.
///
/// A cell of [`BIN`] holds every color of a 2x2 Bayer pattern once,
/// green twice, whatever the pattern's phase, and a cell of
/// [`XTRANS_BIN`] every color of X-Trans; a larger bin holds each
/// several times. A color a cell has none of (a bin the pattern does
/// not fit) is left at zero.
pub fn bin_mosaic(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    bin: usize,
) -> (Vec<f32>, usize, usize) {
    let bin = bin.max(1);
    let (w, h) = (width / bin, height / bin);
    let mut out = vec![0.0f32; w * h * 3];
    // The pattern is read for every sample: two remainders and an
    // index, which is nothing beside the bin's own reads, and holds
    // for every phase of the pattern and every bin.
    let color = |y: usize, x: usize| pattern.color_at(y, x).rgb_index().unwrap_or(3);
    out.par_chunks_mut(w * 3).enumerate().for_each(|(cy, row)| {
        for (cx, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let mut sum = [0.0f32; 4];
            let mut count = [0u32; 4];
            for dy in 0..bin {
                let y = cy * bin + dy;
                let line = &samples[y * width..(y + 1) * width];
                for dx in 0..bin {
                    let x = cx * bin + dx;
                    let c = color(y, x);
                    sum[c] += line[x];
                    count[c] += 1;
                }
            }
            for c in 0..3 {
                px[c] = if count[c] > 0 {
                    sum[c] / count[c] as f32
                } else {
                    0.0
                };
            }
        }
    });
    (out, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::tests::flat_frame;
    use crate::develop::{DemosaicMethod, demosaic_cfa, develop};
    use crate::raw::{CfaColor, Samples, SensorLayout};

    /// A `width` by `height` RGGB mosaic whose samples are `f(y, x)`,
    /// normalized.
    fn mosaic(width: usize, height: usize, f: impl Fn(usize, usize) -> f32) -> Vec<f32> {
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (y, x)))
            .map(|(y, x)| f(y, x))
            .collect()
    }

    #[test]
    fn a_bin_is_the_mean_of_each_color_in_its_cell() {
        // Every sample its own value: only the right cells can pass.
        let samples = mosaic(6, 4, |y, x| (y * 6 + x) as f32);
        let (rgb, w, h) = bin_mosaic(&samples, 6, 4, &CfaPattern::rggb(), 2);
        assert_eq!((w, h), (3, 2));
        for cy in 0..2 {
            for cx in 0..3 {
                let at = |dy: usize, dx: usize| samples[(cy * 2 + dy) * 6 + cx * 2 + dx];
                let px = &rgb[(cy * 3 + cx) * 3..][..3];
                assert_eq!(px[0], at(0, 0), "red of {cy},{cx}");
                assert_eq!(px[1], (at(0, 1) + at(1, 0)) / 2.0, "green of {cy},{cx}");
                assert_eq!(px[2], at(1, 1), "blue of {cy},{cx}");
            }
        }
    }

    #[test]
    fn a_bin_follows_the_pattern_s_phase() {
        // GRBG, BGGR and GBRG: the cell still holds one of each.
        let samples = mosaic(4, 4, |y, x| 1.0 + (y * 4 + x) as f32);
        for (dx, dy) in [(1, 0), (1, 1), (0, 1)] {
            let pattern = CfaPattern::rggb().shifted(dx, dy);
            let (rgb, _, _) = bin_mosaic(&samples, 4, 4, &pattern, 2);
            for (cell, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
                let (cy, cx) = (cell / 2, cell % 2);
                let mut want = [0.0f32; 3];
                let mut n = [0u32; 3];
                for y in cy * 2..cy * 2 + 2 {
                    for x in cx * 2..cx * 2 + 2 {
                        let c = pattern.color_at(y, x).rgb_index().unwrap();
                        want[c] += samples[y * 4 + x];
                        n[c] += 1;
                    }
                }
                for c in 0..3 {
                    assert!(
                        (px[c] - want[c] / n[c] as f32).abs() < 1e-6,
                        "{pattern:?} cell {cell} channel {c}: {} is not {}",
                        px[c],
                        want[c] / n[c] as f32
                    );
                }
            }
        }
    }

    #[test]
    fn the_odd_edge_is_left_out_and_a_flat_field_stays_flat() {
        let samples = mosaic(7, 5, |y, x| match CfaPattern::rggb().color_at(y, x) {
            CfaColor::Red => 0.5,
            CfaColor::Green => 1.0,
            CfaColor::Blue => 0.25,
            CfaColor::Other(_) => unreachable!(),
        });
        let (rgb, w, h) = bin_mosaic(&samples, 7, 5, &CfaPattern::rggb(), 2);
        assert_eq!((w, h), (3, 2));
        for px in rgb.as_chunks::<3>().0 {
            assert_eq!(px, &[0.5, 1.0, 0.25]);
        }
        // The bilinear demosaic of the same flat field, binned by the
        // mean, is the same picture: the bin is a demosaic for a flat
        // field.
        let full = demosaic_cfa(
            &samples,
            7,
            5,
            &CfaPattern::rggb(),
            DemosaicMethod::Bilinear,
        )
        .unwrap();
        for px in full.as_chunks::<3>().0 {
            assert!((px[0] - 0.5).abs() < 1e-6 && (px[1] - 1.0).abs() < 1e-6);
        }
    }

    /// Every 3x3 cell of X-Trans holds every color at every phase, and
    /// a flat field bins to itself.
    #[test]
    fn an_xtrans_bin_of_three_holds_every_color() {
        let want = [0.5, 1.0, 0.25];
        for dy in 0..6 {
            for dx in 0..6 {
                let pattern = CfaPattern::xtrans().shifted(dx, dy);
                let samples = mosaic(13, 10, |y, x| {
                    want[pattern.color_at(y, x).rgb_index().unwrap()]
                });
                let (rgb, w, h) = bin_mosaic(&samples, 13, 10, &pattern, XTRANS_BIN);
                assert_eq!((w, h), (4, 3));
                for px in rgb.as_chunks::<3>().0 {
                    assert_eq!(px, &want, "phase {dx},{dy}");
                }
            }
        }
        assert_eq!(cell_of(&CfaPattern::xtrans()), 3);
        assert_eq!(cell_of(&CfaPattern::rggb()), 2);
        assert_eq!(bin_for_cell(3, 7728, 5152), 3);
        assert_eq!(bin_for_cell(3, 6143, 4000), 1);
        assert_eq!(fit_bin(&CfaPattern::xtrans(), 2), 3);
        assert_eq!(fit_bin(&CfaPattern::xtrans(), 1), 1);
        assert_eq!(fit_bin(&CfaPattern::rggb(), 2), 2);
    }

    /// The reduced develop of an X-Trans frame is the full one's color
    /// at a third of the size, whatever bin was asked.
    #[test]
    fn an_xtrans_frame_is_binned_by_three() {
        let frame = crate::develop::tests::xtrans_frame();
        let full = develop(&frame, &DevelopSettings::default()).unwrap();
        let reduced = develop_reduced_by(&frame, &DevelopSettings::default(), None, 2).unwrap();
        assert_eq!(reduced.bin, 3);
        assert_eq!(reduced.full, (70, 60));
        let image = &reduced.developed.image;
        assert_eq!((image.width, image.height), (23, 20));
        for px in image.pixels() {
            for (i, v) in px.iter().enumerate() {
                assert!((v - full.image.data[i]).abs() < 1e-4, "channel {i}: {v}");
            }
        }
        // Too small to bin on its own: the full develop.
        let plain = develop_reduced(&frame, &DevelopSettings::default(), None).unwrap();
        assert_eq!(plain.bin, 1);
    }

    #[test]
    fn the_bin_is_two_only_when_half_the_frame_is_still_large() {
        assert_eq!(bin_for(6000, 4000), 2);
        assert_eq!(bin_for(4096, 2731), 2);
        assert_eq!(bin_for(4095, 2731), 1);
        assert_eq!(bin_for(2731, 4096), 2);
        assert_eq!(bin_for(1000, 800), 1);
    }

    /// The reduced develop of the flat test frame is the full develop's
    /// color at half the size: the same levels, gains, highlights and
    /// matrix, with the bin in place of the demosaic.
    #[test]
    fn the_reduced_develop_is_the_full_one_s_color_at_half_the_size() {
        let frame = flat_frame();
        let full = develop(&frame, &DevelopSettings::default()).unwrap();
        let reduced = develop_reduced_by(&frame, &DevelopSettings::default(), None, 2).unwrap();
        assert_eq!(reduced.bin, 2);
        assert_eq!(reduced.full, (4, 4));
        assert_eq!(
            (
                reduced.developed.image.width,
                reduced.developed.image.height
            ),
            (2, 2)
        );
        for px in reduced.developed.image.pixels() {
            for (i, v) in px.iter().enumerate() {
                assert!((v - full.image.data[i]).abs() < 1e-4, "channel {i}: {v}");
            }
        }
        assert_eq!(
            reduced.developed.white_balance.coefficients_f32(),
            full.white_balance.coefficients_f32()
        );
        assert_eq!(reduced.developed.clip_level, full.clip_level);
        // Too small to bin on its own: the full develop.
        let plain = develop_reduced(&frame, &DevelopSettings::default(), None).unwrap();
        assert_eq!(plain.bin, 1);
        assert_eq!(plain.developed.image.data, full.image.data);
    }

    /// The orientation turns the binned picture as it turns the full
    /// one, and `full` is the full picture's oriented size.
    #[test]
    fn the_reduced_develop_is_oriented_and_says_the_full_size() {
        let mut frame = flat_frame();
        frame.width = 6;
        frame.height = 4;
        let pattern = CfaPattern::rggb();
        frame.samples = Samples::U16(
            (0..4)
                .flat_map(|y| (0..6).map(move |x| (y, x)))
                .map(|(y, x)| match pattern.color_at(y, x) {
                    CfaColor::Red => 600u16,
                    CfaColor::Green => 1100,
                    CfaColor::Blue => 350,
                    CfaColor::Other(_) => unreachable!(),
                })
                .collect(),
        );
        frame.layout = SensorLayout::Cfa(pattern);
        frame.orientation = Orientation::Rotate90;
        let reduced = develop_reduced_by(&frame, &DevelopSettings::default(), None, 2).unwrap();
        assert_eq!(reduced.full, (4, 6));
        assert_eq!(
            (
                reduced.developed.image.width,
                reduced.developed.image.height
            ),
            (2, 3)
        );
    }

    /// A linear frame has nothing to bin: it develops as it would in
    /// full, whatever bin is asked.
    #[test]
    fn a_linear_frame_is_not_binned() {
        let mut frame = flat_frame();
        frame.channels = 3;
        frame.layout = SensorLayout::Linear;
        frame.samples = Samples::U16([600u16, 1100, 350].repeat(16));
        frame.levels.black = crate::raw::LevelPattern::uniform(100.0, 3);
        frame.levels.white = crate::raw::LevelPattern::uniform(1100.0, 3);
        let reduced = develop_reduced_by(&frame, &DevelopSettings::default(), None, 2).unwrap();
        assert_eq!(reduced.bin, 1);
        assert_eq!(
            (
                reduced.developed.image.width,
                reduced.developed.image.height
            ),
            (4, 4)
        );
    }
}
