use super::*;
use crate::roots::{View, land_sent, open_view};
use crate::testing::{folder, state_for, strip_cell, window};
use crate::worker::Outcome;
use greycard_edit::Edit;
use greycard_library::fixture::{A7, R5, R6, write_frame};
use slint::Model;
use slint::platform::{PointerEventButton, WindowEvent};

/// An app named `name` made in `dir`, and added.
fn choose_app(state: &Rc<Shared>, app: &App, dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, b"").unwrap();
    CHOSEN_IN_TEST.with(|c| *c.borrow_mut() = Some(path.clone()));
    app.invoke_edit_in_add();
    assert_eq!(apps(state).last(), Some(&path));
    path
}

fn apps(state: &Rc<Shared>) -> Vec<PathBuf> {
    let st = state.borrow();
    st.work
        .edit_in
        .apps
        .iter()
        .map(|(p, _)| p.clone())
        .collect()
}

fn names(list: slint::ModelRc<slint::SharedString>) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn watches(state: &Rc<Shared>) -> Vec<(PathBuf, PathBuf)> {
    let st = state.borrow();
    let list = st.work.edit_in.watches.lock().unwrap();
    list.iter()
        .map(|w| (w.out.clone(), w.source.clone()))
        .collect()
}

/// The frame menu's entries titled the catalog's `msgid`, with the
/// menu up over the strip's first frame.
fn menu_items(app: &App, msgid: &str) -> usize {
    let position = slint::LogicalPosition::new(strip_cell(app, 0).0, strip_cell(app, 0).1);
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Right,
        },
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Right,
        },
    ] {
        app.window().dispatch_event(event);
    }
    assert!(app.get_menu_up());
    let n = crate::testing::count_menu_entries(app, msgid);
    crate::testing::press(app, slint::platform::Key::Escape);
    n
}

/// Edit in is not offered until an app is added on the Settings
/// sheet, whose list then names it; a cancel leaves it as it was.
/// One app on the disk is Edit in... of its own; two are a submenu
/// of their names, each handing the frame to its own. An app gone
/// since is offered no more, and nothing is exported for it, but
/// stays on the list; none left on the disk offers no Edit in.
#[test]
fn the_menu_offers_edit_in_once_an_app_is_added() {
    let dir = crate::testing::scratch_dir("edit-in-menu");
    let app = window(2);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(2));
    app.invoke_select(0);
    app.set_busy(false);
    assert!(names(app.get_edit_in_apps()).is_empty());
    assert_eq!(menu_items(&app, "Edit in..."), 0);
    assert_eq!(menu_items(&app, "Edit in"), 0);
    // Pressed with no app: nothing exported.
    app.invoke_menu_edit_in(0);
    assert!(state.borrow().work.exporting.is_none());

    // One app: Edit in... as an item of its own.
    let photoshop = choose_app(&state, &app, &dir, "Photoshop.app");
    assert_eq!(names(app.get_edit_in_list()), ["Photoshop"]);
    assert_eq!(names(app.get_edit_in_apps()), ["Photoshop"]);
    assert_eq!(menu_items(&app, "Edit in..."), 1);
    assert_eq!(menu_items(&app, "Edit in"), 0);
    // A cancel keeps the list.
    CHOSEN_IN_TEST.with(|c| *c.borrow_mut() = None);
    app.invoke_edit_in_add();
    assert_eq!(names(app.get_edit_in_list()), ["Photoshop"]);

    // Two apps: a submenu of their names, in the order added.
    let gimp = choose_app(&state, &app, &dir, "gimp");
    assert_eq!(names(app.get_edit_in_apps()), ["Photoshop", "gimp"]);
    assert_eq!(menu_items(&app, "Edit in..."), 0);
    assert_eq!(menu_items(&app, "Edit in"), 1);
    // A menu entry pressed: the app its export is handed to, the
    // export then ended.
    let handed = |which: i32| {
        app.invoke_menu_edit_in(which);
        let set = state.borrow().work.exporting.clone().expect("the set");
        let to = state
            .borrow()
            .work
            .edit_in
            .frame
            .as_ref()
            .unwrap()
            .2
            .clone();
        let tally = queue::Tally::default();
        crate::panel::deliver::deliver(&app, Outcome::SetDone { set, tally });
        assert!(state.borrow().work.exporting.is_none());
        to
    };
    assert_eq!(handed(1), gimp);
    assert_eq!(handed(0), photoshop);

    // The first gone from the disk: the menu's places are those on
    // the disk, so its first entry is the second app on the list
    // and its second the third.
    let krita = choose_app(&state, &app, &dir, "krita");
    std::fs::remove_file(&photoshop).unwrap();
    assert_eq!(menu_items(&app, "Edit in"), 1);
    assert_eq!(names(app.get_edit_in_apps()), ["gimp", "krita"]);
    assert_eq!(handed(0), gimp);
    assert_eq!(handed(1), krita);

    // Uninstalled since the menu was built: refused, offered no
    // more, still listed.
    std::fs::remove_file(&gimp).unwrap();
    app.invoke_menu_edit_in(0);
    assert!(state.borrow().work.exporting.is_none());
    assert_eq!(
        names(app.get_edit_in_list()),
        ["Photoshop", "gimp", "krita"]
    );
    assert_eq!(names(app.get_edit_in_apps()), ["krita"]);
    assert_eq!(menu_items(&app, "Edit in..."), 1);
    assert_eq!(menu_items(&app, "Edit in"), 0);
    // Gone while the menu is down: the menu, built again, has none.
    std::fs::remove_file(&krita).unwrap();
    assert_eq!(menu_items(&app, "Edit in..."), 0);
    assert_eq!(menu_items(&app, "Edit in"), 0);
    assert!(names(app.get_edit_in_apps()).is_empty());
    assert_eq!(names(app.get_edit_in_list()).len(), 3);
    crate::testing::remove_dir_retry(&dir);
}

/// The Settings sheet's list: Add... puts an app at its end, once;
/// × takes the one at its place off; each change is written to the
/// settings file as a list.
#[test]
fn the_settings_list_adds_and_takes_off_apps() {
    let dir = crate::testing::scratch_dir("edit-in-list");
    let file = dir.join("settings.json");
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    state.borrow_mut().window.settings_file = Some(file.clone());
    let written = || crate::settings::Settings::load_from(&file).edit_in;
    let photoshop = choose_app(&state, &app, &dir, "Photoshop.app");
    let gimp = choose_app(&state, &app, &dir, "gimp");
    let krita = choose_app(&state, &app, &dir, "krita.desktop");
    let path = |p: &PathBuf| p.to_string_lossy().into_owned();
    assert_eq!(
        names(app.get_edit_in_list()),
        ["Photoshop", "gimp", "krita"]
    );
    assert_eq!(written(), [path(&photoshop), path(&gimp), path(&krita)]);
    // One on the list already stays where it is, by another path
    // to the same file too.
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for again in [photoshop.clone(), dir.join("sub").join("..").join("gimp")] {
        CHOSEN_IN_TEST.with(|c| *c.borrow_mut() = Some(again));
        app.invoke_edit_in_add();
        assert_eq!(
            names(app.get_edit_in_list()),
            ["Photoshop", "gimp", "krita"]
        );
    }

    app.invoke_edit_in_remove(1);
    assert_eq!(names(app.get_edit_in_list()), ["Photoshop", "krita"]);
    assert_eq!(names(app.get_edit_in_apps()), ["Photoshop", "krita"]);
    assert_eq!(written(), [path(&photoshop), path(&krita)]);
    // A place past the end takes nothing off.
    app.invoke_edit_in_remove(5);
    assert_eq!(apps(&state), [photoshop.clone(), krita]);
    app.invoke_edit_in_remove(1);
    app.invoke_edit_in_remove(0);
    assert!(names(app.get_edit_in_list()).is_empty());
    assert!(names(app.get_edit_in_apps()).is_empty());
    assert!(written().is_empty());
    crate::testing::remove_dir_retry(&dir);
}

/// Edit in exports the frame on screen as a set of one: TIFF 16-bit
/// in the sheet's color space, beside the raw under "-edit", never
/// over a file there; the written file is handed to the app and
/// watched, and the status line ends on the export's own line. A
/// second Edit in under the same name is watched once.
#[test]
fn edit_in_exports_beside_the_raw_and_hands_the_file_over() {
    let dir = crate::testing::scratch_dir("edit-in-export");
    let app = window(2);
    let (state, _worker) = state_for(&app, folder(2));
    state.borrow_mut().current = Some(1);
    app.set_export_space("Display P3".into());
    app.set_export_format("JPEG".into());
    app.set_export_metadata(crate::export::Metadata::None.name().into());
    let gimp = choose_app(&state, &app, &dir, "gimp");
    app.invoke_menu_edit_in(0);
    let set = state.borrow().work.exporting.clone().expect("the set");
    assert_eq!(set.total, 1);
    assert_eq!(set.settings.format, Format::Tiff);
    assert_eq!(set.settings.space, crate::export::Space::DisplayP3);
    assert_eq!(set.settings.metadata, crate::export::Metadata::None);
    assert_eq!(set.settings.long_edge, None);
    assert_eq!(set.on_exists, OnExists::Increment);
    assert_eq!((set.folder.as_ref(), set.sub.as_ref()), (None, None));
    assert_eq!(set.preset, None);
    let source = state.borrow().files[1].clone();
    assert_eq!(
        out_of(&source, None),
        Path::new("/nowhere/IMG_0001-edit.greycard.tif")
    );
    assert_eq!(
        out_of(&source, Some("Warm")),
        Path::new("/nowhere/IMG_0001-edit-Warm.greycard.tif")
    );
    assert_eq!(
        state.borrow().work.edit_in.frame,
        Some((source.clone(), None, gimp.clone()))
    );
    // A second press while it runs exports nothing more.
    app.invoke_menu_edit_in(0);
    assert!(Arc::ptr_eq(
        state.borrow().work.exporting.as_ref().unwrap(),
        &set
    ));

    let path = PathBuf::from("/nowhere/IMG_0001-edit.greycard.tif");
    LAUNCHED.with(|l| l.borrow_mut().clear());
    let written = |set: &Arc<queue::Set>, path: &Path| {
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameDone {
                set: set.clone(),
                index: 0,
                source: source.clone(),
                edit: Edit::default(),
                version: None,
                done: queue::Done::Exported {
                    path: path.to_path_buf(),
                    seconds: 1.5,
                    note: None,
                    left_out: Vec::new(),
                },
            },
        );
    };
    written(&set, &path);
    let launched = LAUNCHED.with(|l| l.borrow().clone());
    assert_eq!(launched, [how::launch(&gimp, &path, Platform::this())]);
    assert_eq!(watches(&state), [(path.clone(), source.clone())]);
    assert!(state.borrow().work.edit_in.set.is_none());
    let tally = queue::Tally {
        exported: 1,
        ..queue::Tally::default()
    };
    crate::panel::deliver::deliver(&app, Outcome::SetDone { set, tally });
    assert_eq!(
        app.get_status(),
        "exported IMG_0001-edit.greycard.tif in 1.50 s"
    );

    // The file deleted and Edit in again: the same name, one watch.
    app.invoke_menu_edit_in(0);
    let set = state
        .borrow()
        .work
        .exporting
        .clone()
        .expect("the second set");
    written(&set, &path);
    assert_eq!(LAUNCHED.with(|l| l.borrow().len()), 2);
    assert_eq!(watches(&state), [(path.clone(), source.clone())]);
    let tally = queue::Tally {
        exported: 1,
        ..queue::Tally::default()
    };
    crate::panel::deliver::deliver(&app, Outcome::SetDone { set, tally });

    // Another set's frame is never handed over.
    let set = Arc::new(queue::Set::new(
        1,
        None,
        settings_for(&app),
        OnExists::Increment,
    ));
    written(&set, Path::new("/nowhere/IMG_0001.jpg"));
    assert_eq!(LAUNCHED.with(|l| l.borrow().len()), 2);
    assert_eq!(watches(&state).len(), 1);
    crate::testing::remove_dir_retry(&dir);
}

/// Each of the sheet's Metadata choices is the one Edit in exports
/// with, the format staying TIFF whatever the sheet's.
#[test]
fn edit_in_takes_the_sheets_metadata() {
    let app = window(1);
    app.set_export_format("JPEG".into());
    for metadata in crate::export::Metadata::ALL {
        app.set_export_metadata(metadata.name().into());
        let settings = settings_for(&app);
        assert_eq!(settings.metadata, metadata);
        assert_eq!(settings.format, Format::Tiff);
    }
}

struct Setup {
    dir: PathBuf,
    files: Vec<PathBuf>,
    writer: greycard_library::Library,
    app: App,
    state: Rc<Shared>,
    worker: Rc<Worker>,
}

/// A root of a shoot of three frames, indexed, with the grid up
/// over it by name, its stacks kept in the test's own folder.
fn setup(what: &str) -> Setup {
    setup_on(what, |_| Rc::new(Worker::new(|_| {})))
}

/// [`setup`] on the worker `worker` makes for the test's folder.
fn setup_on(what: &str, worker: impl FnOnce(&Path) -> Rc<Worker>) -> Setup {
    let dir = crate::testing::scratch_dir(&format!("edit-in-{what}"));
    let root = dir.join("photos");
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let files: Vec<PathBuf> = (0..3)
        .map(|i| {
            let p = shoot.join(format!("x{i}.tif"));
            write_frame(&p, [&R5, &R6, &A7][i], i as u16 + 1);
            p
        })
        .collect();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1400.0, 900.0));
    app.set_grid_open(true);
    let worker = worker(&dir);
    let state = crate::testing::state_with(&app, Vec::new(), worker.clone());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        let collections = dir.join(crate::collections::DIR);
        st.views.collections.dir = Some(collections.clone());
        st.views.stacks =
            crate::stacks::Store::load(greycard_library::stacks::dir_in(&collections));
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    Setup {
        dir,
        files,
        writer,
        app,
        state,
        worker,
    }
}

fn teardown(s: Setup) {
    let Setup {
        dir,
        writer,
        app,
        state,
        worker,
        ..
    } = s;
    crate::WORKER.with(|w| *w.borrow_mut() = None);
    crate::STATE.with(|s| *s.borrow_mut() = None);
    // The pool's threads too, or one still making a picture writes
    // into the folder after it is removed.
    assert!(worker.stop_all(Duration::from_secs(60)), "the worker stops");
    drop(writer);
    drop(app);
    drop(worker);
    crate::testing::remove_scratch(state, &dir);
    let left = crate::testing::left_in(&dir);
    assert!(left.is_empty(), "nothing left behind: {left:?}");
}

/// A picture written to `path` as the export or the app writes it:
/// a flat grey at `level` of sixteen bits.
fn write_level(path: &Path, level: u16) {
    let (w, h) = (48, 32);
    let data = vec![level; (w * h * 3) as usize];
    greycard_library::fixture::write_frame_of(path, &R6, w, h, &data);
}

/// The level the export writes frame `i`'s file at, plus `i`: dark.
const EXPORTED: u16 = 6000;

/// The grid's rows: each frame's name and its stack's mark, "[3]"
/// on a closed stack's top and "[2/3]" on an open one's members.
fn rows(app: &App) -> Vec<String> {
    let thumbs = app.get_thumbs();
    (0..thumbs.row_count())
        .map(|r| {
            let t = thumbs.row_data(r).unwrap();
            match (t.stack, t.stack_open) {
                (0, _) => t.name.to_string(),
                (n, false) => format!("{} [{n}]", t.name),
                (n, true) => format!("{} [{}/{n}]", t.name, t.stack_place),
            }
        })
        .collect()
}

/// Frame `i` of the shoot opened, Edit in pressed on it, and its
/// export said written to the file the set named: the file's path.
fn edit_in(s: &Setup, i: usize) -> PathBuf {
    edit_in_as(s, i, &|out| write_level(out, EXPORTED + i as u16))
}

/// [`edit_in`], the export written by `write`.
fn edit_in_as(s: &Setup, i: usize, write: &dyn Fn(&Path)) -> PathBuf {
    let c = {
        let st = s.state.borrow();
        st.own_cell(st.file_by_path(&s.files[i]).unwrap()).unwrap()
    };
    let row = crate::panel::browser::row_of(&s.state.borrow(), c).unwrap() as i32;
    // Named before the set starts, as the worker names it: nothing
    // is there yet.
    let out = out_of(&s.files[i], None);
    s.app.invoke_select(row);
    s.app.invoke_menu_edit_in(0);
    crate::rows::land_pending(&s.state, &s.app, &s.worker);
    let set = s.state.borrow().work.exporting.clone().expect("the set");
    // The file is the test's. A worker that took the frame before
    // the cancel exports it all the same, to `out`: waited out, so
    // that write lands before the test's, never over the file once
    // it is handed over (as on a loaded macOS runner, where the
    // export's key was lost and a save's thumbnail never came).
    set.cancel();
    let deadline = Instant::now() + Duration::from_secs(60);
    while {
        let t = set.tally();
        t.exported + t.canceled + t.skipped + t.failed.len()
    } == 0
    {
        assert!(Instant::now() < deadline, "waiting for the set's frame");
        std::thread::sleep(Duration::from_millis(1));
    }
    // What the worker writes: the frame as a TIFF.
    write(&out);
    crate::panel::deliver::deliver(
        &s.app,
        Outcome::SetFrameDone {
            set: set.clone(),
            index: 0,
            source: s.files[i].clone(),
            edit: Edit::default(),
            version: None,
            done: queue::Done::Exported {
                path: out.clone(),
                seconds: 1.0,
                note: None,
                left_out: Vec::new(),
            },
        },
    );
    let tally = queue::Tally {
        exported: 1,
        ..queue::Tally::default()
    };
    crate::panel::deliver::deliver(&s.app, Outcome::SetDone { set, tally });
    out
}

/// The app saving `out`: the watch's look sees the new stamp once
/// and takes it at the next, the file indexed (by the test's
/// writer, as the indexer would) in between, and the save brought
/// to the window.
fn saved(s: &mut Setup, out: &Path, level: u16) {
    saved_as(s, out, &|out| write_level(out, level));
}

/// [`saved`], the save written by `write`.
fn saved_as(s: &mut Setup, out: &Path, write: &dyn Fn(&Path)) {
    let before = how::stamp_of(out);
    std::thread::sleep(Duration::from_millis(20));
    write(out);
    assert_ne!(how::stamp_of(out), before, "the save is seen");
    let list = s.state.borrow().work.edit_in.watches.clone();
    assert!(look(&list).is_empty(), "seen once, not yet taken");
    let mut taken = look(&list);
    assert_eq!(taken.len(), 1, "taken once it held");
    assert!(look(&list).is_empty(), "and only once");
    s.writer.index_file(out).unwrap();
    changed(&s.state, &s.app, &s.worker, taken.remove(0));
    land_sent(&s.state, &s.app, &s.worker);
}

/// The file the app writes is indexed and listed beside its raw, in
/// a new stack of the two, opened; a frame already in a stack takes
/// it after itself in that stack. A second save stacks nothing more.
#[test]
fn a_saved_file_is_listed_and_stacked_with_its_raw() {
    let mut s = setup("stack");
    let apps = s.dir.join("apps");
    choose_app(&s.state, &s.app, &apps, "gimp");
    // A file of the name there already is never written over.
    let taken = s.files[2].with_file_name("x2-edit.greycard.tif");
    std::fs::write(&taken, b"").unwrap();
    assert_eq!(
        out_of(&s.files[2], None),
        s.files[2].with_file_name("x2-edit.greycard-1.tif")
    );
    std::fs::remove_file(&taken).unwrap();
    assert_eq!(rows(&s.app), ["x0.tif", "x1.tif", "x2.tif"]);

    let out = edit_in(&s, 1);
    assert_eq!(out, s.files[1].with_file_name("x1-edit.greycard.tif"));
    // Written, not yet saved by the app: the list is as it was.
    assert_eq!(rows(&s.app), ["x0.tif", "x1.tif", "x2.tif"]);
    saved(&mut s, &out, 50);
    assert_eq!(
        rows(&s.app),
        [
            "x0.tif",
            "x1.tif [1/2]",
            "x1-edit.greycard.tif [2/2]",
            "x2.tif"
        ]
    );
    // Saved again: still one stack of two.
    saved(&mut s, &out, 51);
    assert_eq!(s.state.borrow().views.stacks.list.len(), 1);
    assert_eq!(
        rows(&s.app),
        [
            "x0.tif",
            "x1.tif [1/2]",
            "x1-edit.greycard.tif [2/2]",
            "x2.tif"
        ]
    );

    // x0 and x2 stacked, closed, x0 on top: x0's file goes after it.
    {
        let mut st = s.state.borrow_mut();
        let refs = [0, 2]
            .map(|i| Reference {
                hash: greycard_library::hash_file(&s.files[i]).unwrap(),
                hint: crate::stacks::hint_of_path(&st, &s.files[i]),
                version: None,
            })
            .to_vec();
        st.views.stacks.make(refs);
        crate::panel::stacks::settle(&mut st, &s.app, &s.worker);
    }
    assert_eq!(
        rows(&s.app),
        ["x0.tif [2]", "x1.tif [1/2]", "x1-edit.greycard.tif [2/2]"]
    );
    let out = edit_in(&s, 0);
    saved(&mut s, &out, 52);
    assert_eq!(
        rows(&s.app),
        [
            "x0.tif [1/3]",
            "x0-edit.greycard.tif [2/3]",
            "x2.tif [3/3]",
            "x1.tif [1/2]",
            "x1-edit.greycard.tif [2/2]"
        ]
    );
    teardown(s);
}

/// [`setup`] on a worker that makes pictures for real, its
/// thumbnails kept in the test's folder, and says what it made to
/// the test, which takes it in as the window would.
struct Heard {
    s: Setup,
    heard: std::sync::mpsc::Receiver<Outcome>,
    /// The mean of the open frame's last develop to land.
    developed: std::cell::Cell<Option<f64>>,
    /// The generation of the last open the worker said it made.
    opened: std::cell::Cell<Option<u64>>,
    /// A gate the worker stops at when it says it opened a file at
    /// generation 0, until the test lets it go: the worker held
    /// busy ([`Heard::hold`]).
    gate: Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>>,
}

fn heard(what: &str) -> Heard {
    let (tx, heard) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let gate: Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>> = Arc::default();
    let held = gate.clone();
    let s = setup_on(what, move |dir| {
        let worker = Rc::new(Worker::new(move |o| {
            if matches!(o, Outcome::Opened { generation: 0, .. }) {
                let wait = held.lock().unwrap().take();
                if let Some(go) = wait {
                    let _ = go.recv();
                }
            }
            let _ = tx.lock().unwrap().send(o);
        }));
        worker.set_thumb_cache(Some(greycard_library::thumbs::Thumbs::at(
            dir.join("thumbs"),
            greycard_library::thumbs::DEFAULT_CAP,
        )));
        worker
    });
    s.state.borrow_mut().window.write_sidecars = true;
    Heard {
        s,
        heard,
        developed: std::cell::Cell::new(None),
        opened: std::cell::Cell::new(None),
        gate,
    }
}

impl Heard {
    /// The worker held in an open of `path` the window does not wait
    /// on, until the sender returned is used or dropped.
    fn hold(&self, path: &Path) -> std::sync::mpsc::Sender<()> {
        let (go, wait) = std::sync::mpsc::channel();
        *self.gate.lock().unwrap() = Some(wait);
        self.s.worker.send(crate::worker::Job::Open {
            path: path.to_path_buf(),
            version: None,
            edit: Edit::for_picture(),
            generation: 0,
            seed_blend: false,
            turn: 0,
            stance_known: false,
            learned: Vec::new(),
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.gate.lock().unwrap().is_some() {
            assert!(
                Instant::now() < deadline,
                "waiting for the worker to be held"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        go
    }

    /// A slider moved on the panel, its develop sent once it rests:
    /// the generation it was sent at.
    fn slide(&self, exposure: f32) -> u64 {
        self.s.app.set_exposure(exposure);
        self.s.app.invoke_develop_changed();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(
            crate::panel::edit::DEBOUNCE_MS + 50,
        ));
        slint::platform::update_timers_and_animations();
        self.s.state.borrow().develop.generation
    }
    fn cell_of(&self, path: &Path) -> usize {
        let st = self.s.state.borrow();
        st.own_cell(st.file_by_path(path).expect("listed"))
            .expect("a cell of its own")
    }

    /// The mean of cell `c`'s thumbnail as the window holds it, none
    /// while it holds none.
    fn thumb(&self, c: usize) -> Option<f64> {
        let st = self.s.state.borrow();
        let (_, _, rgb) = st.views.thumb_base.get(c)?.as_ref()?;
        Some(greycard_engine::testing::edited::mean(rgb))
    }

    /// What the worker says taken in until `until` holds, as the
    /// window's event loop would take it.
    fn pump(&self, until: &dyn Fn(&Heard) -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !until(self) {
            assert!(Instant::now() < deadline, "waiting for {what}");
            self.take(Duration::from_millis(50));
        }
    }

    /// Whatever the worker says for `wait`, taken in.
    fn quiet(&self, wait: Duration) {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            self.take(Duration::from_millis(50));
        }
    }

    /// One thing the worker says within `wait`, taken in: the
    /// pictures, and a develop of the open frame, its mean noted.
    fn take(&self, wait: Duration) {
        let Ok(outcome) = self.heard.recv_timeout(wait) else {
            return;
        };
        match &outcome {
            Outcome::Developed {
                generation, image, ..
            } => {
                if *generation == self.s.state.borrow().develop.generation {
                    let crate::worker::Developed::Halves(h) = image else {
                        panic!("a develop on the CPU");
                    };
                    let rgb: f64 = h
                        .pixels
                        .chunks(4)
                        .map(|p| p[..3].iter().map(|v| f64::from(v.to_f32())).sum::<f64>())
                        .sum();
                    self.developed
                        .set(Some(rgb / (h.pixels.len() / 4 * 3) as f64));
                }
            }
            Outcome::Thumbnail { .. }
            | Outcome::NoThumbnail { .. }
            | Outcome::Thumbnails(_)
            | Outcome::EditedKept { .. } => {}
            Outcome::Opened { generation, .. } => self.opened.set(Some(*generation)),
            _ => return,
        }
        crate::panel::deliver::deliver(&self.s.app, outcome);
    }

    /// Cell `c`'s thumbnail asked for and taken in.
    fn thumb_of(&self, c: usize) -> f64 {
        {
            let mut st = self.s.state.borrow_mut();
            st.views.thumb_base[c] = None;
            self.s.worker.send(crate::rows::thumb_job(&st, c));
        }
        self.pump(&|h| h.thumb(c).is_some(), "the thumbnail");
        self.thumb(c).unwrap()
    }

    /// `path` opened in Develop, its sidecar read, and its develop
    /// taken in: the develop's mean.
    fn open(&self, path: &Path) -> f64 {
        let c = self.cell_of(path);
        {
            let mut st = self.s.state.borrow_mut();
            let f = st.file_by_path(path).unwrap();
            assert!(crate::rows::load_file(&mut st, &self.s.app, f));
            crate::panel::browser::open_frame(&mut st, &self.s.app, &self.s.worker, c);
        }
        self.developed.set(None);
        self.pump(&|h| h.developed.get().is_some(), "the develop");
        self.developed.get().unwrap()
    }

    /// The file Edit in wrote for frame `i` listed at once, as under
    /// a watched root, before the app has saved it.
    fn listed(&mut self, out: &Path) {
        self.s.writer.index_file(out).unwrap();
        crate::roots::refresh_view(&self.s.state, &self.s.app, &self.s.worker);
        land_sent(&self.s.state, &self.s.app, &self.s.worker);
    }
}

const BRIGHT: u16 = 50000;
const MIDDLE: u16 = 25000;

/// Edit in on frame 1, the file it wrote carrying an edit of
/// greycard's own and listed at once, as under a watched root, so
/// even the app's first save is over a file the window holds.
fn handed(what: &str) -> (Heard, PathBuf) {
    handed_as(what, &|out| write_level(out, EXPORTED + 1))
}

/// [`handed`], the export written by `write`.
fn handed_as(what: &str, write: &dyn Fn(&Path)) -> (Heard, PathBuf) {
    let mut h = heard(what);
    let apps = h.s.dir.join("apps");
    choose_app(&h.s.state, &h.s.app, &apps, "gimp");
    let out = edit_in_as(&h.s, 1, write);
    {
        let mut sidecar = greycard_edit::Sidecar::default();
        let mut edit = Edit::for_picture();
        edit.light.exposure = 0.25;
        sidecar.line = greycard_edit::Line::of(edit, 0).unwrap();
        sidecar.save(&out).unwrap();
    }
    h.listed(&out);
    (h, out)
}

/// The app's save over the file it was handed shows in the grid
/// without the user doing anything, the first save and a later one;
/// the thumbnail kept under the content the save replaced leaves
/// the cache, the export's read at the watch's first look.
#[test]
fn a_save_shows_in_the_grid() {
    let (mut h, out) = handed("save-grid");
    let tiff = h.cell_of(&out);
    let dark = h.thumb_of(tiff);
    let list = h.s.state.borrow().work.edit_in.watches.clone();
    let held = || list.lock().unwrap()[0].held.clone();
    let tried = list.lock().unwrap()[0].held_tried;
    assert!(look(&list).is_empty(), "the app has not saved");
    let exported = crate::worker::thumb_key(&out).unwrap();
    // A miss says why: the stamp at the hand-over against the
    // file's now, and whether a look had tried before this one.
    let handed = list.lock().unwrap()[0].settle.taken();
    assert_eq!(
        held(),
        Some(exported.clone()),
        "the export's key: handed {handed:?}, now {:?}, tried before: {tried}",
        how::stamp_of(&out)
    );
    let size = h.s.state.borrow().views.thumb_made[tiff];
    let worker = h.s.worker.clone();
    let cached = |(hash, stamp): &(String, u64)| {
        let tag = greycard_library::thumbs::Tag {
            recipe: crate::worker::thumb_recipe(&out),
            stamp: *stamp,
        };
        worker
            .thumb_cache()
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .has(hash, size, tag)
    };
    assert!(cached(&exported));
    saved(&mut h.s, &out, BRIGHT);
    assert!(!cached(&exported), "the export's thumbnail let go");
    assert_eq!(held(), crate::worker::thumb_key(&out), "the save's key");
    let tiff = h.cell_of(&out);
    h.pump(
        &|h| h.thumb(tiff).is_some_and(|m| m > dark + 50.0),
        "the first save's thumbnail",
    );
    let bright = h.thumb(tiff).unwrap();
    // And a later one.
    saved(&mut h.s, &out, MIDDLE);
    let tiff = h.cell_of(&out);
    h.pump(
        &|h| {
            h.thumb(tiff)
                .is_some_and(|m| m > dark + 20.0 && m < bright - 20.0)
        },
        "the second save's thumbnail",
    );
    drop(worker);
    teardown(h.s);
}

/// The file open in Develop: a save is read again under the edit the
/// file carries, which stays; and a save that lands while a develop
/// of the file runs shows, never that develop after it.
#[test]
fn a_save_shows_on_the_open_frame() {
    let (mut h, out) = handed("save-open");
    h.s.app.set_grid_open(false);
    let dark = h.open(&out);
    saved(&mut h.s, &out, MIDDLE);
    h.pump(
        &|h| h.developed.get().is_some_and(|m| m > dark * 1.3),
        "the open frame's develop of the first save",
    );
    let middle = h.developed.get().unwrap();
    assert_eq!(h.s.state.borrow().develop.edit.light.exposure, 0.25);
    saved(&mut h.s, &out, BRIGHT);
    h.pump(
        &|h| h.developed.get().is_some_and(|m| m > middle * 1.3),
        "the open frame's develop of the save",
    );
    assert_eq!(h.s.state.borrow().develop.edit.light.exposure, 0.25);
    let open_bright = h.developed.get().unwrap();

    // A save landing while a develop of the file runs: the develop
    // that shows after is the save's.
    let c = h.cell_of(&out);
    crate::panel::browser::open_frame(&mut h.s.state.borrow_mut(), &h.s.app, &h.s.worker, c);
    h.developed.set(None);
    saved(&mut h.s, &out, MIDDLE);
    h.pump(
        &|h| h.developed.get().is_some_and(|m| m < open_bright * 0.8),
        "the develop of the save made while one ran",
    );
    h.quiet(Duration::from_secs(2));
    assert!(
        h.developed.get().unwrap() < open_bright * 0.8,
        "no stale develop after the save's"
    );
    teardown(h.s);
}

/// The raw open in Develop and the file beside it in its stack: the
/// file's picture in the strip shows each save, and stepped onto it
/// shows the last.
#[test]
fn a_save_shows_beside_the_open_raw() {
    let (mut h, out) = handed("save-beside");
    h.s.app.set_grid_open(false);
    let raw = h.s.files[1].clone();
    h.open(&raw);
    let tiff = h.cell_of(&out);
    let dark = h.thumb_of(tiff);
    let stepped = h.open(&out);
    h.open(&raw);
    saved(&mut h.s, &out, BRIGHT);
    let tiff = h.cell_of(&out);
    h.pump(
        &|h| h.thumb(tiff).is_some_and(|m| m > dark + 50.0),
        "the first save's thumbnail",
    );
    let bright = h.thumb(tiff).unwrap();
    saved(&mut h.s, &out, MIDDLE);
    let tiff = h.cell_of(&out);
    h.pump(
        &|h| {
            h.thumb(tiff)
                .is_some_and(|m| m > dark + 20.0 && m < bright - 20.0)
        },
        "the second save's thumbnail",
    );
    let now = h.open(&out);
    assert!(now > stepped * 1.3, "{now} against {stepped}");
    teardown(h.s);
}

/// A save is not the user opening the frame: over the file open in
/// Develop it leaves the view where it is, the zoom and the center;
/// a slider mid-drag keeps its value, and is what the save develops
/// under; no history step is made and no sidecar written; and the
/// tool in hand, the mask chosen, the crop, and the guide stay.
#[test]
fn a_save_leaves_the_view_and_the_hand_alone() {
    let (mut h, out) = handed("save-hand");
    h.s.app.set_grid_open(false);
    let dark = h.open(&out);
    let c = h.cell_of(&out);
    // Mid-drag: the slider moved, its develop asked, the save of
    // the panel still waiting on its timer.
    h.s.app.set_exposure(1.3);
    h.s.app.invoke_develop_changed();
    let sidecar = greycard_edit::Sidecar::path_for(&out);
    let on_disk = std::fs::read(&sidecar).unwrap();
    let (line, generation) = {
        let mut st = h.s.state.borrow_mut();
        assert_eq!(st.develop.edit.light.exposure, 1.3);
        st.develop.zoom = 2.0;
        st.develop.center = (13.0, 9.0);
        st.develop.target = Some(0);
        st.develop.placing = Some(crate::panel::mask::Placing {
            kind: greycard_edit::mask::Shape::of_kind("Radial"),
            mode: greycard_edit::mask::Mode::Add,
            from: (0.0, 0.0),
            index: 0,
            component: None,
            boxed: false,
            held: false,
        });
        (st.line(c).cloned().unwrap(), st.develop.generation)
    };
    h.s.app.set_placing("Radial".into());
    h.s.app.set_crop_mode(true);
    h.s.app.set_guide_mode("level".into());
    saved(&mut h.s, &out, BRIGHT);
    h.pump(
        &|h| h.developed.get().is_some_and(|m| m > dark * 1.3),
        "the save's develop",
    );
    // The save stacked the file with its raw: the list renumbered.
    let c = h.cell_of(&out);
    let st = h.s.state.borrow();
    assert!(st.develop.generation > generation, "read again");
    assert_eq!(st.develop.zoom, 2.0, "the zoom kept");
    assert_eq!(st.develop.center, (13.0, 9.0), "the center kept");
    assert!(st.develop.held.is_none());
    assert_eq!(h.s.app.get_exposure(), 1.3, "the slider under the hand");
    assert_eq!(st.develop.edit.light.exposure, 1.3);
    assert_eq!(st.line(c), Some(&line), "no history step");
    assert_eq!(
        std::fs::read(&sidecar).unwrap(),
        on_disk,
        "no sidecar write"
    );
    assert_eq!(st.develop.target, Some(0), "the mask chosen");
    assert!(st.develop.placing.is_some(), "the tool in hand");
    assert_eq!(h.s.app.get_placing(), "Radial");
    assert!(h.s.app.get_crop_mode() && !st.develop.crop_start_due);
    assert_eq!(h.s.app.get_guide_mode(), "level");
    assert_eq!(st.current, Some(c));
    drop(st);
    teardown(h.s);
}

/// A pick of another frame waiting on its sidecar wins over a save
/// of the open file: the save reads nothing again, and the pick
/// lands on the frame picked.
#[test]
fn a_pick_waiting_wins_over_a_save() {
    let (mut h, out) = handed("save-pick");
    h.s.app.set_grid_open(false);
    h.open(&out);
    let other = h.s.files[0].clone();
    // A pick's read out while the file is still the one current:
    // the save must neither read it again nor let the pick go.
    let generation = {
        let mut st = h.s.state.borrow_mut();
        st.views.pick_pending = Some(other.clone());
        st.develop.generation
    };
    saved(&mut h.s, &out, BRIGHT);
    {
        let st = h.s.state.borrow();
        assert_eq!(
            st.views.pick_pending.as_ref(),
            Some(&other),
            "the pick stands"
        );
        assert_eq!(st.develop.generation, generation, "nothing read again");
        assert_eq!(
            st.current.and_then(|c| st.path_at(c).cloned()),
            Some(out.clone())
        );
    }
    h.s.state.borrow_mut().views.pick_pending = None;

    // The pick pressed for real: the save lands under it, and the
    // frame picked opens.
    let c = h.cell_of(&other);
    let row = crate::panel::browser::row_of(&h.s.state.borrow(), c).unwrap() as i32;
    h.s.app.invoke_select(row);
    let picked = h.s.state.borrow().views.pick_pending.clone();
    saved(&mut h.s, &out, MIDDLE);
    crate::rows::land_pending(&h.s.state, &h.s.app, &h.s.worker);
    let st = h.s.state.borrow();
    assert_eq!(
        st.current.and_then(|c| st.path_at(c).cloned()),
        Some(other),
        "the pick, {picked:?} on its way"
    );
    drop(st);
    teardown(h.s);
}

/// An uncompressed TIFF written to `path`, its top half `top` and
/// its bottom half `bottom`: the first 64 KB are the header and the
/// top, so the content hash, which reads the head and the length,
/// is the same whatever the bottom is.
fn write_halves(path: &Path, top: u8, bottom: u8) {
    image::RgbImage::from_fn(256, 256, |_, y| {
        image::Rgb([if y < 128 { top } else { bottom }; 3])
    })
    .save_with_format(path, image::ImageFormat::Tiff)
    .unwrap();
}

/// A save that changes the file past the hashed head at the same
/// length keeps its hash: it shows all the same, the edit's
/// pictures kept under that hash let go, the save landing before
/// the watch's first look read the export's key.
#[test]
fn a_save_past_the_hashed_head_shows() {
    let (mut h, out) = handed_as("save-head", &|out| write_halves(out, 80, 25));
    let hash = greycard_library::hash_file(&out).unwrap();
    let len = std::fs::metadata(&out).unwrap().len();
    // The edit's pictures of it kept from its develop, as an open
    // keeps them, and the frame left for another.
    h.s.app.set_grid_open(false);
    h.open(&out);
    let key = {
        let st = h.s.state.borrow();
        h.s.worker.edited().key_of(&out, &st.develop.edit)
    };
    h.pump(
        &|h| h.s.worker.edited().all_kept(&hash, key),
        "the edit's pictures",
    );
    h.open(&h.s.files[0].clone());
    let tiff = h.cell_of(&out);
    let before = h.thumb_of(tiff);
    saved_as(&mut h.s, &out, &|out| write_halves(out, 80, 200));
    assert_eq!(greycard_library::hash_file(&out).unwrap(), hash, "one hash");
    assert_eq!(std::fs::metadata(&out).unwrap().len(), len);
    let tiff = h.cell_of(&out);
    h.pump(
        &|h| h.thumb(tiff).is_some_and(|m| m > before + 30.0),
        "the save's thumbnail",
    );
    teardown(h.s);
}

/// The Original on show when a save lands stays on, made again of
/// the new bytes; turned off after, the edit of the new bytes shows.
#[test]
fn a_save_keeps_the_original_on() {
    let (mut h, out) = handed("save-original");
    h.s.app.set_grid_open(false);
    h.open(&out);
    h.s.app.invoke_original_key();
    assert!(h.s.app.get_original_on());
    h.developed.set(None);
    h.pump(&|h| h.developed.get().is_some(), "the Original");
    let original = h.developed.get().unwrap();
    saved(&mut h.s, &out, BRIGHT);
    h.pump(
        &|h| h.developed.get().is_some_and(|m| m > original * 1.3),
        "the Original of the save",
    );
    {
        let st = h.s.state.borrow();
        assert!(st.develop.original.on && h.s.app.get_original_on());
        assert_eq!(st.develop.original.asked, Some(st.develop.generation));
    }
    // The edit's pictures of the new bytes kept all the same, for the
    // strip and the grid, from the open's develop of the edit.
    let hash = greycard_library::hash_file(&out).unwrap();
    let key = {
        let st = h.s.state.borrow();
        h.s.worker.edited().key_of(&out, &st.develop.edit)
    };
    h.pump(
        &|h| h.s.worker.edited().all_kept(&hash, key),
        "the edit's pictures of the save",
    );
    assert!(h.s.app.get_original_on(), "still on");
    // Off: the edit of the new bytes, not a picture of the old.
    h.s.app.invoke_original_key();
    assert!(!h.s.app.get_original_on());
    h.developed.set(None);
    h.pump(
        &|h| h.developed.get().is_some_and(|m| m > original * 1.3),
        "the edit of the save",
    );
    teardown(h.s);
}

/// A save that lands while the worker is busy, and a slider moved
/// before the worker has taken the reread: the slider's develop
/// does not drop the reread waiting for the worker. The file is
/// opened again under the slider's edit, at its generation, and the
/// picture that lands is of the new bytes.
#[test]
fn a_slider_moved_over_a_waiting_reread_keeps_it() {
    let (mut h, out) = handed("save-slider");
    h.s.app.set_grid_open(false);
    let dark = h.open(&out);
    // The worker held at another file's open, which the window does
    // not wait on.
    let busy = h.s.dir.join("busy.tif");
    write_level(&busy, MIDDLE);
    let go = h.hold(&busy);
    saved(&mut h.s, &out, BRIGHT);
    let reread = h.s.state.borrow().develop.generation;
    let slider = h.slide(1.3);
    drop(go);
    assert!(slider > reread, "the slider's develop sent");
    h.developed.set(None);
    h.pump(
        &|h| h.opened.get() == Some(slider) && h.developed.get().is_some(),
        "the open under the slider's develop",
    );
    let landed = h.developed.get().unwrap();
    assert!(
        landed > dark * 1.3,
        "the new bytes: {landed} against {dark}"
    );
    assert_eq!(h.s.state.borrow().develop.edit.light.exposure, 1.3);
    teardown(h.s);
}

/// An open waiting for the worker takes no develop of another
/// frame: x1's open waits, x2 is picked while its sidecar is read,
/// and a slider moved on x2's panel sends a develop for x2. Nothing
/// of x1 lands on the panel while x2 is current.
#[test]
fn a_develop_of_another_frame_leaves_a_waiting_open_alone() {
    let h = heard("pick-fold");
    h.s.app.set_grid_open(false);
    let files = h.s.files.clone();
    h.open(&files[0]);
    let busy = h.s.dir.join("busy.tif");
    write_level(&busy, MIDDLE);
    let go = h.hold(&busy);
    let x1 = h.cell_of(&files[1]);
    {
        let mut st = h.s.state.borrow_mut();
        let f = st.file_by_path(&files[1]).unwrap();
        assert!(crate::rows::load_file(&mut st, &h.s.app, f));
        crate::panel::browser::open_frame(&mut st, &h.s.app, &h.s.worker, x1);
    }
    let opening = h.s.state.borrow().develop.generation;
    let x2 = h.cell_of(&files[2]);
    let row = {
        // x2's sidecar not yet read, as the window would hold it.
        let mut st = h.s.state.borrow_mut();
        let f = st.file_by_path(&files[2]).unwrap();
        st.from_row[f].read = false;
        crate::panel::browser::row_of(&st, x2).unwrap() as i32
    };
    h.s.app.invoke_select(row);
    assert_eq!(
        h.s.state.borrow().views.pick_pending.as_ref(),
        Some(&files[2]),
        "the pick waits on its sidecar"
    );
    let slider = h.slide(1.3);
    assert!(slider > opening, "the slider's develop sent");
    drop(go);
    h.quiet(Duration::from_secs(2));
    let st = h.s.state.borrow();
    assert_eq!(st.current, Some(x2));
    assert_ne!(h.opened.get(), Some(slider), "x1 opened as x2");
    assert!(
        st.develop.opened_sizes.iter().all(|(p, _)| p != &files[2]),
        "x1's size under x2: {:?}",
        st.develop.opened_sizes
    );
    drop(st);
    teardown(h.s);
}

/// The frames the arrow keys reach are sent again with the reread,
/// so the worker takes a neighbors' change after it, as after any
/// open, rather than refusing it as an older open's.
#[test]
fn a_save_keeps_the_prefetch_going() {
    let (mut h, out) = handed("save-prefetch");
    h.s.app.set_grid_open(false);
    h.open(&out);
    let dark = h.developed.get().unwrap();
    saved(&mut h.s, &out, BRIGHT);
    h.pump(
        &|h| h.developed.get().is_some_and(|m| m > dark * 1.3),
        "the save's develop",
    );
    let c = h.cell_of(&out);
    let st = h.s.state.borrow();
    let sent = st
        .develop
        .neighbors_sent
        .as_ref()
        .expect("sent")
        .generation();
    assert_eq!(sent, st.develop.generation);
    assert!(!crate::panel::browser::neighbors_of(&st, c).is_empty());
    assert!(h.s.worker.neighbors_again(
        sent,
        &out,
        st.develop.walk.heading(),
        crate::panel::browser::neighbors_of(&st, c),
        "a test",
    ));
    drop(st);
    teardown(h.s);
}
