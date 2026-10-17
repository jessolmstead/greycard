//! `--measure-lightroom CATALOG`: how close the Lightroom import's
//! mapping lands on Lightroom's own exports, frame by frame, with no
//! window and nothing written but the report.
//!
//! The camera match pointed at Lightroom instead of at the camera's
//! JPEG. Each catalog image is paired with its export
//! ([`greycard_library::lightroom_exports`]: the export history says
//! which settings a file was rendered from, else the name does and the
//! current settings are assumed). Its raw is developed under those
//! settings mapped as the import maps them
//! ([`greycard_library::lightroom_catalog::imported_edit`], in memory,
//! no sidecar), through the camera's fitted look when the catalog's
//! profile is the camera's own and the look store has that body's
//! table for it, and finished as the camera match finishes a frame
//! ([`crate::worker::FrameDevelop`], [`cm::settings`]). Lightroom's
//! crop is cut from the render, since an export is cropped where our
//! import does not carry the crop yet. Then the export is registered
//! onto the cut and both are averaged over the match's blocks
//! ([`greycard_match::register`], [`greycard_match::pairs`]), which is
//! the blur that takes the export's resize and output sharpening out
//! of the comparison. What a frame gives: the mean ΔE in Oklab, split
//! into lightness and chroma; the exposure offset in stops before any
//! correction, so a frame's brightness error reads apart from its
//! color; and the ΔE again once the brightness is matched through the
//! Exposure slider ([`cm::iterate`]).
//!
//! The report groups the frames by profile, and by slider: over the
//! frames where a slider is off its default against those where it is
//! not, a slider whose frames carry more error is the one to look at.
//! Frames with masks or local adjustments, a lens blur, or an edit on
//! the 2003 or 2010 process are measured and listed apart, and left out
//! of the slider groups' first table, since the mapper has nothing for
//! them. Neither the report nor its JSON names a folder: a frame is its
//! file name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use greycard_core::image::WorkingImage;
use greycard_core::raw::Orientation;
use greycard_edit::Edit;
use greycard_edit::look::{LookLut, LutChoice};
use greycard_library::lightroom_exports::{self as lx, Confidence, Crop, Pairing};
use greycard_match::color::{decode3, oklab};
use greycard_match::register::{NCC_MIN, register};
use greycard_match::{Pairs, Picture};

use crate::camera_match as cm;
use crate::match_compare::Acc;

/// The sliders the report groups by, as Lightroom names them: each is
/// off its default when not zero.
const SLIDERS: [(&str, &str); 11] = [
    ("exposure", "Exposure2012"),
    ("contrast", "Contrast2012"),
    ("highlights", "Highlights2012"),
    ("shadows", "Shadows2012"),
    ("whites", "Whites2012"),
    ("blacks", "Blacks2012"),
    ("vibrance", "Vibrance"),
    ("saturation", "Saturation"),
    ("clarity", "Clarity2012"),
    ("texture", "Texture"),
    ("dehaze", "Dehaze"),
];

/// Every group of the slider tables, in the order they print: the
/// eleven above, the white balance's two and the tone curve.
const GROUPS: [&str; 14] = [
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "temperature",
    "tint",
    "vibrance",
    "saturation",
    "tone curve",
    "clarity",
    "texture",
    "dehaze",
];

/// The process version from which the sliders are the current ones,
/// as the import has it.
const PROCESS_2012: f32 = 6.7;

/// A cut's shape and the export's may differ by this share and still be
/// one turn of the other.
const SHAPE: f64 = 0.03;

/// What the command line asked for.
pub(crate) struct Options {
    pub(crate) catalog: PathBuf,
    pub(crate) roots: Vec<(String, PathBuf)>,
    pub(crate) folders: Vec<PathBuf>,
    pub(crate) limit: Option<usize>,
    pub(crate) out: Option<PathBuf>,
}

/// `--measure-lightroom`, from the command line to the exit code.
pub(crate) fn headless(cli: &crate::Cli, catalog: &Path) -> Result<std::process::ExitCode> {
    let opts = Options {
        catalog: catalog.to_path_buf(),
        roots: cli.root.clone(),
        folders: cli.exports.clone(),
        limit: cli.limit,
        out: cli.out.clone(),
    };
    let report = run(&opts)?;
    print!("{}", text(&report));
    if let Some(out) = &opts.out {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(out, json).with_context(|| format!("writing {}", out.display()))?;
        eprintln!("measure lightroom: {}", out.display());
    }
    Ok(std::process::ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------
// What a frame is.

/// How the catalog's profile stands against the looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Kind {
    /// A camera-matching profile with the body's fitted look for it.
    CameraLook,
    /// A camera-matching profile, no fitted look for it on this body.
    CameraNoLook,
    /// One of Adobe's own.
    Adobe,
    /// Anything else: a creative profile, someone else's.
    Other,
}

impl Kind {
    fn words(self) -> &'static str {
        match self {
            Kind::CameraLook => "camera, look",
            Kind::CameraNoLook => "camera, no look",
            Kind::Adobe => "Adobe",
            Kind::Other => "other",
        }
    }
}

/// One frame's numbers against its export.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Measured {
    /// Mean ΔE in Oklab over the kept blocks.
    pub(crate) de: f32,
    /// Lightness: ours minus Lightroom's, and its size.
    pub(crate) dl: f32,
    pub(crate) dl_abs: f32,
    /// Chroma: ours minus Lightroom's, and its size; the hue's part.
    pub(crate) dc: f32,
    pub(crate) dc_abs: f32,
    pub(crate) dh: f32,
    pub(crate) blocks: usize,
    /// Lightroom's brightness over ours in stops, before any
    /// correction: positive where Lightroom is brighter.
    pub(crate) offset: f32,
    /// The same frame with its brightness matched through the Exposure
    /// slider: where the match ended, the ΔE left and its parts.
    pub(crate) matched_offset: f32,
    pub(crate) matched_de: f32,
    pub(crate) matched_dl: f32,
    pub(crate) matched_dc: f32,
    /// The registration's correlation and scale.
    pub(crate) ncc: f32,
    pub(crate) scale: f32,
    pub(crate) seconds: f32,
}

/// One frame of the report.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct FrameLine {
    /// The raw's file name, and a virtual copy's name after it.
    pub(crate) name: String,
    /// The image's id in the catalog.
    pub(crate) image: i64,
    pub(crate) export: String,
    pub(crate) confidence: String,
    /// The export step whose settings were used, by its name.
    pub(crate) step: Option<String>,
    /// Whether Lightroom counts the image as developed.
    pub(crate) developed: bool,
    /// The temperature and the tint the image arrived with, its as-shot
    /// ones, which `sliders` holds them against.
    pub(crate) arrived: BTreeMap<String, f32>,
    pub(crate) camera: String,
    pub(crate) profile: String,
    pub(crate) kind: Option<Kind>,
    pub(crate) look: Option<String>,
    /// The sliders off their default, Lightroom's values.
    pub(crate) sliders: BTreeMap<String, f32>,
    /// Masks, a lens blur, healed spots, Enhance.
    pub(crate) local: Vec<String>,
    pub(crate) cropped: bool,
    /// The crop's angle, degrees, where it is turned.
    pub(crate) angle: Option<f32>,
    pub(crate) mono: bool,
    pub(crate) process: Option<String>,
    pub(crate) old_process: bool,
    /// What the mapper had no place for.
    pub(crate) unmapped: Vec<String>,
    pub(crate) measured: Option<Measured>,
    pub(crate) failed: Option<String>,
}

impl FrameLine {
    /// Whether the edit is one the mapper can carry: no masks or local
    /// adjustments, no lens blur, the current process. Healed spots
    /// and Enhance stay global: neither moves a block's mean much.
    pub(crate) fn global(&self) -> bool {
        !self.local.iter().any(|l| l == "masks" || l == "lens blur") && !self.old_process
    }

    fn off(&self, group: &str) -> bool {
        self.sliders.contains_key(group)
    }
}

/// The settings' sliders that are off their default: the eleven by
/// value, the temperature and the tint against what the image arrived
/// with (its as-shot ones, or what an import preset gave it) or, with
/// no first step, a white balance other than as shot, and the tone
/// curve when a point curve bends or a parametric amount is set.
pub(crate) fn sliders(settings: &str, imported: Option<&str>) -> BTreeMap<String, f32> {
    let words = lx::words(settings);
    let num =
        |w: &BTreeMap<String, String>, k: &str| w.get(k).and_then(|v| v.trim().parse::<f32>().ok());
    let mut out = BTreeMap::new();
    for (name, key) in SLIDERS {
        if let Some(v) = num(&words, key).filter(|v| *v != 0.0) {
            out.insert(name.to_string(), v);
        }
    }
    let first = imported.map(lx::words);
    let as_shot = words
        .get("WhiteBalance")
        .is_none_or(|w| w.eq_ignore_ascii_case("As Shot"));
    for (name, key) in [("temperature", "Temperature"), ("tint", "Tint")] {
        let Some(v) = num(&words, key) else {
            continue;
        };
        let moved = match first.as_ref().and_then(|f| num(f, key)) {
            Some(was) => (v - was).abs() > 0.5,
            None => !as_shot,
        };
        if moved {
            out.insert(name.to_string(), v);
        }
    }
    if let Some(bend) = curve_bend(settings) {
        out.insert("tone curve".to_string(), bend);
    }
    out
}

/// How far the settings' tone curves bend, on 0..255: the largest
/// distance of a point curve's point from the diagonal, or of a
/// parametric amount from zero on its own scale. None when straight.
fn curve_bend(settings: &str) -> Option<f32> {
    use greycard_edit::lightroom::{Value, parse_table};
    let top = parse_table(settings).ok()?;
    let mut bend = 0.0f32;
    for (key, value) in &top {
        let Some(key) = key else { continue };
        match value {
            Value::Table(items) if key.starts_with("ToneCurvePV2012") => {
                let n: Vec<f32> = items
                    .iter()
                    .filter_map(|(_, v)| match v {
                        Value::Word(w) => w.parse().ok(),
                        Value::Table(_) => None,
                    })
                    .collect();
                for [x, y] in n.as_chunks::<2>().0 {
                    bend = bend.max((x - y).abs());
                }
            }
            Value::Word(w)
                if [
                    "ParametricHighlights",
                    "ParametricLights",
                    "ParametricDarks",
                    "ParametricShadows",
                ]
                .contains(&key.as_str()) =>
            {
                bend = bend.max(w.parse::<f32>().unwrap_or(0.0).abs());
            }
            _ => {}
        }
    }
    (bend > 0.0).then_some(bend)
}

/// The profile the settings name, and its kind before the looks are
/// asked: camera-matching profiles are Lightroom's "Camera <style>".
fn profile_of(words: &BTreeMap<String, String>) -> String {
    words
        .get("CameraProfile")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "(none named)".into())
}

/// A style's name as the looks and Lightroom's profiles can both be
/// compared on: Lightroom's "PROVIA/Standard" is the style reader's
/// "Provia", its "ACROS+G Filter" the reader's "Acros + G Filter".
pub(crate) fn style_key(style: &str) -> String {
    style
        .split('/')
        .next()
        .unwrap_or(style)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The look in `store` the camera match fitted for `camera` (as the
/// panel names a body) and the style a camera-matching `profile` names,
/// by its file name: the body's name, then the style. The table the
/// picture's curve would read must be there.
pub(crate) fn look_for(
    store: &Path,
    camera: &str,
    profile: &str,
    curve: greycard_edit::DisplayCurve,
) -> Option<String> {
    let style = profile.strip_prefix("Camera ")?;
    let want = style_key(style);
    let prefix = format!("{camera} ");
    let mut names: Vec<String> = std::fs::read_dir(store)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".cube").map(str::to_string)
        })
        .collect();
    names.sort();
    names.into_iter().find(|name| {
        name.strip_prefix(&prefix).is_some_and(|rest| {
            !rest.contains('.')
                && style_key(rest) == want
                && LookLut {
                    lut: LutChoice::Named(name.clone()),
                    strength: 1.0,
                }
                .look_under_in(store, curve)
                .is_some()
        })
    })
}

// ---------------------------------------------------------------------
// The develop and the comparison.

/// The inverse of an orientation: a quarter turn one way for the other,
/// the rest their own.
fn undone(o: Orientation) -> Orientation {
    match o {
        Orientation::Rotate90 => Orientation::Rotate270,
        Orientation::Rotate270 => Orientation::Rotate90,
        other => other,
    }
}

fn to_working(p: &Picture) -> WorkingImage {
    WorkingImage {
        width: p.width,
        height: p.height,
        data: p.data.iter().flatten().copied().collect(),
    }
}

fn to_picture(w: WorkingImage) -> Picture {
    let data = w.data.as_chunks::<3>().0.to_vec();
    Picture::new(w.width, w.height, data)
}

/// Lightroom's crop cut from a render turned by `orientation`: the
/// render taken back to the file's own way up, where the crop's corners
/// are, and the turned rectangle sampled out of it. `size` is the file's
/// size as the catalog has it, in which the crop's angle is measured;
/// the render's own shape without it. The cut comes back the file's way
/// up, for [`turns`] to turn.
pub(crate) fn cut(
    render: &Picture,
    orientation: Orientation,
    crop: Option<&Crop>,
    size: Option<(f64, f64)>,
) -> Picture {
    let upright = to_picture(greycard_core::develop::orient(
        to_working(render),
        undone(orientation),
    ));
    let Some(crop) = crop else {
        return upright;
    };
    let (pw, ph) = (upright.width as f64, upright.height as f64);
    let (w, h) = size.unwrap_or((pw, ph));
    let (cw, ch) = crop.size(w, h);
    // Render pixels a catalog pixel, each way.
    let (kx, ky) = (pw / w, ph / h);
    let out_w = ((cw * kx).round() as usize).max(1);
    let out_h = ((ch * ky).round() as usize).max(1);
    let (s, c) = crop.radians().sin_cos();
    let (x0, y0) = (crop.left * w, crop.top * h);
    let mut data = Vec::with_capacity(out_w * out_h);
    for t in 0..out_h {
        for u in 0..out_w {
            // The output pixel's center along the turned sides, in the
            // catalog's pixels.
            let (a, b) = ((u as f64 + 0.5) / kx, (t as f64 + 0.5) / ky);
            let x = x0 + a * c - b * s;
            let y = y0 + a * s + b * c;
            data.push(bilinear(&upright, x * kx - 0.5, y * ky - 0.5));
        }
    }
    Picture::new(out_w, out_h, data)
}

/// The picture at a point, bilinear, the edge repeated past it.
fn bilinear(p: &Picture, x: f64, y: f64) -> [f32; 3] {
    let x = x.clamp(0.0, (p.width - 1) as f64);
    let y = y.clamp(0.0, (p.height - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(p.width - 1), (y0 + 1).min(p.height - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let (a, b, c, d) = (p.at(y0, x0), p.at(y0, x1), p.at(y1, x0), p.at(y1, x1));
    [0, 1, 2].map(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// The ways up to try a cut in against an export of `export`'s shape:
/// the file's own first, then a half turn of it, then the quarter
/// turns, each only when it makes the export's shape. Lightroom keeps
/// an orientation of its own, which a turn in Lightroom moves.
pub(crate) fn turns(cut: &Picture, own: Orientation, export: (usize, usize)) -> Vec<Orientation> {
    let ratio = |w: usize, h: usize| w as f64 / h.max(1) as f64;
    let want = ratio(export.0, export.1);
    [0, 2, 1, 3]
        .into_iter()
        .map(|q| own.turned(q))
        .filter(|o| {
            let swaps = matches!(
                o,
                Orientation::Rotate90
                    | Orientation::Rotate270
                    | Orientation::Transpose
                    | Orientation::Transverse
            );
            let (w, h) = if swaps {
                (cut.height, cut.width)
            } else {
                (cut.width, cut.height)
            };
            (ratio(w, h) - want).abs() <= SHAPE * want
        })
        .collect()
}

/// An export laid over a cut: the way up, the registration, the export
/// on the cut's grid and which of its pixels it reaches.
struct Laid {
    turn: Orientation,
    reg: greycard_match::Registration,
    laid: Picture,
    coverage: greycard_match::Plane,
}

/// Lay `export` over `cut`, trying each way up [`turns`] allows; an
/// error when none lays at the match's correlation.
fn lay(cut: &Picture, own: Orientation, export: &Picture) -> Result<Laid, String> {
    let ways = turns(cut, own, (export.width, export.height));
    if ways.is_empty() {
        return Err(format!(
            "the export's shape ({} by {}) is no turn of the crop's ({} by {})",
            export.width, export.height, cut.width, cut.height
        ));
    }
    let mut best: Option<(Orientation, greycard_match::Registration, Picture)> = None;
    for turn in ways {
        let turned = to_picture(greycard_core::develop::orient(to_working(cut), turn));
        let reg = register(&turned, export);
        let better = best.as_ref().is_none_or(|(_, b, _)| reg.ncc > b.ncc);
        if better {
            best = Some((turn, reg, turned));
        }
        if reg.ncc >= NCC_MIN {
            break;
        }
    }
    let (turn, reg, turned) = best.expect("one way up at least");
    if reg.ncc < NCC_MIN {
        return Err(format!(
            "the export does not lay over the develop (correlation {:.2})",
            reg.ncc
        ));
    }
    let (w, h) = (turned.width, turned.height);
    Ok(Laid {
        turn,
        reg,
        laid: export.warp_to(w, h, reg.scale, reg.dy, reg.dx),
        coverage: export.coverage(w, h, reg.scale, reg.dy, reg.dx),
    })
}

/// The block pairs of a render, cut and turned as `laid` was, against
/// the export it holds.
fn pairs_of(
    render: &Picture,
    own: Orientation,
    crop: Option<&Crop>,
    size: Option<(f64, f64)>,
    laid: &Laid,
) -> Pairs {
    let cut = cut(render, own, crop, size);
    let turned = to_picture(greycard_core::develop::orient(to_working(&cut), laid.turn));
    greycard_match::pairs::pairs(&turned, &laid.laid, &laid.coverage)
}

/// The mean error of a frame's pairs, ours against Lightroom's.
fn summary_of(p: &Pairs) -> crate::match_compare::Summary {
    let mut acc = Acc::default();
    for pair in &p.pairs {
        acc.add(oklab(decode3(pair.render)), oklab(decode3(pair.jpeg)));
    }
    acc.summary()
}

/// The export as the comparison reads it: encoded sRGB in floats. One
/// in another space is refused rather than misread.
pub(crate) fn read_export(path: &Path) -> Result<Picture, String> {
    use image::ImageDecoder;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| e.to_string())?;
    if let Ok(Some(icc)) = decoder.icc_profile()
        && !icc.windows(4).any(|w| w == b"sRGB")
    {
        return Err("the export's profile is not sRGB".into());
    }
    let rgb = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| e.to_string())?
        .to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let data = rgb
        .as_raw()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| c.map(|v| f32::from(v) / 255.0))
        .collect();
    Ok(Picture::new(w, h, data))
}

/// The long edge a frame is finished at so that its crop comes out at
/// about the match's: more where the crop is a small part.
fn long_edge_for(crop: Option<&Crop>, size: Option<(f64, f64)>) -> u32 {
    let share = match (crop, size) {
        (Some(c), Some((w, h))) => {
            let (cw, ch) = c.size(w, h);
            (cw.max(ch) / w.max(h)).clamp(0.25, 1.0)
        }
        _ => 1.0,
    };
    ((f64::from(cm::LONG_EDGE) / share).ceil() as u32).min(2 * cm::LONG_EDGE)
}

/// One raw developed under `edit` and compared with `export`: the error
/// at the edit's own exposure, then with the brightness matched.
pub(crate) fn measure_frame(
    raw: &Path,
    edit: &Edit,
    crop: Option<&Crop>,
    size: Option<(f64, f64)>,
    export: &Picture,
    lenses: Option<&greycard_lens::Database>,
) -> Result<Measured, String> {
    let start = Instant::now();
    let own = greycard_core::decode::stance_path(raw)
        .map(|s| s.orientation)
        .unwrap_or_default();
    let settings = crate::export::Settings {
        long_edge: Some(long_edge_for(crop, size)),
        ..cm::settings()
    };
    let mut developed = crate::worker::FrameDevelop::develop(raw, edit, lenses)?;
    let render = cm::picture_of(&developed.finish(edit, &settings));
    let laid = lay(&cut(&render, own, crop, size), own, export)?;
    let first = pairs_of(&render, own, crop, size, &laid);
    let offset =
        greycard_match::exposure::offset_stops(&first).ok_or("no flat, unclipped blocks")?;
    let at = |developed: &mut crate::worker::FrameDevelop, o: f32| {
        let mut moved = edit.clone();
        moved.light.exposure += o;
        let render = cm::picture_of(&developed.finish(&moved, &settings));
        pairs_of(&render, own, crop, size, &laid)
    };
    let once = at(&mut developed, offset);
    let residual =
        greycard_match::exposure::offset_stops(&once).ok_or("no flat, unclipped blocks")?;
    let (ended, matched) = cm::iterate(offset, residual, once, &mut |o| {
        let p = at(&mut developed, o);
        greycard_match::exposure::offset_stops(&p).map(|r| (r, p))
    });
    let s = summary_of(&first);
    let m = summary_of(&matched);
    Ok(Measured {
        de: s.de,
        dl: s.dl,
        dl_abs: s.dl_abs,
        dc: s.dc,
        dc_abs: s.dc_abs,
        dh: s.dh,
        blocks: s.blocks,
        offset,
        matched_offset: ended.offset,
        matched_de: m.de,
        matched_dl: m.dl,
        matched_dc: m.dc,
        ncc: laid.reg.ncc,
        scale: laid.reg.scale,
        seconds: start.elapsed().as_secs_f32(),
    })
}

// ---------------------------------------------------------------------
// The report.

/// Means over a set of measured frames.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Stats {
    pub(crate) frames: usize,
    pub(crate) de: f32,
    pub(crate) de_median: f32,
    pub(crate) dl: f32,
    pub(crate) dl_abs: f32,
    pub(crate) dc: f32,
    pub(crate) dc_abs: f32,
    pub(crate) offset: f32,
    pub(crate) offset_abs: f32,
    pub(crate) matched_de: f32,
}

/// The means over the measured frames among `frames`; none when none
/// was measured.
pub(crate) fn stats<'a>(frames: impl Iterator<Item = &'a FrameLine>) -> Option<Stats> {
    let m: Vec<&Measured> = frames.filter_map(|f| f.measured.as_ref()).collect();
    if m.is_empty() {
        return None;
    }
    let n = m.len() as f32;
    let mean = |f: &dyn Fn(&Measured) -> f32| m.iter().map(|x| f(x)).sum::<f32>() / n;
    let mut de: Vec<f32> = m.iter().map(|x| x.de).collect();
    de.sort_by(f32::total_cmp);
    let median = if de.len() % 2 == 1 {
        de[de.len() / 2]
    } else {
        (de[de.len() / 2 - 1] + de[de.len() / 2]) / 2.0
    };
    Some(Stats {
        frames: m.len(),
        de: mean(&|x| x.de),
        de_median: median,
        dl: mean(&|x| x.dl),
        dl_abs: mean(&|x| x.dl_abs),
        dc: mean(&|x| x.dc),
        dc_abs: mean(&|x| x.dc_abs),
        offset: mean(&|x| x.offset),
        offset_abs: mean(&|x| x.offset.abs()),
        matched_de: mean(&|x| x.matched_de),
    })
}

/// One profile's frames, global and with local work apart.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ProfileLine {
    pub(crate) profile: String,
    pub(crate) kind: Kind,
    pub(crate) global: Option<Stats>,
    pub(crate) local: Option<Stats>,
}

/// One slider: the frames where it is off its default, against those
/// where it is not.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SliderLine {
    pub(crate) slider: String,
    pub(crate) off: Option<Stats>,
    pub(crate) default: Option<Stats>,
}

/// The frames grouped by profile and kind.
pub(crate) fn by_profile(frames: &[FrameLine]) -> Vec<ProfileLine> {
    let mut keys: Vec<(Kind, String)> = frames
        .iter()
        .filter_map(|f| Some((f.kind?, f.profile.clone())))
        .collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|(kind, profile)| {
            let of = || {
                frames
                    .iter()
                    .filter(|f| f.kind == Some(kind) && f.profile == profile)
            };
            ProfileLine {
                profile: profile.clone(),
                kind,
                global: stats(of().filter(|f| f.global())),
                local: stats(of().filter(|f| !f.global())),
            }
        })
        .collect()
}

/// The frames grouped by each slider, over `frames` as given.
pub(crate) fn by_slider(frames: &[&FrameLine]) -> Vec<SliderLine> {
    GROUPS
        .iter()
        .map(|&g| SliderLine {
            slider: g.to_string(),
            off: stats(frames.iter().copied().filter(|f| f.off(g))),
            default: stats(frames.iter().copied().filter(|f| !f.off(g))),
        })
        .collect()
}

/// What pairing found, in counts.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct PairingLine {
    pub(crate) images: usize,
    pub(crate) copies: usize,
    /// Images their history says were exported.
    pub(crate) exported: usize,
    pub(crate) folders: usize,
    pub(crate) files: usize,
    pub(crate) pairs: usize,
    pub(crate) by_history: usize,
    pub(crate) by_name: usize,
    /// By name, of images exported at other times than the file's.
    pub(crate) by_name_elsewhere: usize,
    pub(crate) not_found: usize,
    pub(crate) misshapen: usize,
    /// Images never developed in the catalog with files of their name.
    pub(crate) undeveloped: usize,
    pub(crate) ambiguous: usize,
    pub(crate) unclaimed: usize,
    pub(crate) never_exported: usize,
    /// Pairs whose raw is not here.
    pub(crate) no_raw: usize,
    /// Each image left unpaired, with why and its files' names.
    pub(crate) unpaired: Vec<String>,
}

impl PairingLine {
    fn of(p: &Pairing) -> PairingLine {
        PairingLine {
            images: p.shots.len(),
            copies: p.shots.iter().filter(|s| s.copy.is_some()).count(),
            exported: p.shots.iter().filter(|s| !s.exports.is_empty()).count(),
            folders: p.folders.len(),
            files: p.folders.iter().map(|(_, n)| n).sum(),
            pairs: p.pairs.len(),
            by_history: p
                .pairs
                .iter()
                .filter(|x| x.confidence == Confidence::History)
                .count(),
            by_name: p
                .pairs
                .iter()
                .filter(|x| x.confidence == Confidence::Name)
                .count(),
            by_name_elsewhere: p
                .pairs
                .iter()
                .filter(|x| x.confidence == Confidence::Elsewhere)
                .count(),
            not_found: p.not_found.len(),
            misshapen: p.misshapen.len(),
            undeveloped: p.undeveloped.len(),
            ambiguous: p.ambiguous.len(),
            unclaimed: p.unclaimed.len(),
            never_exported: p.never_exported,
            no_raw: p
                .pairs
                .iter()
                .filter(|x| p.shots[x.shot].raw.is_none())
                .count(),
            unpaired: unpaired(p),
        }
    }
}

/// The images pairing left, a line each: the image, why, and the
/// files of its name.
fn unpaired(p: &Pairing) -> Vec<String> {
    let name = |s: usize| {
        let shot = &p.shots[s];
        match &shot.copy {
            Some(copy) => format!("{} ({copy})", shot.name),
            None => shot.name.clone(),
        }
    };
    let files = |fs: &[PathBuf]| {
        fs.iter()
            .filter_map(|f| f.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out: Vec<String> = p
        .not_found
        .iter()
        .map(|&s| format!("{}: exported, no file found", name(s)))
        .collect();
    out.extend(
        p.misshapen
            .iter()
            .map(|(s, fs)| format!("{}: not the shape of {}", name(*s), files(fs))),
    );
    out.extend(p.undeveloped.iter().map(|(s, fs)| {
        format!(
            "{}: never developed here, not the render of {}",
            name(*s),
            files(fs)
        )
    }));
    out.extend(
        p.ambiguous
            .iter()
            .map(|(s, fs)| format!("{}: in doubt between {}", name(*s), files(fs))),
    );
    out
}

/// A body and profile, and the look found for it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct LookLine {
    pub(crate) camera: String,
    pub(crate) profile: String,
    pub(crate) look: Option<String>,
    pub(crate) frames: usize,
}

/// The whole report.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Report {
    pub(crate) schema: i64,
    pub(crate) pairing: PairingLine,
    /// The pairs measured, of how many; `--limit` takes fewer.
    pub(crate) measured: usize,
    pub(crate) of: usize,
    pub(crate) looks: Vec<LookLine>,
    pub(crate) frames: Vec<FrameLine>,
    pub(crate) by_profile: Vec<ProfileLine>,
    /// Over the frames with a global edit, then over every frame.
    pub(crate) by_slider: Vec<SliderLine>,
    pub(crate) by_slider_all: Vec<SliderLine>,
    /// The frames with masks, a lens blur or an old process, by name.
    pub(crate) apart: Vec<String>,
    pub(crate) seconds: f32,
}

/// Group the frames into a report.
pub(crate) fn report(pairing: PairingLine, frames: Vec<FrameLine>) -> Report {
    let global: Vec<&FrameLine> = frames.iter().filter(|f| f.global()).collect();
    let all: Vec<&FrameLine> = frames.iter().collect();
    let mut looks: BTreeMap<(String, String), (Option<String>, usize)> = BTreeMap::new();
    for f in frames
        .iter()
        .filter(|f| matches!(f.kind, Some(Kind::CameraLook | Kind::CameraNoLook)))
    {
        let e = looks
            .entry((f.camera.clone(), f.profile.clone()))
            .or_insert((f.look.clone(), 0));
        e.1 += 1;
    }
    Report {
        pairing,
        looks: looks
            .into_iter()
            .map(|((camera, profile), (look, frames))| LookLine {
                camera,
                profile,
                look,
                frames,
            })
            .collect(),
        by_profile: by_profile(&frames),
        by_slider: by_slider(&global),
        by_slider_all: by_slider(&all),
        apart: frames
            .iter()
            .filter(|f| !f.global())
            .map(|f| f.name.clone())
            .collect(),
        frames,
        ..Report::default()
    }
}

/// Read, pair, develop and measure, and group.
pub(crate) fn run(opts: &Options) -> Result<Report> {
    let start = Instant::now();
    let pairing = lx::pair(
        &opts.catalog,
        &lx::Options {
            roots: opts.roots.clone(),
            folders: opts.folders.clone(),
        },
    )
    .with_context(|| format!("reading {}", opts.catalog.display()))?;
    let schema = pairing.schema;
    let line = PairingLine::of(&pairing);
    eprintln!(
        "measure lightroom: {} images, {} exported by their history; {} pairs ({} by history, \
         {} by name), {} not found, {} in doubt",
        line.images,
        line.exported,
        line.pairs,
        line.by_history,
        line.by_name,
        line.not_found,
        line.ambiguous + line.misshapen
    );
    let measurable: Vec<&lx::Pair> = pairing.pairs.iter().collect();
    let take = match opts.limit {
        Some(n) => cm::evenly(measurable.len(), n),
        None => (0..measurable.len()).collect(),
    };
    let lenses = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| db);
    if lenses.is_none() {
        eprintln!("measure lightroom: no lens database; the develop runs without lens corrections");
    }
    let store = greycard_edit::look::store_dir();
    let mut frames = Vec::new();
    for (k, &i) in take.iter().enumerate() {
        let pair = measurable[i];
        let shot = &pairing.shots[pair.shot];
        let line = frame(pair, shot, store.as_deref(), lenses.as_ref());
        eprintln!(
            "measure lightroom: {}/{} {}: {}",
            k + 1,
            take.len(),
            line.name,
            match (&line.measured, &line.failed) {
                (Some(m), _) => format!(
                    "ΔE {:.4} (L {:+.4}, C {:+.4}), {:+.2} stops, {:.4} matched, {:.0} s",
                    m.de, m.dl, m.dc, m.offset, m.matched_de, m.seconds
                ),
                (None, Some(why)) => format!("not measured: {why}"),
                (None, None) => "not measured".into(),
            }
        );
        frames.push(line);
    }
    let mut out = report(line, frames);
    out.schema = schema;
    out.measured = take.len();
    out.of = measurable.len();
    out.seconds = start.elapsed().as_secs_f32();
    Ok(out)
}

/// One pair read, developed and measured.
fn frame(
    pair: &lx::Pair,
    shot: &lx::Shot,
    store: Option<&Path>,
    lenses: Option<&greycard_lens::Database>,
) -> FrameLine {
    let words = lx::words(&pair.settings);
    let profile = profile_of(&words);
    let mut local = Vec::new();
    if words.keys().any(|k| {
        [
            "MaskGroupBasedCorrections",
            "CircularGradientBasedCorrections",
            "GradientBasedCorrections",
            "PaintBasedCorrections",
        ]
        .iter()
        .any(|m| k.starts_with(m))
    }) {
        local.push("masks".to_string());
    }
    for (key, what) in [
        ("LensBlur", "lens blur"),
        ("RetouchAreas", "spots"),
        ("FilterList", "Enhance"),
    ] {
        if words.contains_key(key) {
            local.push(what.to_string());
        }
    }
    let crop = Crop::of(&words);
    // The process the export was rendered under: the step's own, else
    // the image's as it stands.
    let process = words
        .get("ProcessVersion")
        .map(|v| v.trim_matches('"').to_string())
        .or_else(|| shot.process.clone());
    let first = shot.imported.as_deref().map(lx::words).unwrap_or_default();
    let arrived = ["Temperature", "Tint"]
        .into_iter()
        .filter_map(|k| {
            let v = first.get(k)?.trim().parse::<f32>().ok()?;
            Some((k.to_lowercase(), v))
        })
        .collect();
    let mut line = FrameLine {
        name: match &shot.copy {
            Some(copy) => format!("{} ({copy})", shot.name),
            None => shot.name.clone(),
        },
        export: pair
            .export
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        image: shot.image,
        confidence: pair.confidence.word().to_string(),
        step: pair.step.clone(),
        developed: shot.developed,
        arrived,
        profile: profile.clone(),
        sliders: sliders(&pair.settings, shot.imported.as_deref()),
        local,
        cropped: crop.is_some(),
        angle: crop.filter(|c| c.angle != 0.0).map(|c| c.angle as f32),
        mono: words
            .get("ConvertToGrayscale")
            .is_some_and(|v| v.eq_ignore_ascii_case("true")),
        old_process: process
            .as_deref()
            .and_then(|p| p.trim().parse::<f32>().ok())
            .is_some_and(|p| p < PROCESS_2012),
        process,
        ..FrameLine::default()
    };
    let Some(raw) = shot.raw.as_deref() else {
        line.failed = Some("the raw is not here".into());
        return line;
    };
    let (mut edit, unmapped) =
        match greycard_library::lightroom_catalog::imported_edit(raw, &pair.settings) {
            Ok(e) => e,
            Err(e) => {
                line.failed = Some(format!("the settings: {e}"));
                return line;
            }
        };
    line.unmapped = unmapped;
    line.camera = greycard_core::decode::probe_path(raw)
        .map(|p| greycard_library::Exif::from_probe(&p).camera)
        .unwrap_or_default();
    line.kind = Some(if profile.starts_with("Camera ") {
        line.look = store.and_then(|s| look_for(s, &line.camera, &profile, edit.display_curve));
        if line.look.is_some() {
            Kind::CameraLook
        } else {
            Kind::CameraNoLook
        }
    } else if profile.starts_with("Adobe ") {
        Kind::Adobe
    } else {
        Kind::Other
    });
    if let Some(look) = &line.look {
        edit.look_lut = LookLut {
            lut: LutChoice::Named(look.clone()),
            strength: 1.0,
        };
    }
    let export = match read_export(&pair.export) {
        Ok(p) => p,
        Err(e) => {
            line.failed = Some(format!("the export: {e}"));
            return line;
        }
    };
    match measure_frame(raw, &edit, crop.as_ref(), shot.size, &export, lenses) {
        Ok(m) => line.measured = Some(m),
        Err(e) => line.failed = Some(e),
    }
    line
}

// ---------------------------------------------------------------------
// The text.

fn stats_cell(s: Option<&Stats>) -> String {
    match s {
        Some(s) => format!(
            "{:>3}  ΔE {:.4} (median {:.4})  L {:+.4}  C {:+.4}  {:+.2} stops  matched {:.4}",
            s.frames, s.de, s.de_median, s.dl, s.dc, s.offset, s.matched_de
        ),
        None => "  0".into(),
    }
}

/// The report as the command line prints it, in the CLI's style: a
/// word in a column, then what it says.
pub(crate) fn text(r: &Report) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let p = &r.pairing;
    let _ = writeln!(
        out,
        "{:<13} {} images ({} virtual copies), schema {}",
        "catalog", p.images, p.copies, r.schema
    );
    let _ = writeln!(
        out,
        "{:<13} {} exported by their history; {} files in {} export folders",
        "exports", p.exported, p.files, p.folders
    );
    let _ = writeln!(
        out,
        "{:<13} {} ({} by the history's settings, {} by name with the current settings, {} \
         by name of images exported at other times), {} raws not here",
        "pairs", p.pairs, p.by_history, p.by_name, p.by_name_elsewhere, p.no_raw
    );
    let _ = writeln!(
        out,
        "{:<13} {} exported and not found, {} of their name and not their shape, {} never \
         developed, {} in doubt, {} files no image's, {} never exported",
        "unpaired",
        p.not_found,
        p.misshapen,
        p.undeveloped,
        p.ambiguous,
        p.unclaimed,
        p.never_exported
    );
    for u in &p.unpaired {
        let _ = writeln!(out, "{:<13} {u}", "");
    }
    let _ = writeln!(
        out,
        "{:<13} {} of {} pairs in {:.0} s",
        "measured", r.measured, r.of, r.seconds
    );
    for l in &r.looks {
        let _ = writeln!(
            out,
            "{:<13} {} {}: {} ({} frames)",
            "look",
            l.camera,
            l.profile,
            l.look.as_deref().unwrap_or("none fitted"),
            l.frames
        );
    }
    let _ = writeln!(out);
    for f in &r.frames {
        let off: Vec<String> = f.sliders.iter().map(|(k, v)| format!("{k} {v}")).collect();
        let mut notes = f.local.clone();
        if f.cropped {
            notes.push(match f.angle {
                Some(a) => format!("crop at {a}°"),
                None => "crop".into(),
            });
        }
        if f.mono {
            notes.push("mono".into());
        }
        if f.old_process {
            notes.push(format!("process {}", f.process.as_deref().unwrap_or("old")));
        }
        let said = match (&f.measured, &f.failed) {
            (Some(m), _) => format!(
                "ΔE {:.4}  L {:+.4}  C {:+.4}  {:+.2} stops  matched {:.4}",
                m.de, m.dl, m.dc, m.offset, m.matched_de
            ),
            (None, Some(why)) => format!("not measured: {why}"),
            (None, None) => "not measured".into(),
        };
        let _ = writeln!(out, "{:<13} {}  {said}", "frame", f.name);
        let _ = writeln!(
            out,
            "{:<13} {}, {}, {}{}{}",
            "",
            f.profile,
            f.kind.map_or("?", Kind::words),
            f.confidence,
            if off.is_empty() {
                String::new()
            } else {
                format!("; {}", off.join(", "))
            },
            if notes.is_empty() {
                String::new()
            } else {
                format!("; {}", notes.join(", "))
            },
        );
    }
    let _ = writeln!(out);
    for l in &r.by_profile {
        let _ = writeln!(out, "{:<13} {} ({})", "profile", l.profile, l.kind.words());
        let _ = writeln!(out, "{:<13} global {}", "", stats_cell(l.global.as_ref()));
        let _ = writeln!(out, "{:<13} local  {}", "", stats_cell(l.local.as_ref()));
    }
    for (title, lines) in [
        ("global edits", &r.by_slider),
        ("every frame", &r.by_slider_all),
    ] {
        let _ = writeln!(out);
        let _ = writeln!(out, "{:<13} {title}: off its default / at it", "by slider");
        for l in lines {
            let _ = writeln!(out, "{:<13} off  {}", l.slider, stats_cell(l.off.as_ref()));
            let _ = writeln!(out, "{:<13} at   {}", "", stats_cell(l.default.as_ref()));
        }
    }
    if !r.apart.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{:<13} {} frames with masks, a lens blur or an old process: {}",
            "apart",
            r.apart.len(),
            r.apart.join(", ")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(name: &str, de: f32, sliders: &[(&str, f32)], local: bool) -> FrameLine {
        FrameLine {
            name: name.into(),
            profile: "Camera Faithful".into(),
            kind: Some(Kind::CameraLook),
            sliders: sliders.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            local: if local { vec!["masks".into()] } else { vec![] },
            measured: Some(Measured {
                de,
                dl: de / 2.0,
                offset: de * 10.0,
                matched_de: de / 2.0,
                ..Measured::default()
            }),
            ..FrameLine::default()
        }
    }

    #[test]
    fn sliders_group_the_frames_off_their_default_against_the_rest() {
        let frames = vec![
            line(
                "a",
                0.04,
                &[("exposure", 0.5), ("highlights", -40.0)],
                false,
            ),
            line("b", 0.02, &[("exposure", -0.3)], false),
            line("c", 0.01, &[], false),
            line("d", 0.03, &[("highlights", -20.0)], false),
            // Masked: out of the global table, in the every-frame one.
            line("e", 0.10, &[("highlights", -100.0)], true),
            FrameLine {
                failed: Some("no blocks".into()),
                ..line("f", 0.0, &[("highlights", -10.0)], false)
            },
        ];
        let frames: Vec<FrameLine> = frames
            .into_iter()
            .map(|mut f| {
                if f.failed.is_some() {
                    f.measured = None;
                }
                f
            })
            .collect();
        let r = report(PairingLine::default(), frames);
        let of =
            |lines: &[SliderLine], s: &str| lines.iter().find(|l| l.slider == s).cloned().unwrap();
        let h = of(&r.by_slider, "highlights");
        let off = h.off.unwrap();
        let at = h.default.unwrap();
        assert_eq!((off.frames, at.frames), (2, 2));
        assert!((off.de - 0.035).abs() < 1e-6, "{off:?}");
        assert!((at.de - 0.015).abs() < 1e-6, "{at:?}");
        assert!((off.de_median - 0.035).abs() < 1e-6);
        let e = of(&r.by_slider, "exposure").off.unwrap();
        assert_eq!(e.frames, 2);
        assert!((e.offset - 0.3).abs() < 1e-5, "{e:?}");
        // No frame moves the curve: all at the default.
        let c = of(&r.by_slider, "tone curve");
        assert!(c.off.is_none());
        assert_eq!(c.default.unwrap().frames, 4);
        // The masked frame counts in the every-frame table.
        let h_all = of(&r.by_slider_all, "highlights").off.unwrap();
        assert_eq!(h_all.frames, 3);
        assert_eq!(r.apart, vec!["e".to_string()]);
        // By profile: the global and the local apart.
        assert_eq!(r.by_profile.len(), 1);
        assert_eq!(r.by_profile[0].global.as_ref().unwrap().frames, 4);
        assert_eq!(r.by_profile[0].local.as_ref().unwrap().frames, 1);
        assert_eq!(GROUPS.len(), r.by_slider.len());
    }

    #[test]
    fn sliders_are_read_off_the_settings_against_what_the_image_arrived_with() {
        let settings = "s = { Exposure2012 = 0.85, Contrast2012 = 0, Highlights2012 = -35,\n\
            Temperature = 5600, Tint = 12, WhiteBalance = \"Custom\",\n\
            ToneCurvePV2012 = { 0, 0, 64, 70, 255, 255 },\n\
            ParametricShadows = 0, Texture = 10 }";
        let imported = "s = { Temperature = 5600, Tint = 4, WhiteBalance = \"As Shot\" }";
        let s = sliders(settings, Some(imported));
        assert_eq!(
            s.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["exposure", "highlights", "texture", "tint", "tone curve"]
        );
        assert_eq!(s["tone curve"], 6.0);
        // With no first step, the white balance's own word decides.
        let s = sliders(settings, None);
        assert!(s.contains_key("temperature") && s.contains_key("tint"));
        assert!(
            sliders(
                "s = { WhiteBalance = \"As Shot\", Temperature = 5000 }",
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn the_json_has_the_report_s_shape() {
        let frames = vec![line("a", 0.04, &[("exposure", 0.5)], false)];
        let r = report(
            PairingLine {
                pairs: 1,
                ..PairingLine::default()
            },
            frames,
        );
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        for key in [
            "schema",
            "pairing",
            "measured",
            "of",
            "looks",
            "frames",
            "by_profile",
            "by_slider",
            "by_slider_all",
            "apart",
            "seconds",
        ] {
            assert!(v.get(key).is_some(), "no {key} in {v}");
        }
        let frame = &v["frames"][0];
        for key in [
            "name",
            "image",
            "export",
            "confidence",
            "step",
            "developed",
            "arrived",
            "profile",
            "kind",
            "look",
            "sliders",
            "local",
            "cropped",
            "old_process",
            "measured",
            "failed",
        ] {
            assert!(frame.get(key).is_some(), "no {key} in {frame}");
        }
        assert_eq!(frame["kind"], "camera-look");
        assert_eq!(frame["sliders"]["exposure"], 0.5);
        for key in ["de", "dl", "dc", "offset", "matched_de", "ncc"] {
            assert!(frame["measured"].get(key).is_some(), "no {key}");
        }
        let slider = &v["by_slider"][0];
        assert_eq!(slider["slider"], "exposure");
        assert_eq!(slider["off"]["frames"], 1);
        assert!(slider["default"].is_null());
        assert_eq!(v["by_profile"][0]["global"]["frames"], 1);
        assert_eq!(v["pairing"]["pairs"], 1);
    }

    #[test]
    fn a_frame_takes_the_process_of_the_step_its_export_was_rendered_from() {
        let shot = lx::Shot {
            image: 7,
            raw: None,
            name: "IMG_0001.CR3".into(),
            copy: None,
            settings: Some("s = { ProcessVersion = \"15.4\" }".into()),
            process: Some("15.4".into()),
            developed: true,
            imported: Some("s = { Temperature = 5200, Tint = 3 }".into()),
            exports: Vec::new(),
            size: None,
        };
        let pair = lx::Pair {
            shot: 0,
            export: PathBuf::from("IMG_0001.jpg"),
            confidence: Confidence::History,
            step: Some("Export - Hard Drive (1/1/26 12:00:00 PM)".into()),
            settings: "s = { ProcessVersion = \"5.7\", Temperature = 5200, Tint = 3 }".into(),
        };
        let line = frame(&pair, &shot, None, None);
        assert_eq!(line.process.as_deref(), Some("5.7"));
        assert!(line.old_process && !line.global());
        assert_eq!(line.image, 7);
        assert!(line.developed);
        assert_eq!(line.step, pair.step);
        assert_eq!(line.arrived["temperature"], 5200.0);
        assert_eq!(line.arrived["tint"], 3.0);
        // Unmoved from what it arrived with: not among the sliders.
        assert!(!line.sliders.contains_key("temperature") && !line.sliders.contains_key("tint"));
        assert_eq!(line.failed.as_deref(), Some("the raw is not here"));
    }

    #[test]
    fn a_style_reads_the_same_in_a_profile_and_a_look() {
        assert_eq!(style_key("PROVIA/Standard"), style_key("Provia"));
        assert_eq!(style_key("REALA ACE"), style_key("Reala Ace"));
        assert_eq!(style_key("ACROS+G Filter"), style_key("Acros + G Filter"));
        assert_eq!(style_key("CLASSIC Neg."), style_key("Classic Neg."));
        assert_ne!(style_key("Faithful"), style_key("Standard"));
    }

    #[test]
    fn a_look_is_found_by_the_body_and_the_profile_s_style() {
        let dir = std::env::temp_dir().join(format!("greycard-measure-lr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // An identity table under the body's names.
        let mut cube = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    cube.push_str(&format!("{r} {g} {b}\n"));
                }
            }
        }
        std::fs::write(dir.join("Fujifilm GFX 100S II Reala Ace.cube"), &cube).unwrap();
        std::fs::write(dir.join("Fujifilm GFX 100S II Provia.agx.cube"), &cube).unwrap();
        let curve = greycard_edit::DisplayCurve::Channels;
        assert_eq!(
            look_for(&dir, "Fujifilm GFX 100S II", "Camera REALA ACE", curve).as_deref(),
            Some("Fujifilm GFX 100S II Reala Ace")
        );
        // A curve's variant alone is not the look's table; another body,
        // and Adobe's profiles, have none.
        assert_eq!(
            look_for(
                &dir,
                "Fujifilm GFX 100S II",
                "Camera PROVIA/Standard",
                curve
            ),
            None
        );
        assert_eq!(
            look_for(&dir, "Fujifilm X-T5", "Camera REALA ACE", curve),
            None
        );
        assert_eq!(
            look_for(&dir, "Fujifilm GFX 100S II", "Adobe Standard", curve),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A picture of flat patches with edges between them: blocks to
    /// compare and structure to register on.
    fn patches(w: usize, h: usize) -> Picture {
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (i, j) = (x * 7 / w, y * 5 / h);
                let v = 0.15 + 0.1 * ((i * 3 + j * 5) % 7) as f32;
                data.push([v, v * 0.8 + 0.05, v * 0.6 + 0.1]);
            }
        }
        Picture::new(w, h, data)
    }

    #[test]
    fn a_cut_without_a_turn_is_the_crop_s_pixels_and_turns_follow_the_shape() {
        let p = patches(200, 100);
        let crop = Crop {
            left: 0.1,
            top: 0.2,
            right: 0.6,
            bottom: 0.9,
            angle: 0.0,
        };
        let c = cut(&p, Orientation::Normal, Some(&crop), None);
        assert_eq!((c.width, c.height), (100, 70));
        for (y, x) in [(0, 0), (35, 50), (69, 99)] {
            assert_eq!(c.at(y, x), p.at(y + 20, x + 20));
        }
        // A render turned a quarter is taken back first: the same cut.
        let turned = to_picture(greycard_core::develop::orient(
            to_working(&p),
            Orientation::Rotate90,
        ));
        let c2 = cut(&turned, Orientation::Rotate90, Some(&crop), None);
        assert_eq!(c2, c);
        // An export the cut's shape takes it as it is or turned half;
        // one the other way, a quarter turn.
        assert_eq!(
            turns(&c, Orientation::Normal, (1000, 700)),
            vec![Orientation::Normal, Orientation::Rotate180]
        );
        assert_eq!(
            turns(&c, Orientation::Rotate90, (700, 1000)),
            vec![Orientation::Rotate90, Orientation::Rotate270]
        );
        assert!(turns(&c, Orientation::Normal, (1000, 1000)).is_empty());
    }

    /// A linear DNG of flat patches, `w` by `h`.
    fn dng(path: &Path, w: usize, h: usize) {
        use greycard_core::raw::{
            Calibration, CfaPattern, LevelPattern, Levels, RawFrame, Samples, SensorLayout,
        };
        let m = greycard_core::color::WORKING_SPACE
            .from_xyz_matrix()
            .expect("the working space has a matrix");
        let frame = RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            width: w,
            height: h,
            channels: 1,
            layout: SensorLayout::Cfa(CfaPattern::rggb()),
            samples: Samples::U16(Vec::new()),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 1),
                white: LevelPattern::uniform(65535.0, 1),
            },
            as_shot_coefficients: Some([2.0, 1.0, 1.6]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: m.rows.iter().flatten().map(|v| *v as f32).collect(),
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
        };
        let scene = patches(w, h);
        let data: Vec<f32> = scene
            .data
            .iter()
            .flat_map(|c| c.map(|v| v * v * 0.5))
            .collect();
        let image = greycard_core::CameraImage::from_data(w, h, data).unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        greycard_core::dng::write_linear_dng(
            &mut file,
            &frame,
            &image,
            [2.0, 1.0, 1.6],
            &Default::default(),
        )
        .unwrap();
    }

    /// The develop path against itself: a frame developed under mapped
    /// settings, its own finish cut as Lightroom's crop says taken as
    /// the export, lands on it with no error and no offset.
    #[test]
    fn a_frame_measured_against_its_own_render_has_no_error() {
        let dir =
            std::env::temp_dir().join(format!("greycard-measure-lr-dng-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("frame.dng");
        dng(&raw, 840, 560);
        let settings = "s = { Exposure2012 = 0.3, Contrast2012 = 10, WhiteBalance = \"As Shot\",\n\
            CropLeft = 0.25, CropRight = 0.75 }";
        let (edit, _) = greycard_library::lightroom_catalog::imported_edit(&raw, settings).unwrap();
        assert!((edit.light.exposure - 0.3).abs() < 1e-6);
        let crop = Crop::of(&lx::words(settings));
        let mut developed = crate::worker::FrameDevelop::develop(&raw, &edit, None).unwrap();
        let render = cm::picture_of(&developed.finish(
            &edit,
            &crate::export::Settings {
                long_edge: Some(long_edge_for(crop.as_ref(), None)),
                ..cm::settings()
            },
        ));
        let export = cut(&render, Orientation::Normal, crop.as_ref(), None);
        assert_eq!(
            (export.width, export.height),
            (render.width / 2, render.height)
        );
        let m = measure_frame(&raw, &edit, crop.as_ref(), None, &export, None).unwrap();
        assert!(m.blocks > 10, "{m:?}");
        assert!(m.de < 0.002, "{m:?}");
        assert!(m.offset.abs() < 0.01, "{m:?}");
        assert!(m.ncc > 0.99, "{m:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
