//! The folder open, named under the left pane's Open folder button,
//! and Recently opened: the last ten folders, offered from that name.
//!
//! Every list the browser takes passes through `open_loaded`, and the
//! launch's own list is set up before the window runs; both end here,
//! in [`opened`], which works out what the list is a folder of and
//! records it. Nothing else records a folder, so a way to open one
//! added later is in the list without being told.

use crate::files::tilde;
use crate::panel::browser::open_folder;
use crate::settings::{self, push_recent};
use crate::*;

/// What the window knows of the folders: the one open, if the list is
/// a single folder's, and the ones opened, most recent first.
#[derive(Default)]
pub(crate) struct Recent {
    /// The folder open, canonical; none for the roots' view, files
    /// from several folders, or nothing open.
    pub(crate) open: Option<PathBuf>,
    /// As the settings file keeps them, canonical paths.
    pub(crate) folders: Vec<String>,
    /// The list as the window shows it: one model for the session,
    /// its rows replaced only when they change, so a watcher's or the
    /// index's report redrawing the roots leaves an open menu be.
    pub(crate) rows: Rc<VecModel<RecentFolder>>,
}

/// The one folder `files` are all in: a folder opened, or a file
/// opened on its own, which is its folder's. Canonical as the read that
/// brought the list made it (`canonical`, off the window's thread), or
/// as it is when no read has: the disk is not asked here, on the
/// window's thread.
fn folder_of(files: &[PathBuf], canonical: &HashMap<PathBuf, PathBuf>) -> Option<PathBuf> {
    let first = files.first()?.parent()?;
    if first.as_os_str().is_empty() || !files.iter().all(|f| f.parent() == Some(first)) {
        return None;
    }
    Some(
        canonical
            .get(first)
            .cloned()
            .unwrap_or_else(|| first.to_path_buf()),
    )
}

/// The list the browser now holds is open: the folder it is, if it is
/// one, recorded at the front of Recently opened and named in the
/// pane. A run with no settings file to write (a capture, an export,
/// a test) keeps the list in the window only.
pub(crate) fn opened(st: &mut State, app: &App) {
    st.recent.open = match &st.view {
        crate::roots::View::Folder => folder_of(&st.files, &st.library.canonical),
        // A folder of a root's tree is a folder opened, with or without
        // the folders under it; the index's spelling is canonical, and
        // an offline root's folder is recorded without a look at it.
        crate::roots::View::Branch { folder, .. } => Some(folder.clone()),
        crate::roots::View::Roots(_) => None,
    };
    if let Some(dir) = st.recent.open.clone() {
        let dir = dir.to_string_lossy().into_owned();
        match &st.settings_file {
            // Through the file as it is now, so a second window's
            // folders are kept too.
            Some(path) => {
                let mut kept = settings::Settings::load_from(path);
                push_recent(&mut kept.recent_folders, &dir);
                kept.save_to(path);
                st.recent.folders = kept.recent_folders;
            }
            None => push_recent(&mut st.recent.folders, &dir),
        }
    }
    app.set_open_folder_note("".into());
    show(st, app);
}

/// A folder as the pane and the list name it: its own name, and the
/// root it is under (the root's name alone when it is the root).
/// `place` is empty for a folder under no root.
fn named(roots: &greycard_library::Roots, dir: &Path) -> (String, String) {
    match roots.root_of(dir) {
        Some(root) if root == dir => (roots.label(root), String::new()),
        Some(root) => (own_name(dir), roots.label(root)),
        None => (own_name(dir), String::new()),
    }
}

/// The folder's own name, or the whole path for a disk's root.
fn own_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

/// The name under the Open folder button, its hover text, and the
/// list, the one open marked. Called as a list opens and as the roots
/// change, since a root's name is part of what is shown.
pub(crate) fn show(st: &State, app: &App) {
    let roots = &st.library.roots;
    let (name, root, path) = match (&st.view, &st.recent.open) {
        (crate::roots::View::Folder | crate::roots::View::Branch { .. }, Some(dir)) => {
            let (name, root) = named(roots, dir);
            (name, root, dir.display().to_string())
        }
        (crate::roots::View::Roots(Some(r)), _) => {
            (roots.label(r), String::new(), r.display().to_string())
        }
        (crate::roots::View::Roots(None), _) => ("All roots".into(), String::new(), String::new()),
        (crate::roots::View::Folder | crate::roots::View::Branch { .. }, None) => {
            Default::default()
        }
    };
    app.set_open_folder_name(name.into());
    app.set_open_folder_root(root.into());
    app.set_open_folder_path(path.into());
    let open = st
        .recent
        .open
        .as_ref()
        .map(|d| d.to_string_lossy().into_owned());
    let rows: Vec<RecentFolder> = st
        .recent
        .folders
        .iter()
        .map(|f| {
            let dir = Path::new(f);
            let (name, root) = named(roots, dir);
            // Where it is: the root it is under, else the folder it is
            // in, so two shoots both called "raw" are told apart.
            let place = if !root.is_empty() {
                root
            } else {
                dir.parent().map(tilde).unwrap_or_default()
            };
            RecentFolder {
                title: if place.is_empty() {
                    name
                } else {
                    format!("{name}  \u{2014}  {place}")
                }
                .into(),
                path: f.as_str().into(),
                open: open.as_deref() == Some(f.as_str()),
            }
        })
        .collect();
    let model = &st.recent.rows;
    if !model.iter().eq(rows.iter().cloned()) {
        model.set_vec(rows);
    }
}

/// A folder chosen from the list, looked at off the window's thread:
/// a share that has stopped answering holds a look at it for as long
/// as it likes, and a dead share is one click away in this list. It
/// is opened, or said to be gone, back on the window's thread, unless
/// another list has opened since it was chosen.
#[cfg(not(test))]
fn chosen(state: &Rc<RefCell<State>>, app: &App, _worker: &Rc<Worker>, path: &str) {
    let dir = PathBuf::from(path);
    let app_weak = app.as_weak();
    let generation = state.borrow().view_generation;
    let opening = format!("opening {}...", tilde(&dir));
    app.set_status(opening.as_str().into());
    let spawned = std::thread::Builder::new()
        .name("greycard recent check".into())
        .spawn(move || {
            // The roots' look, with its wait: a share gone away does
            // not answer, and is said to after 3 s rather than never.
            let there = crate::roots::answer_of(&dir, crate::roots::ROOT_WAIT);
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                if !still_wanted(&state.borrow(), generation) {
                    tracing::info!(
                        "recently opened: {} came back after another list opened; dropped",
                        dir.display()
                    );
                    if app.get_status() == opening.as_str() {
                        app.set_status("".into());
                    }
                    return;
                }
                checked(&state, &app, &worker, &dir, there);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("recently opened: {e}");
        app.set_status(format!("not opened: {e}").into());
    }
}

/// Whether a folder chosen when the browser's list was `generation`
/// is still the next thing to open: every list opened since (a folder,
/// a view of the roots, this folder itself chosen twice) moves the
/// generation on, and a look that comes back after it is dropped
/// rather than opened over what the user went to meanwhile.
fn still_wanted(st: &State, generation: u64) -> bool {
    st.view_generation == generation
}

/// In a test the folder is looked at in place: there is no event loop
/// to land it on.
#[cfg(test)]
fn chosen(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, path: &str) {
    let dir = Path::new(path);
    checked(
        state,
        app,
        worker,
        dir,
        crate::roots::answer_of(dir, crate::roots::ROOT_WAIT),
    );
}

/// A folder chosen from the list, for another module's test.
#[cfg(test)]
pub(crate) fn choose_for_test(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    dir: &Path,
) {
    chosen(state, app, worker, &dir.to_string_lossy());
}

/// A folder chosen, looked at: opened, or, no longer on the disk (a
/// card taken out, a drive unplugged, a folder moved), said to be
/// gone. It stays listed either way: it may be back when the drive is.
fn checked(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    dir: &Path,
    there: Option<bool>,
) {
    let Some(there) = there else {
        tracing::warn!("recently opened: {} is not answering", dir.display());
        // Not "in 3 s": a look still out from before answers no at once.
        let said = format!("{} isn't reachable; is its drive connected?", tilde(dir));
        app.set_status(said.as_str().into());
        app.set_open_folder_note(said.into());
        return;
    };
    if !there {
        tracing::warn!("recently opened: {} is not there", dir.display());
        let said = format!("{} no longer exists", tilde(dir));
        app.set_status(said.as_str().into());
        app.set_open_folder_note(said.into());
        return;
    }
    open_folder(state, app, worker, dir);
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    app.set_recent_folders(ModelRc::from(state.borrow().recent.rows.clone()));
    let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
    app.on_recent_chosen(move |path| {
        if let Some(app) = app_weak.upgrade() {
            chosen(&state, &app, &worker, &path);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{click, count_labeled, labeled, state_for, window};

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-recent-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    /// A folder of one frame, which the browser lists by its name.
    fn shoot(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("x.tif"), b"").unwrap();
        dir.to_path_buf()
    }

    /// A folder's open, its list landed: it is read off the window's
    /// thread (`roots::open_listing`), and a test lands it.
    fn open_folder(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
        crate::panel::browser::open_folder(state, app, worker, dir);
        crate::roots::land_sent(state, app, worker);
    }

    fn chosen(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &str) {
        app.invoke_recent_chosen(dir.into());
        crate::roots::land_sent(state, app, worker);
    }

    fn listed(app: &App) -> Vec<(String, bool)> {
        app.get_recent_folders()
            .iter()
            .map(|r| (r.path.to_string(), r.open))
            .collect()
    }

    #[test]
    fn a_folder_opened_is_named_and_goes_to_the_front_of_the_list() {
        let dir = scratch("front");
        let (a, b) = (shoot(&dir.join("a")), shoot(&dir.join("b")));
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        // Nothing open: nothing named, and nothing to offer.
        assert_eq!(app.get_open_folder_name(), "");
        assert_eq!(count_labeled(&app, "Recently opened"), 0);

        open_folder(&state, &app, &worker, &a);
        open_folder(&state, &app, &worker, &b);
        assert_eq!(app.get_open_folder_name(), "b");
        assert_eq!(app.get_open_folder_path(), b.to_str().unwrap());
        assert_eq!(app.get_open_folder_root(), "");
        let (a_s, b_s) = (a.to_string_lossy(), b.to_string_lossy());
        assert_eq!(
            listed(&app),
            [(b_s.to_string(), true), (a_s.to_string(), false)]
        );
        // Offered from the name, once under the button: the grid's
        // header is not on screen.
        assert_eq!(count_labeled(&app, "Recently opened"), 1);

        // Chosen from the list: open, and at the front again.
        chosen(&state, &app, &worker, a_s.as_ref());
        assert_eq!(app.get_open_folder_name(), "a");
        assert_eq!(
            listed(&app),
            [(a_s.to_string(), true), (b_s.to_string(), false)]
        );
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A click on the name opens the list, and the press that closes
    /// it is not a second click that opens it again. The name is under
    /// the button, in the left pane, and with the grid up it is still
    /// the pane's, once: the grid's header no longer has one.
    #[test]
    fn a_click_on_the_name_opens_the_list() {
        let dir = scratch("click");
        let a = shoot(&dir.join("a"));
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 700.0));
        let (state, worker) = state_for(&app, Vec::new());
        open_folder(&state, &app, &worker, &a);
        slint::platform::update_timers_and_animations();
        let (at, size) = labeled(&app, "Recently opened");
        let (x, y) = (at.x + 8.0, at.y + size.height / 2.0);
        assert!(!app.get_menu_up());
        click(&app, x, y);
        assert!(app.get_menu_up(), "the list opened");
        click(&app, x, y);
        assert!(!app.get_menu_up(), "the press closed it and opened nothing");
        app.set_grid_open(true);
        assert_eq!(count_labeled(&app, "Recently opened"), 1);
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The hover text goes with the name it is over: the pane put away
    /// under the pointer, by F7 over the grid, takes the name away,
    /// which never hears the pointer leave, and the path must not stay
    /// up over the sheet.
    #[test]
    fn the_hover_text_goes_when_the_pane_takes_its_name_away() {
        use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
        use slint::platform::WindowEvent;
        let dir = scratch("tip");
        let a = shoot(&dir.join("a"));
        let app = window(0);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 700.0));
        let (state, worker) = state_for(&app, Vec::new());
        open_folder(&state, &app, &worker, &a);
        app.set_grid_open(true);
        assert_eq!(count_labeled(&app, "Recently opened"), 1);
        let (at, size) = ElementHandle::find_by_accessible_label(&app, "Recently opened")
            .map(|e| (e.absolute_position(), e.size()))
            .next()
            .unwrap();
        let move_to = |x: f32, y: f32| {
            app.window().dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(x, y),
            });
        };
        move_to(at.x + 8.0, at.y + size.height / 2.0);
        mock_elapsed_time(std::time::Duration::from_millis(800));
        slint::platform::update_timers_and_animations();
        assert_eq!(
            app.get_tip_text(),
            a.to_str().unwrap(),
            "the path after a rest"
        );
        crate::testing::press(&app, slint::platform::Key::F7);
        assert!(app.get_left_hidden());
        move_to(700.0, 400.0);
        mock_elapsed_time(std::time::Duration::from_secs(5));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.get_tip_text(), "", "the hover text stayed up");
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A settings file to write: the folder goes through it as it is on
    /// disk, so a folder another window recorded meanwhile is kept.
    #[test]
    fn a_folder_opened_is_written_through_the_file_as_it_is_now() {
        let dir = scratch("file");
        let (a, b) = (shoot(&dir.join("a")), shoot(&dir.join("b")));
        let file = dir.join("greycard").join("settings.json");
        settings::Settings {
            scope: "Parade".into(),
            ..settings::Settings::default()
        }
        .save_to(&file);
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        state.borrow_mut().settings_file = Some(file.clone());
        open_folder(&state, &app, &worker, &a);
        // Another window opens b meanwhile.
        let mut other = settings::Settings::load_from(&file);
        push_recent(&mut other.recent_folders, "/elsewhere/b");
        other.save_to(&file);
        open_folder(&state, &app, &worker, &b);
        let read = settings::Settings::load_from(&file);
        let (a_s, b_s) = (a.to_string_lossy(), b.to_string_lossy());
        assert_eq!(
            read.recent_folders,
            [b_s.as_ref(), "/elsewhere/b", a_s.as_ref()]
        );
        assert_eq!(read.scope, "Parade", "the rest of the file kept");
        assert_eq!(state.borrow().recent.folders, read.recent_folders);
        assert_eq!(app.get_recent_folders().row_count(), 3);
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_folder_under_a_root_is_named_with_the_root() {
        let dir = scratch("root");
        let root = shoot(&dir.join("archive"));
        let under = shoot(&root.join("2026").join("harbor"));
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.library.roots.add(&root).unwrap();
            st.library.roots.set_name(&root, "Archive");
            // A `--roots` launch whose index then failed: no view will
            // come, and a folder opened by hand is recorded.
            st.library.wanted = Some(crate::roots::View::Roots(None));
        }
        crate::library::told(&app, crate::library::Told::Failed("no index".into()));
        open_folder(&state, &app, &worker, &under);
        assert_eq!(app.get_open_folder_name(), "harbor");
        assert_eq!(app.get_open_folder_root(), "Archive");
        assert_eq!(
            app.get_recent_folders().row_data(0).unwrap().title,
            "harbor  \u{2014}  Archive"
        );
        // The root itself goes by the root's name, and nothing after.
        open_folder(&state, &app, &worker, &root);
        assert_eq!(app.get_open_folder_name(), "Archive");
        assert_eq!(app.get_open_folder_root(), "");
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_folder_gone_from_the_disk_says_so_and_stays_listed() {
        let dir = scratch("gone");
        let (a, b) = (shoot(&dir.join("a")), shoot(&dir.join("b")));
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        open_folder(&state, &app, &worker, &a);
        open_folder(&state, &app, &worker, &b);
        crate::testing::remove_dir_retry(&a);
        let before = listed(&app);

        chosen(&state, &app, &worker, a.to_string_lossy().as_ref());
        assert!(
            app.get_status().contains("no longer exists"),
            "{}",
            app.get_status()
        );
        assert_eq!(app.get_open_folder_note(), app.get_status());
        // The list as it was, the folder open still b's.
        assert_eq!(listed(&app), before);
        assert_eq!(app.get_open_folder_name(), "b");
        assert_eq!(state.borrow().files, [b.join("x.tif")]);
        // The next folder opened takes the note away.
        open_folder(&state, &app, &worker, &b);
        assert_eq!(app.get_open_folder_note(), "");
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A look at a folder that comes back after another list opened is
    /// not wanted any more: it would open over the newer one.
    #[test]
    fn a_late_look_at_a_folder_is_dropped_once_another_opens() {
        let dir = scratch("late");
        let (x, y) = (shoot(&dir.join("x")), shoot(&dir.join("y")));
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        open_folder(&state, &app, &worker, &x);
        // X chosen again, its look out on a share that hangs.
        let chosen_at = state.borrow().view_generation;
        assert!(still_wanted(&state.borrow(), chosen_at));
        // Y opened meanwhile.
        open_folder(&state, &app, &worker, &y);
        assert!(!still_wanted(&state.borrow(), chosen_at));
        // The generation Y opened at is wanted until the next.
        let now = state.borrow().view_generation;
        assert!(still_wanted(&state.borrow(), now));
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn files_from_several_folders_are_no_folder() {
        let dir = scratch("several");
        let (a, b) = (shoot(&dir.join("a")), shoot(&dir.join("b")));
        let none = HashMap::new();
        assert_eq!(folder_of(&[a.join("x.tif")], &none), Some(a.clone()));
        assert_eq!(folder_of(&[a.join("x.tif"), b.join("x.tif")], &none), None);
        assert_eq!(folder_of(&[], &none), None);
        // As a read made it canonical: through a link, the target.
        let link = dir.join("link");
        let known: HashMap<PathBuf, PathBuf> = [(link.clone(), a.clone())].into_iter().collect();
        assert_eq!(folder_of(&[link.join("x.tif")], &known), Some(a.clone()));
        crate::testing::remove_dir_retry(&dir);
    }
}
