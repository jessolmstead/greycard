//! `--measure-baseline OUT`: the default develop's brightness against
//! the camera's own JPEG, per body, with no window.
//!
//! The finish brightens a raw by its body's baseline before its own
//! exposure ([`greycard_edit::baseline`]). Whether one number serves
//! every body is a measurement: per body, a sample of frames spread
//! over ISO, folders and dates (the camera match's spread,
//! [`cm::sample`], within each ISO band), each developed at the default
//! as the camera match develops it ([`cm::fit_edit`], the per-channel
//! curve, lens corrections on), the camera's JPEG laid over it
//! ([`cm::lay`]) and the exposure matched as the match matches it
//! (lightness alone, the median over the flat, unclipped blocks, the
//! match's iteration). The number a frame gives is the stops the
//! Exposure slider moves to land on the camera's JPEG, which is what a
//! baseline would have to move; negative where the default is brighter
//! than the camera. Where the look store holds the camera match's
//! table for the frame's body and style, the same is measured under
//! it, since a fitted look can carry brightness of its own.
//!
//! The camera's JPEG is not the target itself: its brightness is the
//! maker's choice. It is the yardstick the bodies are compared on. The
//! bodies the Lightroom comparisons found right at the default
//! ([`ANCHORS`]) give the yardstick's zero, and a body's verdict is its
//! median against theirs, as the baseline plus the slider, so it reads
//! the same whatever baseline the build applied.
//!
//! Frames of one session share a scene and a light, and a session of
//! fifteen frames is one reading, not fifteen; so beside the median
//! over frames each body has the median over its sessions (a folder
//! each, the median of its frames), and a body moves only when both
//! sit past [`WITHIN`] of the anchors' on the same side, from at least
//! [`MIN_SESSIONS`] sessions. It moves by the sessions' median.
//!
//! Frames with an adaptive setting on (Canon's Auto Lighting
//! Optimizer, Fujifilm's DR200 and DR400 and the like) brighten the
//! camera's JPEG by the scene, so a body with enough frames without
//! one is sampled from those alone, and the verdict reads those.
//!
//! `--baseline-before` measures every body at the one baseline every
//! body had before the table, as the build before it rendered them.
//!
//! Writes `report.md`, `report.json` and `frames.jsonl` under OUT, and
//! nothing else: no sidecar, no look, the library opened read-only. A
//! frame is its file name and a folder a number; no folder is named.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use greycard_edit::DisplayCurve;
use greycard_edit::look::{LookLut, LutChoice};
use greycard_match::exposure::offset_stops;

use crate::camera_match as cm;

/// Frames a body is sampled down to unless asked.
pub(crate) const FRAMES: usize = 32;
/// The bodies whose default the Lightroom comparisons found within a
/// tenth of a stop of Lightroom's at the default baseline: the R5
/// Mark II over a catalog's history pairs, the R6 Mark II on the
/// reference set. Their pooled median against their camera's JPEG is
/// the yardstick's zero.
pub(crate) const ANCHORS: [&str; 2] = ["Canon EOS R5 Mark II", "Canon EOS R6 Mark II"];
/// A body within this many stops of the anchors keeps the default
/// baseline.
pub(crate) const WITHIN: f32 = 0.15;
/// Frames without an adaptive setting a body needs before the verdict
/// reads it.
pub(crate) const MIN_FIXED: usize = 12;
/// Sessions a body needs before it is moved.
pub(crate) const MIN_SESSIONS: usize = 5;
/// The upper edges of the ISO bands the sample is spread over.
const ISO_EDGES: [u32; 3] = [200, 800, 3200];

/// What the command line asked for.
pub(crate) struct Options {
    pub(crate) out: PathBuf,
    pub(crate) library: PathBuf,
    pub(crate) roots: Option<Vec<PathBuf>>,
    /// A folder to read the raws of directly, in place of the index.
    pub(crate) folder: Option<PathBuf>,
    pub(crate) frames: usize,
    /// Bodies whose name contains any of these; all when empty.
    pub(crate) only: Vec<String>,
    /// Every body at the baseline from before the table.
    pub(crate) before: bool,
}

/// `--measure-baseline`, from the command line to the exit code.
pub(crate) fn headless(cli: &crate::Cli, out: &Path) -> Result<std::process::ExitCode> {
    let library = match &cli.library {
        Some(p) => p.clone(),
        None => greycard_library::Library::user_path().context("no data directory")?,
    };
    let opts = Options {
        out: out.to_path_buf(),
        library,
        roots: (!cli.roots.is_empty()).then(|| cli.roots.clone()),
        folder: cli.path.clone().filter(|p| p.is_dir()),
        frames: cli.baseline_frames.unwrap_or(FRAMES),
        only: cli.baseline_body.clone(),
        before: cli.baseline_before,
    };
    let report = run(&opts)?;
    std::fs::write(opts.out.join("report.md"), text(&report))?;
    std::fs::write(
        opts.out.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    print!("{}", text(&report));
    eprintln!(
        "measure baseline: {} bodies, {} frames in {:.0} s; {}",
        report.bodies.len(),
        report.frames.len(),
        report.seconds,
        opts.out.join("report.md").display()
    );
    Ok(std::process::ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------
// The survey and the sample.

/// A raw the survey found.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) path: PathBuf,
    /// The body as the panel names it.
    pub(crate) camera: String,
    /// The style's group key, where it is a fixed style the reader
    /// names.
    pub(crate) style: Option<String>,
    /// A fixed style with every adaptive setting off.
    pub(crate) fixed: bool,
    pub(crate) iso: Option<u32>,
    pub(crate) taken: Option<String>,
    pub(crate) lens: Option<String>,
}

impl Candidate {
    fn frame(&self) -> cm::Frame {
        cm::Frame {
            path: self.path.clone(),
            taken: self.taken.clone(),
            lens: self.lens.clone(),
            fixed: self.fixed,
        }
    }
}

/// The raws the index holds under `roots` (or anywhere), present.
fn survey_index(
    lib: &greycard_library::Library,
    roots: Option<&[PathBuf]>,
) -> Result<Vec<Candidate>> {
    let under = |p: &Path| roots.is_none_or(|r| r.iter().any(|r| p.starts_with(r)));
    Ok(lib
        .query(&greycard_library::Filter::default())?
        .into_iter()
        .filter(|e| {
            !e.missing
                && greycard_core::decode::is_raw_path(&e.path)
                && !greycard_core::decode::is_hidden(&e.path)
                && under(&e.path)
                && !e.exif.camera.is_empty()
        })
        .map(|e| Candidate {
            camera: e.exif.camera.clone(),
            style: e.exif.style.style.clone(),
            fixed: e.exif.style.fixed,
            iso: e.exif.iso,
            taken: e.exif.taken.clone(),
            lens: e.exif.lens.clone(),
            path: e.path,
        })
        .collect())
}

/// The raws directly in `dir`, read from the files.
fn survey_folder(dir: &Path) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| greycard_core::decode::is_raw_path(p) && !greycard_core::decode::is_hidden(p))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(probe) = greycard_core::decode::probe_ends_path(&path) else {
            continue;
        };
        let exif = greycard_library::Exif::from_probe(&probe);
        let style = greycard_core::decode::CameraStyle::read(&path)
            .ok()
            .flatten();
        out.push(Candidate {
            camera: exif.camera,
            style: style.as_ref().and_then(|s| s.group_key()),
            fixed: style.as_ref().is_some_and(|s| s.is_fixed()),
            iso: exif.iso,
            taken: exif.taken,
            lens: exif.lens,
            path,
        });
    }
    Ok(out)
}

fn iso_band(iso: Option<u32>) -> usize {
    match iso {
        // A file that will not say is most likely a base ISO.
        None => 0,
        Some(i) => ISO_EDGES.iter().take_while(|&&e| i > e).count(),
    }
}

/// About `want` of a body's `frames`: the frames without an adaptive
/// setting alone when there are [`cm::MIN_FRAMES`] of them, spread
/// evenly over the ISO bands the body has frames in (a share each,
/// what a thin band cannot fill going to the others), and within a
/// band over folders and dates as the camera match spreads a group.
pub(crate) fn sample_body(frames: &[Candidate], want: usize) -> Vec<Candidate> {
    let fixed = frames.iter().filter(|c| c.fixed).count();
    let pool: Vec<&Candidate> = frames
        .iter()
        .filter(|c| c.fixed || fixed < cm::MIN_FRAMES)
        .collect();
    let mut bands: Vec<Vec<&Candidate>> = vec![Vec::new(); ISO_EDGES.len() + 1];
    for c in pool {
        bands[iso_band(c.iso)].push(c);
    }
    let mut quota = vec![0usize; bands.len()];
    let mut left = want;
    while left > 0 {
        let mut gave = false;
        for (q, b) in quota.iter_mut().zip(&bands) {
            if left > 0 && *q < b.len() {
                *q += 1;
                left -= 1;
                gave = true;
            }
        }
        if !gave {
            break;
        }
    }
    let mut chosen: Vec<Candidate> = Vec::new();
    for (band, q) in bands.iter().zip(quota) {
        if q == 0 {
            continue;
        }
        let frames: Vec<cm::Frame> = band.iter().map(|c| c.frame()).collect();
        let mut picked = cm::sample(&frames, q);
        picked.truncate(q);
        for f in picked {
            if let Some(c) = band.iter().find(|c| c.path == f.path) {
                chosen.push((*c).clone());
            }
        }
    }
    chosen.sort_by(|a, b| (&a.taken, &a.path).cmp(&(&b.taken, &b.path)));
    chosen
}

// ---------------------------------------------------------------------
// One frame.

/// What one frame measured.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct FrameLine {
    /// The file's name.
    pub(crate) name: String,
    /// Its folder, as a number of the run's.
    pub(crate) folder: usize,
    pub(crate) camera: String,
    pub(crate) style: Option<String>,
    pub(crate) fixed: bool,
    pub(crate) iso: Option<u32>,
    /// The baseline the finish applied, in stops.
    pub(crate) baseline: f32,
    /// The registration's correlation.
    pub(crate) ncc: f32,
    /// Blocks kept at the matched exposure, and of how many.
    pub(crate) blocks: usize,
    pub(crate) total: usize,
    /// The median over the blocks at the default of `log2(camera /
    /// develop)` in linear luminance: the default develop's own
    /// lightness against the camera's, in stops; negative where the
    /// default is brighter.
    pub(crate) output: f32,
    /// The Exposure slider's stops that land the blocks' median on the
    /// camera's, iterated as the camera match iterates it.
    pub(crate) slider: f32,
    /// The median left at `slider`; within the match's convergence
    /// unless the iteration stopped at its cap.
    pub(crate) residual: f32,
    /// The camera match's table for the frame's body and style, where
    /// the store has one, and the same two numbers under it.
    pub(crate) look: Option<String>,
    pub(crate) look_output: Option<f32>,
    pub(crate) look_slider: Option<f32>,
    pub(crate) seconds: f32,
    /// Why the frame gave no number.
    pub(crate) failed: Option<String>,
}

impl FrameLine {
    /// The baseline that would land this frame on its camera's JPEG.
    fn landing(&self) -> f32 {
        self.baseline + self.slider
    }
}

/// The fitted look for a body and style in `store`, if any.
fn look_of(store: Option<&Path>, c: &Candidate, curve: DisplayCurve) -> Option<String> {
    let store = store?;
    let style = c.style.as_deref()?;
    let bare = style.split_once(' ').map_or(style, |(_, s)| s);
    crate::measure_lightroom::look_for(store, &c.camera, &format!("Camera {bare}"), curve)
}

/// The exposure under `edit` that lands the blocks' median on the
/// camera's: the output's own offset at the edit, the slider's stops
/// iterated from it as the match iterates, and the blocks there.
fn matched_under(
    developed: &mut crate::worker::FrameDevelop,
    laid: &cm::Laid,
    edit: &greycard_edit::Edit,
) -> Option<(f32, cm::Iterated, greycard_match::Pairs)> {
    let at = |developed: &mut crate::worker::FrameDevelop, o: f32| {
        let mut e = edit.clone();
        e.light.exposure += o;
        cm::pairs_under(developed, laid, &e)
    };
    let output = offset_stops(&at(developed, 0.0))?;
    let once_pairs = at(developed, output);
    let residual = offset_stops(&once_pairs)?;
    let (it, pairs) = cm::iterate(output, residual, once_pairs, &mut |o| {
        let p = at(developed, o);
        offset_stops(&p).map(|r| (r, p))
    });
    Some((output, it, pairs))
}

/// One frame measured, at its body's baseline or, `before`, at the one
/// every body had before the table.
pub(crate) fn measure(
    c: &Candidate,
    folder: usize,
    before: bool,
    lenses: Option<&greycard_lens::Database>,
    store: Option<&Path>,
) -> FrameLine {
    let started = Instant::now();
    let curve = DisplayCurve::Channels;
    let mut line = FrameLine {
        name: c
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        folder,
        camera: c.camera.clone(),
        style: c.style.clone(),
        fixed: c.fixed,
        iso: c.iso,
        ..Default::default()
    };
    let result = (|| -> Result<(), String> {
        let mut edit = cm::fit_edit(curve);
        if before {
            edit.light.baseline = Some(greycard_edit::baseline::BEFORE_BODIES);
        }
        let mut developed = crate::worker::FrameDevelop::develop(&c.path, &edit, lenses)?;
        line.baseline = developed.source().baseline(&edit.light);
        let jpeg = cm::camera_jpeg(&c.path)?;
        let laid = cm::lay(&mut developed, &jpeg, curve)?;
        line.ncc = laid.reg.ncc;
        let (output, it, pairs) =
            matched_under(&mut developed, &laid, &edit).ok_or("no flat, unclipped blocks")?;
        line.output = output;
        line.slider = it.offset;
        line.residual = it.residual;
        line.blocks = pairs.pairs.len();
        line.total = pairs.total;
        line.look = look_of(store, c, curve);
        if let Some(look) = &line.look {
            let mut under = edit.clone();
            under.look_lut = LookLut {
                lut: LutChoice::Named(look.clone()),
                strength: 1.0,
            };
            if let Some((output, it, _)) = matched_under(&mut developed, &laid, &under) {
                line.look_output = Some(output);
                line.look_slider = Some(it.offset);
            }
        }
        Ok(())
    })();
    line.failed = result.err();
    line.seconds = started.elapsed().as_secs_f32();
    line
}

// ---------------------------------------------------------------------
// The report.

/// The 25th, 50th and 75th percentiles of `v`, linearly between
/// samples; `None` when empty.
pub(crate) fn quartiles(v: &[f32]) -> Option<[f32; 3]> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    let at = |q: f32| {
        let x = q * (s.len() - 1) as f32;
        let (i, f) = (x.floor() as usize, x.fract());
        let j = (i + 1).min(s.len() - 1);
        s[i] + (s[j] - s[i]) * f
    };
    Some([at(0.25), at(0.5), at(0.75)])
}

fn median(v: &[f32]) -> Option<f32> {
    quartiles(v).map(|q| q[1])
}

/// Each session's median of `of` over `lines`, a folder a session.
fn sessions(lines: &[&FrameLine], of: impl Fn(&FrameLine) -> f32) -> Vec<f32> {
    let mut by: BTreeMap<usize, Vec<f32>> = BTreeMap::new();
    for l in lines {
        by.entry(l.folder).or_default().push(of(l));
    }
    by.values().filter_map(|v| median(v)).collect()
}

/// One body's line.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct BodyLine {
    pub(crate) camera: String,
    /// Raws of the body in the scope, and without an adaptive setting.
    pub(crate) raws: usize,
    pub(crate) raws_fixed: usize,
    /// Frames sampled, measured, and of those without an adaptive
    /// setting.
    pub(crate) sampled: usize,
    pub(crate) measured: usize,
    pub(crate) fixed: usize,
    /// The sample's ISO range.
    pub(crate) iso: Option<(u32, u32)>,
    /// The baseline the build applied.
    pub(crate) baseline: f32,
    /// The slider's stops: the 25th, 50th and 75th percentiles over
    /// the frames the verdict reads (without an adaptive setting where
    /// the body has [`MIN_FIXED`] of them, else every frame).
    pub(crate) slider: Option<[f32; 3]>,
    /// The same over the sessions' medians, and how many sessions.
    pub(crate) slider_sessions: Option<[f32; 3]>,
    pub(crate) sessions: usize,
    /// The frames' median at ISO 200 and under.
    pub(crate) slider_base_iso: Option<f32>,
    /// The same over the frames with an adaptive setting on.
    pub(crate) slider_adaptive: Option<[f32; 3]>,
    /// The output's own offset at the default, the same frames.
    pub(crate) output: Option<[f32; 3]>,
    /// The fitted looks met, and the slider under them over the frames
    /// that had one.
    pub(crate) looks: Vec<String>,
    pub(crate) look_frames: usize,
    pub(crate) look_slider: Option<[f32; 3]>,
    /// The same frames without the look, to read the look's own pull.
    pub(crate) look_slider_bare: Option<[f32; 3]>,
    /// The frames' and the sessions' medians against the anchors', as
    /// baselines.
    pub(crate) against_anchors: Option<f32>,
    pub(crate) against_anchors_sessions: Option<f32>,
    /// The baseline that lands its sessions' median on the anchors'.
    pub(crate) proposed: Option<f32>,
    pub(crate) verdict: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Report {
    pub(crate) build: String,
    /// The library's file name, or "a folder".
    pub(crate) scope: String,
    pub(crate) roots: usize,
    pub(crate) frames_wanted: usize,
    /// Every body measured at the baseline from before the table.
    pub(crate) before: bool,
    /// The anchors' pooled medians, as the baseline plus the slider:
    /// over their frames without an adaptive setting, and over their
    /// sessions; and how many of each.
    pub(crate) anchor: Option<f32>,
    pub(crate) anchor_frames: usize,
    pub(crate) anchor_sessions: Option<f32>,
    pub(crate) anchor_session_count: usize,
    pub(crate) bodies: Vec<BodyLine>,
    pub(crate) frames: Vec<FrameLine>,
    pub(crate) seconds: f64,
}

/// The frames of a body the verdict reads, and whether those are the
/// fixed ones alone.
fn read_frames<'a>(lines: &[&'a FrameLine]) -> (Vec<&'a FrameLine>, bool) {
    let ok: Vec<&FrameLine> = lines
        .iter()
        .copied()
        .filter(|l| l.failed.is_none())
        .collect();
    let fixed: Vec<&FrameLine> = ok.iter().copied().filter(|l| l.fixed).collect();
    if fixed.len() >= MIN_FIXED {
        (fixed, true)
    } else {
        (ok, false)
    }
}

/// The anchors' and every body's lines from the frames measured.
pub(crate) fn summarize(
    candidates: &BTreeMap<String, Vec<Candidate>>,
    sampled: &BTreeMap<String, usize>,
    frames: &[FrameLine],
) -> (Report, Vec<BodyLine>) {
    let anchor_lines: Vec<&FrameLine> = frames
        .iter()
        .filter(|l| l.failed.is_none() && l.fixed && ANCHORS.contains(&l.camera.as_str()))
        .collect();
    let anchor = median(&anchor_lines.iter().map(|l| l.landing()).collect::<Vec<_>>());
    // Each anchor body's sessions, pooled: a folder of one body is not
    // a folder of the other.
    let mut anchor_session_vals = Vec::new();
    for a in ANCHORS {
        let of: Vec<&FrameLine> = anchor_lines
            .iter()
            .copied()
            .filter(|l| l.camera == a)
            .collect();
        anchor_session_vals.extend(sessions(&of, FrameLine::landing));
    }
    let anchor_sessions = median(&anchor_session_vals);
    let mut bodies = Vec::new();
    for (camera, cands) in candidates {
        let Some(&n_sampled) = sampled.get(camera) else {
            continue;
        };
        let lines: Vec<&FrameLine> = frames.iter().filter(|l| &l.camera == camera).collect();
        let (read, fixed_only) = read_frames(&lines);
        let ok: Vec<&FrameLine> = lines
            .iter()
            .copied()
            .filter(|l| l.failed.is_none())
            .collect();
        let isos: Vec<u32> = lines.iter().filter_map(|l| l.iso).collect();
        let slider = quartiles(&read.iter().map(|l| l.slider).collect::<Vec<_>>());
        let session_sliders = sessions(&read, |l| l.slider);
        let landing = median(&read.iter().map(|l| l.landing()).collect::<Vec<_>>());
        let landing_sessions = median(&sessions(&read, FrameLine::landing));
        let with_look: Vec<&&FrameLine> = read.iter().filter(|l| l.look_slider.is_some()).collect();
        let mut looks: Vec<String> = with_look.iter().filter_map(|l| l.look.clone()).collect();
        looks.sort();
        looks.dedup();
        let baseline = read
            .first()
            .map(|l| l.baseline)
            .unwrap_or_else(|| greycard_edit::baseline::for_body(Some(camera)));
        let fixed = ok.iter().filter(|l| l.fixed).count();
        let against = landing.zip(anchor).map(|(s, a)| s - a);
        let against_sessions = landing_sessions.zip(anchor_sessions).map(|(s, a)| s - a);
        let proposed = against_sessions.map(|d| greycard_edit::baseline::DEFAULT + d);
        let is_anchor = ANCHORS.contains(&camera.as_str());
        let verdict = match (against, against_sessions) {
            _ if read.is_empty() => "no frame measured".to_string(),
            (Some(_), Some(_)) if is_anchor => "an anchor: keeps the default".to_string(),
            (Some(_), Some(_)) if !fixed_only => format!(
                "{fixed} frames without an adaptive setting, under {MIN_FIXED}: \
                 too few to move it"
            ),
            (Some(_), Some(_)) if session_sliders.len() < MIN_SESSIONS => format!(
                "{} sessions, under {MIN_SESSIONS}: too few to move it",
                session_sliders.len()
            ),
            (Some(d), Some(s))
                if d.abs() > WITHIN && s.abs() > WITHIN && d.signum() == s.signum() =>
            {
                format!(
                    "{d:+.2} by frame, {s:+.2} by session against the anchors: \
                     baseline {:.2}",
                    greycard_edit::baseline::DEFAULT + s
                )
            }
            (Some(d), Some(s)) => format!(
                "{d:+.2} by frame, {s:+.2} by session: within {WITHIN} on one or both, \
                 keeps the default"
            ),
            _ => "no anchor measured".to_string(),
        };
        let base_iso: Vec<f32> = read
            .iter()
            .filter(|l| iso_band(l.iso) == 0)
            .map(|l| l.slider)
            .collect();
        bodies.push(BodyLine {
            camera: camera.clone(),
            raws: cands.len(),
            raws_fixed: cands.iter().filter(|c| c.fixed).count(),
            sampled: n_sampled,
            measured: ok.len(),
            fixed,
            iso: isos.iter().min().copied().zip(isos.iter().max().copied()),
            baseline,
            slider,
            slider_sessions: quartiles(&session_sliders),
            sessions: session_sliders.len(),
            slider_base_iso: median(&base_iso),
            slider_adaptive: quartiles(
                &ok.iter()
                    .filter(|l| !l.fixed)
                    .map(|l| l.slider)
                    .collect::<Vec<_>>(),
            ),
            output: quartiles(&read.iter().map(|l| l.output).collect::<Vec<_>>()),
            looks,
            look_frames: with_look.len(),
            look_slider: quartiles(
                &with_look
                    .iter()
                    .filter_map(|l| l.look_slider)
                    .collect::<Vec<_>>(),
            ),
            look_slider_bare: quartiles(&with_look.iter().map(|l| l.slider).collect::<Vec<_>>()),
            against_anchors: against,
            against_anchors_sessions: against_sessions,
            proposed,
            verdict,
        });
    }
    let report = Report {
        anchor,
        anchor_frames: anchor_lines.len(),
        anchor_sessions,
        anchor_session_count: anchor_session_vals.len(),
        ..Default::default()
    };
    (report, bodies)
}

pub(crate) fn run(opts: &Options) -> Result<Report> {
    let started = Instant::now();
    let (all, scope, roots) = match &opts.folder {
        Some(dir) => (survey_folder(dir)?, "a folder".to_string(), 0),
        None => {
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
            let lib = greycard_library::Library::open_read_only(&opts.library)
                .with_context(|| format!("opening {}", opts.library.display()))?;
            let scope = opts
                .library
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let all = survey_index(&lib, (!roots.is_empty()).then_some(roots.as_slice()))?;
            (all, scope, roots.len())
        }
    };
    let mut candidates: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for c in all {
        candidates.entry(c.camera.clone()).or_default().push(c);
    }
    let chosen: Vec<(String, Vec<Candidate>)> = candidates
        .iter()
        .filter(|(camera, _)| {
            opts.only.is_empty() || opts.only.iter().any(|o| camera.contains(o.as_str()))
        })
        .map(|(camera, cands)| (camera.clone(), sample_body(cands, opts.frames)))
        .collect();
    let sampled: BTreeMap<String, usize> =
        chosen.iter().map(|(c, s)| (c.clone(), s.len())).collect();
    for (camera, s) in &chosen {
        eprintln!(
            "measure baseline: {camera}: {} of {} raws ({} without an adaptive setting)",
            s.len(),
            candidates[camera].len(),
            s.iter().filter(|c| c.fixed).count()
        );
    }
    let lenses = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| db);
    if lenses.is_none() {
        eprintln!("measure baseline: no lens database; the develop runs without lens corrections");
    }
    let store = greycard_edit::look::store_dir().filter(|d| d.is_dir());
    std::fs::create_dir_all(&opts.out)?;
    let mut jsonl = std::fs::File::create(opts.out.join("frames.jsonl"))?;
    let todo: Vec<&Candidate> = chosen.iter().flat_map(|(_, s)| s).collect();
    // A folder is a number, in the order the frames first name it.
    let mut folders: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut frames = Vec::new();
    let develop_started = Instant::now();
    for (k, c) in todo.iter().enumerate() {
        let parent = c.path.parent().unwrap_or(Path::new("")).to_path_buf();
        let n = folders.len();
        let folder = *folders.entry(parent).or_insert(n);
        let line = measure(c, folder, opts.before, lenses.as_ref(), store.as_deref());
        writeln!(jsonl, "{}", serde_json::to_string(&line)?)?;
        let per = develop_started.elapsed().as_secs_f64() / (k + 1) as f64;
        eprintln!(
            "[{}/{}] {} {}: {} ({:.1} s, about {:.0} min left)",
            k + 1,
            todo.len(),
            line.camera,
            line.name,
            match &line.failed {
                Some(why) => format!("dropped: {why}"),
                None => format!(
                    "baseline {:.2}, output {:+.2}, slider {:+.2}{}",
                    line.baseline,
                    line.output,
                    line.slider,
                    line.look_slider
                        .map(|s| format!(", under the look {s:+.2}"))
                        .unwrap_or_default()
                ),
            },
            line.seconds,
            (todo.len() - k - 1) as f64 * per / 60.0
        );
        frames.push(line);
    }
    let (head, bodies) = summarize(&candidates, &sampled, &frames);
    Ok(Report {
        build: crate::match_compare::build(),
        scope,
        roots,
        frames_wanted: opts.frames,
        before: opts.before,
        bodies,
        frames,
        seconds: started.elapsed().as_secs_f64(),
        ..head
    })
}

fn q3(q: Option<[f32; 3]>) -> String {
    match q {
        Some([a, b, c]) => format!("{b:+.2} ({a:+.2} to {c:+.2})"),
        None => "-".into(),
    }
}

fn opt(v: Option<f32>) -> String {
    v.map_or("-".into(), |v| format!("{v:+.2}"))
}

pub(crate) fn text(r: &Report) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# The default develop against the camera's JPEG, per body\n"
    );
    let _ = writeln!(
        s,
        "Build {}; scope {} ({} roots); up to {} frames a body; {}; {:.0} s.\n",
        r.build,
        r.scope,
        r.roots,
        r.frames_wanted,
        if r.before {
            "every body at the baseline from before the table"
        } else {
            "each body at its own baseline"
        },
        r.seconds
    );
    let _ = writeln!(
        s,
        "Stops are the Exposure slider's that land the default develop's \
         block median on the camera's JPEG (lightness alone, the camera \
         match's blocks and iteration); negative where the default is \
         brighter than the camera. Percentiles are over the frames without \
         an adaptive setting where a body has {MIN_FIXED} of them, else over \
         every frame measured; a session is a folder, its frames' median. \
         Against the anchors is the body's baseline plus its median, less \
         the anchors' the same way: a body moves when both the frames' and \
         the sessions' sit past {WITHIN} on one side, from {MIN_SESSIONS} \
         sessions or more, by the sessions'.\n"
    );
    let _ = writeln!(
        s,
        "Anchors ({}): pooled median {} over {} frames, {} over {} sessions \
         (as baseline plus slider).\n",
        ANCHORS.join(", "),
        opt(r.anchor),
        r.anchor_frames,
        opt(r.anchor_sessions),
        r.anchor_session_count
    );
    let _ = writeln!(
        s,
        "| body | raws (fixed) | sampled | measured (fixed) | ISO | baseline | slider by frame: median (25th to 75th) | by session (sessions) | ISO 200 and under | adaptive frames | output at default | against anchors: frames, sessions | verdict |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for b in &r.bodies {
        let _ = writeln!(
            s,
            "| {} | {} ({}) | {} | {} ({}) | {} | {:.2} | {} | {} ({}) | {} | {} | {} | {}, {} | {} |",
            b.camera,
            b.raws,
            b.raws_fixed,
            b.sampled,
            b.measured,
            b.fixed,
            b.iso.map_or("-".into(), |(a, c)| format!("{a}-{c}")),
            b.baseline,
            q3(b.slider),
            q3(b.slider_sessions),
            b.sessions,
            opt(b.slider_base_iso),
            q3(b.slider_adaptive),
            q3(b.output),
            opt(b.against_anchors),
            opt(b.against_anchors_sessions),
            b.verdict
        );
    }
    let _ = writeln!(s, "\n## Under the fitted looks\n");
    let _ = writeln!(
        s,
        "| body | looks | frames | slider without the look | slider under it |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|");
    for b in r.bodies.iter().filter(|b| b.look_frames > 0) {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} |",
            b.camera,
            b.looks.join(", "),
            b.look_frames,
            q3(b.look_slider_bare),
            q3(b.look_slider)
        );
    }
    let _ = writeln!(s, "\n## Frames\n");
    let _ = writeln!(
        s,
        "| body | frame | folder | style | fixed | ISO | baseline | correlation | blocks | output | slider | under the look | s |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for f in &r.frames {
        let style = f.style.as_deref().unwrap_or("-");
        let fixed = if f.fixed { "yes" } else { "no" };
        let iso = f.iso.map_or("-".into(), |i| i.to_string());
        match &f.failed {
            Some(why) => {
                let _ = writeln!(
                    s,
                    "| {} | {} | {} | {style} | {fixed} | {iso} | | dropped: {why} | | | | | {:.0} |",
                    f.camera, f.name, f.folder, f.seconds
                );
            }
            None => {
                let _ = writeln!(
                    s,
                    "| {} | {} | {} | {style} | {fixed} | {iso} | {:.2} | {:.3} | {} of {} | {:+.2} | {:+.2} | {} | {:.0} |",
                    f.camera,
                    f.name,
                    f.folder,
                    f.baseline,
                    f.ncc,
                    f.blocks,
                    f.total,
                    f.output,
                    f.slider,
                    opt(f.look_slider),
                    f.seconds
                );
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(i: usize, iso: u32, fixed: bool, folder: &str) -> Candidate {
        Candidate {
            path: PathBuf::from(format!("/{folder}/IMG_{i:04}.CR3")),
            camera: "Canon EOS R5 Mark II".into(),
            style: Some("Canon Faithful".into()),
            fixed,
            iso: Some(iso),
            taken: Some(format!("2026-01-01 00:{:02}:{:02}", i / 60, i % 60)),
            lens: None,
        }
    }

    #[test]
    fn the_sample_spreads_over_the_iso_bands() {
        let mut frames: Vec<Candidate> = (0..100).map(|i| cand(i, 100, true, "a")).collect();
        frames.extend((100..105).map(|i| cand(i, 6400, true, "b")));
        frames.extend((105..140).map(|i| cand(i, 1600, true, "c")));
        let s = sample_body(&frames, 24);
        assert_eq!(s.len(), 24);
        let high = s.iter().filter(|c| c.iso == Some(6400)).count();
        let mid = s.iter().filter(|c| c.iso == Some(1600)).count();
        // The thin band gives all it has, the others share the rest.
        assert_eq!(high, 5);
        assert!(mid >= 9, "{mid}");
    }

    #[test]
    fn the_sample_leaves_the_adaptive_frames_out_once_the_fixed_ones_are_enough() {
        let mut frames: Vec<Candidate> = (0..30).map(|i| cand(i, 100, true, "a")).collect();
        frames.extend((30..90).map(|i| cand(i, 100, false, "a")));
        let s = sample_body(&frames, 24);
        assert_eq!(s.len(), 24);
        assert!(s.iter().all(|c| c.fixed));
        // Under enough, both.
        let few: Vec<Candidate> = frames[20..].to_vec();
        let s = sample_body(&few, 24);
        assert_eq!(s.len(), 24);
        assert_eq!(s.iter().filter(|c| c.fixed).count(), 10);
    }

    #[test]
    fn quartiles_interpolate() {
        assert_eq!(quartiles(&[]), None);
        assert_eq!(quartiles(&[1.0]), Some([1.0; 3]));
        let q = quartiles(&[4.0, 1.0, 3.0, 2.0, 5.0]).unwrap();
        assert_eq!(q, [2.0, 3.0, 4.0]);
    }

    fn line(camera: &str, folder: usize, slider: f32) -> FrameLine {
        FrameLine {
            camera: camera.into(),
            folder,
            slider,
            baseline: 0.8,
            fixed: true,
            ..Default::default()
        }
    }

    /// A body past the line by frame and by session, from enough
    /// sessions, moves by its sessions' median; one past it by frame
    /// alone (one long session) does not, nor one with few sessions or
    /// few fixed frames; and a body measured at another baseline reads
    /// the same.
    #[test]
    fn a_body_is_read_against_the_anchors_by_frame_and_by_session() {
        let mut frames = Vec::new();
        for f in 0..6 {
            for i in 0..2 {
                let v = -0.3 + 0.01 * (f + i) as f32;
                frames.push(line(ANCHORS[0], f, v));
                frames.push(line(ANCHORS[1], 10 + f, v));
                frames.push(line("Body Far", 20 + f, v - 0.3));
                // At another baseline, the same scene: as far.
                frames.push(FrameLine {
                    baseline: 0.5,
                    ..line("Body Moved", 30 + f, v)
                });
                frames.push(line("Body Near", 40 + f, v + 0.05));
            }
        }
        // One long session far off, and five near.
        for i in 0..14 {
            frames.push(line("Body Session", 50, -0.8 + 0.001 * i as f32));
        }
        for f in 0..5 {
            frames.push(line("Body Session", 51 + f, -0.3));
        }
        for i in 0..12 {
            frames.push(line("Body Few", 60 + i % 3, -0.9));
        }
        let mut candidates = BTreeMap::new();
        let mut sampled = BTreeMap::new();
        for b in [
            ANCHORS[0],
            ANCHORS[1],
            "Body Far",
            "Body Moved",
            "Body Near",
            "Body Session",
            "Body Few",
        ] {
            candidates.insert(b.to_string(), Vec::new());
            sampled.insert(b.to_string(), 12);
        }
        let (head, bodies) = summarize(&candidates, &sampled, &frames);
        assert_eq!(head.anchor_frames, 24);
        assert_eq!(head.anchor_session_count, 12);
        let by = |c: &str| bodies.iter().find(|b| b.camera == c).unwrap().clone();
        let far = by("Body Far");
        assert!((far.against_anchors_sessions.unwrap() + 0.3).abs() < 1e-4);
        assert!((far.proposed.unwrap() - 0.5).abs() < 1e-4);
        assert!(far.verdict.contains("baseline 0.50"), "{}", far.verdict);
        let moved = by("Body Moved");
        assert!((moved.proposed.unwrap() - 0.5).abs() < 1e-4, "{moved:?}");
        assert!(by("Body Near").verdict.contains("keeps"));
        let session = by("Body Session");
        assert!(session.against_anchors.unwrap() < -WITHIN);
        assert!(session.verdict.contains("keeps"), "{}", session.verdict);
        assert!(by("Body Few").verdict.contains("sessions"));
        assert!(by(ANCHORS[0]).verdict.contains("anchor"));
    }
}
