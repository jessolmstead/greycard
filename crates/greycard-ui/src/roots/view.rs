//! Opening a view or a folder: the roots it lists, its read sent off
//! the window's thread, and the bar while the read is out.

use super::*;
pub(crate) use greycard_library::grouped;

/// The roots a view lists, as the roots file has them. Which of them
/// are there is looked at off the window's thread, by [`Look::run`].
fn roots_of(st: &State, view: &View) -> Vec<PathBuf> {
    match view {
        View::Folder => Vec::new(),
        View::Roots(None) | View::Collection { .. } | View::Queue => {
            st.views.library.roots.list().to_vec()
        }
        View::Roots(Some(r)) | View::Branch { root: r, .. } => vec![r.clone()],
    }
}

/// The roots whose copies of local frames a view leaves out: the
/// archives, in a view of every root (§216). A view of the archive
/// alone shows all of it.
fn hidden_under(st: &State, view: &View) -> Vec<PathBuf> {
    match view {
        View::Roots(None) => st.views.library.roots.archives().to_vec(),
        _ => Vec::new(),
    }
}

/// Every root, and the view's own if it is not among them: each is
/// looked at off the window's thread, for its row.
fn every_root(st: &State, view: &View) -> Vec<PathBuf> {
    let mut all = st.views.library.roots.list().to_vec();
    for r in roots_of(st, view) {
        if !all.contains(&r) {
            all.push(r);
        }
    }
    all
}

/// The order `view` is read in, its choice as kept: none for a
/// collection in its own order.
fn order_of(st: &State, view: &View) -> Option<greycard_library::Order> {
    st.views.sort.choice(view).order()
}

/// The order a list that is not a collection's is read in. Its choice
/// is never the hand order (`sort::Kept::choice`); should it be, the
/// view's default stands.
fn listed_order(st: &State, view: &View) -> greycard_library::Order {
    order_of(st, view)
        .or_else(|| crate::sort::Slot::of(view).default_choice().order())
        .unwrap_or(greycard_library::Order::NAME)
}

/// The order a read of `view` is made in now: its choice, none for a
/// collection in its own order.
pub(super) fn read_order(st: &State, view: &View) -> Option<greycard_library::Order> {
    match view {
        View::Collection { .. } => order_of(st, view),
        // The queue's own order, always.
        View::Queue => None,
        view => Some(listed_order(st, view)),
    }
}

/// Show every file under the roots, or under one: the rows from the
/// index; the roots and folders looked at off the window's thread, each
/// frame standing in from its row with no sidecar read; and the
/// browser's list replaced once they are in. The window goes on with
/// what it has meanwhile. An open is asked for by hand, so every folder
/// is looked at again.
pub(crate) fn open_view(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, view: View) {
    open_view_on(state, app, worker, view, None);
}

/// [`open_view`], starting with `on` on the frame it names when the
/// view has it (with none named, on the first) rather than on the
/// frame on screen: the way back from the export queue's view, whose
/// frame is the queue's. False when no read was sent: the index not
/// open yet (the view opened once it is), the collection gone, or the
/// index unreadable.
pub(crate) fn open_view_on(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    view: View,
    on: Option<Option<PathBuf>>,
) -> bool {
    let mut st = state.borrow_mut();
    // The export queue's frames are its file's, not the index's: listed
    // as files given are, with or without an index.
    if view == View::Queue {
        crate::panel::queue_view::entering(&mut st);
        let (frames, listed) = crate::panel::queue_view::listing(&st);
        let count = frames.len();
        let source = Source::Queue { frames, listed };
        drop(st);
        send_view(
            state,
            app,
            view,
            source,
            count,
            Instant::now(),
            None,
            None,
            on,
        );
        return true;
    }
    let Some(lib) = &st.views.index_reader else {
        // Asked before the indexer has opened the library: done when
        // it has.
        st.views.library.wanted = Some(view);
        app.set_status("opening the library index...".into());
        return false;
    };
    let asked = Instant::now();
    let order = order_of(&st, &view);
    if let View::Collection { id } = &view {
        let Some(c) = st.views.collections.collection(id) else {
            app.set_status("that collection is gone".into());
            return false;
        };
        let refs = c.refs.clone();
        let archives = st.views.library.roots.archives().to_vec();
        // Found off the window's thread when the window knows where the
        // index is; here only for a window with a reader and no path
        // (the tests').
        let shown = if st.views.index_path.is_some() {
            None
        } else {
            let away: Vec<PathBuf> = st.views.library.offline.iter().cloned().collect();
            match crate::collections::resolve(lib, &refs, &away, &archives, order) {
                Ok(shown) => Some(shown),
                Err(e) => {
                    tracing::warn!("library: {e}");
                    app.set_status(format!("the library index could not be read: {e}").into());
                    loading_done(&mut st.views.library, app);
                    return false;
                }
            }
        };
        let count = refs.len();
        let source = Source::Collection {
            id: id.clone(),
            refs,
            archives,
            shown,
        };
        drop(st);
        send_view(state, app, view, source, count, asked, None, order, on);
        return true;
    }
    let order = listed_order(&st, &view);
    let (roots, only) = view.listed(st.views.library.roots.list());
    let at = match roots.as_slice() {
        [one] => one_root_named(&st.views.library.roots, [one.as_path()]),
        _ => None,
    };
    // The rows are read off the window's thread, on a connection of the
    // read's own, when the window knows where the index is; here only
    // their count, for the status line, from the folder index's range.
    // A window with a reader and no path (the tests') reads them here.
    let (rows, count) = if st.views.index_path.is_some() {
        (None, count_listed(lib, &roots, only))
    } else {
        match rows_listed(lib, &roots, only, order) {
            Ok(rows) => {
                let count = rows.len();
                (Some(rows), count)
            }
            Err(e) => {
                tracing::warn!("library: {e}");
                app.set_status(format!("the library index could not be read: {e}").into());
                st.views.library.awaiting = false;
                loading_done(&mut st.views.library, app);
                return false;
            }
        }
    };
    tracing::info!(
        "library: {} files under {} root(s), counted in {:.1} ms",
        count,
        roots.len(),
        asked.elapsed().as_secs_f64() * 1e3
    );
    let source = Source::Roots {
        roots,
        only,
        rows,
        hide: hidden_under(&st, &view),
    };
    drop(st);
    send_view(state, app, view, source, count, asked, at, Some(order), on);
    true
}

/// Nothing open, as a window with no folder is: the list emptied, the
/// view a folder's, and a read still out dropped. The way back from
/// the export queue's view when nothing was open before it.
pub(crate) fn open_nothing(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    {
        let mut st = state.borrow_mut();
        st.views.view_generation += 1;
        loading_done(&mut st.views.library, app);
        crate::panel::browser::save_leaving(&mut st, app);
        st.views.view = View::Folder;
        // A view's own frames and the queue's listing go with it: no
        // read lands here to replace them.
        st.views.collection_frames = None;
        st.views.queue_listed = None;
    }
    open_loaded(state, app, worker, Vec::new(), Vec::new(), true, 0);
    let mut st = state.borrow_mut();
    crate::tree::want(&mut st, app);
    show(&st, app);
}

/// A view's read sent off the window's thread: the frame on screen (or
/// the one `on` names, or the first when it names none) kept when it is
/// in the new list, the folders
/// looked at again, and the bar put up if the read is slow. `order` is
/// what it is read in.
#[allow(clippy::too_many_arguments)]
fn send_view(
    state: &Rc<RefCell<State>>,
    app: &App,
    view: View,
    source: Source,
    count: usize,
    asked: Instant,
    at: Option<String>,
    order: Option<greycard_library::Order>,
    on: Option<Option<PathBuf>>,
) {
    let mut st = state.borrow_mut();
    st.views.view_generation += 1;
    // The frame on screen stays on screen when it is in the new list;
    // else the last one open, as a folder's open does.
    let last = if count == 0 {
        None
    } else {
        match on {
            Some(on) => on,
            None => st.current.and_then(|c| st.path_at(c)).cloned().or_else(|| {
                let last = settings::Settings::load().last_file;
                (!last.is_empty()).then(|| PathBuf::from(last))
            }),
        }
    };
    st.views.library.readable.clear();
    st.views.library.readable_epoch += 1;
    app.set_status(format!("reading {} frames...", count).into());
    // No sidecar is read for a view of the roots; the count is the
    // roots' and the folders' looks until the read says otherwise.
    let progress = Arc::new(Progress::default());
    let look = Look {
        generation: st.views.view_generation,
        token: 0,
        epoch: st.views.library.readable_epoch,
        roots: every_root(&st, &view),
        source,
        known: HashSet::new(),
        have: HashSet::new(),
        write: st.window.write_sidecars,
        on_screen: None,
        index: st.views.index_path.clone(),
        progress: Some(progress.clone()),
        apart: st.views.jpegs_apart,
        order,
        purpose: Purpose::Open {
            view,
            asked,
            select: Select::Last(last),
        },
    };
    drop(st);
    loading_started(state, app, progress, at);
    if !send_off(app, look) {
        let mut st = state.borrow_mut();
        st.views.library.awaiting = false;
        loading_done(&mut st.views.library, app);
    }
}

/// Open a folder's files, or files given, in the browser: listed and
/// held off the window's thread (each frame from its row where the
/// index has one, its sidecar read on the pool otherwise, with the bar
/// up meanwhile), and the browser's list replaced once they are in,
/// `select` opened. The view stays what it was until then: a folder
/// that turns out empty leaves the list alone. No sidecar is read on
/// the window's thread.
pub(crate) fn open_listing(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    listing: Listing,
    select: Select,
) {
    let mut st = state.borrow_mut();
    let asked = Instant::now();
    // A view of the roots still being read is dropped by the
    // generation, and nothing is loading any more.
    st.views.view_generation += 1;
    loading_done(&mut st.views.library, app);
    let (source, what) = match listing {
        Listing::Folder(dir) => {
            let what = dir.display().to_string();
            (Source::Folder(dir), what)
        }
        Listing::Files(files) => {
            let what = match files.as_slice() {
                [one] => crate::panel::browser::file_name(one),
                many => format!("{} files", many.len()),
            };
            (Source::Files(files), what)
        }
    };
    app.set_status(format!("opening {what}...").into());
    let at = match &source {
        Source::Folder(dir) => one_root_named(&st.views.library.roots, [dir.as_path()]),
        Source::Files(files) => {
            one_root_named(&st.views.library.roots, files.iter().map(PathBuf::as_path))
        }
        _ => None,
    };
    // The bar's count, until the read says how many sidecars it reads
    // from disk: every file given might be one; a folder's are not
    // known until it is listed.
    let progress = Arc::new(Progress {
        total: AtomicUsize::new(match &source {
            Source::Files(files) => files.len(),
            _ => 0,
        }),
        read: AtomicUsize::new(0),
    });
    let look = Look {
        generation: st.views.view_generation,
        token: 0,
        epoch: st.views.library.readable_epoch,
        roots: Vec::new(),
        source,
        known: HashSet::new(),
        have: HashSet::new(),
        write: st.window.write_sidecars,
        on_screen: None,
        index: st.views.index_path.clone(),
        progress: Some(progress.clone()),
        apart: st.views.jpegs_apart,
        order: Some(listed_order(&st, &View::Folder)),
        purpose: Purpose::Open {
            view: View::Folder,
            asked,
            select,
        },
    };
    drop(st);
    loading_started(state, app, progress, at);
    if !send_off(app, look) {
        let mut st = state.borrow_mut();
        loading_done(&mut st.views.library, app);
        app.set_status("the folder could not be read: no thread to read it on".into());
    }
}

/// A view's read sent: `loading` set, and the window's timer started
/// on its count. The bar shows once the read has run past
/// [`LOADING_SHOWN_AFTER`], and goes when it lands or is given up on.
/// `at` is the one root the read is under, named, for the bar to say
/// when the count stands still.
fn loading_started(
    state: &Rc<RefCell<State>>,
    app: &App,
    progress: Arc<Progress>,
    at: Option<String>,
) {
    let mut st = state.borrow_mut();
    let lib = &mut st.views.library;
    let now = Instant::now();
    lib.loading = true;
    lib.loading_since = Some(now);
    // The count as it starts, so the first tick does not take the
    // total it was sent with for movement.
    lib.progress_seen = (
        progress.read.load(Ordering::Relaxed),
        progress.total.load(Ordering::Relaxed),
    );
    lib.progress = Some(progress);
    lib.progress_moved = Some(now);
    lib.progress_counted = Some(now);
    lib.loading_where = at;
    app.set_loading_shown(false);
    let (state_weak, app_weak) = (Rc::downgrade(state), app.as_weak());
    lib.loading_timer
        .start(slint::TimerMode::Repeated, LOADING_TICK, move || {
            if let (Some(state), Some(app)) = (state_weak.upgrade(), app_weak.upgrade()) {
                loading_tick(&mut state.borrow_mut(), &app, Instant::now());
            }
        });
}

/// The timer's tick while a view's read is out, at `now`: the bar
/// filled to the sidecars read, once the read has been out long enough
/// to show it. A read past [`READ_GIVES_UP`] whose count has not moved
/// in [`LOADING_STALLED`] is given up on here as `refresh_view` would,
/// since with no report coming nothing else would.
pub(crate) fn loading_tick(st: &mut State, app: &App, now: Instant) {
    let lib = &mut st.views.library;
    let (true, Some(since), Some(progress)) = (lib.loading, lib.loading_since, &lib.progress)
    else {
        loading_done(lib, app);
        return;
    };
    let total = progress.total.load(Ordering::Relaxed);
    let read = progress.read.load(Ordering::Relaxed).min(total);
    if (read, total) != lib.progress_seen {
        if read != lib.progress_seen.0 {
            lib.progress_moved = Some(now);
        }
        lib.progress_seen = (read, total);
        lib.progress_counted = Some(now);
    }
    let out = now.saturating_duration_since(since);
    let still = now.saturating_duration_since(lib.progress_moved.unwrap_or(since));
    let settled = now.saturating_duration_since(lib.progress_counted.unwrap_or(since));
    if out > READ_GIVES_UP && still > LOADING_STALLED {
        give_up_view(st, app);
        return;
    }
    if out < LOADING_SHOWN_AFTER {
        return;
    }
    let fraction = if total == 0 {
        0.0
    } else {
        read as f32 / total as f32
    };
    app.set_loading_fraction(fraction);
    let waiting = (settled >= LOADING_WAITING_AFTER && read < total)
        .then_some(lib.loading_where.as_deref())
        .flatten();
    app.set_loading_line(loading_words(read, total, waiting).into());
    if !app.get_loading_shown() {
        tracing::info!(
            "library: the view's bar shown {:.0} ms after it was asked for, {read} of {total} read",
            out.as_secs_f64() * 1e3
        );
        app.set_loading_shown(true);
    }
}

/// The view's read done, dropped or given up on: nothing loading, the
/// timer stopped and the bar gone.
pub(crate) fn loading_done(lib: &mut Library, app: &App) {
    lib.loading = false;
    lib.loading_since = None;
    lib.progress = None;
    lib.progress_moved = None;
    lib.progress_counted = None;
    lib.loading_where = None;
    lib.loading_timer.stop();
    app.set_loading_shown(false);
}

/// A view's read given up on: the bar gone, a capture waiting on it let
/// go, and the read dropped by its generation if it ever lands.
fn give_up_view(st: &mut State, app: &App) {
    tracing::warn!("library: the view's read has not come in; given up");
    loading_done(&mut st.views.library, app);
    st.views.library.awaiting = false;
    st.views.view_generation += 1;
    app.set_status("the library's frames did not come in; the list is as it was".into());
}

/// The bar's words: "Reading sidecars… 3,400 of 11,711", or "Waiting
/// on Archive…" when `waiting` names the root a stalled read is under.
pub(crate) fn loading_words(read: usize, total: usize, waiting: Option<&str>) -> String {
    if total == 0 {
        // Nothing read from disk: the roots and the folders being
        // looked at, up to their wait each.
        return "Looking at the library's folders…".to_string();
    }
    if let Some(root) = waiting {
        return format!("Waiting on {root}…");
    }
    format!("Reading sidecars… {} of {}", grouped(read), grouped(total))
}

/// The read for the browser's list as it is, from the folder or from
/// the index. None when there is nothing to read it from.
pub(super) fn ask(st: &State) -> Option<Look> {
    let (source, roots) = match &st.views.view {
        View::Folder => {
            let dir = st
                .files
                .iter()
                .enumerate()
                .find(|(i, _)| Some(*i) != st.current)
                .map(|(_, f)| f)
                .or(st.files.first())
                .and_then(|f| f.parent())
                .map(Path::to_path_buf)?;
            // A list the desktop handed over, files from here and
            // there, is not a folder's to read again. The frame on
            // screen may be elsewhere, followed there by a move.
            let elsewhere = st
                .files
                .iter()
                .enumerate()
                .any(|(i, f)| Some(i) != st.current && f.parent() != Some(dir.as_path()));
            if elsewhere {
                return None;
            }
            // A folder's view looks at no root: a root gone away (a
            // share that does not answer) is none of its business.
            (Source::Folder(dir), Vec::new())
        }
        View::Roots(_) | View::Branch { .. } => {
            let lib = st.views.index_reader.as_ref()?;
            let (roots, only) = st.views.view.listed(st.views.library.roots.list());
            // The rows off the window's thread when the index's path is
            // known, as `open_view` has them; else read here.
            let rows = if st.views.index_path.is_some() {
                None
            } else {
                match rows_listed(lib, &roots, only, listed_order(st, &st.views.view)) {
                    Ok(rows) => Some(rows),
                    Err(e) => {
                        tracing::debug!("library: {e}; the list kept");
                        return None;
                    }
                }
            };
            (
                Source::Roots {
                    roots,
                    only,
                    rows,
                    hide: hidden_under(st, &st.views.view),
                },
                every_root(st, &st.views.view),
            )
        }
        View::Queue => {
            let (frames, listed) = crate::panel::queue_view::listing(st);
            (
                Source::Queue { frames, listed },
                every_root(st, &st.views.view),
            )
        }
        View::Collection { id } => {
            let lib = st.views.index_reader.as_ref()?;
            let refs = st.views.collections.collection(id)?.refs.clone();
            let archives = st.views.library.roots.archives().to_vec();
            let shown = if st.views.index_path.is_some() {
                None
            } else {
                let away: Vec<PathBuf> = st.views.library.offline.iter().cloned().collect();
                let order = order_of(st, &st.views.view);
                match crate::collections::resolve(lib, &refs, &away, &archives, order) {
                    Ok(shown) => Some(shown),
                    Err(e) => {
                        tracing::debug!("library: {e}; the list kept");
                        return None;
                    }
                }
            };
            (
                Source::Collection {
                    id: id.clone(),
                    refs,
                    archives,
                    shown,
                },
                every_root(st, &st.views.view),
            )
        }
    };
    let known = match source {
        Source::Roots { .. } | Source::Collection { .. } => st.views.library.readable.clone(),
        Source::Folder(_) | Source::Files(_) | Source::Queue { .. } => HashSet::new(),
    };
    Some(Look {
        purpose: Purpose::Merge,
        generation: st.views.view_generation,
        token: st.views.library.merge_token,
        epoch: st.views.library.readable_epoch,
        source,
        roots,
        known,
        have: st.files.iter().cloned().collect(),
        write: st.window.write_sidecars,
        on_screen: st.current.and_then(|c| {
            Some((
                st.path_at(c)?.clone(),
                st.views.index_ids.get(st.file_at(c)?).copied().flatten(),
            ))
        }),
        index: st.views.index_path.clone(),
        progress: None,
        apart: st.views.jpegs_apart,
        order: read_order(st, &st.views.view),
    })
}

/// [`refresh_view`] for a list whose view has just changed: a read of
/// the list still out is the view before's, so it is let go (it lands
/// as given up, by its token) and the new view's sent at once rather
/// than after it. A view still opening sends nothing, as
/// [`refresh_view`] has it, and the read out is then still out: its
/// mark is put back.
pub(crate) fn read_view_now(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> bool {
    let was = {
        let mut st = state.borrow_mut();
        (
            std::mem::take(&mut st.views.library.merging),
            st.views.library.merge_since.take(),
        )
    };
    if refresh_view(state, app, worker) {
        return true;
    }
    let mut st = state.borrow_mut();
    if st.views.library.loading {
        (st.views.library.merging, st.views.library.merge_since) = was;
    }
    false
}

/// The browser's list read again, from the folder or from the index,
/// and merged into the window's. The read is off the window's thread,
/// and the new files' sidecars with it. One is out at a time: a
/// refresh asked for meanwhile, however many, is done once when it
/// lands (`stale`). A read out longer than [`READ_GIVES_UP`] (a share
/// that stopped answering mid-read) is given up on, and another sent.
/// True when a merge is coming; false when there was nothing to read
/// the list from.
pub(crate) fn refresh_view(state: &Rc<RefCell<State>>, app: &App, _worker: &Rc<Worker>) -> bool {
    let look = {
        let mut st = state.borrow_mut();
        let late = |since: Option<Instant>| since.is_some_and(|t| t.elapsed() > READ_GIVES_UP);
        if st.views.library.loading {
            if !late(st.views.library.loading_since) {
                return false;
            }
            give_up_view(&mut st, app);
        }
        if st.views.library.merging {
            if !late(st.views.library.merge_since) {
                st.views.library.stale = true;
                return true;
            }
            tracing::warn!("library: a read of the list has not come in; another sent");
        }
        st.views.library.merge_token += 1;
        let Some(look) = ask(&st) else {
            st.views.library.merging = false;
            st.views.library.merge_since = None;
            return false;
        };
        st.views.library.merging = true;
        st.views.library.merge_since = Some(Instant::now());
        st.views.library.stale = false;
        look
    };
    if !send_off(app, look) {
        let mut st = state.borrow_mut();
        st.views.library.merging = false;
        st.views.library.merge_since = None;
        return false;
    }
    true
}
