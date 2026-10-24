//! The export queue as a view of the grid: every queued frame a tile,
//! under a heading for each destination, the left pane's Export queue
//! row its way in.
//!
//! The view lists the queue's frames as files given are listed (each
//! file once, its sidecar read on the pool or standing in from its
//! row), and its cells are the frames, a file queued twice two cells,
//! since each is an export of its own. Entries whose sheet and folder
//! say the same heading are one heading, their frames in the queue's
//! order under it; the destinations in the order each was first queued.
//! A cell follows its queued frame by the frame's id, so the entries
//! around it can change (a set done, frames taken out) and the cell
//! still knows which frame it is.
//!
//! The view is not for editing ([`frozen`]): every write of an edit
//! asks it, the grid's keys and the Loupe's alike. The Loupe from a
//! tile is the frame as it will be exported, developed under the edit
//! it was queued with (`QueuedFrame::edit`, with its turn and seed),
//! not its sidecar's. Delete takes the chosen tiles out of their
//! entries, never their files.

use crate::export_queue::{self, Entry, QueuedFrame};
use crate::roots::View;
use crate::versions::Key;
use crate::*;
use std::collections::HashMap;

/// Where a queued frame is now: its entry, its place there, and its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ref {
    pub(crate) entry: usize,
    pub(crate) frame: usize,
    pub(crate) id: u64,
}

/// The queue as a listing saw it: the id of each listed frame, in the
/// listing's order.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Listed {
    pub(crate) ids: Vec<u64>,
}

/// A heading of the grid: its words, how many frames it heads, and the
/// row of the first of them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Head {
    pub(crate) row: usize,
    pub(crate) text: String,
    pub(crate) count: usize,
}

/// The queued frame the panel holds, opened from the queue's view: its
/// quarter turns and whether its learned blend is seeded, as queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueuedOpen {
    pub(crate) turn: u8,
    pub(crate) seed: bool,
}

/// Whether the frames listed are the export queue's, which are not to
/// be edited: the one question every write of an edit asks, in the
/// grid and the Loupe alike. Ratings, flags, and labels are the
/// sidecar's, not the edit's, and are not asked.
pub(crate) fn frozen(st: &State) -> bool {
    st.views.view == View::Queue
}

/// [`frozen`], said in the log when it refuses `what`: true when
/// `what` is not to be done.
pub(crate) fn refused(st: &State, what: &str) -> bool {
    if frozen(st) {
        tracing::debug!("{what}: the export queue's frames are as they will export; not edited");
        return true;
    }
    false
}

/// A destination's heading: what the sheet asks for, in the words an
/// export's own account uses, and where the files go. The heading is
/// the destination's key: entries that say the same are one.
pub(crate) fn heading(entry: &Entry) -> String {
    let sheet = entry.sheet.settings().describe();
    match (&entry.folder, entry.sheet.subfolder().ok().flatten()) {
        (Some(folder), _) => format!("{sheet}, to {}", crate::report::path_shown(folder)),
        (None, Some(sub)) => format!("{sheet}, to {} beside each original", sub.display()),
        (None, None) => format!("{sheet}, beside each original"),
    }
}

/// The entries by destination: each group the entries with one
/// heading, in the queue's order, the groups in the order each was
/// first queued.
fn groups(entries: &[Entry]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let text = heading(e);
        match out.iter_mut().find(|(t, _)| *t == text) {
            Some((_, g)) => g.push(i),
            None => out.push((text, vec![i])),
        }
    }
    out
}

/// The frames the view lists, in its order, and their ids.
pub(crate) fn listing(st: &State) -> (Vec<Key>, Listed) {
    let entries = &st.work.export_queue;
    let mut keys = Vec::new();
    let mut ids = Vec::new();
    for (_, group) in groups(entries) {
        for e in group {
            for q in &entries[e].frames {
                keys.push(Key::of(
                    q.source.clone(),
                    q.version.as_ref().map(|v| v.id.as_str()),
                ));
                ids.push(q.id);
            }
        }
    }
    (keys, Listed { ids })
}

/// The cells the queue's listing makes now, each with the old cell it
/// carries from, by the queued frame's id rather than its file and
/// version (two copies of a frame queued twice are one file and
/// version): `versions::relist` in the queue's view. None outside it.
pub(crate) fn by_id(st: &State) -> Option<(Vec<crate::versions::Cell>, Vec<Option<usize>>)> {
    if st.views.view != View::Queue {
        return None;
    }
    let (frames, listed) = (
        st.views.collection_frames.as_ref()?,
        st.views.queue_listed.as_ref()?,
    );
    let old_at: HashMap<u64, usize> = ids_of_cells(st)
        .iter()
        .enumerate()
        .filter_map(|(c, id)| Some(((*id)?, c)))
        .collect();
    let (cells, from) = crate::versions::frames_kept(&st.files, &st.sidecars, frames, false)
        .into_iter()
        .map(|(cell, i)| {
            let id = listed.ids.get(i).copied();
            (cell, id.and_then(|id| old_at.get(&id).copied()))
        })
        .unzip();
    Some((cells, from))
}

/// Where each queued frame is now, by its id.
fn where_is(entries: &[Entry]) -> HashMap<u64, (usize, usize)> {
    entries
        .iter()
        .enumerate()
        .flat_map(|(e, entry)| {
            entry
                .frames
                .iter()
                .enumerate()
                .map(move |(f, q)| (q.id, (e, f)))
        })
        .collect()
}

/// The id of the queued frame each cell is, as kept when the cells were
/// last made; all none outside the queue's view.
pub(crate) fn ids_of_cells(st: &State) -> Vec<Option<u64>> {
    let n = st.cells.len();
    if st.views.view != View::Queue {
        return vec![None; n];
    }
    if st.views.queue_cell_ids.len() == n {
        return st.views.queue_cell_ids.clone();
    }
    cell_ids_made(st)
}

/// The id of the queued frame each cell is, by the listing the cells
/// were made from: run as the cells are made (`versions::list`,
/// `versions::carry`), and kept, since a listing that lands later (a
/// merge read's) is not the one the cells were made from until they
/// are made again.
pub(crate) fn cell_ids_made(st: &State) -> Vec<Option<u64>> {
    let n = st.cells.len();
    let (Some(frames), Some(listed)) = (&st.views.collection_frames, &st.views.queue_listed) else {
        return vec![None; n];
    };
    if st.views.view != View::Queue {
        return vec![None; n];
    }
    let kept = crate::versions::frames_kept(&st.files, &st.sidecars, frames, false);
    if kept.len() != n || kept.iter().zip(&st.cells).any(|((c, _), cell)| c != cell) {
        return vec![None; n];
    }
    kept.iter()
        .map(|(_, i)| listed.ids.get(*i).copied())
        .collect()
}

/// Where each cell's queued frame is in the queue now; none for a cell
/// whose frame has left it since it was listed.
pub(crate) fn refs_of_cells(st: &State) -> Vec<Option<Ref>> {
    let at = where_is(&st.work.export_queue);
    ids_of_cells(st)
        .into_iter()
        .map(|id| {
            let id = id?;
            let &(entry, frame) = at.get(&id)?;
            Some(Ref { entry, frame, id })
        })
        .collect()
}

/// The queued frame cell `c` is, in the queue's view.
pub(crate) fn frame_of(st: &State, c: usize) -> Option<&QueuedFrame> {
    let r = refs_of_cells(st).get(c).copied().flatten()?;
    st.work.export_queue.get(r.entry)?.frames.get(r.frame)
}

/// The edit cell `c` opens under on the panel: its queued edit in the
/// queue's view, with its turn and seed kept for the develop
/// (`State::views.queued_open`); none, and nothing kept, for any other
/// frame, which opens under its sidecar's.
pub(crate) fn open_under(st: &mut State, c: usize) -> Option<greycard_edit::Edit> {
    let q = if frozen(st) {
        frame_of(st, c).cloned()
    } else {
        None
    };
    st.views.queued_open = q.as_ref().map(|q| QueuedOpen {
        turn: q.turn,
        seed: q.seed_blend,
    });
    q.map(|q| q.edit)
}

/// The queue's entries or frames changed: the left pane's count
/// follows, and the view, when it is the queue's, is listed again in
/// place (by the event loop, after what changed it is done; a test
/// lists it again itself).
pub(crate) fn changed(st: &mut State, app: &App) {
    show_count(st, app);
    if st.views.view != View::Queue || cfg!(test) || st.views.queue_relist_due {
        return;
    }
    st.views.queue_relist_due = true;
    let app_weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let (Some(app), Some(state), Some(worker)) = (
            app_weak.upgrade(),
            crate::STATE.with(|s| s.borrow().clone()),
            crate::WORKER.with(|w| w.borrow().clone()),
        ) else {
            return;
        };
        relist(&state, &app, &worker);
    });
}

/// The queue's view listed again from the queue as it is, in place: each
/// cell whose frame is still queued keeps its picture, its place in the
/// selection, and the frame on screen stays on screen, by the frame's
/// id, so a frame queued twice keeps its own copy. Nothing is opened,
/// no picture is forgotten, and the status line is left as it is. A
/// frame on screen that has left the queue gives its place to the
/// nearest one left, which is opened, the status line kept. A queue
/// with a file the list does not have yet (a set added) is opened
/// again as a view is.
pub(crate) fn relist(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    let reopen = {
        let mut st = state.borrow_mut();
        st.views.queue_relist_due = false;
        if st.views.view != View::Queue {
            return;
        }
        let (keys, listed) = listing(&st);
        let have: std::collections::HashSet<&PathBuf> = st.files.iter().collect();
        if keys.iter().all(|k| have.contains(&k.path)) {
            in_place(&mut st, app, worker, keys, listed);
            false
        } else {
            true
        }
    };
    if reopen {
        crate::roots::open_view(state, app, worker, View::Queue);
    }
}

/// [`relist`] over the files the list has.
fn in_place(st: &mut State, app: &App, worker: &Worker, keys: Vec<Key>, listed: Listed) {
    st.views.collection_frames = Some(keys);
    st.views.queue_listed = Some(listed);
    let had = st.current;
    crate::versions::relist(st, app, worker, &Some);
    let next = crate::panel::browser::rebuild_browser(st, app);
    if had.is_some() && st.current.is_none() {
        let status = app.get_status();
        match next.or((!st.views.shown.is_empty()).then_some(0)) {
            Some(row) => crate::panel::browser::open_row(st, app, worker, row as i32, false),
            None => {
                app.set_selected(-1);
                crate::panel::browser::show_file_name(st, app, None);
            }
        }
        app.set_status(status);
    }
}

/// The left pane's row: how many frames are queued, and whether the
/// queue's view is open.
pub(crate) fn show_count(st: &State, app: &App) {
    app.set_queue_view_count(export_queue::frame_count(&st.work.export_queue) as i32);
    app.set_queue_view_on(st.views.view == View::Queue);
    app.set_frozen(frozen(st));
}

/// The grid's headings and the tiles' marks, for the rows as the list
/// shows them now: after every rebuild of the rows.
pub(crate) fn show(st: &mut State, app: &App) {
    show_count(st, app);
    let refs = refs_of_cells(st);
    let mut heads: Vec<Head> = Vec::new();
    if st.views.view == View::Queue {
        let entries = &st.work.export_queue;
        let mut group_of = vec![0; entries.len()];
        let mut texts = Vec::new();
        for (k, (text, group)) in groups(entries).into_iter().enumerate() {
            for e in group {
                group_of[e] = k;
            }
            texts.push(text);
        }
        let mut last: Option<usize> = None;
        for (row, &c) in st.views.shown.iter().enumerate() {
            let Some(r) = refs.get(c).copied().flatten() else {
                continue;
            };
            let Some(&g) = group_of.get(r.entry) else {
                continue;
            };
            if last != Some(g) {
                heads.push(Head {
                    row,
                    text: texts[g].clone(),
                    count: 0,
                });
                last = Some(g);
            }
            if let Some(h) = heads.last_mut() {
                h.count += 1;
            }
        }
    }
    if heads != st.views.grid_heads {
        let rows: Vec<i32> = heads.iter().map(|h| h.row as i32).collect();
        let shown: Vec<GridHead> = heads
            .iter()
            .map(|h| GridHead {
                text: h.text.as_str().into(),
                count: h.count as i32,
                row: h.row as i32,
            })
            .collect();
        app.set_grid_head_rows(ModelRc::new(VecModel::from(rows)));
        app.set_grid_heads(ModelRc::new(VecModel::from(shown)));
        st.views.grid_heads = heads;
    }
    marks(st, app);
}

/// The tiles' marks while the queue runs: the frames of the running
/// entry written so far (the exported mark), and the one the worker
/// said it began and has not finished (the running bar). The rest, and
/// every tile outside the queue's view, carry none.
pub(crate) fn marks(st: &State, app: &App) {
    let refs = refs_of_cells(st);
    let run = st.work.queue_run.as_ref();
    let busy = run.and(st.work.queue_busy);
    let model = app.get_thumbs();
    for (row, &c) in st.views.shown.iter().enumerate() {
        let r = refs.get(c).copied().flatten();
        let done = r.is_some_and(|r| {
            run.is_some_and(|run| {
                run.entry == r.entry && run.done.get(r.frame).copied().unwrap_or(false)
            })
        });
        let running = r.is_some_and(|r| busy == Some(r.id));
        let Some(mut t) = model.row_data(row) else {
            continue;
        };
        if t.queue_done != done || t.queue_busy != running {
            t.queue_done = done;
            t.queue_busy = running;
            model.set_row_data(row, t);
        }
    }
    app.set_queue_busy(st.views.view == View::Queue && busy.is_some());
}

/// Frame `index` of the queue's running `set` was begun by the worker:
/// its tile's running bar, until it is done with.
pub(crate) fn began(st: &mut State, app: &App, set: &Arc<crate::queue::Set>, index: usize) {
    st.work.queue_busy = crate::panel::export_queue::id_in_run(st, set, index);
    marks(st, app);
}

/// Delete, or the frame menu's Remove from queue..., in the queue's
/// view: the chosen tiles' frames to be taken out of the queue, asked
/// first. Their files are not touched.
pub(crate) fn ask_out(st: &mut State, app: &App) {
    let ids = ids_of_cells(st);
    let out: Vec<u64> = crate::panel::browser::chosen_frames(st)
        .into_iter()
        .filter_map(|c| ids.get(c).copied().flatten())
        .collect();
    if out.is_empty() {
        tracing::info!("export queue: nothing chosen to take out");
        return;
    }
    crate::panel::collections::ask_queue_out(st, app, out);
}

/// Take the queued frames `out` (by id) out of their entries, where
/// they are now: one that has left the queue since (its set exported)
/// is passed over, said only in the log; an entry left with none is
/// dropped, the file is written, and the run, when one is going, kept
/// on its own entry. The running entry's frames are not taken out: the
/// run counts them by place.
pub(crate) fn take_out(st: &mut State, app: &App, out: &[u64]) {
    let at = where_is(&st.work.export_queue);
    let running = st.work.queue_run.as_ref().map(|r| r.entry);
    let mut by_entry: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    let (mut kept, mut gone) = (0, 0);
    for id in out {
        match at.get(id) {
            None => gone += 1,
            Some(&(e, _)) if Some(e) == running => kept += 1,
            Some(&(e, f)) => by_entry.entry(e).or_default().push(f),
        }
    }
    if kept > 0 {
        tracing::info!(
            "export queue: {} of the set being exported left in",
            crate::queue::frames(kept)
        );
    }
    if gone > 0 {
        tracing::info!(
            "export queue: {} had left the queue already",
            crate::queue::frames(gone)
        );
    }
    let mut taken = 0;
    // From the last entry back, so the places before stay as they were.
    for (&e, frames) in by_entry.iter_mut().rev() {
        frames.sort_unstable();
        frames.dedup();
        let Some(entry) = st.work.export_queue.get_mut(e) else {
            continue;
        };
        for &f in frames.iter().rev() {
            entry.frames.remove(f);
            taken += 1;
        }
        if entry.frames.is_empty() {
            st.work.export_queue.remove(e);
            if let Some(run) = st.work.queue_run.as_mut()
                && e < run.entry
            {
                run.entry -= 1;
            }
        }
    }
    if taken == 0 {
        return;
    }
    crate::panel::export_queue::keep(st);
    crate::panel::export_queue::show_queue(st, app);
    tracing::info!(
        "export queue: {} taken out; {}",
        crate::queue::frames(taken),
        export_queue::count_line(&st.work.export_queue)
    );
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    app.set_grid_head_h(crate::grid::HEAD);
    show_count(&state.borrow(), app);
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_queue_view_picked(move || {
            if let Some(app) = app_weak.upgrade() {
                crate::roots::open_view(&state, &app, &worker, View::Queue);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_queue_menu_remove(move || {
            if let Some(app) = app_weak.upgrade() {
                ask_out(&mut state.borrow_mut(), &app);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export_queue::Entry;
    use crate::sheet::Sheet;
    use greycard_edit::{Edit, Sidecar};

    struct Setup {
        dir: PathBuf,
        files: Vec<PathBuf>,
        app: App,
        state: Rc<RefCell<State>>,
        worker: Rc<Worker>,
    }

    /// A folder of `n` frames (empty files: nothing decodes them), each
    /// with a sidecar whose exposure is a tenth of its number, and a
    /// window with the grid up and the queue's file in the folder.
    fn setup(tag: &str, n: usize) -> Setup {
        let dir = crate::testing::scratch_dir(&format!("qv-{tag}"));
        let files: Vec<PathBuf> = (0..n)
            .map(|i| {
                let f = dir.join(format!("IMG_{i:04}.CR3"));
                std::fs::write(&f, b"").unwrap();
                let mut s = Sidecar::default();
                let mut e = Edit::default();
                e.light.exposure = 0.1 * i as f32;
                s.record(e);
                s.save(&f).unwrap();
                f
            })
            .collect();
        let app = crate::testing::window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1400.0, 900.0));
        app.set_grid_open(true);
        let (state, worker) = crate::testing::state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.work.export_queue_file = Some(dir.join("config").join(export_queue::FILE));
        }
        Setup {
            dir,
            files,
            app,
            state,
            worker,
        }
    }

    fn teardown(s: Setup) {
        let Setup {
            dir,
            app,
            state,
            worker,
            ..
        } = s;
        drop(app);
        drop(worker);
        crate::testing::remove_scratch(state, &dir);
    }

    /// An entry of `frames` to `folder` (none for beside each original)
    /// under `format`, each frame's queued exposure one and a tenth of
    /// its number, its turn one more than its number, a third round.
    fn entry(s: &Setup, frames: &[usize], folder: Option<&str>, format: &str) -> Entry {
        Entry {
            queued: 1_790_000_000,
            folder: folder.map(PathBuf::from),
            preset: None,
            sheet: Sheet {
                format: format.into(),
                ..Sheet::default()
            },
            frames: frames
                .iter()
                .map(|&i| {
                    let mut edit = Edit::default();
                    edit.light.exposure = 1.0 + 0.1 * i as f32;
                    QueuedFrame {
                        source: s.files[i].clone(),
                        edit,
                        turn: (i % 3) as u8 + 1,
                        seed_blend: false,
                        version: None,
                        id: export_queue::fresh_id(),
                    }
                })
                .collect(),
        }
    }

    fn queue(s: &Setup, entries: Vec<Entry>) {
        let mut st = s.state.borrow_mut();
        st.work.export_queue = entries;
        crate::panel::export_queue::keep(&st);
        changed(&mut st, &s.app);
    }

    fn open(s: &Setup) {
        crate::roots::open_view(&s.state, &s.app, &s.worker, View::Queue);
        crate::roots::land_sent(&s.state, &s.app, &s.worker);
    }

    /// The frames listed, by their number in the folder, in the grid's
    /// order.
    fn listed(s: &Setup) -> Vec<usize> {
        let st = s.state.borrow();
        st.views
            .shown
            .iter()
            .map(|&c| s.files.iter().position(|f| f == st.path_of(c)).unwrap())
            .collect()
    }

    fn heads(s: &Setup) -> Vec<(String, i32, i32)> {
        s.app
            .get_grid_heads()
            .iter()
            .map(|h| (h.text.to_string(), h.count, h.row))
            .collect()
    }

    /// The sidecars on disk, every byte: what the queue's view must
    /// never change.
    fn sidecars_on_disk(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut all = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p != dir.join("config") {
                        stack.push(p);
                    }
                } else if p.extension().is_none_or(|x| x != "CR3") {
                    all.push((p.clone(), std::fs::read(&p).unwrap()));
                }
            }
        }
        all.sort();
        all
    }

    /// The id of the queued frame on screen.
    fn current_id(s: &Setup) -> Option<u64> {
        let st = s.state.borrow();
        ids_of_cells(&st).get(st.current?).copied().flatten()
    }

    /// Every frame of two entries in the queue's order, under a heading
    /// for each destination; a frame queued twice is two tiles, and the
    /// strip, which reads the same rows, has them flat in that order.
    /// Entries whose sheet and folder say the same heading are one.
    #[test]
    fn the_queue_view_lists_every_frame_under_its_destination() {
        let s = setup("list", 3);
        let (web, beside) = (
            entry(&s, &[0, 1], Some("/out/web"), "JPEG"),
            entry(&s, &[2, 0], None, "PNG"),
        );
        let (web_said, beside_said) = (heading(&web), heading(&beside));
        assert!(web_said.starts_with("JPEG") && web_said.ends_with(", to /out/web"));
        assert!(beside_said.starts_with("PNG") && beside_said.ends_with(", beside each original"));
        queue(&s, vec![web, beside]);
        assert_eq!(s.app.get_queue_view_count(), 4);
        open(&s);
        assert_eq!(s.state.borrow().views.view, View::Queue);
        assert!(s.app.get_queue_view_on());
        assert!(s.app.get_frozen(), "the view is not for editing");
        assert_eq!(listed(&s), [0, 1, 2, 0]);
        assert_eq!(
            heads(&s),
            [(web_said.clone(), 2, 0), (beside_said.clone(), 2, 2)]
        );
        let rows: Vec<i32> = s.app.get_grid_head_rows().iter().collect();
        assert_eq!(rows, [0, 2]);
        // The strip's rows: flat, in the queue's order, no heading
        // among them.
        let strip: Vec<String> = s
            .app
            .get_thumbs()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(
            strip,
            [
                "IMG_0000.CR3",
                "IMG_0001.CR3",
                "IMG_0002.CR3",
                "IMG_0000.CR3"
            ]
        );
        // Down keeps the column: from the first run's second tile to
        // the second run's second, over the heading.
        s.app.invoke_select(1);
        s.app.invoke_grid_step(0, 1, 8, false);
        assert_eq!(s.app.get_selected(), 3);
        s.app.invoke_grid_step(0, -1, 8, false);
        assert_eq!(s.app.get_selected(), 1);
        // One column across: the last tile above the heading, and down
        // lands on the first below it.
        s.app.invoke_grid_step(0, 1, 1, false);
        assert_eq!(s.app.get_selected(), 2);
        // The Sort menu offers the queue's own order alone.
        assert_eq!(crate::sort::entries(&View::Queue), [None]);

        // The same sheet and folder: one heading, the entries' frames in
        // the queue's order under it.
        queue(
            &s,
            vec![
                entry(&s, &[0, 1], Some("/out/web"), "JPEG"),
                entry(&s, &[2], None, "PNG"),
                entry(&s, &[2], Some("/out/web"), "JPEG"),
            ],
        );
        relist(&s.state, &s.app, &s.worker);
        crate::roots::land_sent(&s.state, &s.app, &s.worker);
        assert_eq!(listed(&s), [0, 1, 2, 2]);
        assert_eq!(heads(&s), [(web_said, 3, 0), (beside_said, 1, 3)]);
        teardown(s);
    }

    /// Delete over the chosen tiles asks, then takes those frames out of
    /// their entries, an emptied entry dropped, and writes the file; the
    /// files themselves are left. The frame menu's Remove from queue...
    /// is the right-clicked tile's when it is not in the selection.
    #[test]
    fn delete_takes_the_chosen_frames_out_of_the_queue() {
        let s = setup("out", 3);
        queue(
            &s,
            vec![
                entry(&s, &[0, 1], Some("/out/web"), "JPEG"),
                entry(&s, &[2], None, "PNG"),
            ],
        );
        open(&s);
        s.app.invoke_select(1);
        s.state.borrow_mut().views.picked = vec![1, 2];
        s.app.invoke_delete_asked("selection".into());
        assert!(s.app.get_collection_sheet_open());
        assert_eq!(
            s.app.get_collection_sheet_title(),
            "Take 2 frames out of the queue?"
        );
        s.app.invoke_collection_sheet_done();
        {
            let st = s.state.borrow();
            assert_eq!(
                st.work.export_queue.len(),
                1,
                "the emptied entry is dropped"
            );
            let left: Vec<&PathBuf> = st.work.export_queue[0]
                .frames
                .iter()
                .map(|f| &f.source)
                .collect();
            assert_eq!(left, [&s.files[0]]);
            let file = st.work.export_queue_file.clone().unwrap();
            assert_eq!(export_queue::load(&file), st.work.export_queue);
        }
        assert!(s.files.iter().all(|f| f.exists()), "no file is touched");
        assert_eq!(listed(&s), [0]);
        assert_eq!(s.app.get_export_queue_line(), "1 set, 1 frame queued");
        assert_eq!(s.app.get_queue_view_count(), 1);

        // The menu over a tile outside the selection: that tile alone.
        queue(
            &s,
            vec![
                entry(&s, &[0, 1], Some("/out/web"), "JPEG"),
                entry(&s, &[2], None, "PNG"),
            ],
        );
        relist(&s.state, &s.app, &s.worker);
        crate::roots::land_sent(&s.state, &s.app, &s.worker);
        s.app.invoke_select(0);
        s.app.invoke_frame_menu_asked(2);
        s.app.invoke_queue_menu_remove();
        assert_eq!(
            s.app.get_collection_sheet_title(),
            "Take 1 frame out of the queue?"
        );
        s.app.invoke_collection_sheet_done();
        assert_eq!(listed(&s), [0, 1]);
        assert_eq!(s.state.borrow().work.export_queue.len(), 1);
        teardown(s);
    }

    /// A removal while the queue runs keeps the run on its own entry: an
    /// entry emptied before it moves it up, and its own frames are not
    /// taken out. A frame that left the queue while the confirm was up
    /// is passed over, and the rest still go.
    #[test]
    fn a_running_queue_keeps_its_entry_through_a_removal() {
        let s = setup("run", 3);
        queue(
            &s,
            vec![
                entry(&s, &[0], None, "JPEG"),
                entry(&s, &[1], None, "PNG"),
                entry(&s, &[2, 1], None, "TIFF"),
            ],
        );
        let mut st = s.state.borrow_mut();
        st.work.queue_run = Some(crate::panel::export_queue::Run {
            entry: 2,
            set: None,
            at: Vec::new(),
            done: vec![false; 2],
            left_behind: 0,
            whole: crate::panel::export_queue::Whole::new(),
            probe: 0,
            sent: Vec::new(),
        });
        let id = |st: &State, e: usize, f: usize| st.work.export_queue[e].frames[f].id;
        let (first, second, running) = (id(&st, 0, 0), id(&st, 1, 0), id(&st, 2, 0));
        take_out(&mut st, &s.app, &[first, running]);
        assert_eq!(st.work.export_queue.len(), 2);
        assert_eq!(st.work.queue_run.as_ref().unwrap().entry, 1);
        assert_eq!(
            st.work.export_queue[1].frames.len(),
            2,
            "the running entry is whole"
        );
        // One gone already (taken out above), one still there: the one
        // still there goes.
        take_out(&mut st, &s.app, &[first, second]);
        assert_eq!(st.work.export_queue.len(), 1);
        assert_eq!(st.work.queue_run.as_ref().unwrap().entry, 0);
        drop(st);
        teardown(s);
    }

    /// The Loupe on a queued frame develops the edit it was queued with
    /// at its queued turn, not its sidecar's; each edit key there, and
    /// in the grid, changes nothing and writes nothing; leaving the
    /// queue's view brings the sidecar's edit back.
    #[test]
    fn the_queue_loupe_is_the_queued_edit_and_frozen() {
        let s = setup("loupe", 2);
        queue(&s, vec![entry(&s, &[1], None, "JPEG")]);
        let before = sidecars_on_disk(&s.dir);
        open(&s);
        s.app.set_grid_open(false);
        s.app.invoke_select(0);
        {
            let mut st = s.state.borrow_mut();
            assert_eq!(st.current, Some(0));
            assert!(frozen(&st));
            assert!((st.develop.edit.light.exposure - 1.1).abs() < 1e-6);
            let held = st.develop.edit.clone();
            let job = crate::panel::browser::open_job(&mut st, 0, held);
            let crate::worker::Job::Open { edit, turn, .. } = job else {
                panic!("an open");
            };
            assert!((edit.light.exposure - 1.1).abs() < 1e-6);
            assert_eq!(turn, 2);
            assert_eq!(crate::panel::edit::current_turn(&st), 2);
        }
        assert!(s.app.get_frozen());
        assert!((s.app.get_exposure() - 1.1).abs() < 1e-6);
        let unmoved = |s: &Setup| {
            let st = s.state.borrow();
            let f = st.file_of(0);
            assert_eq!(st.sidecars[f].turn, 0);
            assert!(st.sidecars[f].versions.is_empty());
            assert_eq!(st.sidecars[f].history.len(), 1);
            assert!((st.sidecars[f].current.light.exposure - 0.1).abs() < 1e-6);
            assert!((st.develop.edit.light.exposure - 1.1).abs() < 1e-6);
        };
        // A turn.
        crate::testing::press(&s.app, "]");
        unmoved(&s);
        // A paste, with settings on the clipboard (a copy reads, and is
        // allowed): no sheet, nothing laid on.
        s.app.invoke_copy_asked();
        assert!(s.state.borrow().sheets.clipboard.is_some());
        s.app.invoke_paste_asked();
        assert!(!s.app.get_sync_open());
        unmoved(&s);
        // An undo with nothing to undo, and a redo.
        s.app.invoke_undo();
        s.app.invoke_redo();
        unmoved(&s);
        // A new version, by its key and by the menu.
        s.app.invoke_version_new_asked(false, false);
        s.app.invoke_version_new_asked(true, true);
        unmoved(&s);
        // The same keys over the grid.
        s.app.set_grid_open(true);
        crate::testing::press(&s.app, "[");
        unmoved(&s);
        s.app.invoke_undo();
        unmoved(&s);
        s.app.set_grid_open(false);
        assert_eq!(sidecars_on_disk(&s.dir), before, "no sidecar written");
        // Leaving the view: the frame under its sidecar's edit again.
        crate::roots::open_listing(
            &s.state,
            &s.app,
            &s.worker,
            crate::roots::Listing::Files(s.files.clone()),
            crate::roots::Select::Row(1),
        );
        crate::roots::land_sent(&s.state, &s.app, &s.worker);
        s.app.invoke_select(1);
        {
            let st = s.state.borrow();
            assert!(!frozen(&st));
            assert_eq!(st.views.view, View::Folder);
            assert!((st.develop.edit.light.exposure - 0.1).abs() < 1e-6);
            assert!(st.views.grid_heads.is_empty());
        }
        assert!(!s.app.get_frozen());
        assert_eq!(sidecars_on_disk(&s.dir), before, "leaving wrote nothing");
        // Out of the view the same key turns the frame: what stopped it
        // above was the view, not the key.
        crate::testing::press(&s.app, "]");
        let st = s.state.borrow();
        assert_eq!(st.sidecars[st.file_of(1)].turn, 1);
        drop(st);
        teardown(s);
    }

    /// A slider moved on a folder's frame and the Export queue row
    /// pressed at once: the change is kept under the folder's view it
    /// was made in, and is there when the frame is opened again.
    #[test]
    fn a_change_made_just_before_the_queue_view_opens_is_kept() {
        let s = setup("leave", 2);
        queue(&s, vec![entry(&s, &[1], None, "JPEG")]);
        crate::roots::open_listing(
            &s.state,
            &s.app,
            &s.worker,
            crate::roots::Listing::Files(s.files.clone()),
            crate::roots::Select::Row(0),
        );
        crate::roots::land_sent(&s.state, &s.app, &s.worker);
        s.app.set_grid_open(false);
        s.app.invoke_select(0);
        s.app.set_exposure(0.7);
        s.app.invoke_queue_view_picked();
        crate::roots::land_sent(&s.state, &s.app, &s.worker);
        assert_eq!(s.state.borrow().views.view, View::Queue);
        let on_disk = Sidecar::load(&s.files[0]).unwrap().unwrap();
        assert!((on_disk.current.light.exposure - 0.7).abs() < 1e-6);
        crate::roots::open_listing(
            &s.state,
            &s.app,
            &s.worker,
            crate::roots::Listing::Files(s.files.clone()),
            crate::roots::Select::Row(0),
        );
        crate::roots::land_sent(&s.state, &s.app, &s.worker);
        s.app.invoke_select(0);
        assert!((s.state.borrow().develop.edit.light.exposure - 0.7).abs() < 1e-6);
        assert!((s.app.get_exposure() - 0.7).abs() < 1e-6);
        teardown(s);
    }

    /// A merge read of the queue's view (an index pass over its folder)
    /// keeps a frame queued twice on its own copy, as a relist does.
    #[test]
    fn a_merge_read_keeps_a_duplicate_on_its_own_copy() {
        let s = setup("merge", 2);
        queue(
            &s,
            vec![
                entry(&s, &[0, 1], Some("/out/web"), "JPEG"),
                entry(&s, &[0], None, "PNG"),
            ],
        );
        open(&s);
        // The first copy on screen, and both copies chosen.
        s.app.invoke_select(0);
        s.state.borrow_mut().views.picked = vec![0, 2];
        let first = current_id(&s).unwrap();
        let second = ids_of_cells(&s.state.borrow())[2].unwrap();
        // The queue's own entries reordered under it, as a merge read
        // lands on a changed listing: the PNG set first.
        s.state.borrow_mut().work.export_queue.swap(0, 1);
        assert!(crate::roots::refresh_view(&s.state, &s.app, &s.worker));
        assert!(crate::roots::land_sent(&s.state, &s.app, &s.worker) > 0);
        assert_eq!(listed(&s), [0, 0, 1]);
        assert_eq!(
            current_id(&s),
            Some(first),
            "the first copy, not the second"
        );
        assert_eq!(s.app.get_selected(), 1);
        let st = s.state.borrow();
        let ids = ids_of_cells(&st);
        let picked: Vec<Option<u64>> = st.views.picked.iter().map(|&c| ids[c]).collect();
        assert!(picked.contains(&Some(first)) && picked.contains(&Some(second)));
        drop(st);
        teardown(s);
    }

    /// Cull from the queue's Loupe and back onto another frame: that
    /// frame comes out under its queued edit and turn, read-only.
    #[test]
    fn a_cull_round_trip_lands_on_the_queued_edit() {
        let s = setup("cull", 2);
        queue(&s, vec![entry(&s, &[0, 1], None, "JPEG")]);
        let before = sidecars_on_disk(&s.dir);
        open(&s);
        s.app.set_grid_open(false);
        s.app.invoke_select(0);
        s.app.invoke_cull_toggled();
        assert!(s.state.borrow().culling.cull.is_some());
        s.app.invoke_step(1, false);
        s.app.invoke_cull_leave();
        {
            let st = s.state.borrow();
            assert!(st.culling.cull.is_none());
            assert_eq!(st.current, Some(1));
            assert!((st.develop.edit.light.exposure - 1.1).abs() < 1e-6);
            assert_eq!(crate::panel::edit::current_turn(&st), 2);
            assert!(frozen(&st));
        }
        crate::testing::press(&s.app, "]");
        assert_eq!(sidecars_on_disk(&s.dir), before, "no sidecar written");
        teardown(s);
    }

    /// The queue changed under a run: the view is listed again in place,
    /// the frame on screen and the set kept by the frame each tile is (a
    /// frame queued twice keeps its own copy), and the status line as it
    /// was; a queue emptied by the run leaves the finished line alone.
    #[test]
    fn a_relist_mid_run_keeps_the_selection_and_the_status() {
        let s = setup("relist", 2);
        queue(
            &s,
            vec![
                entry(&s, &[0, 1], Some("/out/web"), "JPEG"),
                entry(&s, &[0], None, "PNG"),
            ],
        );
        open(&s);
        // The second copy of frame 0, and frame 1, chosen.
        s.app.invoke_select(2);
        s.state.borrow_mut().views.picked = vec![1, 2];
        let dup = current_id(&s).unwrap();
        let other = ids_of_cells(&s.state.borrow())[1].unwrap();
        s.app.set_status("exporting 1 of 3: IMG_0000.jpg".into());
        // The first frame's set written and done with.
        {
            let mut st = s.state.borrow_mut();
            st.work.export_queue[0].frames.remove(0);
            changed(&mut st, &s.app);
        }
        relist(&s.state, &s.app, &s.worker);
        assert_eq!(listed(&s), [1, 0]);
        assert_eq!(current_id(&s), Some(dup), "the second copy, not the first");
        assert_eq!(s.app.get_selected(), 1);
        {
            let st = s.state.borrow();
            let ids = ids_of_cells(&st);
            let picked: Vec<Option<u64>> = st.views.picked.iter().map(|&c| ids[c]).collect();
            assert!(picked.contains(&Some(dup)) && picked.contains(&Some(other)));
        }
        assert_eq!(s.app.get_status(), "exporting 1 of 3: IMG_0000.jpg");
        // The run empties the queue: no "nothing queued" over its line.
        s.app
            .set_status("exported 3 frames in 2.0 s; the queue is done".into());
        {
            let mut st = s.state.borrow_mut();
            st.work.export_queue.clear();
            changed(&mut st, &s.app);
        }
        relist(&s.state, &s.app, &s.worker);
        assert!(listed(&s).is_empty());
        assert_eq!(
            s.app.get_status(),
            "exported 3 frames in 2.0 s; the queue is done"
        );
        teardown(s);
    }
}
