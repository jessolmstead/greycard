use std::path::{Path, PathBuf};

use i_slint_backend_testing::ElementHandle;
use slint::platform::{Key, PointerEventButton};
use slint::{ComponentHandle, Model};

use super::*;
use crate::Shared;
use crate::panel::archive::tests::land_sent;
use crate::testing::{
    folder, menu_entries, menu_entries_starting, press, settle, state_for, type_text, window,
};
use crate::worker::{Outcome, Worker};
use greycard_library::fixture::{A7, R5, R6, write_frame};

/// Below a fluent menu's lowest entry, its frame's margin (5 px) and
/// its shadow's blur (16 px), which must fit in the window too.
const BELOW_THE_LOWEST: f32 = 5.0 + 16.0;

/// The window's height in the tests: 27 rows of a menu (`MenuFit`).
const HEIGHT: f32 = 950.0;

fn rows(app: &App, others: i32) -> usize {
    app.global::<crate::MenuFit>().invoke_rows(others) as usize
}

/// The entry of the menus up titled `title`.
fn entry(app: &App, title: &str) -> Option<ElementHandle> {
    menu_entries(app)
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some(title))
}

/// The foot of the lowest entry of the menus up.
fn foot(app: &App) -> f32 {
    menu_entries(app)
        .iter()
        .map(|e| e.absolute_position().y + e.size().height)
        .fold(0.0, f32::max)
}

/// The frame menu up over the strip's first frame, as `--menu` opens
/// it, and its submenu `title` opened by the keys.
fn frame_submenu(app: &App, title: &str) {
    app.set_menu_at(0);
    settle(app);
    assert!(app.get_menu_up());
    crate::testing::open_submenu(app, title);
}

/// The menus closed, and the frame menu's ask let go so the next is a
/// change.
fn close_menus(app: &App) {
    for _ in 0..2 {
        if app.get_menu_up() {
            press(app, Key::Escape);
        }
    }
    assert!(!app.get_menu_up());
    app.set_menu_at(-1);
    settle(app);
}

/// The sheet's rows as it has made them, top first, by name.
fn sheet_rows(app: &App) -> Vec<String> {
    settle(app);
    let mut rows = crate::testing::all_by_id(app, "ChoiceSheet::row");
    rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    rows.iter()
        .map(|r| r.accessible_label().unwrap().to_string())
        .collect()
}

/// The menu's last entry, `title`, clicked as a hand clicks it: the
/// sheet up once the menu has closed.
fn take(app: &App, title: &str) {
    entry(app, title)
        .unwrap_or_else(|| panic!("no {title:?} in the menu"))
        .mock_single_click(PointerEventButton::Left);
    settle(app);
    assert!(!app.get_menu_up());
    assert!(app.get_choice_sheet_open(), "the sheet up from {title}");
}

/// The list's narrowing: by a name's words, case aside, each by its
/// place in the whole list.
#[test]
fn the_list_narrows_by_name_and_keeps_each_place() {
    let list: Vec<slint::SharedString> = ["Gimp", "Krita", "darktable", "GIMP 3"]
        .iter()
        .map(|&n| n.into())
        .collect();
    let got = |typed: &str| -> Vec<(String, String)> {
        narrow(&list, typed)
            .into_iter()
            .map(|c| (c.name.to_string(), c.id.to_string()))
            .collect()
    };
    assert_eq!(got("").len(), 4);
    assert_eq!(
        got(" gimp "),
        [("Gimp".into(), "0".into()), ("GIMP 3".into(), "3".into())]
    );
    assert_eq!(got("TABLE"), [("darktable".to_string(), "2".to_string())]);
    assert!(got("zz").is_empty());
}

/// A pick's place in the live list: the same while it holds the name,
/// the name's place when the list has moved, none when it is gone.
#[test]
fn a_pick_follows_its_name_into_the_live_list() {
    let names = |l: &[&str]| -> Vec<slint::SharedString> { l.iter().map(|&n| n.into()).collect() };
    let live = |l: &[&str]| ModelRc::new(VecModel::from(names(l)));
    let held = names(&["a", "b", "c"]);
    assert_eq!(place(&held, &live(&["a", "b", "c"]), 1), Some(1));
    assert_eq!(place(&held, &live(&["b", "c"]), 1), Some(0));
    assert_eq!(place(&held, &live(&["a", "c"]), 1), None);
    assert_eq!(place(&held, &live(&[]), 0), None);
    assert_eq!(place(&held, &live(&["a", "b", "c"]), 7), None);
}

/// Apps on the disk, as Edit in offers only those: `n` empty files in
/// `dir`, named "App 000" on.
fn apps(state: &Rc<Shared>, dir: &Path, n: usize) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).unwrap();
    let paths: Vec<PathBuf> = (0..n).map(|i| dir.join(format!("app{i:03}"))).collect();
    for p in &paths {
        std::fs::write(p, b"").unwrap();
    }
    state.borrow_mut().work.edit_in.apps = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.clone(), format!("App {i:03}")))
        .collect();
    paths
}

/// Edit in over 400 apps names what the window holds less one, and All
/// apps... last, all of it in the window; over three, the three and no
/// All apps.... The sheet it opens is headed Edit in; a search and
/// Return hand the frame to the app found, as its menu item does;
/// Escape closes it and hands nothing over.
#[test]
fn edit_in_past_the_window_opens_all_apps() {
    let dir = crate::testing::scratch_dir("choices-edit-in");
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, HEIGHT));
    let (state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_busy(false);
    settle(&app);

    apps(&state, &dir.join("few"), 3);
    frame_submenu(&app, "Edit in");
    assert_eq!(menu_entries_starting(&app, "App "), 3);
    assert!(entry(&app, "All apps...").is_none());
    close_menus(&app);
    // At the edge: as many as the window holds, all named; one more,
    // and the last row is All apps..., the rows as many as before.
    let fit = rows(&app, 0);
    apps(&state, &dir.join("edge"), fit);
    frame_submenu(&app, "Edit in");
    assert_eq!(menu_entries_starting(&app, "App "), fit);
    assert!(entry(&app, "All apps...").is_none());
    close_menus(&app);
    apps(&state, &dir.join("over"), fit + 1);
    frame_submenu(&app, "Edit in");
    assert_eq!(menu_entries_starting(&app, "App ") + 1, fit);
    assert!(entry(&app, "All apps...").is_some());
    close_menus(&app);

    let paths = apps(&state, &dir.join("many"), 400);
    frame_submenu(&app, "Edit in");
    let named = menu_entries_starting(&app, "App ");
    assert_eq!(named + 1, rows(&app, 0), "the item counts toward the rows");
    assert!(entry(&app, "All apps...").is_some());
    assert!(foot(&app) + BELOW_THE_LOWEST <= HEIGHT, "{}", foot(&app));
    take(&app, "All apps...");
    assert_eq!(app.get_choice_sheet_title(), "Edit in");
    assert_eq!(app.get_choice_found().row_count(), 400);
    type_text(&app, "app 321");
    assert_eq!(sheet_rows(&app), ["App 321"]);
    press(&app, Key::Return);
    settle(&app);
    assert!(!app.get_choice_sheet_open());
    // The export written: handed to the app found.
    let set = state.borrow().work.exporting.clone().expect("the set");
    let source = state.borrow().files[0].clone();
    let out = PathBuf::from("/nowhere/IMG_0000-edit.greycard.tif");
    crate::panel::edit_in::LAUNCHED.with(|l| l.borrow_mut().clear());
    crate::panel::deliver::deliver(
        &app,
        Outcome::SetFrameDone {
            set: set.clone(),
            index: 0,
            source,
            edit: greycard_edit::Edit::default(),
            version: None,
            done: crate::queue::Done::Exported {
                path: out.clone(),
                seconds: 1.0,
                note: None,
                left_out: Vec::new(),
            },
        },
    );
    let launched = crate::panel::edit_in::LAUNCHED.with(|l| l.borrow().clone());
    use crate::edit_in::{Platform, launch};
    assert_eq!(launched, [launch(&paths[321], &out, Platform::this())]);
    let tally = crate::queue::Tally::default();
    crate::panel::deliver::deliver(&app, Outcome::SetDone { set, tally });
    assert!(state.borrow().work.exporting.is_none());

    // Escape: closed, nothing handed over.
    close_menus(&app);
    frame_submenu(&app, "Edit in");
    take(&app, "All apps...");
    type_text(&app, "app 1");
    press(&app, Key::Escape);
    settle(&app);
    assert!(!app.get_choice_sheet_open());
    assert!(state.borrow().work.exporting.is_none());
    drop(app);
    crate::testing::remove_scratch(state, &dir);
}

/// A window over a shoot of two frames (and a reject) in a local root,
/// beside `n` archives "arch000" on.
struct Archives {
    dir: PathBuf,
    app: App,
    state: Rc<Shared>,
    worker: Rc<Worker>,
}

fn archives(what: &str, n: usize) -> Archives {
    let dir = crate::testing::scratch_dir(&format!("choices-{what}"));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dunce::canonicalize(&dir).unwrap();
    let local = dir.join("local");
    let shoot = local.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let files: Vec<PathBuf> = ["a.tif", "b.tif"].iter().map(|f| shoot.join(f)).collect();
    write_frame(&files[0], &R5, 901);
    write_frame(&files[1], &R6, 902);
    let rejects = crate::cull::rejects_dir(&shoot);
    std::fs::create_dir_all(&rejects).unwrap();
    write_frame(&rejects.join("r.tif"), &A7, 903);
    let nas: Vec<PathBuf> = (0..n)
        .map(|i| dir.join("archives").join(format!("arch{i:03}")))
        .collect();
    for a in &nas {
        std::fs::create_dir_all(a).unwrap();
    }
    let db = dir.join("library.sqlite");
    {
        let mut lib = greycard_library::Library::open(&db).unwrap();
        lib.index_tree(&local, &mut |_| {}).unwrap();
    }
    let app = window(files.len());
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, HEIGHT));
    let (state, worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.index_path = Some(db.clone());
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        let file = dir.join("roots.json");
        let mut roots = greycard_library::Roots::default();
        roots.add(&local).unwrap();
        for a in &nas {
            roots.add(a).unwrap();
            roots.set_archive(a, true);
        }
        roots.save(&file).unwrap();
        st.views.library.roots = roots;
        st.views.library.file = Some(file);
        crate::panel::browser::rebuild_browser(&mut st, &app);
        crate::panel::archive::show(&st, &app);
    }
    app.invoke_select(0);
    app.set_busy(false);
    settle(&app);
    Archives {
        dir,
        app,
        state,
        worker,
    }
}

impl Archives {
    fn done(self) {
        let Archives {
            dir, app, state, ..
        } = self;
        drop(app);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The grid header's `button` pressed: its menu up.
    fn header_menu(&self, button: &str) {
        self.app.set_grid_open(true);
        settle(&self.app);
        ElementHandle::find_by_element_id(&self.app, button)
            .next()
            .unwrap_or_else(|| panic!("{button}"))
            .invoke_accessible_default_action();
        settle(&self.app);
        assert!(self.app.get_menu_up(), "{button}'s menu up");
    }
}

/// Each archive menu names every archive while the window holds them
/// all, and no All archives...: two, and exactly as many as the frame
/// menu holds; one more, or 400, and it names what the window holds
/// less one, then All archives..., the menu in the window.
#[test]
fn the_archive_menus_offer_all_archives_only_past_the_window() {
    // What the frame menu holds at this height (`MenuFit.rows(0)`).
    const FIT: usize = 27;
    for n in [2, FIT, FIT + 1, 400] {
        let a = archives(&format!("menus-{n}"), n);
        let app = &a.app;
        assert_eq!(rows(app, 0), FIT);
        for (menu, prefix, own) in [
            ("frame menu", "arch", 0),
            ("GridSheet::archive-button", "Back up to arch", 0),
            ("GridSheet::rejects-button", "Remove rejects from arch", 5),
        ] {
            if own == 0 && menu == "frame menu" {
                frame_submenu(app, "Back up to");
            } else {
                a.header_menu(menu);
            }
            let named = menu_entries_starting(app, prefix);
            let cut = n > rows(app, own);
            assert_eq!(entry(app, "All archives...").is_some(), cut, "{menu} {n}");
            if cut {
                assert_eq!(named + 1, rows(app, own), "{menu} {n}");
            } else {
                assert_eq!(named, n, "{menu} {n}");
            }
            assert!(foot(app) + BELOW_THE_LOWEST <= HEIGHT, "{}", foot(app));
            close_menus(app);
        }
        a.done();
    }
}

/// The frame menu's All archives...: headed as its submenu, and the
/// archive found backs up the two frames chosen, as its item does;
/// Escape closes the sheet and asks nothing.
#[test]
fn the_frame_menu_s_all_archives_backs_the_selection_up() {
    let a = archives("frame", 400);
    let (app, state) = (&a.app, &a.state);
    state.borrow_mut().views.picked = vec![0, 1];
    frame_submenu(app, "Back up to");
    take(app, "All archives...");
    assert_eq!(app.get_choice_sheet_title(), "Back up to");
    type_text(app, "ARCH123");
    assert_eq!(sheet_rows(app), ["arch123"]);
    press(app, Key::Return);
    settle(app);
    assert!(!app.get_choice_sheet_open());
    assert!(app.get_archive_open());
    assert_eq!(app.get_archive_title(), "Back up to arch123");
    assert_eq!(land_sent(state, app, &a.worker), 1, "the look");
    let text = app.get_archive_text();
    assert!(text.starts_with("2 frames to copy"), "{text}");
    app.invoke_archive_answered(false);

    close_menus(app);
    frame_submenu(app, "Back up to");
    take(app, "All archives...");
    press(app, Key::Escape);
    settle(app);
    assert!(!app.get_choice_sheet_open());
    assert!(!app.get_archive_open());
    a.done();
}

/// The sheet holds the list as the menu showed it. A view with no side
/// empties the live list while the sheet is up: the rows stay, and a
/// pick then asks nothing. A live list that has moved under the sheet
/// takes the pick to the archive of the name picked.
#[test]
fn the_sheet_holds_its_list_while_the_live_one_changes() {
    let a = archives("live", 400);
    let (app, state) = (&a.app, &a.state);
    frame_submenu(app, "Back up to");
    take(app, "All archives...");
    // What `show` sets for a view with no side.
    app.set_archive_verb("".into());
    app.set_archive_choices(ModelRc::default());
    type_text(app, "arch12");
    assert_eq!(app.get_choice_found().row_count(), 10);
    assert_eq!(sheet_rows(app)[0], "arch120");
    type_text(app, "3");
    assert_eq!(sheet_rows(app), ["arch123"]);
    press(app, Key::Return);
    settle(app);
    assert!(!app.get_choice_sheet_open());
    assert!(!app.get_archive_open(), "{}", app.get_status());

    close_menus(app);
    frame_submenu(app, "Back up to");
    take(app, "All archives...");
    type_text(app, "arch123");
    // The first archive unmarked meanwhile: every place after it moves
    // up one.
    {
        let first = a.dir.join("archives").join("arch000");
        let mut st = state.borrow_mut();
        st.views.library.roots.set_archive(&first, false);
    }
    crate::panel::archive::menu_asked(&state.borrow(), app);
    assert_eq!(app.get_archive_choices().row_count(), 399);
    press(app, Key::Return);
    settle(app);
    assert!(app.get_archive_open());
    assert_eq!(app.get_archive_title(), "Back up to arch123");
    app.invoke_archive_answered(false);
    a.done();
}

/// The grid header's All archives...: headed as the frame menu's
/// submenu, listing the archives by name, and the archive found asks
/// the backup its item does.
#[test]
fn the_header_s_all_archives_asks_the_backup() {
    let a = archives("header", 400);
    let app = &a.app;
    a.header_menu("GridSheet::archive-button");
    take(app, "All archives...");
    assert_eq!(app.get_choice_sheet_title(), "Back up to");
    type_text(app, "arch077");
    assert_eq!(sheet_rows(app), ["arch077"]);
    press(app, Key::Return);
    settle(app);
    assert!(!app.get_choice_sheet_open());
    assert!(app.get_archive_open());
    assert_eq!(app.get_archive_title(), "Back up to arch077");
    app.invoke_archive_answered(false);

    a.header_menu("GridSheet::archive-button");
    take(app, "All archives...");
    press(app, Key::Escape);
    settle(app);
    assert!(!app.get_choice_sheet_open());
    assert!(!app.get_archive_open());
    a.done();
}

/// The rejects menu's All archives...: headed with the Rejects
/// button's words, and the archive found weighs the rejects against
/// it, as its item does.
#[test]
fn the_rejects_menu_s_all_archives_asks_the_remove() {
    let a = archives("rejects", 400);
    let (app, state) = (&a.app, &a.state);
    a.header_menu("GridSheet::rejects-button");
    take(app, "All archives...");
    // The button's words with no count of rejects taken.
    assert_eq!(app.get_choice_sheet_title(), "Rejects");
    type_text(app, "arch250");
    assert_eq!(sheet_rows(app), ["Remove rejects from arch250..."]);
    press(app, Key::Return);
    settle(app);
    assert!(!app.get_choice_sheet_open());
    assert!(land_sent(state, app, &a.worker) >= 1, "the look sent");
    assert!(app.get_archive_rejects_open(), "{}", app.get_status());
    assert_eq!(
        app.get_archive_rejects_title(),
        "Remove rejects from arch250"
    );
    app.invoke_archive_rejects_answered(0);

    a.header_menu("GridSheet::rejects-button");
    take(app, "All archives...");
    press(app, Key::Escape);
    settle(app);
    assert!(!app.get_choice_sheet_open());
    assert_eq!(land_sent(state, app, &a.worker), 0, "nothing asked");
    assert!(!app.get_archive_rejects_open());
    a.done();
}
