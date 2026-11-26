use slint::Model;

use super::*;

/// The pool's making on the status line: said over an empty line
/// and over its own last word, taken off at its end while the line
/// still says it, and never over another's.
#[test]
fn the_making_is_said_over_its_own_word_and_never_over_another() {
    let app = crate::testing::window(0);
    let mut st = State::empty(Vec::new(), &app);
    use crate::thumbpool::Making;
    app.set_status("".into());
    app.set_grid_open(true);
    show_making(&app, &mut st, Making::Uncounted);
    assert_eq!(app.get_status(), "making pictures...");
    show_making(&app, &mut st, Making::Left(12));
    assert_eq!(app.get_status(), "making 12 pictures...");
    show_making(&app, &mut st, Making::Left(1));
    assert_eq!(app.get_status(), "making 1 picture...");
    show_making(&app, &mut st, Making::Over);
    assert_eq!(app.get_status(), "");
    show_making(&app, &mut st, Making::Left(5));
    app.set_status("saved the filter as Dusk".into());
    show_making(&app, &mut st, Making::Left(4));
    assert_eq!(app.get_status(), "saved the filter as Dusk");
    show_making(&app, &mut st, Making::Over);
    assert_eq!(app.get_status(), "saved the filter as Dusk");
}

/// What the edit went without is still said once a develop lands:
/// the engine's denoise standing in for a model not fetched, and a
/// fill left as it was without a GPU, joined.
#[test]
fn what_the_edit_went_without_reaches_the_status_line() {
    use crate::testing::{folder, land_develop, state_for, window};
    use crate::worker::FillReport;
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    // Declined: the sheet that offers the model stays down.
    state
        .borrow_mut()
        .sheets
        .declined
        .push(greycard_ai::DENOISE_BALANCED.id);
    let status = land_develop(
        &app,
        &state,
        LearnedReport::Missing(&greycard_ai::DENOISE_BALANCED),
        FillReport {
            no_gpu: vec!["Fill 2".into()],
            ..FillReport::default()
        },
    );
    assert_eq!(
        status,
        "the engine's denoise until the model is fetched, \
         Fill 2 left as it was: the remove needs a GPU"
    );
    // Nothing to say on the next: an empty line.
    let status = land_develop(&app, &state, LearnedReport::Off, FillReport::default());
    assert_eq!(status, "");
}

/// The learned denoiser on record as off this card rests its tiers
/// on a raw, with the section's hint; off the record, nothing rests; a
/// picture's section is left as it says.
#[test]
fn a_card_the_denoiser_lost_rests_its_tiers() {
    use crate::testing::{folder, state_for, window};
    assert_eq!(
        learned_unavailable(true),
        "learned: not on this graphics card"
    );
    assert_eq!(learned_unavailable(false), "");
    let app = window(1);
    let (_state, _worker) = state_for(&app, folder(1));
    app.set_raw_input(true);
    crate::ai::set_test_off_card(Some(true));
    show_learned_unavailable(&app);
    assert_eq!(
        app.get_learned_unavailable(),
        "learned: not on this graphics card"
    );
    crate::ai::set_test_off_card(Some(false));
    show_learned_unavailable(&app);
    assert_eq!(app.get_learned_unavailable(), "");
    app.set_raw_input(false);
    crate::ai::set_test_off_card(Some(true));
    show_learned_unavailable(&app);
    assert_eq!(app.get_learned_unavailable(), "");
    crate::ai::set_test_off_card(None);
}

/// The hint asks only what the process knows: a record that holds a
/// denoiser, not yet read, is not read on the window's thread, which
/// would load ONNX Runtime and name the adapter there.
#[test]
fn the_hint_never_reads_the_record_on_the_windows_thread() {
    use crate::testing::{folder, state_for, window};
    let app = window(1);
    let (_state, _worker) = state_for(&app, folder(1));
    let root = std::env::temp_dir().join(format!(
        "greycard-hint-no-read-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    crate::testing::remove_dir_retry(&root);
    let (here, build) = ("Test GPU|Vulkan|1.2.3", "b");
    // A tier on record for this card, written as a loss writes it,
    // through a held run of a model that is not one (so the
    // process's flag, which other tests read, stays as it is), then
    // named a tier in the file.
    greycard_ai::runtime::Held::new().put_off(&[greycard_ai::runtime::OnCard::new(
        &root,
        "hint-no-read",
        "h",
        here,
        build,
    )]);
    let path = greycard_ai::runtime::off_card_path(&root);
    let record = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        record.replace("hint-no-read", greycard_ai::DENOISE_FAST.id),
    )
    .unwrap();
    greycard_ai::runtime::set_test_here(Some((here, build)));
    greycard_ai::runtime::set_test_store(Some(&root));
    greycard_ai::runtime::read_store_again_for_tests();
    crate::ai::set_test_off_card(None);
    app.set_raw_input(true);
    let before = greycard_ai::runtime::here_asked_on_this_thread();
    show_learned_unavailable(&app);
    assert_eq!(
        greycard_ai::runtime::here_asked_on_this_thread(),
        before,
        "the window's thread asked for the card"
    );
    greycard_ai::runtime::set_test_store(None);
    greycard_ai::runtime::set_test_here(None);
    crate::testing::remove_dir_retry(&root);
}

/// Try again sits beside the hint, only where the card is why; a
/// press takes this card's entry off the record (another card's
/// stays), lets the tiers back on, and hint and button go.
#[test]
fn try_again_takes_this_cards_record_off_and_brings_the_tiers_back() {
    use crate::testing::{folder, state_for, window};
    use i_slint_backend_testing::ElementHandle;
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    let root = std::env::temp_dir().join(format!("greycard-try-again-{}", std::process::id()));
    crate::testing::remove_dir_retry(&root);
    state.borrow_mut().develop.store = Some(greycard_ai::Store::at(&root));
    let (here, build) = ("Test GPU|Vulkan|1.2.3", "b");
    greycard_ai::runtime::set_test_here(Some((here, build)));
    let entry = |adapter| {
        greycard_ai::runtime::OnCard::new(
            &root,
            // Not a denoiser's id: the process's flag, which other
            // tests' workers read, is left alone; this thread's is set.
            "try-again-test",
            "h",
            adapter,
            build,
        )
    };
    let held = greycard_ai::runtime::Held::new();
    held.put_off(&[entry(here), entry("Other GPU|Vulkan|9.9.9")]);
    // The Develop tab, tall enough that the panel, which builds
    // only what it shows, builds the Noise section.
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 6000.0));
    app.set_panel_tab("Develop".into());
    app.set_raw_input(true);
    app.set_collapsed_noise(false);
    crate::ai::set_test_off_card(Some(true));
    show_learned_unavailable(&app);
    crate::testing::settle(&app);
    assert_eq!(
        app.get_learned_unavailable(),
        "learned: not on this graphics card"
    );
    assert!(app.get_learned_retry());
    let button =
        || ElementHandle::find_by_element_id(&app, "NoiseSection::learned-retry-button").next();
    assert!(!state.borrow().develop.debounce.running());
    button()
        .expect("Try again beside the hint")
        .invoke_accessible_default_action();
    // The frame is asked to develop again, as a change on the panel
    // asks it.
    assert!(state.borrow().develop.debounce.running(), "a develop asked");
    crate::testing::settle(&app);
    assert_eq!(app.get_learned_unavailable(), "");
    assert!(!app.get_learned_retry());
    assert!(button().is_none(), "the button goes with the hint");
    let record = std::fs::read_to_string(greycard_ai::runtime::off_card_path(&root)).unwrap();
    assert!(!record.contains(here), "{record}");
    assert!(record.contains("Other GPU"), "{record}");
    // An X-Trans raw open on a recorded card rests as a Bayer one
    // does, with the card's hint and Try again; off the record, its
    // tiers are on: it has a network for each.
    let xtrans = greycard_core::RawFrame {
        width: 6,
        height: 6,
        layout: greycard_core::raw::SensorLayout::Cfa(greycard_core::raw::CfaPattern::xtrans()),
        samples: greycard_core::raw::Samples::U16(vec![500; 36]),
        ..crate::panel::viewport::tests::daylight_frame()
    };
    let profile = greycard_core::color::profile_from_frame(&xtrans).unwrap();
    state.borrow_mut().develop.frame = Some((std::sync::Arc::new(xtrans), Box::new(profile)));
    crate::ai::set_test_off_card(Some(true));
    show_learned_unavailable(&app);
    crate::testing::settle(&app);
    assert_eq!(
        app.get_learned_unavailable(),
        "learned: not on this graphics card"
    );
    assert!(app.get_learned_retry());
    assert!(button().is_some(), "Try again on X-Trans too");
    crate::ai::set_test_off_card(Some(false));
    show_learned_unavailable(&app);
    crate::testing::settle(&app);
    assert_eq!(app.get_learned_unavailable(), "");
    assert!(!app.get_learned_retry());
    state.borrow_mut().develop.frame = None;
    crate::ai::set_test_off_card(None);
    greycard_ai::runtime::set_test_here(None);
    crate::testing::remove_dir_retry(&root);
}

/// A removal waiting on its model, on a card under the remove's
/// need, is not offered the download: the card could never run it.
#[test]
fn a_card_under_the_need_is_not_offered_the_remove() {
    use crate::testing::{folder, land_develop, state_for, window};
    use crate::worker::FillReport;
    crate::ai::set_test_webgpu(Some(true));
    crate::ai::set_test_card(Some(Some(4 << 30)));
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    land_develop(
        &app,
        &state,
        LearnedReport::Off,
        FillReport {
            missing: vec![("Fill 1".into(), &greycard_ai::REMOVE)],
            ..FillReport::default()
        },
    );
    assert_eq!(state.borrow().sheets.fetch, None);
    assert!(!app.get_fetch_open());
    crate::ai::set_test_card(None);
    crate::ai::set_test_webgpu(None);
}

/// Removals the card was short of right now are said once, with the
/// need and what was free, in place of the model's failure; another
/// failure is still said as one.
#[test]
fn a_card_short_of_a_fill_now_is_said_once_with_its_figures() {
    use crate::testing::{folder, land_develop, state_for, window};
    use crate::worker::FillReport;
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    let gib = 1u64 << 30;
    let status = land_develop(
        &app,
        &state,
        LearnedReport::Off,
        FillReport {
            failed: vec![
                ("Fill 1".into(), "the card is short".into()),
                ("Fill 2".into(), "the card is short".into()),
                ("Fill 3".into(), "bad input".into()),
            ],
            short: vec![
                ("Fill 1".into(), 63 * gib / 10, 21 * gib / 10),
                ("Fill 2".into(), 63 * gib / 10, 21 * gib / 10),
            ],
            ..FillReport::default()
        },
    );
    assert_eq!(
        status,
        "Needs 6.3 GiB VRAM, 2.1 available. Use Patch. \
         Fill 3 left as it was: the fill model failed (bad input)"
    );
}

/// An action's own words left for the develop come first, and what
/// the edit went without after them.
#[test]
fn an_actions_words_and_a_warning_are_joined() {
    use crate::testing::{folder, land_develop, state_for, window};
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    {
        let mut st = state.borrow_mut();
        st.develop.status_after_develop =
            Some((st.develop.generation, "Portra 400 onto 2 frames".into()));
    }
    let status = land_develop(
        &app,
        &state,
        LearnedReport::Unavailable("no version for it"),
        crate::worker::FillReport::default(),
    );
    assert_eq!(
        status,
        "Portra 400 onto 2 frames; no denoise: no version for it"
    );
}

/// In Develop the making stays off the picture's empty line, and
/// its own word left from the grid comes off.
#[test]
fn the_making_is_not_said_over_the_picture_in_develop() {
    let app = crate::testing::window(0);
    let mut st = State::empty(Vec::new(), &app);
    use crate::thumbpool::Making;
    app.set_status("".into());
    app.set_grid_open(true);
    show_making(&app, &mut st, Making::Left(12));
    assert_eq!(app.get_status(), "making 12 pictures...");
    app.set_grid_open(false);
    show_making(&app, &mut st, Making::Left(11));
    assert_eq!(app.get_status(), "");
    show_making(&app, &mut st, Making::Left(10));
    assert_eq!(app.get_status(), "");
    app.set_grid_open(true);
    show_making(&app, &mut st, Making::Left(9));
    assert_eq!(app.get_status(), "making 9 pictures...");
}

/// The fill model's word on the status line: the patch alone when
/// it is the one fill ahead, with its place in the count when more
/// are; a word from a develop since left behind is not said.
#[test]
fn the_fill_models_word_counts_what_is_ahead() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let generation = state.borrow().develop.generation;
    crate::STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    let filling = |name: &str, made: usize, of: usize, generation: u64| {
        deliver(
            &app,
            Outcome::Filling {
                generation,
                name: name.into(),
                made,
                of,
            },
        );
    };
    filling("Fill 3", 0, 1, generation);
    assert_eq!(
        app.get_status(),
        "developing... the fill model is at work on Fill 3"
    );
    filling("Fill 5", 3, 15, generation);
    assert_eq!(
        app.get_status(),
        "developing... the fill model is at work on Fill 5, 4 of 15"
    );
    filling("Fill 9", 0, 2, generation + 1);
    assert_eq!(
        app.get_status(),
        "developing... the fill model is at work on Fill 5, 4 of 15"
    );
    crate::STATE.with(|s| *s.borrow_mut() = None);
}

#[test]
fn the_sheet_picks_saves_marks_and_deletes_presets() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    // A batch state: nothing here may write the user's settings.
    state.borrow_mut().window.batch = true;
    let web = Sheet {
        size: "2048".into(),
        quality: 85.0,
        mark: sheet::MARK_TEXT.into(),
        mark_text: "© me".into(),
        ..Sheet::default()
    };
    state.borrow_mut().work.export_presets = vec![ExportPreset {
        name: "Web".into(),
        sheet: web.clone(),
    }];
    show_presets_picker(&app, &state.borrow().work.export_presets, None);
    assert_eq!(app.get_export_preset(), NO_PRESET);
    assert_eq!(app.get_export_presets().row_count(), 2);
    // One an older file named "none" is not listed beside None.
    let mut with_none = state.borrow().work.export_presets.clone();
    with_none.push(ExportPreset {
        name: "none".into(),
        sheet: Sheet::default(),
    });
    show_presets_picker(&app, &with_none, Some("none"));
    let listed: Vec<String> = app
        .get_export_presets()
        .iter()
        .map(|n| n.to_string())
        .collect();
    assert_eq!(listed, vec![NO_PRESET.to_string(), "Web".to_string()]);
    assert_eq!(app.get_export_preset(), NO_PRESET);
    show_presets_picker(&app, &state.borrow().work.export_presets, None);
    assert!(!app.get_export_preset_edited());

    // Chosen: the sheet is the preset's, and not edited.
    app.invoke_export_preset_chosen("Web".into());
    assert_eq!(app.get_export_preset(), "Web");
    assert!(read_sheet(&app).same(&web));
    assert_eq!(read_export_settings(&app).long_edge, Some(2048));
    assert!(!app.get_export_preset_edited());
    // A field moved: edited; moved back: not.
    app.set_export_mark_position("Top left".into());
    app.invoke_export_sheet_changed();
    assert!(app.get_export_preset_edited());
    app.set_export_mark_position(web.mark_position.as_str().into());
    app.invoke_export_sheet_changed();
    assert!(!app.get_export_preset_edited());

    // Saved as a new name: a second preset, chosen, not edited.
    app.set_export_quality(70.0);
    app.invoke_export_sheet_changed();
    assert!(app.get_export_preset_edited());
    app.invoke_export_preset_saved(" Small ".into());
    assert_eq!(app.get_export_preset(), "Small");
    assert!(!app.get_export_preset_edited());
    assert_eq!(state.borrow().work.export_presets.len(), 2);
    assert_eq!(state.borrow().work.export_presets[1].sheet.quality, 70.0);
    // "None" is the picker's, not a name.
    app.invoke_export_preset_saved(NO_PRESET.into());
    assert_eq!(state.borrow().work.export_presets.len(), 2);

    // Deleted: gone, nothing chosen, the sheet as it was.
    app.invoke_export_preset_deleted();
    assert_eq!(state.borrow().work.export_presets.len(), 1);
    assert_eq!(app.get_export_preset(), NO_PRESET);
    assert_eq!(app.get_export_quality(), 70.0);
    // Nothing was written: the state has no settings file.
    assert!(state.borrow().window.settings_file.is_none());
}

/// In French the Preset picker says no preset in French while its key
/// stays `NO_PRESET`; a preset named that very word is listed as
/// itself and chosen by its place, never taken for none.
#[test]
fn the_preset_picker_says_none_in_the_window_language() {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::Key;
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    state.borrow_mut().window.batch = true;
    let _french = crate::testing::in_language("fr");
    let word = crate::i18n::text(NO_PRESET);
    assert_ne!(word, NO_PRESET, "French has a word for no preset");
    let named = Sheet {
        quality: 61.0,
        ..Sheet::default()
    };
    state.borrow_mut().work.export_presets = vec![
        ExportPreset {
            name: word.clone(),
            sheet: named.clone(),
        },
        ExportPreset {
            name: "Web".into(),
            sheet: Sheet::default(),
        },
    ];
    show_presets_picker(&app, &state.borrow().work.export_presets, None);
    app.set_export_open(true);
    crate::testing::settle(&app);
    let shown = || {
        let picker = ElementHandle::find_by_element_id(&app, "ExportSheet::presets")
            .next()
            .expect("the picker");
        picker.accessible_value().unwrap_or_default().to_string()
    };
    assert_eq!(shown(), word, "no preset, in French");
    assert_eq!(app.get_export_preset(), NO_PRESET);
    // The keys move down the list onto the preset of that name: it is
    // chosen, by its name, and fills the sheet.
    let (at, size) = crate::testing::by_id(&app, "ExportSheet::presets");
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    crate::testing::press(&app, Key::DownArrow);
    crate::testing::press(&app, Key::Return);
    crate::testing::settle(&app);
    assert_eq!(app.get_export_preset(), word.as_str());
    assert!(read_sheet(&app).same(&named), "the sheet is the preset's");
    assert_eq!(chosen_preset(&app).as_deref(), Some(word.as_str()));
    assert_eq!(shown(), word);
    // And up again: no preset, by its key.
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    crate::testing::press(&app, Key::UpArrow);
    crate::testing::press(&app, Key::Return);
    crate::testing::settle(&app);
    assert_eq!(app.get_export_preset(), NO_PRESET);
    assert_eq!(chosen_preset(&app), None);
    assert_eq!(shown(), word);
}

/// The sheet opened on a preset past the first shows that preset: the
/// place the window sets wins over the picker's own, before the sheet
/// is up and while it is.
#[test]
fn the_sheet_opens_on_the_preset_chosen() {
    use i_slint_backend_testing::ElementHandle;
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    state.borrow_mut().window.batch = true;
    let preset = |name: &str| ExportPreset {
        name: name.into(),
        sheet: Sheet::default(),
    };
    let presets = vec![preset("Web"), preset("Print"), preset("Archive")];
    show_presets_picker(&app, &presets, Some("Archive"));
    assert_eq!(app.get_export_preset_index(), 3);
    app.set_export_open(true);
    crate::testing::settle(&app);
    let shown = || {
        ElementHandle::find_by_element_id(&app, "ExportSheet::presets")
            .next()
            .expect("the picker")
            .accessible_value()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(shown(), "Archive");
    assert_eq!(app.get_export_preset(), "Archive");
    // Chosen anew while the sheet is up, and the list changed under it.
    show_presets_picker(&app, &presets[..2], Some("Print"));
    crate::testing::settle(&app);
    assert_eq!(shown(), "Print");
    assert_eq!(app.get_export_preset_index(), 2);
}

/// A name that is the picker's own, typed in French with a space
/// before it, is refused in the words the picker shows.
#[test]
fn a_name_for_none_is_refused_in_the_window_language() {
    use slint::platform::Key;
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    state.borrow_mut().window.batch = true;
    let _french = crate::testing::in_language("fr");
    let word = crate::i18n::text(NO_PRESET);
    assert_ne!(word, NO_PRESET, "French has a word for no preset");
    show_presets_picker(&app, &[], None);
    app.set_export_open(true);
    crate::testing::settle(&app);
    crate::testing::click_id(&app, "ExportSheet::save-as-button");
    crate::testing::settle(&app);
    crate::testing::type_text(&app, " none");
    crate::testing::press(&app, Key::Return);
    crate::testing::settle(&app);
    assert!(
        state.borrow().work.export_presets.is_empty(),
        "nothing saved"
    );
    let status = app.get_status().to_string();
    assert!(
        status.contains(&word) && !status.contains(NO_PRESET),
        "{status}"
    );
}

/// A preset's Delete takes the focus and goes disabled as the
/// preset it deleted leaves None chosen: Return then is the dead
/// button's, taken and doing nothing, not the sheet's Export.
#[test]
fn return_on_a_dead_delete_does_not_export() {
    use slint::platform::Key;
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    state.borrow_mut().window.batch = true;
    state.borrow_mut().work.export_presets = vec![ExportPreset {
        name: "Web".into(),
        sheet: Sheet::default(),
    }];
    show_presets_picker(&app, &state.borrow().work.export_presets, None);
    app.invoke_export_preset_chosen("Web".into());
    let exports = Rc::new(std::cell::Cell::new(0));
    let counted = exports.clone();
    app.on_export(move || counted.set(counted.get() + 1));
    app.set_export_open(true);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        crate::testing::count_id(&app, "ExportSheet::delete-button"),
        1
    );
    let (at, size) = crate::testing::by_id(&app, "ExportSheet::delete-button");
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_export_preset(), NO_PRESET);
    assert!(state.borrow().work.export_presets.is_empty());
    crate::testing::press(&app, Key::Return);
    crate::testing::press(&app, " ");
    slint::platform::update_timers_and_animations();
    assert_eq!(exports.get(), 0, "no export");
    assert!(app.get_export_open(), "the sheet still up");
}

#[test]
fn a_pick_a_save_and_a_delete_are_written_at_once() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let dir = std::env::temp_dir().join(format!("greycard-keep-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    let file = dir.join("greycard").join("settings.json");
    // What else the file holds is kept.
    settings::Settings {
        scope: "Parade".into(),
        ..settings::Settings::default()
    }
    .save_to(&file);
    state.borrow_mut().window.settings_file = Some(file.clone());
    let read = || settings::Settings::load_from(&file);

    app.set_export_size("1024".into());
    app.invoke_export_preset_saved("Small".into());
    let on_disk = read();
    assert_eq!(on_disk.shared.export_presets.len(), 1);
    assert_eq!(on_disk.shared.export_presets[0].name, "Small");
    assert_eq!(on_disk.shared.export_presets[0].sheet.size, "1024");
    assert_eq!(on_disk.shared.export_preset, "Small");
    assert_eq!(on_disk.scope, "Parade");

    // Another saved, then the first picked back: the pick is kept.
    app.set_export_size("2048".into());
    app.invoke_export_preset_saved("Web".into());
    assert_eq!(read().shared.export_presets.len(), 2);
    app.invoke_export_preset_chosen("Small".into());
    let on_disk = read();
    assert_eq!(on_disk.shared.export_preset, "Small");
    assert_eq!(on_disk.shared.export.size, "1024");

    // Deleted: gone from the file, nothing chosen.
    app.invoke_export_preset_deleted();
    let on_disk = read();
    assert_eq!(on_disk.shared.export_presets.len(), 1);
    assert_eq!(on_disk.shared.export_presets[0].name, "Web");
    assert_eq!(on_disk.shared.export_preset, "");
    crate::testing::remove_dir_retry(&dir);
}

/// What the sheet's Folder row says, as the sheet asks it.
fn row(app: &App, room: i32) -> String {
    let typed = app.get_export_subfolder();
    app.invoke_export_folder_line(typed, "".into(), 0, app.get_export_folder_turn(), room)
        .to_string()
}

/// The Folder row shows where Export writes: the frame's own folder
/// until one is chosen, then the folder last chosen, in this run and
/// after a restart (the settings file). Its Choose button opens the
/// chooser on the folder shown, and an answer is the new folder, in
/// the row and in the file; a cancel leaves it. A preset that names
/// a subfolder shows that beside each original and leaves the
/// remembered folder alone; one that names none shows the remembered
/// folder, and its chooser opens there.
#[test]
fn the_folder_row_shows_where_the_export_goes_and_choose_sets_it() {
    let app = crate::testing::window(1);
    let dir = crate::testing::scratch_dir("export-folder");
    let raw = dir.join("shoot").join("a.cr3");
    let (state, _worker) = crate::testing::state_for(&app, vec![raw.clone()]);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let file = dir.join("greycard").join("settings.json");
    settings::Settings::default().save_to(&file);
    {
        let mut st = state.borrow_mut();
        st.window.settings_file = Some(file.clone());
        st.current = Some(0);
    }
    let shown = |p: &Path| p.display().to_string();

    // Nothing chosen yet: the frame's own folder.
    assert_eq!(sheet_folder(&state.borrow(), &raw), dir.join("shoot"));
    assert_eq!(row(&app, 500), shown(&dir.join("shoot")));

    // Choose: the chooser opens on the folder shown, and its
    // answer is the row's and the file's at once.
    CHOSEN.with(|c| *c.borrow_mut() = Some(Some(out.clone())));
    app.invoke_export_folder_choose();
    assert_eq!(
        ASKED.with(|a| a.borrow_mut().take()),
        Some(dir.join("shoot"))
    );
    assert_eq!(
        state.borrow().work.export_last_folder.as_deref(),
        Some(out.as_path())
    );
    assert_eq!(row(&app, 500), shown(&out));
    assert_eq!(
        settings::Settings::load_from(&file).export_last_folder,
        out.to_string_lossy()
    );
    assert!(!state.borrow().work.export_choosing);
    assert_eq!(app.get_status(), format!("exports go to {}", shown(&out)));

    // A cancel leaves it, and the next chooser opens where the
    // last answer was.
    app.invoke_export_folder_choose();
    assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(out.clone()));
    assert_eq!(row(&app, 500), shown(&out));
    assert_eq!(app.get_status(), "the export folder is as it was");

    // Cut at the front, so the folder's own name shows.
    let narrow = row(&app, 12);
    assert_eq!(narrow.chars().count(), 12, "{narrow}");
    assert!(
        narrow.starts_with('\u{2026}') && narrow.ends_with("out"),
        "{narrow}"
    );

    // A preset with a subfolder: beside each original, the
    // remembered folder kept.
    state.borrow_mut().work.export_presets = vec![
        ExportPreset {
            name: "Beside".into(),
            sheet: Sheet {
                subfolder: "web".into(),
                ..Sheet::default()
            },
        },
        ExportPreset {
            name: "Plain".into(),
            sheet: Sheet::default(),
        },
    ];
    app.invoke_export_preset_chosen("Beside".into());
    assert_eq!(row(&app, 500), "web beside each original");
    assert_eq!(sheet_folder(&state.borrow(), &raw), out);
    // Choosing a folder there clears the subfolder, which would
    // otherwise still win, and the preset reads as edited.
    let other = dir.join("other");
    CHOSEN.with(|c| *c.borrow_mut() = Some(Some(other.clone())));
    app.invoke_export_folder_choose();
    assert_eq!(app.get_export_subfolder(), "");
    assert!(app.get_export_preset_edited());
    assert_eq!(row(&app, 500), shown(&other));

    // A preset with none: the remembered folder, and Choose opens
    // the chooser there.
    app.invoke_export_preset_chosen("Plain".into());
    assert_eq!(row(&app, 500), shown(&other));
    app.invoke_export_folder_choose();
    assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(other.clone()));

    // A subfolder the export refuses: the row says nothing, the line
    // under the field says why.
    app.set_export_subfolder("../out".into());
    assert_eq!(row(&app, 500), "");
    app.set_export_subfolder("".into());

    // After a restart, last, since a second window's state is the
    // one this thread's callbacks find from then on: the close
    // writes the file from the panel and the state, as `run` does;
    // the next start reads it back.
    let mut closing = crate::panel::startup::remember(&app);
    crate::panel::startup::take_state_choices(&mut closing, &state.borrow());
    closing.save_to(&file);
    let kept = settings::Settings::load_from(&file);
    assert_eq!(kept.export_last_folder, other.to_string_lossy());
    let app2 = crate::testing::window(1);
    let (second, _worker) = crate::testing::state_for(&app2, vec![raw.clone()]);
    {
        let mut st = second.borrow_mut();
        st.work.export_last_folder = crate::panel::startup::remembered_folder(&kept);
        st.current = Some(0);
    }
    assert_eq!(row(&app2, 500), shown(&other));
    crate::testing::remove_dir_retry(&dir);
}

/// A small picture the editor opens, of our own writing.
fn picture(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(48, 32, |x, y| {
        image::Rgb([(x * 5) as u8, (y * 7) as u8, ((x + y) * 3) as u8])
    })
    .save(path)
    .unwrap();
}

/// The line under Name is the file the set's first frame is
/// written as: its own name by default, the pattern's otherwise
/// with `{seq}` as wide as the set needs, and the frame's own name
/// in the warning's color for a pattern that names none. The
/// pattern is kept with the last settings, for Ctrl+E.
#[test]
fn the_name_line_is_the_first_frame_s_file_under_the_pattern() {
    let app = crate::testing::window(12);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(12));
    {
        let mut st = state.borrow_mut();
        st.current = Some(4);
        st.views.picked = (2..12).collect();
        st.work.export_last_folder = Some(PathBuf::from("/out"));
    }
    app.set_set_count(10);
    app.set_export_open(true);
    let line = || {
        app.invoke_export_name_example(
            app.get_export_name(),
            app.get_export_subfolder(),
            app.get_export_format(),
            "".into(),
            0,
            10,
            0,
        )
        .to_string()
    };
    let shown = || crate::testing::label_of(&app, "ExportSheet::name-example-text");
    assert_eq!(app.get_export_name(), "{name}");
    assert_eq!(line(), "IMG_0002.jpg");
    assert_eq!(shown(), "IMG_0002.jpg");
    assert!(!app.invoke_export_name_refused(app.get_export_name()));

    app.set_export_name("trip {seq} {name}".into());
    assert_eq!(line(), "trip 01 IMG_0002.jpg");
    assert_eq!(shown(), "trip 01 IMG_0002.jpg");
    app.set_export_name("{seq:101}".into());
    app.set_export_format("PNG".into());
    assert_eq!(line(), "101.png");
    // Beside the frame, the name the editor writes there.
    app.set_export_subfolder("web".into());
    assert_eq!(line(), "101.png");
    app.set_export_subfolder("".into());
    state.borrow_mut().work.export_last_folder = None;
    assert_eq!(line(), "101.greycard.png");

    // A pattern that names no file: the frame's own name, flagged.
    app.set_export_name("{name}-{lens}".into());
    assert_eq!(line(), "IMG_0002.greycard.png");
    assert!(app.invoke_export_name_refused(app.get_export_name()));

    // Kept with the last settings.
    app.set_export_name("{date}_{seq}".into());
    let kept = crate::panel::startup::remember(&app);
    assert_eq!(kept.shared.export.name, "{date}_{seq}");
}

/// A set is written under the pattern, each frame its number; two
/// the pattern names alike are told apart as a pair of one name is,
/// and a bad pattern writes each under its own name.
#[test]
fn a_set_is_written_under_the_name_pattern() {
    let app = crate::testing::window(2);
    let dir = crate::testing::scratch_dir("export-name");
    let sources = vec![
        dir.join("shoot").join("a.png"),
        dir.join("shoot").join("b.png"),
    ];
    for s in &sources {
        picture(s);
    }
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let (state, _worker) = crate::testing::state_for(&app, sources.clone());
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.picked = vec![0, 1];
        st.work.export_last_folder = Some(out.clone());
    }
    let written = |name: &str| {
        let file = out.join(name);
        let start = Instant::now();
        while !file.exists() {
            assert!(start.elapsed() < Duration::from_secs(60), "{name} written");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let send = |pattern: &str, names: &[&str]| {
        app.set_export_name(pattern.into());
        app.invoke_export();
        assert!(state.borrow().work.exporting.is_some(), "{pattern}");
        for name in names {
            written(name);
        }
        state.borrow_mut().work.exporting = None;
        app.set_export_running(false);
    };
    send("shoot {seq:7}-{name}", &["shoot 7-a.jpg", "shoot 8-b.jpg"]);
    send("shoot", &["shoot.jpg", "shoot-1.jpg"]);
    send("{nmae}", &["a.jpg", "b.jpg"]);
    crate::testing::remove_scratch(state, &dir);
}

/// Ctrl+E: before any export from the sheet, the sheet, as
/// Ctrl+Shift+E opens it; once one has gone out, here or in an
/// earlier run, the selection goes out as the sheet stands, into
/// its folder, with no sheet and no dialog.
#[test]
fn ctrl_e_sends_the_selection_again_once_an_export_has_set_the_sheet() {
    use slint::platform::{Key, WindowEvent};
    let app = crate::testing::window(1);
    let dir = crate::testing::scratch_dir("export-again");
    let source = dir.join("shoot").join("frame.png");
    picture(&source);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let file = dir.join("greycard").join("settings.json");
    settings::Settings::default().save_to(&file);
    let (state, _worker) = crate::testing::state_for(&app, vec![source.clone()]);
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.work.export_last_folder = Some(out.clone());
        st.window.settings_file = Some(file.clone());
    }
    app.set_selected(0);
    let ctrl_e = || {
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        crate::testing::press(&app, "e");
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    };
    let written = |name: &str| {
        let file = out.join(name);
        let start = Instant::now();
        while !file.exists() {
            assert!(start.elapsed() < Duration::from_secs(60), "{name} written");
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    ctrl_e();
    assert!(app.get_export_open(), "the sheet, before any export");
    assert!(state.borrow().work.exporting.is_none());
    // The sheet's Export sends one, and says so in the file.
    app.set_export_open(false);
    app.invoke_export();
    assert!(settings::Settings::load_from(&file).exported);
    written("frame.jpg");
    state.borrow_mut().work.exporting = None;
    app.set_export_running(false);
    slint::platform::update_timers_and_animations();

    ASKED.with(|a| a.borrow_mut().take());
    ctrl_e();
    assert!(!app.get_export_open(), "no sheet");
    assert!(ASKED.with(|a| a.borrow().is_none()), "no dialog");
    let set = state
        .borrow()
        .work
        .exporting
        .clone()
        .expect("the selection, at once");
    assert_eq!(set.folder.as_deref(), Some(out.as_path()));
    written("frame-1.jpg");
    state.borrow_mut().work.exporting = None;
    crate::testing::remove_scratch(state, &dir);
}

/// Export writes into the folder the row shows, with no dialog: the
/// frame on screen under its own name there, the file on disk. A
/// file of that name already there is the overwrite sheet's
/// question under Overwrite, as for a set. A folder that is gone or
/// will not take files is said, and the chooser asked on it; a
/// cancel exports nothing, an answer exports there.
#[test]
fn export_writes_into_the_shown_folder_with_no_dialog() {
    let app = crate::testing::window(1);
    let dir = crate::testing::scratch_dir("export-shown");
    let source = dir.join("shoot").join("frame.png");
    picture(&source);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let (state, _worker) = crate::testing::state_for(&app, vec![source.clone()]);
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.work.export_last_folder = Some(out.clone());
    }
    ASKED.with(|a| a.borrow_mut().take());

    app.invoke_export();
    assert!(ASKED.with(|a| a.borrow().is_none()), "no dialog");
    let set = state
        .borrow()
        .work
        .exporting
        .clone()
        .expect("the set, at once");
    assert_eq!(set.total, 1);
    assert_eq!(set.folder.as_deref(), Some(out.as_path()));
    assert_eq!(set.sub, None);
    // The frame's own name, in the format's extension.
    let written = out.join("frame.jpg");
    let start = Instant::now();
    while !written.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the worker writes {}",
            written.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Nothing beside the frame, nothing left over from the check.
    let names = |d: &Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    assert!(
        !names(&dir.join("shoot"))
            .iter()
            .any(|n| n.ends_with(".jpg")),
        "{:?}",
        names(&dir.join("shoot"))
    );
    assert!(
        !names(&out).iter().any(|n| n.contains("write-check")),
        "{:?}",
        names(&out)
    );
    // Wait for the set to be let go of, as its end would.
    state.borrow_mut().work.exporting = None;

    // That name is there now: under Overwrite, the sheet asks.
    app.set_export_on_exists("Overwrite".into());
    app.invoke_export();
    assert!(app.get_overwrite_open());
    assert_eq!(
        app.get_overwrite_files()
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>(),
        [written.display().to_string()]
    );
    app.invoke_overwrite_answered(0);
    assert!(state.borrow().work.exporting.is_none());
    app.set_export_on_exists("Increment".into());

    // Gone since it was chosen: said, and the chooser asked on it.
    let gone = dir.join("unplugged");
    state.borrow_mut().work.export_last_folder = Some(gone.clone());
    app.invoke_export();
    assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(gone.clone()));
    assert_eq!(
        app.get_status(),
        format!("not exported: {} is not there", gone.display())
    );
    assert!(state.borrow().work.exporting.is_none());
    assert!(!state.borrow().work.export_choosing);
    assert!(!gone.exists(), "never made");
    // Answered: there, and it is the folder from now on.
    let again = dir.join("again");
    std::fs::create_dir_all(&again).unwrap();
    CHOSEN.with(|c| *c.borrow_mut() = Some(Some(again.clone())));
    app.invoke_export();
    let set = state.borrow().work.exporting.clone().expect("the set");
    assert_eq!(set.folder.as_deref(), Some(again.as_path()));
    assert_eq!(
        state.borrow().work.export_last_folder.as_deref(),
        Some(again.as_path())
    );
    let start = Instant::now();
    while !again.join("frame.jpg").exists() {
        assert!(start.elapsed() < Duration::from_secs(60), "written");
        std::thread::sleep(Duration::from_millis(50));
    }
    state.borrow_mut().work.exporting = None;

    // A folder that will not take files: said the same way.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Not where permissions are not kept (root).
        if folder_problem(&locked).is_some() {
            state.borrow_mut().work.export_last_folder = Some(locked.clone());
            app.invoke_export();
            assert_eq!(ASKED.with(|a| a.borrow_mut().take()), Some(locked.clone()));
            assert_eq!(
                app.get_status(),
                format!("not exported: {} cannot be written to", locked.display())
            );
            assert!(state.borrow().work.exporting.is_none());
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(
        folder_problem(&written),
        Some(format!("{} is not a folder", written.display()))
    );
    crate::testing::remove_dir_retry(&dir);
}

/// Microsoft's runtime that arrived and would not load is said as
/// that, not as a download that failed, lets go of what it held
/// up, and is not offered again this session.
#[test]
fn a_runtime_that_would_not_load_is_said_so_and_not_offered_again() {
    use crate::panel::assets::{AfterRuntime, runtime_first_for};
    use greycard_ai::runtime::Unavailable;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let runtime = &greycard_ai::registry::ONNXRUNTIME_CPU_LINUX_X64;
    {
        let mut st = state.borrow_mut();
        st.sheets.fetching = true;
        st.sheets.runtime_next = Some(AfterRuntime::Model(&greycard_ai::SKY, false));
    }
    let message = "the AI features are off: ONNX Runtime at /c/libonnxruntime.so would not \
                   load: GLIBC_2.27 not found; it is removed again, and offered at the next \
                   start";
    deliver(
        &app,
        Outcome::RuntimeUnusable {
            id: runtime.id,
            message: message.to_string(),
        },
    );
    assert_eq!(app.get_status(), message);
    assert!(!app.get_status().contains("could not be fetched"));
    {
        let st = state.borrow();
        assert!(!st.sheets.fetching);
        assert_eq!(st.sheets.runtime_next, None);
        assert!(st.sheets.fetch_failed.contains(&runtime.id));
    }
    let why: &'static Unavailable = Box::leak(Box::new(Unavailable::Fetchable {
        missing: vec!["AVX2"],
        name: None,
        runtime,
    }));
    assert!(!runtime_first_for(
        &mut state.borrow_mut(),
        &app,
        why,
        AfterRuntime::Nothing
    ));
    assert!(!app.get_fetch_open());
}

/// A set is read as each frame's own edit, the one on screen as
/// the panel has it; a running set stops on Escape, and its end
/// says what came of it and lets the Export button go.
#[test]
fn a_set_takes_each_frame_s_edit_and_stops_on_escape() {
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.picked = vec![0, 2];
        {
            let mut e = st.sidecars[2].current().clone();
            e.light.exposure = 1.0;
            st.sidecars[2].record(e);
        }
        st.sidecars[2].turn = 3;
        {
            let mut e = st.sidecars[1].current().clone();
            e.light.exposure = -2.0;
            st.sidecars[1].record(e);
        }
    }
    app.set_exposure(0.5);
    let frames = set_frames(&mut state.borrow_mut(), &app);
    let got: Vec<(String, f32, u8)> = frames
        .iter()
        .map(|(f, e, t, _, _)| (file_name(f), e.light.exposure, *t))
        .collect();
    assert_eq!(
        got,
        vec![
            ("IMG_0000.CR3".to_string(), 0.5, 0),
            ("IMG_0002.CR3".to_string(), 1.0, 3),
        ]
    );

    // Running: Escape stops it after the frame in hand.
    let set = Arc::new(queue::Set::new(
        2,
        Some(PathBuf::from("/nowhere/out")),
        export::Settings::default(),
        export::OnExists::Increment,
    ));
    state.borrow_mut().work.exporting = Some(set.clone());
    app.set_export_running(true);
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(set.is_canceled());
    assert!(
        app.get_status().contains("stopping"),
        "{}",
        app.get_status()
    );
    // The set collapses on the next Escape, not this one; the
    // next is the window's again though the frame in hand runs on.
    assert_eq!(state.borrow().views.picked, vec![0, 2]);
    assert!(app.get_export_stopping());
    app.set_set_count(2);
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert_eq!(chosen_frames(&state.borrow()), vec![0]);
    assert!(app.get_export_running(), "the frame in hand still runs");

    // Its end: the line, and the button is Export again.
    let tally = queue::Tally {
        exported: 1,
        canceled: 1,
        ..queue::Tally::default()
    };
    deliver(&app, Outcome::SetDone { set, tally });
    assert!(!app.get_export_running());
    assert!(!app.get_export_stopping());
    assert!(state.borrow().work.exporting.is_none());
    assert!(
        app.get_status()
            .starts_with("export stopped: 1 of 2 exported to"),
        "{}",
        app.get_status()
    );
}

#[test]
fn an_empty_mark_keeps_the_sheet_up_and_says_why() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    state.borrow_mut().current = Some(0);
    app.set_export_mark(sheet::MARK_IMAGE.into());
    app.set_export_mark_image("".into());
    app.set_export_open(false);
    app.invoke_export();
    assert!(app.get_export_open());
    assert!(app.get_status().contains("no PNG"), "{}", app.get_status());
    app.set_export_mark(sheet::MARK_TEXT.into());
    app.set_export_mark_text("  ".into());
    app.set_export_open(false);
    app.invoke_export();
    assert!(app.get_export_open());
    assert!(
        app.get_status().contains("text is empty"),
        "{}",
        app.get_status()
    );
}

#[test]
fn a_subfolder_exports_beside_each_frame_without_a_chooser() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    state.borrow_mut().current = Some(0);
    // One that climbs out is refused, and the sheet stays up.
    app.set_export_subfolder("../out".into());
    app.set_export_open(false);
    app.invoke_export();
    assert!(app.get_export_open());
    assert!(state.borrow().work.exporting.is_none());
    assert!(
        app.get_status().starts_with("not exported: the subfolder"),
        "{}",
        app.get_status()
    );
    // One that goes down: the frame on screen is a set of one into
    // it, nothing asked, under the sheet's own policy.
    app.set_export_subfolder(" export ".into());
    app.set_export_on_exists("Skip".into());
    app.invoke_export();
    let st = state.borrow();
    assert!(!st.work.export_choosing);
    let set = st.work.exporting.as_ref().expect("a set, with no chooser");
    assert_eq!(set.total, 1);
    assert_eq!(set.folder, None);
    assert_eq!(set.sub.as_deref(), Some(Path::new("export")));
    assert_eq!(set.on_exists, export::OnExists::Skip);
    assert!(set.place().starts_with("into export"), "{}", set.place());
}

/// The sheet's primary button reads Export, for one frame and for a
/// set alike, since no dialog follows; the chooser is the Folder
/// row's Choose button, and there is no Choose file. The row names a
/// typed subfolder by the reading the export goes by, cut at the
/// front; one the export refuses stops the click.
#[test]
fn the_primary_button_reads_export_and_the_row_names_the_subfolder() {
    let app = crate::testing::window(2);
    let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    use crate::testing::{count_id, label_of};
    let (title, button) = ("ExportSheet::title", "ExportSheet::export-button");
    assert_eq!(count_id(&app, button), 0, "the sheet closed");
    app.set_export_open(true);
    // The title and the button say Export.
    assert_eq!(label_of(&app, title), crate::tr!("Export"));
    assert_eq!(label_of(&app, button), crate::tr!("Export"));
    // The Folder row's button chooses a folder, not a file.
    assert_eq!(
        label_of(&app, "ExportSheet::folder-choose-button"),
        crate::tr!("Choose..."),
        "the Folder row's"
    );
    assert_eq!(count_id(&app, "ExportSheet::queue-add-button"), 1);
    assert_eq!(
        label_of(&app, "ExportSheet::folder-line-text"),
        "/nowhere",
        "the frame's own folder"
    );
    // A set: the title counts the frames, the button is the same.
    app.set_set_count(3);
    assert_eq!(
        label_of(&app, title),
        crate::tr!("Export {n} frame" | "Export {n} frames" % 3)
    );
    assert_eq!(label_of(&app, button), crate::tr!("Export"), "the button");

    let long = "a-very-long-subfolder/for-the-web-gallery";
    app.set_export_subfolder(long.into());
    assert_eq!(row(&app, 500), format!("{long} beside each original"));
    assert_eq!(row(&app, 30), "\u{2026}-gallery beside each original");
    assert_eq!(app.invoke_export_subfolder_name(long.into()), long);
    for typed in ["../out", "/tmp/x", "."] {
        assert!(!app.invoke_export_subfolder_problem(typed.into()).is_empty());
        // And a click on it is refused.
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
        state.borrow_mut().current = Some(0);
        app.set_export_subfolder(typed.into());
        assert_eq!(row(&app, 500), "", "{typed}");
        app.set_export_open(false);
        app.invoke_export();
        assert!(app.get_export_open(), "{typed}");
        assert!(state.borrow().work.exporting.is_none(), "{typed}");
    }
}

#[test]
fn overwrite_names_every_file_there_and_waits_for_an_answer() {
    let dir = std::env::temp_dir().join(format!("greycard-overwrite-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(dir.join("export")).unwrap();
    let files: Vec<PathBuf> = (0..3)
        .map(|i| {
            let f = dir.join(format!("IMG_{i:04}.CR3"));
            std::fs::write(&f, b"").unwrap();
            f
        })
        .collect();
    // Two of the three are there already.
    for i in [0, 2] {
        std::fs::write(dir.join("export").join(format!("IMG_{i:04}.jpg")), b"").unwrap();
    }
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.picked = vec![0, 1, 2];
    }
    app.set_export_subfolder("export".into());
    app.set_export_on_exists("Overwrite".into());
    let ask = || {
        app.invoke_export();
        assert!(app.get_overwrite_open());
        assert!(
            state.borrow().work.exporting.is_none(),
            "nothing written yet"
        );
    };

    ask();
    assert_eq!(app.get_overwrite_title(), "2 files are there already");
    let listed: Vec<String> = app
        .get_overwrite_files()
        .iter()
        .map(|f| f.to_string())
        .collect();
    assert_eq!(
        listed,
        vec![
            dir.join("export")
                .join("IMG_0000.jpg")
                .display()
                .to_string(),
            dir.join("export")
                .join("IMG_0002.jpg")
                .display()
                .to_string(),
        ]
    );
    // Another Export waits for the answer.
    app.set_status("".into());
    app.invoke_export();
    assert!(state.borrow().work.export_pending.is_some());
    // No: nothing goes, and Export is free again.
    app.invoke_overwrite_answered(0);
    assert!(!app.get_overwrite_open());
    assert_eq!(app.get_status(), "export canceled");
    assert!(state.borrow().work.exporting.is_none() && !state.borrow().work.export_choosing);

    // Keep both: the set, under Increment.
    ask();
    app.invoke_overwrite_answered(1);
    let set = state.borrow().work.exporting.clone().expect("the set");
    assert_eq!(set.on_exists, export::OnExists::Increment);
    assert_eq!(set.total, 3);
    state.borrow_mut().work.exporting = None;

    // Overwrite: the set, under Overwrite.
    ask();
    app.invoke_overwrite_answered(2);
    let set = state.borrow().work.exporting.clone().expect("the set");
    assert_eq!(set.on_exists, export::OnExists::Overwrite);
    state.borrow_mut().work.exporting = None;

    // Nothing there: no question.
    for i in [0, 2] {
        std::fs::remove_file(dir.join("export").join(format!("IMG_{i:04}.jpg"))).unwrap();
    }
    app.invoke_export();
    assert!(!app.get_overwrite_open());
    assert!(state.borrow().work.exporting.is_some());
    state.borrow_mut().work.exporting = None;
    crate::testing::remove_dir_retry(&dir);
}

/// Overwrite asks about the files the set will write, a version's
/// included: the version's file there is asked about, and a name
/// the set does not write is not.
#[test]
fn overwrite_asks_about_a_version_s_file() {
    let dir = crate::testing::scratch_dir("overwrite-version");
    std::fs::create_dir_all(dir.join("export")).unwrap();
    let file = dir.join("IMG_0000.CR3");
    std::fs::write(&file, b"").unwrap();
    for there in ["IMG_0000-Warm.jpg", "IMG_0000-1.jpg"] {
        std::fs::write(dir.join("export").join(there), b"").unwrap();
    }
    let app = crate::testing::window(1);
    let (state, worker) = crate::testing::state_for(&app, vec![file]);
    {
        let mut st = state.borrow_mut();
        st.sidecars[0].new_version(None, Some("Warm")).unwrap();
        crate::versions::settle(&mut st, &app, &worker);
    }
    app.invoke_select(0);
    app.invoke_step(1, true);
    app.set_export_subfolder("export".into());
    app.set_export_on_exists("Overwrite".into());
    app.invoke_export();
    assert!(app.get_overwrite_open());
    assert_eq!(
        app.get_overwrite_title(),
        "IMG_0000-Warm.jpg is there already"
    );
    app.invoke_overwrite_answered(0);
    crate::testing::remove_scratch(state, &dir);
}

/// `{date}` and `{camera}` come from the index's row and read
/// nothing from a file it holds; a file it does not hold is read
/// once while the sheet is up, however often the line asks.
#[test]
fn the_name_line_reads_a_file_only_when_the_index_has_no_row() {
    use greycard_library::fixture::{R5, write_frame};
    let dir = crate::testing::scratch_dir("export-name-index");
    let files = vec![dir.join("a.tif"), dir.join("b.tif")];
    write_frame(&files[0], &R5, 1);
    write_frame(&files[1], &R5, 2);
    let mut lib = greycard_library::Library::open_in_memory().unwrap();
    lib.index_file(&files[0]).unwrap();
    let id = lib.by_path(&files[0]).unwrap().unwrap().id;
    // Gone from the disk: what the line says is the index's.
    std::fs::remove_file(&files[0]).unwrap();
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.index_ids = vec![Some(id), None];
        st.views.index_reader = Some(lib);
        st.sidecars[0].meta.time_shift = 9 * 3600;
        st.work.export_last_folder = Some(dir.join("out"));
    }
    forget_shots();
    let read = || SHOTS_READ.with(|n| *n.borrow());
    let before = read();
    let line = |pattern: &str| name_example(&state.borrow(), pattern, "", "JPEG");
    assert_eq!(line("{date} {camera}"), "2024-08-25 Canon EOS R5.jpg");
    assert_eq!(read(), before, "nothing read for an indexed file");
    state.borrow_mut().current = Some(1);
    line("{camera}");
    line("{camera} {date}");
    assert_eq!(read(), before + 1, "read once");
    app.invoke_export_sheet_opened();
    line("{camera}");
    assert_eq!(read(), before + 2, "again once the sheet opens again");
    // A pattern with neither reads nothing.
    forget_shots();
    line("{seq} {name}");
    assert_eq!(read(), before + 2);
    crate::testing::remove_scratch(state, &dir);
}

/// Where the element `id` is, and how big, if it is on show: the
/// topmost of several.
fn shown_id(app: &App, id: &str) -> Option<(slint::LogicalPosition, slint::LogicalSize)> {
    use i_slint_backend_testing::ElementHandle;
    // A control a property has just brought in is built on the
    // next event: a pointer move at the corner, where nothing is.
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
    ElementHandle::find_by_element_id(app, id)
        .map(|e| (e.absolute_position(), e.size()))
        .min_by(|a, b| a.0.y.total_cmp(&b.0.y))
}

fn wheel(app: &App, x: f32, y: f32, delta_y: f32) {
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(x, y),
            delta_x: 0.0,
            delta_y,
        });
}

#[test]
fn a_wheel_over_an_open_sheet_leaves_the_picture_alone() {
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 600.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    // Where the picture is and how big: each wheel is judged on its
    // own, as a swipe out and its swipe back would cancel.
    let view = || {
        let st = state.borrow();
        (st.develop.zoom, st.develop.center)
    };
    // Left of any sheet's card, over the picture.
    let (x, y) = (400.0, 300.0);
    let before = view();
    wheel(&app, x, y, 60.0);
    assert_ne!(view(), before, "with no sheet up, the wheel zooms there");
    let still = |at: (f32, f32), what: &str| {
        for delta in [60.0, -60.0, 15.0, -15.0] {
            let before = view();
            wheel(&app, at.0, at.1, delta);
            assert_eq!(view(), before, "{what}, by {delta}");
        }
    };

    type Open = fn(&App, bool);
    let sheets: [(&str, Open); 7] = [
        ("export", |a, on| a.set_export_open(on)),
        ("preset", |a, on| a.set_preset_open(on)),
        ("import", |a, on| a.set_import_open(on)),
        ("settings", |a, on| a.set_settings_open(on)),
        ("archive", |a, on| a.set_archive_open(on)),
        ("sync", |a, on| a.set_sync_open(on)),
        ("match", |a, on| a.set_match_open(on)),
    ];
    for (name, open) in sheets {
        open(&app, true);
        // Its surround, and the middle of the window, where its
        // card is.
        still((x, y), &format!("the {name} sheet's surround"));
        still((750.0, 300.0), &format!("the {name} sheet's card"));
        open(&app, false);
    }

    // The export sheet's own header and buttons, outside its
    // scrolled choices, and those choices scrolled to either end.
    app.set_export_mark("Text".into());
    app.set_export_open(true);
    let (title, _) = shown_id(&app, "ExportSheet::title").expect("the title");
    let (cancel, size) = shown_id(&app, "ExportSheet::cancel-button").expect("Cancel");
    let (format, _) = shown_id(&app, "ExportSheet::format-row").expect("the Format row");
    still((title.x + 4.0, title.y + 4.0), "the export sheet's header");
    still(
        (cancel.x + size.width / 2.0, cancel.y + size.height / 2.0),
        "the export sheet's buttons",
    );
    for _ in 0..20 {
        still((format.x + 4.0, format.y + 4.0), "the choices");
    }
}

/// The export sheet in every bundled language at 150 %: its label
/// column as wide as its widest label, so none is cut short, every
/// control still inside the card, and the card in the least window
/// the window goes to and in 1024 by 640. English keeps the 68 px
/// column it has always had.
#[test]
fn the_export_sheet_labels_fit_at_150_percent_in_each_language() {
    use i_slint_backend_testing::ElementHandle;
    let languages =
        std::iter::once("").chain(crate::i18n::languages().into_iter().map(|(tag, _)| tag));
    for language in languages {
        let app = crate::testing::window(1);
        let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
        app.invoke_select(0);
        let _language = crate::testing::in_language(language);
        app.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
        // Tall enough that no row is scrolled out of view, with every
        // row the sheet has on show.
        app.window()
            .set_size(slint::LogicalSize::new(1024.0, 1400.0));
        app.set_export_mark("Text".into());
        app.set_export_size("Custom".into());
        app.set_export_border(true);
        app.set_export_open(true);
        crate::testing::settle(&app);
        let one = |id: &str| {
            ElementHandle::find_by_element_id(&app, &format!("ExportSheet::{id}"))
                .next()
                .unwrap_or_else(|| panic!("{language}: {id}"))
        };
        // Each label beside what it needs, measured by the sheet's own
        // hidden copy of its words.
        for (label, words) in [
            ("preset-words", "widest-preset"),
            ("format-words", "widest-format"),
            ("long-edge-words", "widest-long-edge"),
            ("pixels-words", "widest-pixels"),
            ("sharpen-words", "widest-sharpen"),
            ("color-words", "widest-color"),
            ("metadata-words", "widest-metadata"),
            ("subfolder-words", "widest-subfolder"),
            ("name-2-words", "widest-name"),
            ("if-it-exists-words", "widest-if-it-exists"),
            ("watermark-words", "widest-watermark"),
            ("text-words", "widest-text"),
        ] {
            let need = one(words).size().width;
            let has = one(label).size().width;
            assert!(need > 0.0, "{language}: {words} measured");
            assert!(
                has + 0.5 >= need.min(150.0),
                "{language}: {label} is {has} px for {need} px"
            );
            if language.is_empty() {
                assert_eq!(has, 68.0, "{label}: English's column");
            }
        }
        // A slider's label, its gap to the track kept.
        for (slider, words) in [
            ("quality-slider", "widest-quality"),
            ("width-slider", "widest-width"),
            ("size-slider", "widest-size"),
            ("margin-slider", "widest-margin"),
            ("opacity-slider", "widest-opacity"),
        ] {
            let need = one(words).size().width;
            let label = one(slider)
                .query_descendants()
                .match_id("EditSlider::label-words")
                .find_first()
                .unwrap_or_else(|| panic!("{language}: {slider}'s label"));
            let has = label.size().width;
            assert!(
                has + 0.5 >= need.min(150.0),
                "{language}: {slider}'s label is {has} px for {need} px"
            );
        }
        // Every control inside the card's padding (`Theme.space-5`), and
        // the card as wide as English's but for what the column adds.
        let card = one("card");
        let right = card.absolute_position().x + card.size().width - 24.0;
        let column = one("format-words").size().width;
        assert_eq!(
            card.size().width,
            420.0 + column - 68.0,
            "{language}: the card"
        );
        for kind in [
            "Segmented",
            "LineEdit",
            "ComboBox",
            "EditSlider",
            "PanelButton",
        ] {
            for c in card.query_descendants().match_inherits(kind).find_all() {
                let end = c.absolute_position().x + c.size().width;
                assert!(
                    end <= right + 0.5,
                    "{language}: a {kind} ends at {end}, the card's padding at {right}"
                );
            }
        }
        // And in the least window the card fits it, Export in reach.
        let least = (app.get_least_width(), app.get_least_height());
        for (width, height) in [least, (1024.0, 640.0)] {
            app.window()
                .set_size(slint::LogicalSize::new(width, height));
            crate::testing::settle(&app);
            let card = one("card");
            let (at, size) = (card.absolute_position(), card.size());
            assert!(
                at.x >= 0.0 && at.x + size.width <= width && at.y + size.height <= height,
                "{language} in {width} by {height}: the card at {at:?} {size:?}"
            );
            let cancel = one("cancel-button");
            assert!(
                cancel.absolute_position().y + cancel.size().height <= height,
                "{language} in {width} by {height}: Cancel in reach"
            );
        }
    }
}

#[test]
fn the_export_sheet_fits_a_short_window_and_its_choices_scroll() {
    let app = crate::testing::window(1);
    // Just over the shortest the window goes.
    let height = 520.0;
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, height));
    let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    // Everything that makes it tall: a watermark's block and the
    // queue's row.
    app.set_export_mark("Text".into());
    app.set_export_queue_sets(2);
    app.set_export_queue_line("2 sets waiting".into());
    app.set_export_open(true);

    let bottom = |id: &str| {
        let (at, size) = shown_id(&app, id).unwrap_or_else(|| panic!("{id} on show"));
        at.y + size.height
    };
    // Every button in the window, below the choices, and the
    // Folder row with them: its folder by the path it shows.
    for id in [
        "ExportSheet::folder-line-text",
        "ExportSheet::folder-choose-button",
        "ExportSheet::cancel-button",
        "ExportSheet::queue-add-button",
        "ExportSheet::queue-run-button",
        "ExportSheet::queue-clear-button",
    ] {
        assert!(bottom(id) <= height, "{id} at {} in {height}", bottom(id));
    }
    let (title, _) = shown_id(&app, "ExportSheet::title").expect("the title");
    let (cancel, _) = shown_id(&app, "ExportSheet::cancel-button").expect("Cancel");
    let (folder, _) = shown_id(&app, "ExportSheet::folder-line-text").expect("the Folder row");
    // The watermark's strength is past the fold, until the choices
    // are scrolled to it.
    assert!(
        shown_id(&app, "ExportSheet::opacity-slider").is_none(),
        "the choices are taller than their view"
    );
    let (format, _) = shown_id(&app, "ExportSheet::format-row").expect("the Format row");
    for _ in 0..20 {
        wheel(&app, format.x + 4.0, format.y + 4.0, -60.0);
    }
    let (opacity, size) =
        shown_id(&app, "ExportSheet::opacity-slider").expect("scrolled to Opacity");
    assert!(
        opacity.y + size.height <= folder.y,
        "above the folder and buttons"
    );
    assert_eq!(
        shown_id(&app, "ExportSheet::title").expect("the title").0,
        title,
        "the header stays"
    );
    assert_eq!(
        shown_id(&app, "ExportSheet::cancel-button")
            .expect("Cancel")
            .0,
        cancel,
        "the buttons stay"
    );
    assert_eq!(
        shown_id(&app, "ExportSheet::folder-line-text")
            .expect("the Folder row")
            .0,
        folder,
        "the folder stays"
    );
}

/// A set of one ends on the file's own line, as the frame on screen
/// always did: its name, the rename Increment made and what the edit
/// went without, none of which a set's count says. A set of two ends
/// on the count.
#[test]
fn a_set_of_one_ends_on_the_file_s_own_line() {
    let app = crate::testing::window(2);
    let dir = crate::testing::scratch_dir("export-one-line");
    std::fs::create_dir_all(&dir).unwrap();
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.work.export_last_folder = Some(dir.clone());
    }
    app.invoke_export();
    let set = state.borrow().work.exporting.clone().expect("the set");
    assert_eq!(set.total, 1);
    // Taken off the worker's own answers: the frame was renamed and
    // went without a look.
    let source = state.borrow().files[0].clone();
    let path = dir.join("IMG_0000-1.jpg");
    let note = "IMG_0000.jpg was there already: wrote IMG_0000-1.jpg";
    let left_out = vec!["look Faded: not on this machine".to_string()];
    let done = |done| Outcome::SetFrameDone {
        set: set.clone(),
        index: 0,
        source: source.clone(),
        edit: Edit::default(),
        version: None,
        done,
    };
    deliver(
        &app,
        done(queue::Done::Exported {
            path: path.clone(),
            seconds: 1.25,
            note: Some(note.into()),
            left_out: left_out.clone(),
        }),
    );
    let tally = queue::Tally {
        exported: 1,
        ..queue::Tally::default()
    };
    deliver(
        &app,
        Outcome::SetDone {
            set: set.clone(),
            tally,
        },
    );
    let line = app.get_status().to_string();
    assert_eq!(line, exported_line(&path, 1.25, Some(note), &left_out));
    assert!(
        line.starts_with("exported IMG_0000-1.jpg in 1.25 s"),
        "{line}"
    );
    assert!(line.contains("was there already"), "{line}");
    assert!(
        line.contains(", without one thing the edit names"),
        "{line}"
    );
    assert!(state.borrow().work.export_one_line.is_none());
    assert!(state.borrow().work.exporting.is_none());

    // Skipped under Skip: the file's own line too, not a count of
    // nothing.
    app.invoke_export();
    let set = state.borrow().work.exporting.clone().expect("the set");
    let done = |done| Outcome::SetFrameDone {
        set: set.clone(),
        index: 0,
        source: source.clone(),
        edit: Edit::default(),
        version: None,
        done,
    };
    deliver(
        &app,
        done(queue::Done::Skipped {
            path: dir.join("IMG_0000.jpg"),
        }),
    );
    let tally = queue::Tally {
        skipped: 1,
        ..queue::Tally::default()
    };
    deliver(&app, Outcome::SetDone { set, tally });
    assert_eq!(
        app.get_status(),
        "IMG_0000.jpg is there already: nothing exported"
    );

    // Two frames: the count, as before.
    state.borrow_mut().views.picked = vec![0, 1];
    app.invoke_export();
    let set = state
        .borrow()
        .work
        .exporting
        .clone()
        .expect("the set of two");
    assert_eq!(set.total, 2);
    let tally = queue::Tally {
        exported: 2,
        ..queue::Tally::default()
    };
    deliver(&app, Outcome::SetDone { set, tally });
    assert!(
        app.get_status().starts_with("exported 2 frames"),
        "{}",
        app.get_status()
    );
    crate::testing::remove_dir_retry(&dir);
}

/// A look at the folder that has not answered (a mount that keeps
/// it waiting) holds nothing up: Choose still opens, and a second
/// Export overtakes it; the first look's answer, come late, starts
/// nothing, and only the last one's exports.
#[test]
fn a_look_at_the_folder_that_is_slow_to_answer_holds_nothing_up() {
    let app = crate::testing::window(1);
    let dir = crate::testing::scratch_dir("export-slow-look");
    let (first, second) = (dir.join("first"), dir.join("second"));
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.work.export_last_folder = Some(first.clone());
    }
    HELD.with(|h| *h.borrow_mut() = Some(Vec::new()));
    app.invoke_export();
    assert!(state.borrow().work.exporting.is_none(), "still looking");
    assert!(!state.borrow().work.export_choosing, "nothing held up");

    // Choose works meanwhile, and overtakes the look.
    CHOSEN.with(|c| *c.borrow_mut() = Some(Some(second.clone())));
    app.invoke_export_folder_choose();
    assert_eq!(
        state.borrow().work.export_last_folder.as_deref(),
        Some(second.as_path())
    );
    assert_eq!(
        app.get_status(),
        format!(
            "exports go to {}; the export waiting on {} was not started",
            second.display(),
            first.display()
        )
    );
    // Export again: a second look, at the new folder.
    app.invoke_export();
    let held = HELD.with(|h| h.borrow_mut().take()).expect("held");
    assert_eq!(held.len(), 2);
    let mut held = held.into_iter();
    // The first answers late: dropped.
    (held.next().unwrap())(&app);
    assert!(
        state.borrow().work.exporting.is_none(),
        "the late answer starts nothing"
    );
    // The second: the export, into the folder chosen.
    (held.next().unwrap())(&app);
    let set = state.borrow().work.exporting.clone().expect("the set");
    assert_eq!(set.folder.as_deref(), Some(second.as_path()));
    set.cancel();

    // A look that answers with an export under way since (the
    // queue's, say): not started, and said.
    HELD.with(|h| *h.borrow_mut() = Some(Vec::new()));
    state.borrow_mut().work.exporting = None;
    app.invoke_export();
    state.borrow_mut().work.exporting = Some(set);
    let held = HELD.with(|h| h.borrow_mut().take()).expect("held");
    for answer in held {
        answer(&app);
    }
    assert_eq!(
        app.get_status(),
        "not exported: another export is under way"
    );
    crate::testing::remove_dir_retry(&dir);
}

/// The Folder row follows the frame, not its name: two frames of one
/// name in two folders show two folders.
#[test]
fn the_folder_row_follows_the_frame_not_its_name() {
    let app = crate::testing::window(2);
    let files = vec![
        PathBuf::from("/nowhere/card-one/IMG_0001.CR3"),
        PathBuf::from("/nowhere/card-two/IMG_0001.CR3"),
    ];
    let (_state, _worker) = crate::testing::state_for(&app, files);
    app.invoke_select(0);
    app.set_export_open(true);
    let folder = || crate::testing::label_of(&app, "ExportSheet::folder-line-text");
    assert_eq!(folder(), "/nowhere/card-one");
    app.invoke_select(1);
    assert_eq!(app.get_file_name(), "IMG_0001.CR3");
    assert_eq!(folder(), "/nowhere/card-two");
}

/// A probe name taken already is passed over, never read as the
/// folder taking files, and no probe is left behind.
#[test]
fn the_folder_check_leaves_nothing_and_assumes_nothing() {
    let dir = crate::testing::scratch_dir("export-probe");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(folder_problem(&dir), None);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing left");
    // The first name taken, by a check a crash left: the second.
    let left = dir.join("left-by-a-crash");
    std::fs::write(&left, b"kept").unwrap();
    let names = |attempt: u32| match attempt {
        0 => "left-by-a-crash".to_string(),
        _ => "second".to_string(),
    };
    assert_eq!(folder_problem_probing(&dir, names), None);
    assert!(!dir.join("second").exists(), "removed");
    // Every name taken: not read as the folder taking files.
    assert_eq!(
        folder_problem_probing(&dir, |_| "left-by-a-crash".to_string()),
        Some(format!("{} cannot be written to", dir.display()))
    );
    // And the file that was there is left as it was.
    assert_eq!(std::fs::read(&left).unwrap(), b"kept");
    crate::testing::remove_dir_retry(&dir);
}

/// The sheet takes the keys: Enter is Export, Escape Cancel. Enter
/// is nothing while Export is off, and a focused field keeps its
/// own: the preset's name saves it, and once that row goes Enter
/// exports again.
#[test]
fn enter_exports_and_escape_cancels_unless_a_field_has_it() {
    use crate::testing::{click, press, type_text};
    use slint::platform::Key;
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    state.borrow_mut().window.batch = true;
    app.invoke_select(0);
    let exports = Rc::new(RefCell::new(0));
    let seen = exports.clone();
    app.on_export(move || *seen.borrow_mut() += 1);
    let saved = Rc::new(RefCell::new(Vec::<String>::new()));
    let seen = saved.clone();
    app.on_export_preset_saved(move |n| seen.borrow_mut().push(n.to_string()));
    let at = |id: &str| {
        use i_slint_backend_testing::ElementHandle;
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let e = ElementHandle::find_by_element_id(&app, id)
            .next()
            .unwrap_or_else(|| panic!("no {id}"));
        let (p, s) = (e.absolute_position(), e.size());
        (p.x + s.width / 2.0, p.y + s.height / 2.0)
    };

    app.set_export_open(true);
    press(&app, Key::Return);
    assert!(!app.get_export_open());
    assert_eq!(*exports.borrow(), 1);

    app.set_export_open(true);
    press(&app, Key::Escape);
    assert!(!app.get_export_open());
    assert_eq!(*exports.borrow(), 1);

    // A subfolder the export refuses: Export is off, and so is
    // Enter.
    app.set_export_subfolder("../out".into());
    app.set_export_open(true);
    press(&app, Key::Return);
    assert!(app.get_export_open());
    assert_eq!(*exports.borrow(), 1);
    app.set_export_subfolder("".into());

    // The subfolder field focused: its Enter, not Export's.
    let (x, y) = at("ExportSheet::subfolder-field");
    click(&app, x + 120.0, y);
    press(&app, Key::Return);
    assert!(app.get_export_open());
    assert_eq!(*exports.borrow(), 1);
    press(&app, Key::Escape);
    assert!(!app.get_export_open());

    // The name row: Enter saves the preset and exports nothing,
    // then the keys are the sheet's and Enter exports.
    app.set_export_open(true);
    let (x, y) = at("ExportSheet::save-as-button");
    click(&app, x, y);
    type_text(&app, "Web");
    press(&app, Key::Return);
    assert_eq!(*saved.borrow(), ["Web"]);
    assert!(app.get_export_open());
    assert_eq!(*exports.borrow(), 1);
    press(&app, Key::Return);
    assert!(!app.get_export_open());
    assert_eq!(*exports.borrow(), 2);
}

/// The sheet says, under its Sharpen row, that a frame with an Output
/// sharpen goes by it instead: only when the export is made smaller,
/// the row is not Off, and a frame to be exported has one that acts.
#[test]
fn the_sheet_says_an_output_sharpen_takes_its_sharpens_place() {
    use i_slint_backend_testing::ElementHandle;
    let app = crate::testing::window(2);
    let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    app.set_frame_width(4000);
    app.set_frame_height(3000);
    app.set_export_size("1024".into());
    app.set_export_sharpen("Standard".into());
    let said = |app: &crate::App| {
        crate::testing::settle(app);
        ElementHandle::find_by_element_id(app, "ExportSheet::output-sharpen-words").count()
    };
    let reopen = |app: &crate::App| {
        app.set_export_open(false);
        crate::testing::settle(app);
        app.set_export_open(true);
    };
    reopen(&app);
    assert_eq!(said(&app), 0, "no Output sharpen");
    app.set_output_sharpen_amount(0.6);
    reopen(&app);
    assert_eq!(said(&app), 1, "the frame's Output sharpen acts");
    app.set_export_sharpen("Off".into());
    assert_eq!(said(&app), 0, "the row Off");
    app.set_export_sharpen("Standard".into());
    app.set_export_size("Full".into());
    assert_eq!(said(&app), 0, "the export at full size");
    app.set_export_size("1024".into());
    app.set_output_sharpen_enabled(false);
    reopen(&app);
    assert_eq!(said(&app), 0, "the section off");
}

/// A field or a slider on the sheet hands the keys back to the
/// sheet, not the window: Enter after a typed Quality, or with
/// the slider focused, exports; Tab keeps its place. Escape answers the open question
/// first: the name row's Cancel, the queue's no.
#[test]
fn the_sheets_fields_and_sliders_hand_enter_back_to_the_sheet() {
    use crate::testing::{click, press, type_text};
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::Key;
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    state.borrow_mut().window.batch = true;
    app.invoke_select(0);
    let exports = Rc::new(RefCell::new(0));
    let seen = exports.clone();
    app.on_export(move || *seen.borrow_mut() += 1);
    let quality = || {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let e = ElementHandle::find_by_element_id(&app, "ExportSheet::quality-slider")
            .next()
            .expect("the Quality slider");
        (e.absolute_position(), e.size())
    };

    app.set_export_format("JPEG".into());
    app.set_export_quality(92.0);
    app.set_export_open(true);
    // The reading, at the slider's right end, typed over.
    let (p, s) = quality();
    click(&app, p.x + s.width - 12.0, p.y + s.height / 2.0);
    type_text(&app, "85");
    press(&app, Key::Return);
    assert_eq!(app.get_export_quality(), 85.0);
    assert!(app.get_export_open());
    assert_eq!(*exports.borrow(), 0);
    press(&app, Key::Return);
    assert!(!app.get_export_open());
    assert_eq!(*exports.borrow(), 1);

    // The slider's track clicked: Enter is not its own, and comes
    // to the sheet.
    app.set_export_open(true);
    let (p, s) = quality();
    click(&app, p.x + 90.0, p.y + s.height / 2.0);
    press(&app, Key::Return);
    assert!(!app.get_export_open());
    assert_eq!(*exports.borrow(), 2);

    // Tab and Space keep their places: from the slider, Tab past
    // the size, sharpening and both rows of color space choices to
    // the toggle, Space flips it, and Tab goes on past the metadata
    // choice to Subfolder.
    app.set_export_open(true);
    let embed = app.get_export_embed();
    let (p, s) = quality();
    click(&app, p.x + 90.0, p.y + s.height / 2.0);
    for _ in 0..5 {
        press(&app, Key::Tab);
    }
    press(&app, " ");
    assert_eq!(app.get_export_embed(), !embed, "Space on the toggle");
    press(&app, Key::Tab);
    press(&app, Key::Tab);
    type_text(&app, "web");
    assert_eq!(app.get_export_subfolder(), "web");
    app.set_export_subfolder("".into());
    app.set_export_open(false);

    // Escape with the name row up is its Cancel; then the sheet's.
    app.set_export_open(true);
    let (x, y) = {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let e = ElementHandle::find_by_element_id(&app, "ExportSheet::save-as-button")
            .next()
            .expect("Save as...");
        let (p, s) = (e.absolute_position(), e.size());
        (p.x + s.width / 2.0, p.y + s.height / 2.0)
    };
    let names = || crate::testing::count_id(&app, "ExportSheet::name-field");
    let closed = names();
    click(&app, x, y);
    assert_eq!(names(), closed + 1, "the name row");
    press(&app, Key::Escape);
    assert!(app.get_export_open());
    assert_eq!(names(), closed);
    press(&app, Key::Escape);
    assert!(!app.get_export_open());

    // And with the queue's Clear asked, its no.
    app.set_export_queue_sets(1);
    app.set_export_open(true);
    let asked = || crate::testing::count_id(&app, "ExportSheet::clear-question");
    let (p, s) = shown_id(&app, "ExportSheet::queue-clear-button").expect("the queue's Clear");
    click(&app, p.x + s.width / 2.0, p.y + s.height / 2.0);
    assert_eq!(asked(), 1);
    // It counts the one set.
    assert_eq!(
        crate::testing::label_of(&app, "ExportSheet::clear-question"),
        crate::tr!("Clear the queue of {n} set?" | "Clear the queue of {n} sets?" % 1)
    );
    press(&app, Key::Escape);
    assert!(app.get_export_open());
    assert_eq!(asked(), 0);
    press(&app, Key::Escape);
    assert!(!app.get_export_open());
    assert_eq!(*exports.borrow(), 2);
}

/// The preset list keeps its own keys: Enter on it, reached by
/// Tab, opens the list and exports nothing; an arrow and Enter in
/// the open list picks, and exports nothing either.
#[test]
fn the_preset_list_keeps_enter() {
    use crate::testing::{click, press};
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::Key;
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    state.borrow_mut().window.batch = true;
    app.invoke_select(0);
    let exports = Rc::new(RefCell::new(0));
    let seen = exports.clone();
    app.on_export(move || *seen.borrow_mut() += 1);
    let names: Vec<slint::SharedString> = vec!["None".into(), "Web".into()];
    app.set_export_presets(ModelRc::new(VecModel::from(names)));
    app.set_export_preset("None".into());

    app.set_export_open(true);
    press(&app, Key::Tab);
    press(&app, Key::Return);
    assert!(app.get_export_open(), "the list's Enter");
    assert_eq!(*exports.borrow(), 0);
    app.set_export_open(false);

    app.set_export_open(true);
    let (x, y) = {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let e = ElementHandle::find_by_element_id(&app, "ExportSheet::presets")
            .next()
            .expect("the preset list");
        let (p, s) = (e.absolute_position(), e.size());
        (p.x + s.width / 2.0, p.y + s.height / 2.0)
    };
    click(&app, x, y);
    press(&app, Key::DownArrow);
    press(&app, Key::Return);
    assert!(app.get_export_open(), "the open list's Enter");
    assert_eq!(*exports.borrow(), 0);
}

/// What an open of a raw says of it, as the worker says it: a frame
/// of `model`, `width` wide, shot under `as_shot` through a lens and
/// with a shot line named for the model.
fn opened_raw(generation: u64, model: &str, width: usize, as_shot: (f64, f64)) -> Outcome {
    Outcome::Opened {
        generation,
        as_shot,
        frame: Some(Arc::new(greycard_core::raw::RawFrame {
            model: model.into(),
            width,
            ..crate::panel::viewport::tests::daylight_frame()
        })),
        profile: Some(Box::new(
            greycard_core::color::profile_from_frame(
                &crate::panel::viewport::tests::daylight_frame(),
            )
            .expect("the frame's profile"),
        )),
        lens: LensReport::Found {
            lens: format!("{model} lens"),
            camera: true,
            smaller_sensor: false,
            carries: greycard_engine::lens::Carries::ALL,
        },
        lens_options: Vec::new(),
        kind: SourceKind::Raw,
        shot: greycard_core::raw::ShotSummary {
            camera: format!("Test {model}"),
            exposure: "50 mm".into(),
        },
        taken: None,
        blend: None,
    }
}

/// An open never walks the camera profile directory: every open
/// looks at it for a change off the window's thread, naming the
/// files of the profiles on show and the chosen one, and the CAMERA
/// PROFILE section opening walks it. The camera filter runs once
/// for an open however often the panel is shown after it, and the
/// log says a camera's hidden count once while the list stands.
#[test]
fn an_open_walks_no_profile_directory_and_filters_once() {
    use crate::panel::assets::{PROFILE_FILTERS, PROFILE_LOOKED, PROFILE_LOOKS, set_profiles};
    use greycard_edit_io::profiles::walks_here;
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    let open = |i: i32, model: &str| {
        app.invoke_select(i);
        let generation = state.borrow().develop.generation;
        deliver(&app, opened_raw(generation, model, 2, (5000.0, 0.0)));
        deliver(&app, developed(generation, (60, 40)));
    };
    let shown_again = || {
        let st = state.borrow();
        crate::panel::edit::show_edit(&st, &st.develop.edit, &app, st.develop.target);
    };
    // The first open may walk, when nothing is held yet.
    open(0, "Zero");
    let (walks, looks) = (walks_here(), PROFILE_LOOKS.get());
    open(1, "Zero");
    open(0, "Zero");
    open(2, "Two");
    assert_eq!(PROFILE_LOOKS.get(), looks + 3, "a look at each open");
    assert!(!PROFILE_LOOKED.with(|l| l.borrow().0), "not by force");
    assert_eq!(walks_here(), walks, "nothing changed, nothing walked");
    app.invoke_camera_profiles_wanted();
    assert_eq!(PROFILE_LOOKS.get(), looks + 4, "the section opened");
    assert!(PROFILE_LOOKED.with(|l| l.borrow().0), "by force");
    assert_eq!(walks_here(), walks + 1, "the section walks");

    // Other cameras' profiles, for the filter to hide, and one for
    // the camera Two.
    let entry = |name: &str, camera: &str| greycard_edit::camera::Entry {
        name: name.into(),
        path: std::path::PathBuf::from(format!("{name}.dcp")),
        title: None,
        unique_camera_model: Some(camera.into()),
        copyright: None,
        same_as: None,
        tone_curve: false,
    };
    let entries = vec![
        entry("A", "Other Camera"),
        entry("B", "Other Camera"),
        entry("Mine", "Test Two"),
    ];
    set_profiles(&mut state.borrow_mut(), entries.clone());
    let (filters, logged) = PROFILE_FILTERS.get();
    let since = || {
        let (f, l) = PROFILE_FILTERS.get();
        (f - filters, l - logged)
    };
    shown_again();
    shown_again();
    assert_eq!(since(), (1, 1), "Two, filtered and said once");
    open(0, "Zero");
    shown_again();
    assert_eq!(since(), (2, 2), "Zero, once");
    open(2, "Two");
    shown_again();
    assert_eq!(since(), (3, 2), "Two again: filtered, not said again");
    // The open named the file of the profile on show for Two.
    assert_eq!(
        PROFILE_LOOKED.with(|l| l.borrow().1.clone()),
        [std::path::PathBuf::from("Mine.dcp")]
    );
    set_profiles(&mut state.borrow_mut(), entries);
    shown_again();
    assert_eq!(since(), (4, 3), "a new list: said again");
    assert_eq!(walks_here(), walks + 1);
}

/// The panel with no frame open says no hidden count in the log:
/// there is no camera to say it for.
#[test]
fn no_hidden_count_is_said_with_no_frame_open() {
    use crate::panel::assets::{PROFILE_FILTERS, set_profiles};
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let other = greycard_edit::camera::Entry {
        name: "A".into(),
        path: Default::default(),
        title: None,
        unique_camera_model: Some("Other Camera".into()),
        copyright: None,
        same_as: None,
        tone_curve: false,
    };
    set_profiles(&mut state.borrow_mut(), vec![other]);
    let (_, logged) = PROFILE_FILTERS.get();
    {
        let st = state.borrow();
        assert!(st.develop.camera.1.is_empty());
        crate::panel::assets::show_profiles(&st, &st.develop.edit.camera, &app);
    }
    assert_eq!(PROFILE_FILTERS.get().1, logged);
}

/// A develop of `generation` as the worker delivers one: a picture
/// of `size`.
fn developed(generation: u64, size: (u32, u32)) -> Outcome {
    Outcome::Developed {
        generation,
        turn: 0,
        kind: crate::finish::Source::SCENE,
        image: crate::worker::Developed::Halves(Arc::new(crate::worker::Halves {
            width: size.0,
            height: size.1,
            pixels: Vec::new(),
        })),
        guide: Arc::new(crate::finish::Guide::NONE),
        planes: None,
        white: crate::worker::WhiteBase::IDENTITY,
        seconds: 0.1,
        sharpen: None,
        sources: Vec::new(),
        learned: crate::worker::LearnedReport::Off,
        fills: crate::worker::FillReport::default(),
        settles: false,
    }
}

/// A slider moved and its develop asked, the debounce run out.
fn nudge(app: &App, exposure: f32) {
    app.set_exposure(exposure);
    app.invoke_develop_changed();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::DEBOUNCE_MS + 50,
    ));
}

/// What the window took of the open frame: its camera, its white,
/// its lens, and its shot.
fn taken(app: &App, state: &Rc<Shared>) -> Option<(String, (f32, f32), String, String)> {
    let st = state.borrow();
    let (frame, _) = st.develop.frame.as_ref()?;
    Some((
        frame.model.clone(),
        st.develop.as_shot_white?,
        app.get_lens_profile().to_string(),
        app.get_shot_camera().to_string(),
    ))
}

/// A frame stepped to and a slider nudged at once: the develop is
/// asked while the frame decodes, and the generation moves on from
/// the open's. What the open says of the frame (its camera and
/// mosaic, its As shot white, its lens, its shot) is taken all the
/// same, so As shot, the resets, Auto, and the dropper read this
/// frame and not the last; the open's own develop, of the edit
/// before the nudge, is not shown, and the nudge's is.
#[test]
fn an_open_says_its_frame_though_a_develop_was_asked_as_it_decoded() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(0);
    deliver(
        &app,
        opened_raw(generation(&state), "Zero", 2, (5000.0, 0.0)),
    );
    deliver(&app, developed(generation(&state), (60, 40)));
    assert_eq!(
        taken(&app, &state),
        Some((
            "Zero".into(),
            (5000.0, 0.0),
            "Zero lens".into(),
            "Test Zero".into()
        ))
    );

    app.invoke_select(1);
    let open = generation(&state);
    assert_eq!(state.borrow().develop.as_shot_white, None, "not known yet");
    nudge(&app, 0.7);
    let develop = generation(&state);
    assert!(develop > open, "the nudge's develop is asked");
    // The worker's order: the open, its develop, the nudge's.
    deliver(&app, opened_raw(open, "One", 4, (3200.0, 0.004)));
    assert_eq!(
        taken(&app, &state),
        Some((
            "One".into(),
            (3200.0, 0.004),
            "One lens".into(),
            "Test One".into()
        ))
    );
    assert!(app.get_raw_input());
    assert_eq!(generation(&state), develop, "no develop asked again");
    deliver(&app, developed(open, (60, 40)));
    assert_ne!(
        state
            .borrow()
            .develop
            .pending
            .as_ref()
            .map(|l| l.generation),
        Some(open),
        "the open's own develop is not the edit on the panel"
    );
    deliver(&app, developed(develop, (80, 50)));
    let st = state.borrow();
    assert_eq!(
        st.develop.pending.as_ref().map(|l| l.generation),
        Some(develop)
    );
    assert_eq!(st.develop.shown_size, (80, 50));
    assert_eq!(st.develop.edit.light.exposure, 0.7);
}

/// Two steps in quick succession: the first frame's open, landing
/// after the second was chosen, says nothing of the second, a
/// develop asked or not; the second's open says it. Nor does the
/// first open of a frame chosen again since, whose open went out
/// again: the frame is the same, the visit is not.
#[test]
fn an_open_of_a_frame_left_says_nothing() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(0);
    let first = generation(&state);
    app.invoke_select(1);
    let second = generation(&state);
    deliver(&app, opened_raw(first, "Zero", 2, (5000.0, 0.0)));
    assert_eq!(taken(&app, &state), None, "frame 0's open, frame 1 chosen");
    nudge(&app, 0.3);
    deliver(&app, opened_raw(first, "Zero", 2, (5000.0, 0.0)));
    assert_eq!(taken(&app, &state), None, "nor with a develop asked since");
    deliver(&app, opened_raw(second, "One", 4, (3200.0, 0.0)));
    assert_eq!(
        taken(&app, &state).map(|t| t.0),
        Some("One".into()),
        "frame 1's own, behind the develop"
    );

    // Back to frame 0, and on, and back again before either lands.
    app.invoke_select(0);
    let again = generation(&state);
    app.invoke_select(1);
    app.invoke_select(0);
    let last = generation(&state);
    nudge(&app, 0.5);
    for stale in [again, again + 1] {
        deliver(&app, opened_raw(stale, "Zero", 2, (4100.0, 0.0)));
        assert_eq!(state.borrow().develop.as_shot_white, None, "{stale}");
    }
    deliver(&app, opened_raw(last, "Zero", 2, (4500.0, 0.0)));
    assert_eq!(state.borrow().develop.as_shot_white, Some((4500.0, 0.0)));
}

/// Another version of the open file is another frame: an open of
/// the file's own line says nothing of a virtual copy chosen since.
#[test]
fn an_open_of_another_version_of_the_file_is_not_the_frame_s() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    nudge(&app, 0.4);
    let mut st = state.borrow_mut();
    let (sent, (path, version)) = st.develop.open_sent.clone().expect("the open sent");
    assert_eq!((&path, &version), (st.path_of(0), &None));
    assert!(crate::panel::browser::opened_is_current(&st, sent));
    st.develop.open_sent = Some((sent, (path, Some("v1".into()))));
    assert!(!crate::panel::browser::opened_is_current(&st, sent));
    // At the generation waited on it is the frame's, as ever.
    let now = st.develop.generation;
    assert!(crate::panel::browser::opened_is_current(&st, now));
}

/// A fresh raw, frame 0, chosen, and the learned denoiser's tier set
/// to `learned` on the panel as it decodes (with an exposure, one
/// develop); then its open, behind that develop, says the blend it
/// seeded. The generations of the open and the develop.
fn open_seeded_behind(
    app: &App,
    state: &Rc<Shared>,
    learned: greycard_edit::Learned,
) -> (u64, u64) {
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(0);
    let open = generation(state);
    app.set_denoise_learned(learned.name().into());
    nudge(app, 0.2);
    let develop = generation(state);
    let mut outcome = opened_raw(open, "Zero", 2, (5000.0, 0.0));
    if let Outcome::Opened { blend, .. } = &mut outcome {
        *blend = Some(0.35);
    }
    app.set_status("".into());
    deliver(app, outcome);
    (open, develop)
}

/// A raw opened fresh has its learned blend seeded from its ISO by
/// the open. A develop asked while it decoded went without it: the
/// edit, seeded, is developed again, said as any develop is, and
/// that develop is the one shown.
#[test]
fn a_blend_seeded_behind_a_develop_is_developed_again() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    let (_, develop) = open_seeded_behind(&app, &state, greycard_edit::Learned::Fast);
    let again = generation(&state);
    assert!(again > develop, "developed again");
    assert_eq!(app.get_status(), "developing...");
    assert_eq!(state.borrow().develop.edit.noise.learned_strength, 0.35);
    assert_eq!(
        state
            .borrow()
            .line(0)
            .map(|l| l.current().noise.learned_strength),
        Some(0.35)
    );
    deliver(&app, developed(develop, (60, 40)));
    assert_ne!(
        state
            .borrow()
            .develop
            .pending
            .as_ref()
            .map(|l| l.generation),
        Some(develop),
        "the unseeded develop"
    );
    deliver(&app, developed(again, (60, 40)));
    assert_eq!(
        state
            .borrow()
            .develop
            .pending
            .as_ref()
            .map(|l| l.generation),
        Some(again)
    );
}

/// With the learned denoiser off the seeded blend is not used: the
/// develop in flight is the edit's as it is, and none is asked again.
#[test]
fn a_blend_seeded_behind_a_develop_with_the_denoiser_off_develops_nothing_more() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let (_, develop) = open_seeded_behind(&app, &state, greycard_edit::Learned::Off);
    assert_eq!(state.borrow().develop.generation, develop);
    assert_eq!(state.borrow().develop.edit.noise.learned_strength, 0.35);
}

/// The frame left and come back to with no open of it between (a
/// pick still reading its sidecar, there and back): the last open's
/// visit is over, and its late word is not taken for this one,
/// which waits on an open of its own.
#[test]
fn a_return_without_an_open_takes_no_word_of_the_last_visit() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    let open = state.borrow().develop.generation;
    {
        // What a pick standing in does to the frame chosen and the
        // generation, there and back.
        let mut st = state.borrow_mut();
        for c in [1, 0] {
            st.current = Some(c);
            crate::panel::browser::current_moved(&mut st);
            st.develop.generation += 1;
        }
        assert_eq!(st.develop.open_sent, None, "the visit is over");
    }
    deliver(&app, opened_raw(open, "Zero", 2, (5000.0, 0.0)));
    assert_eq!(taken(&app, &state), None);
    // The list renumbered under the same frame keeps the visit.
    app.invoke_select(1);
    let open = state.borrow().develop.generation;
    nudge(&app, 0.3);
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::current_moved(&mut st);
        assert!(st.develop.open_sent.is_some(), "the same frame");
    }
    deliver(&app, opened_raw(open, "One", 4, (3200.0, 0.0)));
    assert_eq!(taken(&app, &state).map(|t| t.0), Some("One".into()));
}

/// Culling entered while the frame decodes takes no word of its
/// open (and so no offer of the models, which the open's landing
/// makes): as before, culling waits on the open its leaving sends.
#[test]
fn culling_entered_as_a_frame_decodes_takes_no_word_of_its_open() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    let open = state.borrow().develop.generation;
    crate::panel::cull::enter_cull(&mut state.borrow_mut(), &app, 0);
    assert!(state.borrow().develop.generation > open);
    assert!(!crate::panel::browser::opened_is_current(
        &state.borrow(),
        open
    ));
    deliver(&app, opened_raw(open, "Zero", 2, (5000.0, 0.0)));
    assert_eq!(taken(&app, &state), None);
}

/// A file followed to another path under the window (found moved,
/// an archive's copy) while the worker holds it under the old one:
/// no develop is asked under a path the worker does not hold (it
/// would make nothing, and the window would wait on it), the frame
/// is opened again under the new path, every path the worker keeps
/// of it set by that one open, and from then on it develops. The
/// old open's late word is not taken; what a job then running says
/// under the old path is the frame's.
#[test]
fn a_frame_whose_file_moved_is_opened_again_under_its_new_path() {
    let app = crate::testing::window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    assert!(worker.stop(), "what is sent stays in the slot");
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(0);
    let (was, open) = (state.borrow().path_of(0).clone(), generation(&state));
    let to = std::path::PathBuf::from("/nowhere/moved/IMG_0000.CR3");
    state.borrow_mut().move_file(0, to.clone());
    {
        let st = state.borrow();
        assert_eq!(st.develop.open_sent, None, "the open was of the old path");
        assert!(st.develop.reopen_due);
        assert!(!crate::panel::edit::develops_here(&st));
        assert!(crate::panel::browser::is_open_file(&st, &was));
        assert!(crate::panel::browser::is_open_file(&st, &to));
    }
    nudge(&app, 0.4);
    assert_eq!(
        generation(&state),
        open,
        "no develop under the new path yet"
    );
    assert_eq!(
        worker.slot_by_test(),
        Some(("open", open, Some(was.clone())))
    );
    assert!(crate::panel::browser::reopen_moved(
        &mut state.borrow_mut(),
        &app,
        &worker
    ));
    let again = generation(&state);
    assert_eq!(
        worker.slot_by_test(),
        Some(("open", again, Some(to.clone())))
    );
    assert!(crate::panel::edit::develops_here(&state.borrow()));
    // The old open's word, behind the open again: not taken.
    deliver(&app, opened_raw(open, "Zero", 2, (5000.0, 0.0)));
    assert_eq!(taken(&app, &state), None);
    deliver(&app, opened_raw(again, "Zero", 2, (5000.0, 0.0)));
    assert_eq!(taken(&app, &state).map(|t| t.0), Some("Zero".into()));
    nudge(&app, 0.6);
    let develop = generation(&state);
    assert!(develop > again);
    assert_eq!(
        worker.slot_by_test(),
        Some(("open", develop, Some(to.clone())))
    );
    // Another frame: the move is not its.
    app.invoke_select(1);
    assert!(!crate::panel::browser::is_open_file(&state.borrow(), &was));
}

/// The learned masks follow their frame's file: a mask asked before
/// the move and made on the old path, landing after it, is taken;
/// so is one made under the new path (the review's probe); one of
/// another file is not.
#[test]
fn a_learned_mask_follows_its_frame_s_file() {
    use greycard_edit::brush::Raster;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.set_panel_tab("Masks".into());
    app.invoke_new_mask();
    app.invoke_add_shape("Subject".into(), "Add".into());
    let id = state.borrow().develop.edit.adjustments[0].id;
    let was = state.borrow().path_of(0).clone();
    state
        .borrow_mut()
        .develop
        .asked
        .insert((id, 0), Shape::Subject {});
    let to = std::path::PathBuf::from("/nowhere/moved/IMG_0000.CR3");
    state.borrow_mut().move_file(0, to.clone());
    assert_eq!(state.borrow().develop.learned_of.as_ref(), Some(&to));
    let made = |file: &std::path::Path, v: u8| Outcome::Mask {
        key: (id, 0),
        shape: Shape::Subject {},
        file: Some(file.to_path_buf()),
        raster: Arc::new(Raster::from_data(1.5, 16, vec![v; 16 * 11])),
        provider: Some("CPU"),
        seconds: 0.1,
        note: None,
        part: None,
    };
    let held = |state: &Rc<Shared>| {
        state
            .borrow()
            .develop
            .learned
            .get(&(id, 0))
            .map(|(_, r)| r.data()[0])
    };
    deliver(&app, made(&was, 7));
    assert_eq!(held(&state), Some(7), "asked before, made on the old path");
    assert!(state.borrow().develop.asked.is_empty());
    deliver(&app, made(&to, 8));
    assert_eq!(held(&state), Some(8), "made under the new path");
    deliver(&app, made(std::path::Path::new("/nowhere/other.CR3"), 9));
    assert_eq!(held(&state), Some(8), "another file's");
}

/// A frame whose open failed, its file then written again under the
/// window: the re-read sends an open of it, which the worker takes
/// as the frame's (`worker::tests::a_failed_open_read_again_develops`).
#[test]
fn a_frame_whose_open_failed_is_read_again() {
    let app = crate::testing::window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    assert!(worker.stop());
    app.invoke_select(0);
    let open = state.borrow().develop.generation;
    deliver(
        &app,
        Outcome::Failed {
            generation: open,
            message: "junk".into(),
        },
    );
    let path = state.borrow().path_of(0).clone();
    assert!(crate::panel::browser::reread_open(
        &mut state.borrow_mut(),
        &app,
        &worker
    ));
    let again = state.borrow().develop.generation;
    assert_eq!(worker.slot_by_test(), Some(("open", again, Some(path))));
}

/// Two moves in a row (A to B to C, then back to A), each followed
/// and opened again: every earlier path is the frame's for the
/// visit. A mask made on A or on B is taken; a mosaic read on A is
/// known by the frame's path now; a merge's answer on A shows its
/// row. Another frame chosen, none of them is.
#[test]
fn every_earlier_path_of_the_frame_is_its_for_the_visit() {
    use greycard_edit::brush::Raster;
    let app = crate::testing::window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    assert!(worker.stop());
    state.borrow_mut().window.write_sidecars = true;
    app.invoke_select(0);
    app.set_panel_tab("Masks".into());
    app.invoke_new_mask();
    app.invoke_add_shape("Subject".into(), "Add".into());
    let id = state.borrow().develop.edit.adjustments[0].id;
    let paths = crate::testing::open_and_follow(
        &mut state.borrow_mut(),
        &app,
        &worker,
        &["/nowhere/B.CR3", "/nowhere/C.CR3"],
    );
    let made = |file: &std::path::Path, v: u8| Outcome::Mask {
        key: (id, 0),
        shape: Shape::Subject {},
        file: Some(file.to_path_buf()),
        raster: Arc::new(Raster::from_data(1.5, 16, vec![v; 16 * 11])),
        provider: Some("CPU"),
        seconds: 0.1,
        note: None,
        part: None,
    };
    let held = |state: &Rc<Shared>| {
        state
            .borrow()
            .develop
            .learned
            .get(&(id, 0))
            .map(|(_, r)| r.data()[0])
    };
    deliver(&app, made(&paths[0], 1));
    assert_eq!(held(&state), Some(1), "made on A");
    deliver(&app, made(&paths[1], 2));
    assert_eq!(held(&state), Some(2), "made on B");
    let mosaic = greycard_core::raw::CfaPattern::rggb().mosaic();
    deliver(
        &app,
        Outcome::Mosaic {
            path: paths[0].clone(),
            mosaic: Some(mosaic),
        },
    );
    assert!(state.borrow().develop.mosaics.contains_key(&paths[2]));
    app.set_merge_again_shown(false);
    deliver(
        &app,
        Outcome::MergeAgain {
            path: paths[0].clone(),
            can: true,
        },
    );
    assert!(app.get_merge_again_shown());
    // Back to A: B is still the frame's.
    {
        let mut st = state.borrow_mut();
        st.move_file(0, paths[0].clone());
        assert!(crate::panel::browser::reopen_moved(&mut st, &app, &worker));
        for p in &paths {
            assert!(crate::panel::browser::is_open_file(&st, p), "{p:?}");
        }
    }
    deliver(&app, made(&paths[1], 3));
    assert_eq!(held(&state), Some(3), "made on B, back at A");
    app.invoke_select(1);
    let st = state.borrow();
    for p in &paths[1..] {
        assert!(!crate::panel::browser::is_open_file(&st, p), "{p:?}");
    }
    assert!(st.develop.moved_from.is_none());
}

/// The frame's file followed while culling: the open again is
/// refused there (culling develops nothing) and no develop is asked;
/// leaving culling sends the open, under the new path.
#[test]
fn a_move_in_culling_is_opened_by_leaving_culling() {
    let app = crate::testing::window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    assert!(worker.stop());
    app.invoke_select(0);
    let to = std::path::PathBuf::from("/nowhere/moved/IMG_0000.CR3");
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 0);
        st.move_file(0, to.clone());
        assert!(!crate::panel::browser::reopen_moved(&mut st, &app, &worker));
        assert_eq!(st.develop.open_sent, None);
        crate::panel::cull::leave_cull(&mut st, &app, &worker, None);
    }
    let st = state.borrow();
    let generation = st.develop.generation;
    assert_eq!(
        worker.slot_by_test(),
        Some(("open", generation, Some(to.clone())))
    );
    assert_eq!(
        st.develop
            .open_sent
            .as_ref()
            .map(|(g, (p, _))| (*g, p.clone())),
        Some((generation, to))
    );
}

/// As shot switched on before the frame's file moved gives the
/// custom white back when switched off after it.
#[test]
fn as_shot_off_after_a_move_gives_the_white_back() {
    let app = crate::testing::window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    let open = state.borrow().develop.generation;
    deliver(&app, opened_raw(open, "Zero", 2, (5000.0, 0.0)));
    app.set_as_shot(false);
    app.set_temperature(3000.0);
    app.set_tint(0.01);
    app.set_as_shot(true);
    app.invoke_as_shot_toggled();
    assert_eq!(app.get_temperature(), 5000.0);
    {
        let mut st = state.borrow_mut();
        st.move_file(0, "/nowhere/moved/IMG_0000.CR3".into());
        assert!(crate::panel::browser::reopen_moved(&mut st, &app, &worker));
    }
    app.set_as_shot(false);
    app.invoke_as_shot_toggled();
    assert_eq!((app.get_temperature(), app.get_tint()), (3000.0, 0.01));
}

/// A middle click's way back from 200% follows the frame's file: it
/// is still this frame's after a move.
#[test]
fn the_way_back_from_200_percent_follows_a_move() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    let to = std::path::PathBuf::from("/nowhere/moved/IMG_0000.CR3");
    let mut st = state.borrow_mut();
    let key = st.key_of(0).expect("frame 0");
    st.develop.zoom_back = Some(crate::owners::ZoomBack {
        frame: Some((key, (60, 40))),
        zoom: 0.5,
        free: false,
        center: (0.5, 0.5),
    });
    st.move_file(0, to.clone());
    let now = st.key_of(0).map(|k| (k, (60, 40)));
    assert_eq!(
        st.develop.zoom_back.as_ref().and_then(|b| b.frame.clone()),
        now
    );
    assert_eq!(now.map(|(k, _)| k.path), Some(to));
}

/// A develop's first round, a settle to follow under its generation:
/// the picture on screen is not the edit's at rest (the Original does
/// not keep it to put back), and a preset's or a sync's words stay
/// for the settle's landing, which then makes the picture whole.
#[test]
fn a_first_round_is_not_the_picture_at_rest_until_its_settle_lands() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let generation = state.borrow().develop.generation;
    deliver(&app, opened_raw(generation, "Zero", 2, (5000.0, 0.0)));
    nudge(&app, 0.7);
    let generation = state.borrow().develop.generation;
    let said = "Muted Slide onto 3 frames".to_string();
    state.borrow_mut().develop.status_after_develop = Some((generation, said.clone()));
    let whole = |state: &Rc<Shared>| crate::panel::original::edit_picture_whole(&state.borrow());
    let land = |state: &Rc<Shared>| {
        let mut st = state.borrow_mut();
        crate::panel::startup::land_pending(&mut st, &app);
    };
    let mut first = developed(generation, (60, 40));
    if let Outcome::Developed { settles, .. } = &mut first {
        *settles = true;
    }
    deliver(&app, first);
    land(&state);
    assert!(!whole(&state), "a first round is not put back");
    assert!(crate::panel::viewport::settling(&state.borrow()));
    assert_eq!(app.get_status().as_str(), said, "the words stay");
    deliver(&app, developed(generation, (60, 40)));
    land(&state);
    assert!(whole(&state), "the settle's is");
    assert!(!crate::panel::viewport::settling(&state.borrow()));
    assert_eq!(app.get_status().as_str(), said, "and the words with it");
    assert!(state.borrow().develop.status_after_develop.is_none());
}
/// An export pressed on a frame whose masks wait on a shape nothing can
/// measure (§724) takes nothing, alone or pressed in a set, and the set
/// is not exported in its place; a set it is one of leaves it out.
#[test]
fn a_frame_whose_masks_wait_on_its_shape_is_not_exported() {
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    state.borrow_mut().sidecars[0] = crate::testing::waiting_sidecar();
    let key = |c: usize| state.borrow().key_of(c).unwrap();
    assert!(
        pressed_frames(&state, &app, &key(0), &[0]).is_none(),
        "alone"
    );
    assert!(
        pressed_frames(&state, &app, &key(0), &[0, 1]).is_none(),
        "pressed in a set"
    );
    let (_, _, frames, _) = pressed_frames(&state, &app, &key(1), &[0, 1]).unwrap();
    let files: Vec<PathBuf> = frames.into_iter().map(|f| f.0).collect();
    assert_eq!(files, [state.borrow().files[1].clone()]);
    let (_, _, frames, _) = pressed_frames(&state, &app, &key(1), &[0, 1, 2]).unwrap();
    let files: Vec<PathBuf> = frames.into_iter().map(|f| f.0).collect();
    let st = state.borrow();
    assert_eq!(files, [st.files[1].clone(), st.files[2].clone()]);
}

/// A waiting frame its raw's header can measure is exported on the
/// first press (§724): measured as the press takes it, by the header
/// and not by its 16:9 thumbnail, and the set goes whole.
#[test]
fn a_waiting_frame_its_header_measures_is_exported_on_the_first_press() {
    let dir = std::env::temp_dir().join(format!("greycard-export-measured-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<PathBuf> = (0..2)
        .map(|i| dir.join(format!("IMG_{i:04}.tif")))
        .collect();
    for f in &files {
        greycard_library::fixture::write_frame_of(
            f,
            &greycard_library::fixture::R5,
            6,
            4,
            &[0; 6 * 4 * 3],
        );
    }
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.sidecars[0] = crate::testing::waiting_sidecar();
        st.views.thumb_base[0] = Some((16, 9, vec![0; 16 * 9 * 3]));
    }
    let key = state.borrow().key_of(0).unwrap();
    let (_, _, frames, _) = pressed_frames(&state, &app, &key, &[0, 1]).unwrap();
    let pressed: Vec<PathBuf> = frames.into_iter().map(|f| f.0).collect();
    assert_eq!(pressed, files);
    assert_eq!(
        state.borrow().sidecars[0].shape,
        greycard_edit::Measured::new(1, 1.0 / 1.5)
    );
    crate::testing::remove_scratch(state, &dir);
}
