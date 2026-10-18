//! An open list kept up with the disk: the passes said, the frame on
//! screen followed, the pictures owed asked for, and the list merged.

use super::*;

/// A pass has finished under `path`: what the window knew of the
/// folders under it is forgotten, to be looked at again by the next
/// read off the window's thread, and a read out now does not put back
/// what it saw of them.
pub(crate) fn folders_passed(st: &mut State, path: &Path) {
    let lib = &mut st.views.library;
    lib.readable.retain(|dir| !dir.starts_with(path));
    lib.readable_epoch += 1;
    // A read out now may have seen the folders before the pass did: one
    // more is sent when it lands.
    if lib.merging {
        lib.stale = true;
    }
    lib.passes.push((lib.readable_epoch, path.to_path_buf()));
    if lib.passes.len() > PASSES_KEPT {
        let (dropped, _) = lib.passes.remove(0);
        lib.passes_from = dropped;
    }
}

/// The frame on screen, followed: when its file is not where the
/// window has it, and the index knows where the row it had went — a
/// move under a root, or a folder renamed — the window takes the new
/// path, so its next save lands beside the frame and not beside a
/// file that is gone. Never to a path the window already lists, which
/// would put one file in the list twice and the frame's edit over the
/// other's sidecar. By what `seen` found of the frame off the window's
/// thread. True when it moved.
pub(crate) fn follow_current(st: &mut State, seen: &OnScreen) -> bool {
    let Some(c) = st.current else {
        return false;
    };
    let Some(file) = st.file_at(c) else {
        return false;
    };
    let path = &st.files[file];
    if !seen.gone(path) {
        return false;
    }
    let Some(to) = seen.moved_to.as_ref().filter(|p| !st.files.contains(p)) else {
        return false;
    };
    tracing::info!("{} moved to {}; followed", path.display(), to.display());
    // A panel standing in for the frame follows it under its new name.
    if st.views.panel_stand_in.as_ref() == Some(path) {
        st.views.panel_stand_in = Some(to.clone());
    }
    // The file's path, which every cell of it reaches through its file.
    st.move_file(file, to.clone());
    true
}

/// The frames still without a picture, to be asked for: after a list
/// is renumbered, every job queued names a file by a number that now
/// means another, and is thrown away when it arrives.
pub(crate) fn owed_thumbnails(st: &State) -> Vec<(usize, crate::versions::Key)> {
    (0..st.cells.len())
        .filter(|&c| {
            st.views.thumb_base.get(c).is_some_and(Option::is_none)
                && !st.views.thumb_failed.get(c).copied().unwrap_or(false)
        })
        .filter_map(|c| Some((c, st.key_of(c)?)))
        .collect()
}

/// Every frame still without a picture asked for again, in place of
/// whatever was queued: those whose file can be read through the pool,
/// the ones on screen first; those under a root that is offline from
/// the cache alone.
pub(crate) fn ask_owed(st: &State, worker: &Worker) {
    let (offline, online): (Vec<_>, Vec<_>) = owed_thumbnails(st)
        .into_iter()
        .partition(|(i, _)| rows::is_offline(st, *i));
    worker.replace_thumbnails(online, on_screen(st));
    for (i, _) in offline {
        worker.send(rows::thumb_job(st, i));
    }
}

/// The files the grid shows now, first and last, for their pictures
/// to be made first; none while the grid has not said.
pub(crate) fn on_screen(st: &State) -> Option<(usize, usize)> {
    let (first, last) = st.views.grid_shown?;
    let first = *st.views.shown.get(usize::try_from(first).ok()?)?;
    let last = *st.views.shown.get(
        usize::try_from(last)
            .ok()?
            .min(st.views.shown.len().checked_sub(1)?),
    )?;
    Some((first, last))
}

/// Put `next` in the browser in place of the list it has, keeping
/// what each file that stays already has: its sidecar as edited, its
/// picture, its row in the index and its place in the selection. A
/// file new to the list has its sidecar from what the read `brought`
/// off the window's thread, and every file its row from there too; a
/// frame whose sidecar stands in from its row takes the row's meta as
/// the read found it. The merge itself asks the disk nothing. Every
/// frame without a picture is asked for one again, in place of
/// whatever was queued. A path twice in `next` is kept once. The frame
/// on screen stays on screen, followed to its new path if the read
/// found it moved; if it is gone, nothing is kept for it and the
/// nearest row is what the caller opens, which this returns; beside
/// it, whether the browser's rows were rebuilt.
pub(crate) fn merge(
    st: &mut State,
    app: &App,
    worker: &Worker,
    next: Vec<PathBuf>,
    brought: Brought,
) -> (Option<usize>, bool) {
    let Brought {
        mut read,
        seen,
        rows,
    } = brought;
    let seen = &seen;
    // The rows as the read found them, into every frame that stays
    // and whose sidecar stands in: a sidecar changed on disk and read
    // by a pass shows here, whether or not the list changed.
    let mut moved = false;
    if let Some(rows) = &rows {
        let hits: Vec<(usize, &RowMeta)> = st
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, p)| rows.get(p).map(|r| (i, r)))
            .collect();
        for (i, row) in hits {
            moved |= rows::apply_row(st, i, row);
        }
    }
    let followed = follow_current(st, seen);
    // The frame on screen gone and not followed anywhere is not in the
    // list, whatever the index still says of it: the last file of a
    // folder deleted leaves its row standing (§160's empty folder).
    let gone = st
        .current
        .and_then(|c| st.path_at(c))
        .filter(|p| !followed && seen.gone(p))
        .cloned();
    let mut listed = HashSet::new();
    let mut next: Vec<PathBuf> = next
        .into_iter()
        .filter(|p| Some(p) != gone.as_ref())
        .filter(|p| listed.insert(p.clone()))
        .collect();
    // A frame on screen that moved out of what this list covers (a
    // folder view, and the frame moved to another folder) stays in
    // the list, where its name sorts: it is still what is being
    // edited, and it is where the index says it is.
    if let Some(c) = st.current
        && let Some(path) = st.path_at(c)
        && gone.as_ref() != Some(path)
        && !listed.contains(path)
    {
        let at = next.partition_point(|p| p.file_name() < path.file_name());
        next.insert(at, path.clone());
    }
    if next == st.files {
        if moved {
            return (rebuild_browser(st, app), true);
        }
        return (None, false);
    }
    let started = Instant::now();
    let old: HashMap<&PathBuf, usize> = st.files.iter().enumerate().map(|(i, p)| (p, i)).collect();
    let from: Vec<Option<usize>> = next.iter().map(|p| old.get(p).copied()).collect();
    drop(old);
    let fresh = from.iter().filter(|f| f.is_none()).count();
    let count = next.len();
    // The files' own vectors, each file that stays carrying what it
    // holds; the cells and their pictures follow (`versions::relist`).
    let mut sidecars = Vec::with_capacity(count);
    let mut from_row = Vec::with_capacity(count);
    let mut index_ids = Vec::with_capacity(count);
    for (path, f) in next.iter().zip(&from) {
        match *f {
            Some(i) => {
                sidecars.push(std::mem::take(&mut st.sidecars[i]));
                from_row.push(std::mem::take(&mut st.from_row[i]));
                let old = st.views.index_ids.get(i).copied().flatten();
                index_ids.push(match &rows {
                    Some(m) => m.get(path).map(|r| r.id),
                    None => old,
                });
            }
            None => {
                let held = read.remove(path).unwrap_or_else(|| Held {
                    sidecar: Sidecar::default(),
                    from_row: rows::FromRow::read(),
                    id: None,
                    trouble: None,
                });
                sidecars.push(held.sidecar);
                from_row.push(held.from_row);
                index_ids.push(rows.as_ref().and_then(|m| m.get(path).map(|r| r.id)));
            }
        }
    }
    let mut new_of: Vec<Option<usize>> = vec![None; st.files.len()];
    for (at, f) in from.iter().enumerate() {
        if let Some(i) = *f {
            new_of[i] = Some(at);
        }
    }
    let old_current = st.current;
    // Where the frame on screen's row was, for when its cell is gone.
    let old_row = old_current.and_then(|c| st.views.shown.iter().position(|&f| f == c));
    st.files = next;
    st.sidecars = sidecars;
    st.from_row = from_row;
    st.views.index_ids = index_ids;
    st.views.index_passed = vec![true; count];
    st.views.index_pass_ready = false;
    // The cells by the new files, each that stays with its picture and
    // its place in the selection; a cell gone (its file, or a version
    // the read found taken out) takes nothing with it. The numbering
    // moved puts down what is keyed by it, asked for again below.
    let compared = crate::versions::compared_file(st);
    let renumbered =
        crate::versions::relist(st, app, worker, &|i| new_of.get(i).copied().flatten());
    let current = st.current;
    let gone_row = old_row.filter(|_| current.is_none());
    if rows.is_none() {
        crate::library::refresh_ids(st);
    }
    // Which frames show their edit, before any picture is asked for.
    crate::edited::note_list(st, worker);
    let hidden = rebuild_browser(st, app);
    if renumbered {
        ask_owed(st, worker);
    }
    tracing::info!(
        "browser: the list merged, {} frames ({} new) in {:.1} ms",
        count,
        fresh,
        started.elapsed().as_secs_f64() * 1e3
    );
    st.views.library.painted = Some((started, "the merge"));
    if st.culling.cull.is_some()
        && let Some(c) = st.current
    {
        crate::panel::cull::cull_select(st, app, c);
    }
    crate::versions::refit_compare(st, app, compared);
    let next = match (current, gone_row) {
        // The frame on screen is gone from the disk: the nearest row
        // is opened, and nothing is saved for the file that went.
        (None, Some(row)) if !st.views.shown.is_empty() => Some(row.min(st.views.shown.len() - 1)),
        (None, _) if old_current.is_none() && !st.views.shown.is_empty() => Some(0),
        _ => hidden,
    };
    (next, true)
}

/// What the indexer said of a pass in the background, on the UI
/// thread: the roots' counts again, the launch pass counted down, the
/// pictures of files the pass found changed asked for again, and the
/// list merged when the pass touched what it shows. True when the
/// list was read again, which reads the index's rows and facets with
/// it.
pub(crate) fn background_done(
    state: &Rc<RefCell<State>>,
    app: &App,
    path: &Path,
    report: &greycard_library::Report,
    launch: bool,
    failed: bool,
) -> bool {
    let changed = report.added
        + report.moved
        + report.changed
        + report.missing
        + report.returned
        + report.meta_refreshed
        > 0;
    let launch_done = {
        let mut st = state.borrow_mut();
        let mut done = false;
        if launch {
            st.views.library.launch_left = st.views.library.launch_left.saturating_sub(1);
            if st.views.library.launch_left == 0 {
                // A capture waiting on the pass goes: what it found is
                // merged below, a frame is opened from it, and the
                // capture waits for that frame's develop as ever.
                st.views.library.awaiting = false;
                done = true;
            }
        }
        recount(&mut st);
        // The open root's tree built again when the pass changed which
        // files are where under it: a folder added, emptied or moved. A
        // sidecar's change (a rating) moves no count.
        if report.added + report.moved + report.missing + report.returned > 0 {
            crate::tree::passed(&mut st, path);
        }
        crate::tree::want(&mut st, app);
        show(&st, app);
        // A file changed on disk (a copy that finished) has another
        // picture: asked for again, those files and no others.
        let again = changed_rows(&st, report);
        if !again.is_empty()
            && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone())
        {
            for file in again {
                for c in st.cells_of_file(file) {
                    worker.send(rows::thumb_job(&st, c));
                }
            }
        }
        done
    };
    // Whether the list is to be read again, by what the window holds
    // and what the pass said, the disk not asked: the read does that
    // off the window's thread. A pass under the frame on screen reads
    // it again whatever the pass says, since the frame may be gone: the
    // last file of a folder deleted leaves the folder empty, which the
    // index takes for a drive not mounted (§160) and marks nothing, so
    // no count moves. So does a pass that could not read a folder,
    // whose files leave the list.
    let touches = {
        let st = state.borrow();
        let under_current = st
            .current
            .and_then(|c| st.path_at(c))
            .is_some_and(|p| p.starts_with(path));
        // A pass that failed outright (a watcher's pass over the folder
        // that was just locked fails at that folder, and says nothing
        // in its report), or one over a folder the last read could not
        // read, may have changed which folders can be read.
        let over_unreadable = st
            .views
            .library
            .unreadable
            .iter()
            .any(|d| d.starts_with(path) || path.starts_with(d));
        let worth =
            changed || under_current || failed || over_unreadable || !report.unreadable.is_empty();
        match st.views.view.clone() {
            // A collection's frames can be under any root.
            View::Roots(None) | View::Collection { .. } => worth,
            View::Roots(Some(r)) => worth && (path.starts_with(&r) || r.starts_with(path)),
            // A folder's own list changes only by a pass over it or over
            // a folder above it; with those under it, by one under it too.
            View::Branch { folder, deep, .. } => {
                worth
                    && (folder.starts_with(path)
                        || if deep {
                            path.starts_with(&folder)
                        } else {
                            path == folder.as_path()
                        })
            }
            // The folder's own list is what the read brings, the frame
            // on screen with it. A folder whose canonical form the
            // window does not know yet is read: the read brings that.
            View::Folder => st.files.first().and_then(|f| f.parent()).is_some_and(|d| {
                st.views
                    .library
                    .canonical
                    .get(d)
                    .is_none_or(|key| key.starts_with(path))
            }),
        }
    };
    let merged = touches
        && crate::WORKER
            .with(|w| w.borrow().clone())
            .is_some_and(|worker| refresh_view(state, app, &worker));
    if launch_done {
        let empty = {
            let st = state.borrow();
            matches!(st.views.view, View::Roots(_))
                && st.files.is_empty()
                && !st.views.library.loading
                && !st.views.library.merging
        };
        if empty {
            empty_view(&mut state.borrow_mut(), app);
        }
    }
    merged
}

/// The rows of the files a pass found changed, by the paths it names:
/// the list's own spelling first, else the index's canonical one for a
/// file of the same name.
pub(crate) fn changed_rows(st: &State, report: &greycard_library::Report) -> Vec<usize> {
    if report.changed_files.is_empty() {
        return Vec::new();
    }
    let changed: HashSet<&PathBuf> = report.changed_files.iter().collect();
    let names: HashSet<&std::ffi::OsStr> = report
        .changed_files
        .iter()
        .filter_map(|p| p.file_name())
        .collect();
    st.files
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            changed.contains(f)
                || (f.file_name().is_some_and(|n| names.contains(n))
                    && changed.contains(&keyed(st, f)))
        })
        .map(|(i, _)| i)
        .collect()
}

/// A file's path as the index keys it, by its folder's canonical form
/// as the window has kept it, else asked of the disk.
fn keyed(st: &State, f: &Path) -> PathBuf {
    match (
        f.parent().and_then(|d| st.views.library.canonical.get(d)),
        f.file_name(),
    ) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => greycard_library::key_path(f),
    }
}

/// The view is empty for good: nothing under the roots, and no pass
/// left to bring anything. Said, and a capture of it ends here, failed,
/// rather than wait for a picture that will never come.
pub(crate) fn empty_view(st: &mut State, app: &App) {
    let why = if st.views.library.roots.is_empty() {
        "the library has no folders: nothing to show"
    } else if st
        .views
        .library
        .roots
        .list()
        .iter()
        .all(|r| st.views.library.offline.contains(r))
    {
        "every folder in the library is offline: nothing to show"
    } else {
        "nothing under the library's folders"
    };
    app.set_status(why.into());
    tracing::warn!("library: {why}");
    st.views.library.awaiting = false;
    if st.window.batch {
        st.launch.failed = true;
        let _ = slint::quit_event_loop();
    }
}
