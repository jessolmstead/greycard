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
    let (w, h) = (image.width as i64, image.height as i64);
    let fx = (x - 0.5).floor();
    let fy = (y - 0.5).floor();
    let wx = weights(x - 0.5 - fx);
    let wy = weights(y - 0.5 - fy);
    let mut out = [0f32; 3];
    for (j, wyj) in wy.iter().enumerate() {
        let sy = (fy as i64 + j as i64 - 1).clamp(0, h - 1) as usize;
        for (i, wxi) in wx.iter().enumerate() {
            let sx = (fx as i64 + i as i64 - 1).clamp(0, w - 1) as usize;
            let k = (sy * image.width + sx) * 3;
            let wgt = wxi * wyj;
            out[0] += wgt * image.data[k];
            out[1] += wgt * image.data[k + 1];
            out[2] += wgt * image.data[k + 2];
        }
    }
    out
}

/// The frame of `geometry` cut from `image`.
pub fn apply(image: &WorkingImage, geometry: &Geometry) -> WorkingImage {
    if geometry.is_identity() {
        return image.clone();
    }
    let (w, h) = (image.width as f32, image.height as f32);
    let frame = geometry.frame(w, h);
    let (ow, oh) = (frame.size.0 as usize, frame.size.1 as usize);
    let mut out = WorkingImage::new(ow, oh);
    let turned = geometry.resamples();
    out.data
        .par_chunks_mut(ow * 3)
        .enumerate()
        .for_each(|(oy, row)| {
            for (ox, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let r = (
                    frame.origin.0 + ox as f32 + 0.5,
                    frame.origin.1 + oy as f32 + 0.5,
                );
                let s = geometry.to_source(r, w, h);
                *px = if turned {
                    if s.0 < 0.0 || s.1 < 0.0 || s.0 >= w || s.1 >= h {
                        [0.0; 3]
                    } else {
                        sample_cubic(image, s.0, s.1)
                    }
                } else {
                    let sx = (s.0.floor() as i64).clamp(0, image.width as i64 - 1) as usize;
                    let sy = (s.1.floor() as i64).clamp(0, image.height as i64 - 1) as usize;
                    let k = (sy * image.width + sx) * 3;
                    [image.data[k], image.data[k + 1], image.data[k + 2]]
                }
            }
        });
    out
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
        let out = apply(&image, &g);
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
        let out = apply(&image, &turned);
        assert!(out.width < 64 && out.height < 48);
        let f = turned.frame(64.0, 48.0);
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
        let out = apply(&image, &g);
        let f = g.frame(w as f32, h as f32);
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
            let out = apply(&image, &g);
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
        let out = apply(&image, &g);
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
