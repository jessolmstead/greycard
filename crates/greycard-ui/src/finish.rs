//! The viewport's transform on the CPU: exposure, contrast, the tone
//! shifts, the black point, the tone curve, the look table, the matrix
//! to the output space and the sRGB encoding, over a whole image, for
//! export, with local adjustments blended in where their masks say.
//! The reference the shader in `viewport.wgsl` is held to.

use std::sync::Arc;

use greycard_core::image::WorkingImage;
use greycard_core::lut;
use greycard_edit::brush::Raster;
use greycard_edit::curve::{CurveLut, color_shift, lookup};
use greycard_edit::mask::Sample;
use greycard_edit::mixer::{BANDS, MEAN_RADIUS, confidence};
use greycard_edit::{BlackWhite, Color, Edit, Grain, Light, Look, Mask, Mixer, Tint};
use rayon::prelude::*;

/// A look baked for the finish: its light, its mixer, its global
/// color, the black and white conversion, its tint and its tables.
#[derive(Debug, Clone, PartialEq)]
pub struct Baked {
    pub light: Light,
    pub mixer: Mixer,
    pub color: Color,
    /// Global only: a local's is never read (`finish_pixel_with`),
    /// so it is [`BlackWhite::OFF`] on everything but the picture's
    /// own look.
    pub bw: BlackWhite,
    /// The tint toward one hue, last in the Oklab pass: a look's, so
    /// a mask carries one.
    pub tint: Tint,
    pub curves: CurveLut,
    /// Global only, as the black and white is: what the picture under
    /// the look is, which decides the baseline and the display curve.
    pub source: Source,
}

/// What the finish is handed. A raw's develop is a scene, which the
/// finish brightens by [`BASELINE_EXPOSURE`] and takes to a display
/// through the curve. A JPEG, PNG or TIFF is a picture something has
/// already rendered for a display (§50): it takes neither, so a
/// picture with nothing done to it comes out as it went in, and the
/// Light sliders act on it as they do on a scene, clipped at white
/// where the curve would have rolled off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Scene,
    Display,
}

impl Source {
    /// Stops the exposure slider's zero stands for.
    pub fn baseline(self) -> f32 {
        match self {
            Source::Scene => BASELINE_EXPOSURE,
            Source::Display => 0.0,
        }
    }

    /// The display curve, or a clip at white for a picture that has
    /// been through one already.
    #[inline]
    fn curve(self, x: f32) -> f32 {
        match self {
            Source::Scene => tone(x),
            Source::Display => x.clamp(0.0, 1.0),
        }
    }
}

impl Baked {
    /// A look on its own: a local adjustment's, or the global one
    /// where the black and white does not apply.
    pub fn of(look: &Look) -> Self {
        Self {
            light: look.light.effective(),
            mixer: look.mixer,
            color: look.color,
            bw: BlackWhite::OFF,
            tint: look.tint,
            curves: look.curves.bake_with(&look.grading),
            source: Source::Scene,
        }
    }

    /// The picture's global look, the black and white with it: that
    /// section is the edit's, not a look's, since a mask carries a
    /// look and a picture is mono or it is not. The mixer comes
    /// through `Edit::acting_mixer`, which is nothing while the
    /// conversion is on.
    pub fn global(edit: &Edit, source: Source) -> Self {
        Self {
            bw: edit.bw,
            mixer: edit.acting_mixer(),
            source,
            ..Self::of(&edit.look())
        }
    }
}

/// A brush's raster, shared: the same one when it is the same
/// allocation at the same version, which is all a copy needs to know.
#[derive(Debug, Clone)]
pub struct RasterRef(pub Arc<Raster>);

impl PartialEq for RasterRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) && self.0.version == other.0.version
    }
}

/// A local adjustment baked: its look, and where it acts.
#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub baked: Baked,
    pub mask: Mask,
    pub enabled: bool,
    /// The mask's brushes, one entry per component, painted.
    pub rasters: Vec<Option<RasterRef>>,
}

impl Local {
    /// The mask's value at (u, v), its brushes from their rasters.
    #[cfg(test)]
    pub fn weight(&self, u: f32, v: f32) -> f32 {
        self.weight_sampled(u, v, None)
    }

    /// The mask's value at (u, v), its brushes from their rasters and
    /// its range shapes from the picture's `sample` there ([`sample`]).
    pub fn weight_sampled(&self, u: f32, v: f32, sample: Option<Sample>) -> f32 {
        self.mask.at_sampled(u, v, sample, |i, u, v| {
            self.rasters
                .get(i)
                .and_then(|r| r.as_ref())
                .map_or(0.0, |r| r.0.at(u, v))
        })
    }

    /// Whether this local's mask reads the picture, switched on.
    pub fn reads_picture(&self) -> bool {
        self.enabled && self.mask.reads_picture()
    }

    /// Whether it reads the picture's color, switched on.
    pub fn reads_color(&self) -> bool {
        self.enabled && self.mask.reads_color()
    }
}

/// The picture at a pixel as the range masks read it
/// ([`greycard_edit::mask::Sample`]): `px`, a pixel of the picture the
/// finish is handed, at `exposure` stops, to Oklab; its lightness its
/// own, not clipped, its a and b `ab`, the mean about it
/// ([`local_ab`]), brought to that exposure, when given, else its own.
///
/// What that pixel has been through: the whole develop — the white
/// balance, the camera profile, the denoise, the lens corrections, the
/// Detail section (texture, clarity, dehaze) and the capture sharpen —
/// and the geometry, and in an export made smaller the resize, but not
/// the output sharpen after it (`export::render` hands the finish the
/// picture from before it). None of the look: not the tone controls,
/// the curves, the mixer, the color, the tint, a look table, the
/// vignette or any local. So a range mask never moves under its own
/// adjustment or another's, and the Detail section moves one a little:
/// dehaze at 0.6 moved 6.3% of the pixels of a sky window (70 to 100,
/// fading 6) on `3G0A4650`, its coverage 36.7% to 30.5%.
///
/// `exposure` is the global exposure alone, not the baseline a raw is
/// shown brighter by (`Source::baseline`), so a lightness of 1 is the
/// sensor's white at an exposure of nothing and the highlights above
/// the display's white stay apart on the scale.
pub fn sample(px: [f32; 3], ab: Option<[f32; 2]>, exposure: f32) -> Sample {
    let gain = 2f32.powf(exposure);
    let lab = oklab(px.map(|v| v * gain));
    let [a, b] = ab.map_or([lab[1], lab[2]], |ab| ab.map(|v| v * gain.cbrt()));
    Sample {
        lightness: lab[0],
        a,
        b,
    }
}

/// A mask's rasters, one entry per component: its brushes painted
/// from nothing for a picture `aspect` (height over width) tall, its
/// learned shapes from `learned` by component index. A component
/// switched off is nothing, and its brush is not painted at all.
pub fn rasterize(
    mask: &Mask,
    aspect: f32,
    learned: impl Fn(usize) -> Option<Arc<Raster>>,
) -> Vec<Option<RasterRef>> {
    mask.components
        .iter()
        .enumerate()
        .map(|(i, c)| match &c.shape {
            _ if !c.enabled => None,
            greycard_edit::mask::Shape::Brush { strokes } => {
                Some(RasterRef(Arc::new(Raster::of(strokes, aspect))))
            }
            s if s.is_learned() => learned(i).map(RasterRef),
            _ => None,
        })
        .collect()
}

/// How many local adjustments a picture may carry: the shader's
/// loop bound, and the size of a per-pixel array there. Nothing is
/// paid for the ones not used.
pub const MAX_LOCALS: usize = 16;

const MID_GREY: f32 = 0.18;

/// Stops every raw is brightened by before its own exposure: the
/// exposure slider at zero is this, so a raw with nothing done to it
/// lands where the camera's JPEG and Lightroom put it rather than 0.8
/// stops under both (notes §141). Added where the exposure turns into
/// a gain, here, in [`pick`] and in the viewport's uniform, so it acts
/// exactly as the slider does, the guide plane included. Never to a
/// picture that is not a raw, which is the camera's JPEG or the like
/// already ([`Source::baseline`]).
pub const BASELINE_EXPOSURE: f32 = 0.8;
/// Luminance weights of the working space, Rec.2020.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// About how long the guide plane's long edge is, in its own texels.
/// The plane is a smoothed log luminance, so it carries nothing at the
/// picture's own scale; holding its size fixed makes the radius and
/// the regularizer below mean the same thing on a 24 MP frame as on a
/// 45 MP one, and keeps it small enough to hand the GPU whole.
const GUIDE_EDGE: usize = 1024;
/// The guided filter's window, in guide texels: a fortieth of the long
/// edge each way, so the region a pixel is read as part of is about a
/// twentieth of the frame across.
const GUIDE_RADIUS: usize = 25;
/// Its regularizer, in stops squared: the knee between what is
/// texture and what is a region. A window whose log luminance varies
/// by about its square root — a stop and a half — keeps half its
/// shape in the guide, less below and more above, so the plane is a
/// map of the scene's regions rather than a picture of it.
///
/// Measured on the lighthouse frame: at 0.6 the plane is the
/// picture at an eighth scale and the shift follows the water's
/// sparkle; at 5 and above a dark object smaller than the window is
/// read as part of the sky behind it and the halo around it doubles.
/// Two is where a tree line and a rock mass keep their edges and the
/// texture inside them does not.
const GUIDE_EPS: f32 = 2.0;
/// The floor of the guide's log luminance, in stops under mid grey:
/// below this is under any sensor's noise, and without it a pixel at
/// zero would drag a whole window down.
const GUIDE_FLOOR: f32 = -12.0;

/// Where the tone equalizer reads a pixel's place in the scene: the
/// log luminance over mid grey of the developed picture, in stops,
/// smoothed by the guided filter so that texture is gone and a region
/// boundary is not, at a reduced scale ([`GUIDE_EDGE`]).
///
/// Of the picture before any exposure, as [`local_ab`] is: the finish
/// adds the exposure at the pixel and takes the contrast's power,
/// which in stops is a shift and a scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    pub width: usize,
    pub height: usize,
    /// Source pixels to a guide texel each way.
    pub scale: usize,
    pub data: Vec<f32>,
}

impl Guide {
    /// A plane that has not been made yet: nothing at all. The worker
    /// holds one on a base until `guide_plane` has run, and the two
    /// places that take a plane — `export::render` and
    /// `Renderer::set_guide` — both drop an empty one and fall back to
    /// the pixel's own luminance, which is the honest answer for a
    /// plane that does not exist. [`Guide::at`] would say mid grey
    /// everywhere, which is not, so nothing is meant to call it here.
    pub const NONE: Self = Self {
        width: 0,
        height: 0,
        scale: 1,
        data: Vec::new(),
    };

    /// The guide at a continuous source position, bilinear, the edges
    /// clamped — the convention the shader's sampler uses, so the two
    /// read one plane: a source position of `scale * (i + 0.5)` is
    /// texel `i`'s center.
    pub fn at(&self, x: f32, y: f32) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let s = self.scale as f32;
        let (gx, gy) = (x / s - 0.5, y / s - 0.5);
        let (x0, y0) = (gx.floor(), gy.floor());
        let (fx, fy) = (gx - x0, gy - y0);
        let cx = |v: f32| (v as isize).clamp(0, self.width as isize - 1) as usize;
        let cy = |v: f32| (v as isize).clamp(0, self.height as isize - 1) as usize;
        let (xa, xb) = (cx(x0), cx(x0 + 1.0));
        let (ya, yb) = (cy(y0), cy(y0 + 1.0));
        let (a, b) = (
            self.data[ya * self.width + xa],
            self.data[ya * self.width + xb],
        );
        let (c, d) = (
            self.data[yb * self.width + xa],
            self.data[yb * self.width + xb],
        );
        let top = a + (b - a) * fx;
        let bottom = c + (d - c) * fx;
        top + (bottom - top) * fy
    }
}

/// The guide plane of a developed picture: the mean log luminance of
/// each cell of the reduced grid, then the guided filter against
/// itself.
///
/// The mean is of the log, not the log of the mean: a geometric mean,
/// so a specular speck in a cell moves the region's reading by its
/// share of the cell and not by its height.
pub fn guide_plane(image: &WorkingImage) -> Guide {
    let (w, h) = (image.width, image.height);
    let scale = w.max(h).div_ceil(GUIDE_EDGE).max(1);
    let (gw, gh) = (w.div_ceil(scale), h.div_ceil(scale));
    let mut small = vec![0.0f32; gw * gh];
    small.par_chunks_mut(gw).enumerate().for_each(|(gy, out)| {
        let (y0, y1) = (gy * scale, ((gy + 1) * scale).min(h));
        for (gx, o) in out.iter_mut().enumerate() {
            let (x0, x1) = (gx * scale, ((gx + 1) * scale).min(w));
            let mut acc = 0.0f32;
            for y in y0..y1 {
                let row = &image.data[y * w * 3..(y + 1) * w * 3];
                for px in row.as_chunks::<3>().0[x0..x1].iter() {
                    let y = LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2];
                    acc += (y.max(1e-6) / MID_GREY).log2().max(GUIDE_FLOOR);
                }
            }
            *o = acc / ((x1 - x0) * (y1 - y0)) as f32;
        }
    });
    let data = greycard_core::guided::filter(&small, None, gw, gh, GUIDE_RADIUS, GUIDE_EPS);
    Guide {
        width: gw,
        height: gh,
        scale,
        data,
    }
}

/// One working-space pixel through `to_out` (the working space to the
/// output's linear primaries, rows) to its sRGB-encoded value, as the
/// shader does it for sRGB. The point curves act on the encoded
/// working-space value, before the matrix, so every output space sees
/// the same picture; the color curves after them, in Oklab.
///
/// `locals` are the local adjustments with their masks' values at
/// this pixel. Each blends into the parameters by its weight: stops
/// and shifts add, contrast adds its excess over one, the mixer's
/// bands add, a point curve adds its departure from the line before
/// the global curve, and the color shifts add.
#[cfg(test)]
pub fn finish_pixel(
    px: [f32; 3],
    global: &Baked,
    locals: &[(&Baked, f32)],
    to_out: &[[f32; 3]; 3],
) -> [f32; 3] {
    finish_pixel_with(px, global, locals, 0.0, None, None, None, None, to_out)
}

/// `finish_pixel` through a look table. The tests' way in to the one
/// stage `finish_pixel` leaves out.
#[cfg(test)]
pub fn finish_pixel_look(
    px: [f32; 3],
    global: &Baked,
    look: Option<&lut::Look>,
    to_out: &[[f32; 3]; 3],
) -> [f32; 3] {
    finish_pixel_with(px, global, &[], 0.0, None, None, None, look, to_out)
}

/// `finish_pixel` with `stops` more exposure at this pixel, the
/// vignette's, and `grain` laid on the encoded output, both places in
/// the frame rather than a look; `reference`, the mean Oklab a
/// and b of the source about this pixel (`local_ab`), which the
/// mixer and the black and white read their hue and their confidence
/// from when it is given; and `guide`, the smoothed log luminance of
/// the source there ([`Guide`]), which the three exposure shifts read
/// their region from.
///
/// Without a guide the shifts read the pixel's own luminance, which is
/// what §19 shipped and what this now falls back to: the droppers'
/// [`pick`], which reads one pixel and has no plane, and the tests
/// that call `finish_pixel` on a color alone.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn finish_pixel_with(
    px: [f32; 3],
    global: &Baked,
    locals: &[(&Baked, f32)],
    stops: f32,
    grain: Option<[f32; 3]>,
    reference: Option<[f32; 2]>,
    guide: Option<f32>,
    look: Option<&lut::Look>,
    to_out: &[[f32; 3]; 3],
) -> [f32; 3] {
    let mut exposure = global.source.baseline() + global.light.exposure + stops;
    let mut t = global.light.tone;
    let mut mixer = global.mixer.effective();
    let mut color = global.color.effective();
    // The tint adds as a vector, `amount` in the hue's direction, so
    // two masks at half a hue each make one at the hue between them.
    let mut tint = global.tint.vector();
    let mut shaded = global.curves.shaded;
    for (b, w) in locals {
        exposure += w * b.light.exposure;
        t.contrast += w * (b.light.tone.contrast - 1.0);
        t.highlights += w * b.light.tone.highlights;
        t.shadows += w * b.light.tone.shadows;
        t.whites += w * b.light.tone.whites;
        t.blacks += w * b.light.tone.blacks;
        if b.mixer.enabled {
            mixer.enabled = true;
            for i in 0..BANDS {
                mixer.hue[i] += w * b.mixer.hue[i];
                mixer.saturation[i] += w * b.mixer.saturation[i];
                mixer.luminance[i] += w * b.mixer.luminance[i];
            }
        }
        if b.color.enabled {
            color.enabled = true;
            color.saturation += w * b.color.saturation;
            color.vibrance += w * b.color.vibrance;
        }
        let v = b.tint.vector();
        tint[0] += w * v[0];
        tint[1] += w * v[1];
        shaded |= b.curves.shaded;
    }
    let tint = Tint::from_vector(tint);
    let gain = 2f32.powf(exposure);
    let mut c = px.map(|v| v * gain);
    // The black and white is the picture's, not a local's: a mask
    // carries a look, and half a mono picture is not one.
    let bw = global.bw.effective();
    if mixer.enabled || color.enabled || bw.enabled || !tint.is_identity() {
        // Oklab's a and b scale with the cube root of a gain, so the
        // source's mean is the exposed picture's mean, scaled.
        let reference = reference.map(|ab| ab.map(|v| v * gain.cbrt()));
        c = mix_with(c, &mixer, &color, &bw, &tint, reference);
    }
    if global.light.tone.enabled {
        // The guide is the scene's, before any exposure: the exposure
        // at this pixel is a shift of it in stops and the contrast a
        // scale, which is what the power about mid grey is in stops.
        let g = guide.map(|g| t.contrast * (g + exposure));
        c = shape(c, &t, g).map(|x| global.source.curve(x));
    } else {
        c = c.map(|v| v.min(1.0));
    }
    c = [0, 1, 2].map(|k| {
        let x = encode(c[k].clamp(0.0, 1.0));
        let mut x2 = x;
        for (b, w) in locals {
            x2 += w * (lookup(&b.curves, k, x) - x);
        }
        decode(lookup(&global.curves, k, x2.clamp(0.0, 1.0)))
    });
    if shaded {
        c = shade_by(c, |l| {
            let mut shift = color_shift(&global.curves, l);
            for (b, w) in locals {
                let s = color_shift(&b.curves, l);
                shift[0] += w * s[0];
                shift[1] += w * s[1];
            }
            shift
        });
    }
    // The look table, last before the output transform: the picture
    // is display-referred by here, which is the only place a table
    // made for a display can be read (notes §78). `Look::at` is the
    // whole stage — into the table's primaries, clipped into them,
    // encoded, looked up tetrahedrally, decoded, back, blended by the
    // strength — and the shader does the same arithmetic.
    if let Some(look) = look {
        c = look.at(c);
    }
    let s = [
        to_out[0][0] * c[0] + to_out[0][1] * c[1] + to_out[0][2] * c[2],
        to_out[1][0] * c[0] + to_out[1][1] * c[1] + to_out[1][2] * c[2],
        to_out[2][0] * c[0] + to_out[2][1] * c[1] + to_out[2][2] * c[2],
    ];
    let e = s.map(|v| encode(v.clamp(0.0, 1.0)));
    match grain {
        Some(noise) => Grain::apply(noise, e),
        None => e,
    }
}

/// What a pixel is where the droppers read it, under the global look
/// alone. The tone shifts read the pixel's own luminance here, not the
/// guide plane: a dropper answers for one pixel and is handed one, so
/// at a strong highlights or whites setting its reading of a small
/// bright detail differs from what the picture shows for it.
///
/// Its Oklab hue after the exposure, where the mixer sees it;
/// its encoded values after the tone curve, where the point curves
/// do; and its Oklab lightness after them, where the color curves
/// do. The same stages as `finish_pixel_with`, without the locals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picked {
    /// Degrees, as `Mixer::at` takes it.
    pub hue: f32,
    pub encoded: [f32; 3],
    /// The encoded luminance, the master curve's x for it.
    pub luma: f32,
    pub lightness: f32,
}

#[allow(clippy::too_many_arguments)]
pub fn pick(
    px: [f32; 3],
    light: &Light,
    mixer: &Mixer,
    color: &Color,
    bw: &BlackWhite,
    tint: &Tint,
    curves: &CurveLut,
    source: Source,
) -> Picked {
    // Each section as it acts, as `finish_pixel_with` takes it: a
    // switch off is nothing to do, whatever its sliders still say.
    let (mixer, color, bw) = (mixer.effective(), color.effective(), bw.effective());
    // And the tint through the vector the finish blends looks with,
    // so the dropper reads the tint the render applies.
    let tint = Tint::from_vector(tint.vector());
    let gain = 2f32.powf(source.baseline() + light.exposure);
    let mut c = px.map(|v| v * gain);
    let hue = oklab(c)[2].atan2(oklab(c)[1]).to_degrees();
    if mixer.enabled || color.enabled || bw.enabled || !tint.is_identity() {
        c = mix_with(c, &mixer, &color, &bw, &tint, None);
    }
    if light.tone.enabled {
        c = shape(c, &light.tone, None).map(|x| source.curve(x));
    } else {
        c = c.map(|v| v.min(1.0));
    }
    let encoded = c.map(|v| encode(v.clamp(0.0, 1.0)));
    let luma = encode((LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]).clamp(0.0, 1.0));
    let shaded = [0, 1, 2].map(|k| decode(lookup(curves, k, encoded[k])));
    Picked {
        hue,
        encoded,
        luma,
        lightness: oklab(shaded)[0],
    }
}

/// The band an Oklab hue falls mostly in, with its weight there.
pub fn nearest_band(hue_degrees: f32) -> (usize, f32) {
    let [a, b] = Mixer::weights(hue_degrees);
    if a.1 >= b.1 { a } else { b }
}

/// A working-space pixel in Oklab: lightness, a, b.
pub fn oklab(c: [f32; 3]) -> [f32; 3] {
    OKLAB.with(|ok| {
        let lms = apply3(&ok.to_lms, c).map(|v| v.abs().cbrt().copysign(v));
        apply3(&LMS_TO_LAB, lms)
    })
}

/// Oklab's matrices for the working space live in the engine, so the
/// mixer here, the shader they are handed to and the develop's
/// defringe all read one hue.
pub use greycard_core::color::{LAB_TO_LMS, LMS_TO_LAB, Oklab, SRGB_TO_LMS, apply3};

thread_local! {
    static OKLAB: Oklab = Oklab::for_working_space();
    static SRGB_FROM_LMS: [[f32; 3]; 3] =
        greycard_core::color::invert3(SRGB_TO_LMS).expect("Ottosson's matrix inverts");
}

/// An Oklab color as encoded sRGB, clipped: for the panel's pictures
/// of colors, not the pipeline.
pub fn oklab_to_srgb(lab: [f32; 3]) -> [f32; 3] {
    let lms = apply3(&LAB_TO_LMS, lab).map(|v| v * v * v);
    SRGB_FROM_LMS.with(|m| apply3(m, lms).map(|v| encode(v.clamp(0.0, 1.0))))
}

/// The mixer and the global color on one scene-linear pixel: to
/// Oklab, the band's shift of hue, scale of chroma and scale of
/// lightness, then vibrance and saturation's further scale of chroma,
/// back. Negative channels keep their sign through the cube root, as
/// the shader does it. The mixer reads the pixel's own hue and
/// chroma; `mix_with` takes a local mean instead.
#[cfg(test)]
#[inline]
pub fn mix(c: [f32; 3], mixer: &Mixer, color: &Color) -> [f32; 3] {
    mix_with(c, mixer, color, &BlackWhite::OFF, &Tint::OFF, None)
}

/// `mix`, with the mixer's hue and its confidence read from
/// `reference`, the mean Oklab a and b about the pixel, when given,
/// and the bands' values faded by `confidence` of that chroma
/// (`mixer.rs`): near grey the hue is noise, and a band's gain on it
/// is speckle. The shift, scale and gain still act on the pixel's own
/// a and b. Vibrance and saturation scale the pixel's chroma as before.
///
/// `bw`, on, throws the chroma away whatever the mixer and the
/// sliders did with it, and the band's weight is one more gain on the
/// lightness, read from the same hue and confidence the mixer used.
/// Nothing after this pass *scales* chroma, so the picture stays
/// neutral until something adds color back.
///
/// `tint` is the last word, after the conversion and by addition, as
/// the grading wheels are: a mono picture takes a local tint and is
/// hand-colored there, which is what a Color mask is for, and no
/// slider that scales chroma can undo the conversion. On a color
/// picture it turns the hue toward the tint's and lifts the chroma to
/// a floor that follows the lightness (`tint.rs`).
#[inline]
pub fn mix_with(
    c: [f32; 3],
    mixer: &Mixer,
    color: &Color,
    bw: &BlackWhite,
    tint: &Tint,
    reference: Option<[f32; 2]>,
) -> [f32; 3] {
    OKLAB.with(|ok| {
        let lms = apply3(&ok.to_lms, c).map(|v| v.abs().cbrt().copysign(v));
        let lab = apply3(&LMS_TO_LAB, lms);
        let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
        let hue = lab[2].atan2(lab[1]).to_degrees();
        let [ra, rb] = reference.unwrap_or([lab[1], lab[2]]);
        let seen = rb.atan2(ra).to_degrees();
        let trust = confidence((ra * ra + rb * rb).sqrt());
        let (shift, chroma_scale, light_scale) = mixer.at_with(seen, trust);
        let hue = hue + shift;
        let mut chroma = chroma * chroma_scale * light_scale;
        if color.enabled {
            chroma *= color.scale(chroma, hue);
        }
        let h = hue.to_radians();
        let mut lab = if bw.enabled {
            [lab[0] * light_scale * bw.light(seen, trust), 0.0, 0.0]
        } else {
            [lab[0] * light_scale, chroma * h.cos(), chroma * h.sin()]
        };
        if !tint.is_identity() {
            let [a, b] = tint.applied(lab[0], lab[1], lab[2]);
            lab[1] = a;
            lab[2] = b;
        }
        let lms = apply3(&LAB_TO_LMS, lab).map(|v| v * v * v);
        apply3(&ok.from_lms, lms)
    })
}

/// The mean Oklab a and b of the source about every pixel, a box of
/// `MEAN_RADIUS` each way with the edges clamped, for `mix_with`:
/// what the mixer reads a hue from, so a near-grey pixel's noise
/// does not choose its band. Of the source before any exposure; the
/// finish scales it by the cube root of the gain.
pub fn local_ab(image: &WorkingImage) -> Vec<[f32; 2]> {
    let (w, h) = (image.width, image.height);
    let ab: Vec<[f32; 2]> = image
        .data
        .par_chunks(3)
        .map(|px| {
            let lab = oklab([px[0], px[1], px[2]]);
            [lab[1], lab[2]]
        })
        .collect();
    let r = MEAN_RADIUS as isize;
    let n = (2 * MEAN_RADIUS + 1) as f32;
    // Along the rows, then down the columns, edges clamped both ways.
    let mut rows = vec![[0.0f32; 2]; w * h];
    rows.par_chunks_mut(w)
        .zip(ab.par_chunks(w))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                let mut acc = [0.0f32; 2];
                for d in -r..=r {
                    let sx = (x as isize + d).clamp(0, w as isize - 1) as usize;
                    acc[0] += row[sx][0];
                    acc[1] += row[sx][1];
                }
                *o = [acc[0] / n, acc[1] / n];
            }
        });
    let mut out = vec![[0.0f32; 2]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, line)| {
        for (x, o) in line.iter_mut().enumerate() {
            let mut acc = [0.0f32; 2];
            for d in -r..=r {
                let sy = (y as isize + d).clamp(0, h as isize - 1) as usize;
                acc[0] += rows[sy * w + x][0];
                acc[1] += rows[sy * w + x][1];
            }
            *o = [acc[0] / n, acc[1] / n];
        }
    });
    out
}

/// The color curves on one pixel: to Oklab, a and b shifted by what
/// the curves say at its lightness, back.
#[cfg(test)]
pub fn shade(c: [f32; 3], curves: &CurveLut) -> [f32; 3] {
    shade_by(c, |l| color_shift(curves, l))
}

/// To Oklab, a and b shifted by `shift` of the lightness, back.
#[inline]
pub fn shade_by(c: [f32; 3], shift: impl Fn(f32) -> [f32; 2]) -> [f32; 3] {
    OKLAB.with(|ok| {
        let lms = apply3(&ok.to_lms, c).map(|v| v.abs().cbrt().copysign(v));
        let mut lab = apply3(&LMS_TO_LAB, lms);
        let [da, db] = shift(lab[0]);
        lab[1] += da;
        lab[2] += db;
        let lms = apply3(&LAB_TO_LMS, lab).map(|v| v * v * v);
        apply3(&ok.from_lms, lms)
    })
}

/// Where the two exposure shifts act, in stops over mid grey as the
/// guide reads them: shadows below it, highlights over it.
///
/// Scene white on these cameras is 2.47 stops over mid grey
/// ([`SCENE_WHITE_STOPS`]). Highlights runs from a stop under mid grey
/// to scene white, where it is full: §85's ramp began at mid grey and
/// gave a region a stop over it a third of the slider, which read as
/// a slider that stopped short of the mid-tones (§98). Now that region
/// takes six tenths and mid grey itself a fifth, so at the slider's
/// limit mid grey moves four tenths of a stop. Shadows is full three
/// and a half stops under mid grey and nothing at it; §85 had three,
/// and the extra half stop is what the wider range costs.
///
/// The sliders are ±2 stops. A smoothstep of `a` stops over a width
/// `w` has a peak slope of `1.5 a / w`: 0.86 for each alone, and
/// where the two overlap, between a stop under mid grey and mid grey,
/// their slopes add to 0.86 at worst with both at their limit. So no
/// setting of either turns the curve back: the guard §19 wrote holds,
/// and a test sweeps it. Both ramps a half stop narrower, or the
/// range at ±2 over §85's ramps, and it does not.
const SHADOWS_RAMP: (f32, f32) = (-3.5, 0.0);
const HIGHLIGHTS_RAMP: (f32, f32) = (-1.0, 2.5);

/// How much of the highlights slider a region takes, by its stops over
/// mid grey: rising over [`HIGHLIGHTS_RAMP`] to [`HIGHLIGHTS_PEAK`], and
/// easing back by [`HIGHLIGHTS_EASE_DEPTH`] of that over
/// [`HIGHLIGHTS_EASE`], so the brightest tones, whose place is display
/// white, take less than the bright mid-tones do. Measured against
/// Lightroom's -100 on the reference set (§146): the slider's -2 is
/// about that, -1.1 stops at the peak and -0.66 at scene white, where
/// §85's ramp gave the top two full stops, twice Lightroom's. In
/// fixed stops of light, not the frame's own range, for now (§146).
/// A peak slope of 0.47 at the slider's limit, rising, and 0.52
/// easing, so the sweep of every corner still holds.
const HIGHLIGHTS_PEAK: f32 = 0.55;
const HIGHLIGHTS_EASE: (f32, f32) = (2.0, DISPLAY_WHITE_STOPS);
const HIGHLIGHTS_EASE_DEPTH: f32 = 0.4;

fn highlights_weight(g: f32) -> f32 {
    HIGHLIGHTS_PEAK
        * smoothstep(HIGHLIGHTS_RAMP.0, HIGHLIGHTS_RAMP.1, g)
        * (1.0 - HIGHLIGHTS_EASE_DEPTH * smoothstep(HIGHLIGHTS_EASE.0, HIGHLIGHTS_EASE.1, g))
}

/// Scene white, in stops over mid grey: where a channel at the
/// sensor's clip lands after the white balance and the matrix, on the
/// cameras measured (§19). Display white, where the shoulder reaches
/// one and the white point aims, is it with the baseline added.
const SCENE_WHITE_STOPS: f32 = 2.47;

/// How far the whites that reaches [`white_point`] may go either way.
///
/// The slider is ±2 (§98; ±1 under §85), and the value that arrives
/// is not the slider's: a local adjustment's whites is added to the
/// picture's in `finish_pixel_with`, so the global look at plus one
/// under two masks at plus one hands this a three. The exponent
/// `DISPLAY_WHITE_STOPS / (DISPLAY_WHITE_STOPS - whites)` has a pole at
/// display white itself — a white point asked to sit where the white
/// already is, which is a division by zero and, past it, a
/// negative exponent that turns the top of the scale over. Unheld, a
/// blended whites near the pole takes a pixel a few stops over grey
/// to an infinity in the shoulder, which quantizes to black.
///
/// So: the slider's own top, two stops, which is 1.27 short of the
/// pole, where the exponent is 2.6 — a luminance 1.27 stops over mid
/// grey is brought to white; and, the other way, short of where the
/// eased exponent's slope first reaches zero, which the monotonic
/// test's arithmetic puts at 0.407, an exponent a whites of -4.76
/// gives. Held at -3.5, where it is 0.48.
const WHITES_RANGE: (f32, f32) = (-3.5, 2.0);

/// The whites control: a white point pivoted at mid grey, before the
/// shoulder. A luminance `whites` stops under display white, where the
/// raw clips and the shoulder reaches white (§145), is brought to
/// display white, and everything over mid grey with it, by a power
/// about mid grey on the luminance: nothing below mid grey, and above
/// it the stops over grey scaled by `3.27 / (3.27 - whites)` (§149).
/// So whites at minus one puts the clip a stop further out and
/// compresses the top three and a quarter stops evenly towards grey, which is highlight
/// compression with no halo because it is global; whites at plus one
/// clips a stop earlier and stretches the top. A zeroed edit is not
/// touched.
///
/// Not a stretch in linear light: pivoted at mid grey that leaves a
/// knee whose slope ratio is 0.45 at minus one, which shows on a
/// gradient crossing grey. The power's knee is 0.71 and is eased away:
/// the exponent runs from one at mid grey to its value a stop above,
/// by a smoothstep, so the curve is smooth through grey and exact from
/// a stop over it, which is everywhere the white point can be at the
/// slider's range. Monotonic while the exponent is over 0.41.
///
/// A gain on the luminance rather than a power per channel, as the
/// shifts are, so a color keeps its hue and its channel ratios.
///
/// The whites it is given is held to [`WHITES_RANGE`] first, because
/// the value that reaches it is not the slider's.
#[inline]
fn white_point(c: [f32; 3], whites: f32) -> [f32; 3] {
    if whites == 0.0 {
        return c;
    }
    let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    if y <= MID_GREY {
        return c;
    }
    let w = whites.clamp(WHITES_RANGE.0, WHITES_RANGE.1);
    let u = (y / MID_GREY).log2();
    let p = DISPLAY_WHITE_STOPS / (DISPLAY_WHITE_STOPS - w);
    let p = 1.0 + (p - 1.0) * smoothstep(0.0, 1.0, u);
    let gain = 2f32.powf(u * (p - 1.0));
    c.map(|v| v * gain)
}

/// The edit's shape of the scene before the curve: contrast about mid
/// grey per channel, then the exposure shifts by region, then the
/// white point, then the black point. Never negative on the way out,
/// since the curve is only meant for a positive scene.
///
/// `guide` is the tone equalizer's read of the region this pixel sits
/// in, in stops over mid grey and already brought to this pixel's
/// exposure and contrast; the pixel's own luminance stands in where
/// there is none (`finish_pixel_with` says where that is). Reading a
/// smoothed luminance is the whole difference: within a region the
/// shift is one constant, so a gain is all it is and the texture keeps
/// every bit of its contrast, where a shift by the pixel's own
/// luminance flattens what it touches.
#[inline]
fn shape(c: [f32; 3], t: &greycard_edit::Tone, guide: Option<f32>) -> [f32; 3] {
    let c = c.map(|v| MID_GREY * (v.max(0.0) / MID_GREY).powf(t.contrast));
    let g = guide.unwrap_or_else(|| {
        let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
        (y.max(1e-6) / MID_GREY).log2()
    });
    let shift = t.shadows * (1.0 - smoothstep(SHADOWS_RAMP.0, SHADOWS_RAMP.1, g))
        + t.highlights * highlights_weight(g);
    let gain = 2f32.powf(shift);
    black_point(white_point(c.map(|v| v * gain), t.whites), t.blacks)
}

/// The top of the blacks slider, where [`BLACKS_LIFT`] is reached.
const BLACKS_TOP: f32 = 0.3;
/// Stops a lift at the top of the slider gives the deep shadows, and
/// where it fades, in stops over mid grey: full at and under the
/// ramp's foot, nothing from its head. Measured against Lightroom's
/// +100 on a low-key frame (§144), which lifts the deep shadows about
/// 1.1 stops, the lower mid-tones about 0.6 and nothing a stop and a
/// half over grey. A peak slope of `1.5 * 1.2 / 6.5`, 0.28, so it
/// never turns the curve back.
const BLACKS_LIFT: f32 = 1.2;
const BLACKS_RAMP: (f32, f32) = (-5.0, 1.5);

/// The black point. Crushing, `blacks` under zero, puts `-blacks` of
/// mid grey at zero and holds scene white: an offset and a scale.
/// Lifting is not the same offset the other way, which adds a fixed
/// amount of light to every pixel and over a dark picture is a grey
/// veil (§143). It is a toe instead, a gain on the luminance that is
/// full in the deep shadows and fades out through the mid-tones, so
/// the shadows open, the blacks keep their shape and a color keeps
/// its hue.
#[inline]
fn black_point(c: [f32; 3], blacks: f32) -> [f32; 3] {
    if blacks <= 0.0 {
        let black = -blacks * MID_GREY;
        return c.map(|v| ((v - black) / (1.0 - black)).max(0.0));
    }
    let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    let u = (y.max(1e-9) / MID_GREY).log2();
    let fade = 1.0 - smoothstep(BLACKS_RAMP.0, BLACKS_RAMP.1, u);
    let gain = 2f32.powf(BLACKS_LIFT * blacks / BLACKS_TOP * fade);
    c.map(|v| v.max(0.0) * gain)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where the display curve reaches white, in stops over mid grey: the
/// sensor's clip, scene white, where the baseline puts it. A channel
/// the raw clipped shows as white, as a camera's JPEG and Lightroom
/// show it, and not as the pale grey Narkowicz's fit gives it (§145).
const DISPLAY_WHITE_STOPS: f32 = SCENE_WHITE_STOPS + BASELINE_EXPOSURE;
/// `MID_GREY * 2^DISPLAY_WHITE_STOPS`, and the gain that takes the fit
/// there to one. Constants because `powf` is not `const`; a test holds
/// them to what they name.
const DISPLAY_WHITE: f32 = 1.736_363;
const SHOULDER_GAIN: f32 = 1.114_332;

/// Narkowicz's fit of the ACES output transform.
fn aces(x: f32) -> f32 {
    (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)
}

/// The display curve, per channel: Narkowicz's fit of the ACES output
/// transform up to mid grey, untouched, and over it a gain eased in by
/// a smoothstep in stops until it is [`SHOULDER_GAIN`] at
/// [`DISPLAY_WHITE`], where the fit reaches exactly one; clipped past
/// it. The fit alone reaches one only 5.3 stops over grey, two stops
/// past anything the sensor records, so the top of every picture sat
/// at 0.9 of white. Both factors rise, so the curve does; the slope it
/// meets white with is the fit's there, a tenth of a display stop per
/// scene stop, so the clip is a soft corner.
fn tone(x: f32) -> f32 {
    if x <= MID_GREY {
        return aces(x).max(0.0);
    }
    if x >= DISPLAY_WHITE {
        return 1.0;
    }
    let u = (x / MID_GREY).log2();
    let gain = 1.0 + (SHOULDER_GAIN - 1.0) * smoothstep(0.0, DISPLAY_WHITE_STOPS, u);
    (aces(x) * gain).min(1.0)
}

pub fn encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// The whole image through `to_out`, each encoded value through
/// `quantize`, interleaved. `position` gives a pixel's place in the
/// masks' units (the developed picture's width), through whatever
/// geometry the image has been through; `frame` the vignette's
/// exposure in stops and the grain at a pixel of the image.
///
/// `guide` is the tone equalizer's plane, made from the developed
/// picture before its geometry (`guide_plane`), so it is read through
/// `position` too and a crop, a turn or a resize all find the same
/// region they would in the viewport; `source` is that picture's width
/// in pixels, which is what `position`'s units are.
///
/// `sampled` is the picture the range masks read ([`sample`]) when it
/// is not `image`: an export's, from before its output sharpen, the
/// same size. The mixer reads `image`.
#[allow(clippy::too_many_arguments)]
pub fn finish_with<T: Copy + Default + Send>(
    image: &WorkingImage,
    sampled: Option<&WorkingImage>,
    global: &Baked,
    locals: &[Local],
    position: impl Fn(usize, usize) -> (f32, f32) + Sync,
    frame: impl Fn(usize, usize) -> (f32, Option<[f32; 3]>) + Sync,
    guide: Option<(&Guide, f32)>,
    look: Option<&lut::Look>,
    to_out: &[[f32; 3]; 3],
    quantize: impl Fn(f32) -> T + Sync,
) -> Vec<T> {
    // The mean the mixer and the black and white read a hue from,
    // only when one of them acts.
    let reference = (global.mixer.enabled
        || global.bw.enabled
        || locals.iter().any(|l| l.enabled && l.baked.mixer.enabled))
    .then(|| local_ab(image));
    // The range masks' sample, at the global exposure alone, and the
    // same mean of their picture only when a color window reads it:
    // the mixer's when that is the same picture.
    let reads = locals.iter().any(Local::reads_picture);
    let source = sampled.filter(|s| s.width == image.width && s.height == image.height);
    let mean = locals
        .iter()
        .any(Local::reads_color)
        .then(|| match (&reference, source) {
            (Some(_), None) => None,
            _ => Some(local_ab(source.unwrap_or(image))),
        });
    let exposure = global.light.exposure;
    let mut out = vec![T::default(); image.width * image.height * 3];
    out.par_chunks_mut(image.width * 3)
        .zip(image.data.par_chunks(image.width * 3))
        .enumerate()
        .for_each(|(y, (row_out, row_in))| {
            let mut on: Vec<(&Baked, f32)> = Vec::with_capacity(locals.len());
            for (x, (o, px)) in row_out
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(row_in.as_chunks::<3>().0)
                .enumerate()
            {
                on.clear();
                let mut g = None;
                let i = y * image.width + x;
                let ab = reference.as_ref().map(|r| r[i]);
                if !locals.is_empty() || guide.is_some() {
                    let (u, v) = position(x, y);
                    let s = reads.then(|| {
                        let px = source.map_or(*px, |s| s.pixel(x, y));
                        let mean_ab = match &mean {
                            Some(Some(m)) => Some(m[i]),
                            Some(None) => ab,
                            None => None,
                        };
                        sample(px, mean_ab, exposure)
                    });
                    for local in locals.iter().filter(|l| l.enabled) {
                        let w = local.weight_sampled(u, v, s);
                        if w > 0.0 {
                            on.push((&local.baked, w));
                        }
                    }
                    g = guide.map(|(plane, sw)| plane.at(u * sw, v * sw));
                }
                let (stops, grain) = frame(x, y);
                *o = finish_pixel_with(*px, global, &on, stops, grain, ab, g, look, to_out)
                    .map(&quantize);
            }
        });
    out
}

/// The parts of `edit` the viewport cannot show over `shown`'s
/// develop: everything the engine does, less the white balance,
/// which the shader previews.
pub fn not_previewed(edit: &Edit, shown: &Edit) -> Vec<&'static str> {
    let mut out = Vec::new();
    if edit.noise != shown.noise {
        out.push("denoise");
    }
    if edit.demosaic != shown.demosaic {
        out.push("demosaic");
    }
    if edit.camera != shown.camera {
        out.push("camera profile");
    }
    if edit.lens != shown.lens {
        out.push("lens correction");
    }
    if edit.retouch != shown.retouch {
        out.push("retouch");
    }
    if edit.detail != shown.detail {
        out.push("local contrast");
    }
    if edit.sharpen != shown.sharpen {
        out.push("sharpen");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::{Curves, Tone};

    /// What the viewport can and cannot show over the develop it
    /// already has: the white balance the shader previews, so it is
    /// not named; everything the engine does is.
    #[test]
    fn a_peek_names_what_the_viewport_cannot_show() {
        let shown = Edit::default();
        let mut edit = shown.clone();
        assert!(not_previewed(&edit, &shown).is_empty());
        // The shader's own: exposure, the curve, the mixer, the
        // white balance. None of them is named.
        edit.light.exposure = 1.0;
        edit.curves.rgb = vec![[0.0, 0.1], [1.0, 1.0]];
        edit.white_balance = greycard_edit::WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.01,
        };
        assert!(not_previewed(&edit, &shown).is_empty());
        // The engine's: all seven, each named once and in the order
        // they are asked about, so the status line reads the same
        // way every time. One mutation apiece, and each one on its
        // own names only itself.
        type Move = (&'static str, fn(&mut Edit));
        let moves: [Move; 7] = [
            ("denoise", |e| e.noise.enabled = !e.noise.enabled),
            ("demosaic", |e| e.demosaic = greycard_edit::Demosaic::Vng4),
            ("camera profile", |e| {
                e.camera.profile = greycard_edit::camera::ProfileChoice::Named("other".into())
            }),
            ("lens correction", |e| e.lens.enabled = !e.lens.enabled),
            ("retouch", |e| {
                e.retouch
                    .patches
                    .push(greycard_edit::retouch::Patch::default())
            }),
            ("local contrast", |e| e.detail.enabled = !e.detail.enabled),
            ("sharpen", |e| e.sharpen.enabled = !e.sharpen.enabled),
        ];
        for (name, apply) in moves {
            let mut alone = shown.clone();
            apply(&mut alone);
            assert_eq!(not_previewed(&alone, &shown), [name], "{name} alone");
        }
        // And all seven together come back in that order.
        let mut all = shown.clone();
        for (_, apply) in moves {
            apply(&mut all);
        }
        assert_eq!(not_previewed(&all, &shown), moves.map(|(name, _)| name));
    }

    /// The straight line, baked.
    fn identity() -> CurveLut {
        Curves::default().bake()
    }

    /// A global look alone, as the tests had it before locals.
    fn fp(
        px: [f32; 3],
        light: &Light,
        mixer: &Mixer,
        curves: &CurveLut,
        m: &[[f32; 3]; 3],
    ) -> [f32; 3] {
        let global = Baked {
            light: *light,
            mixer: *mixer,
            color: Color::default(),
            bw: BlackWhite::OFF,
            tint: Tint::OFF,
            curves: *curves,
            source: Source::Scene,
        };
        finish_pixel(px, &global, &[], m)
    }

    /// The picture's own look, everything at its default: what the
    /// look table's tests put a table behind.
    fn plain_look() -> Baked {
        Baked {
            light: Light {
                enabled: true,
                exposure: 0.0,
                tone: Tone::default(),
            },
            mixer: Mixer::default(),
            color: Color::default(),
            bw: BlackWhite::OFF,
            tint: Tint::OFF,
            curves: identity(),
            source: Source::Scene,
        }
    }

    /// A table of one value everywhere, so what the strength does is
    /// plain to read.
    fn flat_table(to: [f32; 3]) -> Arc<lut::Lut3d> {
        let mut table = lut::Lut3d::identity(2);
        table.data.iter_mut().for_each(|px| *px = to);
        Arc::new(table)
    }

    /// The look table runs after the tone curve and before the output
    /// matrix, and a strength of zero leaves the picture alone —
    /// exactly, not nearly, since the stage is skipped.
    #[test]
    fn the_look_table_goes_on_after_the_tone_curve() {
        let m = crate::export::Space::Srgb.matrix();
        let global = plain_look();
        let px = [0.3, 0.18, 0.09];
        let bare = finish_pixel_look(px, &global, None, &m);

        // Nothing of the table at zero.
        let off = lut::Look::new(flat_table([0.0; 3]), 0.0).unwrap();
        assert_eq!(finish_pixel_look(px, &global, Some(&off), &m), bare);

        // An identity table at full strength is the picture again,
        // within what the round trip through the table's primaries
        // and its encoding costs.
        let id = lut::Look::new(Arc::new(lut::Lut3d::identity(33)), 1.0).unwrap();
        let through = finish_pixel_look(px, &global, Some(&id), &m);
        for k in 0..3 {
            assert!((through[k] - bare[k]).abs() < 0.004, "{through:?} {bare:?}");
        }

        // A table that takes everything to black takes the picture
        // with it, and half of it takes it half way — in linear
        // light, which is where the blend happens, so the encoded
        // value lands well above half.
        let black = flat_table([0.0; 3]);
        let full = lut::Look::new(black.clone(), 1.0).unwrap();
        assert_eq!(finish_pixel_look(px, &global, Some(&full), &m), [0.0; 3]);
        let half = lut::Look::new(black, 0.5).unwrap();
        let mid = finish_pixel_look(px, &global, Some(&half), &m);
        for k in 0..3 {
            assert!(mid[k] > 0.0 && mid[k] < bare[k], "{mid:?} {bare:?}");
            // Half the light is about 0.73 of the encoded value.
            let want = encode(decode(bare[k]) * 0.5);
            assert!((mid[k] - want).abs() < 0.01, "{mid:?} wanted {want}");
        }
    }

    /// A look is one stage of the finish, so the whole-image path
    /// runs it too, and the same table gives the same pixel either
    /// way.
    #[test]
    fn the_whole_image_goes_through_the_look_the_same_way() {
        let m = crate::export::Space::Srgb.matrix();
        let global = plain_look();
        let mut image = WorkingImage::new(2, 2);
        let pixels = [
            [0.5, 0.2, 0.1],
            [0.18; 3],
            [0.02, 0.4, 0.9],
            [1.2, 1.0, 0.8],
        ];
        for (out, px) in image.data.as_chunks_mut::<3>().0.iter_mut().zip(pixels) {
            *out = px;
        }
        // A table with a cross term: not a curve, so the whole path
        // has to be doing the lookup rather than getting away with a
        // straight line.
        let mut table = lut::Lut3d::identity(9);
        let last = 8.0;
        for b in 0..9 {
            for g in 0..9 {
                for r in 0..9 {
                    let (rf, gf, bf) = (r as f32 / last, g as f32 / last, b as f32 / last);
                    table.data[(b * 9 + g) * 9 + r] =
                        [rf * rf, gf * (1.0 - 0.3 * bf), (bf + gf * 0.2).min(1.0)];
                }
            }
        }
        let look = lut::Look::new(Arc::new(table), 0.7).unwrap();
        let out: Vec<u8> = finish_with(
            &image,
            None,
            &global,
            &[],
            |_, _| (0.0, 0.0),
            |_, _| (0.0, None),
            None,
            Some(&look),
            &m,
            |v| (v * 255.0).round() as u8,
        );
        for (i, px) in pixels.iter().enumerate() {
            let want =
                finish_pixel_look(*px, &global, Some(&look), &m).map(|v| (v * 255.0).round() as u8);
            assert_eq!(&out[i * 3..i * 3 + 3], &want, "pixel {i}");
        }
        // And it is not the picture without one.
        let bare: Vec<u8> = finish_with(
            &image,
            None,
            &global,
            &[],
            |_, _| (0.0, 0.0),
            |_, _| (0.0, None),
            None,
            None,
            &m,
            |v| (v * 255.0).round() as u8,
        );
        assert_ne!(out, bare);
    }

    #[test]
    fn mid_grey_and_white_land_where_the_curve_puts_them() {
        // The curve itself, so the baseline is taken back out: a scene
        // value here is where it meets the curve.
        let light = Light {
            enabled: true,
            exposure: -BASELINE_EXPOSURE,
            tone: Tone::default(),
        };
        let m = crate::export::Space::Srgb.matrix();
        let no_mix = Mixer::default();
        let id = identity();
        // Neutral in, neutral out: the matrix keeps grey grey.
        let g = fp([MID_GREY; 3], &light, &no_mix, &id, &m);
        assert!(
            (g[0] - g[1]).abs() < 1e-3 && (g[1] - g[2]).abs() < 1e-3,
            "{g:?}"
        );
        // The curve lifts mid grey about half a stop: 0.18 linear is
        // 0.46 encoded; the curve gives about 0.56.
        assert!(g[1] > 0.53 && g[1] < 0.60, "{g:?}");
        // Past display white the curve is white; a stop under it, not.
        let w = fp([4.0; 3], &light, &no_mix, &id, &m);
        assert!((w[1] - 1.0).abs() < 1e-6, "{w:?}");
        let under = fp([DISPLAY_WHITE / 2.0; 3], &light, &no_mix, &id, &m);
        assert!(under[1] > 0.9 && under[1] < 0.99, "{under:?}");
        // Contrast leaves mid grey where it was and moves a stop above it.
        let hard = Light {
            enabled: true,
            exposure: -BASELINE_EXPOSURE,
            tone: Tone {
                contrast: 1.5,
                ..Tone::default()
            },
        };
        let g2 = fp([MID_GREY; 3], &hard, &no_mix, &id, &m);
        assert!((g2[1] - g[1]).abs() < 1e-4);
        let up = fp([MID_GREY * 2.0; 3], &light, &no_mix, &id, &m);
        let up2 = fp([MID_GREY * 2.0; 3], &hard, &no_mix, &id, &m);
        assert!(up2[1] > up[1] + 0.02, "{up:?} {up2:?}");
        // A stop of exposure is a stop.
        let plus = Light {
            exposure: 1.0 - BASELINE_EXPOSURE,
            ..light
        };
        let a = fp([MID_GREY; 3], &plus, &no_mix, &id, &m);
        assert!((a[1] - up[1]).abs() < 1e-5);
        // Curve off: a clip at one, plain encoding below it.
        let flat = Light {
            enabled: true,
            exposure: -BASELINE_EXPOSURE,
            tone: Tone {
                enabled: false,
                ..Tone::default()
            },
        };
        let c = fp([MID_GREY; 3], &flat, &no_mix, &id, &m);
        assert!((c[1] - encode(MID_GREY)).abs() < 2e-3, "{c:?}");
        assert!((fp([5.0; 3], &flat, &no_mix, &id, &m)[1] - 1.0).abs() < 1e-5);
    }

    /// The slider at zero is the baseline: a picture with nothing done
    /// to it is the one the slider at +0.8 gave before, and the slider
    /// still moves it by the stops it says (§141).
    #[test]
    fn the_default_develop_is_the_baseline_brighter() {
        let m = crate::export::Space::Srgb.matrix();
        let (no_mix, id) = (Mixer::default(), identity());
        let at = |exposure: f32, px: f32| {
            let light = Light {
                enabled: true,
                exposure,
                tone: Tone::default(),
            };
            fp([px; 3], &light, &no_mix, &id, &m)[1]
        };
        let lifted = MID_GREY * 2f32.powf(BASELINE_EXPOSURE);
        assert!((at(0.0, MID_GREY) - at(-BASELINE_EXPOSURE, lifted)).abs() < 1e-5);
        assert!(at(0.0, MID_GREY) > at(-BASELINE_EXPOSURE, MID_GREY) + 0.05);
        assert!((at(1.0, MID_GREY) - at(0.0, MID_GREY * 2.0)).abs() < 1e-5);
    }

    /// A picture that is not a raw has had its baseline and its curve
    /// from whatever rendered it: with nothing done to it, it comes out
    /// as it went in; a stop of exposure is still a stop; the Light
    /// sliders still act; and the droppers read it the same way.
    #[test]
    fn a_rendered_picture_takes_no_baseline_and_no_curve() {
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let picture = |tone: Tone, exposure: f32| Baked {
            light: Light {
                enabled: true,
                exposure,
                tone,
            },
            source: Source::Display,
            ..Baked::global(&Edit::default(), Source::Display)
        };
        let plain = picture(Tone::default(), 0.0);
        for px in [
            [0.0; 3],
            [0.02, 0.05, 0.01],
            [MID_GREY; 3],
            [0.6, 0.3, 0.9],
            [1.0; 3],
        ] {
            let out = finish_pixel(px, &plain, &[], &m);
            for k in 0..3 {
                assert!((out[k] - encode(px[k])).abs() < 1e-5, "{px:?} {out:?}");
            }
            let picked = pick(
                px,
                &plain.light,
                &plain.mixer,
                &plain.color,
                &plain.bw,
                &plain.tint,
                &plain.curves,
                Source::Display,
            );
            for k in 0..3 {
                assert!(
                    (picked.encoded[k] - out[k]).abs() < 1e-5,
                    "{picked:?} {out:?}"
                );
            }
        }
        let up = finish_pixel([0.1; 3], &picture(Tone::default(), 1.0), &[], &m);
        assert!((up[1] - encode(0.2)).abs() < 1e-5, "{up:?}");
        let hard = Tone {
            contrast: 1.5,
            ..Tone::default()
        };
        let a = finish_pixel(grey_at(1.0), &picture(hard, 0.0), &[], &m);
        assert!(a[1] > encode(MID_GREY * 2.0) + 0.02, "{a:?}");
        // The same scene as a raw is brighter and through the curve.
        let raw = Baked {
            source: Source::Scene,
            ..plain.clone()
        };
        assert!(finish_pixel([MID_GREY; 3], &raw, &[], &m)[1] > encode(MID_GREY) + 0.05);
    }

    fn grey_at(stops: f32) -> [f32; 3] {
        [MID_GREY * 2f32.powf(stops); 3]
    }

    #[test]
    fn the_shifts_act_where_they_say() {
        let base = Tone::default();
        // What each gives where it is strongest, where it is nothing,
        // and the share of it mid grey takes: highlights peaks in the
        // bright mid-tones and eases back towards white (§146), and
        // reaches a stop under grey so the mid-tones are in its reach
        // (§98); shadows is full three and a half stops under grey and
        // nothing at it.
        for (field, at, peak, elsewhere, at_grey) in [
            ("highlights", 2.3, 0.514, -1.0, 0.109),
            ("shadows", -3.5, 1.0, 0.0, 0.0),
        ] {
            let mut t = base;
            match field {
                "highlights" => t.highlights = 1.0,
                _ => t.shadows = 1.0,
            }
            // Its strongest, in stops, at a slider of one.
            let full = shape(grey_at(at), &t, None)[1];
            assert!(
                ((full / shape(grey_at(at), &base, None)[1]).log2() - peak).abs() < 0.01,
                "{field}: {full}"
            );
            // Nothing beyond its reach.
            assert_eq!(
                shape(grey_at(elsewhere), &t, None),
                shape(grey_at(elsewhere), &base, None),
                "{field}"
            );
            let grey = shape(grey_at(0.0), &t, None)[1] / shape(grey_at(0.0), &base, None)[1];
            assert!(
                (grey.log2() - at_grey).abs() < 2e-3,
                "{field} at grey: {grey}"
            );
        }
        // A color keeps its channel ratios through a shift.
        let t = Tone {
            highlights: -1.0,
            ..base
        };
        let c = shape([0.6, 0.4, 0.2], &t, None);
        assert!(
            (c[0] / c[1] - 1.5).abs() < 1e-4 && (c[1] / c[2] - 2.0).abs() < 1e-4,
            "{c:?}"
        );
        // The black point: crushing puts that value at zero and holds
        // scene white.
        let crush = Tone {
            blacks: -0.2,
            ..base
        };
        assert_eq!(shape([MID_GREY * 0.2; 3], &crush, None)[1], 0.0);
        assert!((shape([1.0; 3], &crush, None)[1] - 1.0).abs() < 1e-6);
    }

    /// The shoulder reaches white where the raw clips, with the baseline
    /// in: nothing moves up to mid grey, the curve rises all the way,
    /// and the constants are what they name (§145).
    #[test]
    fn the_shoulder_reaches_white_at_the_sensors_clip() {
        let white = MID_GREY * 2f32.powf(DISPLAY_WHITE_STOPS);
        assert!((DISPLAY_WHITE - white).abs() < 1e-4, "{white}");
        assert!((aces(DISPLAY_WHITE) * SHOULDER_GAIN - 1.0).abs() < 1e-5);
        assert!((tone(DISPLAY_WHITE * 0.9999) - 1.0).abs() < 1e-3);
        for x in [0.0, 0.001, 0.02, 0.1, MID_GREY] {
            assert_eq!(tone(x), aces(x), "{x}");
        }
        // The raw's clip under the default develop: what the fit alone
        // showed as 0.90 of white is white.
        let clip = MID_GREY * 2f32.powf(SCENE_WHITE_STOPS) * 2f32.powf(BASELINE_EXPOSURE);
        assert!(aces(clip) < 0.9 && (tone(clip) - 1.0).abs() < 1e-3);
        let mut last = 0.0;
        for i in 0..=2000 {
            let x = MID_GREY * 2f32.powf(-10.0 + i as f32 * 0.01);
            let y = tone(x);
            assert!(y >= last, "turns back at {x}");
            last = y;
        }
    }

    /// Lifting the blacks is a toe, not a veil (§143, §144): black
    /// stays black, the deep shadows take the full lift in stops, the
    /// lift fades through the mid-tones and is gone a stop and a half
    /// over grey, and a color keeps its channel ratios.
    #[test]
    fn a_blacks_lift_opens_the_shadows_without_a_veil() {
        let lift = Tone {
            blacks: BLACKS_TOP,
            ..Tone::default()
        };
        let stops = |at: f32| {
            let v = MID_GREY * 2f32.powf(at);
            (shape([v; 3], &lift, None)[1] / v).log2()
        };
        assert_eq!(shape([0.0; 3], &lift, None), [0.0; 3]);
        for deep in [-12.0, -8.0, -5.0] {
            assert!(
                (stops(deep) - BLACKS_LIFT).abs() < 1e-4,
                "{deep}: {}",
                stops(deep)
            );
        }
        assert!(
            stops(-3.0) > 0.8 && stops(-3.0) < BLACKS_LIFT,
            "{}",
            stops(-3.0)
        );
        assert!(stops(-1.7) > 0.4 && stops(-1.7) < 0.8, "{}", stops(-1.7));
        assert!(stops(BLACKS_RAMP.1).abs() < 1e-5);
        assert!(stops(2.47).abs() < 1e-5, "scene white is held");
        let half = Tone {
            blacks: BLACKS_TOP / 2.0,
            ..Tone::default()
        };
        let v = MID_GREY / 64.0;
        let at_half = (shape([v; 3], &half, None)[1] / v).log2();
        assert!((at_half - BLACKS_LIFT / 2.0).abs() < 1e-4, "{at_half}");
        let c = shape([0.012, 0.006, 0.003], &lift, None);
        assert!(
            (c[0] / c[1] - 2.0).abs() < 1e-4 && (c[1] / c[2] - 2.0).abs() < 1e-4,
            "{c:?}"
        );
    }

    #[test]
    fn every_corner_of_the_tone_controls_is_monotonic() {
        // §19's guard: no combination of the sliders at their limits
        // turns the curve back on itself, so a brighter scene value is
        // never a darker pixel. Each shift has a peak slope of 0.86
        // at its limit and where the two overlap, the stop under mid
        // grey, they add to 0.86 at worst; the white point is a power
        // over 0.41 at its limit; the black point is linear and the
        // shoulder rises. So it holds on all sixteen corners, read
        // from the pixel's own luminance, which is the fallback path
        // and the one that can fold at all: read from the guide, a
        // shift is one constant across a region.
        let m = crate::export::Space::Srgb.matrix();
        let no_mix = Mixer::default();
        let id = identity();
        for corner in 0..16u32 {
            let sign = |bit: u32| if corner & (1 << bit) != 0 { 1.0 } else { -1.0 };
            let tone = Tone {
                highlights: 2.0 * sign(0),
                shadows: 2.0 * sign(1),
                whites: 2.0 * sign(2),
                blacks: 0.3 * sign(3),
                ..Tone::default()
            };
            let light = Light {
                enabled: true,
                exposure: 0.0,
                tone,
            };
            let mut last = -1.0;
            for i in 0..=1400 {
                let stops = -8.0 + i as f32 * 0.01;
                let v = fp(grey_at(stops), &light, &no_mix, &id, &m)[1];
                assert!(v + 1e-6 >= last, "{tone:?} turns back at {stops} stops");
                last = v;
            }
        }
    }

    #[test]
    fn whites_puts_the_stop_it_names_at_display_white_and_leaves_grey() {
        // Whites at plus one: a luminance a stop under display white
        // lands at display white, where the raw clips (§149). At minus
        // one: display white lands where a stop over it would land, at
        // 3.27 stops of a 4.27-stop scale.
        // Mid grey and below do not move, in either direction.
        let white = MID_GREY * 2f32.powf(DISPLAY_WHITE_STOPS);
        for whites in [1.0f32, -1.0] {
            let t = Tone {
                whites,
                ..Tone::default()
            };
            let from = shape(grey_at(DISPLAY_WHITE_STOPS - whites), &t, None)[1];
            assert!((from / white - 1.0).abs() < 1e-4, "{whites}: {from}");
            for stops in [0.0f32, -0.5, -2.0, -6.0] {
                assert_eq!(
                    shape(grey_at(stops), &t, None),
                    shape(grey_at(stops), &Tone::default(), None)
                );
            }
        }
        // Smooth through mid grey: the slope just over it, against the
        // zeroed curve's at the same place, is the slope just under it
        // against its, to a tenth of a percent, at the slider's limit.
        // (Against the zeroed curve because a stop is a ratio, so the
        // slope per stop grows across the gap on its own.)
        let t = Tone {
            whites: -1.0,
            ..Tone::default()
        };
        let base = Tone::default();
        let at = |t: &Tone, stops: f32| shape(grey_at(stops), t, None)[1];
        let slope = |t: &Tone, a: f32, b: f32| (at(t, b) - at(t, a)) / (b - a);
        let below = slope(&t, -0.02, -0.005) / slope(&base, -0.02, -0.005);
        let above = slope(&t, 0.005, 0.02) / slope(&base, 0.005, 0.02);
        assert!(
            (above / below - 1.0).abs() < 1e-3,
            "{below} under, {above} over"
        );
        // And a stop over grey the exponent is in full: the value is
        // the power's, 0.766 stops for one.
        let want = MID_GREY * 2f32.powf(DISPLAY_WHITE_STOPS / (DISPLAY_WHITE_STOPS + 1.0));
        assert!((at(&t, 1.0) / want - 1.0).abs() < 1e-3, "{}", at(&t, 1.0));
        // Hue held: a color's channel ratios survive.
        let c = shape([0.9, 0.5, 0.2], &t, None);
        assert!(
            (c[0] / c[1] - 1.8).abs() < 1e-4 && (c[1] / c[2] - 2.5).abs() < 1e-4,
            "{c:?}"
        );
    }

    #[test]
    fn a_blended_whites_past_the_pole_is_still_a_curve() {
        // The slider is plus or minus two, and a local adjustment's
        // whites adds to the picture's (`finish_pixel_with`), so the
        // global look at plus one with two masks over it at plus one
        // hands `white_point` a three — past the pole its exponent has
        // at scene white. Unheld that was an infinity in the shoulder
        // and a black pixel; held to `WHITES_RANGE` it is a very steep
        // curve and nothing more.
        let m = crate::export::Space::Srgb.matrix();
        let whites = |w: f32| {
            Baked::of(&Look {
                light: Light {
                    tone: Tone {
                        whites: w,
                        ..Tone::default()
                    },
                    ..Light::default()
                },
                ..Look::default()
            })
        };
        let one = whites(1.0);
        let plain = Baked::of(&Look::default());
        // Three ways to a blended three, and each of them finite and
        // never turning back over fourteen stops of scene.
        for locals in [
            vec![(&one, 1.0), (&one, 1.0)],
            vec![(&one, 1.0), (&one, 0.5), (&one, 0.5)],
            vec![(&whites(3.0), 1.0)],
        ] {
            let global = if locals.len() == 1 { &plain } else { &one };
            let mut last = -1.0f32;
            for i in 0..=1400 {
                let stops = -8.0 + i as f32 * 0.01;
                let v = finish_pixel(grey_at(stops), global, &locals, &m)[1];
                assert!(v.is_finite(), "{stops} stops gave {v}");
                assert!((0.0..=1.0).contains(&v), "{stops} stops gave {v}");
                assert!(v + 1e-6 >= last, "turns back at {stops} stops");
                last = v;
            }
            // And it is the clamp's own answer, not something else:
            // the held value is what `WHITES_RANGE` says.
            let held = whites(WHITES_RANGE.1);
            let at_three = finish_pixel(grey_at(1.5), global, &locals, &m);
            let at_held = finish_pixel(grey_at(1.5), &plain, &[(&held, 1.0)], &m);
            for k in 0..3 {
                assert!((at_three[k] - at_held[k]).abs() < 1e-5, "{at_three:?}");
            }
        }
        // The far side has no pole but it does have a slope that
        // reaches zero; the floor holds it short of that, so a blended
        // minus eight is monotonic too.
        let down = whites(-4.0);
        let mut last = -1.0f32;
        for i in 0..=1400 {
            let stops = -8.0 + i as f32 * 0.01;
            let v = finish_pixel(grey_at(stops), &down, &[(&down, 1.0)], &m)[1];
            assert!(v.is_finite() && (0.0..=1.0).contains(&v), "{stops}: {v}");
            assert!(v + 1e-6 >= last, "turns back at {stops} stops");
            last = v;
        }
        // The eased exponent's slope: `1 + (p - 1) * (9u^2 - 8u^3)`
        // peaks at u = 0.75, where the bracket is 1.6875, so the curve
        // holds while p is over 1 - 1/1.6875. That is the number
        // `WHITES_RANGE`'s floor is chosen against.
        let p = DISPLAY_WHITE_STOPS / (DISPLAY_WHITE_STOPS - WHITES_RANGE.0);
        assert!(p > 1.0 - 1.0 / 1.6875, "the floor's exponent is {p}");
        assert!(WHITES_RANGE.1 < DISPLAY_WHITE_STOPS, "the pole is not held");
    }

    /// A picture of one luminance everywhere, `stops` over mid grey.
    fn flat(stops: f32, w: usize, h: usize) -> WorkingImage {
        let v = MID_GREY * 2f32.powf(stops);
        WorkingImage::from_data(w, h, vec![v; w * h * 3]).expect("a flat field")
    }

    /// The finish of one pixel of `image` at (x, y) under `tone`,
    /// reading `guide` where it has one.
    fn at(image: &WorkingImage, guide: Option<&Guide>, tone: Tone, x: usize, y: usize) -> f32 {
        let global = Baked {
            light: Light {
                enabled: true,
                exposure: 0.0,
                tone,
            },
            mixer: Mixer::default(),
            color: Color::default(),
            bw: BlackWhite::OFF,
            tint: Tint::OFF,
            curves: identity(),
            source: Source::Scene,
        };
        let g = guide.map(|g| g.at(x as f32 + 0.5, y as f32 + 0.5));
        let px = image.pixel(x, y);
        finish_pixel_with(
            px,
            &global,
            &[],
            0.0,
            None,
            None,
            g,
            None,
            &crate::export::Space::Srgb.matrix(),
        )[1]
    }

    #[test]
    fn the_guide_of_a_flat_field_is_its_own_luminance() {
        // Nothing to smooth and nothing to keep: every texel is the
        // field's place in the scene, in stops.
        for stops in [-4.0f32, -1.0, 0.0, 2.0] {
            let g = guide_plane(&flat(stops, 300, 200));
            assert!(
                g.data.iter().all(|v| (v - stops).abs() < 1e-4),
                "{stops}: {:?}",
                &g.data[..4]
            );
            // And the bilinear read of it is the same everywhere,
            // corners and edges included.
            for (x, y) in [(0.5, 0.5), (149.5, 99.5), (299.5, 199.5)] {
                assert!((g.at(x, y) - stops).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn a_flat_field_takes_the_gain_the_slider_says_and_nothing_else() {
        // The whole point of a gain by region: over a field with no
        // region boundary the tone equalizer is the plain shift, the
        // same one §19 gave: the slider times its weight there, in stops.
        for (stops, tone, want) in [
            (
                2.5f32,
                Tone {
                    highlights: -1.0,
                    ..Tone::default()
                },
                -0.4746f32,
            ),
            (
                -2.0,
                Tone {
                    shadows: 1.0,
                    ..Tone::default()
                },
                0.6064,
            ),
            (
                -1.0,
                Tone {
                    highlights: 2.0,
                    whites: 1.0,
                    ..Tone::default()
                },
                0.0,
            ),
        ] {
            let image = flat(stops - BASELINE_EXPOSURE, 64, 48);
            let guide = guide_plane(&image);
            let shifted = at(&image, Some(&guide), tone, 32, 24);
            // In stops, read back through the finish: a flat field of
            // `stops + want` under no slider is what it should equal.
            let expect = at(
                &flat(stops + want - BASELINE_EXPOSURE, 64, 48),
                None,
                Tone::default(),
                32,
                24,
            );
            assert!(
                (shifted - expect).abs() < 2e-3,
                "{stops} stops, {tone:?}: {shifted}, wanted {expect}"
            );
        }
    }

    #[test]
    fn whites_at_minus_one_pulls_the_clip_by_three_quarters_of_a_stop_through_the_finish() {
        // Display white is 3.27 stops over mid grey, scene white with the
        // baseline (§145). Under whites at minus one it lands where 2.50
        // stops lands with no slider: the top 3.27 stops scaled to 4.27. §19's placement brought it down
        // 0.07 of a stop, which is the roadmap's "whites do basically
        // nothing". Through the whole finish, with the guide plane
        // present, since whites reads the pixel and not the plane.
        // Each field is placed where it meets the curve, the baseline
        // taken back out.
        let image = flat(DISPLAY_WHITE_STOPS - BASELINE_EXPOSURE, 128, 96);
        let guide = guide_plane(&image);
        let tone = Tone {
            whites: -1.0,
            ..Tone::default()
        };
        let pulled = at(&image, Some(&guide), tone, 64, 48);
        let old = at(
            &flat(DISPLAY_WHITE_STOPS - 0.066 - BASELINE_EXPOSURE, 128, 96),
            None,
            Tone::default(),
            64,
            48,
        );
        let new = at(
            &flat(
                DISPLAY_WHITE_STOPS * DISPLAY_WHITE_STOPS / (DISPLAY_WHITE_STOPS + 1.0)
                    - BASELINE_EXPOSURE,
                128,
                96,
            ),
            None,
            Tone::default(),
            64,
            48,
        );
        assert!((pulled - new).abs() < 3e-3, "{pulled} wanted {new}");
        // Near white the shoulder is flat, so three quarters of a stop
        // there is a few hundredths of display value.
        assert!(pulled < old - 0.03, "{pulled} against {old} under \u{a7}19");
    }

    #[test]
    fn a_detail_keeps_its_ratio_where_the_pixels_own_luminance_flattens_it() {
        // The whole point. A detail one and a half stops over its
        // surround, small against the guide's window: it reads one
        // region, both
        // sides get one gain, and the ratio across the edge survives.
        // Reading the pixel's own luminance instead, the bright side is
        // pulled and the dark side is not, so the edge flattens — which
        // is what "highlights recovery" used to do here and what the
        // roadmap complained of.
        // Big enough that the guide runs at its real scale (a texel to
        // two source pixels here), and the detail six pixels across,
        // which is what a detail is next to a window fifty pixels wide.
        let (w, h) = (1200usize, 800usize);
        let dark = MID_GREY * 2f32.powf(1.0);
        let bright = MID_GREY * 2f32.powf(2.5);
        let mut data = vec![dark; w * h * 3];
        for y in 400..406 {
            for x in 600..606 {
                for k in 0..3 {
                    data[(y * w + x) * 3 + k] = bright;
                }
            }
        }
        let image = WorkingImage::from_data(w, h, data).expect("the detail");
        let guide = guide_plane(&image);
        // Highlights alone: whites is a white point on the pixel's own
        // luminance by design, global, and would compress the detail
        // against its surround as any global curve does.
        let tone = Tone {
            highlights: -2.0,
            ..Tone::default()
        };
        // Linear, before the shoulder and the encoding, so a ratio is
        // a ratio: the finish's shape with and without the plane.
        let shaped = |g: Option<f32>, v: f32| shape([v; 3], &tone, g)[1];
        let gin = guide.at(602.5, 402.5);
        let gout = guide.at(602.5, 500.5);
        let with = shaped(Some(gin), bright) / shaped(Some(gout), dark);
        let without = shaped(None, bright) / shaped(None, dark);
        let plain = bright / dark;
        assert!(
            (with / plain - 1.0).abs() < 0.05,
            "the detail's ratio {with} against {plain}"
        );
        // The surround, a stop over grey, takes 0.33 of the slider and
        // the detail 0.47 (§146), so the flattening is the difference:
        // 0.82 of the ratio at minus two.
        assert!(
            without < plain * 0.85,
            "the pixel's own luminance should flatten it: {without} against {plain}"
        );
        // And the guide really did read one region: the detail moved it
        // by a twentieth of a stop where the picture is one and a half
        // stops brighter.
        assert!((gin - gout).abs() < 0.05, "{gin} against {gout}");
    }

    #[test]
    fn a_region_boundary_is_kept_and_each_side_is_pulled_on_its_own() {
        // The other half of it: a boundary the size of the picture is
        // not detail, so the guide follows it and the bright side comes
        // down while the dark side stays. Halos are the price, and are
        // measured on a real frame in the notes.
        let (w, h) = (600usize, 400usize);
        let dark = MID_GREY * 2f32.powf(-2.0);
        let bright = MID_GREY * 2f32.powf(2.5);
        let mut data = vec![dark; w * h * 3];
        for y in 0..h {
            for x in w / 2..w {
                for k in 0..3 {
                    data[(y * w + x) * 3 + k] = bright;
                }
            }
        }
        let image = WorkingImage::from_data(w, h, data).expect("the boundary");
        let guide = guide_plane(&image);
        // Far from the edge each side reads its own level, within a
        // tenth of a stop.
        let (lit_g, shade_g) = (guide.at(560.5, 200.5), guide.at(40.5, 200.5));
        assert!((lit_g - 2.5).abs() < 0.1, "{lit_g}");
        assert!((shade_g + 2.0).abs() < 0.1, "{shade_g}");
        let tone = Tone {
            highlights: -1.0,
            ..Tone::default()
        };
        let lit = shape([bright; 3], &tone, Some(lit_g))[1] / bright;
        let shade = shape([dark; 3], &tone, Some(shade_g))[1] / dark;
        // The lit side takes the slider times the weight 2.5 stops over
        // grey gives it, 0.47 (§146).
        assert!(
            (lit.log2() + 0.4746).abs() < 0.05,
            "the lit side {}",
            lit.log2()
        );
        assert!(
            shade.log2().abs() < 0.01,
            "the shaded side {}",
            shade.log2()
        );
    }

    #[test]
    fn the_guide_plane_is_one_answer_wherever_it_is_made() {
        // The worker makes it with the base and the export reads that
        // same plane; a second run of it on the same picture has to be
        // the same bits, or the viewport and the file would differ by
        // whatever the thread pool did that time.
        let (w, h) = (500usize, 300usize);
        let data: Vec<f32> = (0..w * h * 3)
            .map(|k| {
                let (x, y) = ((k / 3) % w, (k / 3) / w);
                let v = ((x as f32 / 37.0).sin() + (y as f32 / 23.0).cos() + 2.2) * 0.4;
                v * (1.0 + (k % 3) as f32 * 0.1)
            })
            .collect();
        let image = WorkingImage::from_data(w, h, data).expect("a picture");
        let a = guide_plane(&image);
        let b = guide_plane(&image);
        assert_eq!(a, b);
        // Its texels sit where `Guide::at` says: the center of texel
        // (i, j) is source `scale * (i + 0.5)`, and the bilinear read
        // there is the texel itself.
        let s = a.scale as f32;
        for (i, j) in [(0usize, 0usize), (3, 5), (a.width - 1, a.height - 1)] {
            let read = a.at(s * (i as f32 + 0.5), s * (j as f32 + 0.5));
            assert!(
                (read - a.data[j * a.width + i]).abs() < 1e-6,
                "texel {i},{j}: {read}"
            );
        }
    }

    #[test]
    fn the_curves_act_on_the_encoded_picture_before_the_matrix() {
        let m = crate::export::Space::Srgb.matrix();
        let no_mix = Mixer::default();
        let light = Light::default();
        let curves = Curves {
            rgb: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            ..Default::default()
        };
        let lut = curves.bake();
        let plain = fp([MID_GREY; 3], &light, &no_mix, &identity(), &m);
        let lifted = fp([MID_GREY; 3], &light, &no_mix, &lut, &m);
        // Grey stays grey and rises; the matrix after is the identity
        // for a neutral, so the encoded value is the curve's own.
        assert!(lifted[1] > plain[1] + 0.1, "{plain:?} {lifted:?}");
        assert!((lifted[0] - lifted[1]).abs() < 1e-3 && (lifted[1] - lifted[2]).abs() < 1e-3);
        assert!((lifted[1] - greycard_edit::curve::evaluate(&curves.rgb, plain[1])).abs() < 3e-3);
        // A per-channel curve moves that channel only, seen in the
        // working space's own primaries (through the sRGB matrix a
        // Rec.2020 red pulled this far leaves the sRGB gamut).
        let red = Curves {
            red: vec![[0.0, 0.0], [0.5, 0.3], [1.0, 1.0]],
            ..Default::default()
        }
        .bake();
        let m2020 = crate::export::Space::Rec2020.matrix();
        let plain = fp([MID_GREY; 3], &light, &no_mix, &identity(), &m2020);
        let r = fp([MID_GREY; 3], &light, &no_mix, &red, &m2020);
        assert!(
            r[0] < plain[0] - 0.05 && (r[1] - plain[1]).abs() < 2e-3,
            "{plain:?} {r:?}"
        );
    }

    #[test]
    fn a_color_curve_casts_by_lightness_and_holds_it() {
        let m = crate::export::Space::Rec2020.matrix();
        let no_mix = Mixer::default();
        let light = Light::default();
        // Red up through the middle lightnesses, neutral at the ends.
        let warm = Curves {
            red_green: vec![[0.0, 0.5], [0.5, 0.8], [1.0, 0.5]],
            ..Default::default()
        }
        .bake();
        let plain = fp([MID_GREY; 3], &light, &no_mix, &identity(), &m);
        let cast = fp([MID_GREY; 3], &light, &no_mix, &warm, &m);
        assert!(
            cast[0] > plain[0] + 0.02 && cast[1] < plain[1] - 0.01,
            "{plain:?} {cast:?}"
        );
        // The lightness is the curve's x and is held: Oklab's L of the
        // shaded linear value equals the unshaded one's.
        let l_of = |c: [f32; 3]| {
            OKLAB.with(|ok| {
                let lms = apply3(&ok.to_lms, c).map(|v| v.abs().cbrt().copysign(v));
                apply3(&LMS_TO_LAB, lms)[0]
            })
        };
        let grey = [0.3, 0.3, 0.3];
        let shaded = shade(grey, &warm);
        assert!((l_of(shaded) - l_of(grey)).abs() < 1e-5);
        assert!(shaded[0] > grey[0] && shaded[1] < grey[1], "{shaded:?}");
        // Nowhere the curve is level: black and white keep their grey.
        for v in [0.0, 1.0] {
            let same = shade([v; 3], &warm);
            for k in 0..3 {
                assert!((same[k] - v).abs() < 1e-4, "{same:?}");
            }
        }
        // Blue against yellow, down: bluer.
        let cool = Curves {
            blue_yellow: vec![[0.0, 0.5], [0.5, 0.2], [1.0, 0.5]],
            ..Default::default()
        }
        .bake();
        let b = shade(grey, &cool);
        assert!(b[2] > grey[2] && b[0] < grey[0] && b[1] < grey[1], "{b:?}");
    }

    #[test]
    fn a_local_adjustment_blends_by_its_weight() {
        let m = crate::export::Space::Srgb.matrix();
        let global = Baked::of(&Look::default());
        let px = [0.1, 0.2, 0.05];
        // A local stop of exposure at full weight is a global stop.
        let plus = Baked::of(&Look {
            light: Light {
                exposure: 1.0,
                ..Light::default()
            },
            ..Look::default()
        });
        let local_full = finish_pixel(px, &global, &[(&plus, 1.0)], &m);
        let global_plus = finish_pixel(px, &plus, &[], &m);
        for k in 0..3 {
            assert!((local_full[k] - global_plus[k]).abs() < 1e-5);
        }
        // Nothing at no weight, and between at half.
        let none = finish_pixel(px, &global, &[(&plus, 0.0)], &m);
        assert_eq!(none, finish_pixel(px, &global, &[], &m));
        let half = finish_pixel(px, &global, &[(&plus, 0.5)], &m);
        assert!(
            half[1] > none[1] && half[1] < local_full[1],
            "{none:?} {half:?} {local_full:?}"
        );
        // Two locals add their stops.
        let both = finish_pixel(px, &global, &[(&plus, 0.5), (&plus, 0.5)], &m);
        for k in 0..3 {
            assert!(
                (both[k] - local_full[k]).abs() < 1e-5,
                "{both:?} {local_full:?}"
            );
        }
        // A local curve at full weight composes under the global
        // curve: with a level global curve it is that curve.
        let curved = Baked::of(&Look {
            curves: Curves {
                rgb: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
                ..Default::default()
            },
            ..Look::default()
        });
        let via_local = finish_pixel(px, &global, &[(&curved, 1.0)], &m);
        let via_global = finish_pixel(px, &curved, &[], &m);
        for k in 0..3 {
            assert!((via_local[k] - via_global[k]).abs() < 1e-5);
        }
        // A local mixer and a local grading count too.
        let mixer = Mixer {
            saturation: [-1.0; 8],
            ..Mixer::default()
        };
        let grey = Baked::of(&Look {
            mixer,
            ..Look::default()
        });
        let g = finish_pixel(px, &global, &[(&grey, 1.0)], &m);
        assert!(
            (g[0] - g[1]).abs() < 2e-3 && (g[1] - g[2]).abs() < 2e-3,
            "{g:?}"
        );
        let warm = Baked::of(&Look {
            grading: greycard_edit::Grading {
                shadows: greycard_edit::grading::Wheel {
                    hue: 0.0,
                    saturation: 1.0,
                },
                ..Default::default()
            },
            ..Look::default()
        });
        let w = finish_pixel(px, &global, &[(&warm, 1.0)], &m);
        assert!(w[0] > finish_pixel(px, &global, &[], &m)[0] + 0.02, "{w:?}");
        // A local's color blends by weight too, halfway at 0.5.
        let saturated = Baked::of(&Look {
            color: Color {
                saturation: 1.0,
                ..Color::default()
            },
            ..Look::default()
        });
        let sat_none = finish_pixel(px, &global, &[(&saturated, 0.0)], &m);
        let sat_half = finish_pixel(px, &global, &[(&saturated, 0.5)], &m);
        let sat_full = finish_pixel(px, &global, &[(&saturated, 1.0)], &m);
        assert_eq!(sat_none, finish_pixel(px, &global, &[], &m));
        assert!(
            sat_half[1] > sat_none[1] && sat_half[1] < sat_full[1],
            "{sat_none:?} {sat_half:?} {sat_full:?}"
        );
    }

    #[test]
    fn a_pick_reads_the_stages_the_droppers_want() {
        let light = Light::default();
        let mixer = Mixer::default();
        let color = Color::default();
        let curves = identity();
        // Mid grey, no exposure: encoded as the curve sees it, and
        // the lightness of what the tone curve made of it.
        let grey = pick(
            [MID_GREY; 3],
            &light,
            &mixer,
            &color,
            &BlackWhite::OFF,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        // The baseline is in the pick as it is in the render.
        let expect = encode(tone(
            shape(
                [MID_GREY * 2f32.powf(BASELINE_EXPOSURE); 3],
                &light.tone,
                None,
            )[0],
        ));
        for k in 0..3 {
            assert!((grey.encoded[k] - expect).abs() < 1e-5, "{grey:?}");
        }
        let l = oklab([decode(expect); 3])[0];
        assert!((grey.lightness - l).abs() < 1e-4, "{grey:?} {l}");
        assert!((grey.luma - expect).abs() < 1e-5, "{grey:?}");
        // A red's hue sits in the red band, whatever the exposure
        // and the curve do after it.
        let red = pick(
            [0.5, 0.05, 0.05],
            &light,
            &mixer,
            &color,
            &BlackWhite::OFF,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        let (band, w) = nearest_band(red.hue);
        assert!(band == 0 && w > 0.5, "{red:?} {band} {w}");
        let brighter = Light {
            exposure: 1.0,
            ..Light::default()
        };
        let red2 = pick(
            [0.5, 0.05, 0.05],
            &brighter,
            &mixer,
            &color,
            &BlackWhite::OFF,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        assert!((red2.hue - red.hue).abs() < 1e-3, "{red:?} {red2:?}");
        assert!(red2.encoded[0] > red.encoded[0], "{red:?} {red2:?}");
        // With the tone curve off, the encoded value is the input.s,
        // the baseline taken back out.
        let flat = Light {
            exposure: -BASELINE_EXPOSURE,
            tone: Tone {
                enabled: false,
                ..Default::default()
            },
            ..Light::default()
        };
        let raw = pick(
            [0.1, 0.2, 0.3],
            &flat,
            &mixer,
            &color,
            &BlackWhite::OFF,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        for (k, v) in [0.1f32, 0.2, 0.3].iter().enumerate() {
            assert!((raw.encoded[k] - encode(*v)).abs() < 1e-5, "{raw:?}");
        }
    }

    #[test]
    fn a_switched_off_section_does_not_move_a_pick() {
        let light = Light::default();
        let curves = identity();
        let px = [0.5, 0.18, 0.06];
        let plain = pick(
            px,
            &light,
            &Mixer::default(),
            &Color::default(),
            &BlackWhite::OFF,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        // Every section's sliders wound right up, every switch off:
        // the dropper reads what it read with none of them set.
        let off_mixer = Mixer {
            enabled: false,
            hue: [30.0; BANDS],
            saturation: [1.0; BANDS],
            luminance: [1.0; BANDS],
        };
        let off_color = Color {
            enabled: false,
            saturation: 1.0,
            vibrance: 1.0,
        };
        let off_bw = BlackWhite::with_filter(greycard_edit::bw::Filter::Red, false);
        let held = pick(
            px,
            &light,
            &off_mixer,
            &off_color,
            &off_bw,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        let bare = pick(
            px,
            &light,
            &Mixer {
                enabled: false,
                ..Mixer::default()
            },
            &Color {
                enabled: false,
                ..Color::default()
            },
            &BlackWhite::OFF,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        assert_eq!(held, bare);
        // And that is the all-default answer to the last f32 place
        // the Oklab round trip leaves; `plain` takes the trip, since
        // an on-but-flat mixer still runs the pass.
        for k in 0..3 {
            assert!(
                (held.encoded[k] - plain.encoded[k]).abs() < 1e-5,
                "{held:?}"
            );
        }
        assert!((held.hue - plain.hue).abs() < 1e-3);
        assert!((held.lightness - plain.lightness).abs() < 1e-5);
        // And with the switches on they do move it, so the test is
        // not passing on a pick that ignores them.
        let on_mixer = Mixer {
            enabled: true,
            ..off_mixer
        };
        let moved = pick(
            px,
            &light,
            &on_mixer,
            &Color::default(),
            &BlackWhite::OFF,
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        assert!(moved.encoded[0] != plain.encoded[0], "{moved:?} {plain:?}");
        let mono = pick(
            px,
            &light,
            &Mixer::default(),
            &Color::default(),
            &BlackWhite::with_filter(greycard_edit::bw::Filter::Red, true),
            &Tint::OFF,
            &curves,
            Source::Scene,
        );
        assert!((mono.encoded[0] - mono.encoded[2]).abs() < 1e-4, "{mono:?}");
    }

    #[test]
    fn the_mixer_turns_hue_scales_chroma_and_leaves_grey_alone() {
        let identity = Mixer::default();
        let no_color = Color::default();
        let grey = [MID_GREY; 3];
        let g = mix(grey, &identity, &no_color);
        for k in 0..3 {
            assert!((g[k] - MID_GREY).abs() < 1e-4, "{g:?}");
        }
        // A Rec.2020 red, desaturated in the red band, moves toward grey
        // at the same lightness; the identity mixer returns it.
        let red = [0.5, 0.05, 0.05];
        let back = mix(red, &identity, &no_color);
        for k in 0..3 {
            assert!((back[k] - red[k]).abs() < 1e-4, "{back:?}");
        }
        let mut grey_reds = Mixer::default();
        grey_reds.saturation[0] = -1.0;
        grey_reds.saturation[7] = -1.0;
        let d = mix(red, &grey_reds, &no_color);
        assert!(
            (d[0] - d[1]).abs() < 1e-3 && (d[1] - d[2]).abs() < 1e-3,
            "{d:?}"
        );
        // The blue band leaves a red alone.
        let mut blues = Mixer::default();
        blues.saturation[5] = -1.0;
        blues.hue[5] = 30.0;
        let same = mix(red, &blues, &no_color);
        for k in 0..3 {
            assert!((same[k] - red[k]).abs() < 1e-4, "{same:?}");
        }
        // A hue turn moves a red toward orange: green rises.
        let mut warmer = Mixer::default();
        warmer.hue[0] = 30.0;
        warmer.hue[7] = 30.0;
        let w = mix(red, &warmer, &no_color);
        assert!(w[1] > red[1] * 1.5 && w[2] < red[2], "{w:?}");
        // Luminance up a stop: twice the light, channel ratios held.
        let brighter = Mixer {
            luminance: [1.0; 8],
            ..Default::default()
        };
        let b = mix(red, &brighter, &no_color);
        for k in 0..3 {
            assert!((b[k] - 2.0 * red[k]).abs() < 2e-3, "{b:?}");
        }
    }

    #[test]
    fn near_grey_noise_takes_no_gain_from_a_band() {
        // A dark rock's pixel: near grey, its hue in yellow by noise.
        let speck = [0.052, 0.050, 0.044];
        let lab = oklab(speck);
        let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
        let hue = lab[2].atan2(lab[1]).to_degrees();
        assert!(chroma > 0.005 && chroma < 0.03, "{chroma}");
        assert!(hue > 60.0 && hue < 140.0, "{hue}");
        // A stop on orange, yellow and green, so any hue between
        // their centers gets the whole of it.
        let mut yellows = Mixer::default();
        yellows.luminance[1] = 1.0;
        yellows.luminance[2] = 1.0;
        yellows.luminance[3] = 1.0;
        let no_color = Color::default();
        // Read from a grey surround: left alone.
        let held = mix_with(
            speck,
            &yellows,
            &no_color,
            &BlackWhite::OFF,
            &Tint::OFF,
            Some([0.0, 0.0]),
        );
        for k in 0..3 {
            assert!((held[k] - speck[k]).abs() < 1e-5, "{held:?}");
        }
        // Read from itself: some of the gain, the ramp's share.
        let own = mix(speck, &yellows, &no_color);
        let luma = |c: [f32; 3]| LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
        let ratio = luma(own) / luma(speck);
        assert!(ratio > 1.05 && ratio < 1.9, "{ratio}");
        // A yellow with chroma to spare, read from itself or from a
        // like surround: the whole stop.
        let yellow = [0.5, 0.45, 0.1];
        let ylab = oklab(yellow);
        let full = mix(yellow, &yellows, &no_color);
        let ratio = luma(full) / luma(yellow);
        assert!((ratio - 2.0).abs() < 5e-3, "{ratio}");
        let same = mix_with(
            yellow,
            &yellows,
            &no_color,
            &BlackWhite::OFF,
            &Tint::OFF,
            Some([ylab[1], ylab[2]]),
        );
        assert_eq!(same, full);
        // And a grey pixel in a yellow surround takes the band's gain,
        // as the pixel between two yellow leaves should.
        let grey = [MID_GREY; 3];
        let lifted = mix_with(
            grey,
            &yellows,
            &no_color,
            &BlackWhite::OFF,
            &Tint::OFF,
            Some([ylab[1], ylab[2]]),
        );
        let ratio = luma(lifted) / luma(grey);
        assert!((ratio - 2.0).abs() < 5e-3, "{ratio}");
    }

    #[test]
    fn black_and_white_leaves_a_neutral_alone_and_takes_the_color_out() {
        use greycard_edit::bw::Filter;
        let no_mixer = Mixer::default();
        let no_color = Color::default();
        let chroma = |c: [f32; 3]| {
            let lab = oklab(c);
            (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
        };
        let mono =
            |c: [f32; 3], bw: &BlackWhite| mix_with(c, &no_mixer, &no_color, bw, &Tint::OFF, None);
        // Whatever the weights, a neutral grey comes out as it went
        // in: its confidence is nothing, so no band reaches it.
        let grey = [MID_GREY; 3];
        for f in Filter::ALL {
            let bw = BlackWhite::with_filter(f, true);
            let out = mono(grey, &bw);
            for k in 0..3 {
                assert!((out[k] - grey[k]).abs() < 1e-5, "{}: {out:?}", f.name());
            }
        }
        let custom = BlackWhite {
            enabled: true,
            weights: [1.0, -1.0, 0.7, -0.3, 0.5, -0.9, 0.2, 0.4],
            strength: 1.0,
        };
        let out = mono(grey, &custom);
        for k in 0..3 {
            assert!((out[k] - grey[k]).abs() < 1e-5, "{out:?}");
        }
        // And every picture out of it is neutral: no chroma left,
        // for any hue and any weights.
        for px in [[0.5, 0.05, 0.05], [0.1, 0.4, 0.15], [0.08, 0.2, 0.6]] {
            assert!(chroma(px) > 0.05, "a test color with chroma to lose");
            for bw in [&custom, &BlackWhite::with_filter(Filter::Red, true)] {
                assert!(chroma(mono(px, bw)) < 1e-6, "{:?}", mono(px, bw));
            }
        }
        // A saturated red patch is lighter under a red filter than
        // under a blue one, as it is through the glass.
        let red = [0.5, 0.05, 0.05];
        let light = |c: [f32; 3]| oklab(c)[0];
        let under_red = light(mono(red, &BlackWhite::with_filter(Filter::Red, true)));
        let under_blue = light(mono(red, &BlackWhite::with_filter(Filter::Blue, true)));
        let plain = light(mono(red, &BlackWhite::with_filter(Filter::None, true)));
        assert!(
            under_red > plain && plain > under_blue,
            "{under_red} {plain} {under_blue}"
        );
        // A stop of it either way, as the weights say: red's weight
        // is +0.8 under the red filter, -0.7 under the blue.
        // And by exactly the stops the bands the hue lies between
        // say, which is `bw.rs`'s arithmetic, reached through the
        // whole Oklab round trip.
        let hue = {
            let l = oklab(red);
            l[2].atan2(l[1]).to_degrees()
        };
        for (f, ratio) in [
            (Filter::Red, under_red / plain),
            (Filter::Blue, under_blue / plain),
        ] {
            let want = 2f32.powf(BlackWhite::with_filter(f, true).stops(hue, 1.0) / 3.0);
            assert!((ratio - want).abs() < 2e-3, "{}: {ratio} {want}", f.name());
        }
        // A blue sky darkens under the red filter and lifts under the
        // blue one, the other way about.
        let sky = [0.08, 0.2, 0.6];
        assert!(
            light(mono(sky, &BlackWhite::with_filter(Filter::Red, true)))
                < light(mono(sky, &BlackWhite::with_filter(Filter::None, true)))
        );
        assert!(
            light(mono(sky, &BlackWhite::with_filter(Filter::Blue, true)))
                > light(mono(sky, &BlackWhite::with_filter(Filter::None, true)))
        );
        // The section off is the picture untouched.
        let off = BlackWhite {
            enabled: false,
            ..custom
        };
        assert_eq!(mono(red, &off), mono(red, &BlackWhite::OFF));
        assert!(chroma(mono(red, &off)) > 0.05);
    }

    #[test]
    fn the_strength_scales_the_conversions_gain_and_the_mixer_stands_down() {
        use greycard_edit::bw::Filter;
        let light = |c: [f32; 3]| oklab(c)[0];
        let mono = |c: [f32; 3], bw: &BlackWhite| {
            mix_with(
                c,
                &Mixer::default(),
                &Color::default(),
                bw,
                &Tint::OFF,
                None,
            )
        };
        // A sky under Red: the gain is in stops and the strength
        // multiplies them, so twice the strength squares the ratio
        // the lightness is moved by.
        let sky = [0.10, 0.16, 0.42];
        let flat = light(mono(sky, &BlackWhite::with_filter(Filter::None, true)));
        let red = BlackWhite::with_filter(Filter::Red, true);
        let once = light(mono(sky, &red)) / flat;
        let twice = light(mono(
            sky,
            &BlackWhite {
                strength: 2.0,
                ..red
            },
        )) / flat;
        assert!(once < 0.9, "the sky goes down under a red filter: {once}");
        assert!((twice - once * once).abs() < 1e-4, "{once} {twice}");
        // And nothing of it at a strength of zero.
        let none = mono(
            sky,
            &BlackWhite {
                strength: 0.0,
                ..red
            },
        );
        for k in 0..3 {
            let flat = mono(sky, &BlackWhite::with_filter(Filter::None, true));
            assert!((none[k] - flat[k]).abs() < 1e-6, "{none:?} {flat:?}");
        }
        // The picture's color mixer does not act under the
        // conversion (`Edit::acting_mixer`): its luminance would be a
        // second set of band gains over the conversion's own.
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut edit = Edit {
            bw: BlackWhite::with_filter(Filter::None, true),
            ..Edit::default()
        };
        let bare = finish_pixel(sky, &Baked::global(&edit, Source::Scene), &[], &m);
        edit.mixer.luminance = [0.8; BANDS];
        edit.mixer.saturation = [-0.5; BANDS];
        let wound_up = finish_pixel(sky, &Baked::global(&edit, Source::Scene), &[], &m);
        assert_eq!(bare, wound_up, "the mixer is stood down");
        // With the conversion off it acts as it always did, and the
        // settings were never touched.
        edit.bw.enabled = false;
        let color = finish_pixel(sky, &Baked::global(&edit, Source::Scene), &[], &m);
        edit.mixer.enabled = false;
        assert_ne!(
            color,
            finish_pixel(sky, &Baked::global(&edit, Source::Scene), &[], &m)
        );
        assert_eq!(edit.mixer.luminance[0], 0.8);
    }

    #[test]
    fn nothing_after_the_mono_pass_can_bring_the_color_back() {
        // The mixer's saturation and the global vibrance and
        // saturation act before the conversion, so a picture with
        // both wound right up still comes out neutral, and the
        // conversion reads the band from the mean, not the pixel.
        let bw = BlackWhite::with_filter(greycard_edit::bw::Filter::Orange, true);
        let mixer = Mixer {
            saturation: [1.0; BANDS],
            ..Mixer::default()
        };
        let color = Color {
            enabled: true,
            saturation: 1.0,
            vibrance: 1.0,
        };
        let px = [0.4, 0.12, 0.06];
        let out = mix_with(px, &mixer, &color, &bw, &Tint::OFF, None);
        let lab = oklab(out);
        assert!((lab[1] * lab[1] + lab[2] * lab[2]).sqrt() < 1e-6, "{lab:?}");
        // Through the whole finish, with a curve and a grading over
        // it, the picture is neutral until the color curves tone it.
        let global = Baked {
            light: Light::default(),
            mixer,
            color,
            bw,
            tint: Tint::OFF,
            curves: identity(),
            source: Source::Scene,
        };
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let out = finish_pixel(px, &global, &[], &m);
        assert!(
            (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
            "{out:?}"
        );
    }

    #[test]
    fn the_tint_pulls_a_patch_toward_a_hue_and_at_nothing_does_nothing() {
        let no_mixer = Mixer::default();
        let no_color = Color::default();
        let tinted =
            |c: [f32; 3], t: &Tint| mix_with(c, &no_mixer, &no_color, &BlackWhite::OFF, t, None);
        let chroma = |c: [f32; 3]| {
            let l = oklab(c);
            (l[1] * l[1] + l[2] * l[2]).sqrt()
        };
        let hue = |c: [f32; 3]| {
            let l = oklab(c);
            l[2].atan2(l[1]).to_degrees().rem_euclid(360.0)
        };
        // A zero amount does nothing at all, bit for bit: the tint
        // is not applied, and at the finish the Oklab pass a tint
        // alone would ask for is not entered, so the picture is the
        // picture.
        let grey = [MID_GREY; 3];
        let idle = Tint {
            hue: 210.0,
            amount: 0.0,
        };
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let plain = Baked {
            light: Light::default(),
            mixer: no_mixer,
            color: no_color,
            bw: BlackWhite::OFF,
            tint: Tint::OFF,
            curves: identity(),
            source: Source::Scene,
        };
        let bare = Baked {
            tint: idle,
            ..plain.clone()
        };
        for px in [grey, [0.5, 0.05, 0.05], [0.08, 0.2, 0.6]] {
            assert_eq!(tinted(px, &idle), tinted(px, &Tint::OFF));
            assert_eq!(
                finish_pixel(px, &bare, &[], &m),
                finish_pixel(px, &plain, &[], &m)
            );
        }
        // Mid grey at a full amount lands on the hue, with the chroma
        // `tint.rs` promises: the picture's own grey, colored.
        let full = Tint {
            hue: 210.0,
            amount: 1.0,
        };
        let out = tinted(grey, &full);
        assert!((hue(out) - 210.0).abs() < 0.5, "{:?}", oklab(out));
        let want = Tint::floor(oklab(grey)[0]);
        assert!((chroma(out) - want).abs() < 2e-3, "{} {want}", chroma(out));
        assert!(chroma(out) > 0.09, "clearly colored: {}", chroma(out));
        // The lightness is left where it was.
        assert!((oklab(out)[0] - oklab(grey)[0]).abs() < 1e-3);
        // A saturated red keeps its chroma and turns halfway toward
        // the hue at half the amount.
        let red = [0.5, 0.05, 0.05];
        let half = Tint {
            hue: 210.0,
            amount: 0.5,
        };
        let turned = tinted(red, &half);
        assert!((chroma(turned) - chroma(red)).abs() < 2e-3);
        let (was, now) = (hue(red), hue(turned));
        let step = (now - was + 180.0).rem_euclid(360.0) - 180.0;
        let whole = (210.0 - was + 180.0f32).rem_euclid(360.0) - 180.0;
        assert!((step - whole / 2.0).abs() < 0.5, "{was} {now}");
    }

    #[test]
    fn a_tint_colors_a_mono_picture_and_a_mask_blends_it_in() {
        let bw = BlackWhite::with_filter(greycard_edit::bw::Filter::Red, true);
        let chroma = |c: [f32; 3]| {
            let l = oklab(c);
            (l[1] * l[1] + l[2] * l[2]).sqrt()
        };
        let px = [0.4, 0.12, 0.06];
        let tint = Tint {
            hue: 40.0,
            amount: 1.0,
        };
        // The tint runs after the conversion, so a mono picture takes
        // it: that is what hand-coloring a black and white is. The
        // conversion still holds against everything that scales
        // chroma (`nothing_after_the_mono_pass_can_bring_the_color_back`).
        let mono = mix_with(px, &Mixer::default(), &Color::default(), &bw, &tint, None);
        assert!(chroma(mono) > 0.05, "{:?}", oklab(mono));
        let plain = mix_with(
            px,
            &Mixer::default(),
            &Color::default(),
            &bw,
            &Tint::OFF,
            None,
        );
        assert!(chroma(plain) < 1e-6, "{:?}", oklab(plain));
        // The hue is the tint's, not the picture's: the conversion
        // left no hue to argue with.
        let lab = oklab(mono);
        assert!(
            (lab[2].atan2(lab[1]).to_degrees() - 40.0).abs() < 0.5,
            "{lab:?}"
        );
        // And through the finish a mask blends the tint by its weight,
        // as a vector: half the mask is half the amount.
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let global = Baked {
            light: Light::default(),
            mixer: Mixer::default(),
            color: Color::default(),
            bw: BlackWhite::OFF,
            tint: Tint::OFF,
            curves: identity(),
            source: Source::Scene,
        };
        let local = Baked {
            tint,
            ..global.clone()
        };
        let grey = [MID_GREY; 3];
        let spread = |c: [f32; 3]| {
            c.iter().copied().fold(f32::MIN, f32::max) - c.iter().copied().fold(f32::MAX, f32::min)
        };
        let off = finish_pixel(grey, &global, &[], &m);
        let whole = finish_pixel(grey, &global, &[(&local, 1.0)], &m);
        let part = finish_pixel(grey, &global, &[(&local, 0.5)], &m);
        assert!(spread(off) < 1e-6, "{off:?}");
        assert!(
            spread(part) > 1e-3 && spread(part) < spread(whole),
            "{part:?} {whole:?}"
        );
        // Two masks at half a hue each are one at the hue between them.
        let east = Baked {
            tint: Tint {
                hue: 0.0,
                amount: 0.5,
            },
            ..global.clone()
        };
        let north = Baked {
            tint: Tint {
                hue: 90.0,
                amount: 0.5,
            },
            ..global.clone()
        };
        let both = finish_pixel(grey, &global, &[(&east, 1.0), (&north, 1.0)], &m);
        let middle = Baked {
            tint: Tint::from_vector([0.5, 0.5]),
            ..global.clone()
        };
        let one = finish_pixel(grey, &global, &[(&middle, 1.0)], &m);
        for k in 0..3 {
            assert!((both[k] - one[k]).abs() < 1e-6, "{both:?} {one:?}");
        }
    }

    #[test]
    fn local_ab_is_a_five_by_five_mean_with_clamped_edges() {
        let a = [0.5, 0.2, 0.1];
        let b = [0.1, 0.3, 0.5];
        let (la, lb) = (oklab(a), oklab(b));
        // Flat: every pixel's own a and b, the edges included.
        let flat = WorkingImage {
            width: 7,
            height: 6,
            data: a.repeat(42),
        };
        for ab in local_ab(&flat) {
            assert!((ab[0] - la[1]).abs() < 1e-6 && (ab[1] - la[2]).abs() < 1e-6);
        }
        // A checkerboard: 13 of one and 12 of the other about an
        // interior pixel.
        let mut data = Vec::new();
        for y in 0..9 {
            for x in 0..9 {
                data.extend_from_slice(if (x + y) % 2 == 0 { &a } else { &b });
            }
        }
        let board = WorkingImage {
            width: 9,
            height: 9,
            data,
        };
        let ab = local_ab(&board)[4 * 9 + 4];
        let want = [
            (13.0 * la[1] + 12.0 * lb[1]) / 25.0,
            (13.0 * la[2] + 12.0 * lb[2]) / 25.0,
        ];
        assert!((ab[0] - want[0]).abs() < 1e-6 && (ab[1] - want[1]).abs() < 1e-6);
        // A corner sees itself clamped: 3x3 distinct pixels, the
        // rest repeats of the edge, and the count still comes out.
        let corner = local_ab(&board)[0];
        // Rows -2..=2 clamp to 0,0,0,1,2; columns the same; the
        // pixel (0,0) is `a`, so the tally follows the parity.
        let mut tally = [0.0f32; 2];
        for sy in [0usize, 0, 0, 1, 2] {
            for sx in [0usize, 0, 0, 1, 2] {
                let l = if (sx + sy) % 2 == 0 { la } else { lb };
                tally[0] += l[1] / 25.0;
                tally[1] += l[2] / 25.0;
            }
        }
        assert!((corner[0] - tally[0]).abs() < 1e-6 && (corner[1] - tally[1]).abs() < 1e-6);
    }

    #[test]
    fn color_scales_chroma_and_leaves_grey_alone() {
        let no_mix = Mixer::default();
        let grey = [MID_GREY; 3];
        let up = Color {
            saturation: 1.0,
            ..Color::default()
        };
        let g = mix(grey, &no_mix, &up);
        for k in 0..3 {
            assert!((g[k] - MID_GREY).abs() < 1e-4, "{g:?}");
        }
        // Saturation +1 doubles a color's Oklab chroma, -1 sends it
        // to zero.
        let red = [0.5, 0.05, 0.05];
        let chroma = |c: [f32; 3]| {
            let lab = oklab(c);
            (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
        };
        let base = chroma(red);
        let doubled = chroma(mix(red, &no_mix, &up));
        assert!((doubled / base - 2.0).abs() < 1e-3, "{doubled} {base}");
        let down = Color {
            saturation: -1.0,
            ..Color::default()
        };
        assert!(chroma(mix(red, &no_mix, &down)) < 1e-4);
        // Vibrance lifts a pale color's chroma by a larger ratio than
        // a vivid one's.
        let vib = Color {
            vibrance: 1.0,
            ..Color::default()
        };
        let pale = [0.25, 0.18, 0.15];
        let vivid = [0.5, 0.02, 0.02];
        let pale_ratio = chroma(mix(pale, &no_mix, &vib)) / chroma(pale);
        let vivid_ratio = chroma(mix(vivid, &no_mix, &vib)) / chroma(vivid);
        assert!(
            pale_ratio > vivid_ratio,
            "{pale_ratio} should exceed {vivid_ratio}"
        );
    }

    #[test]
    fn a_switched_off_shape_weighs_nothing_and_paints_nothing() {
        use greycard_edit::brush::{Op, Stroke};
        use greycard_edit::mask::{Component, Mode, Shape};
        let disc = |center: [f32; 2], mode: Mode| Component {
            shape: Shape::Radial {
                center,
                radius: [0.2, 0.2],
                angle: 0.0,
                feather: 0.0,
            },
            mode,
            ..Default::default()
        };
        let local = |mask: Mask| Local {
            baked: Baked::of(&greycard_edit::Look::default()),
            rasters: rasterize(&mask, 0.667, |_| None),
            mask,
            enabled: true,
        };
        let both = local(Mask {
            components: vec![
                disc([0.3, 0.5], Mode::Add),
                disc([0.4, 0.5], Mode::Subtract),
            ],
            invert: false,
        });
        let mut off = both.clone();
        off.mask.components[1].enabled = false;
        let add_alone = local(Mask {
            components: vec![disc([0.3, 0.5], Mode::Add)],
            invert: false,
        });
        assert_eq!(both.weight(0.45, 0.5), 0.0);
        for k in 0..32 {
            let u = k as f32 / 31.0;
            assert_eq!(off.weight(u, 0.5), add_alone.weight(u, 0.5));
        }
        // A brush switched off is not even painted, so the export
        // does not pay for a raster nothing reads.
        let mut stroke = Stroke::new(Op::Add, 0.05, 0.5, 1.0);
        stroke.points.push([0.5, 0.5]);
        let brushed = Mask {
            components: vec![Component {
                shape: Shape::Brush {
                    strokes: vec![stroke],
                },
                enabled: false,
                ..Default::default()
            }],
            invert: false,
        };
        assert!(rasterize(&brushed, 0.667, |_| None)[0].is_none());
        // Every shape off is an empty mask, which the callers turn
        // the local off for rather than blend a mask of zeroes.
        assert!(brushed.is_empty());
    }

    /// A working-space color from Oklab lightness, hue and chroma.
    fn from_lch(l: f32, hue: f32, chroma: f32) -> [f32; 3] {
        let ok = Oklab::for_working_space();
        let from_lms = greycard_core::color::invert3(ok.to_lms).unwrap();
        let (s, c) = hue.to_radians().sin_cos();
        let lms = apply3(&LAB_TO_LMS, [l, chroma * c, chroma * s]).map(|v| v * v * v);
        apply3(&from_lms, lms)
    }

    #[test]
    fn the_sample_is_the_picture_before_the_look_at_the_global_exposure() {
        // Oklab L of mid grey is about 0.57; the sample adds the
        // stops it is given and nothing else.
        let grey = sample([0.18; 3], None, 0.0);
        assert!((grey.lightness - 0.5647).abs() < 2e-3, "{grey:?}");
        assert!(grey.a.abs() < 1e-4 && grey.b.abs() < 1e-4);
        let up = sample([0.18; 3], None, 1.0);
        assert!((up.lightness - 2f32.cbrt() * grey.lightness).abs() < 2e-3);
        // The sensor's white is 1, and past it the scale goes on: a
        // highlight is not clipped into the ones under it.
        assert!((sample([1.0; 3], None, 0.0).lightness - 1.0).abs() < 2e-3);
        assert!((sample([4.0; 3], None, 0.0).lightness - 4f32.cbrt()).abs() < 5e-3);
        // The hue is the pixel's, whatever the exposure; its chroma
        // takes the cube root of the gain, as Oklab's a and b do, and
        // a mean handed in is used in place of the pixel's own.
        let skin = from_lch(0.6, 55.0, 0.08);
        let s0 = sample(skin, None, 0.0);
        let s1 = sample(skin, None, 1.5);
        assert!((s0.hue() - 55.0).abs() < 0.1 && (s1.hue() - 55.0).abs() < 0.1);
        assert!((s0.chroma() - 0.08).abs() < 1e-3, "{}", s0.chroma());
        assert!((s1.chroma() / s0.chroma() - 2f32.powf(0.5)).abs() < 1e-3);
        let meant = sample(skin, Some([0.0, 0.1]), 0.0);
        assert!((meant.hue() - 90.0).abs() < 1e-3);
    }

    #[test]
    fn a_range_mask_acts_where_the_picture_is_and_holds_under_its_own_edit() {
        use greycard_edit::mask::{Component, Shape};
        // Flat halves five pixels wide, so the mean about the middle of
        // each is that half's own: a dark grey, a bright one, skin and
        // a blue.
        let colors = [
            [0.02; 3],
            [0.7; 3],
            from_lch(0.5, 55.0, 0.08),
            from_lch(0.4, 250.0, 0.1),
        ];
        let (w, h) = (20usize, 5usize);
        let mut image = WorkingImage::new(w, h);
        for (i, px) in image.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            *px = colors[(i % w) / 5];
        }
        let middle = |k: usize| 2 * w + k * 5 + 2;
        let m = crate::export::Space::Srgb.matrix();
        let global = plain_look();
        let local = |shape: Shape, stops: f32| {
            let mut look = greycard_edit::Look::default();
            look.light.exposure = stops;
            Local {
                baked: Baked::of(&look),
                mask: Mask {
                    components: vec![Component {
                        shape,
                        ..Default::default()
                    }],
                    invert: false,
                },
                enabled: true,
                rasters: vec![None],
            }
        };
        let render = |global: &Baked, locals: &[Local]| -> Vec<u8> {
            finish_with(
                &image,
                None,
                global,
                locals,
                |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
                |_, _| (0.0, None),
                None,
                None,
                &m,
                |v| (v * 255.0).round() as u8,
            )
        };
        let px = |out: &[u8], k: usize| out[middle(k) * 3..middle(k) * 3 + 3].to_vec();
        let bare = render(&global, &[]);
        // The bright grey, pushed a stop, and nothing else.
        let lum = local(Shape::LUMINANCE, 1.0);
        let out = render(&global, std::slice::from_ref(&lum));
        let want = |k: usize, stops: f32| {
            let mut look = greycard_edit::Look::default();
            look.light.exposure = stops;
            finish_pixel(colors[k], &global, &[(&Baked::of(&look), 1.0)], &m)
                .map(|v| (v * 255.0).round() as u8)
                .to_vec()
        };
        assert_eq!(px(&out, 1), want(1, 1.0));
        for k in [0, 2, 3] {
            assert_eq!(px(&out, k), px(&bare, k), "color {k}");
        }
        // The mask's own exposure does not move it: pushed three stops
        // more, it is on the same pixels.
        let out3 = render(&global, &[local(Shape::LUMINANCE, 3.0)]);
        assert_eq!(px(&out3, 1), want(1, 3.0));
        assert_eq!(px(&out3, 2), px(&bare, 2));
        // The global exposure does: three stops down, the bright grey
        // is under the window and the local is nowhere.
        let mut dark = global.clone();
        dark.light.exposure = -3.0;
        assert_eq!(
            render(&dark, std::slice::from_ref(&lum)),
            render(&dark, &[])
        );
        // The skin preset takes the skin and leaves the blue and the
        // greys; a window at the blue the other way round.
        let skin = render(&global, &[local(Shape::skin(), 1.0)]);
        assert_eq!(px(&skin, 2), want(2, 1.0));
        for k in [0, 1, 3] {
            assert_eq!(px(&skin, k), px(&bare, k), "color {k}");
        }
        let blue = render(&global, &[local(Shape::color_at(250.0), 1.0)]);
        assert_eq!(px(&blue, 3), want(3, 1.0));
        assert_eq!(px(&blue, 2), px(&bare, 2));
        // Handed a picture of its own to sample (an export's, before
        // its output sharpen), the masks read that one and the finish
        // the other: here the sample has the dark and the bright
        // greys swapped, so the push lands on the dark one.
        let mut swapped = image.clone();
        for (i, px) in swapped.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            *px = colors[[1, 0, 2, 3][(i % w) / 5]];
        }
        let out = finish_with(
            &image,
            Some(&swapped),
            &global,
            std::slice::from_ref(&lum),
            |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
            |_, _| (0.0, None),
            None,
            None,
            &m,
            |v| (v * 255.0).round() as u8,
        );
        assert_eq!(px(&out, 0), want(0, 1.0));
        assert_eq!(px(&out, 1), px(&bare, 1));
    }
}

/// What the shipped film presets do to a picture, measured on the
/// render rather than read off the sliders.
///
/// A preset is a set of numbers whose effect is not the sum of their
/// signs: a desaturation before the tone curve fights a contrast
/// after it, and a grading wheel puts chroma into neutrals that had
/// none. Both went wrong once — an earlier "Muted Slide" raised mean
/// saturation and took half a frame to pure black — so what is
/// asserted here is what came out the other end.
#[cfg(test)]
mod shipped_presets {
    use super::*;
    use greycard_edit::preset::{Preset, SHIPPED};

    /// Sixteen hues covering the cube's corners and edges, plus a
    /// neutral, each at full brightness in encoded sRGB. Scaled to a
    /// luminance below, they are the colors the frame is made of.
    const HUES: [[f32; 3]; 16] = [
        [1.0, 1.0, 1.0],
        [1.0, 0.0, 0.0],
        [1.0, 0.5, 0.0],
        [1.0, 1.0, 0.0],
        [0.5, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.5],
        [0.0, 1.0, 1.0],
        [0.0, 0.5, 1.0],
        [0.0, 0.0, 1.0],
        [0.5, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 0.0, 0.5],
        [0.9, 0.7, 0.6],
        [0.6, 0.7, 0.9],
        [0.8, 0.8, 0.75],
    ];

    /// A synthetic frame with a photograph's tone distribution.
    ///
    /// How far out toward [`HUES`] a pixel sits. Weighted low, as a
    /// photograph is: most of a frame is not near the gamut's edge,
    /// and a grading wheel's tint shows on the pale part of it, not
    /// on the saturated part.
    const CHROMAS: [f32; 4] = [0.0, 0.15, 0.4, 0.9];

    /// A synthetic frame with a photograph's distribution.
    ///
    /// Sixty-four levels spread evenly over nine stops, from seven
    /// below mid grey to two above — even on a log axis, which is
    /// roughly what a real histogram looks like — at each of [`HUES`]
    /// and each of [`CHROMAS`], the chroma mixed in about the pixel's
    /// own luminance so the tone ladder stays exact.
    ///
    /// Both halves of the spread earn their place. A uniform grid of
    /// the encoded cube, which this was at first, has only a ninth of
    /// its pixels in the deep shadows and let a black point that
    /// crushed half of a real frame through unnoticed; and without
    /// the pale colors, a grading wheel strong enough to raise the
    /// mean saturation of a real frame by a third did not move this
    /// one at all.
    fn a_frame() -> WorkingImage {
        let (tones, wide) = (64usize, HUES.len() * CHROMAS.len());
        let mut image = WorkingImage::new(wide, tones);
        let mut px = image.data.as_chunks_mut::<3>().0.iter_mut();
        for t in 0..tones {
            let stops = -7.0 + 9.0 * t as f32 / (tones - 1) as f32;
            let want = MID_GREY * 2f32.powf(stops);
            for hue in HUES {
                let linear = hue.map(decode);
                let y = LUMA[0] * linear[0] + LUMA[1] * linear[1] + LUMA[2] * linear[2];
                for c in CHROMAS {
                    // Toward grey about the color's own luminance, so
                    // the gain below is the same whatever the chroma.
                    let toned = linear.map(|v| (y + c * (v - y)) * want / y.max(1e-6));
                    *px.next().unwrap() = toned;
                }
            }
        }
        image
    }

    fn render(edit: &Edit) -> Vec<u8> {
        finish_with(
            &a_frame(),
            None,
            &Baked::global(edit, Source::Scene),
            &[],
            |_, _| (0.0, 0.0),
            |_, _| (0.0, None),
            None,
            None,
            &crate::export::Space::Srgb.matrix(),
            |v| (v * 255.0).round() as u8,
        )
    }

    /// Mean saturation as a viewer reads it: `(max - min) / max` per
    /// pixel, which a change of brightness alone does not move.
    fn mean_saturation(out: &[u8]) -> f32 {
        let px = out.as_chunks::<3>().0;
        let sum: f32 = px
            .iter()
            .map(|p| {
                let hi = *p.iter().max().unwrap() as f32;
                let lo = *p.iter().min().unwrap() as f32;
                if hi <= 0.0 { 0.0 } else { (hi - lo) / hi }
            })
            .sum();
        sum / px.len() as f32
    }

    /// The fraction of pixels with nothing left in any channel.
    fn black_fraction(out: &[u8]) -> f32 {
        let px = out.as_chunks::<3>().0;
        px.iter().filter(|p| p.iter().all(|&v| v == 0)).count() as f32 / px.len() as f32
    }

    #[test]
    fn the_shipped_presets_do_what_their_names_say() {
        let bare = render(&Edit::default());
        let plain_saturation = mean_saturation(&bare);
        assert_eq!(
            black_fraction(&bare),
            0.0,
            "the frame itself is not crushed"
        );

        let of = |stem: &str| {
            let (_, text) = SHIPPED.iter().find(|(s, _)| *s == stem).expect("shipped");
            let preset = Preset::from_json(text).expect("it reads");
            render(&preset.applied(&Edit::default()))
        };

        // None of them may crush the picture. A black point is a
        // per-picture decision, and a shipped preset does not make it.
        for (stem, _) in SHIPPED {
            let black = black_fraction(&of(stem));
            assert!(black < 0.10, "{stem} took {:.1}% to black", black * 100.0);
        }

        // Muted Slide is muted: less saturation than the picture came
        // with, not more. The desaturation has to outweigh a contrast
        // and two grading wheels for that to hold.
        let slide = mean_saturation(&of("muted-slide"));
        assert!(
            slide < plain_saturation * 0.95,
            "Muted Slide: {slide:.3} against {plain_saturation:.3}"
        );

        // Warm Negative is softer than the picture, and also less
        // saturated, though less so than the slide.
        let negative = mean_saturation(&of("warm-negative"));
        assert!(
            negative < plain_saturation,
            "Warm Negative: {negative:.3} against {plain_saturation:.3}"
        );

        // Red-Filter Mono is mono: what is left is the rounding of a
        // neutral to eight bits.
        let mono = mean_saturation(&of("red-filter-mono"));
        assert!(mono < 0.05, "Red-Filter Mono: {mono:.3}");
    }
}
