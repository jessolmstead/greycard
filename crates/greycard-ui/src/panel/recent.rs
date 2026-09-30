//! The folder open, named under the Open folder button and in the
//! grid's header, and Recently opened: the last ten folders, offered
//! from that name.
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
/// opened on its own, which is its folder's.
fn folder_of(files: &[PathBuf]) -> Option<PathBuf> {
    let first = files.first()?.parent()?;
    if first.as_os_str().is_empty() || !files.iter().all(|f| f.parent() == Some(first)) {
        return None;
    }
    dunce::canonicalize(first).ok()
}

/// The list the browser now holds is open: the folder it is, if it is
/// one, recorded at the front of Recently opened and named in the
/// pane. A run with no settings file to write (a capture, an export,
/// a test) keeps the list in the window only.
pub(crate) fn opened(st: &mut State, app: &App) {
    st.recent.open = match st.view {
        crate::roots::View::Folder => folder_of(&st.files),
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
        (crate::roots::View::Folder, Some(dir)) => {
            let (name, root) = named(roots, dir);
            (name, root, dir.display().to_string())
        }
        (crate::roots::View::Roots(Some(r)), _) => {
            (roots.label(r), String::new(), r.display().to_string())
        }
        (crate::roots::View::Roots(None), _) => ("All roots".into(), String::new(), String::new()),
        (crate::roots::View::Folder, None) => Default::default(),
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
            let there = dir.is_dir();
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
    checked(state, app, worker, dir, dir.is_dir());
}

/// A folder chosen, looked at: opened, or, no longer on the disk (a
/// card taken out, a drive unplugged, a folder moved), said to be
/// gone. It stays listed either way: it may be back when the drive is.
fn checked(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path, there: bool) {
    if !there {
        tracing::warn!("recently opened: {} is not there", dir.display());
        let said = format!("{} is not there any more", tilde(dir));
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A click on the name opens the list, and the press that closes
    /// it is not a second click that opens it again. The name is under
    /// the button and, with the grid up, in its header as well.
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
        assert_eq!(count_labeled(&app, "Recently opened"), 2);
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The hover text goes with the name it is over: the grid closed
    /// under the pointer takes its header's name away, which never
    /// hears the pointer leave, and the path must not stay up over
    /// the viewport.
    #[test]
    fn the_hover_text_goes_when_the_grid_takes_its_name_away() {
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
        assert_eq!(count_labeled(&app, "Recently opened"), 2);
        // The header's, in the button's row, above the left pane's
        // under its button.
        let (at, size) = ElementHandle::find_by_accessible_label(&app, "Recently opened")
            .map(|e| (e.absolute_position(), e.size()))
            .min_by(|a, b| a.0.y.total_cmp(&b.0.y))
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
        app.set_grid_open(false);
        move_to(700.0, 400.0);
        mock_elapsed_time(std::time::Duration::from_secs(5));
        slint::platform::update_timers_and_animations();
        assert_eq!(app.get_tip_text(), "", "the hover text stayed up");
        drop(state);
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_gone_from_the_disk_says_so_and_stays_listed() {
        let dir = scratch("gone");
        let (a, b) = (shoot(&dir.join("a")), shoot(&dir.join("b")));
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        open_folder(&state, &app, &worker, &a);
        open_folder(&state, &app, &worker, &b);
        std::fs::remove_dir_all(&a).unwrap();
        let before = listed(&app);

        chosen(&state, &app, &worker, a.to_string_lossy().as_ref());
        assert!(
            app.get_status().contains("is not there any more"),
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
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_from_several_folders_are_no_folder() {
        let dir = scratch("several");
        let (a, b) = (shoot(&dir.join("a")), shoot(&dir.join("b")));
        assert_eq!(folder_of(&[a.join("x.tif")]), Some(a.clone()));
        assert_eq!(folder_of(&[a.join("x.tif"), b.join("x.tif")]), None);
        assert_eq!(folder_of(&[]), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
