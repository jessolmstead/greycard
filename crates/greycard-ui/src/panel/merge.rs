//! Merge to HDR, Merge to Panorama, and Merge to Focus stack, in the
//! frame menu beside Stack bursts: the frames chosen, a tripod bracket,
//! a panorama's frames, or a focus stack, merged by the engine's merge
//! (the one `greycard merge hdr`, `greycard merge pano`, or `greycard
//! stack` runs) into a linear DNG beside the first of them,
//! `<first>-HDR.dng`, `<first>-pano.dng`, or `<first> stack.dng`.
//!
//! The merge reads every frame whole, seconds a frame, so it runs on a
//! thread of its own, saying each frame as it comes in the status line;
//! a second ask waits for the first and then runs. What it made lands on
//! the window's thread: the file given to the indexer, its edit the
//! reference frame's less the exposure (a focus stack keeps it), and the file on top of a stack
//! with its sources when they are of one folder, as a stack is. A
//! session that writes no sidecars holds the merged file's edit in
//! memory, as it holds an edit made there, until the window is closed.
//!
//! A merge that keeps its recipe and has its frames beside it is made
//! again from the Noise section's row (the engine's `merge::again`), its
//! frames denoised at the tier chosen there, on the same thread and
//! queue. The new file takes the old one's name, so its edit and its
//! sidecar stay; what the window keeps of the old file's picture goes.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::tr;
use greycard_edit::{Edit, Learned, Line, Placement, Sidecar};
use greycard_edit_io::on_disk::Fresh;
use greycard_engine::merge::{self, Options, Step, focus, pano};
use greycard_library::collections::Reference;
use slint::ComponentHandle;

use crate::panel::browser::chosen_frames;
use crate::panel::collections::frames_of;
use crate::worker::Worker;
use crate::{App, Shared, State};

/// The merges asked and not yet run, and whether one is running.
/// Shared by the window's thread and the merge's own; a test's are its
/// thread's alone.
#[derive(Default)]
struct Queue {
    running: bool,
    waiting: VecDeque<Job>,
}

#[cfg(not(test))]
fn with_queue<R>(f: impl FnOnce(&mut Queue) -> R) -> R {
    static QUEUE: std::sync::Mutex<Option<Queue>> = std::sync::Mutex::new(None);
    let mut q = QUEUE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(q.get_or_insert_with(Queue::default))
}

#[cfg(test)]
fn with_queue<R>(f: impl FnOnce(&mut Queue) -> R) -> R {
    thread_local! {
        static QUEUE: RefCell<Queue> = RefCell::new(Queue::default());
    }
    QUEUE.with(|q| f(&mut q.borrow_mut()))
}

/// Which merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Hdr,
    Pano,
    /// A focus stack: the CLI's defaults, as `focus::Options` has them.
    Focus,
}

/// One merge asked: of frames chosen, or a merged file made again.
enum Job {
    Merge(Merge),
    Again(Remerge),
}

impl Job {
    /// How many frames it merges.
    fn frames(&self) -> usize {
        match self {
            Job::Merge(m) => m.files.len(),
            Job::Again(a) => a.frames,
        }
    }

    /// The merge, on whatever thread runs it; `said` takes each
    /// progress line.
    fn run(self, said: &dyn Fn(String)) -> Done {
        match self {
            Job::Merge(m) => m.run(said),
            Job::Again(a) => a.run(said),
        }
    }
}

/// Frames chosen merged: the files in the list's order, their edits
/// (the reference's is the merged file's start), their references for
/// the stack, and where a sidecar goes.
struct Merge {
    kind: Kind,
    files: Vec<PathBuf>,
    /// Each frame's edit as the window holds it, where the window has
    /// read its sidecar; none for one read from disk on the thread.
    edits: Vec<Option<Edit>>,
    /// Each frame the window holds whose masks wait on a shape nothing
    /// can measure (§724): its edit goes into no new file, so a merge
    /// whose file would start from it is refused.
    waiting: Vec<bool>,
    /// The sources as a stack keeps them, when they are of one folder.
    stack: Option<Vec<Reference>>,
    placement: Placement,
    write_sidecars: bool,
    /// What a raw with no sidecar starts with (`Window::fresh`).
    fresh: Fresh,
}

/// A merged file made again from its recipe, over itself.
struct Remerge {
    file: PathBuf,
    /// The frames its recipe names, for the first status line.
    frames: usize,
    /// The reference frame, whose edit the frames are walked under, as
    /// the window holds it; none for one read from disk on the thread.
    reference: PathBuf,
    edit: Option<Box<Edit>>,
    /// The learned tier the frames are denoised at; none for none.
    tier: Option<String>,
    /// What a reference with no sidecar starts with (`Window::fresh`).
    fresh: Fresh,
}

/// The edit of a frame the window has not read, from its sidecar on
/// disk, or, with sidecars off or none there, the new edit the
/// window's setting starts it at. On the merge's thread, so what waits
/// on the frame's shape asks its header (§724); an error for one whose
/// masks wait still: a merge would write them into the new file's edit
/// unturned.
fn edit_on_disk(path: &Path, write_sidecars: bool, fresh: Fresh) -> anyhow::Result<Edit> {
    let sidecar = if write_sidecars {
        greycard_edit_io::on_disk::read_sidecar_fresh(
            path,
            fresh,
            greycard_edit_io::on_disk::frame_shape,
        )
        .0
    } else {
        greycard_edit_io::on_disk::fresh_sidecar(path, fresh)
    };
    match sidecar.ready() {
        Some(edit) => Ok(edit.clone()),
        None => Err(waits_on_shape(path)),
    }
}

/// Why a merge whose file would start from the frame at `path` was
/// refused: its masks wait on a shape nothing can measure (§724).
fn waits_on_shape(path: &Path) -> anyhow::Error {
    tracing::warn!(
        "{}: nothing can measure the frame, so it was not merged",
        path.display()
    );
    anyhow::anyhow!(
        "{}: its masks cannot be placed until its raw is reachable",
        path.display()
    )
}

/// What a merge made.
struct Done {
    frames: usize,
    made: anyhow::Result<Made>,
    lands: Lands,
}

/// Where what a merge made lands.
enum Lands {
    /// A new file beside the frames: on top of a stack of them, when
    /// they are of one folder.
    Beside { stack: Option<Vec<Reference>> },
    /// Over the merged file it was made again from: the old file's
    /// thumbnail key, its hash and stamp, read before it was replaced;
    /// none when it would not read.
    Over { old: Option<(String, u64)> },
}

struct Made {
    out: PathBuf,
    /// Why frames that asked for a learned tier went without it: the
    /// finished line says so.
    learned_skipped: Option<merge::LearnedSkipped>,
    /// Its content hash, for the stack; none when it would not read.
    hash: Option<String>,
    /// Its edit, when no sidecar was written for it.
    edit: Option<Edit>,
}

/// The frame menu's Merge to HDR, Merge to Panorama, or Merge to Focus
/// stack: the frames chosen, each file once.
pub(crate) fn ask(st: &mut State, app: &App, worker: &Worker, kind: Kind) {
    let mut cells: Vec<usize> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in chosen_frames(st) {
        let f = st.file_of(c);
        if seen.insert(f) {
            cells.push(c);
        }
    }
    if cells.len() < 2 {
        app.set_status(tr!("choose two frames or more to merge").into());
        return;
    }
    let files: Vec<PathBuf> = cells
        .iter()
        .map(|&c| st.files[st.file_of(c)].clone())
        .collect();
    // A frame whose sidecar the window has read is taken as the window
    // holds it (the one open, and every edit of a session that writes
    // no sidecars); the rest are read from disk on the merge's thread.
    // Each is measured first, as a turn does, so what waited on its
    // shape is shown by it (§724); one that still waits gives no edit,
    // and the merge refuses it only if its edit is the one the new file
    // starts from, which the thread finds.
    for &c in &cells {
        if crate::rows::file_is_loaded(st, st.file_of(c)) {
            crate::panel::browser::migrate_frame(st, c);
        }
    }
    crate::panel::browser::panel_settled(st, app);
    fn held(st: &State, c: usize) -> Option<&greycard_edit::Sidecar> {
        let f = st.file_of(c);
        crate::rows::file_is_loaded(st, f)
            .then(|| st.sidecars.get(f))
            .flatten()
    }
    let edits: Vec<Option<Edit>> = cells
        .iter()
        .map(|&c| held(st, c).and_then(|s| s.ready().cloned()))
        .collect();
    let waiting: Vec<bool> = cells
        .iter()
        .map(|&c| held(st, c).is_some_and(|s| s.current_waits()))
        .collect();
    // A stack is one folder's, and a collection's list makes none.
    let folders: std::collections::HashSet<Option<PathBuf>> = cells
        .iter()
        .map(|&c| crate::stacks::folder_of(st, st.file_of(c)))
        .collect();
    let stack = (st.views.collection_frames.is_none() && folders.len() == 1)
        .then(|| {
            let found = frames_of(st, &cells);
            (found.len() == cells.len()).then(|| {
                found
                    .into_iter()
                    .zip(&cells)
                    .filter_map(|((hash, _, _), &c)| {
                        Some(Reference {
                            hash,
                            hint: crate::stacks::hint_of_file(st, st.file_of(c))?,
                            version: None,
                        })
                    })
                    .collect::<Vec<_>>()
            })
        })
        .flatten();
    let job = Job::Merge(Merge {
        kind,
        files,
        edits,
        waiting,
        stack,
        placement: st.window.placement,
        write_sidecars: st.window.write_sidecars,
        fresh: st.window.fresh(),
    });
    queue(st, app, worker, job);
}

/// The Noise section's row: the open merged file made again from its
/// recipe, its frames denoised at the row's tier: the window's, never
/// written to the edit, which a merge clears of the learned tier on
/// purpose.
pub(crate) fn ask_again(st: &mut State, app: &App, worker: &Worker) {
    if !st.window.write_sidecars {
        app.set_merge_again_shown(false);
        return;
    }
    let (Some(c), Some(tag)) = (
        st.current,
        st.develop
            .frame
            .as_ref()
            .and_then(|(f, _)| f.dng.merged.clone()),
    ) else {
        return;
    };
    let file = st.path_of(c).clone();
    let Some(name) = tag.frames.get(tag.reference) else {
        return;
    };
    let reference = file.with_file_name(name);
    // As the window holds it when it has read its sidecar, as a merge
    // takes it.
    let edit = st
        .file_by_path(&reference)
        .filter(|&f| crate::rows::file_is_loaded(st, f))
        .and_then(|f| st.sidecars.get(f))
        // One whose masks wait on its shape (§724) is read on the
        // thread, which refuses it.
        .and_then(|s| s.ready().cloned())
        .map(Box::new);
    let tier = Learned::from_name(&app.get_merge_again_tier())
        .and_then(Learned::tier)
        .map(str::to_string);
    let job = Job::Again(Remerge {
        file,
        frames: tag.frames.len(),
        reference,
        edit,
        tier,
        fresh: st.window.fresh(),
    });
    queue(st, app, worker, job);
}

/// The worker said whether the merge of ours open now can be made
/// again with its frames denoised: the Noise section's row shown for it
/// in a session that writes sidecars, and not on a frame of the export
/// queue's view. Its tier starts at the one the recipe ran, else off.
pub(crate) fn opened(st: &State, app: &App, can: bool) {
    let shown = can && st.window.write_sidecars && !crate::panel::queue_view::frozen(st);
    app.set_merge_again_shown(shown);
    if shown {
        let ran = st
            .develop
            .frame
            .as_ref()
            .and_then(|(f, _)| f.dng.merged.as_ref()?.recipe.as_ref()?.denoise.clone());
        let tier = ran.unwrap_or_else(|| Learned::default().name().to_string());
        app.set_merge_again_tier(tier.into());
    }
}

/// Another frame is opened: the row is the worker's to show.
pub(crate) fn left(app: &App) {
    app.set_merge_again_shown(false);
}

/// `job` run now, or after the merge in hand.
fn queue(st: &mut State, app: &App, worker: &Worker, job: Job) {
    app.set_merge_running(true);
    let start = with_queue(|q| {
        if q.running {
            q.waiting.push_back(job);
            None
        } else {
            q.running = true;
            Some(job)
        }
    });
    match start {
        Some(job) => send_off(st, app, worker, job),
        None => app.set_status(queued().into()),
    }
}

/// The status line for a merge asked while another runs.
fn queued() -> String {
    tr!("these frames are merged once the merge in hand is done")
}

/// A merge's progress as the status line says it.
fn step_line(step: Step) -> String {
    match step {
        Step::Frame { index, of } => {
            tr!("merging frame {index} of {of}", index = index, of = of)
        }
        Step::Placing => tr!("merging: placing the frames"),
        Step::Blending { index, of } => tr!(
            "merging: blending frame {index} of {of}",
            index = index,
            of = of
        ),
        Step::Writing => tr!("merging: writing the file"),
    }
}

/// A panorama's first run, under the anchor's edit, and its second,
/// under the reference's, as each is written under the name held.
const FIRST: &str = "first.dng";
const SECOND: &str = "second.dng";

impl Merge {
    fn run(self, said: &dyn Fn(String)) -> Done {
        let frames = self.files.len();
        let made = (|| -> anyhow::Result<Made> {
            let edit_of = |k: usize| match self.edits[k].clone() {
                Some(edit) => Ok(edit),
                None if self.waiting[k] => Err(waits_on_shape(&self.files[k])),
                None => edit_on_disk(&self.files[k], self.write_sidecars, self.fresh),
            };
            let progress = |step: Step| said(step_line(step));
            let (out, headroom_stops, seconds, warnings, merged_edit, learned_skipped) =
                match self.kind {
                    Kind::Focus => {
                        // The stack finds its own reference: its edit is
                        // taken after.
                        let r = focus::stack(&self.files, &focus::Options::default(), &progress)?;
                        let left_out = r
                            .stack
                            .frames
                            .iter()
                            .filter_map(|f| {
                                let why = f.dropped.as_ref()?;
                                Some(format!(
                                    "{} was left out of the stack: {why}",
                                    r.names[f.index]
                                ))
                            })
                            .collect();
                        (
                            r.out,
                            0.0,
                            r.seconds,
                            left_out,
                            focus::edit_for_stack(&edit_of(r.stack.reference)?),
                            // A stack asks for no learned tier.
                            None,
                        )
                    }
                    Kind::Hdr => {
                        let edit = &edit_of(merge::reference_of(&self.files, None)?)?;
                        let r = merge::hdr(
                            &self.files,
                            edit,
                            &Options::default(),
                            &mut greycard_engine::walk::NoModels,
                            &progress,
                        )?;
                        (
                            r.out,
                            r.headroom_stops,
                            r.seconds,
                            r.warnings,
                            merge::edit_for_merge(edit),
                            r.learned_skipped,
                        )
                    }
                    Kind::Pano => {
                        let reference = pano::reference_of(self.files.len(), None)?;
                        let edit = &edit_of(reference)?;
                        // Each run is written under the name held and the
                        // one kept renamed onto it, so a second run that
                        // fails leaves the first to keep.
                        let held = merge::hold(
                            &merge::Files(&self.files),
                            &pano::default_output(&self.files[0]),
                            Default::default(),
                        )?;
                        // Under `edit`, the learned denoiser on each frame
                        // when it has it, walked from frame `from` when
                        // given, as file `name` under the folder.
                        let run = |edit: &Edit, from: Option<usize>, name: &str| {
                            let denoiser = edit.noise.tier().and_then(greycard_ai::denoiser);
                            let options = pano::Options {
                                denoiser,
                                reference: from,
                                ..pano::Options::default()
                            };
                            match denoiser {
                                Some(_) => pano::pano_under(
                                    &self.files,
                                    &held,
                                    name,
                                    edit,
                                    &options,
                                    &mut merge::DenoiseModels::new(),
                                    &progress,
                                ),
                                None => pano::pano_under(
                                    &self.files,
                                    &held,
                                    name,
                                    edit,
                                    &options,
                                    &mut greycard_engine::walk::NoModels,
                                    &progress,
                                ),
                            }
                        };
                        let first = run(edit, None, FIRST)?;
                        // The reference is the middle of the turn, which the
                        // list's middle need not be. One frame walks the
                        // panorama and gives it its edit: when the two
                        // frames' edits walk differently, it is merged
                        // again from the reference, under its edit. Should
                        // that fail, the first is kept, with the anchor's.
                        let (r, look) = if first.reference == reference {
                            (first, edit.clone())
                        } else {
                            let other = edit_of(first.reference)?;
                            if pano::walks_alike(edit, &other) {
                                (first, other)
                            } else {
                                match run(&other, Some(first.reference), SECOND) {
                                    // The first goes with the folder.
                                    Ok(second) => (second, other),
                                    Err(e) => {
                                        tracing::warn!(
                                            "merge: the panorama again from {}, under its edit, \
                                         failed, so it keeps {}'s: {e:#}",
                                            first.frames[first.reference],
                                            first.frames[reference]
                                        );
                                        (first, edit.clone())
                                    }
                                }
                            }
                        };
                        let out = held.promote(&r.out)?;
                        drop(held);
                        (
                            out,
                            r.headroom_stops,
                            r.seconds,
                            r.warnings,
                            pano::edit_for_pano(&look),
                            r.learned_skipped,
                        )
                    }
                };
            for w in &warnings {
                tracing::warn!("merge: {w}");
            }
            tracing::info!(
                "merge: {} written from {frames} frames, {headroom_stops:.2} stops of headroom, \
                 in {:.1} s",
                out.display(),
                seconds
                    .iter()
                    .find(|(n, _)| *n == "total")
                    .map_or(0.0, |(_, s)| *s)
            );
            let kept = if self.write_sidecars {
                let mut sidecar = Sidecar {
                    line: Line::of(merged_edit, 0)
                        .expect("a merge's edit is made, never a waiting copy"),
                    ..Sidecar::default()
                };
                match sidecar.save_in(&out, self.placement) {
                    Ok(()) => None,
                    // Held in memory as a session without sidecars
                    // holds it, and saved from the window once the
                    // file is listed (`held`), where a failure is the
                    // window's to say.
                    Err(e) => {
                        tracing::warn!("merge: {}: the edit not written: {e}", out.display());
                        Some(sidecar.line.current().clone())
                    }
                }
            } else {
                Some(merged_edit)
            };
            let hash = greycard_library::hash_file(&out)
                .map_err(|e| tracing::warn!("merge: {}: {e}", out.display()))
                .ok();
            Ok(Made {
                out,
                learned_skipped,
                hash,
                edit: kept,
            })
        })();
        Done {
            frames,
            made,
            lands: Lands::Beside { stack: self.stack },
        }
    }
}

impl Remerge {
    fn run(self, said: &dyn Fn(String)) -> Done {
        // Read before the file is replaced, since it cannot be after.
        let old = crate::worker::thumb_key(&self.file);
        let mut frames = self.frames;
        let made = (|| -> anyhow::Result<Made> {
            let edit = match &self.edit {
                Some(edit) => (**edit).clone(),
                None => edit_on_disk(&self.reference, true, self.fresh)?,
            };
            let overrides = merge::Overrides {
                denoise: Some(self.tier.clone()),
                out: None,
            };
            let progress = |step: Step| said(step_line(step));
            let again = match self.tier {
                Some(_) => merge::again(
                    &self.file,
                    &overrides,
                    &edit,
                    &mut denoise_models(),
                    &progress,
                ),
                None => merge::again(
                    &self.file,
                    &overrides,
                    &edit,
                    &mut greycard_engine::walk::NoModels,
                    &progress,
                ),
            }?;
            let (out, warnings, seconds, learned_skipped) = match again {
                merge::Again::Hdr(r) => {
                    frames = r.frames.len();
                    (r.out, r.warnings, r.seconds, r.learned_skipped)
                }
                merge::Again::Pano(r) => {
                    frames = r.frames.len();
                    (r.out, r.warnings, r.seconds, r.learned_skipped)
                }
            };
            for w in &warnings {
                tracing::warn!("merge: {w}");
            }
            tracing::info!(
                "merge: {} made again from {frames} frames, the learned tier {}, in {:.1} s",
                out.display(),
                self.tier.as_deref().unwrap_or("off"),
                seconds
                    .iter()
                    .find(|(n, _)| *n == "total")
                    .map_or(0.0, |(_, s)| *s)
            );
            let hash = greycard_library::hash_file(&out)
                .map_err(|e| tracing::warn!("merge: {}: {e}", out.display()))
                .ok();
            Ok(Made {
                out,
                learned_skipped,
                hash,
                edit: None,
            })
        })();
        Done {
            frames,
            made,
            lands: Lands::Over { old },
        }
    }
}

/// The models a merge made again with a learned tier runs: the user's
/// store. A test's network is the plain demosaic.
#[cfg(not(test))]
fn denoise_models() -> merge::DenoiseModels {
    merge::DenoiseModels::new()
}

#[cfg(test)]
fn denoise_models() -> greycard_engine::testing::PlainDenoiser {
    greycard_engine::testing::PlainDenoiser
}

/// The merges on a thread of their own, each landed on the window's as
/// it is done, the next asked run after it.
#[cfg(not(test))]
fn send_off(_st: &mut State, app: &App, _worker: &Worker, job: Job) {
    app.set_status(
        tr!(
            "merging frame {index} of {of}",
            index = 1,
            of = job.frames()
        )
        .into(),
    );
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard merge".into())
        .spawn(move || {
            let mut job = Some(job);
            while let Some(j) = job.take() {
                let weak = app_weak.clone();
                let said = move |line: String| {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            app.set_status(line.into());
                        }
                    });
                };
                let done = j.run(&said);
                let weak = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let (Some(app), Some(state), Some(worker)) = (
                        weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    land(&state, &app, &worker, done);
                });
                job = with_queue(|q| {
                    let next = q.waiting.pop_front();
                    q.running = next.is_some();
                    next
                });
            }
            // After the last landing: the row is free again, unless a
            // merge asked since has started one of its own.
            let weak = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    app.set_merge_running(with_queue(|q| q.running));
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("merge: {e}");
        with_queue(|q| {
            q.running = false;
            q.waiting.clear();
        });
        app.set_merge_running(false);
    }
}

/// In a test the merge runs where it is asked, and any waiting after
/// it: there is no event loop to land it on.
#[cfg(test)]
fn send_off(st: &mut State, app: &App, _worker: &Worker, job: Job) {
    app.set_status(
        tr!(
            "merging frame {index} of {of}",
            index = 1,
            of = job.frames()
        )
        .into(),
    );
    let done = job.run(&|line| {
        HEARD.with(|h| {
            if let Some(heard) = h.borrow_mut().as_mut() {
                heard(&line);
            }
        });
        app.set_status(line.into());
    });
    LANDED.with(|l| l.borrow_mut().push(done));
    let _ = st;
    with_queue(|q| {
        q.running = false;
    });
    app.set_merge_running(false);
}

/// What a test hears of a merge's progress lines.
#[cfg(test)]
type Hear = Box<dyn FnMut(&str)>;

#[cfg(test)]
thread_local! {
    /// What a test's merges made, for the test to land with the state
    /// let go.
    static LANDED: RefCell<Vec<Done>> = const { RefCell::new(Vec::new()) };
    /// What a test hears of each progress line, as the merge says it.
    static HEARD: RefCell<Option<Hear>> = const { RefCell::new(None) };
}

/// The finished line's tail for frames that asked for a learned tier
/// and went without it.
fn skipped_words(why: merge::LearnedSkipped) -> String {
    match why {
        merge::LearnedSkipped::OffCard => tr!("no learned denoise on this graphics card"),
        // WORDING PENDING the owner's sign-off.
        merge::LearnedSkipped::XTrans => tr!("no learned denoise in an X-Trans merge"),
    }
}

/// A merge done: the file given to the indexer and stacked on top of
/// its sources, the list read again, the status said.
fn land(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, done: Done) {
    let made = match done.made {
        Ok(made) => made,
        Err(e) => {
            tracing::warn!("merge: {e:#}");
            app.set_status(tr!("not merged: {why}", why = format!("{e:#}")).into());
            return;
        }
    };
    let name = made
        .out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let said = tr!(
        "merged {n} frame into {name}" | "merged {n} frames into {name}" % done.frames,
        name = name
    );
    let said = match made.learned_skipped.map(skipped_words) {
        // The finished line, then why the learned denoise did not run.
        Some(why) => tr!("{line}; {why}", line = said, why = why),
        None => said,
    };
    let stack = match done.lands {
        Lands::Beside { stack } => stack,
        Lands::Over { old } => {
            let mut st = state.borrow_mut();
            let reopened = swapped(&mut st, app, worker, &made, old);
            // The reopen's develop covers the line with its
            // "developing...", and in culling the loupe's: this one is
            // put back once it lands.
            if reopened {
                st.develop.status_after_develop = Some((st.develop.generation, said.clone()));
            } else if let Some(c) = st.current
                && st.path_at(c) == Some(&made.out)
                && let Some(cull) = st.culling.cull.as_mut()
            {
                cull.said = Some(said.clone());
            }
            app.set_status(said.into());
            return;
        }
    };
    {
        let mut held = state.borrow_mut();
        let st: &mut State = &mut held;
        if let Some(edit) = made.edit {
            st.work.merge_edits.insert(made.out.clone(), edit);
        }
        if let Some(index) = &st.views.index {
            index.file(made.out.clone());
            st.work.merge_relist = true;
        }
        if let (Some(sources), Some(hash)) = (stack, made.hash) {
            let mut refs = vec![Reference {
                hash,
                hint: crate::stacks::hint_of_path(st, &made.out),
                version: None,
            }];
            refs.extend(sources);
            st.views.stacks.make(refs);
            crate::panel::stacks::settle(st, app, worker);
        }
    }
    crate::roots::refresh_view(state, app, worker);
    held(&mut state.borrow_mut());
    app.set_status(said.into());
}

/// A merge made again, renamed over its file: the indexer is given it,
/// and what the window kept of the old file's picture let go
/// ([`crate::edited::file_rewritten`]), nothing of its edit. True when
/// it was open and its picture is read again.
fn swapped(
    st: &mut State,
    app: &App,
    worker: &Worker,
    made: &Made,
    old: Option<(String, u64)>,
) -> bool {
    let path = &made.out;
    if let Some(index) = &st.views.index {
        index.file(path.clone());
        st.work.merge_relist = true;
    }
    // An old file that would not hash had nothing kept under a hash.
    crate::edited::file_rewritten(
        st,
        app,
        worker,
        path,
        old.as_ref(),
        None,
        made.hash.as_deref(),
    )
}

/// The merged files' edits held in memory, each given to its file's
/// sidecar once the file is listed and its sidecar read; a file still
/// standing in from its row takes it when it is read.
pub(crate) fn held(st: &mut State) {
    if st.work.merge_edits.is_empty() {
        return;
    }
    let listed: Vec<(PathBuf, usize)> = st
        .work
        .merge_edits
        .keys()
        .filter_map(|p| Some((p.clone(), st.file_by_path(p)?)))
        .filter(|(_, f)| st.from_row.get(*f).is_none_or(|r| r.read))
        // One a newer build has written since is read-only (§724).
        .filter(|(_, f)| !st.sidecars[*f].read_only())
        .collect();
    for (path, f) in listed {
        if let Some(edit) = st.work.merge_edits.remove(&path) {
            // The merge's edit as the frame stands, as the line it
            // replaces was.
            let turn = st.sidecars[f].turn;
            st.sidecars[f].line =
                Line::of(edit, turn).expect("a merge's edit is made, never a waiting copy");
            // Held because the merge's own save failed: saved again
            // here, and said if it fails again. In a session that
            // writes no sidecars this writes nothing.
            crate::sidecars::write_file(st, f);
        }
    }
}

/// The indexer has written rows: a merged file among them is listed
/// now.
pub(crate) fn indexed(state: &Rc<Shared>, app: &App) {
    let relist = std::mem::take(&mut state.borrow_mut().work.merge_relist);
    if relist && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        crate::roots::refresh_view(state, app, &worker);
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
    for kind in [Kind::Hdr, Kind::Pano, Kind::Focus] {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app_weak.clone());
        let asked = move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ask(&mut state.borrow_mut(), &app, &worker, kind);
            #[cfg(test)]
            for done in LANDED.with(|l| std::mem::take(&mut *l.borrow_mut())) {
                land(&state, &app, &worker, done);
            }
        };
        match kind {
            Kind::Hdr => app.on_merge_hdr(asked),
            Kind::Pano => app.on_merge_pano(asked),
            Kind::Focus => app.on_merge_focus(asked),
        }
    }
    app.on_merge_again(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        ask_again(&mut state.borrow_mut(), &app, &worker);
        #[cfg(test)]
        for done in LANDED.with(|l| std::mem::take(&mut *l.borrow_mut())) {
            land(&state, &app, &worker, done);
        }
    });
}

#[cfg(test)]
mod tests;
