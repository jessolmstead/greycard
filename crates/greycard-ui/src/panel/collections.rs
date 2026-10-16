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

use greycard_library::Rehash;

use crate::collections::{Collection, Frame, Pair, Smart, full_name};
use crate::filter::Saved;
use crate::panel::browser::chosen_frames;
use crate::roots::View;
use crate::*;

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
}

/// "Cover picks", or "Clients/2026/Cover picks" for one in sets.
pub(crate) fn name_of(st: &State, id: &str) -> String {
    if let Some(c) = st.collections.collection(id) {
        full_name(&c.set, &c.name)
    } else if let Some(s) = st.collections.smart(id) {
        full_name(&s.set, &s.name)
    } else {
        String::new()
    }
}

/// The list in the left pane and what hangs off it: the menu's names,
/// the collection open, and whether the grid takes a drag.
pub(crate) fn show(st: &State, app: &App) {
    let open = st.view.collection();
    let capture = matches!(st.view, View::Collection { capture: true, .. });
    let filter = Saved::of(&st.filter);
    let smart_on = |s: &Smart| filter == s.filter && st.view == smart_view(st, s.root.as_deref());
    let store = &st.collections;
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
                capture: on && capture,
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
        }))
        .collect();
    if !store.rows.iter().eq(chips.iter().cloned()) {
        store.rows.set_vec(chips);
    }
    let names: Vec<slint::SharedString> = store
        .list
        .iter()
        .map(|c| full_name(&c.set, &c.name).into())
        .collect();
    if !store.names.iter().eq(names.iter().cloned()) {
        store.names.set_vec(names);
    }
    app.set_collection_open(open.map(|id| name_of(st, id)).unwrap_or_default().into());
    app.set_collection_reorder(open.is_some() && !capture);
}

/// The view a smart collection looks over: the root it was saved under,
/// a folder of a root's tree as that folder with the folders in it, or
/// every root (for none, or for a folder no longer under a root).
pub(crate) fn smart_view(st: &State, root: Option<&Path>) -> View {
    let Some(at) = root else {
        return View::Roots(None);
    };
    let roots = &st.library.roots;
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
        View::Roots(Some(r)) => crate::roots::root_name(&st.library.roots, &r),
        View::Branch { folder, .. } => format!("{} and the folders in it", folder.display()),
        _ => "every root".to_string(),
    }
}

/// The index's rows of every member's hash, each with the versions of
/// that picture that are members, asked again: after a collection
/// changed, or the index did. True when they changed.
pub(crate) fn refresh_members(st: &mut State) -> bool {
    let ids = match &st.index_reader {
        Some(lib) => {
            let mut by_hash: HashMap<&str, HashSet<Option<String>>> = HashMap::new();
            for (hash, version) in st.collections.pairs() {
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
                    st.collections.member_ids.clone()
                }
            }
        }
        None => HashMap::new(),
    };
    if ids == st.collections.member_ids {
        return false;
    }
    st.collections.member_ids = ids;
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
    st.index_ids
        .get(cell.file)
        .copied()
        .flatten()
        .and_then(|id| st.collections.member_ids.get(&id))
        .is_some_and(|versions| versions.contains(&cell.version.as_deref().map(str::to_string)))
}

/// The mark put on every row whose membership changed.
pub(crate) fn show_collected(st: &State, app: &App) {
    let model = app.get_thumbs();
    for (row, &c) in st.shown.iter().enumerate() {
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
    let id = st.view.collection()?;
    let c = st.collections.collection(id)?;
    let name = full_name(&c.set, &c.name);
    Some(if c.refs.is_empty() {
        format!("{name} is empty: add frames with B, or Add to collection in the frame menu")
    } else if missing > 0 {
        format!(
            "{name}: {} shown, {missing} missing (moved or deleted, and kept until found)",
            crate::queue::frames(st.cells.len())
        )
    } else {
        format!("{name}: {}", crate::queue::frames(st.cells.len()))
    })
}

/// The index found files rewritten in place: each collection holding
/// one heals by its hints, and the view of one that did is read again.
pub(crate) fn healed(state: &Rc<RefCell<State>>, app: &App, rehashed: &[Rehash]) {
    if rehashed.is_empty() {
        return;
    }
    let viewing = {
        let mut st = state.borrow_mut();
        let st = &mut *st;
        let lib = st.index_reader.as_ref();
        // Asked only of a member's hash (`Store::heal`). With no index to
        // ask, or one that will not answer, the copies are not known:
        // nothing is taken for gone, and no hint moved.
        let copies = |hash: &str| -> Option<Vec<PathBuf>> {
            match lib.map(|l| l.rows_by_hash(hash)) {
                Some(Ok(rows)) => Some(rows.into_iter().map(|r| r.path).collect()),
                _ => None,
            }
        };
        let changed = st.collections.heal(rehashed, &copies);
        if changed.is_empty() {
            return;
        }
        members_changed(st, app);
        st.view
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
        .index_ids
        .get(f)
        .copied()
        .flatten()
        .and_then(|id| st.index_reader.as_ref()?.by_id(id).ok().flatten());
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

/// The selection's frames into collection `id`: new pictures at its end,
/// and another copy of a member moving its hint.
fn add_to(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, id: &str, frames: &[Frame]) {
    let viewing = {
        let mut st = state.borrow_mut();
        let name = name_of(&st, id);
        let Some(c) = st.collections.collection_mut(id) else {
            return;
        };
        let added = c.add(frames);
        st.collections.save(id);
        st.collections.target = Some(id.to_string());
        members_changed(&mut st, app);
        app.set_status(
            match (added.added, added.moved) {
                (0, 0) => format!("already in {name}"),
                (0, _) => format!("already in {name}; shown from this copy now"),
                (n, _) => format!("{} added to {name}", crate::queue::frames(n)),
            }
            .into(),
        );
        tracing::info!(
            "collection {name}: {} added, {} hints moved",
            added.added,
            added.moved
        );
        st.view.collection() == Some(id) && added != Default::default()
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
    let View::Collection { id: open, .. } = st.view.clone() else {
        return false;
    };
    let (Some((hash, path, from)), Some(source)) = (frames_of(st, &[c]).pop(), st.key_of(c)) else {
        return false;
    };
    let frame = (hash.clone(), path, Some(id.to_string()));
    let Some(coll) = st.collections.collection_mut(&open) else {
        return false;
    };
    if !coll.insert_after((&hash, from.as_deref()), &frame) {
        return false;
    }
    st.collections.save(&open);
    if let Some(frames) = st.collection_frames.as_mut() {
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
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    id: &str,
    pairs: &HashSet<Pair<'_>>,
) {
    let view = {
        let mut st = state.borrow_mut();
        let name = name_of(&st, id);
        let Some(c) = st.collections.collection_mut(id) else {
            return;
        };
        let n = c.remove(pairs);
        if n == 0 {
            app.set_status(format!("not in {name}").into());
            return;
        }
        st.collections.save(id);
        members_changed(&mut st, app);
        app.set_status(format!("{} taken out of {name}", crate::queue::frames(n)).into());
        tracing::info!("collection {name}: {n} taken out");
        (st.view.collection() == Some(id)).then(|| st.view.clone())
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
    st.library.starting
        || st.library.launch_left > 0
        || st.index.as_ref().is_some_and(|i| {
            let far: Vec<PathBuf> = st.library.remote.iter().map(|(m, _)| m.clone()).collect();
            i.busy(&st.library.offline, &far)
        })
}

/// How many references of collection `id` its menu offers to forget:
/// from its last read, while it is open and no pass is under way.
fn forgettable(st: &State, id: &str) -> usize {
    if indexing(st) || st.view.collection() != Some(id) {
        return 0;
    }
    match &st.collections.missing {
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
fn forget_missing(state: &Rc<RefCell<State>>, app: &App, id: &str) {
    let mut st = state.borrow_mut();
    let st = &mut *st;
    if st.view.collection() != Some(id) {
        return;
    }
    let name = name_of(st, id);
    if indexing(st) {
        app.set_status(
            format!("the library is being indexed; nothing forgotten from {name} until it is done")
                .into(),
        );
        return;
    }
    let away: Vec<PathBuf> = st.library.offline.iter().cloned().collect();
    let far: Vec<PathBuf> = st.library.remote.iter().map(|(m, _)| m.clone()).collect();
    // Each folder answers by its root, when it is under one: a handful
    // of looks rather than one a folder.
    let roots = &st.library.roots;
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
        st.collections
            .forget_missing(id, st.index_reader.as_ref(), &away, &far, &look)
    else {
        app.set_status(
            format!("the library index could not be read; nothing forgotten from {name}").into(),
        );
        return;
    };
    members_changed(st, app);
    let words = opened_words(st, forgot.missing).unwrap_or_default();
    let said = match (forgot.forgotten, forgot.away, forgot.on_disk) {
        (0, 0, 0) if forgot.unsure > 0 => format!(
            "{words}; nothing forgotten: the sidecar of {} could not be read",
            crate::queue::frames(forgot.unsure)
        ),
        (0, 0, 0) => format!("{words}; nothing to forget"),
        (0, away, 0) => format!(
            "{words}; nothing forgotten: {} on a drive that is offline or not answering",
            crate::queue::frames(away)
        ),
        (0, _, _) => format!("{words}; nothing forgotten: the index has yet to read some of them"),
        (n, _, _) => format!("{words}; forgot {n} missing"),
    };
    app.set_status(said.into());
}

/// The sheet put up with its question.
fn ask(st: &mut State, app: &App, asked: Asked) {
    let (title, note, asks, verb, name) = match &asked {
        Asked::New { add } => (
            "New collection".to_string(),
            match add.len() {
                0 => String::new(),
                n => format!("{} go into it", crate::queue::frames(n)),
            },
            true,
            "Create",
            String::new(),
        ),
        Asked::Smart { filter, root } => (
            "Save the filter as a smart collection".to_string(),
            format!(
                "{}, under {}",
                filter.filter().describe(),
                scope_words(st, root.as_deref())
            ),
            true,
            "Save",
            String::new(),
        ),
        Asked::Rename(id) => (
            "Rename the collection".to_string(),
            "A slash puts it in sets: Clients/2026/Cover".to_string(),
            true,
            "Rename",
            name_of(st, id),
        ),
        Asked::Delete(id) => (
            format!("Delete {}?", name_of(st, id)),
            "Its file is deleted. The frames in it are not touched.".to_string(),
            false,
            "Delete",
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
    st.collections.asked = Some(asked);
    app.set_collection_sheet_open(true);
}

/// The sheet answered.
fn sheet_done(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    if !app.get_collection_sheet_open() {
        return;
    }
    let typed = app.get_collection_sheet_name().trim().to_string();
    let named = !crate::collections::split_name(&typed).1.is_empty();
    let mut st = state.borrow_mut();
    let Some(asked) = st.collections.asked.take() else {
        app.set_collection_sheet_open(false);
        return;
    };
    // A version's name: no sets, and a blank one keeps the sheet up.
    if let Asked::Version { path, id } = asked {
        if typed.is_empty() {
            st.collections.asked = Some(Asked::Version { path, id });
            return;
        }
        app.set_collection_sheet_open(false);
        app.set_collection_sheet_placeholder("Name, or Set/Name".into());
        crate::panel::versions::renamed(&mut st, app, &path, &id, &typed);
        return;
    }
    if !named && !matches!(asked, Asked::Delete(_)) {
        // Nothing typed: the sheet stays, as it was.
        st.collections.asked = Some(asked);
        return;
    }
    app.set_collection_sheet_open(false);
    let mut reopen = None;
    match asked {
        Asked::New { add } => {
            let mut c = Collection::new(&typed);
            while st.collections.id_taken(&c.id) {
                c = Collection::new(&typed);
            }
            let added = c.add(&add).added;
            let (id, name) = (c.id.clone(), full_name(&c.set, &c.name));
            st.collections.list.push(c);
            st.collections.sort();
            st.collections.save(&id);
            st.collections.target = Some(id);
            tracing::info!("collection {name} made, {added} frames in it");
            app.set_status(
                match added {
                    0 => format!("made {name}"),
                    n => format!("made {name} with {}", crate::queue::frames(n)),
                }
                .into(),
            );
        }
        Asked::Smart { filter, root } => {
            let mut s = Smart::new(&typed, filter.clone(), root.clone());
            while st.collections.id_taken(&s.id) {
                s = Smart::new(&typed, filter.clone(), root.clone());
            }
            let (id, name) = (s.id.clone(), full_name(&s.set, &s.name));
            tracing::info!(
                "smart collection {name} saved: {}",
                s.filter.filter().describe()
            );
            st.collections.smart.push(s);
            st.collections.sort();
            st.collections.save(&id);
            app.set_status(format!("saved the filter as {name}").into());
        }
        Asked::Rename(id) => {
            let (set, name) = crate::collections::split_name(&typed);
            if let Some(c) = st.collections.collection_mut(&id) {
                (c.set, c.name) = (set, name);
            } else if let Some(s) = st.collections.smart.iter_mut().find(|s| s.id == id) {
                (s.set, s.name) = (set, name);
            }
            st.collections.sort();
            st.collections.save(&id);
            app.set_status(format!("renamed {}", name_of(&st, &id)).into());
        }
        // Taken before the match.
        Asked::Version { .. } => {}
        Asked::Delete(id) => {
            let name = name_of(&st, &id);
            if st.collections.delete(&id) {
                tracing::info!("collection {name} deleted");
                app.set_status(format!("deleted {name}; no frame was touched").into());
                if st.view.collection() == Some(id.as_str()) {
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
}

/// B: the selection into the collection open, else the last one added
/// to; out of it when all of it is in already; with no collection to
/// go to, a new one asked for.
fn key(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    let (target, frames, all_in) = {
        let st = state.borrow();
        let files = chosen_frames(&st);
        if files.is_empty() {
            app.set_status("nothing chosen to add to a collection".into());
            return;
        }
        let frames = frames_of(&st, &files);
        let target = st
            .view
            .collection()
            .map(str::to_string)
            .or_else(|| st.collections.target.clone())
            .filter(|id| st.collections.collection(id).is_some());
        let all_in = target.as_deref().is_some_and(|id| {
            let c = st.collections.collection(id).expect("looked at above");
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
fn moved(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, from: usize, to: usize) {
    {
        let mut st = state.borrow_mut();
        let View::Collection { id, capture: false } = st.view.clone() else {
            return;
        };
        let Some(&cell) = st.shown.get(from) else {
            return;
        };
        let set = chosen_frames(&st);
        let cells = if set.contains(&cell) { set } else { vec![cell] };
        let moving = frames_of(&st, &cells);
        let before = st.shown.get(to).and_then(|&c| frames_of(&st, &[c]).pop());
        let Some(c) = st.collections.collection_mut(&id) else {
            return;
        };
        let before = before.as_ref().map(|(h, _, v)| (h.as_str(), v.as_deref()));
        if !c.reorder(&pairs_of(&moving), before) {
            return;
        }
        st.collections.save(&id);
        tracing::info!("collection {}: {} moved", name_of(&st, &id), moving.len());
    }
    crate::roots::refresh_view(state, app, worker);
}

/// A row of the list clicked: a collection opened as a view of the
/// index; a smart one's filter put on the bar over the view of its root.
fn picked(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, id: &str) {
    let mut st = state.borrow_mut();
    if st.collections.collection(id).is_some() {
        // Again on the one open: as it is, capture order and all.
        let capture =
            matches!(&st.view, View::Collection { id: open, capture: true } if open == id);
        drop(st);
        crate::roots::open_view(
            state,
            app,
            worker,
            View::Collection {
                id: id.to_string(),
                capture,
            },
        );
        return;
    }
    let Some(s) = st.collections.smart(id).cloned() else {
        return;
    };
    // A root since taken out of the library: every root instead.
    let view = smart_view(&st, s.root.as_deref());
    st.filter = s.filter.filter();
    app.set_filter_text(st.filter.text.as_str().into());
    tracing::info!(
        "smart collection {}: {}",
        full_name(&s.set, &s.name),
        st.filter.describe()
    );
    let same = st.view == view;
    drop(st);
    if same {
        crate::panel::cull::apply_filter(state, app);
    } else {
        crate::roots::open_view(state, app, worker, view);
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    app.set_collections(ModelRc::from(state.borrow().collections.rows.clone()));
    app.set_collection_names(ModelRc::from(state.borrow().collections.names.clone()));
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
            let capture = !matches!(
                &state.borrow().view,
                View::Collection { id: open, capture: true } if open == id.as_str()
            );
            crate::roots::open_view(
                &state,
                &app,
                &worker,
                View::Collection {
                    id: id.to_string(),
                    capture,
                },
            );
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
                    .and_then(|i| st.collections.list.get(i))
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
                let Some(id) = st.view.collection().map(str::to_string) else {
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
            if st.filter.is_empty() {
                app.set_status("nothing to save: the filter shows every frame".into());
                return;
            }
            // The view's own scope: a root, or a folder of one with the
            // folders in it. A collection's view has no Save... (its
            // list is no place a filter can be kept over); a folder
            // opened from the disk is saved over every root, which
            // the sheet says.
            let root = match &st.view {
                View::Roots(r) => r.clone(),
                View::Branch { folder, .. } => Some(folder.clone()),
                View::Folder => None,
                View::Collection { .. } => {
                    app.set_status(
                        "a smart collection is saved from a root's view, not a collection's".into(),
                    );
                    return;
                }
            };
            let filter = Saved::of(&st.filter);
            ask(&mut st, &app, Asked::Smart { filter, root });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::{land_sent, open_view};
    use crate::testing::{click, state_for, window};
    use greycard_library::fixture::{A7, R5, R6, write_frame};

    struct Setup {
        dir: PathBuf,
        /// x, y and z in the shoot.
        files: Vec<PathBuf>,
        /// x's copy in the selects.
        copy: PathBuf,
        writer: greycard_library::Library,
        app: App,
        state: Rc<RefCell<State>>,
        worker: Rc<Worker>,
    }

    /// A root of a shoot and its selects, indexed, on a window with the
    /// grid up over every root, its collections kept in the test's own
    /// folder.
    fn setup(what: &str) -> Setup {
        let dir = crate::testing::scratch_dir(&format!("coll-{what}"));
        std::fs::create_dir_all(dir.join("photos").join("shoot")).unwrap();
        std::fs::create_dir_all(dir.join("photos").join("selects")).unwrap();
        let dir = dunce::canonicalize(&dir).unwrap();
        let root = dir.join("photos");
        let (shoot, selects) = (root.join("shoot"), root.join("selects"));
        let files: Vec<PathBuf> = ["x.tif", "y.tif", "z.tif"]
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let p = shoot.join(n);
                write_frame(&p, [&R5, &R6, &A7][i], i as u16 + 1);
                p
            })
            .collect();
        let copy = selects.join("x.tif");
        std::fs::copy(&files[0], &copy).unwrap();
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&root, &mut |_| {}).unwrap();
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1400.0, 900.0));
        app.set_grid_open(true);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.library.roots.add(&root).unwrap();
            st.collections.dir = Some(dir.join(crate::collections::DIR));
            // Sidecars on, as a session has them: a frame stands in from
            // its row, and the row is what marks it.
            st.write_sidecars = true;
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_sent(&state, &app, &worker);
        Setup {
            dir,
            files,
            copy,
            writer,
            app,
            state,
            worker,
        }
    }

    /// The test's folder removed, everything that holds a file in it
    /// let go first: the writer, and the window's reader, which the
    /// window's callbacks keep alive past a drop of the test's state
    /// (Windows refuses to remove a file SQLite has open).
    fn teardown(s: Setup) {
        let Setup {
            dir,
            writer,
            app,
            state,
            worker,
            ..
        } = s;
        drop(writer);
        state.borrow_mut().index_reader = None;
        drop(app);
        drop(worker);
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The list, each frame as its folder and its name.
    fn names(st: &State) -> Vec<String> {
        st.files
            .iter()
            .map(|f| {
                let dir = f.parent().unwrap().file_name().unwrap().to_string_lossy();
                format!("{dir}/{}", f.file_name().unwrap().to_string_lossy())
            })
            .collect()
    }

    fn row_of(st: &State, path: &Path) -> i32 {
        let f = st.files.iter().position(|p| p == path).unwrap();
        crate::panel::browser::row_of(st, f).unwrap() as i32
    }

    fn marks(app: &App) -> Vec<bool> {
        let model = app.get_thumbs();
        (0..model.row_count())
            .map(|r| model.row_data(r).unwrap().collected)
            .collect()
    }

    /// The middle of the topmost button a screen reader knows as `label`.
    fn middle_of(app: &App, label: &str) -> (f32, f32) {
        let (at, size) = crate::testing::buttons(app, label)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no button {label:?}"));
        (at.x + size.width / 2.0, at.y + size.height / 2.0)
    }

    fn file_of(s: &Setup, id: &str) -> serde_json::Value {
        let path = s
            .dir
            .join(crate::collections::DIR)
            .join(format!("{id}.json"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn hints(s: &Setup, id: &str) -> Vec<String> {
        file_of(s, id)["refs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let p = Path::new(r["hint"].as_str().unwrap());
                let dir = p.parent().unwrap().file_name().unwrap().to_string_lossy();
                format!("{dir}/{}", p.file_name().unwrap().to_string_lossy())
            })
            .collect()
    }

    /// The frame menu makes a collection of the set and the sheet names
    /// it; every copy of a member is marked; the sidebar's row opens it
    /// as the grid's list, each picture once, in its own order; B adds
    /// to it from another view and takes out what is all in already; a
    /// copy of a member moves its hint; Remove takes a frame out of the
    /// list and the file.
    #[test]
    fn the_menu_and_the_sidebar_make_fill_and_open_a_collection() {
        let s = setup("menu");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        assert_eq!(
            names(&state.borrow()),
            ["selects/x.tif", "shoot/x.tif", "shoot/y.tif", "shoot/z.tif"]
        );
        // The shoot's x and z chosen, and the menu's New collection.
        let x = row_of(&state.borrow(), &s.files[0]);
        app.invoke_select(x);
        state.borrow_mut().picked = vec![1, 3];
        app.invoke_collection_menu_add(-1);
        assert!(app.get_collection_sheet_open());
        assert_eq!(app.get_collection_sheet_note(), "2 frames go into it");
        app.set_collection_sheet_name("Clients/Cover picks".into());
        app.invoke_collection_sheet_done();
        assert!(!app.get_collection_sheet_open());
        let id = {
            let st = state.borrow();
            assert_eq!(st.collections.list.len(), 1);
            let c = &st.collections.list[0];
            assert_eq!(
                (c.set.as_str(), c.name.as_str()),
                ("Clients", "Cover picks")
            );
            c.id.clone()
        };
        assert_eq!(hints(&s, &id), ["shoot/x.tif", "shoot/z.tif"]);
        // Every local frame of a member's hash is marked: the selects'
        // copy of x too, and not y.
        assert_eq!(marks(app), [true, true, false, true]);
        let in_menu: Vec<String> = app
            .get_collection_names()
            .iter()
            .map(|n| n.to_string())
            .collect();
        assert_eq!(in_menu, ["Clients/Cover picks"]);
        assert_eq!(app.get_collection_open(), "");

        // Its row in the pane opens it: x and z, each once, x from the
        // shoot where it was added.
        let (cx, cy) = middle_of(app, "Clients/Cover picks");
        click(app, cx, cy);
        land_sent(state, app, worker);
        assert_eq!(
            state.borrow().view,
            View::Collection {
                id: id.clone(),
                capture: false
            }
        );
        assert_eq!(names(&state.borrow()), ["shoot/x.tif", "shoot/z.tif"]);
        assert_eq!(app.get_collection_open(), "Clients/Cover picks");
        assert!(app.get_collection_reorder());
        assert_eq!(app.get_status(), "Clients/Cover picks: 2 frames");
        assert!(state.borrow().collections.rows.row_data(0).unwrap().on);

        // Back to every root: y chosen and B, which goes to the last
        // collection made.
        open_view(state, app, worker, View::Roots(None));
        land_sent(state, app, worker);
        let y = row_of(&state.borrow(), &s.files[1]);
        app.invoke_select(y);
        crate::testing::press(app, "b");
        assert_eq!(app.get_status(), "1 frame added to Clients/Cover picks");
        assert_eq!(
            hints(&s, &id),
            ["shoot/x.tif", "shoot/z.tif", "shoot/y.tif"]
        );
        // B again on y, all of it in: out again.
        crate::testing::press(app, "b");
        assert_eq!(app.get_status(), "1 frame taken out of Clients/Cover picks");
        assert_eq!(hints(&s, &id), ["shoot/x.tif", "shoot/z.tif"]);
        // The selects' x through the menu: already in, its hint moved.
        let copy = row_of(&state.borrow(), &s.copy);
        app.invoke_select(copy);
        app.invoke_collection_menu_add(0);
        assert_eq!(
            app.get_status(),
            "already in Clients/Cover picks; shown from this copy now"
        );
        assert_eq!(hints(&s, &id), ["selects/x.tif", "shoot/z.tif"]);

        // Opened again: x from the selects now. The menu's Remove on
        // z: z leaves the list, and the file.
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(names(&state.borrow()), ["selects/x.tif", "shoot/z.tif"]);
        app.invoke_select(1);
        app.invoke_frame_menu_asked(1);
        app.invoke_collection_menu_remove();
        land_sent(state, app, worker);
        assert_eq!(names(&state.borrow()), ["selects/x.tif"]);
        assert_eq!(hints(&s, &id), ["selects/x.tif"]);
        // Kept on disk: a new session reads the same collection back.
        let back = crate::collections::Store::load(s.dir.join(crate::collections::DIR));
        assert_eq!(back.list, state.borrow().collections.list);
        teardown(s);
    }

    /// A drag in the grid moves a frame in the collection's own order;
    /// capture order from the row's menu shows it by time and moves
    /// nothing.
    #[test]
    fn a_drag_in_the_grid_reorders_the_collection() {
        let s = setup("drag");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let id = {
            let mut st = state.borrow_mut();
            let shoot: Vec<usize> = s
                .files
                .iter()
                .map(|f| st.files.iter().position(|p| p == f).unwrap())
                .collect();
            let frames = frames_of(&st, &shoot);
            let mut c = Collection::new("Order");
            c.add(&frames);
            let id = c.id.clone();
            st.collections.list.push(c);
            st.collections.save(&id);
            id
        };
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(
            names(&state.borrow()),
            ["shoot/x.tif", "shoot/y.tif", "shoot/z.tif"]
        );
        app.invoke_select(1);
        // A cell, by its name under the picture: a point on its picture.
        let at = |name: &str| {
            use i_slint_backend_testing::ElementHandle;
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(0.0, 0.0),
                });
            let e = ElementHandle::find_by_accessible_label(app, name)
                .min_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y))
                .unwrap_or_else(|| panic!("no cell {name}"));
            let (p, size) = (e.absolute_position(), e.size());
            (p.x + size.width / 2.0, p.y - 30.0)
        };
        let (zx, zy) = at("z.tif");
        let (xx, xy) = at("x.tif");
        // z dragged to the gap before x: the left half of x's cell.
        let to = (xx - 40.0, xy);
        crate::testing::drag(app, &[(zx, zy), (zx - 12.0, zy), to]);
        land_sent(state, app, worker);
        assert_eq!(
            names(&state.borrow()),
            ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
        );
        assert_eq!(
            hints(&s, &id),
            ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
        );
        // The drag was no click: y is still the frame on screen.
        {
            let st = state.borrow();
            assert_eq!(
                st.current.map(|c| st.files[c].clone()),
                Some(s.files[1].clone())
            );
        }
        // Capture order: by time (the R5 years before the A7, the A7
        // before the R6), the file's order kept.
        app.invoke_collection_capture(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(
            names(&state.borrow()),
            ["shoot/x.tif", "shoot/z.tif", "shoot/y.tif"]
        );
        assert!(!app.get_collection_reorder());
        assert!(state.borrow().collections.rows.row_data(0).unwrap().capture);
        assert_eq!(
            hints(&s, &id),
            ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
        );
        // A move asked for in capture order is nothing.
        app.invoke_collection_moved(2, 0);
        assert_eq!(
            hints(&s, &id),
            ["shoot/z.tif", "shoot/x.tif", "shoot/y.tif"]
        );
        teardown(s);
    }

    /// The lightbox over a collection lays its tiles out in the
    /// collection's own order, not the frames' time or name.
    #[test]
    fn the_lightbox_over_a_collection_is_in_its_hand_order() {
        use i_slint_backend_testing::{AccessibleRole, ElementHandle};
        let s = setup("lightbox");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let id = {
            let mut st = state.borrow_mut();
            // z, x, y: neither the names' order nor the time's.
            let order: Vec<usize> = [2, 0, 1]
                .iter()
                .map(|&i| st.files.iter().position(|p| *p == s.files[i]).unwrap())
                .collect();
            let frames = frames_of(&st, &order);
            let mut c = Collection::new("Sequence");
            c.add(&frames);
            let id = c.id.clone();
            st.collections.list.push(c);
            st.collections.save(&id);
            id
        };
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        app.invoke_select(0);
        crate::testing::press(app, "l");
        assert!(app.get_lightbox());
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let mut tiles: Vec<(f32, f32, &str)> = ["x.tif", "y.tif", "z.tif"]
            .into_iter()
            .map(|name| {
                let e = ElementHandle::find_by_accessible_label(app, name)
                    .find(|e| e.accessible_role() == Some(AccessibleRole::Image))
                    .unwrap_or_else(|| panic!("no tile {name}"));
                let p = e.absolute_position();
                (p.y, p.x, name)
            })
            .collect();
        tiles.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let order: Vec<&str> = tiles.iter().map(|t| t.2).collect();
        assert_eq!(order, ["z.tif", "x.tif", "y.tif"]);
        teardown(s);
    }

    /// A reference to a picture nothing has is kept and said; a file
    /// another tool rewrote in place heals the reference by its hint in
    /// the pass that found it, and the collection still shows it.
    #[test]
    fn a_missing_frame_is_kept_and_a_rewritten_one_healed() {
        let mut s = setup("heal");
        let id = {
            let mut st = s.state.borrow_mut();
            let x = st.files.iter().position(|p| *p == s.files[0]).unwrap();
            let mut frames = frames_of(&st, &[x]);
            frames.push((
                "0123456789abcdef".into(),
                PathBuf::from("/gone/away.tif"),
                None,
            ));
            let mut c = Collection::new("Kept");
            c.add(&frames);
            let id = c.id.clone();
            st.collections.list.push(c);
            st.collections.save(&id);
            id
        };
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
        assert_eq!(
            app.get_status(),
            "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found)"
        );
        assert_eq!(file_of(&s, &id)["refs"].as_array().unwrap().len(), 2);
        let old = file_of(&s, &id)["refs"][0]["hash"]
            .as_str()
            .unwrap()
            .to_string();

        // The selects' copy gone (another frame left in its folder, so
        // the pass takes it for gone), and x rewritten in place: the
        // old hash is nowhere, the pass says so, and the reference
        // heals by its hint.
        std::fs::remove_file(&s.copy).unwrap();
        write_frame(&s.copy.with_file_name("keep.tif"), &A7, 9);
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_frame(&s.files[0], &R6, 7);
        let root = s.dir.join("photos");
        let report = s.writer.index_tree(&root, &mut |_| {}).unwrap();
        assert_eq!(report.rehashed.len(), 1, "{report:?}");
        healed(&s.state, &s.app, &report.rehashed);
        land_sent(&s.state, &s.app, &s.worker);
        let refs = file_of(&s, &id)["refs"].clone();
        assert_ne!(refs[0]["hash"].as_str().unwrap(), old);
        assert_eq!(refs[0]["hash"].as_str().unwrap(), report.rehashed[0].new);
        assert_eq!(refs.as_array().unwrap().len(), 2, "the missing one kept");
        assert_eq!(names(&s.state.borrow()), ["shoot/x.tif"]);
        assert_eq!(marks(&s.app), [true]);
        teardown(s);
    }

    /// The open collection's row offers to forget its missing frames
    /// when it has some: the one found nowhere goes, from the list and
    /// the file, the one hinted under an offline root stays, and the
    /// status line counts what is left.
    #[test]
    fn forget_missing_drops_the_gone_and_keeps_the_offline() {
        let s = setup("forget");
        let drive = s.dir.join("drive");
        let id = {
            let mut st = s.state.borrow_mut();
            // A root of the library, unplugged: never made on disk.
            st.library.roots.add_checked(drive.clone()).unwrap();
            st.library.offline.insert(drive.clone());
            let x = st.files.iter().position(|p| *p == s.files[0]).unwrap();
            let mut frames = frames_of(&st, &[x]);
            frames.push((
                "0123456789abcdef".into(),
                s.dir.join("gone").join("a.tif"),
                None,
            ));
            frames.push(("fedcba9876543210".into(), drive.join("b.tif"), None));
            let mut c = Collection::new("Kept");
            c.add(&frames);
            let id = c.id.clone();
            st.collections.list.push(c);
            st.collections.save(&id);
            id
        };
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let chip = |app: &App| {
            let rows = app.get_collections();
            (0..rows.row_count())
                .map(|r| rows.row_data(r).unwrap())
                .find(|c| c.id.as_str() == id)
                .unwrap()
        };
        show(&state.borrow(), app);
        let offered = |app: &App| app.invoke_collection_forgettable(id.as_str().into());
        assert_eq!(offered(app), 0, "nothing offered while closed");
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
        assert_eq!(
            app.get_status(),
            "Kept: 1 frame shown, 2 missing (moved or deleted, and kept until found)"
        );
        assert_eq!(offered(app), 1, "the offline one is not offered");
        // A pass under way: nothing offered, and a press refused.
        state.borrow_mut().library.launch_left = 1;
        assert_eq!(offered(app), 0);
        app.invoke_collection_forget_missing(id.as_str().into());
        assert_eq!(
            app.get_status(),
            "the library is being indexed; nothing forgotten from Kept until it is done"
        );
        assert_eq!(hints(&s, &id).len(), 3);
        state.borrow_mut().library.launch_left = 0;
        assert_eq!(chip(app).count, 3);

        app.invoke_collection_forget_missing(id.as_str().into());
        assert_eq!(
            app.get_status(),
            "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found); \
             forgot 1 missing"
        );
        assert_eq!(hints(&s, &id), ["shoot/x.tif", "drive/b.tif"]);
        assert_eq!(offered(app), 0);
        // Pressed again: nothing to forget, and the reason is the drive.
        app.invoke_collection_forget_missing(id.as_str().into());
        assert_eq!(
            app.get_status(),
            "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found); \
             nothing forgotten: 1 frame on a drive that is offline or not answering"
        );
        // The drive pulled since the window last looked: the set it holds
        // does not say offline, but the root answers no, and b is kept.
        state.borrow_mut().library.offline.clear();
        app.invoke_collection_forget_missing(id.as_str().into());
        assert_eq!(
            app.get_status(),
            "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found); \
             nothing forgotten: 1 frame on a drive that is offline or not answering"
        );
        assert_eq!(hints(&s, &id), ["shoot/x.tif", "drive/b.tif"]);
        state.borrow_mut().library.offline.insert(drive.clone());
        assert_eq!(chip(app).count, 2);
        assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
        // Opened again: the status line's count is the one left.
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(
            app.get_status(),
            "Kept: 1 frame shown, 1 missing (moved or deleted, and kept until found)"
        );
        teardown(s);
    }

    /// With no index to read, a pass's rewrite heals nothing it cannot
    /// be sure of: a reference whose new hash another holds keeps its
    /// hint as it was, never an empty one.
    #[test]
    fn a_heal_with_no_index_to_ask_keeps_the_hint() {
        let s = setup("no-reader");
        let id = made(&s, "Both", &[&s.files[0], &s.files[1]]);
        let (hx, hy) = {
            let st = s.state.borrow();
            let c = st.collections.collection(&id).unwrap();
            (c.refs[0].hash.clone(), c.refs[1].hash.clone())
        };
        s.state.borrow_mut().index_reader = None;
        let rehash = Rehash {
            path: s.files[0].clone(),
            old: hx.clone(),
            new: hy.clone(),
        };
        healed(&s.state, &s.app, &[rehash]);
        let refs = s
            .state
            .borrow()
            .collections
            .collection(&id)
            .unwrap()
            .refs
            .clone();
        assert_eq!(refs[0].hash, hx);
        assert_eq!(refs[0].hint, s.files[0].to_string_lossy());
        assert_eq!(
            file_of(&s, &id)["refs"][0]["hint"],
            s.files[0].to_string_lossy().as_ref()
        );
        teardown(s);
    }

    /// The filter bar's Save... keeps the filter as a smart collection;
    /// its row puts the filter back over every root, and is lit while
    /// the bar says it; its menu renames and deletes it.
    #[test]
    fn a_smart_collection_keeps_the_filter_and_puts_it_back() {
        let s = setup("smart");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        app.invoke_filter_save_smart();
        assert!(!app.get_collection_sheet_open(), "nothing to save yet");
        state.borrow_mut().filter.text = "y".into();
        app.set_filter_text("y".into());
        crate::panel::cull::apply_filter(state, app);
        assert_eq!(state.borrow().shown.len(), 1);
        app.invoke_filter_save_smart();
        assert!(app.get_collection_sheet_open());
        assert_eq!(app.get_collection_sheet_note(), "\"y\", under every root");
        app.set_collection_sheet_name("Just y".into());
        app.invoke_collection_sheet_done();
        let id = state.borrow().collections.smart[0].id.clone();
        let file = s
            .dir
            .join(crate::collections::DIR)
            .join(format!("{id}.smart.json"));
        assert!(file.exists());
        assert!(state.borrow().collections.rows.row_data(0).unwrap().on);
        // Cleared, the row goes dark; pressed, the filter is back.
        app.invoke_filter_cleared();
        assert_eq!(state.borrow().shown.len(), 4);
        assert!(!state.borrow().collections.rows.row_data(0).unwrap().on);
        let (cx, cy) = middle_of(app, "Just y");
        click(app, cx, cy);
        land_sent(state, app, worker);
        assert_eq!(state.borrow().filter.text, "y");
        assert_eq!(app.get_filter_text(), "y");
        assert_eq!(state.borrow().shown.len(), 1);
        assert!(state.borrow().collections.rows.row_data(0).unwrap().on);
        // Read back from its file, the same question.
        let back = crate::collections::Store::load(s.dir.join(crate::collections::DIR));
        assert_eq!(back.smart[0].filter, Saved::of(&state.borrow().filter));
        // Renamed, into a set, and deleted, from its menu's sheet.
        app.invoke_collection_rename(id.as_str().into());
        assert_eq!(app.get_collection_sheet_name(), "Just y");
        app.set_collection_sheet_name("Sets/Only y".into());
        app.invoke_collection_sheet_done();
        assert_eq!(state.borrow().collections.smart[0].set, "Sets");
        app.invoke_collection_delete(id.as_str().into());
        assert!(!app.get_collection_sheet_asks_name());
        app.invoke_collection_sheet_done();
        assert!(state.borrow().collections.smart.is_empty());
        assert!(!file.exists());
        teardown(s);
    }

    /// A collection made with the frames in it, saved, by the shoot's
    /// files given.
    fn made(s: &Setup, name: &str, files: &[&PathBuf]) -> String {
        let mut st = s.state.borrow_mut();
        let at: Vec<usize> = files
            .iter()
            .map(|f| st.files.iter().position(|p| p == *f).unwrap())
            .collect();
        let frames = frames_of(&st, &at);
        let mut c = Collection::new(name);
        c.add(&frames);
        let id = c.id.clone();
        st.collections.list.push(c);
        st.collections.save(&id);
        id
    }

    /// A merge read of one collection that lands after another was
    /// opened puts what it found into its own collection, not into the
    /// one open by then.
    #[test]
    fn a_late_merge_read_heals_its_own_collection() {
        let s = setup("late-merge");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let a = made(&s, "A", &[&s.files[0]]);
        let b = made(&s, "B", &[&s.files[0]]);
        app.invoke_collection_picked(a.as_str().into());
        land_sent(state, app, worker);
        // A's hint gone stale, so its read has a change to bring.
        let stale = s.files[0]
            .with_file_name("gone.tif")
            .to_string_lossy()
            .into_owned();
        state
            .borrow_mut()
            .collections
            .collection_mut(&a)
            .unwrap()
            .refs[0]
            .hint = stale.clone();
        // A merge of A's list sent, then B opened; B's read lands first.
        assert!(crate::roots::refresh_view(state, app, worker));
        app.invoke_collection_picked(b.as_str().into());
        crate::roots::land_sent_last_first(state, app, worker);
        let st = state.borrow();
        let hint = |id: &str| st.collections.collection(id).unwrap().refs[0].hint.clone();
        assert_eq!(hint(&b), s.files[0].to_string_lossy(), "B untouched");
        assert_ne!(hint(&a), stale, "A's own read healed A");
        assert_eq!(
            st.view,
            View::Collection {
                id: b.clone(),
                capture: false
            }
        );
        drop(st);
        teardown(s);
    }

    /// The keys come back to the window when the sheet closes: B asks
    /// for a new collection, its name typed and Return, and the next B
    /// adds to it; Escape on the sheet gives the keys back too.
    #[test]
    fn b_works_again_after_the_sheet_closes() {
        let s = setup("keys");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let x = row_of(&state.borrow(), &s.files[0]);
        app.invoke_select(x);
        crate::testing::press(app, "b");
        assert!(app.get_collection_sheet_open(), "no collection yet: asked");
        crate::testing::press(app, slint::platform::Key::Escape);
        assert!(!app.get_collection_sheet_open());
        crate::testing::press(app, "b");
        assert!(app.get_collection_sheet_open(), "B again after Escape");
        crate::testing::type_text(app, "Picks");
        crate::testing::press(app, slint::platform::Key::Return);
        assert!(!app.get_collection_sheet_open());
        assert_eq!(state.borrow().collections.list.len(), 1);
        let y = row_of(&state.borrow(), &s.files[1]);
        app.invoke_select(y);
        crate::testing::press(app, "b");
        assert_eq!(app.get_status(), "1 frame added to Picks");
        assert_eq!(state.borrow().collections.list[0].refs.len(), 2);
        let _ = worker;
        teardown(s);
    }

    /// Save... keeps the view's own scope: a folder of a root's tree is
    /// kept as that folder and the folders in it, and opens so; over a
    /// collection there is nothing to save.
    #[test]
    fn save_keeps_a_branch_and_is_not_offered_over_a_collection() {
        let s = setup("scope");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let root = s.dir.join("photos");
        let shoot = root.join("shoot");
        open_view(
            state,
            app,
            worker,
            View::Branch {
                root: root.clone(),
                folder: shoot.clone(),
                deep: false,
            },
        );
        land_sent(state, app, worker);
        state.borrow_mut().filter.text = "z".into();
        app.set_filter_text("z".into());
        crate::panel::cull::apply_filter(state, app);
        app.invoke_filter_save_smart();
        assert_eq!(
            app.get_collection_sheet_note(),
            format!("\"z\", under {} and the folders in it", shoot.display())
        );
        app.set_collection_sheet_name("Zs".into());
        app.invoke_collection_sheet_done();
        let id = state.borrow().collections.smart[0].id.clone();
        assert_eq!(
            state.borrow().collections.smart[0].root,
            Some(shoot.clone())
        );
        open_view(state, app, worker, View::Roots(None));
        land_sent(state, app, worker);
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(
            state.borrow().view,
            View::Branch {
                root: root.clone(),
                folder: shoot.clone(),
                deep: true
            }
        );
        assert!(state.borrow().collections.rows.row_data(0).unwrap().on);
        // Over a collection: no button, and the callback says why.
        let c = made(&s, "C", &[&s.files[2]]);
        app.invoke_collection_picked(c.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(app.get_collection_open(), "C");
        app.invoke_filter_save_smart();
        assert!(!app.get_collection_sheet_open());
        assert_eq!(
            app.get_status(),
            "a smart collection is saved from a root's view, not a collection's"
        );
        teardown(s);
    }

    /// A version named `name` made in the sidecar of the shoot's x on
    /// disk, the index brought up to date with it and every root's view
    /// opened again: its id.
    fn version_of_x(s: &mut Setup, name: &str) -> String {
        let x = &s.files[0];
        let mut sidecar = greycard_edit::Sidecar::load(x).unwrap().unwrap_or_default();
        let id = sidecar.new_version(None, Some(name)).unwrap();
        sidecar.save(x).unwrap();
        s.writer
            .index_tree(&s.dir.join("photos"), &mut |_| {})
            .unwrap();
        open_view(&s.state, &s.app, &s.worker, View::Roots(None));
        land_sent(&s.state, &s.app, &s.worker);
        id
    }

    /// Each cell as its file's folder and name, and its version's id.
    fn frames(st: &State) -> Vec<(String, Option<String>)> {
        let names = names(st);
        st.cells
            .iter()
            .map(|c| {
                (
                    names[c.file].clone(),
                    c.version.as_deref().map(str::to_string),
                )
            })
            .collect()
    }

    /// The row the cell of `path`'s version `version` is on.
    fn row_of_frame(st: &State, path: &Path, version: Option<&str>) -> i32 {
        let f = st.files.iter().position(|p| p == path).unwrap();
        let c = st.cell_of(f, version).unwrap();
        crate::panel::browser::row_of(st, c).unwrap() as i32
    }

    /// A version put in a collection from the menu marks its own cell
    /// and no other, and opened, the collection shows that version's
    /// tile; B adds the file's own as a frame of its own and takes the
    /// version out again by its pair; two versions of one picture are two
    /// tiles in the collection's order, each once however many times the
    /// file names it; Remove takes the pair out and leaves the other.
    #[test]
    fn a_version_in_a_collection_is_its_own_tile_and_mark() {
        let mut s = setup("version-tile");
        let warm = version_of_x(&mut s, "Warm");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        assert_eq!(
            frames(&state.borrow()),
            [
                ("selects/x.tif".to_string(), None),
                ("shoot/x.tif".to_string(), None),
                ("shoot/x.tif".to_string(), Some(warm.clone())),
                ("shoot/y.tif".to_string(), None),
                ("shoot/z.tif".to_string(), None),
            ]
        );
        // Warm alone into a new collection, from the menu.
        let row = row_of_frame(&state.borrow(), &s.files[0], Some(&warm));
        app.invoke_select(row);
        app.invoke_collection_menu_add(-1);
        app.set_collection_sheet_name("Warm ones".into());
        app.invoke_collection_sheet_done();
        let id = state.borrow().collections.list[0].id.clone();
        let refs = file_of(&s, &id)["refs"].clone();
        assert_eq!(refs.as_array().unwrap().len(), 1);
        assert_eq!(refs[0]["version"], warm.as_str());
        assert_eq!(file_of(&s, &id)["version"], 2);
        assert_eq!(
            marks(app),
            [false, false, true, false, false],
            "the version's cell alone, not its file's own"
        );
        // B on x's own: a frame of its own, beside the version.
        let own = row_of_frame(&state.borrow(), &s.files[0], None);
        app.invoke_select(own);
        crate::testing::press(app, "b");
        assert_eq!(app.get_status(), "1 frame added to Warm ones");
        // Every copy of the picture wears the original's mark; Warm's
        // cell its own.
        assert_eq!(marks(app), [true, true, true, false, false]);
        // B on Warm, all of it in: Warm out, the original kept.
        app.invoke_select(row);
        crate::testing::press(app, "b");
        assert_eq!(app.get_status(), "1 frame taken out of Warm ones");
        assert_eq!(marks(app), [true, true, false, false, false]);
        assert_eq!(file_of(&s, &id)["version"], 1, "no version left in it");
        crate::testing::press(app, "b");
        assert_eq!(app.get_status(), "1 frame added to Warm ones");

        // Opened: the original, then Warm, in the collection's order, a
        // tile each of the one file.
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(
            frames(&state.borrow()),
            [
                ("shoot/x.tif".to_string(), None),
                ("shoot/x.tif".to_string(), Some(warm.clone())),
            ]
        );
        assert_eq!(names(&state.borrow()), ["shoot/x.tif"]);
        assert_eq!(app.get_status(), "Warm ones: 2 frames");
        assert_eq!(marks(app), [true, true]);
        // A second reference to Warm (two editors' joined files): shown
        // once.
        {
            let mut st = state.borrow_mut();
            let c = st.collections.collection_mut(&id).unwrap();
            let again = c.refs[1].clone();
            c.refs.push(again);
        }
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(state.borrow().cells.len(), 2);
        // The menu's Remove on Warm's tile: the pair out, the original
        // kept.
        app.invoke_select(1);
        app.invoke_frame_menu_asked(1);
        app.invoke_collection_menu_remove();
        land_sent(state, app, worker);
        assert_eq!(frames(&state.borrow()), [("shoot/x.tif".to_string(), None)]);
        let refs = state
            .borrow()
            .collections
            .collection(&id)
            .unwrap()
            .refs
            .clone();
        assert!(refs.iter().all(|r| r.version.is_none()), "{refs:?}");
        teardown(s);
    }

    /// A version deleted from its sidecar is missing from the collection,
    /// said in the status line and kept in the file; Forget missing drops
    /// it once the sidecar on disk reads without it.
    #[test]
    fn a_version_gone_from_its_sidecar_is_missing_and_forgettable() {
        let mut s = setup("version-gone");
        let warm = version_of_x(&mut s, "Warm");
        let id = {
            let st = s.state.borrow();
            let f = st.files.iter().position(|p| *p == s.files[0]).unwrap();
            let cells = [
                st.cell_of(f, None).unwrap(),
                st.cell_of(f, Some(&warm)).unwrap(),
            ];
            let frames = frames_of(&st, &cells);
            drop(st);
            let mut c = Collection::new("Both");
            c.add(&frames);
            let id = c.id.clone();
            let mut st = s.state.borrow_mut();
            st.collections.list.push(c);
            st.collections.save(&id);
            id
        };
        // Deleted on another machine, the sidecar synced over.
        let mut sidecar = greycard_edit::Sidecar::load(&s.files[0]).unwrap().unwrap();
        assert!(sidecar.delete_version(&warm).is_some());
        sidecar.save(&s.files[0]).unwrap();
        s.writer
            .index_tree(&s.dir.join("photos"), &mut |_| {})
            .unwrap();
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(frames(&state.borrow()), [("shoot/x.tif".to_string(), None)]);
        assert_eq!(
            app.get_status(),
            "Both: 1 frame shown, 1 missing (moved or deleted, and kept until found)"
        );
        assert_eq!(file_of(&s, &id)["refs"].as_array().unwrap().len(), 2);
        assert_eq!(app.invoke_collection_forgettable(id.as_str().into()), 1);
        app.invoke_collection_forget_missing(id.as_str().into());
        assert_eq!(app.get_status(), "Both: 1 frame; forgot 1 missing");
        let refs = file_of(&s, &id)["refs"].clone();
        assert_eq!(refs.as_array().unwrap().len(), 1);
        assert!(refs[0].get("version").is_none());
        teardown(s);
    }

    /// A version made while a collection is the list goes into it beside
    /// the frame it came from, its cell opened and marked.
    #[test]
    fn a_version_made_in_a_collection_goes_into_it() {
        let s = setup("version-made");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let id = made(&s, "Made here", &[&s.files[0], &s.files[1]]);
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        app.invoke_select(0);
        crate::rows::land_pending(state, app, worker);
        crate::panel::versions::new_version(state, app, worker, false, false);
        app.invoke_collection_sheet_done();
        let refs = state
            .borrow()
            .collections
            .collection(&id)
            .unwrap()
            .refs
            .clone();
        assert_eq!(refs.len(), 3);
        let made = refs[1].version.clone().expect("beside x's own");
        assert_eq!(refs[1].hash, refs[0].hash);
        assert_eq!(
            frames(&state.borrow()),
            [
                ("shoot/x.tif".to_string(), None),
                ("shoot/x.tif".to_string(), Some(made.clone())),
                ("shoot/y.tif".to_string(), None),
            ]
        );
        {
            let st = state.borrow();
            assert_eq!(st.current, Some(1), "the new version's cell open");
        }
        assert_eq!(marks(app), [true, true, true]);
        assert_eq!(file_of(&s, &id)["refs"][1]["version"], made.as_str());
        teardown(s);
    }

    /// A smart collection is a question over rows: a row that passes
    /// brings its versions with it.
    #[test]
    fn a_smart_collection_lists_a_passing_row_s_versions() {
        let mut s = setup("version-smart");
        let warm = version_of_x(&mut s, "Warm");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        state.borrow_mut().filter.text = "x".into();
        app.set_filter_text("x".into());
        crate::panel::cull::apply_filter(state, app);
        app.invoke_filter_save_smart();
        app.set_collection_sheet_name("Just x".into());
        app.invoke_collection_sheet_done();
        let id = state.borrow().collections.smart[0].id.clone();
        app.invoke_filter_cleared();
        assert_eq!(state.borrow().shown.len(), 5);
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        let st = state.borrow();
        let shown: Vec<(String, Option<String>)> =
            st.shown.iter().map(|&c| frames(&st)[c].clone()).collect();
        assert_eq!(
            shown,
            [
                ("selects/x.tif".to_string(), None),
                ("shoot/x.tif".to_string(), None),
                ("shoot/x.tif".to_string(), Some(warm)),
            ]
        );
        drop(st);
        teardown(s);
    }

    /// A version made in a collection, a merge read of the list landing
    /// before the index has read the save (B on another frame while
    /// viewing it sends one): the new cell stays, still the frame on
    /// screen, and Forget missing keeps it, its sidecar on disk having it.
    #[test]
    fn a_version_made_survives_a_merge_before_the_index_reads_it() {
        let s = setup("made-merge");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let id = made(&s, "Made here", &[&s.files[0], &s.files[1]]);
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        app.invoke_select(0);
        crate::rows::land_pending(state, app, worker);
        crate::panel::versions::new_version(state, app, worker, false, false);
        app.invoke_collection_sheet_done();
        assert_eq!(state.borrow().cells.len(), 3);
        let made_id = state.borrow().collections.collection(&id).unwrap().refs[1]
            .version
            .clone()
            .unwrap();
        // The index behind: the save is not read into it.
        crate::roots::refresh_view(state, app, worker);
        land_sent(state, app, worker);
        {
            let st = state.borrow();
            assert_eq!(st.cells.len(), 3, "the new version's cell stays");
            let current = st.current.and_then(|c| st.version_of(c));
            assert_eq!(current, Some(made_id.as_str()));
        }
        app.invoke_collection_forget_missing(id.as_str().into());
        assert!(
            app.get_status()
                .ends_with("nothing forgotten: the index has yet to read some of them"),
            "{}",
            app.get_status()
        );
        let refs = state
            .borrow()
            .collections
            .collection(&id)
            .unwrap()
            .refs
            .clone();
        assert!(
            refs.iter()
                .any(|r| r.version.as_deref() == Some(made_id.as_str())),
            "the version made was forgotten: {refs:?}"
        );
        teardown(s);
    }

    /// x rewritten in place, its sidecar with Warm beside it, a copy of
    /// the old content left in the selects with a sidecar of its own, and
    /// no pass report healing the reference: the open finds Warm in the
    /// rewritten file's sidecar, by the hint, and Forget missing never
    /// takes the copy's sidecar for Warm's.
    #[test]
    fn a_rewritten_file_s_version_is_not_forgotten_by_a_copy() {
        let mut s = setup("rewrite-version");
        let warm = version_of_x(&mut s, "Warm");
        greycard_edit::Sidecar::default().save(&s.copy).unwrap();
        let id = {
            let st = s.state.borrow();
            let f = st.files.iter().position(|p| *p == s.files[0]).unwrap();
            let frames = frames_of(&st, &[st.cell_of(f, Some(&warm)).unwrap()]);
            drop(st);
            let mut c = Collection::new("Warm");
            c.add(&frames);
            let id = c.id.clone();
            let mut st = s.state.borrow_mut();
            st.collections.list.push(c);
            st.collections.save(&id);
            id
        };
        let old = file_of(&s, &id)["refs"][0]["hash"]
            .as_str()
            .unwrap()
            .to_string();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_frame(&s.files[0], &R6, 7);
        let report = s
            .writer
            .index_tree(&s.dir.join("photos"), &mut |_| {})
            .unwrap();
        assert_eq!(report.rehashed.len(), 1);
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        app.invoke_collection_picked(id.as_str().into());
        land_sent(state, app, worker);
        assert_eq!(
            frames(&state.borrow()),
            [("shoot/x.tif".to_string(), Some(warm.clone()))]
        );
        assert_eq!(app.get_status(), "Warm: 1 frame");
        let refs = file_of(&s, &id)["refs"].clone();
        assert_ne!(refs[0]["hash"].as_str().unwrap(), old, "the hash healed");
        app.invoke_collection_forget_missing(id.as_str().into());
        assert_eq!(app.get_status(), "Warm: 1 frame; nothing to forget");
        assert_eq!(file_of(&s, &id)["refs"].as_array().unwrap().len(), 1);
        teardown(s);
    }
}
