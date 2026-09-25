//! The library's roots in the editor (notes §72): the folders the
//! user has added, a pass over them on launch, a watcher on them
//! while the editor runs, and the all-roots view, which is every file
//! under them in one list.
//!
//! The roots are `greycard_library::Roots`, kept in `roots.json`
//! beside the library's database, so a `--library` elsewhere carries
//! its own. The launch pass is `index_tree` over each root on the
//! indexer's thread, behind everything the window asks: an mtime
//! pass, since a file whose size and mtime are unchanged is not read.
//! The watcher's changes go to the same thread, ahead of the launch
//! pass and behind the window.
//!
//! The all-roots view is the index's list of the files under the
//! roots, by folder then name, with the filter bar over it as over a
//! folder. It can be the whole library, so its sidecars are read on
//! the rayon pool rather than the window's thread: the window keeps
//! the folder it has until the new list is ready, and says it is
//! reading meanwhile.
//!
//! A list already open is kept up with the disk rather than opened
//! again: a pass that finds a file added, gone or moved has the list
//! read again and merged into what the window holds, each frame
//! keeping its sidecar, its thumbnail and its place in the selection
//! by its path. The frame on screen is followed to where the index
//! says it went.

use std::collections::HashMap;
use std::time::Instant;

use greycard_library::Roots;
use greycard_library::roots::Added;

use crate::panel::browser::{
    load_sidecars, load_sidecars_parallel, open_loaded, rebuild_browser_from,
};
use crate::panel::cull::drop_placeholder;
use crate::*;

/// What the browser lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum View {
    /// A folder, or files the desktop asked to open.
    #[default]
    Folder,
    /// Every file under the roots, or under one of them.
    Roots(Option<PathBuf>),
}

/// The roots as the editor holds them: the list, where it is kept,
/// and each root's file count as the index last said.
#[derive(Default)]
pub(crate) struct Library {
    pub(crate) roots: Roots,
    /// The roots file; none for a run whose roots came from the
    /// command line, which leaves the user's alone.
    pub(crate) file: Option<PathBuf>,
    pub(crate) counts: Vec<usize>,
    pub(crate) watcher: Option<greycard_library::Watcher>,
    /// A view asked for before the window could read the index.
    pub(crate) wanted: Option<View>,
    /// A capture waits for the view the command line asked for.
    pub(crate) awaiting: bool,
    /// The view is in the browser, and the next frame drawn is the
    /// first to show it: when it was asked for, for the log.
    pub(crate) painted: Option<(Instant, &'static str)>,
    /// A view's sidecars are being read on the pool: the list in the
    /// browser is about to be replaced, and is not merged into.
    pub(crate) loading: bool,
}

/// Start the roots on launch: the list read, the launch pass asked
/// for with the open folder's root first, and the watcher on them.
/// A watcher that cannot start is said and left; the launch pass is
/// the fallback.
pub(crate) fn start(st: &mut State) {
    let Some(indexer) = &st.index else {
        return;
    };
    let roots = st.library.roots.list().to_vec();
    if roots.is_empty() {
        return;
    }
    let open = st
        .files
        .first()
        .and_then(|f| f.parent())
        .and_then(|d| dunce::canonicalize(d).ok());
    let mut order = roots.clone();
    if let Some(first) = open
        .as_deref()
        .and_then(|d| st.library.roots.root_of(d))
        .map(Path::to_path_buf)
    {
        order.retain(|r| *r != first);
        order.insert(0, first);
    }
    tracing::info!(
        "library: {} root(s), a pass over each on the indexer's thread",
        order.len()
    );
    indexer.roots(order);
    watch(st);
}

/// The watcher on the roots as they are now, the old one dropped.
pub(crate) fn watch(st: &mut State) {
    st.library.watcher = None;
    let Some(indexer) = &st.index else {
        return;
    };
    let roots = st.library.roots.list().to_vec();
    if roots.is_empty() {
        return;
    }
    let asker = indexer.asker();
    let started = Instant::now();
    let (watcher, failed) = greycard_library::Watcher::start(
        &roots,
        greycard_library::roots::QUIET,
        greycard_library::roots::LONGEST,
        move |changes| {
            tracing::debug!("watch: {changes:?}");
            asker.changes(changes);
        },
    );
    for (root, why) in &failed {
        tracing::warn!(
            "library: not watching {} ({why}); it is brought up to date when the editor starts",
            root.display()
        );
    }
    if let Some(w) = &watcher {
        tracing::info!(
            "library: watching {} root(s), set up in {:.0} ms",
            w.watched.len(),
            started.elapsed().as_secs_f64() * 1e3
        );
    }
    st.library.watcher = watcher;
}

/// Each root's count, as the index holds it now.
pub(crate) fn recount(st: &mut State) {
    let Some(lib) = &st.index_reader else {
        return;
    };
    let counts: Vec<usize> = st
        .library
        .roots
        .list()
        .iter()
        .map(|r| lib.count_under(r).unwrap_or(0))
        .collect();
    st.library.counts = counts;
}

/// The roots' row in the grid's header.
pub(crate) fn show(st: &State, app: &App) {
    let roots = st.library.roots.list();
    let on = match &st.view {
        View::Roots(r) => Some(r.clone()),
        View::Folder => None,
    };
    let chips: Vec<RootChip> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| RootChip {
            name: root_name(r).into(),
            path: r.to_string_lossy().into_owned().into(),
            count: st.library.counts.get(i).copied().unwrap_or(0) as i32,
            on: on.as_ref() == Some(&Some(r.clone())),
        })
        .collect();
    app.set_library_roots(ModelRc::new(VecModel::from(chips)));
    app.set_library_all_on(on == Some(None));
    app.set_library_all_count(st.library.counts.iter().sum::<usize>() as i32);
    app.set_library_can_add_open(open_folder_to_add(st).is_some());
    app.set_library_note(
        if roots.is_empty() {
            "No folders in the library yet"
        } else {
            ""
        }
        .into(),
    );
}

/// A root as its chip names it: the folder's own name, or the whole
/// path for a disk's root, which has none.
pub(crate) fn root_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// The open folder, when the browser shows one and it is not under a
/// root yet: what "Add this folder" adds.
fn open_folder_to_add(st: &State) -> Option<PathBuf> {
    if st.view != View::Folder {
        return None;
    }
    let dir = st.files.first()?.parent()?;
    let dir = dunce::canonicalize(dir).ok()?;
    st.library.roots.root_of(&dir).is_none().then_some(dir)
}

/// Add a folder to the roots: kept, watched, passed over, and the
/// view brought up if it lists the roots.
pub(crate) fn add(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let mut st = state.borrow_mut();
    let added = st.library.roots.add(dir);
    let new = match added {
        Ok(Added::New(root)) => {
            app.set_status(format!("{} is in the library now", root.display()).into());
            root
        }
        Ok(Added::Absorbed(root, under)) => {
            app.set_status(
                format!(
                    "{} is in the library now, in place of {} folder{} under it",
                    root.display(),
                    under.len(),
                    if under.len() == 1 { "" } else { "s" }
                )
                .into(),
            );
            root
        }
        Ok(Added::Covered(root)) => {
            app.set_status(format!("already in the library, under {}", root.display()).into());
            return;
        }
        Err(e) => {
            tracing::warn!("library: {e}");
            app.set_status(format!("not added: {e}").into());
            return;
        }
    };
    tracing::info!("library: added {}", new.display());
    save(&st);
    if let Some(indexer) = &st.index {
        indexer.roots(vec![new]);
    }
    watch(&mut st);
    recount(&mut st);
    show(&st, app);
    let refresh = matches!(st.view, View::Roots(None));
    drop(st);
    if refresh {
        refresh_view(state, app, worker, true);
    }
}

/// Take a folder out of the roots. The folder and its files are not
/// touched, nor are their rows in the index; the all-roots view no
/// longer lists them.
pub(crate) fn remove(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let mut st = state.borrow_mut();
    if !st.library.roots.remove(dir) {
        return;
    }
    tracing::info!(
        "library: removed {}; nothing on disk touched",
        dir.display()
    );
    app.set_status(
        format!(
            "{} is out of the library; its files are where they were",
            dir.display()
        )
        .into(),
    );
    save(&st);
    watch(&mut st);
    recount(&mut st);
    let view = st.view.clone();
    let refresh = match &view {
        View::Roots(Some(r)) if r == dir => {
            st.view = View::Roots(None);
            true
        }
        View::Roots(_) => true,
        View::Folder => false,
    };
    show(&st, app);
    drop(st);
    if refresh {
        refresh_view(state, app, worker, true);
    }
}

fn save(st: &State) {
    if let Some(path) = &st.library.file
        && let Err(e) = st.library.roots.save(path)
    {
        tracing::warn!("library: roots not saved to {}: {e}", path.display());
    }
}

/// The roots a view lists.
fn roots_of(st: &State, view: &View) -> Vec<PathBuf> {
    match view {
        View::Folder => Vec::new(),
        View::Roots(None) => st.library.roots.list().to_vec(),
        View::Roots(Some(r)) => vec![r.clone()],
    }
}

/// Show every file under the roots, or under one: the list from the
/// index, its sidecars read on the rayon pool, and the browser's list
/// replaced once they are in. The window goes on with what it has
/// meanwhile.
pub(crate) fn open_view(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, view: View) {
    let mut st = state.borrow_mut();
    let Some(lib) = &st.index_reader else {
        // Asked before the indexer has opened the library: done when
        // it has.
        st.library.wanted = Some(view);
        app.set_status("opening the library index...".into());
        return;
    };
    let asked = Instant::now();
    let roots = roots_of(&st, &view);
    let files = match lib.paths_under(&roots) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("library: {e}");
            app.set_status(format!("the library index could not be read: {e}").into());
            st.library.awaiting = false;
            return;
        }
    };
    tracing::info!(
        "library: {} files under {} root(s), listed in {:.1} ms",
        files.len(),
        roots.len(),
        asked.elapsed().as_secs_f64() * 1e3
    );
    st.view_generation += 1;
    let generation = st.view_generation;
    if files.is_empty() {
        // Nothing indexed yet, most likely: the view is taken, empty,
        // and fills in as the launch pass reaches the roots.
        st.view = view;
        let empty = st.library.roots.is_empty();
        drop(st);
        open_loaded(state, app, worker, Vec::new(), Vec::new(), Vec::new(), 0);
        {
            let mut st = state.borrow_mut();
            st.library.awaiting = false;
            st.library.painted = Some((asked, "the view asked for"));
            show(&st, app);
        }
        app.set_status(
            if empty {
                "No folders in the library yet: add one to see it here"
            } else {
                "Nothing indexed under the roots yet; the pass is running"
            }
            .into(),
        );
        return;
    }
    let write = st.write_sidecars;
    // The frame on screen stays on screen when it is in the new list;
    // else the last one open, as a folder's open does.
    let last = st
        .current
        .and_then(|c| st.files.get(c))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| settings::Settings::load().last_file);
    st.library.loading = true;
    app.set_status(format!("reading {} frames...", files.len()).into());
    drop(st);
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard roots".into())
        .spawn(move || {
            let read = Instant::now();
            let (sidecars, seed) = load_sidecars_parallel(&files, write);
            let seconds = read.elapsed().as_secs_f64();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let (Some(state), Some(worker)) = (
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                if state.borrow().view_generation != generation {
                    return;
                }
                tracing::info!(
                    "library: {} sidecars read in {:.2} s on the pool",
                    files.len(),
                    seconds
                );
                let select = (!last.is_empty())
                    .then(|| greycard_library::key_path(Path::new(&last)))
                    .and_then(|last| files.iter().position(|f| *f == last))
                    .unwrap_or(0);
                {
                    let mut st = state.borrow_mut();
                    st.view = view;
                    st.library.loading = false;
                }
                open_loaded(&state, &app, &worker, files, sidecars, seed, select);
                let mut st = state.borrow_mut();
                st.library.awaiting = false;
                st.library.painted = Some((asked, "the view asked for"));
                tracing::info!(
                    "library: the view in the browser {:.0} ms after it was asked for",
                    asked.elapsed().as_secs_f64() * 1e3
                );
                show(&st, &app);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("library: {e}");
        let mut st = state.borrow_mut();
        st.library.awaiting = false;
        st.library.loading = false;
    }
}

/// How many files new to the list a merge reads on the window's
/// thread; past it the list is read again as the view's open reads
/// it, on the pool, the window going on meanwhile.
const MERGE_AT_MOST: usize = 500;

/// The browser's list read again, from the folder or from the index,
/// and merged into the window's.
pub(crate) fn refresh_view(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    reopen: bool,
) {
    let view = state.borrow().view.clone();
    if state.borrow().library.loading {
        return;
    }
    let files = {
        let st = state.borrow();
        match &view {
            View::Folder => {
                let Some(dir) = st
                    .files
                    .iter()
                    .enumerate()
                    .find(|(i, _)| Some(*i) != st.current)
                    .map(|(_, f)| f)
                    .or(st.files.first())
                    .and_then(|f| f.parent())
                    .map(Path::to_path_buf)
                else {
                    return;
                };
                // A list the desktop handed over, files from here and
                // there, is not a folder's to read again. The frame on
                // screen may be elsewhere, followed there by a move.
                let elsewhere = st
                    .files
                    .iter()
                    .enumerate()
                    .any(|(i, f)| Some(i) != st.current && f.parent() != Some(dir.as_path()));
                if elsewhere {
                    return;
                }
                match files::list_files(&dir) {
                    Ok(f) => f,
                    Err(_) => return,
                }
            }
            View::Roots(_) => {
                let Some(lib) = &st.index_reader else {
                    return;
                };
                match lib.paths_under(&roots_of(&st, &view)) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::debug!("library: {e}; the list kept");
                        return;
                    }
                }
            }
        }
    };
    // A root walked for the first time can bring thousands at once:
    // those are read on the pool, as the view's open reads them.
    let new = {
        let st = state.borrow();
        let have: std::collections::HashSet<&PathBuf> = st.files.iter().collect();
        files.iter().filter(|f| !have.contains(f)).count()
    };
    if new > MERGE_AT_MOST && matches!(view, View::Roots(_)) {
        if reopen {
            tracing::info!("library: {new} files new to the view; read again on the pool");
            open_view(state, app, worker, view);
        }
        return;
    }
    let next = merge(&mut state.borrow_mut(), app, worker, files);
    if let Some(row) = next {
        app.invoke_select(row as i32);
    }
}

/// The frame on screen, followed: when its file is not where the
/// window has it, and the index knows where the row it had went — a
/// move under a root, or a folder renamed — the window takes the new
/// path, so its next save lands beside the frame and not beside a
/// file that is gone. True when it moved.
pub(crate) fn follow_current(st: &mut State) -> bool {
    let Some(c) = st.current else {
        return false;
    };
    let Some(path) = st.files.get(c) else {
        return false;
    };
    if path.exists() {
        return false;
    }
    let (Some(lib), Some(Some(id))) = (&st.index_reader, st.index_ids.get(c)) else {
        return false;
    };
    let Ok(found) = lib.found_at(&[*id]) else {
        return false;
    };
    let Some(to) = found.get(id).filter(|p| p.is_file()) else {
        return false;
    };
    tracing::info!("{} moved to {}; followed", path.display(), to.display());
    st.files[c] = to.clone();
    true
}

/// Put `next` in the browser in place of the list it has, keeping
/// what each file that stays already has: its sidecar as edited, its
/// thumbnail, its row in the index and its place in the selection.
/// A file new to the list has its sidecar read and its thumbnail
/// asked for. The frame on screen stays on screen, followed to its
/// new path if it moved; if it is gone, the nearest row is what the
/// caller opens, which this returns.
pub(crate) fn merge(
    st: &mut State,
    app: &App,
    worker: &Worker,
    mut next: Vec<PathBuf>,
) -> Option<usize> {
    follow_current(st);
    // A frame on screen that moved out of what this list covers (a
    // folder view, and the frame moved to another folder) stays in
    // the list, where its name sorts: it is still what is being
    // edited, and it is where the index says it is.
    if let Some(c) = st.current
        && let Some(path) = st.files.get(c)
        && path.is_file()
        && !next.contains(path)
    {
        let at = next.partition_point(|p| p.file_name() < path.file_name());
        next.insert(at, path.clone());
    }
    if next == st.files {
        return None;
    }
    let started = Instant::now();
    let old: HashMap<&PathBuf, usize> = st.files.iter().enumerate().map(|(i, p)| (p, i)).collect();
    let from: Vec<Option<usize>> = next.iter().map(|p| old.get(p).copied()).collect();
    drop(old);
    let fresh: Vec<PathBuf> = next
        .iter()
        .zip(&from)
        .filter(|(_, f)| f.is_none())
        .map(|(p, _)| p.clone())
        .collect();
    let (mut loaded, mut seeded) = {
        let (s, b) = if fresh.len() > 200 {
            load_sidecars_parallel(&fresh, st.write_sidecars)
        } else {
            load_sidecars(&fresh, st.write_sidecars)
        };
        (s.into_iter(), b.into_iter())
    };
    let count = next.len();
    let mut sidecars = Vec::with_capacity(count);
    let mut seed_blend = Vec::with_capacity(count);
    let mut thumb_base = Vec::with_capacity(count);
    let mut thumb_shown = Vec::with_capacity(count);
    let mut thumb_made = Vec::with_capacity(count);
    let mut thumb_asked = Vec::with_capacity(count);
    let mut thumb_failed = Vec::with_capacity(count);
    let mut index_ids = Vec::with_capacity(count);
    for f in &from {
        match *f {
            Some(i) => {
                sidecars.push(std::mem::take(&mut st.sidecars[i]));
                seed_blend.push(st.seed_blend.get(i).copied().unwrap_or(false));
                thumb_base.push(st.thumb_base[i].take());
                thumb_shown.push(st.thumb_shown[i]);
                thumb_made.push(st.thumb_made[i]);
                thumb_asked.push(st.thumb_asked[i]);
                thumb_failed.push(st.thumb_failed.get(i).copied().unwrap_or(false));
                index_ids.push(st.index_ids.get(i).copied().flatten());
            }
            None => {
                sidecars.push(loaded.next().unwrap_or_default());
                seed_blend.push(seeded.next().unwrap_or(false));
                thumb_base.push(None);
                thumb_shown.push(None);
                thumb_made.push(0);
                thumb_asked.push(worker::THUMB_WIDTH);
                thumb_failed.push(false);
                index_ids.push(None);
            }
        }
    }
    let mut new_of: Vec<Option<usize>> = vec![None; st.files.len()];
    for (at, f) in from.iter().enumerate() {
        if let Some(i) = *f {
            new_of[i] = Some(at);
        }
    }
    let to_new = |i: usize| new_of.get(i).copied().flatten();
    // The rows on the window, in the new numbering, so the pictures
    // already on them are carried over.
    let rows: Vec<Option<usize>> = st.shown.iter().map(|&f| to_new(f)).collect();
    let old_current = st.current;
    let current = old_current.and_then(to_new);
    let renumbered = st.files.len() != count || from.iter().enumerate().any(|(i, f)| *f != Some(i));
    // What is keyed by the old numbering and cannot be carried over
    // by path cheaply: the culling pictures and the camera picture
    // standing in. Put down, and asked for again below.
    if renumbered {
        if let Some(cull) = st.cull.as_mut() {
            cull.cache.clear();
            cull.textures.clear();
            cull.failed.clear();
            cull.full_asked = None;
        }
        drop_placeholder(st, app);
        st.hold = None;
        st.prefetch.want(Vec::new());
    }
    st.picked = st.picked.iter().filter_map(|&i| to_new(i)).collect();
    let gone_row = old_current
        .filter(|_| current.is_none())
        .and_then(|c| st.shown.iter().position(|&f| f == c));
    st.files = next;
    st.sidecars = sidecars;
    st.seed_blend = seed_blend;
    st.thumb_base = thumb_base;
    st.thumb_shown = thumb_shown;
    st.thumb_made = thumb_made;
    st.thumb_asked = thumb_asked;
    st.thumb_failed = thumb_failed;
    st.index_ids = index_ids;
    st.index_passed = vec![true; count];
    st.index_pass_ready = false;
    st.current = current;
    crate::library::refresh_ids(st);
    let hidden = rebuild_browser_from(st, app, &rows);
    for (i, f) in from.iter().enumerate() {
        if f.is_none() {
            worker.send(Job::Thumbnail {
                index: i,
                path: st.files[i].clone(),
            });
        }
    }
    tracing::info!(
        "browser: the list merged, {} frames ({} new) in {:.1} ms",
        count,
        fresh.len(),
        started.elapsed().as_secs_f64() * 1e3
    );
    st.library.painted = Some((started, "the merge"));
    if st.cull.is_some()
        && let Some(c) = st.current
    {
        crate::panel::cull::cull_select(st, app, c);
    }
    match (current, gone_row) {
        // The frame on screen is gone from the disk: the nearest row
        // is opened, and nothing is saved for the file that went.
        (None, Some(row)) if !st.shown.is_empty() => Some(row.min(st.shown.len() - 1)),
        (None, _) if old_current.is_none() && !st.shown.is_empty() => Some(0),
        _ => hidden,
    }
}

/// What the indexer said of a pass in the background, on the UI
/// thread: the roots' counts again, and the list merged when the
/// pass touched what it shows. `partway` for a pass still going,
/// which merges what it has found so far but never reads the whole
/// list again for it, unless the list is empty: a large root walked
/// for the first time would otherwise have the view reopened every
/// two seconds.
pub(crate) fn background_done(
    state: &Rc<RefCell<State>>,
    app: &App,
    path: &Path,
    report: &greycard_library::Report,
    partway: bool,
) {
    let reopen = !partway || state.borrow().files.is_empty();
    let changed = report.added
        + report.moved
        + report.changed
        + report.missing
        + report.returned
        + report.meta_refreshed
        > 0;
    let touches = {
        let mut st = state.borrow_mut();
        recount(&mut st);
        show(&st, app);
        if !changed {
            return;
        }
        match &st.view {
            View::Roots(None) => true,
            View::Roots(Some(r)) => path.starts_with(r) || r.starts_with(path),
            View::Folder => {
                st.files
                    .first()
                    .and_then(|f| f.parent())
                    .and_then(|d| dunce::canonicalize(d).ok())
                    .is_some_and(|d| d.starts_with(path))
                    || st.current.is_some_and(|c| !st.files[c].exists())
            }
        }
    };
    if touches && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        refresh_view(state, app, &worker, reopen);
    }
}

/// The header's callbacks: a root chosen, all of them, one added by
/// the chooser or as the folder open, one taken out.
pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_library_root_picked(move |path| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let view = if path.is_empty() {
                View::Roots(None)
            } else {
                View::Roots(Some(PathBuf::from(path.as_str())))
            };
            open_view(&state, &app, &worker, view);
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_library_root_removed(move |path| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            remove(&state, &app, &worker, Path::new(path.as_str()));
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_library_root_add_open(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let dir = open_folder_to_add(&state.borrow());
            if let Some(dir) = dir {
                add(&state, &app, &worker, &dir);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_library_root_add(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let start = {
                let st = state.borrow();
                st.current
                    .and_then(|i| st.files.get(i))
                    .and_then(|f| f.parent())
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/"))
            };
            let app_weak = app.as_weak();
            app.set_status("choosing a folder to add to the library...".into());
            export::choose_folder("Add a folder to the library", start, move |chosen| {
                let app_weak = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    let (Some(state), Some(worker)) = (
                        crate::STATE.with(|s| s.borrow().clone()),
                        crate::WORKER.with(|w| w.borrow().clone()),
                    ) else {
                        return;
                    };
                    match chosen {
                        Ok(Some(dir)) => add(&state, &app, &worker, &dir),
                        Ok(None) => app.set_status("nothing added".into()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{state_for, window};
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    use std::time::Duration;

    /// A folder of this test's own, canonical, as the index keys
    /// folders (macOS's temporary directory is behind a link).
    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-roots-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    fn frames(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let p = dir.join(n);
                write_frame(&p, [&R5, &R6, &A7][i % 3], i as u16);
                p
            })
            .collect()
    }

    /// A list merged keeps what each frame that stays already has, by
    /// its path: the sidecar as edited in memory, the picture made,
    /// its place in the selection. A frame new to the list has its
    /// sidecar read from disk; one gone leaves the selection.
    #[test]
    fn a_list_merged_keeps_each_frames_own_by_its_path() {
        let dir = scratch("merge");
        let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.sidecars[1].meta.rating = 4;
            st.thumb_base[2] = Some((1, 1, vec![9, 9, 9]));
            st.thumb_made[2] = 128;
            st.current = Some(1);
            st.picked = vec![1, 2];
            rebuild_browser_from(&mut st, &app, &[]);
        }
        let aa = frames(&dir, &["aa.tif"]).remove(0);
        let mut s = Sidecar::default();
        s.meta.rating = 2;
        s.save(&aa).unwrap();
        let next = vec![
            files[0].clone(),
            aa.clone(),
            files[1].clone(),
            files[2].clone(),
        ];
        let row = merge(&mut state.borrow_mut(), &app, &worker, next.clone());
        {
            let st = state.borrow();
            assert_eq!(st.files, next);
            assert_eq!(st.sidecars[2].meta.rating, 4, "b's own, as edited");
            assert_eq!(st.sidecars[1].meta.rating, 2, "aa's, from its sidecar");
            assert_eq!(st.thumb_made[3], 128);
            assert!(st.thumb_base[3].is_some());
            assert_eq!(st.current, Some(2));
            assert_eq!(st.picked, vec![2, 3]);
            assert_eq!(row, None, "the frame on screen stays");
            assert_eq!(app.get_thumbs().row_count(), 4);
            assert_eq!(app.get_selected(), 2);
        }
        // c goes from the list: out of the selection too.
        let next = vec![files[0].clone(), aa.clone(), files[1].clone()];
        assert_eq!(merge(&mut state.borrow_mut(), &app, &worker, next), None);
        assert_eq!(state.borrow().picked, vec![2]);
        // The same list again is nothing to do.
        let same = state.borrow().files.clone();
        assert_eq!(merge(&mut state.borrow_mut(), &app, &worker, same), None);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The frame on screen deleted from under the window: the nearest
    /// row is what is opened next, and nothing is kept as current, so
    /// no sidecar is written for a file that is gone.
    #[test]
    fn a_frame_on_screen_that_is_gone_hands_on_to_the_nearest() {
        let dir = scratch("gone");
        let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.current = Some(2);
            rebuild_browser_from(&mut st, &app, &[]);
        }
        std::fs::remove_file(&files[2]).unwrap();
        let next = files[..2].to_vec();
        let row = merge(&mut state.borrow_mut(), &app, &worker, next);
        assert_eq!(row, Some(1), "c's row, clamped to the last");
        assert_eq!(state.borrow().current, None);
        assert!(!files[2].with_extension("tif.gcd").exists());
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A frame on screen renamed, or moved to another folder, is
    /// followed there by its row in the index: its path in the window
    /// is the new one, its edit in memory is kept, and in a folder's
    /// view it stays in the list though the folder no longer has it.
    #[test]
    fn a_frame_on_screen_that_moved_is_followed_to_its_new_path() {
        let dir = scratch("follow");
        let (shoot, other) = (dir.join("shoot"), dir.join("other"));
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let files = frames(&shoot, &["a.tif", "b.tif", "c.tif"]);
        frames(&other, &["o.tif"]);
        let db = dir.join("index").join("library.sqlite");
        let mut writer = greycard_library::Library::open(&db).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let app = window(3);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            crate::library::refresh_ids(&mut st);
            st.sidecars[1].meta.rating = 5;
            st.current = Some(1);
            rebuild_browser_from(&mut st, &app, &[]);
        }
        // Renamed within the folder.
        let renamed = shoot.join("bb.tif");
        std::fs::rename(&files[1], &renamed).unwrap();
        writer.index_folder(&shoot, &mut |_| {}).unwrap();
        let listed = crate::files::list_files(&shoot).unwrap();
        assert_eq!(merge(&mut state.borrow_mut(), &app, &worker, listed), None);
        {
            let st = state.borrow();
            let c = st.current.expect("still on screen");
            assert_eq!(st.files[c], renamed);
            assert_eq!(st.sidecars[c].meta.rating, 5);
        }
        // Moved to the other folder: followed, and kept in the list.
        let moved = other.join("bb.tif");
        std::fs::rename(&renamed, &moved).unwrap();
        writer.index_tree(&dir, &mut |_| {}).unwrap();
        let listed = crate::files::list_files(&shoot).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(merge(&mut state.borrow_mut(), &app, &worker, listed), None);
        {
            let st = state.borrow();
            let c = st.current.expect("still on screen");
            assert_eq!(st.files[c], moved);
            assert_eq!(st.files.len(), 3);
            assert_eq!(st.sidecars[c].meta.rating, 5);
        }
        // The window's thread keeps the state, and with it the
        // reader: closed by hand, since Windows will not remove a
        // folder with a database open in it.
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(writer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The roots' row: a chip a root with its count from the index,
    /// the all-roots chip with their sum, and "Add this folder" while
    /// the folder open is under none of them.
    #[test]
    fn the_header_lists_the_roots_with_their_counts() {
        let dir = scratch("header");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(a.join("day")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        frames(&a, &["x.tif"]);
        frames(&a.join("day"), &["y.tif", "z.tif"]);
        let open = frames(&b, &["o.tif"]);
        let db = dir.join("index").join("library.sqlite");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_tree(&dir, &mut |_| {})
            .unwrap();
        let app = window(1);
        let (state, _worker) = state_for(&app, open);
        let mut st = state.borrow_mut();
        st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.library.roots.add(&a).unwrap();
        recount(&mut st);
        show(&st, &app);
        let chips = app.get_library_roots();
        assert_eq!(chips.row_count(), 1);
        let chip = chips.row_data(0).unwrap();
        assert_eq!((chip.name.as_str(), chip.count, chip.on), ("a", 3, false));
        assert_eq!(app.get_library_all_count(), 3);
        assert!(!app.get_library_all_on());
        assert!(app.get_library_can_add_open(), "b is under no root");
        st.library.roots.add(&b).unwrap();
        st.view = View::Roots(None);
        recount(&mut st);
        show(&st, &app);
        assert_eq!(app.get_library_all_count(), 4);
        assert!(app.get_library_all_on());
        assert!(!app.get_library_can_add_open());
        assert_eq!(app.get_library_note(), "");
        st.index_reader = None;
        drop(st);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The row in the grid's header is wired: along it a press finds
    /// the all-roots chip, then the root's own, then its cross, then
    /// the chooser's chip, in that order.
    #[test]
    fn the_roots_row_hands_back_what_was_pressed() {
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 700.0));
        app.set_grid_open(true);
        app.set_library_roots(ModelRc::new(VecModel::from(vec![RootChip {
            name: "shoots".into(),
            path: "/x/shoots".into(),
            count: 12,
            on: false,
        }])));
        app.set_library_all_count(12);
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let s = seen.clone();
        app.on_library_root_picked(move |p| s.borrow_mut().push(format!("picked {p}")));
        let s = seen.clone();
        app.on_library_root_removed(move |p| s.borrow_mut().push(format!("removed {p}")));
        let s = seen.clone();
        app.on_library_root_add(move || s.borrow_mut().push("add".into()));
        let s = seen.clone();
        app.on_library_root_add_open(move || s.borrow_mut().push("add open".into()));
        // The row is the header's second, under the controls.
        for x in (0..600).step_by(3) {
            crate::testing::click(&app, x as f32, 56.0);
        }
        // In the order first met: the chip's own edge past its cross
        // picks it again, and that is the chip's.
        let mut said: Vec<String> = Vec::new();
        for s in seen.borrow().iter() {
            if !said.contains(s) {
                said.push(s.clone());
            }
        }
        assert_eq!(
            said,
            ["picked ", "picked /x/shoots", "removed /x/shoots", "add"],
            "{:?}",
            seen.borrow()
        );
    }

    /// The indexer's background: the launch pass over a root, and a
    /// change the watcher saw, each said when done — on a library of
    /// the test's own, beside which its roots would be kept.
    #[test]
    fn the_indexer_passes_over_the_roots_and_the_changes() {
        use crate::library::Told;
        let dir = scratch("indexer");
        let root = dir.join("root");
        std::fs::create_dir_all(root.join("day")).unwrap();
        frames(&root, &["a.tif"]);
        frames(&root.join("day"), &["b.tif", "c.tif"]);
        let db = dir.join("data").join("library.sqlite");
        assert_eq!(
            Roots::path_beside(&db),
            dir.join("data").join(greycard_library::roots::FILE_NAME)
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = crate::library::Indexer::start(db.clone(), move |told| {
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
        indexer.roots(vec![root.clone()]);
        match wait(&|t| matches!(t, Told::Background { .. })) {
            Told::Background {
                path,
                launch,
                report,
                error,
                ..
            } => {
                assert_eq!(path, root);
                assert!(launch);
                assert_eq!(error, None);
                assert_eq!(report.added, 3, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        frames(&root.join("day"), &["d.tif"]);
        indexer
            .asker()
            .changes(vec![greycard_library::Change::Folder(root.join("day"))]);
        match wait(&|t| matches!(t, Told::Background { .. })) {
            Told::Background {
                path,
                launch,
                report,
                ..
            } => {
                assert_eq!(path, root.join("day"));
                assert!(!launch);
                assert_eq!(report.added, 1, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        let reader = greycard_library::Library::open_read_only(&db).unwrap();
        assert_eq!(reader.count_under(&root).unwrap(), 4);
        drop(reader);
        // An asker still held, as the watcher holds one, does not keep
        // the thread from leaving.
        let _held = indexer.asker();
        indexer.stop(Duration::from_secs(20));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
