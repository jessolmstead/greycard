//! The camera match's runner: the Look section's "Fit this camera's
//! look", driving `greycard-match` over the frames of each body and
//! picture style in a scope and writing one table per group into the
//! looks store.
//!
//! It lives in the editor and not beside the fit because the develop
//! it needs is the editor's: the export's own develop of a frame that
//! is not open ([`crate::worker::FrameDevelop`]), with the lens
//! corrections, the finish and the geometry exactly as an export has
//! them. `greycard-match` stays a crate of numbers with no develop in
//! it, and this module is the join: sample the frames, develop each at
//! the export's size, lay the camera's JPEG over it, take the block
//! pairs, solve the frame's exposure against the camera's and finish
//! it again at that offset, then fit the shared model and write it.
//!
//! A run develops under one display curve, the open picture's, and
//! every table it writes declares it in its header with the body it is
//! for, so the finish applies it only to a picture on that curve and
//! the look list can group it by body.
//!
//! A group with too few usable frames gets no table of its own. When
//! another body in the same run has the same maker and style with a
//! fit, the group borrows that table under its own name, the title
//! saying which body it was fitted on; otherwise it is skipped with
//! the reason.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use greycard_core::image::WorkingImage;
use greycard_edit::look::{Entry, MadeFor, variant_stem};
use greycard_edit::{DisplayCurve, Edit};
use greycard_match::fit::{LutParams, leave_one_out};
use greycard_match::register::{NCC_MIN, register};
use greycard_match::{Model, Pairs, Picture};

/// Frames a group is sampled down to.
pub(crate) const SAMPLE: usize = 40;
/// Usable frames a group needs for a table of its own; also the
/// number of fixed frames past which the frames with an adaptive
/// setting on are left out.
pub(crate) const MIN_FRAMES: usize = 20;
/// Frames taken from every folder before the rest are spread through
/// the dates, so one long session does not fill the sample.
pub(crate) const PER_FOLDER: usize = 3;
/// The long edge each frame is developed at, the trial's.
pub(crate) const LONG_EDGE: u32 = 2048;
/// Frames held out, one at a time, for the reported error.
const HELD_OUT: usize = 4;

/// One frame of a group.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Frame {
    pub(crate) path: PathBuf,
    /// `YYYY-MM-DD HH:MM:SS`, as the index keeps it.
    pub(crate) taken: Option<String>,
    pub(crate) lens: Option<String>,
    /// A fixed style with every adaptive setting off.
    pub(crate) fixed: bool,
}

impl Frame {
    fn folder(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new(""))
    }
}

/// A body and a fixed picture style, and the frames of it in the scope.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Group {
    /// Make and model as the panel names them.
    pub(crate) camera: String,
    /// Make and model as the raw names them, which the table declares.
    pub(crate) make: String,
    pub(crate) model: String,
    /// The maker as the style reader names it.
    pub(crate) maker: String,
    /// The style's group key: the maker and the style.
    pub(crate) style: String,
    pub(crate) frames: Vec<Frame>,
    /// The look the group's table is written under when it is not the
    /// group's own name: a refit of a look the user renamed, or one
    /// from before the names were the bodies'.
    pub(crate) look: Option<String>,
}

impl Group {
    pub(crate) fn fixed(&self) -> usize {
        self.frames.iter().filter(|f| f.fixed).count()
    }

    /// The look's name in the store.
    pub(crate) fn name(&self) -> String {
        self.look
            .clone()
            .unwrap_or_else(|| look_name(&self.camera, &self.maker, &self.style))
    }

    /// Whether every frame is from one folder: one session, as a
    /// rule, whose light and lens are all the fit will see.
    pub(crate) fn one_folder(&self) -> bool {
        let mut folders = self.frames.iter().map(Frame::folder);
        match folders.next() {
            Some(first) => folders.all(|f| f == first),
            None => false,
        }
    }

    /// The frames left out of the sample for an adaptive setting:
    /// every one of them once the fixed frames are enough alone.
    pub(crate) fn left_out(&self) -> usize {
        if self.fixed() >= MIN_FRAMES {
            self.frames.len() - self.fixed()
        } else {
            0
        }
    }

    /// The frames the fit would develop.
    pub(crate) fn candidates(&self) -> usize {
        sample(&self.frames, SAMPLE).len()
    }
}

/// What the scope's groups and the index's state were found to be.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Survey {
    pub(crate) groups: Vec<Group>,
    /// Raws in the scope whose maker's tags the index has not read
    /// yet, which the next pass over their folders reads.
    pub(crate) unread: usize,
    /// Raws in the scope, read directly, with no fixed style: an
    /// adaptive style, or a maker whose styles are not read.
    pub(crate) no_style: usize,
}

/// The groups the index holds under `roots`, or in the whole library.
pub(crate) fn survey_library(
    lib: &greycard_library::Library,
    roots: Option<&[PathBuf]>,
) -> greycard_library::Result<Survey> {
    let mut groups = Vec::new();
    for g in lib.style_groups(roots)? {
        let frames = lib
            .style_frames(&g, roots)?
            .into_iter()
            .map(|f| Frame {
                path: f.path,
                taken: f.taken,
                lens: f.lens,
                fixed: f.fixed,
            })
            .collect();
        groups.push(Group {
            camera: g.camera,
            make: g.make,
            model: g.model,
            maker: g.maker,
            style: g.style,
            frames,
            look: None,
        });
    }
    Ok(Survey {
        groups,
        unread: lib.styles_unread(roots)?,
        no_style: lib.styles_missing(roots)?,
    })
}

/// The groups of `files`, read from the files themselves: the maker's
/// tags and the EXIF of each raw. For a folder the index does not
/// cover, where it is cheap enough.
pub(crate) fn survey_files(files: &[PathBuf]) -> Survey {
    let mut by_key: BTreeMap<(String, String), Group> = BTreeMap::new();
    let mut no_style = 0;
    for path in files
        .iter()
        .filter(|p| greycard_core::decode::is_raw_path(p))
    {
        let style = match greycard_core::decode::CameraStyle::read(path) {
            Ok(Some(style)) => style,
            Ok(None) => {
                no_style += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!("{}: the maker's tags: {e}", path.display());
                no_style += 1;
                continue;
            }
        };
        let Some(key) = style.group_key() else {
            no_style += 1;
            continue;
        };
        let exif = match greycard_core::decode::probe_path(path) {
            Ok(probe) => greycard_library::Exif::from_probe(&probe),
            Err(e) => {
                tracing::warn!("{}: {e}", path.display());
                no_style += 1;
                continue;
            }
        };
        by_key
            .entry((exif.camera.clone(), key.clone()))
            .or_insert_with(|| Group {
                camera: exif.camera.clone(),
                make: exif.make.clone(),
                model: exif.model.clone(),
                maker: style.maker.name().to_string(),
                style: key,
                frames: Vec::new(),
                look: None,
            })
            .frames
            .push(Frame {
                path: path.clone(),
                taken: exif.taken,
                lens: exif.lens,
                fixed: style.is_fixed(),
            });
    }
    let mut groups: Vec<Group> = by_key.into_values().collect();
    for g in &mut groups {
        g.frames.sort_by(order);
    }
    Survey {
        groups,
        unread: 0,
        no_style,
    }
}

/// By date taken, then path; a frame with no date first.
fn order(a: &Frame, b: &Frame) -> std::cmp::Ordering {
    (&a.taken, &a.path).cmp(&(&b.taken, &b.path))
}

/// `m` indices spread evenly through `0..n`, each in the middle of
/// its share; all of them when `m >= n`.
fn evenly(n: usize, m: usize) -> Vec<usize> {
    if m >= n {
        return (0..n).collect();
    }
    (0..m).map(|k| (2 * k + 1) * n / (2 * m)).collect()
}

/// About `want` of `frames`, spread across folders and dates: the
/// fixed frames alone when there are [`MIN_FRAMES`] of them, else
/// every fixed frame and the rest from the frames with an adaptive
/// setting on. By date taken.
pub(crate) fn sample(frames: &[Frame], want: usize) -> Vec<Frame> {
    let mut all: Vec<&Frame> = frames.iter().collect();
    all.sort_by(|a, b| order(a, b));
    let (fixed, adaptive): (Vec<&Frame>, Vec<&Frame>) = all.into_iter().partition(|f| f.fixed);
    let mut chosen = if fixed.len() >= MIN_FRAMES {
        spread(&fixed, want)
    } else {
        let mut chosen = spread(&fixed, want);
        chosen.extend(spread(&adaptive, want.saturating_sub(chosen.len())));
        chosen
    };
    chosen.sort_by(|a, b| order(a, b));
    chosen.into_iter().cloned().collect()
}

/// `want` of `frames` (sorted by date): a few from every folder
/// first, as many as fit, then the rest evenly through the dates.
fn spread<'a>(frames: &[&'a Frame], want: usize) -> Vec<&'a Frame> {
    if frames.len() <= want {
        return frames.to_vec();
    }
    let mut folders: BTreeMap<&Path, Vec<usize>> = BTreeMap::new();
    for (i, f) in frames.iter().enumerate() {
        folders.entry(f.folder()).or_default().push(i);
    }
    let per = PER_FOLDER.min(want / folders.len().max(1));
    let mut chosen: BTreeSet<usize> = BTreeSet::new();
    for idx in folders.values() {
        chosen.extend(evenly(idx.len(), per).into_iter().map(|k| idx[k]));
    }
    let rest: Vec<usize> = (0..frames.len()).filter(|i| !chosen.contains(i)).collect();
    let left = want.saturating_sub(chosen.len());
    chosen.extend(evenly(rest.len(), left).into_iter().map(|k| rest[k]));
    chosen.into_iter().map(|i| frames[i]).collect()
}

/// A look's name for a body and a style: the camera as the panel names
/// it and the style without the maker said twice, "Canon EOS R6m2
/// Faithful", made safe as a file name in the store.
pub(crate) fn look_name(camera: &str, maker: &str, style: &str) -> String {
    let says_maker = camera.to_lowercase().starts_with(&maker.to_lowercase()) && !maker.is_empty();
    let bare = style
        .strip_prefix(maker)
        .map(str::trim_start)
        .filter(|s| says_maker && !s.is_empty());
    sanitize(&format!("{camera} {}", bare.unwrap_or(style)))
}

/// A name as a file name the look store takes: no separator, nothing a
/// file system refuses, no `..`, spaces collapsed, never empty.
pub(crate) fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    let out = out.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if out.is_empty() {
        "Camera look".to_string()
    } else {
        out
    }
}

/// What the run will do with each group, before it runs: fit one with
/// enough frames, borrow for one without from a body in the run with
/// the same maker and style that has them, skip the rest.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Plan {
    /// With the warning when it replaces a table it would otherwise
    /// have left alone.
    Fit {
        replaces: Option<String>,
    },
    /// From the body named, the group at `donor` in the run.
    Borrow {
        from: String,
        donor: usize,
        replaces: Option<String>,
    },
    /// A group the user did not choose that a chosen group borrows
    /// from: its frames are developed and a table fitted in memory for
    /// the borrower, and nothing of its own is written.
    Donor,
    Skip(String),
}

impl Plan {
    pub(crate) fn is_fit(&self) -> bool {
        matches!(self, Plan::Fit { .. })
    }
}

/// Why a group the user unchecked is left alone.
pub(crate) const NOT_CHOSEN: &str = "not chosen";

/// A table already in the store under a group's name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Existing {
    /// Written by the camera match: the body it was fitted on, the
    /// frames the fit used where the table says, and the display curve
    /// it was fitted under.
    Fitted {
        on: String,
        frames: Option<usize>,
        made_for: MadeFor,
    },
    /// Anything else: a table the user put there.
    Own,
}

/// The tables in the store by file stem, as the match would find them:
/// a look's per-channel table under its name, another curve's under
/// [`greycard_edit::look::variant_stem`].
///
/// A table the match wrote carries its line, and says the body it was
/// fitted on and its frames in its header; one from before the header
/// said them says them in its title, which is read when the header
/// does not. A table without the line is the user's, whatever its
/// title says.
pub(crate) fn existing_in(store: &Path) -> HashMap<String, Existing> {
    use greycard_core::lut::declared;
    use greycard_edit::look::key;
    greycard_core::lut::list_dir(store)
        .into_iter()
        .map(|(name, _, info)| {
            let (title, comments) = info.map(|i| (i.title, i.comments)).unwrap_or_default();
            let from_title = title.as_deref().and_then(greycard_edit::look::parse_fitted);
            let marked = greycard_edit::look::is_fitted(&comments);
            let on = declared(&comments, key::FITTED_ON)
                .filter(|_| marked)
                .or(from_title.map(|(on, _)| on));
            let frames = declared(&comments, key::FRAMES)
                .filter(|_| marked)
                .and_then(|n| n.parse().ok())
                .or(from_title.and_then(|(_, n)| n));
            let found = match on.filter(|_| marked) {
                Some(on) => Existing::Fitted {
                    on: on.to_string(),
                    frames,
                    made_for: MadeFor::read(&comments),
                },
                None => Existing::Own,
            };
            (name, found)
        })
        .collect()
}

/// Whether a table for `camera`, fitted under `curve`, may be written
/// over what is in the store where it would go (its look's table for
/// that curve; another curve's is never where it would go): a fit of
/// this body's own replaces a fit of this body from no more frames,
/// and anything written by a borrow or on another body; a borrow never
/// replaces a fit of this body itself; nothing replaces a table the
/// match did not write, or one that declares another curve. With
/// `replace` each refusal is a replacement with a warning instead.
/// `Ok` carries that warning, `Err` the reason to leave the table
/// alone.
pub(crate) fn may_write(
    existing: Option<&Existing>,
    camera: &str,
    borrow: bool,
    frames: usize,
    curve: DisplayCurve,
    replace: bool,
) -> Result<Option<String>, String> {
    let refused = match existing {
        None => return Ok(None),
        Some(Existing::Fitted { made_for, .. }) if !made_for.applies(curve) => {
            let under = made_for.phrase().unwrap_or_default();
            (
                format!("the table there was fitted under {under}"),
                format!("replaces a table fitted under {under}"),
            )
        }
        Some(Existing::Own) => (
            "a table of that name that the camera match did not write is there".to_string(),
            "replaces a table of that name the camera match did not write".to_string(),
        ),
        Some(Existing::Fitted { on, .. }) if on == camera && borrow => (
            "the table there was fitted on this body itself, and a borrowed one does not \
             replace it"
                .to_string(),
            "replaces the table fitted on this body itself with a borrowed one".to_string(),
        ),
        Some(Existing::Fitted {
            on,
            frames: Some(m),
            ..
        }) if on == camera && frames < *m => (
            format!("the table there was fitted on this body from {m} frames, more than {frames}"),
            format!("replaces a table fitted on this body from {m} frames"),
        ),
        Some(Existing::Fitted { .. }) => return Ok(None),
    };
    if replace {
        Ok(Some(refused.1))
    } else {
        Err(format!("{}; left alone", refused.0))
    }
}

/// The plan for every group against the tables in the store, every
/// group chosen: [`plan_chosen`].
#[cfg(test)]
pub(crate) fn plan(
    groups: &[Group],
    existing: &HashMap<String, Existing>,
    curve: DisplayCurve,
    replace: bool,
) -> Vec<Plan> {
    plan_chosen(groups, existing, curve, replace, &vec![true; groups.len()])
}

/// The plan for every group against the tables in the store, `chosen`
/// saying which groups the user wants fitted (one flag a group; a
/// missing flag is a yes). The groups with enough frames fit; for each
/// style, the one of those with the most frames in the run (the
/// first, on a tie) is the donor a body without enough borrows from,
/// and the run uses the same one.
///
/// A group that is not chosen is skipped as a small one is: never
/// developed for itself, never written. The donor is chosen on merit
/// among every group that could fit, chosen or not, so unchecking a
/// body does not move a borrower to another donor; when a chosen group
/// borrows from one that is not chosen, that group is [`Plan::Donor`]:
/// its frames are read and a table fitted in memory for the borrower,
/// and its own table is not written.
pub(crate) fn plan_chosen(
    groups: &[Group],
    existing: &HashMap<String, Existing>,
    curve: DisplayCurve,
    replace: bool,
    chosen: &[bool],
) -> Vec<Plan> {
    let is_chosen = |i: usize| chosen.get(i).copied().unwrap_or(true);
    let mut plans: Vec<Plan> = groups
        .iter()
        .map(|g| {
            let n = g.candidates();
            if n < MIN_FRAMES {
                return Plan::Skip(too_few(n));
            }
            let at = existing.get(&variant_stem(&g.name(), curve));
            match may_write(at, &g.camera, false, n, curve, replace) {
                Ok(replaces) => Plan::Fit { replaces },
                Err(why) => Plan::Skip(why),
            }
        })
        .collect();
    let mut donors: HashMap<&str, usize> = HashMap::new();
    for (i, g) in groups.iter().enumerate() {
        if !plans[i].is_fit() {
            continue;
        }
        let better = match donors.get(g.style.as_str()) {
            Some(&d) => g.frames.len() > groups[d].frames.len(),
            None => true,
        };
        if better {
            donors.insert(&g.style, i);
        }
    }
    let mut needed: Vec<usize> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        if g.candidates() >= MIN_FRAMES || !is_chosen(i) {
            continue;
        }
        let Some(&donor) = donors.get(g.style.as_str()) else {
            continue;
        };
        if groups[donor].camera == g.camera {
            continue;
        }
        let at = existing.get(&variant_stem(&g.name(), curve));
        plans[i] = match may_write(at, &g.camera, true, 0, curve, replace) {
            Ok(replaces) => {
                needed.push(donor);
                Plan::Borrow {
                    from: groups[donor].camera.clone(),
                    donor,
                    replaces,
                }
            }
            Err(why) => Plan::Skip(why),
        };
    }
    for (i, p) in plans.iter_mut().enumerate() {
        if is_chosen(i) {
            continue;
        }
        *p = if needed.contains(&i) {
            Plan::Donor
        } else {
            Plan::Skip(NOT_CHOSEN.to_string())
        };
    }
    plans
}

/// Whether `g` is the group that makes the look named `name`, whose
/// tables are `tables`: the group's own name is the look's, or the name
/// one of its tables' titles gives before "(fitted on", or a table
/// declares the group's body and style. So a renamed look is still
/// found by its header or its title, and a table whose body cannot be
/// read is found by nothing.
fn makes(g: &Group, name: &str, tables: &[&Entry]) -> bool {
    use greycard_core::lut::declared;
    use greycard_edit::look::key;
    let own = g.name();
    own == name
        || tables.iter().any(|e| {
            let titled = e
                .title
                .as_deref()
                .and_then(|t| t.rsplit_once(" (fitted on "))
                .is_some_and(|(n, _)| n == own);
            let declared_body = e
                .declared_body()
                .is_some_and(|b| b.eq_ignore_ascii_case(&g.camera));
            let declared_style = declared(&e.comments, key::STYLE) == Some(g.style.as_str());
            e.is_fitted() && (titled || (declared_body && declared_style))
        })
}

/// The groups a refit of the look named `name` (its tables `tables`)
/// runs over: the group that makes it ([`makes`]) when it has the
/// frames for a fit of its own; else that group and the donor a borrow
/// would take, the body of the same style with the most frames that
/// has enough (the plan's rule), since a borrowed table is refit by
/// fitting its donor again. The look's group writes under the look's
/// own name, so a renamed look gets its table for the curve beside the
/// one it has. None when no group in the scope makes that look.
pub(crate) fn refit_groups(groups: &[Group], name: &str, tables: &[&Entry]) -> Option<Vec<Group>> {
    let target = groups.iter().position(|g| makes(g, name, tables))?;
    let mut g = groups[target].clone();
    if g.name() != name {
        g.look = Some(name.to_string());
    }
    if g.candidates() >= MIN_FRAMES {
        return Some(vec![g]);
    }
    let mut donor: Option<usize> = None;
    for (i, d) in groups.iter().enumerate() {
        if i == target || d.style != g.style || d.camera == g.camera || d.candidates() < MIN_FRAMES
        {
            continue;
        }
        if donor.is_none_or(|k| d.frames.len() > groups[k].frames.len()) {
            donor = Some(i);
        }
    }
    Some(match donor {
        Some(d) => vec![groups[d].clone(), g],
        None => vec![g],
    })
}

/// Why a group of `n` frames is not fitted, before any is developed.
fn too_few(n: usize) -> String {
    format!(
        "{n} frame{}, under the {MIN_FRAMES} a fit needs",
        if n == 1 { "" } else { "s" }
    )
}

/// Why a group whose frames were developed is not fitted.
fn under(n: usize) -> String {
    format!(
        "{n} usable frame{}, under the {MIN_FRAMES} a fit needs",
        if n == 1 { "" } else { "s" }
    )
}

/// Where a run is, and each group's result as it lands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Progress {
    /// Frame `index` (from one) of `of` in the group named.
    Frame {
        group: String,
        index: usize,
        of: usize,
    },
    Done(GroupResult),
}

/// What became of one group.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GroupResult {
    pub(crate) camera: String,
    pub(crate) style: String,
    /// The look's name in the store.
    pub(crate) name: String,
    pub(crate) outcome: Outcome,
    /// Frames developed and not used, and why.
    pub(crate) dropped: Vec<(PathBuf, String)>,
    /// Frames not sampled for an adaptive setting on.
    pub(crate) left_out: usize,
    /// The camera's vignetting against the lens profile's, per lens.
    pub(crate) radial: Vec<RadialLine>,
    /// Every frame from one folder.
    pub(crate) one_folder: bool,
    /// The warning when the table written replaced one the match would
    /// otherwise have left alone.
    pub(crate) replaced: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Fitted {
        frames: usize,
        /// Mean ΔE in Oklab over every kept block, fitted on all.
        fitted: f32,
        /// Mean over `held_frames` frames, each held out of its own
        /// fit.
        held_out: Option<f32>,
        held_frames: usize,
    },
    Borrowed {
        from: String,
        /// The borrowed table's mean ΔE on this body's frames, where
        /// any were developed.
        measured: Option<f32>,
    },
    Skipped(String),
    Failed(String),
    Canceled,
}

/// The lightness the look leaves, corner minus center, in Oklab, on
/// one lens: positive where the develop is lighter in the corners than
/// the camera's JPEG.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RadialLine {
    pub(crate) lens: String,
    pub(crate) corner_minus_center: f32,
    pub(crate) frames: usize,
}

impl GroupResult {
    /// One line for the sheet.
    pub(crate) fn line(&self) -> String {
        let mut out = match &self.outcome {
            Outcome::Fitted {
                frames,
                fitted,
                held_out,
                held_frames,
            } => match held_out {
                Some(h) => format!(
                    "{}: fitted on {frames} frames, ΔE {h:.3} held out over {held_frames} \
                     frames ({fitted:.3} fitted)",
                    self.name
                ),
                None => format!("{}: fitted on {frames} frames, ΔE {fitted:.3}", self.name),
            },
            Outcome::Borrowed { from, measured } => match measured {
                Some(m) => format!(
                    "{}: borrowed, fitted on {from}; ΔE {m:.3} on this body's frames",
                    self.name
                ),
                None => format!("{}: borrowed, fitted on {from}", self.name),
            },
            Outcome::Skipped(why) => {
                format!("{} {}: skipped, {why}", self.camera, style_word(self))
            }
            Outcome::Failed(why) => format!("{}: not written, {why}", self.name),
            Outcome::Canceled => format!("{}: canceled", self.name),
        };
        if self.left_out > 0 {
            out.push_str(&format!(
                "; {} left out with an adaptive setting on",
                self.left_out
            ));
        }
        if !self.dropped.is_empty() {
            out.push_str(&format!("; {} frames did not register", self.dropped.len()));
        }
        if self.one_folder && matches!(self.outcome, Outcome::Fitted { .. }) {
            out.push_str("; all from one folder, so a narrow fit");
        }
        if let Some(warning) = &self.replaced {
            out.push_str(&format!("; {warning}"));
        }
        for r in &self.radial {
            out.push_str(&format!(
                "\n    {}: corner minus center {:+.3} over {} frames",
                r.lens, r.corner_minus_center, r.frames
            ));
        }
        out
    }
}

fn style_word(r: &GroupResult) -> &str {
    r.style.split_once(' ').map_or(r.style.as_str(), |(_, s)| s)
}

/// One frame's block pairs, finished at the camera's exposure.
struct Kept {
    pairs: Pairs,
    lens: Option<String>,
}

/// The export settings a frame is developed under for the fit: a
/// 16-bit sRGB picture at [`LONG_EDGE`], no output sharpening, no
/// mark.
fn settings() -> crate::export::Settings {
    crate::export::Settings {
        format: crate::export::Format::Tiff,
        long_edge: Some(LONG_EDGE),
        space: crate::export::Space::Srgb,
        sharpen: crate::export::Sharpen::Off,
        metadata: crate::export::Metadata::None,
        watermark: None,
        ..Default::default()
    }
}

/// A rendered picture as the fit reads one: encoded sRGB in floats.
fn picture_of(rendered: &crate::export::Rendered) -> Picture {
    let (w, h) = (rendered.width as usize, rendered.height as usize);
    let data = match &rendered.pixels {
        crate::export::Pixels::Sixteen(p) => p
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&c| c.map(|v| v as f32 / 65535.0))
            .collect(),
        crate::export::Pixels::Eight(p) => p
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&c| c.map(|v| v as f32 / 255.0))
            .collect(),
    };
    Picture::new(w, h, data)
}

/// The camera's own JPEG in the raw, turned by its orientation as the
/// develop is.
fn camera_jpeg(path: &Path) -> Result<Picture, String> {
    let (rgb, orientation) = greycard_core::decode::preview_path(path)
        .map_err(|e| e.to_string())?
        .ok_or("the file carries no camera JPEG")?;
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let data = rgb.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    let turned = greycard_core::develop::orient(
        WorkingImage {
            width: w,
            height: h,
            data,
        },
        orientation,
    );
    let data = turned.data.as_chunks::<3>().0.to_vec();
    Ok(Picture::new(turned.width, turned.height, data))
}

/// The edit a frame is developed under for the fit: the default, on
/// the run's display curve. Before tables declared their curve this
/// was the default edit alone, so every table fitted then was fitted
/// under per channel whatever the open picture was on.
fn fit_edit(curve: DisplayCurve) -> Edit {
    Edit {
        display_curve: curve,
        ..Edit::default()
    }
}

/// A frame developed, its JPEG laid over it, and its pairs taken again
/// at the exposure the camera's JPEG sits at.
///
/// The frame is developed under [`fit_edit`] on `curve`: the table is
/// fitted on what that curve renders, so it is that curve's table and
/// is declared as such.
fn measure(
    frame: &Frame,
    curve: DisplayCurve,
    lenses: Option<&greycard_lens::Database>,
) -> Result<Kept, String> {
    let edit = fit_edit(curve);
    let settings = settings();
    let mut developed = crate::worker::FrameDevelop::develop(&frame.path, &edit, lenses)?;
    let render = picture_of(&developed.finish(&edit, &settings));
    let jpeg = camera_jpeg(&frame.path)?;
    let reg = register(&render, &jpeg);
    if reg.ncc < NCC_MIN {
        return Err(format!(
            "the camera's JPEG does not lay over the develop (correlation {:.2})",
            reg.ncc
        ));
    }
    let (w, h) = (render.width, render.height);
    let laid = jpeg.warp_to(w, h, reg.scale, reg.dy, reg.dx);
    let coverage = jpeg.coverage(w, h, reg.scale, reg.dy, reg.dx);
    let first = greycard_match::pairs::pairs(&render, &laid, &coverage);
    let offset =
        greycard_match::exposure::offset_stops(&first).ok_or("no flat, unclipped blocks")?;
    let mut at = edit.clone();
    at.light.exposure += offset;
    let render = picture_of(&developed.finish(&at, &settings));
    let pairs = greycard_match::pairs::pairs(&render, &laid, &coverage);
    if pairs.pairs.is_empty() {
        return Err("no flat, unclipped blocks".into());
    }
    tracing::info!(
        "camera match {}: scale {:.4}, shift ({:+.1}, {:+.1}), correlation {:.3}, \
         {} of {} blocks, {offset:+.2} stops",
        frame.path.display(),
        reg.scale,
        reg.dy,
        reg.dx,
        reg.ncc,
        pairs.pairs.len(),
        pairs.total
    );
    Ok(Kept {
        pairs,
        lens: frame.lens.clone(),
    })
}

/// Every kept frame's pairs as the fit takes them: the render side,
/// the camera side, and each pair's frame.
fn stacked(kept: &[Kept]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<usize>) {
    let (mut x, mut y, mut ids) = (vec![], vec![], vec![]);
    for (i, k) in kept.iter().enumerate() {
        for p in &k.pairs.pairs {
            x.push(p.render);
            y.push(p.jpeg);
            ids.push(i);
        }
    }
    (x, y, ids)
}

/// The radial term per lens over the kept frames, under `model`.
fn radial_lines(model: &Model, kept: &[Kept]) -> Vec<RadialLine> {
    let mut by_lens: BTreeMap<String, Vec<&Pairs>> = BTreeMap::new();
    for k in kept {
        let lens = k.lens.clone().unwrap_or_else(|| "an unnamed lens".into());
        by_lens.entry(lens).or_default().push(&k.pairs);
    }
    by_lens
        .into_iter()
        .filter_map(|(lens, sets)| {
            let r = greycard_match::radial::radial(model, &sets)?;
            Some(RadialLine {
                lens,
                corner_minus_center: r.corner_minus_center,
                frames: sets.len(),
            })
        })
        .collect()
}

/// Write `model` into `dir` as `<name>.cube` for the body of `group`:
/// its title naming the body it was fitted on and the frames the fit
/// used, as before, and its header declaring the display curve the run
/// developed under, the body and style it is for, and again the body
/// and frames, so nothing has to be read back out of the title. Written
/// beside, flushed to the disk and renamed, so the picker never reads
/// half a table and a crash never leaves one.
fn write_look(
    dir: &Path,
    group: &Group,
    fitted_on: &str,
    frames: usize,
    curve: DisplayCurve,
    model: &Model,
) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir)?;
    let name = group.name();
    let title = greycard_edit::look::fitted_title(&name, fitted_on, Some(frames));
    let declared = greycard_edit::look::fitted_declarations(
        curve,
        &group.make,
        &group.model,
        &group.style,
        fitted_on,
        frames,
    );
    let declared: Vec<(&str, &str)> = declared.iter().map(|(k, v)| (*k, v.as_str())).collect();
    // The look's table for this curve, beside any other curve's.
    let stem = variant_stem(&name, curve);
    let path = dir.join(format!("{stem}.cube"));
    let part = dir.join(format!("{stem}.cube.part"));
    let mut file = std::fs::File::create(&part)?;
    file.write_all(greycard_match::cube::cube_text(model, &title, &declared).as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, &path)?;
    Ok(path)
}

/// What a run is under: the display curve it develops and writes for,
/// whether it replaces what it would leave alone, and which groups the
/// user chose (one flag a group; a missing flag is a yes).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Choices<'a> {
    pub(crate) curve: DisplayCurve,
    pub(crate) replace: bool,
    pub(crate) chosen: &'a [bool],
}

/// Run the camera match over `groups`, writing into the look store at
/// `store`: each group with enough frames developed, fitted and
/// written, then each without enough given a borrowed table or a
/// reason. `progress` hears every frame and every group's result;
/// `cancel` is looked at between frames, and a canceled run writes
/// nothing more.
pub(crate) fn run(
    groups: &[Group],
    store: &Path,
    choices: Choices,
    lenses: Option<&greycard_lens::Database>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Vec<GroupResult> {
    run_with(groups, store, choices, cancel, progress, &mut |frame| {
        measure(frame, choices.curve, lenses)
    })
}

/// [`run`], each frame measured by `measure`: the develop and the
/// registration for a run, a synthetic picture's pairs for a test.
fn run_with(
    groups: &[Group],
    store: &Path,
    choices: Choices,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
    measure: &mut dyn FnMut(&Frame) -> Result<Kept, String>,
) -> Vec<GroupResult> {
    let Choices {
        curve,
        replace,
        chosen,
    } = choices;
    let existing = existing_in(store);
    let plans = plan_chosen(groups, &existing, curve, replace, chosen);
    let mut results: Vec<Option<GroupResult>> = vec![None; groups.len()];
    // The models fitted and written, by group, with their frames.
    let mut fitted: HashMap<usize, (Model, usize)> = HashMap::new();
    let canceled = |g: &Group| GroupResult {
        camera: g.camera.clone(),
        style: g.style.clone(),
        name: g.name(),
        outcome: Outcome::Canceled,
        dropped: Vec::new(),
        left_out: g.left_out(),
        radial: Vec::new(),
        one_folder: g.one_folder(),
        replaced: None,
    };
    // The groups a fit is planned for, first: a borrower needs them.
    let reads = |i: usize| matches!(plans[i], Plan::Fit { .. } | Plan::Donor);
    let order: Vec<usize> = (0..groups.len())
        .filter(|&i| reads(i))
        .chain((0..groups.len()).filter(|&i| !reads(i)))
        .collect();
    let mut kept_for: HashMap<usize, Vec<Kept>> = HashMap::new();
    for &i in &order {
        let g = &groups[i];
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let name = g.name();
        let mut result = GroupResult {
            camera: g.camera.clone(),
            style: g.style.clone(),
            name: name.clone(),
            outcome: Outcome::Canceled,
            dropped: Vec::new(),
            left_out: g.left_out(),
            radial: Vec::new(),
            one_folder: g.one_folder(),
            replaced: None,
        };
        // A group to be skipped is not developed; a borrower is, when
        // its donor was fitted, to measure the borrowed table on its
        // own frames.
        let develop = match &plans[i] {
            Plan::Fit { .. } | Plan::Donor => true,
            Plan::Borrow { donor, .. } => fitted.contains_key(donor),
            Plan::Skip(_) => false,
        };
        let mut kept = Vec::new();
        if develop {
            let frames = sample(&g.frames, SAMPLE);
            let of = frames.len();
            for (k, frame) in frames.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                progress(Progress::Frame {
                    group: name.clone(),
                    index: k + 1,
                    of,
                });
                match measure(frame) {
                    Ok(m) => kept.push(m),
                    Err(why) => {
                        tracing::info!("camera match {}: {why}", frame.path.display());
                        result.dropped.push((frame.path.clone(), why));
                    }
                }
            }
            if cancel.load(Ordering::Relaxed) {
                results[i] = Some(canceled(g));
                break;
            }
        }
        // A group that is not chosen, read for a borrower: its table
        // is fitted in memory and nothing of it is written.
        if matches!(plans[i], Plan::Donor) {
            if kept.len() >= MIN_FRAMES {
                let (x, y, _) = stacked(&kept);
                let model = Model::fit(&x, &y, LutParams::default());
                fitted.insert(i, (model, kept.len()));
                result.outcome = Outcome::Skipped(
                    "not chosen; its frames were read for the look a chosen body borrows, \
                     and no table of its own was written"
                        .to_string(),
                );
            } else {
                result.outcome = Outcome::Skipped(under(kept.len()));
            }
            results[i] = Some(result);
            continue;
        }
        // The store is asked again with the frames the fit really has:
        // the plan counted the candidates, some of which may not have
        // registered.
        let allowed = plans[i].is_fit().then(|| {
            may_write(
                existing.get(&variant_stem(&name, curve)),
                &g.camera,
                false,
                kept.len(),
                curve,
                replace,
            )
        });
        if let Some(Ok(replaces)) = allowed.clone()
            && kept.len() >= MIN_FRAMES
        {
            let (x, y, ids) = stacked(&kept);
            let params = LutParams::default();
            let model = Model::fit(&x, &y, params);
            let fitted_de = model.mean_delta_e(&x, &y);
            let chosen = evenly(kept.len(), HELD_OUT);
            let held = leave_one_out(&x, &y, &ids, &chosen, params);
            let held_out = (!held.is_empty()).then(|| held.iter().sum::<f32>() / held.len() as f32);
            result.radial = radial_lines(&model, &kept);
            result.replaced = replaces;
            result.outcome = match write_look(store, g, &g.camera, kept.len(), curve, &model) {
                Ok(path) => {
                    tracing::info!(
                        "camera match: {} from {} frames, ΔE {fitted_de:.4}",
                        path.display(),
                        kept.len()
                    );
                    fitted.insert(i, (model, kept.len()));
                    Outcome::Fitted {
                        frames: kept.len(),
                        fitted: fitted_de,
                        held_out,
                        held_frames: held.len(),
                    }
                }
                Err(e) => Outcome::Failed(e.to_string()),
            };
            progress(Progress::Done(result.clone()));
            results[i] = Some(result);
        } else {
            match allowed {
                Some(Err(why)) if kept.len() >= MIN_FRAMES => {
                    result.outcome = Outcome::Skipped(why);
                }
                Some(_) => result.outcome = Outcome::Skipped(under(kept.len())),
                None => {}
            }
            kept_for.insert(i, kept);
            results[i] = Some(result);
        }
    }
    // The groups with no table of their own: a borrowed one where a
    // body in this run has the style fitted, else the reason.
    for &i in &order {
        let g = &groups[i];
        let Some(result) = results[i].as_mut() else {
            continue;
        };
        if matches!(result.outcome, Outcome::Fitted { .. } | Outcome::Failed(_)) {
            continue;
        }
        let donor = match &plans[i] {
            Plan::Borrow {
                from,
                donor,
                replaces,
            } => fitted
                .get(donor)
                .map(|(model, frames)| (from, model, *frames, replaces)),
            _ => None,
        };
        if cancel.load(Ordering::Relaxed) {
            result.outcome = Outcome::Canceled;
        } else if let Some((from, model, frames, replaces)) = donor {
            let kept = kept_for.remove(&i).unwrap_or_default();
            let measured = (!kept.is_empty()).then(|| {
                let (x, y, _) = stacked(&kept);
                model.mean_delta_e(&x, &y)
            });
            result.radial = radial_lines(model, &kept);
            result.replaced = replaces.clone();
            result.outcome = match write_look(store, g, from, frames, curve, model) {
                Ok(_) => Outcome::Borrowed {
                    from: from.clone(),
                    measured,
                },
                Err(e) => Outcome::Failed(e.to_string()),
            };
        } else if let Outcome::Canceled = result.outcome {
            result.outcome = match &plans[i] {
                Plan::Skip(why) => Outcome::Skipped(why.clone()),
                Plan::Borrow { from, .. } => Outcome::Skipped(format!(
                    "{}, and the fit on {from} it would borrow was not written",
                    too_few(g.candidates()),
                )),
                Plan::Fit { .. } => Outcome::Skipped(too_few(g.candidates())),
                Plan::Donor => Outcome::Skipped(NOT_CHOSEN.to_string()),
            };
        }
        progress(Progress::Done(result.clone()));
    }
    groups
        .iter()
        .zip(results)
        .map(|(g, r)| r.unwrap_or_else(|| canceled(g)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CH: DisplayCurve = DisplayCurve::Channels;

    fn frame(folder: &str, day: usize, fixed: bool) -> Frame {
        Frame {
            path: PathBuf::from(format!("/shots/{folder}/{day:03}.CR3")),
            taken: Some(format!("2026-01-01 00:{:02}:{:02}", day / 60, day % 60)),
            lens: Some("RF50mm F1.2 L USM".into()),
            fixed,
        }
    }

    fn folders_of(frames: &[Frame]) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for f in frames {
            *out.entry(f.folder().display().to_string()).or_default() += 1;
        }
        out
    }

    #[test]
    fn a_small_group_is_taken_whole() {
        let frames: Vec<Frame> = (0..12).map(|i| frame("a", i, i % 3 != 0)).collect();
        let s = sample(&frames, SAMPLE);
        assert_eq!(s.len(), 12);
        // By date.
        assert!(s.windows(2).all(|w| w[0].taken <= w[1].taken));
    }

    #[test]
    fn the_sample_reaches_every_folder_and_spreads_through_the_dates() {
        // One long session and four short ones.
        let mut frames: Vec<Frame> = (0..200).map(|i| frame("long", i, true)).collect();
        for (k, f) in ["b", "c", "d", "e"].iter().enumerate() {
            frames.extend((0..5).map(|i| frame(f, 300 + 10 * k + i, true)));
        }
        let s = sample(&frames, SAMPLE);
        assert_eq!(s.len(), SAMPLE);
        let per = folders_of(&s);
        assert_eq!(per.len(), 5, "{per:?}");
        for (folder, n) in &per {
            assert!(*n >= PER_FOLDER, "{folder}: {n}");
        }
        // The long session is spread over, not its first forty.
        let long: Vec<&Frame> = s
            .iter()
            .filter(|f| f.path.starts_with("/shots/long"))
            .collect();
        assert!(long.last().unwrap().taken > frames[150].taken, "{long:?}");
        // No frame twice.
        let unique: BTreeSet<&PathBuf> = s.iter().map(|f| &f.path).collect();
        assert_eq!(unique.len(), s.len());
    }

    #[test]
    fn with_enough_fixed_frames_the_adaptive_ones_are_left_out() {
        let frames: Vec<Frame> = (0..60).map(|i| frame("a", i, i % 2 == 0)).collect();
        let s = sample(&frames, SAMPLE);
        assert_eq!(s.len(), 30, "the thirty fixed frames, and no more");
        assert!(s.iter().all(|f| f.fixed));
        let g = Group {
            camera: "Canon EOS R6m2".into(),
            make: "Canon".into(),
            model: "Canon EOS R6m2".into(),
            maker: "Canon".into(),
            style: "Canon Faithful".into(),
            frames,
            look: None,
        };
        assert_eq!(g.left_out(), 30);
        assert!(g.one_folder());
    }

    #[test]
    fn under_twenty_fixed_frames_every_fixed_one_and_the_rest_adaptive() {
        let frames: Vec<Frame> = (0..80).map(|i| frame("a", i, i % 8 == 0)).collect();
        let s = sample(&frames, SAMPLE);
        assert_eq!(s.len(), SAMPLE);
        assert_eq!(s.iter().filter(|f| f.fixed).count(), 10);
    }

    #[test]
    fn a_look_is_named_for_the_body_and_the_style_once() {
        assert_eq!(
            look_name("Canon EOS R6m2", "Canon", "Canon Faithful"),
            "Canon EOS R6m2 Faithful"
        );
        assert_eq!(
            look_name("Fujifilm GFX100S II", "Fujifilm", "Fujifilm Reala Ace"),
            "Fujifilm GFX100S II Reala Ace"
        );
        // A body not spelled with the maker keeps the whole key.
        assert_eq!(
            look_name("ILCE-7M4", "Sony", "Sony Standard"),
            "ILCE-7M4 Sony Standard"
        );
        // Nothing a file name cannot hold, and nothing that climbs.
        assert_eq!(
            look_name("Odd/Cam: 2", "Canon", "Canon User Def. 1"),
            "Odd Cam 2 Canon User Def. 1"
        );
        assert_eq!(sanitize("../..//x"), "x");
        assert_eq!(sanitize("  "), "Camera look");
        assert_eq!(sanitize("a..b"), "a.b");
        let name = look_name("Canon EOS R5", "Canon", "Canon Unknown (0x87)");
        assert!(greycard_edit::look::path_for(&name).is_some(), "{name}");
    }

    #[test]
    fn the_plan_fits_borrows_and_skips() {
        let group = |camera: &str, style: &str, n: usize| Group {
            camera: camera.into(),
            make: "Canon".into(),
            model: camera.into(),
            maker: "Canon".into(),
            style: style.into(),
            frames: (0..n).map(|i| frame(camera, i, true)).collect(),
            look: None,
        };
        let groups = vec![
            group("Canon EOS R6m2", "Canon Faithful", 50),
            group("Canon EOS R5m2", "Canon Faithful", 8),
            group("Canon EOS R5m2", "Canon Standard", 8),
        ];
        let p = plan(&groups, &HashMap::new(), DisplayCurve::Channels, false);
        assert_eq!(p[0], Plan::Fit { replaces: None });
        assert_eq!(
            p[1],
            Plan::Borrow {
                from: "Canon EOS R6m2".into(),
                donor: 0,
                replaces: None,
            }
        );
        assert!(matches!(&p[2], Plan::Skip(why) if why.contains("8 frames")));
    }

    /// A run over groups that are all under the minimum develops
    /// nothing, writes nothing, and says why for each.
    #[test]
    fn a_run_with_too_few_frames_skips_and_says_so() {
        let dir = std::env::temp_dir().join(format!("greycard-match-run-{}", std::process::id()));
        let groups = vec![Group {
            camera: "Canon EOS R6m2".into(),
            make: "Canon".into(),
            model: "Canon EOS R6m2".into(),
            maker: "Canon".into(),
            style: "Canon Faithful".into(),
            frames: (0..3).map(|i| frame("a", i, true)).collect(),
            look: None,
        }];
        let mut heard = Vec::new();
        let results = run(
            &groups,
            &dir,
            Choices {
                curve: DisplayCurve::Channels,
                replace: false,
                chosen: &[],
            },
            None,
            &AtomicBool::new(false),
            &mut |p| heard.push(p),
        );
        assert_eq!(results.len(), 1);
        assert!(
            matches!(&results[0].outcome, Outcome::Skipped(why) if why.contains("3 frames")),
            "{:?}",
            results[0]
        );
        assert_eq!(heard, [Progress::Done(results[0].clone())]);
        assert!(!dir.exists());
        assert!(
            results[0].line().contains("skipped"),
            "{}",
            results[0].line()
        );
    }

    /// The synthetic camera: the develop through a fixed rendering, a
    /// gentle curve and a warm cast.
    fn camera_of(x: [f32; 3]) -> [f32; 3] {
        use greycard_match::color::{decode3, encode3};
        let lin = decode3(x);
        let warm = [lin[0] * 1.06, lin[1], lin[2] * 0.92];
        encode3(warm.map(|v| v.clamp(0.0, 1.0).powf(0.95)))
    }

    /// One synthetic frame's block pairs over a grid, spread through
    /// the cube by `seed`.
    fn synthetic_pairs(seed: usize) -> Pairs {
        let (rows, cols) = (8usize, 12usize);
        let mut pairs = Vec::new();
        for row in 0..rows {
            for col in 0..cols {
                let i = (seed * 97 + row * cols + col) as f32;
                let render = [
                    0.08 + 0.8 * (i * 0.618_034).fract(),
                    0.08 + 0.8 * (i * 0.414_214).fract(),
                    0.08 + 0.8 * (i * 0.267_949).fract(),
                ];
                pairs.push(greycard_match::Pair {
                    render,
                    jpeg: camera_of(render),
                    row,
                    col,
                });
            }
        }
        Pairs {
            pairs,
            rows,
            cols,
            total: rows * cols,
        }
    }

    fn synthetic_group(camera: &str, style: &str, n: usize) -> Group {
        Group {
            camera: camera.into(),
            make: "Canon".into(),
            model: camera.into(),
            maker: "Canon".into(),
            style: style.into(),
            frames: (0..n)
                .map(|i| Frame {
                    lens: Some("RF50mm F1.2 L USM".into()),
                    ..frame(if i % 2 == 0 { "a" } else { "b" }, i, true)
                })
                .collect(),
            look: None,
        }
    }

    fn scratch_store(what: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "greycard-match-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// A run over `groups` whose every frame measures as a synthetic
    /// frame, except the ones `fails` names by index in the run.
    fn run_synthetic(
        groups: &[Group],
        store: &Path,
        replace: bool,
        fails: &[usize],
    ) -> (Vec<GroupResult>, Vec<Progress>) {
        run_synthetic_under(groups, store, DisplayCurve::Channels, replace, fails)
    }

    /// [`run_synthetic`] under a display curve.
    fn run_synthetic_under(
        groups: &[Group],
        store: &Path,
        curve: DisplayCurve,
        replace: bool,
        fails: &[usize],
    ) -> (Vec<GroupResult>, Vec<Progress>) {
        run_synthetic_chosen(groups, store, curve, replace, &[], fails)
    }

    /// [`run_synthetic_under`], with the groups the user chose.
    fn run_synthetic_chosen(
        groups: &[Group],
        store: &Path,
        curve: DisplayCurve,
        replace: bool,
        chosen: &[bool],
        fails: &[usize],
    ) -> (Vec<GroupResult>, Vec<Progress>) {
        let mut heard = Vec::new();
        let mut seed = 0;
        let results = run_with(
            groups,
            store,
            Choices {
                curve,
                replace,
                chosen,
            },
            &AtomicBool::new(false),
            &mut |p| heard.push(p),
            &mut |_| {
                seed += 1;
                if fails.contains(&seed) {
                    return Err("did not register".into());
                }
                Ok(Kept {
                    pairs: synthetic_pairs(seed),
                    lens: Some("RF50mm F1.2 L USM".into()),
                })
            },
        );
        (results, heard)
    }

    /// A table in `store` as a user would put one there, or as an
    /// earlier run would have written one.
    fn put_table(store: &Path, name: &str, title: &str) {
        std::fs::create_dir_all(store).unwrap();
        let mut text = format!("TITLE \"{title}\"\nLUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    text.push_str(&format!("{r} {g} {b}\n"));
                }
            }
        }
        std::fs::write(store.join(format!("{name}.cube")), text).unwrap();
    }

    /// A table as an earlier run of the match wrote one: its line and
    /// a title in its shape.
    fn put_fitted(store: &Path, name: &str, title: &str) {
        put_table(store, name, title);
        let path = store.join(format!("{name}.cube"));
        let text = std::fs::read_to_string(&path).unwrap().replacen(
            "LUT_3D_SIZE",
            &format!("# {}\nLUT_3D_SIZE", greycard_edit::look::FITTED),
            1,
        );
        std::fs::write(path, text).unwrap();
    }

    fn title_in(store: &Path, name: &str) -> Option<String> {
        greycard_core::lut::Lut3d::load(&store.join(format!("{name}.cube")))
            .unwrap()
            .title
    }

    /// Twenty-four synthetic frames of one body fit and write a table
    /// that reproduces the rendering; five of another body in the same
    /// style borrow it under their own name, the title saying where it
    /// was fitted; a third style with five frames is skipped.
    #[test]
    fn a_run_fits_borrows_and_skips_on_synthetic_frames() {
        let groups = vec![
            synthetic_group("Canon EOS R5m2", "Canon Faithful", 5),
            synthetic_group("Canon EOS R6m2", "Canon Faithful", 24),
            synthetic_group("Canon EOS R6m2", "Canon Standard", 5),
        ];
        let store = scratch_store("store");
        let (results, heard) = run_synthetic(&groups, &store, false, &[]);
        // The fit, on its own body, reproducing the rendering.
        let Outcome::Fitted {
            frames,
            fitted,
            held_out,
            held_frames,
        } = results[1].outcome
        else {
            panic!("{:?}", results[1]);
        };
        assert_eq!(frames, 24);
        assert!(fitted < 0.004, "fitted ΔE {fitted}");
        assert!(held_out.unwrap() < 0.006, "held out {held_out:?}");
        assert_eq!(held_frames, HELD_OUT);
        assert!(
            results[1].line().contains("held out over 4 frames"),
            "{}",
            results[1].line()
        );
        assert_eq!(results[1].radial.len(), 1);
        assert_eq!(results[1].radial[0].lens, "RF50mm F1.2 L USM");
        // The borrower, measured on its own frames.
        let Outcome::Borrowed { from, measured } = &results[0].outcome else {
            panic!("{:?}", results[0]);
        };
        assert_eq!(from, "Canon EOS R6m2");
        assert!(measured.unwrap() < 0.006, "{measured:?}");
        assert!(
            matches!(&results[2].outcome, Outcome::Skipped(_)),
            "{:?}",
            results[2]
        );
        // Every group was heard from, and the frames of the two that
        // were developed.
        let done = heard
            .iter()
            .filter(|p| matches!(p, Progress::Done(_)))
            .count();
        assert_eq!(done, 3);
        let frames_heard = heard
            .iter()
            .filter(|p| matches!(p, Progress::Frame { .. }))
            .count();
        assert_eq!(frames_heard, 24 + 5);
        // The store holds two tables, each titled with where it was
        // fitted and from how many frames, and the fitted one renders
        // as the camera does.
        let listed = greycard_core::lut::list_dir(&store);
        let titles: Vec<(String, Option<String>)> = listed
            .iter()
            .map(|(name, _, info)| (name.clone(), info.as_ref().unwrap().title.clone()))
            .collect();
        assert_eq!(
            titles,
            [
                (
                    "Canon EOS R5m2 Faithful".to_string(),
                    Some(
                        "Canon EOS R5m2 Faithful (fitted on Canon EOS R6m2, 24 frames)".to_string()
                    )
                ),
                (
                    "Canon EOS R6m2 Faithful".to_string(),
                    Some(
                        "Canon EOS R6m2 Faithful (fitted on Canon EOS R6m2, 24 frames)".to_string()
                    )
                ),
            ]
        );
        let table =
            greycard_core::lut::Lut3d::load(&store.join("Canon EOS R6m2 Faithful.cube")).unwrap();
        assert_eq!(table.size, greycard_match::LUT_SIZE);
        // The table does to a color what the camera does: a node
        // inside the data, red fastest.
        let n = table.size;
        let (r, g, b) = (16usize, 13, 10);
        let input = [r, g, b].map(|v| v as f32 / (n - 1) as f32);
        let want = camera_of(input);
        let got = table.data[(b * n + g) * n + r];
        for c in 0..3 {
            assert!((got[c] - want[c]).abs() < 0.01, "{got:?} against {want:?}");
        }
        crate::testing::remove_dir_retry(&store);
    }

    /// An unchecked group is skipped as a small one is: not developed,
    /// not written, and said. A checked group that borrows from an
    /// unchecked donor still borrows, the donor read and fitted in
    /// memory and its own table not written.
    #[test]
    fn an_unchecked_group_is_neither_fitted_nor_written() {
        let groups = vec![
            synthetic_group("Canon EOS R5m2", "Canon Faithful", 5),
            synthetic_group("Canon EOS R6m2", "Canon Faithful", 24),
            synthetic_group("Canon EOS R6m2", "Canon Standard", 30),
        ];
        let none = HashMap::new();
        let ch = DisplayCurve::Channels;
        // Everything checked: as before.
        let p = plan_chosen(&groups, &none, ch, false, &[true, true, true]);
        assert!(matches!(p[0], Plan::Borrow { donor: 1, .. }));
        assert!(p[1].is_fit() && p[2].is_fit());
        // The Standard group unchecked is skipped; nothing else moves.
        let p = plan_chosen(&groups, &none, ch, false, &[true, true, false]);
        assert!(matches!(&p[2], Plan::Skip(why) if why == NOT_CHOSEN));
        assert!(p[1].is_fit());
        // The donor unchecked: the borrower still borrows from it, and
        // the donor is read only.
        let p = plan_chosen(&groups, &none, ch, false, &[true, false, true]);
        assert!(matches!(p[0], Plan::Borrow { donor: 1, .. }), "{p:?}");
        assert_eq!(p[1], Plan::Donor);
        // Nobody borrows from it: it is skipped, not read.
        let p = plan_chosen(&groups, &none, ch, false, &[false, false, true]);
        assert!(matches!(&p[0], Plan::Skip(w) if w == NOT_CHOSEN));
        assert!(matches!(&p[1], Plan::Skip(w) if w == NOT_CHOSEN));
        // A borrower unchecked borrows nothing.
        let p = plan_chosen(&groups, &none, ch, false, &[false, true, true]);
        assert!(matches!(&p[0], Plan::Skip(w) if w == NOT_CHOSEN));

        // Run it: only the checked group's table is written, the frames
        // of the skipped ones are never developed.
        let store = scratch_store("chosen");
        let (results, heard) =
            run_synthetic_chosen(&groups, &store, ch, false, &[false, true, false], &[]);
        assert!(matches!(results[1].outcome, Outcome::Fitted { .. }));
        assert!(matches!(&results[0].outcome, Outcome::Skipped(w) if w == NOT_CHOSEN));
        assert!(matches!(&results[2].outcome, Outcome::Skipped(w) if w == NOT_CHOSEN));
        let developed = heard
            .iter()
            .filter(|p| matches!(p, Progress::Frame { .. }))
            .count();
        assert_eq!(developed, 24);
        let names: Vec<String> = greycard_core::lut::list_dir(&store)
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        assert_eq!(names, ["Canon EOS R6m2 Faithful"]);
        crate::testing::remove_dir_retry(&store);

        // The donor unchecked, its borrower checked: the borrower's
        // table is written, from the donor's frames, and the donor's
        // is not.
        let store = scratch_store("donor");
        let (results, heard) =
            run_synthetic_chosen(&groups, &store, ch, false, &[true, false, false], &[]);
        assert!(
            matches!(&results[0].outcome, Outcome::Borrowed { from, .. } if from == "Canon EOS R6m2"),
            "{:?}",
            results[0]
        );
        assert!(matches!(&results[1].outcome, Outcome::Skipped(w) if w.contains("not chosen")));
        let developed = heard
            .iter()
            .filter(|p| matches!(p, Progress::Frame { .. }))
            .count();
        assert_eq!(developed, 24 + 5);
        let names: Vec<String> = greycard_core::lut::list_dir(&store)
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        assert_eq!(names, ["Canon EOS R5m2 Faithful"]);
        crate::testing::remove_dir_retry(&store);
    }

    /// The store's rules, one case each: what is there, and whether a
    /// fit or a borrow for this body may write over it.
    #[test]
    fn the_store_decides_what_a_run_may_replace() {
        let own_fit = |frames| Existing::Fitted {
            on: "Canon EOS R6m2".into(),
            frames: Some(frames),
            made_for: MadeFor::Curve(CH),
        };
        let r6 = "Canon EOS R6m2";
        // (a) A fit of this body replaces a fit of this body from no
        // more frames, and is refused over one from more.
        assert_eq!(
            may_write(Some(&own_fit(30)), r6, false, 30, CH, false),
            Ok(None)
        );
        assert_eq!(
            may_write(Some(&own_fit(30)), r6, false, 40, CH, false),
            Ok(None)
        );
        let refused = may_write(Some(&own_fit(30)), r6, false, 24, CH, false).unwrap_err();
        assert!(
            refused.contains("from 30 frames, more than 24"),
            "{refused}"
        );
        // A title from before the count is replaced by any fit.
        let untold = Existing::Fitted {
            on: r6.into(),
            frames: None,
            made_for: MadeFor::Curve(CH),
        };
        assert_eq!(may_write(Some(&untold), r6, false, 20, CH, false), Ok(None));
        // A table borrowed from another body is replaced by a fit and
        // by another borrow.
        let borrowed = Existing::Fitted {
            on: "Canon EOS R5m2".into(),
            frames: Some(60),
            made_for: MadeFor::Curve(CH),
        };
        assert_eq!(
            may_write(Some(&borrowed), r6, false, 20, CH, false),
            Ok(None)
        );
        assert_eq!(may_write(Some(&borrowed), r6, true, 0, CH, false), Ok(None));
        // (b) A borrow never replaces this body's own fit.
        let refused = may_write(Some(&own_fit(20)), r6, true, 0, CH, false).unwrap_err();
        assert!(refused.contains("fitted on this body itself"), "{refused}");
        // (c) Nothing replaces a table the match did not write.
        let refused = may_write(Some(&Existing::Own), r6, false, 40, CH, false).unwrap_err();
        assert!(refused.contains("did not write"), "{refused}");
        assert!(may_write(Some(&Existing::Own), r6, true, 0, CH, false).is_err());
        // With "Replace existing looks" on, each refusal is a
        // replacement with its warning.
        for (existing, borrow, frames) in [
            (own_fit(30), false, 24),
            (own_fit(20), true, 0),
            (Existing::Own, false, 40),
        ] {
            let warned = may_write(Some(&existing), r6, borrow, frames, CH, true).unwrap();
            assert!(warned.unwrap().starts_with("replaces"));
        }
        // Nothing there: write.
        assert_eq!(may_write(None, r6, true, 0, CH, false), Ok(None));
        // (d) A table that declares another curve where this curve's
        // would go (one tagged by hand) is left alone, as a user's own
        // is; a run never writes over another curve's table.
        let agx = DisplayCurve::Agx;
        for (borrow, frames) in [(false, 40), (true, 0)] {
            let refused =
                may_write(Some(&own_fit(30)), r6, borrow, frames, agx, false).unwrap_err();
            assert!(
                refused.starts_with("the table there was fitted under per channel"),
                "{refused}"
            );
        }
        assert!(may_write(Some(&Existing::Own), r6, false, 40, agx, false).is_err());
    }

    /// A run under a display curve develops every frame under it and
    /// writes it into the table's header with the body the table is
    /// for, its style, and the body and frames it was fitted on; the
    /// finish then applies the table only under that curve. A borrow
    /// says the body it is for and the body it was fitted on apart.
    #[test]
    fn a_table_declares_the_curve_and_the_body_it_was_fitted_for() {
        use greycard_core::lut::declared;
        use greycard_edit::look::key;
        let groups = vec![
            synthetic_group("Canon EOS R6m2", "Canon Faithful", 22),
            synthetic_group("Canon EOS R5m2", "Canon Faithful", 5),
        ];
        let store = scratch_store("declared");
        let (results, _) = run_synthetic_under(&groups, &store, DisplayCurve::Agx, false, &[]);
        assert!(matches!(results[0].outcome, Outcome::Fitted { .. }));
        let read = |name: &str| {
            greycard_core::lut::Lut3d::load(&store.join(format!("{name}.cube")))
                .unwrap()
                .comments
        };
        // Under AgX each look's table goes beside its own file, as its
        // AgX variant.
        let own = read("Canon EOS R6m2 Faithful.agx");
        assert!(!store.join("Canon EOS R6m2 Faithful.cube").exists());
        assert!(greycard_edit::look::is_fitted(&own));
        assert_eq!(declared(&own, key::DISPLAY_CURVE), Some("agx"));
        assert_eq!(declared(&own, key::MAKE), Some("Canon"));
        assert_eq!(declared(&own, key::MODEL), Some("Canon EOS R6m2"));
        assert_eq!(declared(&own, key::STYLE), Some("Canon Faithful"));
        assert_eq!(declared(&own, key::FITTED_ON), Some("Canon EOS R6m2"));
        assert_eq!(declared(&own, key::FRAMES), Some("22"));
        assert_eq!(MadeFor::read(&own), MadeFor::Curve(DisplayCurve::Agx));
        let borrowed = read("Canon EOS R5m2 Faithful.agx");
        assert_eq!(declared(&borrowed, key::MODEL), Some("Canon EOS R5m2"));
        assert_eq!(declared(&borrowed, key::FITTED_ON), Some("Canon EOS R6m2"));
        assert_eq!(declared(&borrowed, key::DISPLAY_CURVE), Some("agx"));
        // The store reads the header back, not the title.
        let existing = existing_in(&store);
        assert_eq!(
            existing.get("Canon EOS R5m2 Faithful.agx"),
            Some(&Existing::Fitted {
                on: "Canon EOS R6m2".into(),
                frames: Some(22),
                made_for: MadeFor::Curve(DisplayCurve::Agx),
            })
        );
        // A run under per channel writes each look's own file beside
        // its AgX table, and the AgX tables are as they were: one look
        // name, a table per curve.
        let p = plan(&groups, &existing, CH, false);
        assert_eq!(p[0], Plan::Fit { replaces: None });
        let before = std::fs::read(store.join("Canon EOS R6m2 Faithful.agx.cube")).unwrap();
        let (results, _) = run_synthetic(&groups, &store, false, &[]);
        assert!(matches!(results[0].outcome, Outcome::Fitted { .. }));
        assert!(matches!(results[1].outcome, Outcome::Borrowed { .. }));
        assert_eq!(
            std::fs::read(store.join("Canon EOS R6m2 Faithful.agx.cube")).unwrap(),
            before
        );
        let mut files: Vec<String> = greycard_core::lut::list_dir(&store)
            .into_iter()
            .map(|(stem, _, _)| stem)
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "Canon EOS R5m2 Faithful",
                "Canon EOS R5m2 Faithful.agx",
                "Canon EOS R6m2 Faithful",
                "Canon EOS R6m2 Faithful.agx",
            ]
        );
        assert_eq!(
            MadeFor::read(&read("Canon EOS R6m2 Faithful")),
            MadeFor::Curve(CH)
        );
        // And the look list reads them as two looks of two tables.
        let entries: Vec<Entry> = greycard_core::lut::list_dir(&store)
            .into_iter()
            .map(|(stem, path, info)| greycard_edit::look::entry_of(&stem, path, info.unwrap()))
            .collect();
        assert_eq!(
            greycard_edit::look::curves_of(&entries, "Canon EOS R6m2 Faithful"),
            [CH, DisplayCurve::Agx]
        );
        crate::testing::remove_dir_retry(&store);
    }

    /// The match writes the line that tells a fitted table apart, and
    /// the edit crate, which cannot depend on the match, reads the
    /// same text.
    #[test]
    fn the_fitted_line_is_the_same_text_on_both_sides() {
        assert_eq!(greycard_match::cube::FITTED, greycard_edit::look::FITTED);
    }

    /// A table as the look list has it, from its title and header.
    fn table(name: &str, title: &str, header: &[&str]) -> Entry {
        greycard_edit::look::entry_of(
            name,
            PathBuf::new(),
            greycard_core::lut::Info {
                title: Some(title.into()),
                size: 33,
                encoding: Default::default(),
                primaries: Default::default(),
                kind: greycard_core::lut::Kind::Cube3d,
                comments: std::iter::once(greycard_edit::look::FITTED)
                    .chain(header.iter().copied())
                    .map(str::to_string)
                    .collect(),
            },
        )
    }

    /// A refit runs over the look's own group when it has the frames,
    /// else over it and the donor it borrows from. The group is found
    /// by the look's name, by the name in its title, or by the body and
    /// style its header declares, and a look found by anything but its
    /// name is written under its own name. A look nothing in the scope
    /// makes has no refit there.
    #[test]
    fn a_refit_runs_over_the_looks_group_and_its_donor() {
        let groups = vec![
            synthetic_group("Canon EOS R5", "Canon Faithful", 21),
            synthetic_group("Canon EOS R6m2", "Canon Faithful", 23),
            synthetic_group("Canon EOS R6m2", "Canon Standard", 30),
            synthetic_group("Canon EOS R5m2", "Canon Faithful", 5),
            synthetic_group("Canon EOS R7", "Canon Standard", 5),
        ];
        let names =
            |gs: Option<Vec<Group>>| gs.map(|gs| gs.iter().map(Group::name).collect::<Vec<_>>());
        let old = |name: &str, camera: &str| {
            table(
                name,
                &format!("{name} (fitted on {camera}, 40 frames)"),
                &[],
            )
        };
        let refit = |name: &str, tables: &[Entry]| {
            let tables: Vec<&Entry> = tables.iter().collect();
            names(refit_groups(&groups, name, &tables))
        };
        assert_eq!(
            refit(
                "Canon EOS R6m2 Faithful",
                &[old("Canon EOS R6m2 Faithful", "Canon EOS R6m2")]
            ),
            Some(vec!["Canon EOS R6m2 Faithful".to_string()])
        );
        assert_eq!(
            refit(
                "Canon EOS R5m2 Faithful",
                &[old("Canon EOS R5m2 Faithful", "Canon EOS R6m2")]
            ),
            Some(vec![
                "Canon EOS R6m2 Faithful".to_string(),
                "Canon EOS R5m2 Faithful".to_string()
            ])
        );
        assert_eq!(
            refit("Canon EOS R7 Standard", &[]),
            Some(vec![
                "Canon EOS R6m2 Standard".to_string(),
                "Canon EOS R7 Standard".to_string()
            ])
        );
        // Renamed: found by the name its title still gives, written
        // under the name the user gave it.
        let mut renamed = old("Canon EOS R6m2 Faithful", "Canon EOS R6m2");
        renamed.name = "My R6".into();
        assert_eq!(refit("My R6", &[renamed]), Some(vec!["My R6".to_string()]));
        // Renamed and retitled: found by its header.
        let declared = table(
            "Warm R6",
            "Warm R6",
            &[
                "make: Canon",
                "model: Canon EOS R6m2",
                "style: Canon Standard",
            ],
        );
        assert_eq!(
            refit("Warm R6", &[declared]),
            Some(vec!["Warm R6".to_string()])
        );
        // An early trial's table: fitted, but its title names a body
        // no group here has, and it declares nothing.
        let trial = table("r6ii-faithful", "Canon EOS R6 Mark II Faithful", &[]);
        assert_eq!(refit("r6ii-faithful", &[trial]), None);
        assert_eq!(refit("Slide Warm", &[]), None);
        // The pair plans as the donor's fit and the borrow.
        let pair = refit_groups(&groups, "Canon EOS R5m2 Faithful", &[]).unwrap();
        let p = plan(&pair, &HashMap::new(), DisplayCurve::Agx, false);
        assert!(p[0].is_fit());
        assert!(matches!(&p[1], Plan::Borrow { from, .. } if from == "Canon EOS R6m2"));
    }

    /// A refit of a renamed look under AgX writes the AgX table under
    /// the look's own name, beside the table it had.
    #[test]
    fn a_refit_of_a_renamed_look_writes_beside_it() {
        let store = scratch_store("renamed");
        put_fitted(
            &store,
            "My R6",
            "Canon EOS R6m2 Faithful (fitted on Canon EOS R6m2, 22 frames)",
        );
        let tables: Vec<Entry> = greycard_core::lut::list_dir(&store)
            .into_iter()
            .map(|(stem, path, info)| greycard_edit::look::entry_of(&stem, path, info.unwrap()))
            .collect();
        let tables: Vec<&Entry> = tables.iter().collect();
        let all = vec![synthetic_group("Canon EOS R6m2", "Canon Faithful", 22)];
        let groups = refit_groups(&all, "My R6", &tables).unwrap();
        let (results, _) = run_synthetic_under(&groups, &store, DisplayCurve::Agx, false, &[]);
        assert!(
            matches!(results[0].outcome, Outcome::Fitted { .. }),
            "{results:?}"
        );
        assert!(store.join("My R6.agx.cube").is_file());
        assert!(!store.join("Canon EOS R6m2 Faithful.agx.cube").exists());
        assert_eq!(
            title_in(&store, "My R6").as_deref(),
            Some("Canon EOS R6m2 Faithful (fitted on Canon EOS R6m2, 22 frames)")
        );
        crate::testing::remove_dir_retry(&store);
    }

    /// The plan reads the store: a user's own table under a group's
    /// name keeps the group from being written, on the plan's line,
    /// and a run leaves the table as it was; with replace on, the run
    /// writes over it and says so.
    #[test]
    fn a_users_own_table_is_left_alone_unless_replace_is_on() {
        let store = scratch_store("own");
        // A title in the match's shape without the match's line is the
        // user's too: only the line says the match wrote a table.
        put_table(
            &store,
            "Canon EOS R6m2 Faithful",
            "Canon EOS R6m2 Faithful (fitted on Canon EOS R6m2, 10 frames)",
        );
        assert_eq!(
            existing_in(&store).get("Canon EOS R6m2 Faithful"),
            Some(&Existing::Own)
        );
        put_table(&store, "Canon EOS R6m2 Faithful", "My Faithful");
        let groups = vec![synthetic_group("Canon EOS R6m2", "Canon Faithful", 22)];
        let p = plan(&groups, &existing_in(&store), CH, false);
        assert!(
            matches!(&p[0], Plan::Skip(why) if why.contains("did not write")),
            "{p:?}"
        );
        let (results, heard) = run_synthetic(&groups, &store, false, &[]);
        assert!(matches!(&results[0].outcome, Outcome::Skipped(_)));
        assert!(!heard.iter().any(|p| matches!(p, Progress::Frame { .. })));
        assert_eq!(
            title_in(&store, "Canon EOS R6m2 Faithful").as_deref(),
            Some("My Faithful")
        );
        let p = plan(&groups, &existing_in(&store), CH, true);
        assert!(matches!(&p[0], Plan::Fit { replaces: Some(_) }), "{p:?}");
        let (results, _) = run_synthetic(&groups, &store, true, &[]);
        assert!(matches!(results[0].outcome, Outcome::Fitted { .. }));
        assert!(
            results[0].line().contains("replaces a table of that name"),
            "{}",
            results[0].line()
        );
        assert_eq!(
            title_in(&store, "Canon EOS R6m2 Faithful").as_deref(),
            Some("Canon EOS R6m2 Faithful (fitted on Canon EOS R6m2, 22 frames)")
        );
        crate::testing::remove_dir_retry(&store);
    }

    /// A body's own fit is not replaced by a borrow, nor by a fit that
    /// ends up with fewer frames than it had once the frames that did
    /// not register are out.
    #[test]
    fn a_fit_from_more_frames_and_a_bodys_own_fit_stay() {
        let store = scratch_store("more");
        put_fitted(
            &store,
            "Canon EOS R6m2 Faithful",
            "Canon EOS R6m2 Faithful (fitted on Canon EOS R6m2, 24 frames)",
        );
        put_fitted(
            &store,
            "Canon EOS R5m2 Faithful",
            "Canon EOS R5m2 Faithful (fitted on Canon EOS R5m2, 30 frames)",
        );
        // 26 candidates plan as a fit over 24; four fail to register,
        // and 22 is fewer than the table there was fitted on.
        let groups = vec![
            synthetic_group("Canon EOS R6m2", "Canon Faithful", 26),
            synthetic_group("Canon EOS R5m2", "Canon Faithful", 5),
        ];
        let p = plan(&groups, &existing_in(&store), CH, false);
        assert_eq!(p[0], Plan::Fit { replaces: None });
        assert!(
            matches!(&p[1], Plan::Skip(why) if why.contains("fitted on this body itself")),
            "{p:?}"
        );
        let (results, _) = run_synthetic(&groups, &store, false, &[1, 2, 3, 4]);
        assert!(
            matches!(&results[0].outcome, Outcome::Skipped(why)
                if why.contains("from 24 frames, more than 22")),
            "{:?}",
            results[0]
        );
        assert!(matches!(&results[1].outcome, Outcome::Skipped(_)));
        assert_eq!(
            title_in(&store, "Canon EOS R6m2 Faithful").as_deref(),
            Some("Canon EOS R6m2 Faithful (fitted on Canon EOS R6m2, 24 frames)")
        );
        assert_eq!(
            title_in(&store, "Canon EOS R5m2 Faithful").as_deref(),
            Some("Canon EOS R5m2 Faithful (fitted on Canon EOS R5m2, 30 frames)")
        );
        crate::testing::remove_dir_retry(&store);
    }

    /// The donor is chosen once, on merit: of the bodies that fit the
    /// style, the one with the most frames, the first on a tie; the
    /// plan names it and the run borrows from it.
    #[test]
    fn a_borrow_takes_the_donor_with_the_most_frames() {
        let groups = vec![
            synthetic_group("Canon EOS R5", "Canon Faithful", 21),
            synthetic_group("Canon EOS R6m2", "Canon Faithful", 23),
            synthetic_group("Canon EOS R7", "Canon Faithful", 23),
            synthetic_group("Canon EOS R5m2", "Canon Faithful", 5),
        ];
        let p = plan(&groups, &HashMap::new(), CH, false);
        assert_eq!(
            p[3],
            Plan::Borrow {
                from: "Canon EOS R6m2".into(),
                donor: 1,
                replaces: None,
            }
        );
        let store = scratch_store("donor");
        let (results, _) = run_synthetic(&groups, &store, false, &[]);
        assert!(
            matches!(&results[3].outcome, Outcome::Borrowed { from, .. } if from == "Canon EOS R6m2"),
            "{:?}",
            results[3]
        );
        assert_eq!(
            title_in(&store, "Canon EOS R5m2 Faithful").as_deref(),
            Some("Canon EOS R5m2 Faithful (fitted on Canon EOS R6m2, 23 frames)")
        );
        crate::testing::remove_dir_retry(&store);
    }

    /// The exposure the match solves goes in through the finish: a
    /// mid-grey frame finished a stop up comes out with twice the
    /// linear luminance, as the camera's brighter JPEG is matched.
    #[test]
    fn a_stop_up_in_the_finish_doubles_the_light() {
        let (w, h) = (64usize, 48usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: vec![0.09; w * h * 3],
        };
        let mut developed = crate::worker::FrameDevelop::of_image(image);
        let edit = Edit::default();
        let mut up = edit.clone();
        up.light.exposure += 1.0;
        let mean = |p: &Picture| {
            p.data
                .iter()
                .map(|&c| greycard_match::color::luminance(greycard_match::color::decode3(c)))
                .sum::<f32>()
                / p.data.len() as f32
        };
        let at = mean(&picture_of(&developed.finish(&edit, &settings())));
        let above = mean(&picture_of(&developed.finish(&up, &settings())));
        let ratio = above / at;
        eprintln!("mid grey {at:.4}, a stop up {above:.4}, ratio {ratio:.3}");
        assert!(
            (ratio - 2.0).abs() < 0.1,
            "a stop up is {ratio:.3} times the light"
        );
    }

    /// The fit develops under the run's curve: the edit carries it,
    /// and the finish the export runs renders it, so a frame finished
    /// under AgX is not the frame finished under per channel.
    #[test]
    fn the_fit_develops_under_the_runs_curve() {
        let (w, h) = (32usize, 24usize);
        let data = (0..w * h)
            .flat_map(|i| {
                let t = i as f32 / (w * h) as f32;
                [0.02 + 1.5 * t, 0.3 * t, 0.9 * (1.0 - t)]
            })
            .collect();
        let mut developed = crate::worker::FrameDevelop::of_image(WorkingImage {
            width: w,
            height: h,
            data,
        });
        let mut pictures = Vec::new();
        for curve in DisplayCurve::ALL {
            let edit = fit_edit(curve);
            assert_eq!(edit.display_curve, curve);
            assert!(edit.look_lut.is_off(), "the fit develops with no look");
            pictures.push(picture_of(&developed.finish(&edit, &settings())));
        }
        let moved = pictures[0]
            .data
            .iter()
            .zip(&pictures[1].data)
            .map(|(a, b)| (0..3).map(|k| (a[k] - b[k]).abs()).sum::<f32>() / 3.0)
            .sum::<f32>()
            / pictures[0].data.len() as f32;
        eprintln!(
            "per channel against AgX: {:.1} levels on average",
            moved * 255.0
        );
        assert!(moved * 255.0 > 2.0, "{moved}");
    }

    /// The develop, the JPEG and the registration on real raws: every
    /// raw in `GREYCARD_SAMPLES` whose maker's tags read, measured as
    /// a fit would measure it. Prints and asserts only that a frame
    /// that registers has blocks; ignored, since it wants the samples
    /// and a release build to be quick.
    #[test]
    #[ignore]
    fn the_samples_measure() {
        let Some(dir) = std::env::var_os("GREYCARD_SAMPLES").map(PathBuf::from) else {
            eprintln!("set GREYCARD_SAMPLES to a folder of raws");
            return;
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        files.sort();
        let survey = survey_files(&files);
        eprintln!(
            "{} groups, {} raws with no fixed style",
            survey.groups.len(),
            survey.no_style
        );
        let lenses = greycard_lens::Store::user()
            .ok()
            .and_then(|s| s.load())
            .map(|(db, _)| db);
        for g in &survey.groups {
            let frame = &g.frames[0];
            match measure(frame, DisplayCurve::Channels, lenses.as_ref()) {
                Ok(kept) => {
                    eprintln!(
                        "{} ({} frames): {} blocks of {} on {}",
                        g.name(),
                        g.frames.len(),
                        kept.pairs.pairs.len(),
                        kept.pairs.total,
                        frame.path.display()
                    );
                    assert!(!kept.pairs.pairs.is_empty());
                }
                Err(why) => eprintln!("{}: {why} on {}", g.name(), frame.path.display()),
            }
        }
    }
}
