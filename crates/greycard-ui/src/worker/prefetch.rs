//! The neighbors' bases, made while the worker has nothing else to do
//! and put on the shelf, so a step to the next frame or the one before
//! finds its base made.
//!
//! The window says, with each open, which frames its arrow keys would
//! reach from the frame opened: the next in the strip's order, then
//! the one before, under the filter and the sort the strip shows, each
//! with the edit, the turn, and the seed an open of it would send
//! ([`Neighbor`]). Once that open's develop has landed, the frame is
//! still open in the editor, nothing is queued, no save waits on a
//! develop, no reduced develop is in hand, and the window has been
//! quiet for [`QUIET`], the worker decodes one neighbor and makes its
//! base as an open would, and shelves it as a base left behind is shelved
//! ([`Shelf::put`]): same key, same budget, the least recently left
//! going first. One neighbor at a time, so no more than one frame is
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

use super::develop::{
    CaRan, CaRun, base_settings, ca_out_of_prepare, guide_step, lens_step, resolved_profile,
};
use super::engine::file_label;
use super::shelf::{FileKey, Shelf, bytes_of};
use super::*;

/// Which way from the open frame a neighbor is, in the strip's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Next,
    Previous,
}

impl Side {
    /// The side in words, for the log.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Side::Next => "next",
            Side::Previous => "previous",
        }
    }
}

/// A frame beside the open one, as an open of it would be asked for:
/// its file, the edit its line holds, its quarter turns, and whether
/// its learned blend is still to be seeded from its ISO.
#[derive(Clone, Debug)]
pub(crate) struct Neighbor {
    pub path: PathBuf,
    pub edit: Edit,
    pub turn: u8,
    pub seed_blend: bool,
    pub side: Side,
}

/// The neighbors of the frame opened at `generation` still to be made,
/// the next first, and how long the window must be quiet before the
/// next of them begins: [`QUIET`], longer each time one gives way.
pub(super) struct Neighbors {
    pub generation: u64,
    pub waiting: VecDeque<Neighbor>,
    pub quiet: Duration,
}

impl Neighbors {
    /// The neighbors of the frame opened at `generation`.
    pub(super) fn new(generation: u64, waiting: Vec<Neighbor>) -> Self {
        Self {
            generation,
            waiting: waiting.into(),
            quiet: QUIET,
        }
    }

    /// One of them gave way to a request and is to be begun again: the
    /// quiet asked of the window before it doubles, to eight times
    /// [`QUIET`]. Someone at work on the open frame, a slider moved
    /// after each look, would otherwise meet a stage to wait out at
    /// each move, and the work thrown away each time; the next open
    /// starts from [`QUIET`] again.
    pub(super) fn gave_way(&mut self, n: Neighbor) {
        self.waiting.push_front(n);
        self.quiet = (self.quiet * 2).min(QUIET * 8);
    }
}

/// How long the window is left alone (no develop asked for, none
/// landing) before a prefetch begins: the reduced develop's own wait
/// while a frame is open, and for its reason. A slider paused for less
/// is not met by a decode that cannot stop, nor its next move by a
/// stage to wait out and work thrown away.
#[cfg(not(test))]
pub(super) const QUIET: Duration = crate::reduced::IDLE;
#[cfg(test)]
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
    /// The base in hand.
    pub open: Option<&'a Base>,
    /// A save of the open frame waits on a develop still to come.
    pub waiting: bool,
    pub shelf: Option<&'a Shelf>,
    /// How long since the window last asked for a develop or one ended.
    pub idle: Duration,
    /// A frame is open in the editor: not culling, the grid, or none.
    pub frame_open: bool,
}

/// What the idle worker does about the neighbors waiting in `queued`,
/// asked only when the queue holds nothing else for it (no develop, no
/// open, no export, no save, no mask). Neighbors of an open since
/// replaced are let go, as are all of them once no frame is open in the
/// editor; the shelf too small for two bases the size of the open
/// frame's lets them all go, said once, since a prefetch would push out
/// the frame just left.
pub(super) fn due(queued: &mut Option<Neighbors>, now: &Idle<'_>) -> Due {
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
    let room = shelf.room_for(bytes_of(open, None));
    if room < 2 {
        for n in &neighbors.waiting {
            note(format!(
                "the prefetch of the {} frame, {}, skipped: the shelf holds {room} base(s) \
                 the size of the open frame's, fewer than two",
                n.side.word(),
                file_label(&n.path)
            ));
        }
        *queued = None;
        return Due::Nothing;
    }
    match neighbors.waiting.pop_front() {
        Some(n) => Due::Fetch(Box::new(n)),
        None => Due::Nothing,
    }
}

/// Reserve the machine against the reduced develops for a prefetch:
/// `reserve` raises the flag that holds the pool's next one back, and
/// only then is the pool asked how many it has `in_hand`, so one begun
/// meanwhile is seen. True when none is, and the prefetch may begin;
/// either way the flag stays up, so a prefetch that waits for the one
/// in hand is the next to run and not behind every one still queued.
pub(crate) fn claim(reserve: impl FnOnce(), in_hand: impl FnOnce() -> usize) -> bool {
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
pub(super) fn settle_reservation(due: &Due, edited: &crate::edited::Edited, wake: impl FnOnce()) {
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
pub(crate) fn behind_prefetch(
    prefetching: bool,
    otherwise: impl FnOnce() -> (usize, Option<Duration>),
) -> (usize, Option<Duration>) {
    if prefetching {
        (0, Some(BEHIND_REDUCED))
    } else {
        otherwise()
    }
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
pub(super) fn ahead(q: &Queue, frame_open: bool, n: &Neighbor) -> Ahead {
    let others = !frame_open
        || q.stopping
        || q.lenses.is_some()
        || q.gpu.is_some()
        || q.people
        || q.landscape
        || !q.masks.is_empty()
        || !q.exports.is_empty()
        || !q.people_on.is_empty();
    if others {
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
            && edit.same_base(&n.edit) =>
        {
            Ahead::ItsOpen
        }
        Some(_) => Ahead::Work,
    }
}

/// How a prefetch ended.
#[derive(Debug, PartialEq)]
pub(super) enum Fetched {
    /// Its base is on the shelf.
    Landed,
    /// It gave way before this stage.
    Abandoned(&'static str),
    /// Not begun, or its base not made, for this reason.
    Skipped(String),
}

/// Make neighbor `n`'s base and put it on `shelf`, as an open of it
/// would make it under the edit it carries, unless it is not worth a
/// place: an edit asking for the learned denoiser (whose model runs for
/// seconds and cannot stop part way, and whose pair would be a
/// gigabyte on the shelf), a picture's file (whose base is its decode),
/// a frame whose base is the one in hand (`in_hand`, the open file and
/// its base), or one already on the shelf. `ahead` is asked between
/// the stages what has been asked of the worker since; on anything but
/// the open of this frame the prefetch stops there and nothing is
/// kept. The CA runs on `gpu` when there is one, which holds nothing of
/// the base after; a device error there leaves the context to the next
/// develop, which finds out whether the device is lost, and this base
/// takes the CPU's correction.
pub(super) fn prefetch(
    n: &Neighbor,
    in_hand: Option<(&Path, &Base)>,
    shelf: &mut Shelf,
    gpu: Option<&greycard_gpu::Context>,
    lenses: Option<&greycard_lens::Database>,
    ahead: &dyn Fn() -> Ahead,
) -> Fetched {
    let name = file_label(&n.path);
    let side = n.side.word();
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
        && b.turn == turn
        && b.edit.same_base(&n.edit)
    {
        return skipped("its base is the open frame's".into());
    }
    if shelf.holds(
        &n.path,
        &key.stat,
        &n.edit,
        n.seed_blend,
        turn,
        gpu.is_some(),
    ) {
        return skipped("its base is on the shelf".into());
    }
    // Asked between the stages: whether to stop before `stage`, said
    // once when the open of this very frame is what came.
    let told = Cell::new(false);
    let stopped = Cell::new(None::<&'static str>);
    let stop = |stage: &'static str| {
        at_check(&n.path, stage);
        match ahead() {
            Ahead::Nothing => false,
            Ahead::ItsOpen => {
                if !told.replace(true) {
                    note(format!(
                        "the prefetch of {name} goes on before its {stage}: \
                         it is the frame being opened"
                    ));
                }
                false
            }
            Ahead::Work => {
                stopped.set(Some(stage));
                true
            }
        }
    };
    let abandoned = |stage: &'static str| {
        note(format!(
            "the prefetch of the {side} frame, {name}, abandoned before its {stage}: \
             the worker was asked for something"
        ));
        Fetched::Abandoned(stage)
    };
    note(format!("prefetching the {side} frame, {name}"));
    let started = Instant::now();
    let mut stages = Stages::new();
    let input = match timed(&mut stages, "decode", || super::decode::open(&n.path)) {
        Ok((input, _)) => input,
        Err(e) => return skipped(format!("its decode failed: {e:#}")),
    };
    let Input::Raw(frame) = &input else {
        return skipped("it is not a raw".into());
    };
    if stop("prepare") {
        return abandoned("prepare");
    }
    let edit = seeded(&n.edit, n.seed_blend, Some(&input));
    let settings = base_settings(&edit, &input, turn);
    let ca_run = CaRun::new(gpu);
    let run_ca =
        |samples: &[f32], w: usize, h: usize, pattern: &CfaPattern, options: &CaOptions| {
            ca_run.correct(samples, w, h, pattern, options)
        };
    let made = engine_base_or_stop(
        frame,
        &edit,
        turn,
        &settings,
        1,
        Some(&run_ca),
        &mut stages,
        &stop,
    );
    let (ca, failed) = ca_run.into_parts();
    if let Some(e) = failed {
        tracing::warn!("the GPU CA correction, prefetching {name}: {e}; the CPU's for this frame");
    }
    ca_out_of_prepare(&mut stages, ca);
    let mut b = match made {
        Ok(Some(b)) => b,
        Ok(None) => return abandoned(stopped.get().unwrap_or("demosaic")),
        Err(e) => return skipped(format!("its base failed: {e:#}")),
    };
    b.ca_on_gpu = matches!(ca, Some((CaRan::Gpu, _)));
    b.profile = resolved_profile(&settings, frame);
    if stop("lens") {
        return abandoned("lens");
    }
    lens_step(&mut b, &input, &edit, lenses, &mut stages);
    guide_step(&mut b, &mut stages);
    let key = key.decoded(&input);
    drop(input);
    if !shelf.put(key, b, None) {
        return skipped("its base is over the shelf's whole budget".into());
    }
    let gone = shelf.fit();
    note(prefetched_line(
        &name,
        side,
        ca,
        &stages,
        started.elapsed().as_secs_f64(),
        &shelf.said(),
        gone,
    ));
    Fetched::Landed
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

    /// A raw base of `width` by `height` under the default edit.
    fn base(width: usize, height: usize) -> Base {
        Base {
            edit: Edit::default(),
            turn: 0,
            image: Arc::new(WorkingImage::new(width, height)),
            guide: Arc::new(crate::finish::Guide::NONE),
            white: WhiteBase::IDENTITY,
            radius: None,
            clip_level: 1.0,
            source: crate::finish::Source::SCENE,
            stamp: 1,
            patched: None,
            pre: None,
            uploaded: None,
            haze: None,
            planes: None,
            ca_on_gpu: false,
            stand_in: None,
            gave_way: None,
            profile: None,
            from_shelf: false,
        }
    }

    fn neighbor(path: &str, side: Side) -> Neighbor {
        Neighbor {
            path: path.into(),
            edit: Edit::default(),
            turn: 0,
            seed_blend: false,
            side,
        }
    }

    /// The worker idle with a frame open in the editor.
    fn now<'a>(
        landed: Option<u64>,
        open: Option<&'a Base>,
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
        }
    }

    fn both(generation: u64) -> Option<Neighbors> {
        Some(Neighbors::new(
            generation,
            vec![
                neighbor("b.CR3", Side::Next),
                neighbor("z.CR3", Side::Previous),
            ],
        ))
    }

    /// The bytes of a base of `width` by `height`.
    fn size(width: usize, height: usize) -> u64 {
        (width * height * 3 * 4) as u64
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
                &now(Some(3), Some(&open), true, Some(&shelf), idle)
            ),
            Due::Nothing
        ));
        assert!(matches!(
            due(&mut queued, &now(Some(3), None, false, Some(&shelf), idle)),
            Due::Nothing
        ));
        // Not quiet long enough: look again when it will have been.
        match due(
            &mut queued,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET / 3),
        ) {
            Due::Wait(d) => assert!(d > Duration::ZERO && d <= QUIET),
            other => panic!("{other:?}"),
        }
        let mut order = Vec::new();
        while let Due::Fetch(n) = due(
            &mut queued,
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
            &now(Some(3), Some(&open), false, Some(&shelf), idle),
        )) else {
            panic!("due");
        };
        queued.as_mut().unwrap().gave_way(*n);
        assert!(matches!(
            due(
                &mut queued,
                &now(Some(3), Some(&open), false, Some(&shelf), QUIET * 3 / 2)
            ),
            Due::Wait(_)
        ));
        match due(
            &mut queued,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET * 2),
        ) {
            Due::Fetch(n) => assert_eq!(n.side, Side::Next, "begun again first"),
            other => panic!("{other:?}"),
        }
        for _ in 0..5 {
            let n = neighbor("b.CR3", Side::Next);
            queued.as_mut().unwrap().gave_way(n);
        }
        assert_eq!(queued.as_ref().unwrap().quiet, QUIET * 8);

        // No frame open in the editor (culling, the grid): let go.
        let mut queued = both(3);
        let away = Idle {
            frame_open: false,
            ..now(Some(3), Some(&open), false, Some(&shelf), idle)
        };
        assert!(matches!(due(&mut queued, &away), Due::Nothing));
        assert!(queued.is_none(), "let go with the editor");

        // A later open's develop landed: these were the last frame's.
        let mut queued = both(3);
        assert!(matches!(
            due(
                &mut queued,
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
        let edited = crate::edited::Edited::new(Arc::new(Mutex::new(None)));
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
            &now(None, Some(&open), false, Some(&shelf), QUIET * 2),
        );
        settle_reservation(&due_now, &edited, wake);
        assert!(!edited.prefetching(), "let go after a failed open");
        assert_eq!(woken.get(), 1, "the pool woken");

        // Stood down, then a quiet still to come.
        stand_down();
        let due_now = due(
            &mut queued,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET / 2),
        );
        assert!(matches!(due_now, Due::Wait(_)));
        settle_reservation(&due_now, &edited, wake);
        assert!(!edited.prefetching(), "let go while it waits");

        // Stood down, then due again: kept for the claim.
        stand_down();
        let due_now = due(
            &mut queued,
            &now(Some(3), Some(&open), false, Some(&shelf), QUIET * 2),
        );
        assert!(matches!(due_now, Due::Fetch(_)));
        settle_reservation(&due_now, &edited, wake);
        assert!(edited.prefetching(), "kept for the prefetch");
        assert_eq!(woken.get(), 2);
    }

    /// A shelf that holds fewer than two bases the size of the open
    /// frame's (an 8 GB machine at 45 MP) prefetches nothing, and says
    /// so once; two is enough.
    #[test]
    fn a_shelf_for_fewer_than_two_bases_prefetches_nothing() {
        let open = base(40, 30);
        let idle = QUIET * 2;
        for (budget, fetches) in [
            (Budget::One, false),
            (Budget::Bytes(0), false),
            (Budget::Bytes(size(40, 30) * 2 - 1), false),
            (Budget::Bytes(size(40, 30) * 2), true),
        ] {
            let shelf = Shelf::new(budget);
            let mut queued = both(1);
            let got = due(
                &mut queued,
                &now(Some(1), Some(&open), false, Some(&shelf), idle),
            );
            assert_eq!(matches!(got, Due::Fetch(_)), fetches, "{budget:?}");
            if !fetches {
                assert!(queued.is_none(), "let go, not asked again: {budget:?}");
            }
        }
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
        };
        let keep = || Job::Keep {
            frame: Frame::file("a.CR3"),
            edit: Edit::default(),
            turn: 0,
            iso: None,
        };
        let mut q = Queue::default();
        assert_eq!(ahead(&q, true, &n), Ahead::Nothing);
        q.masks_for.push_back(Frame::file("c.CR3"));
        assert_eq!(
            ahead(&q, true, &n),
            Ahead::Nothing,
            "waits behind the open frame"
        );
        assert_eq!(ahead(&q, false, &n), Ahead::Work);
        q.masks_for.clear();

        q.develop = Some(open("b.CR3", Edit::default()));
        assert_eq!(ahead(&q, true, &n), Ahead::ItsOpen);
        q.keeps.push_back(keep());
        assert_eq!(ahead(&q, true, &n), Ahead::ItsOpen, "the frame left's save");
        q.keeps.clear();
        q.develop = Some(open("c.CR3", Edit::default()));
        assert_eq!(ahead(&q, true, &n), Ahead::Work, "another frame's open");
        let warmer = Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature: 6500.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        q.develop = Some(open("b.CR3", warmer));
        assert_eq!(ahead(&q, true, &n), Ahead::Work, "under another base");
        q.develop = Some(Job::Develop {
            edit: Edit::default(),
            generation: 9,
            turn: 0,
        });
        assert_eq!(ahead(&q, true, &n), Ahead::Work, "a slider's develop");
        q.develop = None;

        q.keeps.push_back(keep());
        assert_eq!(ahead(&q, true, &n), Ahead::Work, "a save");
        q.keeps.clear();
        q.exports.push_back(Job::Landscape);
        assert_eq!(ahead(&q, true, &n), Ahead::Work, "an export");
        q.exports.clear();
        q.people = true;
        assert_eq!(ahead(&q, true, &n), Ahead::Work);
        q.people = false;
        assert_eq!(ahead(&q, false, &n), Ahead::Work, "the editor left");
        q.stopping = true;
        assert_eq!(ahead(&q, true, &n), Ahead::Work, "leaving");
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
