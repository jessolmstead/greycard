//! The neighbors' bases, made while the worker has nothing else to do
//! and put on the shelf, so a step to the next frame or the one before
//! finds its base made.
//!
//! The window says, with each open, which frames are around the frame
//! opened and which way the strip is being walked: the nearest ahead,
//! the one behind, then two more ahead, under the filter and the sort
//! the strip shows, each with the edit, the turn, and the seed an open
//! of it would send ([`Neighbor`]). The shelf's room says how many of
//! them are made ([`depth`]). Once that open's develop has landed, the
//! frame is still open in the editor, nothing is queued, no save waits
//! on a develop, no reduced develop is in hand, and the window has been
//! quiet for [`QUIET`], the worker decodes one neighbor and makes its
//! base as an open would, and shelves it as a base left behind is
//! shelved ([`Shelf::put`]): same key, same budget, the least recently
//! left going first. One neighbor at a time, so no more than one frame is
//! decoded beyond the open one.
//!
//! The engine cannot stop inside a stage, so the prefetch asks between
//! them (after the decode, the prepare, the demosaic, and the finish)
//! whether anything has been asked for since, and gives way when it
//! has: the request then waits for one stage at most. The one request
//! it does not give way to is the open of the very frame it is making,
//! which the rest of the base serves sooner than a base begun again.

use std::cell::Cell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::develop::CaRan;
use super::engine::file_label;
use super::shelf::{FileKey, Shelf};
use super::*;
use crate::stages::{Facts, Purpose, Stage, Step, stages};
use crate::walk::{Hold, Kept, NoModels, Place, Stopped, plan, walk};

/// Which way from the open frame a neighbor is, in the strip's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Next,
    Previous,
}

impl Side {
    /// The side in words, for the log.
    pub fn word(self) -> &'static str {
        match self {
            Side::Next => "next",
            Side::Previous => "previous",
        }
    }

    /// The frame `steps` away on this side, in words: "next", "second
    /// next", "third previous".
    pub fn at(self, steps: u8) -> String {
        match steps {
            0 | 1 => self.word().to_string(),
            2 => format!("second {}", self.word()),
            3 => format!("third {}", self.word()),
            n => format!("{n}th {}", self.word()),
        }
    }

    /// The other side.
    pub fn other(self) -> Side {
        match self {
            Side::Next => Side::Previous,
            Side::Previous => Side::Next,
        }
    }

    /// Which way the strip is walked toward this side, for the log.
    pub fn way(self) -> &'static str {
        match self {
            Side::Next => "right",
            Side::Previous => "left",
        }
    }
}

/// How many neighbors are made each way from the open frame: `ahead`
/// in the direction the strip is walked, `behind` the other way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth {
    pub ahead: u8,
    pub behind: u8,
}

/// The most the window sends and the worker makes: three ahead, one
/// behind.
pub const DEEPEST: Depth = Depth {
    ahead: 3,
    behind: 1,
};

/// Which way the strip is being walked: the side ahead, and how many
/// steps in a row have just gone that way (`streak`, none after a jump
/// or a step back).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heading {
    pub ahead: Side,
    pub streak: u8,
}

impl Heading {
    /// A walk under way toward `ahead`, two steps or more.
    pub const fn steady(ahead: Side) -> Self {
        Self {
            ahead,
            streak: STREAK_FOR_ONE,
        }
    }
}

/// The steps in a row ahead before a shelf with room for one base
/// makes the next: one step is as likely half a compare of two frames,
/// whose second the prefetch would push out.
pub const STREAK_FOR_ONE: u8 = 2;

/// How deep the prefetch goes with `room` bases the size of the open
/// frame's on the shelf beside it, the strip walked as `heading` says:
/// never more neighbors than the shelf holds. One behind once there is
/// room for two, since the one behind on a walk is the frame just left,
/// already shelved; the rest ahead, to [`DEEPEST`]. With room for one,
/// the next ahead only once [`STREAK_FOR_ONE`] steps have gone that
/// way, since it takes the place of the frame just left; a compare of
/// two frames then keeps both.
pub fn depth(room: u64, heading: Heading) -> Depth {
    if room == 1 && heading.streak < STREAK_FOR_ONE {
        return Depth {
            ahead: 0,
            behind: 0,
        };
    }
    let behind = u8::from(room >= 2).min(DEEPEST.behind);
    let ahead = room
        .saturating_sub(u64::from(behind))
        .min(u64::from(DEEPEST.ahead)) as u8;
    Depth { ahead, behind }
}

/// A frame beside the open one, as an open of it would be asked for:
/// its file, the edit its line holds, its quarter turns, and whether
/// its learned blend is still to be seeded from its ISO; which side of
/// the open frame it is on, and how many steps away (1 for the frame
/// an arrow key reaches). With `read_first`, the window does not hold
/// its edit (its sidecar stands in from the index's row, which says it
/// is edited): the edit, the turn, and the seed sent are placeholders,
/// and the worker reads the sidecar for them in the prefetch's own idle
/// slot ([`read_first`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Neighbor {
    pub path: PathBuf,
    pub edit: Edit,
    pub turn: u8,
    pub seed_blend: bool,
    pub side: Side,
    pub steps: u8,
    pub read_first: Option<ReadFirst>,
}

/// What the worker needs to read a neighbor's edit from its sidecar:
/// which line of it the strip's cell is, a version's id or none for the
/// file's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFirst {
    pub version: Option<Arc<str>>,
}

impl Neighbor {
    /// Where it is from the open frame, in words, for the log.
    pub fn place(&self) -> String {
        self.side.at(self.steps)
    }
}

/// The neighbors of the frame opened at `generation` still to be made,
/// in the order the window sent them (the nearest ahead first), which
/// way the strip is being walked (`walking`, the side ahead), and how
/// long the window must be quiet before the next of them begins:
/// [`QUIET`], longer each time one gives way.
pub(super) struct Neighbors {
    pub generation: u64,
    pub heading: Heading,
    pub waiting: VecDeque<Neighbor>,
    pub quiet: Duration,
    /// The files of the list as trimmed to its depth, made or still to
    /// be: no prefetch of it pushes one of their bases off the shelf
    /// ([`keep`]).
    pub named: Option<Vec<PathBuf>>,
    /// The depth it was trimmed to.
    pub depth: Option<Depth>,
    /// Which list this is, of all the worker was sent: a neighbor that
    /// gave way goes back only onto its own list, not one sent since.
    pub round: u64,
}

/// The files whose bases a prefetch of `list` may not push off the
/// shelf: those the list names, and the frame just left (`left`)
/// unless the depth keeps none behind, which is room for one base on a
/// walk under way, where the next frame takes its place.
pub(super) fn keep(list: &Neighbors, left: Option<&Path>) -> Vec<PathBuf> {
    let mut keep = list.named.clone().unwrap_or_default();
    if list.depth.is_some_and(|d| d.behind > 0) {
        keep.extend(left.map(Path::to_path_buf));
    }
    keep
}

/// The file at `path` opened, `opened` the file open before it and
/// `left` the one left before that: the frame left is the last other
/// file, so an open of the file already open (another version of it,
/// a reload after an outside edit, culling left on the same frame)
/// leaves it as it was.
pub(super) fn opened(opened: &mut Option<PathBuf>, left: &mut Option<PathBuf>, path: PathBuf) {
    if opened.as_ref() != Some(&path) {
        *left = opened.replace(path);
    }
}

/// The rounds of the lists sent ([`Neighbors::round`]).
static ROUNDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Neighbors {
    /// The neighbors of the frame opened at `generation`, the strip
    /// walked as `heading` says.
    pub(super) fn new(generation: u64, heading: Heading, waiting: Vec<Neighbor>) -> Self {
        Self {
            generation,
            heading,
            waiting: waiting.into(),
            quiet: QUIET,
            named: None,
            depth: None,
            round: ROUNDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Those beyond `depth` let go, the order kept; the first time, the
    /// files of those left are what the list names.
    fn trim(&mut self, depth: Depth) {
        let walking = self.heading.ahead;
        self.waiting.retain(|n| {
            n.steps
                <= if n.side == walking {
                    depth.ahead
                } else {
                    depth.behind
                }
        });
        if self.named.is_none() {
            self.named = Some(self.waiting.iter().map(|n| n.path.clone()).collect());
        }
        self.depth = Some(depth);
    }

    /// One of them gave way to a request and is to be begun again: the
    /// quiet asked of the window before it doubles, to eight times
    /// [`QUIET`]. Someone at work on the open frame, a slider moved
    /// after each look, would otherwise meet a stage to wait out at
    /// each move, and the work thrown away each time; the next open
    /// starts from [`QUIET`] again.
    pub(super) fn gave_way(&mut self, n: Neighbor, round: u64) {
        if round != self.round {
            return;
        }
        self.waiting.push_front(n);
        self.quiet = (self.quiet * 2).min(QUIET * 8);
    }
}

/// How long the window is left alone (no develop asked for, none
/// landing) before a prefetch begins: the reduced develop's own wait
/// while a frame is open, and for its reason. A slider paused for less
/// is not met by a decode that cannot stop, nor its next move by a
/// stage to wait out and work thrown away.
#[cfg(not(any(test, feature = "testing")))]
pub(super) const QUIET: Duration = crate::reduced::IDLE;
#[cfg(any(test, feature = "testing"))]
pub(super) const QUIET: Duration = Duration::from_millis(30);

/// What the idle worker does about the neighbors.
#[derive(Debug)]
pub(super) enum Due {
    /// Nothing: none to make, or not now.
    Nothing,
    /// Look again after this long, the window not quiet for long enough.
    Wait(Duration),
    /// Make this one's base.
    Fetch(Box<Neighbor>),
}

/// How long the idle worker waits to look again while a reduced
/// develop is in hand.
pub(super) const BEHIND_REDUCED: Duration = Duration::from_millis(250);

/// What the worker knows of itself when it is idle, for [`due`].
pub(super) struct Idle<'a> {
    /// The generation of the last open whose develop landed.
    pub landed: Option<u64>,
    /// What the open frame's walks kept.
    pub open: Option<&'a FrameKept>,
    /// A save of the open frame waits on a develop still to come.
    pub waiting: bool,
    pub shelf: Option<&'a Shelf>,
    /// How long since the window last asked for a develop or one ended.
    pub idle: Duration,
    /// A frame is open in the editor: not culling, the grid, or none.
    pub frame_open: bool,
    /// The window's device was lost, and the worker has not yet taken
    /// that up (it does at its next job): no prefetch begins, since every
    /// one would give way to it at its first check, its decode thrown away.
    pub device_lost: bool,
}

/// What the idle worker does about the neighbors waiting in `queued`,
/// asked only when the queue holds nothing else for it (no develop, no
/// open, no export, no save, no mask). Neighbors of an open since
/// replaced are let go, as are all of them once no frame is open in the
/// editor. Those beyond the [`depth`] the shelf has room for are let
/// go, the depth and the walk's direction said when they differ from
/// the last said (`said`, the worker's); a depth of none lets them all
/// go.
pub(super) fn due(
    queued: &mut Option<Neighbors>,
    said: &mut Option<(Depth, Side)>,
    now: &Idle<'_>,
) -> Due {
    let Some(neighbors) = queued.as_mut() else {
        return Due::Nothing;
    };
    if !now.frame_open {
        *queued = None;
        return Due::Nothing;
    }
    let Some(landed) = now.landed else {
        return Due::Nothing;
    };
    // Kept for after the loss is taken up.
    if now.device_lost {
        return Due::Nothing;
    }
    if neighbors.generation < landed {
        *queued = None;
        return Due::Nothing;
    }
    // For an open still to come, or one whose develop has not landed.
    if neighbors.generation > landed || now.waiting {
        return Due::Nothing;
    }
    let (Some(open), Some(shelf)) = (now.open, now.shelf) else {
        return Due::Nothing;
    };
    if neighbors.waiting.is_empty() {
        *queued = None;
        return Due::Nothing;
    }
    if now.idle < neighbors.quiet {
        return Due::Wait(neighbors.quiet - now.idle);
    }
    // Counted in bases as a prefetch makes them, without the guide
    // plane the open frame's carries; each put is checked at its own
    // size besides (`Shelf::pushes_out_before`).
    let room = shelf.room_for(open.picture_bytes());
    let deep = depth(room, neighbors.heading);
    let walking = neighbors.heading.ahead;
    if *said != Some((deep, walking)) {
        *said = Some((deep, walking));
        note(depth_line(deep, walking));
    }
    if deep.ahead == 0 {
        *queued = None;
        return Due::Nothing;
    }
    neighbors.trim(deep);
    match neighbors.waiting.pop_front() {
        Some(n) => Due::Fetch(Box::new(n)),
        None => Due::Nothing,
    }
}

/// The log's line for the prefetch's depth and the walk's direction.
fn depth_line(depth: Depth, walking: Side) -> String {
    format!(
        "prefetch: {} ahead, {} behind, walking {}",
        depth.ahead,
        depth.behind,
        walking.way()
    )
}

/// Reserve the machine against the reduced develops for a prefetch:
/// `reserve` raises the flag that holds the pool's next one back, and
/// only then is the pool asked how many it has `in_hand`, so one begun
/// meanwhile is seen. True when none is, and the prefetch may begin;
/// either way the flag stays up, so a prefetch that waits for the one
/// in hand is the next to run and not behind every one still queued.
pub fn claim(reserve: impl FnOnce(), in_hand: impl FnOnce() -> usize) -> bool {
    reserve();
    in_hand() == 0
}

/// The reservation against the reduced develops let go, and the pool
/// woken (`wake`), unless the idle worker's `due` is a prefetch about
/// to claim it again: a stand-down retries and comes back to a fetch
/// while it is still due, and anything else (the list let go, a list
/// for an open that never landed, a save waiting, a quiet still to
/// come) would otherwise hold the reduced develops for as long as it
/// lasts.
pub(super) fn settle_reservation(
    due: &Due,
    edited: &crate::edited_pictures::Edited,
    wake: impl FnOnce(),
) {
    if !matches!(due, Due::Fetch(_)) && edited.prefetching() {
        edited.set_prefetching(false);
        wake();
    }
}

/// How many reduced develops the pool may begin now and when to look
/// again: none while the worker's prefetch holds its reservation
/// (`prefetching`), looked at again in [`BEHIND_REDUCED`] (the worker
/// also wakes the pool when it lets the reservation go); `otherwise`
/// says when it does not.
pub fn behind_prefetch(
    prefetching: bool,
    otherwise: impl FnOnce() -> (usize, Option<Duration>),
) -> (usize, Option<Duration>) {
    if prefetching {
        (0, Some(BEHIND_REDUCED))
    } else {
        otherwise()
    }
}

/// A neighbor as [`read_first`] leaves it.
#[derive(Debug, PartialEq)]
pub(super) enum Peeked {
    /// Its edit, turn, and seed are its sidecar's, or were the window's.
    Ready(Box<Neighbor>),
    /// Something was asked of the worker, before or after the read.
    GaveWay,
    /// Not made, for this reason: its sidecar did not read, or has no
    /// such line.
    Skipped(String),
}

/// Neighbor `n` with the edit, the turn, and the seed an open of it
/// would send, read from its sidecar when the window sent it to be read
/// first ([`Neighbor::read_first`]); as it came otherwise. The reader is
/// the window's own (`on_disk`'s), with no Lightroom develop taken and
/// nothing written: no sidecar, no index, no XMP, so a sidecar from an
/// older build is brought up to date in memory alone. The line is the
/// cell's (its version, or the file's own), the turn the file's, and the
/// seed still to come when that line was never developed, as the
/// window's open asks once it has read the sidecar (`rows::wants_seed`).
/// `asked` is whether anything has been asked of the worker since: asked
/// before the read, and after it, before the decode that cannot stop.
/// The window's own read at the real open stays the word on the edit: a
/// sidecar changed after this read gives a base whose steps the open's
/// do not match, which the shelf does not hand out. A sidecar that would
/// not serve (not there, not readable, without the cell's version) is
/// said once and kept in `peeks`, and skipped without a word or a read
/// at every open after, until it changes. A frame whose folder is on a
/// network mount or an automount not mounted yet, as the window's check
/// says ([`Peeks::not_to_peek`]), is not touched at all: a read cannot
/// be stopped, could hold the worker for as long as the share takes to
/// answer, and would mount an automount. Said once a folder.
pub(super) fn read_first(n: &Neighbor, peeks: &mut Peeks, asked: &dyn Fn() -> bool) -> Peeked {
    let Some(first) = &n.read_first else {
        return Peeked::Ready(Box::new(n.clone()));
    };
    // Not a raw: the prefetch says so, with nothing read.
    if is_picture_path(&n.path) {
        return Peeked::Ready(Box::new(n.clone()));
    }
    let name = file_label(&n.path);
    let side = n.place();
    let gave_way = |when: &str| {
        note(format!(
            "the prefetch of the {side} frame, {name}, abandoned {when} its sidecar's read: \
             the worker was asked for something"
        ));
        Peeked::GaveWay
    };
    let skipped = |why: String| {
        note(format!(
            "the prefetch of the {side} frame, {name}, skipped: {why}"
        ));
        Peeked::Skipped(why)
    };
    if asked() {
        return gave_way("before");
    }
    // Before anything of the frame's is touched on the disk.
    match peeks.not_to_peek(&n.path) {
        Some((why, true)) => return skipped(format!("its folder is on {why}")),
        Some((why, false)) => return Peeked::Skipped(format!("its folder is on {why}")),
        None => {}
    }
    let key = UnreadKey {
        path: n.path.clone(),
        version: first.version.clone(),
        sidecar: sidecar_stat(&n.path),
    };
    if peeks.holds(&key) {
        return Peeked::Skipped("its sidecar did not serve before, and has not changed".into());
    }
    let started = Instant::now();
    touched();
    let (sidecar, trouble) = greycard_edit_io::on_disk::peek_sidecar(&n.path);
    let why = match trouble {
        Some(greycard_edit_io::on_disk::Trouble::Missing) => {
            Some("its sidecar is not there".to_string())
        }
        Some(greycard_edit_io::on_disk::Trouble::Unreadable(e)) => {
            Some(format!("its sidecar does not read: {e}"))
        }
        None => None,
    };
    let line = sidecar.line(first.version.as_deref());
    let why = why.or_else(|| match line {
        None => Some("its sidecar has no such version".into()),
        // What is placed on it waits on a shape nothing can measure
        // (§724): no picture to make ahead of the open.
        Some(line) if line.ready().is_none() => {
            Some("nothing can measure the frame, and its masks wait on its shape".into())
        }
        Some(_) => None,
    });
    if let Some(why) = why {
        peeks.put(key);
        return skipped(why);
    }
    let line = line.expect("a line, or a reason");
    let ready = Neighbor {
        edit: line.current().clone(),
        turn: sidecar.turn,
        seed_blend: greycard_edit_io::on_disk::line_never_developed(line),
        read_first: None,
        ..n.clone()
    };
    note(format!(
        "the {side} frame, {name}: its sidecar read first, in {:.1} ms",
        started.elapsed().as_secs_f64() * 1e3
    ));
    at_check(&n.path, "sidecar's read");
    if asked() {
        return gave_way("after");
    }
    Peeked::Ready(Box::new(ready))
}

/// A neighbor's sidecar as it stood when it did not serve a read: the
/// frame, the cell's version, and the sidecar file found for it with its
/// length and time (none when there was none).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct UnreadKey {
    path: PathBuf,
    version: Option<Arc<str>>,
    sidecar: Option<(PathBuf, u64, Option<std::time::SystemTime>)>,
}

/// The sidecar `Sidecar::find` gives for `raw`, with its length and time.
fn sidecar_stat(raw: &Path) -> Option<(PathBuf, u64, Option<std::time::SystemTime>)> {
    touched();
    let path = greycard_edit::Sidecar::find(raw)?;
    let meta = path.metadata().ok();
    Some((
        path,
        meta.as_ref().map_or(0, std::fs::Metadata::len),
        meta.and_then(|m| m.modified().ok()),
    ))
}

/// A neighbor's frame touched on the disk by [`read_first`] (its
/// sidecar looked for, or read): counted in a test, which asks
/// [`touches`].
fn touched() {
    #[cfg(test)]
    TOUCHES.with(|t| t.set(t.get() + 1));
}

#[cfg(test)]
thread_local! {
    static TOUCHES: Cell<usize> = const { Cell::new(0) };
}

/// How many times [`read_first`] has touched a frame's files on this
/// thread.
#[cfg(test)]
pub(super) fn touches() -> usize {
    TOUCHES.with(Cell::get)
}

/// Why a folder is not to be read ahead (`a network mount (nfs4)`, `an
/// automount not mounted yet`), or none for a local one: the window's
/// `mounts::not_to_peek`, handed over. It asks the system's list of
/// mounts, never the mount.
pub type RemoteFs = fn(&Path) -> Option<String>;

/// What the worker keeps between reads of the neighbors' sidecars
/// ([`read_first`]), for the session: the latest [`UNREAD_KEPT`]
/// sidecars that did not serve a read, the window's check of what a
/// folder is on, and the folders said in the log not to be read.
#[derive(Debug, Default)]
pub(super) struct Peeks {
    unread: VecDeque<UnreadKey>,
    remote: Option<RemoteFs>,
    said: std::collections::HashSet<PathBuf>,
}

/// How many sidecars [`Peeks`] keeps, the oldest let go first.
const UNREAD_KEPT: usize = 64;

impl Peeks {
    fn holds(&self, key: &UnreadKey) -> bool {
        self.unread.contains(key)
    }

    fn put(&mut self, key: UnreadKey) {
        self.unread
            .retain(|k| k.path != key.path || k.version != key.version);
        if self.unread.len() >= UNREAD_KEPT {
            self.unread.pop_front();
        }
        self.unread.push_back(key);
    }

    /// The window's check of what a folder is on, from now on; the
    /// folders said are said again.
    pub(super) fn set_remote(&mut self, check: RemoteFs) {
        self.remote = Some(check);
        self.said.clear();
    }

    /// Why the folder of `raw` is not to be read ahead, and whether it is
    /// the first time that is said of the folder; none for a local
    /// folder, or with no check handed over (a worker with no window).
    /// Asked of the check every time, an answer kept for nothing: the
    /// list of mounts is local and cheap to read, and a share an
    /// automount mounts since, or one unmounted, is then seen at once.
    fn not_to_peek(&mut self, raw: &Path) -> Option<(String, bool)> {
        let check = self.remote?;
        let folder = raw.parent().unwrap_or(Path::new(""));
        let why = check(folder)?;
        Some((why, self.said.insert(folder.to_path_buf())))
    }
}

/// Whether the queue holds any job the worker would take up, or any
/// change it must take up first, that a prefetch gives way to: the open
/// of its own frame among them, since this is what the read of a
/// neighbor's sidecar ([`read_first`]) asks, before the frame's edit is
/// known to compare an open with. See [`others`] for what is not
/// counted, and why.
pub(super) fn asked(q: &Queue, frame_open: bool) -> bool {
    others(q, frame_open) || q.develop.is_some() || !q.keeps.is_empty()
}

/// What the queue `q` holds besides a develop and a save that a prefetch
/// gives way to (see [`ahead`]): every job the worker's loop breaks to,
/// a device or a context handed over or lost, and the masks a new base
/// asked for once their settling is due (which the loop would move onto
/// `masks` were it not held here).
///
/// Not counted: the masks of frames not open (`masks_for`), which wait
/// while a frame is open in the editor, and with none open the editor
/// is left, which is counted; a Subject model to forget and models
/// arrived, which change what a later mask job loads and are taken up
/// at the loop's next pass with nothing waiting on them; edits changed
/// on files, which [`moved_under`] counts for the neighbor's own file
/// and which only let shelved bases go otherwise; and the neighbors, the
/// landed open, and the folded generations, which are the prefetch's own
/// bookkeeping.
fn others(q: &Queue, frame_open: bool) -> bool {
    !frame_open
        || q.stopping
        || q.lenses.is_some()
        || q.gpu.is_some()
        || q.context.is_some()
        || q.gpu_lost.is_some()
        || q.people
        || q.landscape
        || q.mask_picture.is_some()
        || !q.masks.is_empty()
        || super::masks::settling_left(&q.settling, Instant::now())
            .is_some_and(|left| left.is_zero())
        || !q.exports.is_empty()
        || !q.people_on.is_empty()
}

/// What has been asked of the worker since a prefetch began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ahead {
    /// Nothing: the prefetch goes on.
    Nothing,
    /// The open of the frame being prefetched, and at most the saves of
    /// the frame being left: the prefetch goes on, and the open takes
    /// its base from the shelf.
    ItsOpen,
    /// Something else, which the prefetch gives way to.
    Work,
}

/// What the queue `q` holds that a prefetch of `n` should give way to.
/// `frame_open` is whether a frame is open in the editor: none open
/// (culling, the grid) is a prefetch no longer wanted, and while one is
/// the masks of frames not open wait in any case, so they are not
/// counted.
/// `facts` are the frame's, and `lenses` the database its list reads.
pub(super) fn ahead(
    q: &Queue,
    frame_open: bool,
    n: &Neighbor,
    facts: &Facts,
    lenses: Option<&greycard_lens::Database>,
) -> Ahead {
    if others(q, frame_open) || moved_under(q, n, facts, lenses) {
        return Ahead::Work;
    }
    match &q.develop {
        None if q.keeps.is_empty() => Ahead::Nothing,
        None => Ahead::Work,
        Some(Job::Open {
            path,
            edit,
            turn,
            seed_blend,
            ..
        }) if *path == n.path
            && turn % 4 == n.turn % 4
            && *seed_blend == n.seed_blend
            && crate::stages::same_base(
                edit,
                &n.edit,
                &Facts {
                    turn: n.turn % 4,
                    ..facts.clone()
                },
                lenses,
            ) =>
        {
            Ahead::ItsOpen
        }
        Some(_) => Ahead::Work,
    }
}

/// Whether the edit of neighbor `n`'s file changed since it was sent,
/// as the queue's latest word on the file says, so that the base made
/// for it is not one an open of it would take. A change after the base
/// (an exposure synced onto it) is no such change.
pub(super) fn moved_under(
    q: &Queue,
    n: &Neighbor,
    facts: &Facts,
    lenses: Option<&greycard_lens::Database>,
) -> bool {
    q.edits_changed
        .iter()
        .rev()
        .find(|(path, _)| *path == n.path)
        .is_some_and(|(_, now)| {
            !now.as_ref()
                .is_some_and(|now| now.serves(&n.edit, n.turn, facts, lenses))
        })
}

/// How a prefetch ended.
#[derive(Debug, PartialEq)]
pub(super) enum Fetched {
    /// Its base is on the shelf.
    Landed,
    /// It gave way before this stage, with what the decode said of
    /// the frame.
    Abandoned(&'static str, Box<Facts>),
    /// Not begun, or its base not made, for this reason.
    Skipped(String),
}

/// Make neighbor `n`'s base and put it on `shelf`, as an open of it
/// would make it under the edit it carries, unless it is not worth a
/// place: an edit asking for the learned denoiser (whose model runs for
/// seconds and cannot stop part way, and whose pair would be a
/// gigabyte on the shelf), a picture's file (whose base is its decode),
/// a frame whose base is the one in hand (`in_hand`, the open file and
/// what its walks kept), or one already on the shelf. The base is a
/// walk of the frame's list to it ([`Purpose::Prefetch`]); `ahead` is
/// asked before each stage what has been asked of the worker since,
/// and on anything but the open of this frame the prefetch stops there
/// and nothing is kept. The CA and the lens correction run on `gpu`
/// when there is one, which holds nothing of the base after; a device
/// that declines the picture leaves this base to the CPU, and any other
/// device error lets the context go for the session, as a develop's
/// does. No base is made, or put, whose place on the shelf would push
/// out a base of a file in `keep` (the list's own and the frame just
/// left): the neighbor's size is known from its decode, and may be
/// larger than the open frame's the depth was counted in.
pub(super) fn prefetch(
    n: &Neighbor,
    in_hand: Option<(&Path, &FrameKept)>,
    keep: &[PathBuf],
    shelf: &mut Shelf,
    gpu: &mut Option<greycard_gpu::Context>,
    lenses: Option<&greycard_lens::Database>,
    ahead: &dyn Fn(&Facts) -> Ahead,
) -> Fetched {
    let name = file_label(&n.path);
    let side = n.place();
    let side = side.as_str();
    let turn = n.turn % 4;
    let skipped = |why: String| {
        note(format!(
            "the prefetch of the {side} frame, {name}, skipped: {why}"
        ));
        Fetched::Skipped(why)
    };
    if n.edit.noise.tier().is_some() {
        return skipped("its edit asks for the learned denoiser".into());
    }
    if is_picture_path(&n.path) {
        return skipped("it is not a raw".into());
    }
    let Some(key) = FileKey::of(&n.path) else {
        return skipped("the file cannot be read".into());
    };
    if let Some((path, b)) = in_hand
        && path == n.path
        && b.serves(&n.edit, turn, lenses, gpu.is_some(), false)
    {
        return skipped("its base is the open frame's".into());
    }
    if shelf.holds(
        (&n.path, &key.stat),
        &n.edit,
        n.seed_blend,
        turn,
        gpu.is_some(),
        lenses,
    ) {
        return skipped("its base is on the shelf".into());
    }
    let started = Instant::now();
    let mut stages_ran = Stages::new();
    let input = match timed(&mut stages_ran, "decode", || super::decode::open(&n.path)) {
        Ok((input, _)) => input,
        Err(e) => return skipped(format!("its decode failed: {e:#}")),
    };
    if !input.is_raw() {
        return skipped("it is not a raw".into());
    }
    let walking = input.walked();
    let facts = walking.facts(turn);
    if let Some(out) = shelf.pushes_out_before(&key, base_estimate(&facts), keep) {
        return skipped(format!(
            "its base would push the base of {} off the shelf",
            file_label(&out)
        ));
    }
    note(format!("prefetching the {side} frame, {name}"));
    // Asked before each stage: whether to stop there, said once when
    // the open of this very frame is what came.
    struct Asked<'a> {
        path: &'a Path,
        name: &'a str,
        facts: &'a Facts,
        ahead: &'a dyn Fn(&Facts) -> Ahead,
        told: Cell<bool>,
    }
    impl Hold for Asked<'_> {
        fn give_way(&self, next: &Stage) -> bool {
            let stage = next.name();
            at_check(self.path, stage);
            match (self.ahead)(self.facts) {
                Ahead::Nothing => false,
                Ahead::ItsOpen => {
                    if !self.told.replace(true) {
                        note(format!(
                            "the prefetch of {} goes on before its {stage}: \
                             it is the frame being opened",
                            self.name
                        ));
                    }
                    false
                }
                Ahead::Work => true,
            }
        }

        // Inside the highlight fill, the same ask, said nowhere.
        fn give_way_inside(&self, _stage: &Stage) -> bool {
            matches!((self.ahead)(self.facts), Ahead::Work)
        }

        // A neighbor nobody waits on does not cost the session its
        // device over its own failure.
        fn keeps_device(&self) -> bool {
            true
        }
    }
    let asked = Asked {
        path: &n.path,
        name: &name,
        facts: &facts,
        ahead,
        told: Cell::new(false),
    };
    let edit = seeded(&n.edit, n.seed_blend, Some(&input));
    let purpose = Purpose::Prefetch;
    let steps = stages(&edit, &purpose, &facts, lenses);
    let places = plan(&steps, &purpose, gpu.is_some());
    let mut kept = Kept::new();
    let walked = match walk(
        &steps,
        &places,
        &walking,
        &mut kept,
        &mut NoModels,
        gpu,
        &asked,
    ) {
        Ok(walked) => walked,
        Err(Stopped::GaveWay(stage)) => {
            note(format!(
                "the prefetch of the {side} frame, {name}, abandoned before its {stage}: \
                 the worker was asked for something"
            ));
            return Fetched::Abandoned(stage, Box::new(facts));
        }
        Err(Stopped::Failed(e)) => return skipped(format!("its base failed: {e}")),
    };
    let ca = steps
        .steps
        .iter()
        .zip(&walked.places)
        .find(|(s, _)| matches!(s, Step::Op(Stage::Ca(_))))
        .map(|(_, place)| {
            let ran = if *place == Place::Gpu {
                CaRan::Gpu
            } else {
                CaRan::Cpu
            };
            let s = walked
                .seconds
                .iter()
                .filter(|(n, _)| *n == "CA")
                .map(|(_, s)| s)
                .sum::<f64>();
            (ran, s)
        });
    stages_ran.extend(walked.seconds.iter().filter(|(n, _)| *n != "CA").copied());
    let mut held = FrameKept::new(&facts);
    held.on_gpu = steps.steps.iter().zip(&walked.places).any(|(s, p)| {
        *p == Place::Gpu
            && matches!(
                s,
                Step::Op(Stage::Ca(_) | Stage::Demosaic(..) | Stage::Lens(_))
            )
    });
    held.kept = kept;
    held.edit = edit;
    held.turn = turn;
    held.stamp = 1;
    held.meta = walked.meta;
    if let Some(image) = held.kept.picture(crate::stages::Keep::Base) {
        held.image = image.clone();
    }
    let key = key.decoded(&input);
    drop(input);
    drop(walking);
    // As the shelf will hold it, its working copies let go.
    held.kept.shelve();
    if let Some(out) = shelf.pushes_out_of(&key, &held, keep) {
        return skipped(format!(
            "its base would push the base of {} off the shelf",
            file_label(&out)
        ));
    }
    // A prefetch makes no learned frame's base, so none stands in.
    if !shelf.put(key, held, false) {
        return skipped("its base is over the shelf's whole budget".into());
    }
    let gone = shelf.fit();
    note(prefetched_line(
        &name,
        side,
        ca,
        &stages_ran,
        started.elapsed().as_secs_f64(),
        &shelf.said(),
        gone,
    ));
    Fetched::Landed
}

/// The bytes the base of a frame of `facts` will take on the shelf: its
/// picture at the size the develop crops the sensor to, in floats.
pub(super) fn base_estimate(facts: &Facts) -> u64 {
    match &facts.input {
        crate::stages::InputKind::Raw { crop, .. } => {
            let planes = greycard_core::WorkingImage::CHANNELS;
            (crop.0 * crop.1 * planes * std::mem::size_of::<f32>()) as u64
        }
        _ => 0,
    }
}

/// The line for a prefetch landed, on the develop line's shape: the
/// frame, then its CA and each stage with its seconds, the total, and
/// the shelf as it now stands.
fn prefetched_line(
    name: &str,
    side: &str,
    ca: Option<(CaRan, f64)>,
    stages: &Stages,
    total: f64,
    shelf: &str,
    gone: usize,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut stages = stages.iter();
    // The decode first, as it ran, then the CA, which ran inside the
    // prepare after it.
    if let Some((name, s)) = stages.next() {
        parts.push(format!("{name} {s:.2} s"));
    }
    match ca {
        Some((CaRan::Gpu, s)) => parts.push(format!("CA on the GPU {s:.2} s")),
        Some((CaRan::Cpu, s)) => parts.push(format!("CA {s:.2} s")),
        None => {}
    }
    for (name, s) in stages {
        parts.push(format!("{name} {s:.2} s"));
    }
    format!(
        "prefetched {name}, the {side} frame: {}, total {total:.2} s; the shelf: {shelf}{}",
        parts.join(", "),
        if gone > 0 {
            format!(", {gone} let go")
        } else {
            String::new()
        }
    )
}

/// A line for the log; in the tests, also kept for them to read when it
/// names what one watches ([`tap`]).
pub(super) fn note(line: String) {
    tracing::info!("{line}");
    tap(&line);
}

/// In the tests, `line` kept when it names something a test watches
/// ([`watch`]), so a test can read what the worker's thread logged; a
/// line nothing watches is not kept. Nothing outside the tests.
pub(super) fn tap(line: &str) {
    #[cfg(test)]
    {
        let watched = WATCHED.lock().expect("the tap");
        if watched.iter().any(|w| line.contains(w.as_str())) {
            TAP.lock().expect("the tap").push(line.to_string());
        }
    }
    #[cfg(not(test))]
    let _ = line;
}

#[cfg(test)]
static TAP: Mutex<Vec<String>> = Mutex::new(Vec::new());
#[cfg(test)]
static WATCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Keep the lines that name `name` from now on, for [`said`].
#[cfg(test)]
pub(super) fn watch(name: &str) {
    WATCHED.lock().expect("the tap").push(name.to_string());
}

/// What a test has a prefetch of a file do at one of its checks between
/// stages ([`at_next_check`]).
#[cfg(test)]
pub(super) enum AtCheck {
    /// Say the stage on the first channel and wait for a word on the
    /// second before asking the queue.
    Hold(
        std::sync::mpsc::Sender<&'static str>,
        std::sync::mpsc::Receiver<()>,
    ),
    /// Panic there, as a bug in the prefetch would.
    Panic,
}

#[cfg(test)]
static CHECKS: Mutex<Vec<(PathBuf, AtCheck)>> = Mutex::new(Vec::new());

/// At the next check between the stages of a prefetch of `path`, do
/// `what`, once.
#[cfg(test)]
pub(super) fn at_next_check(path: &Path, what: AtCheck) {
    CHECKS
        .lock()
        .expect("the checks")
        .push((path.to_path_buf(), what));
}

/// A check between stages, as a test asked for it ([`at_next_check`]).
fn at_check(path: &Path, stage: &'static str) {
    #[cfg(test)]
    {
        let what = {
            let mut checks = CHECKS.lock().expect("the checks");
            checks
                .iter()
                .position(|(p, _)| p == path)
                .map(|at| checks.remove(at).1)
        };
        match what {
            Some(AtCheck::Hold(said, go)) => {
                let _ = said.send(stage);
                let _ = go.recv();
            }
            Some(AtCheck::Panic) => panic!("a prefetch of {} told to panic", path.display()),
            None => {}
        }
    }
    #[cfg(not(test))]
    let _ = (path, stage);
}

/// The lines [`note`] has written that name `name`, which a test
/// watches.
#[cfg(test)]
pub(super) fn said(name: &str) -> Vec<String> {
    TAP.lock()
        .expect("the tap")
        .iter()
        .filter(|l| l.contains(name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::shelf::Budget;
    use super::*;

    /// The window's keep of a raw of `width` by `height` under the
    /// default edit.
    fn base(width: usize, height: usize) -> FrameKept {
        FrameKept::walked(
            &Arc::new(super::super::tests::aberrated_frame(width, height)),
            &Edit::default(),
            0,
        )
    }

    /// What the list reads of the raw [`base`] makes.
    fn facts() -> Facts {
        crate::walk::Input::Raw(Arc::new(super::super::tests::aberrated_frame(40, 30))).facts(0)
    }

    fn neighbor(path: &str, side: Side) -> Neighbor {
        Neighbor {
            path: path.into(),
            edit: Edit::default(),
            turn: 0,
            seed_blend: false,
            side,
            steps: 1,
            read_first: None,
        }
    }

    /// The worker idle with a frame open in the editor.
    fn now<'a>(
        landed: Option<u64>,
        open: Option<&'a FrameKept>,
        waiting: bool,
        shelf: Option<&'a Shelf>,
        idle: Duration,
    ) -> Idle<'a> {
        Idle {
            landed,
            open,
            waiting,
            shelf,
            idle,
            frame_open: true,
            device_lost: false,
        }
    }

    /// While a lost device waits to be taken up no prefetch begins, and
    /// the list waits with it.
    #[test]
    fn no_prefetch_begins_while_a_lost_device_waits() {
        let open = base(40, 30);
        let shelf = Shelf::new(Budget::Bytes(size(40, 30) * 5));
        let mut queued = both(3);
        let lost = Idle {
            device_lost: true,
            ..now(Some(3), Some(&open), false, Some(&shelf), QUIET * 2)
        };
        assert!(matches!(due(&mut queued, &mut None, &lost), Due::Nothing));
        assert_eq!(queued.as_ref().map(|q| q.waiting.len()), Some(2), "kept");
        assert!(matches!(
            due(
                &mut queued,
                &mut None,
                &now(Some(3), Some(&open), false, Some(&shelf), QUIET * 2)
            ),
            Due::Fetch(_)
        ));
    }

    /// The sidecars that did not serve are kept to the latest 64, one a
    /// line of a file, the oldest let go first.
    #[test]
    fn the_unread_sidecars_are_kept_to_the_latest_64() {
        let key = |i: usize, version: Option<&str>| UnreadKey {
            path: PathBuf::from(format!("/nowhere/{i}.CR3")),
            version: version.map(Arc::from),
            sidecar: None,
        };
        let mut peeks = Peeks::default();
        for i in 0..=UNREAD_KEPT {
            peeks.put(key(i, None));
        }
        assert_eq!(peeks.unread.len(), UNREAD_KEPT);
        assert!(!peeks.holds(&key(0, None)), "the oldest let go");
        assert!(peeks.holds(&key(1, None)));
        assert!(peeks.holds(&key(UNREAD_KEPT, None)));
        // The same line again replaces its own; another version is its own.
        peeks.put(key(5, None));
        peeks.put(key(5, Some("v")));
        assert_eq!(peeks.unread.len(), UNREAD_KEPT);
        assert!(peeks.holds(&key(5, Some("v"))));
        assert!(peeks.holds(&key(5, None)));
    }

    /// The window's check is asked every time, its answer kept for
    /// nothing: an automount that mounts its share in between is seen at
    /// the next ask. Said once a folder, and again once the check is
    /// handed over anew.
    #[test]
    fn a_folder_s_mount_is_asked_every_time_and_said_once() {
        thread_local! {
            static MOUNTED: Cell<bool> = const { Cell::new(false) };
            static ASKED: Cell<usize> = const { Cell::new(0) };
        }
        fn check(folder: &Path) -> Option<String> {
            ASKED.with(|a| a.set(a.get() + 1));
            folder.starts_with("/mnt/archive").then(|| {
                if MOUNTED.with(Cell::get) {
                    "a network mount (nfs4)".to_string()
                } else {
                    "an automount not mounted yet".to_string()
                }
            })
        }
        let mut peeks = Peeks::default();
        let at = |p: &str| Path::new(p).to_path_buf();
        assert_eq!(
            peeks.not_to_peek(&at("/mnt/archive/a/1.CR3")),
            None,
            "no check"
        );
        peeks.set_remote(check);
        assert_eq!(
            peeks.not_to_peek(&at("/mnt/archive/a/1.CR3")),
            Some(("an automount not mounted yet".into(), true))
        );
        MOUNTED.with(|m| m.set(true));
        assert_eq!(
            peeks.not_to_peek(&at("/mnt/archive/a/2.CR3")),
            Some(("a network mount (nfs4)".into(), false)),
            "seen mounted at once, said already"
        );
        assert_eq!(peeks.not_to_peek(&at("/home/p/1.CR3")), None);
        assert_eq!(peeks.not_to_peek(&at("/home/p/2.CR3")), None);
        assert_eq!(ASKED.with(Cell::get), 4, "every time");
        peeks.set_remote(check);
        assert_eq!(
            peeks.not_to_peek(&at("/mnt/archive/a/1.CR3")),
            Some(("a network mount (nfs4)".into(), true)),
            "said again"
        );
    }

    fn both(generation: u64) -> Option<Neighbors> {
        Some(Neighbors::new(
            generation,
            Heading::steady(Side::Next),
            vec![
                neighbor("b.CR3", Side::Next),
                neighbor("z.CR3", Side::Previous),
            ],
        ))
    }

    /// The bytes of a base of `width` by `height`, as the shelf counts
    /// the open frame's.
    fn size(width: usize, height: usize) -> u64 {
        base(width, height).picture_bytes()
    }

    /// Only once the open's develop has landed, no save waits on a
    /// develop, and the window has been quiet: the next, then the one
    /// before, then nothing. An open still to come keeps them; one
    /// since replaced lets them go.
    #[test]
    fn the_neighbors_are_due_only_when_the_worker_is_idle() {
        let open = base(40, 30);
        let shelf = Shelf::new(Budget::Bytes(size(40, 30) * 5));
        let idle = QUIET * 2;

        let mut queued = both(3);
        // No develop landed yet, or one of an older open.
        for landed in [None, Some(2)] {
            assert!(matches!(
                due(
                    &mut queued,
                    &mut None,
                    &now(landed, Some(&open), false, Some(&shelf), idle)
                ),
                Due::Nothing
            ));
            assert!(queued.is_some(), "kept for their open");
        }
        // A save waiting on a develop, or no base in hand.
        assert!(matches!(
            due(
                &mut queued,
                &mut None,
                &now(Some(3), Some(&open), true, Some(&shelf), idle)
            ),
            Due::Nothing
        ));
        assert!(matches!(
            due(
                &mut queued,
                &mut None,
                &now(Some(3), None, false, Some(&shelf), idle)
            ),
            Due::Nothing
        ));
        // Not quiet long enough: look again when it will have been.
        match due(
            &mut queued,
            &mut None,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET / 3),
        ) {
            Due::Wait(d) => assert!(d > Duration::ZERO && d <= QUIET),
            other => panic!("{other:?}"),
        }
        let mut order = Vec::new();
        while let Due::Fetch(n) = due(
            &mut queued,
            &mut None,
            &now(Some(3), Some(&open), false, Some(&shelf), idle),
        ) {
            order.push((n.path, n.side));
        }
        assert_eq!(
            order,
            vec![
                (PathBuf::from("b.CR3"), Side::Next),
                (PathBuf::from("z.CR3"), Side::Previous)
            ]
        );
        assert!(queued.is_none(), "done with");

        // One that gave way waits twice as long for the next quiet,
        // each time, to eight times as long.
        let mut queued = both(3);
        let Some(Due::Fetch(n)) = Some(due(
            &mut queued,
            &mut None,
            &now(Some(3), Some(&open), false, Some(&shelf), idle),
        )) else {
            panic!("due");
        };
        let round = queued.as_ref().unwrap().round;
        queued.as_mut().unwrap().gave_way(*n, round);
        assert!(matches!(
            due(
                &mut queued,
                &mut None,
                &now(Some(3), Some(&open), false, Some(&shelf), QUIET * 3 / 2)
            ),
            Due::Wait(_)
        ));
        match due(
            &mut queued,
            &mut None,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET * 2),
        ) {
            Due::Fetch(n) => assert_eq!(n.side, Side::Next, "begun again first"),
            other => panic!("{other:?}"),
        }
        for _ in 0..5 {
            let n = neighbor("b.CR3", Side::Next);
            let round = queued.as_ref().unwrap().round;
            queued.as_mut().unwrap().gave_way(n, round);
        }
        assert_eq!(queued.as_ref().unwrap().quiet, QUIET * 8);

        // One of a list since replaced is not put on the new one.
        let stale = queued.as_ref().unwrap().round;
        let mut queued = both(3);
        let before = queued.as_ref().unwrap().waiting.len();
        queued
            .as_mut()
            .unwrap()
            .gave_way(neighbor("old.CR3", Side::Next), stale);
        assert_eq!(queued.as_ref().unwrap().waiting.len(), before);
        assert_eq!(queued.as_ref().unwrap().quiet, QUIET);

        // No frame open in the editor (culling, the grid): let go.
        let mut queued = both(3);
        let away = Idle {
            frame_open: false,
            ..now(Some(3), Some(&open), false, Some(&shelf), idle)
        };
        assert!(matches!(due(&mut queued, &mut None, &away), Due::Nothing));
        assert!(queued.is_none(), "let go with the editor");

        // A later open's develop landed: these were the last frame's.
        let mut queued = both(3);
        assert!(matches!(
            due(
                &mut queued,
                &mut None,
                &now(Some(4), Some(&open), false, Some(&shelf), idle)
            ),
            Due::Nothing
        ));
        assert!(queued.is_none());
    }

    /// A prefetch that stood down for the reduced develop in hand keeps
    /// its reservation only while it is still due: an open that then
    /// fails (nothing landed), or a quiet still to come, lets it go and
    /// wakes the pool; a fetch about to claim it again keeps it.
    #[test]
    fn a_reservation_is_let_go_unless_a_prefetch_is_due() {
        let edited = crate::edited_pictures::Edited::new(Arc::new(Mutex::new(None)));
        let open = base(40, 30);
        let shelf = Shelf::new(Budget::Bytes(size(40, 30) * 5));
        let woken = Cell::new(0);
        let wake = || woken.set(woken.get() + 1);
        let stand_down = || {
            assert!(!claim(|| edited.set_prefetching(true), || 1));
            assert!(edited.prefetching(), "stood down, reserved");
        };

        // Stood down, then the open fails: nothing landed.
        stand_down();
        let mut queued = both(3);
        let due_now = due(
            &mut queued,
            &mut None,
            &now(None, Some(&open), false, Some(&shelf), QUIET * 2),
        );
        settle_reservation(&due_now, &edited, wake);
        assert!(!edited.prefetching(), "let go after a failed open");
        assert_eq!(woken.get(), 1, "the pool woken");

        // Stood down, then a quiet still to come.
        stand_down();
        let due_now = due(
            &mut queued,
            &mut None,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET / 2),
        );
        assert!(matches!(due_now, Due::Wait(_)));
        settle_reservation(&due_now, &edited, wake);
        assert!(!edited.prefetching(), "let go while it waits");

        // Stood down, then due again: kept for the claim.
        stand_down();
        let due_now = due(
            &mut queued,
            &mut None,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET * 2),
        );
        assert!(matches!(due_now, Due::Fetch(_)));
        settle_reservation(&due_now, &edited, wake);
        assert!(edited.prefetching(), "kept for the prefetch");
        assert_eq!(woken.get(), 2);
    }

    /// The frame left is the last other file opened: A, then B, then a
    /// version of B, and A is still the one left, kept by the list.
    #[test]
    fn the_frame_left_is_the_last_other_file_opened() {
        let (mut open, mut left) = (None, None);
        opened(&mut open, &mut left, "A.CR3".into());
        assert_eq!(left, None);
        opened(&mut open, &mut left, "B.CR3".into());
        assert_eq!(left, Some(PathBuf::from("A.CR3")));
        opened(&mut open, &mut left, "B.CR3".into());
        assert_eq!(left, Some(PathBuf::from("A.CR3")), "a version of B");
        assert_eq!(open, Some(PathBuf::from("B.CR3")));
        let mut list = Neighbors::new(
            1,
            Heading::steady(Side::Next),
            vec![neighbor("C.CR3", Side::Next)],
        );
        list.trim(DEEPEST);
        assert_eq!(
            keep(&list, left.as_deref()),
            vec![PathBuf::from("C.CR3"), PathBuf::from("A.CR3")]
        );
        opened(&mut open, &mut left, "C.CR3".into());
        assert_eq!(left, Some(PathBuf::from("B.CR3")));
    }

    /// The depth is the shelf's room beside the open frame: never more
    /// neighbors than it holds, one behind from room for two, the rest
    /// ahead to three.
    #[test]
    fn the_depth_is_what_the_shelf_has_room_for() {
        let d = |ahead, behind| Depth { ahead, behind };
        let steady = Heading::steady(Side::Next);
        assert_eq!(depth(0, steady), d(0, 0));
        assert_eq!(depth(1, steady), d(1, 0));
        assert_eq!(depth(2, steady), d(1, 1));
        assert_eq!(depth(3, steady), d(2, 1));
        assert_eq!(depth(4, steady), d(3, 1));
        assert_eq!(depth(7, steady), DEEPEST);
        assert_eq!(depth(u64::MAX, steady), DEEPEST);
        for room in 0..12 {
            let got = depth(room, steady);
            assert!(u64::from(got.ahead + got.behind) <= room, "{room}");
        }
    }

    /// At 45 MP (a base of 515 MB as the shelf counts it) an 8 GB
    /// machine makes the next frame only, and 16 and 32 GB three ahead
    /// and one behind.
    #[test]
    fn the_depth_per_machine() {
        const G: u64 = 1 << 30;
        let at_45_mp = 515 << 20;
        let steady = Heading::steady(Side::Next);
        let of = |ram: u64| depth(Shelf::new(Budget::of(ram)).room_for(at_45_mp), steady);
        assert_eq!(
            of(8 * G),
            Depth {
                ahead: 1,
                behind: 0
            }
        );
        assert_eq!(of(16 * G), DEEPEST);
        assert_eq!(of(32 * G), DEEPEST);
        assert_eq!(
            depth(Shelf::new(Budget::One).room_for(at_45_mp), steady).ahead,
            1
        );
        assert_eq!(
            depth(Shelf::new(Budget::Bytes(0)).room_for(at_45_mp), steady).ahead,
            0
        );
    }

    /// With room for one base, the next is made only after two steps
    /// ahead in a row: one step, a jump, or a step back makes nothing,
    /// so a compare of two frames keeps both. More room does not wait.
    #[test]
    fn room_for_one_waits_for_a_streak() {
        let at = |streak| Heading {
            ahead: Side::Next,
            streak,
        };
        let none = Depth {
            ahead: 0,
            behind: 0,
        };
        assert_eq!(depth(1, at(0)), none);
        assert_eq!(depth(1, at(1)), none);
        assert_eq!(depth(1, at(2)).ahead, 1);
        assert_eq!(depth(1, at(9)).ahead, 1);
        assert_eq!(depth(2, at(0)), depth(2, at(5)));
        assert_eq!(depth(5, at(0)), DEEPEST);
    }

    /// The four a window sends walking right, made in its order and
    /// trimmed to the depth: with room for one only the next; for two
    /// the next and the one behind; for three the second next as well;
    /// no room, none. What was last said of the depth and the walk is
    /// kept for the next list.
    #[test]
    fn the_neighbors_are_made_to_the_depth_in_the_window_s_order() {
        let open = base(40, 30);
        let idle = QUIET * 2;
        let deep = |generation, walking| {
            let n = |p: &str, side, steps| Neighbor {
                steps,
                ..neighbor(p, side)
            };
            Some(Neighbors::new(
                generation,
                Heading::steady(walking),
                vec![
                    n("n1.CR3", Side::Next, 1),
                    n("p1.CR3", Side::Previous, 1),
                    n("n2.CR3", Side::Next, 2),
                    n("n3.CR3", Side::Next, 3),
                ],
            ))
        };
        let mut said_now = None;
        for (budget, made) in [
            (Budget::Bytes(0), vec![]),
            (Budget::One, vec!["n1"]),
            (Budget::Bytes(size(40, 30) * 2 - 1), vec!["n1"]),
            (Budget::Bytes(size(40, 30) * 2), vec!["n1", "p1"]),
            (Budget::Bytes(size(40, 30) * 3), vec!["n1", "p1", "n2"]),
            (
                Budget::Bytes(size(40, 30) * 9),
                vec!["n1", "p1", "n2", "n3"],
            ),
        ] {
            let shelf = Shelf::new(budget);
            let mut queued = deep(1, Side::Next);
            let mut order = Vec::new();
            while let Due::Fetch(n) = due(
                &mut queued,
                &mut said_now,
                &now(Some(1), Some(&open), false, Some(&shelf), idle),
            ) {
                order.push(
                    n.path
                        .to_string_lossy()
                        .trim_end_matches(".CR3")
                        .to_string(),
                );
            }
            assert_eq!(order, made, "{budget:?}");
            assert!(queued.is_none(), "{budget:?}");
        }
        // Walking left, the side ahead is the previous: the one next is
        // behind, and beyond one behind is let go.
        let shelf = Shelf::new(Budget::Bytes(size(40, 30) * 9));
        let mut queued = deep(1, Side::Previous);
        let mut order = Vec::new();
        while let Due::Fetch(n) = due(
            &mut queued,
            &mut said_now,
            &now(Some(1), Some(&open), false, Some(&shelf), idle),
        ) {
            order.push(n.place());
        }
        assert_eq!(order, vec!["next", "previous"]);
        assert_eq!(said_now, Some((DEEPEST, Side::Previous)));
        assert_eq!(
            depth_line(DEEPEST, Side::Next),
            "prefetch: 3 ahead, 1 behind, walking right"
        );
        assert_eq!(
            depth_line(
                Depth {
                    ahead: 1,
                    behind: 0
                },
                Side::Previous
            ),
            "prefetch: 1 ahead, 0 behind, walking left"
        );
    }

    /// Anything asked for is something a prefetch gives way to, but
    /// the open of the frame it is making and the saves of the frame
    /// being left; the masks of frames not open wait while one is.
    #[test]
    fn a_prefetch_gives_way_to_any_request_but_its_own_frame_s_open() {
        let n = neighbor("b.CR3", Side::Next);
        let open = |path: &str, edit: Edit| Job::Open {
            path: path.into(),
            version: None,
            edit,
            generation: 9,
            seed_blend: false,
            turn: 0,
            stance_known: true,
            learned: Vec::new(),
        };
        let keep = || Job::Keep {
            frame: Frame::file("a.CR3"),
            edit: Edit::default(),
            turn: 0,
            iso: None,
        };
        let mut q = Queue::default();
        assert_eq!(ahead(&q, true, &n, &facts(), None), Ahead::Nothing);
        q.masks_for.push_back(Frame::file("c.CR3"));
        assert_eq!(
            ahead(&q, true, &n, &facts(), None),
            Ahead::Nothing,
            "waits behind the open frame"
        );
        assert_eq!(ahead(&q, false, &n, &facts(), None), Ahead::Work);
        q.masks_for.clear();

        q.develop = Some(open("b.CR3", Edit::default()));
        assert_eq!(ahead(&q, true, &n, &facts(), None), Ahead::ItsOpen);
        q.keeps.push_back(keep());
        assert_eq!(
            ahead(&q, true, &n, &facts(), None),
            Ahead::ItsOpen,
            "the frame left's save"
        );
        q.keeps.clear();
        q.develop = Some(open("c.CR3", Edit::default()));
        assert_eq!(
            ahead(&q, true, &n, &facts(), None),
            Ahead::Work,
            "another frame's open"
        );
        let warmer = Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature: 6500.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        q.develop = Some(open("b.CR3", warmer));
        assert_eq!(
            ahead(&q, true, &n, &facts(), None),
            Ahead::Work,
            "under another base"
        );
        q.develop = Some(Job::Develop {
            edit: Edit::default(),
            generation: 9,
            turn: 0,
            of: None,
        });
        assert_eq!(
            ahead(&q, true, &n, &facts(), None),
            Ahead::Work,
            "a slider's develop"
        );
        q.develop = None;

        q.keeps.push_back(keep());
        assert_eq!(ahead(&q, true, &n, &facts(), None), Ahead::Work, "a save");
        q.keeps.clear();
        q.exports.push_back(Job::Landscape);
        assert_eq!(
            ahead(&q, true, &n, &facts(), None),
            Ahead::Work,
            "an export"
        );
        q.exports.clear();
        q.people = true;
        assert_eq!(ahead(&q, true, &n, &facts(), None), Ahead::Work);
        q.people = false;
        assert_eq!(
            ahead(&q, false, &n, &facts(), None),
            Ahead::Work,
            "the editor left"
        );
        q.stopping = true;
        assert_eq!(ahead(&q, true, &n, &facts(), None), Ahead::Work, "leaving");
    }

    /// The landed line names the frame, its CA after its decode, each
    /// stage with its seconds, the total, and the shelf.
    #[test]
    fn the_prefetch_s_line_times_its_stages() {
        let stages: Stages = vec![
            ("decode", 0.25),
            ("prepare", 0.19),
            ("demosaic", 1.14),
            ("finish", 0.07),
            ("lens", 0.7),
            ("guide", 0.02),
        ];
        assert_eq!(
            prefetched_line(
                "B.CR3",
                "next",
                Some((CaRan::Gpu, 0.1)),
                &stages,
                2.47,
                "2 base(s), 1030 MB of 4096 MB",
                0
            ),
            "prefetched B.CR3, the next frame: decode 0.25 s, CA on the GPU 0.10 s, \
             prepare 0.19 s, demosaic 1.14 s, finish 0.07 s, lens 0.70 s, guide 0.02 s, \
             total 2.47 s; the shelf: 2 base(s), 1030 MB of 4096 MB"
        );
        assert!(
            prefetched_line("C.CR3", "previous", None, &stages, 2.0, "x", 1)
                .ends_with(", 1 let go")
        );
    }
}
