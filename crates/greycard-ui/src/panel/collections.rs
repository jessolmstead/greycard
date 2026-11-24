//! The collections in the window (`crate::collections` keeps them):
//! the left pane's list beside the roots and the folders, Add to
//! collection and Remove in the frame menu, B for the selection, a drag
//! in the grid for a collection's own order, the filter bar's Save...
//! for a smart one, and the mark on a member's cell.
//!
//! A collection opens as a view of the index, as a root does
//! (`roots::View::Collection`); a smart one opens the view of its root,
//! or of every root, with its filter on the bar, and is lit in the list
//! while the bar still says that filter there.
//!
//! A frame is a version of a picture: the file's own, or one of the
//! versions its sidecar keeps. Add, B, Remove and a drag act on the
//! chosen cells' (hash, version) pairs; a collection open lists its
//! frames as cells in its order (two versions of one file are two
//! tiles); the mark is on a cell whose own pair is in a collection.
//! A version made while a collection is the list goes into it beside
//! the frame it was made from.
//!
//! B goes to the collection open in the browser, else to the last one
//! added to or made this session; with neither it asks for a new one.
//! Like Lightroom's quick collection key it takes the selection out
//! when every frame of it is in already.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use greycard_library::Rehash;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::collections::{Collection, Frame, Pair, Smart, full_name};
use crate::filter::{Saved, SavedBar};
use crate::panel::browser::chosen_frames;
use crate::roots::View;
use crate::worker::Worker;
use crate::{App, CollectionChip, CollectionChoice, Shared, State, tr};

const QUICK_COLLECTIONS: usize = 10;

/// The collections as the window shows them, beside the store's: the
/// sidebar's rows and the frame menu's names, one model each for the
/// session, and the sheet's question while it is up.
#[derive(Default)]
pub(crate) struct Held {
    /// The sidebar's rows, replaced only when they change.
    pub(crate) rows: Rc<VecModel<CollectionChip>>,
    /// Names in the full collection picker, in the store's order.
    pub(crate) names: Rc<VecModel<slint::SharedString>>,
    /// The destinations in the frame menu, most recently used first.
    quick: Rc<VecModel<CollectionChoice>>,
    /// The sheet's question while it is up.
    pub(crate) asked: Option<Asked>,
}

/// What the collection sheet is asking, while it is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Asked {
    /// A name for a new collection, these frames to go into it.
    New { add: Vec<Frame> },
    /// A name for the filter, kept as a smart collection under `root`,
    /// or every root.
    Smart {
        filter: Saved,
        root: Option<PathBuf>,
    },
    /// A new name for a collection or a smart one, by its id.
    Rename(String),
    /// Whether to delete one, by its id.
    Delete(String),
    /// A name for version `id` of the file at `path`: the sheet lent to
    /// the versions (`panel::versions`), which have no sets.
    Version { path: PathBuf, id: String },
    /// Whether to take these queued frames (by id) out of the export
    /// queue: the sheet lent to the queue's view.
    QueueOut { out: Vec<u64> },
}

/// Ask whether to take the queued frames `out` (by id) out of the
/// export queue, on the sheet a collection's delete asks on.
pub(crate) fn ask_queue_out(st: &mut State, app: &App, out: Vec<u64>) {
    ask(st, app, Asked::QueueOut { out });
}

/// "Cover picks", or "Clients/2026/Cover picks" for one in sets.
pub(crate) fn name_of(st: &State, id: &str) -> String {
    if let Some(c) = st.views.collections.collection(id) {
        full_name(&c.set, &c.name)
    } else if let Some(s) = st.views.collections.smart(id) {
        full_name(&s.set, &s.name)
    } else {
        String::new()
    }
}

/// The list in the left pane and what hangs off it: the menu's names,
/// the collection open, and whether the grid takes a drag.
pub(crate) fn show(st: &State, app: &App) {
    let open = st.views.view.collection();
    // Each collection's order as kept: its own (none), or one chosen,
    // which the row names while it is open and its menu checks.
    let order = |id: &str| {
        st.views
            .sort
            .choice(&View::Collection { id: id.to_string() })
            .order()
    };
    let hand = st.views.sort.choice(&st.views.view) == crate::sort::Choice::Hand;
    // Compared as loaded: a smart collection an older build kept with
    // ISO or focal chips loads as the range's terms, and its file is
    // not rewritten, so the shape on disk is not what the bar keeps.
    let smart_on = |s: &Smart| {
        st.views.filter == s.filter.filter() && st.views.view == smart_view(st, s.root.as_deref())
    };
    let store = &st.views.collections;
    let chips: Vec<CollectionChip> = store
        .list
        .iter()
        .map(|c| {
            let on = open == Some(c.id.as_str());
            CollectionChip {
                id: c.id.as_str().into(),
                name: c.name.as_str().into(),
                set: c.set.as_str().into(),
                count: c.count() as i32,
                on,
                smart: false,
                capture: order(&c.id).is_some(),
                order: order(&c.id)
                    .map(|o| o.by.words().to_lowercase())
                    .unwrap_or_default()
                    .into(),
            }
        })
        .chain(store.smart.iter().map(|s| CollectionChip {
            id: s.id.as_str().into(),
            name: s.name.as_str().into(),
            set: s.set.as_str().into(),
            count: 0,
            on: smart_on(s),
            smart: true,
            capture: false,
            order: Default::default(),
        }))
        .collect();
    // In place, as the roots' rows (`roots::show`): rows made again
    // leave the list empty for a frame, and all under it jumps.
    let held = &st.views.collections_held;
    crate::panel::startup::sync_rows(&held.rows, chips);
    let names: Vec<slint::SharedString> = store
        .list
        .iter()
        .map(|c| full_name(&c.set, &c.name).into())
        .collect();
    let mut quick = Vec::new();
    let recent = store
        .recent
        .iter()
        .filter_map(|id| store.list.iter().position(|c| &c.id == id));
    for i in recent.chain(0..names.len()) {
        if !quick.contains(&i) {
            quick.push(i);
            if quick.len() == QUICK_COLLECTIONS {
                break;
            }
        }
    }
    let choices = quick
        .into_iter()
        .map(|i| CollectionChoice {
            name: names[i].clone(),
            index: i as i32,
        })
        .collect();
    crate::panel::startup::sync_rows(&held.quick, choices);
    if !held.names.iter().eq(names.iter().cloned()) {
        held.names.set_vec(names);
    }
    app.set_collection_open(open.map(|id| name_of(st, id)).unwrap_or_default().into());
    app.set_collection_reorder(open.is_some() && hand);
}

/// The view a smart collection looks over: the root it was saved under,
/// a folder of a root's tree as that folder with the folders in it, or
/// every root (for none, or for a folder no longer under a root).
pub(crate) fn smart_view(st: &State, root: Option<&Path>) -> View {
    let Some(at) = root else {
        return View::Roots(None);
    };
    let roots = &st.views.library.roots;
    if roots.list().iter().any(|r| r == at) {
        return View::Roots(Some(at.to_path_buf()));
    }
    match roots.root_of(at) {
        Some(r) => View::Branch {
            root: r.to_path_buf(),
            folder: at.to_path_buf(),
            deep: true,
        },
        None => View::Roots(None),
    }
}

/// Where a smart collection looks, in words for its sheet.
fn scope_words(st: &State, root: Option<&Path>) -> String {
    match smart_view(st, root) {
        View::Roots(Some(r)) => crate::roots::root_name(&st.views.library.roots, &r),
        View::Branch { folder, .. } => tr!("{} and the folders in it", folder.display()),
        _ => tr!("every root"),
    }
}

/// The index's rows of every member's hash, each with the versions of
/// that picture that are members, asked again: after a collection
/// changed, or the index did. True when they changed.
pub(crate) fn refresh_members(st: &mut State) -> bool {
    let ids = match &st.views.index_reader {
        Some(lib) => {
            let mut by_hash: HashMap<&str, HashSet<Option<String>>> = HashMap::new();
            for (hash, version) in st.views.collections.pairs() {
                by_hash
                    .entry(hash)
                    .or_default()
                    .insert(version.map(str::to_string));
            }
            let mut ids: HashMap<i64, HashSet<Option<String>>> = HashMap::new();
            let mut failed = None;
            for (hash, versions) in by_hash {
                match lib.ids_by_hashes([hash]) {
                    Ok(rows) => {
                        for id in rows {
                            ids.entry(id).or_default().extend(versions.iter().cloned());
                        }
                    }
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            match failed {
                None => ids,
                Some(e) => {
                    tracing::debug!("collections: {e}; the marks kept");
                    st.views.collections.member_ids.clone()
                }
            }
        }
        None => HashMap::new(),
    };
    if ids == st.views.collections.member_ids {
        return false;
    }
    st.views.collections.member_ids = ids;
    true
}

/// Whether cell `c` is a member of a collection: its file's row is of a
/// member's hash, and its own version is among that picture's in a
/// collection. So a version in one marks that version's cell and not
/// its file's own, and the file's own marks no version.
pub(crate) fn collected(st: &State, c: usize) -> bool {
    let Some(cell) = st.cell(c) else {
        return false;
    };
    st.views
        .index_ids
        .get(cell.file)
        .copied()
        .flatten()
        .and_then(|id| st.views.collections.member_ids.get(&id))
        .is_some_and(|versions| versions.contains(&cell.version.as_deref().map(str::to_string)))
}

/// The mark put on every row whose membership changed.
pub(crate) fn show_collected(st: &State, app: &App) {
    let model = app.get_thumbs();
    for (row, &c) in st.views.shown.iter().enumerate() {
        let want = collected(st, c);
        if let Some(mut t) = model.row_data(row)
            && t.collected != want
        {
            t.collected = want;
            model.set_row_data(row, t);
        }
    }
}

/// A collection changed, or the index under it: the marks and the list.
pub(crate) fn members_changed(st: &mut State, app: &App) {
    // The rows' marks are gone over only when the members moved: a
    // list of a hundred thousand is that many rows read.
    if refresh_members(st) {
        show_collected(st, app);
    }
    show(st, app);
}

/// A pass's report, for the marks: asked again only when the pass
/// added, moved, brought back or rewrote rows, which is when a row of a
/// member's hash can have come or gone.
pub(crate) fn report_landed(st: &mut State, app: &App, report: &greycard_library::Report) {
    if report.added + report.moved + report.returned + report.missing > 0
        || !report.rehashed.is_empty()
    {
        members_changed(st, app);
    }
}

/// What the status line says of the collection open: its name and how
/// many of its frames are shown, and missing.
pub(crate) fn opened_words(st: &State, missing: usize) -> Option<String> {
    let id = st.views.view.collection()?;
    let c = st.views.collections.collection(id)?;
    let name = full_name(&c.set, &c.name);
    Some(if c.refs.is_empty() {
        tr!(
            "{name} is empty: add frames with B, or Add to collection in the frame menu",
            name = name
        )
    } else if missing > 0 {
        tr!(
            "{name}: {n} frame shown, {missing} missing (moved or deleted, and kept until found)"
                | "{name}: {n} frames shown, {missing} missing (moved or deleted, and kept until found)"
                    % st.cells.len(),
            name = name,
            missing = missing
        )
    } else {
        tr!(
            "{name}: {n} frame" | "{name}: {n} frames" % st.cells.len(),
            name = name
        )
    })
}

/// The index found files rewritten in place: each collection holding
/// one heals by its hints, and the view of one that did is read again.
pub(crate) fn healed(state: &Rc<Shared>, app: &App, rehashed: &[Rehash]) {
    if rehashed.is_empty() {
        return;
    }
    let viewing = {
        let mut st = state.borrow_mut();
        let st = &mut *st;
        let lib = st.views.index_reader.as_ref();
        // Asked only of a member's hash (`Store::heal`). With no index to
        // ask, or one that will not answer, the copies are not known:
        // nothing is taken for gone, and no hint moved.
        let copies = |hash: &str| -> Option<Vec<PathBuf>> {
            match lib.map(|l| l.rows_by_hash(hash)) {
                Some(Ok(rows)) => Some(rows.into_iter().map(|r| r.path).collect()),
                _ => None,
            }
        };
        let changed = st.views.collections.heal(rehashed, &copies);
        if changed.is_empty() {
            return;
        }
        members_changed(st, app);
        st.views
            .view
            .collection()
            .is_some_and(|id| changed.iter().any(|c| c == id))
    };
    if viewing && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        crate::roots::refresh_view(state, app, &worker);
    }
}

/// Each cell as a collection names it: its file's hash and path, from
/// the file's row in the index, or for a frame the index has not
/// reached, from the file itself (asked once a file), and the cell's
/// version.
pub(crate) fn frames_of(st: &State, cells: &[usize]) -> Vec<Frame> {
    let mut of_file: HashMap<usize, Option<(String, PathBuf)>> = HashMap::new();
    cells
        .iter()
        .filter_map(|&c| {
            let cell = st.cell(c)?;
            let (hash, path) = of_file
                .entry(cell.file)
                .or_insert_with(|| hash_and_path(st, cell.file))
                .clone()?;
            Some((hash, path, cell.version.as_deref().map(str::to_string)))
        })
        .collect()
}

/// File `f`'s hash and path as the index keys them.
fn hash_and_path(st: &State, f: usize) -> Option<(String, PathBuf)> {
    let path = st.files.get(f)?;
    let row = st
        .views
        .index_ids
        .get(f)
        .copied()
        .flatten()
        .and_then(|id| st.views.index_reader.as_ref()?.by_id(id).ok().flatten());
    match row {
        Some(e) => Some((e.hash, e.path)),
        None => match greycard_library::hash_file(path) {
            Ok(h) => Some((h, greycard_library::key_path(path))),
            Err(e) => {
                tracing::warn!("collections: {}: {e}", path.display());
                None
            }
        },
    }
}

/// The frames as a collection's pairs, for a removal or a move.
fn pairs_of(frames: &[Frame]) -> HashSet<Pair<'_>> {
    frames
        .iter()
        .map(|(h, _, v)| (h.as_str(), v.as_deref()))
        .collect()
}

fn remember_destination(st: &mut State, id: &str) {
    let remember = |recent: &mut Vec<String>| {
        recent.retain(|c| c != id);
        recent.insert(0, id.to_string());
        recent.truncate(QUICK_COLLECTIONS);
    };
    match &st.window.settings_file {
        // As for recent folders, keep destinations added by another window.
        Some(path) => {
            st.views.collections.recent =
                crate::settings::Settings::change_at(path, |s| remember(&mut s.recent_collections))
                    .recent_collections;
        }
        None => remember(&mut st.views.collections.recent),
    }
}

/// The selection's frames into collection `id`: new pictures at its end,
/// and another copy of a member moving its hint.
fn add_to(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, id: &str, frames: &[Frame]) {
    let viewing = {
        let mut st = state.borrow_mut();
        let name = name_of(&st, id);
        let Some(c) = st.views.collections.collection_mut(id) else {
            return;
        };
        let added = c.add(frames);
        st.views.collections.save(id);
        st.views.collections.target = Some(id.to_string());
        remember_destination(&mut st, id);
        members_changed(&mut st, app);
        app.set_status(
            match (added.added, added.moved) {
                (0, 0) => tr!("already in {name}", name = name),
                (0, _) => tr!("already in {name}; shown from this copy now", name = name),
                (n, _) => tr!(
                    "{n} frame added to {name}" | "{n} frames added to {name}" % n,
                    name = name
                ),
            }
            .into(),
        );
        tracing::info!(
            "collection {name}: {} added, {} hints moved",
            added.added,
            added.moved
        );
        st.views.view.collection() == Some(id) && added != Default::default()
    };
    if viewing {
        crate::roots::refresh_view(state, app, worker);
    }
}

/// A version just made, `id`, of cell `c`'s frame while a collection is
/// the list: into the collection right after the frame it was made from,
/// so its cell comes in beside it to be opened and named, as it would in
/// a folder. True when it went in.
pub(crate) fn version_made(st: &mut State, app: &App, c: usize, id: &str) -> bool {
    let View::Collection { id: open, .. } = st.views.view.clone() else {
        return false;
    };
    let (Some((hash, path, from)), Some(source)) = (frames_of(st, &[c]).pop(), st.key_of(c)) else {
        return false;
    };
    let frame = (hash.clone(), path, Some(id.to_string()));
    let Some(coll) = st.views.collections.collection_mut(&open) else {
        return false;
    };
    if !coll.insert_after((&hash, from.as_deref()), &frame) {
        return false;
    }
    st.views.collections.save(&open);
    if let Some(frames) = st.views.collection_frames.as_mut() {
        let at = frames
            .iter()
            .position(|k| *k == source)
            .map_or(frames.len(), |i| i + 1);
        frames.insert(at, crate::versions::Key::of(source.path, Some(id)));
    }
    members_changed(st, app);
    true
}

/// The frames `pairs` (each a version of a picture) out of collection
/// `id`; the frames themselves are not touched.
fn remove_from(
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    id: &str,
    pairs: &HashSet<Pair<'_>>,
) {
    let view = {
        let mut st = state.borrow_mut();
        let name = name_of(&st, id);
        let Some(c) = st.views.collections.collection_mut(id) else {
            return;
        };
        let n = c.remove(pairs);
        if n == 0 {
            app.set_status(tr!("not in {name}", name = name).into());
            return;
        }
        st.views.collections.save(id);
        members_changed(&mut st, app);
        app.set_status(
            tr!(
                "{n} frame taken out of {name}" | "{n} frames taken out of {name}" % n,
                name = name
            )
            .into(),
        );
        tracing::info!("collection {name}: {n} taken out");
        (st.views.view.collection() == Some(id)).then(|| st.views.view.clone())
    };
    // Opened again rather than merged: a merge keeps the frame on
    // screen in the list, and a frame taken out of the collection is
    // to leave it.
    if let Some(view) = view {
        crate::roots::open_view(state, app, worker, view);
    }
}

/// Whether the index may yet change its mind about a missing file: the
/// launch's look at the roots out, a launch pass left, or anything asked
/// of the indexer not yet done. A file moved is missing at its old path
/// until the pass reaches the folder it went to, and a rebuilt index has
/// every file missing until its passes are done.
fn indexing(st: &State) -> bool {
    st.views.library.starting
        || st.views.library.launch_left > 0
        || st.views.index.as_ref().is_some_and(|i| {
            let far: Vec<PathBuf> = st
                .views
                .library
                .remote
                .iter()
                .map(|(m, _)| m.clone())
                .collect();
            i.busy(&st.views.library.offline, &far)
        })
}

/// How many references of collection `id` its menu offers to forget:
/// from its last read, while it is open and no pass is under way.
fn forgettable(st: &State, id: &str) -> usize {
    if indexing(st) || st.views.view.collection() != Some(id) {
        return 0;
    }
    match &st.views.collections.missing {
        Some((at, _, gone)) if at == id => *gone,
        _ => 0,
    }
}

/// Forget missing, from the open collection's menu: its references
/// the index has nowhere dropped, read afresh; those hinted under an
/// offline root or a network mount, in a folder that does not answer
/// the editor's look (up to its 3 s), or at a file on disk kept;
/// refused while a pass is under way. The status line is said again
/// with the count that is left.
fn forget_missing(state: &Rc<Shared>, app: &App, id: &str) {
    let mut st = state.borrow_mut();
    let st = &mut *st;
    if st.views.view.collection() != Some(id) {
        return;
    }
    let name = name_of(st, id);
    if indexing(st) {
        app.set_status(
            tr!(
                "the library is being indexed; nothing forgotten from {name} until it is done",
                name = name
            )
            .into(),
        );
        return;
    }
    let away: Vec<PathBuf> = st.views.library.offline.iter().cloned().collect();
    let far: Vec<PathBuf> = st
        .views
        .library
        .remote
        .iter()
        .map(|(m, _)| m.clone())
        .collect();
    // Each folder answers by its root, when it is under one: a handful
    // of looks rather than one a folder.
    let roots = &st.views.library.roots;
    let look = |dirs: &[PathBuf]| {
        let anchor = |d: &PathBuf| {
            roots
                .root_of(d)
                .map_or_else(|| d.clone(), Path::to_path_buf)
        };
        let mut anchors: Vec<PathBuf> = dirs.iter().map(anchor).collect();
        anchors.sort();
        anchors.dedup();
        let answered = crate::roots::answers_of(&anchors, crate::roots::ROOT_WAIT);
        dirs.iter()
            .map(|d| {
                let at = answered.get(&anchor(d)).copied().flatten();
                // A root that answered says nothing of the folder; the
                // file's own stat decides, and keeps it on any doubt. A
                // root that answered no is offline by the roots' own test,
                // though the set the window holds may not say so yet (a
                // drive pulled since the last look): kept, as away.
                let there = match at {
                    Some(false) if roots.root_of(d).is_some() => None,
                    other => other,
                };
                (d.clone(), there)
            })
            .collect()
    };
    let Some(forgot) =
        st.views
            .collections
            .forget_missing(id, st.views.index_reader.as_ref(), &away, &far, &look)
    else {
        app.set_status(
            tr!(
                "the library index could not be read; nothing forgotten from {name}",
                name = name
            )
            .into(),
        );
        return;
    };
    members_changed(st, app);
    let words = opened_words(st, forgot.missing).unwrap_or_default();
    let said = match (forgot.forgotten, forgot.away, forgot.on_disk) {
        (0, 0, 0) if forgot.unsure > 0 => tr!(
            "{words}; nothing forgotten: the sidecar of {n} frame could not be read"
                | "{words}; nothing forgotten: the sidecar of {n} frames could not be read"
                    % forgot.unsure,
            words = words
        ),
        (0, 0, 0) => tr!("{words}; nothing to forget", words = words),
        (0, away, 0) => tr!(
            "{words}; nothing forgotten: {n} frame on a drive that is offline or not answering"
                | "{words}; nothing forgotten: {n} frames on a drive that is offline or not answering"
                    % away,
            words = words
        ),
        (0, _, _) => tr!(
            "{words}; nothing forgotten: the index has yet to read some of them",
            words = words
        ),
        (n, _, _) => tr!("{words}; forgot {n} missing", words = words, n = n),
    };
    app.set_status(said.into());
}

/// The sheet put up with its question.
fn ask(st: &mut State, app: &App, asked: Asked) {
    let (title, note, asks, verb, name) = match &asked {
        Asked::New { add } => (
            tr!("New collection"),
            match add.len() {
                0 => String::new(),
                n => tr!("{n} frame go into it" | "{n} frames go into it" % n),
            },
            true,
            tr!("Create"),
            String::new(),
        ),
        Asked::Smart { filter, root } => (
            tr!("Save the filter as a smart collection"),
            tr!(
                "{}, under {}",
                filter.filter().describe(),
                scope_words(st, root.as_deref())
            ),
            true,
            tr!("Save"),
            String::new(),
        ),
        Asked::Rename(id) => (
            tr!("Rename the collection"),
            tr!("A slash puts it in sets: Clients/2026/Cover"),
            true,
            tr!("Rename"),
            name_of(st, id),
        ),
        Asked::Delete(id) => (
            tr!("Delete {}?", name_of(st, id)),
            tr!("Its file is deleted. The frames in it are not touched."),
            false,
            tr!("Delete"),
            String::new(),
        ),
        Asked::QueueOut { out, .. } => (
            tr!("Take {n} frame out of the queue?"
                | "Take {n} frames out of the queue?" % out.len()),
            String::new(),
            false,
            tr!("Remove"),
            String::new(),
        ),
        // Asked by `panel::versions`, which fills the sheet itself.
        Asked::Version { .. } => return,
    };
    app.set_collection_sheet_title(title.into());
    app.set_collection_sheet_note(note.into());
    app.set_collection_sheet_asks_name(asks);
    app.set_collection_sheet_verb(verb.into());
    app.set_collection_sheet_name(name.into());
    st.views.collections_held.asked = Some(asked);
    app.set_collection_sheet_open(true);
}

/// The sheet answered.
fn sheet_done(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) {
    if !app.get_collection_sheet_open() {
        return;
    }
    let typed = app.get_collection_sheet_name().trim().to_string();
    let named = !crate::collections::split_name(&typed).1.is_empty();
    let mut st = state.borrow_mut();
    let Some(asked) = st.views.collections_held.asked.take() else {
        app.set_collection_sheet_open(false);
        return;
    };
    // A version's name: no sets, and a blank one keeps the sheet up.
    if let Asked::Version { path, id } = asked {
        if typed.is_empty() {
            st.views.collections_held.asked = Some(Asked::Version { path, id });
            return;
        }
        app.set_collection_sheet_open(false);
        app.set_collection_sheet_placeholder(tr!("Name, or Set/Name").into());
        crate::panel::versions::renamed(&mut st, app, &path, &id, &typed);
        return;
    }
    if !named && !matches!(asked, Asked::Delete(_) | Asked::QueueOut { .. }) {
        // Nothing typed: the sheet stays, as it was.
        st.views.collections_held.asked = Some(asked);
        return;
    }
    app.set_collection_sheet_open(false);
    let mut reopen = None;
    let mut relist_queue = false;
    match asked {
        Asked::New { add } => {
            let mut c = Collection::new(&typed);
            while st.views.collections.id_taken(&c.id) {
                c = Collection::new(&typed);
            }
            let added = c.add(&add).added;
            let (id, name) = (c.id.clone(), full_name(&c.set, &c.name));
            st.views.collections.list.push(c);
            st.views.collections.sort();
            st.views.collections.save(&id);
            if added > 0 {
                remember_destination(&mut st, &id);
            }
            st.views.collections.target = Some(id);
            tracing::info!("collection {name} made, {added} frames in it");
            app.set_status(
                match added {
                    0 => tr!("made {name}", name = name),
                    n => tr!(
                        "made {name} with {n} frame" | "made {name} with {n} frames" % n,
                        name = name
                    ),
                }
                .into(),
            );
        }
        Asked::Smart { filter, root } => {
            let mut s = Smart::new(&typed, filter.clone(), root.clone());
            while st.views.collections.id_taken(&s.id) {
                s = Smart::new(&typed, filter.clone(), root.clone());
            }
            let (id, name) = (s.id.clone(), full_name(&s.set, &s.name));
            tracing::info!(
                "smart collection {name} saved: {}",
                s.filter.filter().describe()
            );
            st.views.collections.smart.push(s);
            st.views.collections.sort();
            st.views.collections.save(&id);
            app.set_status(tr!("saved the filter as {name}", name = name).into());
        }
        Asked::Rename(id) => {
            let (set, name) = crate::collections::split_name(&typed);
            if let Some(c) = st.views.collections.collection_mut(&id) {
                (c.set, c.name) = (set, name);
            } else if let Some(s) = st.views.collections.smart.iter_mut().find(|s| s.id == id) {
                (s.set, s.name) = (set, name);
            }
            st.views.collections.sort();
            st.views.collections.save(&id);
            app.set_status(tr!("renamed {}", name_of(&st, &id)).into());
        }
        // Taken before the match.
        Asked::Version { .. } => {}
        Asked::QueueOut { out } => {
            crate::panel::queue_view::take_out(&mut st, app, &out);
            relist_queue = true;
        }
        Asked::Delete(id) => {
            let name = name_of(&st, &id);
            if st.views.collections.delete(&id) {
                tracing::info!("collection {name} deleted");
                // Its order goes with it.
                st.views.sort.forget(&id);
                crate::panel::prefs::keep(&st, |s| s.sort.forget(&id));
                app.set_status(tr!("deleted {name}; no frame was touched", name = name).into());
                if st.views.view.collection() == Some(id.as_str()) {
                    reopen = Some(View::Roots(None));
                }
            }
        }
    }
    members_changed(&mut st, app);
    drop(st);
    if let Some(view) = reopen {
        crate::roots::open_view(state, app, worker, view);
    }
    if relist_queue {
        crate::panel::queue_view::relist(state, app, worker);
    }
}

/// B: the selection into the collection open, else the last one added
/// to; out of it when all of it is in already; with no collection to
/// go to, a new one asked for.
fn key(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) {
    let (target, frames, all_in) = {
        let st = state.borrow();
        let files = chosen_frames(&st);
        if files.is_empty() {
            app.set_status(tr!("nothing chosen to add to a collection").into());
            return;
        }
        let frames = frames_of(&st, &files);
        let target = st
            .views
            .view
            .collection()
            .map(str::to_string)
            .or_else(|| st.views.collections.target.clone())
            .filter(|id| st.views.collections.collection(id).is_some());
        let all_in = target.as_deref().is_some_and(|id| {
            let c = st
                .views
                .collections
                .collection(id)
                .expect("looked at above");
            !frames.is_empty()
                && frames
                    .iter()
                    .all(|(h, _, v)| c.has((h.as_str(), v.as_deref())))
        });
        (target, frames, all_in)
    };
    match target {
        None => ask(&mut state.borrow_mut(), app, Asked::New { add: frames }),
        Some(id) if all_in => {
            remove_from(state, app, worker, &id, &pairs_of(&frames));
        }
        Some(id) => add_to(state, app, worker, &id, &frames),
    }
}

/// A frame dragged in the grid, from its row to before row `to` (the
/// end for one past the last): it, or the set it is in, moved there in
/// the collection's own order.
fn moved(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, from: usize, to: usize) {
    {
        let mut st = state.borrow_mut();
        let View::Collection { id } = st.views.view.clone() else {
            return;
        };
        // In an order chosen, a drag has nothing to move: the order is
        // the frames', not the collection's.
        if st.views.sort.choice(&st.views.view) != crate::sort::Choice::Hand {
            return;
        }
        let Some(&cell) = st.views.shown.get(from) else {
            return;
        };
        let set = chosen_frames(&st);
        let cells = if set.contains(&cell) { set } else { vec![cell] };
        let moving = frames_of(&st, &cells);
        let before = st
            .views
            .shown
            .get(to)
            .and_then(|&c| frames_of(&st, &[c]).pop());
        let Some(c) = st.views.collections.collection_mut(&id) else {
            return;
        };
        let before = before.as_ref().map(|(h, _, v)| (h.as_str(), v.as_deref()));
        if !c.reorder(&pairs_of(&moving), before) {
            return;
        }
        st.views.collections.save(&id);
        tracing::info!("collection {}: {} moved", name_of(&st, &id), moving.len());
    }
    crate::roots::refresh_view(state, app, worker);
}

/// A row of the list clicked: a collection opened as a view of the
/// index; a smart one's filter put on the bar over the view of its root.
fn picked(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, id: &str) {
    let mut st = state.borrow_mut();
    if st.views.collections.collection(id).is_some() {
        // In the order kept for it, its own until another is chosen.
        drop(st);
        crate::roots::open_view(state, app, worker, View::Collection { id: id.to_string() });
        return;
    }
    let Some(s) = st.views.collections.smart(id).cloned() else {
        return;
    };
    // A root since taken out of the library: every root instead.
    let view = smart_view(&st, s.root.as_deref());
    st.views.filter = s.filter.opened();
    app.set_filter_text(st.views.filter.text.as_str().into());
    tracing::info!(
        "smart collection {}: {}",
        full_name(&s.set, &s.name),
        st.views.filter.describe()
    );
    let same = st.views.view == view;
    drop(st);
    if same {
        crate::panel::cull::apply_filter(state, app);
    } else {
        crate::roots::open_view(state, app, worker, view);
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    app.set_collections(ModelRc::from(
        state.borrow().views.collections_held.rows.clone(),
    ));
    app.set_collection_names(ModelRc::from(
        state.borrow().views.collections_held.names.clone(),
    ));
    app.set_collection_quick(ModelRc::from(
        state.borrow().views.collections_held.quick.clone(),
    ));
    app.on_search_collections(|names, query| {
        let query = query.trim().to_lowercase();
        let matches: Vec<_> = names
            .iter()
            .enumerate()
            .filter(|(_, name)| query.is_empty() || name.to_lowercase().contains(&query))
            .map(|(index, name)| CollectionChoice {
                name,
                index: index as i32,
            })
            .collect();
        ModelRc::from(Rc::new(VecModel::from(matches)))
    });
    show(&state.borrow(), app);
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_collection_picked(move |id| {
            if let Some(app) = app_weak.upgrade() {
                picked(&state, &app, &worker, &id);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_collection_rename(move |id| {
            if let Some(app) = app_weak.upgrade() {
                ask(&mut state.borrow_mut(), &app, Asked::Rename(id.to_string()));
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_collection_delete(move |id| {
            if let Some(app) = app_weak.upgrade() {
                ask(&mut state.borrow_mut(), &app, Asked::Delete(id.to_string()));
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_collection_forget_missing(move |id| {
            if let Some(app) = app_weak.upgrade() {
                forget_missing(&state, &app, &id);
            }
        });
    }
    {
        let state = state.clone();
        app.on_collection_forgettable(move |id| {
            // Asked as the menu opens, which the window's own handlers
            // never hold a borrow across.
            state
                .try_borrow()
                .map(|st| forgettable(&st, &id) as i32)
                .unwrap_or(0)
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_collection_capture(move |id| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The row's own way to the Sort menu's capture time, the
            // oldest first, and from any order back to the collection's
            // own.
            let view = View::Collection { id: id.to_string() };
            let sorted = state.borrow().views.sort.choice(&view) != crate::sort::Choice::Hand;
            let choice = if sorted {
                crate::sort::Choice::Hand
            } else {
                crate::sort::Choice::By(greycard_library::Order::TAKEN)
            };
            crate::panel::sort::choose(&state, &app, &worker, &view, choice);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_collection_new(move || {
            if let Some(app) = app_weak.upgrade() {
                ask(
                    &mut state.borrow_mut(),
                    &app,
                    Asked::New { add: Vec::new() },
                );
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_collection_sheet_done(move || {
            if let Some(app) = app_weak.upgrade() {
                sheet_done(&state, &app, &worker);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_collection_menu_add(move |at| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (id, frames) = {
                let st = state.borrow();
                let frames = frames_of(&st, &chosen_frames(&st));
                let id = usize::try_from(at)
                    .ok()
                    .and_then(|i| st.views.collections.list.get(i))
                    .map(|c| c.id.clone());
                (id, frames)
            };
            match id {
                Some(id) => add_to(&state, &app, &worker, &id, &frames),
                None => ask(&mut state.borrow_mut(), &app, Asked::New { add: frames }),
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_collection_menu_remove(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (id, frames) = {
                let st = state.borrow();
                let Some(id) = st.views.view.collection().map(str::to_string) else {
                    return;
                };
                (id, frames_of(&st, &chosen_frames(&st)))
            };
            remove_from(&state, &app, &worker, &id, &pairs_of(&frames));
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_collection_key(move || {
            if let Some(app) = app_weak.upgrade() {
                key(&state, &app, &worker);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_collection_moved(move |from, to| {
            let (Some(app), Ok(from), Ok(to)) = (
                app_weak.upgrade(),
                usize::try_from(from),
                usize::try_from(to),
            ) else {
                return;
            };
            moved(&state, &app, &worker, from, to);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_save_smart(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if st.views.filter.is_empty() {
                app.set_status(tr!("nothing to save: the filter shows every frame").into());
                return;
            }
            // The view's own scope: a root, or a folder of one with the
            // folders in it. A collection's view has no Save... (its
            // list is no place a filter can be kept over); a folder
            // opened from the disk is saved over every root, which
            // the sheet says.
            let root = match &st.views.view {
                View::Roots(r) => r.clone(),
                View::Branch { folder, .. } => Some(folder.clone()),
                View::Folder => None,
                // The queue's list is its file's: nothing to keep a
                // filter over.
                View::Queue => {
                    tracing::info!("a smart collection is not saved over the export queue");
                    return;
                }
                View::Collection { .. } => {
                    app.set_status(
                        tr!("a smart collection is saved from a root's view, not a collection's")
                            .into(),
                    );
                    return;
                }
            };
            let filter = Saved::of(&st.views.filter);
            ask(&mut st, &app, Asked::Smart { filter, root });
        });
    }
}

#[cfg(test)]
mod tests;
