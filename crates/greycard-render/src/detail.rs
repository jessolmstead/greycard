//! Detail in a mask: the bands the develop keeps beside the picture so
//! that a mask's Texture, Clarity, Structure, Dehaze, and Sharpen can
//! be put on at the finish, by the mask's weight, where every other
//! part of its look is.
//!
//! The develop's Texture, Clarity, and Structure are bands of the log
//! luminance put on as a gain, and its Dehaze a transmission from a
//! model fitted on a reduced copy; what is costly in each (the
//! filters, the fit) does not depend on the amount, and the amount
//! enters per pixel at the end.
//! So the develop makes the bands once, at the picture's size, when a
//! mask asks for one, and the finish reads them at the pixel and puts
//! on what the masks' amounts there say, as the global op would.
//!
//! The capture sharpen is no band of an amount: it is a deconvolution
//! at the global's radius. What it did to a pixel is kept instead, its
//! delta in stops (`greycard_core::develop::sharpen::delta`), made
//! where the develop sharpens, and a mask's Sharpen puts that on again
//! or takes it off by its weight.
//!
//! Beside them goes the picture the range masks read when the global
//! Detail section is on: the one it ran on, so a Dehaze or a Clarity
//! does not move what a Luminance or Color window takes in.

use greycard_core::develop::dehaze::{self, Reduced};
use greycard_core::develop::local_contrast::{
    self, CLARITY_GAIN, LocalContrastOptions, STRUCTURE_GAIN, TEXTURE_GAIN, Wanted, clip_fade,
    local_contrast_with_bands,
};
use greycard_core::develop::sharpen::apply_delta;
use greycard_core::image::WorkingImage;
use greycard_edit::DetailWants;
use half::f16;
use rayon::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What goes with the planes, one value for the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetailMeta {
    /// The airlight the dehaze's model read off the reduced copy, the
    /// one the global's would read.
    pub airlight: [f32; 3],
    /// The develop's clip level, which the bands' gain fades out at.
    pub clip_level: f32,
    /// Whether the picture beside the planes was sharpened. A mask's
    /// Sharpen is a share of the sharpen, never less than none: over a
    /// picture never sharpened, its summed share is held at zero from
    /// below, so a negative one does nothing.
    pub sharpened: bool,
}

/// The bands of a developed picture, at its size: Texture's band,
/// Clarity's (its mid-tone weight in it), and Structure's (its weight
/// and its knee in it), in stops, and the haze, what a transmission at
/// a strength `s` is `1 - s * haze` of. A band no mask asked for is
/// zero. And the sharpen's delta, when a mask asked for it.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailPlanes {
    pub width: usize,
    pub height: usize,
    /// The texture, clarity, haze, and structure of each pixel, as the
    /// viewport's `Rgba16Float` texture holds them: half floats, since
    /// a band is a few stops at most and its thousandth of one is below
    /// what a gain can show. Empty when no mask asks for any of the
    /// four. Shared, so planes given another delta copy none of it.
    pub data: Arc<[f16]>,
    /// The sharpen's delta at each pixel, in stops, as the viewport's
    /// `R16Float` texture holds it: made where the develop sharpens,
    /// from the picture the sharpen saw.
    pub sharpen: Option<Arc<[f16]>>,
    /// The picture from before the Detail section, which the range
    /// masks read when that section is on
    /// (`Edit::samples_before_detail`): the one the bands are made of,
    /// shared, not copied.
    pub sample: Option<Arc<WorkingImage>>,
    pub meta: DetailMeta,
    /// The bands asked for when these were made.
    pub wants: DetailWants,
}

/// The sharpen's delta, made of `deltas` in stops, as the planes hold
/// it.
pub fn sharpen_delta(deltas: &[f32]) -> Arc<[f16]> {
    deltas
        .par_iter()
        .map(|&d| f16::from_f32(d))
        .collect::<Vec<_>>()
        .into()
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
        // The sharpen's delta is not made here, from the picture before
        // the local contrast, but where the develop sharpens.
        let wants = DetailWants {
            sharpen: false,
            sample: false,
            ..wants
        };
        if !wants.bands() {
            return None;
        }
        let (w, h) = (patched.width, patched.height);
        let reuse = reuse.filter(|r| (r.width, r.height) == (w, h));
        let kept = |lane: usize, had: bool| reuse.filter(|_| had).map(|r| (r, lane));
        let reused = [
            kept(0, reuse.is_some_and(|r| r.wants.texture)).filter(|_| wants.texture),
            kept(1, reuse.is_some_and(|r| r.wants.clarity)).filter(|_| wants.clarity),
            kept(3, reuse.is_some_and(|r| r.wants.structure)).filter(|_| wants.structure),
        ];
        let wanted = Wanted {
            texture: wants.texture && reused[0].is_none(),
            clarity: wants.clarity && reused[1].is_none(),
            structure: wants.structure && reused[2].is_none(),
        };
        let global = global.filter(|_| wants.haze);
        // With the global's local contrast to read the haze after, the
        // bands still to make come off the same filters it runs.
        let (bands, contrasted) = match global {
            Some(options) => {
                let mut contrasted = patched.clone();
                let (_, bands) =
                    local_contrast_with_bands(&mut contrasted, &options, clip_level, wanted);
                (bands, Some(contrasted))
            }
            None => (local_contrast::bands(patched, wanted), None),
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
        let lanes = [bands.texture, bands.clarity, haze, bands.structure];
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
            data: data.into(),
            sharpen: None,
            sample: None,
            meta: DetailMeta {
                airlight,
                clip_level,
                sharpened: true,
            },
            wants,
        })
    }

    /// Planes of the sharpen's delta alone, `width` by `height`: what a
    /// mask that asks for its Sharpen and no band is handed.
    pub fn sharpen_only(width: usize, height: usize, clip_level: f32, delta: Arc<[f16]>) -> Self {
        debug_assert_eq!(delta.len(), width * height);
        Self {
            width,
            height,
            data: Arc::from(Vec::new()),
            sharpen: Some(delta),
            sample: None,
            meta: DetailMeta {
                airlight: [1.0; 3],
                clip_level,
                sharpened: true,
            },
            wants: DetailWants {
                sharpen: true,
                ..Default::default()
            },
        }
    }

    /// These planes with the sharpen's `delta` beside them, the bands
    /// shared and not copied.
    pub fn with_sharpen(&self, delta: Arc<[f16]>) -> Self {
        debug_assert_eq!(delta.len(), self.width * self.height);
        Self {
            sharpen: Some(delta),
            wants: DetailWants {
                sharpen: true,
                ..self.wants
            },
            ..self.clone()
        }
    }

    /// `planes` with the range masks' `sample` beside them, or the
    /// sample alone when there are none; as they were without one, or
    /// with one of another size than theirs.
    pub fn with_sample(
        planes: Option<Arc<Self>>,
        sample: Option<Arc<WorkingImage>>,
        clip_level: f32,
    ) -> Option<Arc<Self>> {
        let Some(sample) = sample else {
            return planes;
        };
        let (w, h) = (sample.width, sample.height);
        let wants = |wants| DetailWants {
            sample: true,
            ..wants
        };
        Some(Arc::new(match planes {
            Some(p) if (p.width, p.height) == (w, h) => Self {
                sample: Some(sample),
                wants: wants(p.wants),
                ..(*p).clone()
            },
            Some(p) => {
                fits((p.width, p.height), w, h, "the range masks' sample");
                return Some(p);
            }
            None => Self {
                width: w,
                height: h,
                data: Arc::from(Vec::new()),
                sharpen: None,
                sample: Some(sample),
                meta: DetailMeta {
                    airlight: [1.0; 3],
                    clip_level,
                    sharpened: true,
                },
                wants: wants(DetailWants::default()),
            },
        }))
    }

    /// These planes beside a picture that was never sharpened: the
    /// delta is what the sharpen would have done ([`DetailMeta::sharpened`]).
    pub fn unsharpened(mut self) -> Self {
        self.meta.sharpened = false;
        self
    }

    /// Whether the four bands' lanes are held: some band was asked for.
    pub fn has_bands(&self) -> bool {
        !self.data.is_empty()
    }

    /// The planes as pictures, for the geometry and the resize to take
    /// as they take the picture ([`BandImages`]).
    pub fn to_image(&self) -> BandImages {
        self.to_image_reduced(1)
    }

    /// [`Self::to_image`] box-averaged by `factor` each way, as
    /// `reduce::reduce` averages the picture, without the full-size
    /// copy in between.
    pub fn to_image_reduced(&self, factor: usize) -> BandImages {
        let factor = factor.max(1);
        let size = if factor == 1 {
            (self.width, self.height)
        } else {
            ((self.width / factor).max(1), (self.height / factor).max(1))
        };
        let bands = self.has_bands();
        BandImages {
            size,
            lanes: bands.then(|| {
                let (data, width, height) = self.lanes_of(factor, [0, 1, 2]);
                WorkingImage {
                    width,
                    height,
                    data,
                }
            }),
            structure: (bands && self.wants.structure).then(|| {
                let (data, width, height) = self.lanes_of(factor, [3]);
                Plane {
                    width,
                    height,
                    data,
                }
            }),
            sharpen: self.sharpen.as_ref().map(|d| {
                let (data, width, height) = reduce_plane(d, self.width, self.height, factor);
                Plane {
                    width,
                    height,
                    data,
                }
            }),
            sample: self.sample.as_ref().map(|s| {
                if factor == 1 {
                    s.clone()
                } else {
                    Arc::new(crate::reduce::reduce(s, factor))
                }
            }),
        }
    }

    /// `N` of the four lanes interleaved, box-averaged by `factor`, and
    /// the size they come to.
    fn lanes_of<const N: usize>(
        &self,
        factor: usize,
        lanes: [usize; N],
    ) -> (Vec<f32>, usize, usize) {
        let factor = factor.max(1);
        if factor == 1 {
            let data = self
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|px| lanes.map(|k| px[k].to_f32()))
                .collect();
            return (data, self.width, self.height);
        }
        let (w, h) = ((self.width / factor).max(1), (self.height / factor).max(1));
        let mut data = vec![0.0f32; w * h * N];
        let n = (factor * factor) as f32;
        data.par_chunks_mut(w * N).enumerate().for_each(|(y, out)| {
            for (x, px) in out.as_chunks_mut::<N>().0.iter_mut().enumerate() {
                let mut sum = [0.0f32; N];
                for sy in y * factor..((y + 1) * factor).min(self.height) {
                    let row = &self.data[sy * self.width * 4..][..self.width * 4];
                    for sx in x * factor..((x + 1) * factor).min(self.width) {
                        let px = &row[sx * 4..sx * 4 + 4];
                        for (s, &k) in sum.iter_mut().zip(&lanes) {
                            *s += px[k].to_f32();
                        }
                    }
                }
                *px = sum.map(|s| s / n);
            }
        });
        (data, w, h)
    }
}

/// A plane of half floats `width` by `height` box-averaged by
/// `factor` each way, as the picture is, and the size it comes to.
fn reduce_plane(
    plane: &[f16],
    width: usize,
    height: usize,
    factor: usize,
) -> (Vec<f32>, usize, usize) {
    if factor <= 1 {
        return (plane.iter().map(|v| v.to_f32()).collect(), width, height);
    }
    let (w, h) = ((width / factor).max(1), (height / factor).max(1));
    let mut data = vec![0.0f32; w * h];
    let n = (factor * factor) as f32;
    data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        for (x, o) in out.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for sy in y * factor..((y + 1) * factor).min(height) {
                let row = &plane[sy * width..][..width];
                for v in &row[x * factor..((x + 1) * factor).min(width)] {
                    sum += v.to_f32();
                }
            }
            *o = sum / n;
        }
    });
    (data, w, h)
}

/// One band as a plane of its own, a value a pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// The planes brought into a frame by the steps the picture takes:
/// texture, clarity, and haze in the three channels of `lanes`, when a
/// mask asked for any band, and Structure's band and the sharpen's
/// delta, when a mask asked for them, each in a plane of its own, so
/// that a mask without one costs the geometry and the resize nothing
/// more than it did, and one with it a channel more.
#[derive(Debug, Clone, PartialEq)]
pub struct BandImages {
    /// The size they are all of.
    pub size: (usize, usize),
    pub lanes: Option<WorkingImage>,
    pub structure: Option<Plane>,
    pub sharpen: Option<Plane>,
    /// The range masks' picture ([`DetailPlanes::sample`]), taken
    /// through the same steps.
    pub sample: Option<Arc<WorkingImage>>,
}

impl Plane {
    /// The frame of `geometry` cut from it, as from a channel of the
    /// picture.
    /// At `at` when given, as [`crate::geometry::apply_plane_at`] cuts.
    fn framed(
        &self,
        geometry: &greycard_edit::geometry::Geometry,
        at: Option<(usize, usize)>,
    ) -> Result<Self, crate::geometry::TooLarge> {
        let (data, width, height) = match at {
            Some(size) => crate::geometry::apply_plane_at(
                &self.data,
                self.width,
                self.height,
                geometry,
                size,
            )?,
            None => crate::geometry::apply_plane(&self.data, self.width, self.height, geometry)?,
        };
        Ok(Plane {
            width,
            height,
            data,
        })
    }

    /// Resampled to `w` by `h` as a channel of the picture is.
    fn fit_exact(&self, w: u32, h: u32) -> Self {
        Plane {
            width: w as usize,
            height: h as usize,
            data: crate::export::fit_exact_plane(&self.data, self.width, self.height, w, h)
                .unwrap_or_else(|| self.data.clone()),
        }
    }
}

impl BandImages {
    pub fn width(&self) -> usize {
        self.size.0
    }

    pub fn height(&self) -> usize {
        self.size.1
    }

    /// The five bands at a pixel, in the planes' order: texture,
    /// clarity, haze, structure, and the sharpen's delta.
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> [f32; 5] {
        let [t, c, h] = self.lanes.as_ref().map_or([0.0; 3], |l| l.pixel(x, y));
        let one = |p: &Option<Plane>| p.as_ref().map_or(0.0, |p| p.data[y * p.width + x]);
        [t, c, h, one(&self.structure), one(&self.sharpen)]
    }

    /// The frame of `geometry` cut from each, as from the picture, or
    /// none of it when the frame is too large to hold.
    pub fn framed(
        &self,
        geometry: &greycard_edit::geometry::Geometry,
    ) -> Result<Self, crate::geometry::TooLarge> {
        self.framed_with(geometry, None)
    }

    /// [`Self::framed`] with the frame cut at `size`, as
    /// [`crate::geometry::apply_at`] cuts the picture.
    pub fn framed_at(
        &self,
        geometry: &greycard_edit::geometry::Geometry,
        size: (usize, usize),
    ) -> Result<Self, crate::geometry::TooLarge> {
        self.framed_with(geometry, Some(size))
    }

    fn framed_with(
        &self,
        geometry: &greycard_edit::geometry::Geometry,
        at: Option<(usize, usize)>,
    ) -> Result<Self, crate::geometry::TooLarge> {
        let cut = |i: &WorkingImage| match at {
            Some(size) => crate::geometry::apply_at(i, geometry, size),
            None => crate::geometry::apply(i, geometry),
        };
        let lanes = self.lanes.as_ref().map(cut).transpose()?;
        let structure = self
            .structure
            .as_ref()
            .map(|s| s.framed(geometry, at))
            .transpose()?;
        let sharpen = self
            .sharpen
            .as_ref()
            .map(|s| s.framed(geometry, at))
            .transpose()?;
        let sample = self
            .sample
            .as_ref()
            .map(|s| cut(s.as_ref()).map(Arc::new))
            .transpose()?;
        let size = lanes
            .as_ref()
            .map(|l| (l.width, l.height))
            .or_else(|| structure.as_ref().map(|s| (s.width, s.height)))
            .or_else(|| sharpen.as_ref().map(|s| (s.width, s.height)))
            .or_else(|| sample.as_ref().map(|s| (s.width, s.height)))
            .unwrap_or(self.size);
        Ok(Self {
            size,
            lanes,
            structure,
            sharpen,
            sample,
        })
    }

    /// Each resampled to `w` by `h` as the picture is, or none when
    /// they are that size already. The sharpen's delta is resampled as a
    /// channel is; [`Self::fit_exact_beside`] makes it from the picture.
    pub fn fit_exact(&self, w: u32, h: u32) -> Option<Self> {
        self.fit_exact_with(w, h, true, true)
    }

    /// [`Self::fit_exact`], the range masks' picture resampled too
    /// only `with_sample` and the sharpen's delta only `with_sharpen`,
    /// each left out otherwise.
    fn fit_exact_with(
        &self,
        w: u32,
        h: u32,
        with_sample: bool,
        with_sharpen: bool,
    ) -> Option<Self> {
        if self.size == (w as usize, h as usize) {
            return None;
        }
        Some(Self {
            size: (w as usize, h as usize),
            lanes: self
                .lanes
                .as_ref()
                .map(|l| crate::export::fit_exact(l, w, h).unwrap_or_else(|| l.clone())),
            structure: self.structure.as_ref().map(|s| s.fit_exact(w, h)),
            sharpen: self
                .sharpen
                .as_ref()
                .filter(|_| with_sharpen)
                .map(|s| s.fit_exact(w, h)),
            sample: self
                .sample
                .as_ref()
                .filter(|_| with_sample)
                .map(|s| crate::export::fit_exact(s, w, h).map_or_else(|| s.clone(), Arc::new)),
        })
    }

    /// [`Self::fit_exact`] beside `picture`, these planes' picture
    /// (`sharpened` or not), the range masks' picture only
    /// `with_sample`: the sharpen's delta is the one between the
    /// picture and the picture on the sharpen's other side, both
    /// resampled ([`fitted_delta`]), and is not resampled itself. A
    /// picture of another size than the planes' takes the delta
    /// resampled.
    pub fn fit_exact_beside(
        &self,
        w: u32,
        h: u32,
        with_sample: bool,
        picture: &WorkingImage,
        sharpened: bool,
    ) -> Option<Self> {
        let beside = (picture.width, picture.height) == self.size;
        let mut fitted = self.fit_exact_with(w, h, with_sample, !beside)?;
        if let Some(delta) = self.sharpen.as_ref().filter(|_| beside) {
            fitted.sharpen = Some(fitted_delta(picture, delta, sharpened, w, h));
        }
        Some(fitted)
    }

    /// Each no larger than `long_edge` on its long side, as the export
    /// fits the picture, the range masks' picture only `with_sample`;
    /// none when they already fit. [`Self::fit_exact_beside`].
    pub(crate) fn fit(
        &self,
        long_edge: Option<u32>,
        with_sample: bool,
        picture: &WorkingImage,
        sharpened: bool,
    ) -> Option<Self> {
        let (w, h) = crate::export::fit_size(self.width(), self.height(), long_edge)?;
        self.fit_exact_beside(w, h, with_sample, picture, sharpened)
    }
}

/// The sharpen's delta for `picture` resized to `w` by `h`: not
/// `delta` resized, but the delta between the two pictures resized,
/// this one and the one on the sharpen's other side (the picture
/// before the sharpen when this one is `sharpened`, the sharpened one
/// otherwise), each pixel's luminance on the pedestal. A delta resized
/// is a mean of stops: the sharpen moves a pixel up and its neighbor
/// down by about as much, and the mean of the two is near none where
/// the sharpen's mark on the resized picture is not. This one takes the
/// resized picture to the other resized picture, in luminance, at a
/// share of one: -1 over a sharpened picture is the unsharpened export
/// at that size, and +1 over one never sharpened the sharpened.
///
/// Both resized luminances are held at the pedestal from below, as the
/// sharpen holds the luminance it reads: dark noise and the filter's
/// lobes take a sum of them toward zero, where a ratio would run away
/// and the delta put on every channel would make a dark pixel a bright
/// colored dot. Exact in luminance only: one gain a pixel, as the
/// sharpen's own, where the resized pictures differ by a color too at a
/// saturated edge. Each luminance is resized as soon as it is made, so
/// one plane of the picture's size is held at a time.
fn fitted_delta(picture: &WorkingImage, delta: &Plane, sharpened: bool, w: u32, h: u32) -> Plane {
    let side = side(sharpened);
    let fit = |p: Vec<f32>| {
        crate::export::fit_exact_plane(&p, picture.width, picture.height, w, h).unwrap_or(p)
    };
    let pixels = picture.data.as_chunks::<3>().0;
    let there = fit(pixels
        .par_iter()
        .zip(delta.data.par_iter())
        .map(|(p, &d)| on_pedestal(p) * (side * d).exp2())
        .collect());
    let here = fit(pixels.par_iter().map(|p| on_pedestal(p)).collect());
    Plane {
        width: w as usize,
        height: h as usize,
        data: here
            .par_iter()
            .zip(there.par_iter())
            .map(|(&a, &b)| stops_between(a, b, side))
            .collect(),
    }
}

/// The way from a picture to the one on the sharpen's other side: back
/// over a picture `sharpened`, on over one that is not.
fn side(sharpened: bool) -> f32 {
    if sharpened { -1.0 } else { 1.0 }
}

/// A pixel's luminance on the sharpen's pedestal, which a delta scales.
fn on_pedestal(p: &[f32]) -> f32 {
    use greycard_core::develop::sharpen::{BLACK_PEDESTAL, LUMA};
    LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2] + BLACK_PEDESTAL
}

/// The delta from luminance `here` to `there`, both on the pedestal
/// and held at it from below, in stops the way [`side`] goes.
fn stops_between(here: f32, there: f32, side: f32) -> f32 {
    use greycard_core::develop::sharpen::BLACK_PEDESTAL;
    side * (there.max(BLACK_PEDESTAL) / here.max(BLACK_PEDESTAL)).log2()
}

/// `picture` taken to the sharpen's other side by `delta`, at its
/// size: the picture a mask's Sharpen of one reaches.
pub(crate) fn other_side(picture: &WorkingImage, delta: &Plane, sharpened: bool) -> WorkingImage {
    let side = side(sharpened);
    let mut other = picture.clone();
    other
        .data
        .par_chunks_mut(3)
        .zip(delta.data.par_iter())
        .for_each(|(p, &d)| p.copy_from_slice(&apply_delta([p[0], p[1], p[2]], side * d)));
    other
}

/// The sharpen's delta between `picture` and `other`, the picture on
/// its other side, both at one size: what each became after a step
/// both took (the export's output sharpen), so a mask's Sharpen of one
/// still reaches the other.
pub(crate) fn delta_between(
    picture: &WorkingImage,
    other: &WorkingImage,
    sharpened: bool,
) -> Plane {
    let side = side(sharpened);
    Plane {
        width: picture.width,
        height: picture.height,
        data: picture
            .data
            .par_chunks(3)
            .zip(other.data.par_chunks(3))
            .map(|(a, b)| stops_between(on_pedestal(a), on_pedestal(b), side))
            .collect(),
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

/// The masks' Detail at a pixel, summed by their weights: Texture's,
/// Clarity's, and Structure's stops a band, Dehaze's amount, and the
/// share of the sharpen's delta, each as the global op takes it (the
/// gain on the band, the strength on the haze), in the planes' order:
/// texture, clarity, dehaze, structure, sharpen. Two masks over one
/// pixel add their stops, and a Dehaze's amount and a Sharpen's share
/// are held to the slider's ends. None when every sum is
/// [`NEGLIGIBLE`].
#[inline]
pub fn amounts(locals: &[(&crate::finish::Baked, f32)]) -> Option<[f32; 5]> {
    let mut k = [0.0f32; 5];
    for (b, w) in locals {
        if b.detail != [0.0; 5] {
            for (k, d) in k.iter_mut().zip(b.detail) {
                *k += w * d;
            }
        }
    }
    let k = k.map(soft);
    (k != [0.0; 5]).then(|| {
        [
            k[0] * TEXTURE_GAIN,
            k[1] * CLARITY_GAIN,
            k[2].clamp(-1.0, 1.0) * dehaze::FULL_STRENGTH,
            k[3] * STRUCTURE_GAIN,
            k[4].clamp(-1.0, 1.0),
        ]
    })
}

/// A pixel with the masks' Detail put on: first the sharpen's delta at
/// the masks' share, the last thing the develop did undone or done
/// again first; then Texture's band as a gain under the fade toward
/// the clip, then Clarity's and Structure's with the fade read again
/// before each, as the global op puts them on, then the haze cleared
/// by the transmission at the strength, bounded as the global's apply
/// bounds it. `k` is what [`amounts`] makes and `bands` the planes at
/// the pixel. The shader's `detail_at` is the same arithmetic.
#[inline]
pub fn apply(px: [f32; 3], bands: [f32; 5], k: [f32; 5], meta: &DetailMeta) -> [f32; 3] {
    let mut px = px;
    let share = if meta.sharpened { k[4] } else { k[4].max(0.0) };
    if share != 0.0 && bands[4] != 0.0 {
        px = apply_delta(px, share * bands[4]);
    }
    for (gain, band) in [(k[0], bands[0]), (k[1], bands[1]), (k[3], bands[3])] {
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
            structure: d.structure != 0.0,
            sharpen: false,
            sample: false,
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
                ..Default::default()
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
        export_at(image, edit, planes, white, None)
    }

    /// [`export`] no larger than `long_edge`.
    fn export_at(
        image: &WorkingImage,
        edit: &greycard_edit::Edit,
        planes: Option<&DetailPlanes>,
        white: Option<&greycard_edit::WhiteShift>,
        long_edge: Option<u32>,
    ) -> Vec<u16> {
        let settings = crate::export::Settings {
            format: crate::export::Format::Tiff,
            sharpen: crate::export::Sharpen::Off,
            long_edge,
            ..Default::default()
        };
        export_with(image, edit, planes, white, &settings)
    }

    /// [`export`] under the sheet's `settings`.
    fn export_with(
        image: &WorkingImage,
        edit: &greycard_edit::Edit,
        planes: Option<&DetailPlanes>,
        white: Option<&greycard_edit::WhiteShift>,
        settings: &crate::export::Settings,
    ) -> Vec<u16> {
        let bands = planes.map(|p| p.to_image());
        sixteen(crate::export::render(
            image,
            edit,
            &greycard_edit::NoStores,
            (image.width as u32, image.height as u32),
            settings,
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
    /// the planes' half-float rounding: Texture, Clarity, Structure,
    /// and Dehaze each, in and out, and the bands together; and so under a
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
            (0.7, 0.0, 0.0, 0.0),
            (-0.5, 0.0, 0.0, 0.0),
            (0.0, 0.8, 0.0, 0.0),
            (0.0, -0.6, 0.0, 0.0),
            (-0.5, 0.8, 0.0, 0.0),
            (0.0, 0.0, 0.0, 1.0),
            (0.0, 0.0, 0.0, -0.7),
            (0.4, -0.3, 0.0, 0.9),
            (0.0, 0.0, 0.6, 0.0),
            (0.0, 0.0, -0.4, 0.0),
        ];
        let warm = greycard_edit::LocalWhite::Absolute {
            temperature: 3800.0,
            tint: 0.01,
        };
        for white in [greycard_edit::LocalWhite::Off, warm] {
            for (texture, clarity, dehaze, structure) in cases {
                let d = Detail {
                    enabled: true,
                    texture,
                    clarity,
                    structure,
                    dehaze,
                    sharpen: 0.0,
                    output_sharpen: 0.0,
                    output_sharpen_radius: None,
                    output_sharpen_threshold: None,
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
                ..Detail::default()
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
            structure: true,
            sharpen: false,
            sample: false,
        };
        let first = DetailPlanes::make(&image, None, CLIP, all).unwrap();
        let global = Some(LocalContrastOptions {
            texture: 0.4,
            clarity: -0.3,
            structure: 0.5,
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
                ..Detail::default()
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
        for (texture, clarity, dehaze, structure) in [
            (0.8, 0.0, 0.0, 0.0),
            (0.0, -0.7, 0.0, 0.0),
            (0.0, 0.0, 0.8, 0.0),
            (0.0, 0.0, 0.0, 0.9),
        ] {
            let d = Detail {
                enabled: true,
                texture,
                clarity,
                structure,
                dehaze,
                sharpen: 0.0,
                output_sharpen: 0.0,
                output_sharpen_radius: None,
                output_sharpen_threshold: None,
            };
            let half = Detail {
                enabled: true,
                texture: texture / 2.0,
                clarity: clarity / 2.0,
                structure: structure / 2.0,
                dehaze: dehaze / 2.0,
                sharpen: 0.0,
                output_sharpen: 0.0,
                output_sharpen_radius: None,
                output_sharpen_threshold: None,
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
                    let here = Some((bands.at(x, y), &planes.meta));
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
                structure: true,
                sharpen: false,
                sample: false,
            },
        )
        .unwrap();
        for k in [1, 2, 3, 5] {
            let a = planes.to_image_reduced(k);
            let whole = planes.to_image();
            let b = crate::reduce::reduce(whole.lanes.as_ref().unwrap(), k);
            assert_eq!(a.lanes.as_ref(), Some(&b), "by {k}");
            assert_eq!(a.size, (b.width, b.height));
            // Structure's plane as a picture of three channels of it,
            // reduced as the picture is.
            let s = whole.structure.unwrap();
            let s = WorkingImage {
                width: s.width,
                height: s.height,
                data: s.data.iter().flat_map(|&v| [v; 3]).collect(),
            };
            let s = crate::reduce::reduce(&s, k);
            let got = a.structure.unwrap();
            assert_eq!((got.width, got.height), (s.width, s.height));
            let want: Vec<f32> = s.data.chunks(3).map(|p| p[0]).collect();
            assert_eq!(got.data, want, "structure by {k}");
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
                dehaze,
                ..Detail::default()
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

    /// Structure's plane of one channel goes through the geometry and
    /// the resize as a channel of the picture does, to the bit.
    #[test]
    fn the_structure_plane_is_framed_and_fitted_as_a_channel_is() {
        let image = picture(61, 43);
        let planes = DetailPlanes::make(
            &image,
            None,
            CLIP,
            DetailWants {
                texture: true,
                structure: true,
                ..Default::default()
            },
        )
        .unwrap();
        // The lanes with Structure's band in the first channel, so the
        // two paths can be read against each other.
        let mut bands = planes.to_image();
        let plane = bands.structure.clone().unwrap();
        for (px, &v) in bands.lanes.as_mut().unwrap().pixels_mut().zip(&plane.data) {
            px[0] = v;
        }
        let first = |b: &BandImages| -> Vec<f32> {
            b.lanes
                .as_ref()
                .unwrap()
                .data
                .chunks(3)
                .map(|p| p[0])
                .collect()
        };
        let turned = greycard_edit::geometry::Geometry {
            angle: 5.0,
            crop: Some(greycard_edit::geometry::Crop {
                x: 0.1,
                y: 0.15,
                w: 0.7,
                h: 0.6,
            }),
            ..Default::default()
        };
        let framed = bands.framed(&turned).unwrap();
        let s = framed.structure.as_ref().unwrap();
        assert_eq!((s.width, s.height), (framed.width(), framed.height()));
        assert_eq!(s.data, first(&framed));
        // Cut smaller, as an export of a frame too large to hold cuts
        // it, the plane still goes as the channel does.
        let small = bands.framed_at(&turned, (21, 13)).unwrap();
        let s = small.structure.as_ref().unwrap();
        assert_eq!((s.width, s.height), (21, 13));
        assert_eq!(small.size, (21, 13));
        assert_eq!(s.data, first(&small));
        for (w, h) in [(30, 20), (90, 70)] {
            let fitted = framed.fit_exact(w, h).unwrap();
            assert_eq!(
                fitted.structure.as_ref().unwrap().data,
                first(&fitted),
                "{w}x{h}"
            );
        }
        assert!(
            framed
                .fit_exact(framed.width() as u32, framed.height() as u32)
                .is_none()
        );
    }

    /// The sharpen's delta goes through the geometry and the resize as
    /// a channel of the picture does, to the bit, beside the bands.
    #[test]
    fn the_sharpen_plane_is_framed_and_fitted_as_a_channel_is() {
        let image = picture(61, 43);
        let delta: Vec<f32> = (0..61 * 43)
            .map(|i| ((i * 37 % 101) as f32 / 101.0 - 0.5) * 0.6)
            .collect();
        let delta = sharpen_delta(&delta);
        for planes in [
            DetailPlanes::sharpen_only(61, 43, CLIP, delta.clone()),
            DetailPlanes::make(
                &image,
                None,
                CLIP,
                DetailWants {
                    texture: true,
                    ..Default::default()
                },
            )
            .unwrap()
            .with_sharpen(delta.clone()),
        ] {
            let bands = planes.to_image();
            let plane = bands.sharpen.clone().unwrap();
            let as_channel = WorkingImage {
                width: plane.width,
                height: plane.height,
                data: plane.data.iter().flat_map(|&v| [v; 3]).collect(),
            };
            let first =
                |im: &WorkingImage| -> Vec<f32> { im.data.chunks(3).map(|p| p[0]).collect() };
            let turned = greycard_edit::geometry::Geometry {
                angle: -4.0,
                crop: Some(greycard_edit::geometry::Crop {
                    x: 0.12,
                    y: 0.1,
                    w: 0.66,
                    h: 0.7,
                }),
                ..Default::default()
            };
            let framed = bands.framed(&turned).unwrap();
            let channel = crate::geometry::apply(&as_channel, &turned).unwrap();
            let s = framed.sharpen.as_ref().unwrap();
            assert_eq!((s.width, s.height), (framed.width(), framed.height()));
            assert_eq!((channel.width, channel.height), framed.size);
            assert_eq!(s.data, first(&channel));
            let fitted = framed.fit_exact(30, 20).unwrap();
            let channel = crate::export::fit_exact(&channel, 30, 20).unwrap();
            assert_eq!(fitted.sharpen.as_ref().unwrap().data, first(&channel));
            assert_eq!(fitted.size, (30, 20));
            // Reduced from the half floats as the picture of it is.
            let reduced = planes.to_image_reduced(3);
            let want = crate::reduce::reduce(&as_channel, 3);
            assert_eq!(reduced.sharpen.unwrap().data, first(&want));
            assert_eq!(reduced.size, (want.width, want.height));
            assert_eq!(reduced.lanes.is_some(), planes.has_bands());
        }
    }

    /// Every band of an export cut smaller, as a frame too large to
    /// hold is ([`crate::export::reduced_frame`], on a limit lowered to
    /// a quarter of a frame held whole here), is cut at the picture's
    /// size, and fitted is the bands of the whole frame fitted, each to
    /// near its own range.
    #[test]
    fn bands_cut_smaller_are_the_whole_frames_fitted() {
        use crate::export::{fit_size, reduced_frame};
        let (w, h) = (240, 160);
        let image = picture(w, h);
        let delta: Vec<f32> = (0..w * h)
            .map(|i| (((i % w) as f32 / 6.0).sin() * ((i / w) as f32 / 5.0).cos()) * 0.2)
            .collect();
        let planes = DetailPlanes::make(
            &image,
            None,
            CLIP,
            DetailWants {
                texture: true,
                clarity: true,
                structure: true,
                ..Default::default()
            },
        )
        .unwrap()
        .with_sharpen(sharpen_delta(&delta));
        let planes =
            DetailPlanes::with_sample(Some(Arc::new(planes)), Some(Arc::new(image.clone())), CLIP)
                .unwrap();
        let bands = planes.to_image();
        assert!(bands.lanes.is_some() && bands.structure.is_some());
        assert!(bands.sharpen.is_some() && bands.sample.is_some());
        let turned = greycard_edit::geometry::Geometry {
            angle: 7.0,
            vertical: 15.0,
            crop: Some(greycard_edit::geometry::Crop {
                x: 0.08,
                y: 0.1,
                w: 0.8,
                h: 0.75,
            }),
            ..Default::default()
        };
        let whole = bands.framed(&turned).unwrap();
        let (fw, fh) = whole.size;
        let edge = Some(48);
        let target = fit_size(fw, fh, edge).unwrap();
        let at = reduced_frame((fw, fh), edge, fw * fh / 4).unwrap();
        assert!(at.0 < fw && at.0.max(at.1) > 48, "{at:?} of {fw}x{fh}");
        let small = bands.framed_at(&turned, at).unwrap();
        // Each band at the size the picture is cut at.
        let picture = crate::geometry::apply_at(&image, &turned, at).unwrap();
        assert_eq!((picture.width, picture.height), at);
        assert_eq!(small.size, at);
        let lanes = small.lanes.as_ref().unwrap();
        let sample = small.sample.as_ref().unwrap();
        for (what, size) in [
            ("lanes", (lanes.width, lanes.height)),
            ("structure", {
                let s = small.structure.as_ref().unwrap();
                (s.width, s.height)
            }),
            ("sharpen", {
                let s = small.sharpen.as_ref().unwrap();
                (s.width, s.height)
            }),
            ("sample", (sample.width, sample.height)),
        ] {
            assert_eq!(size, at, "{what}");
        }
        // The range masks' sample is the picture, cut as it is.
        assert_eq!(sample.data, picture.data);
        let a = whole.fit_exact(target.0, target.1).unwrap();
        let b = small.fit_exact(target.0, target.1).unwrap();
        assert_eq!(
            (a.size, b.size),
            (b.size, (target.0 as usize, target.1 as usize))
        );
        // The mean and most difference over the band's largest value.
        let read = |a: &[f32], b: &[f32]| {
            let range = a.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let diffs: Vec<f32> = a.iter().zip(b).map(|(a, b)| (a - b).abs()).collect();
            let mean = diffs.iter().sum::<f32>() / diffs.len() as f32 / range;
            let most = diffs.iter().cloned().fold(0.0, f32::max) / range;
            (mean, most)
        };
        // A turn half a degree off, cut and fitted the same, to show
        // the bound tells a band out of place.
        let off = greycard_edit::geometry::Geometry {
            angle: 7.5,
            ..turned
        };
        let off = bands
            .framed_at(&off, at)
            .unwrap()
            .fit_exact(target.0, target.1)
            .unwrap();
        let of = |b: &BandImages| {
            [
                ("lanes", b.lanes.as_ref().unwrap().data.clone()),
                ("structure", b.structure.as_ref().unwrap().data.clone()),
                ("sharpen", b.sharpen.as_ref().unwrap().data.clone()),
                ("sample", b.sample.as_ref().unwrap().data.clone()),
            ]
        };
        for (((what, a), (_, b)), (_, o)) in of(&a).into_iter().zip(of(&b)).zip(of(&off)) {
            assert_eq!(a.len(), b.len(), "{what}");
            let (mean, most) = read(&a, &b);
            // Measured 0.0021 to 0.0042 on the mean and 0.020 to 0.10
            // at the most, the most on the sample's clip spikes and
            // fine pattern, where the cut's box and the whole frame's
            // Lanczos part; the turn half a degree off reads 0.012 to
            // 0.051 on the mean.
            assert!(mean < 0.005 && most < 0.15, "{what}: {mean} {most}");
            let (off, _) = read(&a, &o);
            assert!(off > 0.006, "{what}: {off}");
        }
    }

    /// The CPU reference of a mask's Sharpen: over the whole frame at
    /// -1 it takes the develop's sharpen off, and at +1 on the picture
    /// before the sharpen it puts it on, through the whole export, to
    /// the delta's half-float rounding; at -1 a pixel's luminance is
    /// the unsharpened one's to well under a thousandth; and off by
    /// its switch it is nothing.
    #[test]
    fn a_whole_frame_masks_sharpen_takes_the_sharpen_off_and_puts_it_on() {
        use greycard_core::develop::sharpen::{self, Radius, SharpenOptions, Threshold};
        let image = picture(160, 120);
        let options = SharpenOptions {
            radius: Radius::Fixed(0.9),
            contrast: Threshold::Fixed(0.0),
            ..Default::default()
        };
        let mut sharpened = image.clone();
        let (_, _, delta) = sharpen::sharpen_with_delta(&mut sharpened, None, &options, None, CLIP);
        let planes = DetailPlanes::sharpen_only(160, 120, CLIP, sharpen_delta(&delta));
        let masked = |amount: f32| {
            let mut e = greycard_edit::Edit::default();
            e.adjustments.push(everywhere(Detail {
                sharpen: amount,
                ..Detail::default()
            }));
            e
        };
        let mut off = masked(-1.0);
        off.adjustments[0].look.detail.enabled = false;
        let plain = export(&image, &off, None, None);
        let sharp = export(&sharpened, &off, None, None);
        assert_ne!(plain, sharp, "the sharpen did nothing");
        assert_eq!(export(&sharpened, &off, Some(&planes), None), sharp);
        let worst = |a: &[u16], b: &[u16]| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max();
        let taken = export(&sharpened, &masked(-1.0), Some(&planes), None);
        assert!(
            worst(&taken, &plain) <= Some(10),
            "{:?}",
            worst(&taken, &plain)
        );
        let put = export(&image, &masked(1.0), Some(&planes), None);
        assert!(worst(&put, &sharp) <= Some(10), "{:?}", worst(&put, &sharp));
        // The luminance itself, at the pixel, as the finish takes it.
        let bands = planes.to_image();
        let local = crate::finish::Baked::of(&masked(-1.0).adjustments[0].look);
        let k = amounts(&[(&local, 1.0)]).unwrap();
        let luma = |p: [f32; 3]| {
            sharpen::LUMA[0] * p[0] + sharpen::LUMA[1] * p[1] + sharpen::LUMA[2] * p[2]
        };
        let mut most = 0f32;
        for y in 0..image.height {
            for x in 0..image.width {
                let back = apply(sharpened.pixel(x, y), bands.at(x, y), k, &planes.meta);
                most = most.max((luma(back) - luma(image.pixel(x, y))).abs());
            }
        }
        assert!(most < 1e-4, "{most}");
        // Beside a picture never sharpened the share is never less
        // than none: -1 is nothing, to the bit, and +1 still the
        // sharpen.
        let unsharpened = planes.clone().unsharpened();
        assert_eq!(
            export(&image, &masked(-1.0), Some(&unsharpened), None),
            plain
        );
        let put = export(&image, &masked(1.0), Some(&unsharpened), None);
        assert!(worst(&put, &sharp) <= Some(10), "{:?}", worst(&put, &sharp));
    }

    /// `picture` without its color, and a sharpened copy with the
    /// sharpen's delta, for the checks of a mask's Sharpen made smaller.
    fn neutral_sharpened(w: usize, h: usize) -> (WorkingImage, WorkingImage, DetailPlanes) {
        neutral_sharpened_at(w, h, 1.0)
    }

    /// [`neutral_sharpened`] with the picture's contrast about its
    /// middle grey times `contrast`.
    fn neutral_sharpened_at(
        w: usize,
        h: usize,
        contrast: f32,
    ) -> (WorkingImage, WorkingImage, DetailPlanes) {
        use greycard_core::develop::sharpen::{self, Radius, SharpenOptions, Threshold};
        let mut image = picture(w, h);
        for px in image.data.chunks_mut(3) {
            let y = (px[0] + px[1] + px[2]) / 3.0;
            px.fill(0.18 + (y - 0.18) * contrast);
        }
        let options = SharpenOptions {
            radius: Radius::Fixed(0.9),
            contrast: Threshold::Fixed(0.0),
            ..Default::default()
        };
        let mut sharpened = image.clone();
        let (_, _, delta) = sharpen::sharpen_with_delta(&mut sharpened, None, &options, None, CLIP);
        let planes = DetailPlanes::sharpen_only(w, h, CLIP, sharpen_delta(&delta));
        (image, sharpened, planes)
    }

    /// The sharpen's delta fitted for an export made smaller takes the
    /// fitted picture's luminance to the other fitted picture's, on
    /// the pedestal, to the half floats' rounding: the sharpened one
    /// back to the unsharpened, and the unsharpened on to the
    /// sharpened. Resizing the delta itself missed by tens of times
    /// that.
    #[test]
    fn the_fitted_delta_is_the_delta_of_the_fitted_pictures() {
        use greycard_core::develop::sharpen::{BLACK_PEDESTAL, LUMA};
        let (image, sharpened, planes) = neutral_sharpened(160, 120);
        let luma = |p: &[f32]| LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2] + BLACK_PEDESTAL;
        for (from, to, sharpened_here) in [(&sharpened, &image, true), (&image, &sharpened, false)]
        {
            let bands = planes.to_image();
            let fitted = bands.fit(Some(40), false, from, sharpened_here).unwrap();
            let (w, h) = (fitted.width() as u32, fitted.height() as u32);
            let delta = &fitted.sharpen.as_ref().unwrap().data;
            let a = crate::export::fit_exact(from, w, h).unwrap();
            let b = crate::export::fit_exact(to, w, h).unwrap();
            let side = if sharpened_here { -1.0 } else { 1.0 };
            // Where the filter rings under the pedestal the delta is
            // held to it, as the sharpen holds what it reads.
            let (mut most, mut none) = (0f32, 0);
            for ((a, b), d) in a.data.chunks(3).zip(b.data.chunks(3)).zip(delta) {
                if luma(a) < BLACK_PEDESTAL || luma(b) < BLACK_PEDESTAL {
                    none += 1;
                    continue;
                }
                most = most.max((luma(a) * (side * d).exp2() / luma(b) - 1.0).abs());
            }
            assert!(most < 2e-3, "sharpened {sharpened_here}: {most}");
            assert!(none < 8, "{none}");
        }
    }

    /// A mask's Sharpen in an export made smaller does there what it
    /// does at full size: over the whole frame at -1 it takes the
    /// develop's sharpen off, and at +1 on a picture never sharpened
    /// it puts it on, each against the export at that size of the
    /// other picture; the sharpen itself moves that export many times
    /// as far.
    #[test]
    fn a_masks_sharpen_holds_in_an_export_made_smaller() {
        let (image, sharpened, planes) = neutral_sharpened(160, 120);
        let masked = |amount: f32| {
            let mut e = greycard_edit::Edit::default();
            e.adjustments.push(everywhere(Detail {
                sharpen: amount,
                ..Detail::default()
            }));
            e
        };
        let mean = |a: &[u16], b: &[u16]| {
            a.iter()
                .zip(b)
                .map(|(a, b)| f64::from(a.abs_diff(*b)))
                .sum::<f64>()
                / a.len() as f64
        };
        for edge in [80, 40] {
            let at = |image: &WorkingImage, edit: &greycard_edit::Edit, planes| {
                export_at(image, edit, planes, None, Some(edge))
            };
            let plain = at(&image, &greycard_edit::Edit::default(), None);
            let sharp = at(&sharpened, &greycard_edit::Edit::default(), None);
            let apart = mean(&plain, &sharp);
            let taken = mean(&at(&sharpened, &masked(-1.0), Some(&planes)), &plain);
            let unsharpened = planes.clone().unsharpened();
            let put = mean(&at(&image, &masked(1.0), Some(&unsharpened)), &sharp);
            eprintln!("at {edge}: the sharpen {apart:.1}, taken off {taken:.2}, put on {put:.2}");
            assert!(apart > 100.0, "at {edge}: {apart}");
            assert!(taken < apart / 10.0, "at {edge}: {taken} of {apart}");
            assert!(put < apart / 10.0, "at {edge}: {put} of {apart}");
        }
    }

    /// A whole-frame mask's Sharpen in a smaller export of a dark,
    /// noisy frame, as high-ISO shadows are after the matrix (a channel
    /// under zero at most pixels): at -1 over the sharpened frame and at
    /// +1 over the frame never sharpened, no pixel lands further from
    /// the other picture's export than the sharpen itself moves any
    /// pixel there, by half again. With the resized luminances not held
    /// at the pedestal, a ratio of two sums near zero made dots of tens
    /// of thousands of levels.
    #[test]
    fn a_masks_sharpen_makes_no_dots_in_dark_noise_made_smaller() {
        use greycard_core::develop::sharpen::{self, Radius, SharpenOptions, Threshold};
        let (w, h) = (240usize, 180usize);
        let hash = |i: usize| {
            let mut z = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z ^= z >> 31;
            z = z.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z ^= z >> 29;
            (z % 100_000) as f32 / 100_000.0
        };
        let mut image = WorkingImage::new(w, h);
        for (i, v) in image.data.iter_mut().enumerate() {
            let p = i / 3;
            let (x, y) = ((p % w) as f32, (p / w) as f32);
            let level = 0.0006 + 0.0114 * ((x / 23.0).sin() * (y / 17.0).cos() + 1.0) / 2.0;
            *v = level + 0.024 * (hash(i) - 0.5);
        }
        let under = image
            .data
            .chunks(3)
            .filter(|p| p.iter().any(|&v| v < 0.0))
            .count();
        assert!(under * 2 > w * h, "{under} of {}", w * h);
        let options = SharpenOptions {
            radius: Radius::Fixed(0.9),
            contrast: Threshold::Fixed(0.0),
            ..Default::default()
        };
        let mut sharpened = image.clone();
        let (_, _, delta) = sharpen::sharpen_with_delta(&mut sharpened, None, &options, None, CLIP);
        let planes = DetailPlanes::sharpen_only(w, h, CLIP, sharpen_delta(&delta));
        let masked = |amount: f32| {
            let mut e = greycard_edit::Edit::default();
            e.adjustments.push(everywhere(Detail {
                sharpen: amount,
                ..Detail::default()
            }));
            e
        };
        let at = |image: &WorkingImage, edit: &greycard_edit::Edit, planes| {
            export_at(image, edit, planes, None, Some(120))
        };
        let worst = |a: &[u16], b: &[u16]| {
            a.iter()
                .zip(b)
                .map(|(a, b)| u32::from(a.abs_diff(*b)))
                .max()
                .unwrap()
        };
        let plain = at(&image, &greycard_edit::Edit::default(), None);
        let sharp = at(&sharpened, &greycard_edit::Edit::default(), None);
        let own = worst(&plain, &sharp);
        let taken = worst(&at(&sharpened, &masked(-1.0), Some(&planes)), &plain);
        let unsharpened = planes.clone().unsharpened();
        let put = worst(&at(&image, &masked(1.0), Some(&unsharpened)), &sharp);
        eprintln!("the sharpen at most {own}; taken off, worst {taken}; put on, worst {put}");
        assert!(own > 1000, "{own}");
        assert!(taken * 2 <= own * 3, "taken off: {taken} against {own}");
        assert!(put * 2 <= own * 3, "put on: {put} against {own}");
    }

    /// A mask's Sharpen in a smaller export with the output sharpen on:
    /// the delta is made of the resized pictures before the output
    /// sharpen and put on after it, and over the whole frame at -1 the
    /// export is the unsharpened frame's under the same output sharpen,
    /// to a fraction of a level.
    #[test]
    fn a_masks_sharpen_holds_beside_the_output_sharpen() {
        // At half the contrast, so the sharpened picture resized never
        // rings under black, where no gain reaches the other picture.
        let (image, sharpened, planes) = neutral_sharpened_at(160, 120, 0.5);
        let mut masked = greycard_edit::Edit::default();
        masked.adjustments.push(everywhere(Detail {
            sharpen: -1.0,
            ..Detail::default()
        }));
        let mean = |a: &[u16], b: &[u16]| {
            a.iter()
                .zip(b)
                .map(|(a, b)| f64::from(a.abs_diff(*b)))
                .sum::<f64>()
                / a.len() as f64
        };
        for output in [
            crate::export::Sharpen::Standard,
            crate::export::Sharpen::High,
        ] {
            let settings = crate::export::Settings {
                format: crate::export::Format::Tiff,
                sharpen: output,
                long_edge: Some(80),
                ..Default::default()
            };
            let at = |image: &WorkingImage, edit: &greycard_edit::Edit, planes| {
                export_with(image, edit, planes, None, &settings)
            };
            let plain = at(&image, &greycard_edit::Edit::default(), None);
            let sharp = at(&sharpened, &greycard_edit::Edit::default(), None);
            let apart = mean(&plain, &sharp);
            let taken = mean(&at(&sharpened, &masked, Some(&planes)), &plain);
            eprintln!("{output:?}: the sharpen {apart:.1}, taken off {taken:.2}");
            assert!(apart > 100.0, "{output:?}: {apart}");
            assert!(taken < apart / 100.0, "{output:?}: {taken} of {apart}");
        }
    }

    /// The bands keep their sign through the export's resize: a band
    /// of stops under zero is the same under zero made smaller.
    #[test]
    fn the_bands_keep_their_sign_through_the_resize() {
        let planes = DetailPlanes::make(
            &picture(64, 48),
            None,
            CLIP,
            DetailWants {
                texture: true,
                structure: true,
                ..Default::default()
            },
        )
        .unwrap()
        .with_sharpen(sharpen_delta(&vec![-0.3; 64 * 48]));
        let mut bands = planes.to_image();
        bands.lanes.as_mut().unwrap().data.fill(-0.4);
        bands.structure.as_mut().unwrap().data.fill(-0.2);
        let fitted = bands.fit_exact(16, 12).unwrap();
        let near = |data: &[f32], v: f32| data.iter().all(|d| (d - v).abs() < 1e-4);
        assert!(near(&fitted.lanes.unwrap().data, -0.4));
        assert!(near(&fitted.structure.unwrap().data, -0.2));
        assert!(near(&fitted.sharpen.unwrap().data, -0.3));
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
        // Structure's lane is nothing, and no picture of it is made.
        assert!(planes.data.chunks(4).all(|p| p[3] == f16::ZERO));
        assert!(planes.to_image().structure.is_none());
    }
}
