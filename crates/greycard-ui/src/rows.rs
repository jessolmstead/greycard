//! The index's rows standing in for sidecars.
//!
//! A frame in the browser's list has a `Sidecar` in `State::sidecars`
//! from the moment it is listed. Where the list came from the index (a
//! view of the roots, a folder the index knows) that sidecar was not
//! read from disk: it is a stand-in built from the frame's row, which
//! mirrors the sidecar's meta, whether it holds a develop and which way
//! up its picture is shown (`greycard_library::RowMeta`). That is all
//! the grid, the strip, the badges and the filter ask of a frame, so a
//! view of ten thousand frames opens without a sidecar read, and a
//! frame under a root that is offline is listed like any other.
//!
//! The sidecar itself is read the first time something needs the whole
//! of it — the edit, the history, the snapshots, the turn, the XMP mark
//! — through [`load_frame`] or [`load_frames`], and from then on the
//! copy in memory is the one that counts, as it always was. The rule
//! for the code that reaches into `st.sidecars`:
//!
//! - `.meta` may be read from any frame's sidecar, stand-in or not.
//! - The turns a picture is shown at come from the row while the frame
//!   stands in (`FromRow::turns`): `thumb_turns`, `thumb_turns_of`.
//! - Anything else (`.current`, `.history`, `.turn`, `.xmp`, the
//!   snapshots, a `record`) wants the frame loaded first. Every action
//!   that wants it is a [`request`]: the frames it is pressed on (each
//!   a file and a version of it, `versions::Key`) and what to do with
//!   them, run when their sidecars are in, on exactly those frames, in
//!   the order the requests were made. Opening a frame is one (the
//!   frame is current from the pick, its panel read-only until its
//!   sidecar lands); so are a key, a turn, a sync, a paste, a preset
//!   and an export over a selection. A request never cancels another.
//!   The frame on screen is loaded, unless its root is offline or its
//!   sidecar is still on its way (`Views::pick_pending`). A file and a
//!   version of it are one sidecar, read once.
//! - The row lists the versions the sidecar keeps, each with its own
//!   marks and key (`FromRow::versions`), so a version's cell stands
//!   in as its file's does; the stand-in sidecar lists them too, with
//!   the default edit, so the cells are the same whether or not the
//!   sidecar was read.
//! - A sidecar that is standing in is never written
//!   (`sidecars::writable`): a save of a stand-in would put the
//!   default edit over the frame's own.
//! - A frame under a root that is offline cannot be loaded. A key, a
//!   turn, a sync, a paste, a preset, an export, a delete or the
//!   rejects' move leaves it out and says so; nothing runs from it as
//!   a source; nothing is written under its root.
//!
//! The stand-in is as right as the row, which is as right as the last
//! pass over the folder or the last save indexed. A pass that finds the
//! sidecar changed rewrites the row, and the window takes the new meta
//! into every frame still standing in ([`apply_row`]); a frame already
//! loaded keeps what memory holds, as a merge always did.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[cfg(test)]
use greycard_edit::Sidecar;
use greycard_edit_io::on_disk::{Trouble, line_never_developed};
use greycard_library::RowMeta;
#[cfg(test)]
pub(crate) use greycard_library::held::{DISK_READS, from_nothing};
pub(crate) use greycard_library::held::{FromRow, Held, from_row, versions_of_row};

/// [`greycard_library::held::from_disk`] as the window reads a frame on
/// its own thread: what waits on the frame's shape asks the frame's
/// header through a memory map (`browser::header_shape`), as the window
/// measures a frame, never the unmapped read that takes a raw whole
/// (§724).
pub(crate) fn from_disk(
    path: &std::path::Path,
    sidecars: Option<greycard_edit::Placement>,
    fresh: greycard_edit_io::on_disk::Fresh,
) -> Held {
    greycard_library::held::from_disk(path, sidecars, fresh, crate::panel::browser::header_shape)
}

use crate::panel::browser::file_name;
use slint::ComponentHandle;

use crate::roots::Progress;
use crate::versions::Key;
use crate::worker::{Job, Worker};
use crate::{App, Shared, State, tr};

/// Whether cell `c`'s line still waits for its ISO's learned-denoiser
/// blend: its file is a raw and the line was never developed (the
/// default edit, no history, no snapshot: `on_disk::line_never_developed`),
/// which while the row stands in for the sidecar is the row's word on
/// the line (`FromRow::marks`). Decided a line at a time: a version made
/// from the original before the raw was opened waits on its own, one
/// edited apart does not, and the seed goes to the line that asked.
pub(crate) fn wants_seed(st: &State, c: usize) -> bool {
    let Some(cell) = st.cell(c) else {
        return false;
    };
    let Some(path) = st.files.get(cell.file) else {
        return false;
    };
    if greycard_core::picture::is_picture_path(path) {
        return false;
    }
    match st.from_row.get(cell.file) {
        Some(row) if !row.read => !row.marks(cell.version.as_deref()).0,
        _ => st.line(c).is_some_and(line_never_developed),
    }
}

/// The ISO's seed landed for cell `c`'s line, as the open's develop or
/// an export brought it: onto that line alone, as its start before it
/// holds any state (§724), when the line still waits for one. True when
/// it took it.
pub(crate) fn seed_landed(st: &mut State, c: usize, blend: f32) -> bool {
    if !wants_seed(st, c) {
        return false;
    }
    match st.line_mut(c) {
        Some(mut line) => line.seed(|e| e.noise.learned_strength = blend),
        None => false,
    }
}

/// Each of `files` held: from its row where the index has one, else
/// read from disk on the rayon pool. `read`, when given, is bumped for
/// each sidecar read from disk, for the window's bar. The order is the
/// list's. `sidecars` is where the session writes them, none with
/// sidecars off (`Window::sidecars`), and `fresh` what a raw with none
/// starts with (`Window::fresh`). `shape` measures a frame whose
/// sidecar waits on its shape (§724): the unmapped
/// [`greycard_edit_io::on_disk::frame_shape`] for a read the window does
/// not wait on, the mapped [`crate::panel::browser::header_shape`] for
/// one it does, which never reads a raw whole.
pub(crate) fn bring(
    files: &[PathBuf],
    rows: &[Option<RowMeta>],
    sidecars: Option<greycard_edit::Placement>,
    fresh: greycard_edit_io::on_disk::Fresh,
    read: Option<&AtomicUsize>,
    shape: greycard_edit_io::on_disk::FrameShape,
) -> Vec<Held> {
    use rayon::prelude::*;
    debug_assert_eq!(files.len(), rows.len());
    files
        .par_iter()
        .zip(rows.par_iter())
        .map(|(path, row)| match row {
            Some(row) => {
                // The camera's frame as the row has it, for the cut of
                // the camera's pictures and the keys, with nothing read.
                note_camera_of_row(path, row);
                from_row(path, row, sidecars.is_some(), fresh)
            }
            None => {
                let mut held = greycard_library::held::from_disk(path, sidecars, fresh, shape);
                hold_unreadable(&mut held, path);
                if let Some(read) = read {
                    read.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                held
            }
        })
        .collect()
}

/// A frame whose `.gcd` was read from disk and would not read (a later
/// build's, or one damaged) held as a frame standing in from its row
/// is when its read fails (`take`): the default edit shown and nothing
/// written over the file. Without this a frame the index has no row
/// for was listed with the default edit as its own, and its next save
/// wrote that over the file.
fn hold_unreadable(held: &mut Held, path: &Path) {
    if let Some(Trouble::Unreadable(e)) = &held.trouble {
        held.from_row.read = false;
        held.from_row.unread = Some(unreadable_words(path, e));
    }
}

/// The status line's words for a sidecar that would not read.
fn unreadable_words(path: &Path, e: &str) -> String {
    tr!(
        "{}'s sidecar could not be read ({e}); nothing is written over it",
        file_name(path),
        e = e
    )
}

/// How many of `rows` have no row, which is how many sidecars
/// [`bring`] reads from disk.
pub(crate) fn to_read(rows: &[Option<RowMeta>]) -> usize {
    rows.iter().filter(|r| r.is_none()).count()
}

/// The root cell `c`'s file is under that is offline, if any.
pub(crate) fn offline_root(st: &State, c: usize) -> Option<PathBuf> {
    offline_root_of(st, st.path_at(c)?)
}

/// The root `path` is under that is offline, if any.
pub(crate) fn offline_root_of(st: &State, path: &Path) -> Option<PathBuf> {
    st.views
        .library
        .offline
        .iter()
        .find(|r| path.starts_with(r))
        .cloned()
}

/// Whether cell `c`'s file is under a root that is offline.
pub(crate) fn is_offline(st: &State, c: usize) -> bool {
    !st.views.library.offline.is_empty() && offline_root(st, c).is_some()
}

/// Whether file `file` is under a root that is offline.
pub(crate) fn file_is_offline(st: &State, file: usize) -> bool {
    !st.views.library.offline.is_empty()
        && st
            .files
            .get(file)
            .is_some_and(|p| offline_root_of(st, p).is_some())
}

/// The status line's word for a frame that cannot be reached, and what
/// that costs (`", nothing to sync from"`), or nothing more.
fn say_offline(st: &State, app: &App, root: &Path, what: &str) {
    let name = crate::roots::root_name(&st.views.library.roots, root);
    app.set_status(tr!("{name} is offline{what}", name = name, what = what).into());
}

/// Say that cell `c`'s root is offline, and what that costs. For a
/// frame under no offline root nothing is said.
pub(crate) fn say_offline_for(st: &State, app: &App, c: usize, what: &str) {
    if let Some(root) = offline_root(st, c) {
        say_offline(st, app, &root, what);
    }
}

/// [`say_offline_for`] by the file.
pub(crate) fn say_offline_for_file(st: &State, app: &App, file: usize, what: &str) {
    if let Some(root) = st.files.get(file).and_then(|p| offline_root_of(st, p)) {
        say_offline(st, app, &root, what);
    }
}

/// Whether cell `c`'s whole sidecar is in memory: read from disk, or
/// nothing to read. False while its file's row stands in.
pub(crate) fn is_loaded(st: &State, c: usize) -> bool {
    st.file_at(c).is_none_or(|f| file_is_loaded(st, f))
}

/// Whether file `file`'s whole sidecar is in memory.
pub(crate) fn file_is_loaded(st: &State, file: usize) -> bool {
    st.from_row.get(file).is_none_or(|r| r.read)
}

/// Whether the panel holds cell `c`'s own edit, so what it does may be
/// recorded on the frame: the frame is loaded, and the panel is not
/// still the stand-in's it showed while the sidecar was on its way.
pub(crate) fn panel_is_frames(st: &State, c: usize) -> bool {
    // A frame of the export queue's view holds its queued edit, which
    // is not the frame's own to record.
    !crate::panel::queue_view::frozen(st)
        && is_loaded(st, c)
        && (st.views.panel_stand_in.is_none()
            || st.views.panel_stand_in.as_deref() != st.path_at(c).map(PathBuf::as_path))
}

/// Why nothing the panel does over frame `i` may be recorded, when its
/// sidecar still stands in: its root is offline, its read is still
/// out (the pick pending), or the read was given up on. A change
/// recorded on the stand-in could not be written (`writable`), and the
/// read's landing would put the whole sidecar over it; so it is
/// refused, and this is what the status line says. None for a frame
/// loaded.
pub(crate) fn not_kept(st: &State, i: usize) -> Option<String> {
    refused(st, i, &tr!("the change is not kept"))
}

/// [`not_kept`]'s reason, ending in `what` was refused ("nothing is
/// exported") rather than a change.
pub(crate) fn refused(st: &State, c: usize, what: &str) -> Option<String> {
    // The export queue's view refuses its own way, in the log alone
    // (`queue_view::refused`), at whatever asked: nothing to say here.
    if crate::panel::queue_view::frozen(st) {
        return None;
    }
    if panel_is_frames(st, c) {
        return None;
    }
    if is_loaded(st, c) {
        let name = file_name(st.path_at(c)?);
        return Some(tr!(
            "the panel is not {name}'s own yet: {what}",
            name = name,
            what = what
        ));
    }
    if let Some(root) = offline_root(st, c) {
        let name = crate::roots::root_name(&st.views.library.roots, &root);
        return Some(tr!("{name} is offline: {what}", name = name, what = what));
    }
    let name = file_name(st.path_at(c)?);
    let file = st.file_at(c)?;
    Some(
        if st.from_row.get(file).is_some_and(|r| r.unread.is_some()) {
            tr!(
                "{name}'s sidecar could not be read: {what}",
                name = name,
                what = what
            )
        } else if st.views.pick_pending.as_deref() == st.path_at(c).map(PathBuf::as_path) {
            tr!(
                "{name}'s sidecar is still being read: {what}",
                name = name,
                what = what
            )
        } else {
            tr!(
                "{name}'s sidecar did not come in: {what}",
                name = name,
                what = what
            )
        },
    )
}

/// What runs on the window's thread once a request's sidecars are in:
/// handed, for each frame the request named, the cell it is at now, or
/// `None` for one no longer listed. Never the selection as it is by
/// then: what was pressed on is what is acted on.
pub(crate) type Action = Box<dyn FnOnce(&Rc<Shared>, &App, &Rc<Worker>, Vec<Option<usize>>)>;

/// What a request acts on: a frame's own line (a paste, a sync, a
/// preset, an export, the open of a frame), or its file (a culling key,
/// a turn, the tags' undo and redo), whose meta and turn every version
/// shares. The scope says what a frame whose version is gone resolves
/// to: nothing for a line, since there is no line to lay on, and the
/// file's own cell for a file, since the file is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Line,
    File,
}

/// What a request is for, for the status line when it is dropped or a
/// frame it named is gone: each kind's words whole, so a translator
/// sees sentences, not a noun with an "s" put on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    /// The open of a frame, by its file's name.
    Open(String),
    RatingKey,
    FlagKey,
    LabelKey,
    Keyword,
    Title,
    TimeShift,
    Turn,
    Export,
    Queue,
    Reset,
    Preset,
    Paste,
    Sync,
    Undo,
    Redo,
}

impl Request {
    /// The kind in English, for the log and the tests.
    pub(crate) fn log(&self) -> String {
        match self {
            Request::Open(name) => format!("open of {name}"),
            Request::RatingKey => "rating key".into(),
            Request::FlagKey => "flag key".into(),
            Request::LabelKey => "label key".into(),
            Request::Keyword => "keyword".into(),
            Request::Title => "title".into(),
            Request::TimeShift => "time shift".into(),
            Request::Turn => "turn".into(),
            Request::Export => "export".into(),
            Request::Queue => "queue".into(),
            Request::Reset => "reset".into(),
            Request::Preset => "preset".into(),
            Request::Paste => "paste".into(),
            Request::Sync => "sync".into(),
            Request::Undo => "undo".into(),
            Request::Redo => "redo".into(),
        }
    }

    /// One request of the kind: "the rating key", "the open of x.cr3".
    pub(crate) fn the(&self) -> String {
        match self {
            // The request's name in 'X was not applied to it' and 'X dropped'
            Request::Open(name) => tr!("the open of {name}", name = name),
            Request::RatingKey => tr!("the rating key"),
            Request::FlagKey => tr!("the flag key"),
            Request::LabelKey => tr!("the label key"),
            Request::Keyword => tr!("the keyword"),
            Request::Title => tr!("the title"),
            Request::TimeShift => tr!("the time shift"),
            Request::Turn => tr!("the turn"),
            Request::Export => tr!("the export"),
            Request::Queue => tr!("the queue"),
            Request::Reset => tr!("the reset"),
            Request::Preset => tr!("the preset"),
            Request::Paste => tr!("the paste"),
            Request::Sync => tr!("the sync"),
            Request::Undo => tr!("the undo"),
            Request::Redo => tr!("the redo"),
        }
    }

    /// `n` requests of the kind: "3 rating keys".
    pub(crate) fn counted(&self, n: usize) -> String {
        match self {
            // A pick is one, however many asked for it.
            Request::Open(_) => self.the(),
            Request::RatingKey => tr!("{n} rating key" | "{n} rating keys" % n),
            Request::FlagKey => tr!("{n} flag key" | "{n} flag keys" % n),
            Request::LabelKey => tr!("{n} label key" | "{n} label keys" % n),
            Request::Keyword => tr!("{n} keyword" | "{n} keywords" % n),
            Request::Title => tr!("{n} title" | "{n} titles" % n),
            Request::TimeShift => tr!("{n} time shift" | "{n} time shifts" % n),
            Request::Turn => tr!("{n} turn" | "{n} turns" % n),
            Request::Export => tr!("{n} export" | "{n} exports" % n),
            Request::Queue => tr!("{n} queue" | "{n} queues" % n),
            Request::Reset => tr!("{n} reset" | "{n} resets" % n),
            Request::Preset => tr!("{n} preset" | "{n} presets" % n),
            Request::Paste => tr!("{n} paste" | "{n} pastes" % n),
            Request::Sync => tr!("{n} sync" | "{n} syncs" % n),
            Request::Undo => tr!("{n} undo" | "{n} undos" % n),
            Request::Redo => tr!("{n} redo" | "{n} redos" % n),
        }
    }

    /// What became of the request for the `n` frames it named that are
    /// gone: "the rating key was not applied to it".
    pub(crate) fn not_applied(&self, n: usize) -> String {
        match self {
            // A pick names its one frame.
            Request::Open(_) => tr!("not opened"),
            Request::RatingKey => tr!("the rating key was not applied to it"
                | "the rating key was not applied to them" % n),
            Request::FlagKey => {
                tr!("the flag key was not applied to it"
                    | "the flag key was not applied to them" % n)
            }
            Request::LabelKey => {
                tr!("the label key was not applied to it"
                    | "the label key was not applied to them" % n)
            }
            Request::Keyword => {
                tr!("the keyword was not applied to it" | "the keyword was not applied to them" % n)
            }
            Request::Title => {
                tr!("the title was not applied to it" | "the title was not applied to them" % n)
            }
            Request::TimeShift => tr!("the time shift was not applied to it"
                | "the time shift was not applied to them" % n),
            Request::Turn => {
                tr!("the turn was not applied to it" | "the turn was not applied to them" % n)
            }
            Request::Export => {
                tr!("the export was not applied to it" | "the export was not applied to them" % n)
            }
            Request::Queue => {
                tr!("the queue was not applied to it" | "the queue was not applied to them" % n)
            }
            Request::Reset => {
                tr!("the reset was not applied to it" | "the reset was not applied to them" % n)
            }
            Request::Preset => {
                tr!("the preset was not applied to it" | "the preset was not applied to them" % n)
            }
            Request::Paste => {
                tr!("the paste was not applied to it" | "the paste was not applied to them" % n)
            }
            Request::Sync => {
                tr!("the sync was not applied to it" | "the sync was not applied to them" % n)
            }
            Request::Undo => {
                tr!("the undo was not applied to it" | "the undo was not applied to them" % n)
            }
            Request::Redo => {
                tr!("the redo was not applied to it" | "the redo was not applied to them" % n)
            }
        }
    }
}

/// A request for some frames' sidecars, with what to do once they are
/// in. Requests run in the order they were made, each on exactly the
/// frames it named; a later one never cancels an earlier one.
pub(crate) struct Pending {
    id: u64,
    frames: Vec<Key>,
    scope: Scope,
    action: Option<Action>,
    /// What the action is, for the status line when it is dropped or a
    /// frame it named is gone.
    what: Request,
    /// The request is a frame's pick (`open_row`): dropped, it lets the
    /// pick go.
    pick: bool,
    /// The read is in, or there was nothing to read: ready to run once
    /// every request before it has.
    landed: bool,
    /// When the request was made: what it records when it runs stands
    /// in the session's order where the press did
    /// ([`greycard_edit::Order::during`]), not where its read landed.
    at: greycard_edit::Order,
}

/// The dropped requests, said: "3 rating keys and the open of x.cr3".
fn describe(dropped: &[Pending]) -> String {
    let mut kinds: Vec<(&Request, usize, bool)> = Vec::new();
    for p in dropped {
        match kinds.iter_mut().find(|(w, _, _)| **w == p.what) {
            Some((_, n, _)) => *n += 1,
            None => kinds.push((&p.what, 1, p.pick)),
        }
    }
    let words: Vec<String> = kinds
        .iter()
        // A pick is one, however many asked for it.
        .map(|&(what, n, pick)| {
            if pick || n == 1 {
                what.the()
            } else {
                what.counted(n)
            }
        })
        .collect();
    match words.len() {
        0 => String::new(),
        1 => words[0].clone(),
        n => tr!("{} and {}", words[..n - 1].join(", "), words[n - 1]),
    }
}

/// The requests out, in order, and the bar over their reads: state of
/// its own, beside the roots view's read and its bar (`roots::Library`),
/// which a frames' read never touches.
#[derive(Default)]
pub(crate) struct Loads {
    pub(crate) queue: VecDeque<Pending>,
    next: u64,
    /// Every read out on its thread, by its request's id, with the
    /// paths it reads: a request dropped from the queue still lands its
    /// read, so this, not the queue, is what is in flight.
    pub(crate) in_flight: std::collections::HashMap<u64, Vec<PathBuf>>,
    /// How many sidecars the reads out cover, and how many are in.
    pub(crate) progress: Option<Arc<Progress>>,
    /// Since when a read has been out.
    pub(crate) since: Option<Instant>,
    /// The count read and its total as the timer last saw them, when
    /// the count last moved (for the give-up), and when either did: a
    /// request joining a read out adds to the total, and is not
    /// "Waiting" from its first tick.
    seen: (usize, usize),
    moved: Option<Instant>,
    counted: Option<Instant>,
    pub(crate) timer: slint::Timer,
}

impl Loads {
    /// Whether any request is still waiting on its read.
    pub(crate) fn reading(&self) -> bool {
        self.queue.iter().any(|p| !p.landed)
    }

    /// Whether a request waiting here will record a mark or step the
    /// marks: a culling key, a keyword, a title, a time shift, or an
    /// undo or redo queued behind one. An undo pressed meanwhile waits
    /// its turn, so it takes back the last change asked for, not the
    /// last that landed.
    pub(crate) fn marks_waiting(&self) -> bool {
        self.queue.iter().any(|p| {
            matches!(
                p.what,
                Request::RatingKey
                    | Request::FlagKey
                    | Request::LabelKey
                    | Request::Keyword
                    | Request::Title
                    | Request::TimeShift
                    | Request::Undo
                    | Request::Redo
            )
        })
    }

    /// Whether a frame's open is waiting here: the pick is current,
    /// its line a stand-in until the read lands.
    pub(crate) fn open_waiting(&self) -> bool {
        self.queue
            .iter()
            .any(|p| matches!(p.what, Request::Open(_)))
    }

    /// What is queued, in order, for the tests.
    #[cfg(test)]
    pub(crate) fn whats(&self) -> Vec<String> {
        self.queue.iter().map(|p| p.what.log()).collect()
    }
}

/// How long a frames' read may be out, its count not moving, before
/// it is given up on: the roots view's two limits (§192).
const FRAMES_GIVE_UP: Duration = Duration::from_secs(60);
const FRAMES_STALLED: Duration = Duration::from_secs(10);

/// Where each of `frames` is in the list now, as a cell, for an action
/// of `scope`. A version whose line a read has found gone (the sidecar
/// rewritten outside, the version deleted from another cell) is, for a
/// line action, nobody's: `None`, said as a frame gone, and nothing is
/// laid on it or claimed of it. For a file action it is its file, which
/// is there still: its cell while the cells have not followed, else the
/// file's own cell.
pub(crate) fn resolve(st: &State, frames: &[Key], scope: Scope) -> Vec<Option<usize>> {
    let file_at: HashMap<&Path, usize> = st
        .files
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_path(), i))
        .collect();
    frames
        .iter()
        .map(|k| {
            let file = *file_at.get(k.path.as_path())?;
            match (scope, st.cell_of(file, k.version())) {
                (Scope::Line, Some(c)) => st.line(c).is_some().then_some(c),
                (Scope::Line, None) => None,
                (Scope::File, Some(c)) => Some(c),
                (Scope::File, None) => st.file_cell(file),
            }
        })
        .collect()
}

/// What a request left to do after it was queued.
enum Queued {
    /// Nothing to read and nothing ahead: the action runs now.
    Inline(Action, Vec<Key>),
    /// Queued; these paths are to be read for it (none when an earlier
    /// request is reading them, or there is nothing to read).
    Later { id: u64, to_read: Vec<PathBuf> },
}

/// Queue a request for `frames`' sidecars with `action`, from the
/// window's state: which of them are to be read (standing in, under no
/// offline root, and not already being read for an earlier request). A
/// file and a version of it are one sidecar, read once.
fn queue(
    st: &mut State,
    frames: Vec<Key>,
    scope: Scope,
    what: Request,
    pick: bool,
    action: Action,
) -> Queued {
    let listed = resolve(st, &frames, scope);
    let reading: HashSet<&Path> = st
        .views
        .loads
        .queue
        .iter()
        .filter(|p| !p.landed)
        .flat_map(|p| p.frames.iter().map(|k| k.path.as_path()))
        .collect();
    let mut once = HashSet::new();
    let to_read: Vec<PathBuf> = frames
        .iter()
        .zip(&listed)
        .filter(|(k, i)| {
            i.is_some_and(|i| !is_loaded(st, i) && !is_offline(st, i))
                && !reading.contains(k.path.as_path())
                && once.insert(k.path.as_path())
        })
        .map(|(k, _)| k.path.clone())
        .collect();
    if to_read.is_empty() && st.views.loads.queue.is_empty() {
        return Queued::Inline(action, frames);
    }
    st.views.loads.next += 1;
    let id = st.views.loads.next;
    let landed = to_read.is_empty();
    st.views.loads.queue.push_back(Pending {
        id,
        frames,
        scope,
        action: Some(action),
        what,
        pick,
        landed,
        at: greycard_edit::Order::act(),
    });
    Queued::Later { id, to_read }
}

/// A request for `frames`' sidecars, and `action` once they are in: run
/// at once when every one is in memory already and nothing is queued
/// ahead; else queued, the sidecars not yet read read on the pool with
/// the bar up, and the action run on the window's thread in its turn.
/// Frames under an offline root are not read and not waited for; each
/// action says what it does about them. True when the action ran now.
pub(crate) fn request(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    frames: Vec<Key>,
    scope: Scope,
    what: Request,
    action: Action,
) -> bool {
    let queued = queue(
        &mut state.borrow_mut(),
        frames,
        scope,
        what.clone(),
        false,
        action,
    );
    match queued {
        Queued::Inline(action, frames) => {
            let at = resolve(&state.borrow(), &frames, scope);
            action(state, app, worker, at.clone());
            say_gone(app, &frames, &at, &what);
            true
        }
        Queued::Later { id, to_read } => {
            if to_read.is_empty() {
                run_ready(state, app, worker);
            } else {
                read(&mut state.borrow_mut(), app, id, to_read);
            }
            false
        }
    }
}

/// [`request`] from inside the window's state, for a caller that holds
/// it (`open_row`): always queued, never run inline. When nothing is to
/// be read and nothing is ahead, the action runs at the next turn of
/// the event loop; a test's `land_pending` runs it.
pub(crate) fn request_in(
    st: &mut State,
    app: &App,
    frames: Vec<Key>,
    scope: Scope,
    what: Request,
    pick: bool,
    action: Action,
) {
    match queue(st, frames, scope, what.clone(), pick, action) {
        Queued::Inline(action, frames) => {
            st.views.loads.next += 1;
            let id = st.views.loads.next;
            st.views.loads.queue.push_back(Pending {
                id,
                frames,
                scope,
                action: Some(action),
                what,
                pick,
                landed: true,
                at: greycard_edit::Order::act(),
            });
            if !cfg!(test) {
                let app_weak = app.as_weak();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    let (Some(app), Some(state), Some(worker)) = (
                        app_weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    run_ready(&state, &app, &worker);
                });
            }
        }
        Queued::Later { id, to_read } => {
            if !to_read.is_empty() {
                read(st, app, id, to_read);
            }
        }
    }
}

/// `action` on `frames` once `paths`' mosaics are known, for a request
/// whose action is running and finds it needs them (a sync carrying a
/// demosaic onto frames the library has not indexed yet). Run now when
/// there are none to read; else queued at the front, ahead of every
/// request behind the one running, the mosaics read on a thread of
/// their own and kept for the session (`browser::learn_mosaic`), and
/// the action run on the window's thread once they are in.
// A request's frames, scope, words, and action, passed apart in every
// call of this module, and the paths to read.
#[allow(clippy::too_many_arguments)]
pub(crate) fn after_mosaics(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    paths: Vec<PathBuf>,
    frames: Vec<Key>,
    scope: Scope,
    what: Request,
    action: Action,
) {
    // A test that does not hold the read has it here, in place.
    #[cfg(test)]
    let paths = if HOLD_MOSAICS.with(|h| h.get()) {
        paths
    } else {
        let read = read_mosaics(paths);
        let mut st = state.borrow_mut();
        for (path, m) in read {
            crate::panel::browser::learn_mosaic(&mut st, app, &path, m);
        }
        Vec::new()
    };
    if paths.is_empty() {
        let at = resolve(&state.borrow(), &frames, scope);
        action(state, app, worker, at);
        return;
    }
    let mut st = state.borrow_mut();
    st.views.loads.next += 1;
    let id = st.views.loads.next;
    st.views.loads.queue.push_front(Pending {
        id,
        frames,
        scope,
        action: Some(action),
        what,
        pick: false,
        landed: false,
        at: greycard_edit::Order::act(),
    });
    let line = tr!("reading {n} frame..." | "reading {n} frames..." % paths.len());
    app.set_status(line.as_str().into());
    st.views.reading_line = Some(line);
    #[cfg(test)]
    {
        MOSAICS_QUEUED.with(|q| q.borrow_mut().push((id, paths)));
    }
    #[cfg(not(test))]
    {
        let app_weak = app.as_weak();
        let kept = paths.clone();
        let spawned = std::thread::Builder::new()
            .name("greycard mosaics read".into())
            .spawn(move || {
                let started = Instant::now();
                let n = paths.len();
                let results = read_mosaics(paths);
                tracing::debug!(
                    "{n} mosaic(s) read off the window's thread in {:.1} ms",
                    started.elapsed().as_secs_f64() * 1e3
                );
                let _ = slint::invoke_from_event_loop(move || {
                    let (Some(app), Some(state), Some(worker)) = (
                        app_weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    mosaics_landed(&state, &app, &worker, id, results);
                });
            });
        if let Err(e) = spawned {
            // No thread to be had: read at the next turn, on the
            // window's thread, as before; the caller holds the state.
            tracing::warn!("{e}; the mosaics read on the window's thread");
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::ZERO, move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                mosaics_landed(&state, &app, &worker, id, read_mosaics(kept));
            });
        }
    }
}

/// Each of `paths`' mosaics, read from its image description
/// (`decode::mosaic_path`, a millisecond or two locally); a file that
/// will not say is unreadable, said in the log.
fn read_mosaics(paths: Vec<PathBuf>) -> Vec<(PathBuf, crate::panel::browser::OpenMosaic)> {
    use crate::panel::browser::OpenMosaic;
    paths
        .into_iter()
        .map(|path| {
            let m = match greycard_core::decode::mosaic_path(&path) {
                Ok(Some(m)) => OpenMosaic::Known(m),
                Ok(None) => OpenMosaic::NotMosaic,
                Err(e) => {
                    tracing::warn!("{}: its mosaic: {e}", file_name(&path));
                    OpenMosaic::Unreadable
                }
            };
            (path, m)
        })
        .collect()
}

/// The mosaics read for request `id`, on the window's thread: kept,
/// the request marked, and the requests ready run in order.
fn mosaics_landed(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    id: u64,
    results: Vec<(PathBuf, crate::panel::browser::OpenMosaic)>,
) {
    {
        let mut st = state.borrow_mut();
        for (path, m) in results {
            crate::panel::browser::learn_mosaic(&mut st, app, &path, m);
        }
        match st.views.loads.queue.iter_mut().find(|p| p.id == id) {
            Some(p) => p.landed = true,
            None => tracing::debug!("a mosaics read came in for a request since dropped"),
        }
        // A sidecar read that landed while this was out left the bar
        // to whoever landed last.
        if !st.views.loads.reading() {
            bar_done(&mut st, app);
        }
    }
    run_ready(state, app, worker);
}

/// A held test's mosaic reads, each read in place, not yet landed.
#[cfg(test)]
pub(crate) type HeldMosaics = Vec<(u64, Vec<(PathBuf, crate::panel::browser::OpenMosaic)>)>;

/// The mosaic reads a held test asked for, read here.
#[cfg(test)]
pub(crate) fn read_held_mosaics() -> HeldMosaics {
    let held: Vec<(u64, Vec<PathBuf>)> =
        MOSAICS_QUEUED.with(|q| q.borrow_mut().drain(..).collect());
    held.into_iter()
        .map(|(id, paths)| (id, read_mosaics(paths)))
        .collect()
}

/// [`read_held_mosaics`]' reads landed, in order.
#[cfg(test)]
pub(crate) fn land_mosaics(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, read: HeldMosaics) {
    for (id, results) in read {
        mosaics_landed(state, app, worker, id, results);
    }
}

#[cfg(test)]
thread_local! {
    /// Whether a test's mosaic reads wait for `land_pending`, as off
    /// the window's thread they would; else they are read in place.
    pub(crate) static HOLD_MOSAICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The mosaic reads a held test's requests asked for, by id.
    static MOSAICS_QUEUED: std::cell::RefCell<Vec<(u64, Vec<PathBuf>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A read a test's request asked for: the request's id, the list's
/// generation, the paths to read, whether sidecars are on, and
/// what a raw with none starts with.
#[cfg(test)]
type QueuedRead = (
    u64,
    u64,
    Vec<PathBuf>,
    Option<greycard_edit::Placement>,
    greycard_edit_io::on_disk::Fresh,
);

#[cfg(test)]
thread_local! {
    /// The reads a test's requests asked for, waiting for the test to
    /// run and land them (`land_pending`): there is no event loop to
    /// land them on, as the roots' reads are queued.
    pub(crate) static QUEUED: std::cell::RefCell<Vec<QueuedRead>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The read for request `id`: `to_read`'s sidecars on the pool, landed
/// on the window's thread when in, under the list it was started on
/// (`Views::view_generation`): started under another list, it fills
/// nothing. In a test, queued for `land_pending`.
fn read(st: &mut State, app: &App, id: u64, to_read: Vec<PathBuf>) {
    let write = st.window.sidecars();
    let fresh = st.window.fresh();
    let generation = st.views.view_generation;
    bar_started(st, app, to_read.len());
    st.views.loads.in_flight.insert(id, to_read.clone());
    #[cfg(test)]
    {
        QUEUED.with(|q| q.borrow_mut().push((id, generation, to_read, write, fresh)));
    }
    #[cfg(not(test))]
    {
        let progress = st.views.loads.progress.clone();
        let app_weak = app.as_weak();
        let paths = to_read.clone();
        let spawned = std::thread::Builder::new()
            .name("greycard sidecars read".into())
            .spawn(move || {
                let started = Instant::now();
                let none = vec![None; to_read.len()];
                // Nothing waits on it: the frame's header unmapped, as
                // every reader off the window's thread asks it.
                let held = bring(
                    &to_read,
                    &none,
                    write,
                    fresh,
                    progress.as_deref().map(|p| &p.read),
                    greycard_edit_io::on_disk::frame_shape,
                );
                tracing::debug!(
                    "{} sidecar(s) read on the pool in {:.1} ms",
                    to_read.len(),
                    started.elapsed().as_secs_f64() * 1e3
                );
                let results: Vec<(PathBuf, Held)> = to_read.into_iter().zip(held).collect();
                let _ = slint::invoke_from_event_loop(move || {
                    let (Some(app), Some(state), Some(worker)) = (
                        app_weak.upgrade(),
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    landed(&state, &app, &worker, id, generation, results);
                });
            });
        if let Err(e) = spawned {
            // No thread to be had: read here, on the window's thread.
            tracing::warn!("{e}; the sidecars read on the window's thread");
            st.views.loads.in_flight.remove(&id);
            let none = vec![None; paths.len()];
            // The window waits on it: its mapped header read.
            let held = bring(
                &paths,
                &none,
                write,
                fresh,
                None,
                crate::panel::browser::header_shape,
            );
            let results: Vec<(PathBuf, Held)> = paths.into_iter().zip(held).collect();
            for (path, h) in results {
                if let Some(file) = st.file_by_path(&path)
                    && !file_is_loaded(st, file)
                {
                    take(st, app, file, h);
                }
            }
            if let Some(p) = st.views.loads.queue.iter_mut().find(|p| p.id == id) {
                p.landed = true;
            }
            if !st.views.loads.reading() {
                bar_done(st, app);
            }
            // The actions run at the next turn: the caller holds the
            // state.
            let app_weak = app.as_weak();
            slint::Timer::single_shot(Duration::ZERO, move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                run_ready(&state, &app, &worker);
            });
        }
    }
}

/// A test's turn at the reads its requests queued: each read in place
/// and landed, its action run in its turn; how many reads landed.
#[cfg(test)]
pub(crate) fn land_pending(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) -> usize {
    let mut n = 0;
    loop {
        let reads: Vec<QueuedRead> = QUEUED.with(|q| q.borrow_mut().drain(..).collect());
        let mosaics = read_held_mosaics();
        if reads.is_empty() && mosaics.is_empty() {
            break;
        }
        n += mosaics.len();
        land_mosaics(state, app, worker, mosaics);
        for (id, generation, to_read, write, fresh) in reads {
            let none = vec![None; to_read.len()];
            let held = bring(
                &to_read,
                &none,
                write,
                fresh,
                None,
                greycard_edit_io::on_disk::frame_shape,
            );
            landed(
                state,
                app,
                worker,
                id,
                generation,
                to_read.into_iter().zip(held).collect(),
            );
            n += 1;
        }
    }
    run_ready(state, app, worker);
    n
}

/// A read done, on the window's thread: each sidecar into the frame
/// its path is at, where that frame still stands in (a request dropped
/// meanwhile still brings the sidecars it read; they are as good), the
/// request marked, the bar moved on, and the requests ready to run
/// run in order. A read started under another list fills nothing: the
/// path may have been written since it started (a culling control, an
/// export's record), and its older bytes would be saved over the newer
/// by the next key. A stand-in is filled only by a read of its own list.
fn landed(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    id: u64,
    generation: u64,
    results: Vec<(PathBuf, Held)>,
) {
    {
        let mut st = state.borrow_mut();
        if st.views.view_generation == generation {
            for (path, h) in results {
                if let Some(file) = st.file_by_path(&path)
                    && !file_is_loaded(&st, file)
                {
                    take(&mut st, app, file, h);
                }
            }
        } else {
            tracing::debug!(
                "a read of {} sidecar(s) started under another list; nothing taken from it",
                results.len()
            );
        }
        match st.views.loads.queue.iter_mut().find(|p| p.id == id) {
            Some(p) => p.landed = true,
            None => tracing::debug!("a sidecar read came in for a request since dropped"),
        }
        st.views.loads.in_flight.remove(&id);
        crate::panel::look_rename::read_landed(&mut st, app, id);
        if !st.views.loads.reading() {
            bar_done(&mut st, app);
        }
    }
    run_ready(state, app, worker);
}

/// Run every request at the front of the queue whose read is in, in
/// order, each on the frames its paths are at now.
pub(crate) fn run_ready(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) {
    loop {
        let next = {
            let mut st = state.borrow_mut();
            match st.views.loads.queue.front() {
                Some(p) if p.landed => st.views.loads.queue.pop_front(),
                _ => None,
            }
        };
        let Some(mut p) = next else {
            return;
        };
        let at = resolve(&state.borrow(), &p.frames, p.scope);
        if let Some(action) = p.action.take() {
            greycard_edit::Order::during(p.at, || action(state, app, worker, at.clone()));
        }
        say_gone(app, &p.frames, &at, &p.what);
    }
}

/// A frame a request named that is no longer in the list when the
/// request runs (moved, or gone, or a version since deleted) took
/// nothing of it: said, after the action has had its say.
fn say_gone(app: &App, frames: &[Key], at: &[Option<usize>], what: &Request) {
    let gone: Vec<&Key> = frames
        .iter()
        .zip(at)
        .filter(|(_, i)| i.is_none())
        .map(|(p, _)| p)
        .collect();
    let said = match gone.as_slice() {
        [] => return,
        [one] => tr!(
            "{frame} moved or is gone; {not_applied}",
            frame = one.said(),
            not_applied = what.not_applied(1)
        ),
        many => tr!(
            "{n} frame moved or is gone; {not_applied}"
                | "{n} frames moved or are gone; {not_applied}" % many.len(),
            not_applied = what.not_applied(many.len())
        ),
    };
    tracing::info!(
        "{} frame(s) moved or gone; the {} was not applied to them",
        gone.len(),
        what.log()
    );
    app.set_status(said.into());
}

/// The pick whose sidecar was on its way is over, opened or not: no
/// pick pending, and the "reading X's sidecar..." line it put up goes
/// with it, when it is still up (an undo that selected another frame,
/// Escape out of culling, a click elsewhere, the read given up on).
/// In culling the bar goes with it even when the line is gone: the
/// mode's own line takes the status over while a slow read is out,
/// and nothing else there puts the bar up.
pub(crate) fn pick_over(st: &mut State, app: &App) {
    if st.views.pick_pending.take().is_none() {
        return;
    }
    let line = st.views.reading_line.take();
    if line.is_some_and(|l| app.get_status() == l.as_str()) {
        app.set_status("".into());
        app.set_busy(false);
    } else if st.culling.cull.is_some() {
        app.set_busy(false);
    }
}

/// The list was replaced: every request out is dropped, actions and
/// all, since the frames they named are gone; a read still out lands
/// on whatever of its paths the new list has.
pub(crate) fn list_replaced(st: &mut State, app: &App) {
    if !st.views.loads.queue.is_empty() {
        tracing::debug!(
            "{} request(s) for sidecars dropped with the list",
            st.views.loads.queue.len()
        );
    }
    st.views.loads.queue.clear();
    pick_over(st, app);
    bar_done(st, app);
}

/// A read of `n` sidecars going out: the bar's count, and the bar's
/// timer if it is not running. The bar itself is the roots view's
/// (`loading_shown` and its line), shown here only while no view's
/// read owns it.
fn bar_started(st: &mut State, app: &App, n: usize) {
    let now = Instant::now();
    let progress = st
        .views
        .loads
        .progress
        .get_or_insert_with(|| Arc::new(Progress::default()));
    let total = progress.total.fetch_add(n, Ordering::Relaxed) + n;
    let read = progress.read.load(Ordering::Relaxed);
    if st.views.loads.since.is_none() {
        st.views.loads.since = Some(now);
        st.views.loads.moved = Some(now);
        st.views.loads.counted = Some(now);
        st.views.loads.seen = (read, total);
        let app_weak = app.as_weak();
        st.views.loads.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                bar_tick(&state, &app, &worker, Instant::now());
            },
        );
    }
}

/// The timer's tick while frames' reads are out, at `now`: the bar
/// filled to the sidecars read, once a read has been out
/// `roots::LOADING_SHOWN_AFTER`; a read out past the give-up whose
/// count has stood still is given up on, its requests dropped.
pub(crate) fn bar_tick(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, now: Instant) {
    let gave_up = {
        let mut st = state.borrow_mut();
        let (Some(since), Some(progress)) = (st.views.loads.since, st.views.loads.progress.clone())
        else {
            bar_done(&mut st, app);
            return;
        };
        let total = progress.total.load(Ordering::Relaxed);
        let read = progress.read.load(Ordering::Relaxed).min(total);
        if (read, total) != st.views.loads.seen {
            if read != st.views.loads.seen.0 {
                st.views.loads.moved = Some(now);
            }
            st.views.loads.seen = (read, total);
            st.views.loads.counted = Some(now);
        }
        let out = now.saturating_duration_since(since);
        let still = now.saturating_duration_since(st.views.loads.moved.unwrap_or(since));
        let settled = now.saturating_duration_since(st.views.loads.counted.unwrap_or(since));
        if st.views.loads.queue.is_empty() {
            // Nothing left to wait on: the bar's work is done.
            bar_done(&mut st, app);
            false
        } else if out > FRAMES_GIVE_UP && still > FRAMES_STALLED {
            // Every request is dropped, those waiting on the read and
            // those queued behind them alike, and said by name; a pick
            // among them is let go, the frame left as an offline one is
            // shown, its stand-in on the panel.
            let dropped: Vec<Pending> = st.views.loads.queue.drain(..).collect();
            if dropped.iter().any(|p| p.pick) {
                pick_over(&mut st, app);
                app.set_busy(false);
            }
            let said = tr!(
                "{} dropped: the sidecars did not come in",
                describe(&dropped)
            );
            let kinds: Vec<String> = dropped.iter().map(|p| p.what.log()).collect();
            tracing::warn!(
                "requests dropped, the sidecars did not come in: {}",
                kinds.join(", ")
            );
            bar_done(&mut st, app);
            app.set_status(said.into());
            true
        } else {
            if out >= crate::roots::LOADING_SHOWN_AFTER && !st.views.library.loading {
                let fraction = if total == 0 {
                    0.0
                } else {
                    read as f32 / total as f32
                };
                app.set_loading_fraction(fraction);
                let waiting = (settled >= crate::roots::LOADING_WAITING_AFTER && read < total)
                    .then(|| {
                        let out = st.views.loads.queue.iter().filter(|p| !p.landed);
                        let paths = out.flat_map(|p| &p.frames);
                        crate::roots::one_root_named(
                            &st.views.library.roots,
                            paths.map(|k| k.path.as_path()),
                        )
                    })
                    .flatten();
                app.set_loading_line(
                    crate::roots::loading_words(read, total, waiting.as_deref()).into(),
                );
                app.set_loading_shown(true);
            }
            false
        }
    };
    if gave_up {
        run_ready(state, app, worker);
    }
}

/// No frames' read is out: the bar's timer stopped, and the bar gone
/// unless a view's read has it.
pub(crate) fn bar_done(st: &mut State, app: &App) {
    st.views.loads.since = None;
    st.views.loads.progress = None;
    st.views.loads.moved = None;
    st.views.loads.counted = None;
    st.views.loads.seen = (0, 0);
    st.views.loads.timer.stop();
    if !st.views.library.loading {
        app.set_loading_shown(false);
    }
}

/// Cell `c`'s whole sidecar in hand: read now if it was standing in
/// from its row, one read on this thread. False when it cannot be
/// read, its root being offline, which the status line then says.
/// The meta the read brings replaces the row's (a stale row, an XMP
/// beside the frame taken), and the frame's badges follow.
pub(crate) fn load_frame(st: &mut State, app: &App, c: usize) -> bool {
    let Some(file) = st.file_at(c) else {
        return false;
    };
    load_file(st, app, file)
}

/// [`load_frame`] by the file.
pub(crate) fn load_file(st: &mut State, app: &App, file: usize) -> bool {
    if file_is_loaded(st, file) {
        return true;
    }
    if let Some(root) = offline_root_of(st, &st.files[file]) {
        tracing::info!(
            "{}: its sidecar is not read, {} being offline",
            file_name(&st.files[file]),
            root.display()
        );
        say_offline(st, app, &root, "");
        return false;
    }
    if let Some(why) = st.from_row[file].unread.clone() {
        // A request's read just failed on it: not tried again here, on
        // the window's thread; said again.
        app.set_status(why.into());
        return false;
    }
    let held = from_disk(&st.files[file], st.window.sidecars(), st.window.fresh());
    take(st, app, file, held)
}

/// File `file`'s sidecar read again from the disk and taken, whatever
/// the window held of it: another writer (the Lightroom import) wrote
/// it. False when the read did not take.
pub(crate) fn reread_file(st: &mut State, app: &App, file: usize) -> bool {
    let path = st.files[file].clone();
    let held = from_disk(&path, st.window.sidecars(), st.window.fresh());
    crate::sidecars::reread(st, &path);
    take(st, app, file, held)
}

/// [`load_frames`] for a change to each frame's file: those whose
/// sidecar is read-only (§724) left out too, each logged and the
/// status line saying so.
pub(crate) fn writable_frames(st: &mut State, app: &App, cells: &[usize]) -> Vec<usize> {
    let mut kept = load_frames(st, app, cells);
    let before = kept.len();
    kept.retain(|&c| !crate::panel::browser::refused_read_only(st, c));
    if kept.len() < before {
        app.set_status(crate::panel::browser::newer_words().into());
    }
    kept
}

/// Every cell of `cells` loaded, those under an offline root left out
/// and said; the ones in hand, in the order given. A file is read once
/// however many of its cells are named; more than one to read is read
/// on the rayon pool.
pub(crate) fn load_frames(st: &mut State, app: &App, cells: &[usize]) -> Vec<usize> {
    let mut kept = Vec::with_capacity(cells.len());
    let mut to_read: Vec<usize> = Vec::new();
    let mut offline: Option<PathBuf> = None;
    let mut unread: Option<String> = None;
    for &c in cells {
        let Some(file) = st.file_at(c) else {
            continue;
        };
        if file_is_loaded(st, file) {
            kept.push(c);
        } else if let Some(root) = offline_root(st, c) {
            offline.get_or_insert(root);
        } else if let Some(why) = &st.from_row[file].unread {
            // A request's read just failed on it: left out, not read
            // again on the window's thread.
            unread.get_or_insert_with(|| why.clone());
        } else {
            kept.push(c);
            if !to_read.contains(&file) {
                to_read.push(file);
            }
        }
    }
    if let Some(root) = offline {
        let left_out = cells.len() - kept.len();
        tracing::info!(
            "{left_out} frame(s) under {} left out: it is offline",
            root.display()
        );
        say_offline(st, app, &root, &tr!(": left out"));
    }
    if let Some(why) = unread {
        app.set_status(why.into());
    }
    let mut failed: Vec<usize> = Vec::new();
    match to_read.len() {
        0 => {}
        1 => {
            let file = to_read[0];
            let held = from_disk(&st.files[file], st.window.sidecars(), st.window.fresh());
            if !take(st, app, file, held) {
                failed.push(file);
            }
        }
        n => {
            let started = std::time::Instant::now();
            let paths: Vec<PathBuf> = to_read.iter().map(|&f| st.files[f].clone()).collect();
            let rows = vec![None; n];
            // The window waits on the pool: its mapped header read,
            // never a raw read whole.
            let held = bring(
                &paths,
                &rows,
                st.window.sidecars(),
                st.window.fresh(),
                None,
                crate::panel::browser::header_shape,
            );
            for (file, h) in to_read.into_iter().zip(held) {
                if !take(st, app, file, h) {
                    failed.push(file);
                }
            }
            tracing::debug!(
                "{n} sidecars read on the pool in {:.1} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
    }
    // A sidecar that could not be read is left out: whatever the
    // caller would write, it would write over the frame's own. A
    // version's cell whose line the read found gone stays: a file action
    // (a key, a turn) is the file's still, and a line action asks the
    // line and lays nothing on none (`State::line_mut`).
    kept.retain(|&c| st.file_at(c).is_some_and(|f| !failed.contains(&f)));
    kept
}

/// A read of file `file`'s sidecar landing with `sidecar`, as a
/// request's does: for a test of what comes in late.
#[cfg(test)]
pub(crate) fn land_read(st: &mut State, app: &App, file: usize, sidecar: Sidecar) -> bool {
    let held = Held {
        sidecar,
        ..from_nothing(&st.files[file], st.window.fresh())
    };
    take(st, app, file, held)
}

/// `held` becomes file `file`'s: the sidecar, the seed, and the row no
/// longer standing in. Badges follow the meta the read brought, on
/// every cell of the file. A read that did not take is not taken — a
/// `.gcd` that could not be read, over any frame, or none found where
/// the row of a frame standing in says there is a develop — since the
/// next save would write the default edit over the frame's own: the
/// frame is held (standing in, what it held kept), the status line
/// says why, and false. A read that
/// found other versions than the row listed moves the list at the next
/// turn ([`crate::versions::settle_later`]): the caller may hold cell
/// numbers.
fn take(st: &mut State, app: &App, file: usize, held: Held) -> bool {
    let Held {
        sidecar,
        from_row,
        trouble,
        ..
    } = held;
    let standing_in = st.from_row.get(file).is_some_and(|r| !r.read);
    let why = match &trouble {
        Some(Trouble::Unreadable(e)) => Some(unreadable_words(&st.files[file], e)),
        Some(Trouble::Missing)
            if standing_in
                && (st.from_row[file].edited
                    || st.from_row[file].exported
                    || !st.from_row[file].versions.is_empty()) =>
        {
            // A version, edited or not, is data the default must not go
            // over: its cell would go at the next turn, and the next save
            // would write a sidecar without it over the real one.
            Some(tr!(
                "{}'s sidecar was not found, though the library has an edit, an \
                 export or a version of it on record; nothing is written over it",
                file_name(&st.files[file])
            ))
        }
        _ => None,
    };
    if let Some(why) = why {
        tracing::warn!("{why}");
        app.set_status(why.clone().into());
        // A loaded frame read again (another writer's file) is held
        // too, keeping what it held.
        st.from_row[file].read = false;
        st.from_row[file].unread = Some(why);
        return false;
    }
    st.sidecars[file] = sidecar;
    // A read that was out when a look it names was renamed: it takes the
    // new name, the open frame's too, before the panel is opened on it.
    crate::panel::look_rename::caught_up(st, file);
    // A merged file's edit held in memory, sidecars being off.
    crate::panel::merge::held(st);
    let iso = st.from_row[file].iso;
    st.from_row[file] = FromRow { iso, ..from_row };
    // The versions the read found against the cells the row made: a
    // version the row did not list, or one it listed that is gone,
    // moves the list once the caller is done with its numbers.
    if versions_differ(st, file) {
        crate::versions::settle_later(app);
    }
    // The cells follow what the read found: the meta, and the marks
    // for an edit and an export, which the row only stood in for.
    for c in st.cells_of_file(file) {
        crate::panel::browser::show_badges(st, app, c);
    }
    // Its edits are known now: shown in its cells, or the camera's.
    crate::edited::refresh_file(st, file);
    // The frame on screen, its panel the stand-in's: opened on its own
    // sidecar now, whatever brought it in (a stalled read after the
    // give-up, a key's read, a preset's load), so the next slider
    // moves from the frame's edit and not over it. A pick still
    // pending is opened by its own request; culling's panel is not
    // the frame's.
    if let Some(c) = st.current
        && st.file_at(c) == Some(file)
        && st.culling.cull.is_none()
        && st.views.pick_pending.is_none()
        && st.views.panel_stand_in.as_deref() == st.files.get(file).map(PathBuf::as_path)
        && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone())
    {
        crate::panel::browser::open_frame(st, app, &worker, c);
    }
    true
}

/// Whether file `file`'s cells are not its sidecar's versions, in
/// order: the list is to follow the sidecar.
pub(crate) fn versions_differ(st: &State, file: usize) -> bool {
    let listed: Vec<Option<&str>> = st
        .cells_of_file(file)
        .into_iter()
        .map(|c| st.version_of(c))
        .collect();
    let wants = st.wanted_of_file(file);
    let wants: Vec<Option<&str>> = wants.iter().map(|v| v.as_deref()).collect();
    listed != wants
}

/// File `file`'s row as the index has it now, taken into the frame
/// while its sidecar stands in: the meta, the seed, the turns and the
/// versions. A frame already loaded keeps what memory holds. True when
/// what the browser shows of the frame changed (its badges, its turns,
/// a version's name, what the filter would say). A version the row now
/// lists that the cells lack, or one gone, moves the list at the next
/// turn: the caller may hold cell numbers.
pub(crate) fn apply_row(st: &mut State, file: usize, row: &RowMeta) -> bool {
    // A frame whose camera's frame the row read anew asks again for its
    // pictures, which are cut to it (a row whose maker's tags the step
    // to schema 17 left unread, read since).
    if st
        .files
        .get(file)
        .is_some_and(|path| note_camera_of_row(path, row))
    {
        reframed(st, file);
    }
    let Some(from) = st.from_row.get_mut(file) else {
        return false;
    };
    if from.read {
        return false;
    }
    let next = FromRow::of(row);
    let edited = from.edited != next.edited || from.versions != next.versions;
    let rewritten = from.sidecar != next.sidecar || from.develop_key != next.develop_key;
    let turned = from.turns != next.turns || edited || from.exported != next.exported;
    *from = next;
    // Whether each cell shows an edit, and under what key, as the row
    // now says.
    if rewritten {
        for c in st.cells_of_file(file) {
            crate::edited::changed(st, c);
        }
    } else if edited {
        crate::edited::refresh_file(st, file);
    }
    let mut moved = st.sidecars[file].meta != row.meta;
    if moved {
        st.sidecars[file].meta = row.meta.clone();
    }
    // The versions as the row lists them now, on the stand-in: a name
    // changed shows on its cell, and one made or deleted elsewhere
    // moves the list.
    let named = |versions: &[greycard_edit::Version]| -> Vec<(String, String)> {
        versions
            .iter()
            .map(|v| (v.id.clone(), v.name.clone()))
            .collect()
    };
    let listed: Vec<(String, String)> = row
        .versions
        .iter()
        .map(|v| (v.id.clone(), v.name.clone()))
        .collect();
    if named(&st.sidecars[file].versions) != listed {
        let line = st.sidecars[file].line.clone();
        st.sidecars[file].versions = versions_of_row(row, || line.clone());
        moved = true;
        if versions_differ(st, file)
            && let Some(app) = st.window.app.upgrade()
        {
            crate::versions::settle_later(&app);
        }
    }
    moved || turned
}

/// File `file`'s camera frame became known (its row read since its
/// pictures were made): the culling loupe's, the stand-in's and the
/// lightbox's copies of the camera's picture go, to be asked for again
/// cut; and its cells' thumbnails are asked for again as they are in
/// view, those in view now at once ([`ask_reframed`]).
fn reframed(st: &mut State, file: usize) {
    let Some(path) = st.files.get(file).cloned() else {
        return;
    };
    st.views.reframed.insert(path);
    for c in st.cells_of_file(file) {
        crate::lightbox::forget(st, c);
        for cull in [st.culling.cull.as_mut(), st.culling.hold.as_mut()]
            .into_iter()
            .flatten()
        {
            if cull.cache.get(c).is_some_and(|p| !p.edited) {
                cull.cache.forget(c);
            }
        }
    }
    if st.culling.cull.is_some() {
        crate::panel::cull::cull_refresh(st);
    }
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    for (first, last) in [st.views.grid_shown, st.views.strip_shown]
        .into_iter()
        .flatten()
    {
        ask_reframed(st, &worker, first.max(0) as usize, last.max(0) as usize);
    }
}

/// The cells shown in rows `first..=last` whose file was reframed
/// ([`reframed`]) and has a picture in hand ask for it again, cut; a
/// file goes off the list once asked. Called where the grid and the
/// strip say what they show.
pub(crate) fn ask_reframed(st: &mut State, worker: &Worker, first: usize, last: usize) {
    if st.views.reframed.is_empty() {
        return;
    }
    let mut asked = Vec::new();
    for row in first..=last {
        let Some(&c) = st.views.shown.get(row) else {
            break;
        };
        let path = st.path_of(c);
        if st.views.reframed.contains(path) && st.views.thumb_made.get(c).is_some_and(|&m| m > 0) {
            asked.push(path.clone());
            worker.send(thumb_job(st, c));
        }
    }
    for path in asked {
        st.views.reframed.remove(&path);
    }
}

/// The camera's frame of the file at `path` as its row says, where the
/// row's maker's tags are read; what was known goes where they are not,
/// to be read with them. True when the row moved what a picture of the
/// file is cut to.
fn note_camera_of_row(path: &std::path::Path, row: &RowMeta) -> bool {
    match row.camera_frame {
        Some(camera) => crate::edited_pictures::note_camera(path, camera),
        None => {
            crate::edited_pictures::forget_camera(path);
            false
        }
    }
}

/// The thumbnail job for cell `c`: its frame's picture made from the
/// file, or, under a root that is offline, looked up in the cache by
/// the row's key and nothing else, since the file cannot be read.
pub(crate) fn thumb_job(st: &State, c: usize) -> Job {
    let frame = st.key_of(c).expect("a cell of the list");
    if is_offline(st, c)
        && let Some((hash, stamp)) = st.from_row.get(st.file_of(c)).and_then(|r| r.key.clone())
    {
        return Job::CachedThumbnail {
            index: c,
            frame,
            hash,
            stamp,
        };
    }
    Job::Thumbnail { index: c, frame }
}

/// The roots found offline changed under the list: a frame whose root
/// came back has its picture asked for again, since a miss in the
/// cache while it was offline was counted as a failure, and so has one
/// whose picture is smaller than the cells want, since a miss at the
/// larger size left the smaller up; every tile's dimming follows in the
/// rebuild the caller does.
pub(crate) fn roots_changed(
    st: &mut State,
    worker: &crate::worker::Worker,
    was: &HashSet<PathBuf>,
    now: &HashSet<PathBuf>,
) {
    let back: Vec<&PathBuf> = was.iter().filter(|r| !now.contains(*r)).collect();
    if back.is_empty() {
        return;
    }
    let mut again = 0;
    let want = st.views.thumb_want;
    for c in 0..st.cells.len() {
        if !back.iter().any(|r| st.path_of(c).starts_with(r)) {
            continue;
        }
        let failed = st.views.thumb_failed.get(c) == Some(&true)
            && st.views.thumb_base.get(c).is_some_and(Option::is_none);
        // A picture in hand smaller than the cells want, a larger one
        // missed in the cache while the root was away.
        let made = st.views.thumb_made.get(c).copied().unwrap_or(0);
        let small = made > 0 && crate::grid::wants_bigger(made, want);
        if failed || small {
            st.views.thumb_failed[c] = false;
            if small {
                st.views.thumb_asked[c] = want;
            }
            worker.send(thumb_job(st, c));
            again += 1;
        }
    }
    if again > 0 {
        tracing::info!("library: {again} picture(s) asked for again, their root back online");
    }
}

#[cfg(test)]
mod unreadable {
    use super::*;
    use crate::testing::{remove_dir_retry, scratch_dir, state_for, window};

    /// The two `.gcd`s that will not read: an edit at a schema past
    /// this build's, as a later build writes, and one damaged.
    fn bodies() -> [(&'static str, String); 2] {
        let newer = greycard_edit::VERSION + 1;
        [
            ("A.CR3", format!(r#"{{"current": {{"version": {newer}}}}}"#)),
            ("B.CR3", "{ not json".to_string()),
        ]
    }

    /// A frame the index has no row for, whose `.gcd` will not read:
    /// held as a frame standing in from its row is, and never written
    /// over, however its meta is set. Before, it was listed with the
    /// default edit as its own, and the next save wrote that over the
    /// file.
    #[test]
    fn a_sidecar_from_disk_that_will_not_read_is_never_written_over() {
        let dir = scratch_dir("unreadable-from-disk");
        for (name, body) in bodies() {
            let raw = dir.join(name);
            std::fs::write(&raw, b"raw").unwrap();
            let gcd = greycard_edit::Sidecar::path_for(&raw);
            std::fs::write(&gcd, &body).unwrap();

            let held = bring(
                std::slice::from_ref(&raw),
                &[None],
                Some(greycard_edit::Placement::Beside),
                Default::default(),
                None,
                greycard_edit_io::on_disk::frame_shape,
            )
            .remove(0);
            assert!(!held.from_row.read, "{name}");
            assert!(held.from_row.unread.is_some(), "{name}");

            let app = window(1);
            let (state, _worker) = state_for(&app, vec![raw.clone()]);
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.sidecars[0] = held.sidecar;
            st.from_row[0] = held.from_row;
            assert!(!crate::sidecars::file_writable(&st, 0), "{name}");
            st.sidecars[0].meta.rating = 3;
            crate::sidecars::write_file(&mut st, 0);
            assert_eq!(std::fs::read_to_string(&gcd).unwrap(), body, "{name}");
        }
        remove_dir_retry(&dir);
    }

    /// A frame already loaded whose `.gcd` turned unreadable before a
    /// read again (another writer's, as the Lightroom import's): the
    /// read is not taken, the frame keeps what it held and is held,
    /// and nothing is written over the file.
    #[test]
    fn a_loaded_frame_read_again_unreadable_is_held() {
        let dir = scratch_dir("unreadable-reread");
        for (name, body) in bodies() {
            let raw = dir.join(name);
            std::fs::write(&raw, b"raw").unwrap();
            let gcd = greycard_edit::Sidecar::path_for(&raw);
            let app = window(1);
            let (state, _worker) = state_for(&app, vec![raw.clone()]);
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.sidecars[0].meta.rating = 2;
            crate::sidecars::write_file(&mut st, 0);
            assert!(gcd.exists(), "{name}");
            std::fs::write(&gcd, &body).unwrap();

            assert!(!reread_file(&mut st, &app, 0), "{name}");
            assert_eq!(st.sidecars[0].meta.rating, 2, "{name}: what it held");
            assert!(st.from_row[0].unread.is_some(), "{name}");
            assert!(!crate::sidecars::file_writable(&st, 0), "{name}");
            st.sidecars[0].meta.rating = 5;
            crate::sidecars::write_file(&mut st, 0);
            assert_eq!(std::fs::read_to_string(&gcd).unwrap(), body, "{name}");
        }
        remove_dir_retry(&dir);
    }
}

#[cfg(test)]
mod camera_frames {
    use super::*;
    use greycard_core::raw::Orientation;
    use std::path::PathBuf;

    fn row(camera_frame: Option<Option<greycard_core::raw::CameraFrame>>) -> RowMeta {
        RowMeta {
            id: 1,
            hash: "ab".repeat(32),
            mtime: 1,
            meta: Default::default(),
            edited: false,
            turns: 0,
            exported: false,
            iso: None,
            sidecar_hash: None,
            develop_key: None,
            versions: Vec::new(),
            taken: None,
            edited_at: None,
            focus: None,
            camera_frame,
        }
    }

    /// A row that comes to say a frame's camera frame, after the
    /// migration, drops the stand-in's camera picture of it (asked for
    /// again, cut) and marks its cells to ask for their thumbnails again
    /// as they come into view, not all at once: one out of view stays
    /// marked until it is shown.
    #[test]
    fn a_frame_told_its_camera_frame_asks_again_as_it_is_shown() {
        let dir = crate::testing::scratch_dir("rows-reframed");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| {
                let p = dir.join(format!("P000000{i}.ORF"));
                std::fs::write(&p, b"not a raw").unwrap();
                p
            })
            .collect();
        let app = crate::testing::window(files.len());
        let (state, worker) = crate::testing::state_for(&app, files.clone());
        let mut st = state.borrow_mut();
        st.views.thumb_made[..3].fill(256);
        let mut hold = crate::panel::cull::Cull::new("".into(), 1024);
        hold.cache.insert(
            2,
            Arc::new(crate::cull::Preview::new(4, 3, vec![0; 48], (4, 3), false)),
        );
        st.culling.hold = Some(hold);
        st.views.strip_shown = Some((0, 0));
        let frame = greycard_core::testing::three_two_frame(Orientation::Normal);
        apply_row(&mut st, 2, &row(Some(Some(frame))));
        assert!(st.views.reframed.contains(&files[2]), "marked, out of view");
        assert!(
            st.culling.hold.as_ref().unwrap().cache.get(2).is_none(),
            "the stand-in's camera picture goes, to come back cut"
        );
        // Told the same again: nothing moves.
        st.views.reframed.clear();
        apply_row(&mut st, 2, &row(Some(Some(frame))));
        assert!(st.views.reframed.is_empty());
        // Shown: asked, and off the list.
        st.views.reframed.insert(files[2].clone());
        ask_reframed(&mut st, &worker, 0, 1);
        assert!(
            st.views.reframed.contains(&files[2]),
            "rows 0 and 1 are others"
        );
        ask_reframed(&mut st, &worker, 2, 2);
        assert!(st.views.reframed.is_empty());
        drop(st);
        crate::edited_pictures::forget_camera(&files[2]);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The shape of a frame whose camera frame is not known yet is never
    /// read off a picture that may be cut to it: it waits. Known framed,
    /// it is the area's; known whole, a picture still says nothing of it
    /// (§724), and with no header to read it waits.
    #[test]
    fn a_frames_aspect_waits_rather_than_read_a_cut_picture() {
        let dir = crate::testing::scratch_dir("rows-aspect-unknown");
        let raw = dir.join("P0000001.ORF");
        std::fs::write(&raw, b"not a raw").unwrap();
        let app = crate::testing::window(1);
        let (state, _worker) = crate::testing::state_for(&app, vec![raw.clone()]);
        let mut st = state.borrow_mut();
        st.views.thumb_base[0] = Some((300, 200, vec![0; 300 * 200 * 3]));
        crate::edited_pictures::forget_camera(&raw);
        assert_eq!(crate::panel::browser::frame_aspect(&mut st, 0), None);
        let frame = greycard_core::testing::three_two_frame(Orientation::Normal);
        crate::edited_pictures::note_camera(&raw, Some(frame));
        let a = crate::panel::browser::frame_aspect(&mut st, 0).unwrap();
        assert!((a - 5184.0 / 3888.0).abs() < 1e-4, "{a}");
        crate::edited_pictures::note_camera(&raw, None);
        assert_eq!(crate::panel::browser::frame_aspect(&mut st, 0), None);
        drop(st);
        crate::edited_pictures::forget_camera(&raw);
        crate::testing::remove_scratch(state, &dir);
    }
}
