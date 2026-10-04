use crate::panel::browser::{
    file_name, filter_frames, grid_filled_rows, migrate_frame, rebuild_browser, reject_count,
    row_of, show_frame_tags,
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
    /// Files whose local preview was asked for and is not kept: not
    /// asked for again, their file (where it can be read) is what
    /// shows.
    pub(crate) no_local: std::collections::HashSet<usize>,
    /// Files whose local preview is up and whose file could not be
    /// read after it: the preview stays, and the file is not asked
    /// for again, so a share that has stopped answering is not tried
    /// at every step.
    pub(crate) file_failed: std::collections::HashSet<usize>,
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
    /// Another list, or the old one renumbered: what was learned of
    /// its files by their numbers goes.
    pub(crate) fn clear_failures(&mut self) {
        self.failed.clear();
        self.no_local.clear();
        self.file_failed.clear();
    }

    /// Whether `file`'s own picture is still worth asking its file for.
    fn wants_file(&self, file: usize) -> bool {
        !self.failed.contains_key(&file) && !self.file_failed.contains(&file)
    }

    pub(crate) fn new(tab_kept: slint::SharedString, size: u32) -> Self {
        Self {
            cache: cull::Cache::default(),
            failed: HashMap::new(),
            no_local: std::collections::HashSet::new(),
            file_failed: std::collections::HashSet::new(),
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
    st.turn_pressed = None;
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
    st.crop_draw = None;
    st.mask_drag = None;
    st.patch_drag = None;
    if st.current.is_some() {
        let outgoing = read_edit(app, &st.edit, st.target);
        save_edit(st, outgoing);
    }
    // The panel is not the frame's from here, and the sidecar's when
    // culling ends: the command line's overrides are done with.
    st.overridden = None;
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
    // A pick whose sidecar was on its way is left with the mode: the
    // frame stays current, and the open that would have followed
    // finds it no longer the pick.
    crate::rows::pick_over(st, app);
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
    // The word for the last key goes with the mode, or coming back
    // within its second would show it over another frame.
    st.notice_timer.stop();
    app.set_notice_on(false);
    // The panel takes the frame's edit here, so it is no stand-in's
    // any more, and the load is not an open of its own.
    st.panel_stand_in = None;
    if !crate::rows::load_frame(st, app, c) {
        st.panel_stand_in = Some(st.files[c].clone());
    }
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
    crate::panel::viewport::settle_source_size(st);
    // The loupe's picture is already up: it stands in for the develop
    // now, by the same rule a selected frame's does, and the panel
    // keeps its shapes and its scopes off it until ours lands.
    // Only where the loupe had a picture of this frame to hold: a
    // mode left before its first decode has nothing standing in, and
    // saying it had would gate the panel off the developed picture
    // that is still on screen.
    if let Some(preview) = st.hold.as_ref().and_then(|h| h.cache.best(c)) {
        let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, c, app, true);
        let plane = Geometry {
            turns,
            flip,
            ..Geometry::default()
        }
        .plane_size(preview.source.0 as f32, preview.source.1 as f32);
        let shown = (plane.0.round() as u32, plane.1.round() as u32);
        let (small, local) = (preview.small(), preview.local);
        st.placeholder = Some(placeholder::Wait::held(c, st.generation));
        show_overlays(st, app, true);
        say_placeholder(app, shown, small, local);
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
    if !crate::rows::load_frame(st, app, c) {
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
    // Culling reaching a frame reads its whole sidecar, if it was
    // standing in from its row: the history panel and the keys want
    // it, and leaving the mode develops it. One under a root that is
    // offline is stepped onto all the same, and the status says why
    // there is no picture.
    crate::rows::load_frame(st, app, file);
    // As the loupe takes it: leaving culling develops this frame,
    // and a part-migrated Original crop would be measured first.
    migrate_frame(st, file);
    st.current = Some(file);
    crate::panel::viewport::settle_source_size(st);
    app.set_selected(row as i32);
    app.set_file_name(file_name(&st.files[file]).into());
    app.set_shot_camera("".into());
    app.set_shot_exposure("".into());
    app.set_shot_size("".into());
    show_history(st, app);
    show_frame_tags(st, app);
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
    let (files, from_row, library) = (&st.files, &st.from_row, &st.library);
    let plan = |f: usize| loupe_plan(files, from_row, library, f);
    let frames: Vec<usize> = order.iter().map(|&r| st.shown[r]).collect();
    let mut wants = Vec::new();
    // At 1:1, the frame's full JPEG before anything, where its file can
    // be read.
    if st.zoom > 0.0 && !cull.cache.has_own_size(c) && cull.wants_file(c) && plan(c).file {
        wants.push(cull::Want::for_loupe(c, files[c].clone(), 0));
    }
    // The local previews of the frames with no picture yet, all of
    // them ahead of any file: under an offline root they are all there
    // is, and on a network mount they are up in milliseconds while a
    // file's read may take seconds, or hang. A frame out of reach with
    // no key to find a preview by is said so, and not asked for.
    for &f in &frames {
        if cull.cache.get(f).is_some() || cull.failed.contains_key(&f) || cull.no_local.contains(&f)
        {
            continue;
        }
        match plan(f) {
            cull::Plan {
                preview: Some(key), ..
            } => wants.push(cull::Want::local(f, files[f].clone(), size, key)),
            cull::Plan {
                preview: None,
                file: false,
            } => {
                cull.failed.insert(f, cull::NO_LOCAL_PREVIEW.to_string());
            }
            _ => {}
        }
    }
    // Then the files, for the frames whose view-size copy is not the
    // camera's yet, and whose file has not already failed them.
    for &f in &frames {
        let had = cull.cache.get(f).is_some_and(|p| !p.local);
        if had || !cull.wants_file(f) || !plan(f).file {
            continue;
        }
        wants.push(cull::Want::for_loupe(f, files[f].clone(), size));
    }
    st.prefetch.want(wants);
}

/// Where the loupe takes frame `file`'s picture from, by its root:
/// see [`cull::source_for`]. The key is the row's while the frame
/// stands in from it; a frame read from disk has none.
pub(crate) fn loupe_plan(
    files: &[PathBuf],
    from_row: &[crate::rows::FromRow],
    library: &crate::roots::Library,
    file: usize,
) -> cull::Plan {
    let Some(path) = files.get(file) else {
        return cull::source_for(true, false, None);
    };
    let offline = library.offline.iter().any(|r| path.starts_with(r));
    let remote = library.remote.iter().any(|(r, _)| path.starts_with(r));
    let key = from_row.get(file).and_then(|r| r.key.clone());
    cull::source_for(offline, remote, key)
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
    st.prefetch
        .want(vec![cull::Want::of_file(file, path, size)]);
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
    let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, file, app, true);
    let plane = Geometry {
        turns,
        flip,
        ..Geometry::default()
    }
    .plane_size(preview.source.0 as f32, preview.source.1 as f32);
    let shown = (plane.0.round() as u32, plane.1.round() as u32);
    let (small, local) = (preview.small(), preview.local);
    st.zoom = 0.0;
    st.image_size = (0, 0);
    say_placeholder(app, shown, small, local);
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
pub(crate) fn say_placeholder(app: &App, shown: (u32, u32), small: bool, local: bool) {
    let line = placeholder::status(shown, small, local);
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
        // Never a full-size copy's size, so never taken for one below.
        cull::Loaded::NoPreview { file, path } => (*file, path.clone(), u32::MAX),
    };
    // A preview of a list since replaced would land on a stranger's
    // slot: only its own file's.
    if st.files.get(file) != Some(&path) {
        return;
    }
    let current = st.current;
    let culling = st.cull.is_some();
    let plan = loupe_plan(&st.files, &st.from_row, &st.library, file);
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
            if preview.local {
                // The preview and the file are fetched on two threads,
                // and a preview that lands after the camera's JPEG does
                // not take its place.
                let slot = if preview.full {
                    cull.cache.full(file)
                } else {
                    cull.cache.get(file)
                };
                if slot.is_some_and(|p| !p.local) {
                    return;
                }
                // The file failed before the preview landed: the
                // preview is what shows, and the file is not asked for
                // again, as when it fails after.
                if cull.failed.remove(&file).is_some() {
                    cull.file_failed.insert(file);
                }
            }
            cull.cache.insert(file, preview);
            true
        }
        // The local preview is up and stays up: the file's failure is
        // for the log, and the file is not asked for again.
        cull::Loaded::Failed { message, .. } if cull.cache.get(file).is_some_and(|p| p.local) => {
            tracing::warn!(
                "preview {}: {message}; the local preview stays",
                file_name(&path)
            );
            cull.file_failed.insert(file);
            false
        }
        cull::Loaded::Failed { message, .. } => {
            tracing::warn!("preview {}: {message}", file_name(&path));
            cull.failed.insert(file, message);
            false
        }
        // No local preview kept: the file is what shows, where it can
        // be read; where it cannot, there is nothing.
        cull::Loaded::NoPreview { .. } => {
            if plan.file {
                cull.no_local.insert(file);
            } else {
                cull.failed.insert(file, cull::NO_LOCAL_PREVIEW.to_string());
            }
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
        let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, c, app, from_panel(c));
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
    // A frame out of reach has its local preview and nothing more.
    if !holding
        && st.zoom > 0.0
        && !cull.cache.has_own_size(c)
        && cull.wants_file(c)
        && cull.full_asked != Some(c)
        && loupe_plan(&st.files, &st.from_row, &st.library, c).file
    {
        cull.full_asked = Some(c);
        st.prefetch
            .push_front(cull::Want::for_loupe(c, st.files[c].clone(), 0));
    }
    let mut tiles = Vec::new();
    let mut overlay = Vec::new();
    let mut ready = true;
    let mut names = Vec::new();
    for (k, &row) in rows.iter().enumerate() {
        let file = st.shown[row];
        let rect = rects[k];
        let meta = &st.sidecars[file].meta;
        overlay.push(CompareTile {
            x: rect.0 as f32 / scale_factor,
            y: rect.1 as f32 / scale_factor,
            w: rect.2 as f32 / scale_factor,
            h: rect.3 as f32 / scale_factor,
            name: file_name(&st.files[file]).into(),
            on: file == c,
            rating: meta.rating.min(meta::STARS) as i32,
            flag: meta.flag.code(),
            label: meta.label.code(),
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
        let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, file, app, from_panel(file));
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
        names.push(Named {
            file,
            size: (full.0.round() as u32, full.1.round() as u32),
            small: preview.small(),
            own: preview.own_size(),
            local: preview.local,
        });
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
    if holding && let Some(n) = names.iter().find(|n| n.file == c) {
        say_placeholder(app, n.size, n.small, n.local);
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
            // `--cull-key`: the key, from the event loop; the flag
            // is cleared there, as `--turn`'s is, so the capture
            // waits for the frame after it.
            if let Some(key) = st.cull_key_at_start.clone() {
                let app_weak = app.as_weak();
                let state = state.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        state.borrow_mut().cull_key_at_start = None;
                        app.invoke_meta_key(key.into());
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
        && st.cull_key_at_start.is_none()
        && !st.awaiting_index
        && !st.library.awaiting
        && st.asked.is_empty()
        && grid_filled_rows(
            &st.shown,
            &st.thumb_made,
            &st.thumb_failed,
            st.thumb_want,
            st.grid_shown,
            st.grid_wait_over,
            app,
        );
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
    from_row: &[crate::rows::FromRow],
    file: usize,
    app: &App,
    from_panel: bool,
) -> (u8, bool) {
    // A frame whose sidecar stands in from its row: the row says.
    if let Some(from) = from_row.get(file)
        && !from.read
    {
        return from.turns;
    }
    let turn = sidecars[file].turn;
    if from_panel {
        read_geometry(app).shown_turns(turn)
    } else {
        sidecars[file].current.geometry.shown_turns(turn)
    }
}

/// What a tile of the loupe shows, for the status line: the frame,
/// its size as shown, whether the camera's preview is a small one,
/// whether this copy is its every pixel, and whether it is the local
/// preview rather than the camera's JPEG.
pub(crate) struct Named {
    pub(crate) file: usize,
    pub(crate) size: (u32, u32),
    pub(crate) small: bool,
    pub(crate) own: bool,
    pub(crate) local: bool,
}

/// The picture shown, in the status line's words: the camera's JPEG,
/// a small camera preview, or the local preview kept in the cache.
pub(crate) fn picture_words((w, h): (u32, u32), small: bool, local: bool) -> String {
    if local {
        format!("the local preview, {w} \u{d7} {h}")
    } else if small {
        format!("a small camera preview, {w} \u{d7} {h}")
    } else {
        format!("the camera JPEG, {w} \u{d7} {h}")
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
    names: &[Named],
) -> String {
    let mode = if cull.compare > 1 {
        format!("culling, {} up", cull.compare)
    } else {
        "culling".to_string()
    };
    if let Some(why) = cull.failed.get(&current) {
        if why == cull::NO_LOCAL_PREVIEW {
            return format!("{mode}: nothing to show; {why}");
        }
        return format!("{mode}: no camera preview ({why}); Enter develops");
    }
    let Some(named) = names.iter().find(|n| n.file == current) else {
        return format!("{mode}: decoding the camera JPEG...");
    };
    if !shown {
        return format!("{mode}: decoding the camera JPEG...");
    }
    let what = picture_words(named.size, named.small, named.local);
    let at = if zoom <= 0.0 {
        "fitted".to_string()
    } else if named.own {
        format!("{}%", (zoom * 100.0).round() as i32)
    } else {
        format!(
            "{}%, screen-size copy until the full one decodes",
            (zoom * 100.0).round() as i32
        )
    };
    format!("{mode}: {what}, {at}; Enter develops")
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
            app.invoke_step(1, false);
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

/// Where the rejects go, said: the one rejects folder by its whole
/// path, or, over the all-roots view where the rejects are in several
/// folders, that each goes to its own folder's.
fn rejects_where(st: &State) -> Option<String> {
    let files: Vec<usize> = st
        .files
        .iter()
        .enumerate()
        .filter(|(i, _)| to_move_out(st, *i))
        .map(|(i, _)| i)
        .collect();
    rejects_where_of(st, &files)
}

/// Where these frames go, said as `rejects_where` says it.
fn rejects_where_of(st: &State, files: &[usize]) -> Option<String> {
    let mut folders: Vec<&Path> = Vec::new();
    for &i in files {
        if let Some(p) = st.files.get(i).and_then(|f| f.parent())
            && !folders.contains(&p)
        {
            folders.push(p);
        }
    }
    match folders.len() {
        0 => shoot_rejects_dir(st).map(|d| d.display().to_string()),
        1 => {
            let shoot = std::fs::canonicalize(folders[0]).unwrap_or_else(|_| folders[0].into());
            Some(cull::rejects_dir(&shoot).display().to_string())
        }
        n => Some(format!(
            "a rejects folder in each of the {n} folders they are in"
        )),
    }
}

/// Whether file `i` is a reject still to be moved out: flagged, and
/// not in a rejects folder already, as the all-roots view lists those.
pub(crate) fn to_move_out(st: &State, i: usize) -> bool {
    st.sidecars
        .get(i)
        .is_some_and(|s| s.meta.flag == meta::Flag::Reject)
        // Under a root that is offline nothing is moved and nothing is
        // made: a rejects folder made where the drive was would be a
        // folder on the mount point, and its frames marked missing.
        && !crate::rows::is_offline(st, i)
        && st
            .files
            .get(i)
            .and_then(|f| f.parent())
            .and_then(Path::file_name)
            != Some(std::ffi::OsStr::new(cull::REJECTS))
}

/// The move-rejects sheet: how many frames and where they would go.
pub(crate) fn ask_rejects(st: &State, app: &App) {
    if st.deleting.is_some() {
        app.set_status("a delete is still under way".into());
        return;
    }
    let n = reject_count(st);
    if n == 0 {
        app.set_status("no frames are flagged reject".into());
        return;
    }
    let Some(dir) = rejects_where(st) else {
        return;
    };
    app.set_rejects_text(
        format!(
            "{n} frame{} flagged reject, with {} sidecar{}, will be moved to {}.",
            if n == 1 { "" } else { "s" },
            if n == 1 { "its" } else { "their" },
            if n == 1 { "" } else { "s" },
            dir
        )
        .into(),
    );
    app.set_rejects_open(true);
}

/// Move the rejects out, and take them out of the browser's list.
/// The frame the selection was on, if it went, gives way to the
/// nearest one left.
pub(crate) fn move_rejects(st: &mut State, app: &App, worker: &Worker) {
    if st.deleting.is_some() {
        app.set_status("a delete is still under way".into());
        return;
    }
    // Those already out, in a rejects folder, stay where they are and
    // are not counted.
    let rejected: Vec<usize> = (0..st.files.len())
        .filter(|&i| to_move_out(st, i))
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
    // Where the ones that went, went: said while the list still has
    // them where they were.
    let dir = rejects_where_of(st, &moved.files).unwrap_or_default();
    let mut status = format!(
        "moved {} frame{} and {} sidecar{} to {}",
        moved.files.len(),
        if moved.files.len() == 1 { "" } else { "s" },
        moved.sidecars,
        if moved.sidecars == 1 { "" } else { "s" },
        dir
    );
    status.push_str(&left_words(&moved.skipped));
    tracing::info!("{status}");
    hand_moves(st, &moved);
    hand_folders(st, &moved);
    drop_files(st, app, worker, &moved.files, status);
}

/// The window's own moves told to the indexer, each row taken from the
/// old path to the new: a pass would see a move only by the content
/// key, and not at all out of a folder the move emptied, which it
/// takes for a drive that is away. Called with the list still holding
/// the old paths, and ahead of the folders' passes.
fn hand_moves(st: &State, moved: &cull::Moved) {
    if let Some(indexer) = &st.index {
        indexer.moved(
            moved
                .files
                .iter()
                .zip(&moved.to)
                .map(|(&i, to)| (st.files[i].clone(), to.clone()))
                .collect(),
        );
    }
}

/// The folders a move touched, the one each frame left and the one it
/// went to, passed over by the indexer now rather than at the watcher's
/// word or the next poll, so the grid, the tree and the counts follow
/// at once. Called with the list still holding the old paths.
fn hand_folders(st: &State, moved: &cull::Moved) {
    let mut folders: Vec<PathBuf> = Vec::new();
    for (&i, to) in moved.files.iter().zip(&moved.to) {
        for dir in [st.files[i].parent(), to.parent()].into_iter().flatten() {
            if !folders.iter().any(|f| f == dir) {
                folders.push(dir.to_path_buf());
            }
        }
    }
    if let Some(indexer) = &st.index
        && !folders.is_empty()
    {
        indexer.asker().changes(
            folders
                .into_iter()
                .map(greycard_library::Change::Folder)
                .collect(),
        );
    }
}

/// What a move left where it was, as the status line's tail: how
/// many, and the first one's reason; empty when it left none.
fn left_words(skipped: &[(usize, String)]) -> String {
    let Some((_, why)) = skipped.first() else {
        return String::new();
    };
    format!(
        "; {} left where {} ({why}{})",
        skipped.len(),
        if skipped.len() == 1 {
            "it was"
        } else {
            "they were"
        },
        if skipped.len() > 1 {
            "; the log has each"
        } else {
            ""
        }
    )
}

/// Whether file `i` can be moved back: it is in a rejects folder, and
/// not under a root that is offline, where nothing is moved.
pub(crate) fn to_move_back(st: &State, i: usize) -> bool {
    st.files
        .get(i)
        .is_some_and(|f| cull::above_rejects(f).is_some())
        && !crate::rows::is_offline(st, i)
}

/// The frames Move back is over: those of the selection in a rejects
/// folder.
fn back_frames(st: &State) -> Vec<usize> {
    crate::panel::browser::chosen_frames(st)
        .into_iter()
        .filter(|&i| to_move_back(st, i))
        .collect()
}

/// The folders above the rejects folders `frames` are in, each once.
fn folders_above(st: &State, frames: &[usize]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for above in frames
        .iter()
        .filter_map(|&i| st.files.get(i).and_then(|f| cull::above_rejects(f)))
    {
        if !out.contains(&above) {
            out.push(above);
        }
    }
    out
}

/// A folder's own name, for a line: from the path when it has one,
/// and from its canonical form when it is "." or "..".
fn folder_name(dir: &Path) -> String {
    let named = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    named(dir)
        .filter(|n| n != "..")
        .or_else(|| dunce::canonicalize(dir).ok().as_deref().and_then(named))
        .unwrap_or_else(|| {
            dunce::canonicalize(dir)
                .unwrap_or_else(|_| dir.into())
                .display()
                .to_string()
        })
}

/// Move back on the frame menu and in CULLING: how many of the
/// selection are in a rejects folder, and the folder above it by name
/// ("their folders" when they are in more than one); none hides it.
pub(crate) fn show_back(st: &State, app: &App) {
    let frames = back_frames(st);
    let to = match folders_above(st, &frames).as_slice() {
        [] => String::new(),
        // Named from the path alone, with no look at the disk: this is
        // asked at every change of the selection. Only a frame opened
        // by a bare relative name (`rejects/A.CR3`, the folder above
        // the working one) has no name in its path, and is looked up.
        [one] => folder_name(one),
        _ => "their folders".into(),
    };
    app.set_back_count(frames.len() as i32);
    app.set_back_to(to.into());
}

/// Move back: the frames of the selection in a rejects folder into
/// the folder above it, with their sidecars, out of the browser's
/// list as Move rejects takes its frames out. A frame moved back is
/// no longer a reject: the move is the user saying so, and a flag
/// left on would send it out again at the next Move rejects, so the
/// flag comes off in its sidecar where it now is.
pub(crate) fn move_back(st: &mut State, app: &App, worker: &Worker) {
    if st.deleting.is_some() {
        app.set_status("a delete is still under way".into());
        return;
    }
    let frames = back_frames(st);
    if frames.is_empty() {
        app.set_status("no frame chosen is in a rejects folder".into());
        return;
    }
    let moved = cull::move_back(&st.files, &frames);
    for (i, why) in &moved.skipped {
        tracing::warn!("{} left where it is: {why}", file_name(&st.files[*i]));
    }
    // Said while the list still has them where they were.
    let above = folders_above(st, &moved.files);
    let mut status = match above.as_slice() {
        [] => "nothing moved back".to_string(),
        many => format!(
            "moved {} frame{} and {} sidecar{} back to {}",
            moved.files.len(),
            if moved.files.len() == 1 { "" } else { "s" },
            moved.sidecars,
            if moved.sidecars == 1 { "" } else { "s" },
            match many {
                [one] => dunce::canonicalize(one)
                    .unwrap_or_else(|_| one.clone())
                    .display()
                    .to_string(),
                _ => format!(
                    "the folder above each of the {} rejects folders",
                    many.len()
                ),
            }
        ),
    };
    status.push_str(&left_words(&moved.skipped));
    tracing::info!("{status}");
    // The frames' flags written at their new paths, then the index told
    // of the moves, then both folders passed over. A save's word that
    // reaches the index before the move's makes a row at the new path,
    // which the move's then puts the old row over, reading the sidecar
    // again; one after it finds the row moved. Either way the row keeps
    // its id and has the flag as written.
    let old: Vec<PathBuf> = moved.files.iter().map(|&i| st.files[i].clone()).collect();
    for (&i, to) in moved.files.iter().zip(&moved.to) {
        st.files[i] = to.clone();
        unreject(st, i);
    }
    for (&i, from) in moved.files.iter().zip(old) {
        st.files[i] = from;
    }
    hand_moves(st, &moved);
    hand_folders(st, &moved);
    drop_files(st, app, worker, &moved.files, status);
}

/// The reject flag taken off frame `i`, moved back to the path the list
/// now has for it, and its sidecar written there: through the window's
/// own write when the sidecar in memory is the frame's, and otherwise
/// (a row of the index standing in for it) read from the disk, changed
/// and written back where it was found.
fn unreject(st: &mut State, i: usize) {
    if st.sidecars[i].meta.flag != meta::Flag::Reject {
        return;
    }
    st.sidecars[i].meta.flag = meta::Flag::None;
    if crate::panel::edit::writable(st, i) {
        crate::panel::edit::write_sidecar(st, i);
        return;
    }
    if !st.write_sidecars {
        return;
    }
    let path = st.files[i].clone();
    let placement = greycard_edit::Placement::of(&path);
    match Sidecar::load(&path) {
        Ok(Some(mut sidecar)) if sidecar.meta.flag == meta::Flag::Reject => {
            sidecar.meta.flag = meta::Flag::None;
            if st.xmp_sidecars
                && let Err(e) = greycard_edit::xmp::save(&path, &sidecar.meta, None)
            {
                tracing::warn!("{}: xmp not written: {e}", file_name(&path));
            }
            if let Err(e) = sidecar.save_in(&path, placement) {
                tracing::warn!("{}: sidecar not saved: {e}", file_name(&path));
            }
            crate::library::sidecar_written(st, i);
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("{}: sidecar not read: {e}", file_name(&path)),
    }
}

/// Take the files at `gone` (indices into the list) out of the
/// browser's list, as moving them out or deleting them does, and say
/// `status`. Everything numbered by the list is numbered again; the
/// culling pictures, keyed by the old numbers, are put down and made
/// again. The frame the selection was on, if it went, gives way to
/// the nearest one left.
pub(crate) fn drop_files(
    st: &mut State,
    app: &App,
    worker: &Worker,
    gone: &[usize],
    status: String,
) {
    if gone.is_empty() {
        app.set_status(status.into());
        return;
    }
    let current_path = st.current.map(|c| st.files[c].clone());
    let keep = |i: &usize| !gone.contains(i);
    // Where the frame on screen would stand in the new list if it is
    // among those going: the frame after it that stays, else the last.
    // `rebuild_browser` cannot say, since it looks for the current
    // frame and there is none then.
    let stand_in = st.current.filter(|c| gone.contains(c)).and_then(|c| {
        let kept = (0..st.files.len()).filter(keep).count();
        (kept > 0).then(|| (0..c).filter(keep).count().min(kept - 1))
    });
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
    st.from_row = kept.iter().map(|&i| st.from_row[i].clone()).collect();
    st.thumb_base = kept.iter().map(|&i| st.thumb_base[i].take()).collect();
    st.thumb_shown = kept.iter().map(|&i| st.thumb_shown[i]).collect();
    st.thumb_made = kept.iter().map(|&i| st.thumb_made[i]).collect();
    st.thumb_failed = kept.iter().map(|&i| st.thumb_failed[i]).collect();
    st.thumb_asked = kept.iter().map(|&i| st.thumb_asked[i]).collect();
    // The index's answers by the new numbering, and a pass over the
    // folder, which marks the rows of frames moved out missing (a
    // delete has had its rows forgotten already).
    st.index_passed = kept
        .iter()
        .map(|&i| st.index_passed.get(i).copied().unwrap_or(true))
        .collect();
    crate::library::index_open_folder(st, None);
    // The indices the previews and the worker's thumbnails were
    // keyed by have moved: the previews are decoded again (cheap),
    // and a thumbnail still owed is asked for again.
    if let Some(cull) = st.cull.as_mut() {
        cull.cache.clear();
        cull.textures.clear();
        cull.clear_failures();
    }
    drop_placeholder(st, app);
    st.hold = None;
    st.prefetch.want(Vec::new());
    if let Some(run) = st.thumb_run.as_mut() {
        let from: Vec<Option<usize>> = kept.iter().map(|&i| Some(i)).collect();
        run.renumber(&from);
    }
    // The rows are the old list's until the rebuild below, so none are
    // put first.
    crate::roots::ask_owed(st, worker);
    let went = current_path
        .as_ref()
        .map(|p| st.files.iter().position(|f| f == p));
    st.current = went.flatten();
    // The set was numbered by the old list; the frame on screen is
    // what is left of it.
    st.picked.clear();
    let next = rebuild_browser(st, app)
        .or_else(|| stand_in.and_then(|f| crate::cull::nearest_row(&st.shown, f)));
    app.set_status(status.into());
    match (st.current, next) {
        (Some(c), _) => {
            if st.cull.is_some() {
                cull_select(st, app, c);
            }
        }
        (None, Some(row)) => {
            // The frame on show went; the nearest left takes its
            // place, as a click on it would. In culling that shows
            // its JPEG; the first cut left the old picture up with
            // nothing chosen, since `next` was never set here.
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
    // The EXIF facets, counted by the index; none until it has
    // rows for the folder, when the note says why.
    let rows = crate::library::facet_rows(st);
    let (long, short) = crate::library::split_facet_rows(&rows);
    app.set_filter_facets(ModelRc::new(VecModel::from(rows)));
    app.set_filter_facets_long(ModelRc::new(VecModel::from(long)));
    app.set_filter_facets_short(ModelRc::new(VecModel::from(short)));
    app.set_filter_facet_note(crate::library::facet_note(st).into());
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
    apply_filter_said(state, app, true, true);
}

/// The same, for a character typed into the text field.
fn apply_filter_typed(state: &Rc<RefCell<State>>, app: &App) {
    apply_filter_said(state, app, false, true);
}

/// The same, for the index having moved under the filter: a pass
/// has reached more of the folder, or a row was written after a
/// save. `settled` when it is worth the log's line.
pub(crate) fn refilter(state: &Rc<RefCell<State>>, app: &App, settled: bool) {
    // The index moving is no reason to say a typing mistake again.
    apply_filter_said(state, app, settled, false);
}

fn apply_filter_said(state: &Rc<RefCell<State>>, app: &App, settled: bool, say_errors: bool) {
    let mut st = state.borrow_mut();
    let next = rebuild_browser(&mut st, app);
    let (what, shown, count) = (st.filter.describe(), st.shown.len(), st.files.len());
    if settled {
        tracing::info!("browser filter: {what}, {shown} of {count} frames");
    } else {
        tracing::debug!("browser filter: {what}, {shown} of {count} frames");
    }
    say_if_empty(&st, app);
    // A term typed that does not parse is looked for as a word, as
    // it was before the field took terms, and the reason is said.
    if say_errors && let Some(e) = st.filter.typed().errors.first() {
        app.set_status(format!("filter: {e}").into());
    }
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
    // The section's switch: kept in the settings as it is flipped,
    // as the Settings sheet's are, so a session that never closes
    // cleanly keeps it too.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_cull_move_on_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.cull_move_on = app.get_cull_move_on();
            let on = st.cull_move_on;
            crate::panel::prefs::keep(&st, |s| s.cull_move_on = on);
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
        app.on_filter_facet_toggled(move |code, value| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&facet) = filter::Facet::ALL.get(code.max(0) as usize) else {
                return;
            };
            state.borrow_mut().filter.toggle_facet(facet, &value);
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
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_move_back_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                move_back(&mut state.borrow_mut(), &app, &worker);
            }
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

    /// A frame's local preview and its file land in either order, on
    /// two threads: a preview after the camera's JPEG does not replace
    /// it, and a file failure before the preview leaves the preview up
    /// with no failure said over it and the file not asked for again.
    #[test]
    fn a_late_local_preview_and_an_early_file_failure_leave_the_right_picture() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        {
            let mut st = state.borrow_mut();
            st.current = Some(1);
            enter_cull(&mut st, &app, 1);
        }
        let local = |st: &State, file: usize| cull::Loaded::Ok {
            file,
            path: st.files[file].clone(),
            size: 1024,
            preview: Arc::new(cull::Preview::local(
                greycard_library::thumbs::Thumb {
                    width: 4,
                    height: 2,
                    rgb: vec![0; 24],
                },
                false,
            )),
        };
        // The camera's JPEG first, then a late local preview.
        let camera = camera_picture(&state.borrow(), 1, (8192, 5464));
        deliver_preview(&app, camera);
        let late = local(&state.borrow(), 1);
        deliver_preview(&app, late);
        {
            let st = state.borrow();
            let shown = st.cull.as_ref().unwrap().cache.get(1).unwrap();
            assert!(!shown.local, "the camera's JPEG stays");
        }
        // The file's failure first, then the local preview.
        let failed = cull::Loaded::Failed {
            file: 2,
            path: state.borrow().files[2].clone(),
            size: 1024,
            message: "the share did not answer".into(),
        };
        deliver_preview(&app, failed);
        assert!(
            state
                .borrow()
                .cull
                .as_ref()
                .unwrap()
                .failed
                .contains_key(&2)
        );
        let preview = local(&state.borrow(), 2);
        deliver_preview(&app, preview);
        let mut st = state.borrow_mut();
        {
            let cull = st.cull.as_ref().unwrap();
            assert!(cull.cache.get(2).is_some_and(|p| p.local), "up");
            assert!(!cull.failed.contains_key(&2), "no failure said over it");
            assert!(cull.file_failed.contains(&2));
        }
        cull_refresh(&mut st);
        assert!(!st.prefetch.asked().iter().any(|w| w.file == 2));
    }

    /// The frame's full-size copy held from 1:1 and no view-size one
    /// (the compare view entered from 1:1 sets the fit and asks again
    /// before the next frame lets the full copy go): the view-size copy
    /// is asked for all the same, or its tile would stay blank.
    #[test]
    fn a_held_full_copy_does_not_stand_in_for_the_view_size_one() {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        let mut st = state.borrow_mut();
        st.current = Some(1);
        enter_cull(&mut st, &app, 1);
        let size = st.cull.as_ref().unwrap().size;
        let full = Arc::new(cull::Preview::new(8, 6, vec![0; 192], (8192, 5464), true));
        st.cull.as_mut().unwrap().cache.insert(1, full);
        st.zoom = 0.0;
        cull_refresh(&mut st);
        let asked = st.prefetch.asked();
        assert!(
            asked
                .iter()
                .any(|w| w.file == 1 && w.size == size && w.from == cull::Source::File),
            "{asked:?}"
        );
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
            "camera preview: the camera JPEG, 8192 \u{d7} 5464, fitted; developing..."
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
        developed_at(generation, size, 0)
    }

    /// The same, developed at `turn` quarter turns of the frame.
    fn developed_at(generation: u64, size: (u32, u32), turn: u8) -> crate::worker::Outcome {
        crate::worker::Outcome::Developed {
            generation,
            turn,
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
        let turns_before = {
            let st = state.borrow();
            thumb_turns_of(&st.sidecars, &st.from_row, 1, &app, true)
        };
        app.invoke_frame_turned(1);
        let st = state.borrow();
        assert!(st.generation > before, "the develop was not re-keyed");
        assert_eq!(standing_in(&st), Some(1), "the turn took it down");
        assert_ne!(
            turns_before,
            thumb_turns_of(&st.sidecars, &st.from_row, 1, &app, true),
            "the camera picture would be drawn the same way up"
        );
        assert!(app.get_placeholder());
        assert!(st.bins.is_none(), "the last frame's bins came back");
    }

    /// A frame turned with its develop on screen is drawn turned on
    /// the frame the key was pressed on: the develop there is read
    /// through the turn and the size the edit is measured by stands
    /// on end, without waiting for the develop at the new turn. A
    /// second turn before that lands stacks on the first, a develop
    /// asked for at the turn between is dropped when it lands late,
    /// and the one at the turn on screen settles it to no difference.
    #[test]
    fn a_turn_is_drawn_at_once_from_the_develop_on_screen() {
        use crate::panel::viewport::drawn_picture;
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        // What the render does with a develop that has landed.
        let land = |outcome| {
            crate::panel::deliver::deliver(&app, outcome);
            let mut st = state.borrow_mut();
            st.pending.take().expect("the develop was taken");
            st.source_size = st.shown_size;
            crate::panel::viewport::settle_source_size(&mut st);
        };
        let generation = state.borrow().generation;
        land(developed(generation, (6000, 4000)));
        assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 0));

        app.invoke_frame_turned(1);
        let first = state.borrow().generation;
        {
            let st = state.borrow();
            assert_eq!(st.sidecars[1].turn, 1);
            assert_eq!(
                drawn_picture(&st),
                ((4000, 6000), 1),
                "the turn waited for its develop"
            );
            assert_eq!(st.source_size, (4000, 6000));
            // The edit is measured on the source standing on end.
            let frame = st.edit.geometry.frame(4000.0, 6000.0);
            assert!(frame.size.1 > frame.size.0, "{:?}", frame.size);
        }

        app.invoke_frame_turned(1);
        assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 2));
        // The first turn's develop, late: stale, and nothing moves.
        land_if_taken(&app, &state, developed_at(first, (4000, 6000), 1));
        assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 2));

        // The second's lands, and the picture on the GPU is turned.
        let generation = state.borrow().generation;
        land(developed_at(generation, (6000, 4000), 2));
        assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 0));

        // Turned under the culling loupe, which draws the camera's
        // picture: once culling is left, the develop on screen is
        // drawn turned by the same rule, measured the way up it is.
        app.invoke_cull_toggled();
        app.invoke_frame_turned(1);
        app.invoke_cull_leave();
        {
            let st = state.borrow();
            assert_eq!(drawn_picture(&st), ((4000, 6000), 1));
            assert_eq!(st.source_size, (4000, 6000));
        }

        // Another frame chosen: the develop on screen is the last
        // frame's, under its own edit, and no turn of this one is
        // read into it.
        app.invoke_select(2);
        app.invoke_frame_turned(1);
        assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 0));
    }

    /// A delivery that may be dropped as stale: taken, if it was not.
    fn land_if_taken(app: &App, state: &Rc<RefCell<State>>, outcome: crate::worker::Outcome) {
        crate::panel::deliver::deliver(app, outcome);
        let mut st = state.borrow_mut();
        if st.pending.take().is_some() {
            st.source_size = st.shown_size;
            crate::panel::viewport::settle_source_size(&mut st);
        }
    }

    /// The size the open frame is measured by follows its turn
    /// however the turn came: a selection turned from the culling
    /// loupe while another frame was under it, and a turn pressed
    /// between a develop's delivery and the frame that takes it up.
    #[test]
    fn the_measured_size_follows_a_turn_however_it_came() {
        use crate::panel::viewport::drawn_picture;
        let app = window(4);
        let (state, worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let generation = state.borrow().generation;
        land_if_taken(&app, &state, developed(generation, (6000, 4000)));
        assert_eq!(state.borrow().source_size, (6000, 4000));

        // Culling, on to frame 2, and both turned together; then
        // back to frame 1 and out, with no camera picture of it.
        app.invoke_cull_toggled();
        app.invoke_select(2);
        crate::panel::browser::turn_frames(&mut state.borrow_mut(), &app, &worker, &[1, 2], 1);
        app.invoke_select(1);
        {
            let st = state.borrow();
            assert_eq!(st.current, Some(1));
            assert_eq!(
                st.source_size,
                (4000, 6000),
                "back on frame 1 under the loupe"
            );
        }
        app.invoke_cull_leave();
        {
            let st = state.borrow();
            assert_eq!(drawn_picture(&st), ((4000, 6000), 1));
            assert_eq!(st.source_size, (4000, 6000));
        }

        // Its develop delivered, and a turn before the frame that
        // takes it up: that frame measures it on end again.
        let generation = state.borrow().generation;
        crate::panel::deliver::deliver(&app, developed_at(generation, (4000, 6000), 1));
        app.invoke_frame_turned(1);
        {
            let mut st = state.borrow_mut();
            // What the render does with it.
            st.pending.take().expect("the develop was taken");
            st.source_size = (4000, 6000);
            crate::panel::viewport::settle_source_size(&mut st);
            assert_eq!(st.source_size, (6000, 4000));
            assert_eq!(drawn_picture(&st), ((6000, 4000), 1));
        }
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

    /// The frame on screen taken out of the list (moved with the
    /// rejects, or deleted): the nearest frame left is chosen and
    /// opens, in culling as in the loupe. The first cut left the old
    /// picture up with no row chosen, since the row to select was
    /// asked of `rebuild_browser`, which has no current frame then.
    #[test]
    fn the_frame_on_screen_dropped_from_the_list_hands_on_to_the_next() {
        let app = window(5);
        let (state, worker) = state_for(&app, folder(5));
        app.invoke_select(2);
        app.invoke_cull_toggled();
        assert!(state.borrow().cull.is_some());
        assert_eq!(state.borrow().current, Some(2));
        drop_files(&mut state.borrow_mut(), &app, &worker, &[2], "gone".into());
        assert_eq!(state.borrow().files.len(), 4);
        // The select is a timer away, as a click's would be.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(
            state.borrow().current,
            Some(2),
            "the frame after took its place"
        );
        assert_eq!(app.get_selected(), 2);
        assert_eq!(app.get_file_name(), "IMG_0003.CR3");
        assert!(state.borrow().cull.is_some(), "culling was left");

        // The last frame: the one before takes over.
        app.invoke_select(3);
        drop_files(&mut state.borrow_mut(), &app, &worker, &[3], "gone".into());
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        assert_eq!(state.borrow().current, Some(2));
        assert_eq!(app.get_file_name(), "IMG_0003.CR3");
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
        let weak = app.as_weak();
        app.on_rejects_answered(move |yes| {
            seen.borrow_mut().push(yes);
            weak.unwrap().set_rejects_open(false);
        });
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
        // The rejects sheet takes every key it is shown; Return is
        // its Move, once, and Escape its no.
        app.set_rejects_open(true);
        a_moment_on(&app);
        press(&app, "c");
        press(&app, "4");
        assert_eq!(*toggles.borrow(), 1);
        assert_eq!(*leaves.borrow(), 2);
        assert_eq!(rated.borrow().len(), 2);
        press(&app, Key::Return);
        assert_eq!(*answers.borrow(), vec![true]);
        assert!(!app.get_rejects_open());
        assert_eq!(*leaves.borrow(), 2);
        app.set_rejects_open(true);
        a_moment_on(&app);
        press(&app, Key::Escape);
        assert_eq!(*answers.borrow(), vec![true, false]);
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

    /// Ctrl+Z in culling steps back the ratings, flags and labels the
    /// session gave, newest first, going to each frame it changes;
    /// Ctrl+Shift+Z makes them again. No develop moves, and culling
    /// stays up.
    #[test]
    fn undo_in_culling_takes_back_the_tags_and_nothing_else() {
        use greycard_edit::meta::Flag;
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            // A develop step on the frame, for undo to leave alone.
            let mut e = st.sidecars[0].current.clone();
            e.light.exposure = 1.0;
            st.sidecars[0].record(e);
            enter_cull(&mut st, &app, 1);
        }
        let develop = |st: &State| (st.sidecars[0].states(), st.sidecars[0].position());
        let before = develop(&state.borrow());
        let tags = |st: &State, i: usize| (st.sidecars[i].meta.rating, st.sidecars[i].meta.flag);

        app.invoke_meta_key("3".into());
        app.invoke_select(1);
        app.invoke_meta_key("p".into());
        assert_eq!(tags(&state.borrow(), 0), (3, Flag::None));
        assert_eq!(tags(&state.borrow(), 1), (0, Flag::Pick));

        // The pick first, on the frame on screen.
        app.invoke_undo();
        assert_eq!(tags(&state.borrow(), 1), (0, Flag::None));
        assert_eq!(state.borrow().current, Some(1));
        // Then the stars, and over to the frame that had them.
        app.invoke_undo();
        assert_eq!(tags(&state.borrow(), 0), (0, Flag::None));
        assert_eq!(state.borrow().current, Some(0));
        assert_eq!(
            app.get_thumbs().row_data(0).unwrap().rating,
            0,
            "the badge too"
        );
        // Nothing left: nothing moves.
        app.invoke_undo();
        assert_eq!(state.borrow().current, Some(0));

        app.invoke_redo();
        assert_eq!(tags(&state.borrow(), 0), (3, Flag::None));
        assert!(state.borrow().cull.is_some(), "still culling");
        assert_eq!(
            develop(&state.borrow()),
            before,
            "the develop was not touched"
        );

        // A new rating drops what was left to redo.
        app.invoke_meta_key("5".into());
        app.invoke_redo();
        assert_eq!(tags(&state.borrow(), 1), (0, Flag::None));
    }

    /// The CULLING section's switch: off, a key leaves the selection
    /// where it is; on, a rating, a flag or a label moves it on to
    /// the next frame by the arrow's path and stops at the end, and
    /// each key's tags land on the frame it was pressed over. Undo
    /// still puts that frame back and goes to it, and the word over
    /// the picture and the loupe's badge say what the frame carries.
    #[test]
    fn move_on_advances_after_a_key_stops_at_the_end_and_undo_goes_back() {
        use greycard_edit::meta::{Flag, Label};
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(0);
        enter_cull(&mut state.borrow_mut(), &app, 1);
        assert!(!app.get_cull_move_on(), "off by default");
        assert!(!state.borrow().cull_move_on);

        // Off: the key and the arrow are two presses.
        app.invoke_meta_key("2".into());
        assert_eq!(state.borrow().current, Some(0));
        assert_eq!(app.get_notice(), "2 stars");
        assert!(app.get_notice_on());
        assert_eq!(app.get_frame_rating(), 2, "the loupe's badge");

        app.set_cull_move_on(true);
        app.invoke_cull_move_on_changed();
        assert!(state.borrow().cull_move_on);

        app.invoke_meta_key("3".into());
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(app.get_selected(), 1);
        assert_eq!(app.get_frame_rating(), 0, "the next frame's badge");
        assert_eq!(app.get_notice(), "3 stars");
        assert!(app.get_notice_on(), "moving on does not take the word down");
        app.invoke_meta_key("p".into());
        assert_eq!(state.borrow().current, Some(2));
        app.invoke_meta_key("6".into());
        assert_eq!(state.borrow().current, Some(3));
        assert_eq!(app.get_notice(), "Red");
        // At the end: nothing past it, and the key still lands.
        app.invoke_meta_key("x".into());
        assert_eq!(state.borrow().current, Some(3));
        assert_eq!(app.get_selected(), 3);
        assert_eq!(app.get_notice(), "Rejected");

        // Each on the frame it was pressed over, and on no other.
        let tags = |st: &State, i: usize| {
            let m = &st.sidecars[i].meta;
            (m.rating, m.flag, m.label)
        };
        {
            let st = state.borrow();
            assert_eq!(tags(&st, 0), (3, Flag::None, Label::None));
            assert_eq!(tags(&st, 1), (0, Flag::Pick, Label::None));
            assert_eq!(tags(&st, 2), (0, Flag::None, Label::Red));
            assert_eq!(tags(&st, 3), (0, Flag::Reject, Label::None));
        }
        assert_eq!(app.get_frame_flag(), Flag::Reject.code());

        // Undo: the reject off frame 3, which is on screen; then the
        // label off frame 2, and over to it.
        app.invoke_undo();
        assert_eq!(tags(&state.borrow(), 3), (0, Flag::None, Label::None));
        assert_eq!(state.borrow().current, Some(3));
        assert_eq!(app.get_frame_flag(), 0);
        assert_eq!(app.get_notice(), "Undo: Unflagged", "the step's word");
        app.invoke_undo();
        assert_eq!(tags(&state.borrow(), 2), (0, Flag::None, Label::None));
        assert_eq!(state.borrow().current, Some(2));
        assert_eq!(app.get_frame_label(), 0);
        assert_eq!(app.get_notice(), "Undo: No label");
        // Redo makes it again and stays: a redo is not a key.
        app.invoke_redo();
        assert_eq!(tags(&state.borrow(), 2), (0, Flag::None, Label::Red));
        assert_eq!(state.borrow().current, Some(2));
        assert_eq!(app.get_frame_label(), Label::Red.code());
        assert_eq!(app.get_notice(), "Redo: Red");
        // A key after the undo rates this frame and moves on again;
        // the label key on the red frame clears it, and says so.
        app.invoke_meta_key("4".into());
        assert_eq!(tags(&state.borrow(), 2), (4, Flag::None, Label::Red));
        assert_eq!(state.borrow().current, Some(3));
        app.invoke_select(2);
        app.invoke_meta_key("6".into());
        assert_eq!(tags(&state.borrow(), 2), (4, Flag::None, Label::None));
        assert_eq!(app.get_notice(), "No label");
    }

    /// A culling key held down is one press. The window's repeats of
    /// it are dropped before they reach the browser, so with move-on
    /// on a held 3 rates one frame and steps once, not a run of them.
    #[test]
    fn a_held_culling_key_is_one_press_and_one_step() {
        let app = window(6);
        let (state, _worker) = state_for(&app, folder(6));
        app.invoke_select(0);
        enter_cull(&mut state.borrow_mut(), &app, 1);
        app.set_cull_move_on(true);
        app.invoke_cull_move_on_changed();
        let text: slint::SharedString = "3".into();
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        for _ in 0..3 {
            app.window()
                .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
        }
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text });
        let st = state.borrow();
        assert_eq!(st.current, Some(1), "one step");
        let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
        assert_eq!(ratings, vec![3, 0, 0, 0, 0, 0], "one frame rated");
        assert_eq!(app.get_notice(), "3 stars");
    }

    /// Move-on stays out of two cases: a set of several frames, whose
    /// arrow would collapse it, and a frame the key took out of the
    /// filtered list, whose nearest is already the next.
    #[test]
    fn move_on_leaves_a_set_alone_and_does_not_skip_past_the_filters_nearest() {
        let app = window(5);
        let (state, _worker) = state_for(&app, folder(5));
        app.invoke_select(1);
        enter_cull(&mut state.borrow_mut(), &app, 1);
        app.set_cull_move_on(true);
        app.invoke_cull_move_on_changed();

        // Frames 1 and 2 as a set: rated together, and left there.
        app.invoke_frame_clicked(2, false, true);
        assert_eq!(
            crate::panel::browser::chosen_frames(&state.borrow()),
            vec![1, 2]
        );
        app.invoke_meta_key("4".into());
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(state.borrow().sidecars[1].meta.rating, 4);
        assert_eq!(state.borrow().sidecars[2].meta.rating, 4);
        assert_eq!(state.borrow().sidecars[3].meta.rating, 0);

        // Under "No rejects", a reject leaves the list: the nearest
        // frame shown takes the selection, once, not the one after.
        {
            let mut st = state.borrow_mut();
            st.filter = filter::Filter::from_name("No rejects").expect("it parses");
            rebuild_browser(&mut st, &app);
        }
        app.invoke_select(0);
        assert_eq!(state.borrow().current, Some(0));
        app.invoke_meta_key("x".into());
        assert_eq!(state.borrow().shown, vec![1, 2, 3, 4]);
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(app.get_selected(), 0);
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

    /// A shoot with a rejects folder in it: c.tif in the shoot, a.tif
    /// and b.tif in its rejects folder flagged reject, a's sidecar
    /// beside it with an XMP, b's under the hidden folder. The scratch
    /// folder, the shoot, and the three frames in that order.
    fn shoot_with_rejects(what: &str) -> (PathBuf, PathBuf, Vec<PathBuf>) {
        use greycard_library::fixture::{A7, R5, R6, write_frame};
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-back-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shoot = dir.join("shoot");
        let rejects = cull::rejects_dir(&shoot);
        std::fs::create_dir_all(&rejects).unwrap();
        let shoot = dunce::canonicalize(&shoot).unwrap();
        let rejects = cull::rejects_dir(&shoot);
        let files = vec![
            shoot.join("c.tif"),
            rejects.join("a.tif"),
            rejects.join("b.tif"),
        ];
        for (i, (f, camera)) in files.iter().zip([&R6, &R5, &A7]).enumerate() {
            write_frame(f, camera, i as u16 + 1);
        }
        let mut s = Sidecar::default();
        s.meta.flag = meta::Flag::Reject;
        s.save_in(&files[1], greycard_edit::Placement::Beside)
            .unwrap();
        greycard_edit::xmp::save(&files[1], &s.meta, None).unwrap();
        s.save_in(&files[2], greycard_edit::Placement::Folder)
            .unwrap();
        (dir, shoot, files)
    }

    /// `files` open with their sidecars read and writes on, the first
    /// frame on screen.
    fn opened_with_sidecars(app: &App, files: &[PathBuf]) -> (Rc<RefCell<State>>, Rc<Worker>) {
        let (state, worker) = state_for(app, files.to_vec());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            for (i, f) in files.iter().enumerate() {
                if let Some(s) = Sidecar::load(f).unwrap() {
                    st.sidecars[i] = s;
                }
            }
            st.current = Some(0);
            rebuild_browser(&mut st, app);
        }
        (state, worker)
    }

    /// Move back is offered for a frame in a rejects folder and not for
    /// one in the shoot, on the menu and in CULLING alike; asked, it
    /// takes the frame home with its sidecar and XMP and the flag off,
    /// and a frame whose name is taken above stays whole, said.
    #[test]
    fn move_back_takes_the_frame_home_unflagged_and_leaves_a_taken_name_whole() {
        let (dir, shoot, files) = shoot_with_rejects("menu");
        let rejects = cull::rejects_dir(&shoot);
        let app = window(files.len());
        let (state, worker) = opened_with_sidecars(&app, &files);
        // c, in the shoot: nothing to move back.
        app.invoke_frame_menu_asked(0);
        assert_eq!(app.get_back_count(), 0);
        assert_eq!(app.get_back_to(), "");
        // a, in the rejects folder: back to the shoot.
        app.invoke_select(1);
        assert_eq!(app.get_back_count(), 1);
        assert_eq!(app.get_back_to(), "shoot");
        app.invoke_frame_menu_asked(1);
        assert_eq!(app.get_back_count(), 1);
        // The set of all three: the two in the rejects folder.
        {
            let mut st = state.borrow_mut();
            st.picked = vec![0, 1, 2];
            crate::panel::browser::show_set(&mut st, &app);
        }
        assert_eq!(app.get_back_count(), 2);
        // b's name is taken in the shoot already.
        std::fs::write(shoot.join("b.tif"), b"another").unwrap();
        app.invoke_move_back_asked();

        let status = app.get_status().to_string();
        assert!(
            status.starts_with(&format!(
                "moved 1 frame and 1 sidecar back to {}",
                shoot.display()
            )),
            "{status}"
        );
        assert!(
            status.contains("1 left where it was (b.tif is in"),
            "{status}"
        );
        // a is home, with its sidecar beside and its XMP, and no longer
        // a reject.
        let home = shoot.join("a.tif");
        assert!(home.exists() && !files[1].exists());
        assert!(shoot.join("a.tif.gcd").exists() && shoot.join("a.xmp").exists());
        assert!(!rejects.join("a.tif.gcd").exists() && !rejects.join("a.xmp").exists());
        let s = Sidecar::load(&home).unwrap().unwrap();
        assert_eq!(s.meta.flag, meta::Flag::None);
        // b is whole where it was, still a reject; the file above as
        // it was.
        assert!(files[2].exists());
        let hidden = rejects
            .join(greycard_edit::SIDECAR_FOLDER)
            .join("b.tif.gcd");
        assert!(hidden.exists());
        let s = Sidecar::load(&files[2]).unwrap().unwrap();
        assert_eq!(s.meta.flag, meta::Flag::Reject);
        assert_eq!(std::fs::read(shoot.join("b.tif")).unwrap(), b"another");
        // a went out of the list, as Move rejects takes its frames out.
        let st = state.borrow();
        assert_eq!(st.files, [files[0].clone(), files[2].clone()]);
        assert_eq!(st.sidecars[1].meta.flag, meta::Flag::Reject);
        drop(st);
        // Asked again for b alone: nothing goes, and the line says so.
        app.invoke_select(1);
        app.invoke_move_back_asked();
        let status = app.get_status().to_string();
        assert!(
            status.starts_with("nothing moved back; 1 left where it was (b.tif is in"),
            "{status}"
        );
        assert!(files[2].exists());
        drop(state);
        drop(worker);
        crate::testing::remove_dir_retry(&dir);
    }

    /// In CULLING, the button is there for a frame in a rejects folder
    /// and gone for one that is not.
    #[test]
    fn the_culling_section_offers_move_back_for_a_frame_in_rejects() {
        let (dir, shoot, files) = shoot_with_rejects("section");
        let app = window(files.len());
        let (state, _worker) = opened_with_sidecars(&app, &files);
        {
            let mut st = state.borrow_mut();
            // The hidden folder is the setting here: the flag's write
            // after the move settles a sidecar where the setting says,
            // as every save does.
            st.placement = greycard_edit::Placement::Folder;
            enter_cull(&mut st, &app, 0);
        }
        assert_eq!(app.get_panel_tab(), "Cull");
        assert_eq!(crate::testing::count_labeled(&app, "Move back to shoot"), 0);
        app.invoke_select(2);
        assert_eq!(crate::testing::count_labeled(&app, "Move back to shoot"), 1);
        app.invoke_move_back_asked();
        assert!(shoot.join("b.tif").exists());
        assert!(
            shoot
                .join(greycard_edit::SIDECAR_FOLDER)
                .join("b.tif.gcd")
                .exists(),
            "the sidecar kept its place under the hidden folder"
        );
        assert_eq!(
            crate::testing::count_labeled(&app, "Move back to shoot"),
            0,
            "{}",
            app.get_status()
        );
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Move back of every frame in the rejects folder, which empties
    /// it, and Move rejects after: each move told to the index, so the
    /// rows follow with their ids, nothing is left at an old path, and
    /// nothing is missing. A pass over the emptied folder takes it for
    /// a drive that is away and sees no move, which is why it is told.
    #[test]
    fn the_rows_follow_both_moves_even_out_of_an_emptied_folder() {
        use crate::library::{Indexer, Told};
        use std::time::Duration;
        let (dir, shoot, files) = shoot_with_rejects("emptied");
        let rejects = cull::rejects_dir(&shoot);
        let db = dir.join("data").join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db.clone(), move |told| {
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        let wait = |want: &dyn Fn(&Told) -> bool| loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return told;
            }
        };
        // Both folders' passes over, by a word of either kind.
        let passed = |a: &Path, b: &Path| {
            let mut seen: Vec<PathBuf> = Vec::new();
            while !(seen.iter().any(|p| p == a) && seen.iter().any(|p| p == b)) {
                match wait(&|t| matches!(t, Told::Background { .. } | Told::Skipped { .. })) {
                    Told::Background { path, .. } | Told::Skipped { path, .. } => seen.push(path),
                    _ => {}
                }
            }
        };
        wait(&|t| matches!(t, Told::Opened(_)));
        indexer.folders(vec![shoot.clone(), rejects.clone()], 1);
        wait(&|t| matches!(t, Told::Indexed { .. }));
        let ids: Vec<i64> = {
            let reader = greycard_library::Library::open_read_only(&db).unwrap();
            files
                .iter()
                .map(|f| reader.by_path(f).unwrap().expect("indexed").id)
                .collect()
        };

        let app = window(files.len());
        let (state, worker) = opened_with_sidecars(&app, &files);
        {
            let mut st = state.borrow_mut();
            st.index = Some(indexer);
            // a has an XMP, which the index reads too: written with the
            // flag, as the editor does with XMPs on.
            st.xmp_sidecars = true;
        }
        app.invoke_select(1);
        {
            let mut st = state.borrow_mut();
            st.picked = vec![1, 2];
            crate::panel::browser::show_set(&mut st, &app);
        }
        app.invoke_move_back_asked();
        let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
        assert!(a.exists() && b.exists(), "{}", app.get_status());
        passed(&shoot, &rejects);
        {
            let reader = greycard_library::Library::open_read_only(&db).unwrap();
            for (f, (old, id)) in [&a, &b].iter().zip(files[1..].iter().zip(&ids[1..])) {
                let row = reader.by_path(f).unwrap().expect("the row at the new path");
                assert_eq!(row.id, *id, "{}", f.display());
                assert!(!row.missing);
                assert_eq!(row.meta.flag, meta::Flag::None, "the flag's write read");
                assert!(reader.by_path(old).unwrap().is_none(), "{}", old.display());
            }
        }

        // Move rejects, the other way, the same: c out.
        {
            let mut st = state.borrow_mut();
            assert_eq!(st.files[0], files[0]);
            st.sidecars[0].meta.flag = meta::Flag::Reject;
            crate::panel::edit::write_sidecar(&mut st, 0);
        }
        app.invoke_rejects_answered(true);
        let out = rejects.join("c.tif");
        assert!(out.exists(), "{}", app.get_status());
        passed(&shoot, &rejects);
        {
            let reader = greycard_library::Library::open_read_only(&db).unwrap();
            let row = reader.by_path(&out).unwrap().expect("c's row in rejects");
            assert_eq!(row.id, ids[0]);
            assert!(!row.missing);
            assert!(reader.by_path(&files[0]).unwrap().is_none());
        }
        let indexer = state.borrow_mut().index.take().unwrap();
        indexer.stop(Duration::from_secs(20));
        drop(state);
        drop(worker);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A frame in a rejects folder under a root that is offline is not
    /// offered, and asked anyway nothing moves.
    #[test]
    fn move_back_leaves_a_frame_under_an_offline_root_alone() {
        let (dir, shoot, files) = shoot_with_rejects("offline");
        let app = window(files.len());
        let (state, worker) = opened_with_sidecars(&app, &files);
        app.invoke_select(1);
        assert_eq!(app.get_back_count(), 1);
        {
            let mut st = state.borrow_mut();
            st.library.offline.insert(shoot.clone());
            assert!(!to_move_back(&st, 1));
            crate::panel::browser::show_set(&mut st, &app);
        }
        assert_eq!(app.get_back_count(), 0);
        app.invoke_move_back_asked();
        assert_eq!(app.get_status(), "no frame chosen is in a rejects folder");
        assert!(files[1].exists() && !shoot.join("a.tif").exists());
        drop(state);
        drop(worker);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A folder with no name in its path, "." or "..", is named by its
    /// canonical form, never as the dot.
    #[test]
    fn a_folder_without_a_name_in_its_path_is_named_all_the_same() {
        let here = std::env::current_dir().unwrap();
        let name = here.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(folder_name(Path::new(".")), name);
        assert_eq!(folder_name(&here), name);
        let up = here
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert_eq!(folder_name(Path::new("..")), up);
    }

    /// The rejects folder and the folder above are handed to the
    /// indexer at once, so the index has the frame at its new path
    /// without waiting for the watcher or a poll.
    #[test]
    fn move_back_hands_both_folders_to_the_indexer() {
        use crate::library::{Indexer, Told};
        use std::time::Duration;
        let (dir, shoot, files) = shoot_with_rejects("index");
        let rejects = cull::rejects_dir(&shoot);
        let db = dir.join("data").join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db.clone(), move |told| {
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        let wait = |want: &dyn Fn(&Told) -> bool| loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return told;
            }
        };
        wait(&|t| matches!(t, Told::Opened(_)));
        indexer.folders(vec![shoot.clone(), rejects.clone()], 1);
        wait(&|t| matches!(t, Told::Indexed { .. }));

        let app = window(files.len());
        let (state, worker) = opened_with_sidecars(&app, &files);
        state.borrow_mut().index = Some(indexer);
        app.invoke_select(1);
        app.invoke_move_back_asked();
        assert!(shoot.join("a.tif").exists());
        let mut passed: Vec<PathBuf> = Vec::new();
        while !(passed.contains(&shoot) && passed.contains(&rejects)) {
            if let Told::Background { path, .. } = wait(&|t| matches!(t, Told::Background { .. })) {
                passed.push(path);
            }
        }
        let reader = greycard_library::Library::open_read_only(&db).unwrap();
        let row = reader
            .by_path(&shoot.join("a.tif"))
            .unwrap()
            .expect("a's row at its new path");
        assert!(!row.missing);
        drop(reader);
        let indexer = state.borrow_mut().index.take().unwrap();
        indexer.stop(Duration::from_secs(20));
        drop(state);
        drop(worker);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The window's rejects callbacks, each recorded by name and
    /// nothing more: no state behind them, so a click that reaches one
    /// moves, deletes and opens nothing.
    fn recorded_rejects(app: &App) -> Rc<RefCell<Vec<String>>> {
        let got = Rc::new(RefCell::new(Vec::new()));
        let g = got.clone();
        app.on_rejects_asked(move || g.borrow_mut().push("move rejects".into()));
        let g = got.clone();
        app.on_move_back_asked(move || g.borrow_mut().push("move back".into()));
        let g = got.clone();
        app.on_delete_asked(move |w| g.borrow_mut().push(format!("delete {w}")));
        let g = got.clone();
        app.on_archive_rejects_asked(move |i| g.borrow_mut().push(format!("archive {i}")));
        got
    }

    /// The grid header's Rejects menu opened by a click inside its
    /// button's own words, and the item `downs` steps down it (the
    /// menu skips its separators) taken with Return. The menu's items
    /// are not clicked: the testing backend places them in the
    /// popup's own coordinates, and a click there would land on the
    /// window under it instead.
    fn rejects_menu_pick(app: &App, button: &str, item: &str, downs: usize) {
        let (at, size) = crate::testing::labeled(app, button);
        click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(crate::testing::count_labeled(app, item), 1, "{item:?}");
        for _ in 0..downs {
            press(app, Key::DownArrow);
        }
        press(app, Key::Return);
        slint::platform::update_timers_and_animations();
    }

    /// The grid's header holds CULLING's rejects actions in a menu,
    /// each the same window callback the section's button calls, so
    /// each runs the one handler and opens the one sheet.
    #[test]
    fn the_grids_rejects_menu_asks_what_the_culling_section_asks() {
        let app = window(3);
        let got = recorded_rejects(&app);
        app.set_reject_count(2);
        app.set_back_count(1);
        app.set_back_to("shoot".into());
        app.set_archive_rejects_choices(ModelRc::new(VecModel::from(vec![
            slint::SharedString::from("Remove rejects from nas..."),
        ])));
        let items = [
            ("Move 2 rejects...", "move rejects"),
            ("Move back to shoot", "move back"),
            ("Delete rejects folder...", "delete rejects"),
            ("Remove rejects from nas...", "archive 0"),
        ];
        app.set_grid_open(true);
        for (downs, (item, call)) in items.iter().enumerate() {
            rejects_menu_pick(&app, "2 rejects", item, downs + 1);
            assert_eq!(got.borrow_mut().drain(..).collect::<Vec<_>>(), [*call]);
        }
        app.set_grid_open(false);
        app.set_panel_tab(CULL_TAB.into());
        for (item, call) in items {
            let (at, size) = crate::testing::labeled(&app, item);
            click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            assert_eq!(got.borrow_mut().drain(..).collect::<Vec<_>>(), [call]);
        }
    }

    /// The Move rejects button keeps no focus once its sheet is up: the
    /// sheet takes the keys, so Return confirms once instead of asking
    /// again, Escape closes it, and the window has the keys after.
    #[test]
    fn the_move_rejects_sheet_takes_the_keys_from_the_button_that_opened_it() {
        let app = window(3);
        app.set_reject_count(2);
        app.set_panel_tab(CULL_TAB.into());
        let asked = Rc::new(RefCell::new(0));
        let answers = Rc::new(RefCell::new(Vec::new()));
        let n = asked.clone();
        let weak = app.as_weak();
        app.on_rejects_asked(move || {
            *n.borrow_mut() += 1;
            weak.unwrap().set_rejects_open(true);
        });
        let seen = answers.clone();
        let weak = app.as_weak();
        app.on_rejects_answered(move |yes| {
            seen.borrow_mut().push(yes);
            weak.unwrap().set_rejects_open(false);
        });
        let open = |app: &App| {
            let (at, size) = crate::testing::labeled(app, "Move 2 rejects...");
            click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            slint::platform::update_timers_and_animations();
            assert!(app.get_rejects_open());
        };
        open(&app);
        assert_eq!(*asked.borrow(), 1);
        press(&app, Key::Return);
        assert_eq!(*asked.borrow(), 1, "Return does not ask again");
        assert_eq!(*answers.borrow(), [true]);
        assert!(!app.get_rejects_open());
        open(&app);
        press(&app, Key::Escape);
        assert_eq!(*answers.borrow(), [true, false]);
        assert!(!app.get_rejects_open());
        // The keys are the window's again: C enters the mode.
        let toggles = Rc::new(RefCell::new(0));
        let seen = toggles.clone();
        app.on_cull_toggled(move || *seen.borrow_mut() += 1);
        press(&app, "c");
        assert_eq!(*toggles.borrow(), 1);
    }

    /// The same for the archive's sheet, opened by its section button.
    #[test]
    fn the_archive_rejects_sheet_takes_the_keys_from_its_button_too() {
        let app = window(3);
        app.set_panel_tab(CULL_TAB.into());
        app.set_archive_rejects_choices(ModelRc::new(VecModel::from(vec![
            slint::SharedString::from("Remove rejects from nas..."),
        ])));
        let asked = Rc::new(RefCell::new(0));
        let answers = Rc::new(RefCell::new(Vec::new()));
        let n = asked.clone();
        let weak = app.as_weak();
        app.on_archive_rejects_asked(move |_| {
            *n.borrow_mut() += 1;
            let app = weak.unwrap();
            app.set_archive_rejects_move("Move to rejects".into());
            app.set_archive_rejects_open(true);
        });
        let seen = answers.clone();
        let weak = app.as_weak();
        app.on_archive_rejects_answered(move |a| {
            seen.borrow_mut().push(a);
            weak.unwrap().set_archive_rejects_open(false);
        });
        let open = |app: &App| {
            let (at, size) = crate::testing::labeled(app, "Remove rejects from nas...");
            click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            slint::platform::update_timers_and_animations();
            assert!(app.get_archive_rejects_open());
        };
        open(&app);
        press(&app, Key::Return);
        assert_eq!(*asked.borrow(), 1);
        assert_eq!(*answers.borrow(), [1]);
        open(&app);
        press(&app, Key::Escape);
        assert_eq!(*answers.borrow(), [1, 0]);
    }

    /// A sheet closed from Rust, as the `--move-rejects` driver does,
    /// leaves the window its keys.
    #[test]
    fn a_sheet_closed_from_rust_gives_the_window_its_keys() {
        let app = window(3);
        app.set_reject_count(2);
        app.set_panel_tab(CULL_TAB.into());
        let weak = app.as_weak();
        app.on_rejects_asked(move || weak.unwrap().set_rejects_open(true));
        // As the real handler: the answer closes the sheet from Rust.
        let weak = app.as_weak();
        app.on_rejects_answered(move |_| weak.unwrap().set_rejects_open(false));
        let toggles = Rc::new(RefCell::new(0));
        let seen = toggles.clone();
        app.on_cull_toggled(move || *seen.borrow_mut() += 1);
        let (at, size) = crate::testing::labeled(&app, "Move 2 rejects...");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        a_moment_on(&app);
        assert!(app.get_rejects_open());
        app.invoke_rejects_answered(true);
        a_moment_on(&app);
        assert!(!app.get_rejects_open());
        press(&app, "c");
        assert_eq!(*toggles.borrow(), 1);
    }

    /// A sheet that closes in the turn another opens leaves the keys
    /// with the one that opened: the closed one's handler runs after
    /// the turn and finds a sheet up.
    #[test]
    fn a_sheet_closed_as_another_opens_leaves_it_the_keys() {
        let app = window(3);
        let answers = Rc::new(RefCell::new(Vec::new()));
        let seen = answers.clone();
        app.on_rejects_answered(move |yes| seen.borrow_mut().push(yes));
        app.set_archive_open(true);
        a_moment_on(&app);
        app.set_archive_open(false);
        app.set_rejects_open(true);
        a_moment_on(&app);
        press(&app, Key::Return);
        assert_eq!(*answers.borrow(), [true]);
    }

    /// The menu grays and leaves out what CULLING does: no rejects
    /// grays Move rejects, none of the selection in a rejects folder
    /// leaves Move back out, and Delete rejects folder is always there
    /// to ask, its handler saying when there is no folder.
    #[test]
    fn the_grids_rejects_menu_grays_and_hides_as_the_section_does() {
        let app = window(3);
        let got = recorded_rejects(&app);
        // A name with no frame to move back: the count alone decides.
        app.set_back_to("shoot".into());
        app.set_grid_open(true);
        rejects_menu_pick(&app, "Rejects", "No rejects to move", 1);
        assert!(got.borrow().is_empty(), "a grayed item asks nothing");
        // Still open, a grayed pick closing nothing: what it holds.
        assert_eq!(crate::testing::count_labeled(&app, "Move back to shoot"), 0);
        assert_eq!(
            crate::testing::count_labeled(&app, "Delete rejects folder..."),
            1
        );
        press(&app, Key::Escape);
        slint::platform::update_timers_and_animations();
        // Down past Move rejects and its separator: Delete is next,
        // with nothing between them.
        rejects_menu_pick(&app, "Rejects", "Delete rejects folder...", 2);
        assert_eq!(
            got.borrow_mut().drain(..).collect::<Vec<_>>(),
            ["delete rejects"]
        );
    }

    /// A moment past the menu's 1 ms refocus, with any sheet a pick
    /// brought in built first: what a person's next key meets.
    fn a_moment_on(app: &App) {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(5));
        slint::platform::update_timers_and_animations();
    }

    /// Delete rejects folder and Remove rejects, picked from the grid's
    /// menu, open sheets that keep the focus, so Return answers them
    /// as it does when they come from CULLING. The window's callbacks
    /// open the sheets and record the answers, and nothing else: no
    /// state behind them, nothing deleted or moved.
    #[test]
    fn return_answers_the_sheets_the_grids_rejects_menu_opens() {
        let app = window(3);
        let got = Rc::new(RefCell::new(Vec::<String>::new()));
        let (g, weak) = (got.clone(), app.as_weak());
        app.on_delete_asked(move |w| {
            g.borrow_mut().push(format!("delete {w}"));
            weak.unwrap().set_delete_open(true);
        });
        let (g, weak) = (got.clone(), app.as_weak());
        app.on_delete_answered(move |a| {
            g.borrow_mut().push(format!("delete answered {a}"));
            weak.unwrap().set_delete_open(false);
        });
        let (g, weak) = (got.clone(), app.as_weak());
        app.on_archive_rejects_asked(move |i| {
            g.borrow_mut().push(format!("archive {i}"));
            let app = weak.unwrap();
            app.set_archive_rejects_move("Move to rejects".into());
            app.set_archive_rejects_open(true);
        });
        let (g, weak) = (got.clone(), app.as_weak());
        app.on_archive_rejects_answered(move |a| {
            g.borrow_mut().push(format!("archive answered {a}"));
            weak.unwrap().set_archive_rejects_open(false);
        });
        let (g, weak) = (got.clone(), app.as_weak());
        app.on_rejects_asked(move || {
            g.borrow_mut().push("move rejects".into());
            weak.unwrap().set_rejects_open(true);
        });
        let (g, weak) = (got.clone(), app.as_weak());
        app.on_rejects_answered(move |yes| {
            g.borrow_mut().push(format!("move answered {yes}"));
            weak.unwrap().set_rejects_open(false);
        });
        app.set_delete_trash(true);
        app.set_reject_count(2);
        app.set_archive_rejects_choices(ModelRc::new(VecModel::from(vec![
            slint::SharedString::from("Remove rejects from nas..."),
        ])));
        let items = [
            ("Move 2 rejects...", 1, "move rejects", "move answered true"),
            (
                "Delete rejects folder...",
                2,
                "delete rejects",
                "delete answered 1",
            ),
            (
                "Remove rejects from nas...",
                3,
                "archive 0",
                "archive answered 1",
            ),
        ];
        app.set_grid_open(true);
        for (item, downs, asked, answered) in items {
            rejects_menu_pick(&app, "2 rejects", item, downs);
            a_moment_on(&app);
            press(&app, Key::Return);
            assert_eq!(
                got.borrow_mut().drain(..).collect::<Vec<_>>(),
                [asked, answered],
                "{item} from the grid"
            );
        }
        app.set_grid_open(false);
        app.set_panel_tab(CULL_TAB.into());
        for (item, _, asked, answered) in items {
            let (at, size) = crate::testing::labeled(&app, item);
            click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            a_moment_on(&app);
            press(&app, Key::Return);
            assert_eq!(
                got.borrow_mut().drain(..).collect::<Vec<_>>(),
                [asked, answered],
                "{item} from CULLING"
            );
        }
    }
}
