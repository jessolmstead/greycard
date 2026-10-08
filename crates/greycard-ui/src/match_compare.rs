//! `--match-compare OUT`: the camera match measured under each display
//! curve on the same frames, with no window and no table written.
//!
//! The match fits a look on what the display curve renders, so a look
//! fitted under AgX and one fitted under per channel are fitted on
//! different pictures, and the errors their runs report are on
//! different frames whenever the runs are apart. This runs the match's
//! own pieces once over a scope: the groups and the sample the sheet
//! would take ([`camera_match::survey_library`],
//! [`camera_match::sample`]), each sampled frame developed once and
//! finished under both curves ([`camera_match::lay`],
//! [`camera_match::matched`]), and the fit ([`Model::fit`]) on each
//! curve's pairs. The exposure match is the match's own, one function
//! for both ([`camera_match::matched`]): it gives the first pass and
//! where the iteration from it ended, so the tool measures the match as
//! it was (one pass) and as it runs (converged) on the same develops.
//! What it reports, per group and curve: the error with no look, fitted
//! at each stage, held out over every frame (the sheet's figure), each
//! split into lightness and chroma and hue and by the camera's
//! lightness and chroma; the per-frame brightness one pass left, and
//! the fit on the converged match's pairs; what each costs a frame;
//! and, for a body that borrows, every donor of its style measured on
//! its frames.
//!
//! Each frame's measurement is kept under `OUT/frames` as numbers (the
//! block means, no pixels), keyed by a hash of the file's path, with
//! its size and time checked beside the key and the build that made it
//! stamped on it, so a second run of the same build over the same scope
//! redoes the analysis and not the develops (another build's records
//! are taken only when that build is named, `--match-reuse-records
//! BUILD`, and the report lists the builds its records came from).
//! Neither the records nor the report name a folder: a frame is its
//! file name, a folder a number, the library its file name and the
//! roots a count. Nothing else is written: no `.cube`, no sidecar,
//! nothing in the looks store or the library, which is opened
//! read-only.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use greycard_edit::DisplayCurve;
use greycard_match::color::{decode3, oklab};
use greycard_match::fit::LutParams;
use greycard_match::{Model, Pair, Pairs};

use crate::camera_match::{self as cm, CONVERGED, Group, MAX_STEP, PASSES, Plan, SLOPE_RANGE};

/// Bumped when what a frame's record holds changes, so an old record
/// is measured again rather than misread.
const RECORD_VERSION: u32 = 2;
/// How the match's exposure iteration steps ([`cm::iterate`]), bumped
/// when that changes: a record whose iteration is another's is measured
/// again unless it had converged, since a converged offset is the same
/// whatever the step.
const ITERATION: u32 = 2;
/// Edges of the lightness bands, Oklab L of the camera's block.
pub(crate) const L_EDGES: [f32; 4] = [0.35, 0.5, 0.65, 0.8];
/// Edges of the chroma bands, Oklab C of the camera's block.
pub(crate) const C_EDGES: [f32; 3] = [0.02, 0.05, 0.1];
const L_BANDS: usize = L_EDGES.len() + 1;
const C_BANDS: usize = C_EDGES.len() + 1;
/// The two curves, in the order every table gives them.
const CURVES: [DisplayCurve; 2] = [DisplayCurve::Channels, DisplayCurve::Agx];

/// What the command line asked for.
pub(crate) struct Options {
    pub(crate) out: PathBuf,
    pub(crate) library: PathBuf,
    pub(crate) roots: Option<Vec<PathBuf>>,
    /// Frames a group is sampled down to: the sheet's 40 unless asked.
    pub(crate) frames: usize,
    /// Groups whose look name contains any of these, and the donors
    /// they borrow from; all when empty.
    pub(crate) only: Vec<String>,
    /// Other builds whose records are taken, by [`build`]'s string.
    pub(crate) reuse: Vec<String>,
}

/// `--match-compare`, from the command line to the exit code.
pub(crate) fn headless(cli: &crate::Cli, out: &Path) -> Result<std::process::ExitCode> {
    let library = match &cli.library {
        Some(p) => p.clone(),
        None => greycard_library::Library::user_path().context("no data directory")?,
    };
    let opts = Options {
        out: out.to_path_buf(),
        library,
        roots: (!cli.roots.is_empty()).then(|| cli.roots.clone()),
        frames: cli.match_frames.unwrap_or(cm::SAMPLE),
        only: cli.match_group.clone(),
        reuse: cli.match_reuse_records.clone(),
    };
    let report = run(&opts)?;
    eprintln!(
        "match compare: {} groups, {} frames developed ({} from the record) in {:.0} s; {}",
        report.groups.len(),
        report.developed,
        report.recorded,
        report.seconds,
        opts.out.join("report.md").display()
    );
    Ok(std::process::ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------
// The measurement: one record a frame, both curves.

/// One kept block, as the record keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Block {
    pub(crate) r: [f32; 3],
    pub(crate) j: [f32; 3],
    pub(crate) row: u16,
    pub(crate) col: u16,
}

fn blocks_of(p: &Pairs) -> Vec<Block> {
    p.pairs
        .iter()
        .map(|p| Block {
            r: p.render,
            j: p.jpeg,
            row: p.row as u16,
            col: p.col as u16,
        })
        .collect()
}

/// One frame under one curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Measured {
    pub(crate) scale: f32,
    pub(crate) dy: f32,
    pub(crate) dx: f32,
    pub(crate) ncc: f32,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) total: usize,
    /// The match's first pass: the offset solved once on the pairs at
    /// the default ([`cm::Matched::once`]).
    pub(crate) offset: f32,
    /// What one pass leaves: the median of the pairs at `offset`, in
    /// stops, positive where the camera is still brighter.
    pub(crate) residual: f32,
    /// The pairs at `offset`: what the match fitted before it
    /// converged the exposure.
    pub(crate) pairs: Vec<Block>,
    /// The offset iterated until the median is within [`CONVERGED`]
    /// ([`cm::iterate`]), and the pairs there: what the match fits.
    pub(crate) offset_iterated: f32,
    pub(crate) residual_iterated: f32,
    pub(crate) passes: usize,
    pub(crate) pairs_iterated: Vec<Block>,
    /// A step of the iteration left no flat, unclipped block, so it
    /// stopped at the last offset that had some.
    #[serde(default)]
    pub(crate) iteration_lost: bool,
    /// Seconds laying the JPEG over the develop (one finish, the
    /// registration and the first pairs) and matching the exposure
    /// (every finish after it, `passes` of them), as the match spends
    /// them on a frame; zero in a record from before they were kept.
    #[serde(default)]
    pub(crate) seconds_laid: f32,
    #[serde(default)]
    pub(crate) seconds_matched: f32,
}

impl Measured {
    /// Whether the iterated match reached [`CONVERGED`].
    pub(crate) fn converged(&self) -> bool {
        self.residual_iterated.abs() <= CONVERGED
    }
}

impl Measured {
    pub(crate) fn pairs(&self, iterated: bool) -> &[Block] {
        if iterated {
            &self.pairs_iterated
        } else {
            &self.pairs
        }
    }

    fn as_pairs(&self) -> Pairs {
        Pairs {
            pairs: self
                .pairs
                .iter()
                .map(|b| Pair {
                    render: b.r,
                    jpeg: b.j,
                    row: b.row as usize,
                    col: b.col as usize,
                })
                .collect(),
            rows: self.rows,
            cols: self.cols,
            total: self.total,
        }
    }
}

/// A frame's record: both curves, or why each could not be had.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Record {
    version: u32,
    /// The build that measured it: [`build`].
    build: String,
    /// The iterated match's step, [`ITERATION`].
    iteration: u32,
    /// The file's name, and a hash of its whole path.
    pub(crate) name: String,
    key: String,
    size: u64,
    mtime: i64,
    pub(crate) lens: Option<String>,
    pub(crate) channels: Result<Measured, String>,
    pub(crate) agx: Result<Measured, String>,
    seconds: f32,
}

impl Record {
    pub(crate) fn under(&self, curve: DisplayCurve) -> Option<&Measured> {
        match curve {
            DisplayCurve::Channels => self.channels.as_ref().ok(),
            DisplayCurve::Agx => self.agx.as_ref().ok(),
        }
    }

    fn why(&self, curve: DisplayCurve) -> Option<&str> {
        match curve {
            DisplayCurve::Channels => self.channels.as_ref().err().map(String::as_str),
            DisplayCurve::Agx => self.agx.as_ref().err().map(String::as_str),
        }
    }

    /// Whether its iterated match stands under this build's step: made
    /// by it, or converged under both curves wherever there was one.
    fn iteration_stands(&self) -> bool {
        self.iteration == ITERATION
            || CURVES
                .into_iter()
                .filter_map(|c| self.under(c))
                .all(Measured::converged)
    }
}

/// This build: the version and a hash of the running binary, which
/// changes whenever anything in the develop does.
pub(crate) fn build() -> String {
    let hash = std::env::current_exe()
        .and_then(std::fs::read)
        .map(|b| blake3::hash(&b).to_hex()[..16].to_string())
        .unwrap_or_else(|_| "unknown".into());
    format!("{} {hash}", env!("CARGO_PKG_VERSION"))
}

fn stamp(path: &Path) -> (u64, i64) {
    std::fs::metadata(path)
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            (m.len(), mtime)
        })
        .unwrap_or((0, 0))
}

/// A hash of the whole path, so a record names no folder; BLAKE3,
/// which is the same from one build to the next.
fn key_of(path: &Path) -> String {
    blake3::hash(path.as_os_str().as_encoded_bytes()).to_hex()[..16].to_string()
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "frame".into(), |n| n.to_string_lossy().into_owned())
}

fn record_path(dir: &Path, name: &str, key: &str) -> PathBuf {
    dir.join(format!("{name}-{key}.json"))
}

/// The record kept for `path`, when the file has not changed since and
/// it is `build`'s or one of the builds `reuse` names, with whether it
/// was another build's.
fn recorded(dir: &Path, path: &Path, build: &str, reuse: &[String]) -> Option<(Record, bool)> {
    let key = key_of(path);
    let text = std::fs::read_to_string(record_path(dir, &name_of(path), &key)).ok()?;
    let r: Record = serde_json::from_str(&text).ok()?;
    let (size, mtime) = stamp(path);
    let other = r.build != build;
    (r.version == RECORD_VERSION
        && r.key == key
        && r.size == size
        && r.mtime == mtime
        && (!other || reuse.contains(&r.build))
        && r.iteration_stands())
    .then_some((r, other))
}

fn keep(dir: &Path, r: &Record) -> Result<()> {
    let path = record_path(dir, &r.name, &r.key);
    let part = path.with_extension("json.part");
    std::fs::write(&part, serde_json::to_string(r)?)?;
    std::fs::rename(&part, &path)?;
    Ok(())
}

/// One curve of a developed frame, laid and matched as the match does
/// it ([`cm::lay`], [`cm::matched`]): its first pass and where the
/// iteration from it ended.
fn measure_curve(
    developed: &mut crate::worker::FrameDevelop,
    jpeg: &greycard_match::Picture,
    curve: DisplayCurve,
) -> Result<Measured, String> {
    let started = Instant::now();
    let laid = cm::lay(developed, jpeg, curve)?;
    let seconds_laid = started.elapsed().as_secs_f32();
    let m = cm::matched(developed, &laid, curve)?;
    let seconds_matched = started.elapsed().as_secs_f32() - seconds_laid;
    Ok(Measured {
        scale: laid.reg.scale,
        dy: laid.reg.dy,
        dx: laid.reg.dx,
        ncc: laid.reg.ncc,
        rows: m.once_pairs.rows,
        cols: m.once_pairs.cols,
        total: m.once_pairs.total,
        offset: m.once,
        residual: m.once_residual,
        pairs: blocks_of(&m.once_pairs),
        offset_iterated: m.exposure.offset,
        residual_iterated: m.exposure.residual,
        passes: m.exposure.passes,
        pairs_iterated: blocks_of(&m.pairs),
        iteration_lost: m.exposure.lost,
        seconds_laid,
        seconds_matched,
    })
}

/// A frame developed once, finished under both curves. The develop
/// does not read the display curve; the finish does. With whether it
/// failed before either curve was reached (the develop, or the camera's
/// JPEG), which may be a read that fails once and is not kept.
fn measure(
    frame: &cm::Frame,
    lenses: Option<&greycard_lens::Database>,
    build: &str,
) -> (Record, bool) {
    let started = Instant::now();
    let (size, mtime) = stamp(&frame.path);
    let edit = cm::fit_edit(DisplayCurve::Channels);
    let both = crate::worker::FrameDevelop::develop(&frame.path, &edit, lenses)
        .and_then(|d| cm::camera_jpeg(&frame.path).map(|j| (d, j)));
    let failed_early = both.is_err();
    let (channels, agx) = match both {
        Ok((mut d, jpeg)) => (
            measure_curve(&mut d, &jpeg, DisplayCurve::Channels),
            measure_curve(&mut d, &jpeg, DisplayCurve::Agx),
        ),
        Err(e) => (Err(e.clone()), Err(e)),
    };
    let record = Record {
        version: RECORD_VERSION,
        build: build.to_string(),
        iteration: ITERATION,
        name: name_of(&frame.path),
        key: key_of(&frame.path),
        size,
        mtime,
        lens: frame.lens.clone(),
        channels,
        agx,
        seconds: started.elapsed().as_secs_f32(),
    };
    (record, failed_early)
}

// ---------------------------------------------------------------------
// The statistics.

/// Sums over blocks of the model's Oklab error against the camera's.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct Acc {
    pub(crate) n: f64,
    de: f64,
    de2: f64,
    /// Lightness: the model's minus the camera's.
    dl: f64,
    dl_abs: f64,
    dl2: f64,
    /// The a, b plane's distance, and its split into chroma (signed:
    /// the model's minus the camera's) and hue.
    dab: f64,
    dc: f64,
    dc_abs: f64,
    dh: f64,
}

/// Means of an [`Acc`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct Summary {
    pub(crate) blocks: usize,
    pub(crate) de: f32,
    pub(crate) dl_abs: f32,
    pub(crate) dl: f32,
    pub(crate) dab: f32,
    pub(crate) dc: f32,
    pub(crate) dc_abs: f32,
    pub(crate) dh: f32,
    /// Lightness's share of the squared error.
    pub(crate) l_share: f32,
}

impl Acc {
    /// One block, both sides in Oklab.
    pub(crate) fn add(&mut self, out: [f32; 3], cam: [f32; 3]) {
        let dl = (out[0] - cam[0]) as f64;
        let (da, db) = ((out[1] - cam[1]) as f64, (out[2] - cam[2]) as f64);
        let dab2 = da * da + db * db;
        let c_out = (out[1] as f64).hypot(out[2] as f64);
        let c_cam = (cam[1] as f64).hypot(cam[2] as f64);
        let dc = c_out - c_cam;
        let de2 = dl * dl + dab2;
        self.n += 1.0;
        self.de += de2.sqrt();
        self.de2 += de2;
        self.dl += dl;
        self.dl_abs += dl.abs();
        self.dl2 += dl * dl;
        self.dab += dab2.sqrt();
        self.dc += dc;
        self.dc_abs += dc.abs();
        self.dh += (dab2 - dc * dc).max(0.0).sqrt();
    }

    pub(crate) fn merge(&mut self, o: &Acc) {
        self.n += o.n;
        self.de += o.de;
        self.de2 += o.de2;
        self.dl += o.dl;
        self.dl_abs += o.dl_abs;
        self.dl2 += o.dl2;
        self.dab += o.dab;
        self.dc += o.dc;
        self.dc_abs += o.dc_abs;
        self.dh += o.dh;
    }

    pub(crate) fn summary(&self) -> Summary {
        let n = self.n.max(1.0);
        Summary {
            blocks: self.n as usize,
            de: (self.de / n) as f32,
            dl_abs: (self.dl_abs / n) as f32,
            dl: (self.dl / n) as f32,
            dab: (self.dab / n) as f32,
            dc: (self.dc / n) as f32,
            dc_abs: (self.dc_abs / n) as f32,
            dh: (self.dh / n) as f32,
            l_share: if self.de2 > 0.0 {
                (self.dl2 / self.de2) as f32
            } else {
                0.0
            },
        }
    }
}

pub(crate) fn band(v: f32, edges: &[f32]) -> usize {
    edges.iter().take_while(|&&e| v >= e).count()
}

/// An [`Acc`] over every block, and by the camera's lightness band,
/// chroma band and both.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub(crate) struct Banded {
    pub(crate) all: Acc,
    pub(crate) by_l: [Acc; L_BANDS],
    pub(crate) by_c: [Acc; C_BANDS],
    pub(crate) by_lc: [[Acc; C_BANDS]; L_BANDS],
}

impl Banded {
    /// One block: the model's output and the camera's, encoded sRGB.
    pub(crate) fn add(&mut self, out_enc: [f32; 3], cam_enc: [f32; 3]) {
        self.add_lab(oklab(decode3(out_enc)), oklab(decode3(cam_enc)));
    }

    pub(crate) fn add_lab(&mut self, out: [f32; 3], cam: [f32; 3]) {
        let l = band(cam[0], &L_EDGES);
        let c = band(cam[1].hypot(cam[2]), &C_EDGES);
        self.all.add(out, cam);
        self.by_l[l].add(out, cam);
        self.by_c[c].add(out, cam);
        self.by_lc[l][c].add(out, cam);
    }

    pub(crate) fn merge(&mut self, o: &Banded) {
        self.all.merge(&o.all);
        for (a, b) in self.by_l.iter_mut().zip(&o.by_l) {
            a.merge(b);
        }
        for (a, b) in self.by_c.iter_mut().zip(&o.by_c) {
            a.merge(b);
        }
        for (ra, rb) in self.by_lc.iter_mut().zip(&o.by_lc) {
            for (a, b) in ra.iter_mut().zip(rb) {
                a.merge(b);
            }
        }
    }
}

/// The model's first stages: the matrix alone, then with its curves,
/// then the whole.
fn stages(m: &Model) -> [Model; 3] {
    [
        Model {
            matrix: m.matrix,
            curves: None,
            lattice: None,
        },
        Model {
            matrix: m.matrix,
            curves: m.curves.clone(),
            lattice: None,
        },
        m.clone(),
    ]
}

/// One group's blocks under one curve, ready to fit.
#[derive(Debug, Default, Clone)]
pub(crate) struct Data {
    pub(crate) x: Vec<[f32; 3]>,
    pub(crate) y: Vec<[f32; 3]>,
    /// Each block's frame, an index into the group's sample.
    pub(crate) ids: Vec<usize>,
}

impl Data {
    fn frames(&self) -> Vec<usize> {
        let mut f = self.ids.clone();
        f.dedup();
        f
    }

    fn push(&mut self, id: usize, b: &Block) {
        self.x.push(b.r);
        self.y.push(b.j);
        self.ids.push(id);
    }
}

/// Which blocks a dataset takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum Blocks {
    /// Each curve's own: the match as it runs.
    Own,
    /// The frames both curves registered, and the blocks both kept.
    Common,
}

/// The data of `records` under `curve`.
pub(crate) fn data(
    records: &[&Record],
    curve: DisplayCurve,
    blocks: Blocks,
    iterated: bool,
) -> Data {
    let mut d = Data::default();
    for (i, r) in records.iter().enumerate() {
        let Some(m) = r.under(curve) else { continue };
        match blocks {
            Blocks::Own => {
                for b in m.pairs(iterated) {
                    d.push(i, b);
                }
            }
            Blocks::Common => {
                let other = CURVES.into_iter().find(|c| *c != curve).unwrap();
                let Some(o) = r.under(other) else { continue };
                let theirs: std::collections::HashSet<(u16, u16)> =
                    o.pairs(iterated).iter().map(|b| (b.row, b.col)).collect();
                for b in m.pairs(iterated) {
                    if theirs.contains(&(b.row, b.col)) {
                        d.push(i, b);
                    }
                }
            }
        }
    }
    d
}

/// One frame's figures under one curve.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct FrameLine {
    pub(crate) frame: usize,
    pub(crate) blocks: usize,
    pub(crate) no_look: f32,
    pub(crate) fitted: f32,
    pub(crate) held_out: f32,
    /// The held-out output's lightness minus the camera's, mean and
    /// mean absolute, and the a, b distance.
    pub(crate) held_dl: f32,
    pub(crate) held_dl_abs: f32,
    pub(crate) held_dab: f32,
    /// The held-out mean ΔL in each of the camera's lightness bands;
    /// NaN where the frame has fewer than [`BAND_MIN`] blocks there.
    pub(crate) held_dl_by_l: [f32; L_BANDS],
}

/// Blocks a frame needs in a band for its mean there to count.
const BAND_MIN: f64 = 20.0;

/// What one fit shows on one dataset.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Evaluation {
    pub(crate) frames: usize,
    pub(crate) blocks: usize,
    pub(crate) no_look: Banded,
    /// Fitted on every frame and measured on them, per stage.
    pub(crate) fitted: [Banded; 3],
    /// Every frame held out once: the mean of the frames' means (the
    /// sheet's figure, [`cm::fit_group`], at the last stage), and per
    /// block, per stage.
    pub(crate) held_all: [f32; 3],
    pub(crate) held_all_blocks: [Banded; 3],
    /// The held-out error with each frame's mean lightness error taken
    /// off its blocks first: what a perfect per-frame brightness match
    /// would leave, over blocks.
    pub(crate) held_lightness_oracle: Summary,
    /// The same with each frame's lightness error taken off as a line
    /// in the camera's lightness, an offset and a slope: what a
    /// per-frame brightness and contrast match would leave.
    pub(crate) held_tone_oracle: Summary,
    /// Over frames, the mean and spread of that line's slope (ΔL per
    /// unit of the camera's L).
    pub(crate) held_tone_slope: (f32, f32),
    /// Spread across frames of the held-out mean lightness error, and
    /// of the same within each lightness band.
    pub(crate) held_dl_spread: f32,
    pub(crate) held_dl_spread_by_l: [f32; L_BANDS],
    pub(crate) per_frame: Vec<FrameLine>,
    /// The error no table on the render's color could remove: blocks
    /// sharing a cell of the 33³ lattice by their render, each against
    /// its cell's mean camera color; over cells with four blocks or
    /// more.
    pub(crate) floor: Banded,
    /// The same with a cell taken within one frame: the frame's own
    /// blocks that share a render color, against their mean. What one
    /// frame's camera separates that the render does not.
    pub(crate) floor_within: Banded,
    /// How much the fitted model's lightness moves with its input's
    /// chroma ([`Model::coupling`]): the mean and 95th percentile.
    pub(crate) coupling: [f32; 2],
    #[serde(skip)]
    pub(crate) model: Option<Model>,
}

/// Fit `d` and measure it every way, every frame held out once.
pub(crate) fn evaluate(d: &Data) -> Option<Evaluation> {
    let frames = d.frames();
    if frames.len() < 3 {
        return None;
    }
    let params = LutParams::default();
    let model = Model::fit(&d.x, &d.y, params);
    let st = stages(&model);
    let mut no_look = Banded::default();
    let mut fitted: [Banded; 3] = Default::default();
    for (x, y) in d.x.iter().zip(&d.y) {
        no_look.add(*x, *y);
        for (s, f) in st.iter().zip(fitted.iter_mut()) {
            f.add(s.apply(*x), *y);
        }
    }
    // Held out: per frame, the three stages' outputs on its blocks.
    let by_frame: HashMap<usize, Vec<usize>> =
        d.ids
            .iter()
            .enumerate()
            .fold(HashMap::new(), |mut m, (k, &f)| {
                m.entry(f).or_insert_with(Vec::new).push(k);
                m
            });
    let hold = |f: usize| -> Vec<[[f32; 3]; 3]> {
        let (mut tx, mut ty) = (Vec::new(), Vec::new());
        for ((x, y), &id) in d.x.iter().zip(&d.y).zip(&d.ids) {
            if id != f {
                tx.push(*x);
                ty.push(*y);
            }
        }
        let m = Model::fit(&tx, &ty, params);
        let st = stages(&m);
        by_frame[&f]
            .iter()
            .map(|&k| [0, 1, 2].map(|s| st[s].apply(d.x[k])))
            .collect()
    };
    let outs: Vec<(usize, Vec<[[f32; 3]; 3]>)> = frames.par_iter().map(|&f| (f, hold(f))).collect();
    let outs: HashMap<usize, Vec<[[f32; 3]; 3]>> = outs.into_iter().collect();
    let frame_mean = |f: usize, s: usize| -> f32 {
        let ks = &by_frame[&f];
        ks.iter()
            .zip(&outs[&f])
            .map(|(&k, o)| greycard_match::color::delta_e(decode3(o[s]), decode3(d.y[k])))
            .sum::<f32>()
            / ks.len().max(1) as f32
    };
    let held_all = [0, 1, 2]
        .map(|s| frames.iter().map(|&f| frame_mean(f, s)).sum::<f32>() / frames.len() as f32);
    let mut held_all_blocks: [Banded; 3] = Default::default();
    let mut oracle = Acc::default();
    let mut tone_oracle = Acc::default();
    let mut slopes = Vec::new();
    let mut per_frame = Vec::new();
    let mut dls = Vec::new();
    for &f in &frames {
        let ks = &by_frame[&f];
        let mut nl = Acc::default();
        let mut fit = Acc::default();
        let mut held = Acc::default();
        let mut held_l = [Acc::default(); L_BANDS];
        let mut labs = Vec::with_capacity(ks.len());
        for (j, &k) in ks.iter().enumerate() {
            let cam = oklab(decode3(d.y[k]));
            nl.add(oklab(decode3(d.x[k])), cam);
            fit.add(oklab(decode3(model.apply(d.x[k]))), cam);
            if let Some(o) = outs.get(&f) {
                for s in 0..3 {
                    held_all_blocks[s].add(o[j][s], d.y[k]);
                }
                let out = oklab(decode3(o[j][2]));
                held.add(out, cam);
                held_l[band(cam[0], &L_EDGES)].add(out, cam);
                labs.push((out, cam));
            }
        }
        let hs = held.summary();
        if outs.contains_key(&f) {
            dls.push(hs.dl);
            let (a, b) = line(&labs);
            slopes.push(b);
            for (out, cam) in labs {
                oracle.add([out[0] - hs.dl, out[1], out[2]], cam);
                tone_oracle.add([out[0] - (a + b * cam[0]), out[1], out[2]], cam);
            }
        }
        per_frame.push(FrameLine {
            frame: f,
            blocks: ks.len(),
            no_look: nl.summary().de,
            fitted: fit.summary().de,
            held_out: if outs.contains_key(&f) {
                hs.de
            } else {
                f32::NAN
            },
            held_dl: if outs.contains_key(&f) {
                hs.dl
            } else {
                f32::NAN
            },
            held_dl_abs: if outs.contains_key(&f) {
                hs.dl_abs
            } else {
                f32::NAN
            },
            held_dab: if outs.contains_key(&f) {
                hs.dab
            } else {
                f32::NAN
            },
            held_dl_by_l: held_l.map(|a| {
                if a.n >= BAND_MIN {
                    a.summary().dl
                } else {
                    f32::NAN
                }
            }),
        });
    }
    let mean_dl = dls.iter().sum::<f32>() / dls.len().max(1) as f32;
    let held_dl_spread =
        (dls.iter().map(|v| (v - mean_dl).powi(2)).sum::<f32>() / dls.len().max(1) as f32).sqrt();
    let held_dl_spread_by_l: [f32; L_BANDS] = std::array::from_fn(|b| {
        let v: Vec<f32> = per_frame
            .iter()
            .map(|f| f.held_dl_by_l[b])
            .filter(|v| v.is_finite())
            .collect();
        spread(&v).1
    });
    Some(Evaluation {
        frames: frames.len(),
        blocks: d.x.len(),
        no_look,
        fitted,
        held_all,
        held_all_blocks,
        held_lightness_oracle: oracle.summary(),
        held_tone_oracle: tone_oracle.summary(),
        held_tone_slope: spread(&slopes),
        held_dl_spread,
        held_dl_spread_by_l,
        per_frame,
        floor: floor(d, false),
        floor_within: floor(d, true),
        coupling: {
            let c = model.coupling();
            [c.mean, c.p95]
        },
        model: Some(model),
    })
}

/// The least-squares line of the lightness error on the camera's
/// lightness over one frame's blocks: offset and slope.
pub(crate) fn line(labs: &[([f32; 3], [f32; 3])]) -> (f32, f32) {
    let n = labs.len() as f64;
    if n < 2.0 {
        return (0.0, 0.0);
    }
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (out, cam) in labs {
        let (x, y) = (cam[0] as f64, (out[0] - cam[0]) as f64);
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let var = sxx - sx * sx / n;
    if var <= 1e-12 {
        return ((sy / n) as f32, 0.0);
    }
    let b = (sxy - sx * sy / n) / var;
    (((sy - b * sx) / n) as f32, b as f32)
}

/// The within-cell spread of the camera's color among blocks whose
/// render falls in one cell of the lattice: the part of the error no
/// per-pixel table on the render could take away, at the table's own
/// resolution. Across every frame, or `within` one frame at a time.
pub(crate) fn floor(d: &Data, within: bool) -> Banded {
    let n = (greycard_match::LUT_SIZE - 1) as f32;
    let key = |x: [f32; 3], id: usize| {
        let c = x.map(|v| (v.clamp(0.0, 1.0) * n).round() as u32);
        (if within { id } else { 0 }, c)
    };
    let mut cells: HashMap<(usize, [u32; 3]), (usize, [f64; 3])> = HashMap::new();
    let labs: Vec<[f32; 3]> = d.y.iter().map(|y| oklab(decode3(*y))).collect();
    for ((x, lab), &id) in d.x.iter().zip(&labs).zip(&d.ids) {
        let e = cells.entry(key(*x, id)).or_insert((0, [0.0; 3]));
        e.0 += 1;
        for (sum, v) in e.1.iter_mut().zip(lab) {
            *sum += *v as f64;
        }
    }
    let mut out = Banded::default();
    for ((x, lab), &id) in d.x.iter().zip(&labs).zip(&d.ids) {
        let (count, sum) = cells[&key(*x, id)];
        if count < 4 {
            continue;
        }
        let mean = sum.map(|s| (s / count as f64) as f32);
        out.add_lab(mean, *lab);
    }
    out
}

// ---------------------------------------------------------------------
// The run.

/// One group under one curve.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CurveReport {
    pub(crate) curve: String,
    /// Frames kept under this curve, and the dropped with the reason.
    pub(crate) kept: usize,
    pub(crate) dropped: Vec<(String, String)>,
    /// The match as it was: each curve's own blocks, one exposure pass.
    pub(crate) own: Option<Evaluation>,
    /// Both curves' frames and blocks in common.
    pub(crate) common: Option<Evaluation>,
    /// The exposure match iterated to convergence, own blocks (the
    /// match as it runs) and common.
    pub(crate) iterated: Option<Evaluation>,
    pub(crate) iterated_common: Option<Evaluation>,
    /// What the sheet reports for this group, by the sheet's own
    /// function ([`cm::fit_group`]) on the converged own blocks: fitted,
    /// held out over every frame, and the seconds that call took.
    pub(crate) sheet: Option<[f32; 3]>,
    /// Per frame, in the sample's order: offset, one-pass residual,
    /// iterated offset and passes.
    pub(crate) exposure: Vec<Option<[f32; 4]>>,
    /// Corner minus center of the lightness the fitted look leaves,
    /// per lens.
    pub(crate) radial: Vec<(String, f32, usize)>,
}

/// A donor's table on a borrower's frames.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DonorLine {
    pub(crate) donor: String,
    pub(crate) curve: String,
    pub(crate) donor_frames: usize,
    pub(crate) chosen_by_plan: bool,
    pub(crate) on_borrower: Summary,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GroupReport {
    pub(crate) name: String,
    pub(crate) camera: String,
    pub(crate) style: String,
    pub(crate) in_scope: usize,
    pub(crate) fixed: usize,
    /// The sample, each frame its folder's number in this run and its
    /// file name.
    pub(crate) sampled: Vec<(usize, String)>,
    pub(crate) plan: String,
    /// Frames in the sample that are another sampled frame's copy (one
    /// file in two places), left out of the analysis so a frame held
    /// out is not also fitted.
    pub(crate) duplicates: usize,
    pub(crate) curves: Vec<CurveReport>,
    /// For a borrower: no look on its frames, per curve, and every
    /// donor's table.
    pub(crate) no_look: Vec<(String, Summary)>,
    pub(crate) donors: Vec<DonorLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Report {
    /// The library's file name, and how many roots, how many of them
    /// not there: no folder is named.
    pub(crate) library: String,
    pub(crate) roots: usize,
    pub(crate) unreachable_roots: usize,
    pub(crate) build: String,
    pub(crate) frames_per_group: usize,
    pub(crate) developed: usize,
    pub(crate) recorded: usize,
    /// Of the records taken, how many another build made.
    pub(crate) recorded_other_build: usize,
    /// Every build the analysed records came from, with how many.
    pub(crate) builds: Vec<(String, usize)>,
    /// Frames whose develop or camera JPEG failed, not kept.
    pub(crate) not_kept: usize,
    /// How the iterated exposure match ended, per curve, over the
    /// fitted groups' frames.
    pub(crate) convergence: Vec<(String, Convergence)>,
    pub(crate) develop_seconds: f64,
    /// The mean seconds a frame's develop and camera JPEG took, over
    /// the fitted groups' frames timed.
    pub(crate) develop_per_frame: f32,
    pub(crate) seconds: f64,
    pub(crate) l_edges: Vec<f32>,
    pub(crate) c_edges: Vec<f32>,
    pub(crate) groups: Vec<GroupReport>,
    /// Pooled over every fitted group, common blocks, held out over
    /// every frame, with the table: per curve.
    pub(crate) pooled_common_held: Vec<(String, Banded)>,
    pub(crate) pooled_own_held: Vec<(String, Banded)>,
    pub(crate) pooled_common_floor: Vec<(String, Banded)>,
    pub(crate) pooled_common_floor_within: Vec<(String, Banded)>,
    pub(crate) pooled_common_iter_floor: Vec<(String, Banded)>,
    pub(crate) pooled_common_iter_floor_within: Vec<(String, Banded)>,
    pub(crate) pooled_common_iter_held: Vec<(String, Banded)>,
    pub(crate) pooled_common_no_look: Vec<(String, Banded)>,
    pub(crate) pooled_common_fitted: Vec<(String, Banded)>,
    /// Over the fitted groups' frames both curves registered, the
    /// blocks by the camera's lightness band: kept under per channel
    /// only, under both, under AgX only.
    pub(crate) kept_by_l: [[usize; 3]; L_BANDS],
}

/// Where the iterated exposure match ended over a set of frames.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub(crate) struct Convergence {
    pub(crate) frames: usize,
    /// Stopped at [`PASSES`] without converging.
    pub(crate) at_cap: usize,
    pub(crate) over_005: usize,
    pub(crate) over_002: usize,
    pub(crate) largest: f32,
    /// Stopped where a step left no block.
    pub(crate) lost: usize,
    /// Frames whose iteration is this build's step, [`ITERATION`], and
    /// their mean finishes: an older step's converged records stand
    /// but their finishes are another step's count.
    pub(crate) this_step: usize,
    pub(crate) mean_passes: f32,
    /// Over the frames timed: the mean seconds laying the
    /// JPEG over the develop, matching the exposure (every finish after
    /// the lay), and one of those finishes. One pass cost the lay and
    /// one finish; the converged match costs the lay and the matching.
    pub(crate) timed: usize,
    pub(crate) mean_laid: f32,
    pub(crate) mean_matched: f32,
    pub(crate) mean_finish: f32,
}

pub(crate) fn convergence<'a>(of: impl Iterator<Item = (&'a Measured, u32)>) -> Convergence {
    let mut c = Convergence::default();
    let mut passes = 0usize;
    for (m, iteration) in of {
        let miss = m.residual_iterated.abs();
        c.frames += 1;
        if iteration == ITERATION {
            c.this_step += 1;
            passes += m.passes;
        }
        c.at_cap += usize::from(m.passes >= PASSES && miss > CONVERGED);
        c.over_005 += usize::from(miss > 0.05);
        c.over_002 += usize::from(miss > 0.02);
        c.lost += usize::from(m.iteration_lost);
        c.largest = c.largest.max(miss);
        if m.seconds_matched > 0.0 {
            c.timed += 1;
            c.mean_laid += m.seconds_laid;
            c.mean_matched += m.seconds_matched;
            c.mean_finish += m.seconds_matched / m.passes.max(1) as f32;
        }
    }
    c.mean_passes = passes as f32 / c.this_step.max(1) as f32;
    let timed = c.timed.max(1) as f32;
    c.mean_laid /= timed;
    c.mean_matched /= timed;
    c.mean_finish /= timed;
    c
}

/// Blocks kept by per channel only, both, AgX only, by the camera's
/// lightness band.
pub(crate) fn kept_by_l(records: &[&Record], iterated: bool) -> [[usize; 3]; L_BANDS] {
    let mut out = [[0usize; 3]; L_BANDS];
    for r in records {
        let (Some(p), Some(a)) = (r.under(DisplayCurve::Channels), r.under(DisplayCurve::Agx))
        else {
            continue;
        };
        let cells = |m: &Measured| -> HashMap<(u16, u16), [f32; 3]> {
            m.pairs(iterated)
                .iter()
                .map(|b| ((b.row, b.col), b.j))
                .collect()
        };
        let (pc, agx) = (cells(p), cells(a));
        for (k, j) in &pc {
            let l = band(oklab(decode3(*j))[0], &L_EDGES);
            out[l][if agx.contains_key(k) { 1 } else { 0 }] += 1;
        }
        for (k, j) in &agx {
            if !pc.contains_key(k) {
                out[band(oklab(decode3(*j))[0], &L_EDGES)][2] += 1;
            }
        }
    }
    out
}

/// The records with a second copy of a file (its name and size)
/// left out, and how many were.
fn distinct<'a>(records: impl Iterator<Item = &'a Record>) -> (Vec<&'a Record>, usize) {
    let mut seen = std::collections::HashSet::new();
    let (mut out, mut twins) = (Vec::new(), 0);
    for r in records {
        if seen.insert((r.name.clone(), r.size)) {
            out.push(r);
        } else {
            twins += 1;
        }
    }
    (out, twins)
}

/// The groups whose look name contains any of `only`, with the donors
/// their plans borrow from, and the plans with the donors' indices
/// moved to the narrowed list; everything when `only` is empty.
pub(crate) fn narrowed(
    groups: Vec<Group>,
    plans: Vec<Plan>,
    only: &[String],
) -> (Vec<Group>, Vec<Plan>) {
    if only.is_empty() {
        return (groups, plans);
    }
    let named: Vec<bool> = groups
        .iter()
        .map(|g| only.iter().any(|o| g.name().contains(o.as_str())))
        .collect();
    let mut keep = named.clone();
    for (i, p) in plans.iter().enumerate() {
        if let (true, Plan::Borrow { donor, .. }) = (named[i], p) {
            keep[*donor] = true;
        }
    }
    let at: HashMap<usize, usize> = (0..groups.len())
        .filter(|&i| keep[i])
        .enumerate()
        .map(|(new, old)| (old, new))
        .collect();
    let plans = plans
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, p)| match p {
            Plan::Borrow {
                from,
                donor,
                replaces,
            } => Plan::Borrow {
                from,
                donor: at[&donor],
                replaces,
            },
            p => p,
        })
        .collect();
    let groups = groups
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, g)| g)
        .collect();
    (groups, plans)
}

fn plan_word(p: &Plan) -> String {
    match p {
        Plan::Fit { .. } => "fit".into(),
        Plan::Borrow { from, .. } => format!("borrow from {from}"),
        Plan::Donor => "donor".into(),
        Plan::Skip(why) => format!("skip: {why}"),
    }
}

fn curve_word(c: DisplayCurve) -> String {
    c.name().to_string()
}

pub(crate) fn run(opts: &Options) -> Result<Report> {
    let started = Instant::now();
    let roots = match &opts.roots {
        Some(r) => r.clone(),
        None => {
            let file = greycard_library::Roots::path_beside(&opts.library);
            greycard_library::Roots::load(&file)
                .with_context(|| format!("reading {}", file.display()))?
                .list()
                .to_vec()
        }
    };
    anyhow::ensure!(!roots.is_empty(), "the library has no roots");
    let unreachable_roots = roots.iter().filter(|r| !r.is_dir()).count();
    for r in roots.iter().filter(|r| !r.is_dir()) {
        eprintln!(
            "match compare: {} is not there; its frames will not develop",
            r.display()
        );
    }
    let build = build();
    let lib = greycard_library::Library::open_read_only(&opts.library)
        .with_context(|| format!("opening {}", opts.library.display()))?;
    let survey = cm::survey_library(&lib, Some(&roots))?;
    drop(lib);
    // What a fresh run over this scope would do, with nothing in the
    // store: who fits, who borrows from whom. Planned over every group,
    // so narrowing to some does not change a borrower's donor.
    let all = survey.groups;
    let all_plans = cm::plan_chosen(&all, &HashMap::new(), DisplayCurve::Channels, false, &[]);
    let (groups, plans) = narrowed(all, all_plans, &opts.only);
    let samples: Vec<Vec<cm::Frame>> = groups
        .iter()
        .map(|g| {
            let mut s = cm::sample(&g.frames, opts.frames);
            s.truncate(opts.frames);
            s
        })
        .collect();
    let wanted: Vec<bool> = plans
        .iter()
        .map(|p| matches!(p, Plan::Fit { .. } | Plan::Borrow { .. }))
        .collect();

    // Develop: every sampled frame of a group that fits or borrows,
    // once, in one pass.
    let dir = opts.out.join("frames");
    std::fs::create_dir_all(&dir)?;
    let lenses = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| db);
    if lenses.is_none() {
        eprintln!("match compare: no lens database; the develop runs without lens corrections");
    }
    let todo: Vec<&cm::Frame> = samples
        .iter()
        .zip(&wanted)
        .filter(|(_, w)| **w)
        .flat_map(|(s, _)| s.iter())
        .collect();
    let mut records: HashMap<PathBuf, Record> = HashMap::new();
    let (mut developed, mut recorded_n, mut other_build, mut not_kept) = (0, 0, 0, 0);
    let develop_started = Instant::now();
    for (k, frame) in todo.iter().enumerate() {
        if records.contains_key(&frame.path) {
            continue;
        }
        if let Some((r, other)) = recorded(&dir, &frame.path, &build, &opts.reuse) {
            recorded_n += 1;
            other_build += usize::from(other);
            records.insert(frame.path.clone(), r);
            continue;
        }
        let (r, failed_early) = measure(frame, lenses.as_ref(), &build);
        developed += 1;
        if failed_early {
            // Perhaps a read that fails once: measured again next run.
            not_kept += 1;
        } else {
            keep(&dir, &r)?;
        }
        let per = develop_started.elapsed().as_secs_f64() / developed as f64;
        let left = (todo.len() - k - 1) as f64 * per;
        let said = |c: DisplayCurve| match r.under(c) {
            Some(m) => format!("{} blocks {:+.2} st", m.pairs.len(), m.offset),
            None => format!("dropped ({})", r.why(c).unwrap_or("")),
        };
        eprintln!(
            "[{}/{}] {:.1} s, about {:.0} min left: per channel {}; AgX {}",
            k + 1,
            todo.len(),
            r.seconds,
            left / 60.0,
            said(DisplayCurve::Channels),
            said(DisplayCurve::Agx),
        );
        tracing::info!("match compare: {} in {:.1} s", r.name, r.seconds);
        records.insert(frame.path.clone(), r);
    }
    let develop_seconds = develop_started.elapsed().as_secs_f64();
    let mut builds: BTreeMap<String, usize> = BTreeMap::new();
    for r in records.values() {
        *builds.entry(r.build.clone()).or_default() += 1;
    }
    let builds: Vec<(String, usize)> = builds.into_iter().collect();
    if builds.len() > 1 {
        eprintln!(
            "match compare: the records come from {} builds: {}",
            builds.len(),
            builds
                .iter()
                .map(|(b, n)| format!("{n} from {b}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Fit and measure each group under each curve.
    let mut reports: Vec<GroupReport> = Vec::new();
    let mut models: HashMap<(usize, usize), (Model, usize)> = HashMap::new();
    let mut folders: HashMap<PathBuf, usize> = HashMap::new();
    let mut kept_l = [[0usize; 3]; L_BANDS];
    let mut measured_fits: Vec<&Record> = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let (recs, duplicates) = if wanted[gi] {
            distinct(samples[gi].iter().filter_map(|f| records.get(&f.path)))
        } else {
            (Vec::new(), 0)
        };
        let fits = matches!(plans[gi], Plan::Fit { .. });
        if fits {
            measured_fits.extend(recs.iter().copied());
            for (t, k) in kept_l.iter_mut().zip(kept_by_l(&recs, false)) {
                for i in 0..3 {
                    t[i] += k[i];
                }
            }
        }
        let mut curves = Vec::new();
        for (ci, curve) in CURVES.into_iter().enumerate() {
            if recs.is_empty() {
                break;
            }
            eprintln!("match compare: fitting {} under {}", g.name(), curve.name());
            let own = data(&recs, curve, Blocks::Own, false);
            let kept = own.frames().len();
            let dropped = recs
                .iter()
                .filter_map(|r| r.why(curve).map(|w| (r.name.clone(), w.to_string())))
                .collect();
            let own_e = fits.then(|| evaluate(&own)).flatten();
            let common_e = fits
                .then(|| evaluate(&data(&recs, curve, Blocks::Common, false)))
                .flatten();
            let iter_e = fits
                .then(|| evaluate(&data(&recs, curve, Blocks::Own, true)))
                .flatten();
            let iter_common_e = fits
                .then(|| evaluate(&data(&recs, curve, Blocks::Common, true)))
                .flatten();
            let sheet = fits.then(|| {
                let d = data(&recs, curve, Blocks::Own, true);
                let started = Instant::now();
                let f = cm::fit_group(&d.x, &d.y, &d.ids);
                [
                    f.fitted,
                    f.held_out.unwrap_or(f32::NAN),
                    started.elapsed().as_secs_f32(),
                ]
            });
            let mut radial = Vec::new();
            if let Some(m) = own_e.as_ref().and_then(|e| e.model.as_ref()) {
                models.insert((gi, ci), (m.clone(), kept));
                let mut by_lens: BTreeMap<String, Vec<Pairs>> = BTreeMap::new();
                for r in &recs {
                    if let Some(ms) = r.under(curve) {
                        let lens = r.lens.clone().unwrap_or_else(|| "an unnamed lens".into());
                        by_lens.entry(lens).or_default().push(ms.as_pairs());
                    }
                }
                for (lens, sets) in by_lens {
                    let refs: Vec<&Pairs> = sets.iter().collect();
                    if let Some(rad) = greycard_match::radial::radial(m, &refs) {
                        radial.push((lens, rad.corner_minus_center, sets.len()));
                    }
                }
            }
            curves.push(CurveReport {
                curve: curve_word(curve),
                kept,
                dropped,
                own: own_e,
                common: common_e,
                iterated: iter_e,
                iterated_common: iter_common_e,
                sheet,
                exposure: recs
                    .iter()
                    .map(|r| {
                        r.under(curve)
                            .map(|m| [m.offset, m.residual, m.offset_iterated, m.passes as f32])
                    })
                    .collect(),
                radial,
            });
        }
        reports.push(GroupReport {
            name: g.name(),
            camera: g.camera.clone(),
            style: g.style.clone(),
            in_scope: g.frames.len(),
            fixed: g.fixed(),
            sampled: samples[gi]
                .iter()
                .map(|f| {
                    let folder = f.path.parent().map(Path::to_path_buf).unwrap_or_default();
                    let next = folders.len();
                    (*folders.entry(folder).or_insert(next), name_of(&f.path))
                })
                .collect(),
            plan: plan_word(&plans[gi]),
            duplicates,
            curves,
            no_look: Vec::new(),
            donors: Vec::new(),
        });
    }

    // The borrowers: no look, and every donor of the style.
    for (gi, g) in groups.iter().enumerate() {
        let Plan::Borrow { donor: chosen, .. } = plans[gi] else {
            continue;
        };
        let (recs, _) = distinct(samples[gi].iter().filter_map(|f| records.get(&f.path)));
        for (ci, curve) in CURVES.into_iter().enumerate() {
            let own = data(&recs, curve, Blocks::Own, false);
            let mut nl = Acc::default();
            for (x, y) in own.x.iter().zip(&own.y) {
                nl.add(oklab(decode3(*x)), oklab(decode3(*y)));
            }
            reports[gi].no_look.push((curve_word(curve), nl.summary()));
            for (di, d) in groups.iter().enumerate() {
                if d.style != g.style || di == gi {
                    continue;
                }
                let Some((m, n)) = models.get(&(di, ci)) else {
                    continue;
                };
                let mut acc = Acc::default();
                for (x, y) in own.x.iter().zip(&own.y) {
                    acc.add(oklab(decode3(m.apply(*x))), oklab(decode3(*y)));
                }
                reports[gi].donors.push(DonorLine {
                    donor: d.camera.clone(),
                    curve: curve_word(curve),
                    donor_frames: *n,
                    chosen_by_plan: di == chosen,
                    on_borrower: acc.summary(),
                });
            }
        }
    }

    // Pooled over the fitted groups.
    let pool = |pick: &dyn Fn(&CurveReport) -> Option<Banded>| -> Vec<(String, Banded)> {
        CURVES
            .into_iter()
            .map(|c| {
                let mut b = Banded::default();
                for g in &reports {
                    for cr in g.curves.iter().filter(|cr| cr.curve == curve_word(c)) {
                        if let Some(x) = pick(cr) {
                            b.merge(&x);
                        }
                    }
                }
                (curve_word(c), b)
            })
            .collect()
    };
    let pooled_common_held = pool(&|cr| cr.common.as_ref().map(|e| e.held_all_blocks[2].clone()));
    let pooled_own_held = pool(&|cr| cr.own.as_ref().map(|e| e.held_all_blocks[2].clone()));
    let pooled_common_floor = pool(&|cr| cr.common.as_ref().map(|e| e.floor.clone()));
    let pooled_common_floor_within = pool(&|cr| cr.common.as_ref().map(|e| e.floor_within.clone()));
    let pooled_common_iter_floor = pool(&|cr| cr.iterated_common.as_ref().map(|e| e.floor.clone()));
    let pooled_common_iter_floor_within =
        pool(&|cr| cr.iterated_common.as_ref().map(|e| e.floor_within.clone()));
    let pooled_common_iter_held = pool(&|cr| {
        cr.iterated_common
            .as_ref()
            .map(|e| e.held_all_blocks[2].clone())
    });
    let convergence = CURVES
        .into_iter()
        .map(|c| {
            (
                curve_word(c),
                convergence(
                    measured_fits
                        .iter()
                        .filter_map(|r| r.under(c).map(|m| (m, r.iteration))),
                ),
            )
        })
        .collect();
    // The develop and the JPEG's read: a timed record's seconds less
    // what both curves spent on it. A record where a curve failed is
    // left out, since what that curve spent before it failed is not
    // kept and would be counted as the develop's.
    let develops: Vec<f32> = measured_fits
        .iter()
        .filter_map(|r| {
            let curves: Vec<&Measured> = CURVES.into_iter().filter_map(|c| r.under(c)).collect();
            (curves.len() == CURVES.len() && curves.iter().all(|m| m.seconds_matched > 0.0)).then(
                || {
                    r.seconds
                        - curves
                            .iter()
                            .map(|m| m.seconds_laid + m.seconds_matched)
                            .sum::<f32>()
                },
            )
        })
        .collect();
    let develop_per_frame = develops.iter().sum::<f32>() / develops.len().max(1) as f32;
    let pooled_common_no_look = pool(&|cr| cr.common.as_ref().map(|e| e.no_look.clone()));
    let pooled_common_fitted = pool(&|cr| cr.common.as_ref().map(|e| e.fitted[2].clone()));

    let report = Report {
        library: name_of(&opts.library),
        roots: roots.len(),
        unreachable_roots,
        build,
        frames_per_group: opts.frames,
        developed,
        recorded: recorded_n,
        recorded_other_build: other_build,
        builds,
        not_kept,
        convergence,
        develop_seconds,
        develop_per_frame,
        seconds: started.elapsed().as_secs_f64(),
        l_edges: L_EDGES.to_vec(),
        c_edges: C_EDGES.to_vec(),
        groups: reports,
        pooled_common_held,
        pooled_own_held,
        pooled_common_floor,
        pooled_common_floor_within,
        pooled_common_iter_floor,
        pooled_common_iter_floor_within,
        pooled_common_iter_held,
        pooled_common_no_look,
        pooled_common_fitted,
        kept_by_l: kept_l,
    };
    std::fs::write(
        opts.out.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    std::fs::write(opts.out.join("report.md"), markdown(&report))?;
    Ok(report)
}

// ---------------------------------------------------------------------
// The summary.

/// A table cell's text with its bars escaped, so a lens named
/// "28mm F1.4 DG HSM | Art" stays in its column.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

fn f4(v: f32) -> String {
    if v.is_finite() {
        format!("{v:.4}")
    } else {
        "-".into()
    }
}

fn band_names(edges: &[f32]) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..=edges.len() {
        out.push(match i {
            0 => format!("< {}", edges[0]),
            i if i == edges.len() => format!(">= {}", edges[i - 1]),
            i => format!("{}-{}", edges[i - 1], edges[i]),
        });
    }
    out
}

fn pc_agx(g: &GroupReport) -> Option<(&CurveReport, &CurveReport)> {
    let pc = g
        .curves
        .iter()
        .find(|c| c.curve == curve_word(DisplayCurve::Channels))?;
    let agx = g
        .curves
        .iter()
        .find(|c| c.curve == curve_word(DisplayCurve::Agx))?;
    Some((pc, agx))
}

fn spread(v: &[f32]) -> (f32, f32) {
    if v.is_empty() {
        return (f32::NAN, f32::NAN);
    }
    let m = v.iter().sum::<f32>() / v.len() as f32;
    let s = (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt();
    (m, s)
}

/// The report as markdown: the headline per group, then each test.
pub(crate) fn markdown(r: &Report) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# Camera match: per channel against AgX on the same frames\n"
    );
    let _ = writeln!(
        s,
        "{} frames developed this run, {} from the record; develop {:.0} s, all {:.0} s. \
         Up to {} frames a group, the sheet's sample. ΔE is Oklab, mean over blocks unless \
         said; \"held out (all)\" is the mean of every frame's mean, each held out of its \
         own fit. \"One pass\" is the exposure matched once, as the match did before it \
         converged; \"converged\" is the match as it runs, the exposure stepped until the \
         pairs' median is within {CONVERGED} stops of the camera's, and its fitted and \
         held-out figures are the ones the sheet reports.\n",
        r.developed, r.recorded, r.develop_seconds, r.seconds, r.frames_per_group
    );
    let _ = writeln!(
        s,
        "Build {}; {} of the records taken were another build's. {} root(s), {} not \
         reachable. {} frame(s) whose develop or camera JPEG failed were not kept.\n",
        r.build, r.recorded_other_build, r.roots, r.unreachable_roots, r.not_kept
    );
    if r.builds.len() > 1 {
        let _ = writeln!(s, "The records come from more than one build:\n");
        for (b, n) in &r.builds {
            let _ = writeln!(s, "- {n} from {b}");
        }
        let _ = writeln!(s);
    }
    let fitted: Vec<&GroupReport> = r
        .groups
        .iter()
        .filter(|g| pc_agx(g).is_some_and(|(p, _)| p.own.is_some()))
        .collect();

    let _ = writeln!(
        s,
        "## Headline: each curve's own blocks (the match as it runs)\n"
    );
    let _ = writeln!(
        s,
        "| group | frames pc / AgX | no look pc / AgX | one pass: fitted pc / AgX | one pass: held out (all) pc / AgX | converged: fitted pc / AgX | converged: held out (all) pc / AgX |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|");
    for g in &fitted {
        let (p, a) = pc_agx(g).unwrap();
        let (pe, ae) = (p.own.as_ref().unwrap(), a.own.as_ref());
        let (pi, ai) = (p.iterated.as_ref(), a.iterated.as_ref());
        let or = |e: Option<&Evaluation>, f: &dyn Fn(&Evaluation) -> f32| e.map_or(f32::NAN, f);
        let fitted = |e: &Evaluation| e.fitted[2].all.summary().de;
        let held = |e: &Evaluation| e.held_all[2];
        let _ = writeln!(
            s,
            "| {} | {} / {} | {} / {} | {} / {} | {} / {} | {} / {} | {} / {} |",
            cell(&g.name),
            pe.frames,
            ae.map_or(0, |e| e.frames),
            f4(pe.no_look.all.summary().de),
            f4(or(ae, &|e| e.no_look.all.summary().de)),
            f4(fitted(pe)),
            f4(or(ae, &fitted)),
            f4(held(pe)),
            f4(or(ae, &held)),
            f4(or(pi, &fitted)),
            f4(or(ai, &fitted)),
            f4(or(pi, &held)),
            f4(or(ai, &held)),
        );
    }

    let _ = writeln!(s, "\n## The same on common frames and blocks\n");
    let _ = writeln!(
        s,
        "Only frames both curves registered, and in each only the blocks both kept, so the \
         two fits see the same places.\n"
    );
    let _ = writeln!(
        s,
        "| group | frames | blocks | no look pc / AgX | one pass: fitted pc / AgX | one pass: held out (all) pc / AgX | converged: held out (all) pc / AgX |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|");
    for g in &fitted {
        let (p, a) = pc_agx(g).unwrap();
        let (Some(pe), Some(ae)) = (p.common.as_ref(), a.common.as_ref()) else {
            continue;
        };
        let held_it = |c: &CurveReport| {
            c.iterated_common
                .as_ref()
                .map_or(f32::NAN, |e| e.held_all[2])
        };
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} / {} | {} / {} | {} / {} | {} / {} |",
            cell(&g.name),
            pe.frames,
            pe.blocks,
            f4(pe.no_look.all.summary().de),
            f4(ae.no_look.all.summary().de),
            f4(pe.fitted[2].all.summary().de),
            f4(ae.fitted[2].all.summary().de),
            f4(pe.held_all[2]),
            f4(ae.held_all[2]),
            f4(held_it(p)),
            f4(held_it(a)),
        );
    }

    let _ = writeln!(
        s,
        "\n## Stages, own blocks: fitted / held out (all), mean of frames' means for held out\n"
    );
    let _ = writeln!(
        s,
        "| group | curve | no look | matrix | matrix+curves | with LUT |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|");
    for g in &fitted {
        for c in &g.curves {
            let Some(e) = c.own.as_ref() else { continue };
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} / {} | {} / {} | {} / {} |",
                cell(&g.name),
                c.curve,
                f4(e.no_look.all.summary().de),
                f4(e.fitted[0].all.summary().de),
                f4(e.held_all[0]),
                f4(e.fitted[1].all.summary().de),
                f4(e.held_all[1]),
                f4(e.fitted[2].all.summary().de),
                f4(e.held_all[2]),
            );
        }
    }

    let _ = writeln!(
        s,
        "\n## Lightness against chroma and hue, common blocks, held out (all), with LUT\n"
    );
    let _ = writeln!(
        s,
        "Per block: ΔE, mean |ΔL|, mean distance in the a, b plane, that split into |ΔC| \
         and ΔH, mean signed ΔC (the look's chroma minus the camera's), and lightness's \
         share of the squared error.\n"
    );
    let _ = writeln!(
        s,
        "| group | curve | ΔE | abs ΔL | Δab | abs ΔC | ΔH | signed ΔC | L share |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|");
    for g in &fitted {
        for c in &g.curves {
            let Some(e) = c.common.as_ref() else { continue };
            let m = e.held_all_blocks[2].all.summary();
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {} | {} | {:+.4} | {:.2} |",
                cell(&g.name),
                c.curve,
                f4(m.de),
                f4(m.dl_abs),
                f4(m.dab),
                f4(m.dc_abs),
                f4(m.dh),
                m.dc,
                m.l_share
            );
        }
    }

    // Pooled, by band.
    let lnames = band_names(&L_EDGES);
    let cnames = band_names(&C_EDGES);
    let get = |v: &[(String, Banded)], c: DisplayCurve| -> Banded {
        v.iter()
            .find(|(n, _)| *n == curve_word(c))
            .map(|(_, b)| b.clone())
            .unwrap_or_default()
    };
    let (hp, ha) = (
        get(&r.pooled_common_held, DisplayCurve::Channels),
        get(&r.pooled_common_held, DisplayCurve::Agx),
    );
    let (fp, fa) = (
        get(&r.pooled_common_floor, DisplayCurve::Channels),
        get(&r.pooled_common_floor, DisplayCurve::Agx),
    );
    let (tp, ta) = (
        get(&r.pooled_common_fitted, DisplayCurve::Channels),
        get(&r.pooled_common_fitted, DisplayCurve::Agx),
    );
    let (np, na) = (
        get(&r.pooled_common_no_look, DisplayCurve::Channels),
        get(&r.pooled_common_no_look, DisplayCurve::Agx),
    );
    let total = hp.all.n.max(1.0);
    let _ = writeln!(
        s,
        "\n## Pooled over the fitted groups, common blocks, held out (all), with LUT\n"
    );
    let _ = writeln!(
        s,
        "{} blocks. pc {} against AgX {}: AgX − pc = {:+.4}, of which the lightness \
         part (mean abs ΔL) moves {:+.4} and the a, b part {:+.4}. \"contrib\" is a band's \
         share of AgX − pc in ΔE: (sum of AgX's ΔE − sum of pc's) over every block.\n",
        hp.all.n as usize,
        f4(hp.all.summary().de),
        f4(ha.all.summary().de),
        ha.all.summary().de - hp.all.summary().de,
        ha.all.summary().dl_abs - hp.all.summary().dl_abs,
        ha.all.summary().dab - hp.all.summary().dab,
    );
    type Sides<'a> = (&'a [Acc], &'a [Acc]);
    let _ = writeln!(
        s,
        "The floor (within-cell spread, below) is lightness for {:.0}% (pc) and {:.0}% (AgX) of \
         its squared error; its mean is {} against {}.\n",
        100.0 * fp.all.summary().l_share,
        100.0 * fa.all.summary().l_share,
        f4(fp.all.summary().de),
        f4(fa.all.summary().de),
    );
    let mut by_band = |title: &str,
                       names: &[String],
                       (p, a): Sides,
                       (fp, fa): Sides,
                       (tp, ta): Sides,
                       (np, na): Sides| {
        let _ = writeln!(s, "### By the camera's {title}\n");
        let _ = writeln!(
            s,
            "| band | share of blocks | no look pc / AgX | fitted pc / AgX | held out ΔE pc / AgX | abs ΔL pc / AgX | Δab pc / AgX | signed ΔC pc / AgX | contrib to AgX − pc | of it ΔL / Δab | floor pc / AgX |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|");
        for i in 0..names.len() {
            let (sp, sa) = (p[i].summary(), a[i].summary());
            let _ = writeln!(
                s,
                "| {} | {:.1}% | {} / {} | {} / {} | {} / {} | {} / {} | {} / {} | {:+.4} / {:+.4} | {:+.5} | {:+.5} / {:+.5} | {} / {} |",
                names[i],
                100.0 * p[i].n / total,
                f4(np[i].summary().de),
                f4(na[i].summary().de),
                f4(tp[i].summary().de),
                f4(ta[i].summary().de),
                f4(sp.de),
                f4(sa.de),
                f4(sp.dl_abs),
                f4(sa.dl_abs),
                f4(sp.dab),
                f4(sa.dab),
                sp.dc,
                sa.dc,
                (a[i].de - p[i].de) / total,
                (a[i].dl_abs - p[i].dl_abs) / total,
                (a[i].dab - p[i].dab) / total,
                f4(fp[i].summary().de),
                f4(fa[i].summary().de),
            );
        }
        let _ = writeln!(s);
    };
    by_band(
        "lightness (Oklab L)",
        &lnames,
        (&hp.by_l, &ha.by_l),
        (&fp.by_l, &fa.by_l),
        (&tp.by_l, &ta.by_l),
        (&np.by_l, &na.by_l),
    );
    by_band(
        "chroma (Oklab C)",
        &cnames,
        (&hp.by_c, &ha.by_c),
        (&fp.by_c, &fa.by_c),
        (&tp.by_c, &ta.by_c),
        (&np.by_c, &na.by_c),
    );
    let _ = writeln!(
        s,
        "### Which blocks each curve keeps, by the camera's lightness\n"
    );
    let _ = writeln!(
        s,
        "The cuts (texture, clipping, black) are made on both pictures, so a curve that \
         renders darker shadows or more contrast keeps other blocks. Fitted groups, frames \
         both curves registered, one exposure pass.\n"
    );
    let _ = writeln!(s, "| band | per channel only | both | AgX only |");
    let _ = writeln!(s, "|---|---|---|---|");
    for (i, k) in r.kept_by_l.iter().enumerate() {
        let _ = writeln!(s, "| {} | {} | {} | {} |", lnames[i], k[0], k[1], k[2]);
    }
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "### AgX − pc by lightness and chroma: contribution to the mean ΔE (×10⁴), and share of blocks\n"
    );
    let _ = write!(s, "| L \\ C |");
    for c in &cnames {
        let _ = write!(s, " {c} |");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "|---|{}", "---|".repeat(cnames.len()));
    for (li, ln) in lnames.iter().enumerate() {
        let _ = write!(s, "| {ln} |");
        for ci in 0..cnames.len() {
            let (p, a) = (&hp.by_lc[li][ci], &ha.by_lc[li][ci]);
            let _ = write!(
                s,
                " {:+.1} ({:.1}%) |",
                1e4 * (a.de - p.de) / total,
                100.0 * p.n / total
            );
        }
        let _ = writeln!(s);
    }

    // The brightness.
    let _ = writeln!(s, "\n## The per-frame brightness (hypothesis 2)\n");
    let _ = writeln!(
        s,
        "Offset: the match's exposure in stops, mean and spread over frames. Residual: what \
         one pass leaves (the pairs' median log2 ratio at the offset), mean and spread, and \
         mean absolute. ΔL spread: the spread over frames of the held-out mean ΔL. Iterated: \
         the exposure solved again until the median is within {CONVERGED} stops, and the fit \
         redone. Oracle: held-out ΔE (over blocks) with each frame's mean ΔL taken off, \
         against the same without; \
         the tone oracle takes off a line in the camera's L per frame instead (an offset and \
         a slope), and the slope's mean and spread over frames are given.\n"
    );
    let _ = writeln!(
        s,
        "| group | curve | offset mean / spread | residual mean / spread / abs | ΔL spread | held out (all) one pass / iterated | fitted one pass / iterated | held out over blocks / offset oracle / tone oracle | tone slope mean / spread |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|");
    for g in &fitted {
        for c in &g.curves {
            let Some(e) = c.own.as_ref() else { continue };
            let offs: Vec<f32> = c.exposure.iter().flatten().map(|v| v[0]).collect();
            let res: Vec<f32> = c.exposure.iter().flatten().map(|v| v[1]).collect();
            let (om, os) = spread(&offs);
            let (rm, rs) = spread(&res);
            let ra = res.iter().map(|v| v.abs()).sum::<f32>() / res.len().max(1) as f32;
            let it = c.iterated.as_ref();
            let _ = writeln!(
                s,
                "| {} | {} | {:+.2} / {:.2} | {:+.3} / {:.3} / {:.3} | {} | {} / {} | {} / {} | {} / {} / {} | {:+.3} / {:.3} |",
                cell(&g.name),
                c.curve,
                om,
                os,
                rm,
                rs,
                ra,
                f4(e.held_dl_spread),
                f4(e.held_all[2]),
                f4(it.map_or(f32::NAN, |i| i.held_all[2])),
                f4(e.fitted[2].all.summary().de),
                f4(it.map_or(f32::NAN, |i| i.fitted[2].all.summary().de)),
                f4(e.held_all_blocks[2].all.summary().de),
                f4(e.held_lightness_oracle.de),
                f4(e.held_tone_oracle.de),
                e.held_tone_slope.0,
                e.held_tone_slope.1,
            );
        }
    }

    let _ = writeln!(
        s,
        "\nThe spread over frames of the held-out mean ΔL within each of the camera's \
         lightness bands (frames with {BAND_MIN} blocks or more in the band): a per-frame \
         brightness error is the same in every band; one that grows toward the top is the \
         shoulder's.\n"
    );
    let lnames_h = band_names(&L_EDGES);
    let _ = writeln!(s, "| group | curve | {} |", lnames_h.join(" | "));
    let _ = writeln!(s, "|---|---|{}", "---|".repeat(L_BANDS));
    for g in &fitted {
        for c in &g.curves {
            let Some(e) = c.own.as_ref() else { continue };
            let cells: Vec<String> = e.held_dl_spread_by_l.iter().map(|v| f4(*v)).collect();
            let _ = writeln!(
                s,
                "| {} | {} | {} |",
                cell(&g.name),
                c.curve,
                cells.join(" | ")
            );
        }
    }

    let _ = writeln!(s, "\n## The iterated exposure match: where it ended\n");
    let _ = writeln!(
        s,
        "Over the fitted groups' frames. A secant step from the match's two points, its \
         slope held to {:?} and its length to {MAX_STEP} stop, at most {PASSES} finishes; \
         converged is within {CONVERGED} stops.\n",
        SLOPE_RANGE
    );
    let _ = writeln!(
        s,
        "| curve | frames | of them this step's | mean finishes, this step's only | stopped at the cap | over 0.05 stops off | over 0.02 | largest miss | stopped with no block |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|");
    for (c, v) in &r.convergence {
        let _ = writeln!(
            s,
            "| {c} | {} | {} | {:.2} | {} | {} | {} | {:.3} | {} |",
            v.frames,
            v.this_step,
            v.mean_passes,
            v.at_cap,
            v.over_005,
            v.over_002,
            v.largest,
            v.lost
        );
    }
    let _ = writeln!(
        s,
        "\nWhat it costs a frame, over the frames timed, as the match spends it \
         under one curve. The develop and the camera's JPEG come first, {:.2} s a frame on \
         average here (shared by both curves in this tool, once a run in the match); the \
         lay is one finish, the registration and the first pairs; one pass added one more \
         finish, and the converged match adds them all.\n",
        r.develop_per_frame
    );
    let _ = writeln!(
        s,
        "| curve | frames timed | lay s | one finish s | one pass: lay + one finish s | converged: lay + matching s | more a frame s |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|");
    for (c, v) in &r.convergence {
        let once = v.mean_laid + v.mean_finish;
        let converged = v.mean_laid + v.mean_matched;
        let _ = writeln!(
            s,
            "| {c} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:+.2} |",
            v.timed,
            v.mean_laid,
            v.mean_finish,
            once,
            converged,
            converged - once
        );
    }
    let _ = writeln!(
        s,
        "\nThe sheet's figures by its own function (`fit_group`) on the converged match's own \
         blocks: fitted, held out over every frame, and the seconds the call took (the fit \
         and a fit for each frame held out); and how much the fitted look's lightness moves \
         with its input's chroma, the mean and 95th percentile over the common colors \
         (notes §260).\n"
    );
    let _ = writeln!(
        s,
        "| group | curve | frames | fitted | held out | seconds | lightness on chroma |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|");
    for g in &fitted {
        for c in &g.curves {
            let (Some([f, h, t]), Some(e)) = (c.sheet, c.iterated.as_ref()) else {
                continue;
            };
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {t:.2} | {:.2} / {:.2} |",
                cell(&g.name),
                c.curve,
                e.frames,
                f4(f),
                f4(h),
                e.coupling[0],
                e.coupling[1]
            );
        }
    }

    let _ = writeln!(
        s,
        "\n## Common blocks: one pass against the iterated match, held out (all)\n"
    );
    let _ = writeln!(
        s,
        "| group | pc one pass / iterated | AgX one pass / iterated |"
    );
    let _ = writeln!(s, "|---|---|---|");
    for g in &fitted {
        let (p, a) = pc_agx(g).unwrap();
        let both = |c: &CurveReport| {
            format!(
                "{} / {}",
                f4(c.common.as_ref().map_or(f32::NAN, |e| e.held_all[2])),
                f4(c.iterated_common
                    .as_ref()
                    .map_or(f32::NAN, |e| e.held_all[2]))
            )
        };
        let _ = writeln!(s, "| {} | {} | {} |", cell(&g.name), both(p), both(a));
    }
    let blocks = |v: &[(String, Banded)], c: DisplayCurve| f4(get(v, c).all.summary().de);
    let _ = writeln!(
        s,
        "| pooled, over blocks | {} / {} | {} / {} |",
        blocks(&r.pooled_common_held, DisplayCurve::Channels),
        blocks(&r.pooled_common_iter_held, DisplayCurve::Channels),
        blocks(&r.pooled_common_held, DisplayCurve::Agx),
        blocks(&r.pooled_common_iter_held, DisplayCurve::Agx),
    );

    let _ = writeln!(
        s,
        "\n## Floors: across frames and within one frame, common blocks\n"
    );
    let _ = writeln!(
        s,
        "Blocks sharing a cell of the 33³ lattice by their render, each against the cell's \
         mean camera color, over cells of four blocks or more: across every frame of a \
         group, and within one frame (the cell keyed by frame and lattice cell). Within a \
         frame it is what the camera separates that the render does not; the rest of the \
         across-frames floor is the frames disagreeing.\n"
    );
    let _ = writeln!(
        s,
        "| band | pc across / within | AgX across / within | pc iterated across / within | AgX iterated across / within |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|");
    let floors = [
        &r.pooled_common_floor,
        &r.pooled_common_floor_within,
        &r.pooled_common_iter_floor,
        &r.pooled_common_iter_floor_within,
    ];
    let floor_row = |pick: &dyn Fn(&Banded) -> Acc| -> String {
        let v = |i: usize, c: DisplayCurve| f4(pick(&get(floors[i], c)).summary().de);
        format!(
            "{} / {} | {} / {} | {} / {} | {} / {}",
            v(0, DisplayCurve::Channels),
            v(1, DisplayCurve::Channels),
            v(0, DisplayCurve::Agx),
            v(1, DisplayCurve::Agx),
            v(2, DisplayCurve::Channels),
            v(3, DisplayCurve::Channels),
            v(2, DisplayCurve::Agx),
            v(3, DisplayCurve::Agx),
        )
    };
    let _ = writeln!(s, "| all | {} |", floor_row(&|b| b.all));
    for (i, n) in band_names(&L_EDGES).iter().enumerate() {
        let _ = writeln!(s, "| L {n} | {} |", floor_row(&|b| b.by_l[i]));
    }
    for (i, n) in band_names(&C_EDGES).iter().enumerate() {
        let _ = writeln!(s, "| C {n} | {} |", floor_row(&|b| b.by_c[i]));
    }

    let _ = writeln!(
        s,
        "\n## Radial: corner minus center of the lightness the fitted look leaves\n"
    );
    let _ = writeln!(s, "| group | lens | frames | pc | AgX |");
    let _ = writeln!(s, "|---|---|---|---|---|");
    for g in &fitted {
        let (p, a) = pc_agx(g).unwrap();
        for (lens, v, n) in &p.radial {
            let av = a
                .radial
                .iter()
                .find(|(l, _, _)| l == lens)
                .map_or(f32::NAN, |x| x.1);
            let _ = writeln!(
                s,
                "| {} | {} | {n} | {:+.4} | {:+.4} |",
                cell(&g.name),
                cell(lens),
                v,
                av
            );
        }
    }

    let _ = writeln!(
        s,
        "\n## Borrowers: every donor of the style on the borrower's frames\n"
    );
    for g in r
        .groups
        .iter()
        .filter(|g| !g.donors.is_empty() || g.plan.starts_with("borrow"))
    {
        let frames = g.curves.first().map_or(0, |c| c.kept);
        let _ = writeln!(
            s,
            "### {} ({} frames kept; plan: {})\n",
            g.name, frames, g.plan
        );
        let _ = writeln!(s, "| table | curve | donor frames | ΔE | abs ΔL | Δab |");
        let _ = writeln!(s, "|---|---|---|---|---|---|");
        for (c, m) in &g.no_look {
            let _ = writeln!(
                s,
                "| no look | {c} | | {} | {} | {} |",
                f4(m.de),
                f4(m.dl_abs),
                f4(m.dab)
            );
        }
        for d in &g.donors {
            let _ = writeln!(
                s,
                "| {}{} | {} | {} | {} | {} | {} |",
                cell(&d.donor),
                if d.chosen_by_plan {
                    " (the plan's)"
                } else {
                    ""
                },
                d.curve,
                d.donor_frames,
                f4(d.on_borrower.de),
                f4(d.on_borrower.dl_abs),
                f4(d.on_borrower.dab),
            );
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "## Groups not fitted\n");
    for g in r
        .groups
        .iter()
        .filter(|g| !fitted.iter().any(|f| f.name == g.name))
    {
        let _ = writeln!(
            s,
            "- {}: {} in scope, {} fixed; {}",
            g.name, g.in_scope, g.fixed, g.plan
        );
    }
    let _ = writeln!(s, "\n## Samples\n");
    for g in &r.groups {
        let _ = writeln!(
            s,
            "- {}: {} in scope, {} fixed, {} sampled from {} folders; {} a copy of another \
             sampled frame, left out; plan: {}",
            g.name,
            g.in_scope,
            g.fixed,
            g.sampled.len(),
            g.sampled
                .iter()
                .map(|(f, _)| f)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            g.duplicates,
            g.plan
        );
    }
    let _ = writeln!(s, "\n## Frames dropped\n");
    for g in &r.groups {
        for c in &g.curves {
            for (f, why) in &c.dropped {
                let _ = writeln!(s, "- {} under {}: {f}: {why}", g.name, c.curve);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(r: [f32; 3], j: [f32; 3], row: u16, col: u16) -> Block {
        Block { r, j, row, col }
    }

    fn measured(blocks: Vec<Block>) -> Measured {
        Measured {
            scale: 1.0,
            dy: 0.0,
            dx: 0.0,
            ncc: 1.0,
            rows: 4,
            cols: 4,
            total: 16,
            offset: 0.0,
            residual: 0.0,
            pairs: blocks.clone(),
            offset_iterated: 0.0,
            residual_iterated: 0.0,
            passes: 1,
            pairs_iterated: blocks,
            iteration_lost: false,
            seconds_laid: 0.0,
            seconds_matched: 0.0,
        }
    }

    fn record(channels: Result<Measured, String>, agx: Result<Measured, String>) -> Record {
        Record {
            version: RECORD_VERSION,
            build: "test".into(),
            iteration: ITERATION,
            name: "a.cr3".into(),
            key: key_of(Path::new("/nowhere/a.cr3")),
            size: 0,
            mtime: 0,
            lens: None,
            channels,
            agx,
            seconds: 0.0,
        }
    }

    #[test]
    fn a_line_through_the_lightness_error_is_found() {
        let labs: Vec<([f32; 3], [f32; 3])> = (0..10)
            .map(|i| {
                let l = 0.1 * i as f32;
                ([l + 0.02 + 0.1 * l, 0.0, 0.0], [l, 0.0, 0.0])
            })
            .collect();
        let (a, b) = line(&labs);
        assert!((a - 0.02).abs() < 1e-5 && (b - 0.1).abs() < 1e-5, "{a} {b}");
    }

    #[test]
    fn bands_split_at_their_edges() {
        assert_eq!(band(0.1, &L_EDGES), 0);
        assert_eq!(band(0.35, &L_EDGES), 1);
        assert_eq!(band(0.79, &L_EDGES), 3);
        assert_eq!(band(0.95, &L_EDGES), 4);
        assert_eq!(band(0.0, &C_EDGES), 0);
        assert_eq!(band(0.2, &C_EDGES), 3);
    }

    /// A lightness error is all lightness, a hue turn at equal chroma
    /// is all hue, and the two parts add up in the squares.
    #[test]
    fn the_error_splits_into_lightness_chroma_and_hue() {
        let mut a = Acc::default();
        a.add([0.6, 0.0, 0.1], [0.5, 0.0, 0.1]);
        let s = a.summary();
        assert!(
            (s.de - 0.1).abs() < 1e-6 && (s.dl - 0.1).abs() < 1e-6,
            "{s:?}"
        );
        assert!(
            s.dab.abs() < 1e-6 && (s.l_share - 1.0).abs() < 1e-6,
            "{s:?}"
        );
        let mut h = Acc::default();
        h.add([0.5, 0.1, 0.0], [0.5, 0.0, 0.1]);
        let s = h.summary();
        assert!(s.dc_abs < 1e-6, "{s:?}");
        assert!((s.dh - 0.02f32.sqrt()).abs() < 1e-5, "{s:?}");
        assert!(s.l_share.abs() < 1e-6, "{s:?}");
        let mut c = Acc::default();
        c.add([0.5, 0.0, 0.05], [0.5, 0.0, 0.1]);
        let s = c.summary();
        assert!((s.dc + 0.05).abs() < 1e-6 && s.dh < 1e-6, "{s:?}");
    }

    /// Common blocks are the frames both curves kept, and in each the
    /// blocks both kept; own blocks are each curve's whole.
    #[test]
    fn common_blocks_are_what_both_curves_kept() {
        let a = record(
            Ok(measured(vec![
                block([0.5; 3], [0.4; 3], 0, 0),
                block([0.6; 3], [0.5; 3], 0, 1),
            ])),
            Ok(measured(vec![
                block([0.45; 3], [0.5; 3], 0, 1),
                block([0.7; 3], [0.6; 3], 1, 1),
            ])),
        );
        let b = record(
            Ok(measured(vec![block([0.3; 3], [0.3; 3], 2, 2)])),
            Err("no".into()),
        );
        let recs = [&a, &b];
        let own = data(&recs, DisplayCurve::Channels, Blocks::Own, false);
        assert_eq!(own.ids, vec![0, 0, 1]);
        let pc = data(&recs, DisplayCurve::Channels, Blocks::Common, false);
        assert_eq!((pc.ids.clone(), pc.x.clone()), (vec![0], vec![[0.6; 3]]));
        let agx = data(&recs, DisplayCurve::Agx, Blocks::Common, false);
        assert_eq!((agx.ids.clone(), agx.y.clone()), (vec![0], vec![[0.5; 3]]));
    }

    /// Blocks whose render falls in one cell but whose camera colors
    /// differ make a floor; blocks that agree make none.
    #[test]
    fn the_floor_is_the_spread_within_a_cell() {
        let mut d = Data::default();
        for k in 0..8 {
            let j = if k % 2 == 0 { [0.4; 3] } else { [0.6; 3] };
            d.push(k, &block([0.5; 3], j, 0, 0));
        }
        let f = floor(&d, false).all.summary();
        // Each block its own frame: no cell within a frame has four.
        assert_eq!(floor(&d, true).all.summary().blocks, 0);
        assert_eq!(f.blocks, 8);
        assert!(f.de > 0.05 && f.l_share > 0.99, "{f:?}");
        let mut same = Data::default();
        for k in 0..8 {
            same.push(k, &block([0.5; 3], [0.4; 3], 0, 0));
        }
        assert!(floor(&same, false).all.summary().de < 1e-6);
    }

    /// A camera that is the render with a fixed tint: the fit finds it,
    /// held out as well as fitted, and the stages come down in order.
    #[test]
    fn a_tint_is_fitted_and_holds_out() {
        let mut d = Data::default();
        let mut k = 0usize;
        for f in 0..6 {
            for i in 0..400 {
                let t = (k as f32 * 0.618_034).fract();
                let u = (k as f32 * 0.414_214).fract();
                let v = (k as f32 * 0.267_949).fract();
                let r = [0.1 + 0.8 * t, 0.1 + 0.8 * u, 0.1 + 0.8 * v];
                let j = [r[0] * 0.95, r[1], (r[2] * 1.05).min(0.96)];
                d.push(f, &block(r, j, (i / 20) as u16, (i % 20) as u16));
                k += 1;
            }
        }
        let e = evaluate(&d).unwrap();
        let none = e.no_look.all.summary().de;
        let fit = e.fitted[2].all.summary().de;
        assert_eq!(e.frames, 6);
        assert!(fit < none / 3.0, "{none} {fit}");
        assert!(e.held_all[2] < none / 2.0, "{:?}", e.held_all);
        assert_eq!(e.per_frame.len(), 6);
        let md = markdown(&Report {
            library: String::new(),
            roots: 0,
            unreachable_roots: 0,
            build: "test".into(),
            frames_per_group: 40,
            developed: 0,
            recorded: 0,
            recorded_other_build: 0,
            builds: vec![],
            not_kept: 0,
            convergence: vec![],
            develop_seconds: 0.0,
            develop_per_frame: 0.0,
            seconds: 0.0,
            l_edges: L_EDGES.to_vec(),
            c_edges: C_EDGES.to_vec(),
            groups: vec![],
            pooled_common_held: vec![],
            pooled_own_held: vec![],
            pooled_common_floor: vec![],
            pooled_common_floor_within: vec![],
            pooled_common_iter_floor: vec![],
            pooled_common_iter_floor_within: vec![],
            pooled_common_iter_held: vec![],
            pooled_common_no_look: vec![],
            pooled_common_fitted: vec![],
            kept_by_l: [[0; 3]; L_BANDS],
        });
        assert!(md.starts_with("# Camera match"));
    }

    /// A frame's record survives the trip through the file, and is
    /// read back only for the same file as it was, and for another
    /// build only when asked.
    #[test]
    fn a_record_is_read_back_for_the_same_file_and_build_only() {
        let dir = crate::testing::scratch_dir("match-compare-record");
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("a.cr3");
        std::fs::write(&raw, b"not a raw").unwrap();
        let (size, mtime) = stamp(&raw);
        let mut r = record(
            Ok(measured(vec![block([0.5; 3], [0.4; 3], 1, 2)])),
            Err("x".into()),
        );
        r.name = name_of(&raw);
        r.key = key_of(&raw);
        r.size = size;
        r.mtime = mtime;
        keep(&dir, &r).unwrap();
        assert_eq!(recorded(&dir, &raw, "test", &[]), Some((r.clone(), false)));
        assert_eq!(recorded(&dir, &raw, "another", &[]), None);
        assert_eq!(recorded(&dir, &raw, "another", &["third".into()]), None);
        assert_eq!(
            recorded(&dir, &raw, "another", &["test".into()]),
            Some((r.clone(), true))
        );
        // An iteration of another step stands only where it converged.
        let mut old = r.clone();
        old.iteration = ITERATION - 1;
        keep(&dir, &old).unwrap();
        assert!(recorded(&dir, &raw, "test", &[]).is_some());
        if let Ok(m) = old.channels.as_mut() {
            m.residual_iterated = 0.1;
        }
        keep(&dir, &old).unwrap();
        assert_eq!(recorded(&dir, &raw, "test", &[]), None);
        std::fs::write(&raw, b"a longer file than it was").unwrap();
        assert_eq!(recorded(&dir, &raw, "another", &["test".into()]), None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The path's key is BLAKE3's, the same in every build.
    #[test]
    fn the_key_is_a_stable_hash_of_the_path() {
        assert_eq!(key_of(Path::new("/a/b.cr3")), key_of(Path::new("/a/b.cr3")));
        assert_ne!(key_of(Path::new("/a/b.cr3")), key_of(Path::new("/c/b.cr3")));
        assert_eq!(
            key_of(Path::new("abc")),
            blake3::hash(b"abc").to_hex()[..16].to_string()
        );
    }

    /// Narrowed to a borrower, the run keeps its donor, planned over
    /// every group, with the donor's index moved to the narrowed list.
    #[test]
    fn narrowing_to_a_borrower_keeps_its_donor() {
        let group = |camera: &str, n: usize| Group {
            camera: camera.into(),
            make: "Canon".into(),
            model: camera.into(),
            maker: "Canon".into(),
            style: "Canon Faithful".into(),
            frames: (0..n)
                .map(|i| cm::Frame {
                    path: PathBuf::from(format!("/nowhere/f{i}/{camera}-{i}.cr3")),
                    taken: None,
                    lens: None,
                    fixed: true,
                })
                .collect(),
            look: None,
        };
        let groups = vec![group("R5", 30), group("R6m2", 40), group("R7", 3)];
        let plans = cm::plan_chosen(&groups, &HashMap::new(), DisplayCurve::Channels, false, &[]);
        let (kept, plans) = narrowed(groups, plans, &["R7".to_string()]);
        let names: Vec<&str> = kept.iter().map(|g| g.camera.as_str()).collect();
        assert_eq!(names, vec!["R6m2", "R7"]);
        assert!(
            matches!(plans[1], Plan::Borrow { donor: 0, .. }),
            "{plans:?}"
        );
    }

    #[test]
    fn a_bar_in_a_cell_is_escaped() {
        assert_eq!(cell("28mm F1.4 DG HSM | Art"), "28mm F1.4 DG HSM \\| Art");
    }
}
