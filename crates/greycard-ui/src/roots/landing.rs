//! A read landed on the window's thread.

use super::*;

/// A read done, on the window's thread: what it saw of the roots and
/// the folders kept, and the list opened or merged. Nothing here asks
/// the disk.
pub(super) fn land(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, found: Found) {
    let Found {
        purpose,
        generation,
        token,
        epoch,
        files,
        said,
        fresh,
        held,
        from_disk,
        rows,
        select,
        offline,
        readable,
        unreadable,
        canonical,
        on_screen,
        seconds,
        collection,
        frames,
        companions,
    } = found;
    {
        let st = state.borrow();
        // A read given up on brings nothing: another is out for it.
        let given_up = match purpose {
            Purpose::Merge => token != st.library.merge_token,
            Purpose::Open { .. } => st.view_generation != generation,
        };
        if given_up {
            tracing::debug!("library: a read given up on, or overtaken, came in; dropped");
            return;
        }
    }
    // The frames a collection's read shows, for the cells: each file's
    // versions that are in it, in its order. None for any other list,
    // whose cells are each file's own and all its versions.
    {
        let mut st = state.borrow_mut();
        if st.view_generation == generation && files.is_some() {
            st.collection_frames = frames;
        }
    }
    // The camera's JPEGs beside the list's raws, as the same read found
    // them: a JPEG come or gone beside a raw changes no frame, and the
    // tiles are made again for its mark all the same.
    let paired = {
        let mut st = state.borrow_mut();
        if st.view_generation == generation
            && let Some(listed) = &files
        {
            // A JPEG that is a frame of this list itself (a collection
            // holding it, put there while the JPEGs were listed apart)
            // is no raw's companion here: no mark for it, no picture
            // from it, and a move or a delete of the raw leaves it.
            let listed: HashSet<&PathBuf> = listed.iter().collect();
            let next: HashMap<PathBuf, Vec<PathBuf>> = companions
                .into_iter()
                .map(|(raw, with)| {
                    let with: Vec<PathBuf> =
                        with.into_iter().filter(|c| !listed.contains(c)).collect();
                    (raw, with)
                })
                .filter(|(_, with)| !with.is_empty())
                .collect();
            let changed = st.companions != next;
            st.companions = next;
            changed
        } else {
            false
        }
    };
    // A collection's references found moved or rewritten: its file
    // takes their new hashes and hints, whatever the list does next.
    let mut missing = 0;
    if let Some((id, lost, gone, changes)) = collection {
        missing = lost;
        let mut st = state.borrow_mut();
        // What it found missing is kept for the menu's Forget missing
        // only from a read of the view the window holds now: an older
        // one landing after a forget would bring the old count back.
        if st.view_generation == generation {
            // Those under a network mount are kept by a forget, so not
            // offered.
            let far = &st.library.remote;
            let gone = gone
                .iter()
                .filter(|h| !far.iter().any(|(m, _)| Path::new(h).starts_with(m)))
                .count();
            st.collections.missing = Some((id.clone(), lost, gone));
        }
        // Into the collection the read was of, by its id: a merge read
        // of one that lands after another was opened is still its own.
        if st.collections.apply(&id, &changes) {
            crate::panel::collections::members_changed(&mut st, app);
        }
    }
    // What the read saw is kept only from a read of the list the window
    // holds now; a merge read overtaken by a view opened since still
    // clears `merging` below.
    let mut dimmed = false;
    if state.borrow().view_generation == generation {
        let mut st = state.borrow_mut();
        st.library.canonical.extend(canonical);
        for dir in &readable {
            st.library.unreadable.remove(dir);
        }
        st.library.unreadable.extend(unreadable);
        // What the read saw of a folder is kept unless a pass has gone
        // over the folder since the read was asked for, or the window
        // no longer remembers the passes back that far.
        if epoch >= st.library.passes_from {
            let lib = &mut st.library;
            for dir in readable {
                let passed = lib
                    .passes
                    .iter()
                    .any(|(e, p)| *e > epoch && dir.starts_with(p));
                if !passed {
                    lib.readable.insert(dir);
                }
            }
        }
        // An archive that answered this look runs its queued moves.
        if let Some(offline) = &offline {
            crate::panel::archive::rejects::heard_over(&mut st, app, offline);
            crate::sync::heard_over(&mut st, app, offline);
        }
        if let Some(offline) = offline
            && st.library.offline != offline
        {
            // The tiles' dimming follows, and a root back online has
            // its frames' pictures asked for again.
            let was = std::mem::replace(&mut st.library.offline, offline);
            let now = st.library.offline.clone();
            rows::roots_changed(&mut st, worker, &was, &now);
            // The frame on screen follows to the archive's copy when
            // its own root went (§233).
            crate::sync::follow_offline(&mut st, app);
            dimmed = true;
            show(&st, app);
        }
    }
    match purpose {
        Purpose::Open { view, asked, .. } => {
            // A later list owns `loading`, and clears it itself.
            if state.borrow().view_generation != generation {
                return;
            }
            // No list to be had (a folder with nothing in it, or not a
            // folder; the index unreadable): said, and the list left as
            // it was.
            let Some(files) = files.filter(|f| view != View::Folder || !f.is_empty()) else {
                let mut st = state.borrow_mut();
                loading_done(&mut st.library, app);
                st.library.awaiting = false;
                let why = said.unwrap_or_else(|| "nothing to open".to_string());
                tracing::warn!("{why}");
                app.set_status(why.into());
                return;
            };
            tracing::info!(
                "library: {} frames listed in {:.2} s off the window's thread, {} of them new, \
                 {} sidecar(s) read from disk",
                files.len(),
                seconds,
                fresh.len(),
                from_disk
            );
            // A folder of a root's tree with no frame of its own (the
            // year above the days) is open, empty, and says where its
            // frames are: the tree marks it, and the switch is a press
            // away. With the switch on, that there is nothing under it.
            let bare = match (&view, files.is_empty()) {
                (View::Branch { folder, deep, .. }, true) => Some((folder.clone(), *deep)),
                _ => None,
            };
            if files.is_empty() && bare.is_none() {
                take_empty(state, app, worker, view, asked, missing);
                return;
            }
            {
                let mut st = state.borrow_mut();
                st.view = view;
                loading_done(&mut st.library, app);
            }
            open_loaded(state, app, worker, files, held, rows.is_some(), select);
            let mut st = state.borrow_mut();
            st.library.awaiting = false;
            st.library.painted = Some((asked, "the list asked for"));
            tracing::info!(
                "library: the list in the browser {:.0} ms after it was asked for",
                asked.elapsed().as_secs_f64() * 1e3
            );
            crate::tree::want(&mut st, app);
            show(&st, app);
            if let Some((folder, deep)) = bare {
                app.set_status(crate::tree::nothing_in(&folder, deep).into());
            }
            if let Some(words) = crate::panel::collections::opened_words(&st, missing) {
                app.set_status(words.into());
            }
        }
        Purpose::Merge => {
            let mut again = {
                let mut st = state.borrow_mut();
                st.library.merging = false;
                st.library.merge_since = None;
                std::mem::take(&mut st.library.stale)
            };
            let ours = state.borrow().view_generation == generation;
            let mut rebuilt = false;
            if let (true, Some(files)) = (ours, files) {
                let read: HashMap<PathBuf, Held> = fresh.into_iter().zip(held).collect();
                // A file new to the list since the read was asked for
                // has no sidecar here: read again, rather than merged
                // with a blank one a save would write over its own.
                let whole = {
                    let st = state.borrow();
                    let have: HashSet<&PathBuf> = st.files.iter().collect();
                    files
                        .iter()
                        .all(|f| have.contains(f) || read.contains_key(f))
                };
                if whole {
                    let brought = Brought {
                        read,
                        seen: on_screen,
                        rows,
                    };
                    let (next, changed) =
                        merge(&mut state.borrow_mut(), app, worker, files, brought);
                    // A collection's frames may have moved where its
                    // files did not (a version added to it, or taken
                    // out): the cells follow, the frame on screen with
                    // them.
                    let settled = crate::versions::settle(&mut state.borrow_mut(), app, worker);
                    rebuilt = changed || settled;
                    if let Some(row) = next.filter(|_| !settled) {
                        app.invoke_select(row as i32);
                    }
                } else {
                    again = true;
                }
            }
            if (dimmed || paired) && ours && !rebuilt {
                rebuild_browser(&mut state.borrow_mut(), app);
            }
            if paired && ours {
                let st = state.borrow();
                let on_screen = st.current.and_then(|c| st.path_at(c)).cloned();
                crate::panel::browser::show_file_name(&st, app, on_screen.as_deref());
            }
            if again {
                refresh_view(state, app, worker);
                return;
            }
            // The launch pass done and nothing came of it: said, and a
            // capture waiting on the view ends.
            let empty = {
                let st = state.borrow();
                matches!(st.view, View::Roots(_))
                    && st.files.is_empty()
                    && st.library.launch_left == 0
                    && !st.library.starting
                    && !st.library.loading
            };
            if empty {
                empty_view(&mut state.borrow_mut(), app);
            }
        }
    }
}

/// An empty view taken: nothing indexed yet, most likely, or nothing
/// there that can be read. It fills in as the launch pass reaches the
/// roots. A capture of it waits for that pass when there is one to
/// wait for, and ends when there is none.
fn take_empty(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    view: View,
    asked: Instant,
    missing: usize,
) {
    let collection = view.collection().is_some();
    let passing = {
        let mut st = state.borrow_mut();
        st.view = view;
        st.library.launch_left > 0 || st.library.starting
    };
    open_loaded(state, app, worker, Vec::new(), Vec::new(), true, 0);
    let mut st = state.borrow_mut();
    loading_done(&mut st.library, app);
    st.library.painted = Some((asked, "the view asked for"));
    crate::tree::want(&mut st, app);
    show(&st, app);
    if collection {
        // Nothing in it, or nothing of it to be found: said, and a
        // capture waiting on it ends as an empty folder's would.
        let words = crate::panel::collections::opened_words(&st, missing).unwrap_or_default();
        app.set_status(words.into());
        st.library.awaiting = false;
    } else if passing {
        app.set_status("Nothing indexed under the roots yet; the pass is running".into());
    } else {
        empty_view(&mut st, app);
    }
}
