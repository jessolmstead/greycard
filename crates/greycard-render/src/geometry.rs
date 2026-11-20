//! The straighten and crop on the CPU: the leveled plane's frame
//! resampled from the working image, Catmull-Rom when turned, plain
//! pixels when not. The reference the shader is held to.

use greycard_core::image::WorkingImage;
use greycard_edit::geometry::Geometry;
use rayon::prelude::*;

/// Catmull-Rom's weights for the four taps about a position `t` (0 to
/// 1) past the second.
#[inline]
fn weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// The image at a source position, pixel centers at halves, edges
/// clamped, Catmull-Rom.
#[inline]
pub fn sample_cubic(image: &WorkingImage, x: f32, y: f32) -> [f32; 3] {
    sample_cubic_of::<3>(&image.data, image.width, image.height, x, y)
}

/// [`sample_cubic`] on `N` interleaved channels of a `width` by
/// `height` plane.
#[inline]
fn sample_cubic_of<const N: usize>(
    data: &[f32],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
) -> [f32; N] {
    let (w, h) = (width as i64, height as i64);
    let fx = (x - 0.5).floor();
    let fy = (y - 0.5).floor();
    let wx = weights(x - 0.5 - fx);
    let wy = weights(y - 0.5 - fy);
    let mut out = [0f32; N];
    for (j, wyj) in wy.iter().enumerate() {
        let sy = (fy as i64 + j as i64 - 1).clamp(0, h - 1) as usize;
        for (i, wxi) in wx.iter().enumerate() {
            let sx = (fx as i64 + i as i64 - 1).clamp(0, w - 1) as usize;
            let k = (sy * width + sx) * N;
            let wgt = wxi * wyj;
            for (c, o) in out.iter_mut().enumerate() {
                *o += wgt * data[k + c];
            }
        }
    }
    out
}

/// The most pixels a frame may have past its source's own: 200 MP,
/// 2.4 GB of three f32 channels before the finish copies it. Twice a
/// panorama's canvas cap and above any still camera's single frame
/// (150 MP the largest), so only a crop dragged far out to a
/// keystone's bounds, which may reach 64 times the source, meets it.
/// The finish frames the masks' detail bands at the same size beside
/// the picture, so the peak is several times one frame. A frame no
/// larger than its source is never refused: the source is already
/// held at that size.
pub const MAX_FRAME_PIXELS: usize = 200_000_000;

/// A frame refused as too large to hold: its size and the most pixels
/// it may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLarge {
    pub size: (usize, usize),
    pub limit: usize,
}

impl std::fmt::Display for TooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.size;
        write!(
            f,
            "a frame of {w} by {h} pixels is more than the {} megapixels one may hold",
            self.limit / 1_000_000
        )
    }
}

impl std::error::Error for TooLarge {}

/// Whether a frame of `size` can be held within `limit` pixels.
pub fn holdable(size: (usize, usize), limit: usize) -> Result<(), TooLarge> {
    match size.0.checked_mul(size.1) {
        Some(pixels) if pixels <= limit => Ok(()),
        _ => Err(TooLarge { size, limit }),
    }
}

/// The most pixels a frame cut from a `width` by `height` source may
/// have: [`MAX_FRAME_PIXELS`], or the source's own when more.
pub fn limit_for(width: usize, height: usize) -> usize {
    MAX_FRAME_PIXELS.max(width.saturating_mul(height))
}

/// The frame of `geometry` cut from `image`. A frame the camera framed
/// comes here with its frame resolved
/// (`Geometry::with_camera_frame`), as every renderer here takes it.
/// A frame past [`limit_for`] the image is refused before any of it is
/// allocated.
pub fn apply(image: &WorkingImage, geometry: &Geometry) -> Result<WorkingImage, TooLarge> {
    if geometry.is_identity_on(image.width as f32, image.height as f32, None) {
        return Ok(image.clone());
    }
    let limit = limit_for(image.width, image.height);
    let (data, width, height) =
        frame_of::<3>(&image.data, image.width, image.height, geometry, limit)?;
    Ok(WorkingImage {
        width,
        height,
        data,
    })
}

/// [`apply`] on a plane of one channel, `width` by `height`: its data,
/// width and height in the frame.
pub fn apply_plane(
    data: &[f32],
    width: usize,
    height: usize,
    geometry: &Geometry,
) -> Result<(Vec<f32>, usize, usize), TooLarge> {
    if geometry.is_identity_on(width as f32, height as f32, None) {
        return Ok((data.to_vec(), width, height));
    }
    frame_of::<1>(data, width, height, geometry, limit_for(width, height))
}

/// The frame of `geometry` cut from `N` interleaved channels, refused
/// past `limit` pixels before it is allocated.
fn frame_of<const N: usize>(
    data: &[f32],
    width: usize,
    height: usize,
    geometry: &Geometry,
    limit: usize,
) -> Result<(Vec<f32>, usize, usize), TooLarge> {
    let (w, h) = (width as f32, height as f32);
    let frame = geometry.frame(w, h, None);
    let (ow, oh) = (frame.size.0 as usize, frame.size.1 as usize);
    holdable((ow, oh), limit)?;
    let mut out = vec![0.0f32; ow * oh * N];
    let turned = geometry.resamples();
    out.par_chunks_mut((ow * N).max(1))
        .enumerate()
        .for_each(|(oy, row)| {
            for (ox, px) in row.as_chunks_mut::<N>().0.iter_mut().enumerate() {
                let r = (
                    frame.origin.0 + ox as f32 + 0.5,
                    frame.origin.1 + oy as f32 + 0.5,
                );
                *px = sample_of::<N>(data, width, height, turned, geometry.to_source(r, w, h));
            }
        });
    Ok((out, ow, oh))
}

/// The source at a position `s` as the frame takes it: Catmull-Rom
/// when `turned`, black off the source, and the pixel under it when
/// not.
#[inline]
fn sample_of<const N: usize>(
    data: &[f32],
    width: usize,
    height: usize,
    turned: bool,
    s: (f32, f32),
) -> [f32; N] {
    let (w, h) = (width as f32, height as f32);
    if turned {
        if s.0 < 0.0 || s.1 < 0.0 || s.0 >= w || s.1 >= h {
            [0.0; N]
        } else {
            sample_cubic_of::<N>(data, width, height, s.0, s.1)
        }
    } else {
        let sx = (s.0.floor() as i64).clamp(0, width as i64 - 1) as usize;
        let sy = (s.1.floor() as i64).clamp(0, height as i64 - 1) as usize;
        let k = (sy * width + sx) * N;
        std::array::from_fn(|c| data[k + c])
    }
}

/// [`apply`] with the frame cut at `size` rather than its own: for a
/// frame too large to hold at its own size that an export asks for
/// smaller, which the export's fit then takes the rest of the way.
/// Refused past [`limit_for`] the image, as [`apply`] is.
pub fn apply_at(
    image: &WorkingImage,
    geometry: &Geometry,
    size: (usize, usize),
) -> Result<WorkingImage, TooLarge> {
    let limit = limit_for(image.width, image.height);
    let (data, width, height) = frame_at_of::<3>(
        &image.data,
        image.width,
        image.height,
        geometry,
        size,
        limit,
    )?;
    Ok(WorkingImage {
        width,
        height,
        data,
    })
}

/// [`apply_at`] on a plane of one channel, as [`apply_plane`] is.
pub fn apply_plane_at(
    data: &[f32],
    width: usize,
    height: usize,
    geometry: &Geometry,
    size: (usize, usize),
) -> Result<(Vec<f32>, usize, usize), TooLarge> {
    frame_at_of::<1>(
        data,
        width,
        height,
        geometry,
        size,
        limit_for(width, height),
    )
}

/// The frame of `geometry` cut at `size` from `N` interleaved
/// channels: each pixel stands for a block of the frame's own pixels
/// and is the mean of samples spread evenly over that block, as many
/// along each side as the source pixels the side spans, from one (where
/// the frame magnifies the source, a block is smooth and its center
/// says enough) to the frame pixels the side stands for. So a frame cut
/// many times smaller than its own size is a box filter of it, not an
/// alias of the source, and the samples taken stay near the source's
/// pixels rather than the frame's, which may be 64 times as many.
fn frame_at_of<const N: usize>(
    data: &[f32],
    width: usize,
    height: usize,
    geometry: &Geometry,
    size: (usize, usize),
    limit: usize,
) -> Result<(Vec<f32>, usize, usize), TooLarge> {
    let (w, h) = (width as f32, height as f32);
    let frame = geometry.frame(w, h, None);
    let (ow, oh) = size;
    holdable((ow, oh), limit)?;
    let (bx, by) = (frame.size.0 / ow as f32, frame.size.1 / oh as f32);
    let (most_x, most_y) = (bx.ceil().max(1.0), by.ceil().max(1.0));
    let mut out = vec![0.0f32; ow * oh * N];
    let turned = geometry.resamples();
    let at = |x: f32, y: f32| geometry.to_source((frame.origin.0 + x, frame.origin.1 + y), w, h);
    out.par_chunks_mut((ow * N).max(1))
        .enumerate()
        .for_each(|(oy, row)| {
            for (ox, px) in row.as_chunks_mut::<N>().0.iter_mut().enumerate() {
                let (x0, y0) = (ox as f32 * bx, oy as f32 * by);
                let c = at(x0 + bx / 2.0, y0 + by / 2.0);
                let span = |s: (f32, f32)| (s.0 - c.0).hypot(s.1 - c.1) * 2.0;
                let along_x = span(at(x0 + bx, y0 + by / 2.0));
                let along_y = span(at(x0 + bx / 2.0, y0 + by));
                let count = |v: f32, most: f32| v.ceil().clamp(1.0, most) as usize;
                let (kx, ky) = (count(along_x, most_x), count(along_y, most_y));
                let mut sum = [0.0f32; N];
                for j in 0..ky {
                    let y = y0 + (j as f32 + 0.5) / ky as f32 * by;
                    for i in 0..kx {
                        let x = x0 + (i as f32 + 0.5) / kx as f32 * bx;
                        let v = sample_of::<N>(data, width, height, turned, at(x, y));
                        for (s, v) in sum.iter_mut().zip(v) {
                            *s += v;
                        }
                    }
                }
                let n = (kx * ky) as f32;
                *px = sum.map(|s| s / n);
            }
        });
    Ok((out, ow, oh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::geometry::{Aspect, Crop};

    fn ramp(w: usize, h: usize) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let k = (y * w + x) * 3;
                image.data[k] = x as f32;
                image.data[k + 1] = y as f32;
                image.data[k + 2] = 1.0;
            }
        }
        image
    }

    #[test]
    fn a_crop_takes_exact_pixels_and_a_turn_keeps_a_ramp() {
        let image = ramp(64, 48);
        let g = Geometry {
            crop: Some(Crop {
                x: 0.25,
                y: 0.25,
                w: 0.5,
                h: 0.5,
            }),
            ..Default::default()
        };
        let out = apply(&image, &g).unwrap();
        assert_eq!((out.width, out.height), (32, 24));
        assert_eq!(out.data[0..3], [16.0, 12.0, 1.0]);
        assert_eq!(
            out.data[(23 * 32 + 31) * 3..(23 * 32 + 31) * 3 + 3],
            [47.0, 35.0, 1.0]
        );
        // The cubic reproduces a linear ramp exactly, and a turn keeps
        // it a linear ramp of the turned coordinates.
        assert_eq!(sample_cubic(&image, 10.5, 20.5), [10.0, 20.0, 1.0]);
        let s = sample_cubic(&image, 10.25, 20.75);
        assert!(
            (s[0] - 9.75).abs() < 1e-4 && (s[1] - 20.25).abs() < 1e-4,
            "{s:?}"
        );
        let turned = Geometry {
            angle: 7.0,
            aspect: Aspect::Original,
            ..Default::default()
        };
        let out = apply(&image, &turned).unwrap();
        assert!(out.width < 64 && out.height < 48);
        let f = turned.frame(64.0, 48.0, None);
        // Away from the frame's corners, which the fit puts on the
        // source's edges, where the cubic's taps clamp.
        for (oy, ox) in [
            (6usize, 6usize),
            (out.height / 2, out.width / 3),
            (out.height - 7, out.width - 7),
        ] {
            let r = (f.origin.0 + ox as f32 + 0.5, f.origin.1 + oy as f32 + 0.5);
            let s = turned.to_source(r, 64.0, 48.0);
            let k = (oy * out.width + ox) * 3;
            assert!(
                (out.data[k] - (s.0 - 0.5)).abs() < 0.05
                    && (out.data[k + 1] - (s.1 - 0.5)).abs() < 0.05,
                "{ox},{oy}: {:?} vs {s:?}",
                &out.data[k..k + 3]
            );
        }
    }
}

#[cfg(test)]
mod keystone {
    use super::*;

    /// A keystoned ramp comes through where `to_source` says, and the
    /// top of a picture that looked up is spread wider than it was.
    #[test]
    fn the_keystone_resamples_where_to_source_says() {
        let (w, h) = (96usize, 64usize);
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let k = (y * w + x) * 3;
                image.data[k] = x as f32;
                image.data[k + 1] = y as f32;
            }
        }
        let g = Geometry {
            vertical: 15.0,
            horizontal: -6.0,
            ..Default::default()
        };
        let out = apply(&image, &g).unwrap();
        let f = g.frame(w as f32, h as f32, None);
        assert!(out.width < w || out.height < h);
        for (oy, ox) in [
            (5usize, 5usize),
            (out.height / 2, out.width / 3),
            (out.height - 6, out.width - 6),
        ] {
            let r = (f.origin.0 + ox as f32 + 0.5, f.origin.1 + oy as f32 + 0.5);
            let s = g.to_source(r, w as f32, h as f32);
            let k = (oy * out.width + ox) * 3;
            assert!(
                (out.data[k] - (s.0 - 0.5)).abs() < 0.05
                    && (out.data[k + 1] - (s.1 - 0.5)).abs() < 0.05,
                "{ox},{oy}: {:?} vs {s:?}",
                &out.data[k..k + 3]
            );
        }
        // The top row of the frame spans fewer source columns than the
        // bottom row: the top is spread out.
        let span = |oy: usize| {
            let k0 = (oy * out.width) * 3;
            let k1 = (oy * out.width + out.width - 1) * 3;
            out.data[k1] - out.data[k0]
        };
        assert!(
            span(2) < span(out.height - 3),
            "{} {}",
            span(2),
            span(out.height - 3)
        );
    }

    /// A frame is held up to its limit and refused one pixel past it,
    /// with its size and the limit; one whose pixels overflow is too.
    #[test]
    fn a_frame_is_held_up_to_its_limit() {
        assert_eq!(holdable((100, 100), 10_000), Ok(()));
        assert_eq!(holdable((10_000, 1), 10_000), Ok(()));
        assert_eq!(holdable((0, 7), 0), Ok(()));
        assert_eq!(
            holdable((101, 100), 10_000),
            Err(TooLarge {
                size: (101, 100),
                limit: 10_000
            })
        );
        assert!(holdable((usize::MAX, 2), usize::MAX).is_err());
        assert_eq!(limit_for(10, 10), MAX_FRAME_PIXELS);
        assert_eq!(limit_for(20_000, 20_000), 400_000_000);
        assert_eq!(
            TooLarge {
                size: (40_000, 30_000),
                limit: MAX_FRAME_PIXELS
            }
            .to_string(),
            "a frame of 40000 by 30000 pixels is more than the 200 megapixels one may hold"
        );
    }

    /// A crop dragged out to an extreme keystone's bounds asks for many
    /// times its source's pixels: past the limit it is refused with its
    /// size, before any of it is allocated, and within it it is cut.
    #[test]
    fn a_frame_past_the_limit_is_refused() {
        let (w, h) = (48usize, 32usize);
        let image = WorkingImage::new(w, h);
        let mut g = Geometry {
            vertical: 40.0,
            ..Default::default()
        };
        let (fw, fh) = (w as f32, h as f32);
        let (pw, ph) = g.plane_size(fw, fh);
        let b = g.bounds(fw, fh);
        // The largest crop that fits, of a coarse search over the
        // bounds: a band across them, centered, from one depth to
        // another.
        let steps = (1..=20).map(|i| i as f32 / 20.0);
        let crop = steps
            .clone()
            .flat_map(|s| steps.clone().map(move |t| (s, t)))
            .flat_map(|(s, t)| (0..20).map(move |u| (s, t, u as f32 / 20.0)))
            .filter(|&(_, t, u)| u < t)
            .map(|(s, t, u)| greycard_edit::geometry::Crop {
                x: (pw / 2.0 - s * b.size.0 / 2.0) / pw,
                y: (b.origin.1 + u * b.size.1) / ph,
                w: s * b.size.0 / pw,
                h: (t - u) * b.size.1 / ph,
            })
            .filter(|c| g.fits(c, fw, fh))
            .max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h)))
            .unwrap();
        g.crop = Some(crop);
        let f = g.frame(fw, fh, None);
        let size = (f.size.0 as usize, f.size.1 as usize);
        assert!(size.0 * size.1 > 4 * w * h, "{size:?}");
        let limit = size.0 * size.1 - 1;
        assert_eq!(
            frame_of::<3>(&image.data, w, h, &g, limit),
            Err(TooLarge { size, limit })
        );
        let held = frame_of::<3>(&image.data, w, h, &g, limit + 1).unwrap();
        assert_eq!((held.1, held.2), size);
        assert_eq!(apply(&image, &g).map(|i| (i.width, i.height)), Ok(size));
    }

    /// The source and the geometry of [`over_the_limit`]: 12.4 MP, as
    /// small a source as reaches [`MAX_FRAME_PIXELS`]. A search of
    /// every tilt and angle at a 32nd of the bounds found no crop that
    /// fits past about 17 times its source, so a smaller one cannot.
    const OVER: (usize, usize) = (4320, 2880);

    /// Both tilts far over, a turn, and the crop dragged out to the
    /// reach of the bounds on one side, a little inside it.
    fn over_the_limit() -> Geometry {
        Geometry {
            vertical: -40.0,
            horizontal: -40.0,
            angle: -30.0,
            crop: Some(greycard_edit::geometry::Crop {
                x: -3.49,
                y: 0.26,
                w: 3.98,
                h: 4.22,
            }),
            ..Default::default()
        }
    }

    /// A frame past its limit cut at the size an export asks for it
    /// smaller, then fitted, is the whole frame fitted to that size: of
    /// [`over_the_limit`] on a tenth of [`OVER`], a ramp with a square
    /// of a checker of single pixels on it, held whole here to read the
    /// two against each other.
    #[test]
    fn a_frame_cut_smaller_is_the_whole_frame_fitted() {
        use crate::export::{fit_exact, fit_size, reduced_frame};
        let (w, h) = (OVER.0 / 10, OVER.1 / 10);
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mark = (150..250).contains(&x) && (100..180).contains(&y);
                let v = if mark {
                    ((x + y) % 2) as f32
                } else {
                    0.1 + 0.3 * x as f32 / w as f32
                };
                let k = (y * w + x) * 3;
                image.data[k..k + 3].copy_from_slice(&[v, 0.2, 0.3 * y as f32 / h as f32]);
            }
        }
        let g = over_the_limit();
        let (data, fw, fh) = frame_of::<3>(&image.data, w, h, &g, usize::MAX).unwrap();
        let whole = WorkingImage {
            width: fw,
            height: fh,
            data,
        };
        let limit = fw * fh / 4;
        let edge = Some(240);
        let target = fit_size(fw, fh, edge).unwrap();
        let at = reduced_frame((fw, fh), edge, limit).unwrap();
        assert!(at.0 * at.1 <= limit && at.0 > 240, "{at:?}");
        let small = apply_at(&image, &g, at).unwrap();
        assert_eq!((small.width, small.height), at);
        let a = fit_exact(&whole, target.0, target.1).unwrap();
        let b = fit_exact(&small, target.0, target.1).unwrap();
        let diffs: Vec<f32> = a
            .data
            .iter()
            .zip(&b.data)
            .map(|(a, b)| (a - b).abs())
            .collect();
        let mean = diffs.iter().sum::<f32>() / diffs.len() as f32;
        let most = diffs.iter().cloned().fold(0.0, f32::max);
        // Measured 2.4e-5 and 0.22, the most on the checker, where the
        // two filters part; a sample a pixel cut smaller, without the
        // box, aliases it to 6.8e-5 and 0.36.
        assert!(mean < 4e-5, "mean {mean}");
        assert!(most < 0.3, "most {most}");
    }

    /// The real limit refuses a crop dragged out on a 12 MP source,
    /// before any of its frame is allocated, which held would be
    /// 2.5 GB.
    #[test]
    fn the_limit_refuses_a_crop_dragged_out_on_a_real_source() {
        let (w, h) = OVER;
        let g = over_the_limit();
        assert!(g.fits(&g.crop.unwrap(), w as f32, h as f32));
        let f = g.frame(w as f32, h as f32, None);
        let size = (f.size.0 as usize, f.size.1 as usize);
        assert!(size.0 * size.1 > MAX_FRAME_PIXELS, "{size:?}");
        let image = WorkingImage::new(w, h);
        assert_eq!(
            apply(&image, &g).map(|_| ()),
            Err(TooLarge {
                size,
                limit: MAX_FRAME_PIXELS
            })
        );
    }
}

#[cfg(test)]
mod orientation {
    use super::*;

    /// A quarter turn and a mirror move whole pixels: the ramp comes
    /// through exactly, where `to_source` says.
    #[test]
    fn quarter_turns_and_mirrors_move_whole_pixels() {
        let image = ramp_of(40, 30);
        for (turns, flip) in [(1, false), (2, false), (3, true), (0, true)] {
            let g = Geometry {
                turns,
                flip,
                ..Default::default()
            };
            let out = apply(&image, &g).unwrap();
            let (pw, ph) = g.plane_size(40.0, 30.0);
            assert_eq!((out.width, out.height), (pw as usize, ph as usize));
            for (ox, oy) in [(0usize, 0usize), (7, 3), (out.width - 1, out.height - 1)] {
                let s = g.to_source((ox as f32 + 0.5, oy as f32 + 0.5), 40.0, 30.0);
                let k = (oy * out.width + ox) * 3;
                assert_eq!(
                    out.data[k..k + 2],
                    [s.0 - 0.5, s.1 - 0.5],
                    "turns {turns} flip {flip} at {ox},{oy}"
                );
            }
        }
    }

    fn ramp_of(w: usize, h: usize) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let k = (y * w + x) * 3;
                image.data[k] = x as f32;
                image.data[k + 1] = y as f32;
            }
        }
        image
    }
}

#[cfg(test)]
mod direction {
    use super::*;

    /// A positive angle turns the picture counter-clockwise on the
    /// screen: a level line in the source rises to the right after it.
    #[test]
    fn a_positive_angle_is_counter_clockwise_on_the_screen() {
        let (w, h) = (200usize, 120usize);
        let mut image = WorkingImage::new(w, h);
        for x in 0..w {
            let k = (60 * w + x) * 3;
            image.data[k] = 1.0;
            image.data[k + 1] = 1.0;
            image.data[k + 2] = 1.0;
        }
        let g = Geometry {
            angle: 10.0,
            ..Default::default()
        };
        let out = apply(&image, &g).unwrap();
        let brightest_row = |x: usize| {
            (0..out.height)
                .max_by(|&a, &b| {
                    out.data[(a * out.width + x) * 3].total_cmp(&out.data[(b * out.width + x) * 3])
                })
                .unwrap()
        };
        let left = brightest_row(out.width / 4);
        let right = brightest_row(3 * out.width / 4);
        assert!(
            right < left,
            "left {left}, right {right}: the line should rise to the right"
        );
    }
}
