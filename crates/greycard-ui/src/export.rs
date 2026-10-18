//! Export: the finished picture in a chosen format, size and color
//! space, with the profile that says which, and the desktop's file
//! chooser for where it goes.

use crate::border::Border;
use crate::finish;
use crate::watermark::Mark;
use anyhow::{Context, Result, bail};
use greycard_core::RgbSpace;
use greycard_core::color::{CAT, WORKING_SPACE};
use greycard_core::decode::RawMetadata;
use greycard_core::develop::sharpen;
use greycard_core::exif::{self, Provenance};
use greycard_core::image::WorkingImage;
use greycard_edit::brush::Raster;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "linux"))]
use std::sync::{Arc, Mutex};

/// What an export does when a file of that name is already there,
/// and what it decided ([`greycard_core::output::Resolved`]). The
/// engine holds them, since the command line writes files too and the
/// policy is one thing, not two.
pub use greycard_core::output::OnExists;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 8-bit, lossy, with a quality.
    Jpeg,
    /// 8-bit, lossless.
    Png,
    /// 16-bit, lossless.
    Tiff,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Jpeg, Format::Png, Format::Tiff];

    /// The name the panel shows.
    pub fn name(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::Tiff => "TIFF 16-bit",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Tiff => "tif",
        }
    }

    /// The format a path's extension asks for.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "png" => Some(Format::Png),
            "tif" | "tiff" => Some(Format::Tiff),
            _ => None,
        }
    }

    /// The file chooser's filter for the format.
    #[cfg(test)]
    fn globs(self) -> &'static [&'static str] {
        match self {
            Format::Jpeg => &["*.jpg", "*.jpeg"],
            Format::Png => &["*.png"],
            Format::Tiff => &["*.tif", "*.tiff"],
        }
    }
}

/// Adobe RGB (1998)'s transfer function: a pure power, 563/256. The
/// monitor profile of the same name reads this too.
pub const ADOBE_RGB_GAMMA: f64 = 563.0 / 256.0;

/// The output's primaries and transfer function. sRGB, Display P3 and
/// Rec.2020 take the sRGB curve; Adobe RGB takes its own gamma. The
/// embedded profile says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Srgb,
    DisplayP3,
    Rec2020,
    AdobeRgb,
}

impl Space {
    pub const ALL: [Space; 4] = [
        Space::Srgb,
        Space::DisplayP3,
        Space::Rec2020,
        Space::AdobeRgb,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Space::Srgb => "sRGB",
            Space::DisplayP3 => "Display P3",
            Space::Rec2020 => "Rec.2020",
            Space::AdobeRgb => "Adobe RGB",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    fn rgb(self) -> RgbSpace {
        match self {
            Space::Srgb => RgbSpace::SRGB,
            Space::DisplayP3 => RgbSpace::DISPLAY_P3,
            Space::Rec2020 => RgbSpace::REC2020,
            Space::AdobeRgb => RgbSpace::ADOBE_RGB,
        }
    }

    /// The gamma of a space whose transfer function is a pure power;
    /// `None` for the spaces under the sRGB curve.
    fn gamma(self) -> Option<f64> {
        match self {
            Space::AdobeRgb => Some(ADOBE_RGB_GAMMA),
            _ => None,
        }
    }

    /// A value the finish encoded with the sRGB curve, recoded with
    /// this space's own. The finish encodes every picture with the
    /// sRGB curve (its curves and look tables work on that encoding);
    /// the spaces under it keep the value as it is, so their files are
    /// what they were.
    pub fn recode(self, v: f32) -> f32 {
        match self.gamma() {
            None => v,
            Some(g) => crate::finish::decode(v).max(0.0).powf((1.0 / g) as f32),
        }
    }

    /// The working space to this one, linear, rows.
    pub fn matrix(self) -> [[f32; 3]; 3] {
        WORKING_SPACE
            .to_space_matrix(&self.rgb(), CAT)
            .expect("the working space reaches every output space")
            .rows
            .map(|row| row.map(|v| v as f32))
    }

    /// An ICC profile for the space: its primaries and white, its
    /// transfer curve, and its name. Built once a process: lcms stamps the
    /// header with the second the profile was made, so two builds
    /// straddling a tick differ in four bytes, and an export's profile
    /// and a check of it a moment later must be the same bytes.
    pub fn icc(self) -> Result<Vec<u8>> {
        self.cached_icc(false)
    }

    /// The profile of the encoding the finish hands the display table:
    /// the space's primaries under the sRGB curve, which is `icc()`
    /// for the spaces that take it. For Adobe RGB it differs only in
    /// the curve, and the table reading the finish's values through it
    /// shows the colors the file's own values (`recode`) hold.
    pub fn finish_icc(self) -> Result<Vec<u8>> {
        self.cached_icc(true)
    }

    fn cached_icc(self, finish: bool) -> Result<Vec<u8>> {
        static BUILT: [std::sync::OnceLock<Vec<u8>>; 8] = [const { std::sync::OnceLock::new() }; 8];
        let at = Space::ALL
            .iter()
            .position(|s| *s == self)
            .expect("every space");
        let slot = &BUILT[at * 2 + usize::from(finish)];
        if let Some(bytes) = slot.get() {
            return Ok(bytes.clone());
        }
        let bytes = self.build_icc(finish)?;
        Ok(slot.get_or_init(|| bytes).clone())
    }

    fn build_icc(self, finish: bool) -> Result<Vec<u8>> {
        use lcms2::{CIExyY, CIExyYTRIPLE, Locale, MLU, Profile, Tag, TagSignature, ToneCurve};
        let s = self.rgb();
        let point = |x: f64, y: f64| CIExyY { x, y, Y: 1.0 };
        let white = point(s.white.x, s.white.y);
        let primaries = CIExyYTRIPLE {
            Red: point(s.red.x, s.red.y),
            Green: point(s.green.x, s.green.y),
            Blue: point(s.blue.x, s.blue.y),
        };
        // The sRGB transfer function, lcms's parametric type 4; or a
        // gamma, type 1.
        let curve = match self.gamma().filter(|_| !finish) {
            Some(g) => ToneCurve::new_parametric(1, &[g])
                .map_err(|e| anyhow::anyhow!("the {} curve: {e}", self.name()))?,
            None => ToneCurve::new_parametric(
                4,
                &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045],
            )
            .map_err(|e| anyhow::anyhow!("the sRGB curve: {e}"))?,
        };
        let mut profile = Profile::new_rgb(&white, &primaries, &[&curve, &curve, &curve])
            .map_err(|e| anyhow::anyhow!("building the {} profile: {e}", self.name()))?;
        let mut description = MLU::new(1);
        // Adobe's name is its mark: the profile says "compatible", as
        // darktable's and RawTherapee's do.
        let named = match self {
            Space::AdobeRgb => "Adobe RGB (compatible)",
            _ => self.name(),
        };
        description.set_text_ascii(named, Locale::none());
        profile.write_tag(TagSignature::ProfileDescriptionTag, Tag::MLU(&description));
        let mut copyright = MLU::new(1);
        copyright.set_text_ascii("No copyright, use freely", Locale::none());
        profile.write_tag(TagSignature::CopyrightTag, Tag::MLU(&copyright));
        profile
            .icc()
            .map_err(|e| anyhow::anyhow!("writing the {} profile: {e}", self.name()))
    }
}

/// Output sharpening: how much a downsized export is sharpened after
/// its resize, which softens it. The capture sharpening's
/// deconvolution (§21) again, on the small picture, under a point
/// spread that is the resize's rather than the lens's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sharpen {
    Off,
    Low,
    #[default]
    Standard,
    High,
}

impl Sharpen {
    pub const ALL: [Sharpen; 4] = [Sharpen::Off, Sharpen::Low, Sharpen::Standard, Sharpen::High];

    pub fn name(self) -> &'static str {
        match self {
            Sharpen::Off => "Off",
            Sharpen::Low => "Low",
            Sharpen::Standard => "Standard",
            Sharpen::High => "High",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The deconvolution's options for the level, or none when off.
    /// A Lanczos downsize leaves a point spread of about half a pixel,
    /// so the radius sits near the narrowest the deconvolution
    /// assumes, and the levels differ mostly in how far it iterates;
    /// the contrast threshold is measured on the small picture as the
    /// capture sharpening measures its own, so its flat parts keep
    /// their grain.
    pub fn options(self) -> Option<sharpen::SharpenOptions> {
        let (radius, iterations) = match self {
            Sharpen::Off => return None,
            Sharpen::Low => (0.45, 20),
            Sharpen::Standard => (0.55, 20),
            Sharpen::High => (0.65, 20),
        };
        Some(sharpen::SharpenOptions {
            radius: sharpen::Radius::Fixed(radius),
            iterations,
            contrast: sharpen::Threshold::Auto,
            stop_early: true,
        })
    }
}

/// What an export says about where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Metadata {
    /// The camera's EXIF, and in the XMP the source's name, what the
    /// sheet asked for and the edit that made it.
    #[default]
    All,
    /// All of that but the edit: a picture to hand on without the
    /// recipe.
    NoEdit,
    /// Nothing but the profile: no EXIF, no XMP. The camera, the time
    /// and any location stay behind.
    None,
}

impl Metadata {
    pub const ALL: [Metadata; 3] = [Metadata::All, Metadata::NoEdit, Metadata::None];

    pub fn name(self) -> &'static str {
        match self {
            Metadata::All => "All",
            Metadata::NoEdit => "No edit",
            Metadata::None => "None",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// What an export is.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub format: Format,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
    /// The long side in pixels; none is the picture's own size. Never
    /// enlarges.
    pub long_edge: Option<u32>,
    pub space: Space,
    /// Write the space's ICC profile into the file.
    pub embed_profile: bool,
    /// Sharpening after the resize; nothing without one.
    pub sharpen: Sharpen,
    /// What the file says about where it came from.
    pub metadata: Metadata,
    /// A band added round the picture after its resize and sharpening,
    /// before the mark; the long edge is the picture's, without it.
    pub border: Option<Border>,
    /// A mark laid over the picture after its resize, and over the
    /// border when there is one.
    pub watermark: Option<Mark>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            format: Format::Jpeg,
            quality: 92,
            long_edge: None,
            space: Space::Srgb,
            embed_profile: true,
            sharpen: Sharpen::Standard,
            metadata: Metadata::All,
            border: None,
            watermark: None,
        }
    }
}

/// The sizes the panel offers, by name; `CUSTOM` beside them takes
/// its pixels from a field.
pub const SIZES: [(&str, Option<u32>); 4] = [
    ("Full", None),
    ("4096", Some(4096)),
    ("2048", Some(2048)),
    ("1024", Some(1024)),
];

/// The sheet's name for a long edge typed in.
pub const CUSTOM: &str = "Custom";

/// The smallest long edge a typed number can ask for; under it the
/// number is a slip, not a picture.
pub const MIN_EDGE: u32 = 16;

/// The long edge the sheet's choice asks for: a size by name, the
/// typed pixels for `CUSTOM`, and the picture's own (none) for `Full`
/// or for text that is not a size.
pub fn long_edge(size: &str, custom: &str) -> Option<u32> {
    if size == CUSTOM {
        return parse_edge(custom);
    }
    SIZES
        .iter()
        .find(|(name, _)| *name == size)
        .and_then(|(_, edge)| *edge)
}

/// A typed long edge: whole pixels, `MIN_EDGE` or more, spaces
/// around it forgiven; anything else is no size at all.
pub fn parse_edge(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|n| *n >= MIN_EDGE)
}

pub enum Pixels {
    Eight(Vec<u8>),
    Sixteen(Vec<u16>),
}

/// A finished picture, interleaved RGB.
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
}

/// The working image, already through the edit's geometry, finished
/// for `settings`: resized in linear light if asked and sharpened
/// after it, then the viewport's transform into the output space,
/// the local adjustments placed by the developed picture's size
/// `source`, their learned masks from `learned` by adjustment id and
/// component. `clip_level` is the develop's: the value a channel is
/// clipped at or above, which the sharpen leaves alone.
///
/// `kind` says whether the image is a raw's scene or a picture
/// already rendered, which takes no baseline and no display curve.
///
/// `guide` is the tone equalizer's plane, the worker's own, made from
/// the developed picture before the geometry; it is read through
/// the same mapping the masks are, so the export and the viewport read
/// one plane at one place and the resize below cannot move it.
///
/// `white` is the develop's white and the profile it was resolved
/// through, which the masks' own white balances are made matrices
/// from (`finish::Local::of`), as the viewport makes them.
#[allow(clippy::too_many_arguments)]
pub fn render(
    image: &WorkingImage,
    edit: &greycard_edit::Edit,
    source: (u32, u32),
    settings: &Settings,
    learned: &std::collections::HashMap<(u64, usize), std::sync::Arc<Raster>>,
    clip_level: f32,
    guide: Option<&finish::Guide>,
    kind: finish::Source,
    white: Option<&greycard_edit::WhiteShift>,
    detail: Option<(&WorkingImage, &crate::detail::DetailMeta)>,
) -> Rendered {
    render_framed(
        image,
        image.width as f32,
        edit,
        source,
        settings,
        learned,
        clip_level,
        guide,
        kind,
        white,
        detail,
    )
}

/// [`render`] of an image that stands for a framed picture `framed`
/// pixels wide, which it may be smaller than: the frame's own picture
/// already reduced, as the pictures kept of an edit are made, so the
/// masks, the guide plane and the vignette are placed by the frame and
/// not by the reduced copy's pixels.
///
/// `detail` is the masks' bands ([`crate::detail::DetailPlanes`])
/// brought into the frame as `image` was, the same size as it: they
/// are resized here with the picture, by the same filter.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_framed(
    image: &WorkingImage,
    framed: f32,
    edit: &greycard_edit::Edit,
    source: (u32, u32),
    settings: &Settings,
    learned: &std::collections::HashMap<(u64, usize), std::sync::Arc<Raster>>,
    clip_level: f32,
    guide: Option<&finish::Guide>,
    kind: finish::Source,
    white: Option<&greycard_edit::WhiteShift>,
    detail: Option<(&WorkingImage, &crate::detail::DetailMeta)>,
) -> Rendered {
    // The bands with the picture through its resize, when one reads
    // them and they are the picture's size.
    let detail = detail
        .filter(|_| {
            edit.adjustments
                .iter()
                .any(|a| a.enabled && !a.mask.is_empty() && a.look.detail.enabled)
        })
        .filter(|(d, _)| {
            crate::detail::fits((d.width, d.height), image.width, image.height, "the export")
        });
    let detail_fitted = detail.and_then(|(d, _)| fit(d, settings.long_edge));
    let fitted;
    // The range masks read the picture from before the output
    // sharpen, so an export at any size samples the stage the
    // viewport does (`finish::sample`); kept only when one reads it.
    let unsharpened;
    let mut sampled = None;
    let reads = edit
        .adjustments
        .iter()
        .any(|a| a.enabled && a.mask.reads_picture());
    let image = match fit(image, settings.long_edge) {
        Some(mut f) => {
            if let Some(options) = settings.sharpen.options() {
                if reads {
                    unsharpened = f.clone();
                    sampled = Some(&unsharpened);
                }
                sharpen::sharpen(&mut f, &options, None, clip_level);
            }
            fitted = f;
            &fitted
        }
        None => image,
    };
    let global = finish::Baked::global(edit, kind);
    let (sw, sh) = (source.0 as f32, source.1 as f32);
    let locals: Vec<finish::Local> = edit
        .adjustments
        .iter()
        .take(finish::MAX_LOCALS)
        .map(|a| {
            let rasters = finish::rasterize(&a.mask, sh / sw, |i| learned.get(&(a.id, i)).cloned());
            finish::Local::of(a, rasters, white)
        })
        .collect();
    // A pixel of the image, through the resize and the frame, to the
    // source, in units of the source's width.
    let frame = edit.geometry.frame(sw, sh);
    let scale = framed / image.width as f32;
    let position = |x: usize, y: usize| {
        let r = (
            frame.origin.0 + (x as f32 + 0.5) * scale,
            frame.origin.1 + (y as f32 + 0.5) * scale,
        );
        let s = edit.geometry.to_source(r, sw, sh);
        (s.0 / sw, s.1 / sw)
    };
    // The vignette over the frame as written, whatever its size.
    let vignette = edit.vignette;
    let (iw, ih) = (image.width as f32, image.height as f32);
    let stops = |x: usize, y: usize| {
        if vignette.is_off() {
            0.0
        } else {
            vignette.amount * vignette.at((x as f32 + 0.5) / iw, (y as f32 + 0.5) / ih, iw / ih)
        }
    };
    let grain = edit.grain;
    let frame_at = |x: usize, y: usize| {
        let noise = if grain.is_off() {
            None
        } else {
            Some(grain.at((x as f32 + 0.5) / iw, (y as f32 + 0.5) / ih, iw / ih))
        };
        (stops(x, y), noise)
    };
    let m = settings.space.matrix();
    // The look table, read once for the whole image; the viewport is
    // handed the same one (`edit::look` keeps it), so the two agree.
    // A look at no strength is no look, and skipping it here spares
    // the finish a branch a pixel.
    // A table fitted under another display curve is no look here, as
    // it is none in the viewport (`LookLut::look_under`).
    let look = edit
        .look_lut
        .look_under(edit.display_curve)
        .filter(|l| !l.is_off());
    // The plane is only read where a shift asks for it; the pixel's
    // own luminance is the same answer when none does, and skipping it
    // spares the finish a lookup a pixel. A plane with nothing in it
    // is no plane, as `Renderer::set_guide` also has it, and the same
    // fallback is the right answer for it.
    let guide = guide
        .filter(|g| !g.data.is_empty())
        .filter(|_| reads_guide(edit))
        .map(|g| (g, sw));
    let detail = detail.map(|(d, meta)| (detail_fitted.as_ref().unwrap_or(d), meta));
    let pixels = match settings.format {
        Format::Tiff => Pixels::Sixteen(finish::finish_with(
            image,
            sampled,
            &global,
            &locals,
            position,
            frame_at,
            guide,
            detail,
            look.as_ref(),
            &m,
            |v| (settings.space.recode(v) * 65535.0).round() as u16,
        )),
        Format::Jpeg | Format::Png => Pixels::Eight(finish::finish_with(
            image,
            sampled,
            &global,
            &locals,
            position,
            frame_at,
            guide,
            detail,
            look.as_ref(),
            &m,
            |v| (settings.space.recode(v) * 255.0).round() as u8,
        )),
    };
    Rendered {
        width: image.width as u32,
        height: image.height as u32,
        pixels,
    }
}

/// What the finish is handed for a frame, before any pixel goes
/// through it: the picture through the resize (never the sharpen), in
/// the linear working space, with the guide plane's value and the
/// vignette's stops at each of its pixels. For a measurement that
/// emulates the finish on a block's means rather than finishing the
/// whole picture again for every setting tried
/// (`--fit-lightroom`).
pub(crate) struct Scene {
    pub(crate) image: WorkingImage,
    /// The guide at each pixel, none when no shift reads it or the
    /// frame has no plane; the plane's range beside it.
    pub(crate) guide: Option<Vec<f32>>,
    pub(crate) range: [f32; 2],
    /// The vignette's stops at each pixel, zero where it is off.
    pub(crate) stops: Vec<f32>,
}

/// [`render`]'s picture, guide and vignette before the finish, as
/// [`Scene`] has them: the same resize, the same mapping of a pixel to
/// the source and the same reading of the plane, so a block's mean of
/// these is what the finish would have read over that block.
pub(crate) fn scene_framed(
    image: &WorkingImage,
    edit: &greycard_edit::Edit,
    source: (u32, u32),
    settings: &Settings,
    guide: Option<&finish::Guide>,
) -> Scene {
    let fitted;
    let image = match fit(image, settings.long_edge) {
        Some(f) => {
            fitted = f;
            &fitted
        }
        None => image,
    };
    let (sw, sh) = (source.0 as f32, source.1 as f32);
    let frame = edit.geometry.frame(sw, sh);
    let scale = sw / image.width as f32;
    let position = |x: usize, y: usize| {
        let r = (
            frame.origin.0 + (x as f32 + 0.5) * scale,
            frame.origin.1 + (y as f32 + 0.5) * scale,
        );
        let s = edit.geometry.to_source(r, sw, sh);
        (s.0 / sw, s.1 / sw)
    };
    let plane = guide
        .filter(|g| !g.data.is_empty())
        .filter(|_| reads_guide(edit));
    let (w, h) = (image.width, image.height);
    let guide_at = plane.map(|g| {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = position(x, y);
                out.push(g.at(u * sw, v * sw));
            }
        }
        out
    });
    let vignette = edit.vignette;
    let (iw, ih) = (w as f32, h as f32);
    let mut stops = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            stops.push(if vignette.is_off() {
                0.0
            } else {
                vignette.amount * vignette.at((x as f32 + 0.5) / iw, (y as f32 + 0.5) / ih, iw / ih)
            });
        }
    }
    Scene {
        image: image.clone(),
        guide: guide_at,
        range: plane.map_or([0.0, 0.0], |g| g.range),
        stops,
    }
}

/// Add the settings' border, if any, round a rendered picture: on the
/// finished, encoded pixels, after the resize and the sharpening,
/// before the watermark and the file's encode. The picture grows by
/// the band on every side.
pub fn border(rendered: &mut Rendered, settings: &Settings) {
    let Some(border) = &settings.border else {
        return;
    };
    let (w, h) = (rendered.width, rendered.height);
    let (width, height) = match &mut rendered.pixels {
        Pixels::Eight(p) => {
            let (out, ow, oh) = border.apply_in(p, w, h, |l| settings.space.recode(l));
            *p = out;
            (ow, oh)
        }
        Pixels::Sixteen(p) => {
            let (out, ow, oh) = border.apply_in(p, w, h, |l| settings.space.recode(l));
            *p = out;
            (ow, oh)
        }
    };
    rendered.width = width;
    rendered.height = height;
}

/// Lay the settings' watermark, if any, over a rendered picture: on
/// the finished, encoded pixels, after the resize and the sharpening,
/// before the file's encode.
pub fn mark(rendered: &mut Rendered, settings: &Settings) -> Result<()> {
    let Some(mark) = &settings.watermark else {
        return Ok(());
    };
    let (w, h) = (rendered.width, rendered.height);
    let start = std::time::Instant::now();
    match &mut rendered.pixels {
        Pixels::Eight(p) => mark.apply(p, w, h, settings.space)?,
        Pixels::Sixteen(p) => mark.apply(p, w, h, settings.space)?,
    }
    tracing::debug!(
        "watermark on {w}x{h} in {:.3} s",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Whether either exposure shift or the whites is set, in the
/// picture's own look or in a local adjustment that acts: the only
/// things that read the guide plane. The shifts read its regions;
/// whites, a white point on the pixel's own luminance, reads only the
/// frame's range from it, where its pivot sits.
pub(crate) fn reads_guide(edit: &greycard_edit::Edit) -> bool {
    let set = |t: &greycard_edit::Tone| t.highlights != 0.0 || t.shadows != 0.0 || t.whites != 0.0;
    set(&edit.look().light.effective().tone)
        || edit
            .adjustments
            .iter()
            .any(|a| a.enabled && !a.mask.is_empty() && set(&a.look.light.effective().tone))
}

/// The image no larger than `long_edge` on its long side, in linear
/// light, Lanczos; none when it already fits.
pub(crate) fn fit(image: &WorkingImage, long_edge: Option<u32>) -> Option<WorkingImage> {
    let long = image.width.max(image.height) as u32;
    let edge = long_edge?;
    if edge >= long {
        return None;
    }
    let scale = edge as f64 / long as f64;
    let w = ((image.width as f64 * scale).round() as u32).max(1);
    let h = ((image.height as f64 * scale).round() as u32).max(1);
    fit_exact(image, w, h)
}

/// `image` resampled to `w` by `h` as [`fit`] resamples it, up or down;
/// `None` when it is that size already.
pub(crate) fn fit_exact(image: &WorkingImage, w: u32, h: u32) -> Option<WorkingImage> {
    if (image.width as u32, image.height as u32) == (w, h) {
        return None;
    }
    let src: image::ImageBuffer<image::Rgb<f32>, &[f32]> =
        image::ImageBuffer::from_raw(image.width as u32, image.height as u32, &image.data[..])?;
    let out = image::imageops::resize(&src, w, h, image::imageops::FilterType::Lanczos3);
    Some(WorkingImage {
        width: w as usize,
        height: h as usize,
        data: out.into_raw(),
    })
}

/// What an export says about itself beyond the source's tags: where
/// it came from and the edit that made it, both into the XMP.
#[derive(Debug, Clone, Default)]
pub struct Origin {
    /// The source file's name.
    pub source_name: Option<String>,
    /// The edit, as the sidecar writes it.
    pub edit: Option<String>,
}

impl Settings {
    /// What the sheet asked for, in words, for the export's XMP.
    pub fn describe(&self) -> String {
        let mut s = self.format.name().to_string();
        if self.format == Format::Jpeg {
            s.push_str(&format!(" quality {}", self.quality));
        }
        match self.long_edge {
            Some(edge) => s.push_str(&format!(", long edge {edge}")),
            None => s.push_str(", full size"),
        }
        s.push_str(&format!(", {}", self.space.name()));
        if !self.embed_profile {
            s.push_str(" without its profile");
        }
        if self.sharpen != Sharpen::Off && self.long_edge.is_some() {
            s.push_str(&format!(", output sharpening {}", self.sharpen.name()));
        }
        if let Some(b) = &self.border {
            let percent = (b.width * 1000.0).round() / 10.0;
            s.push_str(&match b.color {
                crate::border::Color::Match => {
                    format!(", a border {percent} % in the picture's mean color")
                }
                c => format!(", a {} border {percent} %", c.name().to_lowercase()),
            });
        }
        match self.watermark.as_ref().map(|m| &m.kind) {
            Some(crate::watermark::Kind::Text { .. }) => s.push_str(", a text watermark"),
            Some(crate::watermark::Kind::Image { .. }) => s.push_str(", an image watermark"),
            None => {}
        }
        s
    }
}

/// Write a rendered picture to `path` in the settings' format, with
/// the source's EXIF carried over when `source` gives it, and the
/// export's own account of itself in the XMP.
pub fn write(
    rendered: &Rendered,
    settings: &Settings,
    path: &Path,
    source: Option<&RawMetadata>,
    origin: &Origin,
) -> Result<()> {
    use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
    use image::{ExtendedColorType, ImageEncoder};
    let icc = if settings.embed_profile {
        Some(settings.space.icc()?)
    } else {
        None
    };
    let (w, h) = (rendered.width, rendered.height);
    let output = settings.describe();
    let source = source.filter(|_| settings.metadata != Metadata::None);
    let edit = origin
        .edit
        .as_deref()
        .filter(|_| settings.metadata == Metadata::All);
    let provenance = source.map(|metadata| Provenance {
        metadata,
        software: SOFTWARE,
        width: w,
        height: h,
        srgb: settings.space == Space::Srgb,
        written: Some(std::time::SystemTime::now()),
        source_name: origin.source_name.as_deref(),
        output: Some(&output),
        edit,
    });
    let payload = match &provenance {
        Some(p) => Some(exif::payload(p).context("the EXIF")?),
        None => None,
    };
    // The image crate's encoders take no XMP: the JPEG and the PNG are
    // encoded to memory and get their packet before they are written.
    let mut bytes: Vec<u8> = Vec::new();
    match (&rendered.pixels, settings.format) {
        (Pixels::Eight(p), Format::Jpeg) => {
            let mut enc = JpegEncoder::new_with_quality(&mut bytes, settings.quality.clamp(1, 100));
            if let Some(icc) = icc {
                enc.set_icc_profile(icc).context("a profile in a JPEG")?;
            }
            if let Some(exif) = payload {
                enc.set_exif_metadata(exif).context("EXIF in a JPEG")?;
            }
            enc.write_image(p, w, h, ExtendedColorType::Rgb8)?;
        }
        (Pixels::Eight(p), Format::Png) => {
            let mut enc = PngEncoder::new(&mut bytes);
            if let Some(icc) = icc {
                enc.set_icc_profile(icc).context("a profile in a PNG")?;
            }
            if let Some(exif) = payload {
                enc.set_exif_metadata(exif).context("EXIF in a PNG")?;
            }
            enc.write_image(p, w, h, ExtendedColorType::Rgb8)?;
        }
        (Pixels::Sixteen(p), Format::Tiff) => {
            let mut cursor = std::io::Cursor::new(&mut bytes);
            exif::write_rgb16_tiff(&mut cursor, w, h, p, icc.as_deref(), provenance.as_ref())
                .context("writing the TIFF")?;
        }
        _ => bail!("the pixels' depth is not the format's"),
    }
    if let Some(p) = &provenance
        && settings.format != Format::Tiff
    {
        bytes = exif::with_xmp(bytes, p);
    }
    std::fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The `Software` tag on what the editor writes.
const SOFTWARE: &str = concat!("greycard ", env!("CARGO_PKG_VERSION"));

/// What a file chooser is being asked for. The desktop portal and the
/// native dialogs answer the same request in their own ways, so the
/// calls below build one of these and hand it to `ask`.
struct Ask {
    /// The dialog's title.
    title: &'static str,
    /// The folder it opens on.
    folder: PathBuf,
    /// The name a save dialog offers. None asks for a file to open:
    /// there is nothing to name until the user picks one.
    name: Option<String>,
    /// A name and its globs; empty offers no filter at all.
    filter: (String, Vec<String>),
    /// A folder chooser rather than a file one.
    directory: bool,
    /// Files to open, as many as the user picks, rather than one.
    multiple: bool,
}

/// Ask the desktop for a file to open: `start` is the folder to open
/// on, `filter` a name and its globs. `done` gets the path, none when
/// the user canceled, or the error when there was no chooser to ask.
pub fn choose_open(
    title: &'static str,
    start: PathBuf,
    filter: (String, Vec<String>),
    done: impl FnOnce(Result<Option<PathBuf>>) + Send + 'static,
) {
    ask(
        Ask {
            title,
            folder: start,
            name: None,
            filter,
            directory: false,
            multiple: false,
        },
        move |picked| done(picked.map(|p| p.into_iter().next())),
    );
}

/// Ask for files to open, the same way, as many as the user picks:
/// `done` gets them, none when the user canceled.
pub fn choose_open_many(
    title: &'static str,
    start: PathBuf,
    filter: (String, Vec<String>),
    done: impl FnOnce(Result<Vec<PathBuf>>) + Send + 'static,
) {
    ask(
        Ask {
            title,
            folder: start,
            name: None,
            filter,
            directory: false,
            multiple: true,
        },
        done,
    );
}

/// Ask the desktop for a folder to open, the same way. A folder
/// chooser has no filter and replies with one directory.
pub fn choose_folder(
    title: &'static str,
    start: PathBuf,
    done: impl FnOnce(Result<Option<PathBuf>>) + Send + 'static,
) {
    ask(
        Ask {
            title,
            folder: start,
            name: None,
            filter: (String::new(), Vec::new()),
            directory: true,
            multiple: false,
        },
        move |picked| done(picked.map(|p| p.into_iter().next())),
    );
}

/// Linux asks the desktop's file chooser portal, on a thread of its
/// own since the dialog takes as long as the user and the call is a
/// blocking one. `done` gets what was picked, nothing when the user
/// canceled.
#[cfg(target_os = "linux")]
fn ask(request: Ask, done: impl FnOnce(Result<Vec<PathBuf>>) + Send + 'static) {
    // No file chooser from a test: it answers as a cancel.
    if cfg!(test) {
        return done(Ok(Vec::new()));
    }
    // The folder it opens on is looked at here, on the thread that
    // may wait on a mount, not on the window's.
    std::thread::spawn(move || {
        let mut request = request;
        request.folder = nearest_folder(&request.folder);
        done(choose(&request))
    });
}

/// The portal, or zenity and then kdialog when the portal could not
/// be asked at all: a window manager started bare, as i3 often is,
/// has no session bus or no portal on it, and either program draws
/// a chooser of its own. A portal that was asked and failed is not
/// asked again elsewhere, since the user may have seen its dialog.
/// With neither program there, the portal's error stands, and
/// `chooser_failed` says what to install.
#[cfg(target_os = "linux")]
fn choose(request: &Ask) -> Result<Vec<PathBuf>> {
    route(portal(request), || {
        [Helper::Zenity, Helper::Kdialog]
            .into_iter()
            .find_map(|helper| helper.ask(request))
    })
}

/// The portal's answer, or `helper`'s when the portal could not be
/// asked and a helper is installed to ask instead.
#[cfg(target_os = "linux")]
fn route(
    answer: Result<Vec<PathBuf>>,
    helper: impl FnOnce() -> Option<Result<Vec<PathBuf>>>,
) -> Result<Vec<PathBuf>> {
    let e = match answer {
        Err(e) if e.downcast_ref::<Unreached>().is_some() => e,
        answer => return answer,
    };
    tracing::info!("file chooser: {e:#}; trying zenity, then kdialog");
    helper().unwrap_or(Err(e))
}

/// Whether the portal's call failed because nothing was there to
/// take it: no portal on the bus, one that would not start, or no
/// file chooser behind it. Any other error is a portal that took the
/// request and refused it, our mistake (§88's empty filter was one)
/// and not a reason to ask zenity. zbus hands every error reply over
/// as a `MethodError`, by its D-Bus name.
#[cfg(target_os = "linux")]
fn absent(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => absent_name(name.as_str()),
        _ => false,
    }
}

/// `absent` by the error's D-Bus name. The bus's own: no owner, no
/// such method, a start that failed (`Spawn.*`), or a start that
/// timed out. dbus-daemon sends `TimedOut` only for a start, and
/// drops the call; the portal sends it only when its backend would
/// not start, before it asks the backend. Either way no dialog was
/// shown. Any of systemd's, which dbus-daemon passes on when systemd
/// will not queue the portal's unit (none, masked, a bad setting);
/// dbus-broker says `NameHasNoOwner` for all of those. `NoReply` is
/// not one: the call did reach a portal, which may yet show its
/// dialog.
#[cfg(target_os = "linux")]
fn absent_name(name: &str) -> bool {
    if let Some(name) = name.strip_prefix("org.freedesktop.DBus.Error.") {
        matches!(
            name,
            "ServiceUnknown"
                | "NameHasNoOwner"
                | "UnknownMethod"
                | "UnknownObject"
                | "UnknownInterface"
                | "TimedOut"
        ) || name.starts_with("Spawn.")
    } else {
        name.starts_with("org.freedesktop.systemd1.")
    }
}

/// Why the portal could not be asked, as against a portal asked that
/// then failed: only this sends the request to a helper instead.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Unreached {
    /// No session bus: `DBUS_SESSION_BUS_ADDRESS` unset and nothing at
    /// `$XDG_RUNTIME_DIR/bus`.
    Bus,
    /// A bus, but no portal on it with a file chooser behind it, or
    /// one that would not start.
    Portal,
}

#[cfg(target_os = "linux")]
impl std::fmt::Display for Unreached {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Unreached::Bus => "no D-Bus session bus",
            Unreached::Portal => "no file chooser portal on the session bus",
        })
    }
}

/// The status line for a chooser that answered with an error: what
/// was missing and what to install when it was the desktop's
/// plumbing, the error itself otherwise.
pub fn chooser_failed(e: &anyhow::Error) -> String {
    #[cfg(target_os = "linux")]
    match e.downcast_ref::<Unreached>() {
        Some(Unreached::Bus) => {
            return "no file chooser: there is no D-Bus session bus; \
                    install zenity, or see the user guide"
                .into();
        }
        Some(Unreached::Portal) => {
            return "no file chooser: no desktop portal answered; \
                    install xdg-desktop-portal-gtk or zenity"
                .into();
        }
        None => {}
    }
    format!("the file chooser failed: {e:#}")
}

/// A program that draws a file chooser and prints the paths picked,
/// one a line: GNOME's zenity, KDE's kdialog.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Helper {
    Zenity,
    Kdialog,
}

#[cfg(target_os = "linux")]
impl Helper {
    fn program(self) -> &'static str {
        match self {
            Helper::Zenity => "zenity",
            Helper::Kdialog => "kdialog",
        }
    }

    /// The request as the program's arguments. Both take the folder
    /// to open on as a path, with the offered name on the end for a
    /// save; zenity wants the trailing slash to open inside a folder
    /// rather than offer it as the name.
    fn args(self, request: &Ask) -> Vec<std::ffi::OsString> {
        use std::ffi::OsString;
        let mut start = request.folder.clone().into_os_string();
        match &request.name {
            Some(name) => {
                start.push("/");
                start.push(name);
            }
            None if self == Helper::Zenity => start.push("/"),
            None => {}
        }
        let (name, globs) = &request.filter;
        let globs: Vec<&str> = globs
            .iter()
            .map(String::as_str)
            .filter(|g| !g.is_empty())
            .collect();
        let filter = (!name.is_empty() && !globs.is_empty()).then(|| (name, globs.join(" ")));
        let mut args: Vec<OsString> = Vec::new();
        match self {
            Helper::Zenity => {
                args.push("--file-selection".into());
                args.push(format!("--title={}", request.title).into());
                let mut filename = OsString::from("--filename=");
                filename.push(&start);
                args.push(filename);
                if request.name.is_some() {
                    args.push("--save".into());
                    args.push("--confirm-overwrite".into());
                }
                if request.directory {
                    args.push("--directory".into());
                }
                if request.multiple {
                    args.push("--multiple".into());
                    args.push("--separator=\n".into());
                }
                if let Some((name, globs)) = filter {
                    args.push(format!("--file-filter={name} | {globs}").into());
                }
            }
            Helper::Kdialog => {
                args.push("--title".into());
                args.push(request.title.into());
                args.push(
                    if request.directory {
                        "--getexistingdirectory"
                    } else if request.name.is_some() {
                        "--getsavefilename"
                    } else {
                        "--getopenfilename"
                    }
                    .into(),
                );
                args.push(start);
                if let Some((name, globs)) = filter
                    && !request.directory
                {
                    // Qt's form: kdialog splits on `|` into separate
                    // filters, so `*.jpg|JPEG` would offer one that
                    // matches a file named JPEG.
                    args.push(format!("{name} ({globs})").into());
                }
                if request.multiple {
                    args.push("--multiple".into());
                    args.push("--separate-output".into());
                }
            }
        }
        args
    }

    /// The chooser, waited on: the paths picked, none for a cancel,
    /// and nothing at all when the program is not installed, so the
    /// next is tried.
    fn ask(self, request: &Ask) -> Option<Result<Vec<PathBuf>>> {
        self.run(self.program().as_ref(), request)
    }

    /// `ask` with the program named by path, for the tests' stand-ins.
    /// Both programs exit 1 for a cancel, but GTK exits 1 too when it
    /// cannot open the display, which is told apart by what it says
    /// (`display_failure`).
    /// zenity takes its exit codes from `ZENITY_OK` and the like when
    /// they are set, so they are not passed on. A path picked to open
    /// that is not there was mangled on its way back (kdialog prints
    /// in the locale's encoding, and a name with a newline in it
    /// splits in two), and is said so rather than opened.
    fn run(self, program: &std::ffi::OsStr, request: &Ask) -> Option<Result<Vec<PathBuf>>> {
        let name = self.program();
        let mut command = std::process::Command::new(program);
        for var in [
            "ZENITY_OK",
            "ZENITY_CANCEL",
            "ZENITY_ESC",
            "ZENITY_ERROR",
            "ZENITY_EXTRA",
            "ZENITY_TIMEOUT",
        ] {
            command.env_remove(var);
        }
        let output = match command
            .args(self.args(request))
            .stdin(std::process::Stdio::null())
            .output()
        {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => return Some(Err(e).with_context(|| format!("starting {name}"))),
            Ok(output) => output,
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        Some(match output.status.code() {
            Some(0) => {
                let paths = helper_paths(&output.stdout);
                match paths.iter().find(|p| request.name.is_none() && !p.exists()) {
                    Some(gone) => Err(anyhow::anyhow!(
                        "{name} answered with {}, which is not there",
                        gone.display()
                    )),
                    None => Ok(paths),
                }
            }
            Some(1) if display_failure(stderr).is_none() => Ok(Vec::new()),
            _ => {
                tracing::warn!("file chooser: {name}: {stderr}");
                match display_failure(stderr) {
                    Some(line) => Err(anyhow::anyhow!("{name}: {line}")),
                    None => Err(anyhow::anyhow!("{name} {}", output.status)),
                }
            }
        })
    }
}

/// The line of a helper's stderr that says it could not reach the
/// display: GTK 3's "cannot open display" and GTK 4's "failed to
/// open display" before they exit 1, Qt's "could not connect to
/// display" before it aborts. Only those: GTK
/// mentions the display in warnings it prints on a dialog that works,
/// as Xlib's missing RANDR under Xvfb.
#[cfg(target_os = "linux")]
fn display_failure(stderr: &str) -> Option<&str> {
    stderr.lines().map(str::trim).find(|line| {
        let line = line.to_lowercase();
        [
            "cannot open display",
            "failed to open display",
            "could not connect to display",
        ]
        .iter()
        .any(|phrase| line.contains(phrase))
    })
}

/// A helper's output as paths: one a line, the blank ones dropped.
#[cfg(target_os = "linux")]
fn helper_paths(stdout: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    stdout
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| PathBuf::from(std::ffi::OsString::from_vec(line.to_vec())))
        .collect()
}

/// `start` if it is a folder, else the nearest of its parents that
/// is: a chooser opened on a folder since removed would otherwise
/// open nowhere in particular. A mount point left behind with nothing
/// mounted on it is still a folder, and passes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn nearest_folder(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| start.to_path_buf())
}

/// Everywhere else it is the platform's own dialog, through `rfd`.
/// AppKit and the Windows common dialogs want the thread their window
/// is on, so the dialog is built on the event loop's thread rather
/// than a thread of its own, and awaited there too: `rfd` hands back
/// a future the loop can poll, so the window behind the dialog goes
/// on drawing. `done` then runs on that thread, where the callers'
/// `invoke_from_event_loop` is a queued call like any other.
#[cfg(not(target_os = "linux"))]
fn ask(request: Ask, done: impl FnOnce(Result<Vec<PathBuf>>) + Send + 'static) {
    if cfg!(test) {
        return done(Ok(Vec::new()));
    }
    // `done` is answered exactly once, by whichever of the two arms
    // gets to it: the dialog when there is an event loop to put it
    // on, this thread when there is not. It waits in a slot both can
    // reach because posting to the loop takes ownership of it.
    let slot = Arc::new(Mutex::new(Some(done)));
    let posted = {
        let slot = slot.clone();
        slint::invoke_from_event_loop(move || {
            let dialog = native(&request);
            let spawned = slint::spawn_local(async move {
                let picked: Vec<rfd::FileHandle> = if request.directory {
                    dialog.pick_folder().await.into_iter().collect()
                } else if request.name.is_some() {
                    dialog.save_file().await.into_iter().collect()
                } else if request.multiple {
                    dialog.pick_files().await.unwrap_or_default()
                } else {
                    dialog.pick_file().await.into_iter().collect()
                };
                if let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    done(Ok(picked.iter().map(|f| f.path().to_path_buf()).collect()));
                }
            });
            if spawned.is_err() {
                tracing::error!("file chooser: the event loop would not take it");
            }
        })
    };
    if posted.is_err()
        && let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    {
        // No window yet, so nowhere to put a dialog; the caller hears
        // it the way it hears a missing portal.
        done(Err(anyhow::anyhow!("no event loop to ask the desktop on")));
    }
}

/// The platform's dialog as `rfd` builds it. A filter's globs are
/// patterns to the portal but bare extensions here, so `*.jpg`
/// becomes `jpg`; a filter with nothing left is not offered.
#[cfg(not(target_os = "linux"))]
fn native(request: &Ask) -> rfd::AsyncFileDialog {
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title(request.title)
        .set_directory(&request.folder);
    if let Some(name) = &request.name {
        dialog = dialog.set_file_name(name);
    }
    let (name, globs) = &request.filter;
    let extensions = native_extensions(globs);
    if !name.is_empty() && !extensions.is_empty() {
        dialog = dialog.add_filter(name, &extensions);
    }
    dialog
}

/// `*.jpg` and `*.JPG` to the one extension a native dialog matches
/// with, case and all, in the order they were given.
#[cfg(not(target_os = "linux"))]
fn native_extensions(globs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(globs.len());
    for glob in globs {
        let extension = glob.rsplit('.').next().unwrap_or_default().to_lowercase();
        if extension.is_empty() || extension.contains('*') || out.contains(&extension) {
            continue;
        }
        out.push(extension);
    }
    out
}

/// The portal's `filters` option, `a(sa(us))`: each filter a name and
/// a list of patterns, each pattern a kind (0 a glob, 1 a MIME type)
/// and the pattern itself.
#[cfg(target_os = "linux")]
type PortalFilters = Vec<(String, Vec<(u32, String)>)>;

/// A name and its globs as the portal wants them. A filter must have
/// a name and at least one pattern, so an empty one is left out of
/// the options altogether rather than sent as a nameless entry, which
/// the portal refuses with `InvalidArgument`: a folder chooser asks
/// with no filter at all.
#[cfg(target_os = "linux")]
fn portal_filters(filter: (String, Vec<String>)) -> Option<PortalFilters> {
    let (name, globs) = filter;
    let globs: Vec<(u32, String)> = globs
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| (0u32, g))
        .collect();
    if name.is_empty() || globs.is_empty() {
        return None;
    }
    Some(vec![(name, globs)])
}

/// The portal's `SaveFile` or `OpenFile`: the request's folder is
/// where the chooser opens, its name is offered when there is one,
/// `directory` asks for a folder chooser rather than a file one, and
/// `multiple` for as many files as the user picks. Nothing picked is
/// a cancel.
#[cfg(target_os = "linux")]
fn portal(request: &Ask) -> Result<Vec<PathBuf>> {
    use std::collections::HashMap;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, SerializeDict, Type};

    #[derive(SerializeDict, Type)]
    #[zvariant(signature = "a{sv}")]
    struct Options {
        handle_token: String,
        modal: bool,
        /// Only a save dialog takes a name; left out otherwise.
        current_name: Option<String>,
        current_folder: Vec<u8>,
        /// Left out when there is no filter to offer: the portal
        /// refuses a filter whose name is empty.
        filters: Option<PortalFilters>,
        /// Only a folder chooser asks for a directory; left out
        /// otherwise.
        directory: Option<bool>,
        /// Likewise a chooser of several files.
        multiple: Option<bool>,
    }

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    let method = if request.name.is_some() {
        "SaveFile"
    } else {
        "OpenFile"
    };
    let conn = Connection::session().context(Unreached::Bus)?;
    let sender = conn
        .unique_name()
        .context(Unreached::Bus)?
        .trim_start_matches(':')
        .replace('.', "_");
    let token = format!(
        "greycard{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    // The reply comes as a signal on a request object whose path is
    // known in advance; listen before asking, so it cannot be missed.
    let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let signals = Proxy::new(
        &conn,
        PORTAL,
        request_path.as_str(),
        "org.freedesktop.portal.Request",
    )?;
    let mut responses = signals.receive_signal("Response")?;
    let chooser = Proxy::new(
        &conn,
        PORTAL,
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.FileChooser",
    )?;
    let mut folder = request.folder.as_os_str().as_bytes().to_vec();
    folder.push(0);
    let options = Options {
        handle_token: token,
        modal: true,
        current_name: request.name.clone(),
        current_folder: folder,
        filters: portal_filters(request.filter.clone()),
        directory: request.directory.then_some(true),
        multiple: request.multiple.then_some(true),
    };
    // Nothing is on the screen until the call returns, so a call that
    // found nothing to take it can go to a helper without a second
    // dialog.
    let handle: OwnedObjectPath = match chooser.call(method, &("", request.title, options)) {
        Ok(handle) => handle,
        Err(e) if absent(&e) => return Err(e).context(Unreached::Portal),
        Err(e) => return Err(e).context("the file chooser portal"),
    };
    if handle.as_str() != request_path {
        // An old portal names the request itself.
        let signals = Proxy::new(&conn, PORTAL, handle, "org.freedesktop.portal.Request")?;
        responses = signals.receive_signal("Response")?;
    }
    let message = responses.next().context("the file chooser went away")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize()?;
    // 1 is the Cancel button. 2 is the chooser ended some other way:
    // its window closed or Escape pressed, which the released GTK and
    // GNOME backends answer with 2 (xdg-desktop-portal-gtk's main
    // answers 1 since 2025), or the backend failing after the call
    // returned. The two cannot be told apart from here, and the first
    // is the common one, so 2 is a cancel too, with a line in the log
    // for the second; the portal's own log says which it was.
    match code {
        0 => {}
        1 => return Ok(Vec::new()),
        _ => {
            tracing::warn!(
                "file chooser: the portal ended it without an answer (response {code}): \
                 its window closed, or its backend failed"
            );
            return Ok(Vec::new());
        }
    }
    let uris = results.get("uris").context("a reply without a file")?;
    let uris = Vec::<String>::try_from(uris.clone()).context("the reply's file")?;
    if uris.is_empty() {
        anyhow::bail!("a reply without a file");
    }
    uris.iter()
        .map(|uri| {
            let path = uri
                .strip_prefix("file://")
                .with_context(|| format!("not a local file: {uri}"))?;
            Ok(PathBuf::from(percent_decode(path)))
        })
        .collect()
}

/// `%20` and friends back to bytes.
#[cfg(target_os = "linux")]
fn percent_decode(s: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                16,
            )
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    std::ffi::OsString::from_vec(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chooser_opens_on_the_nearest_folder_that_is_there() {
        let dir = crate::testing::scratch_dir("nearest-folder");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(nearest_folder(&dir), dir);
        assert_eq!(nearest_folder(&dir.join("gone").join("deeper")), dir);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_sheets_size_names_a_long_edge() {
        assert_eq!(long_edge("Full", "1600"), None);
        assert_eq!(long_edge("2048", "1600"), Some(2048));
        assert_eq!(long_edge(CUSTOM, "1600"), Some(1600));
        assert_eq!(long_edge(CUSTOM, " 3000 "), Some(3000));
        // Not a size: the picture's own, as Full.
        assert_eq!(long_edge(CUSTOM, ""), None);
        assert_eq!(long_edge(CUSTOM, "big"), None);
        assert_eq!(long_edge(CUSTOM, "-5"), None);
        assert_eq!(long_edge(CUSTOM, "1.5e3"), None);
        assert_eq!(long_edge(CUSTOM, "8"), None);
        assert_eq!(long_edge(CUSTOM, "16"), Some(16));
        // A name from another version is the picture's own too.
        assert_eq!(long_edge("Huge", "1600"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_chooser_sends_no_filter() {
        // The portal refuses a filter with an empty name, so the
        // folder chooser's empty filter has to be left out.
        assert_eq!(portal_filters((String::new(), Vec::new())), None);
        assert_eq!(portal_filters((String::new(), vec!["*.jpg".into()])), None);
        assert_eq!(portal_filters(("JPEG".into(), Vec::new())), None);
        assert_eq!(portal_filters(("JPEG".into(), vec![String::new()])), None);
        // A real filter still goes as it did: globs are glob-type 0.
        assert_eq!(
            portal_filters(("JPEG".into(), vec!["*.jpg".into(), "*.jpeg".into()])),
            Some(vec![(
                "JPEG".to_string(),
                vec![(0u32, "*.jpg".to_string()), (0u32, "*.jpeg".to_string())]
            )])
        );
        // Every export format offers one.
        for f in Format::ALL {
            let globs = f.globs().iter().map(|g| g.to_string()).collect();
            assert!(portal_filters((f.name().to_string(), globs)).is_some());
        }
    }

    #[cfg(target_os = "linux")]
    fn asking(name: Option<&str>, directory: bool, multiple: bool) -> Ask {
        Ask {
            title: "Pick",
            folder: PathBuf::from("/home/me/My Pictures"),
            name: name.map(str::to_string),
            filter: if directory {
                (String::new(), Vec::new())
            } else {
                ("JPEG".into(), vec!["*.jpg".into(), "*.jpeg".into()])
            },
            directory,
            multiple,
        }
    }

    #[cfg(target_os = "linux")]
    fn strings(args: Vec<std::ffi::OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn zenity_is_asked_what_the_portal_would_be() {
        let zenity = |ask: Ask| strings(Helper::Zenity.args(&ask));
        // A folder opens inside the folder, with no filter.
        assert_eq!(
            zenity(asking(None, true, false)),
            [
                "--file-selection",
                "--title=Pick",
                "--filename=/home/me/My Pictures/",
                "--directory"
            ]
        );
        // A save offers its name and confirms a replace.
        assert_eq!(
            zenity(asking(Some("a.jpg"), false, false)),
            [
                "--file-selection",
                "--title=Pick",
                "--filename=/home/me/My Pictures/a.jpg",
                "--save",
                "--confirm-overwrite",
                "--file-filter=JPEG | *.jpg *.jpeg"
            ]
        );
        // Several files come back one a line.
        assert_eq!(
            zenity(asking(None, false, true)),
            [
                "--file-selection",
                "--title=Pick",
                "--filename=/home/me/My Pictures/",
                "--multiple",
                "--separator=\n",
                "--file-filter=JPEG | *.jpg *.jpeg"
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kdialog_is_asked_what_the_portal_would_be() {
        let kdialog = |ask: Ask| strings(Helper::Kdialog.args(&ask));
        assert_eq!(
            kdialog(asking(None, true, false)),
            [
                "--title",
                "Pick",
                "--getexistingdirectory",
                "/home/me/My Pictures"
            ]
        );
        assert_eq!(
            kdialog(asking(Some("a.jpg"), false, false)),
            [
                "--title",
                "Pick",
                "--getsavefilename",
                "/home/me/My Pictures/a.jpg",
                "JPEG (*.jpg *.jpeg)"
            ]
        );
        assert_eq!(
            kdialog(asking(None, false, true)),
            [
                "--title",
                "Pick",
                "--getopenfilename",
                "/home/me/My Pictures",
                "JPEG (*.jpg *.jpeg)",
                "--multiple",
                "--separate-output"
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_helpers_answer_is_one_path_a_line() {
        assert_eq!(
            helper_paths(b"/a/b c.jpg\n/d/e.jpg\n"),
            [PathBuf::from("/a/b c.jpg"), PathBuf::from("/d/e.jpg")]
        );
        assert_eq!(helper_paths(b""), Vec::<PathBuf>::new());
        assert_eq!(helper_paths(b"\n"), Vec::<PathBuf>::new());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_a_portal_that_is_not_there_goes_to_a_helper() {
        for name in [
            "ServiceUnknown",
            "NameHasNoOwner",
            "UnknownMethod",
            "UnknownObject",
            "UnknownInterface",
            "Spawn.ChildExited",
            "Spawn.ExecFailed",
        ] {
            assert!(absent_name(&format!("org.freedesktop.DBus.Error.{name}")));
        }
        // The bus timing out a start answers before the portal.
        assert!(absent_name("org.freedesktop.DBus.Error.TimedOut"));
        // A portal started as a systemd unit that systemd would not
        // queue.
        for name in ["NoSuchUnit", "UnitMasked", "BadUnitSetting", "LoadFailed"] {
            assert!(absent_name(&format!("org.freedesktop.systemd1.{name}")));
        }
        // A portal that took the request and refused it is ours to
        // hear, and one that did not reply may yet show its dialog.
        for name in [
            "org.freedesktop.DBus.Error.InvalidArgs",
            "org.freedesktop.DBus.Error.NoReply",
            "org.freedesktop.portal.Error.InvalidArgument",
            "ServiceUnknown",
        ] {
            assert!(!absent_name(name), "{name}");
        }
        assert!(!absent(&zbus::Error::InvalidReply));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_helper_is_asked_only_when_the_portal_was_not_reached() {
        let unreached =
            || Err::<Vec<PathBuf>, _>(anyhow::anyhow!("ServiceUnknown")).context(Unreached::Portal);
        let picked = || Some(Ok(vec![PathBuf::from("/picked")]));
        // The portal's own answer, picked or canceled, stands.
        let asked = std::cell::Cell::new(false);
        let answer = route(Ok(vec![PathBuf::from("/portal")]), || {
            asked.set(true);
            picked()
        });
        assert_eq!(answer.unwrap(), [PathBuf::from("/portal")]);
        assert!(!asked.get());
        // A portal that failed after it was asked is not asked again.
        let failed = route(Err(anyhow::anyhow!("the file chooser went away")), || {
            asked.set(true);
            picked()
        });
        assert!(failed.is_err());
        assert!(!asked.get());
        // Not reached: the helper's answer.
        assert_eq!(
            route(unreached(), picked).unwrap(),
            [PathBuf::from("/picked")]
        );
        // Not reached and no helper: the portal's error, which says
        // what to install.
        let none = route(unreached(), || None).unwrap_err();
        assert!(chooser_failed(&none).contains("xdg-desktop-portal-gtk"));
    }

    /// A stand-in for zenity: a shell script with `body`, in `dir`.
    #[cfg(target_os = "linux")]
    fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_helpers_exit_says_picked_canceled_or_failed() {
        let dir = crate::testing::scratch_dir("helper-exits");
        std::fs::create_dir_all(dir.join("a b")).unwrap();
        std::fs::create_dir_all(dir.join("c")).unwrap();
        let run_asking = |body: &str, name: &str, ask: Ask| {
            let program = stub(&dir, name, body);
            // A script just written can be busy for a moment, while a
            // child another test forked still holds it open to write.
            for _ in 0..50 {
                let answer = Helper::Zenity.run(program.as_os_str(), &ask);
                let busy = matches!(&answer, Some(Err(e)) if e
                    .root_cause()
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::ExecutableFileBusy));
                if !busy {
                    return answer;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("{name} stayed busy");
        };
        let run = |body: &str, name: &str| run_asking(body, name, asking(None, false, true));
        // Two picked, one a line.
        let two = format!(
            "printf '%s\\n%s\\n' '{}' '{}'",
            dir.join("a b").display(),
            dir.join("c").display()
        );
        assert_eq!(
            run(&two, "two").unwrap().unwrap(),
            [dir.join("a b"), dir.join("c")]
        );
        // A cancel is nothing picked, not an error.
        assert_eq!(
            run("exit 1", "cancel").unwrap().unwrap(),
            Vec::<PathBuf>::new()
        );
        // GTK's exit 1 without a display is an error that says so.
        let no_display = run(
            "echo '(zenity:1): Gtk-WARNING **: cannot open display: :9' >&2; exit 1",
            "no-display",
        )
        .unwrap()
        .unwrap_err();
        assert!(no_display.to_string().contains("cannot open display"));
        // GTK 4's, as zenity 4 says it.
        let no_display = run("echo 'Failed to open display' >&2; exit 1", "no-display-4")
            .unwrap()
            .unwrap_err();
        assert!(no_display.to_string().contains("Failed to open display"));
        // A warning that mentions the display is still a cancel.
        assert_eq!(
            run(
                "echo 'Xlib:  extension \"RANDR\" missing on display \":1\".' >&2; exit 1",
                "randr",
            )
            .unwrap()
            .unwrap(),
            Vec::<PathBuf>::new()
        );
        // Anything else is an error.
        assert!(run("exit 255", "crash").unwrap().is_err());
        // A path that is not there came back mangled.
        assert!(
            run(&format!("echo '{}'", dir.join("gone").display()), "mangled")
                .unwrap()
                .is_err()
        );
        // A save names a file not there yet, and is not checked.
        let new = dir.join("new.jpg");
        assert_eq!(
            run_asking(
                &format!("echo '{}'", new.display()),
                "save",
                asking(Some("new.jpg"), false, false)
            )
            .unwrap()
            .unwrap(),
            [new]
        );
        // Not installed: the next helper is tried.
        assert!(
            Helper::Zenity
                .run(dir.join("missing").as_os_str(), &asking(None, true, false))
                .is_none()
        );
        crate::testing::remove_dir_retry(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_bus_or_portal_says_what_to_install() {
        let no_bus = Err::<(), _>(anyhow::anyhow!("connection refused"))
            .context(Unreached::Bus)
            .unwrap_err();
        assert!(chooser_failed(&no_bus).contains("no D-Bus session bus"));
        assert!(chooser_failed(&no_bus).contains("zenity"));
        let no_portal = Err::<(), _>(anyhow::anyhow!("ServiceUnknown"))
            .context(Unreached::Portal)
            .unwrap_err();
        assert!(chooser_failed(&no_portal).contains("xdg-desktop-portal-gtk"));
        // A portal that was asked and failed says so as it is.
        let failed = anyhow::anyhow!("the file chooser went away");
        assert_eq!(
            chooser_failed(&failed),
            "the file chooser failed: the file chooser went away"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_native_filter_takes_extensions_not_globs() {
        // A native dialog matches on the extension itself, so the
        // portal's patterns have to come apart; case folds, and a
        // pattern that leaves nothing behind is dropped.
        assert_eq!(
            native_extensions(&["*.jpg".to_string(), "*.jpeg".to_string()]),
            vec!["jpg".to_string(), "jpeg".to_string()]
        );
        // The same extension in two cases is one entry, in the order
        // it was first given.
        assert_eq!(
            native_extensions(&[
                "*.xmp".to_string(),
                "*.XMP".to_string(),
                "*.gcp".to_string()
            ]),
            vec!["xmp".to_string(), "gcp".to_string()]
        );
        assert_eq!(native_extensions(&[]), Vec::<String>::new());
        assert_eq!(native_extensions(&[String::new()]), Vec::<String>::new());
        assert_eq!(native_extensions(&["*".to_string()]), Vec::<String>::new());
        // Every export format still offers one.
        for f in Format::ALL {
            let globs: Vec<String> = f.globs().iter().map(|g| g.to_string()).collect();
            assert!(!native_extensions(&globs).is_empty());
        }
    }

    #[test]
    fn names_and_extensions_round_trip() {
        for f in Format::ALL {
            assert_eq!(Format::from_name(f.name()), Some(f));
            assert_eq!(
                Format::from_path(Path::new(&format!("a.{}", f.extension()))),
                Some(f)
            );
        }
        assert_eq!(Format::from_path(Path::new("a.JPEG")), Some(Format::Jpeg));
        assert_eq!(Format::from_path(Path::new("a.cr3")), None);
        for s in Space::ALL {
            assert_eq!(Space::from_name(s.name()), Some(s));
        }
        for s in Sharpen::ALL {
            assert_eq!(Sharpen::from_name(s.name()), Some(s));
        }
        assert!(Sharpen::Off.options().is_none());
    }

    #[test]
    fn the_profiles_say_what_the_matrices_do() {
        // A working-space color through the matrix into the space,
        // encoded, then through the profile back to XYZ, lands where
        // the same color through the sRGB matrix and profile does.
        use lcms2::{Intent, PixelFormat, Profile, Transform};
        let to_xyz = |space: Space, rgb: [f32; 3]| -> [f64; 3] {
            let profile = Profile::new_icc(&space.icc().unwrap()).unwrap();
            let xyz = Profile::new_xyz();
            let t: Transform<[f32; 3], [f64; 3]> = Transform::new(
                &profile,
                PixelFormat::RGB_FLT,
                &xyz,
                PixelFormat::XYZ_DBL,
                Intent::RelativeColorimetric,
            )
            .unwrap();
            let m = space.matrix();
            let lin = [0, 1, 2].map(|r| m[r][0] * rgb[0] + m[r][1] * rgb[1] + m[r][2] * rgb[2]);
            let enc = lin.map(|v| space.recode(finish::encode(v.clamp(0.0, 1.0))));
            let mut out = [[0f64; 3]];
            t.transform_pixels(&[enc], &mut out);
            out[0]
        };
        // A color inside all three gamuts.
        let color = [0.4, 0.3, 0.2];
        let reference = to_xyz(Space::Srgb, color);
        for space in [Space::DisplayP3, Space::Rec2020, Space::AdobeRgb] {
            let xyz = to_xyz(space, color);
            for k in 0..3 {
                assert!(
                    (xyz[k] - reference[k]).abs() < 2e-3,
                    "{}: {xyz:?} vs {reference:?}",
                    space.name()
                );
            }
        }
        // Rec.2020 is the working space: its matrix is the identity.
        let m = Space::Rec2020.matrix();
        for r in 0..3 {
            for c in 0..3 {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((m[r][c] - want).abs() < 1e-5, "{m:?}");
            }
        }
    }

    #[test]
    fn the_four_names_round_trip() {
        let names = ["sRGB", "Display P3", "Rec.2020", "Adobe RGB"];
        assert_eq!(Space::ALL.map(Space::name), names);
        for name in names {
            assert_eq!(Space::from_name(name).map(Space::name), Some(name));
        }
        assert_eq!(Space::from_name("Adobe"), None);
    }

    #[test]
    fn the_spaces_under_the_srgb_curve_encode_as_they_did() {
        // `recode` leaves the finish's value alone, bit for bit, and a
        // render writes what the finish quantizes, as it did before
        // there was a curve to choose.
        for space in [Space::Srgb, Space::DisplayP3, Space::Rec2020] {
            for i in 0..=65535u32 {
                let v = i as f32 / 65535.0;
                assert_eq!(space.recode(v).to_bits(), v.to_bits(), "{}", space.name());
            }
        }
        let (w, h) = (24, 16);
        let data: Vec<f32> = (0..w * h * 3)
            .map(|i| ((i * 37) % 101) as f32 / 90.0)
            .collect();
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let edit = greycard_edit::Edit::default();
        let source = (w as u32, h as u32);
        for space in [Space::Srgb, Space::DisplayP3, Space::Rec2020] {
            for format in [Format::Jpeg, Format::Tiff] {
                let settings = Settings {
                    format,
                    space,
                    ..Settings::default()
                };
                let rendered = render(
                    &image,
                    &edit,
                    source,
                    &settings,
                    &Default::default(),
                    1.0,
                    None,
                    finish::Source::Scene,
                    None,
                    None,
                );
                let global = finish::Baked::global(&edit, finish::Source::Scene);
                let m = space.matrix();
                let plain = |q: &(dyn Fn(f32) -> u32 + Sync)| -> Vec<u32> {
                    finish::finish_with(
                        &image,
                        None,
                        &global,
                        &[],
                        |x, y| (x as f32 / w as f32, y as f32 / w as f32),
                        |_, _| (0.0, None),
                        None,
                        None,
                        None,
                        &m,
                        q,
                    )
                };
                match &rendered.pixels {
                    Pixels::Eight(p) => {
                        let want = plain(&|v| (v * 255.0).round() as u32);
                        assert!(p.iter().zip(&want).all(|(a, b)| *a as u32 == *b));
                    }
                    Pixels::Sixteen(p) => {
                        let want = plain(&|v| (v * 65535.0).round() as u32);
                        assert!(p.iter().zip(&want).all(|(a, b)| *a as u32 == *b));
                    }
                }
            }
        }
    }

    #[test]
    fn a_border_keeps_its_grey_on_every_curve() {
        use crate::border::{Border, Color};
        let (w, h) = (8usize, 6usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: vec![0.3; w * h * 3],
        };
        let edit = greycard_edit::Edit::default();
        let render_with = |space: Space, color: Color| {
            let settings = Settings {
                format: Format::Tiff,
                space,
                border: Some(Border { width: 0.25, color }),
                ..Settings::default()
            };
            let mut rendered = render(
                &image,
                &edit,
                (w as u32, h as u32),
                &settings,
                &Default::default(),
                1.0,
                None,
                finish::Source::Scene,
                None,
                None,
            );
            let plain = match &rendered.pixels {
                Pixels::Sixteen(p) => p.clone(),
                _ => unreachable!(),
            };
            border(&mut rendered, &settings);
            (rendered, plain, settings)
        };
        // The old spaces: the border is what `apply` makes of the
        // picture, with no recode.
        for space in [Space::Srgb, Space::DisplayP3, Space::Rec2020] {
            for color in Color::ALL {
                let (rendered, plain, settings) = render_with(space, color);
                let want = settings.border.unwrap().apply(&plain, w as u32, h as u32);
                let Pixels::Sixteen(p) = &rendered.pixels else {
                    unreachable!()
                };
                assert_eq!(p, &want.0, "{} {}", space.name(), color.name());
            }
        }
        // Adobe RGB: the grey is the same light, so the same L*.
        for (color, level) in [(Color::LightGrey, 0.8f32), (Color::DarkGrey, 0.2)] {
            let (rendered, _, _) = render_with(Space::AdobeRgb, color);
            let Pixels::Sixteen(p) = &rendered.pixels else {
                unreachable!()
            };
            let v = p[0] as f64 / 65535.0;
            let light = v.powf(ADOBE_RGB_GAMMA);
            let want = finish::decode(level) as f64;
            assert!((light - want).abs() < 1e-3, "{level}: {light} vs {want}");
        }
    }

    #[test]
    fn adobe_rgb_encodes_with_its_gamma_and_its_profile_says_so() {
        // The profile read back by lcms: a grey's luminance is the
        // value to the 563/256, and that is what a linear light
        // recodes to, to within 1/4096.
        use lcms2::{Intent, PixelFormat, Profile, Transform};
        let profile = Profile::new_icc(&Space::AdobeRgb.icc().unwrap()).unwrap();
        let xyz = Profile::new_xyz();
        let t: Transform<[f32; 3], [f64; 3]> = Transform::new(
            &profile,
            PixelFormat::RGB_FLT,
            &xyz,
            PixelFormat::XYZ_DBL,
            Intent::RelativeColorimetric,
        )
        .unwrap();
        let mut worst = 0f64;
        for lin in [0.0f32, 0.001, 0.01, 0.05, 0.18, 0.35, 0.6, 0.9, 1.0] {
            let e = Space::AdobeRgb.recode(finish::encode(lin));
            let mut out = [[0f64; 3]];
            t.transform_pixels(&[[e; 3]], &mut out);
            let off = (out[0][1] - lin as f64).abs();
            worst = worst.max(off);
            assert!(off < 1.0 / 4096.0, "{lin}: {out:?}");
            // And the curve itself.
            let want = (lin as f64).powf(1.0 / ADOBE_RGB_GAMMA);
            assert!((e as f64 - want).abs() < 1e-6, "{lin}: {e} vs {want}");
        }
        eprintln!("adobe profile round trip, worst luminance error {worst:e}");
        // The finish's own view of the space is the sRGB curve.
        let view = Profile::new_icc(&Space::AdobeRgb.finish_icc().unwrap()).unwrap();
        let t: Transform<[f32; 3], [f64; 3]> = Transform::new(
            &view,
            PixelFormat::RGB_FLT,
            &xyz,
            PixelFormat::XYZ_DBL,
            Intent::RelativeColorimetric,
        )
        .unwrap();
        let mut out = [[0f64; 3]];
        t.transform_pixels(&[[finish::encode(0.18); 3]], &mut out);
        assert!((out[0][1] - 0.18).abs() < 1.0 / 4096.0, "{out:?}");
    }

    #[test]
    fn an_adobe_rgb_jpeg_embeds_adobe_rgb() {
        use lcms2::{InfoType, Intent, Locale, PixelFormat, Profile, Transform};
        let (w, h) = (16, 12);
        let image = WorkingImage {
            width: w,
            height: h,
            data: vec![0.2; w * h * 3],
        };
        let settings = Settings {
            format: Format::Jpeg,
            space: Space::AdobeRgb,
            ..Settings::default()
        };
        let rendered = render(
            &image,
            &greycard_edit::Edit::default(),
            (w as u32, h as u32),
            &settings,
            &Default::default(),
            1.0,
            None,
            finish::Source::Scene,
            None,
            None,
        );
        let dir = std::env::temp_dir().join(format!("greycard-adobe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jpg");
        write(&rendered, &settings, &path, None, &Origin::default()).unwrap();
        let mut decoder = image::ImageReader::open(&path)
            .unwrap()
            .into_decoder()
            .unwrap();
        let icc = image::ImageDecoder::icc_profile(&mut decoder)
            .unwrap()
            .expect("a profile");
        assert_eq!(icc, Space::AdobeRgb.icc().unwrap());
        let profile = Profile::new_icc(&icc).unwrap();
        assert!(
            profile
                .info(InfoType::Description, Locale::none())
                .unwrap_or_default()
                .contains("Adobe RGB (compatible)")
        );
        // Its colorants, adapted to D50.
        let xyz = Profile::new_xyz();
        let t: Transform<[f32; 3], [f64; 3]> = Transform::new(
            &profile,
            PixelFormat::RGB_FLT,
            &xyz,
            PixelFormat::XYZ_DBL,
            Intent::RelativeColorimetric,
        )
        .unwrap();
        let mut out = [[0f64; 3]; 3];
        t.transform_pixels(
            &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            &mut out,
        );
        let want = [
            [0.6097, 0.3111, 0.0195],
            [0.2053, 0.6257, 0.0609],
            [0.1492, 0.0632, 0.7446],
        ];
        for (got, want) in out.iter().zip(want) {
            for k in 0..3 {
                assert!((got[k] - want[k]).abs() < 1e-3, "{out:?}");
            }
        }
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn output_sharpening_steepens_the_edges_and_leaves_the_flats() {
        // A soft vertical edge in a mid grey, blurred over a few
        // pixels, with a flat field beside it; downsized to half.
        let (w, h) = (256, 128);
        let mut data = Vec::with_capacity(w * h * 3);
        for _y in 0..h {
            for x in 0..w {
                let t = ((x as f32 - 128.0) / 6.0).tanh() * 0.5 + 0.5;
                let v = 0.1 + 0.4 * t;
                data.extend_from_slice(&[v, v * 0.8, v * 0.6]);
            }
        }
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let edit = greycard_edit::Edit::default();
        let source = (w as u32, h as u32);
        let at = |level: Sharpen| {
            let settings = Settings {
                format: Format::Tiff,
                long_edge: Some(128),
                sharpen: level,
                ..Settings::default()
            };
            let rendered = render(
                &image,
                &edit,
                source,
                &settings,
                &Default::default(),
                1.0,
                None,
                finish::Source::Scene,
                None,
                None,
            );
            let Pixels::Sixteen(p) = rendered.pixels else {
                panic!("a TIFF is sixteen bits")
            };
            assert_eq!((rendered.width, rendered.height), (128, 64));
            p
        };
        let (off, low, standard, high) = (
            at(Sharpen::Off),
            at(Sharpen::Low),
            at(Sharpen::Standard),
            at(Sharpen::High),
        );
        // The steepest step along the middle row, in the green.
        let slope = |p: &[u16]| {
            let row = &p[32 * 128 * 3..33 * 128 * 3];
            (0..127)
                .map(|x| (row[x * 3 + 1] as i32 - row[(x + 1) * 3 + 1] as i32).abs())
                .max()
                .unwrap()
        };
        // On this edge, half a dozen pixels wide before the resize,
        // the levels steepen it by about one, three and six percent;
        // the point spread deconvolved is the resize's, so a wide edge
        // moves little and the pixel-level detail of a photograph
        // moves most.
        assert!(slope(&low) > slope(&off));
        assert!(slope(&standard) > slope(&low));
        assert!(slope(&high) > slope(&standard));
        // Far from the edge nothing moves, and color ratios hold on
        // the edge itself.
        for p in [&low, &standard, &high] {
            for x in [4usize, 120] {
                let i = (32 * 128 + x) * 3;
                assert!(
                    p[i].abs_diff(off[i]) <= 2,
                    "flat at {x}: {} vs {}",
                    p[i],
                    off[i]
                );
            }
            let i = (32 * 128 + 64) * 3;
            let ratio = |p: &[u16]| p[i + 1] as f32 / p[i] as f32;
            assert!((ratio(p) - ratio(&off)).abs() < 0.01);
        }
    }

    #[test]
    fn a_resize_keeps_the_light_and_a_tiff_keeps_the_bits() {
        let (w, h) = (64, 40);
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let v = ((x + y) % 7) as f32 * 0.05;
                data.extend_from_slice(&[v, v * 0.5, 0.1]);
            }
        }
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let mean = |img: &WorkingImage| img.data.iter().sum::<f32>() / img.data.len() as f32;
        let small = fit(&image, Some(32)).unwrap();
        assert_eq!((small.width, small.height), (32, 20));
        assert!((mean(&small) - mean(&image)).abs() < 2e-3);
        assert!(fit(&image, Some(64)).is_none(), "never enlarged");

        let edit = greycard_edit::Edit::default();
        let source = (image.width as u32, image.height as u32);
        let settings = Settings {
            format: Format::Tiff,
            long_edge: Some(32),
            space: Space::DisplayP3,
            ..Settings::default()
        };
        let rendered = render(
            &image,
            &edit,
            source,
            &settings,
            &Default::default(),
            1.0,
            None,
            finish::Source::Scene,
            None,
            None,
        );
        let Pixels::Sixteen(p) = &rendered.pixels else {
            panic!("a TIFF is sixteen bits")
        };
        assert_eq!(p.len(), 32 * 20 * 3);
        let dir = std::env::temp_dir().join(format!("greycard-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.tif");
        write(&rendered, &settings, &path, None, &Origin::default()).unwrap();
        let back = image::open(&path).unwrap().into_rgb16();
        assert_eq!(back.as_raw(), p);
        // The JPEG carries the profile.
        let jpeg = Settings {
            format: Format::Jpeg,
            ..settings
        };
        let path = dir.join("t.jpg");
        write(
            &render(
                &image,
                &edit,
                source,
                &jpeg,
                &Default::default(),
                1.0,
                None,
                finish::Source::Scene,
                None,
                None,
            ),
            &jpeg,
            &path,
            None,
            &Origin::default(),
        )
        .unwrap();
        let mut decoder = image::ImageReader::open(&path)
            .unwrap()
            .into_decoder()
            .unwrap();
        let icc = image::ImageDecoder::icc_profile(&mut decoder)
            .unwrap()
            .expect("a profile");
        assert_eq!(icc, Space::DisplayP3.icc().unwrap());
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn every_format_carries_the_source_exif() {
        let (w, h) = (24usize, 16usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: vec![0.2; w * h * 3],
        };
        let metadata = RawMetadata {
            exif: Default::default(),
            model: "EOS R6m2".into(),
            make: "Canon".into(),
            lens: None,
            unique_image_id: None,
            rating: None,
        };
        let edit = greycard_edit::Edit::default();
        let source = (w as u32, h as u32);
        let dir = std::env::temp_dir().join(format!("greycard-exif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let carries = |bytes: &[u8], what: &[u8]| bytes.windows(what.len()).any(|w| w == what);
        for format in Format::ALL {
            let settings = Settings {
                format,
                ..Settings::default()
            };
            let path = dir.join(format!("e.{}", format.extension()));
            let rendered = render(
                &image,
                &edit,
                source,
                &settings,
                &Default::default(),
                1.0,
                None,
                finish::Source::Scene,
                None,
                None,
            );
            let origin = Origin {
                source_name: Some("IMG_0001.CR3".into()),
                edit: Some(edit.to_json()),
            };
            write(&rendered, &settings, &path, Some(&metadata), &origin).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            // The export's own account, in the XMP, whatever the container.
            // The TIFF's packet lies in the file as it is (the image crate
            // sizes the tiff crate's buffers by the picture, and refuses a
            // packet longer than this small one's pixels; exiv2 reads it).
            let xmp = match format {
                Format::Tiff => bytes.clone(),
                _ => {
                    let mut decoder = image::ImageReader::open(&path)
                        .unwrap()
                        .into_decoder()
                        .unwrap();
                    image::ImageDecoder::xmp_metadata(&mut decoder)
                        .unwrap()
                        .expect("XMP")
                }
            };
            assert!(carries(
                &xmp,
                b"<greycard:Source>IMG_0001.CR3</greycard:Source>"
            ));
            assert!(carries(&xmp, b"<greycard:Edit>"), "{}", format.name());
            assert!(
                carries(&xmp, settings.describe().as_bytes()),
                "{}",
                format.name()
            );
            // The camera and the software, wherever the container keeps them.
            let exif = match format {
                Format::Tiff => bytes,
                _ => {
                    let mut decoder = image::ImageReader::open(&path)
                        .unwrap()
                        .into_decoder()
                        .unwrap();
                    image::ImageDecoder::exif_metadata(&mut decoder)
                        .unwrap()
                        .expect("EXIF")
                }
            };
            assert!(carries(&exif, b"EOS R6m2"), "{}", format.name());
            assert!(carries(&exif, SOFTWARE.as_bytes()), "{}", format.name());
            // The picture still opens.
            let back = image::open(&path).unwrap();
            assert_eq!((back.width(), back.height()), source);
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// lcms writes the second of its making into a profile's header,
    /// so a profile built per export differed from one built a moment
    /// later whenever the clock ticked between them; the metadata test
    /// below failed that way on a slow runner, comparing the file's
    /// profile with a fresh one.
    #[test]
    fn a_spaces_profile_is_the_same_bytes_every_time() {
        for space in Space::ALL {
            let first = space.icc().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
            assert_eq!(first, space.icc().unwrap(), "{}", space.name());
        }
    }

    #[test]
    fn the_metadata_policy_leaves_out_what_it_says() {
        let (w, h) = (24usize, 16usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: vec![0.2; w * h * 3],
        };
        let metadata = RawMetadata {
            exif: Default::default(),
            model: "EOS R6m2".into(),
            make: "Canon".into(),
            lens: None,
            unique_image_id: None,
            rating: None,
        };
        let edit = greycard_edit::Edit::default();
        let dir = scratch("metadata");
        let carries = |bytes: &[u8], what: &[u8]| bytes.windows(what.len()).any(|w| w == what);
        let origin = Origin {
            source_name: Some("IMG_0001.CR3".into()),
            edit: Some(edit.to_json()),
        };
        for (policy, format) in Metadata::ALL
            .into_iter()
            .flat_map(|p| Format::ALL.map(|f| (p, f)))
        {
            let settings = Settings {
                metadata: policy,
                format,
                ..Settings::default()
            };
            let rendered = render(
                &image,
                &edit,
                (w as u32, h as u32),
                &settings,
                &Default::default(),
                1.0,
                None,
                finish::Source::Scene,
                None,
                None,
            );
            let path = dir.join(format!("m-{}.{}", policy.name(), format.extension()));
            write(&rendered, &settings, &path, Some(&metadata), &origin).unwrap();
            // Whatever the container, the packets lie in the file
            // uncompressed, so the bytes say what is there.
            let bytes = std::fs::read(&path).unwrap();
            let camera = carries(&bytes, b"EOS R6m2");
            let source = carries(&bytes, b"<greycard:Source>");
            let recipe = carries(&bytes, b"<greycard:Edit>");
            let what = format!("{} {}", policy.name(), format.name());
            match policy {
                Metadata::All => assert!(camera && source && recipe, "{what}"),
                Metadata::NoEdit => assert!(camera && source && !recipe, "{what}"),
                Metadata::None => assert!(!camera && !source && !recipe, "{what}"),
            }
            // The profile is the embed toggle's, whatever the policy.
            // A PNG's is compressed, so the decoder reads it; the image
            // crate reads none out of a TIFF, whose bytes hold it whole.
            let icc = settings.space.icc().unwrap();
            let profile = match format {
                Format::Tiff => carries(&bytes, &icc),
                _ => {
                    let mut decoder = image::ImageReader::open(&path)
                        .unwrap()
                        .into_decoder()
                        .unwrap();
                    image::ImageDecoder::icc_profile(&mut decoder).unwrap() == Some(icc)
                }
            };
            assert!(profile, "{what}");
        }
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_watermark_moves_its_box_and_nothing_else() {
        let (w, h) = (900usize, 600usize);
        // A gradient, so "unchanged" is a real comparison.
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[x as f32 / w as f32, y as f32 / h as f32, 0.2]);
            }
        }
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let dir = scratch("watermark-export");
        let png = crate::watermark::tests::bar_png(&dir, 30);
        let edit = greycard_edit::Edit::default();
        let plain = Settings {
            format: Format::Png,
            long_edge: Some(600),
            ..Settings::default()
        };
        let render_with = |settings: &Settings| {
            let mut r = render(
                &image,
                &edit,
                (w as u32, h as u32),
                settings,
                &Default::default(),
                1.0,
                None,
                finish::Source::Scene,
                None,
                None,
            );
            mark(&mut r, settings).unwrap();
            let Pixels::Eight(p) = r.pixels else {
                panic!("a PNG is eight bits")
            };
            (r.width, r.height, p)
        };
        let (pw, ph, base) = render_with(&plain);
        assert_eq!((pw, ph), (600, 400));
        use crate::watermark::{Kind, Mark, Position};
        let marks = [
            Mark {
                kind: Kind::Image { path: png },
                position: Position::TopLeft,
                size: 0.2,
                margin: 0.05,
                opacity: 0.5,
            },
            Mark {
                kind: Kind::Text {
                    text: "greycard".into(),
                    white: true,
                },
                position: Position::BottomRight,
                size: 0.3,
                margin: 0.05,
                opacity: 0.5,
            },
        ];
        // The boxes: 120 x 60 at 30, 30; and 180 wide, its right and
        // bottom 30 in.
        let boxes = [
            (30u32, 30u32, 150u32, 90u32),
            (600 - 30 - 181, 250, 571, 371),
        ];
        for (m, b) in marks.into_iter().zip(boxes) {
            let settings = Settings {
                watermark: Some(m),
                ..plain.clone()
            };
            let (_, _, marked) = render_with(&settings);
            let mut moved = 0;
            for y in 0..ph {
                for x in 0..pw {
                    let i = ((y * pw + x) * 3) as usize;
                    let inside = x >= b.0 && x < b.2 && y >= b.1 && y < b.3;
                    if marked[i..i + 3] != base[i..i + 3] {
                        assert!(inside, "moved at {x},{y} outside {b:?}");
                        moved += 1;
                    }
                }
            }
            assert!(moved > 500, "{moved}");
        }
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_border_goes_round_the_resized_picture_and_the_mark_over_both() {
        use crate::border::{Border, Color};
        let (w, h) = (900usize, 600usize);
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[x as f32 / w as f32, y as f32 / h as f32, 0.2]);
            }
        }
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let edit = greycard_edit::Edit::default();
        let finished = |settings: &Settings| {
            let mut r = render(
                &image,
                &edit,
                (w as u32, h as u32),
                settings,
                &Default::default(),
                1.0,
                None,
                finish::Source::Scene,
                None,
                None,
            );
            border(&mut r, settings);
            mark(&mut r, settings).unwrap();
            r
        };
        let plain = Settings {
            format: Format::Tiff,
            long_edge: Some(600),
            ..Settings::default()
        };
        let framed = Settings {
            border: Some(Border {
                width: 0.05,
                color: Color::LightGrey,
            }),
            ..plain.clone()
        };
        let a = finished(&plain);
        let b = finished(&framed);
        // 5 % of the resized long edge, 600: 30 px a side.
        assert_eq!((a.width, a.height), (600, 400));
        assert_eq!((b.width, b.height), (660, 460));
        let (Pixels::Sixteen(pa), Pixels::Sixteen(pb)) = (&a.pixels, &b.pixels) else {
            panic!("a TIFF is sixteen bits")
        };
        for y in 0..460u32 {
            for x in 0..660u32 {
                let i = ((y * 660 + x) * 3) as usize;
                if (30..630).contains(&x) && (30..430).contains(&y) {
                    let j = (((y - 30) * 600 + x - 30) * 3) as usize;
                    assert_eq!(pb[i..i + 3], pa[j..j + 3], "({x}, {y})");
                } else {
                    assert_eq!(pb[i..i + 3], [52428; 3], "({x}, {y})");
                }
            }
        }
        assert!(framed.describe().contains("a light grey border 5 %"));

        // A mark at the bottom right sits its margin in from the file's
        // edges, border and all: a share of the file's long edge.
        let dir = scratch("border-mark");
        let marked = Settings {
            watermark: Some(crate::watermark::Mark {
                kind: crate::watermark::Kind::Image {
                    path: crate::watermark::tests::bar_png(&dir, 30),
                },
                position: crate::watermark::Position::BottomRight,
                size: 0.1,
                margin: 0.01,
                opacity: 1.0,
            }),
            ..framed.clone()
        };
        let m = finished(&marked);
        assert_eq!((m.width, m.height), (660, 460));
        let Pixels::Sixteen(pm) = &m.pixels else {
            panic!("a TIFF is sixteen bits")
        };
        // The mark is 66 by 33 with its right and bottom 7 px in, so
        // the middle of its bar lies in the band: white over grey.
        let i = (((460 - 7 - 16) * 660 + 660 - 7 - 33) * 3) as usize;
        assert_eq!(pm[i..i + 3], [65535; 3]);
        crate::testing::remove_dir_retry(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn percent_decoding() {
        assert_eq!(
            percent_decode("/a%20b/c%2Fd"),
            std::ffi::OsString::from("/a b/c/d")
        );
        assert_eq!(percent_decode("plain"), std::ffi::OsString::from("plain"));
        assert_eq!(percent_decode("bad%"), std::ffi::OsString::from("bad%"));
    }

    use greycard_core::output::Resolved;

    /// A directory of this test's own, emptied first.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn the_policy_reads_back_by_name() {
        for p in OnExists::ALL {
            assert_eq!(OnExists::from_name(p.name()), Some(p));
        }
        // The command line says them in lower case.
        assert_eq!(OnExists::from_name("increment"), Some(OnExists::Increment));
        assert_eq!(OnExists::from_name("SKIP"), Some(OnExists::Skip));
        assert_eq!(
            OnExists::from_name(" overwrite "),
            Some(OnExists::Overwrite)
        );
        assert_eq!(OnExists::from_name("rename"), None);
        assert_eq!(OnExists::default(), OnExists::Increment);
    }

    #[test]
    fn a_free_name_is_the_first_one_free() {
        let dir = scratch("increment");
        let asked = dir.join("frame.jpg");
        // Nothing there: the name as asked, whatever the policy.
        for p in OnExists::ALL {
            assert_eq!(p.resolve(&asked), Resolved::Free(asked.clone()));
        }
        touch(&asked);
        assert_eq!(
            OnExists::Increment.resolve(&asked),
            Resolved::Renamed {
                path: dir.join("frame-1.jpg"),
                asked: asked.clone(),
            }
        );
        // With a -1 there already, the next free one.
        touch(&dir.join("frame-1.jpg"));
        touch(&dir.join("frame-2.jpg"));
        assert_eq!(
            OnExists::Increment.resolve(&asked).path(),
            Some(dir.join("frame-3.jpg").as_path())
        );
        // A name that ends in a dash and a number is never a counter:
        // a camera's `DSC-0042` clashes into `DSC-0042-1`, not the next
        // frame's `DSC-0043`, and `frame-1` into `frame-1-1`.
        let cam = dir.join("DSC-0042.jpg");
        touch(&cam);
        assert_eq!(
            OnExists::Increment.resolve(&cam).path(),
            Some(dir.join("DSC-0042-1.jpg").as_path())
        );
        assert_eq!(
            OnExists::Increment.resolve(&dir.join("frame-1.jpg")).path(),
            Some(dir.join("frame-1-1.jpg").as_path())
        );
        // An old ` (2)` is an ordinary name too.
        touch(&dir.join("old (2).jpg"));
        assert_eq!(
            OnExists::Increment.resolve(&dir.join("old (2).jpg")).path(),
            Some(dir.join("old (2)-1.jpg").as_path())
        );
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_numbered_name_still_fits_the_file_system() {
        let dir = scratch("increment-long");
        // A stem at the limit, with a multibyte character at the cut.
        let stem = format!("{}\u{e9}", "a".repeat(250));
        let asked = dir.join(format!("{stem}.jpg"));
        let free = greycard_core::output::numbered(&asked, 1);
        let name = free.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.len() <= 255, "{}", name.len());
        assert!(name.ends_with("-1.jpg"), "{name}");
        // It writes, and a second clash goes on to `-2` at the same length.
        touch(&free);
        assert!(free.exists());
        let again = OnExists::Increment.resolve(&free);
        assert!(again.path().is_some());
        let long = dir.join(format!("{}.jpg", "b".repeat(251)));
        touch(&long);
        let first = OnExists::Increment
            .resolve(&long)
            .path()
            .unwrap()
            .to_path_buf();
        assert!(first.file_name().unwrap().len() <= 255);
        touch(&first);
        let second = OnExists::Increment
            .resolve(&long)
            .path()
            .unwrap()
            .to_path_buf();
        assert!(second.file_name().unwrap().len() <= 255);
        assert_ne!(first, second);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_dotted_stem_and_a_name_with_no_extension_keep_their_shape() {
        let dir = scratch("increment-stems");
        // The fallback export's own name has two dots in it.
        let dotted = dir.join("IMG_1234.greycard.jpg");
        touch(&dotted);
        assert_eq!(
            OnExists::Increment.resolve(&dotted).path(),
            Some(dir.join("IMG_1234.greycard-1.jpg").as_path())
        );
        let bare = dir.join("export");
        touch(&bare);
        assert_eq!(
            OnExists::Increment.resolve(&bare).path(),
            Some(dir.join("export-1").as_path())
        );
        // A dotfile's name is its stem, not its extension.
        let dotfile = dir.join(".keep");
        touch(&dotfile);
        assert_eq!(
            OnExists::Increment.resolve(&dotfile).path(),
            Some(dir.join(".keep-1").as_path())
        );
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn skip_writes_nothing_and_says_so() {
        let dir = scratch("skip");
        let asked = dir.join("frame.tif");
        touch(&asked);
        let resolved = OnExists::Skip.resolve(&asked);
        assert_eq!(resolved, Resolved::Skipped(asked.clone()));
        assert_eq!(resolved.path(), None);
        assert_eq!(resolved.asked(), asked.as_path());
        let note = resolved.note().expect("a skip says so");
        assert!(
            note.contains("skipped") && note.contains("frame.tif"),
            "{note}"
        );
        // Overwrite writes to the path asked for, and says that too.
        let over = OnExists::Overwrite.resolve(&asked);
        assert_eq!(over.path(), Some(asked.as_path()));
        assert!(over.note().is_some());
        // Nothing in the way is nothing to report.
        assert!(
            OnExists::Skip
                .resolve(&dir.join("new.tif"))
                .note()
                .is_none()
        );
        crate::testing::remove_dir_retry(&dir);
    }
}
