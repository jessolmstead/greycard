//! The roots pane: the rows and their counts, and a root added, taken
//! out or named.

use super::*;

/// Each root's count, as the index holds it now.
pub(crate) fn recount(st: &mut State) {
    let Some(lib) = &st.views.index_reader else {
        return;
    };
    let counts: Vec<usize> = st
        .views
        .library
        .roots
        .list()
        .iter()
        .map(|r| lib.count_under_canonical(r).unwrap_or(0))
        .collect();
    st.views.library.counts = counts;
}

/// The roots in the grid's left pane, and what depends on them: the
/// open folder's name and the tree.
pub(crate) fn show(st: &State, app: &App) {
    let roots = st.views.library.roots.list();
    // A folder of a root's tree keeps the root's row lit.
    let on = match &st.views.view {
        View::Roots(r) => Some(r.clone()),
        View::Branch { root, .. } => Some(Some(root.clone())),
        View::Folder | View::Collection { .. } | View::Queue => None,
    };
    let chips: Vec<RootChip> = roots
        .iter()
        .enumerate()
        .map(|(i, r)| RootChip {
            name: root_name(&st.views.library.roots, r).into(),
            path: r.to_string_lossy().into_owned().into(),
            count: st.views.library.counts.get(i).copied().unwrap_or(0) as i32,
            on: on.as_ref() == Some(&Some(r.clone())),
            offline: st.views.library.offline.contains(r),
            archive: st.views.library.roots.is_archive(r),
        })
        .collect();
    let model = &st.views.library.rows;
    if !model.iter().eq(chips.iter().cloned()) {
        model.set_vec(chips);
    }
    app.set_library_all_on(on == Some(None));
    app.set_library_all_count(st.views.library.counts.iter().sum::<usize>() as i32);
    app.set_library_can_add_open(open_folder_to_add(st).is_some());
    app.set_library_note(
        if roots.is_empty() {
            "No folders in the library yet"
        } else {
            ""
        }
        .into(),
    );
    // The open folder's name, which says the root it is under.
    crate::panel::recent::show(st, app);
    // The open root's folders, the one open marked.
    crate::tree::show(st, app);
    // The header's Back up or Bring back, by where the view is now.
    crate::panel::archive::show(st, app);
    // The collection open, lit.
    crate::panel::collections::show(st, app);
}

/// A root as its row names it: the name the user gave it, else the
/// folder's own name, or the whole path for a disk's root, which has
/// none.
pub(crate) fn root_name(roots: &Roots, root: &Path) -> String {
    roots.label(root)
}

/// The one root every path is under, named, or `None` when they are
/// under several or none.
pub(crate) fn one_root_named<'a>(
    roots: &Roots,
    paths: impl IntoIterator<Item = &'a Path>,
) -> Option<String> {
    let mut one: Option<&Path> = None;
    for path in paths {
        let root = roots.root_of(path)?;
        match one {
            Some(r) if r != root => return None,
            _ => one = Some(root),
        }
    }
    one.map(|r| root_name(roots, r))
}

/// A root as the status line says it: its path, after its name when
/// it has one, so a root named alike to another is still told apart.
pub(crate) fn said(roots: &Roots, root: &Path) -> String {
    match roots.name(root) {
        Some(name) => format!("{name} ({})", root.display()),
        None => root.display().to_string(),
    }
}

/// The folder's own name, as the root sheet offers it for an empty
/// name.
fn folder_name(dir: &Path) -> String {
    root_name(&Roots::default(), dir)
}

/// The open folder, when the browser shows one and it is not under a
/// root yet: what "Add this folder" adds. By its canonical form as the
/// reads have kept it; a folder no read has seen yet goes as it is,
/// and the add makes it canonical off the window's thread, where one
/// under a root after all is said to be.
pub(super) fn open_folder_to_add(st: &State) -> Option<PathBuf> {
    if st.views.view != View::Folder {
        return None;
    }
    let dir = st.files.first()?.parent()?;
    let dir = st
        .views
        .library
        .canonical
        .get(dir)
        .cloned()
        .unwrap_or_else(|| dir.to_path_buf());
    st.views
        .library
        .roots
        .root_of(&dir)
        .is_none()
        .then_some(dir)
}

/// Change the roots: through the file as it is on disk now when there
/// is one, so a root another editor added or removed meanwhile is
/// kept, else in memory alone.
pub(crate) fn edit_roots<T>(
    st: &mut State,
    change: impl FnOnce(&mut Roots) -> T,
) -> std::result::Result<T, String> {
    match &st.views.library.file {
        Some(path) => match Roots::edit(path, change) {
            Ok((roots, out)) => {
                st.views.library.roots = roots;
                Ok(out)
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        },
        None => Ok(change(&mut st.views.library.roots)),
    }
}

/// Add a folder to the roots: kept, watched, passed over, and the
/// view brought up if it lists the roots. It goes by its folder's
/// name until it is given one from its row's menu.
///
/// The folder is made canonical and looked at off the window's thread
/// (a folder on a share can take a round trip, or never answer), and
/// added when that is done.
pub(crate) fn add(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    app.set_status(format!("adding {}...", dir.display()).into());
    send_check(state, app, worker, dir.to_path_buf(), Check::Add);
}

/// [`add`], and the folder's frames opened once it is in: its own
/// view as a root, or, under a root already, that root's view of it
/// and the folders under it. The Import sheet's Add in place.
pub(crate) fn add_and_open(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    app.set_status(format!("adding {}...", dir.display()).into());
    send_check(state, app, worker, dir.to_path_buf(), Check::AddOpen);
}

/// What a folder is looked at off the window's thread for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Check {
    /// To be added: made canonical, and a folder.
    Add,
    /// The same, and its frames opened once it is in.
    AddOpen,
    /// To be taken out, named by a path the list does not have as it
    /// is: made canonical, to be found in the list by that.
    Remove,
}

impl Check {
    /// The look itself, which asks the disk.
    fn run(self, dir: &Path) -> std::result::Result<PathBuf, greycard_library::roots::RootError> {
        match self {
            Check::Add | Check::AddOpen => Roots::checked(dir),
            // A path that cannot be made canonical (gone, say) is looked
            // for in the list as it is.
            Check::Remove => Ok(dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())),
        }
    }

    /// The answer, on the window's thread.
    pub(super) fn land(
        self,
        state: &Rc<RefCell<State>>,
        app: &App,
        worker: &Rc<Worker>,
        dir: &Path,
        checked: std::result::Result<PathBuf, greycard_library::roots::RootError>,
    ) {
        match self {
            Check::Add => add_checked(state, app, worker, checked, false),
            Check::AddOpen => add_checked(state, app, worker, checked, true),
            Check::Remove => match checked {
                Ok(canonical)
                    if state
                        .borrow()
                        .views
                        .library
                        .roots
                        .list()
                        .contains(&canonical) =>
                {
                    remove_listed(state, app, worker, &canonical)
                }
                Ok(_) => app.set_status(format!("{} is not in the library", dir.display()).into()),
                Err(e) => {
                    tracing::warn!("library: {e}");
                    app.set_status(format!("not removed: {e}").into());
                }
            },
        }
    }
}

/// `check` run on a thread of its own, waited for up to the roots'
/// wait: a folder on a share that does not answer is an error saying
/// so, and the thread asking it is left to itself.
pub(super) fn checked_within(
    check: Check,
    dir: &Path,
) -> std::result::Result<PathBuf, greycard_library::roots::RootError> {
    use greycard_library::roots::RootError;
    let late = || {
        RootError::Io(
            dir.to_path_buf(),
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out after {} s", ROOT_WAIT.as_secs()),
            ),
        )
    };
    // In a test, in place: a test's wait is the test's, not a clock's
    // under a loaded machine.
    #[cfg(test)]
    return if tests::not_answering(dir) {
        Err(late())
    } else {
        check.run(dir)
    };
    #[cfg_attr(test, allow(unreachable_code))]
    let (tx, rx) = std::sync::mpsc::channel();
    let asked = dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("greycard roots look".into())
        .spawn(move || {
            let _ = tx.send(check.run(&asked));
        });
    if let Err(e) = spawned {
        return Err(RootError::Io(dir.to_path_buf(), e));
    }
    rx.recv_timeout(ROOT_WAIT).unwrap_or_else(|_| Err(late()))
}

/// Look at a folder off the window's thread, and act on it on the
/// window's thread when that is done.
#[cfg(not(test))]
fn send_check(
    _state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    dir: PathBuf,
    check: Check,
) {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard roots add".into())
        .spawn(move || {
            let checked = checked_within(check, &dir);
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                check.land(&state, &app, &worker, &dir, checked);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("library: {e}");
        app.set_status(format!("not done: {e}").into());
    }
}

/// In a test the folder is looked at in place, there being no event
/// loop to land it on; or, when the test holds them, queued for it to
/// run (`tests::run_checks`), so it can see the window's thread asked
/// nothing meanwhile.
#[cfg(test)]
fn send_check(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    dir: PathBuf,
    check: Check,
) {
    if tests::HOLD_CHECKS.with(|h| *h.borrow()) {
        tests::CHECKS_SENT.with(|c| c.borrow_mut().push((dir, check)));
        return;
    }
    let checked = checked_within(check, &dir);
    check.land(state, app, worker, &dir, checked);
}

/// A folder to add, looked at: kept, watched, passed over; and with
/// `then_open`, its frames opened.
fn add_checked(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    checked: std::result::Result<PathBuf, greycard_library::roots::RootError>,
    then_open: bool,
) {
    let dir = match checked {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!("library: {e}");
            app.set_status(format!("not added: {e}").into());
            return;
        }
    };
    let mut st = state.borrow_mut();
    let asked = dir.clone();
    let added = match edit_roots(&mut st, |r| r.add_checked(dir)) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("library: roots not saved: {e}");
            app.set_status(format!("not added: {e}").into());
            return;
        }
    };
    let new = match added {
        Ok(Added::New(root)) => {
            app.set_status(
                format!(
                    "{} is in the library now; right-click its row to name it",
                    said(&st.views.library.roots, &root)
                )
                .into(),
            );
            root
        }
        Ok(Added::Absorbed(root, under)) => {
            app.set_status(
                format!(
                    "{} is in the library now, in place of {} folder{} under it",
                    said(&st.views.library.roots, &root),
                    under.len(),
                    if under.len() == 1 { "" } else { "s" }
                )
                .into(),
            );
            root
        }
        Ok(Added::Covered(root)) => {
            app.set_status(
                format!(
                    "already in the library, under {}",
                    said(&st.views.library.roots, &root)
                )
                .into(),
            );
            if then_open {
                drop(st);
                // The root itself chosen is its own view.
                let view = if asked == root {
                    View::Roots(Some(root))
                } else {
                    View::Branch {
                        root,
                        folder: asked,
                        deep: true,
                    }
                };
                open_view(state, app, worker, view);
            }
            return;
        }
        Err(e) => {
            tracing::warn!("library: {e}");
            app.set_status(format!("not added: {e}").into());
            return;
        }
    };
    tracing::info!("library: added {}", new.display());
    if let Some(indexer) = &st.views.index {
        indexer.roots(vec![new.clone()]);
    }
    watch(&mut st, app);
    recount(&mut st);
    show(&st, app);
    let refresh = matches!(st.views.view, View::Roots(None));
    drop(st);
    if then_open {
        open_view(state, app, worker, View::Roots(Some(new)));
    } else if refresh {
        refresh_view(state, app, worker);
    }
}

/// Roots another writer added to the roots file (the Lightroom
/// catalog import, which writes it through the library's own
/// `Roots::edit`): the list read again from the file, or, with no file
/// this session, the folders added in memory; then each kept, watched
/// and passed over, as an add from the pane is.
pub(crate) fn taken_from_file(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: Option<&Rc<Worker>>,
    added: Vec<PathBuf>,
) {
    let mut st = state.borrow_mut();
    match st.views.library.file.clone() {
        Some(file) => match Roots::load(&file) {
            Ok(roots) => st.views.library.roots = roots,
            Err(e) => tracing::warn!("library: roots not read again: {e}"),
        },
        None => {
            for dir in &added {
                if let Err(e) = st.views.library.roots.add(dir) {
                    tracing::warn!("library: {e}");
                }
            }
        }
    }
    tracing::info!("library: {} roots added by the import", added.len());
    if let Some(indexer) = &st.views.index {
        indexer.roots(added);
    }
    watch(&mut st, app);
    recount(&mut st);
    show(&st, app);
    let refresh = matches!(st.views.view, View::Roots(None));
    drop(st);
    if refresh && let Some(worker) = worker {
        refresh_view(state, app, worker);
    }
}

/// Take a folder out of the roots. The folder and its files are not
/// touched, nor are their rows in the index; the all-roots view no
/// longer lists them. A root named as the list has it (the chips' cross
/// always does) is taken out at once, asking the disk nothing; any
/// other path is made canonical off the window's thread first, and
/// taken out by that when it is in the list.
pub(crate) fn remove(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    if state
        .borrow()
        .views
        .library
        .roots
        .list()
        .iter()
        .any(|r| r == dir)
    {
        remove_listed(state, app, worker, dir);
    } else {
        send_check(state, app, worker, dir.to_path_buf(), Check::Remove);
    }
}

/// Take out a root the list has as `dir`, as it is.
fn remove_listed(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let mut st = state.borrow_mut();
    let was = said(&st.views.library.roots, dir);
    match edit_roots(&mut st, |r| r.remove(dir)) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!("library: roots not saved: {e}");
            app.set_status(format!("not removed: {e}").into());
            return;
        }
    }
    tracing::info!(
        "library: removed {}; nothing on disk touched",
        dir.display()
    );
    app.set_status(format!("{was} is out of the library; its files are where they were").into());
    // Nothing is owed to or by a root that is out (§233).
    if let Some(indexer) = &st.views.index {
        indexer.forget_pending_under(dir.to_path_buf());
    }
    // The watcher there is kept until the one on the rest is ready, so
    // the other roots are not left unwatched while it is built (on a
    // large tree, many seconds). The timer is started again at once
    // without the root, which asks the disk nothing.
    // A network mount below the root goes with it.
    st.views.library.remote.retain(|(r, _)| !r.starts_with(dir));
    restart_poll(&mut st);
    watch(&mut st, app);
    recount(&mut st);
    let view = st.views.view.clone();
    let refresh = match &view {
        View::Roots(Some(r)) | View::Branch { root: r, .. } if r == dir => {
            st.views.view = View::Roots(None);
            true
        }
        View::Roots(_) | View::Branch { .. } | View::Collection { .. } | View::Queue => true,
        View::Folder => false,
    };
    crate::tree::want(&mut st, app);
    show(&st, app);
    // A row gone with the root, outside any pass's report: the marks
    // asked again, so no reused row wears a member's.
    crate::panel::collections::members_changed(&mut st, app);
    drop(st);
    if refresh {
        refresh_view(state, app, worker);
    }
}

/// Give a root a name, or with an empty one take its name away. The
/// root is still its path, and nothing on disk is touched; only its
/// row and what the status line calls it change.
pub(crate) fn rename(state: &Rc<RefCell<State>>, app: &App, dir: &Path, name: &str) {
    let mut st = state.borrow_mut();
    let before = st.views.library.roots.name(dir).map(str::to_owned);
    match edit_roots(&mut st, |r| r.set_name(dir, name)) {
        Ok(true) => {}
        Ok(false) => {
            app.set_status(format!("{} is not in the library", dir.display()).into());
            return;
        }
        Err(e) => {
            tracing::warn!("library: roots not saved: {e}");
            app.set_status(format!("not renamed: {e}").into());
            return;
        }
    }
    let after = st.views.library.roots.name(dir).map(str::to_owned);
    if after != before {
        tracing::info!("library: {} named {:?}", dir.display(), after);
    }
    app.set_status(
        match &after {
            Some(name) => format!(
                "{} is called {name} in the library; the folder is as it was",
                dir.display()
            ),
            None => format!("{} goes by its folder's name", dir.display()),
        }
        .into(),
    );
    show(&st, app);
}

/// The root sheet opened over a root, with the name it has.
pub(super) fn open_sheet(st: &State, app: &App, dir: &Path) {
    app.set_root_sheet_path(dir.to_string_lossy().into_owned().into());
    app.set_root_sheet_folder(folder_name(dir).into());
    app.set_root_sheet_name(st.views.library.roots.name(dir).unwrap_or("").into());
    app.set_root_sheet_open(true);
}
