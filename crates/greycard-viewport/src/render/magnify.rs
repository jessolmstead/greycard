//! The view past 1:1. A picture pixel there covers more than one
//! target pixel, and the bilinear the view reads below it blurs one
//! pixel into the next, so the pixel-peeper cannot tell a pixel from
//! its neighbor. Past 1:1 the shader reads each target pixel at the
//! center of the picture pixel under it instead: every target pixel
//! of a picture pixel takes the same position, so the picture, its
//! masks, its planes, its grain and its output sharpen all come out
//! one color a picture pixel, a square block, as the export has them
//! a pixel. At 1:1 and below nothing changes.
//!
//! The snap is of the position, not a nearest sampler, because the
//! texture is not always the picture at its own size: one reduced to
//! fit the device holds a box of pixels a texel, and the loupe's copy
//! standing in for a picture can be smaller than it. Either way the
//! blocks are the picture's pixels, read from what is on hand.
//!
//! The decision is the window's: `zoom` is target pixels, which are
//! physical pixels, per frame pixel, so 100% is one picture pixel a
//! physical pixel at any UI scale. The rest is the shader's
//! reference, for the tests.

/// What a product a hair over one still counts as one. The loupe
/// divides its zoom by its copy's scale and the view multiplies it
/// back (`tile_view`); measured over 38,686 copy and picture sizes in
/// f32 that never came out above 1.0 at 100%, but nothing in the
/// arithmetic promises it, and a zoom within this of 1:1 is none
/// anyone sets, so the guard costs nothing.
const SLACK: f32 = 1e-5;

/// Whether a view at `zoom` target pixels a frame pixel, a picture
/// pixel `pixel` frame pixels a side, is past 1:1: strictly more
/// than one target pixel a picture pixel. False for nothing that is
/// a number.
pub fn magnified(zoom: f32, pixel: f32) -> bool {
    zoom * pixel > 1.0 + SLACK
}

/// The center of the picture pixel holding frame position `q`, the
/// picture pixel `side` frame pixels a side: as the shader snaps it.
#[cfg(test)]
pub fn snap(q: (f32, f32), side: f32) -> (f32, f32) {
    let at = |v: f32| ((v / side).floor() + 0.5) * side;
    (at(q.0), at(q.1))
}

/// The center of the source pixel holding source position `at`,
/// the picture pixel `side` source pixels a side: where nothing
/// resamples, the export takes the pixel its position falls in, the
/// floor, and past 1:1 the shader reads that pixel's center.
#[cfg(test)]
pub fn whole(at: (f32, f32), side: f32) -> (f32, f32) {
    snap(at, side)
}

/// The picture under the target pixel centered at `at` for a view
/// whose map from a target position to a source one is `to_source`
/// through `to_frame` (the frame position first): past 1:1 the
/// bilinear at the center of the picture pixel there, and where the
/// view does not `resample`, at the center of the source pixel that
/// center falls in ([`whole`]); below it the bilinear at the target
/// pixel's own center, as the view has always read it.
#[cfg(test)]
pub fn drawn(
    image: &greycard_core::image::WorkingImage,
    at: (f32, f32),
    zoom: f32,
    pixel: f32,
    resamples: bool,
    to_frame: impl Fn((f32, f32)) -> (f32, f32),
    to_source: impl Fn((f32, f32)) -> (f32, f32),
) -> [f32; 3] {
    let q = to_frame(at);
    if !magnified(zoom, pixel) {
        return super::minify::bilinear(image, to_source(q));
    }
    let s = to_source(snap(q, pixel));
    let s = if resamples { s } else { whole(s, pixel) };
    super::minify::bilinear(image, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::image::WorkingImage;

    #[test]
    fn past_1_to_1_is_strictly_more_than_a_target_pixel_a_picture_pixel() {
        assert!(!magnified(1.0, 1.0));
        assert!(!magnified(0.5, 1.0));
        assert!(magnified(1.01, 1.0));
        assert!(magnified(2.0, 1.0));
        assert!(magnified(4.0, 1.0));
        // A copy half the picture's size at 100% is drawn at 2 a copy
        // pixel; its picture pixels are half a copy pixel.
        assert!(!magnified(2.0, 0.5));
        assert!(magnified(4.0, 0.5));
        // The loupe's divide and multiply back, at 100%.
        for (copy, drawn) in [(1620.0f32, 6000.0f32), (2048.0, 8256.0), (1365.0, 4000.0)] {
            let scale = copy / drawn;
            assert!(!magnified(1.0 / scale, scale), "{copy} of {drawn}");
            assert!(magnified(2.0 / scale, scale), "{copy} of {drawn}");
        }
        assert!(!magnified(f32::NAN, 1.0));
        assert!(!magnified(0.0, 1.0));
    }

    #[test]
    fn a_snap_lands_on_the_center_of_the_pixel_under_it() {
        assert_eq!(snap((0.0, 0.99), 1.0), (0.5, 0.5));
        assert_eq!(snap((3.25, 7.5), 1.0), (3.5, 7.5));
        assert_eq!(snap((1.1, 0.4), 0.5), (1.25, 0.25));
    }

    fn checker(w: usize, h: usize) -> WorkingImage {
        WorkingImage {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    let v = if (x + y).is_multiple_of(2) { 0.1 } else { 0.7 };
                    [v, v * 0.5, x as f32 / w as f32]
                })
                .collect(),
        }
    }

    /// A checker at 400%, panned a quarter of a target pixel off the
    /// grid, is exact 4x4 blocks, each its pixel; at 100% panned half
    /// a pixel it is the bilinear between pixels, as it always was.
    #[test]
    fn a_checker_at_400_is_blocks_and_at_100_the_bilinear() {
        let (w, h) = (6usize, 4usize);
        let image = checker(w, h);
        let zoom = 4.0;
        let to_frame = |(x, y): (f32, f32)| (x / zoom, y / zoom);
        for ty in 0..h * 4 {
            for tx in 0..w * 4 {
                let at = (tx as f32 + 0.5, ty as f32 + 0.5);
                let got = drawn(&image, at, zoom, 1.0, false, to_frame, |q| q);
                assert_eq!(got, image.pixel(tx / 4, ty / 4), "{tx},{ty}");
            }
        }
        // The pixel a picture pixel is a side of when the copy is
        // half the picture: blocks of two at 400% of the picture.
        let half = |(x, y): (f32, f32)| (x / 8.0, y / 8.0);
        for tx in 0..w * 8 {
            let a = drawn(&image, (tx as f32 + 0.5, 0.5), 8.0, 0.5, false, half, |q| q);
            let b = drawn(
                &image,
                ((tx / 4 * 4) as f32 + 0.5, 0.5),
                8.0,
                0.5,
                false,
                half,
                |q| q,
            );
            assert_eq!(a, b, "{tx}");
        }
        let one = |(x, y): (f32, f32)| (x + 0.5, y);
        let got = drawn(&image, (1.5, 1.5), 1.0, 1.0, false, one, |q| q);
        let (a, b) = (image.pixel(1, 1), image.pixel(2, 1));
        assert!((got[0] - (a[0] + b[0]) / 2.0).abs() < 1e-6, "{got:?}");
    }
}
