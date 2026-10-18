//! `--fit-lightroom DIR...`: the Lightroom mapper's scales and its
//! per-body exposure offsets fitted to Lightroom's own exports, from
//! the blocks `--measure-lightroom CATALOG --blocks DIR` kept.
//!
//! The measurement develops each paired raw once and finishes it under
//! the catalog's settings as the import maps them, and keeps, for every
//! block the comparison kept, what the finish read over that block
//! (the scene's mean in the working space, the guide plane's mean, the
//! vignette's stops) and what both sides came out as. A block the
//! comparison kept is flat on both sides, so the finish of its mean is
//! within a hair of the mean of its finish, and the fit can run the
//! finish's own pixel function ([`crate::finish::finish_pixel_with`])
//! over the blocks for any scales it tries, thousands of times, without
//! developing a frame again. The record of the real render's block mean
//! beside the emulation's is the check on that: the report says how
//! far the two sit apart at the scales the measurement ran under.
//!
//! What is fitted: one exposure offset per body (where Lightroom's
//! Exposure zero sits against ours), and the mapper's scales
//! ([`greycard_edit::lightroom::Scales`]) in the order of how much they
//! move a picture. The error minimized is the mean over frames of the
//! frame's mean ΔE in Oklab against the export, brightness and all;
//! the brightness-matched error is reported beside it. History pairs
//! only: a frame paired by name is read and left out. Held out by
//! catalog where more than one has history pairs, by body, and by the
//! edit the photographer pasted (the frames sharing one set of slider
//! values, exposure aside), since within one shoot those are the
//! independent points. Nothing is written but `--out`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use greycard_core::lut;
use greycard_edit::lightroom::{self, Scales};
use greycard_edit::look::{LookLut, LutChoice};
use greycard_edit::{DisplayCurve, Edit};
use greycard_match::color::{decode3, luminance, oklab};

use crate::finish;
use crate::match_compare::Acc;
use crate::measure_lightroom::{FrameLine, Kept};

/// One kept block, as the record keeps it: the scene's mean in the
/// working space before any exposure, the guide at it (none when no
/// shift reads the plane), the vignette's stops, the export's mean and
/// the real render's, both encoded sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Block {
    pub(crate) s: [f32; 3],
    pub(crate) g: Option<f32>,
    pub(crate) v: f32,
    pub(crate) e: [f32; 3],
    pub(crate) r: [f32; 3],
}

/// One measured frame's blocks and what the fit needs to map its
/// settings again: the settings text, the body, the look and the curve
/// the measurement used, and the offset it applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FrameBlocks {
    pub(crate) name: String,
    pub(crate) image: i64,
    pub(crate) camera: String,
    pub(crate) profile: String,
    pub(crate) look: Option<String>,
    pub(crate) curve: String,
    pub(crate) confidence: String,
    pub(crate) global: bool,
    pub(crate) settings: String,
    /// The stops the mapper added to the Exposure slider when the
    /// frame was measured.
    pub(crate) offset: f32,
    /// The scales the settings were mapped with, by name
    /// ([`Scales::NAMES`]), so the emulation can run at them.
    #[serde(default)]
    pub(crate) scales: BTreeMap<String, f32>,
    /// A hash of the look's table file as read, so a later run can
    /// tell when the store's table is another one.
    #[serde(default)]
    pub(crate) look_hash: Option<String>,
    pub(crate) range: [f32; 2],
    pub(crate) sliders: BTreeMap<String, f32>,
    pub(crate) blocks: Vec<Block>,
}

impl FrameBlocks {
    /// `settings` are kept without what the fit does not read and a
    /// photographer may have named ([`without_names`]).
    pub(crate) fn of(
        line: &FrameLine,
        settings: &str,
        edit: &Edit,
        kept: Kept,
        scales: &Scales,
        look_hash: Option<String>,
    ) -> Self {
        FrameBlocks {
            name: line.name.clone(),
            image: line.image,
            camera: line.camera.clone(),
            profile: line.profile.clone(),
            look: line.look.clone(),
            curve: edit.display_curve.key(),
            confidence: line.confidence.clone(),
            global: line.global(),
            settings: without_names(settings),
            offset: if line.offset_applied {
                line.body_offset
            } else {
                0.0
            },
            scales: Scales::NAMES
                .iter()
                .filter_map(|n| Some((n.to_string(), scales.get(n)?)))
                .collect(),
            look_hash,
            range: kept.range,
            sliders: line.sliders.clone(),
            blocks: kept.blocks,
        }
    }

    /// The scales the frame was measured under: the shipped ones for a
    /// name the record does not hold.
    fn scales(&self) -> Scales {
        let mut s = Scales::DEFAULT;
        for (name, v) in &self.scales {
            s.set(name, *v);
        }
        s
    }

    /// Written into `dir` as `<image id>-<file name>.json`.
    pub(crate) fn write(&self, dir: &Path) -> Result<PathBuf> {
        let stem: String = self
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let path = dir.join(format!("{}-{stem}.json", self.image));
        std::fs::write(&path, serde_json::to_vec(self)?)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
}

/// The settings with the names a photographer gives masks and
/// corrections emptied: the fit reads the top-level keys alone, and a
/// block record is kept beside a report, where no one's name belongs.
/// Each is `MaskName = "..."` wherever it sits in the table.
pub(crate) fn without_names(settings: &str) -> String {
    let mut out = settings.to_string();
    for key in ["MaskName", "CorrectionName"] {
        let mut from = 0;
        while let Some(at) = out[from..].find(key).map(|i| i + from) {
            let after = at + key.len();
            // The key as a word, then `=`, then a quoted value.
            let word = at == 0 || !out.as_bytes()[at - 1].is_ascii_alphanumeric();
            let rest = &out[after..];
            let eq = rest.trim_start();
            if word
                && let Some(value) = eq.strip_prefix('=')
                && let Some(quoted) = value.trim_start().strip_prefix('"')
                && let Some(len) = quote_end(quoted)
            {
                let open = out.len() - quoted.len();
                out.replace_range(open..open + len, "");
                from = open + 1;
            } else {
                from = after;
            }
        }
    }
    out
}

/// Where a quoted string's text ends: the first `"` not escaped by a
/// backslash, as the table's own strings escape one.
fn quote_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// A hash of the look table `name` under `curve` in `store`, as the
/// file is: the first sixteen hex digits of blake3 over its bytes.
pub(crate) fn look_hash(store: &Path, name: &str, curve: DisplayCurve) -> Option<String> {
    let path = store.join(format!(
        "{}.cube",
        greycard_edit::look::variant_stem(name, curve)
    ));
    let bytes = std::fs::read(path).ok()?;
    Some(blake3::hash(&bytes).to_hex()[..16].to_string())
}

// ---------------------------------------------------------------------
// The frames and the emulated finish.

/// A frame as the fit holds it: its record, which catalog folder it
/// came from, the export's blocks in Oklab and linear luminance, and
/// the look's table read once.
struct Frame {
    rec: FrameBlocks,
    catalog: usize,
    curve: DisplayCurve,
    /// The slider set the photographer pasted: the sliders off their
    /// default, the exposure aside, with the profile.
    cluster: String,
    lab_e: Vec<[f32; 3]>,
    lum_e: Vec<f32>,
    look: Option<lut::Look>,
}

/// The sliders the fit does not touch, left out of a frame's cluster
/// key: the exposure the photographer nudges per frame.
const NOT_CLUSTERED: [&str; 1] = ["exposure"];

fn load(dirs: &[PathBuf], store: Option<&Path>) -> Result<(Vec<Frame>, usize)> {
    let mut frames = Vec::new();
    let mut looks: HashMap<(String, String), Option<lut::Look>> = HashMap::new();
    let mut name_pairs = 0;
    for (ci, dir) in dirs.iter().enumerate() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read(&path)?;
            let rec: FrameBlocks = serde_json::from_slice(&text)
                .with_context(|| format!("reading {}", path.display()))?;
            if rec.confidence != "history" {
                name_pairs += 1;
                continue;
            }
            if rec.blocks.is_empty() {
                continue;
            }
            let curve = DisplayCurve::from_key(&rec.curve).unwrap_or_default();
            let look = rec.look.as_ref().and_then(|name| {
                looks
                    .entry((name.clone(), curve.key()))
                    .or_insert_with(|| {
                        let store = store?;
                        LookLut {
                            lut: LutChoice::Named(name.clone()),
                            strength: 1.0,
                        }
                        .look_under_in(store, curve)
                        .filter(|l| !l.is_off())
                    })
                    .clone()
            });
            if rec.look.is_some() && look.is_none() {
                anyhow::bail!(
                    "{}: the look {:?} is not in the look store",
                    rec.name,
                    rec.look
                );
            }
            if let (Some(name), Some(was), Some(store)) = (&rec.look, &rec.look_hash, store)
                && look_hash(store, name, curve).as_ref() != Some(was)
            {
                eprintln!(
                    "fit lightroom: {}: the store's table {name} is not the one the frame \
                     was measured with; the emulation check reads against another look",
                    rec.name
                );
            }
            let cluster = format!(
                "{} | {}",
                rec.profile,
                rec.sliders
                    .iter()
                    .filter(|(k, _)| !NOT_CLUSTERED.contains(&k.as_str()))
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let lab_e = rec.blocks.iter().map(|b| oklab(decode3(b.e))).collect();
            let lum_e = rec
                .blocks
                .iter()
                .map(|b| luminance(decode3(b.e)).max(1e-6))
                .collect();
            frames.push(Frame {
                rec,
                catalog: ci,
                curve,
                cluster,
                lab_e,
                lum_e,
                look,
            });
        }
    }
    Ok((frames, name_pairs))
}

/// The frame's blocks through the finish under the settings mapped
/// with `scales` and `offset` stops on the Exposure slider: encoded
/// sRGB a block. None when the settings will not read.
fn render(frame: &Frame, scales: &Scales, offset: f32) -> Option<Vec<[f32; 3]>> {
    let imported = lightroom::read_table_with(&frame.rec.settings, "", scales, offset).ok()?;
    let mut edit = imported.preset.applied(&Edit::default());
    edit.display_curve = frame.curve;
    // The body's baseline, as the measurement's develop had it.
    let baseline = greycard_edit::baseline::for_body(Some(&frame.rec.camera));
    let global = finish::Baked::global(
        &edit,
        finish::Source::Scene(finish::Baseline::of_stops(baseline)),
    );
    let m = crate::export::Space::Srgb.matrix();
    let look = frame.look.as_ref();
    let range = frame.rec.range;
    Some(
        frame
            .rec
            .blocks
            .iter()
            .map(|b| {
                finish::finish_pixel_with(
                    b.s,
                    &global,
                    &[],
                    b.v,
                    None,
                    None,
                    b.g.map(|g| (g, range)),
                    None,
                    look,
                    &m,
                )
            })
            .collect(),
    )
}

/// A frame's numbers at one setting of the mapper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
struct Eval {
    de: f32,
    dl: f32,
    dl_abs: f32,
    dc: f32,
    dc_abs: f32,
    /// Lightroom's brightness over ours, stops, the median over the
    /// blocks: positive where Lightroom is brighter.
    offset: f32,
    matched_de: f32,
    matched_dl: f32,
    matched_dc: f32,
    /// Where the match ended: the stops added to the mapped exposure.
    matched_offset: f32,
    blocks: usize,
}

/// The emulated blocks against the export's: the summary and the
/// median offset in stops.
fn compare(frame: &Frame, out: &[[f32; 3]]) -> (crate::match_compare::Summary, f32) {
    let mut acc = Acc::default();
    let mut ratios: Vec<f32> = Vec::with_capacity(out.len());
    for (i, o) in out.iter().enumerate() {
        let lin = decode3(*o);
        acc.add(oklab(lin), frame.lab_e[i]);
        ratios.push((frame.lum_e[i] / luminance(lin).max(1e-6)).log2());
    }
    (acc.summary(), median(&mut ratios))
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// How close the match's exposure step brings the median, in stops,
/// and how many steps it may take.
const MATCHED_WITHIN: f32 = 0.02;
const MATCH_PASSES: usize = 4;

fn evaluate(frame: &Frame, scales: &Scales, offset: f32) -> Option<Eval> {
    let out = render(frame, scales, offset)?;
    let (s, off) = compare(frame, &out);
    // The brightness matched through the Exposure slider: a stop of
    // exposure is near a stop of the picture's median, so the residual
    // is the next step, taken until it is within the match's tolerance.
    let (mut at, mut residual, mut m) = (0.0f32, off, s);
    for _ in 0..MATCH_PASSES {
        if residual.abs() <= MATCHED_WITHIN {
            break;
        }
        at += residual;
        let out = render(frame, scales, offset + at)?;
        (m, residual) = compare(frame, &out);
    }
    Some(Eval {
        de: s.de,
        dl: s.dl,
        dl_abs: s.dl_abs,
        dc: s.dc,
        dc_abs: s.dc_abs,
        offset: off,
        matched_de: m.de,
        matched_dl: m.dl,
        matched_dc: m.dc,
        matched_offset: at,
        blocks: s.blocks,
    })
}

// ---------------------------------------------------------------------
// The parameters and the fit.

/// What is fitted: the bodies' offsets, then the scales in the order
/// of how much they move a picture.
#[derive(Debug, Clone)]
struct Space {
    bodies: Vec<String>,
}

/// The scales fitted, by name, each with its bounds and the step the
/// first round searches either side of its start.
const SCALE_PARAMS: [(&str, f32, f32, f32); 10] = [
    ("highlights", 0.0, 4.0, 1.5),
    ("shadows", 0.0, 3.0, 1.0),
    ("whites", 0.0, 3.0, 1.0),
    ("blacks", 0.0, 1.2, 0.3),
    ("contrast", 0.0, 1.5, 0.5),
    ("curve", 0.0, 2.5, 1.0),
    ("vibrance", 0.0, 3.0, 1.0),
    ("saturation", 0.0, 3.0, 1.0),
    ("mixer_luminance", 0.0, 3.0, 1.0),
    ("mixer_saturation", 0.0, 3.0, 1.0),
];
/// A body's offset: bounds and first step, stops.
const OFFSET_BOUNDS: (f32, f32, f32) = (-3.0, 1.5, 1.0);

fn scale_of(s: &Scales, name: &str) -> f32 {
    s.get(name)
        .unwrap_or_else(|| unreachable!("no scale named {name}"))
}

fn set_scale(s: &mut Scales, name: &str, value: f32) {
    assert!(s.set(name, value), "no scale named {name}");
}

impl Space {
    fn len(&self) -> usize {
        self.bodies.len() + SCALE_PARAMS.len()
    }

    fn name(&self, i: usize) -> String {
        if i < self.bodies.len() {
            format!("offset {}", self.bodies[i])
        } else {
            SCALE_PARAMS[i - self.bodies.len()].0.to_string()
        }
    }

    fn bounds(&self, i: usize) -> (f32, f32, f32) {
        if i < self.bodies.len() {
            OFFSET_BOUNDS
        } else {
            let (_, lo, hi, step) = SCALE_PARAMS[i - self.bodies.len()];
            (lo, hi, step)
        }
    }

    /// The vector at `scales` and `offsets`.
    fn start(&self, scales: &Scales, offsets: &HashMap<String, f32>) -> Vec<f32> {
        let mut x: Vec<f32> = self
            .bodies
            .iter()
            .map(|b| offsets.get(b).copied().unwrap_or(0.0))
            .collect();
        for (name, _, _, _) in SCALE_PARAMS {
            x.push(scale_of(scales, name));
        }
        x
    }

    fn unpack(&self, x: &[f32]) -> (Scales, HashMap<String, f32>) {
        let offsets = self.bodies.iter().cloned().zip(x.iter().copied()).collect();
        let mut s = Scales::DEFAULT;
        for (k, (name, _, _, _)) in SCALE_PARAMS.iter().enumerate() {
            set_scale(&mut s, name, x[self.bodies.len() + k]);
        }
        (s, offsets)
    }
}

/// The mean over `frames` of each frame's mean ΔE, at `x`.
fn cost(frames: &[&Frame], space: &Space, x: &[f32]) -> f32 {
    let (scales, offsets) = space.unpack(x);
    let evals: Vec<Option<Eval>> = frames
        .par_iter()
        .map(|f| {
            let o = offsets.get(&f.rec.camera).copied().unwrap_or(0.0);
            evaluate_once(f, &scales, o)
        })
        .collect();
    let (sum, n) = evals
        .iter()
        .flatten()
        .fold((0.0f64, 0usize), |(s, n), e| (s + f64::from(e.de), n + 1));
    if n == 0 {
        f32::MAX
    } else {
        (sum / n as f64) as f32
    }
}

/// [`evaluate`] without the match: what the fit minimizes.
fn evaluate_once(frame: &Frame, scales: &Scales, offset: f32) -> Option<Eval> {
    let out = render(frame, scales, offset)?;
    let (s, off) = compare(frame, &out);
    Some(Eval {
        de: s.de,
        dl: s.dl,
        dl_abs: s.dl_abs,
        dc: s.dc,
        dc_abs: s.dc_abs,
        offset: off,
        blocks: s.blocks,
        ..Eval::default()
    })
}

/// Golden-section search of `f` over `[lo, hi]`, `n` evaluations.
fn golden(lo: f32, hi: f32, n: usize, mut f: impl FnMut(f32) -> f32) -> (f32, f32) {
    const R: f32 = 0.618_034;
    let (mut a, mut b) = (lo, hi);
    let mut c = b - R * (b - a);
    let mut d = a + R * (b - a);
    let (mut fc, mut fd) = (f(c), f(d));
    for _ in 0..n.saturating_sub(2) {
        if fc < fd {
            b = d;
            d = c;
            fd = fc;
            c = b - R * (b - a);
            fc = f(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + R * (b - a);
            fd = f(d);
        }
    }
    if fc < fd { (c, fc) } else { (d, fd) }
}

/// Evaluations a parameter gets each round.
const EVALS: usize = 10;

/// Coordinate descent from `x0` over `frames`: each parameter in turn
/// searched within its step of where it stands, the steps halved each
/// round, a parameter kept where it was unless the search beats it.
/// `log` hears each round's cost.
fn fit(
    frames: &[&Frame],
    space: &Space,
    x0: &[f32],
    rounds: usize,
    log: &mut dyn FnMut(usize, f32, &[f32]),
) -> (Vec<f32>, f32) {
    let mut x = x0.to_vec();
    let mut best = cost(frames, space, &x);
    let mut steps: Vec<f32> = (0..space.len()).map(|i| space.bounds(i).2).collect();
    for round in 0..rounds {
        for i in 0..space.len() {
            let (lo, hi, _) = space.bounds(i);
            let (a, b) = ((x[i] - steps[i]).max(lo), (x[i] + steps[i]).min(hi));
            let (v, fv) = golden(a, b, EVALS, |v| {
                let mut y = x.clone();
                y[i] = v;
                cost(frames, space, &y)
            });
            if fv < best {
                x[i] = v;
                best = fv;
            }
        }
        log(round, best, &x);
        for s in &mut steps {
            *s *= 0.5;
        }
    }
    (x, best)
}

// ---------------------------------------------------------------------
// The report.

/// Means over a set of frames' evaluations.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Group {
    pub(crate) frames: usize,
    pub(crate) de: f32,
    pub(crate) dl: f32,
    pub(crate) dc: f32,
    pub(crate) dc_abs: f32,
    pub(crate) offset: f32,
    pub(crate) offset_sd: f32,
    pub(crate) matched_de: f32,
}

fn group(evals: &[Eval]) -> Group {
    if evals.is_empty() {
        return Group::default();
    }
    let n = evals.len() as f32;
    let mean = |f: &dyn Fn(&Eval) -> f32| evals.iter().map(f).sum::<f32>() / n;
    let offset = mean(&|e| e.offset);
    let var = evals
        .iter()
        .map(|e| (e.offset - offset).powi(2))
        .sum::<f32>()
        / n;
    Group {
        frames: evals.len(),
        de: mean(&|e| e.de),
        dl: mean(&|e| e.dl),
        dc: mean(&|e| e.dc),
        dc_abs: mean(&|e| e.dc_abs),
        offset,
        offset_sd: var.sqrt(),
        matched_de: mean(&|e| e.matched_de),
    }
}

/// One row of the tables: a body and profile before and after.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Row {
    pub(crate) camera: String,
    pub(crate) profile: String,
    pub(crate) look: Option<String>,
    pub(crate) before: Group,
    pub(crate) after: Group,
}

/// A hold-out: what was held, how many frames, and the held frames'
/// error under the shipped scales, under the fit on everything, and
/// under the fit on the rest alone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct HeldOut {
    pub(crate) by: String,
    pub(crate) held: String,
    pub(crate) frames: usize,
    pub(crate) before: f32,
    pub(crate) fitted_on_all: f32,
    pub(crate) held_out: f32,
    /// The held frames' body offsets as the fit on the rest left
    /// them, when the body was among the rest; else fitted on the held
    /// frames alone, which is said.
    pub(crate) note: String,
}

/// A frame's offset by the export's lightness band, at the shipped
/// scales: what the three Reala Ace frames within a tenth of a stop
/// share, or do not, with the twelve.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BandLine {
    pub(crate) name: String,
    pub(crate) camera: String,
    pub(crate) profile: String,
    pub(crate) offset: f32,
    /// Blocks and median offset in each band of [`BAND_EDGES`].
    pub(crate) bands: [(usize, f32); BANDS],
}

/// Edges of the lightness bands, Oklab L of the export's block: the
/// deep shadows, the darks, the mid-tones and the brights.
const BAND_EDGES: [f32; 3] = [0.3, 0.5, 0.7];
const BANDS: usize = BAND_EDGES.len() + 1;

/// A band's frame counts for a body and profile: the median over the
/// frames with at least [`BAND_FRAME_BLOCKS`] blocks in the band of
/// that band's offset, and how many frames that was.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BandRow {
    pub(crate) camera: String,
    pub(crate) profile: String,
    pub(crate) frames: usize,
    pub(crate) bands: [(usize, f32); BANDS],
}

/// Blocks a frame needs in a band for its band offset to count.
const BAND_FRAME_BLOCKS: usize = 50;

fn band_rows(lines: &[BandLine]) -> Vec<BandRow> {
    let mut keys: Vec<(String, String)> = lines
        .iter()
        .map(|l| (l.camera.clone(), l.profile.clone()))
        .collect();
    keys.sort();
    keys.dedup();
    let mut rows = Vec::new();
    let every: Vec<(String, String)> = {
        let mut b: Vec<String> = keys.iter().map(|(c, _)| c.clone()).collect();
        b.dedup();
        b.into_iter()
            .map(|c| (c, "(every profile)".to_string()))
            .collect()
    };
    for (camera, profile) in keys.iter().chain(&every) {
        let of: Vec<&BandLine> = lines
            .iter()
            .filter(|l| l.camera == *camera && (l.profile == *profile || profile.starts_with('(')))
            .collect();
        let bands = std::array::from_fn(|k| {
            let mut v: Vec<f32> = of
                .iter()
                .filter(|l| l.bands[k].0 >= BAND_FRAME_BLOCKS)
                .map(|l| l.bands[k].1)
                .collect();
            (v.len(), median(&mut v))
        });
        rows.push(BandRow {
            camera: camera.clone(),
            profile: profile.clone(),
            frames: of.len(),
            bands,
        });
    }
    rows
}

fn bands(frame: &Frame, out: &[[f32; 3]]) -> [(usize, f32); BANDS] {
    let mut r: [Vec<f32>; BANDS] = std::array::from_fn(|_| Vec::new());
    for (i, o) in out.iter().enumerate() {
        let l = frame.lab_e[i][0];
        let k = BAND_EDGES.iter().position(|e| l < *e).unwrap_or(BANDS - 1);
        r[k].push((frame.lum_e[i] / luminance(decode3(*o)).max(1e-6)).log2());
    }
    r.map(|mut v| (v.len(), median(&mut v)))
}

/// A scale's one-dimensional profile at the fit: the cost at a few
/// values of it with everything else held, which says how sharply the
/// data holds that scale.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Profile {
    pub(crate) name: String,
    pub(crate) at: Vec<(f32, f32)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Report {
    pub(crate) catalogs: usize,
    pub(crate) frames: usize,
    pub(crate) name_pairs_left_out: usize,
    pub(crate) blocks: usize,
    /// The emulation against the real render's block means at the
    /// scales the measurement ran under: mean over frames of the
    /// frame's mean ΔE, and the largest frame.
    pub(crate) emulation_de: f32,
    pub(crate) emulation_de_max: f32,
    pub(crate) before_scales: BTreeMap<String, f32>,
    pub(crate) after_scales: BTreeMap<String, f32>,
    /// Each body: the offset the measurement applied, the median of
    /// the output's offset at the start's scales, the median of the
    /// slider's matched offset (the constant's estimator), and the
    /// fit's.
    pub(crate) bodies: Vec<(String, f32, f32, f32, f32)>,
    pub(crate) cost_before: f32,
    pub(crate) cost_after: f32,
    pub(crate) rounds: Vec<(usize, f32)>,
    pub(crate) rows: Vec<Row>,
    pub(crate) held_out: Vec<HeldOut>,
    pub(crate) band_rows: Vec<BandRow>,
    pub(crate) bands: Vec<BandLine>,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) seconds: f32,
}

/// What the command line asked for.
pub(crate) struct Options {
    pub(crate) dirs: Vec<PathBuf>,
    pub(crate) out: Option<PathBuf>,
    pub(crate) rounds: usize,
    /// Fit the scales too; without it only the bodies' offsets move.
    pub(crate) scales: bool,
    /// Run the hold-outs, a fit each.
    pub(crate) hold_out: bool,
    /// The scales the fit starts from and the "before" is measured
    /// at: the shipped ones unless `--fit-scale` moved some.
    pub(crate) start: Scales,
}

/// `--fit-lightroom`, from the command line to the exit code.
pub(crate) fn headless(cli: &crate::Cli, dirs: &[PathBuf]) -> Result<std::process::ExitCode> {
    let mut start = Scales::DEFAULT;
    for given in &cli.fit_scale {
        start
            .set_from(given)
            .map_err(|e| anyhow::anyhow!("--fit-scale {e}"))?;
    }
    let opts = Options {
        dirs: dirs.to_vec(),
        out: cli.out.clone(),
        rounds: cli.fit_rounds,
        scales: !cli.offsets_only,
        hold_out: !cli.no_hold_out,
        start,
    };
    let report = run(&opts)?;
    print!("{}", text(&report));
    if let Some(out) = &opts.out {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(out, json).with_context(|| format!("writing {}", out.display()))?;
        eprintln!("fit lightroom: {}", out.display());
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn scales_map(s: &Scales) -> BTreeMap<String, f32> {
    SCALE_PARAMS
        .iter()
        .map(|(name, _, _, _)| (name.to_string(), scale_of(s, name)))
        .collect()
}

pub(crate) fn run(opts: &Options) -> Result<Report> {
    let start = Instant::now();
    let store = greycard_edit::look::store_dir();
    let (frames, name_pairs) = load(&opts.dirs, store.as_deref())?;
    anyhow::ensure!(
        !frames.is_empty(),
        "no history pairs with blocks under the folders given"
    );
    eprintln!(
        "fit lightroom: {} frames from {} folders, {} blocks; {} name pairs left out",
        frames.len(),
        opts.dirs.len(),
        frames.iter().map(|f| f.rec.blocks.len()).sum::<usize>(),
        name_pairs
    );
    let mut bodies: Vec<String> = frames.iter().map(|f| f.rec.camera.clone()).collect();
    bodies.sort();
    bodies.dedup();
    let space = Space {
        bodies: bodies.clone(),
    };
    let all: Vec<&Frame> = frames.iter().collect();

    // The emulation against the real render, at the scales and the
    // offset the measurement ran under (the record's), with the look
    // as the store has it now.
    let emulation: Vec<f32> = all
        .par_iter()
        .filter_map(|f| {
            let out = render(f, &f.rec.scales(), f.rec.offset)?;
            let n = out.len() as f32;
            Some(
                out.iter()
                    .zip(&f.rec.blocks)
                    .map(|(o, b)| {
                        let a = oklab(decode3(*o));
                        let r = oklab(decode3(b.r));
                        ((a[0] - r[0]).powi(2) + (a[1] - r[1]).powi(2) + (a[2] - r[2]).powi(2))
                            .sqrt()
                    })
                    .sum::<f32>()
                    / n,
            )
        })
        .collect();
    let emulation_de = emulation.iter().sum::<f32>() / emulation.len().max(1) as f32;
    let emulation_de_max = emulation.iter().copied().fold(0.0f32, f32::max);
    eprintln!(
        "fit lightroom: the emulated finish sits {emulation_de:.4} ΔE from the real render on \
         average, {emulation_de_max:.4} at most"
    );

    // Before: the start's scales with no offset applied, so the
    // measured offset per body reads whole.
    let zero: HashMap<String, f32> = bodies.iter().map(|b| (b.clone(), 0.0)).collect();
    let x_before = space.start(&opts.start, &zero);
    let before: Vec<Option<Eval>> = all
        .par_iter()
        .map(|f| evaluate(f, &opts.start, 0.0))
        .collect();
    let cost_before = cost(&all, &space, &x_before);
    // The bodies' measured offsets: the median over each body's frames
    // of the stops the Exposure slider moved to match the export,
    // since that is where the constant goes; the median of the
    // output's offset beside it, for the report.
    let median_of = |pick: &dyn Fn(&Eval) -> f32| -> HashMap<String, f32> {
        bodies
            .iter()
            .map(|b| {
                let mut v: Vec<f32> = all
                    .iter()
                    .zip(&before)
                    .filter(|(f, e)| f.rec.camera == *b && e.is_some())
                    .map(|(_, e)| pick(e.as_ref().unwrap()))
                    .collect();
                (b.clone(), median(&mut v))
            })
            .collect()
    };
    let measured = median_of(&|e| e.matched_offset);
    let measured_output = median_of(&|e| e.offset);
    // The bands, before.
    let band_lines: Vec<BandLine> = all
        .par_iter()
        .zip(&before)
        .filter_map(|(f, e)| {
            let out = render(f, &opts.start, 0.0)?;
            Some(BandLine {
                name: f.rec.name.clone(),
                camera: f.rec.camera.clone(),
                profile: f.rec.profile.clone(),
                offset: e.as_ref()?.offset,
                bands: bands(f, &out),
            })
        })
        .collect();

    // The fit: from the measured offsets, over everything.
    let x0 = space.start(&opts.start, &measured);
    let mut rounds = Vec::new();
    let (x_after, cost_after) = if opts.scales {
        fit(&all, &space, &x0, opts.rounds, &mut |r, c, x| {
            eprintln!("fit lightroom: round {} cost {c:.5} at {x:.3?}", r + 1);
            rounds.push((r + 1, c));
        })
    } else {
        fit_offsets(&all, &space, &x0, opts.rounds)
    };
    let (scales_after, offsets_after) = space.unpack(&x_after);
    let after: Vec<Option<Eval>> = all
        .par_iter()
        .map(|f| {
            let o = offsets_after.get(&f.rec.camera).copied().unwrap_or(0.0);
            evaluate(f, &scales_after, o)
        })
        .collect();

    // The rows: by body and profile, then by body.
    let mut keys: Vec<(String, String, Option<String>)> = all
        .iter()
        .map(|f| {
            (
                f.rec.camera.clone(),
                f.rec.profile.clone(),
                f.rec.look.clone(),
            )
        })
        .collect();
    keys.sort();
    keys.dedup();
    let mut rows = Vec::new();
    let pick = |evals: &[Option<Eval>], keep: &dyn Fn(&Frame) -> bool| -> Vec<Eval> {
        all.iter()
            .zip(evals)
            .filter(|(f, e)| keep(f) && e.is_some())
            .map(|(_, e)| e.unwrap())
            .collect()
    };
    for (camera, profile, look) in keys {
        let keep = |f: &Frame| f.rec.camera == camera && f.rec.profile == profile;
        rows.push(Row {
            camera: camera.clone(),
            profile: profile.clone(),
            look: look.clone(),
            before: group(&pick(&before, &keep)),
            after: group(&pick(&after, &keep)),
        });
    }
    for camera in &bodies {
        let keep = |f: &Frame| f.rec.camera == *camera;
        rows.push(Row {
            camera: camera.clone(),
            profile: "(every profile)".into(),
            look: None,
            before: group(&pick(&before, &keep)),
            after: group(&pick(&after, &keep)),
        });
    }
    rows.push(Row {
        camera: "(every body)".into(),
        profile: "(every profile)".into(),
        look: None,
        before: group(&pick(&before, &|_| true)),
        after: group(&pick(&after, &|_| true)),
    });

    // Held out.
    let mut held_out = Vec::new();
    if opts.scales && opts.hold_out {
        let catalogs: Vec<usize> = {
            let mut c: Vec<usize> = all.iter().map(|f| f.catalog).collect();
            c.sort();
            c.dedup();
            c
        };
        if catalogs.len() > 1 {
            for c in catalogs {
                held_out.push(hold(
                    &all,
                    &space,
                    &x0,
                    &x_before,
                    &x_after,
                    opts.rounds,
                    "catalog",
                    &format!("catalog folder {}", c + 1),
                    &|f| f.catalog == c,
                ));
            }
        }
        if bodies.len() > 1 {
            for b in &bodies {
                held_out.push(hold(
                    &all,
                    &space,
                    &x0,
                    &x_before,
                    &x_after,
                    opts.rounds,
                    "body",
                    b,
                    &|f| f.rec.camera == *b,
                ));
            }
        }
        let mut clusters: Vec<String> = all.iter().map(|f| f.cluster.clone()).collect();
        clusters.sort();
        clusters.dedup();
        if clusters.len() > 1 {
            for c in &clusters {
                held_out.push(hold(
                    &all,
                    &space,
                    &x0,
                    &x_before,
                    &x_after,
                    opts.rounds,
                    "edit",
                    c,
                    &|f| f.cluster == *c,
                ));
            }
        }
    }

    // The profiles.
    let profiles: Vec<Profile> = if opts.scales {
        (space.bodies.len()..space.len())
            .map(|i| {
                let (lo, hi, _) = space.bounds(i);
                let shipped = x_before[i];
                let mut points: Vec<f32> = [0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0]
                    .into_iter()
                    .map(|k| (shipped * k).clamp(lo, hi))
                    .collect();
                points.push(x_after[i]);
                points.sort_by(f32::total_cmp);
                points.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
                let at: Vec<(f32, f32)> = points
                    .into_iter()
                    .map(|p| {
                        let mut y = x_after.clone();
                        y[i] = p;
                        (p, cost(&all, &space, &y))
                    })
                    .collect();
                Profile {
                    name: space.name(i),
                    at,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Report {
        catalogs: opts.dirs.len(),
        frames: all.len(),
        name_pairs_left_out: name_pairs,
        blocks: all.iter().map(|f| f.rec.blocks.len()).sum(),
        emulation_de,
        emulation_de_max,
        before_scales: scales_map(&opts.start),
        after_scales: scales_map(&scales_after),
        bodies: bodies
            .iter()
            .map(|b| {
                let applied = all
                    .iter()
                    .find(|f| f.rec.camera == *b)
                    .map_or(0.0, |f| f.rec.offset);
                (
                    b.clone(),
                    applied,
                    measured_output[b],
                    measured[b],
                    offsets_after.get(b).copied().unwrap_or(0.0),
                )
            })
            .collect(),
        cost_before,
        cost_after,
        rounds,
        rows,
        held_out,
        band_rows: band_rows(&band_lines),
        bands: band_lines,
        profiles,
        seconds: start.elapsed().as_secs_f32(),
    })
}

/// The offsets alone fitted, the scales at the shipped values.
fn fit_offsets(frames: &[&Frame], space: &Space, x0: &[f32], rounds: usize) -> (Vec<f32>, f32) {
    let mut x = x0.to_vec();
    let mut best = cost(frames, space, &x);
    let mut step = OFFSET_BOUNDS.2;
    for _ in 0..rounds {
        for i in 0..space.bodies.len() {
            let (lo, hi, _) = OFFSET_BOUNDS;
            let (v, fv) = golden((x[i] - step).max(lo), (x[i] + step).min(hi), EVALS, |v| {
                let mut y = x.clone();
                y[i] = v;
                cost(frames, space, &y)
            });
            if fv < best {
                x[i] = v;
                best = fv;
            }
        }
        step *= 0.5;
    }
    (x, best)
}

/// Hold the frames `held` picks out: fit on the rest from `x0`, and
/// measure the held frames under the shipped scales, under `x_all`
/// (the fit on everything) and under the fit on the rest. A held body
/// has no offset among the rest, so its offset alone is then fitted on
/// the held frames, which is said.
#[allow(clippy::too_many_arguments)]
fn hold(
    all: &[&Frame],
    space: &Space,
    x0: &[f32],
    x_before: &[f32],
    x_all: &[f32],
    rounds: usize,
    by: &str,
    held: &str,
    pick: &dyn Fn(&Frame) -> bool,
) -> HeldOut {
    let held_frames: Vec<&Frame> = all.iter().copied().filter(|f| pick(f)).collect();
    let rest: Vec<&Frame> = all.iter().copied().filter(|f| !pick(f)).collect();
    let before = cost(&held_frames, space, x_before);
    let fitted_on_all = cost(&held_frames, space, x_all);
    let (mut x, _) = fit(&rest, space, x0, rounds, &mut |_, _, _| {});
    // A body not among the rest keeps no offset from the fit: fit that
    // one number on the held frames, the scales as the rest left them.
    let mut note = String::new();
    for (i, b) in space.bodies.iter().enumerate() {
        if !rest.iter().any(|f| f.rec.camera == *b)
            && held_frames.iter().any(|f| f.rec.camera == *b)
        {
            let (lo, hi, step) = OFFSET_BOUNDS;
            let (v, _) = golden(
                (x[i] - step).max(lo),
                (x[i] + step).min(hi),
                2 * EVALS,
                |v| {
                    let mut y = x.clone();
                    y[i] = v;
                    cost(&held_frames, space, &y)
                },
            );
            x[i] = v;
            note = format!("{b}'s offset fitted on the held frames alone: {v:+.2}");
        }
    }
    let held_out = cost(&held_frames, space, &x);
    HeldOut {
        by: by.to_string(),
        held: held.to_string(),
        frames: held_frames.len(),
        before,
        fitted_on_all,
        held_out,
        note,
    }
}

fn group_cell(g: &Group) -> String {
    format!(
        "{:>3}  ΔE {:.4}  L {:+.4}  C {:+.4} ({:.4})  {:+.2} ± {:.2} stops  matched {:.4}",
        g.frames, g.de, g.dl, g.dc, g.dc_abs, g.offset, g.offset_sd, g.matched_de
    )
}

/// The report as the command line prints it.
pub(crate) fn text(r: &Report) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<13} {} frames from {} catalog folders, {} blocks; {} name pairs left out",
        "fit", r.frames, r.catalogs, r.blocks, r.name_pairs_left_out
    );
    let _ = writeln!(
        out,
        "{:<13} the emulated finish against the real render's blocks: ΔE {:.4} on average, {:.4} \
         at most",
        "emulation", r.emulation_de, r.emulation_de_max
    );
    let _ = writeln!(
        out,
        "{:<13} mean frame ΔE {:.4} at the start's scales with no offset, {:.4} fitted",
        "cost", r.cost_before, r.cost_after
    );
    for (n, c) in &r.rounds {
        let _ = writeln!(out, "{:<13} round {n}: {c:.5}", "");
    }
    for (b, applied, output, matched, fitted) in &r.bodies {
        let _ = writeln!(
            out,
            "{:<13} {b}: the measurement applied {applied:+.2}; at the start's scales the \
             median output offset is {output:+.2} and the median matched offset through the \
             slider {matched:+.2}; fitted {fitted:+.2} stops",
            "body"
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{:<16} {:>8} {:>8}", "scale", "shipped", "fitted");
    for (k, v) in &r.before_scales {
        let _ = writeln!(out, "{k:<16} {v:>8.3} {:>8.3}", r.after_scales[k]);
    }
    let _ = writeln!(out);
    for row in &r.rows {
        let _ = writeln!(
            out,
            "{:<13} {} {}{}",
            "row",
            row.camera,
            row.profile,
            row.look
                .as_deref()
                .map(|l| format!(" ({l})"))
                .unwrap_or_default()
        );
        let _ = writeln!(out, "{:<13} before {}", "", group_cell(&row.before));
        let _ = writeln!(out, "{:<13} after  {}", "", group_cell(&row.after));
    }
    if !r.held_out.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{:<13} held frames' mean ΔE: shipped / fitted on all / fitted on the rest",
            "held out"
        );
        for h in &r.held_out {
            let _ = writeln!(
                out,
                "{:<13} by {} ({} frames): {:.4} / {:.4} / {:.4}  {}{}",
                "",
                h.by,
                h.frames,
                h.before,
                h.fitted_on_all,
                h.held_out,
                h.held,
                if h.note.is_empty() {
                    String::new()
                } else {
                    format!("; {}", h.note)
                }
            );
        }
    }
    if !r.profiles.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{:<13} the cost at each scale's values with the rest held at the fit",
            "profile"
        );
        for p in &r.profiles {
            let _ = writeln!(
                out,
                "{:<16} {}",
                p.name,
                p.at.iter()
                    .map(|(v, c)| format!("{v:.3}: {c:.4}"))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<13} offset in stops at the start's scales, by the export's lightness: all; under L \
         {}; {} to {}; {} to {}; from {} (blocks)",
        "bands",
        BAND_EDGES[0],
        BAND_EDGES[0],
        BAND_EDGES[1],
        BAND_EDGES[1],
        BAND_EDGES[2],
        BAND_EDGES[2]
    );
    for b in &r.band_rows {
        let cell = |(n, o): (usize, f32)| {
            if n == 0 {
                "      -   ".to_string()
            } else {
                format!("{o:+.2} ({n:>3})")
            }
        };
        let _ = writeln!(
            out,
            "{:<13} {} {} ({} frames): {}  (median over the frames with {} blocks in the band, \
             frames)",
            "by band",
            b.camera,
            b.profile,
            b.frames,
            b.bands
                .iter()
                .map(|b| cell(*b))
                .collect::<Vec<_>>()
                .join("  "),
            BAND_FRAME_BLOCKS
        );
    }
    for b in &r.bands {
        let cell = |(n, o): (usize, f32)| {
            if n == 0 {
                "      -   ".to_string()
            } else {
                format!("{o:+.2} ({n:>3})")
            }
        };
        let _ = writeln!(
            out,
            "{:<24} {:+.2}  {}  {} {}",
            b.name,
            b.offset,
            b.bands
                .iter()
                .map(|b| cell(*b))
                .collect::<Vec<_>>()
                .join("  "),
            b.camera,
            b.profile
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of flat blocks whose export is the emulated finish of
    /// them at a known exposure: the fit finds that exposure as the
    /// body's offset, and the emulation at it has no error.
    fn frame_at(camera: &str, stops: f32, settings: &str) -> Frame {
        let blocks: Vec<Block> = (0..40)
            .map(|i| {
                let t = 0.03 + 0.012 * i as f32;
                Block {
                    s: [t, t * 0.9, t * 0.7],
                    g: Some((t / 0.18).log2()),
                    v: 0.0,
                    e: [0.0; 3],
                    r: [0.0; 3],
                }
            })
            .collect();
        let mut rec = FrameBlocks {
            name: "x".into(),
            image: 1,
            camera: camera.into(),
            profile: "Adobe Standard".into(),
            look: None,
            curve: DisplayCurve::default().key(),
            confidence: "history".into(),
            global: true,
            settings: settings.into(),
            offset: 0.0,
            scales: BTreeMap::new(),
            look_hash: None,
            range: [0.0, 2.0],
            sliders: BTreeMap::new(),
            blocks,
        };
        let mut f = Frame {
            rec: rec.clone(),
            catalog: 0,
            curve: DisplayCurve::default(),
            cluster: String::new(),
            lab_e: Vec::new(),
            lum_e: Vec::new(),
            look: None,
        };
        let out = render(&f, &Scales::DEFAULT, stops).unwrap();
        for (b, o) in rec.blocks.iter_mut().zip(&out) {
            b.e = *o;
            b.r = *o;
        }
        f.lab_e = rec.blocks.iter().map(|b| oklab(decode3(b.e))).collect();
        f.lum_e = rec
            .blocks
            .iter()
            .map(|b| luminance(decode3(b.e)).max(1e-6))
            .collect();
        f.rec = rec;
        f
    }

    #[test]
    fn a_body_s_offset_is_found_and_the_match_lands_on_it() {
        let settings = "s = { Exposure2012 = 0.5, Highlights2012 = -40, Shadows2012 = 30 }";
        let frames = [
            frame_at("Body A", -0.75, settings),
            frame_at("Body A", -0.75, settings),
        ];
        let all: Vec<&Frame> = frames.iter().collect();
        let space = Space {
            bodies: vec!["Body A".into()],
        };
        // Before: the export is three quarters of a stop darker. The
        // offset read off the output is the right way and about the
        // size, not the stops themselves: the tone shifts and the
        // display curve sit between the slider and the picture. The
        // match through the slider lands on the stops.
        let e = evaluate(&frames[0], &Scales::DEFAULT, 0.0).unwrap();
        assert!(e.offset < -0.3 && e.offset > -1.2, "{e:?}");
        assert!(e.de > 0.03, "{e:?}");
        assert!(e.matched_de < 0.003, "{e:?}");
        assert!((e.matched_offset + 0.75).abs() < 0.1, "{e:?}");
        // Fitted: the offset is the exposure the export was made at.
        let x0 = space.start(&Scales::DEFAULT, &HashMap::new());
        let (x, c) = fit_offsets(&all, &space, &x0, 4);
        assert!((x[0] + 0.75).abs() < 0.03, "{x:?}");
        assert!(c < 0.003, "{c}");
        // The emulation at the right offset is the export.
        let e = evaluate(&frames[0], &Scales::DEFAULT, x[0]).unwrap();
        assert!(e.de < 0.003, "{e:?}");
    }

    #[test]
    fn a_scale_is_found_from_frames_made_under_it() {
        let settings = "s = { Exposure2012 = 0.2, Highlights2012 = -80, Shadows2012 = 60 }";
        let made = Scales {
            highlights: 1.2,
            ..Scales::DEFAULT
        };
        let mut frames = [frame_at("Body A", 0.0, settings)];
        for f in &mut frames {
            let out = render(f, &made, 0.0).unwrap();
            for (b, o) in f.rec.blocks.iter_mut().zip(&out) {
                b.e = *o;
            }
            f.lab_e = f.rec.blocks.iter().map(|b| oklab(decode3(b.e))).collect();
            f.lum_e = f
                .rec
                .blocks
                .iter()
                .map(|b| luminance(decode3(b.e)).max(1e-6))
                .collect();
        }
        let all: Vec<&Frame> = frames.iter().collect();
        let space = Space {
            bodies: vec!["Body A".into()],
        };
        let x0 = space.start(&Scales::DEFAULT, &HashMap::new());
        let before = cost(&all, &space, &x0);
        let (x, after) = fit(&all, &space, &x0, 3, &mut |_, _, _| {});
        let (s, _) = space.unpack(&x);
        assert!(after < before / 3.0, "{before} -> {after}");
        assert!((s.highlights - 1.2).abs() < 0.25, "{s:?}");
    }

    /// A body with an offset renders darker by it: the export made at
    /// no offset reads brighter than the blocks rendered at the body's
    /// offset by about that many stops, and the match lands back on
    /// zero.
    #[test]
    fn the_body_s_offset_darkens_the_rendered_blocks() {
        let camera = "Fujifilm GFX 100S II";
        let want = lightroom::body_offset(Some(camera));
        assert!(want < -0.1, "the GFX takes an offset: {want}");
        let settings = "s = { Exposure2012 = 0.3, Highlights2012 = -30 }";
        let f = frame_at(camera, 0.0, settings);
        let e = evaluate(&f, &Scales::DEFAULT, want).unwrap();
        // Darker, by about the offset; the match through the slider
        // takes it back.
        assert!(e.offset > 0.3 && e.offset < 1.5, "{e:?}");
        assert!((e.matched_offset + want).abs() < 0.1, "{e:?}");
        // 0.005 where it was 0.003: the match lands the blocks' median,
        // and under Highlights at -30 the slider's stop is not a stop
        // at every block, so at the GFX's offset since its baseline
        // moved it stops a few hundredths of a stop short and leaves
        // about 0.0034 of ΔE; at the earlier -0.90 it left under 0.003.
        assert!(e.matched_de < 0.005, "{e:?}");
        let none = evaluate(&f, &Scales::DEFAULT, lightroom::body_offset(None)).unwrap();
        assert!(none.de < 0.003, "{none:?}");
    }

    /// A block record keeps the settings without the names given to
    /// masks and corrections, and the scales it was measured under.
    #[test]
    fn a_record_holds_no_names_and_its_scales() {
        let settings = "s = { Exposure2012 = 0.5,\n\
            MaskGroupBasedCorrections = { { CorrectionActive = true,\n\
            CorrectionName = \"Someone \\\"Nick\\\" Name\",\n\
            CorrectionMasks = { { MaskName = \"Sky 1\",\n\
            What = \"Mask/Image\" } },\n\
            What = \"Correction\" } },\n\
            ToneCurveName2012 = \"Custom\" }";
        let stripped = without_names(settings);
        assert!(!stripped.contains("Someone") && !stripped.contains("Nick"));
        assert!(!stripped.contains("Sky 1"));
        assert!(stripped.contains("CorrectionName = \"\","));
        assert!(stripped.contains("MaskName = \"\","));
        assert!(stripped.contains("ToneCurveName2012 = \"Custom\""));
        assert!(lightroom::read_table(&stripped, "x").is_ok());
        let f = frame_at("Body A", 0.0, settings);
        let line = FrameLine {
            name: "x".into(),
            look: None,
            offset_applied: true,
            body_offset: -0.5,
            ..FrameLine::default()
        };
        let scales = Scales {
            shadows: 0.4,
            ..Scales::DEFAULT
        };
        let rec = FrameBlocks::of(
            &line,
            settings,
            &Edit::default(),
            Kept {
                range: f.rec.range,
                blocks: f.rec.blocks.clone(),
            },
            &scales,
            Some("abc".into()),
        );
        assert!(!rec.settings.contains("Someone"));
        assert_eq!(rec.scales["shadows"], 0.4);
        assert_eq!(rec.scales().shadows, 0.4);
        assert_eq!(rec.scales().highlights, Scales::DEFAULT.highlights);
        assert_eq!(rec.offset, -0.5);
        assert_eq!(rec.look_hash.as_deref(), Some("abc"));
    }

    #[test]
    fn golden_finds_a_parabola_s_minimum() {
        let (x, f) = golden(-3.0, 5.0, 20, |v| (v - 1.3).powi(2));
        assert!((x - 1.3).abs() < 0.01, "{x}");
        assert!(f < 1e-3);
    }

    #[test]
    fn the_report_s_text_and_json_hold_the_tables() {
        let r = Report {
            frames: 2,
            before_scales: scales_map(&Scales::DEFAULT),
            after_scales: scales_map(&Scales::DEFAULT),
            bodies: vec![("Body A".into(), -1.0, -0.9, -0.92, -0.95)],
            rows: vec![Row {
                camera: "Body A".into(),
                profile: "Camera Faithful".into(),
                look: Some("Body A Faithful".into()),
                before: Group {
                    frames: 2,
                    de: 0.1,
                    ..Group::default()
                },
                after: Group {
                    frames: 2,
                    de: 0.03,
                    ..Group::default()
                },
            }],
            held_out: vec![HeldOut {
                by: "body".into(),
                held: "Body A".into(),
                frames: 2,
                before: 0.1,
                fitted_on_all: 0.03,
                held_out: 0.04,
                note: String::new(),
            }],
            ..Report::default()
        };
        let t = text(&r);
        assert!(t.contains("Body A Camera Faithful (Body A Faithful)"));
        assert!(t.contains("before   2  ΔE 0.1000"));
        assert!(t.contains("by body (2 frames): 0.1000 / 0.0300 / 0.0400"));
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        let near = |v: &serde_json::Value, want: f64| (v.as_f64().unwrap() - want).abs() < 1e-6;
        assert!(near(&v["rows"][0]["after"]["de"], 0.03), "{v}");
        assert!(near(&v["bodies"][0][4], -0.95), "{v}");
    }
}
