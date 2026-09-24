use crate::panel::browser::{
    file_name, filter_frames, grid_filled_rows, migrate_frame, rebuild_browser, reject_count,
    row_of,
};
use crate::panel::crop::read_geometry;
use crate::panel::curve::draw_curve;
use crate::panel::edit::{read_edit, save_edit, schedule_save, show_edit};
use crate::panel::history::show_history;
use crate::panel::startup::sync_rows;
use crate::panel::viewport::{schedule_snapshot, sync_display, write_screenshot, zoom_label};
use crate::*;

/// The panel's tab while culling, which the tab bar does not list:
/// the develop sections go away by their own condition and the
/// CULLING section takes their place.
pub(crate) const CULL_TAB: &str = "Cull";

/// Culling mode: the camera's JPEG of each frame in the viewport,
/// the ones about the selection decoded ahead of the arrow, and no
/// develop until the mode is left.
pub(crate) struct Cull {
    pub(crate) cache: cull::Cache,
    /// Files with no preview to show, and why.
    pub(crate) failed: HashMap<usize, String>,
    /// The file whose full-size copy has been asked for and has not
    /// arrived.
    pub(crate) full_asked: Option<usize>,
    /// The previews on the GPU, by file: the number of the copy each
    /// was made from, and the texture.
    pub(crate) textures: HashMap<usize, (u64, gpu::Texture)>,
    /// How many frames side by side, 1, 2 or 4, and the first row of
    /// the set.
    pub(crate) compare: usize,
    pub(crate) anchor: usize,
    /// The panel's tab before the mode, put back on leaving.
    pub(crate) tab_kept: slint::SharedString,
    /// Which way the last arrow went, for the order of the decodes.
    pub(crate) forward: bool,
    /// The long edge the screen-size previews are made at, fixed
    /// when the mode is entered, and the frames either side of the
    /// selection kept at that size within `cull::BUDGET_BYTES`.
    pub(crate) size: u32,
    pub(crate) reach: usize,
    /// When the selection last moved, until the frame that shows it.
    pub(crate) switched: Option<std::time::Instant>,
    /// When the frame was last turned, until the frame that shows it
    /// turned: a redraw of the preview already on the GPU, since the
    /// turn rides in the same matrix the edit's quarter turns do.
    pub(crate) turned: Option<std::time::Instant>,
    /// The status line as last set, so it is set only on a change.
    pub(crate) status: String,
}

impl Cull {
    pub(crate) fn new(tab_kept: slint::SharedString, size: u32) -> Self {
        Self {
            cache: cull::Cache::default(),
            failed: HashMap::new(),
            full_asked: None,
            textures: HashMap::new(),
            compare: 1,
            anchor: 0,
            tab_kept,
            forward: true,
            size,
            reach: cull::reach_for(size),
            switched: None,
            turned: None,
            status: String::new(),
        }
    }
}

/// Into culling mode: whatever is in hand is put down and the edit
/// on the panel written, since neither can act on the camera's
/// picture; a develop on its way is dropped; the panel shows the
/// CULLING section in place of the develop tabs'; and the frame the
/// selection is on is shown from its JPEG, the ones about it decoded
/// ahead.
pub(crate) fn enter_cull(st: &mut State, app: &App, compare: usize) {
    if st.cull.is_some() {
        return;
    }
    st.placing = None;
    app.set_placing("".into());
    st.retouching = None;
    app.set_retouch_mode("".into());
    st.picking = None;
    app.set_picking("".into());
    st.guiding.clear();
    app.set_guide_mode("".into());
    app.set_level_mode(false);
    app.set_crop_mode(false);
    st.crop_drag = None;
    st.mask_drag = None;
    st.patch_drag = None;
    if st.current.is_some() {
        let outgoing = read_edit(app, &st.edit, st.target);
        save_edit(st, outgoing);
    }
    // A save or a develop still on its timer would read a panel that
    // is no longer the frame's.
    st.save_timer.stop();
    st.debounce.stop();
    st.held = None;
    st.peek = None;
    st.status_kept = None;
    st.view_kept = None;
    // A frame whose develop was still on its way: that develop is
    // dropped with the generation, and the camera's picture standing
    // in for it with the mode's own. The previews the develop view
    // kept go too — the mode makes its own, at its own size.
    drop_placeholder(st, app);
    st.hold = None;
    st.generation += 1;
    app.set_busy(false);
    app.set_navigator(slint::Image::default());
    app.set_scope_picture(slint::Image::default());
    let tab_kept = app.get_panel_tab();
    app.set_panel_tab(CULL_TAB.into());
    app.set_culling(true);
    // The previews at the view's size, and no smaller than a strip
    // would be laughable at.
    let size = (app.get_view_width().max(app.get_view_height()).max(1) as u32).max(1024);
    let mut cull = Cull::new(tab_kept, size);
    cull.compare = match compare {
        2 | 4 => compare,
        _ => 1,
    };
    app.set_compare(cull.compare as i32);
    app.set_compare_text(cull.compare.to_string().into());
    let reach = cull.reach;
    st.cull = Some(cull);
    // Fitted, as a file opens; at the start the command line's zoom
    // stands, so `--zoom 1` opens the loupe at 1:1.
    if st.current.is_some() {
        st.zoom = 0.0;
    }
    st.image_size = (0, 0);
    tracing::info!("culling: on, previews at {size} px, {reach} either side");
    if let Some(c) = st.current {
        cull_select(st, app, c);
    }
}

/// Out of culling mode, on to the frame the selection is on: the
/// panel's tab comes back, the frame's edit (or `with`, the frame's
/// edit plus the control that was reached for) goes on the panel,
/// and its real develop is asked for. The camera's picture stays on
/// screen until that lands, when the viewport swaps to ours in one
/// frame: they differ by design, and a blend would hide that.
pub(crate) fn leave_cull(st: &mut State, app: &App, worker: &Worker, with: Option<Edit>) {
    let Some(mut cull) = st.cull.take() else {
        return;
    };
    st.prefetch.want(Vec::new());
    app.set_culling(false);
    app.set_panel_tab(cull.tab_kept.clone());
    sync_rows(&st.compare_tiles, Vec::new());
    tracing::info!("culling: off");
    let Some(c) = st.current else {
        drop_placeholder(st, app);
        st.hold = None;
        app.set_status("".into());
        return;
    };
    // The camera's picture is kept until the develop lands; only the
    // one frame's is needed for that.
    cull.compare = 1;
    cull.textures.retain(|f, _| *f == c);
    st.hold = Some(cull);
    let edit = with.unwrap_or_else(|| st.sidecars[c].current.clone());
    st.target = None;
    show_edit(st, &edit, app, None);
    // A control's change is a step of the frame's history, as it
    // would be out of the mode: written once the panel rests.
    if edit != st.sidecars[c].current {
        schedule_save(st, app.as_weak());
    }
    show_history(st, app);
    st.edit = edit.clone();
    st.generation += 1;
    st.zoom = 0.0;
    st.image_size = (0, 0);
    // The loupe's picture is already up: it stands in for the develop
    // now, by the same rule a selected frame's does, and the panel
    // keeps its shapes and its scopes off it until ours lands.
    // Only where the loupe had a picture of this frame to hold: a
    // mode left before its first decode has nothing standing in, and
    // saying it had would gate the panel off the developed picture
    // that is still on screen.
    if let Some(preview) = st.hold.as_ref().and_then(|h| h.cache.best(c)) {
        let (turns, flip) = thumb_turns_of(&st.sidecars, c, app, true);
        let plane = Geometry {
            turns,
            flip,
            ..Geometry::default()
        }
        .plane_size(preview.source.0 as f32, preview.source.1 as f32);
        let shown = (plane.0.round() as u32, plane.1.round() as u32);
        let small = preview.small();
        st.placeholder = Some(placeholder::Wait::held(c, st.generation));
        show_overlays(st, app, true);
        say_placeholder(app, shown, small);
    }
    app.set_status("developing...".into());
    app.set_busy(true);
    worker.send(Job::Open {
        path: st.files[c].clone(),
        edit,
        generation: st.generation,
        seed_blend: st.seed_blend.get(c).copied().unwrap_or(false),
        turn: st.sidecars[c].turn,
    });
}

/// A control reached in culling, as the edit to leave with: the
/// panel still holds the last-opened frame's edit (`st.edit`) with
/// the one control moved, and the culled frame's own edit is in its
/// sidecar; the control's change alone is laid over the latter. What
/// changed is read as a difference of the two panel edits, leaf by
/// leaf, rather than a section of the other frame's edit coming
/// across with it. None when nothing on the panel moved.
pub(crate) fn control_over_frame(st: &mut State, app: &App) -> Option<Edit> {
    let c = st.current?;
    let before = st.edit.clone();
    let touched = read_edit(app, &before, st.target);
    if touched == before {
        return None;
    }
    let frame = &st.sidecars[c].current;
    match greycard_edit::overlay_changes(&before, &touched, frame) {
        Ok(edit) => Some(edit),
        Err(e) => {
            tracing::warn!("the control reached in culling could not be kept: {e}");
            None
        }
    }
}

/// The selection moves to `file` in culling mode: no develop, the
/// panel left as it is, the frame's JPEG shown from the cache or
/// asked for, and the window of decodes about it renewed.
pub(crate) fn cull_select(st: &mut State, app: &App, file: usize) {
    let Some(row) = row_of(st, file) else {
        return;
    };
    let was = st.current;
    // As the loupe takes it: leaving culling develops this frame,
    // and a part-migrated Original crop would be measured first.
    migrate_frame(st, file);
    st.current = Some(file);
    app.set_selected(row as i32);
    app.set_file_name(file_name(&st.files[file]).into());
    app.set_shot_camera("".into());
    app.set_shot_exposure("".into());
    app.set_shot_size("".into());
    show_history(st, app);
    let count = st.shown.len();
    if let Some(cull) = st.cull.as_mut() {
        cull.forward = was.is_none_or(|w| file >= w);
        cull.anchor = cull::anchor_for(cull.anchor, row, cull.compare, count);
        cull.switched = Some(std::time::Instant::now());
        // A full-size copy is the frame's own; the next one asks for
        // its own if 1:1 is still on.
        cull.cache.drop_full_unless(file);
        cull.full_asked = None;
        // A decode that failed is tried again when the frame comes
        // round: a file being written, a moment's shortage.
        cull.failed.remove(&file);
    }
    cull_refresh(st);
    app.window().request_redraw();
}

/// The decodes wanted now, in order: the compared frames first, then
/// the window about the selection outward, the way the arrow went
/// first; a full-size copy of the frame at 1:1 before any of them.
/// The whole list every time, less what is cached; the prefetcher
/// leaves out what a thread is on. What the window left is dropped.
pub(crate) fn cull_refresh(st: &mut State) {
    let Some(c) = st.current else {
        return;
    };
    let Some(row) = row_of(st, c) else {
        return;
    };
    let count = st.shown.len();
    let Some(cull) = st.cull.as_mut() else {
        return;
    };
    let rows = cull::compare_rows(cull.anchor, cull.compare, count);
    let mut order: Vec<usize> = rows.clone();
    for r in cull::order(row, count, cull.reach, cull.forward) {
        if !order.contains(&r) {
            order.push(r);
        }
    }
    let kept = cull::kept(row, count, cull.reach);
    let window: Vec<usize> = kept.clone().chain(rows).map(|r| st.shown[r]).collect();
    cull.cache.keep(&window, c);
    cull.textures.retain(|f, _| window.contains(f));
    tracing::debug!(
        "cull window: rows {}..{} about {row}, {} previews held, {} MB",
        kept.start,
        kept.end,
        cull.cache.count(),
        cull.cache.bytes() / 1_000_000
    );
    let size = cull.size;
    let mut wants = Vec::new();
    if st.zoom > 0.0 && !cull.cache.has_own_size(c) && !cull.failed.contains_key(&c) {
        wants.push(cull::Want {
            file: c,
            path: st.files[c].clone(),
            size: 0,
        });
    }
    for r in order {
        let f = st.shown[r];
        if cull.cache.get(f).is_some() || cull.failed.contains_key(&f) {
            continue;
        }
        wants.push(cull::Want {
            file: f,
            path: st.files[f].clone(),
            size,
        });
    }
    st.prefetch.want(wants);
}

/// A frame selected in the develop view: its camera JPEG asked for,
/// to stand in for the develop until that lands.
///
/// The pictures live where the culling mode's do — the same cache,
/// the same decode threads — so a frame just culled, or just arrowed
/// away from, is on screen in the next frame rather than decoded
/// again. Only the selection's own is asked for: nothing is decoded
/// ahead here, where the develop wants the cores.
pub(crate) fn start_placeholder(st: &mut State, app: &App, file: usize) {
    let count = st.shown.len();
    let row = row_of(st, file);
    // At the view's long edge, as the mode's are, and no smaller than
    // a strip would be laughable at.
    let size = (app.get_view_width().max(app.get_view_height()).max(1) as u32).max(1024);
    // What is kept is trimmed on every selection, whether or not this
    // frame gets a placeholder of its own: a run through a folder of
    // pictures would otherwise hold on to whatever a run through the
    // raws before it left.
    if let Some(hold) = st.hold.as_mut() {
        // A view resized since: what is kept was made for a view that
        // is gone, and the mode's rule is to remake at the size in
        // hand.
        if hold.size != size {
            hold.cache.clear();
            hold.textures.clear();
            hold.size = size;
        }
        let window: Vec<usize> = match row {
            Some(row) => placeholder::window(row, count)
                .map(|r| st.shown[r])
                .collect(),
            // Hidden by the filter, which is not a row to keep a
            // window about; its own picture is all that is wanted.
            None => vec![file],
        };
        hold.cache.keep(&window, file);
        hold.textures.retain(|f, _| window.contains(f));
    }
    if row.is_none() || !placeholder::worth_it(&st.files[file]) {
        // Nothing to stand in with, and nothing worth standing in
        // for: what is on screen stays until the develop lands.
        drop_placeholder(st, app);
        return;
    }
    // The hold is the mode's own state without the mode: its tab is
    // never put back from here, since nothing took it away.
    let hold = st
        .hold
        .get_or_insert_with(|| Cull::new(slint::SharedString::new(), size));
    let held = hold.cache.get(file).is_some();
    // A frame whose decode came back with nothing is not asked for
    // again: there is no camera JPEG in that file, and every select
    // would pay a decode to be told so. The record goes when the
    // folder does.
    let none_in_it = hold.failed.contains_key(&file);
    let path = st.files[file].clone();
    // Whatever camera picture is on screen stays there until this
    // frame's own is in hand.
    let showing = st.placeholder.as_ref().and_then(|w| w.showing);
    st.placeholder = Some(placeholder::Wait::asked(file, st.generation, showing));
    if held {
        // Already decoded: up on the next frame, no thread needed.
        placeholder_arrived(st, app, file);
        return;
    }
    if none_in_it {
        drop_placeholder(st, app);
        return;
    }
    st.prefetch.want(vec![cull::Want { file, path, size }]);
    // The frame before this one may have gone with the window — a
    // jump across the folder — and then there is nothing standing in
    // and the developed picture is what shows.
    show_overlays(st, app, standing_in(st).is_some());
}

/// A develop of `generation` is on the frame being drawn now: the
/// camera's picture standing in for it comes down, unless the wait is
/// for a frame chosen since, whose own develop is still to come.
pub(crate) fn develop_landed(st: &mut State, app: &App, generation: u64) -> bool {
    if !st
        .placeholder
        .as_ref()
        .is_some_and(|w| w.replaced_by(generation))
    {
        return false;
    }
    drop_placeholder(st, app)
}

/// The frame whose camera picture the viewport draws in place of a
/// develop, when it is drawing one: the selected frame's once it is
/// decoded, the one before it until then, and none where neither is
/// held any more, which leaves the developed picture on screen as it
/// was before any of this.
pub(crate) fn standing_in(st: &State) -> Option<usize> {
    let file = st.placeholder.as_ref()?.showing?;
    st.hold.as_ref()?.cache.best(file)?;
    Some(file)
}

/// The camera's picture of the frame being waited on is in hand: it
/// goes up on the next frame, the status line says what it is and
/// what it is not, and the panel takes its shapes, its navigator and
/// its scopes off a picture they do not belong to.
///
/// Fitted, whatever the view was at: the develop this stands in for
/// arrives fitted, and a magnified screen-size copy in between would
/// be a blur that jumps twice.
fn placeholder_arrived(st: &mut State, app: &App, file: usize) {
    let Some(preview) = st.hold.as_ref().and_then(|h| h.cache.best(file)) else {
        return;
    };
    let (turns, flip) = thumb_turns_of(&st.sidecars, file, app, true);
    let plane = Geometry {
        turns,
        flip,
        ..Geometry::default()
    }
    .plane_size(preview.source.0 as f32, preview.source.1 as f32);
    let shown = (plane.0.round() as u32, plane.1.round() as u32);
    let small = preview.small();
    st.zoom = 0.0;
    st.image_size = (0, 0);
    say_placeholder(app, shown, small);
    show_overlays(st, app, true);
    app.window().request_redraw();
}

/// What the camera's picture is, in the culling loupe's words.
///
/// The line goes on its own property rather than into the status
/// line, and the window shows it in the status line's place for as
/// long as the placeholder is up. Everything that asks for a develop
/// writes "developing..." there — a slider, a turn, an undo, leaving
/// the culling mode — and the picture on screen is not that develop
/// yet, so the word over the picture and the line under it would
/// disagree for as long as it took the next frame to put them right.
/// One property, set where the picture is, and they cannot.
pub(crate) fn say_placeholder(app: &App, shown: (u32, u32), small: bool) {
    let line = placeholder::status(shown, small);
    if app.get_placeholder_status() != line.as_str() {
        app.set_placeholder_status(line.into());
    }
}

/// The rule for what the panel draws over the picture and beside it
/// while the camera's JPEG stands in for a develop, put on the
/// window. What it takes away comes back with the develop: the
/// viewport's own frame fills the navigator and the scopes again,
/// and the shapes are drawn from the picture they are in.
fn show_overlays(st: &mut State, app: &App, up: bool) {
    let over = placeholder::overlays(up);
    app.set_placeholder(!over.shapes);
    if !over.navigator {
        app.set_navigator(slint::Image::default());
        app.set_nav_partial(false);
    }
    if !over.scopes {
        // Every reading of the developed picture, and not the scope
        // alone: the histogram behind the curve and the two clipping
        // lamps are the last frame's as much as it is. The bins go
        // with them, or the next touch of the curve editor — which
        // redraws it from whatever is in hand — paints the last
        // frame's histogram back behind a picture it does not
        // describe.
        st.bins = None;
        app.set_scope_picture(slint::Image::default());
        app.set_curve_image(draw_curve(app, None));
        app.set_clip_shadows_lit(false);
        app.set_clip_highlights_lit(false);
    }
}

/// The develop landed, or the frame is no longer the one waited on:
/// the camera's picture comes down and the panel's overlays, the
/// navigator and the scopes come back with the picture they read.
/// True when one was up.
///
/// The pictures themselves stay in the hold, for the arrow back; the
/// textures made from them do not, since nothing draws them until
/// another frame is chosen.
pub(crate) fn drop_placeholder(st: &mut State, app: &App) -> bool {
    let was = st.placeholder.take().is_some();
    if was {
        show_overlays(st, app, false);
        app.set_placeholder_status("".into());
        if let Some(hold) = st.hold.as_mut() {
            hold.textures.clear();
        }
    }
    was
}

/// A preview decoded on a cull thread, on the UI thread: into the
/// mode's cache while the mode is on, and into the develop view's
/// hold otherwise, where it stands in for the frame's develop as
/// soon as it is the frame still being waited on. A frame with no
/// preview is remembered as such, and the picture on screen is left
/// where it is: without a camera JPEG there is nothing to stand in.
pub(crate) fn deliver_preview(app: &App, loaded: cull::Loaded) {
    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    let mut st = state.borrow_mut();
    // The fields are taken apart: the mode's cache and the develop
    // view's hold are borrowed one or the other.
    let st = &mut *st;
    let (file, path, size) = match &loaded {
        cull::Loaded::Ok {
            file, path, size, ..
        }
        | cull::Loaded::Failed {
            file, path, size, ..
        } => (*file, path.clone(), *size),
    };
    // A preview of a list since replaced would land on a stranger's
    // slot: only its own file's.
    if st.files.get(file) != Some(&path) {
        return;
    }
    let current = st.current;
    let culling = st.cull.is_some();
    let Some(cull) = st.cull.as_mut().or(st.hold.as_mut()) else {
        return;
    };
    if cull.full_asked == Some(file) && size == 0 {
        cull.full_asked = None;
    }
    let made = match loaded {
        cull::Loaded::Ok { preview, .. } => {
            tracing::debug!(
                "preview {}: {}x{} of {}x{}{} in {:.0} ms",
                file_name(&path),
                preview.width,
                preview.height,
                preview.source.0,
                preview.source.1,
                if preview.full { " (full)" } else { "" },
                preview.seconds * 1e3
            );
            if preview.full && current != Some(file) {
                return;
            }
            cull.cache.insert(file, preview);
            true
        }
        cull::Loaded::Failed { message, .. } => {
            tracing::warn!("preview {}: {message}", file_name(&path));
            cull.failed.insert(file, message);
            false
        }
    };
    app.window().request_redraw();
    if culling {
        return;
    }
    // The develop view: the frame still waited on puts its picture
    // up, and a frame with no camera JPEG in it gives up the wait,
    // leaving the picture on screen where it is until the develop.
    if st
        .placeholder
        .as_mut()
        .is_some_and(|w| made && w.arrived(file))
    {
        placeholder_arrived(st, app, file);
    } else if !made
        && st
            .placeholder
            .as_ref()
            .is_some_and(|w| w.file == file && !w.up())
    {
        drop_placeholder(st, app);
    }
}

/// One frame of the culling loupe: the compared frames' previews on
/// the GPU (made now if their copy changed), each fitted to its tile
/// or at the zoom, turned as its edit turns it; the tiles' boxes
/// and names for the overlay; the status line; and the captures a
/// batch run waits on, once every tile has its picture.
pub(crate) fn cull_frame(st: &mut State, app: &App, state: &Rc<RefCell<State>>) {
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let scale_factor = app.window().scale_factor();
    let canvas = render::canvas_rgb(app.get_canvas_choice());
    let Some(selected) = st.current else {
        return;
    };
    let count = st.shown.len();
    // The mode itself, or the camera's picture standing in for a
    // develop: the mode just left, or a frame just selected.
    let holding = st.cull.is_none();
    // Which frame is drawn: while one stands in, the frame there is a
    // picture of, which lags the selection until its own is decoded.
    let c = match standing_in(st) {
        Some(file) if holding => file,
        _ => selected,
    };
    // The panel holds the selected frame's edit and nothing else's,
    // so only that frame's turns are read off it.
    let from_panel = |file: usize| holding && Some(file) == st.current;
    // `--cull-develop` is about to leave: no capture of this picture.
    let leave_next = st.cull_develop;
    let Some(cull) = st.cull.as_mut().or(st.hold.as_mut()) else {
        return;
    };
    let Some(renderer) = st.renderer.as_mut() else {
        return;
    };
    sync_display(
        &mut st.lut_for,
        &mut st.encoded_lut_for,
        &st.monitors,
        app,
        renderer,
    );
    let rows = if holding {
        cull::row_of_shown(&st.shown, c).into_iter().collect()
    } else {
        cull::compare_rows(cull.anchor, cull.compare, count)
    };
    let rects = cull::tile_rects(vw, vh, rows.len().max(1));
    // The current frame's picture, which the others' centers follow.
    let current_full = cull.cache.best(c).map(|p| {
        let (turns, flip) = thumb_turns_of(&st.sidecars, c, app, from_panel(c));
        Geometry {
            turns,
            flip,
            ..Geometry::default()
        }
        .plane_size(p.source.0 as f32, p.source.1 as f32)
    });
    // A new frame, or a fit view, is centered; a zoomed view of the
    // same frame keeps its place.
    if let Some(full) = current_full {
        let size = (full.0.round() as u32, full.1.round() as u32);
        if size != st.image_size {
            st.image_size = size;
            st.center = (full.0 / 2.0, full.1 / 2.0);
        }
    }
    // A frame needs its full-size copy only while it is looked at
    // 1:1; at a fit that copy and its texture are let go, and at
    // 1:1 it is asked for before anything else.
    if st.zoom <= 0.0 && cull.cache.full(c).is_some() {
        cull.cache.drop_full();
        cull.textures.remove(&c);
    }
    // Asked once: the thread's own record of it is gone a moment
    // before the delivery reaches this thread, and a frame in that
    // moment would ask again.
    if !holding
        && st.zoom > 0.0
        && !cull.cache.has_own_size(c)
        && !cull.failed.contains_key(&c)
        && cull.full_asked != Some(c)
    {
        cull.full_asked = Some(c);
        st.prefetch.push_front(cull::Want {
            file: c,
            path: st.files[c].clone(),
            size: 0,
        });
    }
    let mut tiles = Vec::new();
    let mut overlay = Vec::new();
    let mut ready = true;
    let mut names = Vec::new();
    for (k, &row) in rows.iter().enumerate() {
        let file = st.shown[row];
        let rect = rects[k];
        overlay.push(CompareTile {
            x: rect.0 as f32 / scale_factor,
            y: rect.1 as f32 / scale_factor,
            w: rect.2 as f32 / scale_factor,
            h: rect.3 as f32 / scale_factor,
            name: file_name(&st.files[file]).into(),
            on: file == c,
        });
        let Some(preview) = cull.cache.best(file) else {
            // A frame whose decode failed is as ready as it will be.
            if !cull.failed.contains_key(&file) {
                ready = false;
            }
            continue;
        };
        if cull
            .textures
            .get(&file)
            .is_none_or(|(id, _)| *id != preview.id)
        {
            let texture = renderer.encoded_texture(preview.width, preview.height, &preview.rgba);
            cull.textures.insert(file, (preview.id, texture));
        }
        let (turns, flip) = thumb_turns_of(&st.sidecars, file, app, from_panel(file));
        let geometry = Geometry {
            turns,
            flip,
            ..Geometry::default()
        };
        let (tw, th) = (preview.width as f32, preview.height as f32);
        let (fw, fh) = (preview.source.0 as f32, preview.source.1 as f32);
        // The zoom and the center are in the JPEG's own pixels, so
        // 1:1 means 1:1 whichever copy is on the GPU; the shader
        // works in the copy's, so the zoom (display pixels per pixel)
        // grows by the copy's scale and the center shrinks by it.
        let scale = tw / fw;
        let full = geometry.plane_size(fw, fh);
        let frame = geometry.frame(tw, th);
        let zoom = if st.zoom > 0.0 {
            st.zoom
        } else {
            (rect.2 as f32 / full.0)
                .min(rect.3 as f32 / full.1)
                .min(1.0)
        };
        // The compared frames share the selection's center as a
        // fraction of the frame, so a 1:1 of four shows the same
        // corner of each.
        let center = match current_full {
            Some(cur) if file != c => (
                st.center.0 / cur.0.max(1.0) * full.0,
                st.center.1 / cur.1.max(1.0) * full.1,
            ),
            _ => st.center,
        };
        tiles.push((
            file,
            View {
                zoom: zoom / scale,
                center: (center.0 * scale, center.1 * scale),
                matrix: geometry.matrix(),
                plane: geometry.plane_size(tw, th),
                frame_origin: frame.origin,
                frame_size: frame.size,
                canvas,
                ..View::blank()
            },
            rect,
        ));
        if file == c && st.zoom > 0.0 && !preview.own_size() && !cull.failed.contains_key(&c) {
            ready = false;
        }
        // The size as shown, so the line follows the frame's turn as
        // the picture does; the JPEG's own pixels either way, since
        // the only geometry here is quarter turns and the mirror.
        names.push((
            file,
            (full.0.round() as u32, full.1.round() as u32),
            preview.small(),
            preview.own_size(),
        ));
    }
    let drawn: Vec<render::Tile> = tiles
        .iter()
        .map(|(file, view, rect)| render::Tile {
            texture: &cull.textures[file].1,
            view: view.clone(),
            rect: *rect,
        })
        .collect();
    let texture = renderer.render_encoded(vw, vh, canvas, &drawn);
    // The rule and the names are for telling compared frames apart;
    // one frame alone needs neither.
    if holding || rows.len() < 2 {
        sync_rows(&st.compare_tiles, Vec::new());
    } else {
        sync_rows(&st.compare_tiles, overlay);
    }
    let label = zoom_label(st.zoom);
    if app.get_zoom_text() != label {
        app.set_zoom_text(label);
    }
    app.set_nav_partial(false);
    let shown_now = tiles.iter().any(|(f, _, _)| *f == c);
    // The camera's picture standing in for a develop says what it is
    // here as well as when it came in hand: the frame may have been
    // turned since, and then the size in the line is the other way
    // round.
    if holding && let Some(&(_, size, small, _)) = names.iter().find(|n| n.0 == c) {
        say_placeholder(app, size, small);
    }
    // The frame that shows it is the one a timing hook measures to.
    if holding
        && shown_now
        && st
            .placeholder
            .as_mut()
            .is_some_and(placeholder::Wait::drawn)
        && let Some(at) = st.selected_at
    {
        let ms = at.elapsed().as_secs_f64() * 1e3;
        tracing::debug!("select to the camera picture: {ms:.1} ms");
        if let Some((_, to_picture, _)) = st.time_select.as_mut() {
            to_picture.push(ms);
        }
    }
    // The status: what is on screen and how, or why not yet.
    if !holding {
        let status = cull_status(cull, c, st.zoom, shown_now, &names);
        if cull.status != status {
            cull.status = status.clone();
            app.set_status(status.into());
        }
        // The frame that shows the frame the arrow landed on: the
        // switch's time, for the log and `--time-cull`.
        if shown_now && let Some(at) = cull.turned.take() {
            tracing::info!(
                "cull turn to frame: {:.1} ms",
                at.elapsed().as_secs_f64() * 1e3
            );
            // The turned picture is up; a capture waiting on it may
            // go on the next frame.
            st.awaiting_turn = false;
        }
        if shown_now && let Some(at) = cull.switched.take() {
            let ms = at.elapsed().as_secs_f64() * 1e3;
            tracing::debug!("cull switch to frame: {ms:.1} ms");
            time_cull(
                &mut st.time_cull,
                app,
                state,
                ms,
                cull::row_of_shown(&st.shown, c),
                count,
            );
            // `--turn`: the key, from the event loop, and the Enter
            // of `--cull-develop` behind it in the same callback when
            // a capture asks for both, so the develop that is caught
            // is of the turned frame and not of both in turn. The
            // flag is cleared in the callback rather than here, so a
            // capture waiting on it does not go off in between.
            if let Some(quarters) = st.turn_at_start {
                let app_weak = app.as_weak();
                let then_leave = std::mem::take(&mut st.cull_develop);
                let state = state.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        state.borrow_mut().turn_at_start = None;
                        app.invoke_frame_turned(quarters);
                        if then_leave {
                            app.invoke_cull_leave();
                        }
                    }
                });
            }
            // `--cull-develop`: the Enter, from the event loop.
            if std::mem::take(&mut st.cull_develop) {
                let app_weak = app.as_weak();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        app.invoke_cull_leave();
                    }
                });
            }
            // `--ask-rejects` and `--move-rejects`: the button, and
            // for the second the sheet's yes, from the event loop.
            if std::mem::take(&mut st.ask_rejects) {
                let answer = std::mem::take(&mut st.move_rejects);
                let app_weak = app.as_weak();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        app.invoke_rejects_asked();
                        if answer && app.get_rejects_open() {
                            app.invoke_rejects_answered(true);
                        }
                    }
                });
            }
        }
    }
    // A capture waits for every tile's picture, and for the develop
    // when the mode is being left.
    let ready = ready
        && !leave_next
        && !st.awaiting_turn
        && st.asked.is_empty()
        && grid_filled_rows(&st.shown, &st.thumb_made, st.thumb_want, st.grid_shown, app);
    // `--snapshot-placeholder` waits for the camera's picture instead,
    // and for one standing in over a develop already on screen rather
    // than for the first file's own, which is up before anything has
    // been developed at all. The run goes on afterwards: the develop
    // it stands in for is on the GPU this moment.
    let standing_in = holding && st.snapshot_placeholder && shown_now && st.base_white.is_some();
    schedule_snapshot(
        &mut st.snapshot,
        &mut st.panel_scroll,
        &mut st.snapshot_shown,
        app,
        state,
        ready && (standing_in || !holding),
        !standing_in,
    );
    write_screenshot(
        &mut st.screenshot,
        &mut st.failed,
        renderer,
        &texture,
        ready && !holding,
    );
    app.set_texture(slint::Image::try_from(texture).expect("the texture imports"));
}

/// `thumb_turns` without the state, for a frame with the state's
/// fields borrowed apart.
pub(crate) fn thumb_turns_of(
    sidecars: &[Sidecar],
    file: usize,
    app: &App,
    from_panel: bool,
) -> (u8, bool) {
    let turn = sidecars[file].turn;
    if from_panel {
        read_geometry(app).shown_turns(turn)
    } else {
        sidecars[file].current.geometry.shown_turns(turn)
    }
}

/// The culling loupe's status line: what mode the viewport is in,
/// what is shown of the frame and at what, and what a small preview
/// is.
pub(crate) fn cull_status(
    cull: &Cull,
    current: usize,
    zoom: f32,
    shown: bool,
    names: &[(usize, (u32, u32), bool, bool)],
) -> String {
    let mode = if cull.compare > 1 {
        format!("culling, {} up", cull.compare)
    } else {
        "culling".to_string()
    };
    if let Some(why) = cull.failed.get(&current) {
        return format!("{mode}: no camera preview ({why}); Enter develops the frame");
    }
    let Some(&(_, (w, h), small, full)) = names.iter().find(|n| n.0 == current) else {
        return format!("{mode}: decoding the camera JPEG...");
    };
    if !shown {
        return format!("{mode}: decoding the camera JPEG...");
    }
    let what = if small {
        format!("a small camera preview, {w} \u{d7} {h}")
    } else {
        format!("the camera JPEG, {w} \u{d7} {h}")
    };
    let at = if zoom <= 0.0 {
        "fitted".to_string()
    } else if full {
        format!("{}% of its own pixels", (zoom * 100.0).round() as i32)
    } else {
        format!(
            "{}% of its own pixels, the screen-size copy until the full one is decoded",
            (zoom * 100.0).round() as i32
        )
    };
    format!("{mode}: {what}, {at}, through the monitor profile only; Enter develops")
}

/// `--time-cull`, on each frame that showed a stepped-to picture:
/// the milliseconds since the key, then the next step a tenth of a
/// second on, as a hand would arrow; after the last, the numbers on
/// the log and the terminal, and the editor quits.
pub(crate) fn time_cull(
    timing: &mut Option<(u32, Vec<f64>)>,
    app: &App,
    state: &Rc<RefCell<State>>,
    ms: f64,
    row: Option<usize>,
    count: usize,
) {
    let Some((left, samples)) = timing.as_mut() else {
        return;
    };
    tracing::info!("cull step to frame: {ms:.1} ms");
    samples.push(ms);
    let at_end = row.is_some_and(|r| r + 1 >= count);
    if *left == 0 || at_end {
        // The first sample is the mode's own first picture, decoded
        // from nothing; the rest are steps into the window.
        let steps = &samples[1.min(samples.len())..];
        let n = steps.len().max(1) as f64;
        let mean = steps.iter().sum::<f64>() / n;
        let min = steps.iter().copied().fold(f64::INFINITY, f64::min);
        let max = steps.iter().copied().fold(0.0, f64::max);
        let line = format!(
            "cull step to frame over {} steps: mean {mean:.1} ms, min {min:.1}, max {max:.1}; the first picture {:.1} ms",
            steps.len(),
            samples.first().copied().unwrap_or(0.0)
        );
        tracing::info!("{line}");
        eprintln!("{line}");
        *timing = None;
        let _ = slint::quit_event_loop();
        return;
    }
    *left -= 1;
    let app_weak = app.as_weak();
    let state = state.clone();
    slint::Timer::single_shot(std::time::Duration::from_millis(100), move || {
        if let Some(app) = app_weak.upgrade() {
            // The key's moment is the step's: the arrow, as pressed.
            if let Some(cull) = state.borrow_mut().cull.as_mut() {
                cull.switched = Some(std::time::Instant::now());
            }
            app.invoke_step(1);
        }
    });
}

/// The rejects folder beside the open folder's frames, by its whole
/// path, which is what a sheet should name.
pub(crate) fn shoot_rejects_dir(st: &State) -> Option<PathBuf> {
    let shoot = st.files.first()?.parent()?;
    let shoot = std::fs::canonicalize(shoot).unwrap_or_else(|_| shoot.to_path_buf());
    Some(cull::rejects_dir(&shoot))
}

/// The move-rejects sheet: how many frames and where they would go.
pub(crate) fn ask_rejects(st: &State, app: &App) {
    let n = reject_count(st);
    if n == 0 {
        app.set_status("no frames are flagged reject".into());
        return;
    }
    let Some(dir) = shoot_rejects_dir(st) else {
        return;
    };
    app.set_rejects_text(
        format!(
            "{n} frame{} flagged reject, with {} sidecar{}, will be moved to {}.",
            if n == 1 { "" } else { "s" },
            if n == 1 { "its" } else { "their" },
            if n == 1 { "" } else { "s" },
            dir.display()
        )
        .into(),
    );
    app.set_rejects_open(true);
}

/// Move the rejects out, and take them out of the browser's list.
/// The frame the selection was on, if it went, gives way to the
/// nearest one left.
pub(crate) fn move_rejects(st: &mut State, app: &App, worker: &Worker) {
    let rejected: Vec<usize> = st
        .sidecars
        .iter()
        .enumerate()
        .filter(|(_, s)| s.meta.flag == meta::Flag::Reject)
        .map(|(i, _)| i)
        .collect();
    let moved = match cull::move_rejects(&st.files, &rejected) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("moving the rejects: {e:#}");
            app.set_status(format!("the rejects could not be moved: {e:#}").into());
            return;
        }
    };
    for (i, why) in &moved.skipped {
        tracing::warn!("{} left where it is: {why}", file_name(&st.files[*i]));
    }
    let dir = shoot_rejects_dir(st).unwrap_or_default();
    let mut status = format!(
        "moved {} frame{} and {} sidecar{} to {}",
        moved.files.len(),
        if moved.files.len() == 1 { "" } else { "s" },
        moved.sidecars,
        if moved.sidecars == 1 { "" } else { "s" },
        dir.display()
    );
    if !moved.skipped.is_empty() {
        status.push_str(&format!(
            "; {} left where {} (a file of that name is there already)",
            moved.skipped.len(),
            if moved.skipped.len() == 1 {
                "it was"
            } else {
                "they were"
            }
        ));
    }
    tracing::info!("{status}");
    if moved.files.is_empty() {
        app.set_status(status.into());
        return;
    }
    let current_path = st.current.map(|c| st.files[c].clone());
    let keep = |i: &usize| !moved.files.contains(i);
    let files: Vec<PathBuf> = st
        .files
        .iter()
        .enumerate()
        .filter(|(i, _)| keep(i))
        .map(|(_, f)| f.clone())
        .collect();
    let kept: Vec<usize> = (0..st.files.len()).filter(keep).collect();
    st.files = files;
    st.sidecars = kept.iter().map(|&i| st.sidecars[i].clone()).collect();
    st.seed_blend = kept.iter().map(|&i| st.seed_blend[i]).collect();
    st.thumb_base = kept.iter().map(|&i| st.thumb_base[i].take()).collect();
    st.thumb_shown = kept.iter().map(|&i| st.thumb_shown[i]).collect();
    st.thumb_made = kept.iter().map(|&i| st.thumb_made[i]).collect();
    st.thumb_asked = kept.iter().map(|&i| st.thumb_asked[i]).collect();
    // The indices the previews and the worker's thumbnails were
    // keyed by have moved: the previews are decoded again (cheap),
    // and a thumbnail still owed is asked for again.
    if let Some(cull) = st.cull.as_mut() {
        cull.cache.clear();
        cull.textures.clear();
        cull.failed.clear();
    }
    drop_placeholder(st, app);
    st.hold = None;
    st.prefetch.want(Vec::new());
    for (i, f) in st.files.iter().enumerate() {
        if st.thumb_base[i].is_none() {
            worker.send(Job::Thumbnail {
                index: i,
                path: f.clone(),
            });
        }
    }
    let went = current_path
        .as_ref()
        .map(|p| st.files.iter().position(|f| f == p));
    st.current = went.flatten();
    let next = rebuild_browser(st, app);
    app.set_status(status.into());
    match (st.current, next) {
        (Some(c), _) => {
            if st.cull.is_some() {
                cull_select(st, app, c);
            }
        }
        (None, Some(row)) => {
            // The frame on show went with the rejects; the nearest
            // left takes its place, as a click on it would.
            let app_weak = app.as_weak();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                if let Some(app) = app_weak.upgrade() {
                    app.invoke_select(row as i32);
                }
            });
        }
        (None, None) => {
            app.set_selected(-1);
            app.set_file_name("".into());
        }
    }
}

/// The filter's chips and what they count, on the grid's header and
/// on the CULLING section alike: one pass over the sidecars the open
/// folder has already loaded.
///
/// Nothing is decided here. Which frames pass, and how many a chip
/// would leave, are [`filter`]'s; this reads the answer onto the
/// window.
pub(crate) fn show_filter(st: &State, app: &App) {
    let frames = filter_frames(st);
    let counts = filter::Counts::of(&st.filter, &frames);
    let exact = st.filter.stars.is_exact();
    let picked = st.filter.stars.count();
    let stars: Vec<FilterChip> = (0..=meta::STARS)
        .map(|n| FilterChip {
            text: filter::Stars::chip_name(n, exact).into(),
            count: counts.stars[n as usize] as i32,
            on: n == picked,
            code: n as i32,
        })
        .collect();
    // By the chip's place in the row, which is what the callback
    // gets back and what `Counts` is indexed by; the code is only
    // what the badge draws.
    let flags: Vec<FilterChip> = meta::Flag::ALL
        .iter()
        .enumerate()
        .map(|(n, f)| FilterChip {
            text: f.name().into(),
            count: counts.flags[n] as i32,
            on: st.filter.flags.contains(f),
            code: f.code(),
        })
        .collect();
    let labels: Vec<FilterChip> = meta::Label::ALL
        .iter()
        .enumerate()
        .map(|(n, l)| FilterChip {
            // A color says itself; the chip for no color at all has
            // nothing to say it with, so it says it in words.
            text: if *l == meta::Label::None {
                "No label"
            } else {
                ""
            }
            .into(),
            count: counts.labels[n] as i32,
            on: st.filter.labels.contains(l),
            code: l.code(),
        })
        .collect();
    app.set_filter_stars(ModelRc::new(VecModel::from(stars)));
    app.set_filter_flags(ModelRc::new(VecModel::from(flags)));
    app.set_filter_labels(ModelRc::new(VecModel::from(labels)));
    app.set_filter_exact(exact);
    app.set_filter_shown(counts.shown as i32);
    app.set_filter_total(counts.total as i32);
    app.set_filter_on(!st.filter.is_empty());
}

/// The browser has nothing left to show and the filter is why, which
/// is the one thing worth a word in the status line.
///
/// §117's rule — the badge is the whole answer to a culling key, and
/// the arrow to the next frame would write over any word before it
/// had been read — holds while there is still a frame on screen to
/// carry the badge. When the list has just gone empty there is no
/// badge, no next frame and no arrow, only a blank sheet and a zero.
pub(crate) fn say_if_empty(st: &State, app: &App) {
    if st.shown.is_empty() && !st.files.is_empty() {
        app.set_status(filter::NOTHING_SHOWN.into());
    }
}

/// The filter moved: the browser's list again, and the selection
/// either kept on its row or handed to the nearest frame still
/// shown, which the caller opens — a switch in culling, a develop
/// out of it.
///
/// A chip, a clear or a reading turned over is one deliberate act and
/// says so in the log. A character typed into the text field is not:
/// a word typed is a dozen filters of which only the last was meant,
/// so that path goes to debug through [`apply_filter_typed`].
pub(crate) fn apply_filter(state: &Rc<RefCell<State>>, app: &App) {
    apply_filter_said(state, app, true);
}

/// The same, for a character typed into the text field.
fn apply_filter_typed(state: &Rc<RefCell<State>>, app: &App) {
    apply_filter_said(state, app, false);
}

fn apply_filter_said(state: &Rc<RefCell<State>>, app: &App, settled: bool) {
    let mut st = state.borrow_mut();
    let next = rebuild_browser(&mut st, app);
    let (what, shown, count) = (st.filter.describe(), st.shown.len(), st.files.len());
    if settled {
        tracing::info!("browser filter: {what}, {shown} of {count} frames");
    } else {
        tracing::debug!("browser filter: {what}, {shown} of {count} frames");
    }
    say_if_empty(&st, app);
    match next {
        Some(row) => {
            drop(st);
            app.invoke_select(row as i32);
        }
        None => {
            if st.cull.is_some()
                && let Some(c) = st.current
            {
                cull_select(&mut st, app, c);
            }
            app.window().request_redraw();
        }
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // Culling mode: in and out, the compare view, the browser's
    // filter, and the rejects moved out.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_cull_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.cull.is_some() {
                leave_cull(&mut st, &app, &worker, None);
            } else if st.current.is_some() {
                enter_cull(&mut st, &app, 1);
            } else {
                app.set_status("nothing to cull: open a folder".into());
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_cull_leave(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            leave_cull(&mut state.borrow_mut(), &app, &worker, None);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_compare_changed(move |n| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let n = match n {
                2 | 4 => n as usize,
                _ => 1,
            };
            let count = st.shown.len();
            let row = st.current.and_then(|c| row_of(&st, c)).unwrap_or(0);
            // The row and the key agree with the count whether or
            // not the mode takes it.
            app.set_compare(n as i32);
            app.set_compare_text(n.to_string().into());
            let Some(cull) = st.cull.as_mut() else {
                return;
            };
            cull.compare = n;
            cull.anchor = cull::anchor_for(cull.anchor, row, n, count);
            // The tiles are another size: fitted afresh.
            st.zoom = 0.0;
            st.image_size = (0, 0);
            cull_refresh(&mut st);
            app.window().request_redraw();
        });
    }
    // The filter's chips: the rating and how it reads, the flags,
    // the labels, and the words looked for. Every one of them does
    // the same thing to the browser — a new list, and the selection
    // kept or handed on — so they all end at `apply_filter`.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_star_picked(move |chip| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Ok(stars) = u8::try_from(chip) else {
                return;
            };
            {
                let mut st = state.borrow_mut();
                let want = filter::Stars::AtLeast(stars).read_as(st.filter.stars.is_exact());
                if st.filter.stars == want {
                    return;
                }
                st.filter.stars = want;
            }
            apply_filter(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_flag_toggled(move |chip| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&flag) = meta::Flag::ALL.get(chip.max(0) as usize) else {
                return;
            };
            state.borrow_mut().filter.toggle_flag(flag);
            apply_filter(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_label_toggled(move |chip| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&label) = meta::Label::ALL.get(chip.max(0) as usize) else {
                return;
            };
            state.borrow_mut().filter.toggle_label(label);
            apply_filter(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_exact_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            {
                let mut st = state.borrow_mut();
                let exact = st.filter.stars.is_exact();
                st.filter.stars = st.filter.stars.read_as(!exact);
            }
            apply_filter(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_text_edited(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            {
                let mut st = state.borrow_mut();
                if st.filter.text == text.as_str() {
                    return;
                }
                st.filter.text = text.to_string();
            }
            apply_filter_typed(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_cleared(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            {
                let mut st = state.borrow_mut();
                if st.filter.is_empty() {
                    return;
                }
                st.filter = filter::Filter::default();
            }
            // The field holds its own text, so emptying the filter
            // has to empty it too.
            app.set_filter_text("".into());
            apply_filter(&state, &app);
        });
    }
    // Ctrl+F, and / for a hand already on the arrows. The chips are
    // in the grid's header and in the CULLING section, so the key
    // opens the grid unless the culling panel is already showing
    // them; whichever bar is on screen takes the focus.
    {
        let app_weak = app.as_weak();
        app.on_filter_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The panel holds a bar only on the Cull tab, and only
            // an unfolded CULLING section shows it: a folded one
            // clips the field to nothing, where the focus does not
            // land and the key is swallowed. The key asked for the
            // field, so the section is unfolded rather than the grid
            // opened over it. Every other tab goes to the grid,
            // whose header always has the chips; and when the grid
            // is already up, its bar is the one that answers.
            if !app.get_grid_open() {
                if app.get_panel_tab() == CULL_TAB {
                    app.set_collapsed_culling(false);
                } else {
                    app.set_grid_open(true);
                }
            }
            // A header built this instant has not heard yet, and a
            // `changed` says nothing about a first value, so the ask
            // goes out on the next turn of the loop and the bar
            // answers it on `init` or on the change, whichever it
            // reaches first.
            let app_weak = app.as_weak();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                if let Some(app) = app_weak.upgrade() {
                    app.set_filter_focus(true);
                }
            });
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_rejects_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                ask_rejects(&state.borrow(), &app);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_rejects_answered(move |yes| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_rejects_open(false);
            if yes {
                move_rejects(&mut state.borrow_mut(), &app, &worker);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{click, folder, press, state_for, window};
    use slint::platform::{Key, WindowEvent};

    /// The camera's picture of `file`, as a decode thread delivers
    /// one: a small copy of a JPEG of `source` pixels.
    fn camera_picture(st: &State, file: usize, source: (u32, u32)) -> cull::Loaded {
        let (w, h) = (8u32, 6u32);
        cull::Loaded::Ok {
            file,
            path: st.files[file].clone(),
            size: 1024,
            preview: Arc::new(cull::Preview::new(
                w,
                h,
                vec![0; (w * h * 4) as usize],
                source,
                false,
            )),
        }
    }

    /// A frame chosen in the develop view shows its camera JPEG at
    /// once and keeps it until its develop lands, and the panel takes
    /// its readings of the developed picture off the screen while it
    /// is up. Driven through the window's own select callback.
    #[test]
    fn a_frame_chosen_shows_its_camera_picture_until_its_develop_lands() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        // A scope and a navigator from the frame before it.
        app.set_nav_partial(true);
        app.set_clip_highlights_lit(true);
        app.invoke_select(1);
        {
            let st = state.borrow();
            let wait = st.placeholder.as_ref().expect("a frame is waited on");
            assert_eq!(wait.file, 1);
            assert!(!wait.up(), "nothing stands in until the JPEG is decoded");
            assert_eq!(standing_in(&st), None);
        }
        assert!(
            !app.get_placeholder(),
            "the picture on screen is still ours"
        );

        // The decode comes back: the camera's picture stands in.
        let loaded = camera_picture(&state.borrow(), 1, (8192, 5464));
        deliver_preview(&app, loaded);
        {
            let st = state.borrow();
            assert!(st.placeholder.as_ref().unwrap().up());
            assert_eq!(standing_in(&st), Some(1));
            assert_eq!(st.zoom, 0.0, "a placeholder is the fit view's");
        }
        assert!(app.get_placeholder());
        assert_eq!(
            app.get_placeholder_status(),
            "camera preview: the camera JPEG, 8192 \u{d7} 5464, fitted, \
             through the monitor profile only; developing..."
        );
        // The readings of a picture that is not on screen wait.
        assert!(!app.get_nav_partial());
        assert!(!app.get_clip_highlights_lit());
        assert!(state.borrow().bins.is_none());
        assert_eq!(app.get_scope_picture().size().width, 0);

        // The develop lands: ours takes its place and the panel comes
        // back to it.
        let generation = state.borrow().generation;
        assert!(develop_landed(&mut state.borrow_mut(), &app, generation));
        assert!(!app.get_placeholder());
        assert_eq!(app.get_placeholder_status(), "");
        assert!(state.borrow().placeholder.is_none());
        assert!(
            state.borrow().hold.as_ref().unwrap().cache.get(1).is_some(),
            "the picture is kept for the arrow back"
        );
    }

    /// A develop as the worker delivers one: a picture of `size` and
    /// nothing else of interest.
    fn developed(generation: u64, size: (u32, u32)) -> crate::worker::Outcome {
        crate::worker::Outcome::Developed {
            generation,
            image: crate::worker::Developed::Halves(Arc::new(crate::worker::Halves {
                width: size.0,
                height: size.1,
                pixels: Vec::new(),
            })),
            guide: Arc::new(crate::finish::Guide::NONE),
            white: crate::worker::WhiteBase::IDENTITY,
            seconds: 0.1,
            detail: None,
            sharpen: None,
            dehaze: None,
            sources: Vec::new(),
            learned: crate::worker::LearnedReport::Off,
            fills: crate::worker::FillReport::default(),
        }
    }

    /// The picture on screen stays there until something replaces it.
    /// A frame chosen whose camera JPEG never comes — its file has
    /// none, or its decode came back with nothing — is drawn from the
    /// develop still on the GPU and not from the open frame's own
    /// size, which a select clears. Read from that, the viewport
    /// draws into a one-pixel frame: the canvas over the whole of it,
    /// dark until the develop lands.
    #[test]
    fn a_frame_with_no_camera_picture_keeps_the_last_develop_on_screen() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let generation = state.borrow().generation;
        crate::panel::deliver::deliver(&app, developed(generation, (6000, 4000)));
        assert_eq!(
            state.borrow().shown_size,
            (6000, 4000),
            "the develop delivered is the picture the viewport draws"
        );

        // On to a frame whose file has no camera JPEG in it: the
        // decode comes back with nothing and the wait is given up.
        app.invoke_select(2);
        let path = state.borrow().files[2].clone();
        deliver_preview(
            &app,
            cull::Loaded::Failed {
                file: 2,
                path,
                size: 1024,
                message: "no camera preview in the file".into(),
            },
        );
        let st = state.borrow();
        assert!(st.placeholder.is_none(), "nothing stands in for it");
        assert_eq!(
            st.source_size,
            (0, 0),
            "the open frame has no develop of its own yet"
        );
        assert_eq!(
            placeholder::drawn_source(st.source_size, st.shown_size),
            (6000, 4000),
            "the picture on screen is drawn at its own size, not at nothing"
        );
    }

    /// A second frame chosen before the first has developed shows the
    /// second frame's camera picture; the first frame's stays on
    /// screen until the second's is decoded, rather than the viewport
    /// going blank between them.
    #[test]
    fn a_second_frame_chosen_first_keeps_the_picture_there_is() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        assert_eq!(standing_in(&state.borrow()), Some(1));

        app.invoke_select(2);
        {
            let st = state.borrow();
            let wait = st.placeholder.as_ref().expect("the second frame waits");
            assert_eq!(wait.file, 2);
            assert!(!wait.up());
            assert_eq!(standing_in(&st), Some(1), "the picture there is stays");
        }
        assert!(app.get_placeholder(), "a camera picture is still on screen");

        // The first frame's develop, landing late for a frame no
        // longer selected, is not this frame's picture.
        assert!(!state.borrow().placeholder.as_ref().unwrap().replaced_by(1));

        let loaded = camera_picture(&state.borrow(), 2, (6000, 4000));
        deliver_preview(&app, loaded);
        {
            let st = state.borrow();
            assert!(st.placeholder.as_ref().unwrap().up());
            assert_eq!(standing_in(&st), Some(2));
        }
    }

    /// The develop wins the race: it lands before the JPEG is
    /// decoded, and the JPEG coming back afterwards must not put a
    /// camera picture over the developed one.
    #[test]
    fn a_develop_that_lands_first_keeps_the_late_camera_picture_down() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let generation = state.borrow().generation;
        assert!(develop_landed(&mut state.borrow_mut(), &app, generation));
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        let st = state.borrow();
        assert!(st.placeholder.is_none(), "the late JPEG re-armed the wait");
        assert_eq!(standing_in(&st), None, "the late JPEG went up over ours");
        assert!(!app.get_placeholder());
        assert!(
            st.hold.as_ref().unwrap().cache.get(1).is_some(),
            "the picture is still kept for the arrow back"
        );
    }

    /// A develop asked for before the frame on the panel was chosen
    /// is a picture of another frame: it leaves the placeholder
    /// standing, whether it arrives as a picture or as a failure.
    #[test]
    fn an_older_develop_leaves_the_placeholder_standing() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let older = state.borrow().generation;
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        app.invoke_select(2);
        let loaded = camera_picture(&state.borrow(), 2, (6000, 4000));
        deliver_preview(&app, loaded);
        assert_eq!(standing_in(&state.borrow()), Some(2));
        // The render loop's question, asked of the older develop.
        assert!(!develop_landed(&mut state.borrow_mut(), &app, older));
        // And the same develop failing rather than landing.
        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Failed {
                generation: older,
                message: "late".into(),
            },
        );
        let st = state.borrow();
        assert!(st.placeholder.is_some());
        assert_eq!(standing_in(&st), Some(2));
        assert!(app.get_placeholder());
    }

    /// A control moved while the camera's picture stands in asks for
    /// a develop, which writes "developing..." into the status line.
    /// The line the window shows is the placeholder's own until that
    /// develop lands, so the word over the picture and the line under
    /// it cannot say different things.
    #[test]
    fn a_slider_over_the_placeholder_keeps_it_up_and_says_so() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        let said = app.get_placeholder_status().to_string();
        assert!(said.starts_with("camera preview: "), "{said}");
        let before = state.borrow().generation;
        app.set_exposure(1.25);
        app.invoke_develop_changed();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        {
            let st = state.borrow();
            assert!(st.generation > before, "no develop was asked for");
            assert!(st.placeholder.is_some(), "the placeholder went down early");
            assert!(st.placeholder.as_ref().unwrap().replaced_by(st.generation));
        }
        assert!(app.get_placeholder());
        assert_eq!(app.get_placeholder_status(), said.as_str());
        assert_eq!(app.get_exposure(), 1.25, "the panel lost the slider");
        // That develop lands, and the line goes with the picture.
        let generation = state.borrow().generation;
        assert!(develop_landed(&mut state.borrow_mut(), &app, generation));
        assert!(!app.get_placeholder());
        assert_eq!(app.get_placeholder_status(), "");
    }

    /// Undo, redo and a snapshot restored over the placeholder: each
    /// re-keys a develop and leaves the camera picture standing until
    /// it lands.
    #[test]
    fn undo_and_redo_over_the_placeholder_keep_it_up() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        for e in [1.25f32, 2.0] {
            app.set_exposure(e);
            app.invoke_develop_changed();
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        }
        app.invoke_snapshot_taken();
        app.invoke_undo();
        let undone = app.get_exposure();
        assert_eq!(standing_in(&state.borrow()), Some(1), "undo took it down");
        assert!(app.get_placeholder());
        app.invoke_redo();
        let redone = app.get_exposure();
        assert_eq!(standing_in(&state.borrow()), Some(1), "redo took it down");
        assert!(app.get_placeholder());
        assert_ne!(undone, redone, "undo and redo did not move the panel");
        app.invoke_undo();
        app.invoke_snapshot_restored(0);
        assert_eq!(standing_in(&state.borrow()), Some(1));
        assert!(app.get_placeholder());
    }

    /// A frame turned while the placeholder is up: the camera picture
    /// is drawn the other way up from the same bytes, a develop is
    /// re-keyed, and nothing of the last frame's readings comes back
    /// behind it.
    #[test]
    fn a_turn_over_the_placeholder_keeps_it_up_and_turns_the_picture() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        state.borrow_mut().bins = Some((0..768).map(|i| (i % 97) as u32 + 1).collect());
        app.invoke_select(1);
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        let before = state.borrow().generation;
        let turns_before = thumb_turns_of(&state.borrow().sidecars, 1, &app, true);
        app.invoke_frame_turned(1);
        let st = state.borrow();
        assert!(st.generation > before, "the develop was not re-keyed");
        assert_eq!(standing_in(&st), Some(1), "the turn took it down");
        assert_ne!(
            turns_before,
            thumb_turns_of(&st.sidecars, 1, &app, true),
            "the camera picture would be drawn the same way up"
        );
        assert!(app.get_placeholder());
        assert!(st.bins.is_none(), "the last frame's bins came back");
    }

    /// The culling mode entered over a placeholder takes it, and
    /// leaving the mode hands one back — but only where the loupe had
    /// a picture of the frame to hold. A mode left before its first
    /// decode has nothing standing in, and the developed picture on
    /// screen keeps the panel.
    #[test]
    fn culling_entered_over_a_placeholder_takes_it_and_gives_it_back() {
        for decoded in [false, true] {
            let app = window(4);
            let (state, _worker) = state_for(&app, folder(4));
            app.invoke_select(1);
            let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
            deliver_preview(&app, loaded);
            assert!(app.get_placeholder());
            app.invoke_cull_toggled();
            {
                let st = state.borrow();
                assert!(st.placeholder.is_none(), "a wait survived into the mode");
                assert!(st.hold.is_none(), "the develop view's pictures survived");
                assert!(st.cull.is_some(), "the mode did not start");
            }
            assert!(!app.get_placeholder(), "the mode draws with the gate on");
            if decoded {
                let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
                deliver_preview(&app, loaded);
            }
            app.invoke_cull_leave();
            let st = state.borrow();
            assert!(st.cull.is_none());
            assert_eq!(st.placeholder.is_some(), decoded);
            assert_eq!(app.get_placeholder(), decoded);
            assert_eq!(
                app.get_placeholder_status().starts_with("camera preview: "),
                decoded,
                "the loupe's picture said {:?}",
                app.get_placeholder_status()
            );
        }
    }

    /// A frame with no camera JPEG in it is remembered as having
    /// none: choosing it again costs no second decode, and the
    /// picture on screen stays where it is either time.
    #[test]
    fn a_frame_with_no_camera_jpeg_is_not_asked_for_again() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let failed = {
            let st = state.borrow();
            cull::Loaded::Failed {
                file: 1,
                path: st.files[1].clone(),
                size: 1024,
                message: "no camera preview in the file".into(),
            }
        };
        deliver_preview(&app, failed);
        app.invoke_select(2);
        app.invoke_select(1);
        let st = state.borrow();
        assert!(
            st.hold.as_ref().unwrap().failed.contains_key(&1),
            "the frame is asked for again on every select"
        );
        assert!(
            st.placeholder.is_none(),
            "it waits for a decode nobody sent"
        );
        assert!(!app.get_placeholder());
    }

    /// A picture file chosen while a placeholder is up takes it down
    /// — there is no camera JPEG in a JPEG — and the pictures kept
    /// are still trimmed to the window about it.
    #[test]
    fn a_picture_file_chosen_takes_the_placeholder_down_and_still_prunes() {
        let app = window(5);
        let mut files = folder(5);
        files[4] = std::path::PathBuf::from("/nowhere/scan.jpg");
        let (state, _worker) = state_for(&app, files);
        app.invoke_select(0);
        let loaded = camera_picture(&state.borrow(), 0, (6000, 4000));
        deliver_preview(&app, loaded);
        assert!(app.get_placeholder());
        app.invoke_select(4);
        let st = state.borrow();
        assert!(st.placeholder.is_none(), "a picture file got a placeholder");
        assert!(!app.get_placeholder());
        assert!(
            st.hold.as_ref().unwrap().cache.get(0).is_none(),
            "the window was not trimmed on the way past"
        );
    }

    /// The histogram behind the curve editor goes with the scopes
    /// while the camera picture stands in, and a touch of the curve
    /// does not paint the last frame's back: the bins go with it.
    #[test]
    fn the_curve_editor_does_not_paint_the_last_frames_histogram_back() {
        fn bytes(img: &slint::Image) -> Vec<u8> {
            img.to_rgba8()
                .map(|b| b.as_bytes().to_vec())
                .unwrap_or_default()
        }
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        let last = (0..768).map(|i| (i % 97) as u32 + 1).collect::<Vec<u32>>();
        state.borrow_mut().bins = Some(last.clone());
        app.invoke_select(1);
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        assert!(app.get_placeholder());
        let blank = bytes(&draw_curve(&app, None));
        assert_eq!(
            bytes(&app.get_curve_image()),
            blank,
            "the histogram was not taken off the curve"
        );
        // A point put on the curve, as the panel's own touch area
        // does: the curve is redrawn from whatever bins are in hand.
        app.invoke_curve_press(0.5, 0.5);
        assert_ne!(
            bytes(&app.get_curve_image()),
            bytes(&draw_curve(&app, Some(&last))),
            "the last frame's histogram came back behind the camera picture"
        );
    }

    /// A frame with no camera JPEG in it — a DNG written without one —
    /// falls back to what the editor did before there were
    /// placeholders: the picture on screen stays where it is until the
    /// develop lands, and nothing says otherwise.
    #[test]
    fn a_frame_with_no_camera_jpeg_keeps_the_picture_on_screen() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let failed = {
            let st = state.borrow();
            cull::Loaded::Failed {
                file: 1,
                path: st.files[1].clone(),
                size: 1024,
                message: "no camera preview in the file".into(),
            }
        };
        deliver_preview(&app, failed);
        let st = state.borrow();
        assert!(st.placeholder.is_none(), "nothing is waited on any more");
        assert_eq!(standing_in(&st), None);
        assert!(!app.get_placeholder());
        // Asked for once: the frame is not re-decoded every frame.
        assert!(st.hold.as_ref().unwrap().failed.contains_key(&1));
    }

    /// Nothing that belongs to the developed picture is drawn over a
    /// camera picture standing in for it: a mask handle is neither
    /// drawn nor pressable while one is up, and both come back when
    /// the develop does.
    #[test]
    fn a_mask_handle_is_not_pressable_over_a_camera_picture() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.set_mask_kind("Linear".into());
        let grabbed = Rc::new(std::cell::Cell::new(false));
        {
            let grabbed = grabbed.clone();
            app.on_mask_grabbed(move |_| grabbed.set(true));
        }
        // A handle at these view pixels sits at 240 + x of the window,
        // the left bar being 240 wide.
        let press_handle = || {
            app.set_mask_handles(ModelRc::new(VecModel::from(vec![Pt {
                x: 360.0,
                y: 400.0,
            }])));
            grabbed.set(false);
            click(&app, 240.0 + 360.0, 400.0);
            grabbed.get()
        };
        assert!(press_handle(), "a handle on the developed picture");

        app.invoke_select(1);
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        assert!(app.get_placeholder());
        assert!(
            !press_handle(),
            "a handle took a click over the camera's picture"
        );

        drop_placeholder(&mut state.borrow_mut(), &app);
        assert!(press_handle(), "the handle did not come back");
    }

    #[test]
    fn c_enters_culling_return_and_escape_leave_it_and_v_cycles_the_compare() {
        let app = window(11);
        let toggles = Rc::new(RefCell::new(0));
        let counted = toggles.clone();
        app.on_cull_toggled(move || *counted.borrow_mut() += 1);
        let leaves = Rc::new(RefCell::new(0));
        let counted = leaves.clone();
        app.on_cull_leave(move || *counted.borrow_mut() += 1);
        let compares = Rc::new(RefCell::new(Vec::new()));
        let seen = compares.clone();
        app.on_compare_changed(move |n| seen.borrow_mut().push(n));
        let rated = Rc::new(RefCell::new(Vec::new()));
        let seen = rated.clone();
        app.on_meta_key(move |key| {
            let known = meta::Change::from_key(&key).is_some();
            if known {
                seen.borrow_mut().push(key.to_string());
            }
            known
        });
        let answers = Rc::new(RefCell::new(Vec::new()));
        let seen = answers.clone();
        app.on_rejects_answered(move |yes| seen.borrow_mut().push(yes));
        // C is the way in; out of the mode Return and V are nobody's.
        press(&app, "c");
        assert_eq!(*toggles.borrow(), 1);
        press(&app, Key::Return);
        press(&app, "v");
        assert_eq!(*leaves.borrow(), 0);
        assert!(compares.borrow().is_empty());
        // In it: V cycles two, four, one; the culling keys still
        // reach the browser; Return or Escape leaves.
        app.set_culling(true);
        press(&app, "v");
        app.set_compare(2);
        press(&app, "V");
        app.set_compare(4);
        press(&app, "v");
        assert_eq!(*compares.borrow(), vec![2, 4, 1]);
        press(&app, "3");
        press(&app, "x");
        assert_eq!(*rated.borrow(), ["3", "x"]);
        press(&app, Key::Return);
        assert_eq!(*leaves.borrow(), 1);
        press(&app, Key::Escape);
        assert_eq!(*leaves.borrow(), 2);
        // The grid over the mode: Return closes the grid, and the
        // mode stays.
        press(&app, "g");
        assert!(app.get_grid_open());
        press(&app, Key::Return);
        assert!(!app.get_grid_open());
        assert_eq!(*leaves.borrow(), 2);
        // The rejects sheet takes every key it is shown; Escape is
        // its no.
        app.set_rejects_open(true);
        press(&app, "c");
        press(&app, Key::Return);
        press(&app, "4");
        assert_eq!(*toggles.borrow(), 1);
        assert_eq!(*leaves.borrow(), 2);
        assert_eq!(rated.borrow().len(), 2);
        press(&app, Key::Escape);
        assert_eq!(*answers.borrow(), vec![false]);
        // Ctrl+C is not the mode's.
        app.set_rejects_open(false);
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(&app, "c");
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        assert_eq!(*toggles.borrow(), 1);
    }

    /// Everything the filter does to the browser, driven through the
    /// window's own callbacks rather than the functions behind them:
    /// the rows it leaves, the counts it puts on the chips, and what
    /// happens to the selection when a star takes the frame on screen
    /// out of the list.
    ///
    /// A row and a file are two different numbers under a filter, and
    /// confusing them is a bug this crate has shipped once already,
    /// so the rows are checked by name and not by count.
    #[test]
    fn a_filter_leaves_the_rows_it_says_it_leaves() {
        let app = window(0);
        let (state, _worker) = state_for(&app, folder(6));
        {
            let mut st = state.borrow_mut();
            // Two five-star frames, two threes, two nothing.
            for (i, stars) in [0u8, 5, 3, 0, 5, 3].into_iter().enumerate() {
                st.sidecars[i].meta.set_rating(stars);
            }
            st.sidecars[1].meta.flag = meta::Flag::Pick;
            st.sidecars[4].meta.flag = meta::Flag::Reject;
            st.sidecars[2].meta.label = meta::Label::Red;
            st.sidecars[2].meta.keywords = vec!["harbor".into()];
            st.shown = (0..6).collect();
            st.current = Some(2);
        }
        rebuild_browser(&mut state.borrow_mut(), &app);
        assert_eq!(app.get_filter_total(), 6);
        assert_eq!(app.get_filter_shown(), 6);
        assert!(!app.get_filter_on(), "nothing asked of a frame yet");

        // Three stars or more: the two fives and the two threes, and
        // the chips say so before they are pressed.
        let stars = app.get_filter_stars();
        assert_eq!(stars.row_data(3).unwrap().text, "3+");
        assert_eq!(stars.row_data(3).unwrap().count, 4);
        assert_eq!(stars.row_data(5).unwrap().count, 2);
        app.invoke_filter_star_picked(3);
        assert_eq!(state.borrow().shown, vec![1, 2, 4, 5]);
        assert_eq!(app.get_filter_shown(), 4);
        assert!(app.get_filter_on());
        assert_eq!(app.get_thumbs().row_count(), 4);
        assert_eq!(app.get_thumbs().row_data(0).unwrap().name, "IMG_0001.CR3");

        // The flag chips read how the frames the stars left are
        // flagged — their own group set aside, the other groups as
        // they stand: of those four, one is a pick and one a reject.
        let flags = app.get_filter_flags();
        assert_eq!(flags.row_data(1).unwrap().text, "Pick");
        assert_eq!(flags.row_data(1).unwrap().count, 1);
        assert_eq!(flags.row_data(2).unwrap().count, 1);
        app.invoke_filter_flag_toggled(1);
        assert_eq!(state.borrow().shown, vec![1]);
        // Any-of: the rejects as well, and then neither again.
        app.invoke_filter_flag_toggled(2);
        assert_eq!(state.borrow().shown, vec![1, 4]);
        app.invoke_filter_flag_toggled(1);
        app.invoke_filter_flag_toggled(2);
        assert_eq!(state.borrow().shown, vec![1, 2, 4, 5]);

        // "Exactly" reads the same chip the other way.
        app.invoke_filter_exact_toggled();
        assert!(app.get_filter_exact());
        assert_eq!(state.borrow().shown, vec![2, 5]);
        app.invoke_filter_exact_toggled();

        // A word: a keyword one frame carries, then part of a name,
        // and both of them narrow what the stars already left.
        app.invoke_filter_text_edited("harbor".into());
        assert_eq!(state.borrow().shown, vec![2]);
        app.invoke_filter_text_edited("0004".into());
        assert_eq!(state.borrow().shown, vec![4]);

        // Clear puts the folder back, the field with it.
        app.invoke_filter_cleared();
        assert_eq!(state.borrow().shown, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(app.get_filter_text(), "");
        assert!(!app.get_filter_on());
    }

    /// A star that takes the frame on screen out of the filtered list
    /// does what a reject under "No rejects" has always done: the
    /// selection goes to the nearest frame still shown and that one
    /// opens. It never goes nowhere, and it never stays on a row that
    /// is no longer there.
    #[test]
    fn a_rating_that_hides_the_shown_frame_hands_the_selection_on() {
        let app = window(0);
        let (state, _worker) = state_for(&app, folder(5));
        {
            let mut st = state.borrow_mut();
            for (i, stars) in [4u8, 4, 4, 0, 4].into_iter().enumerate() {
                st.sidecars[i].meta.set_rating(stars);
            }
            st.shown = (0..5).collect();
            st.current = Some(2);
            app.set_selected(2);
        }
        app.invoke_filter_star_picked(4);
        assert_eq!(state.borrow().shown, vec![0, 1, 2, 4]);
        assert_eq!(app.get_selected(), 2, "the frame kept its own row");

        // One star on it: it leaves the list, and file 4 — the next
        // one still shown, at row 2 now — is what opens.
        app.invoke_meta_key("1".into());
        assert_eq!(state.borrow().shown, vec![0, 1, 4]);
        assert_eq!(state.borrow().current, Some(4));
        assert_eq!(app.get_selected(), 2);
        assert_eq!(app.get_thumbs().row_data(2).unwrap().name, "IMG_0004.CR3");
        assert_eq!(app.get_file_name(), "IMG_0004.CR3");

        // And a star that does not hide anything leaves the selection
        // where it is, and still moves the counts.
        app.invoke_meta_key("5".into());
        assert_eq!(state.borrow().current, Some(4));
        assert_eq!(state.borrow().shown, vec![0, 1, 4]);
        assert_eq!(app.get_filter_stars().row_data(5).unwrap().count, 1);

        // The last frame the filter shows, rated out of it: there is
        // nothing after it, so the one before takes the selection.
        app.invoke_select(2);
        app.invoke_meta_key("0".into());
        assert_eq!(state.borrow().shown, vec![0, 1]);
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(app.get_selected(), 1);
    }

    /// Ctrl+F and / both ask for the filter's text field, and neither
    /// is eaten by anything else in the window. The culling keys keep
    /// their own keys, and a sheet over the window keeps all of them.
    #[test]
    fn ctrl_f_and_slash_ask_for_the_filter_and_nothing_else_does() {
        let app = window(11);
        let asked = Rc::new(RefCell::new(0));
        let counted = asked.clone();
        app.on_filter_asked(move || *counted.borrow_mut() += 1);
        let rated = Rc::new(RefCell::new(Vec::new()));
        let seen = rated.clone();
        app.on_meta_key(move |key| {
            let known = meta::Change::from_key(&key).is_some();
            if known {
                seen.borrow_mut().push(key.to_string());
            }
            known
        });
        press(&app, "/");
        assert_eq!(*asked.borrow(), 1);
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        press(&app, "f");
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        assert_eq!(*asked.borrow(), 2);
        // A rating key is still a rating key, and it did not ask.
        press(&app, "3");
        assert_eq!(*rated.borrow(), ["3"]);
        assert_eq!(*asked.borrow(), 2);
        // Nor does a sheet over the window let it through.
        app.set_export_open(true);
        press(&app, "/");
        assert_eq!(*asked.borrow(), 2);
    }

    /// Where the key puts the cursor, through the handler itself and
    /// not a stand-in for it: the grid when the panel has no bar, and
    /// the CULLING section when it has one — unfolding it first,
    /// since a folded section clips its field to nothing and the
    /// focus lands nowhere. The press is answered when the bar is
    /// already up and when the same press is what built it, so the
    /// proof either way is a character typed afterwards landing in
    /// the field rather than rating the frame.
    #[test]
    fn the_filter_key_opens_the_grid_or_unfolds_the_culling_section() {
        // A develop tab, no grid: the chips are only in the grid's
        // header, so that is where the key goes.
        let app = window(0);
        let (state, _worker) = state_for(&app, folder(4));
        state.borrow_mut().current = Some(0);
        rebuild_browser(&mut state.borrow_mut(), &app);
        assert!(!app.get_grid_open());
        press(&app, "/");
        assert!(app.get_grid_open(), "the key opened the grid");
        // The header is built with the grid and takes the ask on the
        // next turn of the loop, where the timer puts it.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        assert!(!app.get_filter_focus(), "the bar answered and put it back");
        press(&app, "5");
        assert_eq!(
            app.get_filter_text(),
            "5",
            "the character went to the field"
        );
        assert_eq!(
            state.borrow().sidecars[0].meta.rating,
            0,
            "and rated nothing"
        );
    }

    #[test]
    fn the_filter_key_unfolds_a_folded_culling_section() {
        let app = window(0);
        let (state, _worker) = state_for(&app, folder(4));
        state.borrow_mut().current = Some(0);
        rebuild_browser(&mut state.borrow_mut(), &app);
        // Culling, with the section folded away by a click on its
        // header: the bar is there but clipped to nothing.
        app.set_panel_tab(CULL_TAB.into());
        app.set_collapsed_culling(true);
        press(&app, "/");
        assert!(!app.get_grid_open(), "the panel already has the chips");
        assert!(!app.get_collapsed_culling(), "so the key unfolded them");
        // The field takes the focus at once, but a section unfolds
        // over an eighth of a second and a field with no height yet
        // is not where a key lands. Past the fold, then: a hand
        // reaching for the next character is slower than that, and
        // the alternative is a fold that snaps open.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
        assert!(!app.get_filter_focus(), "the bar answered and put it back");
        press(&app, "5");
        assert_eq!(
            app.get_filter_text(),
            "5",
            "the character went to the field"
        );
        assert_eq!(
            state.borrow().sidecars[0].meta.rating,
            0,
            "and rated nothing"
        );
    }
}
