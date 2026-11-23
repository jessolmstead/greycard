//! Opening a frame from the browser: a new list landing
//! ([`open_loaded`]), a row picked ([`open_row`]) while its sidecar is
//! read ([`pick_pending`]), the open proper sent to the worker
//! ([`open_frame`], [`open_job`]), what its answer is taken for
//! ([`opened_is_current`]), the frame left for it ([`save_leaving`]),
//! and the open frame read again under it ([`reread_open`]).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use greycard_edit::Edit;

use super::{
    NeighborsSent, OpenMosaic, ThumbRun, chosen_frames, end_flash, file_name, known_mosaic,
    migrate_opening, neighbors_of, panel_frame, rebuild_browser, row_of, seed_near, show_file_name,
    show_set, walked_to,
};
use crate::panel::cull::{cull_select, drop_placeholder, enter_cull, start_placeholder};
use crate::panel::edit::{read_edit, save_edit, show_edit};
use crate::panel::history::show_history;
use crate::worker::{self, Job, Worker};
use crate::{App, Shared, State, WORKER, cull, filter, selection, tr};

/// The open frame left for another: the picture on the GPU stays on
/// screen until the next frame's develop lands, drawn under the edit
/// it is a develop of.
///
/// The held edit is that edit whenever the picture on the GPU is not
/// the open frame's own: set here on a select away and by
/// `cull::leave_cull` on leaving Cull onto another frame than the one
/// developed, and let go when the open frame's develop is delivered,
/// when it fails, and on entering Cull, which draws no develop. So
/// with none held, the picture is the frame being left's own, and the
/// panel's edit as it stands is the one to draw it under; with one
/// held, the frame being left was chosen and left before its own
/// develop landed, the picture is still an earlier frame's and so is
/// the held edit, which stays. The frame being left's edit over it
/// would draw one frame's picture under another's look until the next
/// develop lands.
///
/// The one exception is a develop that failed: nothing is held after
/// it, though the picture is an earlier frame's, drawn under the open
/// frame's panel, and that frame's edit is what is held when it is
/// left.
///
/// No picture yet, nothing to hold: the next frame opens fitted.
/// The frame on screen's edit saved as a list replaces it, under the
/// view it is on now: run before the view changes, so a frame left for
/// the export queue's view is kept, and one of the queue left for
/// another view, whose panel holds its queued edit, is not
/// (`queue_view::frozen`). Nothing in culling, where the panel is no
/// frame's.
pub(crate) fn save_leaving(st: &mut State, app: &App) {
    if st.current.is_some() && st.culling.cull.is_none() && st.views.queued_open.is_none() {
        let outgoing = read_edit(app, &st.develop.edit, st.develop.target);
        save_edit(st, outgoing);
    }
}

fn hold_outgoing(st: &mut State, outgoing: &Edit) {
    // The frame's Original on show: the picture on screen is its.
    let shown = crate::panel::original::forget(st);
    if st.develop.base_white.is_none() {
        st.develop.zoom = 0.0;
    } else if st.develop.held.is_none() {
        st.develop.held = Some(shown.unwrap_or_else(|| outgoing.clone()));
    }
}

/// A new list of files in the browser, each with its sidecar as a read
/// off the window's thread brought it (read from disk, or standing in
/// from its row in the index: see [`crate::rows`]), `select` opened.
/// `ids_known` says the read asked the index for every file's row, so
/// each `Held::id` is the answer and the index is not asked again here.
pub(crate) fn open_loaded(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    files: Vec<PathBuf>,
    held: Vec<crate::rows::Held>,
    ids_known: bool,
    select: usize,
) {
    let started = std::time::Instant::now();
    let mut st = state.borrow_mut();
    // Leave the old file as a normal selection would: its edit saved,
    // its picture held on screen until the new one develops. In
    // culling the panel is not the frame's, and there is nothing to
    // save; the mode stays on for the new folder.
    if st.current.is_some() && st.culling.cull.is_none() {
        let outgoing = read_edit(app, &st.develop.edit, st.develop.target);
        hold_outgoing(&mut st, &outgoing);
        // The panel under a queued edit is never the frame's to keep,
        // and the view the list is leaving is no longer there to say
        // so.
        if st.views.queued_open.is_none() {
            save_edit(&mut st, outgoing);
        }
    }
    st.views.queued_open = None;
    if let Some(cull) = st.culling.cull.as_mut() {
        cull.cache.clear();
        cull.textures.clear();
        cull.clear_failures();
        cull.anchor = 0;
        // A versions compare is of the last list's cells: not this one's.
        cull.leave_versions(app);
    }
    // Another folder's files, and the pictures kept are keyed by the
    // last one's numbering.
    drop_placeholder(&mut st, app);
    st.culling.hold = None;
    st.culling.prefetch.want(Vec::new());
    // The frame the list is left on, for the history's back and forward.
    let left = crate::panel::visited::frame_of(&st);
    st.current = None;
    current_moved(&mut st);
    crate::panel::look_preview::end(&st, app);
    st.views.picked.clear();
    // The requests for the old list's sidecars are dropped with it.
    crate::rows::list_replaced(&mut st, app);
    let count = files.len();
    let mut sidecars = Vec::with_capacity(count);
    let mut from_row = Vec::with_capacity(count);
    let mut ids = Vec::with_capacity(count);
    for h in held {
        sidecars.push(h.sidecar);
        from_row.push(h.from_row);
        ids.push(h.id);
    }
    // An edit not saved is in memory only: the disk's copy just read
    // would put the older edit back.
    let carried = crate::sidecars::carried(&st);
    st.sidecars = sidecars;
    st.from_row = from_row;
    let ids = ids_known.then_some(ids);
    // The cells, each file's own and its versions' (a collection's
    // frames, found by their files' paths, so the files go in first),
    // and their pictures' vectors, every one to be asked for. A folder
    // of its own: the grid's zoom does not carry its appetite for large
    // pictures over to it.
    st.files = files.clone();
    // Each frame still listed keeps its edit not saved over the disk's;
    // one gone from the list lets it go, and says so.
    crate::sidecars::put_back(&mut st, carried);
    crate::sidecars::listed(&mut st);
    crate::panel::merge::held(&mut st);
    crate::versions::list(&mut st);
    st.views.thumb_want = worker::THUMB_WIDTH;
    st.views.grid_shown = None;
    st.views.strip_shown = None;
    st.views.thumb_run = Some(ThumbRun::new(st.cells.len()));
    st.views.fill_clock = Some(std::time::Instant::now());
    st.views.fill_over = false;
    // The old folder's thumbnails still waiting are dropped: their
    // numbering is its list's.
    worker.forget_thumbnails();
    worker.set_thumb_size(worker::THUMB_WIDTH);
    // A local preview is made for the frames under a root that is
    // there now, for when it is not or is slow to read.
    worker.set_preview_roots(
        st.views
            .library
            .roots
            .list()
            .iter()
            .filter(|r| !st.views.library.offline.contains(*r))
            .cloned()
            .collect(),
    );
    // Which frames show their edit, before any picture is asked for.
    crate::edited::note_list(&st, worker);
    // The new folder's rows as the index has them now, and a pass
    // over it on the indexer's thread to bring them up to date.
    st.views.index_passed = vec![true; files.len()];
    st.views.index_pass_ready = false;
    st.views.index_progress = None;
    st.views.index_tries = 0;
    st.views.index_error = None;
    crate::library::index_open_folder(&mut st, ids);
    // Named, and put at the front of Recently opened; not the launch's
    // folder while the roots' view asked for is about to take its
    // place (`wanted`, cleared when the index fails or there is none,
    // so no view is waited on for ever).
    if st.views.library.wanted.is_none() {
        crate::panel::recent::opened(&mut st, app, left);
    }
    let ids = started.elapsed();
    // What the command line asked of the first frame, onto its
    // sidecar before it is opened: the launch's list is the first to
    // land, and the frame's own sidecar is read for it.
    if let Some(tweak) = st.launch.tweak_at_start.take()
        && select < st.files.len()
        && crate::rows::load_file(&mut st, app, select)
    {
        tweak(&mut st, select);
    }
    rebuild_browser(&mut st, app);
    // The header's count of frames not on the archive this source backs
    // up to, taken again for the new list.
    crate::panel::archive::want_count(&mut st, app);
    let listed = started.elapsed();
    // The file to open, its own cell, as a row of the list; hidden by
    // the filter, the nearest one shown.
    // The file the last launch died opening, which this list was to
    // open again (`open_note`): not opened, its folder shown in the
    // Library and the frame left for a click. Once: the note is gone.
    let not_reopened = st.launch.not_reopened.take().filter(|died| {
        st.files
            .get(select)
            .is_some_and(|f| crate::open_note::same_file(f, died))
    });
    let select = st.file_cell(select).unwrap_or(select);
    let mut row = row_of(&st, select).or_else(|| cull::nearest_row(&st.views.shown, select));
    if let Some(died) = &not_reopened {
        tracing::warn!(
            "not opening {}, which the last launch died opening: its folder in the Library",
            died.display()
        );
        row = None;
        app.set_grid_open(true);
    }
    // A list that arrives before the window has its device (the
    // all-roots view asked for on the command line, read in less time
    // than the window takes to come up) opens its frame from the
    // rendering setup, as the launch's own does, so the first develop
    // runs on the GPU like every later one.
    // A frame the setup was to open belongs to the list just
    // replaced, and goes with it.
    let deferred = !st.launch.setup_ran && !files.is_empty();
    st.launch.select_at_start = deferred
        .then_some(select)
        .filter(|_| not_reopened.is_none());
    let seed = row.or_else(|| not_reopened.as_ref().and_then(|_| row_of(&st, select)));
    if deferred {
        row = None;
    }
    drop(st);

    if row.is_some() {
        hold_thumbnails_for_develop(app, worker);
    }
    {
        let st = state.borrow();
        let seed = seed_near(&st, worker, seed);
        for c in 0..st.cells.len() {
            worker.send(crate::rows::thumb_job(&st, c));
        }
        // After the pushes, which clear the queue's order.
        if let Some(cell) = seed {
            worker.want_thumbnails(cell, cell);
        }
    }
    tracing::debug!(
        "browser: {} frames in, rows read by {:.1} ms, the list made by {:.1} ms, \
         thumbnails asked by {:.1} ms",
        files.len(),
        ids.as_secs_f64() * 1e3,
        listed.as_secs_f64() * 1e3,
        started.elapsed().as_secs_f64() * 1e3
    );
    // The Library's grid or lightbox shows thumbnails, never a frame's
    // full picture: nothing the open note guards goes up for it, unless
    // the frame opens in Develop, whose develop runs under the grid all
    // the same; that develop's picture, drawn, takes the note off.
    let developing = {
        let st = state.borrow();
        st.culling.cull.is_none()
            && st.launch.cull_at_start.is_none()
            && (row.is_some() || deferred)
    };
    if app.get_grid_open() && !developing {
        crate::open_note::shown();
    }
    if let Some(row) = row {
        open_at_start(state, app, row);
    } else if !files.is_empty() && !deferred && not_reopened.is_none() {
        tracing::warn!("no frames pass the filter");
        app.set_status(filter::nothing_shown().into());
        // A batch run would wait for a picture that never comes.
        let mut st = state.borrow_mut();
        if st.window.batch {
            st.launch.failed = true;
            let _ = slint::quit_event_loop();
        }
    }
}

/// Open browser row `row`, and put the `--also` rows beside it in the
/// set (Ctrl+clicks on those rows), which the launch asked for once:
/// from the rendering setup when the list was in before it, else when
/// the list lands.
pub(crate) fn open_at_start(state: &Rc<Shared>, app: &App, row: usize) {
    app.invoke_select(row as i32);
    let (also, rows) = {
        let mut st = state.borrow_mut();
        (
            std::mem::take(&mut st.launch.also_at_start),
            st.views.shown.len(),
        )
    };
    for r in also {
        // A row the filter left out is said, not passed over quietly.
        if r >= rows {
            tracing::warn!("--also {r}: the browser shows {rows} rows (from 0); not in the set");
            continue;
        }
        app.invoke_frame_clicked(r as i32, true, false);
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

/// The window's frame has changed (or gone): where a mark that hid
/// its frame carried the window, or a move away from there, which ends
/// Develop's reach of that mark ([`tags::History::opened`](crate::tags::History::opened)). Called
/// wherever the current frame is set. Another frame than the last
/// open's ends that open's visit: what it says is no longer taken, nor
/// on a return to its frame without an open of its own (a pick still
/// reading its sidecar), only from the next open sent.
pub(crate) fn current_moved(st: &mut State) {
    let file = st.current.and_then(|c| st.path_at(c)).cloned();
    // Another frame than the one noted opening: the user moved on.
    if let Some(file) = &file {
        crate::open_note::moved_to(file);
    }
    st.culling.tags.opened(file.as_deref());
    // The frame for the next run to come back to, however it was
    // chosen.
    crate::panel::startup::chosen(st, file.clone());
    // A zoom noted from a stand-in is for the frame it stood in for
    // (`viewport::frame_on_show`).
    st.develop.stood_view = None;
    if st.develop.open_sent.as_ref().map(|(_, frame)| frame)
        != crate::panel::edit::develop_of(st).as_ref()
    {
        st.develop.open_sent = None;
    }
    // A move is of the frame it moved; another frame's visit has none.
    if st.develop.moved_from.as_ref().map(|(at, _)| at) != file.as_ref() {
        st.develop.moved_from = None;
        st.develop.reopen_due = false;
    }
}

/// Open the frame on browser row `row`: decode and develop it on the
/// worker, or in culling show its JPEG. It becomes the current frame;
/// the set becomes that frame alone, or with `extend` (Shift and an
/// arrow) keeps what it held and takes this frame as well. A frame
/// whose sidecar stands in from its row is current from this moment
/// and opened when its sidecar lands ([`pick_pending`]).
pub(crate) fn open_row(st: &mut State, app: &App, worker: &Worker, row: i32, extend: bool) {
    // The window's rows are the browser's list, which the
    // filter may have shortened; the file is what is opened.
    let Some(&i) = usize::try_from(row)
        .ok()
        .and_then(|r| st.views.shown.get(r))
    else {
        return;
    };
    if st.current != Some(i) {
        end_flash(st, app);
    }
    st.views.picked = if extend {
        selection::union(&chosen_frames(st), &[i])
    } else {
        vec![i]
    };
    // `--cull`: the first file opens the mode, not a develop.
    if let Some(compare) = st.launch.cull_at_start.take() {
        enter_cull(st, app, compare);
    }
    // The last run's Cull drawn developed: the frame opens as in
    // Develop, under the CULLING section.
    if std::mem::take(&mut st.launch.developed_at_start) {
        crate::panel::cull::enter_developed(st, app);
    }
    if !crate::rows::is_loaded(st, i) && !crate::rows::is_offline(st, i) {
        pick_pending(st, app, i, row);
        return;
    }
    open_frame(st, app, worker, i);
}

/// Frame `i` picked while its sidecar stands in from its row: it is
/// the current frame from now, the panel shows the default edit that
/// stands in, read-only, the status says the sidecar is being read,
/// and a request goes out for it; the open proper
/// ([`open_frame`]) runs when the read lands, if the frame is still the
/// pick. Keys pressed on it meanwhile are requests of their own, queued
/// behind this one, and reach the frame once it is open; nothing
/// reaches the frame that was current before the pick.
fn pick_pending(st: &mut State, app: &App, i: usize, row: i32) {
    // A look previewed from the list was the last frame's.
    crate::panel::look_preview::end(st, app);
    // Leaving the frame before: its panel saved, as an open saves it,
    // unless it was itself a pick whose panel was never its own.
    if st.culling.cull.is_none()
        && let Some(c) = st.current
        && st.views.pick_pending.as_deref() != st.path_at(c).map(PathBuf::as_path)
    {
        let outgoing = read_edit(app, &st.develop.edit, st.develop.target);
        hold_outgoing(st, &outgoing);
        save_edit(st, outgoing);
    }
    let path = st.path_of(i).clone();
    st.views.pick_pending = Some(path.clone());
    st.current = Some(i);
    current_moved(st);
    // What the Fill painted on it, waiting, lit or not.
    crate::panel::painted::show(st, app);
    st.develop.generation += 1;
    if st.culling.cull.is_none() {
        let edit = st.line(i).map(|l| l.current().clone()).unwrap_or_default();
        st.develop.target = None;
        st.develop.placing = None;
        app.set_placing("".into());
        st.develop.guiding.clear();
        st.launch.overridden = None;
        panel_frame(st, Some(i));
        show_edit(st, &edit, app, None);
        st.develop.edit = edit;
        st.views.panel_stand_in = Some(path.clone());
        show_history(st, app);
    }
    app.set_selected(row);
    show_file_name(st, app, Some(&path));
    app.set_busy(true);
    let line = tr!("reading {}'s sidecar...", file_name(&path));
    app.set_status(line.as_str().into());
    st.views.reading_line = Some(line);
    show_set(st, app);
    let frame = st.key_of(i).expect("a cell of the list");
    crate::rows::request_in(
        st,
        app,
        vec![frame],
        crate::rows::Scope::Line,
        crate::rows::Request::Open(file_name(&path).to_string()),
        true,
        Box::new(move |state, app, worker, at| {
            let mut st = state.borrow_mut();
            let Some(i) = at.first().copied().flatten() else {
                // Gone before its sidecar came: the pick is over.
                if st.views.pick_pending.as_deref() == Some(path.as_path()) {
                    crate::rows::pick_over(&mut st, app);
                    app.set_busy(false);
                }
                return;
            };
            // Still the pick: opened. Moved on from: its sidecar is in
            // memory now, and nothing shows.
            if st.views.pick_pending.as_deref() != Some(path.as_path()) || st.current != Some(i) {
                return;
            }
            // Still the pick and standing in: its read was started
            // under a list since replaced by a view's read (the
            // generation moved) and filled nothing, so it is asked for
            // again. One that failed opens as it stands in, the status
            // line saying why (`rows::take`).
            if !crate::rows::is_loaded(&st, i)
                && !crate::rows::is_offline(&st, i)
                && st
                    .from_row
                    .get(st.file_of(i))
                    .is_some_and(|r| r.unread.is_none())
            {
                let row = row_of(&st, i).map_or(-1, |r| r as i32);
                pick_pending(&mut st, app, i, row);
                return;
            }
            open_frame(&mut st, app, worker, i);
        }),
    );
}

/// The open proper of frame `i`, its sidecar in memory (or its root
/// offline): the develop on the worker, or in culling its JPEG.
pub(crate) fn open_frame(st: &mut State, app: &App, worker: &Worker, i: usize) {
    let row = row_of(st, i).map_or(-1, |r| r as i32);
    crate::panel::merge::left(app);
    // A look previewed from the list was the last frame's.
    crate::panel::look_preview::end(st, app);
    // So are the lens and its rows: none until this frame's open says,
    // and none for a frame that does not develop, so the switches
    // never show another frame's profile.
    if st.key_of(i) != st.develop.lens_frame {
        st.develop.lens = crate::worker::LensReport::Unread;
        st.develop.lens_options.clear();
    }
    // Whether the frame takes the highlight fill is its open's to say:
    // until then (and in culling, where nothing opens) the switch is
    // free, never the last frame's, and the frame develops with the
    // fill or without by its own kind.
    st.develop.fill_takes = None;
    app.set_highlight_fill_takes(true);
    let offline = !crate::rows::is_loaded(st, i);
    // The session's first open, whichever way it goes, is the one
    // the command line's mask, patch and zoom are for. A first file
    // the index stands in is current before this open, from its
    // pick, so the current frame cannot say so; and a first open
    // that cannot take them (culling, an offline root) leaves none
    // for a later frame.
    let first = !std::mem::replace(&mut st.launch.opened, true);
    st.launch.zoom_stands = first && st.culling.cull.is_none() && !offline;
    if first && (st.culling.cull.is_some() || offline) {
        st.launch.show_mask = None;
        st.launch.show_patch = None;
    }
    // In culling nothing is developed: the frame's JPEG shows. A frame
    // of the export queue's view takes its queued edit onto the panel
    // all the same, read-only, for the scopes and the way out.
    if st.culling.cull.is_some() {
        crate::rows::pick_over(st, app);
        if let Some(edit) = crate::panel::queue_view::open_under(st, i) {
            show_edit(st, &edit, app, None);
            st.develop.edit = edit;
        }
        cull_select(st, app, i);
        crate::panel::assets::lens_frame_shown(st, app);
        show_set(st, app);
        return;
    }
    // The panel is the file's: keep the old one's, show the new one's.
    // The viewport keeps the old picture under the old look
    // until the new frame's camera JPEG is decoded, which
    // stands in until its develop lands; a file chosen from
    // the strip opens fitted either way. A pick whose sidecar was on
    // its way had no panel of its own to keep.
    if let Some(c) = st.current
        && st.views.pick_pending.as_deref() != st.path_at(c).map(PathBuf::as_path)
    {
        let outgoing = read_edit(app, &st.develop.edit, st.develop.target);
        hold_outgoing(st, &outgoing);
        save_edit(st, outgoing);
    }
    crate::rows::pick_over(st, app);
    // The panel is the frame's own from here, or the stand-in's again
    // below.
    st.views.panel_stand_in = None;
    // Its masks' rasters are its own, not the last frame's.
    let file = st.path_of(i).clone();
    crate::panel::mask::learned_for(st, &file);
    if offline {
        // The panel shows the frame's default edit, the one standing
        // in, and not the last frame's: a sync, a copy or a preset
        // from here would otherwise take another frame's edit for this
        // one's, and every one of them refuses an unloaded source. A
        // frame of the export queue's view stands in under its queued
        // edit and turn, as it opens when it is online.
        let edit = match crate::panel::queue_view::open_under(st, i) {
            Some(queued) => queued,
            None => st.line(i).map(|l| l.current().clone()).unwrap_or_default(),
        };
        st.develop.target = None;
        st.develop.placing = None;
        app.set_placing("".into());
        st.develop.guiding.clear();
        panel_frame(st, Some(i));
        show_edit(st, &edit, app, None);
        st.develop.edit = edit;
        st.views.panel_stand_in = Some(file.clone());
        st.current = Some(i);
        current_moved(st);
        // What the Fill painted on it, waiting, lit or not.
        crate::panel::painted::show(st, app);
        crate::panel::edit::show_white(st, &st.develop.edit, app);
        st.develop.generation += 1;
        app.set_selected(row);
        show_file_name(st, app, Some(&file));
        app.set_busy(false);
        show_history(st, app);
        crate::rows::say_offline_for(
            st,
            app,
            i,
            &tr!(": nothing to develop, and no change is kept"),
        );
        crate::panel::assets::lens_frame_shown(st, app);
        show_set(st, app);
        return;
    }
    // A frame of the export queue, opened from its view: the edit it
    // was queued under, which is what will be exported, and nothing
    // the panel does is kept. Its sidecar is not touched, not even to
    // migrate it.
    let queued = crate::panel::queue_view::open_under(st, i);
    if crate::panel::queue_view::frozen(st) {
        // No tool in hand over a frame that is not to be edited.
        if app.get_crop_mode() {
            app.set_crop_mode(false);
            app.invoke_crop_toggled();
        }
        app.set_level_mode(false);
        app.set_guide_mode("".into());
    } else {
        // Before the edit reaches the panel, and so before any
        // refit can run on it: a sidecar from an older build
        // does not know which way up its Original crop goes
        // until the frame does.
        migrate_opening(st, i);
        // What is placed on it waits on a shape nothing can measure
        // (§724): it opens read-only, every change put back
        // (`edit::refused_waiting`), and nothing is made from it.
        if st.line(i).is_some_and(|l| l.current_waits()) {
            tracing::warn!(
                "{}: nothing can measure the frame, so it opens read-only",
                file_name(&file)
            );
        }
    }
    // The cell's own line: a version gone from the sidecar since the
    // cells were made has nothing to open, and the list follows.
    let Some(line) = st.line(i) else {
        tracing::warn!("{}: no such version to open", file_name(&file));
        return;
    };
    let mut edit = queued.unwrap_or_else(|| line.current().clone());
    // The frame the command line's overrides were on is left, or
    // opened again: they are done with (`panel_state`).
    st.launch.overridden = None;
    // The command line's temperature and exposure, over the first
    // file's edit on the panel and not on its sidecar.
    let this_file = st.file_of(i);
    if let Some((_, overrides)) = st
        .launch
        .overrides_at_start
        .take_if(|(f, _)| *f == this_file)
    {
        overrides.apply(&mut edit);
        st.launch.overridden = Some((file.clone(), edit.clone()));
    }
    // A mask asked for on the command line is the first file's
    // target, so a screenshot can show the panel's block for it.
    st.develop.target = if first {
        let m = st
            .launch
            .show_mask
            .take()
            .filter(|&m| m < edit.adjustments.len());
        if m.is_some() {
            app.set_show_mask(true);
            app.set_component(0);
        }
        // Likewise a patch, which the panel's list then shows
        // chosen and the viewport outlines.
        if let Some(p) = st.launch.show_patch.take() {
            app.set_patch(p as i32);
        }
        m
    } else {
        None
    };
    st.develop.placing = None;
    app.set_placing("".into());
    // Its people are not the last picture's, nor is an offer of one.
    crate::panel::parts::another_picture(st);
    crate::panel::parts_all::another_picture(st, app);
    // Nor are the analyses on their way: let go when they land.
    if let Some(renderer) = st.develop.renderer.as_mut() {
        renderer.another_picture();
    }
    // Another picture: a guide stroke drawn on the last one
    // means nothing here, and the source's size is not known
    // again until this frame's develop lands — a turn before
    // that must not map this frame's masks by the last
    // frame's shape.
    st.develop.guiding.clear();
    st.develop.source_size = (0, 0);
    st.develop.turn_pressed = None;
    app.set_guide_mode("".into());
    // The frame's mosaic as far as the session knows it, with no read
    // here: the worker says ahead of its decode.
    panel_frame(st, Some(i));
    // Its own white is not known until its decode reads it.
    st.develop.as_shot_white = None;
    crate::panel::color::forget_before_as_shot(st);
    show_edit(st, &edit, app, st.develop.target);
    st.develop.edit = edit.clone();
    // Opened with the crop in hand: the starting aspect once its
    // develop says which way up it stands (`crop::start_landed`).
    st.develop.crop_start_due = app.get_crop_mode();
    st.current = Some(i);
    current_moved(st);
    // What the Fill painted on it, waiting, lit or not.
    crate::panel::painted::show(st, app);
    crate::panel::edit::show_white(st, &st.develop.edit, app);
    st.develop.generation += 1;
    // The key's moment, until the frame that shows this
    // frame: the log's line, and `--time-select`.
    st.views.selected_at = Some(std::time::Instant::now());
    app.set_selected(row);
    show_file_name(st, app, Some(&file));
    // The last file's shot is not this one's; the open says
    // what this one was.
    app.set_shot_camera("".into());
    app.set_shot_exposure("".into());
    app.set_shot_size("".into());
    app.set_shot_time("".into());
    app.set_status(tr!("decoding...").into());
    app.set_busy(true);
    show_history(st, app);
    worker.send(open_job(st, i, edit));
    // The frames the arrow keys reach from here and beyond, deeper the
    // way the strip is being walked, for the worker to make their bases
    // while it waits on the user.
    walked_to(st, i);
    worker.neighbors(
        st.develop.generation,
        st.develop.walk.heading(),
        neighbors_of(st, i),
    );
    st.develop.neighbors_sent = Some(NeighborsSent::of(st, i, st.develop.generation));
    // The frame's own camera JPEG, to stand in for that
    // develop: after the job, so the decode that matters
    // most is under way first.
    start_placeholder(st, app, i);
    // Its neighbors' pictures come as the strip or the grid follows it
    // there and says what it shows.
    show_set(st, app);
}

/// The job that has the worker open frame `i` under `edit`, at the
/// current generation. Every open the panel sends is made here, so
/// the masks' rasters the panel holds always name the file the worker
/// has open (`panel::mask::learned_for`): a raster made on another
/// file is never drawn on this one, and none made on this one is
/// dropped as another's.
pub(crate) fn open_job(st: &mut State, i: usize, edit: Edit) -> Job {
    // Whatever the Original kept of the frame before goes, its texture
    // with it: culling's switches open by here too.
    crate::panel::original::forget(st);
    let path = st.path_of(i).clone();
    let file = st.file_of(i);
    crate::panel::mask::learned_for(st, &path);
    crate::panel::mask::learned_anew(st, st.cells[i].version.clone());
    let learned = crate::panel::mask::asks_at_open(st, &edit);
    let version = st.cells[i].version.clone();
    sending_open(st, &path, &version);
    Job::Open {
        path,
        version,
        edit,
        generation: st.develop.generation,
        seed_blend: match st.views.queued_open {
            Some(f) => f.seed,
            None => crate::rows::wants_seed(st, i),
        },
        turn: match st.views.queued_open {
            Some(f) => f.turn,
            None => st.sidecars[file].turn,
        },
        stance_known: known_mosaic(st, i) != OpenMosaic::Unknown,
        learned,
    }
}

/// An open of the frame `path`, `version` is sent at the current
/// generation: what it says of the frame is taken from here on, while
/// that frame is the one chosen ([`opened_is_current`]).
fn sending_open(st: &mut State, path: &Path, version: &Option<std::sync::Arc<str>>) {
    st.develop.open_sent = Some((st.develop.generation, (path.to_path_buf(), version.clone())));
    st.develop.reopen_due = false;
}

/// The frame on screen's file was followed to another path while the
/// worker held it under the old one (`file_moved`): it is opened again
/// under the new path, as a file written again under the window is
/// (`reread_open`). Whether it was.
pub(crate) fn reopen_moved(st: &mut State, app: &App, worker: &Worker) -> bool {
    std::mem::take(&mut st.develop.reopen_due) && reread_open(st, app, worker)
}

/// Whether the worker's `Opened` at `generation` is for the frame
/// chosen now, and what it says of the frame (its kind, its mosaic, its
/// white, its lens, its shot) to be taken. It is when it is the
/// generation waited on, as it is unless a develop was asked since; and
/// when it is the last open sent, or that open with a develop of its
/// frame folded in by the worker (`Queue::take_develop_of`), at a
/// generation a develop asked since has moved on from, while the frame
/// it was sent for is still the one chosen. An open of a frame left
/// since, or one sent before the last (the same frame opened again, its
/// file written again), is not. Its picture is another matter: a
/// develop lands only at the generation waited on.
pub(crate) fn opened_is_current(st: &State, generation: u64) -> bool {
    if generation == st.develop.generation {
        return true;
    }
    // Culling, entered while it decoded, takes no word of it: leaving
    // culling sends the open the frame is developed under.
    if st.culling.cull.is_some() {
        return false;
    }
    st.develop.open_sent.as_ref().is_some_and(|(sent, frame)| {
        (*sent..=st.develop.generation).contains(&generation)
            && crate::panel::edit::develop_of(st).as_ref() == Some(frame)
    })
}

/// The open frame's file was written again under it, unasked (another
/// editor's save, a merge made again): its picture read again from the
/// new bytes and developed under the panel's edit as it stands, a
/// slider mid-drag included, and nothing else of an open done. No save
/// and so no history step or sidecar write, no slider set, no tool or
/// mask target put down, the view where it is (a picture of the same
/// size keeps its zoom and its center). A develop of the old bytes
/// still out is given up by the new generation. The worker starts the
/// masks' models afresh at any open, so the learned rasters stay drawn
/// until it makes them again on the new pixels, as for the same frame
/// opened again (`mask::learned_anew`); the people a Part's pick shows
/// are found again (`parts::picture_changed`). The Original on show
/// stays on, made again of the new bytes (`original::rewritten`). The
/// frames the arrow keys reach are sent again under the new open, as
/// an open sends them, so the prefetch goes on. Not in culling, whose
/// caches the caller lets go, nor while a pick waits on a sidecar: that
/// open wins, and reads the file as it is now. Whether it was read
/// again.
pub(crate) fn reread_open(st: &mut State, app: &App, worker: &Worker) -> bool {
    let Some(i) = st.current else {
        return false;
    };
    if st.culling.cull.is_some()
        || st.views.pick_pending.is_some()
        || st.views.panel_stand_in.is_some()
        || !crate::rows::is_loaded(st, i)
    {
        return false;
    }
    let edit = crate::panel::edit::edit_to_develop(app, &st.develop.edit);
    st.develop.generation += 1;
    // The masks' file, which a move has followed already.
    let path = st.path_of(i).clone();
    crate::panel::mask::learned_for(st, &path);
    crate::panel::original::rewritten(st, st.develop.generation);
    crate::panel::parts::picture_changed(st, app);
    app.set_busy(true);
    let version = st.cells[i].version.clone();
    crate::panel::mask::learned_anew(st, version.clone());
    let learned = crate::panel::mask::asks_at_open(st, &edit);
    tracing::info!(
        "{}: written again under the window; read again",
        file_name(st.path_of(i))
    );
    sending_open(st, &path, &version);
    worker.send(Job::Open {
        path,
        version,
        edit,
        generation: st.develop.generation,
        seed_blend: false,
        turn: crate::panel::edit::current_turn(st),
        stance_known: known_mosaic(st, i) != OpenMosaic::Unknown,
        learned,
    });
    // Those sent with the last open are let go by the worker once this
    // one is out; a step of the walk it is not.
    worker.neighbors(
        st.develop.generation,
        st.develop.walk.heading(),
        neighbors_of(st, i),
    );
    st.develop.neighbors_sent = Some(NeighborsSent::of(st, i, st.develop.generation));
    true
}
