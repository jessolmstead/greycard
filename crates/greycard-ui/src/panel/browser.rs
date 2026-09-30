use crate::panel::crop::read_geometry;
use crate::panel::cull::{
    cull_select, drop_placeholder, enter_cull, say_if_empty, show_filter, start_placeholder,
};
use crate::panel::edit::{
    current_turn, edit_to_develop, read_edit, save_edit, show_edit, write_sidecar,
};
use crate::panel::history::show_history;
use crate::tags::{self, Tags};
use crate::*;

/// A folder's first round of thumbnails as it comes back from the
/// worker: which have not yet, and what the ones that have cost. The
/// log gets one line when the last arrives, which is how a folder
/// open is timed with the cache cold, warm, and after a rename.
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
}

/// What a finished run says, for the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RunTotals {
    pub(crate) files: usize,
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
        (self.left == 0).then(|| RunTotals {
            files: self.waiting.len(),
            cached: self.cached,
            made: self.made,
            failed: self.failed,
            seconds: self.started.elapsed().as_secs_f64(),
            work: self.work,
        })
    }
}

/// Count a thumbnail into the folder's run. The log hears of the
/// last at the end of the turn that took it in ([`end_thumb_turn`]).
pub(crate) fn count_thumb(st: &mut State, index: usize, cached: Option<bool>, seconds: f64) {
    let Some(run) = st.thumb_run.as_mut() else {
        return;
    };
    if let Some(t) = run.arrived(index, cached, seconds) {
        run.done = Some(t);
    }
}

/// A turn of the window's that took pictures in, begun at `started`,
/// is over: counted into the run, and when the run's last has come,
/// the run said in the log and closed.
pub(crate) fn end_thumb_turn(st: &mut State, started: std::time::Instant) {
    let Some(run) = st.thumb_run.as_mut() else {
        return;
    };
    run.turn(started.elapsed().as_secs_f64());
    let Some(t) = run.done else {
        return;
    };
    let w = run.window();
    // The work is summed over the pool's threads, so it can be
    // more than the wall time.
    let threads = WORKER.with(|w| w.borrow().as_ref().map_or(1, |w| w.thumb_threads()));
    tracing::info!(
        "thumbnails: {} files in {:.2} s from the folder's open, {:.2} s of decoding across \
         {threads} threads; {} from the cache, {} made, {} failed; taken in by the window in \
         {} turns, {:.2} s, the longest {:.0} ms; {} frames drawn meanwhile, the first {}, the \
         longest wait between two {:.0} ms",
        t.files,
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
    st.thumb_run = None;
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

    /// A frame is being drawn. The moves begin a second after the
    /// folder's last picture came, with the window settled; once they
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
    let Some(timing) = st.time_scroll.as_mut() else {
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
        st.time_scroll = None;
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
    if let Some(timing) = state.borrow_mut().time_scroll.as_mut() {
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
        st.grid_shown
    } else {
        st.strip_shown
    };
    let Some((first, last)) = range else {
        return false;
    };
    (first.max(0) as usize..=last.max(0) as usize)
        .filter_map(|r| st.shown.get(r))
        .all(|&f| st.thumb_base[f].is_some() || st.thumb_failed[f])
}

/// A browser row for a file: its name, the badges its meta asks for,
/// and no picture until the worker has made one.
pub(crate) fn thumb_for(path: &Path, meta: &Meta) -> Thumb {
    Thumb {
        name: file_name(path).into(),
        image: slint::Image::default(),
        rating: meta.rating.min(meta::STARS) as i32,
        flag: meta.flag.code(),
        label: meta.label.code(),
        chosen: false,
        failed: false,
    }
}

/// The selection as it is acted on: the current frame and the rest
/// of the set, as files, sorted, and only what the filter shows. A
/// current frame the filter has just hidden is not in it: a key must
/// not reach a frame nobody can see is chosen. Empty with nothing
/// open.
pub(crate) fn chosen_frames(st: &State) -> Vec<usize> {
    selection::frames(&st.picked, st.current)
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
    for (row, &f) in st.shown.iter().enumerate() {
        let chosen = chosen_set.binary_search(&f).is_ok();
        if let Some(mut t) = model.row_data(row)
            && t.chosen != chosen
        {
            t.chosen = chosen;
            model.set_row_data(row, t);
        }
    }
    app.set_set_count(chosen_set.len().max(1) as i32);
}

/// Put file `i`'s meta on its row in the strip and the grid, leaving
/// whatever picture the row already holds.
///
/// The row and the file are two different numbers whenever a filter
/// is on — `row_of` is what maps one to the other — and both the
/// read and the write are the row's, as `show_thumb` has it.
pub(crate) fn show_badges(st: &State, app: &App, i: usize) {
    let Some(meta) = st.sidecars.get(i).map(|s| &s.meta) else {
        return;
    };
    let Some(row) = row_of(st, i) else {
        return;
    };
    let model = app.get_thumbs();
    if let Some(mut t) = model.row_data(row) {
        t.rating = meta.rating.min(meta::STARS) as i32;
        t.flag = meta.flag.code();
        t.label = meta.label.code();
        model.set_row_data(row, t);
    }
    if Some(i) == st.current {
        show_frame_tags(st, app);
    }
}

/// Put the frame on screen's meta on the window, for the culling
/// loupe's badge: the same three numbers its strip row carries, read
/// off the same sidecar. Called when the selection lands on a frame
/// and whenever that frame's badges are put out, so the two cannot
/// say different things.
pub(crate) fn show_frame_tags(st: &State, app: &App) {
    let Some(meta) = st.current.and_then(|c| st.sidecars.get(c)).map(|s| &s.meta) else {
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
    let was_shown: Vec<bool> = frames.iter().map(|&i| row_of(st, i).is_some()).collect();
    let before: Vec<Tags> = frames
        .iter()
        .map(|&i| Tags::of(&st.sidecars[i].meta))
        .collect();
    let (settled, moved) = greycard_edit::meta_into(&mut st.sidecars, frames, change);
    let step = frames
        .iter()
        .zip(before)
        .filter(|(i, _)| moved.contains(i))
        .map(|(&i, before)| tags::Moved {
            path: st.files[i].clone(),
            before,
            after: Tags::of(&st.sidecars[i].meta),
        })
        .collect();
    st.tags.record(step);
    (settled, tags_shown(st, app, frames, &was_shown, &moved))
}

/// What follows a change to frames' tags, from a key or an undo:
/// each moved frame written and its badges put out, the rejects
/// counted, and the browser's rows rebuilt if the filter now shows
/// a different set; the row to move the selection to when the
/// current frame left them.
fn tags_shown(
    st: &mut State,
    app: &App,
    frames: &[usize],
    was_shown: &[bool],
    moved: &[usize],
) -> Option<usize> {
    for &i in moved {
        write_sidecar(st, i);
        show_badges(st, app, i);
    }
    app.set_reject_count(reject_count(st) as i32);
    if moved.is_empty() {
        return None;
    }
    let hides = frames
        .iter()
        .zip(was_shown)
        .any(|(&i, &was)| was != filter_shows(st, i));
    if !hides {
        show_filter(st, app);
        return None;
    }
    let next = rebuild_browser(st, app);
    if next.is_none()
        && st.cull.is_some()
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
    // The frame whose edit the panel owns: the open one, and not
    // while culling, where the panel still holds the last-opened
    // frame's edit and the sidecars are the truth.
    let panel_frame = st.current.filter(|_| st.cull.is_none());
    if let Some(cull) = st.cull.as_mut() {
        cull.turned = Some(std::time::Instant::now());
    }
    let mut developing = false;
    for &f in frames {
        if st.sidecars.get(f).is_none() {
            continue;
        }
        migrate_frame(st, f);
        let aspect = frame_aspect(st, f);
        if aspect.is_none() && st.sidecars[f].placed() {
            tracing::warn!(
                "{}: no picture of it yet to measure by, so its masks and \
                 repair patches stay where they are on the screen",
                file_name(&st.files[f])
            );
        }
        if Some(f) == panel_frame {
            // The panel owns the open frame's edit, so what it is
            // holding is recorded first and the sidecar's states are
            // then turned together; the turn itself records nothing.
            let pending = read_edit(app, &st.edit, st.target);
            save_edit(st, pending);
            st.sidecars[f].turn_by(quarters, aspect);
            let edit = st.sidecars[f].current.clone();
            // The whole panel, since a turn moves masks and patches
            // as well as the crop. `show_edit` puts a chosen
            // adjustment's tab up, which a turn pressed on the Crop
            // tab has not asked for, so the tab is kept.
            let tab = app.get_panel_tab();
            show_edit(st, &edit, app, st.target);
            app.set_panel_tab(tab);
            st.edit = edit;
            // The developed size stands on end now, and the develop
            // that will say so is a second away: a second turn
            // pressed before it lands would otherwise measure the
            // frame by the shape it used to be.
            if quarters.rem_euclid(2) == 1 {
                st.source_size = (st.source_size.1, st.source_size.0);
            }
            developing = true;
        } else {
            greycard_edit::turn_into(&mut st.sidecars, &[f], quarters, aspect);
        }
        write_sidecar(st, f);
        let (turns, flip) = thumb_turns(st, app, f);
        show_thumb(st, app, f, turns, flip);
    }
    // The rasters are keyed by adjustment and component and know
    // nothing of a turn: a brush's is only brought up to its strokes
    // rather than remade, and a subject's shape does not change at
    // all, so both would go on masking the pixels they masked
    // before. They are cheap beside a mask in the wrong place.
    st.rasters.clear();
    st.learned.clear();
    st.asked.clear();
    // The open frame, turned from the culling loupe or among a
    // selection, is measured the way up the viewport will draw it.
    crate::panel::viewport::settle_source_size(st);
    // The picture stands on end now, so whatever was fitted is
    // fitted afresh; the culling loupe reads the turn out of the
    // same matrix on its next frame, which is a redraw and not a
    // decode.
    st.image_size = (0, 0);
    if developing {
        // Whatever the panel was holding was recorded on the way in;
        // the turn itself added no state.
        show_history(st, app);
        st.generation += 1;
        st.turn_pressed = Some(std::time::Instant::now());
        app.set_status("developing...".into());
        app.set_busy(true);
        worker.send(Job::Develop {
            edit: edit_to_develop(app, &st.edit),
            generation: st.generation,
            turn: current_turn(st),
        });
    }
    app.window().request_redraw();
}

/// Put file `i`'s picture on the filmstrip turned `turns` quarters and
/// mirrored if `flip`, as the viewport turns the file.
pub(crate) fn show_thumb(st: &mut State, app: &App, i: usize, turns: u8, flip: bool) {
    let Some((w, h, rgb)) = &st.thumb_base[i] else {
        return;
    };
    // A frame the filter hides has no row; its picture waits for the
    // list to show it again.
    let Some(row) = row_of(st, i) else {
        return;
    };
    // Nor does a row neither view has a cell for: it takes its
    // picture when one comes to it (`settle_pictures`), turned as the
    // frame is turned then.
    if !cells::has_cell(app, row) {
        return;
    }
    st.thumb_shown[i] = Some((turns, flip));
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
    for (first, count) in std::mem::replace(&mut st.pictured, now) {
        for row in (first..first + count).filter(|&r| !has(r)) {
            let Some(&f) = st.shown.get(row) else {
                break;
            };
            if st.thumb_shown[f].take().is_some()
                && let Some(mut t) = model.row_data(row)
            {
                t.image = slint::Image::default();
                model.set_row_data(row, t);
            }
        }
    }
    for (first, count) in now {
        for row in first..first + count {
            let Some(&f) = st.shown.get(row) else {
                break;
            };
            if st.thumb_base[f].is_some() && st.thumb_shown[f].is_none() {
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
    let Some(had) = st.thumb_base.get(i).map(Option::is_some) else {
        return;
    };
    st.thumb_failed[i] = true;
    if had {
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
pub(crate) fn stance(st: &mut State, file: usize) -> Option<greycard_core::decode::Stance> {
    let path = st.files.get(file)?.clone();
    if let Some(s) = st.stances.get(&path) {
        return Some(*s);
    }
    match greycard_core::decode::stance_path(&path) {
        Ok(s) => {
            st.stances.insert(path, s);
            Some(s)
        }
        Err(e) => {
            tracing::warn!("{}: how the frame stands: {e}", file_name(&path));
            None
        }
    }
}

/// A frame's own orientation tag. `None` when the file will not say,
/// which is when nothing here writes or believes an XMP's
/// orientation for it.
pub(crate) fn camera_tag(st: &mut State, file: usize) -> Option<Orientation> {
    stance(st, file).map(|s| s.orientation)
}

/// A turn in words, for the log: which way and how far.
pub(crate) fn turn_words(quarters: i32) -> &'static str {
    match quarters.rem_euclid(4) {
        1 => "a quarter turn to the right of its tag",
        2 => "a half turn from its tag",
        3 => "a quarter turn to the left of its tag",
        _ => "as its tag says",
    }
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
pub(crate) fn frame_aspect(st: &mut State, file: usize) -> Option<f32> {
    let turn = st.sidecars.get(file).map_or(0, |s| s.turn);
    let aspect = |(w, h): (u32, u32)| (w > 0 && h > 0).then(|| w as f32 / h as f32);
    // The open frame's own develop: exactly the picture the masks
    // are in, and already the way up the frame's turn puts it.
    if st.current == Some(file)
        && st.cull.is_none()
        && let Some(a) = aspect(st.source_size)
    {
        return Some(a);
    }
    // The file itself, which is the same frame however it is shown.
    if let Some(a) = stance(st, file)
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
    if let Some(cull) = st.cull.as_ref().or(st.hold.as_ref())
        && let Some(preview) = cull.cache.best(file)
        && let Some(a) = shown(preview.source)
    {
        return Some(a);
    }
    st.thumb_base
        .get(file)
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
pub(crate) fn migrate_frame(st: &mut State, file: usize) {
    if !st.sidecars.get(file).is_some_and(Sidecar::needs_frame) {
        return;
    }
    match frame_aspect(st, file) {
        Some(aspect) => st.sidecars[file].migrate_with_frame(aspect < 1.0),
        None => tracing::warn!(
            "{}: nothing can say which way up it is yet, so its Original crop \
             waits before it is brought up to date",
            file_name(&st.files[file])
        ),
    }
}

/// The browser's row of a file, when the filter shows it.
pub(crate) fn row_of(st: &State, file: usize) -> Option<usize> {
    st.shown.binary_search(&file).ok()
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
            index: st.index_passed.get(i).copied().unwrap_or(true),
        })
        .collect()
}

/// Whether the filter shows one file, as it stands now.
pub(crate) fn filter_shows(st: &State, file: usize) -> bool {
    match (st.files.get(file), st.sidecars.get(file)) {
        (Some(path), Some(s)) => st.filter.shows(filter::Frame {
            path,
            meta: &s.meta,
            index: st.index_passed.get(file).copied().unwrap_or(true),
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
pub(crate) fn thumb_turns(st: &State, app: &App, file: usize) -> (u8, bool) {
    let turn = st.sidecars[file].turn;
    if st.current == Some(file) && st.cull.is_none() {
        read_geometry(app).shown_turns(turn)
    } else {
        st.sidecars[file].current.geometry.shown_turns(turn)
    }
}

/// How many frames of the folder are flagged reject.
pub(crate) fn reject_count(st: &State) -> usize {
    (0..st.files.len())
        .filter(|&i| crate::panel::cull::to_move_out(st, i))
        .count()
}

/// The browser's list again: the filter over the flags, the strip's
/// and the grid's rows made afresh, their cells' pictures put on from
/// those already made, and the selection kept on its row, or put on
/// the nearest row when its frame is hidden (which the caller then
/// opens).
pub(crate) fn rebuild_browser(st: &mut State, app: &App) -> Option<usize> {
    if !std::mem::take(&mut st.index_pass_ready) {
        st.index_passed = crate::library::index_pass(st);
    }
    st.shown = st.filter.apply(&filter_frames(st));
    show_filter(st, app);
    let thumbs = st
        .shown
        .iter()
        .map(|&f| Thumb {
            failed: st.thumb_failed[f] && st.thumb_base[f].is_none(),
            ..thumb_for(&st.files[f], &st.sidecars[f].meta)
        })
        .collect();
    // No row has a picture on it now. The rows the views have cells
    // for take theirs from the pixels in hand, a few screens' worth
    // however long the list: nothing is carried over from the old
    // rows, so nothing can land on a row renumbered under it.
    st.thumb_shown.iter_mut().for_each(|s| *s = None);
    st.pictured = [(0, 0); 2];
    cells::set_rows(app, thumbs);
    settle_pictures(st, app);

    app.set_reject_count(reject_count(st) as i32);
    // A frame the filter now hides leaves the set: a key or a sync
    // must not reach a frame nobody can see is chosen.
    st.picked = selection::prune(&st.picked, &st.shown, st.current);
    show_set(st, app);
    let Some(c) = st.current else {
        app.set_selected(-1);
        return None;
    };
    match row_of(st, c) {
        Some(row) => {
            app.set_selected(row as i32);
            None
        }
        None => {
            app.set_selected(-1);
            cull::nearest_row(&st.shown, c)
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
pub(crate) fn load_sidecars(files: &[PathBuf], write_sidecars: bool) -> (Vec<Sidecar>, Vec<bool>) {
    files
        .iter()
        .map(|f| load_sidecar(f, write_sidecars))
        .unzip()
}

/// [`load_sidecars`] on every core, for a list too long to read on
/// the window's thread: the all-roots view's, which is the whole
/// library. The order is the list's. `read`, when given, is bumped
/// for each sidecar read, for the window's bar.
pub(crate) fn load_sidecars_parallel(
    files: &[PathBuf],
    write_sidecars: bool,
    read: Option<&std::sync::atomic::AtomicUsize>,
) -> (Vec<Sidecar>, Vec<bool>) {
    use rayon::prelude::*;
    files
        .par_iter()
        .map(|f| {
            let one = load_sidecar(f, write_sidecars);
            if let Some(read) = read {
                read.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            one
        })
        .unzip()
}

/// One file's sidecar, as [`load_sidecars`] reads each.
pub(crate) fn load_sidecar(f: &Path, write_sidecars: bool) -> (Sidecar, bool) {
    if !write_sidecars {
        return (Sidecar::default(), false);
    }
    // A picture that is not a raw starts from its own default.
    let fresh = || {
        if greycard_core::picture::is_picture_path(f) {
            (
                Sidecar {
                    current: Edit::for_picture(),
                    ..Sidecar::default()
                },
                false,
            )
        } else {
            (Sidecar::default(), true)
        }
    };
    let (mut sidecar, seed) = match Sidecar::load(f) {
        Ok(Some(s)) => {
            // A shape from a later build: loaded as nothing,
            // the rest of the edit kept, and gone on the next
            // save.
            let unknown: usize = s.current.adjustments.iter().map(|a| a.mask.unknown()).sum();
            if unknown > 0 {
                tracing::warn!(
                    "{}: sidecar: {unknown} mask shape(s) of a kind this build does not know; \
                     left out, and dropped when the edit is saved",
                    file_name(f)
                );
            }
            let raw = !greycard_core::picture::is_picture_path(f);
            let seed = raw && files::never_developed(&s);
            (s, seed)
        }
        Ok(None) => fresh(),
        Err(e) => {
            tracing::warn!(
                "{}: sidecar: {e}; starting from the default edit",
                file_name(f)
            );
            fresh()
        }
    };
    // What an XMP beside the frame has to say, when it has
    // changed since the sidecar last took its word. Reading
    // one is not behind the setting: writing an XMP is a
    // choice about somebody else's folder, believing one
    // that is already there is not. Nothing is written
    // here — the meta and the mark ride in memory until the
    // frame's next real save — so opening a folder of
    // somebody else's raws deposits nothing in it.
    // The camera's tag, for the one field of a packet that
    // is measured against it. The closure is called only for
    // a packet that carries `tiff:Orientation` at all, so a
    // folder whose XMPs are silent about it opens without
    // reading a byte of any raw.
    let took = xmp::adopt(f, &mut sidecar, || {
        greycard_core::decode::orientation_path(f)
            .inspect_err(|e| tracing::warn!("{}: orientation: {e}", file_name(f)))
            .ok()
    });
    // A turn taken from somebody else's file turns the
    // picture under the user without their having pressed
    // anything, and records no state to undo it with, so it
    // is said out loud and by name.
    if took.turned.is_some() {
        tracing::info!(
            "{}: the XMP beside it says the frame stands {}; turned to match",
            file_name(f),
            // Where it now stands, not how far it moved to
            // get there: a reader of the log wants the frame,
            // not the delta.
            turn_words(i32::from(sidecar.turn))
        );
        if took.left_placed {
            tracing::warn!(
                "{}: its masks and repair patches stay where they are on the \
                 screen, since nothing is decoded at a folder's opening to \
                 measure the frame by",
                file_name(f)
            );
        }
    }
    (sidecar, seed)
}

/// Replace the file list with `dir`'s files, the same scan the launch
/// path does, and select the last file open if it lies there, else
/// the first. An empty folder is left alone rather than emptying the
/// strip under the user's feet.
pub(crate) fn open_folder(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let Ok(files) = files::list_files(dir) else {
        tracing::warn!("{}: not a folder", dir.display());
        app.set_status(format!("{}: not a folder", dir.display()).into());
        return;
    };
    if files.is_empty() {
        tracing::warn!("no pictures in {}", dir.display());
        app.set_status(format!("no pictures in {}", dir.display()).into());
        return;
    }
    let last = settings::Settings::load().last_file;
    let last = (!last.is_empty()).then(|| PathBuf::from(last));
    let select = files::select_index(&files, last.as_deref());
    open_files(state, app, worker, files, select);
}

/// What the desktop asked to open, the Finder's double-click or Open
/// With: each path as the command line's path would be, a file on its
/// own and a folder's pictures, starting on the first.
pub(crate) fn open_paths(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    paths: &[PathBuf],
) {
    let (files, refused) = files::list_paths(paths);
    for e in &refused {
        tracing::warn!("{e:#}");
    }
    if files.is_empty() {
        let said = refused
            .first()
            .map_or_else(|| "nothing to open".to_string(), |e| format!("{e:#}"));
        app.set_status(said.into());
        return;
    }
    // Files asked for by name, not a folder: a filter left on from
    // before must not hide what was just asked for.
    if paths.iter().all(|p| !p.is_dir()) && !state.borrow().filter.is_empty() {
        tracing::info!("browser filter cleared for the files asked for");
        state.borrow_mut().filter = filter::Filter::default();
        app.set_filter_text("".into());
    }
    open_files(state, app, worker, files, 0);
}

/// A new list of files in the browser, a folder's or the desktop's,
/// `select` opened. Its sidecars are read here, on the window's
/// thread: a folder is a few hundred at most. The all-roots view,
/// which can be the whole library, reads them elsewhere and comes in
/// by [`open_loaded`].
fn open_files(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    files: Vec<PathBuf>,
    select: usize,
) {
    let (sidecars, seed_blend) = {
        let mut st = state.borrow_mut();
        // A view of the roots still being read is dropped by the
        // generation, and nothing is loading any more.
        st.view_generation += 1;
        crate::roots::loading_done(&mut st.library, app);
        st.view = crate::roots::View::Folder;
        // The chips follow the view: the root that was on goes off,
        // and "Add this folder" is offered when this one is not under
        // a root. The first cut left the old chip lit.
        crate::roots::show(&st, app);
        load_sidecars(&files, st.write_sidecars)
    };
    open_loaded(state, app, worker, files, sidecars, seed_blend, select);
}

/// A new list of files in the browser with their sidecars read,
/// `select` opened.
pub(crate) fn open_loaded(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    files: Vec<PathBuf>,
    sidecars: Vec<Sidecar>,
    seed_blend: Vec<bool>,
    select: usize,
) {
    let started = std::time::Instant::now();
    let mut st = state.borrow_mut();
    // Leave the old file as a normal selection would: its edit saved,
    // its picture held on screen until the new one develops. In
    // culling the panel is not the frame's, and there is nothing to
    // save; the mode stays on for the new folder.
    if st.current.is_some() && st.cull.is_none() {
        let outgoing = read_edit(app, &st.edit, st.target);
        if st.base_white.is_some() {
            st.held = Some(outgoing.clone());
        } else {
            st.zoom = 0.0;
        }
        save_edit(&mut st, outgoing);
    }
    if let Some(cull) = st.cull.as_mut() {
        cull.cache.clear();
        cull.textures.clear();
        cull.failed.clear();
        cull.anchor = 0;
    }
    // Another folder's files, and the pictures kept are keyed by the
    // last one's numbering.
    drop_placeholder(&mut st, app);
    st.hold = None;
    st.prefetch.want(Vec::new());
    st.current = None;
    st.picked.clear();
    st.sidecars = sidecars;
    st.seed_blend = seed_blend;
    st.thumb_base = vec![None; files.len()];
    st.thumb_shown = vec![None; files.len()];
    // A folder of its own: the grid's zoom does not carry its
    // appetite for large pictures over to it.
    st.thumb_made = vec![0; files.len()];
    st.thumb_failed = vec![false; files.len()];
    st.thumb_asked = vec![worker::THUMB_WIDTH; files.len()];
    st.thumb_want = worker::THUMB_WIDTH;
    st.grid_shown = None;
    st.strip_shown = None;
    st.thumb_run = Some(ThumbRun::new(files.len()));
    st.fill_clock = Some(std::time::Instant::now());
    // The old folder's thumbnails still waiting are dropped: their
    // numbering is its list's.
    worker.forget_thumbnails();
    worker.set_thumb_size(worker::THUMB_WIDTH);
    st.files = files.clone();
    // The new folder's rows as the index has them now, and a pass
    // over it on the indexer's thread to bring them up to date.
    st.index_passed = vec![true; files.len()];
    st.index_pass_ready = false;
    st.index_progress = None;
    st.index_tries = 0;
    st.index_error = None;
    crate::library::index_open_folder(&mut st);
    crate::panel::recent::opened(&mut st, app);
    let ids = started.elapsed();
    rebuild_browser(&mut st, app);
    let listed = started.elapsed();
    // The file to open, as a row of the list; hidden by the filter,
    // the nearest one shown.
    let mut row = row_of(&st, select).or_else(|| cull::nearest_row(&st.shown, select));
    // A list that arrives before the window has its device (the
    // all-roots view asked for on the command line, read in less time
    // than the window takes to come up) opens its frame from the
    // rendering setup, as the launch's own does, so the first develop
    // runs on the GPU like every later one.
    // A frame the setup was to open belongs to the list just
    // replaced, and goes with it.
    let deferred = !st.setup_ran && !files.is_empty();
    st.select_at_start = deferred.then_some(select);
    if deferred {
        row = None;
    }
    drop(st);

    if row.is_some() {
        hold_thumbnails_for_develop(app, worker);
    }
    for (i, f) in files.iter().enumerate() {
        worker.send(Job::Thumbnail {
            index: i,
            path: f.clone(),
        });
    }
    tracing::debug!(
        "browser: {} frames in, rows read by {:.1} ms, the list made by {:.1} ms, \
         thumbnails asked by {:.1} ms",
        files.len(),
        ids.as_secs_f64() * 1e3,
        listed.as_secs_f64() * 1e3,
        started.elapsed().as_secs_f64() * 1e3
    );
    if let Some(row) = row {
        app.invoke_select(row as i32);
    } else if !files.is_empty() && !deferred {
        tracing::warn!("no frames pass the filter");
        app.set_status(filter::NOTHING_SHOWN.into());
    }
}

/// The longest a folder opened in the loupe holds its thumbnails for
/// its first develop: one that never comes (a device that never
/// arrives, a frame whose open is lost) must not leave the strip
/// empty for the session.
pub(crate) const HOLD_AT_MOST: std::time::Duration = std::time::Duration::from_secs(10);

/// A folder opening in the loupe: its thumbnails wait for the first
/// develop, which is what the window is waiting to show, and are let
/// go when it is delivered (or fails, or the grid opens, or
/// [`HOLD_AT_MOST`] passes). Made beside it they cost that develop's
/// first paint a sixth of a second on a cold folder of 300. On the
/// grid the thumbnails are the content, and nothing is held.
pub(crate) fn hold_thumbnails_for_develop(app: &App, worker: &Worker) {
    if app.get_grid_open() {
        return;
    }
    worker.hold_thumbnails(true);
    slint::Timer::single_shot(HOLD_AT_MOST, release_thumbnails);
}

/// Let the thumbnails go, from wherever the first develop's end is
/// heard.
pub(crate) fn release_thumbnails() {
    WORKER.with(|w| {
        if let Some(w) = &*w.borrow() {
            w.hold_thumbnails(false);
        }
    });
}

/// How long a snapshot waits for the grid's pictures before it takes
/// the grid as it stands. A cold folder of a few hundred fills in a
/// few seconds; a picture that has not come by this is not coming.
pub(crate) const GRID_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The snapshot's wait for the grid is over: when it is still
/// waiting, the cells without their picture are named in the log and
/// the grid is taken as it stands. Says whether it gave up.
pub(crate) fn give_up_on_grid(st: &mut State, app: &App) -> bool {
    if st.snapshot.is_none() || grid_filled(st, app) {
        return false;
    }
    // What the grid still waits for: a picture that never came, and
    // one that came too small for the cells and whose larger one has
    // not.
    let (mut missing, mut small) = (Vec::new(), Vec::new());
    if let Some((first, last)) = st.grid_shown {
        for f in (first.max(0) as usize..=last.max(0) as usize).filter_map(|r| st.shown.get(r)) {
            if st.thumb_failed[*f] {
                continue;
            }
            if st.thumb_made[*f] == 0 {
                missing.push(file_name(&st.files[*f]));
            } else if grid::wants_bigger(st.thumb_made[*f], st.thumb_want) {
                small.push(file_name(&st.files[*f]));
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
    st.grid_wait_over = true;
    app.window().request_redraw();
    true
}

/// Whether every frame the grid shows has its picture, at the size
/// the cells ask for. True whenever the grid is closed.
pub(crate) fn grid_filled(st: &State, app: &App) -> bool {
    grid_filled_rows(
        &st.shown,
        &st.thumb_made,
        &st.thumb_failed,
        st.thumb_want,
        st.grid_shown,
        st.grid_wait_over,
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
    slint::Timer::single_shot(std::time::Duration::from_millis(100), move || {
        if let Some(app) = app_weak.upgrade() {
            app.invoke_step(1, false);
        }
    });
}

pub(crate) fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Open the frame on browser row `row`: decode and develop it on the
/// worker, or in culling show its JPEG. It becomes the current frame;
/// the set becomes that frame alone, or with `extend` (Shift and an
/// arrow) keeps what it held and takes this frame as well.
pub(crate) fn open_row(st: &mut State, app: &App, worker: &Worker, row: i32, extend: bool) {
    // The window's rows are the browser's list, which the
    // filter may have shortened; the file is what is opened.
    let Some(&i) = usize::try_from(row).ok().and_then(|r| st.shown.get(r)) else {
        return;
    };
    st.picked = if extend {
        selection::union(&chosen_frames(st), &[i])
    } else {
        vec![i]
    };
    // `--cull`: the first file opens the mode, not a develop.
    if let Some(compare) = st.cull_at_start.take() {
        enter_cull(st, app, compare);
    }
    // In culling nothing is developed: the frame's JPEG shows.
    if st.cull.is_some() {
        cull_select(st, app, i);
        show_set(st, app);
        return;
    }
    // The panel is the file's: keep the old one's, show the new one's.
    // The viewport keeps the old picture under the old look
    // until the new frame's camera JPEG is decoded, which
    // stands in until its develop lands; a file chosen from
    // the strip opens fitted either way.
    if st.current.is_some() {
        let outgoing = read_edit(app, &st.edit, st.target);
        if st.base_white.is_some() {
            st.held = Some(outgoing.clone());
        } else {
            st.zoom = 0.0;
        }
        save_edit(st, outgoing);
    }
    // Before the edit reaches the panel, and so before any
    // refit can run on it: a sidecar from an older build
    // does not know which way up its Original crop goes
    // until the frame does.
    migrate_frame(st, i);
    let mut edit = st.sidecars[i].current.clone();
    // The frame the command line's overrides were on is left, or
    // opened again: they are done with (`panel_state`).
    st.overridden = None;
    // The command line's temperature and exposure, over the first
    // file's edit on the panel and not on its sidecar.
    if let Some((_, overrides)) = st.overrides_at_start.take_if(|(f, _)| *f == i) {
        overrides.apply(&mut edit);
        st.overridden = Some((st.files[i].clone(), edit.clone()));
    }
    // A mask asked for on the command line is the first file's
    // target, so a screenshot can show the panel's block for it.
    st.target = if st.current.is_none() {
        let m = st.show_mask.take().filter(|&m| m < edit.adjustments.len());
        if m.is_some() {
            app.set_show_mask(true);
            app.set_component(0);
        }
        // Likewise a patch, which the panel's list then shows
        // chosen and the viewport outlines.
        if let Some(p) = st.show_patch.take() {
            app.set_patch(p as i32);
        }
        m
    } else {
        None
    };
    st.placing = None;
    app.set_placing("".into());
    // Another picture: a guide stroke drawn on the last one
    // means nothing here, and the source's size is not known
    // again until this frame's develop lands — a turn before
    // that must not map this frame's masks by the last
    // frame's shape.
    st.guiding.clear();
    st.source_size = (0, 0);
    st.turn_pressed = None;
    app.set_guide_mode("".into());
    show_edit(st, &edit, app, st.target);
    st.edit = edit.clone();
    st.current = Some(i);
    st.generation += 1;
    // The key's moment, until the frame that shows this
    // frame: the log's line, and `--time-select`.
    st.selected_at = Some(std::time::Instant::now());
    app.set_selected(row);
    app.set_file_name(file_name(&st.files[i]).into());
    // The last file's shot is not this one's; the open says
    // what this one was.
    app.set_shot_camera("".into());
    app.set_shot_exposure("".into());
    app.set_shot_size("".into());
    app.set_status("decoding...".into());
    app.set_busy(true);
    show_history(st, app);
    worker.send(Job::Open {
        path: st.files[i].clone(),
        edit,
        generation: st.generation,
        seed_blend: st.seed_blend.get(i).copied().unwrap_or(false),
        turn: st.sidecars[i].turn,
    });
    // The frame's own camera JPEG, to stand in for that
    // develop: after the job, so the decode that matters
    // most is under way first.
    start_placeholder(st, app, i);
    // Its neighbors' pictures come as the strip or the grid follows it
    // there and says what it shows.
    show_set(st, app);
}

/// An arrow's landing on browser row `row`, which `moves` when it is
/// not the row already current. A plain arrow opens it as a click
/// would and the set collapses to it, even at the end of the strip
/// where nothing moved; with Shift the set keeps what it had and
/// takes the frame landed on as well.
fn step_to(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Worker,
    row: i32,
    moves: bool,
    extend: bool,
) {
    match (moves, extend) {
        (true, false) => app.invoke_select(row),
        (true, true) => open_row(&mut state.borrow_mut(), app, worker, row, true),
        (false, false) => {
            let mut st = state.borrow_mut();
            st.picked = st.current.into_iter().collect();
            show_set(&mut st, app);
        }
        (false, true) => {}
    }
}

/// Culling's undo (`back`) or redo: the session's last rating, flag
/// or label change taken back or made again, and nothing else, with
/// the word over the picture saying what it left ("Undo: No stars"). The
/// row to select after, for the caller to open once the state is
/// free: the changed frame's, so what changed is on screen, or the
/// nearest when the filter no longer shows the current one. None
/// when there was nothing to step, or nothing to move to.
pub(crate) fn step_tags(st: &mut State, app: &App, back: bool) -> Option<usize> {
    let step = if back { st.tags.undo() } else { st.tags.redo() }?;
    let landing = tags::landing(
        &step,
        |p| st.files.iter().position(|f| f == p),
        |i| Tags::of(&st.sidecars[i].meta),
    );
    let frames: Vec<usize> = landing.iter().map(|&(i, _)| i).collect();
    let was_shown: Vec<bool> = frames.iter().map(|&i| row_of(st, i).is_some()).collect();
    // What the step does to each frame, in the key's terms, for
    // the word; read before the frames are put back.
    let landed: Vec<Option<meta::Change>> = landing
        .iter()
        .map(|&(i, tags)| tags::between(Tags::of(&st.sidecars[i].meta), tags))
        .collect();
    for &(i, tags) in &landing {
        tags.put(&mut st.sidecars[i].meta);
    }
    let next = tags_shown(st, app, &frames, &was_shown, &frames);
    // The step's word, as a key's: silent only when it found no
    // frame to put back.
    if !landed.is_empty() {
        say_notice(st, app, tags::step_notice(back, &landed));
    }
    // Over to the frame that changed, if it is not the one on
    // screen and the filter still shows it.
    if !frames.iter().any(|&i| Some(i) == st.current)
        && let Some(row) = frames.first().and_then(|&i| row_of(st, i))
    {
        return Some(row);
    }
    next
}

/// A culling key's change over the whole selection, as the key and
/// the frame menu both ask for it; false when nothing is selected.
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
    state: &Rc<RefCell<State>>,
    app: &App,
    change: meta::Change,
) -> bool {
    let mut st = state.borrow_mut();
    // The whole selection: the current frame and the set.
    let frames = chosen_frames(&st);
    if frames.is_empty() {
        return false;
    }
    let (settled, next) = set_meta(&mut st, app, &frames, change);
    let culling = st.cull.is_some();
    let move_on = culling && st.cull_move_on && frames.len() == 1;
    if culling {
        say_notice(&mut st, app, tags::notice(settled));
    }
    drop(st);
    // The frame left the filtered list: on to the nearest.
    if let Some(row) = next {
        app.invoke_select(row as i32);
    } else if move_on {
        app.invoke_step(1, false);
    }
    true
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
    st.notice_timer
        .start(slint::TimerMode::SingleShot, NOTICE_UP, move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_notice_on(false);
            }
        });
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // The culling keys: a rating, a flag or a color label on the
    // selection. None of this is an edit, so nothing here records a
    // history state, marks the panel dirty or asks for a develop.
    // A key that names what a frame already carries is still the
    // browser's key and is still eaten: it just writes nothing.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_meta_key(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let Some(change) = meta::Change::from_key(&key) else {
                return false;
            };
            meta_on_selection(&state, &app, change)
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
            let mut st = state.borrow_mut();
            // The whole selection, as the meta keys have it.
            let frames = chosen_frames(&st);
            turn_frames(&mut st, &app, &worker, &frames, quarters);
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
                let row = usize::try_from(app.get_selected())
                    .ok()
                    .or_else(|| st.current.and_then(|c| cull::nearest_row(&st.shown, c)));
                (row, st.shown.len())
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
            let Some(&file) = usize::try_from(row).ok().and_then(|r| st.shown.get(r)) else {
                return;
            };
            match selection::click(&st.picked, &st.shown, st.current, file, ctrl, shift) {
                // A plain click on the frame already on screen: the
                // set goes back to it, and nothing is opened again.
                selection::Click::Open(f) if Some(f) == st.current => {
                    st.picked = vec![f];
                    show_set(&mut st, &app);
                }
                selection::Click::Open(_) => {
                    drop(st);
                    app.invoke_select(row);
                }
                selection::Click::Set(set) => {
                    st.picked = set;
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
            st.picked = st.current.into_iter().collect();
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
            let (Some(&f), Some(&l)) = (st.shown.get(first as usize), st.shown.get(last as usize))
            else {
                return;
            };
            st.strip_shown = Some((first, last));
            worker.want_thumbnails(f, l);
        });
    }
    // The grid's layout, asked of the pure arithmetic in `grid` so
    // the numbers are tested rather than left in a binding.
    app.on_grid_columns(grid::columns);
    app.on_grid_slack(grid::slack);
    app.on_grid_max_scroll(grid::max_scroll);
    app.on_grid_reveal_to(grid::reveal);
    // An arrow in the grid: along the row, or by a whole row, and
    // the frame it lands on is opened as a click on it would.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_grid_step(move |dx, dy, columns, extend| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let count = state.borrow().shown.len() as i32;
            let next = grid::step(app.get_selected(), dx, dy, columns, count);
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
    // a step and the grid re-flows.
    {
        let app_weak = app.as_weak();
        app.on_grid_zoom(move |by| {
            if let Some(app) = app_weak.upgrade() {
                app.set_grid_cell(grid::zoom(app.get_grid_cell(), by));
            }
        });
    }
    // The frames the grid shows, from where it is scrolled: the
    // strip's mechanism over a rectangle rather than a row. A folder
    // of hundreds is never rendered up front; what is on screen
    // jumps the queue, and a cell larger than the pictures were made
    // for asks for those on screen again at the larger size.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_grid_range(move |scroll, height, columns| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The grid is open: its pictures are what is on screen.
            worker.hold_thumbnails(false);
            let cell = app.get_grid_cell();
            let mut st = state.borrow_mut();
            let count = st.shown.len() as i32;
            // The grid's cells follow, and the pictures with them.
            let (at, wanted) = grid::window(scroll, height, cell, columns, count);
            if cells::show(&app, cells::View::Grid, at, wanted) {
                settle_pictures(&mut st, &app);
            }
            let Some((first, last)) = grid::visible(scroll, height, cell, columns, count) else {
                return;
            };
            st.grid_shown = Some((first, last));
            let (first, last) = (
                first.max(0) as usize,
                (last.max(0) as usize).min(st.shown.len().saturating_sub(1)),
            );
            // What is made from now on is this cell's size, up or
            // down: a cell that has shrunk should not go on paying
            // the memory of the one before it.
            let want = grid::render_size(cell);
            if want != st.thumb_want {
                st.thumb_want = want;
                worker.set_thumb_size(want);
            }
            // Making one again is the part that is only ever a step
            // up: only pictures already on screen and a quarter too
            // small, and only once each. One still in the queue will
            // come back at the size the worker now has anyway, and
            // asking again would only make it twice.
            for row in first..=last {
                let i = st.shown[row];
                if st.thumb_made[i] > 0
                    && grid::wants_bigger(st.thumb_made[i], want)
                    && st.thumb_asked[i] < want
                {
                    st.thumb_asked[i] = want;
                    worker.send(Job::Thumbnail {
                        index: i,
                        path: st.files[i].clone(),
                    });
                }
            }
            // After the pushes, which clear the queue's order.
            worker.want_thumbnails(st.shown[first], st.shown[last]);
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
                    .and_then(|i| st.files.get(i))
                    .and_then(|f| f.parent())
                    .map(Path::to_path_buf)
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/"))
            };
            let app_weak = app.as_weak();
            app.set_status("choosing a folder to open...".into());
            export::choose_folder("Open folder", start, move |chosen| {
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
                        Ok(None) => app.set_status("open folder canceled".into()),
                        Err(e) => {
                            tracing::warn!("file chooser: {e:#}");
                            app.set_status("the desktop offered no file chooser".into());
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
mod tests {
    use super::*;
    use crate::testing::{press, window};
    use slint::platform::{Key, WindowEvent};

    /// A row of the browser and a file of the folder are two
    /// different numbers whenever a filter is on. The badges wrote
    /// the file's number into the list, so a star set under "No
    /// rejects" landed on whichever frame happened to be that many
    /// rows down — someone else's thumbnail, not an index off the
    /// end, which is why nothing ever panicked over it.
    #[test]
    fn a_badge_set_under_a_filter_lands_on_the_filtered_row() {
        use meta::{Change, Flag, Label};
        let app = window(0);
        let files: Vec<PathBuf> = ["a.CR3", "b.CR3", "c.CR3", "d.CR3", "e.CR3"]
            .iter()
            .map(|n| PathBuf::from("/nowhere").join(n))
            .collect();
        let mut st = State::empty(files, &app);
        // One reject at the front, so every row is the file one
        // along: row 0 is file 1, and the file numbers 1 to 4 are
        // all rows that exist. The old code wrote file 1's badge at
        // row 1, which is file 2 — a neighbor, in range, wrong.
        st.sidecars[0].meta.flag = Flag::Reject;
        st.filter = filter::Filter::from_name("No rejects").expect("it parses");
        rebuild_browser(&mut st, &app);
        assert_eq!(st.shown, vec![1, 2, 3, 4]);
        assert_eq!(app.get_thumbs().row_count(), 4);

        // Four stars on file 1: row 0, and nothing on row 1.
        set_meta(&mut st, &app, &[1], Change::Rating(4));
        let rows = app.get_thumbs();
        assert_eq!(rows.row_data(0).unwrap().name, "b.CR3");
        assert_eq!(rows.row_data(0).unwrap().rating, 4, "the file's own row");
        assert_eq!(
            rows.row_data(1).unwrap().rating,
            0,
            "and not its neighbor's"
        );
        assert!(
            (2..4).all(|r| rows.row_data(r).unwrap().rating == 0),
            "and nobody else's"
        );

        // A label on the last file shown, which is file 4 at row 3.
        set_meta(&mut st, &app, &[4], Change::Label(Label::Red));
        let rows = app.get_thumbs();
        assert_eq!(rows.row_data(3).unwrap().name, "e.CR3");
        assert_eq!(rows.row_data(3).unwrap().label, Label::Red.code());
        assert!(
            (0..3).all(|r| rows.row_data(r).unwrap().label == 0),
            "the label went to one row"
        );
        // And the star set a moment ago is still where it was put.
        assert_eq!(rows.row_data(0).unwrap().rating, 4);
    }

    #[test]
    fn a_rating_from_another_tool_arrives_and_the_folder_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("greycard-xmp-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let theirs = dir.join("IMG_0001.CR3");
        let mine = dir.join("IMG_0002.CR3");
        for f in [&theirs, &mine] {
            std::fs::write(f, b"raw").expect("a file");
        }
        let three = Meta {
            rating: 3,
            ..Meta::default()
        };

        // Rated in Lightroom, never opened here: the XMP is all
        // there is, so its word is taken.
        std::fs::write(xmp::short_path(&theirs), xmp::fresh(&three, None)).expect("an xmp");
        let (sidecars, _) = load_sidecars(std::slice::from_ref(&theirs), true);
        assert_eq!(sidecars[0].meta, three);
        // And nothing was written into somebody else's folder for
        // it: the meta rides in memory until the frame is saved.
        assert!(
            !Sidecar::path_for(&theirs).exists(),
            "opening a folder deposits nothing"
        );

        // Cleared here after the XMP was taken, and saved: the XMP
        // still says three and is older than nothing in particular,
        // but it is the same XMP that was read, so it has nothing
        // new to say and the three do not come back.
        let mut sidecar = Sidecar::default();
        std::fs::write(xmp::short_path(&mine), xmp::fresh(&three, None)).expect("an xmp");
        assert!(greycard_edit::xmp::adopt(&mine, &mut sidecar, || None).moved());
        sidecar.meta.set_rating(0);
        sidecar.save(&mine).expect("the sidecar writes");
        set_modified(&xmp::short_path(&mine), 2_000_000_000);
        set_modified(&Sidecar::path_for(&mine), 1_000_000_000);
        assert_eq!(
            load_sidecars(std::slice::from_ref(&mine), true).0[0]
                .meta
                .rating,
            0,
            "a newer mtime is not a newer opinion"
        );

        // With `--no-sidecars` neither file is read at all.
        assert_eq!(
            load_sidecars(&[theirs, mine], false).0[0].meta,
            Meta::default()
        );
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }

    /// A file's modification time, set, so the newer-wins rule can
    /// be tested without sleeping through a filesystem's resolution.
    fn set_modified(path: &Path, secs: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("the file opens")
            .set_modified(t)
            .expect("the time is set");
    }

    #[test]
    fn the_badges_read_the_meta() {
        use meta::{Flag, Label};
        let meta = Meta {
            rating: 200,
            flag: Flag::Reject,
            label: Label::Purple,
            ..Meta::default()
        };
        // A rating past the range does not draw two hundred stars.
        let thumb = thumb_for(Path::new("/tmp/IMG_0001.CR3"), &meta);
        assert_eq!(thumb.name, "IMG_0001.CR3");
        assert_eq!(thumb.rating, 5);
        assert_eq!(thumb.flag, 2);
        assert_eq!(thumb.label, 5);
        let bare = thumb_for(Path::new("/tmp/a.CR3"), &Meta::default());
        assert_eq!((bare.rating, bare.flag, bare.label), (0, 0, 0));
        assert_eq!(Flag::Pick.code(), 1);
        assert_eq!(Label::Red.code(), 1);
        assert_eq!(Label::Blue.code(), 4);
    }

    /// A two-pixel picture for file `index`, as the pool delivers one.
    fn picture_for(files: &[PathBuf], index: usize) -> crate::worker::Outcome {
        crate::worker::Outcome::Thumbnail {
            index,
            path: files[index].clone(),
            size: 176,
            width: 2,
            height: 1,
            rgb: vec![0; 6],
            cached: false,
            seconds: 0.0,
        }
    }

    /// A file no picture can be made of marks its cell, on the grid
    /// and after the rows are made again, and a snapshot of the grid
    /// counts it as filled; a picture that does come clears it.
    #[test]
    fn a_file_with_no_picture_marks_its_cell_and_fills_the_grid() {
        use crate::panel::deliver::deliver;
        let app = window(3);
        let files = crate::testing::folder(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        app.set_grid_open(true);
        app.invoke_grid_range(0.0, 773.0, 8);
        assert_eq!(state.borrow().grid_shown, Some((0, 2)));
        let failed = |row: usize| app.get_thumbs().row_data(row).unwrap().failed;
        deliver(&app, picture_for(&files, 0));
        deliver(&app, picture_for(&files, 2));
        assert!(!grid_filled(&state.borrow(), &app));
        deliver(
            &app,
            crate::worker::Outcome::NoThumbnail {
                index: 1,
                path: files[1].clone(),
            },
        );
        assert!(failed(1));
        assert!(!failed(0) && !failed(2));
        assert!(grid_filled(&state.borrow(), &app));
        rebuild_browser(&mut state.borrow_mut(), &app);
        assert!(failed(1), "kept when the rows are made again");
        // Another folder's failure, by the path it carries, is not
        // this one's.
        deliver(
            &app,
            crate::worker::Outcome::NoThumbnail {
                index: 0,
                path: PathBuf::from("/elsewhere/IMG_0000.CR3"),
            },
        );
        assert!(!failed(0));
        deliver(&app, picture_for(&files, 1));
        assert!(!failed(1));
        assert!(!state.borrow().thumb_failed[1]);
        // A larger picture that fails leaves the one there, unmarked,
        // and the cell filled.
        state.borrow_mut().thumb_want = 360;
        assert!(!grid_filled(&state.borrow(), &app));
        deliver(
            &app,
            crate::worker::Outcome::NoThumbnail {
                index: 0,
                path: files[0].clone(),
            },
        );
        assert!(!failed(0));
        assert!(state.borrow().thumb_base[0].is_some());
        state.borrow_mut().thumb_made[1] = 360;
        state.borrow_mut().thumb_made[2] = 360;
        assert!(grid_filled(&state.borrow(), &app));
    }

    /// A row carries its picture while the grid or the strip has a cell
    /// for it, and only then: one delivered for a row without a cell is
    /// kept in hand, goes on when a view comes to the row, and comes
    /// off when the views have left it. A batch from the pool is taken
    /// in one call.
    #[test]
    fn a_row_carries_its_picture_while_a_view_has_a_cell_for_it() {
        use crate::panel::deliver::deliver;
        use crate::worker::Outcome;
        let app = window(1000);
        let files = crate::testing::folder(1000);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        let pictured = |row: usize| app.get_thumbs().row_data(row).unwrap().image.size().width;
        app.set_grid_open(true);
        // Eight columns, four rows and a part on screen: cells for
        // rows 0 to 6, frames 0 to 55.
        app.invoke_grid_range(0.0, 773.0, 8);
        assert_eq!(cells::window(&app, cells::View::Grid), (0, 56));
        deliver(
            &app,
            Outcome::Thumbnails(crate::thumbpool::Batch::of(
                [5, 300, 900].map(|i| picture_for(&files, i)).into(),
            )),
        );
        {
            let st = state.borrow();
            assert!([5, 300, 900].iter().all(|&i| st.thumb_base[i].is_some()));
        }
        assert_eq!(pictured(5), 2, "a cell: on its row");
        assert_eq!(pictured(300), 0, "no cell: kept for later");
        assert_eq!(pictured(900), 0);
        // The grid scrolled to 900's screenful: on, and 5 off.
        app.invoke_grid_range(grid::PAD + 111.0 * 206.0, 773.0, 8);
        assert_eq!(pictured(900), 2);
        assert_eq!(pictured(5), 0);
        assert!(state.borrow().thumb_shown[5].is_none());
        assert!(state.borrow().thumb_base[5].is_some(), "still in hand");
        assert_eq!(pictured(300), 0);
        // The strip, through its own report.
        app.invoke_strip_range(295, 305);
        assert_eq!(pictured(300), 2);
        // Back to the top: 5 on again, 900 off, 300 kept by the strip.
        app.invoke_grid_range(0.0, 773.0, 8);
        assert_eq!((pictured(5), pictured(900), pictured(300)), (2, 0, 2));
        // A larger picture for a row without a cell waits in hand, and
        // is the one put on when the grid comes back to it.
        deliver(
            &app,
            Outcome::Thumbnail {
                index: 900,
                path: files[900].clone(),
                size: 360,
                width: 4,
                height: 2,
                rgb: vec![0; 24],
                cached: true,
                seconds: 0.0,
            },
        );
        assert_eq!(pictured(900), 0);
        app.invoke_grid_range(grid::PAD + 111.0 * 206.0, 773.0, 8);
        assert_eq!(pictured(900), 4);
    }

    /// A picture that came while the filter hid its frame goes on its
    /// row when the filter is cleared only if a view has a cell for the
    /// row: a cleared filter over thousands would otherwise put them
    /// all on in one call.
    #[test]
    fn pictures_that_came_while_hidden_wait_for_a_cell_when_the_filter_clears() {
        use crate::panel::deliver::deliver;
        let app = window(0);
        let files = crate::testing::folder(1000);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            for i in 500..1000 {
                st.sidecars[i].meta.flag = meta::Flag::Reject;
            }
            st.filter = filter::Filter::from_name("No rejects").expect("it parses");
            rebuild_browser(&mut st, &app);
            assert_eq!(st.shown.len(), 500);
        }
        app.set_grid_open(true);
        app.invoke_grid_range(0.0, 773.0, 8);
        deliver(&app, picture_for(&files, 5));
        deliver(&app, picture_for(&files, 900));
        let pictured = |row: usize| app.get_thumbs().row_data(row).unwrap().image.size().width;
        {
            let mut st = state.borrow_mut();
            assert!(st.thumb_base[900].is_some() && st.thumb_shown[900].is_none());
            st.filter = filter::Filter::from_name("All").expect("it parses");
            rebuild_browser(&mut st, &app);
        }
        assert_eq!(pictured(5), 2, "a cell: made again on its row");
        assert_eq!(pictured(900), 0, "no cell: kept for later");
        app.invoke_grid_range(grid::PAD + 111.0 * 206.0, 773.0, 8);
        assert_eq!(pictured(900), 2);
    }

    /// A batch larger than a turn takes: the first turn takes one slice
    /// and leaves the rest to a timer, whose turn takes them.
    #[test]
    fn a_batch_left_over_a_turn_is_taken_on_the_next() {
        use crate::panel::deliver::take_thumbnails;
        let app = window(100);
        let files = crate::testing::folder(100);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        // A sheet tall enough for a cell for every frame.
        app.invoke_grid_range(0.0, 3000.0, 8);
        let batch = crate::thumbpool::Batch::of((0..100).map(|i| picture_for(&files, i)).collect());
        take_thumbnails(&app, &state, batch, std::time::Duration::ZERO);
        let taken = |st: &State| st.thumb_base.iter().filter(|b| b.is_some()).count();
        assert_eq!(taken(&state.borrow()), 32, "one slice in a turn of nothing");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(taken(&state.borrow()), 100, "the rest on the timer's turn");
        assert!((0..100).all(|r| app.get_thumbs().row_data(r).unwrap().image.size().width == 2));
    }

    /// How many cells of the strip, or of the grid, are on screen: the
    /// testing backend finds only elements its clip leaves showing.
    /// Every cell a view's model holds is made, a `for` outside a
    /// `ListView` making one for each row, so the model's count is the
    /// count made.
    fn cells_on_screen(app: &App, id: &str) -> usize {
        // A cell a model change has just brought in is built on the
        // next event: a pointer move at the corner, where nothing is.
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        i_slint_backend_testing::ElementHandle::find_by_element_id(app, id).count()
    }

    /// The rows a view's cells show, sorted.
    fn cell_rows(model: slint::ModelRc<crate::ThumbCell>) -> Vec<i32> {
        let mut rows: Vec<i32> = model.iter().map(|c| c.row).collect();
        rows.sort();
        rows
    }

    /// A browser of five thousand frames in a 1500 by 950 window,
    /// opened on the first, with pictures in hand for every frame.
    fn five_thousand() -> (App, Rc<RefCell<State>>, Vec<PathBuf>) {
        use crate::panel::deliver::deliver;
        let app = window(5000);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let files = crate::testing::folder(5000);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        state.borrow_mut().current = Some(0);
        app.set_selected(0);
        press(&app, Key::Shift);
        deliver(
            &app,
            crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(
                (0..5000).map(|i| picture_for(&files, i)).collect(),
            )),
        );
        // The whole batch, over as many turns as it takes.
        for _ in 0..500 {
            if state.borrow().thumb_base.iter().all(Option::is_some) {
                break;
            }
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        }
        assert!(state.borrow().thumb_base.iter().all(Option::is_some));
        (app, state, files)
    }

    /// The strip and the grid make cells for the rows on screen and a
    /// margin, however long the list: five thousand frames are a strip
    /// of a screen's width and a grid of seven rows, and no more rows
    /// than those carry a picture.
    #[test]
    fn five_thousand_frames_make_cells_for_the_screen_alone() {
        let (app, state, _files) = five_thousand();
        // The strip, 1500 wide at 186 a frame: eight and a part on
        // screen, and cells for a part more and the margin either side.
        assert_eq!(cells_on_screen(&app, "Filmstrip::cell"), 9);
        let strip = app.get_strip_cells().row_count();
        assert!(strip <= 10 + 2 * cells::STRIP_MARGIN, "{strip}");
        assert_eq!(app.get_cells_made() as usize, strip, "the grid is closed");
        assert_eq!(cells_on_screen(&app, "GridSheet::cell"), 0);
        press(&app, "g");
        // Eight columns; four rows on screen at the top, as the layout
        // test has it, and a fifth's part as it scrolls; and cells for
        // a row either side.
        assert_eq!(cells_on_screen(&app, "GridSheet::cell"), 4 * 8);
        let grid = app.get_grid_cells().row_count();
        assert_eq!(grid, 7 * 8);
        assert_eq!(app.get_cells_made() as usize, strip + grid);
        assert_eq!(cell_rows(app.get_grid_cells()), (0..56).collect::<Vec<_>>());
        // What carries a picture is what has a cell.
        let rows = app.get_thumbs();
        let pictured = (0..5000)
            .filter(|&r| rows.row_data(r).unwrap().image.size().width > 0)
            .count();
        assert_eq!(pictured, 56, "the grid's cells take in the strip's");
        let st = state.borrow();
        assert_eq!(st.thumb_shown.iter().filter(|s| s.is_some()).count(), 56);
    }

    /// Scrolled to the far end and back, the grid and the strip still
    /// have the current frame chosen, ringed and pictured, and a
    /// picture that came for it while it was off screen is the one it
    /// shows.
    #[test]
    fn scrolling_to_the_end_and_back_keeps_the_current_frame_and_its_picture() {
        use crate::panel::deliver::deliver;
        use crate::worker::Outcome;
        let (app, state, files) = five_thousand();
        let cell_of =
            |model: slint::ModelRc<crate::ThumbCell>, row: i32| model.iter().find(|c| c.row == row);
        for grid in [true, false] {
            if grid {
                press(&app, "g");
            }
            let cells = || {
                if grid {
                    app.get_grid_cells()
                } else {
                    app.get_strip_cells()
                }
            };
            let made = app.get_cells_made();
            assert!(cell_of(cells(), 0).is_some());
            assert!(app.invoke_scroll_by(1e9));
            press(&app, Key::Shift);
            assert!(
                cell_of(cells(), 4999).is_some(),
                "the last frame has a cell"
            );
            assert!(cell_of(cells(), 0).is_none());
            assert_eq!(app.get_cells_made(), made, "as many cells at the end");
            assert_eq!(app.get_selected(), 0);
            // Its picture is on its row while the other view has a cell
            // for it, and off when neither has.
            assert_eq!(
                state.borrow().thumb_shown[0].is_some(),
                cells::has_cell(&app, 0)
            );
            // A new picture for it while it is away.
            let (w, h) = if grid { (6, 3) } else { (8, 4) };
            deliver(
                &app,
                Outcome::Thumbnail {
                    index: 0,
                    path: files[0].clone(),
                    size: 360,
                    width: w,
                    height: h,
                    rgb: vec![0; (w * h * 3) as usize],
                    cached: true,
                    seconds: 0.0,
                },
            );
            assert!(app.invoke_scroll_by(-1e9));
            press(&app, Key::Shift);
            let back = cell_of(cells(), 0).expect("the first frame's cell again");
            assert_eq!(back.thumb.image.size().width, w, "the newer picture");
            assert_eq!(app.get_selected(), 0, "still the current frame, ringed");
            assert_eq!(state.borrow().current, Some(0));
            if grid {
                press(&app, "g");
            }
        }
    }

    /// A frame far down the folder, made current, is scrolled into
    /// view in the grid and in the strip, and has a cell on screen in
    /// each.
    #[test]
    fn a_far_frame_made_current_is_revealed_with_a_cell() {
        let (app, _state, _files) = five_thousand();
        press(&app, "g");
        app.set_selected(4321);
        press(&app, Key::Shift);
        let rows = cell_rows(app.get_grid_cells());
        assert!(rows.contains(&4321), "{rows:?}");
        let (first, count) = cells::window(&app, cells::View::Grid);
        assert!(first <= 4321 && 4321 < first + count);
        assert_eq!(
            app.get_thumbs().row_data(4321).unwrap().image.size().width,
            2
        );
        assert_eq!(cells_on_screen(&app, "GridSheet::cell"), 5 * 8);
        // The strip, out of the grid.
        press(&app, "g");
        app.set_selected(3210);
        press(&app, Key::Shift);
        let rows = cell_rows(app.get_strip_cells());
        assert!(rows.contains(&3210), "{rows:?}");
        assert_eq!(
            app.get_thumbs().row_data(3210).unwrap().image.size().width,
            2
        );
        assert!(cells_on_screen(&app, "Filmstrip::cell") >= 8);
    }

    /// The window's share of a turn: as long as it spent drawing and
    /// listening since the last, within its bounds.
    #[test]
    fn a_turn_takes_pictures_in_for_as_long_as_the_frame_took() {
        use crate::panel::deliver::{THUMB_TURN, THUMB_TURN_MOST, thumb_turn};
        use std::time::Duration;
        assert_eq!(thumb_turn(None), THUMB_TURN);
        assert_eq!(thumb_turn(Some(Duration::from_millis(1))), THUMB_TURN);
        assert_eq!(
            thumb_turn(Some(Duration::from_millis(60))),
            Duration::from_millis(60)
        );
        assert_eq!(thumb_turn(Some(Duration::from_secs(2))), THUMB_TURN_MOST);
    }

    /// A snapshot of a grid whose pictures have not all come stops
    /// waiting at its limit and takes the grid as it stands; without
    /// a snapshot, or with the grid filled, there is nothing to give
    /// up.
    #[test]
    fn a_snapshot_of_the_grid_stops_waiting_at_its_limit() {
        use crate::panel::deliver::deliver;
        let app = window(3);
        let files = crate::testing::folder(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        app.set_grid_open(true);
        state.borrow_mut().grid_shown = Some((0, 2));
        deliver(&app, picture_for(&files, 0));
        assert!(!give_up_on_grid(&mut state.borrow_mut(), &app));
        assert!(!grid_filled(&state.borrow(), &app));
        state.borrow_mut().snapshot = Some(PathBuf::from("grid.png"));
        assert!(give_up_on_grid(&mut state.borrow_mut(), &app));
        assert!(grid_filled(&state.borrow(), &app));
        // Every picture in, but one too small for the cells, its
        // larger one not come: still waited for, and given up on.
        let app = window(2);
        let files = crate::testing::folder(2);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        app.set_grid_open(true);
        state.borrow_mut().grid_shown = Some((0, 1));
        state.borrow_mut().snapshot = Some(PathBuf::from("grid.png"));
        deliver(&app, picture_for(&files, 0));
        deliver(&app, picture_for(&files, 1));
        state.borrow_mut().thumb_want = 360;
        assert!(give_up_on_grid(&mut state.borrow_mut(), &app));
        // A grid filled in time is not given up on.
        let app = window(2);
        let files = crate::testing::folder(2);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        app.set_grid_open(true);
        state.borrow_mut().grid_shown = Some((0, 1));
        state.borrow_mut().snapshot = Some(PathBuf::from("grid.png"));
        deliver(&app, picture_for(&files, 0));
        deliver(&app, picture_for(&files, 1));
        assert!(!give_up_on_grid(&mut state.borrow_mut(), &app));
    }

    /// The filmstrip's picture, the grid's and the culling loupe's are
    /// the camera's own, so a frame's turn rides in the same quarter
    /// turns the edit's do: what comes out is the picture the develop
    /// would make of the turned frame.
    #[test]
    fn a_thumbnail_follows_the_frames_own_turn() {
        use greycard_core::WorkingImage;
        use greycard_core::develop::orient;
        use greycard_core::raw::Orientation;
        let rgb: Vec<u8> = (0..6).flat_map(|i| [i, i, i]).collect();
        let plane = |w: u32, h: u32, rgb: &[u8]| {
            WorkingImage::from_data(
                w as usize,
                h as usize,
                rgb.iter().map(|&b| b as f32).collect(),
            )
            .unwrap()
        };
        for geometry in [
            Geometry::default(),
            Geometry {
                turns: 1,
                ..Geometry::default()
            },
            Geometry {
                flip: true,
                ..Geometry::default()
            },
            Geometry {
                turns: 3,
                flip: true,
                ..Geometry::default()
            },
        ] {
            for turn in 0..4u8 {
                // The develop: the source turned by the frame's own
                // quarter turns, then the edit's geometry on top.
                let turned = orient(
                    plane(3, 2, &rgb),
                    Orientation::from_exif(match turn {
                        1 => 6,
                        2 => 3,
                        3 => 8,
                        _ => 1,
                    })
                    .unwrap(),
                );
                let bytes: Vec<u8> = turned.data.iter().map(|&v| v as u8).collect();
                let under = geometry.under_turned_source(i32::from(turn));
                let want = greycard_edit::geometry::turn_pixels(
                    turned.width as u32,
                    turned.height as u32,
                    &bytes,
                    under.turns,
                    under.flip,
                );
                // The strip: the camera's picture, with the turn
                // folded into the quarter turns.
                let (turns, flip) = under.shown_turns(turn);
                let got = greycard_edit::geometry::turn_pixels(3, 2, &rgb, turns, flip);
                assert_eq!(got, want, "{geometry:?} + {turn}");
            }
        }
    }

    #[test]
    fn g_opens_the_grid_and_three_keys_leave_it() {
        let app = window(11);
        assert!(!app.get_grid_open());
        press(&app, "g");
        assert!(app.get_grid_open());
        press(&app, Key::Return);
        assert!(!app.get_grid_open());
        press(&app, "G");
        assert!(app.get_grid_open());
        press(&app, Key::Escape);
        assert!(!app.get_grid_open());
        press(&app, "g");
        press(&app, "g");
        assert!(!app.get_grid_open());
    }

    #[test]
    fn the_arrows_step_the_strip_in_one_dimension_and_the_grid_in_two() {
        let app = window(11);
        let steps = Rc::new(RefCell::new(Vec::new()));
        let flat = steps.clone();
        app.on_step(move |by, _| flat.borrow_mut().push((by, 0)));
        let two = steps.clone();
        app.on_grid_step(move |dx, dy, cols, _| {
            assert!(cols >= 1, "the grid always has a column");
            two.borrow_mut().push((dx, dy));
        });
        // In the loupe the arrows are the strip's, along the folder.
        press(&app, Key::LeftArrow);
        press(&app, Key::RightArrow);
        press(&app, Key::UpArrow);
        assert_eq!(*steps.borrow(), vec![(-1, 0), (1, 0)]);
        steps.borrow_mut().clear();
        // In the grid they are the grid's, and the vertical pair
        // reaches it too.
        press(&app, "g");
        press(&app, Key::LeftArrow);
        press(&app, Key::RightArrow);
        press(&app, Key::UpArrow);
        press(&app, Key::DownArrow);
        assert_eq!(*steps.borrow(), vec![(-1, 0), (1, 0), (0, -1), (0, 1)]);
    }

    #[test]
    fn g_at_a_sized_window_reads_the_sheet_it_is_born_at() {
        // Opened by G with the window already at its size, the sheet
        // is created at that size and `changed width` has nothing to
        // say; the first release laid one column down the left.
        let app = window(120);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let reports = seen.clone();
        app.on_grid_range(move |scroll, height, cols| {
            reports.borrow_mut().push((scroll, height, cols));
        });
        app.set_selected(0);
        press(&app, "g");
        assert!(
            app.get_grid_w() > 1000.0,
            "sheet width {}",
            app.get_grid_w()
        );
        assert!(
            app.get_grid_h() > 500.0,
            "sheet height {}",
            app.get_grid_h()
        );
        let cols = grid::columns(app.get_grid_w(), 176.0);
        assert_eq!(cols, 8);
        assert_eq!(seen.borrow().last().map(|r| r.2), Some(8));
    }

    #[test]
    fn the_grid_lays_out_and_reports_what_it_shows() {
        let app = window(120);
        // The sheet a 1500 by 950 window leaves.
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let reports = seen.clone();
        app.on_grid_range(move |scroll, height, cols| {
            reports.borrow_mut().push((scroll, height, cols));
        });
        let steps = Rc::new(RefCell::new(0));
        let cols = steps.clone();
        app.on_grid_step(move |_, _, c, _| *cols.borrow_mut() = c);
        app.set_selected(0);
        press(&app, "g");
        press(&app, Key::DownArrow);
        // Eight 176 cells across 1500, rows 206 apart: under a
        // header of two rows, the library's roots, the filter's chips
        // and its facets the sheet shows four rows and none of a
        // fifth, so the first thirty-two frames are what the worker
        // is told to make first.
        assert_eq!(*steps.borrow(), 8);
        assert_eq!(*seen.borrow(), vec![(0.0, 773.0, 8)]);
        assert_eq!(grid::visible(0.0, 773.0, 176.0, 8, 120), Some((0, 31)));
        // A frame at the foot of the sheet scrolls it there, and what
        // it says it shows follows the scroll.
        seen.borrow_mut().clear();
        app.set_selected(119);
        // Slint runs the changed handlers with the next event, which
        // in a window is the next frame.
        press(&app, Key::Shift);
        let scrolled = grid::reveal(0.0, 773.0, 176.0, 8, 120, 119);
        assert_eq!(scrolled, grid::max_scroll(773.0, 176.0, 8, 120));
        assert_eq!(*seen.borrow(), vec![(scrolled, 773.0, 8)]);
        assert_eq!(
            grid::visible(scrolled, 773.0, 176.0, 8, 120),
            Some((88, 119))
        );
    }

    #[test]
    fn a_sheet_owns_the_keys_over_the_grid_and_a_new_folder_re_flows_it() {
        let app = window(120);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        app.set_selected(0);
        press(&app, "g");
        assert!(app.get_grid_open());
        // A sheet is drawn over the grid, so the keys are its own:
        // neither G nor Return reaches the grid under it.
        app.set_export_open(true);
        press(&app, "g");
        press(&app, Key::Return);
        assert!(app.get_grid_open());
        app.set_export_open(false);
        press(&app, "g");
        assert!(!app.get_grid_open());
        // Another folder while the grid is up: it re-flows and says
        // afresh what it shows, rather than keeping the old scroll.
        press(&app, "g");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let reports = seen.clone();
        app.on_grid_range(move |scroll, _, cols| reports.borrow_mut().push((scroll, cols)));
        let twelve: Vec<Thumb> = (0..12)
            .map(|i| Thumb {
                name: format!("n{i:02}.CR3").into(),
                image: slint::Image::default(),
                ..Default::default()
            })
            .collect();
        cells::set_rows(&app, twelve);
        press(&app, Key::Shift);
        assert_eq!(*seen.borrow(), vec![(0.0, 8)]);
    }

    /// The shape a turn maps masks by is the frame's own, and a
    /// frame whose develop has not landed has no shape on the panel
    /// to borrow: the last frame's would map this one's masks by an
    /// aspect that is not theirs, and say nothing about it.
    #[test]
    fn a_frame_without_its_own_develop_does_not_borrow_the_last_ones_shape() {
        let app = window(2);
        let files = vec![
            PathBuf::from("/nowhere/a.CR3"),
            PathBuf::from("/nowhere/b.CR3"),
        ];
        let mut st = State::empty(files, &app);
        // Nothing developed yet, and nothing else to measure by.
        assert_eq!(st.source_size, (0, 0));
        st.current = Some(0);
        assert_eq!(frame_aspect(&mut st, 0), None);
        // The first frame's develop lands: landscape.
        st.source_size = (6000, 4000);
        assert_eq!(frame_aspect(&mut st, 0), Some(1.5));
        // The selection moves, as `on_select` leaves it: the size is
        // forgotten, and the second frame has no shape of its own to
        // be had (its file is not there), so nothing is claimed.
        st.current = Some(1);
        st.source_size = (0, 0);
        assert_eq!(
            frame_aspect(&mut st, 1),
            None,
            "the last frame's shape is not this one's"
        );
        // Its thumbnail arrives, portrait: that is what answers, and
        // the frame's own turn goes on top of it.
        st.thumb_base[1] = Some((400, 600, Vec::new()));
        assert_eq!(frame_aspect(&mut st, 1), Some(400.0 / 600.0));
        st.sidecars[1].turn = 1;
        assert_eq!(frame_aspect(&mut st, 1), Some(600.0 / 400.0));
    }

    /// The frame's own turn: a bracket either way, unmodified, and
    /// nobody else's when a modifier is down or a sheet is over the
    /// window.
    #[test]
    fn the_brackets_turn_the_frame_and_a_modifier_takes_them_away() {
        let app = window(11);
        let turns = Rc::new(RefCell::new(Vec::new()));
        let seen = turns.clone();
        app.on_frame_turned(move |q| seen.borrow_mut().push(q));
        press(&app, "[");
        press(&app, "]");
        assert_eq!(*turns.borrow(), vec![-1, 1]);
        // In culling and on the grid alike.
        app.set_culling(true);
        press(&app, "]");
        app.set_culling(false);
        app.set_grid_open(true);
        press(&app, "[");
        app.set_grid_open(false);
        assert_eq!(*turns.borrow(), vec![-1, 1, 1, -1]);
        // Ctrl+bracket is somebody else's, and a sheet keeps its own
        // keys.
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(&app, "[");
        press(&app, "]");
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        assert_eq!(turns.borrow().len(), 4);
        app.set_export_open(true);
        press(&app, "[");
        press(&app, "]");
        assert_eq!(turns.borrow().len(), 4);
    }

    #[test]
    fn the_culling_keys_reach_the_browser_in_both_views() {
        let app = window(11);
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let seen = pressed.clone();
        app.on_meta_key(move |key| {
            // What the window sends is the key's text; what it means
            // is `meta::Change`'s to say, as it is in the editor.
            match meta::Change::from_key(&key) {
                Some(_) => {
                    seen.borrow_mut().push(key.to_string());
                    true
                }
                None => false,
            }
        });
        let zooms = Rc::new(RefCell::new(0));
        let counted = zooms.clone();
        app.on_zoom_key(move |_| *counted.borrow_mut() += 1);
        // The loupe: the strip is the browser there.
        for key in ["1", "5", "0", "p", "X", "u", "6", "9"] {
            press(&app, key);
        }
        assert_eq!(*pressed.borrow(), ["1", "5", "0", "p", "X", "u", "6", "9"]);
        // And the grid, where the plus and minus are still its own.
        press(&app, "g");
        press(&app, "3");
        press(&app, "+");
        assert_eq!(pressed.borrow().last().map(String::as_str), Some("3"));
        // A sheet over the window takes every key it is shown.
        app.set_export_open(true);
        press(&app, "4");
        assert_eq!(pressed.borrow().last().map(String::as_str), Some("3"));
        app.set_export_open(false);
        // The keys that are not the browser's are untouched: Z still
        // zooms, and G still closes the grid.
        press(&app, "g");
        press(&app, "z");
        assert_eq!(*zooms.borrow(), 1);
        assert!(!app.get_grid_open());
        assert_eq!(pressed.borrow().len(), 9);
    }

    #[test]
    fn plus_and_minus_size_the_cells_only_in_the_grid() {
        let app = window(11);
        let zooms = Rc::new(RefCell::new(Vec::new()));
        let seen = zooms.clone();
        app.on_grid_zoom(move |by| seen.borrow_mut().push(by));
        press(&app, "+");
        assert!(zooms.borrow().is_empty());
        press(&app, "g");
        press(&app, "+");
        press(&app, "=");
        press(&app, "-");
        assert_eq!(*zooms.borrow(), vec![1, 1, -1]);
    }

    #[test]
    fn a_frame_rated_but_never_developed_still_gets_its_iso_blend() {
        let dir = std::env::temp_dir().join(format!("greycard-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let rated = dir.join("IMG_0001.CR3");
        let developed = dir.join("IMG_0002.CR3");
        let untouched = dir.join("IMG_0003.CR3");
        let picture = dir.join("IMG_0004.JPG");

        // A star and nothing else: a sidecar with a default edit.
        let mut sidecar = Sidecar::default();
        sidecar.meta.set_rating(4);
        sidecar.save(&rated).expect("the sidecar writes");
        assert!(files::never_developed(&sidecar));
        // One that has been developed, however lightly.
        let mut sidecar = Sidecar::default();
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        sidecar.record(edit);
        sidecar.save(&developed).expect("the sidecar writes");
        assert!(!files::never_developed(&sidecar));
        // And one rated in a picture that is not a raw, which has no
        // learned denoiser to seed.
        let mut sidecar = Sidecar {
            current: Edit::for_picture(),
            ..Sidecar::default()
        };
        sidecar.meta.set_rating(1);
        sidecar.save(&picture).expect("the sidecar writes");

        let files = vec![rated, developed, untouched, picture];
        let (sidecars, seed) = load_sidecars(&files, true);
        assert_eq!(sidecars[0].meta.rating, 4, "the meta came back");
        assert_eq!(seed, vec![true, false, true, false]);
        // With sidecars off nothing is read and nothing is seeded.
        assert_eq!(load_sidecars(&files, false).1, vec![false; 4]);
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }

    /// The folder's round of thumbnails closes on the last first
    /// arrival, a failure counting as one, and a larger picture made
    /// again for the grid not counting at all.
    #[test]
    fn a_folders_thumbnails_are_counted_once_each() {
        let mut run = ThumbRun::new(3);
        assert_eq!(run.arrived(0, Some(true), 0.001), None);
        assert_eq!(run.arrived(0, Some(false), 0.2), None, "the grid's second");
        assert_eq!(run.arrived(7, Some(false), 0.2), None, "not this folder's");
        assert_eq!(run.arrived(2, None, 0.0), None);
        let done = run.arrived(1, Some(false), 0.05).expect("the last one");
        assert_eq!(
            (done.files, done.cached, done.made, done.failed),
            (3, 1, 1, 1)
        );
        assert!((done.work - 0.051).abs() < 1e-9);
        assert_eq!(run.arrived(1, Some(true), 0.0), None);
    }

    /// For the numbers: every sidecar under `GREYCARD_SIDECAR_BENCH`,
    /// read one after another as a folder's open reads them and on the
    /// pool as the all-roots view does. `cargo test --release --
    /// --ignored sidecars_for_the_numbers --nocapture`.
    #[test]
    #[ignore = "reads a tree named by GREYCARD_SIDECAR_BENCH"]
    fn sidecars_for_the_numbers() {
        let Some(top) = std::env::var_os("GREYCARD_SIDECAR_BENCH") else {
            return;
        };
        let mut files = Vec::new();
        let mut dirs = vec![PathBuf::from(top)];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    if !entry.file_name().to_string_lossy().starts_with('.') {
                        dirs.push(p);
                    }
                } else if greycard_core::decode::is_raw_path(&p) {
                    files.push(p);
                }
            }
        }
        files.sort();
        for round in 0..2 {
            let t = std::time::Instant::now();
            let (one, _) = load_sidecars(&files, true);
            let serial = t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let (pool, _) = load_sidecars_parallel(&files, true, None);
            let parallel = t.elapsed().as_secs_f64();
            assert_eq!(one.len(), pool.len());
            eprintln!(
                "round {round}: {} sidecars, {serial:.3} s one after another, {parallel:.3} s on the pool",
                files.len()
            );
        }
    }
}
