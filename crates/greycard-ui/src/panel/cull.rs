use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crate::tr;
use greycard_edit::geometry::Geometry;
use greycard_edit::{Edit, Sidecar, meta};
use slint::wgpu_30::wgpu as gpu;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::capture::{schedule_snapshot, write_screenshot};
use crate::panel::browser::{
    file_name, filter_frames, grid_filled_rows, migrate_frame, rebuild_browser, reject_count,
    row_of, show_frame_tags,
};
use crate::panel::crop::read_geometry;
use crate::panel::curve::draw_curve;
use crate::panel::edit::{read_edit, save_edit, schedule_save, show_edit};
use crate::panel::history::show_history;
use crate::panel::startup::sync_rows;
use crate::panel::viewport::{sync_display, zoom_label};
use crate::render::{self, View};
use crate::worker::Worker;
use crate::{
    App, CompareTile, FacetChip, FacetRow, FilterChip, STATE, Shared, State, cull, filter,
    placeholder,
};

// The status line's words for a picture and its zoom, in the library
// beside the placeholder's line that says them too.
pub(crate) use crate::placeholder::{picture_words, zoom_words};

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
    /// The files whose full-size copies have been asked for and have
    /// not arrived.
    pub(crate) full_asked: std::collections::HashSet<usize>,
    /// The frames whose full-size copies the log last said were in, so
    /// it says so once a set.
    pub(crate) full_said: Option<Vec<usize>>,
    /// The previews on the GPU, by file: the number of the copy each
    /// was made from, and the texture.
    pub(crate) textures: HashMap<usize, (u64, gpu::Texture)>,
    /// How many frames side by side, 1, 2 or 4 (3 from Compare versions
    /// alone), and the first row of the set.
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
    /// A line said before the status line's own once the frame is drawn
    /// again: a merge made again under the loupe, which a develop's
    /// stash does not reach in culling.
    pub(crate) said: Option<String>,
    /// A word the status line carries after its own while the compare
    /// set stands where it was put: Compare versions' count of the
    /// versions left out. Gone when the set moves or the view changes.
    pub(crate) note: Option<String>,
    /// The file Compare versions set the view on, by path, while the set
    /// is its cells: a change to its versions fits the set to them again
    /// (`versions::fit_compare`), so the view never takes in another
    /// frame. Gone with the note.
    pub(crate) versions_of: Option<PathBuf>,
    /// The stack Compare stack set the view on, by its id, while the
    /// set is its members: kept to as `versions_of` keeps the view to a
    /// file. Gone with the note.
    pub(crate) stack_of: Option<String>,
    /// The cells a versions compare shows, in list order, in place of
    /// the run of rows from the anchor: in a collection a file's cells
    /// need not stand together, and the run would take in another
    /// file's frame. By cell, not by row, so a filter that moves the
    /// rows does not move the set. Gone with the note.
    pub(crate) set: Option<Vec<usize>>,
}

impl Cull {
    /// The browser rows the view shows, in tile order: a versions
    /// compare's own cells where they are listed, else the run of
    /// `compare` rows from the anchor. `shown` is the browser's rows.
    /// The one place the compare's rows are made.
    pub(crate) fn rows(&self, shown: &[usize]) -> Vec<usize> {
        match &self.set {
            Some(set) => set
                .iter()
                .filter_map(|c| shown.binary_search(c).ok())
                .collect(),
            None => cull::compare_rows(self.anchor, self.compare, shown.len()),
        }
    }

    /// The frames held whole at 1:1, in the order they are asked for:
    /// every tile's, `chosen`'s first. A versions compare's chosen tile
    /// alone: its versions share one camera JPEG, and the other tiles
    /// keep the pictures of their edits. `shown` is the browser's rows.
    pub(crate) fn whole(&self, chosen: usize, shown: &[usize]) -> Vec<usize> {
        if self.versions_of.is_some() {
            return vec![chosen];
        }
        let cells: Vec<usize> = self.rows(shown).into_iter().map(|r| shown[r]).collect();
        cull::chosen_first(chosen, &cells)
    }

    /// The textures made from full-size copies the cache let go: a
    /// full copy of a 45 MP frame is some 180 MB on the GPU too.
    pub(crate) fn let_go(&mut self, dropped: cull::Dropped) {
        for (cell, id) in dropped {
            if self.textures.get(&cell).is_some_and(|(t, _)| *t == id) {
                self.textures.remove(&cell);
            }
        }
    }

    /// The log's word once every frame held whole at 1:1 (`whole`) is
    /// in at its own size or has no copy coming: how many are and how
    /// much memory they hold, since four can be most of a gigabyte.
    /// Once a set; one frame alone says nothing more than its decode's
    /// own line does.
    fn say_full(&mut self, whole: &[usize]) {
        let set = sorted(whole);
        if set.len() < 2
            || self.full_said.as_ref() == Some(&set)
            || set.iter().any(|f| self.full_asked.contains(f))
        {
            return;
        }
        let held: Vec<_> = set
            .iter()
            .filter(|&&f| self.cache.has_own_size(f))
            .filter_map(|&f| self.cache.best(f))
            .collect();
        let bytes: usize = held.iter().map(|p| p.bytes()).sum();
        tracing::info!(
            "culling: {} frame{} at 1:1, {} MB of full copies",
            held.len(),
            if held.len() == 1 { "" } else { "s" },
            bytes / 1_000_000
        );
        self.full_said = Some(set);
    }

    /// Another list, or the old one renumbered: what was learned of
    /// its files by their numbers goes, and what was asked of them.
    pub(crate) fn clear_failures(&mut self) {
        self.failed.clear();
        self.no_local.clear();
        self.file_failed.clear();
        self.full_asked.clear();
        self.full_said = None;
    }

    /// The view is no longer a versions compare: its note and its file
    /// go, and three up, which is Compare versions' alone, becomes four,
    /// the next step of V's cycle (the same 2×2, so nothing is fitted
    /// afresh). For every way out but V and the compare field, which set
    /// the count themselves.
    pub(crate) fn leave_versions(&mut self, app: &App) {
        self.note = None;
        self.versions_of = None;
        self.stack_of = None;
        self.set = None;
        if self.compare == 3 {
            self.compare = 4;
            app.set_compare(4);
            app.set_compare_text("4".into());
        }
    }

    /// Whether cell `c` is shown from the picture of its edit, where one
    /// is kept, rather than the camera's JPEG: a versions compare's tile,
    /// since versions of one file differ only in their edits. Everywhere
    /// else the mode shows the camera's JPEG, as its switch says. A cell
    /// that changes kind keeps its copy up until one of its new kind
    /// lands (`cull::Preview::fits`).
    pub(crate) fn shows_edit(&self, cell: usize) -> bool {
        self.versions_of.is_some() && self.set.as_ref().is_some_and(|s| s.contains(&cell))
    }

    /// Whether `file`'s own picture is still worth asking its file for.
    fn wants_file(&self, cell: usize) -> bool {
        !self.failed.contains_key(&cell) && !self.file_failed.contains(&cell)
    }

    pub(crate) fn new(tab_kept: slint::SharedString, size: u32) -> Self {
        Self {
            cache: cull::Cache::default(),
            failed: HashMap::new(),
            no_local: std::collections::HashSet::new(),
            file_failed: std::collections::HashSet::new(),
            full_asked: std::collections::HashSet::new(),
            full_said: None,
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
            said: None,
            note: None,
            versions_of: None,
            stack_of: None,
            set: None,
        }
    }
}

/// `cells` in order, to be compared as a set.
fn sorted(cells: &[usize]) -> Vec<usize> {
    let mut set = cells.to_vec();
    set.sort_unstable();
    set
}

/// Into culling mode: whatever is in hand is put down and the edit
/// on the panel written, since neither can act on the camera's
/// picture; a develop on its way is dropped; the panel shows the
/// CULLING section in place of the develop tabs'; and the frame the
/// selection is on is shown from its JPEG, the ones about it decoded
/// ahead.
pub(crate) fn enter_cull(st: &mut State, app: &App, compare: usize) {
    if st.culling.cull.is_some() {
        return;
    }
    // Out of Cull drawn developed first: its tab is the one to keep.
    leave_developed(st, app);
    put_tools_down(st, app);
    // Cull draws no develop: the Original's kept pictures go.
    crate::panel::original::forget(st);
    app.set_original_on(false);
    if st.current.is_some() {
        let outgoing = read_edit(app, &st.develop.edit, st.develop.target);
        save_edit(st, outgoing);
    }
    // The panel is not the frame's from here, and the sidecar's when
    // culling ends: the command line's overrides are done with.
    st.launch.overridden = None;
    // A save or a develop still on its timer would read a panel that
    // is no longer the frame's.
    st.develop.save_timer.stop();
    st.develop.debounce.stop();
    st.develop.held = None;
    st.develop.peek = None;
    st.develop.status_kept = None;
    st.develop.view_kept = None;
    // A frame whose develop was still on its way: that develop is
    // dropped with the generation, and the camera's picture standing
    // in for it with the mode's own. The previews the develop view
    // kept go too — the mode makes its own, at its own size.
    drop_placeholder(st, app);
    st.culling.hold = None;
    st.develop.generation += 1;
    st.develop.stood_view = None;
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
    st.culling.cull = Some(cull);
    set_loupe_cull(st, true);
    // No frame is being edited while culling.
    // Nor does the highlight fill go on: its words go with the editor.
    crate::panel::deliver::show_fill_pending(app, false);
    if let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        worker.set_open_frame(None);
        crate::panel::browser::previews_ahead(st, &worker);
    }
    // Fitted, as a file opens; at the start the command line's zoom
    // stands, so `--zoom 1` opens the loupe at 1:1.
    if st.current.is_some() {
        st.develop.zoom = 0.0;
    }
    st.develop.image_size = (0, 0);
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
    let Some(mut cull) = st.culling.cull.take() else {
        return;
    };
    // The panel shows the line's state, which may have moved in Cull:
    // As shot's switch has nothing to give back.
    crate::panel::color::forget_before_as_shot(st);
    set_loupe_cull(st, false);
    // A pick whose sidecar was on its way is left with the mode: the
    // frame stays current, and the open that would have followed
    // finds it no longer the pick.
    crate::rows::pick_over(st, app);
    st.culling.prefetch.want(Vec::new());
    crate::panel::browser::previews_ahead(st, worker);
    app.set_culling(false);
    app.set_panel_tab(cull.tab_kept.clone());
    sync_rows(&st.culling.compare_tiles, Vec::new());
    tracing::info!("culling: off");
    let Some(c) = st.current else {
        drop_placeholder(st, app);
        st.culling.hold = None;
        app.set_status("".into());
        return;
    };
    // The camera's picture is kept until the develop lands; only the
    // one frame's is needed for that.
    cull.compare = 1;
    let dropped = cull.cache.keep_full(&[c]);
    cull.let_go(dropped);
    cull.full_asked.retain(|f| *f == c);
    cull.textures.retain(|f, _| *f == c);
    // The develop view stands an edited frame in with the picture of its
    // edit: the camera's of the frames about this one, which the mode
    // asked for alone, go, and are asked for again as a stand-in when
    // chosen. This frame's stays up until its develop lands, or until
    // the edit's picture takes its place under Developed (`to_cull`).
    cull.cache
        .retain(|f, p| f == c || !p.camera_only || !crate::edited::held_edited(st, f));
    st.culling.hold = Some(cull);
    // The word for the last key goes with the mode, or coming back
    // within its second would show it over another frame.
    st.culling.notice_timer.stop();
    app.set_notice_on(false);
    // The panel takes the frame's edit here, so it is no stand-in's
    // any more, and the load is not an open of its own.
    st.views.panel_stand_in = None;
    if !crate::rows::load_frame(st, app, c) {
        st.views.panel_stand_in = Some(st.path_of(c).clone());
    }
    // A frame of the export queue's view lands under its queued edit,
    // whatever was moved meanwhile: it is not to be edited.
    let edit = match crate::panel::queue_view::open_under(st, c) {
        Some(queued) => queued,
        None => with.unwrap_or_else(|| st.line(c).map(|l| l.current().clone()).unwrap_or_default()),
    };
    st.develop.target = None;
    // The tool is put down. Left onto another frame than the one open
    // before the mode, its people are not that frame's either; left on
    // the same one, a part's question who it is of still stands, as
    // its raster does.
    if st.develop.learned_of.as_deref() == Some(st.path_of(c).as_path()) {
        crate::panel::parts::stopped(st);
    } else {
        crate::panel::parts::another_picture(st);
        crate::panel::parts_all::another_picture(st, app);
    }
    crate::panel::browser::panel_frame(st, Some(c));
    show_edit(st, &edit, app, None);
    // A control's change is a step of the frame's history, as it
    // would be out of the mode: written once the panel rests.
    if st.line(c).is_some_and(|l| edit != l.current().clone()) {
        schedule_save(st, app.as_weak());
    }
    show_history(st, app);
    st.develop.edit = edit.clone();
    // The develop on the GPU is drawn under the edit it is a develop
    // of until this frame's lands: this frame's own, under the panel,
    // or the frame's open before the mode, under that frame's edit as
    // it was saved on the way in (`enter_cull` lets go of the held
    // edit, since the mode draws no develop).
    if st.develop.base_white.is_some()
        && let Some((drawn, _)) = st.develop.shown_turn
        && drawn != c
    {
        st.develop.held = st.line(drawn).map(|l| l.current().clone());
    }
    st.develop.generation += 1;
    st.develop.stood_view = None;
    st.develop.zoom = 0.0;
    st.develop.image_size = (0, 0);
    crate::panel::viewport::settle_source_size(st);
    // The loupe's picture is already up: it stands in for the develop
    // now, by the same rule a selected frame's does, and the panel
    // keeps its shapes and its scopes off it until ours lands.
    // Only where the loupe had a picture of this frame to hold: a
    // mode left before its first decode has nothing standing in, and
    // saying it had would gate the panel off the developed picture
    // that is still on screen.
    if let Some(preview) = st.culling.hold.as_ref().and_then(|h| h.cache.best(c)) {
        let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, c, app, true);
        let plane = Geometry {
            turns,
            flip,
            ..Geometry::default()
        }
        .plane_size(preview.source.0 as f32, preview.source.1 as f32);
        let shown = (plane.0.round() as u32, plane.1.round() as u32);
        let (small, local, own) = (preview.small(), preview.origin(), preview.own_size());
        st.culling.placeholder = Some(placeholder::Wait::held(c, st.develop.generation));
        show_overlays(st, app, true);
        say_placeholder(app, shown, small, local, (st.develop.zoom, own));
    }
    app.set_status(tr!("developing...").into());
    app.set_busy(true);
    crate::panel::browser::walked_to(st, c);
    worker.send(crate::panel::browser::open_job(st, c, edit));
}

/// Whatever is in hand put down, as a mode with no tools of its own
/// is entered: a placing, a retouch, a pick, a guide, the level, the
/// crop and any drag.
fn put_tools_down(st: &mut State, app: &App) {
    st.develop.turn_pressed = None;
    crate::panel::look_preview::end(st, app);
    st.develop.placing = None;
    app.set_placing("".into());
    st.develop.retouching = None;
    app.set_retouch_mode("".into());
    // What the Fill painted waits, unseen, for its return; the sliders
    // go back to what they now edit.
    crate::panel::retouch::show_patches(st, &st.develop.edit, app);
    st.develop.picking = None;
    app.set_picking("".into());
    // A hint the tool in hand left on the status line goes with it.
    crate::panel::deliver::drop_hint(app, st);
    st.develop.guiding.clear();
    app.set_guide_mode("".into());
    app.set_level_mode(false);
    app.set_crop_mode(false);
    st.develop.crop_drag = None;
    st.develop.crop_draw = None;
    st.develop.mask_drag = None;
    st.develop.patch_drag = None;
}

/// Whether the Loupe is in Cull now, from the camera's JPEG or
/// developed: the culling keys' rules hold there (Move on, the word
/// over the picture, culling's own undo).
pub(crate) fn cull_mode(st: &State) -> bool {
    st.culling.cull.is_some() || st.culling.developed.is_some()
}

/// The Loupe's mode and Cull's switch, kept for the next run as they
/// change, as the Move on switch is. Not from a run whose start is the
/// command line's: its mode is its flags', not the user's.
fn keep_mode(st: &State) {
    if st.launch.scripted {
        return;
    }
    let (cull, developed) = (st.culling.loupe_cull, st.culling.show_developed);
    crate::panel::prefs::keep(st, |s| {
        s.loupe_cull = cull;
        s.cull_developed = developed;
    });
}

fn set_loupe_cull(st: &mut State, on: bool) {
    if st.culling.loupe_cull != on {
        st.culling.loupe_cull = on;
        keep_mode(st);
    }
}

/// The mode the last run left the Loupe in, at startup: the first frame
/// chosen opens in Cull, from the camera's JPEG or developed as its
/// switch was. Not for a run whose start is the command line's (a
/// capture, a timing run), which is its flags' alone, nor over a
/// `--cull` that asked for its own.
pub(crate) fn take_remembered_mode(st: &mut State, loupe_cull: bool, show_developed: bool) {
    if st.launch.scripted {
        return;
    }
    st.culling.loupe_cull = loupe_cull;
    st.culling.show_developed = show_developed;
    if !loupe_cull || st.launch.cull_at_start.is_some() {
        return;
    }
    if show_developed {
        st.launch.developed_at_start = true;
    } else {
        st.launch.cull_at_start = Some(1);
    }
}

/// Into Cull drawn developed, from the develop view: the frame stays
/// open and developed as it is, and every frame chosen after it opens
/// as it does in Develop, but whatever was in hand is put down and the
/// panel shows the CULLING section in place of the edit's. The keys
/// are culling's: Move on, the word over the picture, its own undo.
pub(crate) fn enter_developed(st: &mut State, app: &App) {
    if st.culling.developed.is_some() || st.culling.cull.is_some() {
        return;
    }
    let cropping = app.get_crop_mode();
    put_tools_down(st, app);
    if cropping {
        // The frame is shown cropped again, fitted.
        st.develop.zoom = 0.0;
        st.develop.image_size = (0, 0);
    }
    // A mask chosen is let go, as a tab away from Masks lets it go.
    if st.develop.target.is_some() {
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        st.develop.target = None;
        app.set_component(0);
        show_edit(st, &edit, app, None);
        st.develop.edit = edit;
    }
    st.culling.developed = Some(app.get_panel_tab());
    app.set_panel_tab(CULL_TAB.into());
    app.set_cull_developed(true);
    set_loupe_cull(st, true);
    tracing::info!("culling: on, developed");
    app.window().request_redraw();
}

/// Out of Cull drawn developed, to Develop: the panel's tab comes
/// back, and the frame on screen is the one open, already developed.
/// False when it was not on.
pub(crate) fn leave_developed(st: &mut State, app: &App) -> bool {
    let Some(tab) = st.culling.developed.take() else {
        return false;
    };
    app.set_cull_developed(false);
    app.set_panel_tab(tab);
    set_loupe_cull(st, false);
    // The word for the last key goes with the mode.
    st.culling.notice_timer.stop();
    app.set_notice_on(false);
    tracing::info!("culling: off, developed");
    true
}

/// The Loupe in Cull, from the camera's JPEG or developed as Cull's
/// switch says, from either of the other two states.
pub(crate) fn to_cull(st: &mut State, app: &App, worker: &Worker) {
    if st.culling.show_developed {
        if st.culling.cull.is_some() {
            leave_cull(st, app, worker, None);
            stand_in_edit(st);
        }
        enter_developed(st, app);
    } else if st.culling.cull.is_none() {
        enter_cull(st, app, 1);
    }
}

/// Out of Cull on the camera's JPEG into Developed: an edited frame's
/// camera picture, held up as the stand-in for its develop, gives way
/// to the picture of its edit as soon as that is read, as the develop
/// view stands an edited frame in. The camera's stays up meanwhile.
fn stand_in_edit(st: &mut State) {
    let Some(c) = st.current else {
        return;
    };
    let Some(hold) = st.culling.hold.as_ref() else {
        return;
    };
    if !hold.cache.get(c).is_some_and(|p| p.camera_only) || !crate::edited::held_edited(st, c) {
        return;
    }
    let size = hold.size;
    let path = st.path_of(c).clone();
    let whole = crop_stored(st, c);
    st.culling.prefetch.want(vec![
        cull::Want::of_file(c, path.clone(), size)
            .of_version(st.cells[c].version.clone())
            .with_picture(companion_picture(&st.views.companions, &path))
            .whole(whole),
    ]);
}

/// Whether cell `c`'s edit stores a crop of its own, as far as its
/// sidecar has been read: the camera's picture standing in for its
/// develop is then the whole sensor's, as the crop was made on, and not
/// cut to the camera's frame. A sidecar still standing in from its row
/// says no crop: the row keeps none.
pub(crate) fn crop_stored(st: &State, c: usize) -> bool {
    let read = st
        .cells
        .get(c)
        .is_some_and(|cell| st.from_row.get(cell.file).is_none_or(|r| r.read));
    read && st
        .line(c)
        .is_some_and(|line| line.current().geometry.crop.is_some())
}

/// The Loupe in Develop: out of either Cull, the frame developed and
/// the edit panel back.
pub(crate) fn to_develop(st: &mut State, app: &App, worker: &Worker) {
    if st.culling.cull.is_some() {
        leave_cull(st, app, worker, None);
    } else {
        leave_developed(st, app);
    }
}

/// Cull's switch: the developed picture or the camera's JPEG, kept,
/// and in Cull the picture swapped to it at once.
pub(crate) fn set_cull_source(st: &mut State, app: &App, worker: &Worker, developed: bool) {
    if st.culling.show_developed != developed {
        st.culling.show_developed = developed;
        keep_mode(st);
    }
    if cull_mode(st) {
        to_cull(st, app, worker);
    }
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
    let before = st.develop.edit.clone();
    let touched = read_edit(app, &before, st.develop.target);
    if touched == before {
        return None;
    }
    if !crate::rows::load_frame(st, app, c) {
        return None;
    }
    let frame = &st.line(c)?.current();
    match greycard_edit::overlay_changes(&before, &touched, frame) {
        Ok(edit) => Some(edit),
        Err(e) => {
            tracing::warn!("the control reached in culling could not be kept: {e}");
            None
        }
    }
}

/// What the library index knows of how `file` was shot, in the words
/// the edit panel uses. Culling develops nothing, so the file is not
/// opened for it; the row is read by the id the window already holds,
/// so the disk is not asked either. A frame the index has not reached,
/// or one under a root that is offline, reads empty. None when the
/// index could not be asked just now (busy, or no reader yet), which
/// is not an answer.
pub(crate) fn shot_text(st: &State, file: usize) -> Option<greycard_core::raw::ShotSummary> {
    let path = st.files.get(file)?;
    if st.views.library.offline.iter().any(|r| path.starts_with(r)) {
        return Some(Default::default());
    }
    let reader = st.views.index_reader.as_ref()?;
    let Some(id) = st.views.index_ids.get(file).copied().flatten() else {
        return Some(Default::default());
    };
    match reader.by_id(id) {
        Ok(Some(entry)) => Some(
            entry
                .exif
                .shot()
                .summary(&entry.exif.make, &entry.exif.model),
        ),
        Ok(None) => Some(Default::default()),
        Err(_) => None,
    }
}

/// The shot lines for the frame under the loupe, asked of the index
/// again: it may have reached the frame since the focus came to it.
/// Nothing outside culling, where the develop writes them; and the
/// lines already up stay when the index could not be asked.
pub(crate) fn show_shot(st: &State, app: &App) {
    let (Some(_), Some(file)) = (&st.culling.cull, st.current) else {
        return;
    };
    if let Some(shot) = shot_text(st, file) {
        app.set_shot_camera(shot.camera.as_str().into());
        app.set_shot_exposure(shot.exposure.as_str().into());
    }
}

/// The selection moves to `file` in culling mode: no develop, the
/// panel left as it is, the frame's JPEG shown from the cache or
/// asked for, and the window of decodes about it renewed.
pub(crate) fn cull_select(st: &mut State, app: &App, cell: usize) {
    let Some(row) = row_of(st, cell) else {
        return;
    };
    let was = st.current;
    // Culling reaching a frame reads its whole sidecar, if it was
    // standing in from its row: the history panel and the keys want
    // it, and leaving the mode develops it. One under a root that is
    // offline is stepped onto all the same, and the status says why
    // there is no picture.
    crate::rows::load_frame(st, app, cell);
    // As the loupe takes it: leaving culling develops this frame,
    // and a part-migrated Original crop would be measured first.
    migrate_frame(st, cell);
    st.current = Some(cell);
    crate::panel::browser::current_moved(st);
    crate::panel::viewport::settle_source_size(st);
    app.set_selected(row as i32);
    crate::panel::browser::show_file_name(st, app, Some(st.path_of(cell)));
    let shot = shot_text(st, st.file_of(cell)).unwrap_or_default();
    app.set_shot_camera(shot.camera.as_str().into());
    app.set_shot_exposure(shot.exposure.as_str().into());
    app.set_shot_size("".into());
    crate::panel::time_shift::show_shot_time(st, app);
    show_history(st, app);
    show_frame_tags(st, app);
    let count = st.views.shown.len();
    if let Some(cull) = st.culling.cull.as_mut() {
        cull.forward = was.is_none_or(|w| cell >= w);
        // A versions compare holds while the focus is one of its cells,
        // wherever they stand in the list; off them it is no longer the
        // file's compare, and the view is placed again as V's views are,
        // at the count that leaves.
        let in_set = cull.set.as_ref().map(|set| set.contains(&cell));
        if in_set == Some(false) {
            cull.leave_versions(app);
        }
        if in_set != Some(true) {
            cull.anchor = cull::anchor_for(cull.anchor, row, cull.compare, count);
        }
        cull.switched = Some(std::time::Instant::now());
        // A full-size copy is its frame's own; a frame no longer shown
        // lets its copy go and its ask lapse, and each one shown asks
        // for its own if 1:1 is still on.
        let shown = cull.whole(cell, &st.views.shown);
        let dropped = cull.cache.keep_full(&shown);
        cull.let_go(dropped);
        cull.full_asked.retain(|f| shown.contains(f));
        if cull
            .full_said
            .as_ref()
            .is_some_and(|s| *s != sorted(&shown))
        {
            cull.full_said = None;
        }
        // A decode that failed is tried again when the frame comes
        // round: a cell being written, a moment's shortage.
        cull.failed.remove(&cell);
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
    let count = st.views.shown.len();
    let Some(cull) = st.culling.cull.as_mut() else {
        return;
    };
    let rows = cull.rows(&st.views.shown);
    let whole = cull.whole(c, &st.views.shown);
    let mut order: Vec<usize> = rows.clone();
    for r in cull::order(row, count, cull.reach, cull.forward) {
        if !order.contains(&r) {
            order.push(r);
        }
    }
    let kept = cull::kept(row, count, cull.reach);
    let window: Vec<usize> = kept
        .clone()
        .chain(rows)
        .map(|r| st.views.shown[r])
        .collect();
    let dropped = cull.cache.keep(&window, &whole);
    cull.let_go(dropped);
    cull.textures.retain(|f, _| window.contains(f));
    tracing::debug!(
        "cull window: rows {}..{} about {row}, {} previews held, {} MB",
        kept.start,
        kept.end,
        cull.cache.count(),
        cull.cache.bytes() / 1_000_000
    );
    let size = cull.size;
    let (files, from_row, cells, library) = (&st.files, &st.from_row, &st.cells, &st.views.library);
    let plan = |c: usize| loupe_plan_of(files, from_row, cells, library, c);
    let path_of = |c: usize| files[cells[c].file].clone();
    let picture_of = |c: usize| companion_picture(&st.views.companions, &files[cells[c].file]);
    let version_of = |c: usize| cells[c].version.clone();
    let frames: Vec<usize> = order.iter().map(|&r| st.views.shown[r]).collect();
    let mut wants = Vec::new();
    // At 1:1, the full JPEG of each frame shown before anything, the
    // chosen one's first, where its file can be read.
    if st.develop.zoom > 0.0 {
        for &f in &whole {
            if !cull.cache.has_own_size(f) && cull.wants_file(f) && plan(f).file {
                wants.push(
                    cull::Want::for_loupe(f, path_of(f), 0)
                        .of_version(version_of(f))
                        .with_picture(picture_of(f)),
                );
            }
        }
    }
    // The local previews of the frames with no picture yet, all of
    // them ahead of any file: under an offline root they are all there
    // is, and on a network mount they are up in milliseconds while a
    // file's read may take seconds, or hang. A frame out of reach with
    // no key to find a preview by is said so, and not asked for. A copy
    // of the other kind than the frame wants (`Cull::shows_edit`) stays
    // up and is asked for again of the right one.
    for &f in &frames {
        let camera = !cull.shows_edit(f);
        let held = cull.cache.get(f);
        if held.is_some_and(|p| p.fits(camera))
            || cull.failed.contains_key(&f)
            || cull.no_local.contains(&f)
        {
            continue;
        }
        match plan(f) {
            cull::Plan {
                preview: Some(key), ..
            } => wants.push(
                cull::Want::local(f, path_of(f), size, key)
                    .of_version(version_of(f))
                    .camera_only(camera),
            ),
            cull::Plan {
                preview: None,
                file: false,
            } if held.is_none() => {
                cull.failed.insert(f, cull::NO_LOCAL_PREVIEW.to_string());
            }
            _ => {}
        }
    }
    // Then the files, for the frames whose view-size copy is not the
    // camera's yet, or not of the kind wanted, and whose file has not
    // already failed them.
    for &f in &frames {
        let camera = !cull.shows_edit(f);
        let had = cull
            .cache
            .get(f)
            .is_some_and(|p| !p.local && p.fits(camera));
        if had || !cull.wants_file(f) || !plan(f).file {
            continue;
        }
        wants.push(
            cull::Want::for_loupe(f, path_of(f), size)
                .of_version(version_of(f))
                .with_picture(picture_of(f))
                .camera_only(camera),
        );
    }
    // The full asks of this list are the ones outstanding, and one it
    // drops from the queue (at a fit, or a set moved on) is asked again
    // at the next 1:1; one a thread is on is not begun twice.
    cull.full_asked = wants
        .iter()
        .filter(|w| w.size == 0)
        .map(|w| w.file)
        .collect();
    st.culling.prefetch.want(wants);
}

/// The camera's JPEG beside the raw at `path`, which the loupe and the
/// stand-in show in place of the raw's own preview; none for a frame
/// with none.
pub(crate) fn companion_picture(
    companions: &HashMap<PathBuf, Vec<PathBuf>>,
    path: &Path,
) -> Option<PathBuf> {
    companions.get(path)?.first().cloned()
}

/// Where the loupe takes cell `c`'s picture from, by its root: see
/// [`cull::source_for`]. The key is the row's while the frame stands
/// in from it; a frame read from disk has none.
pub(crate) fn loupe_plan(st: &State, c: usize) -> cull::Plan {
    loupe_plan_of(&st.files, &st.from_row, &st.cells, &st.views.library, c)
}

/// [`loupe_plan`] with the state's fields borrowed apart.
pub(crate) fn loupe_plan_of(
    files: &[PathBuf],
    from_row: &[crate::rows::FromRow],
    cells: &[crate::versions::Cell],
    library: &crate::roots::Library,
    c: usize,
) -> cull::Plan {
    let Some(file) = cells.get(c).map(|cell| cell.file) else {
        return cull::source_for(true, false, None);
    };
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
pub(crate) fn start_placeholder(st: &mut State, app: &App, cell: usize) {
    let count = st.views.shown.len();
    let row = row_of(st, cell);
    // At the view's long edge, as the mode's are, and no smaller than
    // a strip would be laughable at.
    let size = (app.get_view_width().max(app.get_view_height()).max(1) as u32).max(1024);
    // What is kept is trimmed on every selection, whether or not this
    // frame gets a placeholder of its own: a run through a folder of
    // pictures would otherwise hold on to whatever a run through the
    // raws before it left.
    if let Some(hold) = st.culling.hold.as_mut() {
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
                .map(|r| st.views.shown[r])
                .collect(),
            // Hidden by the filter, which is not a row to keep a
            // window about; its own picture is all that is wanted.
            None => vec![cell],
        };
        let dropped = hold.cache.keep(&window, &[cell]);
        hold.let_go(dropped);
        hold.textures.retain(|f, _| window.contains(f));
    }
    if row.is_none() || !placeholder::worth_it(st.path_of(cell)) {
        // Nothing to stand in with, and nothing worth standing in
        // for: what is on screen stays until the develop lands.
        drop_placeholder(st, app);
        return;
    }
    // An edited frame stands in with its edit's picture: a camera
    // picture Cull asked for alone is not that, and is asked again.
    let edited = crate::edited::held_edited(st, cell);
    // A frame with a crop of its own stands in with the camera's whole
    // picture: one cut to the camera's frame is asked again.
    let whole = crop_stored(st, cell);
    // The hold is the mode's own state without the mode: its tab is
    // never put back from here, since nothing took it away.
    let hold = st
        .culling
        .hold
        .get_or_insert_with(|| Cull::new(slint::SharedString::new(), size));
    let held = hold
        .cache
        .get(cell)
        .is_some_and(|p| (!p.camera_only || !edited) && !(whole && p.cut));
    // A frame whose decode came back with nothing is not asked for
    // again: there is no camera JPEG in that cell, and every select
    // would pay a decode to be told so. The record goes when the
    // folder does.
    let none_in_it = hold.failed.contains_key(&cell);
    let path = st.path_of(cell).clone();
    if held {
        // Already decoded: up on the next frame, no thread needed, and
        // the wait is up as a decode landing would have put it, so
        // this frame's picture is what stands in. A wait left asked
        // here would stand nothing in, and the last frame's develop,
        // or the camera picture the last wait was showing, would stay
        // on screen under this frame's line for the whole develop.
        // The edit's picture may wait for its frame's size instead.
        if waits_for_size(st, app, cell) {
            let showing = st.culling.placeholder.as_ref().and_then(|w| w.showing);
            st.culling.placeholder = Some(placeholder::Wait::asked(
                cell,
                st.develop.generation,
                showing,
            ));
            show_overlays(st, app, standing_in(st).is_some());
            return;
        }
        st.culling.placeholder = Some(placeholder::Wait::in_hand(cell, st.develop.generation));
        placeholder_arrived(st, app, cell);
        return;
    }
    // Whatever camera picture is on screen stays there until this
    // frame's own is in hand.
    let showing = st.culling.placeholder.as_ref().and_then(|w| w.showing);
    st.culling.placeholder = Some(placeholder::Wait::asked(
        cell,
        st.develop.generation,
        showing,
    ));
    if none_in_it {
        drop_placeholder(st, app);
        return;
    }
    st.culling.prefetch.want(vec![
        // The frame's own want, version and all: the delivery is
        // checked against the cell's key, and a version's picture
        // asked as its file's would be dropped there.
        cull::Want::of_file(cell, path.clone(), size)
            .of_version(st.cells[cell].version.clone())
            .with_picture(companion_picture(&st.views.companions, &path))
            .whole(whole),
    ]);
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
        .culling
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
    let file = st.culling.placeholder.as_ref()?.showing?;
    st.culling.hold.as_ref()?.cache.best(file)?;
    Some(file)
}

/// The zoom chosen on the open frame's own picture standing in, for
/// the develop of `generation` that replaces it to take on
/// (`viewport::frame_on_show`): a frame the user zoomed into while it
/// developed stays zoomed on the same place. None at a fit, which the
/// develop lands at anyway, and while another frame's picture stands
/// in, whose zoom is not this frame's.
pub(crate) fn stood_view(st: &State, generation: u64) -> Option<crate::owners::StoodView> {
    use crate::owners::{Stood, StoodView};
    let file = standing_in(st)?;
    let replaced = st
        .culling
        .placeholder
        .as_ref()
        .is_some_and(|w| w.replaced_by(generation));
    let (iw, ih) = st.develop.image_size;
    if st.culling.cull.is_some()
        || !replaced
        || Some(file) != st.current
        || st.develop.zoom <= 0.0
        || iw == 0
        || ih == 0
    {
        return None;
    }
    let (cx, cy) = st.develop.center;
    // The command line's zoom about the middle stands into the
    // develop as it is, centered (`placeholder_arrived`).
    let middle = (cx - iw as f32 / 2.0).abs() < 0.5 && (cy - ih as f32 / 2.0).abs() < 0.5;
    if st.launch.zoom_stands && middle {
        return None;
    }
    let preview = st.culling.hold.as_ref()?.cache.best(file)?;
    Some(StoodView {
        generation,
        zoom: st.develop.zoom,
        named: !st.develop.zoom_free,
        at: (cx / iw as f32, cy / ih as f32),
        width: iw as f32,
        shows: if preview.edited {
            Stood::Frame
        } else {
            Stood::Camera {
                cut: preview.cut || preview.companion,
            }
        },
    })
}

/// The camera's picture of the frame being waited on is in hand: it
/// goes up on the next frame, the status line says what it is and
/// what it is not, and the panel takes its shapes, its navigator and
/// its scopes off a picture they do not belong to.
///
/// Fitted, whatever the view was at: a file chosen opens fitted, and
/// a magnified screen-size copy in between would be a blur that jumps
/// twice. The one exception is the session's first open
/// (`zoom_stands`), where the zoom is the command line's and stands
/// into the develop, so `--zoom 1` opens at 1:1.
fn placeholder_arrived(st: &mut State, app: &App, cell: usize) {
    let Some(preview) = st.culling.hold.as_ref().and_then(|h| h.cache.best(cell)) else {
        return;
    };
    let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, cell, app, true);
    let plane = Geometry {
        turns,
        flip,
        ..Geometry::default()
    }
    .plane_size(preview.source.0 as f32, preview.source.1 as f32);
    let (small, local, own) = (preview.small(), preview.origin(), preview.own_size());
    // The edit's picture says the frame's size, as `cull_frame` will.
    let shown = match stands_for(st, app, cell) {
        Some(s) => Geometry {
            turns,
            flip,
            ..Geometry::default()
        }
        .plane_size(s.0 as f32, s.1 as f32),
        None => plane,
    };
    let shown = (shown.0.round() as u32, shown.1.round() as u32);
    if !st.launch.zoom_stands {
        st.develop.zoom = 0.0;
    }
    st.develop.image_size = (0, 0);
    say_placeholder(app, shown, small, local, (st.develop.zoom, own));
    show_overlays(st, app, true);
    app.window().request_redraw();
}

/// The size the edit's picture of `cell` stands for, in that
/// picture's own orientation: the frame as its develop lands, cut by
/// its edit, once the worker's open has said the frame's size. The
/// copy is kept at a 2048 px long edge with the crop in it; drawn as
/// the frame, it is fitted as the develop will be and 1:1 is the
/// frame's, so the develop lands where it stood. None for any other
/// picture, which is its own size, and in Cull, where the copy is the
/// picture.
fn stands_for(st: &State, app: &App, cell: usize) -> Option<(u32, u32)> {
    edit_copy(st, cell)?;
    let (w, h) = opened_size(st, cell)??;
    let sidecar = st.sidecar(cell);
    let panel = st.current == Some(cell);
    let geometry = if panel {
        read_geometry(app)
    } else {
        let version = st.cells[cell].version.as_deref();
        let line = sidecar.line(version).unwrap_or(&sidecar.line);
        line.current().geometry.clone()
    };
    let (w, h) = if sidecar.turn % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    // The develop's frame by the camera's where the edit has no crop:
    // the frame as the index's row or the thumbnails told it.
    let camera = crate::edited_pictures::camera_known(st.path_of(cell)).map(|c| {
        let at = c.orientation.turned(i32::from(sidecar.turn % 4));
        greycard_edit::geometry::Crop::of(c.oriented(at))
    });
    let frame = geometry.frame(w as f32, h as f32, camera).size;
    let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, cell, app, panel);
    let (fw, fh) = Geometry {
        turns,
        flip,
        ..Geometry::default()
    }
    .plane_size(frame.0, frame.1);
    Some((fw as u32, fh as u32))
}

/// The size the camera's picture of `cell` stands for while it stands
/// in for a develop, in that picture's own orientation: its own size
/// grown by the one scale that makes it the develop's footprint, once
/// the worker's open has said the frame's size. A camera JPEG may be
/// far smaller than the raw (our merges write one of 1024 px, some
/// cameras 1620), and a fit never enlarges, so drawn at its own size it
/// sat small in the view and grew when the develop landed; drawn as the
/// develop's size it fits as the develop will, and 1:1 is the
/// develop's. A picture of the develop's shape, either way up, takes
/// its size, cut as the picture is; one of another shape is the largest
/// of its shape inside the develop, scaled by `min(dw / sw, dh / sh)`,
/// so a full-size one stays its own size. None for the edit's picture
/// (`stands_for`), in Cull, where the camera's picture is the picture,
/// and before the open has said.
fn camera_stands_for(st: &State, cell: usize) -> Option<(u32, u32)> {
    if st.culling.cull.is_some() {
        return None;
    }
    // The picture the loupe draws, as `draw_cull` takes it.
    let loupe = st.culling.cull.as_ref().or(st.culling.hold.as_ref())?;
    let preview = loupe.cache.best(cell)?;
    if preview.edited {
        return None;
    }
    let (w, h) = opened_size(st, cell)??;
    let (sw, sh) = preview.source;
    if w == 0 || h == 0 || sw == 0 || sh == 0 {
        return None;
    }
    let (fw, fh) = if preview.cut || preview.companion {
        crate::edited_pictures::camera_known(st.path_of(cell)).map_or((1.0, 1.0), |c| {
            let [_, _, fw, fh] = c.shown();
            (f64::from(fw), f64::from(fh))
        })
    } else {
        (1.0, 1.0)
    };
    // The develop, cut as the picture is, lying the way nearer the
    // picture's shape. The cut's fractions lie as the develop does, so
    // they turn with it.
    let seen = f64::from(sw) / f64::from(sh);
    let off = |(dw, dh): (f64, f64)| (dw / dh / seen).ln().abs();
    let (dw, dh) = [(w, h, fw, fh), (h, w, fh, fw)]
        .map(|(dw, dh, fw, fh)| (f64::from(dw) * fw, f64::from(dh) * fh))
        .into_iter()
        .min_by(|a, b| off(*a).total_cmp(&off(*b)))?;
    // Of its shape: the develop's own size, exactly its footprint.
    if (dw / dh - seen).abs() <= 0.01 * seen {
        return Some((dw.round().max(1.0) as u32, dh.round().max(1.0) as u32));
    }
    // Of another shape (a camera set to 1:1, 4:3, or 16:9 on a 3:2
    // sensor), the picture is the largest of its shape inside the
    // develop: scaled until it meets the develop's nearer side, so a
    // full-size one stays its own size.
    let k = (dw / f64::from(sw)).min(dh / f64::from(sh));
    Some((
        (f64::from(sw) * k).round().max(1.0) as u32,
        (f64::from(sh) * k).round().max(1.0) as u32,
    ))
}

/// The size the picture of `cell` standing in for its develop is
/// drawn as: the frame's for the edit's picture, the develop's for
/// the camera's, once the open has said it; None while it is drawn as
/// its own pixels.
fn drawn_as(st: &State, app: &App, cell: usize) -> Option<(u32, u32)> {
    stands_for(st, app, cell).or_else(|| camera_stands_for(st, cell))
}

/// The edit's picture of `cell` standing in, which is drawn as the
/// frame it is a copy of: never in Cull, where the copy is the
/// picture, nor for a frame standing in from its row, which has no
/// edit read to cut the frame by.
fn edit_copy(st: &State, cell: usize) -> Option<&Arc<cull::Preview>> {
    let preview = st.culling.hold.as_ref()?.cache.best(cell)?;
    let from_row = st
        .from_row
        .get(st.cells[cell].file)
        .is_some_and(|f| !f.read);
    (st.culling.cull.is_none() && preview.origin() == cull::Origin::Edit && !from_row)
        .then_some(preview)
}

/// What the worker's open of `cell`'s file said of its size, once it
/// has: `Some(None)` for a picture that is not a raw.
fn opened_size(st: &State, cell: usize) -> Option<Option<(u32, u32)>> {
    let path = st.path_of(cell);
    let opened = &st.develop.opened_sizes;
    opened
        .iter()
        .rev()
        .find(|(p, _)| p == path)
        .map(|(_, s)| *s)
}

/// The edit's picture of the selected `cell` waits for the open to say
/// the frame's size before it goes up, when the view has room for more
/// than the copy, or at a zoom that stands into it: up at once it
/// would stand at its own 2048 px and grow when the size came. The
/// picture on screen stays meanwhile, as it does for a decode.
fn waits_for_size(st: &State, app: &App, cell: usize) -> bool {
    if opened_size(st, cell).is_some() {
        return false;
    }
    let (w, h) = if let Some(preview) = edit_copy(st, cell) {
        // A zoom other than the fit is let go as the picture goes up,
        // unless it stands (`placeholder_arrived`).
        if st.launch.zoom_stands && st.develop.zoom > 0.0 {
            return true;
        }
        (preview.width, preview.height)
    } else {
        // The camera's picture read from a raw waits as the edit's
        // does (`camera_stands_for`); a local preview, made because
        // the file is out of reach or slow, goes up at once.
        let Some(preview) = st.culling.hold.as_ref().and_then(|h| h.cache.best(cell)) else {
            return false;
        };
        let from_file = matches!(
            preview.origin(),
            cull::Origin::Camera | cull::Origin::Companion
        );
        if st.culling.cull.is_some()
            || !from_file
            || !greycard_core::decode::is_raw_path(st.path_of(cell))
        {
            return false;
        }
        preview.source
    };
    let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, cell, app, true);
    let (cw, ch) = Geometry {
        turns,
        flip,
        ..Geometry::default()
    }
    .plane_size(w as f32, h as f32);
    let view = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let room = if st.develop.mat {
        crate::zoom::matted(view)
    } else {
        view
    };
    (room.0 as f32 / cw).min(room.1 as f32 / ch) > 1.0
}

/// The worker's open has said the selected frame's size: the picture
/// waiting on it goes up, or the one already up is drawn as the size
/// from now on, with a zoom on it kept where it is on screen.
pub(crate) fn size_opened(st: &mut State, app: &App) {
    let Some(cell) = st.current else {
        return;
    };
    if standing_in(st) == Some(cell) {
        resize_standing(st, app, cell);
        return;
    }
    let waiting = edit_copy(st, cell).is_some() || camera_stands_for(st, cell).is_some();
    if waiting
        && st
            .culling
            .placeholder
            .as_mut()
            .is_some_and(|w| w.arrived(cell))
    {
        placeholder_arrived(st, app, cell);
    }
}

/// The picture of `cell` on screen as it stands in, as the view takes
/// it (`draw_cull`'s): the size it is drawn as, in its own orientation
/// (its own, or the frame's or the develop's for a stand-in,
/// `drawn_as`), and that size turned as it is drawn. `panel`: the
/// frame's turns are the panel's, as for the selected frame outside
/// Cull.
fn standing_plane(
    st: &State,
    app: &App,
    cell: usize,
    panel: bool,
) -> Option<((u32, u32), (f32, f32))> {
    let cull = st.culling.cull.as_ref().or(st.culling.hold.as_ref())?;
    let preview = cull.cache.best(cell)?;
    let source = drawn_as(st, app, cell).unwrap_or(preview.source);
    let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, cell, app, panel);
    let full = Geometry {
        turns,
        flip,
        ..Geometry::default()
    }
    .plane_size(source.0 as f32, source.1 as f32);
    Some((source, full))
}

/// The loupe's current frame `c` laid out for the view: its picture's
/// size as drawn (`standing_plane`) is the view's image size, and a
/// new frame, or a fit view, is centered; a zoomed view of the same
/// frame keeps its place, and across the frame's own turn or mirror
/// keeps the same point of the picture in the middle, turned about it
/// as the develop view's is (`viewport::carried_center`). The size
/// drawn and turned, for its tile (`tile_view`) and for the other
/// tiles to follow.
fn lay_out_current(
    st: &mut State,
    app: &App,
    c: usize,
    panel: bool,
) -> Option<((u32, u32), (f32, f32))> {
    let (source, full) = standing_plane(st, app, c, panel)?;
    let size = (full.0.round() as u32, full.1.round() as u32);
    // The picture drawn is the same source whichever way the frame
    // stands; the turns and the mirror are the geometry's.
    let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, c, app, panel);
    let geometry = Geometry {
        turns,
        flip,
        ..Geometry::default()
    };
    let (sw, sh) = (source.0 as f32, source.1 as f32);
    let cell = &st.cells[c];
    let laid = crate::owners::Laid {
        of: Some((st.files[cell.file].clone(), cell.version.clone())),
        frame: geometry.frame(sw, sh, None),
        geometry,
        source: (sw, sh),
        turn: 0,
    };
    let before = st.culling.laid.replace(laid.clone());
    // Only a zoom on the same picture standing another way: a fit, a
    // view re-centered (a size of none), and a frame drawn again as it
    // was are laid out as they always were.
    let carried = before
        .filter(|b| st.develop.zoom > 0.0 && st.develop.image_size != (0, 0) && *b != laid)
        .and_then(|b| crate::panel::viewport::carried_center(&b, &laid, st.develop.center));
    if let Some(center) = carried {
        st.develop.image_size = size;
        st.develop.center = center;
    } else if size != st.develop.image_size {
        st.develop.image_size = size;
        st.develop.center = (full.0 / 2.0, full.1 / 2.0);
    }
    Some((source, full))
}

/// A loupe tile's view of the GPU's `copy` of a picture drawn as
/// `drawn` (its own size, or the frame's or the develop's for a
/// stand-in) under `geometry`'s quarter turns and mirror, in `room`:
/// at `zoom` display pixels per drawn pixel, or for 0 the fit, which
/// never enlarges, about `center` in drawn pixels. The zoom and the
/// center are in the drawn picture's pixels, so 1:1 means 1:1 whichever
/// copy is on the GPU; the shader works in the copy's, so the zoom
/// grows by the copy's scale and the center shrinks by it. With the
/// drawn size turned.
fn tile_view(
    copy: (u32, u32),
    drawn: (u32, u32),
    geometry: &Geometry,
    room: (u32, u32),
    zoom: f32,
    center: (f32, f32),
    canvas: [f32; 3],
) -> (View, (f32, f32)) {
    let (tw, th) = (copy.0 as f32, copy.1 as f32);
    let (fw, fh) = (drawn.0 as f32, drawn.1 as f32);
    let scale = tw / fw;
    let full = geometry.plane_size(fw, fh);
    // The camera's picture comes cut to the camera's frame
    // (`thumbs::cut_to_camera`), so it is drawn whole.
    let frame = geometry.frame(tw, th, None);
    let zoom = if zoom > 0.0 {
        zoom
    } else {
        (room.0 as f32 / full.0)
            .min(room.1 as f32 / full.1)
            .min(1.0)
    };
    let view = View {
        zoom: zoom / scale,
        // A drawn pixel is `scale` of the copy's, so past 1:1 the
        // blocks are the drawn picture's pixels whatever copy stands.
        pixel: scale,
        center: (center.0 * scale, center.1 * scale),
        matrix: geometry.matrix(),
        plane: geometry.plane_size(tw, th),
        frame_origin: frame.origin,
        frame_size: frame.size,
        canvas,
        ..View::blank()
    };
    (view, full)
}

/// The loupe's tiles planned on the copies they are drawn from: at a
/// fit the full-size copies, which a frame needs only while it is
/// looked at 1:1 (`ask_whole`), and their textures are let go first,
/// so each tile is planned on the screen-size copy left in their place.
fn plan_tiles(
    st: &mut State,
    app: &App,
    c: usize,
    current: Option<((u32, u32), (f32, f32))>,
    rows: &[usize],
    rects: &[(u32, u32, u32, u32)],
    canvas: [f32; 3],
) -> Vec<Option<TileView>> {
    if st.develop.zoom <= 0.0
        && let Some(cull) = st.culling.cull.as_mut().or(st.culling.hold.as_mut())
    {
        let dropped = cull.cache.drop_full();
        cull.full_said = None;
        cull.let_go(dropped);
    }
    tile_views(st, app, c, current, rows, rects, canvas)
}

/// A tile's view, and the size its line names (`tile_views`).
type TileView = (View, (u32, u32));

/// The views of the loupe's tiles, one a row of `rows` in `rects`, for
/// the pictures in hand (None for a tile with none): each fitted or at
/// the zoom, turned as its frame is, the current frame `c` drawn as its
/// layout (`current`, `lay_out_current`) took it, and the others
/// following its center as a fraction of the frame, so a 1:1 of four
/// shows the same corner of each. With each drawn size turned, and the
/// tile's turns.
fn tile_views(
    st: &State,
    app: &App,
    c: usize,
    current: Option<((u32, u32), (f32, f32))>,
    rows: &[usize],
    rects: &[(u32, u32, u32, u32)],
    canvas: [f32; 3],
) -> Vec<Option<TileView>> {
    let holding = st.culling.cull.is_none();
    // The camera's picture is named by its own size, though it is drawn
    // as the develop's; the edit's picture by the frame's.
    let edit_stands = stands_for(st, app, c).is_some();
    let Some(cull) = st.culling.cull.as_ref().or(st.culling.hold.as_ref()) else {
        return vec![None; rows.len()];
    };
    rows.iter()
        .zip(rects)
        .map(|(&row, rect)| {
            let file = st.views.shown[row];
            let preview = cull.cache.best(file)?;
            // The panel holds the selected frame's edit and nothing
            // else's, so only that frame's turns are read off it.
            let panel = holding && Some(file) == st.current;
            let (turns, flip) =
                thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, file, app, panel);
            let geometry = Geometry {
                turns,
                flip,
                ..Geometry::default()
            };
            // The copy's own size, which the view is in whatever the
            // GPU holds of it (`Renderer::encoded_texture` reduces one
            // past the device's textures, and the tile says the size it
            // stands for).
            let drawn = match current {
                Some((source, _)) if file == c => source,
                _ => preview.source,
            };
            let full = geometry.plane_size(drawn.0 as f32, drawn.1 as f32);
            // One frame alone fits inside the mat, as the develop's does
            // (`effective_zoom`): the camera's picture standing in for
            // it must not jump when the develop lands.
            let room = if st.develop.mat && rows.len() <= 1 {
                crate::zoom::matted((rect.2, rect.3))
            } else {
                (rect.2, rect.3)
            };
            let center = match current {
                Some((_, cur)) if file != c => (
                    st.develop.center.0 / cur.0.max(1.0) * full.0,
                    st.develop.center.1 / cur.1.max(1.0) * full.1,
                ),
                _ => st.develop.center,
            };
            let (view, full) = tile_view(
                (preview.width, preview.height),
                drawn,
                &geometry,
                room,
                st.develop.zoom,
                center,
                canvas,
            );
            // The size as shown, so the line follows the frame's turn
            // as the picture does.
            let named = if file == c && edit_stands {
                full
            } else {
                geometry.plane_size(preview.source.0 as f32, preview.source.1 as f32)
            };
            Some((view, (named.0.round() as u32, named.1.round() as u32)))
        })
        .collect()
}

/// The picture standing in for `cell` was up in its own pixels when
/// the open said the frame's size: a zoom on it is taken into the new
/// units, so the next draw neither re-centers it nor moves it. A named
/// level stays that level, now of the develop's pixels, as it lands
/// anyway (§697); a free zoom keeps its scale on screen.
fn resize_standing(st: &mut State, app: &App, cell: usize) {
    let Some((_, full)) = standing_plane(st, app, cell, true) else {
        return;
    };
    let size = (full.0.round() as u32, full.1.round() as u32);
    let (iw, ih) = st.develop.image_size;
    if size == (iw, ih) || iw == 0 || ih == 0 {
        return;
    }
    let (rx, ry) = (full.0 / iw as f32, full.1 / ih as f32);
    let (cx, cy) = st.develop.center;
    st.develop.center = (cx * rx, cy * ry);
    if st.develop.zoom > 0.0 && st.develop.zoom_free {
        st.develop.zoom /= rx;
    }
    st.develop.image_size = size;
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
pub(crate) fn say_placeholder(
    app: &App,
    shown: (u32, u32),
    small: bool,
    local: cull::Origin,
    at: (f32, bool),
) {
    let line = placeholder::status(shown, small, local, at);
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
        st.develop.bins = None;
        app.set_scope_picture(slint::Image::default());
        app.set_curve_image(draw_curve(app, None));
        app.set_clip_shadows_lit(false);
        app.set_clip_highlights_lit(false);
        // And the color under the pointer, which `show_hover` leaves
        // off while the camera's picture stands in.
        st.develop.hover_shown = None;
        app.set_scope_readout("".into());
        app.set_scope_marks(ModelRc::default());
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
    let was = st.culling.placeholder.take().is_some();
    if was {
        show_overlays(st, app, false);
        app.set_placeholder_status("".into());
        if let Some(hold) = st.culling.hold.as_mut() {
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
    let (file, frame, size) = match &loaded {
        cull::Loaded::Ok {
            file, frame, size, ..
        }
        | cull::Loaded::Failed {
            file, frame, size, ..
        } => (*file, frame.clone(), *size),
        // Never a full-size copy's size, so never taken for one below.
        cull::Loaded::NoPreview { file, frame } => (*file, frame.clone(), u32::MAX),
    };
    // A preview of a list since replaced, or renumbered under it, would
    // land on a stranger's cell: only its own frame's.
    if st.key_of(file).as_ref() != Some(&frame) {
        return;
    }
    let path = frame.path.clone();
    let current = st.current;
    let zoom = st.develop.zoom;
    let culling = st.culling.cull.is_some();
    // The frames whose full-size copies are taken in: those held whole
    // at 1:1 while culling, the one standing in otherwise.
    let shown: Vec<usize> = match (st.culling.cull.as_ref(), current) {
        (Some(cull), Some(c)) => cull.whole(c, &st.views.shown),
        _ => current.into_iter().collect(),
    };
    let plan = loupe_plan(st, file);
    let Some(cull) = st.culling.cull.as_mut().or(st.culling.hold.as_mut()) else {
        return;
    };
    if size == 0 {
        cull.full_asked.remove(&file);
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
            // A full-size copy is for a frame held whole at 1:1, and one
            // that lands after a fit or a move is not taken in.
            if preview.full && (zoom <= 0.0 || !shown.contains(&file)) {
                return;
            }
            // The kind the cell wants: the camera's in Cull on the
            // camera's JPEG but for a versions compare's tiles, and the
            // edit's where one is kept for the develop view's stand-in.
            // One of the other kind was asked before the view changed
            // (a stand-in's before the mode, the mode's before it was
            // left, a tile's before Compare versions) and the right one
            // is on its way: it is not taken in, and asked for nothing.
            let camera = culling && !cull.shows_edit(file);
            if !preview.full && !preview.fits(camera) {
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
                if slot.is_some_and(|p| !p.local && p.fits(camera)) {
                    return;
                }
                // The file failed before the preview landed: the
                // preview is what shows, and the file is not asked for
                // again, as when it fails after.
                if cull.failed.remove(&file).is_some() {
                    cull.file_failed.insert(file);
                }
            } else {
                // The file's own picture, after a failure from another
                // want of it (the develop view's stand-in, which does
                // not develop a raw with no camera picture): the
                // picture is what shows, with no failure said over it.
                cull.failed.remove(&file);
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
    if culling && size == 0 && zoom > 0.0 {
        cull.say_full(&shown);
    }
    app.window().request_redraw();
    if culling {
        return;
    }
    // The develop view: the frame still waited on puts its picture
    // up, and a frame with no camera JPEG in it gives up the wait,
    // leaving the picture on screen where it is until the develop.
    // The edit's picture may wait for its frame's size.
    let waits = made && waits_for_size(st, app, file);
    if st
        .culling
        .placeholder
        .as_mut()
        .is_some_and(|w| made && !waits && w.arrived(file))
    {
        placeholder_arrived(st, app, file);
    } else if !made
        && st
            .culling
            .placeholder
            .as_ref()
            .is_some_and(|w| w.file == file && !w.up())
    {
        drop_placeholder(st, app);
    }
}

/// At 1:1, the full-size copy of each frame held whole before anything
/// else, the chosen one's first. Asked once: the thread's own record of
/// it is gone a moment before the delivery reaches this thread, and a
/// frame in that moment would ask again. A frame out of reach has its
/// local preview and nothing more.
pub(crate) fn ask_whole(st: &mut State) {
    let Some(c) = st.current else {
        return;
    };
    if st.develop.zoom <= 0.0 {
        return;
    }
    let Some(cull) = st.culling.cull.as_mut() else {
        return;
    };
    let mut asks = Vec::new();
    for f in cull.whole(c, &st.views.shown) {
        if !cull.cache.has_own_size(f)
            && cull.wants_file(f)
            && !cull.full_asked.contains(&f)
            && loupe_plan_of(&st.files, &st.from_row, &st.cells, &st.views.library, f).file
        {
            cull.full_asked.insert(f);
            let path = st.files[st.cells[f].file].clone();
            asks.push(
                cull::Want::for_loupe(f, path.clone(), 0)
                    .of_version(st.cells[f].version.clone())
                    .with_picture(companion_picture(&st.views.companions, &path)),
            );
        }
    }
    // Each goes to the front of the queue, so the last one put there
    // is the first decoded.
    for want in asks.into_iter().rev() {
        st.culling.prefetch.push_front(want);
    }
}

/// One frame of the culling loupe: the compared frames' previews on
/// the GPU (made now if their copy changed), each fitted to its tile
/// or at the zoom, turned as its edit turns it; the tiles' boxes
/// and names for the overlay; the status line; and the captures a
/// batch run waits on, once every tile has its picture.
pub(crate) fn cull_frame(st: &mut State, app: &App, state: &Rc<Shared>) {
    draw_cull(st, app, state);
    // A focus point that came in after the zoom key went to 1:1 about
    // the middle takes the view there now.
    crate::panel::viewport::focus_arrived(st, app);
}

/// Whether the viewport draws the culling loupe's picture (the camera's
/// or the edit's) rather than the develop: in Cull and the lightbox's
/// loupe, and while the camera's picture stands in for a develop
/// (`startup::land_pending`'s choice).
pub(crate) fn loupe_draws(st: &State) -> bool {
    st.culling.cull.is_some() || standing_in(st).is_some()
}

/// Where the camera focused on the frame the loupe draws, in the
/// loupe's units for `develop.center` (the camera picture's own pixels
/// at its turns and mirror, as `draw_cull` lays it): the point read
/// from the file's head (`focus_points`), through the cut to the
/// camera's frame the picture comes with, to the picture's pixels, and
/// through the frame's quarter turns and flip. Never the edit's crop or
/// straightening, which the camera's picture is not drawn by. None for
/// a frame whose point is not known (yet), for the edit's picture
/// standing in, and for a point the camera's cut leaves out.
pub(crate) fn loupe_focus_center(st: &State, app: &App) -> Option<(f32, f32)> {
    let holding = st.culling.cull.is_none();
    let selected = st.current?;
    let c = match standing_in(st) {
        Some(file) if holding => file,
        _ => selected,
    };
    if stands_for(st, app, c).is_some() {
        return None;
    }
    let cull = st.culling.cull.as_ref().or(st.culling.hold.as_ref())?;
    let preview = cull.cache.best(c)?;
    if preview.edited {
        return None;
    }
    let path = st.path_of(c);
    let (point, tag) = crate::focus_points::known(path)??;
    let (mut u, mut v) = point.center(tag);
    // The shape of the picture the point's fractions are of, as shown.
    let mut shape = point.oriented_aspect(tag);
    // The camera's picture of a frame the camera framed comes cut to
    // that frame (`cull::cut_to_camera`), and the camera's JPEG beside
    // the raw is that cut already.
    if (preview.cut || preview.companion)
        && let Some(frame) = crate::edited_pictures::camera_known(path)
    {
        let [x, y, w, h] = frame.shown();
        (u, v) = ((u - x) / w, (v - y) / h);
        shape *= w / h;
    }
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    // A picture of another shape is not the one the point is on: a
    // camera JPEG beside the raw shot at 16:9, 1:1, or 4:3 while the
    // point is of the raw's 3:2. Within 1%, as the reader holds a
    // decode's crop to the maker's picture.
    // In the develop's pixels where the picture is drawn as the
    // develop's size (`camera_stands_for`).
    let source = camera_stands_for(st, c)
        .filter(|_| holding)
        .unwrap_or(preview.source);
    let (sw, sh) = (source.0 as f32, source.1 as f32);
    let seen = sw / sh;
    let same =
        shape.is_finite() && seen.is_finite() && (shape - seen).abs() <= 0.01 * shape.max(seen);
    if !same {
        return None;
    }
    let from_panel = holding && Some(c) == st.current;
    let (turns, flip) = thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, c, app, from_panel);
    let geometry = Geometry {
        turns,
        flip,
        ..Geometry::default()
    };
    let r = geometry.to_plane((u * sw, v * sh), sw, sh);
    let origin = geometry.frame(sw, sh, None).origin;
    Some((r.0 - origin.0, r.1 - origin.1))
}

/// Whether the frame the loupe draws has said where its camera focused,
/// a point or none: its picture in hand, and its file's head read, or
/// a file no read can find a point in.
pub(crate) fn loupe_focus_answered(st: &State) -> bool {
    let holding = st.culling.cull.is_none();
    let Some(selected) = st.current else {
        return true;
    };
    let c = match standing_in(st) {
        Some(file) if holding => file,
        _ => selected,
    };
    let in_hand = st
        .culling
        .cull
        .as_ref()
        .or(st.culling.hold.as_ref())
        .is_some_and(|cull| cull.cache.best(c).is_some());
    let path = st.path_of(c);
    in_hand
        && (!greycard_core::decode::may_have_focus_point(path)
            || crate::focus_points::known(path).is_some())
}

/// The point of the frame the loupe draws is owed a read of its file's
/// head: its picture came from the local previews, which read nothing
/// of the file. The path to read, when one is owed.
pub(crate) fn loupe_focus_owed(st: &State) -> Option<PathBuf> {
    let holding = st.culling.cull.is_none();
    let selected = st.current?;
    let c = match standing_in(st) {
        Some(file) if holding => file,
        _ => selected,
    };
    let path = st.path_of(c);
    crate::focus_points::wanted(path).then(|| path.clone())
}

fn draw_cull(st: &mut State, app: &App, state: &Rc<Shared>) {
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    let scale_factor = app.window().scale_factor();
    let canvas = render::canvas_rgb(app.get_canvas_choice());
    let Some(selected) = st.current else {
        return;
    };
    let count = st.views.shown.len();
    // The mode itself, or the camera's picture standing in for a
    // develop: the mode just left, or a frame just selected.
    let holding = st.culling.cull.is_none();
    if !holding {
        ask_whole(st);
    }
    // Which frame is drawn: while one stands in, the frame there is a
    // picture of, which lags the selection until its own is decoded.
    let c = match standing_in(st) {
        Some(file) if holding => file,
        _ => selected,
    };
    // `--cull-develop` is about to leave: no capture of this picture.
    let leave_next = st.launch.cull_develop;
    // The picture standing in is drawn as the frame it stands for: the
    // edit's as its frame, the camera's as the develop's size. The
    // current frame's picture, which the others' centers follow.
    let current = lay_out_current(st, app, c, holding && Some(c) == st.current);
    let Some(rows) = st
        .culling
        .cull
        .as_ref()
        .or(st.culling.hold.as_ref())
        .map(|cull| {
            if holding {
                cull::row_of_shown(&st.views.shown, c).into_iter().collect()
            } else {
                cull.rows(&st.views.shown)
            }
        })
    else {
        return;
    };
    let rects = cull::tile_rects(vw, vh, rows.len().max(1));
    if st.develop.renderer.is_none() {
        return;
    }
    let planned = plan_tiles(st, app, c, current, &rows, &rects, canvas);
    let Some(cull) = st.culling.cull.as_mut().or(st.culling.hold.as_mut()) else {
        return;
    };
    let Some(renderer) = st.develop.renderer.as_mut() else {
        return;
    };
    sync_display(
        &mut st.develop.lut_for,
        &mut st.develop.encoded_lut_for,
        &st.develop.monitors,
        app,
        renderer,
    );
    let whole = if holding {
        vec![c]
    } else {
        cull.whole(c, &st.views.shown)
    };
    let mut tiles = Vec::new();
    let mut overlay = Vec::new();
    let mut ready = true;
    let mut names = Vec::new();
    for (k, &row) in rows.iter().enumerate() {
        let file = st.views.shown[row];
        let rect = rects[k];
        let meta = &st.sidecars[st.cells[file].file].meta;
        let version = version_name_of(&st.sidecars, &st.cells, file);
        overlay.push(CompareTile {
            x: rect.0 as f32 / scale_factor,
            y: rect.1 as f32 / scale_factor,
            w: rect.2 as f32 / scale_factor,
            h: rect.3 as f32 / scale_factor,
            name: tile_name(
                &st.files[st.cells[file].file],
                st.cells[file].version.as_ref(),
                version.as_deref(),
            )
            .into(),
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
        let Some((view, named)) = planned[k].clone() else {
            continue;
        };
        tiles.push((file, (preview.width, preview.height), view, rect));
        // At 1:1 a capture waits for every pixel of each tile held whole.
        if whole.contains(&file)
            && st.develop.zoom > 0.0
            && !preview.own_size()
            && !cull.failed.contains_key(&file)
        {
            ready = false;
        }
        names.push(Named {
            file,
            size: named,
            small: preview.small(),
            own: preview.own_size(),
            local: preview.origin(),
            version,
        });
    }
    let drawn: Vec<render::Tile> = tiles
        .iter()
        .map(|(file, full, view, rect)| render::Tile {
            texture: &cull.textures[file].1,
            full: *full,
            view: view.clone(),
            rect: *rect,
        })
        .collect();
    let texture = renderer.render_encoded(
        vw,
        vh,
        canvas,
        &drawn,
        crate::panel::startup::peaking_of(app),
    );
    // The rule and the names are for telling compared frames apart;
    // one frame alone needs neither.
    if holding || rows.len() < 2 {
        sync_rows(&st.culling.compare_tiles, Vec::new());
    } else {
        sync_rows(&st.culling.compare_tiles, overlay);
    }
    let label = zoom_label(st.develop.zoom);
    if app.get_zoom_text() != label {
        app.set_zoom_text(label);
    }
    app.set_nav_partial(false);
    let shown_now = tiles.iter().any(|(f, _, _, _)| *f == c);
    // The camera's picture standing in for a develop says what it is
    // here as well as when it came in hand: the frame may have been
    // turned since, and then the size in the line is the other way
    // round.
    if holding && let Some(n) = names.iter().find(|n| n.file == c) {
        say_placeholder(app, n.size, n.small, n.local, (st.develop.zoom, n.own));
    }
    // The frame that shows it is the one a timing hook measures to.
    if holding
        && shown_now
        && st
            .culling
            .placeholder
            .as_mut()
            .is_some_and(placeholder::Wait::drawn)
        && let Some(at) = st.views.selected_at
    {
        let ms = at.elapsed().as_secs_f64() * 1e3;
        tracing::debug!("select to the camera picture: {ms:.1} ms");
        if let Some((_, to_picture, _)) = st.launch.time_select.as_mut() {
            to_picture.push(ms);
        }
    }
    // The status: what is on screen and how, or why not yet.
    if !holding {
        let status = cull_status(cull, c, st.develop.zoom, shown_now, &names);
        if shown_now && let Some(said) = cull.said.take() {
            app.set_status(tr!("{said}; {status}", said = said, status = status).into());
            cull.status = status;
        } else if cull.status != status {
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
            st.launch.awaiting_turn = false;
        }
        if shown_now && let Some(at) = cull.switched.take() {
            let ms = at.elapsed().as_secs_f64() * 1e3;
            tracing::debug!("cull switch to frame: {ms:.1} ms");
            time_cull(
                &mut st.launch.time_cull,
                app,
                state,
                ms,
                cull::row_of_shown(&st.views.shown, c),
                count,
            );
            // `--turn`: the key, from the event loop, and the Enter
            // of `--cull-develop` behind it in the same callback when
            // a capture asks for both, so the develop that is caught
            // is of the turned frame and not of both in turn. The
            // flag is cleared in the callback rather than here, so a
            // capture waiting on it does not go off in between.
            if let Some(quarters) = st.launch.turn_at_start {
                let app_weak = app.as_weak();
                let then_leave = std::mem::take(&mut st.launch.cull_develop);
                let state = state.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        state.borrow_mut().launch.turn_at_start = None;
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
            if let Some(key) = st.launch.cull_key_at_start.clone() {
                let app_weak = app.as_weak();
                let state = state.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        state.borrow_mut().launch.cull_key_at_start = None;
                        app.invoke_meta_key(key.into());
                    }
                });
            }
            // `--cull-develop`: the Enter, from the event loop.
            if std::mem::take(&mut st.launch.cull_develop) {
                let app_weak = app.as_weak();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    if let Some(app) = app_weak.upgrade() {
                        app.invoke_cull_leave();
                    }
                });
            }
            // `--ask-rejects` and `--move-rejects`: the button, and
            // for the second the sheet's yes, from the event loop.
            if std::mem::take(&mut st.launch.ask_rejects) {
                let answer = std::mem::take(&mut st.launch.move_rejects);
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
        && !st.launch.awaiting_turn
        && st.launch.cull_key_at_start.is_none()
        && !st.launch.awaiting_index
        && !st.views.library.awaiting
        && st.develop.asked.is_empty()
        && grid_filled_rows(
            &st.views.shown,
            &st.views.thumb_made,
            &st.views.thumb_failed,
            st.views.thumb_want,
            st.views.grid_shown,
            st.launch.grid_wait_over,
            app,
        );
    // `--snapshot-placeholder` waits for the camera's picture instead,
    // and for one standing in over a develop already on screen rather
    // than for the first file's own, which is up before anything has
    // been developed at all. The run goes on afterwards: the develop
    // it stands in for is on the GPU this moment.
    let standing_in =
        holding && st.launch.snapshot_placeholder && shown_now && st.develop.base_white.is_some();
    schedule_snapshot(
        &mut st.launch.snapshot,
        &mut st.launch.panel_scroll,
        &mut st.launch.snapshot_shown,
        app,
        state,
        ready && (standing_in || !holding),
        !standing_in,
    );
    write_screenshot(
        &mut st.launch.screenshot,
        &mut st.launch.failed,
        renderer,
        &texture,
        ready && !holding,
    );
    app.set_texture(slint::Image::try_from(texture).expect("the texture imports"));
    // The open frame's picture drawn in the loupe: past what the open
    // note guards (`open_note::shown`).
    if shown_now {
        crate::open_note::shown();
        // And the frame to come back to, once it settles.
        crate::panel::startup::loupe_drew(st);
    }
}

/// `thumb_turns` without the state, for a frame with the state's
/// fields borrowed apart.
pub(crate) fn thumb_turns_of(
    sidecars: &[Sidecar],
    from_row: &[crate::rows::FromRow],
    cells: &[crate::versions::Cell],
    c: usize,
    app: &App,
    from_panel: bool,
) -> (u8, bool) {
    let cell = &cells[c];
    // A frame whose sidecar stands in from its row: the row says.
    if let Some(from) = from_row.get(cell.file)
        && !from.read
    {
        return from.turns;
    }
    let sidecar = &sidecars[cell.file];
    let turn = sidecar.turn;
    if from_panel {
        read_geometry(app).shown_turns(turn)
    } else {
        sidecar
            .line(cell.version.as_deref())
            .unwrap_or(&sidecar.line)
            .current()
            .geometry
            .shown_turns(turn)
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
    pub(crate) local: cull::Origin,
    /// The version's name, for a version's tile; none for the file's
    /// own, whose words are as they always were.
    pub(crate) version: Option<String>,
}

/// The name of the version cell `c` is, none for the file's own, with
/// the state's fields borrowed apart.
pub(crate) fn version_name_of(
    sidecars: &[Sidecar],
    cells: &[crate::versions::Cell],
    c: usize,
) -> Option<String> {
    let cell = cells.get(c)?;
    let id = cell.version.as_deref()?;
    sidecars.get(cell.file)?.version(id).map(|v| v.name.clone())
}

/// A compare tile's name at its top: the file's name for the file's
/// own frame, and the frame's label for a version, "IMG_0001 (Warm)",
/// so two tiles of one file are told apart.
/// `id` is the version's id, none for the file's own frame, and `name`
/// its name, the id standing in where it is empty or not had.
pub(crate) fn tile_name(path: &Path, id: Option<&Arc<str>>, name: Option<&str>) -> String {
    crate::versions::Key {
        path: path.to_path_buf(),
        version: id.cloned(),
        name: name.map(Arc::from),
    }
    .said()
}

/// What a tile shows in the plate's words: the picture's
/// ([`picture_words`]), after the version's name for a version's tile,
/// "Warm, the edit, 2048 × 1365".
pub(crate) fn plate_words(named: &Named) -> String {
    let what = picture_words(named.size, named.small, named.local);
    match named.version.as_deref() {
        Some(v) if !v.is_empty() => tr!("{version}, {what}", version = v, what = what),
        _ => what,
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
        tr!("culling, {n} up", n = cull.compare)
    } else {
        tr!("culling")
    };
    if let Some(why) = cull.failed.get(&current) {
        if why == cull::NO_LOCAL_PREVIEW {
            return tr!(
                "{mode}: nothing to show; {why}",
                mode = mode,
                why = crate::i18n::text(why)
            );
        }
        return tr!(
            "{mode}: no camera preview ({why}); Enter develops",
            mode = mode,
            why = why
        );
    }
    let Some(named) = names.iter().find(|n| n.file == current) else {
        return tr!("{mode}: decoding the camera JPEG...", mode = mode);
    };
    if !shown {
        return tr!("{mode}: decoding the camera JPEG...", mode = mode);
    }
    let what = plate_words(named);
    let at = zoom_words(zoom, named.own);
    match &cull.note {
        Some(note) => tr!(
            "{mode}: {what}, {at}; Enter develops; {note}",
            mode = mode,
            what = what,
            at = at,
            note = note
        ),
        None => tr!(
            "{mode}: {what}, {at}; Enter develops",
            mode = mode,
            what = what,
            at = at
        ),
    }
}

/// `--time-cull`, on each frame that showed a stepped-to picture:
/// the milliseconds since the key, then the next step a tenth of a
/// second on, as a hand would arrow; after the last, the numbers on
/// the log and the terminal, and the editor quits.
pub(crate) fn time_cull(
    timing: &mut Option<(u32, Vec<f64>)>,
    app: &App,
    state: &Rc<Shared>,
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
            if let Some(cull) = state.borrow_mut().culling.cull.as_mut() {
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
    let shoot = dunce::canonicalize(shoot).unwrap_or_else(|_| shoot.to_path_buf());
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
            let shoot = dunce::canonicalize(folders[0]).unwrap_or_else(|_| folders[0].into());
            Some(cull::rejects_dir(&shoot).display().to_string())
        }
        n => Some(tr!(
            "a rejects folder in each of the {n} folder they are in"
                | "a rejects folder in each of the {n} folders they are in" % n
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
        && !crate::rows::file_is_offline(st, i)
        && st
            .files
            .get(i)
            .and_then(|f| f.parent())
            .and_then(Path::file_name)
            != Some(std::ffi::OsStr::new(cull::REJECTS))
}

/// The move-rejects sheet: how many frames and where they would go.
pub(crate) fn ask_rejects(st: &State, app: &App) {
    if st.sheets.deleting.is_some() {
        app.set_status(tr!("a delete is still under way").into());
        return;
    }
    let n = reject_count(st);
    if n == 0 {
        app.set_status(tr!("no frames are flagged reject").into());
        return;
    }
    let Some(dir) = rejects_where(st) else {
        return;
    };
    app.set_rejects_text(
        tr!(
            "{n} frame flagged reject, with its sidecar, will be moved to {folder}."
                | "{n} frames flagged reject, with their sidecars, will be moved to {folder}." % n,
            folder = dir
        )
        .into(),
    );
    app.set_rejects_open(true);
}

/// Move the rejects out, and take them out of the browser's list.
/// The frame the selection was on, if it went, gives way to the
/// nearest one left.
pub(crate) fn move_rejects(st: &mut State, app: &App, worker: &Worker) {
    if st.sheets.deleting.is_some() {
        app.set_status(tr!("a delete is still under way").into());
        return;
    }
    // A batch of sidecars still out lands first, or its renames would
    // put sidecars back where the frames were.
    crate::panel::saves::finish(st, None);
    // Those already out, in a rejects folder, stay where they are and
    // are not counted.
    let rejected: Vec<usize> = (0..st.files.len())
        .filter(|&i| to_move_out(st, i))
        .collect();
    let moved = match cull::move_rejects_as(
        &st.files,
        &rejected,
        cull::Orphans::WriteOver,
        st.views.jpegs_apart,
    ) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("moving the rejects: {e:#}");
            app.set_status(
                tr!(
                    "the rejects could not be moved: {why}",
                    why = format!("{e:#}")
                )
                .into(),
            );
            return;
        }
    };
    for (i, why) in &moved.skipped {
        tracing::warn!("{} left where it is: {why}", file_name(&st.files[*i]));
    }
    // Where the ones that went, went: said while the list still has
    // them where they were.
    let dir = rejects_where_of(st, &moved.files).unwrap_or_default();
    let mut status = tr!(
        "moved {n} frame{companions} and {sidecars} to {folder}"
            | "moved {n} frames{companions} and {sidecars} to {folder}" % moved.files.len(),
        companions = crate::delete::companion_words(moved.companions.len()),
        sidecars = sidecars_word(moved.sidecars),
        folder = dir
    );
    status.push_str(&left_words(&moved.skipped));
    status.push_str(&half_words(&moved.companions_left));
    tracing::info!("rejects: {status}");
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
    if let Some(indexer) = &st.views.index {
        indexer.moved(
            moved
                .files
                .iter()
                .zip(&moved.to)
                .map(|(&i, to)| (st.files[i].clone(), to.clone()))
                .chain(moved.companions.iter().cloned())
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
    if let Some(indexer) = &st.views.index
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
    tr!(
        "; {n} left where it was ({why})"
            | "; {n} left where they were ({why}; the log has each)" % skipped.len(),
        why = why
    )
}

/// "1 sidecar", "3 sidecars", as a move's line says them.
fn sidecars_word(n: usize) -> String {
    tr!("{n} sidecar" | "{n} sidecars" % n)
}

/// The camera's JPEGs a move left behind their raws, as the status
/// line's tail: half a pair is said, not only logged. Empty for none.
fn half_words(left: &[(PathBuf, String)]) -> String {
    let Some((jpeg, why)) = left.first() else {
        return String::new();
    };
    tr!(
        "; {n} camera JPEG stayed behind its raw ({name}: {why})"
            | "; {n} camera JPEGs stayed behind their raws ({name}: {why})" % left.len(),
        name = file_name(jpeg),
        why = why
    )
}

/// Whether file `i` can be moved back: it is in a rejects folder, and
/// not under a root that is offline, where nothing is moved.
pub(crate) fn to_move_back(st: &State, i: usize) -> bool {
    st.files
        .get(i)
        .is_some_and(|f| cull::above_rejects(f).is_some())
        && !crate::rows::file_is_offline(st, i)
}

/// The files Move back is over: those of the selection in a rejects
/// folder, once each whichever of their cells is chosen.
fn back_frames(st: &State) -> Vec<usize> {
    st.files_of(&crate::panel::browser::chosen_frames(st))
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
        _ => tr!("their folders"),
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
    if st.sheets.deleting.is_some() {
        app.set_status(tr!("a delete is still under way").into());
        return;
    }
    let frames = back_frames(st);
    if frames.is_empty() {
        app.set_status(tr!("no frame chosen is in a rejects folder").into());
        return;
    }
    crate::panel::saves::finish(st, None);
    let moved = cull::move_back(&st.files, &frames, st.views.jpegs_apart);
    for (i, why) in &moved.skipped {
        tracing::warn!("{} left where it is: {why}", file_name(&st.files[*i]));
    }
    // Said while the list still has them where they were.
    let above = folders_above(st, &moved.files);
    let mut status = match above.as_slice() {
        [] => tr!("nothing moved back"),
        many => tr!(
            "moved {n} frame{companions} and {sidecars} back to {folder}"
                | "moved {n} frames{companions} and {sidecars} back to {folder}"
                    % moved.files.len(),
            companions = crate::delete::companion_words(moved.companions.len()),
            sidecars = sidecars_word(moved.sidecars),
            folder = match many {
                [one] => dunce::canonicalize(one)
                    .unwrap_or_else(|_| one.clone())
                    .display()
                    .to_string(),
                _ => tr!("the folder above each of the {n} rejects folder"
                    | "the folder above each of the {n} rejects folders" % many.len()),
            }
        ),
    };
    status.push_str(&left_words(&moved.skipped));
    status.push_str(&half_words(&moved.companions_left));
    tracing::info!("rejects: {status}");
    // The frames' flags written at their new paths, then the index told
    // of the moves, then both folders passed over. A save's word that
    // reaches the index before the move's makes a row at the new path,
    // which the move's then puts the old row over, reading the sidecar
    // again; one after it finds the row moved. Either way the row keeps
    // its id and has the flag as written.
    let old: Vec<PathBuf> = moved.files.iter().map(|&i| st.files[i].clone()).collect();
    let before = st.work.unsaved.said_count();
    for (&i, to) in moved.files.iter().zip(&moved.to) {
        st.move_file(i, to.clone());
        unreject(st, i);
    }
    for (&i, from) in moved.files.iter().zip(old) {
        st.move_file(i, from);
    }
    // A flag that would not come off says so over the move's line,
    // which the log keeps: the frames leave the list, and the note,
    // as the move's line is said.
    let unsaved = crate::sidecars::said_since(st, before);
    hand_moves(st, &moved);
    hand_folders(st, &moved);
    drop_files(st, app, worker, &moved.files, status);
    if let Some(line) = unsaved {
        app.set_status(line.into());
    }
}

/// The reject flag taken off frame `i`, moved back to the path the list
/// now has for it, and its sidecar written there: through the window's
/// own write when the sidecar in memory is the frame's, and otherwise
/// (a row of the index standing in for it) read from the disk, changed
/// and written back where it was found.
pub(crate) fn unreject(st: &mut State, i: usize) {
    if st.sidecars[i].meta.flag != meta::Flag::Reject {
        return;
    }
    // A newer build's sidecar is read-only (§724): its flag stays.
    if st.sidecars[i].read_only() {
        tracing::warn!(
            "{}: read-only; its reject flag is left",
            file_name(&st.files[i])
        );
        return;
    }
    st.sidecars[i].meta.flag = meta::Flag::None;
    if crate::sidecars::file_writable(st, i) {
        crate::sidecars::write_file(st, i);
        return;
    }
    if !st.window.write_sidecars {
        return;
    }
    let path = st.files[i].clone();
    let placement = greycard_edit::Placement::of(&path);
    match Sidecar::load(&path, &st.sidecars[i].start) {
        Ok(Some(sidecar)) if sidecar.read_only() => {
            tracing::warn!("{}: read-only; its reject flag is left", file_name(&path));
        }
        Ok(Some(mut sidecar)) if sidecar.meta.flag == meta::Flag::Reject => {
            sidecar.meta.flag = meta::Flag::None;
            if st.window.xmp_sidecars
                && let Err(e) = greycard_edit_io::xmp::save(&path, &sidecar.meta, None)
            {
                tracing::warn!("{}: xmp not written: {e}", file_name(&path));
            }
            match sidecar.save_in(&path, placement) {
                Ok(()) => {
                    if let Some(indexer) = &st.views.index {
                        indexer.file(path.clone());
                    }
                    // The archive's copy, from the file: the sidecar in
                    // memory is the row standing in, not this.
                    crate::sync::after_disk_save(st, &path);
                    crate::sidecars::landed(st, &path);
                }
                Err(e) => {
                    tracing::warn!("{}: sidecar not saved: {e}", file_name(&path));
                    let why = crate::sidecars::Why::of(&e);
                    crate::sidecars::failed(st, &path, why);
                }
            }
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
    // A frame deleted or moved out of its folder leaves its stack, and
    // the Fill's strokes waiting on it go with it.
    crate::stacks::left_files(st, gone);
    let paths: Vec<std::path::PathBuf> = gone
        .iter()
        .filter_map(|&i| st.files.get(i).cloned())
        .collect();
    crate::panel::browser::files_gone(st, &paths);
    // The files that go take every cell of theirs, versions included.
    let keep = |i: &usize| !gone.contains(i);
    // Where the frame on screen would stand in the new list if it is
    // among those going: the cell after it that stays, else the last.
    // `rebuild_browser` cannot say, since it looks for the current
    // frame and there is none then.
    let stand_in = st
        .current
        .filter(|&c| gone.contains(&st.file_of(c)))
        .and_then(|c| {
            let stays = |x: &usize| keep(&st.cells[*x].file);
            let kept = (0..st.cells.len()).filter(stays).count();
            (kept > 0).then(|| (0..c).filter(stays).count().min(kept - 1))
        });
    let files: Vec<PathBuf> = st
        .files
        .iter()
        .enumerate()
        .filter(|(i, _)| keep(i))
        .map(|(_, f)| f.clone())
        .collect();
    let kept: Vec<usize> = (0..st.files.len()).filter(keep).collect();
    let mut new_of: Vec<Option<usize>> = vec![None; st.files.len()];
    for (at, &i) in kept.iter().enumerate() {
        new_of[i] = Some(at);
    }
    st.files = files;
    st.sidecars = kept.iter().map(|&i| st.sidecars[i].clone()).collect();
    st.from_row = kept.iter().map(|&i| st.from_row[i].clone()).collect();
    // A frame gone takes an edit not saved with it.
    crate::sidecars::listed(st);
    // The index's answers by the new numbering, and a pass over the
    // folder, which marks the rows of frames moved out missing (a
    // delete has had its rows forgotten already).
    st.views.index_passed = kept
        .iter()
        .map(|&i| st.views.index_passed.get(i).copied().unwrap_or(true))
        .collect();
    // The cells follow the files: each that stays keeps its picture and
    // its place in the selection; the pictures keyed by the old
    // numbering (the previews, the camera picture standing in) are put
    // down and asked for again.
    let compared = crate::versions::compared_file(st);
    crate::versions::relist(st, app, worker, &|i| new_of.get(i).copied().flatten());
    crate::library::index_open_folder(st, None);
    // A rejects folder made or emptied under a folder of no root: its
    // tiles read again, as the index tells the tree for a root's.
    crate::tree::loose_changed(st, app);
    // The set was numbered by the old list; the frame on screen is
    // what is left of it.
    st.views.picked.clear();
    let next = rebuild_browser(st, app)
        .or_else(|| stand_in.and_then(|f| crate::cull::nearest_row(&st.views.shown, f)));
    // The same list less some frames: the strip and the grid hold
    // their scroll and move only to show the frame chosen next. The
    // window reads this after this call, when its change handlers
    // run; the next list put says false again.
    app.set_rows_held(true);
    // After the rebuild, so the rows put first and the rows near the
    // screen are the new list's.
    crate::roots::ask_owed(st, worker);
    app.set_status(status.into());
    match (st.current, next) {
        (Some(c), _) => {
            if st.culling.cull.is_some() {
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
            crate::panel::browser::show_file_name(st, app, None);
        }
    }
    crate::versions::refit_compare(st, app, compared);
}

/// The filter's chips and what they count, on the grid's header and
/// on the CULLING section alike: one pass over the sidecars the open
/// folder has already loaded.
///
/// Nothing is decided here. Which frames pass, and how many a chip
/// would leave, are [`filter`]'s; this reads the answer onto the
/// window.
pub(crate) fn show_filter(st: &State, app: &App) {
    let mut frames = filter_frames(st);
    // A closed stack is its top: the frames behind it are not counted.
    if let Some(behind) = crate::stacks::behind(st) {
        let mut f = 0;
        frames.retain(|_| {
            f += 1;
            !behind.get(f - 1).copied().unwrap_or(false)
        });
    }
    let counts = filter::Counts::of(&st.views.filter, &frames);
    let exact = st.views.filter.stars.is_exact();
    let picked = st.views.filter.stars.count();
    let stars: Vec<FilterChip> = (0..=meta::STARS)
        .map(|n| FilterChip {
            text: filter::Stars::chip_name(n, exact).into(),
            count: counts.stars[n as usize] as i32,
            on: n == picked,
            out: false,
            code: n as i32,
        })
        .collect();
    // By the chip's place in the row, which is what the callback gets
    // back and what `Counts` is indexed by; the code is only what the
    // badge draws. The names are marked for the catalog in `filter`.
    let flags: Vec<FilterChip> = meta::Flag::ALL
        .iter()
        .enumerate()
        .map(|(n, f)| FilterChip {
            text: crate::i18n::text(f.name()).into(),
            count: counts.flags[n] as i32,
            on: st.views.filter.flags.contains(f),
            out: st.views.filter.not_flags.contains(f),
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
                tr!("No label")
            } else {
                String::new()
            }
            .into(),
            count: counts.labels[n] as i32,
            on: st.views.filter.labels.contains(l),
            out: st.views.filter.not_labels.contains(l),
            code: l.code(),
        })
        .collect();
    // Every list here in place, row by row: a pass over the folder puts
    // the filter on the window again at each batch, and lists made
    // again would make every chip and facet row again at each, a facet
    // row scrolled sideways snapping back to its start.
    keep_rows(app.get_filter_stars(), |m| app.set_filter_stars(m), stars);
    keep_rows(app.get_filter_flags(), |m| app.set_filter_flags(m), flags);
    keep_rows(
        app.get_filter_labels(),
        |m| app.set_filter_labels(m),
        labels,
    );
    app.set_filter_exact(exact);
    app.set_filter_shown(counts.shown as i32);
    app.set_filter_total(counts.total as i32);
    app.set_filter_on(!st.views.filter.is_empty());
    // The facets: the EXIF's counted by the index, none until it has
    // rows for the folder; the keyword's and the kind's counted from
    // the frames in hand. The note beside them says how the index is
    // getting on, or why there are none.
    let rows = crate::library::facet_rows(st);
    let note = crate::library::facet_note(st, !rows.is_empty());
    let (long, short) = crate::library::split_facet_rows(&rows);
    // ISO and the focal length are the CULLING section's ranges over
    // the folder's values, and chips nowhere.
    let mut ranges = crate::library::facet_ranges(st).into_iter();
    app.set_filter_iso_range(ranges.next().unwrap_or_default());
    app.set_filter_focal_range(ranges.next().unwrap_or_default());
    keep_facets(app.get_filter_facets(), |m| app.set_filter_facets(m), rows);
    keep_facets(
        app.get_filter_facets_long(),
        |m| app.set_filter_facets_long(m),
        long,
    );
    keep_facets(
        app.get_filter_facets_short(),
        |m| app.set_filter_facets_short(m),
        short,
    );
    app.set_filter_facet_note(note.into());
}

/// `rows` into the window's list `now`, in place when it is one this
/// put there, else as a list of its own (`set`), the first time.
fn keep_rows<T: Clone + PartialEq + 'static>(
    now: ModelRc<T>,
    set: impl FnOnce(ModelRc<T>),
    rows: Vec<T>,
) {
    match now.as_any().downcast_ref::<VecModel<T>>() {
        Some(model) => sync_rows(model, rows),
        None => set(ModelRc::new(VecModel::from(rows))),
    }
}

/// [`keep_rows`] for facet rows, each with its chips: a row that is
/// still the same facet keeps its chips' list and has the chips put
/// into it in place, so the row is not made again either.
fn keep_facets(now: ModelRc<FacetRow>, set: impl FnOnce(ModelRc<FacetRow>), rows: Vec<FacetRow>) {
    let Some(model) = now.as_any().downcast_ref::<VecModel<FacetRow>>() else {
        set(ModelRc::new(VecModel::from(rows)));
        return;
    };
    let rows: Vec<FacetRow> = rows
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let Some(was) = model
                .row_data(i)
                .filter(|was| was.name == row.name && was.code == row.code)
            else {
                return row;
            };
            match was.chips.as_any().downcast_ref::<VecModel<FacetChip>>() {
                Some(chips) => {
                    sync_rows(chips, row.chips.iter().collect());
                    was
                }
                None => row,
            }
        })
        .collect();
    sync_rows(model, rows);
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
    if st.views.shown.is_empty() && !st.files.is_empty() {
        app.set_status(filter::nothing_shown().into());
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
pub(crate) fn apply_filter(state: &Rc<Shared>, app: &App) {
    apply_filter_said(state, app, true, true);
}

/// The same, for a character typed into the text field.
fn apply_filter_typed(state: &Rc<Shared>, app: &App) {
    apply_filter_said(state, app, false, true);
}

/// The filter's text put by a control rather than typed: a range's
/// terms. The field holds its own text, so it is given the new one;
/// `settled` as for [`apply_filter_said`].
fn set_filter_text(state: &Rc<Shared>, app: &App, text: String, settled: bool) {
    {
        let mut st = state.borrow_mut();
        if st.views.filter.text == text {
            return;
        }
        st.views.filter.text = text.clone();
    }
    app.set_filter_text(text.into());
    apply_filter_said(state, app, settled, true);
}

/// The same, for the index having moved under the filter: a pass
/// has reached more of the folder, or a row was written after a
/// save. `settled` when it is worth the log's line.
pub(crate) fn refilter(state: &Rc<Shared>, app: &App, settled: bool) {
    // The index moving is no reason to say a typing mistake again.
    apply_filter_said(state, app, settled, false);
}

fn apply_filter_said(state: &Rc<Shared>, app: &App, settled: bool, say_errors: bool) {
    let mut st = state.borrow_mut();
    let next = rebuild_browser(&mut st, app);
    let (what, shown, count) = (
        st.views.filter.describe(),
        st.views.shown.len(),
        st.files.len(),
    );
    if settled {
        tracing::info!("browser filter: {what}, {shown} of {count} frames");
    } else {
        tracing::debug!("browser filter: {what}, {shown} of {count} frames");
    }
    say_if_empty(&st, app);
    // A term typed that does not parse is looked for as a word, as
    // it was before the field took terms, and the reason is said.
    if say_errors && let Some(e) = st.views.filter.typed().errors.first() {
        app.set_status(tr!("filter: {why}", why = e).into());
    }
    match next {
        Some(row) => {
            drop(st);
            app.invoke_select(row as i32);
        }
        None => {
            if st.culling.cull.is_some()
                && let Some(c) = st.current
            {
                cull_select(&mut st, app, c);
            }
            app.window().request_redraw();
        }
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    // Culling mode: in and out, the compare view, the browser's
    // filter, and the rejects moved out.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        // C: from Library (not the lightbox, a view of its own) the
        // Loupe in Cull, since Cull is a mode of the Loupe; in the Loupe
        // Cull and Develop in turn.
        app.on_cull_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let library = app.get_grid_open() && !app.get_lightbox();
            // In the Loupe's Cull (or the lightbox over it) C leaves,
            // with a frame or without one.
            if !library && cull_mode(&state.borrow()) {
                to_develop(&mut state.borrow_mut(), &app, &worker);
                return;
            }
            if state.borrow().current.is_none() {
                app.set_status(tr!("nothing to cull: open a folder").into());
                return;
            }
            if library {
                app.set_grid_open(false);
            }
            // A Cull already on under the grid stays as it is: a
            // compare it holds is not ended.
            let mut st = state.borrow_mut();
            if !cull_mode(&st) {
                to_cull(&mut st, &app, &worker);
            }
        });
    }
    // Develop: Return or Escape in Cull, the CULLING section's Develop,
    // the Loupe's Develop and the Develop button's way from Library.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_cull_leave(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            to_develop(&mut state.borrow_mut(), &app, &worker);
        });
    }
    // The Loupe's Cull.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_cull_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // Already lit: a compare Cull holds is not ended.
            if cull_mode(&st) {
                return;
            }
            if st.current.is_none() {
                app.set_status(tr!("nothing to cull: open a folder").into());
                return;
            }
            to_cull(&mut st, &app, &worker);
        });
    }
    // Cull's switch: Camera JPEG or Developed.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_cull_source_asked(move |developed| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            set_cull_source(&mut state.borrow_mut(), &app, &worker, developed);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_compare_changed(move |n| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // Three is Compare versions' alone: the field offers it only
            // while the view is at three, and a press on it changes
            // nothing.
            if n == 3 {
                return;
            }
            let n = match n {
                2 | 4 => n as usize,
                _ => 1,
            };
            let count = st.views.shown.len();
            let row = st.current.and_then(|c| row_of(&st, c)).unwrap_or(0);
            // The row and the key agree with the count whether or
            // not the mode takes it.
            app.set_compare(n as i32);
            app.set_compare_text(n.to_string().into());
            let Some(cull) = st.culling.cull.as_mut() else {
                return;
            };
            cull.compare = n;
            cull.anchor = cull::anchor_for(cull.anchor, row, n, count);
            cull.note = None;
            cull.versions_of = None;
            cull.stack_of = None;
            cull.set = None;
            // The tiles are another size: fitted afresh.
            st.develop.zoom = 0.0;
            st.develop.image_size = (0, 0);
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
            st.culling.cull_move_on = app.get_cull_move_on();
            let on = st.culling.cull_move_on;
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
                let want = filter::Stars::AtLeast(stars).read_as(st.views.filter.stars.is_exact());
                if st.views.filter.stars == want {
                    return;
                }
                st.views.filter.stars = want;
            }
            apply_filter(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_flag_toggled(move |chip, out| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&flag) = meta::Flag::ALL.get(chip.max(0) as usize) else {
                return;
            };
            {
                let filter = &mut state.borrow_mut().views.filter;
                if out {
                    filter.exclude_flag(flag);
                } else {
                    filter.toggle_flag(flag);
                }
            }
            apply_filter(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_label_toggled(move |chip, out| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&label) = meta::Label::ALL.get(chip.max(0) as usize) else {
                return;
            };
            {
                let filter = &mut state.borrow_mut().views.filter;
                if out {
                    filter.exclude_label(label);
                } else {
                    filter.toggle_label(label);
                }
            }
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
                let exact = st.views.filter.stars.is_exact();
                st.views.filter.stars = st.views.filter.stars.read_as(!exact);
            }
            apply_filter(&state, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_facet_toggled(move |code, value, out| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&facet) = filter::Facet::ALL.get(code.max(0) as usize) else {
                return;
            };
            {
                let filter = &mut state.borrow_mut().views.filter;
                if out {
                    filter.exclude_facet(facet, &value);
                } else {
                    filter.toggle_facet(facet, &value);
                }
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
                if st.views.filter.text == text.as_str() {
                    return;
                }
                st.views.filter.text = text.to_string();
            }
            apply_filter_typed(&state, &app);
        });
    }
    // A range's end moved, or the whole range asked for: the terms it
    // stands for written into the text, which the field shows, and the
    // filter applied and logged. The slider asks once a move is done,
    // on a drag's release or an arrow's, so each ask was meant.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_range_moved(move |code, end, low, high| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&facet) = filter::Facet::ALL.get(code.max(0) as usize) else {
                return;
            };
            let text = {
                let st = state.borrow();
                filter::with_range(&st.views.filter.text, facet, end == 0, &low, &high)
            };
            set_filter_text(&state, &app, text, true);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_range_reset(move |code| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(&facet) = filter::Facet::ALL.get(code.max(0) as usize) else {
                return;
            };
            let text = filter::without_range(&state.borrow().views.filter.text, facet);
            set_filter_text(&state, &app, text, true);
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
                if st.views.filter.is_empty() {
                    return;
                }
                st.views.filter = filter::Filter::default();
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
            // The bar put away in the Settings sheet: no field to
            // send the key to, and an ask left standing would take the
            // focus the moment the bar came back.
            if !app.get_filter_bar() {
                tracing::info!("filter key: the filter bar is hidden");
                return;
            }
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
pub(crate) mod tests;
