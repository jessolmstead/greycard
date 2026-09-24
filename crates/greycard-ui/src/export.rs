//! Export: the finished picture in a chosen format, size and color
//! space, with the profile that says which, and the desktop's file
//! chooser for where it goes.

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

    fn globs(self) -> &'static [&'static str] {
        match self {
            Format::Jpeg => &["*.jpg", "*.jpeg"],
            Format::Png => &["*.png"],
            Format::Tiff => &["*.tif", "*.tiff"],
        }
    }
}

/// The output's primaries. The transfer function is sRGB's for all
/// three, and the embedded profile says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Srgb,
    DisplayP3,
    Rec2020,
}

impl Space {
    pub const ALL: [Space; 3] = [Space::Srgb, Space::DisplayP3, Space::Rec2020];

    pub fn name(self) -> &'static str {
        match self {
            Space::Srgb => "sRGB",
            Space::DisplayP3 => "Display P3",
            Space::Rec2020 => "Rec.2020",
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

    /// An ICC profile for the space: its primaries and white, the sRGB
    /// curve, and its name.
    pub fn icc(self) -> Result<Vec<u8>> {
        use lcms2::{CIExyY, CIExyYTRIPLE, Locale, MLU, Profile, Tag, TagSignature, ToneCurve};
        let s = self.rgb();
        let point = |x: f64, y: f64| CIExyY { x, y, Y: 1.0 };
        let white = point(s.white.x, s.white.y);
        let primaries = CIExyYTRIPLE {
            Red: point(s.red.x, s.red.y),
            Green: point(s.green.x, s.green.y),
            Blue: point(s.blue.x, s.blue.y),
        };
        // The sRGB transfer function, lcms's parametric type 4.
        let curve =
            ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045])
                .map_err(|e| anyhow::anyhow!("the sRGB curve: {e}"))?;
        let mut profile = Profile::new_rgb(&white, &primaries, &[&curve, &curve, &curve])
            .map_err(|e| anyhow::anyhow!("building the {} profile: {e}", self.name()))?;
        let mut description = MLU::new(1);
        description.set_text_ascii(self.name(), Locale::none());
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
    /// A mark laid over the picture after its resize.
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
) -> Rendered {
    let framed = image.width as f32;
    let fitted;
    let image = match fit(image, settings.long_edge) {
        Some(mut f) => {
            if let Some(options) = settings.sharpen.options() {
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
        .map(|a| finish::Local {
            baked: finish::Baked::of(&a.look),
            mask: a.mask.clone(),
            enabled: a.enabled && !a.mask.is_empty(),
            rasters: finish::rasterize(&a.mask, sh / sw, |i| learned.get(&(a.id, i)).cloned()),
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
    let look = edit.look_lut.look().filter(|l| !l.is_off());
    // The plane is only read where a shift asks for it; the pixel's
    // own luminance is the same answer when none does, and skipping it
    // spares the finish a lookup a pixel. A plane with nothing in it
    // is no plane, as `Renderer::set_guide` also has it, and the same
    // fallback is the right answer for it.
    let guide = guide
        .filter(|g| !g.data.is_empty())
        .filter(|_| global.light.tone.enabled && reads_guide(edit))
        .map(|g| (g, sw));
    let pixels = match settings.format {
        Format::Tiff => Pixels::Sixteen(finish::finish_with(
            image,
            &global,
            &locals,
            position,
            frame_at,
            guide,
            look.as_ref(),
            &m,
            |v| (v * 65535.0).round() as u16,
        )),
        Format::Jpeg | Format::Png => Pixels::Eight(finish::finish_with(
            image,
            &global,
            &locals,
            position,
            frame_at,
            guide,
            look.as_ref(),
            &m,
            |v| (v * 255.0).round() as u8,
        )),
    };
    Rendered {
        width: image.width as u32,
        height: image.height as u32,
        pixels,
    }
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

/// Whether either exposure shift is set, in the picture's own look or
/// in a local adjustment that acts: the only thing that reads the
/// guide plane. Not whites, which is a white point and global.
fn reads_guide(edit: &greycard_edit::Edit) -> bool {
    let set = |t: &greycard_edit::Tone| t.highlights != 0.0 || t.shadows != 0.0;
    set(&edit.look().light.effective().tone)
        || edit
            .adjustments
            .iter()
            .any(|a| a.enabled && !a.mask.is_empty() && set(&a.look.light.effective().tone))
}

/// The image no larger than `long_edge` on its long side, in linear
/// light, Lanczos; none when it already fits.
fn fit(image: &WorkingImage, long_edge: Option<u32>) -> Option<WorkingImage> {
    let long = image.width.max(image.height) as u32;
    let edge = long_edge?;
    if edge >= long {
        return None;
    }
    let scale = edge as f64 / long as f64;
    let w = ((image.width as f64 * scale).round() as u32).max(1);
    let h = ((image.height as f64 * scale).round() as u32).max(1);
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
/// three calls below build one of these and hand it to `ask`.
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
}

/// Ask the desktop where to save. `done` gets the path, none when the
/// user canceled, or the error when there was no chooser to ask.
pub fn choose_path(
    suggested: PathBuf,
    format: Format,
    done: impl FnOnce(Result<Option<PathBuf>>) + Send + 'static,
) {
    ask(
        Ask {
            title: "Export",
            folder: suggested.parent().unwrap_or(Path::new("/")).to_path_buf(),
            name: Some(
                suggested
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            filter: (
                format.name().to_string(),
                format.globs().iter().map(|g| g.to_string()).collect(),
            ),
            directory: false,
        },
        done,
    );
}

/// Ask the desktop for a file to open, the same way: `start` is the
/// folder to open on, `filter` a name and its globs.
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
        },
        done,
    );
}

/// Linux asks the desktop's file chooser portal, on a thread of its
/// own since the dialog takes as long as the user and the call is a
/// blocking one.
#[cfg(target_os = "linux")]
fn ask(request: Ask, done: impl FnOnce(Result<Option<PathBuf>>) + Send + 'static) {
    std::thread::spawn(move || done(portal(request)));
}

/// Everywhere else it is the platform's own dialog, through `rfd`.
/// AppKit and the Windows common dialogs want the thread their window
/// is on, so the dialog is built on the event loop's thread rather
/// than a thread of its own, and awaited there too: `rfd` hands back
/// a future the loop can poll, so the window behind the dialog goes
/// on drawing. `done` then runs on that thread, where the callers'
/// `invoke_from_event_loop` is a queued call like any other.
#[cfg(not(target_os = "linux"))]
fn ask(request: Ask, done: impl FnOnce(Result<Option<PathBuf>>) + Send + 'static) {
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
                let picked = if request.directory {
                    dialog.pick_folder().await
                } else if request.name.is_some() {
                    dialog.save_file().await
                } else {
                    dialog.pick_file().await
                };
                if let Some(done) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    done(Ok(picked.map(|f| f.path().to_path_buf())));
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
/// and `directory` asks for a folder chooser rather than a file one.
#[cfg(target_os = "linux")]
fn portal(request: Ask) -> Result<Option<PathBuf>> {
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
    }

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    let method = if request.name.is_some() {
        "SaveFile"
    } else {
        "OpenFile"
    };
    let conn = Connection::session().context("the session bus")?;
    let sender = conn
        .unique_name()
        .context("no name on the bus")?
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
        current_name: request.name,
        current_folder: folder,
        filters: portal_filters(request.filter),
        directory: request.directory.then_some(true),
    };
    let handle: OwnedObjectPath = chooser
        .call(method, &("", request.title, options))
        .context("the file chooser portal")?;
    if handle.as_str() != request_path {
        // An old portal names the request itself.
        let signals = Proxy::new(&conn, PORTAL, handle, "org.freedesktop.portal.Request")?;
        responses = signals.receive_signal("Response")?;
    }
    let message = responses.next().context("the file chooser went away")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize()?;
    if code != 0 {
        return Ok(None);
    }
    let uris = results.get("uris").context("a reply without a file")?;
    let uris = Vec::<String>::try_from(uris.clone()).context("the reply's file")?;
    let uri = uris.first().context("a reply without a file")?;
    let path = uri
        .strip_prefix("file://")
        .with_context(|| format!("not a local file: {uri}"))?;
    Ok(Some(PathBuf::from(percent_decode(path))))
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
            let enc = lin.map(|v| finish::encode(v.clamp(0.0, 1.0)));
            let mut out = [[0f64; 3]];
            t.transform_pixels(&[enc], &mut out);
            out[0]
        };
        // A color inside all three gamuts.
        let color = [0.4, 0.3, 0.2];
        let reference = to_xyz(Space::Srgb, color);
        for space in [Space::DisplayP3, Space::Rec2020] {
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
                path: dir.join("frame (2).jpg"),
                asked: asked.clone(),
            }
        );
        // With a (2) there already, the next free one.
        touch(&dir.join("frame (2).jpg"));
        touch(&dir.join("frame (3).jpg"));
        assert_eq!(
            OnExists::Increment.resolve(&asked).path(),
            Some(dir.join("frame (4).jpg").as_path())
        );
        // Asked for the numbered name itself: counted from the stem it
        // was made from, not numbered twice.
        assert_eq!(
            OnExists::Increment
                .resolve(&dir.join("frame (2).jpg"))
                .path(),
            Some(dir.join("frame (4).jpg").as_path())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dotted_stem_and_a_name_with_no_extension_keep_their_shape() {
        let dir = scratch("increment-stems");
        // The fallback export's own name has two dots in it.
        let dotted = dir.join("IMG_1234.greycard.jpg");
        touch(&dotted);
        assert_eq!(
            OnExists::Increment.resolve(&dotted).path(),
            Some(dir.join("IMG_1234.greycard (2).jpg").as_path())
        );
        let bare = dir.join("export");
        touch(&bare);
        assert_eq!(
            OnExists::Increment.resolve(&bare).path(),
            Some(dir.join("export (2)").as_path())
        );
        // A dotfile's name is its stem, not its extension.
        let dotfile = dir.join(".keep");
        touch(&dotfile);
        assert_eq!(
            OnExists::Increment.resolve(&dotfile).path(),
            Some(dir.join(".keep (2)").as_path())
        );
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
