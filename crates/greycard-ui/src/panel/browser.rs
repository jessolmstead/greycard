use std::path::{Path, PathBuf};
use std::rc::Rc;

use greycard_core::raw::Orientation;
use greycard_edit::meta::{self, Meta};
use greycard_edit::{Edit, Sidecar};
#[cfg(test)]
use greycard_edit_io::on_disk::load_sidecar_said;
use slint::{ComponentHandle, Model, ModelRc};

pub(crate) use greycard_edit_io::on_disk::file_name;

use crate::panel::crop::read_geometry;
use crate::panel::cull::{cull_select, say_if_empty, show_filter};
use crate::panel::edit::{current_turn, edit_to_develop, read_edit, save_edit, show_edit};
use crate::panel::history::show_history;
use crate::sidecars::write_sidecar;
use crate::tags::{self, Tags};
use crate::worker::{Job, Worker};
use crate::{
    App, GridPlace, STATE, Shared, State, StripPlace, StripSpan, Thumb, WORKER, cells, chooser,
    cull, files, filter, grid, selection, settings, strip, tr,
};

mod open;

pub(crate) use open::{
    current_moved, open_at_start, open_frame, open_job, open_loaded, open_row, opened_is_current,
    release_thumbnails, reopen_moved, reread_open, save_leaving,
};

/// A folder's first round of thumbnails as it comes back from the
/// worker: which have not yet, and what the ones that have cost. The
/// log gets one line when the last arrives, or once the pool has gone
/// idle, the window has taken in all it finished, and the rows on
/// screen are filled (a cold root's far frames come only with a
/// scroll), which is how a folder open is timed with the cache cold,
/// warm, and after a rename.
#[derive(Debug)]
pub(crate) struct ThumbRun {
    started: std::time::Instant,
    waiting: Vec<bool>,
    left: usize,
    cached: usize,
    made: usize,
    failed: usize,
    /// The worker's own time over them, without the develop that
    /// goes first.
    work: f64,
    /// The window's own: how many turns of its event loop took
    /// pictures in, the time they held it, and the longest.
    turns: usize,
    window: f64,
    longest_turn: f64,
    /// The frames drawn while the pictures came: how many, when the
    /// first was from the folder's open, and the longest wait between
    /// two, which is how long the window stood unresponsive.
    frames: usize,
    first_frame: Option<f64>,
    last_frame: Option<std::time::Instant>,
    longest_gap: f64,
    /// The totals, once the last has arrived, for the log at the end
    /// of the turn that took it in.
    done: Option<RunTotals>,
    /// Whether the pool has said the end of a pass that made pictures
    /// for this run, and whether the window's last turn left pictures
    /// finished and not yet taken in: the two a run closed early waits
    /// on.
    pool_idle: bool,
    outbox_left: bool,
}

/// What a finished run says, for the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RunTotals {
    /// The files arrived, of `of` in the folder.
    pub(crate) files: usize,
    pub(crate) of: usize,
    pub(crate) cached: usize,
    pub(crate) made: usize,
    pub(crate) failed: usize,
    pub(crate) seconds: f64,
    pub(crate) work: f64,
}

/// What a run cost the window's thread, for the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RunWindow {
    pub(crate) turns: usize,
    pub(crate) window: f64,
    pub(crate) longest_turn: f64,
    pub(crate) frames: usize,
    pub(crate) first_frame: Option<f64>,
    pub(crate) longest_gap: f64,
}

impl ThumbRun {
    pub(crate) fn new(files: usize) -> Self {
        Self {
            started: std::time::Instant::now(),
            waiting: vec![true; files],
            left: files,
            cached: 0,
            made: 0,
            failed: 0,
            work: 0.0,
            turns: 0,
            window: 0.0,
            longest_turn: 0.0,
            frames: 0,
            first_frame: None,
            last_frame: None,
            longest_gap: 0.0,
            done: None,
            pool_idle: false,
            outbox_left: false,
        }
    }

    /// A turn of the window's event loop took pictures in, and held
    /// it `seconds`.
    pub(crate) fn turn(&mut self, seconds: f64) {
        self.turns += 1;
        self.window += seconds;
        self.longest_turn = self.longest_turn.max(seconds);
    }

    /// The window drew a frame.
    pub(crate) fn frame(&mut self) {
        let now = std::time::Instant::now();
        self.frames += 1;
        match self.last_frame {
            Some(last) => {
                self.longest_gap = self.longest_gap.max(now.duration_since(last).as_secs_f64());
            }
            None => self.first_frame = Some(now.duration_since(self.started).as_secs_f64()),
        }
        self.last_frame = Some(now);
    }

    /// What the run has cost the window so far. The gap still open,
    /// from the last frame to now, counts: a window that has drawn
    /// nothing since the pictures started is waiting all that while.
    pub(crate) fn window(&self) -> RunWindow {
        let open = self
            .last_frame
            .unwrap_or(self.started)
            .elapsed()
            .as_secs_f64();
        RunWindow {
            turns: self.turns,
            window: self.window,
            longest_turn: self.longest_turn,
            frames: self.frames,
            first_frame: self.first_frame,
            longest_gap: self.longest_gap.max(open),
        }
    }

    /// The list renumbered under the run (a merge): `from` says, for
    /// each file now, the number it had, `None` for one new, which is
    /// waited for as well.
    pub(crate) fn renumber(&mut self, from: &[Option<usize>]) {
        self.waiting = from
            .iter()
            .map(|f| f.is_none_or(|i| self.waiting.get(i).copied().unwrap_or(false)))
            .collect();
        self.left = self.waiting.iter().filter(|w| **w).count();
    }

    /// File `index`'s thumbnail came back — from the cache, made, or
    /// not at all when `made` is `None` — and the worker spent
    /// `seconds` on it. A second picture for the same file, a larger
    /// one for the grid, is not the folder's first round and is not
    /// counted. The totals when this was the last.
    pub(crate) fn arrived(
        &mut self,
        index: usize,
        made: Option<bool>,
        seconds: f64,
    ) -> Option<RunTotals> {
        let waiting = self.waiting.get_mut(index)?;
        if !*waiting {
            return None;
        }
        *waiting = false;
        self.left -= 1;
        self.work += seconds;
        match made {
            Some(true) => self.cached += 1,
            Some(false) => self.made += 1,
            None => self.failed += 1,
        }
        (self.left == 0).then(|| self.totals())
    }

    /// The pool said the end of a pass. Heard before anything made
    /// for this run has come, it is the last folder's pass, or one of
    /// lookups alone, and says nothing of this run.
    pub(crate) fn pool_idle(&mut self) {
        if self.made > 0 {
            self.pool_idle = true;
        }
    }

    /// Whether the run may close before every file has come: the pool
    /// idle, nothing it finished still waiting for the window, and the
    /// rows on screen filled.
    pub(crate) fn may_close(&self, screen_filled: bool) -> bool {
        self.pool_idle && !self.outbox_left && screen_filled
    }

    /// What has arrived so far.
    fn totals(&self) -> RunTotals {
        RunTotals {
            files: self.waiting.len() - self.left,
            of: self.waiting.len(),
            cached: self.cached,
            made: self.made,
            failed: self.failed,
            seconds: self.started.elapsed().as_secs_f64(),
            work: self.work,
        }
    }
}

/// Count a thumbnail into the folder's run. The log hears of the
/// last at the end of the turn that took it in ([`end_thumb_turn`]).
pub(crate) fn count_thumb(st: &mut State, index: usize, cached: Option<bool>, seconds: f64) {
    let Some(run) = st.views.thumb_run.as_mut() else {
        return;
    };
    if let Some(t) = run.arrived(index, cached, seconds) {
        run.done = Some(t);
    }
}

/// A turn of the window's that took pictures in, begun at `started`,
/// is over, `left` saying whether it left finished pictures for the
/// next: counted into the run, and when the run's last has come, or it
/// may close early ([`ThumbRun::may_close`]), the run said in the log
/// and closed.
pub(crate) fn end_thumb_turn(st: &mut State, app: &App, started: std::time::Instant, left: bool) {
    let filled = screen_filled(st, app);
    let Some(run) = st.views.thumb_run.as_mut() else {
        return;
    };
    run.turn(started.elapsed().as_secs_f64());
    run.outbox_left = left;
    close_early(run, filled);
    if run.done.is_some() {
        say_thumb_run(st);
    }
}

/// The thumbnails' pool said the end of a pass: the run closes now if
/// it may, with what has come, since the far frames of a cold root come
/// only with a scroll; else at the end of the turn that takes in the
/// last of what the pool finished, or with the next pass's end while
/// the rows on screen wait (the pool held for a develop).
pub(crate) fn thumbs_idle(st: &mut State, app: &App) {
    let filled = screen_filled(st, app);
    let Some(run) = st.views.thumb_run.as_mut() else {
        return;
    };
    run.pool_idle();
    close_early(run, filled);
    if run.done.is_some() {
        say_thumb_run(st);
    }
}

fn close_early(run: &mut ThumbRun, filled: bool) {
    if run.done.is_none() && run.may_close(filled) {
        run.done = Some(run.totals());
    }
}

/// The run's line in the log, and the run closed.
fn say_thumb_run(st: &mut State) {
    let Some(run) = st.views.thumb_run.as_ref() else {
        return;
    };
    let Some(t) = run.done else {
        return;
    };
    let w = run.window();
    // The work is summed over the pool's threads, so it can be
    // more than the wall time.
    let threads = WORKER.with(|w| w.borrow().as_ref().map_or(1, |w| w.thumb_threads()));
    tracing::info!(
        "thumbnails: {} of {} files in {:.2} s from the folder's open, {:.2} s of decoding \
         across {threads} threads; {} from the cache, {} made, {} failed; taken in by the \
         window in {} turns, {:.2} s, the longest {:.0} ms; {} frames drawn meanwhile, the \
         first {}, the longest wait between two {:.0} ms",
        t.files,
        t.of,
        t.seconds,
        t.work,
        t.cached,
        t.made,
        t.failed,
        w.turns,
        w.window,
        w.longest_turn * 1e3,
        w.frames,
        w.first_frame
            .map_or("none".to_string(), |s| format!("at {s:.2} s")),
        w.longest_gap * 1e3,
    );
    st.views.thumb_run = None;
}

/// `--time-scroll`: the moves left, which way the next goes, and for
/// each move the milliseconds from the move to the end of the frame
/// that drew it, and of those the frame's own drawing.
#[derive(Debug)]
pub(crate) struct ScrollTiming {
    left: u32,
    by: f32,
    begun: bool,
    moved: Option<std::time::Instant>,
    drawing: Option<std::time::Instant>,
    to_frame: Vec<f64>,
    drawn: Vec<f64>,
}

impl ScrollTiming {
    pub(crate) fn new(moves: u32, by: f32) -> Self {
        Self {
            left: moves,
            by,
            begun: false,
            moved: None,
            drawing: None,
            to_frame: Vec::new(),
            drawn: Vec::new(),
        }
    }

    /// A frame is being drawn. The moves begin a second after the rows
    /// on screen were drawn with their pictures, with the window
    /// settled; once they
    /// have, the drawing's own time starts here when a move waits for
    /// it.
    pub(crate) fn before_frame(&mut self, pictures_in: bool, app: &App) {
        if !self.begun && pictures_in {
            self.begun = true;
            let app_weak = app.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_secs(1), move || {
                if let Some(app) = app_weak.upgrade() {
                    scroll_step(&app);
                }
            });
        }
        if self.moved.is_some() {
            self.drawing = Some(std::time::Instant::now());
        }
    }

    /// A frame has been drawn. When it drew a move its numbers are
    /// kept and the next move follows.
    pub(crate) fn after_frame(&mut self, app: &App) {
        let (Some(moved), Some(drawing)) = (self.moved.take(), self.drawing.take()) else {
            return;
        };
        self.to_frame.push(moved.elapsed().as_secs_f64() * 1e3);
        self.drawn.push(drawing.elapsed().as_secs_f64() * 1e3);
        let app_weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(app) = app_weak.upgrade() {
                scroll_step(&app);
            }
        });
    }
}

/// The mean, the 95th percentile and the greatest of `samples`.
fn spread(what: &str, samples: &[f64]) -> String {
    if samples.is_empty() {
        return format!("{what}: nothing measured");
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p95 = sorted[((sorted.len() as f64 * 0.95).ceil() as usize).clamp(1, sorted.len()) - 1];
    format!(
        "{what}: {:.2} ms mean, {:.2} ms at the 95th percentile, {:.2} ms at most, over {} moves",
        sorted.iter().sum::<f64>() / sorted.len() as f64,
        p95,
        sorted[sorted.len() - 1],
        sorted.len()
    )
}

/// The resident set, from the kernel, for the scroll's last line.
fn resident_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb / 1024.0)
}

/// `--time-scroll`'s next move, or its numbers and the end.
fn scroll_step(app: &App) {
    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    let mut st = state.borrow_mut();
    let Some(timing) = st.launch.time_scroll.as_mut() else {
        return;
    };
    if timing.left == 0 {
        for line in [
            spread("scroll to its frame drawn", &timing.to_frame),
            spread("the frame's drawing", &timing.drawn),
            format!(
                "scroll: {} cells made, {:.0} MB resident",
                app.get_cells_made(),
                resident_mb().unwrap_or(0.0)
            ),
        ] {
            tracing::info!("{line}");
            eprintln!("{line}");
        }
        st.launch.time_scroll = None;
        let _ = slint::quit_event_loop();
        return;
    }
    timing.left -= 1;
    let mut by = timing.by;
    drop(st);
    if !app.invoke_scroll_by(by) {
        by = -by;
        app.invoke_scroll_by(by);
    }
    if let Some(timing) = state.borrow_mut().launch.time_scroll.as_mut() {
        timing.by = by;
        timing.moved = Some(std::time::Instant::now());
    }
    app.window().request_redraw();
}

/// Whether every row on screen, the grid's when it is open and the
/// strip's when not, has its picture or is marked as having none.
/// False while neither has said what it shows.
pub(crate) fn screen_filled(st: &State, app: &App) -> bool {
    let range = if app.get_grid_open() {
        st.views.grid_shown
    } else {
        st.views.strip_shown
    };
    let Some((first, last)) = range else {
        return false;
    };
    (first.max(0) as usize..=last.max(0) as usize)
        .filter_map(|r| st.views.shown.get(r))
        .all(|&f| st.views.thumb_base[f].is_some() || st.views.thumb_failed[f])
}

/// The name of the frame on screen over the panel, and under it the
/// camera's JPEG beside its raw when there is one; both empty for none.
pub(crate) fn show_file_name(st: &State, app: &App, path: Option<&Path>) {
    app.set_file_name(path.map(file_name).unwrap_or_default().into());
    app.set_file_companion(
        path.map(|p| companion_words(st, p))
            .unwrap_or_default()
            .into(),
    );
}

/// The camera's JPEGs beside the raw at `path`, in the panel's words:
/// "With the camera's JPEG, IMG_0001.JPG"; empty for a frame with none.
pub(crate) fn companion_words(st: &State, path: &Path) -> String {
    match st.views.companions.get(path).map(Vec::as_slice) {
        None | Some([]) => String::new(),
        Some(with) => {
            let names: Vec<String> = with.iter().map(|p| file_name(p)).collect();
            tr!("With the camera's JPEG, {}", names.join(", "))
        }
    }
}

/// A browser row for a file: its name, the badges its meta asks for,
/// and no picture until the worker has made one.
pub(crate) fn thumb_for(path: &Path, meta: &Meta, marks: (bool, bool)) -> Thumb {
    Thumb {
        name: file_name(path).into(),
        image: slint::Image::default(),
        big: slint::Image::default(),
        queue_done: false,
        queue_busy: false,
        pending: false,
        rating: meta.rating.min(meta::STARS) as i32,
        flag: meta.flag.code(),
        label: meta.label.code(),
        chosen: false,
        failed: false,
        offline: false,
        edited: marks.0,
        exported: marks.1,
        collected: false,
        paired: false,
        version: Default::default(),
        stack: 0,
        stack_place: 0,
        stack_open: false,
    }
}

/// Whether cell `c` shows as edited and as exported: what its own line
/// of the sidecar says once the window holds it, and what its row said
/// of it while the row stands in (`greycard_library::line_marks`, the
/// rule the index applies to each line).
pub(crate) fn frame_marks(st: &State, c: usize) -> (bool, bool) {
    let Some(cell) = st.cell(c) else {
        return (false, false);
    };
    let version = cell.version.as_deref();
    match (
        st.from_row.get(cell.file),
        st.sidecars.get(cell.file),
        st.files.get(cell.file),
    ) {
        (Some(r), _, _) if !r.read => r.marks(version),
        (r, Some(s), Some(f)) => s.line(version).map_or((false, false), |line| {
            greycard_library::line_marks(line, f, r.and_then(|r| r.iso))
        }),
        _ => (false, false),
    }
}

/// The selection as it is acted on: the current frame and the rest
/// of the set, as files, sorted, and only what the filter shows. A
/// current frame the filter has just hidden is not in it: a key must
/// not reach a frame nobody can see is chosen. Empty with nothing
/// open.
pub(crate) fn chosen_frames(st: &State) -> Vec<usize> {
    selection::frames(&st.views.picked, st.current)
        .into_iter()
        .filter(|&f| row_of(st, f).is_some())
        .collect()
}

/// Put the set on the strip's and the grid's rows, and its size on
/// the window. Only the rows whose mark changed are written, so a
/// click in a folder of hundreds touches two rows and not all of
/// them.
pub(crate) fn show_set(st: &mut State, app: &App) {
    let chosen_set = chosen_frames(st);
    let model = app.get_thumbs();
    for (row, &f) in st.views.shown.iter().enumerate() {
        let chosen = chosen_set.binary_search(&f).is_ok();
        if let Some(mut t) = model.row_data(row)
            && t.chosen != chosen
        {
            t.chosen = chosen;
            model.set_row_data(row, t);
        }
    }
    app.set_set_count(chosen_set.len().max(1) as i32);
    crate::panel::cull::show_back(st, app);
    crate::panel::words::show_frame_words(st, app);
}

/// Put cell `c`'s meta and marks on its row in the strip and the grid,
/// leaving whatever picture the row already holds. The meta is the
/// file's: a change to it is shown on every cell of the file
/// ([`show_badges_of_file`]).
///
/// The row and the cell are two different numbers whenever a filter
/// is on — `row_of` is what maps one to the other — and both the
/// read and the write are the row's, as `show_thumb` has it.
pub(crate) fn show_badges(st: &State, app: &App, c: usize) {
    let Some(meta) = st.sidecar_at(c).map(|s| &s.meta) else {
        return;
    };
    let Some(row) = row_of(st, c) else {
        return;
    };
    let marks = frame_marks(st, c);
    let model = app.get_thumbs();
    if let Some(mut t) = model.row_data(row) {
        let was = (t.rating, t.flag, t.label, t.edited, t.exported);
        t.rating = meta.rating.min(meta::STARS) as i32;
        t.flag = meta.flag.code();
        t.label = meta.label.code();
        t.edited = marks.0;
        t.exported = marks.1;
        // Only a row that changed is written, so a cell not touched
        // is not drawn again.
        if was != (t.rating, t.flag, t.label, t.edited, t.exported) {
            model.set_row_data(row, t);
        }
    }
    if Some(c) == st.current {
        show_frame_tags(st, app);
    }
}

/// [`show_badges`] on every cell of file `file`: for a change to the
/// file's meta, or a save of its sidecar, which every line may have
/// moved in.
pub(crate) fn show_badges_of_file(st: &State, app: &App, file: usize) {
    for c in st.cells_of_file(file) {
        show_badges(st, app, c);
    }
}

/// Put the frame on screen's meta on the window, for the culling
/// loupe's badge: the same three numbers its strip row carries, read
/// off the same sidecar. Called when the selection lands on a frame
/// and whenever that frame's badges are put out, so the two cannot
/// say different things. The METADATA section's title and keywords
/// are put out with them, over the whole selection.
pub(crate) fn show_frame_tags(st: &State, app: &App) {
    crate::panel::words::show_frame_words(st, app);
    let Some(meta) = st.current.and_then(|c| st.sidecar_at(c)).map(|s| &s.meta) else {
        return;
    };
    app.set_frame_rating(meta.rating.min(meta::STARS) as i32);
    app.set_frame_flag(meta.flag.code());
    app.set_frame_label(meta.label.code());
}

/// Put a culling key's change into every frame in `frames`, write
/// each one's sidecar, and put the badges on its thumbnail.
///
/// The badge is the whole answer to the key; there is no word in the
/// status line to go with it. A cull is one key and then the arrow
/// to the next frame, and the develop that arrow starts would write
/// over any such word before it had been read.
///
/// The set is the whole selection (`chosen_frames`): the frame on
/// screen and whatever Ctrl, Shift or Shift and an arrow put beside
/// it. A label key is settled against the set first, so one press on
/// a mixed selection makes it uniform rather than half one thing and
/// half the other.
///
/// The write goes through the sidecar the edit is saved from, so
/// neither can lose the other: they are one file and one struct, and
/// what is written is always both. The panel may hold an edit newer
/// than the one on disk, which the save timer writes with this meta
/// beside it a moment later.
///
/// Under a filter the list can change: a frame just rejected leaves
/// a browser showing the picks, and so does one just given a star
/// under a filter that wants four. The selection then moves to the
/// nearest frame still shown, whose row comes back for the caller to
/// open once the state is free — a switch in culling, a develop out
/// of it, which is the rule the three-way Show set and the one
/// Lightroom keeps.
///
/// Whether the frame is still shown is asked of the filter rather
/// than of the kind of key, since every one of the three fields the
/// keys write is a field the filter can be reading. The chips' counts
/// move with any of them, so they are put out again either way.
///
/// Returned with the row is the change as it was settled against the
/// frames (a label key that cleared the label comes back as `None`),
/// which is what the word over the picture says.
pub(crate) fn set_meta(
    st: &mut State,
    app: &App,
    frames: &[usize],
    change: meta::Change,
) -> (meta::Change, Option<usize>) {
    // A change is written to the whole sidecar, so each frame's is
    // read first where it stands in from its row; one under a root
    // that is offline is left out, and the status line says so. The
    // meta is the file's: a key over a file and a version of it sets
    // it once, and every cell of the file follows.
    let cells = &crate::rows::load_frames(st, app, frames);
    let files = st.files_of(cells);
    let was_shown: Vec<bool> = cells.iter().map(|&c| row_of(st, c).is_some()).collect();
    let before: Vec<Tags> = files
        .iter()
        .map(|&f| Tags::of(&st.sidecars[f].meta))
        .collect();
    let (settled, moved) = greycard_edit::meta_into(&mut st.sidecars, &files, change);
    let step = files
        .iter()
        .zip(before)
        .filter(|(f, _)| moved.contains(f))
        .map(|(&f, before)| tags::Moved {
            path: st.files[f].clone(),
            before,
            after: Tags::of(&st.sidecars[f].meta),
        })
        .collect();
    st.culling.tags.record(step);
    let next = tags_shown(st, app, cells, &was_shown, &moved);
    if matches!(
        settled,
        meta::Change::TimeShift(_) | meta::Change::ClearTimeShift
    ) {
        crate::panel::time_shift::moved(st, app, &moved);
    }
    (settled, next)
}

/// What follows a change to files' tags, from a key or an undo: each
/// moved file written once and the badges of its cells put out, the
/// rejects counted, and the browser's rows rebuilt if the filter now
/// shows a different set; the row to move the selection to when the
/// current frame left them. `cells` are the cells the change was
/// over, `moved` the files that moved.
fn tags_shown(
    st: &mut State,
    app: &App,
    cells: &[usize],
    was_shown: &[bool],
    moved: &[usize],
) -> Option<usize> {
    crate::sidecars::write_files(st, moved);
    for &f in moved {
        show_badges_of_file(st, app, f);
    }
    app.set_reject_count(reject_count(st) as i32);
    // The selection's words, whichever of its frames moved: the
    // badges put them out only when the frame on screen did.
    crate::panel::words::show_frame_words(st, app);
    if moved.is_empty() {
        return None;
    }
    let hides = cells
        .iter()
        .zip(was_shown)
        .any(|(&c, &was)| was != filter_shows(st, c));
    if !hides {
        show_filter(st, app);
        return None;
    }
    let next = rebuild_browser(st, app);
    if next.is_some() {
        // The step took the window off its frame: where it lands, the
        // step is that frame's to undo too, in Develop. `carry` takes
        // the step last acted on, which is this one: the callers
        // record or step it just before.
        st.culling.tags.carry();
    }
    if next.is_none()
        && st.culling.cull.is_some()
        && let Some(c) = st.current
    {
        // The rows moved under the window: decode about them anew.
        cull_select(st, app, c);
    }
    say_if_empty(st, app);
    next
}

/// Turn every frame in `frames` by `quarters` quarter turns
/// clockwise: the camera's orientation tag put right, which is a
/// fact about the picture and not a step in developing it.
///
/// The turn goes on the sidecar beside the edit, as the meta does
/// (§117), and is written promptly through the same `Sidecar::save`.
/// Nothing here records a history state or marks the panel dirty.
/// The crop, the keystone and a held aspect turn with the picture
/// (`Geometry::under_turned_source`); masks do not, since they are
/// in the developed picture's own units and a turn moves what is
/// under them, which is what an edit's own quarter turns already do.
///
/// For the open frame outside culling the panel holds the edit, so
/// the crop turns there and the sidecar takes the panel's edit
/// without recording it; whatever the panel was holding before is
/// recorded first, so a turn cannot swallow a slider's state.
pub(crate) fn turn_frames(
    st: &mut State,
    app: &App,
    worker: &Worker,
    frames: &[usize],
    quarters: i32,
) {
    if quarters.rem_euclid(4) == 0 {
        return;
    }
    // The Original kept is of the turn before.
    crate::panel::original::drop_picture(st);
    // A turn is written to the whole sidecar, so each frame's is read
    // first where it stands in from its row; one under a root that is
    // offline is left out, and the status line says so.
    let frames = &crate::rows::load_frames(st, app, frames);
    // The frame whose edit the panel owns: the open one, and not
    // while culling, where the panel still holds the last-opened
    // frame's edit and the sidecars are the truth.
    let panel_frame = st.current.filter(|_| st.culling.cull.is_none());
    // The turn is the file's and turns every line of it: a file once,
    // from the panel's own cell when that is one of its.
    let frames = &st.once_a_file(frames, panel_frame);
    if let Some(cull) = st.culling.cull.as_mut() {
        cull.turned = Some(std::time::Instant::now());
    }
    let mut developing = false;
    for &f in frames {
        if st.sidecar_at(f).is_none() {
            continue;
        }
        migrate_frame(st, f);
        let aspect = frame_aspect(st, f);
        if aspect.is_none() && st.sidecar(f).placed() {
            tracing::warn!(
                "{}: no picture of it yet to measure by, so its masks and \
                 repair patches stay where they are on the screen",
                file_name(st.path_of(f))
            );
        }
        // The panel's own cell, while its line is there: a version a
        // read found gone has no line to record on or to show, and its
        // file turns as any other's does until the cells follow.
        if Some(f) == panel_frame && st.line(f).is_some() {
            // The panel owns the open frame's edit, so what it is
            // holding is recorded first and the sidecar's states are
            // then turned together; the turn itself records nothing.
            let pending = read_edit(app, &st.develop.edit, st.develop.target);
            save_edit(st, pending);
            st.sidecar_mut(f).turn_by(quarters, aspect);
            // The Fill's strokes waiting on it turn with its patches.
            let path = st.path_of(f).clone();
            crate::panel::painted::turn(st, &path, quarters, aspect);
            let edit = st.line(f).map(|l| l.current.clone()).unwrap_or_default();
            // The whole panel, since a turn moves masks and patches
            // as well as the crop. `show_edit` puts a chosen
            // adjustment's tab up, which a turn pressed on the Crop
            // tab has not asked for, so the tab is kept.
            let tab = app.get_panel_tab();
            show_edit(st, &edit, app, st.develop.target);
            app.set_panel_tab(tab);
            st.develop.edit = edit;
            // The developed size stands on end now, and the develop
            // that will say so is a second away: a second turn
            // pressed before it lands would otherwise measure the
            // frame by the shape it used to be.
            if quarters.rem_euclid(2) == 1 {
                st.develop.source_size = (st.develop.source_size.1, st.develop.source_size.0);
            }
            developing = true;
        } else {
            let file = st.file_of(f);
            greycard_edit::turn_into(&mut st.sidecars, &[file], quarters, aspect);
            let path = st.path_of(f).clone();
            crate::panel::painted::turn(st, &path, quarters, aspect);
        }
        write_sidecar(st, f);
        for g in st.cells_of_file(st.file_of(f)) {
            let (turns, flip) = thumb_turns(st, app, g);
            show_thumb(st, app, g, turns, flip);
            crate::lightbox::show(st, app, g);
        }
    }
    // The rasters are keyed by adjustment and component and know
    // nothing of a turn: a brush's is only brought up to its strokes
    // rather than remade, and a subject's shape does not change at
    // all, so both would go on masking the pixels they masked
    // before. They are cheap beside a mask in the wrong place.
    st.develop.rasters.clear();
    st.develop.learned.clear();
    st.develop.asked.clear();
    st.develop.masks_failed.clear();
    // The open frame, turned from the culling loupe or among a
    // selection, is measured the way up the viewport will draw it.
    crate::panel::viewport::settle_source_size(st);
    // The picture stands on end now, so whatever was fitted is
    // fitted afresh; the culling loupe reads the turn out of the
    // same matrix on its next frame, which is a redraw and not a
    // decode. A develop zoomed into keeps the picture point in the
    // middle, turned about it (`viewport::frame_on_show`).
    if st.develop.zoom == 0.0 || crate::panel::cull::loupe_draws(st) {
        st.develop.image_size = (0, 0);
    }
    // Whatever the panel was holding was recorded on the way in; the
    // turn itself added no state. A frame the worker was not sent (a
    // pick reading its sidecar) develops when its open goes out.
    if developing {
        show_history(st, app);
    }
    if developing && crate::panel::edit::develops_here(st) {
        st.develop.generation += 1;
        st.develop.turn_pressed = Some(std::time::Instant::now());
        app.set_status(tr!("developing...").into());
        app.set_busy(true);
        worker.send(Job::Develop {
            edit: edit_to_develop(app, &st.develop.edit),
            generation: st.develop.generation,
            turn: current_turn(st),
            of: crate::panel::edit::develop_of(st),
        });
    }
    app.window().request_redraw();
}

/// Put file `i`'s picture on the filmstrip turned `turns` quarters and
/// mirrored if `flip`, as the viewport turns the file.
pub(crate) fn show_thumb(st: &mut State, app: &App, i: usize, turns: u8, flip: bool) {
    let Some((w, h, rgb)) = &st.views.thumb_base[i] else {
        return;
    };
    // A frame the filter hides has no row; its picture waits for the
    // list to show it again.
    let Some(row) = row_of(st, i) else {
        return;
    };
    // The grid lays its rows out by the pictures' shapes, those off
    // screen included.
    note_shape(app, row, aspect_of(st, i, turns));
    // Nor does a row neither view has a cell for: it takes its
    // picture when one comes to it (`settle_pictures`), turned as the
    // frame is turned then.
    if !cells::has_cell(app, row) {
        return;
    }
    st.views.thumb_shown[i] = Some((turns, flip));
    let (pw, ph, turned) = greycard_edit::geometry::turn_pixels(*w, *h, rgb, turns, flip);
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(pw, ph);
    buf.make_mut_bytes().copy_from_slice(&turned);
    let model = app.get_thumbs();
    if let Some(mut t) = model.row_data(row) {
        t.image = slint::Image::from_rgb8(buf);
        t.failed = false;
        model.set_row_data(row, t);
    }
}

/// Put the pictures in hand on the rows the strip or the grid has come
/// to have a cell for, and take them off the rows neither has one for
/// any more. A row carries its picture only while a view can draw it:
/// the rest are kept in `thumb_base`, once, rather than twice over as
/// bytes and as an image for every frame of the folder.
pub(crate) fn settle_pictures(st: &mut State, app: &App) {
    let now = [
        cells::window(app, cells::View::Strip),
        cells::window(app, cells::View::Grid),
    ];
    let has = |row: usize| now.iter().any(|&(f, n)| (f..f + n).contains(&row));
    let model = app.get_thumbs();
    for (first, count) in std::mem::replace(&mut st.views.pictured, now) {
        for row in (first..first + count).filter(|&r| !has(r)) {
            let Some(&f) = st.views.shown.get(row) else {
                break;
            };
            if st.views.thumb_shown[f].take().is_some()
                && let Some(mut t) = model.row_data(row)
            {
                t.image = slint::Image::default();
                model.set_row_data(row, t);
            }
        }
    }
    for (first, count) in now {
        for row in first..first + count {
            let Some(&f) = st.views.shown.get(row) else {
                break;
            };
            if st.views.thumb_base[f].is_some() && st.views.thumb_shown[f].is_none() {
                let (turns, flip) = thumb_turns(st, app, f);
                show_thumb(st, app, f, turns, flip);
            }
        }
    }
}

/// No picture could be made of file `i`: its cell says so, and
/// counts as filled for a snapshot of the grid. A file that already
/// has a picture (a larger one failed) keeps it and is not marked,
/// but is as filled as it will get.
pub(crate) fn show_no_thumb(st: &mut State, app: &App, i: usize) {
    let Some(had) = st.views.thumb_base.get(i).map(Option::is_some) else {
        return;
    };
    st.views.thumb_failed[i] = true;
    // A frame under a root that is offline had only the cache to
    // come from: its tile is dimmed already, and says nothing more.
    if had || crate::rows::is_offline(st, i) {
        return;
    }
    let Some(row) = row_of(st, i) else {
        return;
    };
    let model = app.get_thumbs();
    if let Some(mut t) = model.row_data(row) {
        t.failed = true;
        model.set_row_data(row, t);
    }
}

/// How a frame stands in its own file, read once and kept: the
/// orientation tag, and the size a develop of it comes out at. One
/// metadata read a frame a session, and only for a frame that is
/// turned or whose meta is written to an XMP.
pub(crate) fn stance(st: &mut State, c: usize) -> Option<greycard_core::decode::Stance> {
    let path = st.path_at(c)?.clone();
    if let Some(s) = st.develop.stances.get(&path) {
        return Some(*s);
    }
    match greycard_core::decode::stance_path(&path) {
        Ok(s) => {
            st.develop.stances.insert(path, s);
            Some(s)
        }
        Err(e) => {
            tracing::warn!("{}: how the frame stands: {e}", file_name(&path));
            None
        }
    }
}

/// What is known of a file's mosaic, which decides its demosaics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OpenMosaic {
    /// Not read yet: the demosaic menu rests rather than guess.
    #[default]
    Unknown,
    Known(greycard_core::raw::Mosaic),
    /// A picture, or a raw that is not a mosaic (a linear DNG, a
    /// monochrome sensor): there is no demosaic to choose.
    NotMosaic,
    /// Neither the read ahead nor the decode could read the raw: the
    /// open failed, and the menu says so rather than wait.
    Unreadable,
}

impl OpenMosaic {
    /// The mosaic, when it is known to be one.
    pub(crate) fn kind(self) -> Option<greycard_core::raw::Mosaic> {
        match self {
            OpenMosaic::Known(m) => Some(m),
            _ => None,
        }
    }
}

/// What the session knows of cell `c`'s mosaic without reading the
/// file: a picture is none, then what the worker has said of it, then
/// a stance already read, then the library's row, which its indexer
/// filled in the background.
pub(crate) fn known_mosaic(st: &State, c: usize) -> OpenMosaic {
    let Some(path) = st.path_at(c) else {
        return OpenMosaic::Unknown;
    };
    if greycard_core::picture::is_picture_path(path) {
        return OpenMosaic::NotMosaic;
    }
    if let Some(m) = st.develop.mosaics.get(path) {
        return *m;
    }
    if let Some(m) = st.develop.stances.get(path).and_then(|s| s.mosaic) {
        return OpenMosaic::Known(m);
    }
    match st
        .views
        .index_reader
        .as_ref()
        .and_then(|lib| lib.mosaic(path).ok().flatten())
    {
        Some(greycard_library::FileMosaic::Mosaic(m)) => OpenMosaic::Known(m),
        Some(greycard_library::FileMosaic::NotMosaic) => OpenMosaic::NotMosaic,
        None => OpenMosaic::Unknown,
    }
}

/// The panel is cell `c`'s from here (or no file's): the one place the
/// panel's mosaic is set, called wherever the panel's file changes
/// (an open, a pick, an offline frame, leaving culling) before its
/// edit is shown. Nothing is read: what the session knows, else
/// unknown until the worker says ([`learn_mosaic`]).
pub(crate) fn panel_frame(st: &mut State, c: Option<usize>) {
    st.develop.panel_mosaic = c.and_then(|c| {
        let known = known_mosaic(st, c);
        st.path_at(c).cloned().map(|p| (p, known))
    });
}

/// Remember what the worker found of `path`'s mosaic and, when it is
/// the panel's file, show the demosaic menu for it.
pub(crate) fn learn_mosaic(st: &mut State, app: &App, path: &Path, mosaic: OpenMosaic) {
    if mosaic == OpenMosaic::Unknown {
        return;
    }
    st.develop.mosaics.insert(path.to_path_buf(), mosaic);
    if let Some((p, m)) = st.develop.panel_mosaic.as_mut()
        && p == path
        && *m != mosaic
    {
        *m = mosaic;
        // What the panel stores, which a pick may have changed since
        // the edit was shown.
        let stored = crate::panel::edit::read_demosaic(app);
        crate::panel::edit::show_demosaic(mosaic, stored, app);
    }
}

/// The panel's file failed to open while its mosaic was still unknown:
/// the menu's last word is that the raw could not be read.
pub(crate) fn mosaic_unreadable(st: &mut State, app: &App) {
    if let Some((_, m)) = st.develop.panel_mosaic.as_mut()
        && *m == OpenMosaic::Unknown
    {
        *m = OpenMosaic::Unreadable;
        let stored = crate::panel::edit::read_demosaic(app);
        crate::panel::edit::show_demosaic(OpenMosaic::Unreadable, stored, app);
    }
}

/// Cell `c`'s mosaic for a caller that needs it now (a sync carrying a
/// demosaic): what is known ([`known_mosaic`], the library's row
/// among it), else a read of the file's image description on this
/// thread, a millisecond or so locally, kept for the session. Only a
/// file the library has not indexed yet comes to the read.
pub(crate) fn mosaic_of(st: &mut State, c: usize) -> Option<greycard_core::raw::Mosaic> {
    match known_mosaic(st, c) {
        OpenMosaic::Known(m) => Some(m),
        OpenMosaic::NotMosaic | OpenMosaic::Unreadable => None,
        OpenMosaic::Unknown => {
            let path = st.path_at(c)?.clone();
            let read = greycard_core::decode::mosaic_path(&path)
                .inspect_err(|e| tracing::warn!("{}: its mosaic: {e}", file_name(&path)))
                .ok()?;
            st.develop
                .mosaics
                .insert(path, read.map_or(OpenMosaic::NotMosaic, OpenMosaic::Known));
            read
        }
    }
}

/// A frame's own orientation tag. `None` when the file will not say,
/// which is when nothing here writes or believes an XMP's
/// orientation for it.
pub(crate) fn camera_tag(st: &mut State, c: usize) -> Option<Orientation> {
    stance(st, c).map(|s| s.orientation)
}

/// The shape of a frame's developed picture, width over height, for
/// the map that carries masks and repair patches round a turn.
///
/// They are in units of that picture's width, both ways, so the map
/// wants its aspect and nothing else — but it has to be *that*
/// picture's. The file's own answer comes first: the develop on
/// screen when it is this frame's, and otherwise the size rawler
/// reports for the frame with no pixel decoded, turned by the tag and
/// by the frame's own quarters. The camera's JPEG is only a fallback
/// for want of those, because a body can write its embedded picture
/// at 16:9 or square with the raw left full frame, and a mask mapped
/// by 1.778 where 1.5 was wanted lands nowhere.
///
/// `None` when nothing can say, which is the one case where a turn
/// cannot carry placed work with it.
pub(crate) fn frame_aspect(st: &mut State, cell: usize) -> Option<f32> {
    let turn = st.sidecar_at(cell).map_or(0, |s| s.turn);
    let aspect = |(w, h): (u32, u32)| (w > 0 && h > 0).then(|| w as f32 / h as f32);
    // The open frame's own develop: exactly the picture the masks
    // are in, and already the way up the frame's turn puts it.
    if st.current == Some(cell)
        && st.culling.cull.is_none()
        && let Some(a) = aspect(st.develop.source_size)
    {
        return Some(a);
    }
    // The cell itself, which is the same frame however it is shown.
    if let Some(a) = stance(st, cell)
        .and_then(|s| s.shown_size(turn))
        .and_then(aspect)
    {
        return Some(a);
    }
    // Failing that, a picture of it the window already has. These
    // are turned by the camera's tag alone, so the frame's own turn
    // goes on top.
    let shown = |(w, h): (u32, u32)| {
        let (w, h) = if turn % 2 == 1 { (h, w) } else { (w, h) };
        aspect((w, h))
    };
    // A frame the camera framed: its develop is the whole area the
    // frame is a part of, which its pictures may be cut from
    // (`cull::cut_to_camera`) or not (the camera's JPEG beside it).
    // One whose frame is not known yet may have pictures cut to it:
    // only a picture known to be whole says its shape, and failing one
    // the answer waits (`None`) rather than be a cut picture's.
    let path = st.path_of(cell);
    let unknown = match crate::edited_pictures::camera_state(path) {
        Some(Some(camera)) => return shown(camera.shown_area()),
        Some(None) => false,
        None => greycard_core::decode::may_have_camera_frame(path),
    };
    if let Some(cull) = st.culling.cull.as_ref().or(st.culling.hold.as_ref())
        && let Some(preview) = cull.cache.best(cell)
        && !(unknown && (preview.cut || preview.companion))
        && let Some(a) = shown(preview.source)
    {
        return Some(a);
    }
    if unknown {
        return None;
    }
    st.views
        .thumb_base
        .get(cell)
        .and_then(|t| t.as_ref())
        .and_then(|(w, h, _)| shown((*w, *h)))
}

/// Finish a sidecar's version 4 migration, now that the frame's
/// shape can be had: see [`greycard_edit::Edit::migrate_with_frame`].
///
/// The folder's scan loads every sidecar with no frame to measure
/// by, so an `Original` crop is left part-migrated and reads, renders
/// and saves back as the version 3 it still is. This is called the
/// first moment a frame is in hand and before anything can act on
/// its geometry: as it is selected, as it is culled, and before a
/// turn maps it. The shape comes from [`frame_aspect`], so it is the
/// same picture the masks are in, and the file's own answer is a
/// cached metadata read rather than a decode.
///
/// A frame nothing can measure is left as it is and tried again next
/// time; the alternative is guessing which way up it is, and a guess
/// here re-crops the picture.
pub(crate) fn migrate_frame(st: &mut State, c: usize) {
    if crate::panel::queue_view::refused(st, "a sidecar's migration") {
        return;
    }
    if !st.sidecar_at(c).is_some_and(Sidecar::needs_frame) {
        return;
    }
    match frame_aspect(st, c) {
        Some(aspect) => st.sidecar_mut(c).migrate_with_frame(aspect < 1.0),
        None => tracing::warn!(
            "{}: nothing can say which way up it is yet, so its Original crop \
             waits before it is brought up to date",
            file_name(st.path_of(c))
        ),
    }
}

/// The browser's row of a cell, when the filter shows it.
pub(crate) fn row_of(st: &State, cell: usize) -> Option<usize> {
    st.views.shown.binary_search(&cell).ok()
}

/// The folder as the filter reads it: each file beside the meta its
/// sidecar holds, and whether the library index's tests passed it
/// when the list was last made. Nothing is read from disk for this —
/// the sidecars of the open folder are already in hand, and the
/// index's answer is asked once a list, in `rebuild_browser`.
pub(crate) fn filter_frames(st: &State) -> Vec<filter::Frame<'_>> {
    st.files
        .iter()
        .zip(&st.sidecars)
        .enumerate()
        .map(|(i, (path, s))| filter::Frame {
            path,
            meta: &s.meta,
            index: st.views.index_passed.get(i).copied().unwrap_or(true),
        })
        .collect()
}

/// Whether the filter shows one cell, as it stands now: the filter is
/// over files, and a file that passes brings its versions with it.
pub(crate) fn filter_shows(st: &State, c: usize) -> bool {
    let Some(file) = st.file_at(c) else {
        return false;
    };
    match (st.files.get(file), st.sidecars.get(file)) {
        (Some(path), Some(s)) => st.views.filter.shows(filter::Frame {
            path,
            meta: &s.meta,
            index: st.views.index_passed.get(file).copied().unwrap_or(true),
        }),
        _ => false,
    }
}

/// How a file's picture is turned in the browser and the loupe: the
/// open file's as the panel has it, another's as its sidecar does,
/// and the frame's own quarter turns folded into both.
///
/// The pictures these three draw are the camera's own, turned by the
/// camera's own tag and no further, so a frame's turn is folded into
/// the quarter turns rather than the picture being turned again: see
/// [`Geometry::shown_turns`]. That is what makes a turn cost a
/// redraw here and not a decode.
pub(crate) fn thumb_turns(st: &State, app: &App, c: usize) -> (u8, bool) {
    // A frame whose sidecar stands in from its row: the row says (the
    // file's, for a version too, whose own geometry the row does not
    // have).
    let file = st.file_of(c);
    if let Some(from) = st.from_row.get(file)
        && !from.read
    {
        return from.turns;
    }
    let turn = st.sidecars[file].turn;
    if st.current == Some(c) && st.culling.cull.is_none() {
        read_geometry(app).shown_turns(turn)
    } else {
        // A version's cell whose line is gone (taken out by a read, the
        // cells to follow): the turn alone, never the file's own
        // geometry.
        st.line(c).map_or_else(
            || greycard_edit::Geometry::default().shown_turns(turn),
            |l| l.current.geometry.shown_turns(turn),
        )
    }
}

/// How many files of the folder are flagged reject.
pub(crate) fn reject_count(st: &State) -> usize {
    (0..st.files.len())
        .filter(|&f| crate::panel::cull::to_move_out(st, f))
        .count()
}

/// The tile size slider, the grid header's and the lightbox bar's: its
/// knob's place asked of `grid` and `lightbox`, its range handed down
/// from their constants, and a place it is moved to taken as the
/// grid's cell or the lightbox's tiles across, by which is on. Both
/// are kept in the settings as the keys' sizes are.
pub(crate) fn tile_size(app: &App) {
    app.on_grid_most(grid::most);
    app.on_grid_slider_at(grid::slider_at);
    app.on_lightbox_slider_at(crate::lightbox::slider_at);
    app.set_grid_least(grid::STEPS[0]);
    app.set_lightbox_fewest(crate::lightbox::FEWEST);
    app.set_lightbox_most(crate::lightbox::MOST);
    let weak = app.as_weak();
    app.on_tile_size_moved(move |at| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if app.get_lightbox() {
            let across = crate::lightbox::slider_across(at);
            if across != app.get_lightbox_across() {
                app.set_lightbox_across(across);
            }
        } else {
            // The right end is the largest step, as plus's last step
            // is, whatever the sheet holds it to now: a wider window
            // gives the largest back.
            let cell = if at >= 1.0 {
                grid::STEPS[grid::STEPS.len() - 1]
            } else {
                grid::slider_cell(at, grid::most(app.get_grid_w()))
            };
            if cell != app.get_grid_cell() {
                app.set_grid_cell(cell);
                crate::panel::prefs::show_grid_cell(&app);
            }
        }
    });
}

/// The grid's layout under its headings, asked of the arithmetic in
/// `grid`: where each frame sits, how far the sheet scrolls, and the
/// scroll that shows a frame.
///
/// The headings come with each call, as the window has them, so a
/// binding that asks follows them when they change.
pub(crate) fn grid_layout(app: &App) {
    fn heads(m: &ModelRc<i32>) -> Vec<i32> {
        m.iter().collect()
    }
    let weak = app.as_weak();
    app.on_grid_place(move |i, cell, cols, h, shape| {
        let Some(app) = weak.upgrade() else {
            return GridPlace::default();
        };
        let count = app.get_thumbs().row_count() as i32;
        let layout = cells::layout(&app, cell, cols, count, &heads(&h), shape);
        let Some(p) = layout.place(i) else {
            return GridPlace {
                pic: cell,
                ..GridPlace::default()
            };
        };
        let aspect = if shape == cells::SQUARE {
            1.0
        } else {
            cells::aspect(&app, i.max(0) as usize)
        };
        GridPlace {
            col: p.col,
            line: p.line,
            heads: p.heads,
            y: p.y,
            pic: grid::picture_room(cell, aspect),
        }
    });
    let weak = app.as_weak();
    app.on_grid_row_at(move |y, cell, cols, count, h, shape| {
        weak.upgrade().map_or(-1, |app| {
            cells::layout(&app, cell, cols, count, &heads(&h), shape).start_at(y)
        })
    });
    let weak = app.as_weak();
    app.on_grid_max_scroll(move |height, cell, cols, count, top, h, shape| {
        weak.upgrade().map_or(0.0, |app| {
            cells::layout(&app, cell, cols, count, &heads(&h), shape).max_scroll(height, top)
        })
    });
    let weak = app.as_weak();
    app.on_grid_reveal_to(
        move |offset, height, cell, cols, count, selected, top, h, shape| {
            weak.upgrade().map_or(0.0, |app| {
                cells::layout(&app, cell, cols, count, &heads(&h), shape)
                    .reveal(offset, height, selected, top)
            })
        },
    );
}

/// The strip's layout by the rows' shapes, asked of the arithmetic in
/// `strip`: where each frame's cell is, how long the row is, what a
/// scroll shows, and the scroll that shows a frame. The window's own
/// geometry is set from the same constants.
///
/// The layout's number comes with each call so a binding that asks
/// follows the shapes as they come; the answer is the layout as it
/// stands.
pub(crate) fn strip_layout(app: &App) {
    app.set_strip_photo_default(strip::PHOTO);
    app.set_strip_photo_least(strip::LEAST_PHOTO);
    app.set_strip_ring(strip::RING);
    app.set_strip_pad(strip::PAD);
    // A drag on the strip's edge, or a window that holds it lower: the
    // layout's number moves on so the cells ask again, and the strip
    // is held still about the selected frame as it is under new
    // shapes. The pictures follow in `strip-range`, which the hold
    // reports to.
    let weak = app.as_weak();
    app.on_strip_sized(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let before = cells::strip_resized(&app);
        cells::renumber(&app);
        match before {
            Some(before) => hold_strip(&app, &before),
            None => app.invoke_strip_hold(app.get_strip_scroll()),
        }
    });
    let weak = app.as_weak();
    app.on_strip_place(move |row, _| {
        weak.upgrade()
            .and_then(|app| {
                let layout = cells::strip_layout(&app);
                let (x, width) = layout.place(row)?;
                let (inset, photo) = layout.photo(row)?;
                Some(StripPlace {
                    x,
                    width,
                    inset,
                    photo,
                })
            })
            .unwrap_or_default()
    });
    let weak = app.as_weak();
    app.on_strip_length(move |_| {
        weak.upgrade()
            .map_or(2.0 * strip::PAD, |app| cells::strip_layout(&app).length())
    });
    let weak = app.as_weak();
    app.on_strip_shown(move |offset, view, _| {
        let shown = weak
            .upgrade()
            .and_then(|app| cells::strip_layout(&app).shown(offset, view));
        let (first, last) = shown.unwrap_or((-1, -1));
        StripSpan { first, last }
    });
    let weak = app.as_weak();
    app.on_strip_reveal_to(move |offset, view, row, _| {
        weak.upgrade().map_or(offset, |app| {
            cells::strip_layout(&app).reveal(offset, view, row)
        })
    });
}

/// The strip held still under the shapes that made `before` into the
/// layout it has now ([`strip::held`]), and what it shows said again
/// whether or not the scroll moved: narrower cells bring rows on
/// screen at the same scroll, and those need cells and pictures.
fn hold_strip(app: &App, before: &strip::Layout) {
    let view = app.get_strip_view();
    if view <= 0.0 {
        return;
    }
    let after = cells::strip_layout(app);
    let scroll = app.get_strip_scroll();
    app.invoke_strip_hold(strip::held(
        before,
        &after,
        scroll,
        view,
        app.get_selected(),
    ));
}

/// The `shape` the window passes the layout's questions now: the
/// layout's number, or square in the lightbox.
pub(crate) fn grid_shape(app: &App) -> i32 {
    if app.get_lightbox() {
        cells::SQUARE
    } else {
        app.get_grid_shape()
    }
}

/// Frame `c`'s picture's shape as its cell shows it, height over
/// width, turned as `turns` turns it; zero while it has none.
fn aspect_of(st: &State, c: usize, turns: u8) -> f32 {
    match st.views.thumb_base.get(c) {
        Some(Some((w, h, _))) if *w > 0 && *h > 0 => {
            if turns % 2 == 1 {
                *w as f32 / *h as f32
            } else {
                *h as f32 / *w as f32
            }
        }
        _ => 0.0,
    }
}

/// Row `row`'s picture has come, or turned, at `aspect`: the grid lays
/// out again for it, once for whatever else comes in the same turn of
/// the event loop.
fn note_shape(app: &App, row: usize, aspect: f32) {
    if cells::note_aspect(app, row, aspect) {
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(app) = weak.upgrade() {
                reshape(&app);
            }
        });
    }
}

/// The shapes noted since the last layout laid out: the strip's cells
/// move, and its scroll with them so what is on screen stays
/// ([`strip::held`]). The grid's rows move, and
/// the sheet's scroll moves with them so what was on screen stays. A
/// sheet scrolled to its end stays at its end, as near it as shows the
/// selected frame whole when that was whole on screen; short of the
/// end, the selected frame on screen is the row held in place and is
/// shown whole again if its row grew; else the first row on screen is
/// held. A selected frame the wheel has cut at an edge is left cut:
/// bringing it back whole would undo the scroll, a notch at a time, as
/// long as pictures kept coming.
pub(crate) fn reshape(app: &App) {
    let seen = STATE.with(|s| {
        let state = s.borrow().clone()?;
        let st = state.try_borrow().ok()?;
        Some((st.views.grid_shown, grid_breaks(&st, app)))
    });
    let (shown, heads) = seen.unwrap_or((None, Vec::new()));
    let cell = app.get_tile_cell();
    let cols = grid::columns(app.get_grid_w(), cell);
    let count = app.get_thumbs().row_count() as i32;
    let height = app.get_grid_h();
    let top = grid::tiles_top(app.get_grid_tiles().row_count() as i32, cell, cols);
    let selected = app.get_selected();
    let on_screen = shown.is_some_and(|(f, l)| (f..=l).contains(&selected));
    let anchor = if on_screen {
        Some(selected)
    } else {
        shown.map(|(first, _)| first)
    };
    let laid = |shape: i32| cells::layout(app, cell, cols, count, &heads, shape);
    let before = laid(grid_shape(app));
    let strip_before = cells::strip_layout(app);
    if !cells::take_aspects(app) {
        return;
    }
    hold_strip(app, &strip_before);
    if !app.get_grid_open() || app.get_lightbox() {
        return;
    }
    let after = laid(grid_shape(app));
    let scroll = app.get_grid_scroll();
    let at_end = scroll > 0.0 && scroll >= before.max_scroll(height, top) - 0.5;
    // Whole on screen before the rows moved: the reveal, which brings
    // it back whole, is for that frame alone.
    let whole = on_screen && before.reveal(scroll, height, selected, top) == scroll;
    let to = if at_end && whole {
        after.reveal(after.max_scroll(height, top), height, selected, top)
    } else if at_end {
        after.max_scroll(height, top)
    } else {
        let y = |l: &grid::Layout| anchor.and_then(|a| l.place(a)).map(|p| p.y);
        let moved = match (y(&before), y(&after)) {
            (Some(was), Some(now)) if scroll > 0.0 => scroll + now - was,
            _ => scroll,
        };
        if whole {
            after.reveal(moved, height, selected, top)
        } else {
            moved
        }
    };
    // The window's scroll, held to the new range, and what the grid
    // shows said again: shorter rows bring more of them on screen.
    app.invoke_scroll_by(to - scroll);
}

/// Where the grid's headings stand, by row: none in the lightbox, which
/// lays the frames out as they come.
pub(crate) fn grid_breaks(st: &State, app: &App) -> Vec<i32> {
    if app.get_lightbox() {
        return Vec::new();
    }
    st.views.grid_heads.iter().map(|h| h.row as i32).collect()
}

/// The rows `first..=last` are on screen, of the strip or the grid: the
/// worker has the cells of the rows near them for the pictures it makes,
/// and they are kept for a list made again under them.
pub(crate) fn near_shown(st: &mut State, worker: &Worker, first: usize, last: usize) {
    st.views.near_rows = Some((first, last));
    send_near(st, worker);
}

/// The cells near the rows on screen to the worker, and the reach ahead
/// while culling, as the list stands now. Nothing near is said before
/// the strip or the grid has said what it shows: until then the worker
/// makes every thumbnail, the frames shown first, rather than none.
pub(crate) fn send_near(st: &State, worker: &Worker) {
    if let Some(cells) = near_cells(st) {
        worker.near_thumbnails(cells);
    }
    previews_ahead(st, worker);
}

/// How many rows either side of those on screen culling has the local
/// previews made for.
pub(crate) const PREVIEWS_AHEAD: usize = 200;

/// Culling, in the Cull view or the lightbox, goes through the frames
/// one after another, so the pool makes the local previews of the rows
/// within [`PREVIEWS_AHEAD`] of those on screen there, the nearest
/// first; elsewhere only those near the screen. Never the whole list:
/// the loupe reads a file that is there itself, so these are for a cull
/// that goes on once the root is away, and a root of thousands would
/// take an hour of the machine for them.
pub(crate) fn previews_ahead(st: &State, worker: &Worker) {
    let culling = st.culling.cull.is_some() || st.culling.lightbox.is_some();
    worker.previews_ahead(culling.then(|| ahead_cells(st)));
}

/// The cells of the rows within [`PREVIEWS_AHEAD`] of those last on
/// screen, or of the selected frame's row before any was: by row, so a
/// filter's hidden cells are never among them.
pub(crate) fn ahead_cells(st: &State) -> std::collections::HashSet<usize> {
    let rows = st.views.near_rows.or_else(|| {
        let c = st.current?;
        let row = st.views.shown.iter().position(|&s| s == c)?;
        Some((row, row))
    });
    let Some((first, last)) = rows else {
        return Default::default();
    };
    let end = last
        .saturating_add(PREVIEWS_AHEAD + 1)
        .min(st.views.shown.len());
    (first.saturating_sub(PREVIEWS_AHEAD).min(end)..end)
        .map(|row| st.views.shown[row])
        .collect()
}

/// How many rows either side of the row about to be selected count as
/// near before the strip or the grid has said what it shows: a large
/// grid's screenful about it.
const SEED_ROWS: usize = 12;

/// Before the strip or the grid has said what it shows, the window's
/// first list: the rows about `row`, the one about to be selected (the
/// first without one), handed to the worker as near, so its first
/// cold thumbnails are those and not the list's start. The cell to
/// make first, for the queue's order; none for an empty list or one
/// whose rows on screen are known.
pub(super) fn seed_near(st: &State, worker: &Worker, row: Option<usize>) -> Option<usize> {
    let len = st.views.shown.len();
    if st.views.near_rows.is_some() || len == 0 {
        return None;
    }
    let at = row.unwrap_or(0).min(len - 1);
    let rows = crate::thumbpool::near_rows(at.saturating_sub(SEED_ROWS), at + SEED_ROWS, len);
    worker.near_thumbnails(rows.map(|r| st.views.shown[r]).collect());
    Some(st.views.shown[at])
}

/// The cells of the rows near those last on screen
/// ([`crate::thumbpool::near_rows`]), as the list stands now: by row,
/// so a filter's hidden cells between two rows are never near. None
/// before any rows were on screen.
pub(crate) fn near_cells(st: &State) -> Option<std::collections::HashSet<usize>> {
    let (first, last) = st.views.near_rows?;
    Some(
        crate::thumbpool::near_rows(first, last, st.views.shown.len())
            .map(|row| st.views.shown[row])
            .collect(),
    )
}

/// The browser's list again: the filter over the flags, the strip's
/// and the grid's rows made afresh, their cells' pictures put on from
/// those already made, and the selection kept on its row, or put on
/// the nearest row when its frame is hidden (which the caller then
/// opens).
pub(crate) fn rebuild_browser(st: &mut State, app: &App) -> Option<usize> {
    if !std::mem::take(&mut st.views.index_pass_ready) {
        st.views.index_passed = crate::library::index_pass(st);
    }
    // The filter is over files; a file that passes brings every cell
    // of it, its own and its versions, in the list's order.
    let passing = st.views.filter.apply(&filter_frames(st));
    let mut passes = vec![false; st.files.len()];
    for f in passing {
        passes[f] = true;
    }
    st.views.shown = (0..st.cells.len())
        .filter(|&c| passes[st.cells[c].file])
        .collect();
    // Other rows under the ones on screen: other cells near them.
    if let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        send_near(st, &worker);
    }

    show_filter(st, app);
    // Under a root that is offline the tile is dimmed, and a picture
    // the cache did not have is no failure of the file's.
    let dimmed = !st.views.library.offline.is_empty();
    // A closed stack's top wears its count, an open one's members their
    // places in it.
    let stacked = crate::stacks::marks(&crate::stacks::listed(st));
    let thumbs = st
        .views
        .shown
        .iter()
        .map(|&c| {
            let file = st.cells[c].file;
            let offline = dimmed && crate::rows::is_offline(st, c);
            let stack = stacked.get(&st.cells[c]);
            Thumb {
                stack: stack.map_or(0, |m| m.count as i32),
                stack_place: stack.map_or(0, |m| m.place as i32),
                stack_open: stack.is_some_and(|m| m.open),
                offline,
                failed: st.views.thumb_failed[c] && st.views.thumb_base[c].is_none() && !offline,
                // The cell's own frame, by its file's hash and its
                // version.
                collected: crate::panel::collections::collected(st, c),
                paired: st.cells[c].version.is_none()
                    && st.views.companions.contains_key(&st.files[file]),
                version: st.version_name(c).unwrap_or_default().into(),
                ..thumb_for(&st.files[file], &st.sidecars[file].meta, frame_marks(st, c))
            }
        })
        .collect();
    // No row has a picture on it now. The rows the views have cells
    // for take theirs from the pixels in hand, a few screens' worth
    // however long the list: nothing is carried over from the old
    // rows, so nothing can land on a row renumbered under it.
    st.views.thumb_shown.iter_mut().for_each(|s| *s = None);
    st.views.pictured = [(0, 0); 2];
    cells::set_rows(app, thumbs);
    // The rows' shapes, from the pictures in hand.
    let aspects = st
        .views
        .shown
        .iter()
        .map(|&c| {
            if st.views.thumb_base[c].is_some() {
                aspect_of(st, c, thumb_turns(st, app, c).0)
            } else {
                0.0
            }
        })
        .collect();
    cells::set_aspects(app, aspects);
    settle_pictures(st, app);
    // The export queue's view: its headings over the rows, and its
    // tiles' marks while it runs.
    crate::panel::queue_view::show(st, app);

    app.set_reject_count(reject_count(st) as i32);
    // A smart collection is lit while the bar says its filter.
    crate::panel::collections::show(st, app);
    // The Sort menu says the view's order.
    crate::panel::sort::show(st, app);
    // A frame the filter now hides leaves the set: a key or a sync
    // must not reach a frame nobody can see is chosen.
    st.views.picked = selection::prune(&st.views.picked, &st.views.shown, st.current);
    show_set(st, app);
    let Some(c) = st.current else {
        app.set_selected(-1);
        return None;
    };
    match row_of(st, c) {
        Some(row) => {
            app.set_selected(row as i32);
            // The filter or the order may have put other frames beside
            // the open one: the worker's neighbors follow.
            neighbors_moved(st);
            None
        }
        None => {
            app.set_selected(-1);
            cull::nearest_row(&st.views.shown, c)
        }
    }
}

/// `grid_filled` over the browser's rows, with the state's fields
/// borrowed apart.
pub(crate) fn grid_filled_rows(
    shown: &[usize],
    thumb_made: &[u32],
    thumb_failed: &[bool],
    thumb_want: u32,
    grid_shown: Option<(i32, i32)>,
    gave_up: bool,
    app: &App,
) -> bool {
    if !app.get_grid_open() || gave_up {
        return true;
    }
    let Some((first, last)) = grid_shown else {
        return false;
    };
    (first.max(0) as usize..=last.max(0) as usize)
        .filter_map(|r| shown.get(r))
        .all(|&f| {
            thumb_failed.get(f) == Some(&true)
                || (thumb_made[f] > 0 && !grid::wants_bigger(thumb_made[f], thumb_want))
        })
}

/// Each file's edit, from its sidecar when there is one; the fresh
/// edit when sidecars are off, or none is there yet. Beside them,
/// which files are raws whose edit is still the default: their
/// learned-denoiser blend is seeded from the ISO by the worker on
/// their first open, since reading the ISO here would cost a
/// metadata probe a file (3 to 40 ms) on the launch of every folder.
///
/// What decides that is the sidecar's contents, not whether there is
/// one: a frame given a star in the browser has a sidecar with a
/// default edit in it, and it should still open at the blend its ISO
/// asks for.
#[cfg(test)]
pub(crate) fn load_sidecars(files: &[PathBuf], write_sidecars: bool) -> (Vec<Sidecar>, Vec<bool>) {
    files
        .iter()
        .map(|f| load_sidecar(f, write_sidecars))
        .unzip()
}

/// [`load_sidecar_said`] without the word on what went wrong, for the
/// tests.
#[cfg(test)]
pub(crate) fn load_sidecar(f: &Path, write_sidecars: bool) -> (Sidecar, bool) {
    let (sidecar, _) = load_sidecar_said(
        f,
        write_sidecars.then_some(greycard_edit::Placement::Beside),
        Default::default(),
    );
    let seed = write_sidecars
        && !greycard_core::picture::is_picture_path(f)
        && files::never_developed(&sidecar);
    (sidecar, seed)
}

/// Replace the file list with `dir`'s files, and select the last file
/// open if it lies there, else the first. The folder is listed and its
/// frames' sidecars found off the window's thread
/// ([`crate::roots::open_listing`]); an empty folder, or a path that is
/// not one, is said and the list left alone rather than emptied under
/// the user's feet.
pub(crate) fn open_folder(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    open_folder_on(state, app, worker, dir, None);
}

/// [`open_folder`], on `left` when the folder has it: the frame a place
/// on the history was left on (§594), else as an open by hand.
pub(crate) fn open_folder_on(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    dir: &Path,
    left: Option<PathBuf>,
) {
    // A folder opened by hand while the index is still opening wins
    // over the view the launch asked for: that view is not opened
    // over it once the index is open.
    state.borrow_mut().views.library.wanted = None;
    state.borrow_mut().views.visited.opened_by_hand();
    let last = settings::Settings::load().shared.last_file;
    let last = (!last.is_empty()).then(|| PathBuf::from(last));
    let select = match left {
        Some(left) => crate::roots::Select::Left(left, last),
        None => crate::roots::Select::Last(last),
    };
    crate::roots::open_listing(
        state,
        app,
        worker,
        crate::roots::Listing::Folder(dir.to_path_buf()),
        select,
    );
}

/// What the desktop asked to open, the Finder's double-click or Open
/// With: each path as the command line's path would be, a file on its
/// own and a folder's pictures, starting on the first.
pub(crate) fn open_paths(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, paths: &[PathBuf]) {
    let apart = state.borrow().views.jpegs_apart;
    let (files, refused) = files::list_paths(paths, apart);
    for e in &refused {
        tracing::warn!("{e:#}");
    }
    if files.is_empty() {
        let said = refused
            .first()
            .map_or_else(|| tr!("nothing to open"), |e| format!("{e:#}"));
        app.set_status(said.into());
        return;
    }
    // Files asked for by name, not a folder: a filter left on from
    // before must not hide what was just asked for.
    if paths.iter().all(|p| !p.is_dir()) && !state.borrow().views.filter.is_empty() {
        tracing::info!("browser filter cleared for the files asked for");
        state.borrow_mut().views.filter = filter::Filter::default();
        app.set_filter_text("".into());
    }
    // As for a folder opened by hand: the launch's view is not opened
    // over these once the index is open.
    state.borrow_mut().views.library.wanted = None;
    state.borrow_mut().views.visited.opened_by_hand();
    crate::roots::open_listing(
        state,
        app,
        worker,
        crate::roots::Listing::Files(files),
        crate::roots::Select::Row(0),
    );
}

/// How long a snapshot waits for the grid's pictures before it takes
/// the grid as it stands. A cold folder of a few hundred fills in a
/// few seconds; a picture that has not come by this is not coming.
pub(crate) const GRID_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The snapshot's wait for the grid is over: when it is still
/// waiting, the cells without their picture are named in the log and
/// the grid is taken as it stands. Says whether it gave up.
pub(crate) fn give_up_on_grid(st: &mut State, app: &App) -> bool {
    if st.launch.snapshot.is_none() || grid_filled(st, app) {
        return false;
    }
    // What the grid still waits for: a picture that never came, and
    // one that came too small for the cells and whose larger one has
    // not.
    let (mut missing, mut small) = (Vec::new(), Vec::new());
    if let Some((first, last)) = st.views.grid_shown {
        for f in
            (first.max(0) as usize..=last.max(0) as usize).filter_map(|r| st.views.shown.get(r))
        {
            if st.views.thumb_failed[*f] {
                continue;
            }
            if st.views.thumb_made[*f] == 0 {
                missing.push(file_name(st.path_of(*f)));
            } else if grid::wants_bigger(st.views.thumb_made[*f], st.views.thumb_want) {
                small.push(file_name(st.path_of(*f)));
            }
        }
    }
    tracing::warn!(
        "snapshot: after {} s the grid still waits for {} picture{} ({}) and {} larger one{} ({}); \
         taking it as it stands",
        GRID_WAIT.as_secs(),
        missing.len(),
        if missing.len() == 1 { "" } else { "s" },
        missing.join(", "),
        small.len(),
        if small.len() == 1 { "" } else { "s" },
        small.join(", ")
    );
    st.launch.grid_wait_over = true;
    app.window().request_redraw();
    true
}

/// Whether every frame the grid shows has its picture, at the size
/// the cells ask for. True whenever the grid is closed.
pub(crate) fn grid_filled(st: &State, app: &App) -> bool {
    grid_filled_rows(
        &st.views.shown,
        &st.views.thumb_made,
        &st.views.thumb_failed,
        st.views.thumb_want,
        st.views.grid_shown,
        st.launch.grid_wait_over,
        app,
    )
}

/// A file's name without its directory, for the panel and the log.
/// The mean, least and greatest of what a timed run measured, over
/// the steps alone: the first sample is the file the editor opened
/// on, decoded from nothing, and the rest are steps along the folder.
fn over_steps(what: &str, samples: &[f64]) -> String {
    let steps = &samples[1.min(samples.len())..];
    if steps.is_empty() {
        return format!("{what}: nothing measured");
    }
    let n = steps.len() as f64;
    let mean = steps.iter().sum::<f64>() / n;
    let min = steps.iter().copied().fold(f64::INFINITY, f64::min);
    let max = steps.iter().copied().fold(0.0, f64::max);
    format!(
        "{what} over {} steps: mean {mean:.1} ms, min {min:.1}, max {max:.1}; the first file {:.1} ms",
        steps.len(),
        samples.first().copied().unwrap_or(0.0)
    )
}

/// `--time-select`, on each frame that shows a stepped-to develop:
/// the milliseconds since the key, then the next step a tenth of a
/// second on, as a hand would arrow; after the last, the numbers on
/// the log and the terminal, and the editor quits. `time_cull`'s
/// twin, for the develop view, and it reports two numbers a step:
/// the key to the camera's picture standing in, and the key to the
/// develop that replaces it.
pub(crate) fn time_select(
    timing: &mut Option<(u32, Vec<f64>, Vec<f64>)>,
    app: &App,
    ms: f64,
    row: Option<usize>,
    count: usize,
) {
    let Some((left, to_picture, to_develop)) = timing.as_mut() else {
        return;
    };
    to_develop.push(ms);
    let at_end = row.is_some_and(|r| r + 1 >= count);
    if *left == 0 || at_end {
        for line in [
            over_steps("select to the camera picture", to_picture),
            over_steps("select to develop", to_develop),
        ] {
            tracing::info!("{line}");
            eprintln!("{line}");
        }
        *timing = None;
        let _ = slint::quit_event_loop();
        return;
    }
    *left -= 1;
    let app_weak = app.as_weak();
    // `GREYCARD_TIME_SELECT_GAP_MS` waits longer between a develop and
    // the next step, for measuring a step onto a frame whose base the
    // worker made while it waited.
    let gap = std::env::var("GREYCARD_TIME_SELECT_GAP_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(100);
    let gap = std::time::Duration::from_millis(gap);
    // `GREYCARD_TIME_SELECT_SORT_AT=N:E` picks the Sort menu's entry E
    // once the Nth develop has landed and waits the gap again before
    // the step, for measuring a step after a sort change.
    let sort = std::env::var("GREYCARD_TIME_SELECT_SORT_AT")
        .ok()
        .and_then(|v| {
            let (at, entry) = v.trim().split_once(':')?;
            Some((at.parse::<usize>().ok()?, entry.parse::<i32>().ok()?))
        })
        .filter(|&(at, _)| at == to_develop.len());
    slint::Timer::single_shot(gap, move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some((_, entry)) = sort else {
            app.invoke_step(1, false);
            return;
        };
        tracing::info!("time select: the Sort menu's entry {entry} picked");
        app.invoke_sort_picked(entry);
        let app_weak = app.as_weak();
        slint::Timer::single_shot(gap, move || {
            if let Some(app) = app_weak.upgrade() {
                app.invoke_step(1, false);
            }
        });
    });
}

/// File `file`'s path changed from `was` to `to` under the window (found
/// moved, followed to an archive's copy, a move made), its bytes the
/// same: everything the window keeps of it by path follows in the same
/// step ([`follow_paths`] names each). The frame on screen's file is the frame's
/// still, and what a job the worker was running on it says under `was`
/// is taken as the frame's (`moved_from`). The worker, which holds the
/// file by the path
/// it was sent, is not told: an open it was sent under `was` ends with
/// the move (`open_sent`, so no develop is asked of a path the worker
/// does not hold), and the frame is opened again under `to` by the
/// follow's caller (`reopen_moved`), which sets every path the worker
/// keeps of it in one job.
pub(crate) fn file_moved(st: &mut State, file: usize, was: &Path, to: &Path) {
    let on_screen = st.current.and_then(|c| st.file_at(c)) == Some(file);
    follow_paths(st, was, to);
    crate::panel::painted::file_moved(st, was, to);
    let d = &mut st.develop;
    if on_screen {
        // Moved again: every earlier path is still what a job then
        // running may say.
        let mut earlier = match d.moved_from.take() {
            Some((at, earlier)) if at == was => earlier,
            _ => Vec::new(),
        };
        if !earlier.iter().any(|p| p == was) {
            earlier.push(was.to_path_buf());
        }
        d.moved_from = Some((to.to_path_buf(), earlier));
    }
    if d.open_sent.as_ref().is_some_and(|(_, (p, _))| p == was) {
        d.open_sent = None;
        d.reopen_due = true;
    }
}

/// The owners' fields that name a file by its path, followed from `was`
/// to `to` for [`file_moved`]. Every owner is taken apart whole, with no
/// `..`: a field added to one does not compile until it is followed here
/// or named as one that need not be.
fn follow_paths(st: &mut State, was: &Path, to: &Path) {
    use crate::owners::{Culling, Develop, Launch, Sheets, Views, Window, Work};
    use std::collections::{HashMap, HashSet};
    fn follow(p: &mut PathBuf, was: &Path, to: &Path) {
        if p == was {
            *p = to.to_path_buf();
        }
    }
    // What is kept at `to` with nothing at `was` was a file's gone from
    // there, and goes: the file moved in is not that one.
    fn follow_key<V>(m: &mut HashMap<PathBuf, V>, was: &Path, to: &Path) {
        match m.remove(was) {
            Some(v) => {
                m.insert(to.to_path_buf(), v);
            }
            None => {
                m.remove(to);
            }
        }
    }
    fn follow_member(s: &mut HashSet<PathBuf>, was: &Path, to: &Path) {
        if s.remove(was) {
            s.insert(to.to_path_buf());
        } else {
            s.remove(to);
        }
    }
    let State {
        // `State::move_file` follows the list; the rest is by file or
        // cell, or a sheet's own.
        files: _,
        cells: _,
        current: _,
        sidecars: _,
        from_row: _,
        camera_match: _,
        import: _,
        lightroom: _,
        rawtherapee: _,
        archive: _,
        launch:
            Launch {
                overridden,
                // A folder, a capture's file, or no path.
                last_folder: _,
                not_reopened: _,
                screenshot: _,
                snapshot: _,
                tweak_at_start: _,
                time_sharpen: _,
                time_clarity: _,
                time_dehaze: _,
                time_texture: _,
                time_cull: _,
                time_select: _,
                time_scroll: _,
                cull_develop: _,
                snapshot_placeholder: _,
                turn_at_start: _,
                awaiting_turn: _,
                cull_key_at_start: _,
                ask_rejects: _,
                move_rejects: _,
                cull_at_start: _,
                developed_at_start: _,
                scripted: _,
                owned: _,
                facets_wanted: _,
                awaiting_index: _,
                select_at_start: _,
                overrides_at_start: _,
                setup_ran: _,
                also_at_start: _,
                preview_temperature: _,
                show_mask: _,
                show_patch: _,
                opened: _,
                zoom_stands: _,
                grid_wait_over: _,
                panel_scroll: _,
                snapshot_shown: _,
                failed: _,
            },
        window:
            Window {
                // The settings file, or no path.
                settings_file: _,
                write_sidecars: _,
                xmp_sidecars: _,
                placement: _,
                lens_on_new: _,
                author: _,
                copyright: _,
                deletes_allowed: _,
                batch: _,
                app: _,
            },
        sheets:
            Sheets {
                // Folders.
                trash_refused: _,
                // Planned on the list as it stood: a delete out holds
                // the frames it names.
                deleting: _,
                // Their own types, which follow what they keep or let
                // a path that no longer matches go.
                delete_asked: _,
                version_delete: _,
                look_remove: _,
                look_rename: _,
                look_renaming: _,
                time_shift: _,
                fetch: _,
                runtime_next: _,
                models: _,
                preset_checks: _,
                preset_rows: _,
                sync_checks: _,
                sync_rows: _,
                sync_last: _,
                sync_source: _,
                clipboard: _,
                mask_clipboard: _,
                // No path.
                look_remove_generation: _,
                look_removing: _,
                look_rename_generation: _,
                fetching: _,
                declined: _,
                remove_offered: _,
                edge_declined: _,
                edge_offer_due: _,
                edge_loaded: _,
                fetch_failed: _,
                lenses_declined: _,
                lenses_asked: _,
                sync_asked: _,
                paste_asked: _,
                frame_sync_waiting: _,
            },
        work:
            Work {
                merge_edits,
                // `sync::Sync::file_moved`, taken apart there.
                sync,
                // Folders, the queue's file, or no path.
                export_looking: _,
                export_queue_file: _,
                export_last_folder: _,
                export_choosing: _,
                export_checks: _,
                queue_probes: _,
                queue_busy: _,
                merge_relist: _,
                // Their own types, which follow what they keep or let
                // a path that no longer matches go.
                exporting: _,
                export_one_line: _,
                export_pending: _,
                export_queue: _,
                queue_run: _,
                export_presets: _,
                edit_in: _,
                saves: _,
                // By the frame's path, followed below.
                unsaved,
            },
        culling:
            Culling {
                // By cell, or their own types.
                cull: _,
                cull_move_on: _,
                loupe_cull: _,
                show_developed: _,
                developed: _,
                notice_timer: _,
                badge_timer: _,
                tags: _,
                hold: _,
                placeholder: _,
                prefetch: _,
                lightbox: _,
                lightbox_fetch: _,
                compare_tiles: _,
                // By path, and never by cell: a path the list no longer
                // holds is not the current frame's, so `keep_last_file`
                // lets the owe go, and a moved or deleted one fails to
                // resolve and is skipped; the next choice owes the frame
                // then open.
                drawn: _,
                owed: _,
            },
        views:
            Views {
                reframed,
                panel_stand_in,
                // `State::move_file`.
                companions: _,
                collection_frames: _,
                // Matched against the list when its read lands.
                pick_pending: _,
                // The index's file.
                index_path: _,
                // By cell or file, or no path.
                jpegs_apart: _,
                queue_cell_ids: _,
                queue_relist_due: _,
                cells_by_file: _,
                picked: _,
                shown: _,
                pictured: _,
                index_ids: _,
                index_passed: _,
                index_generation: _,
                index_progress: _,
                index_error: _,
                index_tries: _,
                index_pass_ready: _,
                sort_reveal: _,
                view_generation: _,
                reading_line: _,
                thumb_base: _,
                thumb_shown: _,
                thumb_made: _,
                thumb_failed: _,
                thumb_asked: _,
                thumb_want: _,
                making_said: _,
                near_rows: _,
                fill_clock: _,
                fill_over: _,
                selected_at: _,
                grid_shown: _,
                strip_shown: _,
                menu_file: _,
                // Their own types, which follow what they keep or let
                // a path that no longer matches go.
                queue_listed: _,
                grid_heads: _,
                queued_open: _,
                before_queue: _,
                back_on_land: _,
                stacks_listed: _,
                stacks: _,
                filter: _,
                index: _,
                index_reader: _,
                facets_last: _,
                ranges_last: _,
                library: _,
                collections: _,
                collections_held: _,
                recent: _,
                visited: _,
                tree: _,
                view: _,
                sort: _,
                loads: _,
                thumb_run: _,
                words: _,
            },
        develop:
            Develop {
                stances,
                mosaics,
                monochromes,
                opened_sizes,
                panel_mosaic,
                learned_of,
                masks_failed,
                lens_frame,
                crop_fit_due,
                before_as_shot,
                zoom_back,
                walk,
                // `painted::file_moved`, and `file_moved` after this.
                painted: _,
                moved_from: _,
                open_sent: _,
                reopen_due: _,
                // Compared with the open frame where read, and let go
                // or sent again when it no longer matches.
                focus_pending: _,
                original: _,
                neighbors_sent: _,
                people_all: _,
                // No path, or the open frame's by key or by look.
                edit: _,
                pending: _,
                base_white: _,
                frame: _,
                drawn_kind: _,
                drawn_guide: _,
                drawn_generation: _,
                drawn_planes: _,
                picture_asked: _,
                as_shot_white: _,
                white_cache: _,
                shown_white: _,
                debounce: _,
                save_timer: _,
                image_size: _,
                zoom: _,
                mat: _,
                center: _,
                laid: _,
                stood_view: _,
                zoom_free: _,
                renderer: _,
                gpu: _,
                start_note: _,
                bins: _,
                scope: _,
                hover: _,
                hover_shown: _,
                curve_drag: _,
                source_size: _,
                shown_size: _,
                shown_turn: _,
                drawn_camera: _,
                drawn_focus: _,
                turn_pressed: _,
                crop_drag: _,
                crop_draw: _,
                crop_start: _,
                crop_last: _,
                crop_start_due: _,
                fill_choice: _,
                target: _,
                fresh_mask: _,
                placing: _,
                brush_row_cuts: _,
                choosing: _,
                part_pending: _,
                landscape_pending: _,
                landscape_offer: _,
                part_asks: _,
                part_dismissed: _,
                picking: _,
                wheel_rest: _,
                mask_drag: _,
                rasters: _,
                refined: _,
                retouching: _,
                patch_stroke: _,
                fill_resized: _,
                fill_engines: _,
                patch_drag: _,
                patch_handles: _,
                patch_shape: _,
                lit_shape: _,
                fills_pending: _,
                highlight_pending: _,
                fill_takes: _,
                pending_shapes: _,
                guiding: _,
                guide_kept: _,
                learned: _,
                asked: _,
                learned_version: _,
                reasks: _,
                quiet: _,
                store: _,
                lens: _,
                lens_options: _,
                profiles: _,
                profiles_kept: _,
                camera: _,
                other_names: _,
                looks: _,
                look: _,
                look_for: _,
                profile_tone: _,
                profile_tone_for: _,
                look_groups_open: _,
                profile_groups_open: _,
                look_hover: _,
                white_profile: _,
                mask_handles: _,
                mask_boxes: _,
                mask_picks: _,
                person_boxes: _,
                monitors: _,
                lut_for: _,
                encoded_lut_for: _,
                presets: _,
                preset_store: _,
                preset_favorites: _,
                preset_folds: _,
                preset_group_asked: _,
                status_after_develop: _,
                hint: _,
                peek: _,
                held: _,
                status_kept: _,
                view_kept: _,
                show_mask_kept: _,
                look_seen: _,
                planes_wants: _,
                planes_asked: _,
                generation: _,
            },
    } = st;
    follow_key(stances, was, to);
    follow_key(mosaics, was, to);
    follow_key(merge_edits, was, to);
    follow_member(monochromes, was, to);
    follow_member(reframed, was, to);
    unsaved.file_moved(was, to);
    for (p, _) in opened_sizes.iter_mut() {
        follow(p, was, to);
    }
    for p in masks_failed.values_mut().filter_map(|(_, p)| p.as_mut()) {
        follow(p, was, to);
    }
    let frame = zoom_back.as_mut().and_then(|b| b.frame.as_mut());
    for p in [
        panel_mosaic.as_mut().map(|(p, _)| p),
        learned_of.as_mut(),
        crop_fit_due.as_mut().map(|(p, _)| p),
        overridden.as_mut().map(|(p, _)| p),
        panel_stand_in.as_mut(),
        lens_frame.as_mut().map(|k| &mut k.path),
        before_as_shot.as_mut().map(|(k, _)| &mut k.path),
        frame.map(|(k, _)| &mut k.path),
    ]
    .into_iter()
    .flatten()
    {
        follow(p, was, to);
    }
    walk.file_moved(was, to);
    sync.file_moved(was, to);
}

/// The files at `paths` gone from the list, deleted or moved away: what
/// the session learned of them by path goes with them, and the Fill's
/// strokes waiting on them (`painted::drop_files`).
pub(crate) fn files_gone(st: &mut State, paths: &[PathBuf]) {
    for p in paths {
        st.develop.stances.remove(p);
        st.develop.mosaics.remove(p);
        st.develop.monochromes.remove(p);
        st.views.reframed.remove(p);
    }
    crate::panel::painted::drop_files(st, paths);
}

/// Whether `file`, as an outcome of the worker names it, is the frame
/// on screen's: its path, or a path it was followed from in this visit,
/// which a job running on it when it moved still says.
pub(crate) fn is_open_file(st: &State, file: &Path) -> bool {
    let open = st.current.and_then(|c| st.path_at(c));
    open.map(PathBuf::as_path) == Some(file) || open.is_some_and(|o| was_at(st, o, file))
}

/// Whether the frame on screen, at `now`, was at `file` earlier in this
/// visit, its file followed from there.
pub(crate) fn was_at(st: &State, now: &Path, file: &Path) -> bool {
    st.develop
        .moved_from
        .as_ref()
        .is_some_and(|(at, earlier)| at == now && earlier.iter().any(|p| p == file))
}

/// The frames around cell `i` in the order their bases are to be made
/// ([`beside`], deeper the way the strip is walked), as an open of each
/// would be sent: each under the edit and the turn its sidecar gives.
/// A frame whose sidecar still stands in from the index's row is sent
/// under the stand-in's default edit
/// when the row says the sidecar holds no develop and no versions,
/// which is what the read will give (a wrong guess only misses the
/// shelf), its turn read off the row's. Any other stand-in (its row
/// says it is edited, holds versions, or shows a mirror) is sent to be
/// read first ([`crate::worker::ReadFirst`]): the worker reads its
/// sidecar in the prefetch's own idle slot, so nothing queues ahead of
/// the user's requests or puts the window's bar up, and nothing lands
/// in the window's state. Left out, and said in the log: a frame under
/// an offline root, which has nothing to decode, and a stand-in to be
/// read first whose read is not the worker's to make
/// ([`not_read_first`]).
pub(crate) fn neighbors_of(st: &State, i: usize) -> Vec<crate::worker::Neighbor> {
    neighbors_said(st, i, true)
}

/// [`neighbors_of`], with the ones left out said in the log when `say`.
fn neighbors_said(st: &State, i: usize, say: bool) -> Vec<crate::worker::Neighbor> {
    beside(&st.views.shown, i, st.develop.walk.ahead)
        .into_iter()
        .filter_map(|(c, side, steps)| {
            let path = st.path_at(c)?.clone();
            // Only the nearest each way: any shelf with room makes those,
            // where the farther may be beyond the depth it allows.
            let left_out = |why: &str| {
                if say && steps == 1 {
                    tracing::info!(
                        "the prefetch of the {} frame, {}, skipped: {why}",
                        side.at(steps),
                        file_name(&path)
                    );
                }
            };
            if crate::rows::is_offline(st, c) {
                left_out("its root is offline");
                return None;
            }
            let turn = if crate::rows::is_loaded(st, c) {
                Some(st.sidecar_at(c)?.turn)
            } else if let Some(turn) = unedited_stand_in(st, c) {
                Some(turn)
            } else if let Some(why) = not_read_first(st, c) {
                left_out(why);
                return None;
            } else {
                None
            };
            // In the export queue's view, a neighbor as its Loupe will
            // open it: under its queued edit.
            if let Some(q) = crate::panel::queue_view::frame_of(st, c) {
                return Some(crate::worker::Neighbor {
                    edit: q.edit.clone(),
                    path,
                    turn: q.turn,
                    seed_blend: q.seed_blend,
                    side,
                    steps,
                    read_first: None,
                });
            }
            let Some(turn) = turn else {
                // The edit, the turn, and the seed stand in until the
                // worker's read.
                return Some(crate::worker::Neighbor {
                    edit: st.line(c).map(|l| l.current.clone()).unwrap_or_default(),
                    path,
                    turn: 0,
                    seed_blend: false,
                    side,
                    steps,
                    read_first: Some(crate::worker::ReadFirst {
                        version: st.cell(c)?.version.clone(),
                    }),
                });
            };
            Some(crate::worker::Neighbor {
                edit: st.line(c)?.current.clone(),
                path,
                turn,
                seed_blend: crate::rows::wants_seed(st, c),
                side,
                steps,
                read_first: None,
            })
        })
        .collect()
}

/// What the worker was last told of the open frame's neighbors: the
/// open they went with, the neighbors as sent, and the filter and the
/// order the strip had then, which say in the log what moved them.
pub(crate) struct NeighborsSent {
    generation: u64,
    open: PathBuf,
    neighbors: Vec<crate::worker::Neighbor>,
    filter: String,
    sort: crate::sort::Choice,
}

impl NeighborsSent {
    /// The open they were sent with, for a test.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The neighbors of cell `c` as they are now, sent with the open at
    /// `generation`.
    pub(crate) fn of(st: &State, c: usize, generation: u64) -> Self {
        Self {
            generation,
            open: st.path_at(c).cloned().unwrap_or_default(),
            neighbors: neighbors_said(st, c, false),
            filter: st.views.filter.describe(),
            sort: st.views.sort.choice(&st.views.view),
        }
    }

    /// Whether the worker would make the same bases from `now` as from
    /// these: the same frames on the same sides, at the same turns and
    /// seeds, under edits with the same base. A move after the base on a
    /// neighbor (its exposure, a rating) is not worth sending again.
    fn serves(&self, now: &[crate::worker::Neighbor]) -> bool {
        self.neighbors.len() == now.len()
            && self.neighbors.iter().zip(now).all(|(a, b)| {
                a.path == b.path
                    && a.side == b.side
                    && a.steps == b.steps
                    && a.turn % 4 == b.turn % 4
                    && a.seed_blend == b.seed_blend
                    && a.read_first == b.read_first
                    && same_base(&a.edit, &b.edit)
            })
    }

    /// Whether `now` names the same frames on the same sides.
    fn same_frames(&self, now: &[crate::worker::Neighbor]) -> bool {
        self.neighbors.len() == now.len()
            && self
                .neighbors
                .iter()
                .zip(now)
                .all(|(a, b)| a.path == b.path && a.side == b.side && a.steps == b.steps)
    }
}

/// Whether a neighbor's base would be the same under `a` as under `b`,
/// so far as the window can tell without the frame: the sections a base
/// is made from. The worker's own test is the frame's list, after the
/// decode; this one only saves sending what has not moved.
fn same_base(a: &Edit, b: &Edit) -> bool {
    a.white_balance == b.white_balance
        && a.noise == b.noise
        && a.demosaic == b.demosaic
        && a.highlight_fill == b.highlight_fill
        && a.camera == b.camera
        && a.lens == b.lens
}

/// The open frame's neighbors sent to the worker again when they are no
/// longer what it was told: the filter or the order moved the strip
/// under the frame, or a neighbor's edit changed its base. Once per
/// change, since what was sent is kept and compared; only while the
/// frame is open in the editor and still in the strip (a frame the
/// filter hid is left for the nearest, whose open sends its own), and
/// only for the open they went with, which the worker checks against
/// the open whose develop landed.
pub(crate) fn neighbors_moved(st: &mut State) {
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    let Some(c) = st.current.filter(|_| st.culling.cull.is_none()) else {
        return;
    };
    if !worker.edited().any_open() || row_of(st, c).is_none() {
        return;
    }
    let Some(sent) = st.develop.neighbors_sent.as_ref() else {
        return;
    };
    if st.path_at(c) != Some(&sent.open) {
        return;
    }
    let now = neighbors_said(st, c, false);
    if sent.serves(&now) {
        return;
    }
    let fresh = NeighborsSent::of(st, c, sent.generation);
    let why = if fresh.filter != sent.filter {
        "a filter change"
    } else if fresh.sort != sent.sort {
        "a sort change"
    } else if sent.same_frames(&now) {
        "a neighbor's edit changed"
    } else {
        "a change to the list"
    };
    // Kept as sent only when the worker took them: one refused (the
    // open's list let go before its develop landed) is offered again at
    // the next change rather than taken for the worker's.
    let (generation, open) = (fresh.generation, fresh.open.clone());
    let heading = st.develop.walk.heading();
    if worker.neighbors_again(generation, &open, heading, neighbors_of(st, c), why) {
        st.develop.neighbors_sent = Some(fresh);
    }
}

/// The editor back on cell `c` from culling or the grid without an
/// open, which let its neighbors go: sent again as the strip has them
/// now, whatever moved meanwhile.
pub(crate) fn neighbors_back(st: &mut State, worker: &crate::worker::Worker, c: usize) {
    let Some(generation) = st.develop.neighbors_sent.as_ref().map(|s| s.generation) else {
        return;
    };
    let Some(open) = st.path_at(c).cloned() else {
        return;
    };
    if worker.neighbors_again(
        generation,
        &open,
        st.develop.walk.heading(),
        neighbors_of(st, c),
        "the return to the editor",
    ) {
        st.develop.neighbors_sent = Some(NeighborsSent::of(st, c, generation));
    }
}

/// Why the worker is not to read the sidecar of cell `c`'s frame, which
/// stands in from its row, for a prefetch; none when it may. Not with
/// sidecars off, when the window's open reads none either; and not once
/// a read of it has failed, when the open opens it as it stands in. A
/// frame in a folder on a network mount is the worker's to pass over,
/// which asks each folder's mount itself (`Worker::set_remote_check`).
fn not_read_first(st: &State, c: usize) -> Option<&'static str> {
    if st.window.sidecars().is_none() {
        return Some("its sidecar is not read, and sidecars are off");
    }
    if st
        .file_at(c)
        .and_then(|f| st.from_row.get(f))
        .is_some_and(|r| r.unread.is_some())
    {
        return Some("its sidecar did not read");
    }
    None
}

/// The quarter turns of cell `c`'s frame when it stands in from a row
/// that says its sidecar holds no develop and no versions, read off the
/// turns the row shows its picture at: with the geometry the default,
/// those are the frame's own turn taken back. `None` for any other
/// cell: a version, an edited row, or one that shows a mirror.
fn unedited_stand_in(st: &State, c: usize) -> Option<u8> {
    let cell = st.cell(c)?;
    let row = st.from_row.get(cell.file)?;
    if cell.version.is_some() || row.edited || !row.versions.is_empty() {
        return None;
    }
    match row.turns {
        (shown, false) => Some((4 - shown % 4) % 4),
        (_, true) => None,
    }
}

/// Which way the strip is being walked: `ahead`, the side the last
/// steps went, and how many steps in a row have gone the other way
/// since. One step back is a look at the frame just left, which is on
/// the shelf; the walk turns on the second, so a back-and-forth compare
/// does not send the prefetch to and fro. `streak` is how many steps
/// in a row have just gone ahead, which a shelf with room for one base
/// waits on. `from` is the cell last opened in the editor and its file,
/// which a step is counted from.
#[derive(Clone, Debug)]
pub(crate) struct Walk {
    pub(crate) ahead: crate::worker::Side,
    against: u8,
    streak: u8,
    from: Option<(usize, PathBuf)>,
}

impl Default for Walk {
    fn default() -> Self {
        Self {
            ahead: crate::worker::Side::Next,
            against: 0,
            streak: 0,
            from: None,
        }
    }
}

/// The steps the other way that turn a walk.
const TURNS_AFTER: u8 = 2;

impl Walk {
    /// A step to the frame on `side` of the last, or `None` for a jump,
    /// which keeps the direction and forgets the steps against it.
    pub(crate) fn step(&mut self, side: Option<crate::worker::Side>) {
        match side {
            Some(side) if side == self.ahead => {
                self.against = 0;
                self.streak = self.streak.saturating_add(1);
            }
            Some(side) => {
                self.against += 1;
                self.streak = 0;
                if self.against >= TURNS_AFTER {
                    // Turned by as many steps in a row the new way.
                    self.ahead = side;
                    self.against = 0;
                    self.streak = TURNS_AFTER;
                }
            }
            None => {
                self.against = 0;
                self.streak = 0;
            }
        }
    }

    /// The walk as the worker is told it.
    pub(crate) fn heading(&self) -> crate::worker::Heading {
        crate::worker::Heading {
            ahead: self.ahead,
            streak: self.streak,
        }
    }
}

impl Walk {
    /// The file the walk was last at moved from `was` to `to`: the next
    /// step is counted from it still.
    fn file_moved(&mut self, was: &Path, to: &Path) {
        if let Some((_, p)) = self.from.as_mut()
            && p == was
        {
            *p = to.to_path_buf();
        }
    }
}

/// Cell `i` opened in the editor: the walk takes it as a step from the
/// cell opened before it when it is that cell's next or previous in the
/// strip, as a jump otherwise. The same cell opened again is neither.
pub(crate) fn walked_to(st: &mut State, i: usize) {
    let Some(path) = st.path_at(i).cloned() else {
        return;
    };
    let from = st
        .develop
        .walk
        .from
        .take()
        .filter(|(c, p)| st.path_at(*c) == Some(p))
        .map(|(c, _)| c);
    if from != Some(i) {
        let side = from.and_then(|f| step_side(&st.views.shown, f, i));
        st.develop.walk.step(side);
    }
    st.develop.walk.from = Some((i, path));
}

/// Which side of cell `from` cell `to` is, when it is the one the right
/// or the left key reaches from it among the rows `shown`.
pub(crate) fn step_side(shown: &[usize], from: usize, to: usize) -> Option<crate::worker::Side> {
    use crate::worker::Side;
    [Side::Next, Side::Previous]
        .into_iter()
        .find(|&side| reach(shown, from, side, 1) == Some(to))
}

/// The cell `steps` rows away from cell `current` on `side` among the
/// browser's rows `shown`: for one step, the one the right or the left
/// key lands on from it. From a cell the filter hides, the keys step
/// from the nearest row shown, as `step` does. Never the cell itself.
fn reach(shown: &[usize], current: usize, side: crate::worker::Side, steps: u8) -> Option<usize> {
    use crate::worker::Side;
    let (row, on) = match shown.binary_search(&current) {
        Ok(row) => (row, true),
        Err(_) => (cull::nearest_row(shown, current)?, false),
    };
    let last = shown.len().checked_sub(1)?;
    let further = usize::from(steps.max(1)) - 1;
    let at = match side {
        Side::Next => Some((row + 1).min(last) + further).filter(|&at| at <= last)?,
        Side::Previous => row.saturating_sub(1).checked_sub(further)?,
    };
    let c = shown[at];
    (!(at == row && on) && c != current).then_some(c)
}

/// The cells around cell `current` among the browser's rows `shown`,
/// with their side and how many steps away, the strip walked toward
/// `ahead`: in the order their bases are to be made, the nearest ahead
/// first so a single step finds its own, then the one behind (on a walk
/// the frame just left, so a step back finds it too), then the farther
/// ahead, as deep as [`crate::worker::DEEPEST`]. How many of them the
/// worker makes is the shelf's to say. The cell itself is never one of
/// them, nor any twice.
pub(crate) fn beside(
    shown: &[usize],
    current: usize,
    ahead: crate::worker::Side,
) -> Vec<(usize, crate::worker::Side, u8)> {
    let deepest = crate::worker::DEEPEST;
    let behind = ahead.other();
    let wanted = std::iter::once((ahead, 1))
        .chain((1..=deepest.behind).map(|k| (behind, k)))
        .chain((2..=deepest.ahead).map(|k| (ahead, k)));
    let mut out: Vec<(usize, crate::worker::Side, u8)> = Vec::new();
    for (side, steps) in wanted {
        if let Some(c) = reach(shown, current, side, steps)
            && !out.iter().any(|&(o, _, _)| o == c)
        {
            out.push((c, side, steps));
        }
    }
    out
}

/// An arrow's landing on browser row `row`, which `moves` when it is
/// not the row already current. A plain arrow opens it as a click
/// would and the set collapses to it, even at the end of the strip
/// where nothing moved; with Shift the set keeps what it had and
/// takes the frame landed on as well.
fn step_to(state: &Rc<Shared>, app: &App, worker: &Worker, row: i32, moves: bool, extend: bool) {
    match (moves, extend) {
        (true, false) => app.invoke_select(row),
        (true, true) => open_row(&mut state.borrow_mut(), app, worker, row, true),
        (false, false) => {
            let mut st = state.borrow_mut();
            st.views.picked = st.current.into_iter().collect();
            show_set(&mut st, app);
        }
        (false, true) => {}
    }
}

/// Culling's undo (`back`) or redo, and the one outside it when a
/// mark was the last thing done: the session's last rating, flag
/// or label change taken back or made again, and nothing else, with
/// the word over the picture saying what it left ("Undo: No stars"). The
/// row to select after, for the caller to open once the state is
/// free: the changed frame's, so what changed is on screen, or the
/// nearest when the filter no longer shows the current one. None
/// when there was nothing to step, or nothing to move to. With it the
/// word, for the status line where there is no word over the picture.
pub(crate) fn step_tags(st: &mut State, app: &App, back: bool) -> (Option<usize>, Option<String>) {
    step_tags_at(st, app, back, None)
}

/// [`step_tags`] on the step `at` (`None` the latest on its side):
/// Develop's, past marks on other frames, whole or the open frame's
/// share.
pub(crate) fn step_tags_at(
    st: &mut State,
    app: &App,
    back: bool,
    at: Option<tags::Reach>,
) -> (Option<usize>, Option<String>) {
    let step = match (back, at) {
        (true, None) => st.culling.tags.undo(),
        (false, None) => st.culling.tags.redo(),
        (_, Some(at)) => st.culling.tags.take(back, &at),
    };
    let Some(step) = step else {
        return (None, None);
    };
    let landing = tags::landing(
        &step,
        |p| st.file_by_path(p),
        |f| Tags::of(&st.sidecars[f].meta),
    );
    let files: Vec<usize> = landing.iter().map(|&(f, _)| f).collect();
    // The files' cells, every one, since the meta is shared.
    let cells: Vec<usize> = files.iter().flat_map(|&f| st.cells_of_file(f)).collect();
    let was_shown: Vec<bool> = cells.iter().map(|&c| row_of(st, c).is_some()).collect();
    // What the step does to each frame, in the key's terms, for
    // the word; read before the frames are put back.
    let landed: Vec<Option<meta::Change>> = landing
        .iter()
        .map(|(f, tags)| tags::between(&Tags::of(&st.sidecars[*f].meta), tags))
        .collect();
    let shifted: Vec<usize> = landing
        .iter()
        .filter(|(f, tags)| st.sidecars[*f].meta.time_shift != tags.time_shift)
        .map(|(f, _)| *f)
        .collect();
    for (f, tags) in landing {
        tags.put(&mut st.sidecars[f].meta);
    }
    let next = tags_shown(st, app, &cells, &was_shown, &files);
    if !shifted.is_empty() {
        crate::panel::time_shift::moved(st, app, &shifted);
    }
    // The step's word, as a key's: silent only when it found no
    // frame to put back.
    let said = (!landed.is_empty()).then(|| tags::step_notice(back, &landed));
    if let Some(word) = &said {
        say_notice(st, app, word.clone());
    }
    // Over to the frame that changed, if it is not the one on
    // screen and the filter still shows it.
    if !cells.iter().any(|&c| Some(c) == st.current)
        && let Some(row) = files
            .first()
            .and_then(|&f| st.file_cell(f))
            .and_then(|c| row_of(st, c))
    {
        return (Some(row), said);
    }
    (next, said)
}

/// A culling key's change over the whole selection, as the key and
/// the frame menu both ask for it, or a keyword or a title from the
/// METADATA section's fields; false when nothing is selected.
/// A frame that leaves the filtered list moves the selection on to
/// the nearest, once the state is free.
///
/// In culling the key's answer goes up over the picture as a word
/// (`say_notice`), and with the CULLING section's switch on the
/// selection moves on to the next frame, by the arrow's own path
/// (`step`) so it stops at the end as the arrow does. Not when the
/// frame left the filtered list, whose nearest is already the next;
/// and not over a set of several, where an arrow would collapse the
/// set that was just rated. The move is asked for after the change
/// is recorded, so undo's step names the frame that was rated and
/// not the one moved on to.
pub(crate) fn meta_on_selection(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    change: meta::Change,
) -> bool {
    // The whole selection: the current frame and the set, as it is
    // now. A request for their sidecars, and the change over exactly
    // those frames once they are in (`rows::request`): at once when
    // they are, in its turn otherwise.
    let paths: Vec<crate::versions::Key> = {
        let st = state.borrow();
        chosen_keys(&st)
    };
    meta_on_keys(state, app, worker, paths, change)
}

/// The selection as keys, which outlive a change to the list: what
/// a request names its frames by.
pub(crate) fn chosen_keys(st: &State) -> Vec<crate::versions::Key> {
    chosen_frames(st)
        .into_iter()
        .filter_map(|c| st.key_of(c))
        .collect()
}

/// [`meta_on_selection`] over the frames `paths` name, chosen or not:
/// for a title typed for a selection that has moved on since.
pub(crate) fn meta_on_keys(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    paths: Vec<crate::versions::Key>,
    change: meta::Change,
) -> bool {
    if paths.is_empty() {
        return false;
    }
    // Culling's move-on steps now, at the press, not when the key
    // lands: the request carries the frame the user saw, and the pick
    // moves to the next frame (a stand-in if unloaded, a request of its
    // own), so each key in a run of keys reaches its own frame. Stepped
    // when the key landed, every key pressed during the first frame's
    // read named that frame, and each stepped again.
    //
    // Not for words typed in the panel, nor a time shift from its
    // sheet: the switch is for the keys, and a keyword that moved the
    // selection on would leave the next one typed on another frame.
    //
    // In the Loupe's Cull, either picture, and in Library whatever
    // mode the Loupe is in: the grid has no mode of its own, and its
    // keys are culling's. The lightbox is a view apart, and moves on
    // only over Cull, as it always has.
    let move_on = {
        let st = state.borrow();
        let library = app.get_grid_open() && !app.get_lightbox();
        (crate::panel::cull::cull_mode(&st) || library)
            && st.culling.cull_move_on
            && paths.len() == 1
            && change.is_key()
    };
    // On the last row the step has nowhere to go, so a key that takes
    // the frame out of the filtered list leaves nothing on screen
    // unless the landing may put the selection on the nearest row
    // (§123's rule), as the key applied at once does below. So it may,
    // there: the end frame is the same whether the key applied at
    // once or waited for its read.
    let at_end = {
        let st = state.borrow();
        st.current
            .and_then(|c| row_of(&st, c))
            .is_some_and(|r| r + 1 == st.views.shown.len())
    };
    let may_select = !move_on || at_end;
    let what = match change {
        meta::Change::Rating(_) => crate::rows::Request::RatingKey,
        meta::Change::Flag(_) => crate::rows::Request::FlagKey,
        meta::Change::Label(_) => crate::rows::Request::LabelKey,
        meta::Change::AddKeywords(_) | meta::Change::RemoveKeyword(_) => {
            crate::rows::Request::Keyword
        }
        meta::Change::Title(_) => crate::rows::Request::Title,
        meta::Change::TimeShift(_) | meta::Change::ClearTimeShift => {
            crate::rows::Request::TimeShift
        }
    };
    // The meta is the file's: a key on a version's cell whose line a
    // read just found gone still rates the file.
    let ran = crate::rows::request(
        state,
        app,
        worker,
        paths,
        crate::rows::Scope::File,
        what,
        Box::new(move |state, app, _, at| {
            let frames: Vec<usize> = at.into_iter().flatten().collect();
            meta_on(state, app, &frames, change, may_select);
        }),
    );
    if move_on {
        // The key applied now, and the frame taken out of the filtered
        // list by it: the nearest frame shown takes the selection, once,
        // not the one after (the landing did that itself on the last
        // row). Otherwise one step on, whether the key applied or is
        // queued behind the frame's read.
        let hidden = {
            let st = state.borrow();
            st.current
                .filter(|&c| ran && row_of(&st, c).is_none())
                .and_then(|c| cull::nearest_row(&st.views.shown, c))
        };
        match hidden {
            Some(_) if may_select => {}
            Some(row) => app.invoke_select(row as i32),
            None => app.invoke_step(1, false),
        }
    }
    true
}

/// `change` over `frames`, their sidecars in hand: recorded, written,
/// and the word said in culling; with `may_select`, the selection put
/// on the nearest row when the frame left the filtered list (not when
/// the press moved it on already).
fn meta_on(
    state: &Rc<Shared>,
    app: &App,
    frames: &[usize],
    change: meta::Change,
    may_select: bool,
) {
    let mut st = state.borrow_mut();
    if frames.is_empty() {
        return;
    }
    let was_shown: Vec<bool> = frames.iter().map(|&i| row_of(&st, i).is_some()).collect();
    let (settled, next) = set_meta(&mut st, app, frames, change);
    // The frames the key took out of the filtered list. Without a
    // word they are simply gone from the strip and the grid, and read
    // as lost.
    let hidden: Vec<usize> = frames
        .iter()
        .zip(was_shown)
        .filter(|&(&i, was)| was && !filter_shows(&st, i))
        .map(|(&i, _)| i)
        .collect();
    if crate::panel::cull::cull_mode(&st) {
        let word = tags::notice(&settled);
        let word = if hidden.is_empty() {
            word
        } else {
            tr!("{word}; hidden by the filter", word = word)
        };
        say_notice(&mut st, app, word);
    } else if settled.is_key()
        && !app.get_grid_open()
        && !app.get_lightbox()
        && st.current.is_some_and(|c| {
            row_of(&st, c).is_some()
                && st
                    .file_at(c)
                    .is_some_and(|f| st.files_of(frames).contains(&f))
        })
    {
        // The develop view: the frame on screen's badge over the
        // picture for a moment, the strip tile's being small or put
        // away. Not for a key that took the frame out of the filtered
        // list: the selection moves on, and the badge would be the
        // next frame's.
        flash_badge(&mut st, app);
    }
    let said = match hidden.as_slice() {
        [] => None,
        [c] => Some(tags::hidden_by_filter(
            &settled,
            &file_name(st.path_of(*c)),
            false,
            &st.views.filter.describe(),
        )),
        many => Some(tags::hidden_by_filter(
            &settled,
            &tr!("{n} frame" | "{n} frames" % many.len()),
            true,
            &st.views.filter.describe(),
        )),
    };
    let generation = st.develop.generation;
    drop(st);
    // The frame left the filtered list: on to the nearest.
    if let (Some(row), true) = (next, may_select) {
        app.invoke_select(row as i32);
    }
    // Said after the selection moved on, so the open's own word does
    // not cover it; and an open that asked for a develop has it put
    // back when that lands, over its "developing...".
    if let Some(said) = said {
        let mut st = state.borrow_mut();
        if st.develop.generation != generation && st.culling.cull.is_none() {
            st.develop.status_after_develop = Some((st.develop.generation, said.clone()));
        }
        app.set_status(said.into());
    }
}

/// How long the word for a key stays up before it fades: long enough
/// to read after the arrow that follows the key, short enough that
/// the next key's word is not waiting behind it.
const NOTICE_UP: std::time::Duration = std::time::Duration::from_millis(1200);

/// Put `text` up over the picture and start the timer that takes it
/// down. Only the timer takes it down: an arrow, a develop or the
/// next key leave it (the next key restarts the timer with its own
/// word), so what a key did can be read after moving on.
pub(crate) fn say_notice(st: &mut State, app: &App, text: String) {
    app.set_notice(text.into());
    app.set_notice_on(true);
    let app_weak = app.as_weak();
    st.culling
        .notice_timer
        .start(slint::TimerMode::SingleShot, NOTICE_UP, move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_notice_on(false);
            }
        });
}

/// How long the frame's badge stays over the picture in the develop
/// view after a key, before it fades.
const BADGE_UP: std::time::Duration = std::time::Duration::from_secs(2);

/// Put the frame's badge up over the picture in the develop view and
/// start the timer that takes it down; a key while it is up starts
/// the timer again.
fn flash_badge(st: &mut State, app: &App) {
    app.set_badge_flash(true);
    let app_weak = app.as_weak();
    st.culling
        .badge_timer
        .start(slint::TimerMode::SingleShot, BADGE_UP, move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_badge_flash(false);
            }
        });
}

/// Take the badge's flash down at once: it answered a key on the frame
/// that was open, and is not the next frame's.
pub(super) fn end_flash(st: &State, app: &App) {
    st.culling.badge_timer.stop();
    app.set_badge_flash(false);
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    // The culling keys: a rating, a flag or a color label on the
    // selection. None of this is an edit, so nothing here records a
    // history state, marks the panel dirty or asks for a develop.
    // A key that names what a frame already carries is still the
    // browser's key and is still eaten: it just writes nothing.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_meta_key(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(change) = meta::Change::from_key(&key) else {
                return false;
            };
            meta_on_selection(&state, &app, &worker, change)
        });
    }
    // The frame's own turn, [ and ], and the panel's two buttons:
    // on the selection, as the meta keys are, and in the loupe, the
    // grid and culling alike.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_frame_turned(move |quarters| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if crate::panel::queue_view::refused(&state.borrow(), "a turn") {
                return;
            }
            // The whole selection, as the meta keys have it: a request
            // for its sidecars, and the turn over exactly those frames
            // once they are in.
            let paths: Vec<crate::versions::Key> = {
                let st = state.borrow();
                chosen_frames(&st)
                    .into_iter()
                    .filter_map(|c| st.key_of(c))
                    .collect()
            };
            crate::rows::request(
                &state,
                &app,
                &worker,
                paths,
                crate::rows::Scope::File,
                crate::rows::Request::Turn,
                Box::new(move |state, app, worker, at| {
                    let frames: Vec<usize> = at.into_iter().flatten().collect();
                    turn_frames(&mut state.borrow_mut(), app, worker, &frames, quarters);
                }),
            );
        });
    }
    // Selecting a file: decode and develop it on the worker.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_select(move |row| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            open_row(&mut state.borrow_mut(), &app, &worker, row, false);
        });
    }
    // Arrow keys step along the strip: the set collapses to the
    // frame landed on, or with Shift held takes it as well.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_step(move |by, extend| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // Along the browser's rows, which the filter may have
            // shortened; a selection the filter hid steps from the
            // nearest row shown.
            let (row, count) = {
                let st = state.borrow();
                let row = usize::try_from(app.get_selected()).ok().or_else(|| {
                    st.current
                        .and_then(|c| cull::nearest_row(&st.views.shown, c))
                });
                (row, st.views.shown.len())
            };
            let (Some(row), true) = (row, count > 0) else {
                return;
            };
            let next = (row as i64 + by as i64).clamp(0, count as i64 - 1) as usize;
            step_to(
                &state,
                &app,
                &worker,
                next as i32,
                next != row || app.get_selected() < 0,
                extend,
            );
        });
    }
    // A click on a frame in the strip or the grid: a plain one opens
    // it, a Ctrl or a Shift one changes the set about the frame on
    // screen and leaves it there.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_frame_clicked(move |row, ctrl, shift| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(&file) = usize::try_from(row)
                .ok()
                .and_then(|r| st.views.shown.get(r))
            else {
                return;
            };
            match selection::click(
                &st.views.picked,
                &st.views.shown,
                st.current,
                file,
                ctrl,
                shift,
            ) {
                // A plain click on the frame already on screen: the
                // set goes back to it, and nothing is opened again.
                selection::Click::Open(f) if Some(f) == st.current => {
                    st.views.picked = vec![f];
                    show_set(&mut st, &app);
                }
                selection::Click::Open(_) => {
                    drop(st);
                    app.invoke_select(row);
                }
                selection::Click::Set(set) => {
                    st.views.picked = set;
                    show_set(&mut st, &app);
                }
            }
        });
    }
    // Escape over a set: back to the frame on screen.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_set_collapsed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.views.picked = st.current.into_iter().collect();
            show_set(&mut st, &app);
        });
    }
    // The frames the strip shows: their pictures jump the worker's
    // thumbnail queue, so a scroll into unseen ground fills in
    // seconds rather than after the whole folder. Reported on every
    // move of the strip; the worker drops a repeat. The strip's cells
    // follow, and the pictures with them.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_strip_range(move |first, last| {
            if first < 0 || last < first {
                return;
            }
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (at, wanted) = cells::strip_window(first as usize, last as usize);
            let moved = cells::show(&app, cells::View::Strip, at, wanted);
            let mut st = state.borrow_mut();
            if moved {
                settle_pictures(&mut st, &app);
            }
            // Rows to files: the range between the two, which under a
            // filter is a superset of what is shown, and fine for an
            // order.
            let (Some(&f), Some(&l)) = (
                st.views.shown.get(first as usize),
                st.views.shown.get(last as usize),
            ) else {
                return;
            };
            st.views.strip_shown = Some((first, last));
            // The strip's pictures at its row's height, as the grid's
            // at its cell: a taller strip asks for larger ones, and
            // those on screen made too small are asked for again, once.
            let want = strip::render_size(app.get_strip_photo());
            if want != st.views.thumb_want {
                st.views.thumb_want = want;
                worker.set_thumb_size(want);
            }
            for row in first as usize..=last as usize {
                let Some(&i) = st.views.shown.get(row) else {
                    break;
                };
                if st.views.thumb_made[i] > 0
                    && grid::wants_bigger(st.views.thumb_made[i], want)
                    && grid::asks_again(st.views.thumb_asked[i], want)
                {
                    st.views.thumb_asked[i] = want;
                    worker.send(crate::rows::thumb_job(&st, i));
                }
            }
            crate::rows::ask_reframed(&mut st, &worker, first as usize, last as usize);
            worker.want_thumbnails(f, l);
            near_shown(&mut st, &worker, first as usize, last as usize);
            crate::edited::note_editing(&mut st, &app, &worker, false);
        });
    }
    // The grid's layout, asked of the pure arithmetic in `grid` so
    // the numbers are tested rather than left in a binding.
    app.on_grid_columns(grid::columns);
    app.on_grid_slack(grid::slack);
    grid_layout(app);
    strip_layout(app);
    app.on_grid_tiles_top(grid::tiles_top);
    app.on_grid_common_pitch(grid::common_pitch);
    app.on_bar_length(grid::bar_length);
    app.on_bar_offset(grid::bar_offset);
    app.on_bar_drag(grid::bar_drag);
    // Home and End, in the grid and the strip: the first frame or the
    // last, landed on as an arrow lands. With Shift the set takes the
    // whole run from the current frame there, as Shift held on an
    // arrow to the end would, and as a Shift+click on that frame does.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_step_end(move |end, extend| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let count = state.borrow().views.shown.len() as i32;
            let next = grid::end(end, count);
            if next >= 0 {
                if extend {
                    let mut st = state.borrow_mut();
                    if let Some(current) = st.current {
                        let run = selection::range(
                            &st.views.shown,
                            current,
                            st.views.shown[next as usize],
                        );
                        st.views.picked = selection::union(&chosen_frames(&st), &run);
                    }
                }
                step_to(
                    &state,
                    &app,
                    &worker,
                    next,
                    next != app.get_selected(),
                    extend,
                );
            }
        });
    }
    // Ctrl+A, in the strip, the grid and the lightbox: every frame
    // shown is chosen, and the frame on screen stays current, the
    // anchor a Shift+click measures from. Nothing is opened. With
    // nothing current there is no set to grow, and with nothing shown
    // nothing to choose.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_select_all(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(current) = st.current else {
                return;
            };
            if st.views.shown.is_empty() {
                return;
            }
            st.views.picked = selection::all(&st.views.shown, current);
            show_set(&mut st, &app);
        });
    }
    // An arrow in the grid: along the row, or by a whole row, and
    // the frame it lands on is opened as a click on it would.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_grid_step(move |dx, dy, columns, extend| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let count = state.borrow().views.shown.len() as i32;
            let heads = grid_breaks(&state.borrow(), &app);
            let next = grid::step_in(app.get_selected(), dx, dy, columns, count, &heads);
            if next >= 0 {
                step_to(
                    &state,
                    &app,
                    &worker,
                    next,
                    next != app.get_selected(),
                    extend,
                );
            }
        });
    }
    // Ctrl and the wheel, or the plus and minus keys: the cell takes
    // a step and the grid re-flows. The step is from the cell laid
    // out, which a narrow sheet holds under the size asked.
    {
        let app_weak = app.as_weak();
        app.on_grid_zoom(move |by| {
            if let Some(app) = app_weak.upgrade() {
                let laid = grid::laid(app.get_grid_cell(), app.get_grid_w());
                app.set_grid_cell(grid::zoom(laid, by));
                crate::panel::prefs::show_grid_cell(&app);
            }
        });
    }
    tile_size(app);
    // The frames the grid shows, from where it is scrolled: the
    // strip's mechanism over a rectangle rather than a row. A folder
    // of hundreds is never rendered up front; what is on screen
    // jumps the queue, and a cell larger than the pictures were made
    // for asks for those on screen again at the larger size.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_grid_range(move |scroll, height, columns, top| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The grid is open: its pictures are what is on screen.
            worker.hold_thumbnails(false);
            // The lightbox's tiles when it is on, else the grid's cells.
            let cell = app.get_tile_cell();
            let mut st = state.borrow_mut();
            let count = st.views.shown.len() as i32;
            let heads = grid_breaks(&st, &app);
            let layout = cells::layout(&app, cell, columns, count, &heads, grid_shape(&app));
            // The grid's cells follow, and the pictures with them.
            let (at, wanted) = layout.window(scroll, height, top);
            if cells::show(&app, cells::View::Grid, at, wanted) {
                settle_pictures(&mut st, &app);
            }
            let Some((first, last)) = layout.visible(scroll, height, top) else {
                return;
            };
            st.views.grid_shown = Some((first, last));
            let (first, last) = (
                first.max(0) as usize,
                (last.max(0) as usize).min(st.views.shown.len().saturating_sub(1)),
            );
            // What is made from now on is this cell's size, up or
            // down: a cell that has shrunk should not go on paying
            // the memory of the one before it.
            let want = grid::render_size(cell);
            if want != st.views.thumb_want {
                st.views.thumb_want = want;
                worker.set_thumb_size(want);
            }
            // Making one again is the part that is only ever a step
            // up: only pictures already on screen and a quarter too
            // small, and only once each. One still in the queue will
            // come back at the size the worker now has anyway, and
            // asking again would only make it twice.
            for row in first..=last {
                let i = st.views.shown[row];
                if st.views.thumb_made[i] > 0
                    && grid::wants_bigger(st.views.thumb_made[i], want)
                    && grid::asks_again(st.views.thumb_asked[i], want)
                {
                    st.views.thumb_asked[i] = want;
                    worker.send(crate::rows::thumb_job(&st, i));
                }
            }
            crate::rows::ask_reframed(&mut st, &worker, first, last);
            // After the pushes, which clear the queue's order.
            worker.want_thumbnails(st.views.shown[first], st.views.shown[last]);
            near_shown(&mut st, &worker, first, last);
            // In the grid, no frame is being edited.
            crate::edited::note_editing(&mut st, &app, &worker, true);
            // The lightbox's tiles follow the cells.
            if st.culling.lightbox.is_some() {
                crate::lightbox::refresh(&mut st, &app);
            }
        });
    }
    // The desktop's folder chooser, for the Open folder button and
    // Ctrl+O: the file list becomes that folder's, the last file
    // open selected if it lies there, else the first.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_open_folder(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let start = {
                let st = state.borrow();
                st.current
                    .and_then(|i| st.path_at(i))
                    .and_then(|f| f.parent())
                    .map(Path::to_path_buf)
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/"))
            };
            let app_weak = app.as_weak();
            app.set_status(tr!("choosing a folder to open...").into());
            chooser::choose_folder(tr!("Open folder"), start, move |chosen| {
                let app_weak = app_weak.clone();
                // The state and the worker through their thread
                // locals, as this runs on the chooser's own thread and
                // an `Rc` cannot cross it; `choose_open`'s importer
                // does the same.
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let (Some(state), Some(worker)) = (
                        STATE.with(|s| s.borrow().clone()),
                        WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    match chosen {
                        Ok(Some(dir)) => open_folder(&state, &app, &worker, &dir),
                        Ok(None) => app.set_status(tr!("open folder canceled").into()),
                        Err(e) => {
                            tracing::warn!("file chooser: {e:#}");
                            app.set_status(chooser::chooser_failed(&e).into());
                        }
                    }
                });
            });
        });
    }
}

/// The window's keys, on the headless backend: what the grid does
/// with them cannot be reached from a unit test of `grid` alone, and
/// there is no way to press a key at a Wayland session from here.
#[cfg(test)]
mod tests;
