use super::*;
use crate::SortMenu;
use greycard_edit::Placement;
use slint::Model;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-prefs-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Four frames: two with a sidecar beside, one under the hidden
/// folder, one with none.
fn shoot(dir: &Path) -> Vec<PathBuf> {
    let files: Vec<PathBuf> = ["A", "B", "C", "D"]
        .iter()
        .map(|n| dir.join(format!("{n}.CR3")))
        .collect();
    for f in &files {
        std::fs::write(f, b"raw").unwrap();
    }
    let mut sidecar = Sidecar::default();
    sidecar.meta.rating = 3;
    sidecar.save_in(&files[0], Placement::Beside).unwrap();
    sidecar.save_in(&files[1], Placement::Beside).unwrap();
    sidecar.save_in(&files[2], Placement::Folder).unwrap();
    files
}

#[test]
fn the_move_puts_a_folders_sidecars_in_the_chosen_place() {
    let dir = scratch("move");
    let files = shoot(&dir);
    assert_eq!(sidecars_elsewhere(&files, Placement::Folder), 2);
    assert_eq!(sidecars_elsewhere(&files, Placement::Beside), 1);

    let moved = move_sidecars(&files, Placement::Folder);
    assert_eq!(
        moved,
        Moved {
            moved: 2,
            dropped: 0,
            failed: Vec::new()
        }
    );
    assert_eq!(sidecars_elsewhere(&files, Placement::Folder), 0);
    for f in &files[..3] {
        assert!(Sidecar::path_in(f, Placement::Folder).exists());
        assert!(!Sidecar::path_in(f, Placement::Beside).exists());
        // What was there is what is read: the rating came along.
        assert_eq!(Sidecar::load(f).unwrap().unwrap().meta.rating, 3);
    }
    assert!(Sidecar::find(&files[3]).is_none(), "none made");
    assert_eq!(
        moved_words(&moved, Placement::Folder),
        "Moved 2 sidecars into the hidden folder."
    );

    // A second move finds nothing to do; and back beside, all three.
    assert_eq!(move_sidecars(&files, Placement::Folder).moved, 0);
    assert_eq!(move_sidecars(&files, Placement::Beside).moved, 3);
    assert_eq!(sidecars_elsewhere(&files, Placement::Beside), 0);
    crate::testing::remove_dir_retry(&dir);
}

/// The Settings switch for a new edit's lens section: on by
/// default; a click turns it off, and a raw with no sidecar then
/// reads with the section off and still untouched, while one with
/// a sidecar keeps its own. Back on, the next read is the default.
#[test]
fn the_lens_switch_decides_a_new_edits_lens_section() {
    let dir = scratch("lens-new");
    let files = shoot(&dir);
    let app = crate::testing::window(files.len());
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    state.borrow_mut().window.write_sidecars = true;
    app.invoke_settings_asked();
    assert!(app.get_lens_on_new(), "on by default");
    app.set_settings_category("Editing".into());
    let label = "Lens corrections on new edits";
    let (at, size) = crate::testing::labeled(&app, label);
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(!app.get_lens_on_new());
    assert!(!state.borrow().window.lens_on_new);

    let lens_of = |file: usize| {
        let mut st = state.borrow_mut();
        assert!(crate::rows::reread_file(&mut st, &app, file));
        let line = &st.sidecars[file].line;
        let (edited, _) = greycard_library::develop_marks(&st.sidecars[file], &files[file], None);
        (line.current.lens.enabled, edited)
    };
    assert_eq!(lens_of(3), (false, false), "none on disk: off, untouched");
    assert_eq!(lens_of(0), (true, false), "the sidecar's own");
    {
        let st = state.borrow();
        assert!(greycard_edit_io::on_disk::never_developed(&st.sidecars[3]));
    }
    // Nothing was written for the frame with none.
    assert!(Sidecar::find(&files[3]).is_none());

    app.set_lens_on_new(true);
    app.invoke_lens_on_new_changed();
    assert_eq!(lens_of(3), (true, false));
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn the_sheet_opens_counts_and_moves() {
    let dir = scratch("sheet");
    let files = shoot(&dir);
    let app = crate::testing::window(files.len());
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    state.borrow_mut().window.write_sidecars = true;

    // Ctrl+, opens it, with the place and the count as they are.
    app.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: slint::platform::Key::Control.into(),
        });
    crate::testing::press(&app, ",");
    app.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
            text: slint::platform::Key::Control.into(),
        });
    assert!(app.get_settings_open());
    assert_eq!(app.get_sidecar_placement(), BESIDE);
    assert_eq!(app.get_sidecars_elsewhere(), 1);

    // Flipping the place moves nothing: only the count changes.
    app.set_sidecar_placement(FOLDER.into());
    app.invoke_placement_changed();
    assert_eq!(state.borrow().window.placement, Placement::Folder);
    assert_eq!(app.get_sidecars_elsewhere(), 2);
    assert!(Sidecar::path_in(&files[0], Placement::Beside).exists());

    // The move, confirmed: done, reported, and the count is none.
    app.invoke_sidecars_move();
    assert_eq!(app.get_sidecars_elsewhere(), 0);
    assert_eq!(
        app.get_settings_note(),
        "Moved 2 sidecars into the hidden folder."
    );
    assert!(Sidecar::path_in(&files[0], Placement::Folder).exists());

    app.set_xmp_sidecars(true);
    app.invoke_xmp_changed();
    assert!(state.borrow().window.xmp_sidecars);

    // Escape closes it.
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_settings_open());
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn a_stale_copy_removed_is_not_counted_as_moved() {
    let dir = scratch("stale");
    let (a, b) = (dir.join("A.CR3"), dir.join("B.CR3"));
    let mut sidecar = Sidecar::default();
    // A under the folder only; B beside, with an older copy
    // under the folder too.
    sidecar.save_in(&a, Placement::Folder).unwrap();
    sidecar.save_in(&b, Placement::Beside).unwrap();
    let stale = Sidecar::path_in(&b, Placement::Folder);
    std::fs::write(&stale, "{}").unwrap();
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();

    let moved = move_sidecars(&[a.clone(), b.clone()], Placement::Beside);
    assert_eq!((moved.moved, moved.dropped), (1, 1));
    assert_eq!(
        moved_words(&moved, Placement::Beside),
        "Moved 1 sidecar beside their raws. Removed 1 older copy left in the other place."
    );
    assert!(Sidecar::path_in(&a, Placement::Beside).exists());
    assert!(!stale.exists());
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn no_sidecars_moves_nothing() {
    let dir = scratch("nosidecars");
    let files = shoot(&dir);
    let app = crate::testing::window(files.len());
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = false;
        st.window.placement = Placement::Folder;
    }
    app.invoke_settings_asked();
    assert!(!app.get_sidecars_written(), "the sheet is told");
    app.invoke_sidecars_move();
    assert!(Sidecar::path_in(&files[0], Placement::Beside).exists());
    assert_eq!(app.get_sidecars_elsewhere(), 2);
    crate::testing::remove_dir_retry(&dir);
}

/// The scale row: on the sheet at the percentage the window
/// started at; one typed is taken on Enter, held to 50 to 300, and
/// one that differs from the start's says it waits for the next
/// start; text that is not a number puts the field back. A factor
/// set by hand says it wins instead.
#[test]
fn the_scale_row_takes_any_percentage_for_the_next_start() {
    const NEXT: &str = "Applies at the next start.";
    const BY_HAND: &str = "SLINT_SCALE_FACTOR is set, and wins over this.";
    let app = crate::testing::window(0);
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    show_ui_scale(&app, 150);
    app.set_ui_scale_started(150);
    app.invoke_settings_asked();
    assert!(app.get_settings_open());
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::interface-heading"),
        1
    );
    assert_eq!(app.get_ui_scale(), "150");
    // The Language row says the same under it, always: the base.
    let base = crate::testing::count_labeled(&app, NEXT);
    assert_eq!(base, 1);
    let typed = |text: &str| {
        app.set_ui_scale(text.into());
        app.invoke_ui_scale_changed();
        (app.get_ui_scale().to_string(), app.get_ui_scale_percent())
    };
    assert_eq!(typed("137"), ("137".into(), 137));
    assert_eq!(crate::testing::count_labeled(&app, NEXT), base + 1);
    assert_eq!(typed("20"), ("50".into(), 50));
    assert_eq!(typed("400"), ("300".into(), 300));
    assert_eq!(typed("abc"), ("300".into(), 300), "put back");
    assert_eq!(typed(""), ("300".into(), 300), "put back");
    assert_eq!(crate::testing::count_labeled(&app, NEXT), base + 1);
    assert_eq!(typed("150"), ("150".into(), 150));
    assert_eq!(crate::testing::count_labeled(&app, NEXT), base);
    // Typed and not entered: taken as the sheet closes.
    app.set_ui_scale("112".into());
    app.set_settings_open(false);
    // Slint runs the changed handlers with the next event.
    crate::testing::press(&app, slint::platform::Key::Shift);
    assert_eq!(app.get_ui_scale_percent(), 112);
    app.invoke_settings_asked();
    assert_eq!(crate::testing::count_labeled(&app, NEXT), base + 1);
    // By hand: the variable's line, and not the next start's.
    app.set_ui_scale_by_hand(true);
    assert_eq!(typed("125"), ("125".into(), 125));
    assert_eq!(crate::testing::count_labeled(&app, BY_HAND), 1);
    assert_eq!(crate::testing::count_labeled(&app, NEXT), base);
}

/// Start in: the last folder until a folder is chosen; a click on
/// Chosen folder asks, a cancel leaves the row as it was, a folder
/// picked is shown, and Last folder goes back.
#[test]
fn the_start_row_takes_a_chosen_folder_or_the_last() {
    let app = crate::testing::window(0);
    let (state, _worker) = crate::testing::state_for(&app, Vec::new());
    app.set_settings_category("Library".into());
    app.invoke_settings_asked();
    assert_eq!(app.get_start_choice(), START_LAST);
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::start-in-row"),
        1
    );
    assert_eq!(crate::testing::count_labeled(&app, START_CHOSEN), 1);
    // Canceled: still the last folder.
    app.invoke_start_picked(START_CHOSEN.into());
    assert_eq!(app.get_start_choice(), START_LAST);
    // A folder picked, shown under the row.
    let dir = scratch("start");
    START_CHOSEN_IN_TEST.with(|c| *c.borrow_mut() = Some(Some(dir.clone())));
    app.invoke_start_picked(START_CHOSEN.into());
    assert_eq!(app.get_start_choice(), START_CHOSEN);
    let shown = crate::report::path_shown(&dir);
    assert_eq!(app.get_start_folder(), shown.as_str());
    assert_eq!(crate::testing::count_labeled(&app, &shown), 1);
    // Back to the last folder: the path goes.
    app.invoke_start_picked(START_LAST.into());
    assert_eq!(app.get_start_choice(), START_LAST);
    assert_eq!(crate::testing::count_labeled(&app, &shown), 0);
    crate::testing::remove_scratch(state, &dir);
}

/// Start in's view: the Loupe until the Library is clicked, which
/// the row then shows; a word the settings do not know is the
/// Loupe.
#[test]
fn the_start_view_row_takes_the_loupe_or_the_library() {
    let app = crate::testing::window(0);
    let (state, _worker) = crate::testing::state_for(&app, Vec::new());
    app.set_settings_category("Interface".into());
    app.invoke_settings_asked();
    let row = "SettingsSheet::start-view-row";
    assert_eq!(crate::testing::segments_shown(&app, row), 0);
    app.set_settings_category("Library".into());
    assert_eq!(app.get_start_view(), "Loupe");
    // The row's two.
    assert_eq!(crate::testing::segments_shown(&app, row), 2);
    app.set_start_view("Library".into());
    app.invoke_start_view_changed();
    assert_eq!(app.get_start_view(), "Library");
    app.set_start_view("Grid".into());
    app.invoke_start_view_changed();
    assert_eq!(app.get_start_view(), "Loupe");
    drop(state);
}

/// The filter bar's switch: off, the bar is gone from the grid's
/// header and from culling's section, and the filter set still
/// holds; on, it is back in both.
#[test]
fn the_filter_bar_switch_hides_the_bar_in_the_grid_and_in_culling() {
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, Vec::new());
    app.set_filter_on(true);
    const TIP: &str = "Sort: Name, A to Z";
    app.set_sort_menu(SortMenu {
        tip: TIP.into(),
        ..Default::default()
    });
    // "Save..." is the grid's bar's alone; the Sort button is the
    // bar's, and the header's top row's while the bar is away.
    app.set_grid_open(true);
    let grid_on = crate::testing::count_id(&app, "FilterBar::save-button");
    assert_eq!(grid_on, 1);
    assert_eq!(crate::testing::count_labeled(&app, TIP), 1);
    let (bar_sort, _) = crate::testing::labeled(&app, TIP);
    app.set_filter_bar(false);
    app.invoke_filter_bar_changed();
    assert_eq!(crate::testing::count_id(&app, "FilterBar::save-button"), 0);
    assert_eq!(crate::testing::count_labeled(&app, TIP), 1, "Sort stays");
    let (row_sort, _) = crate::testing::labeled(&app, TIP);
    assert!(row_sort.y < bar_sort.y, "on the top row: {row_sort:?}");
    assert!(app.get_filter_on(), "the filter still applies");
    app.set_filter_bar(true);
    app.invoke_filter_bar_changed();
    assert_eq!(crate::testing::count_id(&app, "FilterBar::save-button"), 1);
    assert_eq!(crate::testing::count_labeled(&app, TIP), 1);
    // Culling's section, with its "Show" over a stacked bar.
    app.set_grid_open(false);
    app.set_panel_tab(crate::panel::cull::CULL_TAB.into());
    let cull_on = crate::testing::count_id(&app, "FilterBar::stars-caption");
    assert!(cull_on >= 1, "the stacked bar's Stars");
    app.set_filter_bar(false);
    app.invoke_filter_bar_changed();
    assert_eq!(
        crate::testing::count_id(&app, "FilterBar::stars-caption"),
        cull_on - 1
    );
    assert!(app.get_filter_on(), "the filter still applies");
    // The filter key, with no bar to send it to, opens nothing.
    app.invoke_filter_asked();
    assert!(!app.get_grid_open());
    drop(state);
}

/// The theme row: a click puts its palette in force at once, the
/// window's `Theme` with it, and a name the settings do not know
/// goes back to Charcoal.
#[test]
fn the_theme_row_sets_the_palette() {
    let app = crate::testing::window(0);
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    app.invoke_settings_asked();
    assert_eq!(app.global::<crate::Theme>().get_name(), "Charcoal");
    let charcoal = app.global::<crate::Theme>().get_surface();
    let chosen = |t: &crate::Theme| (t.get_selection(), t.get_focus_ring());
    let charcoal_chosen = chosen(&app.global::<crate::Theme>());
    const ROW: &str = "SettingsSheet::theme-row";
    const THEMES: [&str; 3] = ["Charcoal", "Grey", "Light"];
    assert_eq!(crate::testing::count_id(&app, ROW), 1);
    crate::testing::click_segment(&app, ROW, &THEMES, "Light");
    assert_eq!(app.get_theme(), "Light");
    let theme = app.global::<crate::Theme>();
    assert_eq!(theme.get_name(), "Light");
    assert!(theme.get_light());
    assert_ne!(theme.get_surface(), charcoal);
    // A chosen row and the focus show on Light's grounds too.
    let light_chosen = chosen(&theme);
    assert_ne!(light_chosen.0, charcoal_chosen.0);
    assert_ne!(light_chosen.1, charcoal_chosen.1);
    crate::testing::click_segment(&app, ROW, &THEMES, "Grey");
    assert_eq!(app.global::<crate::Theme>().get_name(), "Grey");
    crate::testing::click_segment(&app, ROW, &THEMES, "Charcoal");
    assert_eq!(app.global::<crate::Theme>().get_surface(), charcoal);
    app.set_theme("Mauve".into());
    app.invoke_theme_changed();
    assert_eq!(app.global::<crate::Theme>().get_name(), "Charcoal");
}

/// The window bundles French: selected, the row lists English as
/// the source and shows French at its place, then selects English
/// again. (Translated text is proved by greycard-i18n-probe, on a
/// catalog of its own.)
#[test]
fn the_window_selects_a_bundled_language() {
    use slint::Model;
    let app = crate::testing::window(0);
    slint::select_bundled_translation("fr").expect("French is bundled");
    show_languages(&app, "fr");
    let names: Vec<String> = app.get_languages().iter().map(|s| s.to_string()).collect();
    assert_eq!(names.len(), crate::i18n::languages().len() + 2);
    assert_eq!(names[1], "English");
    let french = names
        .iter()
        .position(|n| n == "Français")
        .expect("French is listed");
    assert_eq!(app.get_language_index(), i32::try_from(french).unwrap());
    slint::select_bundled_translation("").expect("English is the source");
    start_language(&app, "en");
    assert_eq!(app.get_language_index(), 1);
    assert_eq!(crate::i18n::selected(), "");
}

/// The Language row lists System, English and each catalog's
/// language in its own name; a choice is the tag the setting keeps,
/// System empty, and an unknown tag shows as System.
#[test]
fn the_language_row_lists_the_catalogs() {
    use slint::Model;
    let app = crate::testing::window(0);
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    app.invoke_settings_asked();
    assert!(crate::testing::count_id(&app, "SettingsSheet::language-box") >= 1);
    let names: Vec<String> = app.get_languages().iter().map(|s| s.to_string()).collect();
    // The catalogs follow English in their tags' order, French among
    // them wherever the other languages put it.
    assert_eq!(names[..2], ["System", "English"]);
    let french = names
        .iter()
        .position(|n| n == "Français")
        .expect("French is listed");
    let french = i32::try_from(french).unwrap();
    let spanish = names
        .iter()
        .position(|n| n == "Español")
        .expect("Spanish is listed");
    let spanish = i32::try_from(spanish).unwrap();
    assert_eq!(app.get_language_index(), 0);
    assert_eq!(language_chosen(0), "");
    assert_eq!(language_chosen(1), "en");
    assert_eq!(language_chosen(french), "fr");
    assert_eq!(language_chosen(spanish), "es");
    assert_eq!(language_chosen(-1), "");
    assert_eq!(language_chosen(99), "");
    app.set_language_index(french);
    app.invoke_language_changed();
    assert_eq!(language_chosen(app.get_language_index()), "fr");
    show_languages(&app, "fr");
    assert_eq!(app.get_language_index(), french);
    show_languages(&app, "fr-CA");
    assert_eq!(app.get_language_index(), french);
    show_languages(&app, "es-MX");
    assert_eq!(app.get_language_index(), spanish);
    show_languages(&app, "en");
    assert_eq!(app.get_language_index(), 1);
    show_languages(&app, "xx");
    assert_eq!(app.get_language_index(), 0);
}

/// The scroll bar rows: a click puts the way and the size in force
/// at once, through the window's `ScrollBars`, and every bar's
/// thumb is as thick as the size says, at rest and under the
/// pointer; a name the settings do not know goes back to the first.
#[test]
fn the_scroll_bar_rows_set_every_bar() {
    use i_slint_backend_testing::ElementHandle;
    let app = crate::testing::window(40);
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    let settle = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1000));
        slint::platform::update_timers_and_animations();
    };
    // The thinnest of the thumbs that scroll something, the strip's
    // among them: its thickness across its bar.
    let thickness = || {
        settle();
        ElementHandle::find_by_element_id(&app, "ScrollBand::thumb")
            .map(|e| e.size())
            .filter(|s| s.width > 0.0 && s.height > 0.0)
            .map(|s| s.width.min(s.height))
            .reduce(f32::min)
            .expect("a bar that scrolls")
    };
    assert_eq!(thickness(), 2.0, "Thin at the start");
    app.invoke_settings_asked();
    let bars = || app.global::<crate::ScrollBars>();
    assert_eq!(
        (bars().get_mode(), bars().get_size()),
        ("Fade".into(), "Thin".into())
    );
    use crate::testing::click_segment;
    const MODES: [&str; 2] = ["Fade", "Stay"];
    const SIZES: [&str; 3] = ["Thin", "Medium", "Wide"];
    click_segment(&app, "SettingsSheet::scroll-bars-row", &MODES, "Stay");
    assert_eq!(app.get_scroll_bars(), "Stay");
    assert!(bars().get_stay());
    click_segment(&app, "SettingsSheet::scroll-bars-row", &MODES, "Fade");
    assert!(!bars().get_stay());
    for (size, rest, lit, band) in [
        ("Medium", 4.0, 10.0, 18.0),
        ("Wide", 6.0, 14.0, 22.0),
        ("Thin", 2.0, 6.0, 14.0),
    ] {
        click_segment(&app, "SettingsSheet::scroll-bar-size-row", &SIZES, size);
        assert_eq!(app.get_scroll_bar_size(), size);
        assert_eq!(bars().get_size(), size);
        assert_eq!(bars().get_rest(), rest, "{size}");
        assert_eq!(bars().get_lit(), lit, "{size}");
        assert_eq!(
            app.global::<crate::Theme>().get_scrollbar_band(),
            band,
            "{size}"
        );
        assert_eq!(thickness(), rest, "{size}");
    }
    app.set_scroll_bars("Always".into());
    app.invoke_scroll_bars_changed();
    assert_eq!(bars().get_mode(), "Fade");
    app.set_scroll_bar_size("Huge".into());
    app.invoke_scroll_bar_size_changed();
    assert_eq!(bars().get_size(), "Thin");
}

/// About: the version, and the folders the run keeps its files in,
/// the home folder short.
#[test]
fn the_about_category_says_the_version_and_the_folders() {
    let app = crate::testing::window(0);
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    app.set_settings_category("About".into());
    app.invoke_settings_asked();
    assert_eq!(app.get_about_version(), greycard_version::LABEL);
    assert_eq!(
        crate::testing::count_labeled(&app, greycard_version::LABEL),
        1
    );
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::check-now-button"),
        1
    );
    // Each folder greycard's own; on Windows the logs go in a
    // folder of their own under the library's, and the library and
    // the caches share `%LOCALAPPDATA%\greycard`, so a folder is
    // said once for each row that names it.
    let shown = about_dirs();
    let [_, library, caches, log] = &shown;
    for dir in [library, caches, log] {
        let logs = cfg!(windows) && dir.ends_with(r"greycard\logs");
        assert!(dir.ends_with("greycard") || logs, "{dir}");
        let rows = shown.iter().filter(|d| *d == dir).count();
        assert_eq!(crate::testing::count_labeled(&app, dir), rows, "{dir}");
    }
    if let Some(home) = dirs::home_dir() {
        assert!(!log.contains(&*home.to_string_lossy()), "{log}");
    }
}

#[test]
fn the_rows_words_name_the_places() {
    for p in [Placement::Beside, Placement::Folder] {
        assert_eq!(placement_named(placement_name(p)), p);
    }
    assert_eq!(placement_named("anything"), Placement::Beside);
}

#[test]
fn the_thumbnail_cache_is_said_in_megabytes() {
    use greycard_library::thumbs::{Split, Usage};
    let mb = 1024 * 1024;
    let known = |bytes, entries, cap| CacheShown::Known {
        usage: Split {
            thumbs: Usage { bytes, entries },
            previews: Usage::default(),
        },
        cap,
        preview_cap: 0,
    };
    assert_eq!(
        thumb_cache_words(known(5 * mb / 2, 312, 300 * mb)),
        "312 thumbnails kept, 2.5 MB of 300 MB."
    );
    assert_eq!(
        thumb_cache_words(known(0, 1, mb)),
        "1 thumbnail kept, 0.0 MB of 1 MB."
    );
    // Off with nothing kept, and off with a cache left from before,
    // which the sheet still offers to clear.
    assert!(thumb_cache_words(known(0, 0, 0)).starts_with("Off:"));
    assert_eq!(
        thumb_cache_words(known(3 * mb, 40, 0)),
        "Off. 40 pictures from before, 3.0 MB."
    );
    assert_eq!(known(3 * mb, 40, 0).entries(), 40);
    // The previews against their own cap, counted in what Clear
    // takes.
    let both = CacheShown::Known {
        usage: Split {
            thumbs: Usage {
                bytes: 5 * mb / 2,
                entries: 312,
            },
            previews: Usage {
                bytes: 18 * mb,
                entries: 40,
            },
        },
        cap: 300 * mb,
        preview_cap: 8192 * mb,
    };
    assert_eq!(
        thumb_cache_words(both),
        "312 thumbnails kept, 2.5 MB of 300 MB. 40 local previews, 18.0 MB of 8192 MB."
    );
    assert_eq!(both.entries(), 352);
    assert_eq!(CacheShown::Counting.entries(), 0);
    assert_eq!(thumb_cache_words(CacheShown::Counting), "Counting...");
    assert!(thumb_cache_words(CacheShown::Missing).starts_with("No cache"));
}

/// The network roots' minutes: shown as the window runs them, taken
/// when a whole number and changed (the timer's every with it), and
/// put back when not a number; zero is never.
#[test]
fn the_network_poll_field_takes_whole_minutes() {
    let app = crate::testing::window(0);
    let (state, _worker) = crate::testing::state_for(&app, Vec::new());
    state.borrow_mut().views.library.poll_every = crate::roots::poll_every(10);
    app.invoke_settings_asked();
    assert_eq!(app.get_network_poll_minutes(), "10");
    app.set_network_poll_minutes(" 25 ".into());
    app.invoke_network_poll_changed();
    assert_eq!(app.get_network_poll_minutes(), "25");
    assert_eq!(
        state.borrow().views.library.poll_every,
        Some(std::time::Duration::from_secs(25 * 60))
    );
    for bad in ["soon", "2.5", "-1", ""] {
        app.set_network_poll_minutes(bad.into());
        app.invoke_network_poll_changed();
        assert_eq!(app.get_network_poll_minutes(), "25", "{bad:?} put back");
        assert_eq!(
            state.borrow().views.library.poll_every,
            Some(std::time::Duration::from_secs(25 * 60))
        );
    }
    app.set_network_poll_minutes("0".into());
    app.invoke_network_poll_changed();
    assert_eq!(
        state.borrow().views.library.poll_every,
        None,
        "zero is never"
    );
    app.invoke_settings_asked();
    assert_eq!(app.get_network_poll_minutes(), "0");
    // Past what a clock holds: saturates, as the launch's does.
    app.set_network_poll_minutes(u64::MAX.to_string().into());
    app.invoke_network_poll_changed();
    assert!(state.borrow().views.library.poll_every.is_some());
}

/// The previews' cap field: shown as the cache has it, taken when a
/// whole number and changed (the cache's previews capped by it, the
/// thumbnails' cap left), and put back when not a number.
#[test]
fn the_previews_cap_field_takes_whole_megabytes() {
    let dir = scratch("preview-cap");
    let app = crate::testing::window(0);
    let (_state, worker) = crate::testing::state_for(&app, Vec::new());
    let mb = 1024 * 1024;
    let mut cache = greycard_library::Thumbs::at(dir.join("thumbs"), 300 * mb)
        .with_previews(crate::previews::SIZE, 8192 * mb);
    cache.seed_split(Default::default());
    worker.set_thumb_cache(Some(cache));
    app.invoke_settings_asked();
    assert_eq!(app.get_preview_cache_cap(), "8192");
    assert_eq!(app.get_thumb_cache_cap(), "300");
    let caps = || {
        let held = worker.thumb_cache();
        let held = held.lock().unwrap();
        let c = held.as_ref().unwrap();
        (c.cap() / mb, c.preview_cap() / mb)
    };
    let until = |want: (u64, u64)| {
        let start = std::time::Instant::now();
        while caps() != want {
            assert!(start.elapsed().as_secs() < 10, "{:?} for {want:?}", caps());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    app.set_preview_cache_cap(" 2048 ".into());
    app.invoke_preview_cache_cap_changed();
    assert_eq!(app.get_preview_cache_cap(), "2048");
    until((300, 2048));
    for bad in ["lots", "1.5", "-3", ""] {
        app.set_preview_cache_cap(bad.into());
        app.invoke_preview_cache_cap_changed();
        assert_eq!(app.get_preview_cache_cap(), "2048", "{bad:?} put back");
    }
    assert_eq!(caps(), (300, 2048));
    app.set_preview_cache_cap("0".into());
    app.invoke_preview_cache_cap_changed();
    until((300, 0));
    crate::testing::remove_dir_retry(&dir);
}

/// The caps the sheet shows are the ones last set, even while a
/// count or an eviction holds the cache: never the settings file,
/// which a held lock used to fall back to.
#[test]
fn the_caps_shown_are_live_while_the_cache_is_held() {
    let dir = scratch("caps-held");
    let app = crate::testing::window(0);
    let (_state, worker) = crate::testing::state_for(&app, Vec::new());
    let mb = 1024 * 1024;
    let mut cache = greycard_library::Thumbs::at(dir.join("thumbs"), 300 * mb)
        .with_previews(crate::previews::SIZE, 8192 * mb);
    cache.seed_split(Default::default());
    worker.set_thumb_cache(Some(cache));
    app.set_preview_cache_cap("2048".into());
    app.invoke_preview_cache_cap_changed();
    app.set_thumb_cache_cap("500".into());
    app.invoke_thumb_cache_cap_changed();
    let shared = worker.thumb_cache();
    let held = shared.lock().unwrap();
    app.invoke_settings_asked();
    assert_eq!(app.get_preview_cache_cap(), "2048");
    assert_eq!(app.get_thumb_cache_cap(), "500");
    app.set_preview_cache_cap("lots".into());
    app.invoke_preview_cache_cap_changed();
    assert_eq!(
        app.get_preview_cache_cap(),
        "2048",
        "put back to the live cap"
    );
    app.set_thumb_cache_cap("lots".into());
    app.invoke_thumb_cache_cap_changed();
    assert_eq!(app.get_thumb_cache_cap(), "500", "put back to the live cap");
    drop(held);
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn the_cap_is_whole_megabytes_and_never_overflows() {
    assert_eq!(cap_typed(" 300 "), Some(300));
    assert_eq!(cap_typed("0"), Some(0));
    assert_eq!(cap_typed("2.5"), None);
    assert_eq!(cap_typed("-1"), None);
    assert_eq!(cap_typed(""), None);
    assert_eq!(cap_bytes(300), 300 * 1024 * 1024);
    assert_eq!(cap_bytes(1 << 44), u64::MAX);
    assert_eq!(cap_bytes(u64::MAX), u64::MAX);
}

/// A window with a frame open on the Develop tab, the panel's
/// MONITOR and SOFT PROOF sections showing.
fn develop_window() -> (App, Rc<Shared>, Rc<Worker>) {
    let app = crate::testing::window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(0);
    (app, state, worker)
}

/// The Settings sheet's row `id`: what it shows as chosen.
fn row_shows(app: &App, id: &str) -> String {
    use i_slint_backend_testing::ElementHandle;
    crate::testing::count_labeled(app, id);
    let row = ElementHandle::find_by_element_id(app, &format!("SettingsSheet::{id}"))
        .next()
        .unwrap_or_else(|| panic!("{id} on the sheet"));
    let (at, size) = (row.absolute_position(), row.size());
    // The row's radio group, which says its current option.
    ElementHandle::find_by_element_id(app, "Segmented::fs")
        .find(|g| {
            let p = g.absolute_position();
            p.x >= at.x && p.x < at.x + size.width && p.y >= at.y && p.y < at.y + size.height
        })
        .unwrap_or_else(|| panic!("{id}'s radio group"))
        .accessible_value()
        .unwrap_or_default()
        .to_string()
}

/// A click on `option` in the sheet's row `id`.
fn click_row(app: &App, id: &str, option: &str) {
    use i_slint_backend_testing::ElementHandle;
    crate::testing::count_labeled(app, option);
    let row = ElementHandle::find_by_element_id(app, &format!("SettingsSheet::{id}"))
        .next()
        .unwrap_or_else(|| panic!("{id} on the sheet"));
    let (top, foot) = (
        row.absolute_position().y,
        row.absolute_position().y + row.size().height,
    );
    let (left, right) = (
        row.absolute_position().x,
        row.absolute_position().x + row.size().width,
    );
    let segment = ElementHandle::find_by_accessible_label(app, option)
        .find(|e| {
            let (at, size) = (e.absolute_position(), e.size());
            let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
            (left..right).contains(&x) && (top..foot).contains(&y)
        })
        .unwrap_or_else(|| panic!("{option:?} in {id}"));
    let (at, size) = (segment.absolute_position(), segment.size());
    crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// A click on the one `option` the panel shows, the sheet closed.
fn click_panel(app: &App, id: &str, options: &[&str], option: &str) {
    crate::testing::by_id(app, id);
    assert_eq!(crate::testing::count_id(app, id), 1, "{id}");
    crate::testing::click_segment(app, id, options, option);
}

fn open_editing(app: &App) {
    app.set_settings_category("Editing".into());
    app.invoke_settings_asked();
    assert!(app.get_settings_open());
}

/// The canvas, the monitor's profile, and the soft proof's profile
/// and intent: a row of the sheet changes the setting the panel's
/// control changes, and the panel's control shows on the row.
#[test]
fn the_editing_rows_are_the_panels_own_choices() {
    let (app, _state, _worker) = develop_window();
    assert_eq!(
        app.get_canvas_names()
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>(),
        crate::render::CANVAS_NAMES
    );
    // The swatches paint what the shader paints outside the frame.
    let colors = app.get_canvas_colors();
    assert_eq!(colors.row_count(), crate::render::CANVAS_NAMES.len());
    for (i, c) in colors.iter().enumerate() {
        let want = crate::render::canvas_rgb(i as i32).map(|v| (v * 255.0).round() as u8);
        assert_eq!([c.red(), c.green(), c.blue()], want, "canvas {i}");
    }
    let kept = || crate::panel::startup::remember(&app);
    // The sheet's rows, read back from the settings kept.
    open_editing(&app);
    for name in crate::render::CANVAS_NAMES.iter() {
        click_row(&app, "canvas-row", name);
        assert_eq!(kept().canvas_color, *name);
        assert_eq!(row_shows(&app, "canvas-row"), *name);
    }
    click_row(&app, "monitor-row", "Adobe RGB");
    assert_eq!(app.get_display_profile(), "Adobe RGB");
    assert_eq!(kept().display_profile, "Adobe RGB");
    click_row(&app, "proof-row", "Display P3");
    assert_eq!(kept().proof_profile, "Display P3");
    click_row(&app, "intent-row", "Relative");
    assert_eq!(kept().proof_intent, "Relative");
    // A file chosen shows its path under the row, as the panel's.
    app.set_display_file("/profiles/monitor.icc".into());
    click_row(&app, "monitor-row", "File");
    assert_eq!(kept().display_profile, "/profiles/monitor.icc");
    assert_eq!(
        crate::testing::count_labeled(&app, "/profiles/monitor.icc"),
        1
    );
    // The panel's controls, the sheet closed; the rows show them.
    app.set_settings_open(false);
    const PROFILES: [&str; 5] = ["System", "sRGB", "Adobe RGB", "Display P3", "File"];
    const PROOFS: [&str; 5] = ["sRGB", "Display P3", "Rec.2020", "Adobe RGB", "File"];
    const INTENTS: [&str; 2] = ["Perceptual", "Relative"];
    click_panel(&app, "MonitorSection::profile-row", &PROFILES, "System");
    assert_eq!(kept().display_profile, "System");
    click_panel(&app, "ProofSection::proof-row", &PROOFS, "Rec.2020");
    assert_eq!(kept().proof_profile, "Rec.2020");
    click_panel(&app, "ProofSection::intent-row", &INTENTS, "Perceptual");
    assert_eq!(kept().proof_intent, "Perceptual");
    // The canvas's first swatch, at the left of its row of swatches.
    let (at, size) = crate::testing::by_id(&app, "MonitorSection::canvas-swatches");
    crate::testing::click(&app, at.x + 4.0, at.y + size.height / 2.0);
    assert_eq!(kept().canvas_color, crate::render::CANVAS_NAMES[0]);
    open_editing(&app);
    assert_eq!(
        row_shows(&app, "canvas-row"),
        crate::render::CANVAS_NAMES[0]
    );
    assert_eq!(row_shows(&app, "monitor-row"), "System");
    assert_eq!(row_shows(&app, "proof-row"), "Rec.2020");
    assert_eq!(row_shows(&app, "intent-row"), "Perceptual");
}

/// A window whose settings file is a scratch one, and the timer
/// that keeps the panel's choices in it, as a run starts it.
fn kept_window() -> (
    App,
    Rc<Shared>,
    Rc<Worker>,
    slint::Timer,
    std::path::PathBuf,
) {
    let (app, state, worker, timer, _, dir) = kept_window_over(None);
    (app, state, worker, timer, dir)
}

/// `kept_window` over a file holding `text` as the run starts, with
/// the choices the close writes.
fn kept_window_over(
    text: Option<&str>,
) -> (
    App,
    Rc<Shared>,
    Rc<Worker>,
    slint::Timer,
    crate::panel::startup::Choices,
    std::path::PathBuf,
) {
    let dir = crate::testing::scratch_dir("choices-kept");
    let file = dir.join("settings.json");
    if let Some(text) = text {
        std::fs::write(&file, text).unwrap();
    }
    crate::settings::use_file(Some(file));
    let (_, set_aside) = crate::settings::Settings::load_at_start();
    let (app, state, worker) = develop_window();
    let choices = crate::panel::startup::Choices::new(&app, &state.borrow(), set_aside, false);
    let timer = crate::panel::startup::keep_choices(&app, &state, &choices);
    (app, state, worker, timer, choices, dir)
}

/// One tick of the keeping timer, and the file as it then stands.
fn file_after_a_tick() -> crate::settings::Settings {
    i_slint_backend_testing::mock_elapsed_time(crate::panel::startup::CHOICES_EVERY);
    crate::settings::Settings::load()
}

/// A Mac's Quit ends the process inside the event loop, before the
/// close's write: the canvas, the monitor, and the soft proof are
/// in the file as soon as they are chosen.
#[test]
fn the_editing_choices_are_in_the_file_before_any_close() {
    let (app, _state, _worker, _timer, dir) = kept_window();
    open_editing(&app);
    let canvas = crate::render::CANVAS_NAMES[1];
    click_row(&app, "canvas-row", canvas);
    click_row(&app, "monitor-row", "Adobe RGB");
    click_row(&app, "proof-row", "Display P3");
    click_row(&app, "intent-row", "Relative");
    let file = file_after_a_tick();
    assert_eq!(file.canvas_color, canvas);
    assert_eq!(file.display_profile, "Adobe RGB");
    assert_eq!(file.proof_profile, "Display P3");
    assert_eq!(file.proof_intent, "Relative");
    crate::settings::use_file(None);
    crate::testing::remove_dir_retry(&dir);
}

/// A field written to the file as it changes (the sidecar switch,
/// by `keep`) stays as written when the panel's choices are written
/// after it.
#[test]
fn a_kept_field_outlives_the_next_write_of_the_choices() {
    let (app, _state, _worker, _timer, dir) = kept_window();
    assert!(!file_after_a_tick().shared.xmp_sidecars);
    // As `keep` writes, which a test build skips.
    let mut file = crate::settings::Settings::load();
    file.shared.xmp_sidecars = true;
    file.save();
    open_editing(&app);
    click_row(&app, "monitor-row", "Adobe RGB");
    let file = file_after_a_tick();
    assert_eq!(file.display_profile, "Adobe RGB");
    assert!(file.shared.xmp_sidecars);
    crate::settings::use_file(None);
    crate::testing::remove_dir_retry(&dir);
}

/// A file that will not parse at the start is kept aside, and the
/// close writes nothing over it until a choice changes; then the
/// defaults and that change are written.
#[test]
fn a_broken_file_is_kept_aside_and_written_only_after_a_change() {
    let (app, state, _worker, _timer, choices, dir) = kept_window_over(Some("{ not json"));
    let (file, aside) = (dir.join("settings.json"), dir.join("settings.json.bad"));
    assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ not json");
    assert!(!file.exists());
    choices.write(&app, &state.borrow(), true);
    assert!(!file.exists(), "the close wrote the defaults");
    open_editing(&app);
    click_row(&app, "monitor-row", "Adobe RGB");
    assert_eq!(file_after_a_tick().display_profile, "Adobe RGB");
    assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ not json");
    crate::settings::use_file(None);
    crate::testing::remove_dir_retry(&dir);
}

/// Once anything writes a set-aside file again (the update check,
/// a Settings switch), the close writes the panel's fields over it,
/// no choice changed.
#[test]
fn the_close_writes_once_a_set_aside_file_is_written_again() {
    use crate::settings::Settings;
    let (app, state, _worker, _timer, choices, dir) = kept_window_over(Some("{ not json"));
    let file = dir.join("settings.json");
    // As the update check writes, the panel untouched.
    Settings::change(|s| s.shared.xmp_sidecars = true);
    // A panel field other than shown, by hand, so only the write
    // above is this run's.
    let shown = crate::panel::startup::remember(&app).display_monitor;
    let other = Settings {
        display_monitor: format!("{shown}-other"),
        ..Settings::load()
    };
    std::fs::write(&file, serde_json::to_string(&other).unwrap()).unwrap();
    choices.write(&app, &state.borrow(), true);
    let kept = Settings::load();
    assert_eq!(kept.display_monitor, shown);
    assert!(kept.shared.xmp_sidecars);
    crate::settings::use_file(None);
    crate::testing::remove_dir_retry(&dir);
}

/// A file broken mid-run (a hand edit with a typo) is kept aside by
/// the next write, which is the defaults and the change.
#[test]
fn a_file_broken_mid_run_is_kept_aside_by_the_next_change() {
    let (app, _state, _worker, _timer, dir) = kept_window();
    let file = dir.join("settings.json");
    std::fs::write(&file, "{ not json").unwrap();
    open_editing(&app);
    click_row(&app, "monitor-row", "Adobe RGB");
    assert_eq!(file_after_a_tick().display_profile, "Adobe RGB");
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.bad")).unwrap(),
        "{ not json"
    );
    crate::settings::use_file(None);
    crate::testing::remove_dir_retry(&dir);
}

/// A good file is written at the close as it always was, choice
/// changed or not.
#[test]
fn the_close_writes_over_a_good_file() {
    let canvas = crate::render::CANVAS_NAMES[1];
    let good = serde_json::to_string(&crate::settings::Settings {
        canvas_color: canvas.into(),
        ..Default::default()
    })
    .unwrap();
    let (app, state, _worker, _timer, choices, dir) = kept_window_over(Some(&good));
    let shown = crate::panel::startup::remember(&app).canvas_color;
    assert_ne!(shown, canvas);
    choices.write(&app, &state.borrow(), true);
    assert_eq!(crate::settings::Settings::load().canvas_color, shown);
    assert!(!dir.join("settings.json.bad").exists());
    crate::settings::use_file(None);
    crate::testing::remove_dir_retry(&dir);
}

/// The window is written at the close and not by the timer's
/// writes, which keep the file's; a window this build does not know
/// stays in the file.
#[test]
fn the_window_is_written_at_the_close_alone() {
    use crate::window_fit::kept;
    let other = r#"{"windows": {"second": {"width": 900, "height": 600}}}"#;
    let (app, state, _worker, _timer, choices, dir) = kept_window_over(Some(other));
    kept::start(false);
    let screen = kept::Screen {
        name: "DP-1".into(),
        position: (0, 0),
        size: (2560, 1440),
        scale: 1.0,
    };
    let normal = kept::normal_of((1300, 850), 1.0, Some((40, 30)), Some(&screen));
    kept::observe_as(kept::State::default(), normal);
    open_editing(&app);
    click_row(&app, "monitor-row", "Adobe RGB");
    let file = file_after_a_tick();
    assert_eq!(file.display_profile, "Adobe RGB");
    assert_eq!(file.windows.keys().collect::<Vec<_>>(), ["second"]);
    choices.write(&app, &state.borrow(), true);
    kept::stop();
    let file = crate::settings::Settings::load();
    assert_eq!(file.windows.keys().collect::<Vec<_>>(), ["main", "second"]);
    let main = &file.windows["main"];
    assert_eq!(
        (main.width, main.height, main.at, main.maximized),
        (1300.0, 850.0, Some([40.0, 30.0]), false)
    );
    assert_eq!(main.monitor.as_ref().unwrap().name, "DP-1");
    assert_eq!(file.windows["second"].width, 900.0);
    crate::settings::use_file(None);
    crate::testing::remove_dir_retry(&dir);
}

/// The export's format and color space: a row of the sheet is the
/// export sheet's choice, and the export sheet's shows on the row.
#[test]
fn the_export_rows_are_the_export_sheets_own() {
    let (app, _state, _worker) = develop_window();
    let kept = || crate::panel::startup::remember(&app);
    open_editing(&app);
    assert_eq!(
        crate::testing::count_id(&app, "SettingsSheet::export-heading"),
        1
    );
    click_row(&app, "format-row", "PNG");
    assert_eq!(kept().shared.export.format, "PNG");
    click_row(&app, "space-row", "Adobe RGB");
    assert_eq!(kept().shared.export.space, "Adobe RGB");
    app.set_settings_open(false);
    // The export sheet shows them, and its clicks come back.
    app.set_export_open(true);
    assert_eq!(app.get_export_format(), "PNG");
    crate::testing::click_segment(
        &app,
        "ExportSheet::format-row",
        &["JPEG", "PNG", "TIFF 16-bit", "WebP"],
        "TIFF 16-bit",
    );
    crate::testing::click_segment(
        &app,
        "ExportSheet::space-row",
        &["sRGB", "Display P3", "Rec.2020", "Adobe RGB"],
        "Rec.2020",
    );
    assert_eq!(kept().shared.export.format, "TIFF 16-bit");
    assert_eq!(kept().shared.export.space, "Rec.2020");
    app.set_export_open(false);
    open_editing(&app);
    assert_eq!(row_shows(&app, "format-row"), "TIFF 16-bit");
    assert_eq!(row_shows(&app, "space-row"), "Rec.2020");
}

/// The sheet's text field `id`: clicked into, emptied, `text`
/// typed, and Enter pressed when `enter`.
fn type_into(app: &App, id: &str, text: &str, enter: bool) {
    use i_slint_backend_testing::ElementHandle;
    let field = ElementHandle::find_by_element_id(app, &format!("SettingsSheet::{id}"))
        .next()
        .unwrap_or_else(|| panic!("{id} on the sheet"));
    let (at, size) = (field.absolute_position(), field.size());
    crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    // Select all and type over it.
    use slint::platform::{Key, WindowEvent};
    let control: slint::SharedString = Key::Control.into();
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: control.clone(),
    });
    crate::testing::press(app, "a");
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: control });
    crate::testing::press(app, Key::Backspace);
    crate::testing::type_text(app, text);
    if enter {
        crate::testing::press(app, Key::Return);
    }
}

/// The Author and Copyright fields, in Editing alone: typed and
/// entered, they are the next export's rights; the sheet opened
/// again shows them; one typed and not entered is taken as the
/// sheet closes; emptied, it is none.
#[test]
fn the_author_and_copyright_fields_set_the_exports_rights() {
    let (app, state, _worker) = develop_window();
    for category in crate::settings::CATEGORIES {
        app.set_settings_open(false);
        app.set_settings_category(category.into());
        app.invoke_settings_asked();
        let shown = usize::from(category == "Editing");
        assert_eq!(
            crate::testing::count_id(&app, "SettingsSheet::author-field"),
            shown,
            "{category}"
        );
        assert_eq!(
            crate::testing::count_id(&app, "SettingsSheet::copyright-field"),
            shown,
            "{category}"
        );
    }
    app.set_settings_open(false);
    open_editing(&app);
    assert_eq!(app.get_settings_author(), "");
    assert!(state.borrow().window.rights().is_empty());

    type_into(&app, "author-field", " Zoë Ångström; Jo Doe ", true);
    type_into(&app, "copyright-field", "© 2026 Zoë Ångström", true);
    {
        let st = state.borrow();
        assert_eq!(st.window.author, "Zoë Ångström; Jo Doe");
        assert_eq!(st.window.copyright, "© 2026 Zoë Ångström");
        let rights = st.window.rights();
        assert_eq!(rights.author.as_deref(), Some("Zoë Ångström; Jo Doe"));
        assert_eq!(rights.copyright.as_deref(), Some("© 2026 Zoë Ångström"));
    }
    assert_eq!(app.get_settings_author(), "Zoë Ångström; Jo Doe", "trimmed");

    // Closed and opened again: shown as kept.
    app.set_settings_open(false);
    crate::testing::press(&app, slint::platform::Key::Shift);
    app.set_settings_author("stale".into());
    open_editing(&app);
    assert_eq!(app.get_settings_author(), "Zoë Ångström; Jo Doe");
    assert_eq!(app.get_settings_copyright(), "© 2026 Zoë Ångström");

    // Emptied and not entered: Escape closes the sheet and takes it,
    // as none.
    type_into(&app, "copyright-field", "", false);
    assert_eq!(state.borrow().window.copyright, "© 2026 Zoë Ångström");
    crate::testing::press(&app, slint::platform::Key::Escape);
    crate::testing::press(&app, slint::platform::Key::Shift);
    assert!(!app.get_settings_open());
    {
        let st = state.borrow();
        assert_eq!(st.window.copyright, "");
        assert_eq!(st.window.rights().copyright, None);
        assert_eq!(st.window.author, "Zoë Ångström; Jo Doe");
    }
    open_editing(&app);
    assert_eq!(app.get_settings_copyright(), "");
    // Tab goes from Author on to Copyright.
    type_into(&app, "author-field", "Jo Doe", false);
    crate::testing::press(&app, slint::platform::Key::Tab);
    crate::testing::type_text(&app, "© Jo");
    assert_eq!(app.get_settings_copyright(), "© Jo");
}

/// A field on the sheet takes every key, the window's shortcuts
/// among them, and the arrows, Home, and End move in its text, not
/// the frame or the edit behind the sheet.
#[test]
fn a_sheets_field_takes_every_key() {
    use slint::platform::Key;
    let (app, state, _worker) = develop_window();
    open_editing(&app);
    let look = crate::panel::edit::read_look(&app);
    type_into(&app, "author-field", "gGeLx1[]/?c", false);
    for key in [
        Key::Home,
        Key::RightArrow,
        Key::Delete,
        Key::End,
        Key::LeftArrow,
    ] {
        crate::testing::press(&app, key);
    }
    crate::testing::press(&app, Key::Backspace);
    crate::testing::type_text(&app, "z");
    // On macOS Home is not a move in a line of text, so the Delete
    // after it finds the end and takes nothing.
    let typed = if cfg!(target_os = "macos") {
        "gGeLx1[]/zc"
    } else {
        "geLx1[]/zc"
    };
    assert_eq!(app.get_settings_author(), typed);
    assert!(app.get_settings_open());
    assert!(!app.get_grid_open() && !app.get_lightbox() && !app.get_cull_mode());
    assert_eq!(state.borrow().current, Some(0));
    assert_eq!(crate::panel::edit::read_look(&app), look);
}

/// What the fields keep in the file: the text, and an empty one
/// left out, as a file that never had it.
#[test]
fn the_author_and_copyright_kept_in_the_file_are_left_out_empty() {
    let dir = crate::testing::scratch_dir("rights-kept");
    let file = dir.join("settings.json");
    crate::settings::Settings::change_at(&file, |s| {
        keep_rights(s, "Zoë Ångström", "© 2026 Zoë Ångström")
    });
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"author\": \"Zoë Ångström\""), "{text}");
    let read = crate::settings::Settings::load_from(&file);
    assert_eq!(read.shared.copyright, "© 2026 Zoë Ångström");
    crate::settings::Settings::change_at(&file, |s| keep_rights(s, "", ""));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        !text.contains("author") && !text.contains("copyright"),
        "{text}"
    );
    crate::testing::remove_dir_retry(&dir);
}

/// The Editing category with the two fields filled, at 150 % and in
/// Spanish and French as in English: the card in the smallest window, the
/// fields inside the card and wide enough to type in.
#[test]
fn the_author_and_copyright_rows_fit_at_150_percent_in_french() {
    use i_slint_backend_testing::ElementHandle;
    for language in ["", "es", "fr"] {
        let (app, _state, _worker) = develop_window();
        slint::select_bundled_translation(language).expect("bundled");
        app.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
        app.window()
            .set_size(slint::LogicalSize::new(1024.0, 640.0));
        app.set_settings_author("Zoë Ångström; Jo Doe; A Third Author With A Long Name".into());
        app.set_settings_copyright("© 2026 Zoë Ångström, all rights reserved".into());
        open_editing(&app);
        crate::testing::settle(&app);
        let done = ElementHandle::find_by_element_id(&app, "SettingsSheet::done")
            .next()
            .expect("Done");
        let foot = done.absolute_position().y + done.size().height;
        assert!(foot <= 640.0, "{language}: Done at {foot}");
        // The pane scrolled to its foot, where the two rows are: a
        // row clipped out of view is not found.
        for _ in 0..10 {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(600.0, 320.0),
                    delta_x: 0.0,
                    delta_y: -200.0,
                });
        }
        crate::testing::settle(&app);
        for id in ["author-field", "copyright-field"] {
            crate::testing::count_labeled(&app, id);
            let field = ElementHandle::find_by_element_id(&app, &format!("SettingsSheet::{id}"))
                .next()
                .unwrap_or_else(|| panic!("{language}: {id} on the sheet"));
            let (at, size) = (field.absolute_position(), field.size());
            assert!(
                at.x >= 0.0 && at.x + size.width <= 1024.0,
                "{language} {id}: {at:?} {size:?}"
            );
            assert!(size.width >= 120.0, "{language} {id}: {size:?}");
        }
        slint::select_bundled_translation("").expect("English is the source");
    }
}

/// In the smallest window the fit allows, each category keeps the
/// card in the window and Done in reach.
#[test]
fn every_category_fits_the_smallest_window() {
    use i_slint_backend_testing::ElementHandle;
    let (app, _state, _worker) = develop_window();
    app.window()
        .set_size(slint::LogicalSize::new(1024.0, 640.0));
    for category in crate::settings::CATEGORIES {
        app.set_settings_open(false);
        app.set_settings_category(category.into());
        app.invoke_settings_asked();
        assert!(
            crate::testing::count_id(&app, "SettingsSheet::done") > 0,
            "{category}"
        );
        for done in ElementHandle::find_by_element_id(&app, "SettingsSheet::done") {
            let at = done.absolute_position();
            let foot = at.y + done.size().height;
            assert!(at.y >= 0.0 && foot <= 640.0, "{category}: Done at {foot}");
        }
    }
}

/// The grid at 1500 by 950 over 120 frames, open on the first,
/// with the window's callbacks.
fn grid_window() -> (App, std::rc::Rc<crate::Shared>) {
    let app = crate::testing::window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(120));
    app.invoke_select(0);
    crate::testing::press(&app, "g");
    settle(&app);
    assert!(app.get_grid_open());
    (app, state)
}

fn settle(app: &App) {
    slint::platform::update_timers_and_animations();
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
    slint::platform::update_timers_and_animations();
}

/// A notch of the wheel over the sheet, with Ctrl held or not.
fn wheel(app: &App, by: f32, ctrl: bool) {
    use slint::platform::{Key, WindowEvent};
    let control: slint::SharedString = Key::Control.into();
    if ctrl {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: control.clone(),
        });
    }
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(800.0, 500.0),
        delta_x: 0.0,
        delta_y: by,
    });
    if ctrl {
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: control });
    }
    settle(app);
}

/// The Cell size row and the wheel set the one cell: a size typed
/// is taken, held to the grid's range, and a word put back; a
/// Ctrl and wheel after it shows in the row.
#[test]
fn the_cell_size_row_and_the_wheel_agree() {
    let (app, _state) = grid_window();
    app.invoke_settings_asked();
    settle(&app);
    assert_eq!(app.get_grid_cell_typed(), "176");
    for (typed, cell) in [
        (" 300 ", 300.0),
        ("4000", 512.0),
        ("10", 96.0),
        ("200.4", 200.0),
    ] {
        app.set_grid_cell_typed(typed.into());
        app.invoke_grid_cell_typed_changed();
        assert_eq!(app.get_grid_cell(), cell, "{typed:?}");
        assert_eq!(app.get_grid_cell_typed(), format!("{cell}"), "{typed:?}");
    }
    for bad in ["big", "", "inf", "NaN"] {
        app.set_grid_cell_typed(bad.into());
        app.invoke_grid_cell_typed_changed();
        assert_eq!(app.get_grid_cell(), 200.0, "{bad:?} leaves the cell");
        assert_eq!(app.get_grid_cell_typed(), "200", "{bad:?} put back");
    }
    // Typed and not entered: the sheet's closing takes it.
    app.set_grid_cell_typed("360".into());
    app.set_settings_open(false);
    // Slint runs the changed handlers with the next event.
    crate::testing::press(&app, slint::platform::Key::Shift);
    settle(&app);
    assert_eq!(app.get_grid_cell(), 360.0);
    // The wheel steps from there, and the row follows.
    wheel(&app, 60.0, true);
    assert_eq!(app.get_grid_cell(), 512.0);
    assert_eq!(app.get_grid_cell_typed(), "512");
    wheel(&app, -60.0, true);
    assert_eq!(app.get_grid_cell(), 360.0);
    app.invoke_settings_asked();
    assert_eq!(app.get_grid_cell_typed(), "360");
}

/// The wheel's switch off: Ctrl and the wheel scroll the grid as
/// the wheel alone does, the cell left as it was; plus and minus
/// still size it.
#[test]
fn the_wheel_switch_off_only_scrolls() {
    let (app, _state) = grid_window();
    assert!(app.get_wheel_resizes(), "on by default");
    let cell = app.get_grid_cell();
    app.set_wheel_resizes(false);
    app.invoke_wheel_resizes_changed();
    assert_eq!(app.get_grid_scroll(), 0.0);
    wheel(&app, -60.0, true);
    assert_eq!(app.get_grid_cell(), cell);
    let scrolled = app.get_grid_scroll();
    assert!(scrolled > 0.0, "Ctrl and the wheel scroll");
    wheel(&app, -60.0, false);
    assert!(app.get_grid_scroll() > scrolled, "the wheel alone too");
    assert_eq!(app.get_grid_cell(), cell);
    crate::testing::press(&app, "+");
    settle(&app);
    assert_eq!(
        app.get_grid_cell(),
        grid::zoom(cell, 1),
        "the keys still size"
    );
    // Back on, Ctrl and the wheel size the cells again.
    app.set_wheel_resizes(true);
    app.invoke_wheel_resizes_changed();
    wheel(&app, -60.0, true);
    assert_eq!(app.get_grid_cell(), cell);
}
