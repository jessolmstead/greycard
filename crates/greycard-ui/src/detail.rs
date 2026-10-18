//! Detail in a mask: the bands the develop keeps beside the picture so
//! that a mask's Texture, Clarity and Dehaze can be put on at the
//! finish, by the mask's weight, where every other part of its look is.
//!
//! The develop's Texture and Clarity are bands of the log luminance put
//! on as a gain, and its Dehaze a transmission from a model fitted on a
//! reduced copy; what is costly in each (the filters, the fit) does not
//! depend on the amount, and the amount enters per pixel at the end.
//! So the develop makes the bands once, at the picture's size, when a
//! mask asks for one, and the finish reads them at the pixel and puts
//! on what the masks' amounts there say, as the global op would.

use greycard_core::develop::dehaze::{self, Reduced};
use greycard_core::develop::local_contrast::{
    self, CLARITY_GAIN, LocalContrastOptions, TEXTURE_GAIN, clip_fade, local_contrast_with_bands,
};
use greycard_core::image::WorkingImage;
use greycard_edit::DetailWants;
use half::f16;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// What goes with the planes, one value for the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetailMeta {
    /// The airlight the dehaze's model read off the reduced copy, the
    /// one the global's would read.
    pub airlight: [f32; 3],
    /// The develop's clip level, which the bands' gain fades out at.
    pub clip_level: f32,
}

/// The bands of a developed picture, at its size: Texture's band and
/// Clarity's (its mid-tone weight in it), in stops, and the haze, what
/// a transmission at a strength `s` is `1 - s * haze` of. A band no
/// mask asked for is zero.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailPlanes {
    pub width: usize,
    pub height: usize,
    /// The texture, clarity and haze of each pixel and a fourth lane
    /// of nothing, as the viewport's `Rgba16Float` texture holds them:
    /// half floats, since a band is a few stops at most and its
    /// thousandth of one is below what a gain can show.
    pub data: Vec<f16>,
    pub meta: DetailMeta,
    /// The bands asked for when these were made.
    pub wants: DetailWants,
}

/// Whether planes of `planes` size fit a picture of `width` by
/// `height`; said in the log, and held in a debug build, when they do
/// not: the masks' Detail is then left off the picture, where it was
/// asked for, so a mismatch is a bug in whoever carried them.
pub fn fits(planes: (usize, usize), width: usize, height: usize, whose: &str) -> bool {
    let fits = planes == (width, height);
    if !fits {
        tracing::warn!(
            "{whose}: the masks' detail planes are {}x{}, the picture {width}x{height}; \
             the masks' Detail is left off",
            planes.0,
            planes.1
        );
        debug_assert!(fits, "detail planes of another size in {whose}");
    }
    fits
}

impl DetailPlanes {
    /// The planes the masks of an edit ask for, of `patched`, the
    /// developed picture with its retouch, before its own Detail: the
    /// bands of it, and the haze of it after the global's local
    /// contrast `global` (none when the global's Texture and Clarity
    /// rest), which is the picture the global's dehaze is fitted on, so
    /// a mask's Dehaze over a global Texture or Clarity clears what the
    /// global's would. None when no mask asks.
    pub fn make(
        patched: &WorkingImage,
        global: Option<LocalContrastOptions>,
        clip_level: f32,
        wants: DetailWants,
    ) -> Option<Self> {
        Self::make_with(
            patched,
            global,
            clip_level,
            wants,
            None,
            &AtomicBool::new(false),
        )
    }

    /// [`Self::make`], taking the texture and clarity bands from
    /// `reuse` where it holds them (planes of the same picture whose
    /// haze was read after another global local contrast), and giving
    /// up, with none, once `cancel` is set: checked between the bands
    /// and the haze, the two costly halves.
    pub fn make_with(
        patched: &WorkingImage,
        global: Option<LocalContrastOptions>,
        clip_level: f32,
        wants: DetailWants,
        reuse: Option<&DetailPlanes>,
        cancel: &AtomicBool,
    ) -> Option<Self> {
        if !wants.any() {
            return None;
        }
        let (w, h) = (patched.width, patched.height);
        let reuse = reuse.filter(|r| (r.width, r.height) == (w, h));
        let kept = |lane: usize, had: bool| reuse.filter(|_| had).map(|r| (r, lane));
        let reused = [
            kept(0, reuse.is_some_and(|r| r.wants.texture)).filter(|_| wants.texture),
            kept(1, reuse.is_some_and(|r| r.wants.clarity)).filter(|_| wants.clarity),
        ];
        let (texture, clarity) = (
            wants.texture && reused[0].is_none(),
            wants.clarity && reused[1].is_none(),
        );
        let global = global.filter(|_| wants.haze);
        // With the global's local contrast to read the haze after, the
        // bands still to make come off the same filters it runs.
        let (bands, contrasted) = match global {
            Some(options) => {
                let mut contrasted = patched.clone();
                let (_, bands) = local_contrast_with_bands(
                    &mut contrasted,
                    &options,
                    clip_level,
                    texture,
                    clarity,
                );
                (bands, Some(contrasted))
            }
            None => (local_contrast::bands(patched, texture, clarity), None),
        };
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (haze, airlight) = if wants.haze {
            haze_of(contrasted.as_ref().unwrap_or(patched))
        } else {
            (None, [1.0; 3])
        };
        drop(contrasted);
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let mut data = vec![f16::ZERO; w * h * 4];
        let lanes = [bands.texture, bands.clarity, haze];
        for (k, lane) in lanes.iter().enumerate() {
            if let Some(lane) = lane {
                for (px, &v) in data.as_chunks_mut::<4>().0.iter_mut().zip(lane) {
                    px[k] = f16::from_f32(v);
                }
            }
        }
        for (r, k) in reused.into_iter().flatten() {
            for (px, from) in data
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(r.data.as_chunks::<4>().0)
            {
                px[k] = from[k];
            }
        }
        Some(Self {
            width: w,
            height: h,
            data,
            meta: DetailMeta {
                airlight,
                clip_level,
            },
            wants,
        })
    }

    /// The three planes as a picture of three channels, for the
    /// geometry and the resize to take as they take the picture.
    pub fn to_image(&self) -> WorkingImage {
        WorkingImage {
            width: self.width,
            height: self.height,
            data: self
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|px| [px[0].to_f32(), px[1].to_f32(), px[2].to_f32()])
                .collect(),
        }
    }

    /// [`Self::to_image`] box-averaged by `factor` each way, as
    /// `edited::reduce` averages the picture, without the full-size
    /// copy in between.
    pub fn to_image_reduced(&self, factor: usize) -> WorkingImage {
        let factor = factor.max(1);
        if factor == 1 {
            return self.to_image();
        }
        let (w, h) = ((self.width / factor).max(1), (self.height / factor).max(1));
        let mut data = vec![0.0f32; w * h * 3];
        let n = (factor * factor) as f32;
        data.par_chunks_mut(w * 3).enumerate().for_each(|(y, out)| {
            for (x, px) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                for sy in y * factor..((y + 1) * factor).min(self.height) {
                    let row = &self.data[sy * self.width * 4..][..self.width * 4];
                    for sx in x * factor..((x + 1) * factor).min(self.width) {
                        for (s, v) in sum.iter_mut().zip(&row[sx * 4..sx * 4 + 3]) {
                            *s += v.to_f32();
                        }
                    }
                }
                *px = sum.map(|s| s / n);
            }
        });
        WorkingImage {
            width: w,
            height: h,
            data,
        }
    }
}

/// The haze of a picture and the airlight it was read with: the
/// dehaze's model fitted at a strength of one, on the reduced copy the
/// global's would be fitted on.
fn haze_of(image: &WorkingImage) -> (Option<Vec<f32>>, [f32; 3]) {
    let reduced = Reduced::new(image);
    match reduced.fit(1.0) {
        Some(model) => (Some(model.haze(image)), reduced.airlight()),
        None => (None, [1.0; 3]),
    }
}

/// A summed amount under which the masks' Detail at a pixel is
/// nothing: a thousandth of a slider's travel, below anything a
/// picture shows. A mask's weight where it has all but faded out is
/// read a millionth apart on the GPU and the CPU, and a Dehaze that
/// puts haze in lifts a black pixel by it, where everything else a
/// look does leaves black as it is; the shader holds to the same
/// ([`soft`]).
pub const NEGLIGIBLE: f32 = 1e-3;

/// A summed amount through the threshold: nothing under
/// [`NEGLIGIBLE`], itself from twice that up, and a straight ramp
/// between, so the amount rises from nothing without a step (a step
/// would be a contour along a feather: a Dehaze putting haze in over
/// near black lifts it by a few levels at once) and every amount a
/// slider is set to is put on as it is.
#[inline]
pub fn soft(k: f32) -> f32 {
    let a = k.abs();
    if a < NEGLIGIBLE {
        0.0
    } else if a < 2.0 * NEGLIGIBLE {
        k.signum() * 2.0 * (a - NEGLIGIBLE)
    } else {
        k
    }
}

/// The masks' Detail at a pixel, summed by their weights: Texture's
/// and Clarity's stops a band and Dehaze's amount, each as the global
/// op takes it (the gain on the band, the strength on the haze).
/// Two masks over one pixel add their stops, and a Dehaze's amount is
/// held to the slider's ends. None when every sum is
/// [`NEGLIGIBLE`].
#[inline]
pub fn amounts(locals: &[(&crate::finish::Baked, f32)]) -> Option<[f32; 3]> {
    let mut k = [0.0f32; 3];
    for (b, w) in locals {
        if b.detail != [0.0; 3] {
            k[0] += w * b.detail[0];
            k[1] += w * b.detail[1];
            k[2] += w * b.detail[2];
        }
    }
    let k = k.map(soft);
    (k != [0.0; 3]).then(|| {
        [
            k[0] * TEXTURE_GAIN,
            k[1] * CLARITY_GAIN,
            k[2].clamp(-1.0, 1.0) * dehaze::FULL_STRENGTH,
        ]
    })
}

/// A pixel with the masks' Detail put on: Texture's band as a gain
/// under the fade toward the clip, then Clarity's with the fade read
/// again, then the haze cleared by the transmission at the strength,
/// bounded as the global's apply bounds it. `k` is what [`amounts`]
/// makes and `bands` the planes at the pixel. The shader's `detail_at`
/// is the same arithmetic.
#[inline]
pub fn apply(px: [f32; 3], bands: [f32; 3], k: [f32; 3], meta: &DetailMeta) -> [f32; 3] {
    let mut px = px;
    for (gain, band) in [(k[0], bands[0]), (k[1], bands[1])] {
        if gain != 0.0 {
            let fade = clip_fade(px[0].max(px[1]).max(px[2]), meta.clip_level);
            let g = (fade * gain * band).exp2();
            px = px.map(|v| v * g);
        }
    }
    if k[2] != 0.0 {
        let a = meta.airlight;
        let u = dehaze::local_clearing(bands[2], k[2], &px, &a);
        px = [0, 1, 2].map(|c| (px[c] - a[c] * u) / (1.0 - u));
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::Detail;

    fn picture(w: usize, h: usize) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let fine = ((x * 7 + y * 13) % 5) as f32 / 5.0;
                let coarse = ((x as f32 / 9.0).sin() * (y as f32 / 7.0).cos() + 1.0) / 2.0;
                let v = 0.03 + 0.4 * coarse + 0.1 * fine;
                let i = (y * w + x) * 3;
                image.data[i] = v * 1.1;
                image.data[i + 1] = v;
                image.data[i + 2] = v * 0.6;
                // A few pixels toward and past the clip, for the fade.
                if (x / 7 + y / 5) % 11 == 0 {
                    image.data[i] = 0.9 + 0.3 * fine;
                    image.data[i + 1] = 0.85;
                }
            }
        }
        image
    }

    /// A haze over the picture: toward a warm grey, more of it up the
    /// frame, as a far ridge has.
    fn hazy(w: usize, h: usize) -> WorkingImage {
        let mut image = picture(w, h);
        let a = [0.82, 0.8, 0.76];
        for y in 0..h {
            let t = 0.35 + 0.6 * y as f32 / h as f32;
            for x in 0..w {
                let i = (y * w + x) * 3;
                for (v, a) in image.data[i..i + 3].iter_mut().zip(a) {
                    *v = *v * t + a * (1.0 - t);
                }
            }
        }
        image
    }

    const CLIP: f32 = 1.0;

    fn identity() -> [[f32; 3]; 3] {
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    }

    /// The develop's global Detail at `d` on `image`, as the worker
    /// runs it: the local contrast, then the dehaze.
    fn global(image: &WorkingImage, d: &Detail) -> WorkingImage {
        let mut out = image.clone();
        if let Some(options) = d.options() {
            local_contrast::local_contrast(&mut out, &options, CLIP);
        }
        if d.enabled && d.dehaze != 0.0 {
            dehaze::dehaze(&mut out, &dehaze::DehazeOptions { amount: d.dehaze });
        }
        out
    }

    fn wants(d: &Detail) -> DetailWants {
        DetailWants {
            texture: d.texture != 0.0,
            clarity: d.clarity != 0.0,
            haze: d.dehaze != 0.0,
        }
    }

    /// A mask over the whole frame.
    fn everywhere(detail: Detail) -> greycard_edit::Adjustment {
        greycard_edit::Adjustment {
            id: 1,
            mask: greycard_edit::mask::Mask {
                components: vec![greycard_edit::mask::Component {
                    shape: greycard_edit::mask::Shape::Linear {
                        from: [0.5, 10.0],
                        to: [0.5, 11.0],
                    },
                    ..Default::default()
                }],
                invert: false,
            },
            look: greycard_edit::Look {
                detail,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn sixteen(rendered: crate::export::Rendered) -> Vec<u16> {
        match rendered.pixels {
            crate::export::Pixels::Sixteen(p) => p,
            crate::export::Pixels::Eight(_) => panic!("a TIFF is sixteen bits"),
        }
    }

    /// The export of `image` under `edit`, with the planes when given.
    fn export(
        image: &WorkingImage,
        edit: &greycard_edit::Edit,
        planes: Option<&DetailPlanes>,
        white: Option<&greycard_edit::WhiteShift>,
    ) -> Vec<u16> {
        let settings = crate::export::Settings {
            format: crate::export::Format::Tiff,
            sharpen: crate::export::Sharpen::Off,
            ..Default::default()
        };
        let bands = planes.map(|p| p.to_image());
        sixteen(crate::export::render(
            image,
            edit,
            (image.width as u32, image.height as u32),
            &settings,
            &Default::default(),
            CLIP,
            None,
            crate::finish::Source::SCENE,
            white,
            bands.as_ref().zip(planes.map(|p| &p.meta)),
        ))
    }

    /// The worst and the mean difference, in levels of 65535, between the global
    /// Detail at `d` and a mask over the whole frame with `d`, the
    /// mask's own white balance `white` on both; and that the mask's
    /// Detail switched off is nothing.
    fn whole_against_global(
        image: &WorkingImage,
        d: Detail,
        white: greycard_edit::LocalWhite,
    ) -> (u16, f64) {
        let shift = crate::testing::white_shift();
        let shift = (white != greycard_edit::LocalWhite::Off).then_some(&shift);
        let planes = DetailPlanes::make(image, None, CLIP, wants(&d)).expect("planes");
        let mut masked = greycard_edit::Edit::default();
        masked.adjustments.push(everywhere(d));
        masked.adjustments[0].look.white_balance = white;
        // Off, the mask's Detail is nothing; and the global is
        // finished under that same mask, so the two differ by the
        // Detail alone.
        let mut off = masked.clone();
        off.adjustments[0].look.detail.enabled = false;
        let bare = export(image, &off, None, shift);
        assert_eq!(export(image, &off, Some(&planes), shift), bare);
        let want = export(&global(image, &d), &off, None, shift);
        assert!(want != bare, "{d:?} did nothing");
        let got = export(image, &masked, Some(&planes), shift);
        let d: Vec<u16> = want.iter().zip(&got).map(|(a, b)| a.abs_diff(*b)).collect();
        let mean = d.iter().map(|&v| v as f64).sum::<f64>() / d.len() as f64;
        (*d.iter().max().unwrap(), mean)
    }

    /// The CPU reference: a mask over the whole frame at weight one,
    /// the global Detail off, no sharpen and no geometry, is the
    /// global Detail at the same amounts, through the whole export, to
    /// the planes' half-float rounding: Texture, Clarity and Dehaze
    /// each, in and out, and Texture with Clarity; and so under a
    /// white balance of the mask's own, which its Detail goes on
    /// before, as the global's goes on before everything at the finish.
    ///
    /// Two levels in 65535 for the bands, eight for the haze at 0.6 (a
    /// half float's step on a haze near one, divided by a transmission
    /// near the floor).
    #[test]
    fn a_whole_frame_mask_is_the_global() {
        let image = hazy(160, 120);
        let cases = [
            (0.7, 0.0, 0.0),
            (-0.5, 0.0, 0.0),
            (0.0, 0.8, 0.0),
            (0.0, -0.6, 0.0),
            (-0.5, 0.8, 0.0),
            (0.0, 0.0, 0.6),
            (0.0, 0.0, -0.4),
        ];
        let warm = greycard_edit::LocalWhite::Absolute {
            temperature: 3800.0,
            tint: 0.01,
        };
        for white in [greycard_edit::LocalWhite::Off, warm] {
            for (texture, clarity, dehaze) in cases {
                let d = Detail {
                    enabled: true,
                    texture,
                    clarity,
                    dehaze,
                };
                let (worst, _) = whole_against_global(&image, d, white);
                assert!(worst <= 10, "{white:?} {d:?}: {worst} of 65535");
            }
        }
    }

    /// A mask's Dehaze over the global's Texture or Clarity is the
    /// global's Dehaze over them: the haze is read after the global's
    /// local contrast, the picture the global's dehaze is fitted on.
    /// Held to the single ops' bound, and every slider at its end to
    /// what it measured (106): the output's sensitivity to the stored
    /// haze u is (px - A) / (1 - u)^2, and a Dehaze of one takes the
    /// transmission to its floor, where that is up to 25 |px - A|, so
    /// the haze plane's half-float rounding is magnified that much.
    #[test]
    fn a_masks_dehaze_over_the_globals_bands_is_the_global() {
        let image = hazy(160, 120);
        for ((texture, clarity, dehaze), bound) in [
            ((-0.5, 0.0, 0.6), 10),
            ((0.0, 0.8, 0.6), 10),
            ((-1.0, -1.0, 1.0), 110),
        ] {
            let bands = Detail {
                enabled: true,
                texture,
                clarity,
                dehaze: 0.0,
            };
            let all = Detail { dehaze, ..bands };
            let mask = Detail {
                enabled: true,
                dehaze,
                ..Detail::default()
            };
            // The develop's picture with the global's bands on.
            let developed = global(&image, &bands);
            let planes = DetailPlanes::make(&image, bands.options(), CLIP, wants(&mask)).unwrap();
            let mut masked = greycard_edit::Edit {
                detail: bands,
                ..Default::default()
            };
            masked.adjustments.push(everywhere(mask));
            let mut off = masked.clone();
            off.adjustments[0].look.detail.enabled = false;
            let want = export(&global(&image, &all), &off, None, None);
            let got = export(&developed, &masked, Some(&planes), None);
            let worst = want
                .iter()
                .zip(&got)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(worst <= bound, "{all:?}: {worst} of 65535");
        }
    }

    /// Planes made again for another global local contrast, their
    /// bands taken from the last, are the planes made fresh; and a
    /// job cancelled gives none.
    #[test]
    fn planes_made_again_keep_their_bands_and_a_cancel_stops_them() {
        let image = hazy(64, 48);
        let all = DetailWants {
            texture: true,
            clarity: true,
            haze: true,
        };
        let first = DetailPlanes::make(&image, None, CLIP, all).unwrap();
        let global = Some(LocalContrastOptions {
            texture: 0.4,
            clarity: -0.3,
        });
        let fresh = DetailPlanes::make(&image, global, CLIP, all).unwrap();
        let again = DetailPlanes::make_with(
            &image,
            global,
            CLIP,
            all,
            Some(&first),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(again, fresh);
        assert_ne!(fresh.meta.airlight, first.meta.airlight);
        assert!(
            DetailPlanes::make_with(&image, global, CLIP, all, None, &AtomicBool::new(true))
                .is_none()
        );
    }

    /// The design's limit: a mask's Dehaze with its own Texture or
    /// Clarity is not the global's. The global's dehaze is fitted on
    /// the picture after its Texture and Clarity; the mask's haze is of
    /// the picture before them, since the planes are made before any
    /// amount is known and its bands are put on at the finish. Each
    /// case is held to what it measured, so a change to the order
    /// shows. Measured: Texture -0.5 with Dehaze 0.6 at most 657 levels
    /// of 65535 (2.6 of 255), Clarity 0.8 with Dehaze 0.6 at most 2641
    /// (10.3), all three at their ends at most 17715 (69).
    #[test]
    fn a_masks_dehaze_with_its_bands_is_near_the_global() {
        let image = hazy(160, 120);
        for ((texture, clarity, dehaze), bound) in [
            ((-0.5, 0.0, 0.6), 700),
            ((0.0, 0.8, 0.6), 2700),
            ((-1.0, -1.0, 1.0), 18000),
        ] {
            let d = Detail {
                enabled: true,
                texture,
                clarity,
                dehaze,
            };
            let (worst, mean) = whole_against_global(&image, d, greycard_edit::LocalWhite::Off);
            eprintln!("{d:?}: worst {worst}, mean {mean:.1} of 65535");
            assert!(worst <= bound, "{d:?}: {worst} of 65535");
        }
    }

    /// At half weight a mask's Texture and Clarity are half the stops
    /// and its Dehaze half the amount: the global at half the amounts;
    /// and two masks at half each over a pixel are one at one.
    #[test]
    fn half_a_mask_is_half_the_amount() {
        let image = hazy(96, 72);
        let m = identity();
        let plain = crate::finish::Baked::global(
            &greycard_edit::Edit::default(),
            crate::finish::Source::SCENE,
        );
        for (texture, clarity, dehaze) in [(0.8, 0.0, 0.0), (0.0, -0.7, 0.0), (0.0, 0.0, 0.8)] {
            let d = Detail {
                enabled: true,
                texture,
                clarity,
                dehaze,
            };
            let half = Detail {
                enabled: true,
                texture: texture / 2.0,
                clarity: clarity / 2.0,
                dehaze: dehaze / 2.0,
            };
            let want = global(&image, &half);
            let planes = DetailPlanes::make(&image, None, CLIP, wants(&d)).unwrap();
            let bands = planes.to_image();
            let local = crate::finish::Baked::of(&greycard_edit::Look {
                detail: d,
                ..Default::default()
            });
            let mut worst = 0f32;
            let mut halves = 0f32;
            for y in 0..image.height {
                for x in 0..image.width {
                    let here = Some((bands.pixel(x, y), &planes.meta));
                    let px = image.pixel(x, y);
                    let at = |locals: &[(&crate::finish::Baked, f32)]| {
                        crate::finish::finish_pixel_with(
                            px, &plain, locals, 0.0, None, None, None, here, None, &m,
                        )
                    };
                    let a = crate::finish::finish_pixel(want.pixel(x, y), &plain, &[], &m);
                    let b = at(&[(&local, 0.5)]);
                    let one = at(&[(&local, 1.0)]);
                    let two = at(&[(&local, 0.5), (&local, 0.5)]);
                    for c in 0..3 {
                        worst = worst.max((a[c] - b[c]).abs());
                        halves = halves.max((one[c] - two[c]).abs());
                    }
                }
            }
            // The planes' half floats, as the whole frame's ten levels
            // of 65535 are: measured 5e-6 for Texture, 2.2e-5 for
            // Clarity, 4.7e-5 for Dehaze, held under 1e-4 (6.5 levels).
            assert!(worst < 1e-4, "{d:?}: {worst}");
            assert!(halves < 1e-6, "{d:?}: {halves}");
        }
    }

    /// The threshold rises from nothing without a step and leaves an
    /// amount past twice it as it is.
    #[test]
    fn the_threshold_has_no_step() {
        assert_eq!(soft(0.0), 0.0);
        assert_eq!(soft(NEGLIGIBLE * 0.99), 0.0);
        assert_eq!(soft(-NEGLIGIBLE * 0.99), 0.0);
        assert_eq!(soft(2.0 * NEGLIGIBLE), 2.0 * NEGLIGIBLE);
        assert_eq!(soft(0.3), 0.3);
        assert_eq!(soft(-0.3), -0.3);
        // Continuous across both ends: a millionth of travel moves it
        // by no more than twice that.
        for k in [NEGLIGIBLE, 2.0 * NEGLIGIBLE, -NEGLIGIBLE, -2.0 * NEGLIGIBLE] {
            let (a, b) = (soft(k - 1e-6), soft(k + 1e-6));
            assert!((a - b).abs() <= 4.01e-6, "{k}: {a} {b}");
        }
    }

    /// The planes reduced straight from the half floats are the full
    /// picture of them reduced as the picture is.
    #[test]
    fn the_planes_reduced_are_the_picture_of_them_reduced() {
        let image = picture(37, 23);
        let planes = DetailPlanes::make(
            &image,
            None,
            CLIP,
            DetailWants {
                texture: true,
                clarity: true,
                haze: true,
            },
        )
        .unwrap();
        for k in [1, 2, 3, 5] {
            let a = planes.to_image_reduced(k);
            let b = crate::edited::reduce(&planes.to_image(), k);
            assert_eq!((a.width, a.height), (b.width, b.height));
            assert_eq!(a.data, b.data, "by {k}");
        }
    }

    /// The amounts summed: each local by its weight, its section off
    /// as nothing, and none at all when no local has any.
    #[test]
    fn the_amounts_sum_by_weight() {
        let look = |texture, dehaze| greycard_edit::Look {
            detail: Detail {
                enabled: true,
                texture,
                clarity: 0.0,
                dehaze,
            },
            ..Default::default()
        };
        let a = crate::finish::Baked::of(&look(0.4, 0.8));
        let b = crate::finish::Baked::of(&look(-0.2, 0.9));
        let none = crate::finish::Baked::of(&greycard_edit::Look::default());
        assert_eq!(amounts(&[(&none, 1.0)]), None);
        let k = amounts(&[(&a, 0.5), (&b, 1.0), (&none, 1.0)]).unwrap();
        assert!((k[0] - 0.0 * TEXTURE_GAIN).abs() < 1e-6);
        // The dehaze's amount held at the slider's end before the
        // strength.
        assert_eq!(k[2], dehaze::FULL_STRENGTH);
    }

    #[test]
    fn nothing_is_made_when_no_mask_asks() {
        assert!(DetailPlanes::make(&picture(8, 6), None, 1.0, DetailWants::default()).is_none());
    }

    #[test]
    fn a_band_not_asked_for_is_zero() {
        let image = picture(12, 7);
        let planes = DetailPlanes::make(
            &image,
            None,
            1.0,
            DetailWants {
                clarity: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(planes.data.chunks(4).all(|p| p[0] == f16::ZERO));
        assert!(planes.data.chunks(4).any(|p| p[1] != f16::ZERO));
    }
}
