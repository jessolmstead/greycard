//! The merge as it was before it read its frames in passes (issue
//! 134): every frame held at once by the caller, scored, chained, and
//! blended from the slice. Kept verbatim: the merge in `super` places
//! the frames as it does, and blends them as it does where the stack
//! is too short for the stack's prior (`super::prior`) and the spill
//! (`super::spill`) is off, both of which it predates. The bed
//! (`super::bed`) measures the prior against it.

use rayon::prelude::*;

use super::{
    CHANNELS, Dropped, Edge, Error, FrameReport, Kernel, Options, Reference, Result, StackReport,
    Transform, expand, gaussian_pyramid, laplacian_pyramid, luminance_of, pyramid_sizes, register,
    sharpness,
};
use crate::register::Fit;

pub(super) fn merge(
    frames: &[&[f32]],
    width: usize,
    height: usize,
    luma: [f32; 3],
    options: &Options,
) -> Result<(Vec<f32>, StackReport)> {
    for (n, f) in frames.iter().enumerate() {
        if f.len() != width * height * CHANNELS {
            return Err(Error::Unsupported(format!(
                "frame {n} has {} samples, not the {} a {width}x{height} RGB frame has",
                f.len(),
                width * height * CHANNELS
            )));
        }
    }

    // The score every frame gets whatever else happens: it is what
    // picks the reference, and what a caller is shown. One frame's
    // luminance at a time — a plane is a third of a frame, and ten
    // planes of a 45-megapixel stack is nearly two gigabytes held for
    // no reason, since nothing downstream wants more than two of them
    // at once.
    let sharpness: Vec<f32> = frames
        .iter()
        .map(|frame| {
            let lum = luminance_of(frame, luma);
            let log = register::log_plane(&lum, options.register.floor_stops);
            let energy = sharpness::laplacian_energy(&log, width, height);
            (energy.iter().map(|&e| e as f64).sum::<f64>() / energy.len() as f64).sqrt() as f32
        })
        .collect();

    let reference = match options.reference {
        Reference::Middle => frames.len() / 2,
        Reference::Sharpest => sharpness
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(0),
        Reference::Index(i) => {
            if i >= frames.len() {
                return Err(Error::Unsupported(format!(
                    "frame {i} asked for as the reference of a stack of {}",
                    frames.len()
                )));
            }
            i
        }
    };

    let chain = fit_all(frames, width, height, luma, reference, options);

    let reports: Vec<FrameReport> = (0..frames.len())
        .map(|index| FrameReport {
            index,
            reference: index == reference,
            sharpness: sharpness[index],
            against: chain.against[index],
            fit: chain.fits[index],
            transform: chain.onto[index],
            dropped: chain.dropped[index].clone(),
        })
        .collect();

    let sizes = pyramid_sizes(width, height, options);
    let levels = sizes.len();

    // Unnormalized weights, renormalized level by level against the
    // weight that actually landed there. Each frame's full-resolution
    // weight map is carried down its own Gaussian pyramid and used as
    // it comes, and the sum of the weights is accumulated beside the
    // sum of the weighted bands; the division happens once, at the
    // end.
    //
    // This is *not* Mertens's order, which normalizes the maps against
    // each other at full resolution and then builds the pyramids, so
    // that every level's weights already sum to one. The two are not
    // the same thing: the effective weight here is `G(w_i) / sum_j
    // G(w_j)` where Mertens's is `G(w_i / sum_j w_j)`, and a blur of a
    // ratio is not the ratio of blurs. Measured on the synthetic
    // stack, the effective per-frame weights part company by up to
    // 0.18 at levels 1 to 3.
    //
    // It is kept because what the seam argument needs is true of both
    // and the differences do not matter. At level 0 they are
    // identical, the pyramid being the identity there, so the detail
    // is selected exactly as Mertens selects it. At every level the
    // result is a convex combination of the frames' own bands —
    // nonnegative weights over their own sum — so nothing can
    // overshoot and the brightness is a weighted mean of the frames'
    // and not a scaling of it. Both carry the weight map down a
    // pyramid, which is the whole of why a seam is soft. And once
    // `selectivity` is on, the per-level weights are not the pyramid
    // of any full-resolution map at all, so normalizing first is not
    // even defined for them.
    //
    // What it buys is one pass over the frames and one accumulator,
    // instead of every frame's weight map held at once to normalize
    // them against each other.
    let mut bands: Vec<Vec<f32>> = sizes
        .iter()
        .map(|&(w, h)| vec![0.0; w * h * CHANNELS])
        .collect();
    let mut weights: Vec<Vec<f32>> = sizes.iter().map(|&(w, h)| vec![0.0; w * h]).collect();
    // The same bands with no weights on them, for the pixels where
    // every frame's weight is zero. That happens: the weight floor is
    // a fraction of the frame's own mean sharpness, and a region that
    // is flat in every frame has no sharpness for the floor to be a
    // fraction of. Without this, such a region divides zero by nothing
    // and comes out black.
    let mut plain: Vec<Vec<f32>> = sizes
        .iter()
        .map(|&(w, h)| vec![0.0; w * h * CHANNELS])
        .collect();
    let mut merged_frames = 0usize;
    let mut coverage = vec![0u16; width * height];
    let ones = vec![1.0f32; width * height];

    for (index, frame) in frames.iter().enumerate() {
        if reports[index].dropped.is_some() {
            continue;
        }
        let transform = reports[index].transform;
        let identity = transform == Transform::IDENTITY;

        // The frame in the reference's geometry, and where it reached.
        // The picture is warped with the edge held rather than filled,
        // so that what hangs over the side is a continuation and not a
        // black cliff the pyramid would spread down every level; the
        // mask is what actually keeps it out of the merge.
        // Owned, because the Laplacian pyramid takes it as its base
        // rather than copying it: at 45 megapixels that copy is half a
        // gigabyte live beside the pyramid it is being copied into.
        // The reference is the one frame that has to be copied, since
        // the caller's own frame is not the merge's to consume.
        // Lanczos-3, which keeps the detail a stack is for; its ring
        // at a hard edge is held to the frame's own range.
        let warped: Vec<f32> = if identity {
            frame.to_vec()
        } else {
            let mut warped = register::warp_samples(
                frame,
                width,
                height,
                CHANNELS,
                &transform,
                Edge::Clamp,
                Kernel::Lanczos3,
            );
            register::clamp_samples(&mut warped, register::sample_range(frame));
            warped
        };
        let mask = if identity {
            vec![1.0f32; width * height]
        } else {
            register::warp_plane(
                &ones,
                width,
                height,
                width,
                height,
                &transform,
                Edge::Fill(0.0),
                Kernel::Bilinear,
            )
        };
        // A frame covers a pixel when it contributes to it at all,
        // which is the same test its weight makes: the mask multiplies
        // the weight, so anything above zero is in the merge. The
        // bilinear ramp means the outermost row or column of a warped
        // frame counts as covered on a fraction of a sample.
        for (c, m) in coverage.iter_mut().zip(&mask) {
            if *m > 0.0 {
                *c += 1;
            }
        }

        // The same weighting the registration and the frame scores
        // used, so a camera-space stack is measured in camera-space
        // luminance throughout and a working-space one in the working
        // space's.
        let lum = luminance_of(&warped, luma);
        let map = sharpness::map(
            &lum,
            width,
            height,
            options.register.floor_stops,
            options.sharpness_radius,
        );
        drop(lum);

        let mean = (map.iter().map(|&v| v as f64).sum::<f64>() / map.len().max(1) as f64) as f32;
        let floor = mean * options.weight_floor.max(0.0);
        let weight: Vec<f32> = map
            .par_iter()
            .zip(mask.par_iter())
            .map(|(&s, &m)| (s + floor) * m)
            .collect();
        drop(map);
        drop(mask);

        let laplacian = laplacian_pyramid(warped, &sizes, CHANNELS);
        // (`warped` is gone: it is the pyramid's base now.)
        let gaussian = gaussian_pyramid(weight, &sizes);

        for k in 0..levels {
            let (lw, lh) = sizes[k];
            // The top of the pyramid is not a band, it is the
            // picture's own low frequency: nothing to select on, so it
            // is the plain weighted average and the merge keeps its
            // brightness.
            let level_weight: Vec<f32> = if k + 1 == levels || options.selectivity <= 0.0 {
                gaussian[k].clone()
            } else {
                let magnitude: Vec<f32> = laplacian[k]
                    .par_chunks(CHANNELS)
                    .map(|px| px.iter().map(|v| v.abs()).sum::<f32>() / CHANNELS as f32)
                    .collect();
                let contrast = sharpness::box_blur(&magnitude, lw, lh, options.band_radius);
                let floor = options.band_floor;
                // Squaring is the default and `powf` is not cheap over
                // a pyramid a frame.
                let square = options.selectivity == 2.0;
                let exponent = options.selectivity;
                gaussian[k]
                    .par_iter()
                    .zip(contrast.par_iter())
                    .map(|(&g, &c)| {
                        let t = c + floor;
                        g * if square { t * t } else { t.powf(exponent) }
                    })
                    .collect()
            };
            bands[k]
                .par_chunks_mut(CHANNELS)
                .zip(laplacian[k].par_chunks(CHANNELS))
                .zip(level_weight.par_iter())
                .for_each(|((acc, band), &w)| {
                    for (a, b) in acc.iter_mut().zip(band) {
                        *a += w * b;
                    }
                });
            weights[k]
                .par_iter_mut()
                .zip(level_weight.par_iter())
                .for_each(|(s, &w)| *s += w);
            plain[k]
                .par_iter_mut()
                .zip(laplacian[k].par_iter())
                .for_each(|(a, b)| *a += b);
        }
        merged_frames += 1;
    }

    // Normalize each band by the weight that went into it, then
    // collapse from the top. Where no weight went into it at all —
    // every frame flat there — the plain mean of the frames' own bands
    // stands in, which is what a merge with nothing to choose between
    // should give and what dividing by nothing would not.
    let share = 1.0 / merged_frames.max(1) as f32;
    for k in 0..levels {
        bands[k]
            .par_chunks_mut(CHANNELS)
            .zip(weights[k].par_iter())
            .zip(plain[k].par_chunks(CHANNELS))
            .for_each(|((px, &w), flat)| {
                // Below the smallest normal float the ratio is not a
                // ratio any more: the numerator has underflowed with
                // the denominator.
                if w > f32::MIN_POSITIVE {
                    let inv = 1.0 / w;
                    for v in px.iter_mut() {
                        *v *= inv;
                    }
                } else {
                    for (v, f) in px.iter_mut().zip(flat) {
                        *v = f * share;
                    }
                }
            });
    }
    drop(plain);
    let mut out = bands.pop().expect("a pyramid has a top");
    for k in (0..levels - 1).rev() {
        let (w, h) = sizes[k];
        let (cw, ch) = sizes[k + 1];
        let up = expand(&out, cw, ch, w, h, CHANNELS);
        out = bands.pop().expect("one band a level");
        out.par_iter_mut()
            .zip(up.par_iter())
            .for_each(|(o, u)| *o += u);
    }

    Ok((
        out,
        StackReport {
            width,
            height,
            reference,
            frames: reports,
            levels,
            coverage,
            clipped: false,
        },
    ))
}

/// Every frame's link to its neighbor, chained outward from the
/// reference, and those links composed onto the reference.
struct Chain {
    against: Vec<Option<usize>>,
    fits: Vec<Option<Fit>>,
    onto: Vec<Transform>,
    dropped: Vec<Option<Dropped>>,
}

/// Two planes are held at a time and no more: the frame the chain has
/// reached and the one being fitted to it.
fn fit_all(
    frames: &[&[f32]],
    width: usize,
    height: usize,
    luma: [f32; 3],
    reference: usize,
    options: &Options,
) -> Chain {
    let n = frames.len();
    let mut chain = Chain {
        against: vec![None; n],
        fits: vec![None; n],
        onto: vec![Transform::IDENTITY; n],
        dropped: vec![None; n],
    };
    let up: Vec<usize> = (reference + 1..n).collect();
    let down: Vec<usize> = (0..reference).rev().collect();
    for order in [up, down] {
        // The last frame this way whose link held, and the link that
        // got there: one focus step looks much like the next, so the
        // previous answer is most of the next one's.
        let mut anchor = reference;
        let mut anchor_lum = luminance_of(frames[reference], luma);
        let mut seed = Transform::IDENTITY;
        for i in order {
            chain.against[i] = Some(anchor);
            let moving = luminance_of(frames[i], luma);
            let limit = options.max_motion * (width as f64).hypot(height as f64);
            match register::fit_from(
                &anchor_lum,
                &moving,
                width,
                height,
                &options.register,
                &seed,
            ) {
                Ok(fit) => {
                    let moved = fit
                        .transform
                        .corner_distance(&Transform::IDENTITY, width, height);
                    chain.fits[i] = Some(fit);
                    if fit.residual > options.max_residual {
                        chain.dropped[i] = Some(Dropped::NotAligned {
                            residual: fit.residual,
                            overlap: fit.overlap,
                            threshold: options.max_residual,
                        });
                    } else if moved > limit {
                        chain.dropped[i] = Some(Dropped::TooFar {
                            pixels: moved,
                            limit,
                        });
                    } else {
                        chain.onto[i] = fit.transform.compose(&chain.onto[anchor]);
                        seed = fit.transform;
                        anchor = i;
                        anchor_lum = moving;
                    }
                }
                Err(e) => chain.dropped[i] = Some(Dropped::NoFit(e.to_string())),
            }
        }
    }
    chain
}
