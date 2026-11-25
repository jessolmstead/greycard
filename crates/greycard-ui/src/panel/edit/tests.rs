use slint::Model;

use super::*;
use crate::testing::window;

/// The profiled Strength slider runs over the edit crate's range,
/// and a value at its new low end goes into the edit as it is.
#[test]
fn the_profiled_strength_slider_runs_over_the_edit_s_range() {
    let text =
        std::fs::read_to_string(std::path::Path::new(greycard_slint::PANEL_DIR).join("edit.slint"))
            .unwrap();
    let slider = text
        .split("EditSlider {")
        .find(|s| s.contains("label: @tr(\"Strength\");") && s.contains("denoise-strength"))
        .expect("the profiled Strength slider");
    let field = |name: &str| -> f32 {
        let at = slider.find(&format!("{name}: ")).expect(name) + name.len() + 2;
        slider[at..]
            .split(';')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    assert_eq!(field("minimum"), greycard_edit::Noise::STRENGTH_MIN);
    assert_eq!(field("maximum"), greycard_edit::Noise::STRENGTH_MAX);

    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let mut edit = state.borrow().develop.edit.clone();
    edit.noise.strength = 0.1;
    show_edit(&state.borrow(), &edit, &app, Some(0));
    assert_eq!(app.get_denoise_strength(), 0.1);
    assert_eq!(read_edit(&app, &edit, Some(0)).noise.strength, 0.1);
}

/// The learned blend's chroma slider has one name on both sides,
/// and the slider reads and writes the edit's field.
#[test]
fn the_learned_chroma_slider_has_one_name_and_carries_the_field() {
    let app = window(1);
    assert_eq!(
        app.global::<crate::NoiseWords>().get_chroma(),
        greycard_edit::LEARNED_CHROMA
    );
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let mut edit = state.borrow().develop.edit.clone();
    edit.noise.learned_strength = 0.4;
    show_edit(&state.borrow(), &edit, &app, Some(0));
    assert_eq!(app.get_denoise_learned_chroma_strength(), 1.0);
    edit.noise.learned_chroma_strength = 0.3;
    show_edit(&state.borrow(), &edit, &app, Some(0));
    assert_eq!(app.get_denoise_learned_chroma_strength(), 0.3);
    app.set_denoise_learned_chroma_strength(0.65);
    let read = read_edit(&app, &edit, Some(0));
    assert_eq!(read.noise.learned_chroma_strength, 0.65);
}

/// The Detail section's widest band has one name on both sides:
/// the Slint's slider reads its global, the history and the copy
/// sheet the edit crate's constant. And its slider reads and
/// writes the edit's field, the global's and a mask's.
#[test]
fn structure_has_one_name_and_its_sliders_carry_it() {
    let app = window(1);
    assert_eq!(
        app.global::<crate::DetailWords>().get_structure(),
        greycard_edit::STRUCTURE
    );
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let mut edit = state.borrow().develop.edit.clone();
    edit.detail.structure = 0.35;
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        ..Default::default()
    });
    edit.adjustments[0].look.detail.structure = -0.6;
    show_edit(&state.borrow(), &edit, &app, Some(0));
    assert_eq!(app.get_detail_structure(), 0.35);
    assert_eq!(app.get_local_structure(), -0.6);
    let read = read_edit(&app, &edit, Some(0));
    assert_eq!(read.detail.structure, 0.35);
    assert_eq!(read.adjustments[0].look.detail.structure, -0.6);
}

/// The vignette's place is a choice under its sliders: a click on its
/// second half puts the vignette on the picture as shown, in the edit
/// the window reads, and a screen reader hears the option it is at;
/// an edit with it set shows it, and one without shows the scene.
#[test]
fn the_vignette_place_choice_carries_the_field() {
    use greycard_edit::vignette::Place;
    use i_slint_backend_testing::ElementHandle;
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 2400.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    app.set_collapsed_vignette(false);
    crate::testing::settle(&app);
    assert_eq!(app.get_vignette_place(), "Scene");
    let choice = || {
        crate::testing::settle(&app);
        ElementHandle::find_by_element_id(&app, "VignetteSection::vignette-place-choice")
            .next()
            .expect("the vignette's place is drawn")
    };
    let row = choice();
    let (at, size) = (row.absolute_position(), row.size());
    crate::testing::click(&app, at.x + size.width * 0.75, at.y + size.height / 2.0);
    crate::testing::settle(&app);
    assert_eq!(app.get_vignette_place(), "Display", "the click chose");
    let read = read_edit(&app, &state.borrow().develop.edit, Some(0));
    assert_eq!(read.vignette.place, Place::Display);
    let checked: Vec<String> = choice()
        .query_descendants()
        .find_all()
        .into_iter()
        .filter(|e| e.accessible_checked() == Some(true))
        .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
        .collect();
    assert_eq!(checked, ["Overlay"]);

    let mut edit = state.borrow().develop.edit.clone();
    edit.vignette.place = Place::Scene;
    show_edit(&state.borrow(), &edit, &app, Some(0));
    assert_eq!(app.get_vignette_place(), "Scene");
    assert_eq!(read_edit(&app, &edit, Some(0)).vignette.place, Place::Scene);
    edit.vignette.place = Place::Display;
    show_edit(&state.borrow(), &edit, &app, Some(0));
    assert_eq!(app.get_vignette_place(), "Display");
    assert_eq!(
        read_edit(&app, &edit, Some(0)).vignette.place,
        Place::Display
    );
}

/// One section open at a time: off, sections open side by side;
/// on, opening one, by its header or by the settings' restore,
/// folds the others of the tab and leaves the panes' alone.
#[test]
fn one_section_open_folds_the_tabs_others() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let settle = |app: &App| {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        slint::platform::update_timers_and_animations();
    };
    assert_eq!(app.get_panel_tab(), "Develop");
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    settle(&app);
    app.set_collapsed_light(false);
    app.set_collapsed_color(false);
    app.set_collapsed_history(false);
    settle(&app);
    assert!(
        !app.get_collapsed_light() && !app.get_collapsed_color(),
        "off: both open"
    );

    app.set_one_section(true);
    app.set_collapsed_grain(false);
    settle(&app);
    assert!(!app.get_collapsed_grain());
    assert!(
        app.get_collapsed_light() && app.get_collapsed_color(),
        "on: the rest fold"
    );
    assert!(
        !app.get_collapsed_history(),
        "the left pane's are not the panel's"
    );
    app.set_collapsed_light(false);
    settle(&app);
    assert!(!app.get_collapsed_light() && app.get_collapsed_grain());
    // Folding one opens nothing.
    app.set_collapsed_light(true);
    settle(&app);
    assert!(read_folds(&app).iter().any(|f| f == "grain"));

    // METADATA opened in the grid's pane shares its fold with the
    // panel's, which the grid covers: the panel behind stays.
    app.set_collapsed_grain(false);
    settle(&app);
    assert!(app.get_collapsed_metadata());
    app.set_grid_open(true);
    settle(&app);
    app.set_collapsed_metadata(false);
    settle(&app);
    assert!(!app.get_collapsed_metadata());
    assert!(
        !app.get_collapsed_grain(),
        "the grid's METADATA folded the panel"
    );
    // Back in Develop, an opening folds as before.
    app.set_grid_open(false);
    settle(&app);
    app.set_collapsed_light(false);
    settle(&app);
    assert!(app.get_collapsed_grain() && app.get_collapsed_metadata());
}

/// One section open at a time over a fold two tabs share: LIGHT
/// opened on a mask folds Develop's SHARPEN, unmade there, so
/// Develop comes back with LIGHT alone open (issue 143); METADATA
/// opened on Cull folds LIGHT the same way.
#[test]
fn a_fold_two_tabs_share_folds_the_other_tabs_sections() {
    use crate::testing::{click, settle};
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    app.set_one_section(true);
    settle(&app);
    let header = |section: &str| {
        let (at, size) = crate::testing::header(&app, section);
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        for _ in 0..6 {
            settle(&app);
        }
    };
    let open = || -> Vec<&str> {
        FOLDS
            .iter()
            .filter(|(_, get, _)| !get(&app))
            .map(|(n, _, _)| *n)
            .collect()
    };
    header("SharpenSection");
    app.set_adjustment_names(
        std::rc::Rc::new(slint::VecModel::from(vec![
            slint::SharedString::from("Global"),
            "Mask 1".into(),
        ]))
        .into(),
    );
    app.set_target(1);
    app.set_panel_tab("Masks".into());
    settle(&app);
    header("LightSection");
    app.set_panel_tab("Develop".into());
    settle(&app);
    assert_eq!(open(), ["light"], "Develop shows one open");

    app.set_panel_tab("Cull".into());
    settle(&app);
    app.set_collapsed_metadata(false);
    settle(&app);
    app.set_panel_tab("Develop".into());
    settle(&app);
    assert_eq!(open(), ["metadata"], "Develop shows one open");

    // And back the other way: LIGHT opened on Develop folds the
    // mask's MASK, METADATA opened on Develop Cull's CULLING.
    app.set_panel_tab("Masks".into());
    settle(&app);
    app.set_collapsed_mask(false);
    settle(&app);
    app.set_panel_tab("Develop".into());
    settle(&app);
    header("LightSection");
    app.set_panel_tab("Masks".into());
    settle(&app);
    assert_eq!(open(), ["light"], "Masks shows one open");

    app.set_panel_tab("Cull".into());
    settle(&app);
    app.set_collapsed_culling(false);
    settle(&app);
    app.set_panel_tab("Develop".into());
    settle(&app);
    app.set_collapsed_metadata(false);
    settle(&app);
    app.set_panel_tab("Cull".into());
    settle(&app);
    assert_eq!(open(), ["metadata"], "Cull shows one open");
}

/// A window for one section open at a time over the Masks tab: every
/// section folded but the masks' list, the rule on or off.
fn one_open_window(
    on: bool,
    files: Vec<std::path::PathBuf>,
) -> (App, Rc<Shared>, Rc<crate::worker::Worker>) {
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = crate::testing::state_for(&app, files);
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    app.set_collapsed_adjustments(false);
    app.set_one_section(on);
    crate::testing::settle(&app);
    (app, state, worker)
}

/// The sections open, by name, made or not.
fn open_folds(app: &App) -> Vec<&'static str> {
    FOLDS
        .iter()
        .filter(|(_, get, _)| !get(app))
        .map(|(n, _, _)| *n)
        .collect()
}

/// Settled after a click: the folds' animations run out.
fn settled(app: &App) {
    for _ in 0..6 {
        crate::testing::settle(app);
    }
}

fn click_header(app: &App, section: &str) {
    let (at, size) = crate::testing::header(app, section);
    crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    settled(app);
}

fn click_new_mask(app: &App) {
    let (at, size) = crate::testing::by_id(app, "AdjustmentsList::new-mask-button");
    crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    settled(app);
}

fn show_tab(app: &App, tab: &str) {
    crate::testing::click_panel_tab(app, tab);
    settled(app);
}

fn choose_mask(app: &App, i: usize) {
    crate::testing::click_key(app, "AdjustmentsList::mask-item", i);
    settled(app);
}

/// One section open at a time, and New mask with none chosen while
/// LIGHT is open: MASK is made open with LIGHT, and MASK, asked for,
/// is the one open (issue 211), then and after the mask is chosen
/// again.
#[test]
fn new_mask_with_none_chosen_opens_mask_alone() {
    let (app, _state, _worker) = one_open_window(true, Vec::new());
    click_header(&app, "LightSection");
    show_tab(&app, "Masks");
    click_new_mask(&app);
    assert_eq!(open_folds(&app), ["adjustments", "mask"]);
    choose_mask(&app, 0);
    choose_mask(&app, 1);
    assert_eq!(open_folds(&app), ["adjustments", "mask"], "chosen again");
}

/// The other way round: a mask made first, then LIGHT opened by its
/// header on the Masks tab folds MASK, and MASK opened again folds
/// LIGHT (issue 211).
#[test]
fn light_opened_after_a_mask_is_made_folds_mask() {
    let (app, _state, _worker) = one_open_window(true, Vec::new());
    show_tab(&app, "Masks");
    click_new_mask(&app);
    assert_eq!(open_folds(&app), ["adjustments", "mask"]);
    click_header(&app, "LightSection");
    assert_eq!(open_folds(&app), ["adjustments", "light"]);
    click_header(&app, "AdjustmentsSection");
    assert_eq!(open_folds(&app), ["adjustments", "mask"]);
}

/// One section open at a time, and MASK and LIGHT left open while
/// no section of the mask's was made (as a restored session leaves
/// them): a mask chosen in the list, by the edit (undo, redo), or
/// made by M shows MASK alone, since a mask is what was asked for.
#[test]
fn a_mask_chosen_or_made_shows_mask_alone_over_folds_set_unmade() {
    let both_open = |app: &App| {
        app.set_collapsed_light(false);
        crate::testing::settle(app);
        app.set_collapsed_mask(false);
        crate::testing::settle(app);
    };

    // Chosen in the list.
    let (app, _state, _worker) = one_open_window(true, Vec::new());
    show_tab(&app, "Masks");
    click_new_mask(&app);
    choose_mask(&app, 0);
    both_open(&app);
    choose_mask(&app, 1);
    assert_eq!(
        open_folds(&app),
        ["adjustments", "mask"],
        "chosen in the list"
    );

    // Chosen by the edit, from Develop.
    let (app, state, _worker) = one_open_window(true, Vec::new());
    show_tab(&app, "Masks");
    click_new_mask(&app);
    show_tab(&app, "Develop");
    both_open(&app);
    let edit = state.borrow().develop.edit.clone();
    show_edit(&state.borrow(), &edit, &app, Some(0));
    settled(&app);
    assert_eq!(
        (app.get_panel_tab().as_str(), app.get_target()),
        ("Masks", 1)
    );
    assert_eq!(
        open_folds(&app),
        ["adjustments", "mask"],
        "chosen by the edit"
    );

    // Made by M, from Develop.
    let (app, _state, _worker) = one_open_window(true, crate::testing::folder(1));
    app.invoke_select(0);
    settled(&app);
    both_open(&app);
    crate::testing::press(&app, "m");
    settled(&app);
    assert_eq!(
        (app.get_panel_tab().as_str(), app.get_target()),
        ("Masks", 1)
    );
    assert_eq!(open_folds(&app), ["adjustments", "mask"], "made by M");
}

/// One section open at a time, and two of Develop's sections left
/// open while Develop's were unmade (a restored session, another
/// tab): Develop shows one of them.
#[test]
fn sections_made_open_under_one_at_a_time_leave_one_open() {
    let (app, _state, _worker) = one_open_window(true, Vec::new());
    show_tab(&app, "Crop");
    app.set_collapsed_light(false);
    app.set_collapsed_grain(false);
    crate::testing::settle(&app);
    show_tab(&app, "Develop");
    assert_eq!(open_folds(&app), ["adjustments", "grain"]);
}

/// With the rule off, the same moves leave every section as it was.
#[test]
fn sections_made_open_with_the_rule_off_stay_open() {
    let (app, _state, _worker) = one_open_window(false, Vec::new());
    click_header(&app, "LightSection");
    show_tab(&app, "Masks");
    click_new_mask(&app);
    assert_eq!(open_folds(&app), ["adjustments", "mask", "light"]);
    choose_mask(&app, 0);
    choose_mask(&app, 1);
    assert_eq!(open_folds(&app), ["adjustments", "mask", "light"]);
    show_tab(&app, "Crop");
    app.set_collapsed_grain(false);
    crate::testing::settle(&app);
    show_tab(&app, "Develop");
    assert_eq!(open_folds(&app), ["adjustments", "mask", "light", "grain"]);
}

/// The sharpen's Threshold slider runs 0 to 200 percent in steps of
/// one, RawTherapee's range, and shows a value at its top as such.
#[test]
fn the_sharpen_threshold_runs_to_200_percent() {
    use crate::testing::settle;
    use i_slint_backend_testing::ElementHandle;
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 4000.0));
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    app.set_collapsed_sharpen(false);
    app.set_sharpen_auto_threshold(false);
    app.set_sharpen_threshold(2.0);
    settle(&app);
    let sliders: Vec<_> =
        ElementHandle::find_by_element_id(&app, "SharpenSection::threshold-slider").collect();
    assert_eq!(sliders.len(), 1, "the sharpen's Threshold, built once");
    let s = &sliders[0];
    assert_eq!(s.accessible_value_minimum(), Some(0.0));
    assert_eq!(s.accessible_value_maximum(), Some(2.0));
    assert_eq!(s.accessible_value_step(), Some(0.01));
    assert_eq!(s.accessible_value().as_deref(), Some("200%"));
}

/// One section open at a time: a section opened by its header,
/// with an open one above it folding, is brought into the panel's
/// view, its header first, rather than carried up out of sight.
#[test]
fn a_section_opened_under_one_at_a_time_comes_into_view() {
    use crate::testing::{click, settle};
    use i_slint_backend_testing::ElementHandle;
    let app = window(3);
    // Short, so the panel scrolls.
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 640.0));
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    app.set_one_section(true);
    settle(&app);
    app.set_collapsed_light(false);
    settle(&app);
    let view = || {
        let v = ElementHandle::find_by_element_id(&app, "App::panel-view")
            .next()
            .expect("the panel's view");
        (
            v.absolute_position().y,
            v.absolute_position().y + v.size().height,
        )
    };
    // DETAIL's header in the view, under the open LIGHT.
    let (at, size) = crate::testing::header(&app, "DetailSection");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    for _ in 0..6 {
        settle(&app);
    }
    assert!(!app.get_collapsed_detail() && app.get_collapsed_light());
    let header = crate::testing::section_part(&app, "DetailSection", "Section::row")
        .expect("DETAIL's header in the view, the only part the panel builds");
    let (top, bottom) = view();
    let y = header.absolute_position().y;
    assert!(
        y >= top && y + header.size().height <= bottom,
        "DETAIL's header at {y}, the view {top}..{bottom}"
    );
}

/// The wheel during an opening's follow wins: the section is let
/// go of, not pulled back into view against it.
#[test]
fn the_wheel_lets_go_of_a_section_being_brought_into_view() {
    use crate::testing::{click, settle};
    use slint::platform::WindowEvent;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 640.0));
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    app.set_one_section(true);
    settle(&app);
    app.set_collapsed_light(false);
    settle(&app);
    let (at, size) = crate::testing::header(&app, "DetailSection");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    // A frame of the folds, then the wheel up to the top.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
    for _ in 0..20 {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(at.x + 40.0, at.y),
            delta_x: 0.0,
            delta_y: 120.0,
        });
    }
    assert_eq!(app.get_panel_scroll(), 0.0, "the wheel took it to the top");
    for _ in 0..6 {
        settle(&app);
    }
    assert_eq!(app.get_panel_scroll(), 0.0, "and it stays there");
}

/// The DEMOSAIC menu lists the open raw's mosaic's methods and
/// selects what its develop runs; read back untouched it keeps the
/// stored choice; a pick through the window's callback is stored,
/// the method already shown included; while the mosaic is not
/// known the menu rests and offers no other mosaic's methods.
#[test]
fn the_demosaic_menu_follows_the_open_raw_s_mosaic() {
    use crate::panel::browser::OpenMosaic;
    use greycard_core::raw::Mosaic;
    let app = window(1);
    let names =
        |app: &App| -> Vec<String> { app.get_demosaics().iter().map(|s| s.to_string()).collect() };
    show_demosaic(OpenMosaic::Known(Mosaic::XTrans), Demosaic::Amaze, &app);
    assert_eq!(names(&app), ["markesteijn-3", "markesteijn-1", "bilinear"]);
    assert_eq!(app.get_demosaic(), "markesteijn-3");
    assert_eq!(app.get_demosaic_hint(), "");
    // Opening, or another slider: the stored choice stays.
    assert_eq!(read_demosaic(&app), Demosaic::Amaze);
    // A click on the method already shown stores it.
    app.invoke_demosaic_picked("markesteijn-3".into());
    assert_eq!(read_demosaic(&app), Demosaic::Markesteijn3);
    // One, then three: three.
    app.invoke_demosaic_picked("markesteijn-1".into());
    app.invoke_demosaic_picked("markesteijn-3".into());
    assert_eq!(read_demosaic(&app), Demosaic::Markesteijn3);

    // A frame opening whose mosaic is not known yet: no Bayer list
    // to click, the stored choice as it is, and why.
    show_demosaic(OpenMosaic::Unknown, Demosaic::Markesteijn1, &app);
    assert_eq!(names(&app), ["markesteijn-1"]);
    assert_eq!(app.get_demosaic(), "markesteijn-1");
    assert_eq!(app.get_demosaic_hint(), "reading the raw");
    assert_eq!(read_demosaic(&app), Demosaic::Markesteijn1);

    // A raw that is no mosaic says so for good, not "reading".
    show_demosaic(OpenMosaic::NotMosaic, Demosaic::Markesteijn1, &app);
    assert_eq!(app.get_demosaic_hint(), "not a mosaic");

    show_demosaic(
        OpenMosaic::Known(Mosaic::Bayer),
        Demosaic::Markesteijn1,
        &app,
    );
    assert_eq!(names(&app).len(), 6);
    assert_eq!(names(&app)[0], "amaze-vng4");
    assert_eq!(app.get_demosaic(), "amaze-vng4");
    assert_eq!(read_demosaic(&app), Demosaic::Markesteijn1);
    app.invoke_demosaic_picked("rcd".into());
    assert_eq!(read_demosaic(&app), Demosaic::Rcd);
}

/// The defringe's dropper needs the fringes on screen to aim at,
/// so while it is out the picture is developed with the pass off.
/// The edit itself is not touched: the sidecar still says the
/// defringe is on, and every other dropper leaves the develop
/// alone.
#[test]
fn the_defringe_dropper_takes_the_pass_off_the_picture() {
    let app = window(1);
    let mut edit = Edit::default();
    edit.lens.defringe = true;
    edit.lens.defringe_radius = 3.0;

    assert!(app.get_picking().is_empty());
    assert!(edit_to_develop(&app, &edit).lens.defringe);

    app.set_picking("Defringe".into());
    let shown = edit_to_develop(&app, &edit);
    assert!(!shown.lens.defringe, "the pass is off while picking");
    assert!(shown.lens.defringe().is_none());
    // Only the defringe: the rest of the edit goes as it is, and
    // what is saved still has the pass on.
    assert_eq!(shown.lens.defringe_radius, 3.0);
    assert_eq!(
        greycard_edit::Lens {
            defringe: true,
            ..shown.lens
        },
        edit.lens
    );
    assert!(edit.lens.defringe, "the edit itself is untouched");

    // Another dropper changes nothing about the develop.
    app.set_picking("White".into());
    assert!(edit_to_develop(&app, &edit).lens.defringe);
    // And putting it down puts the pass back.
    app.set_picking("".into());
    assert!(edit_to_develop(&app, &edit).lens.defringe);
}

/// A mouse wheel's click is one step of the slider, whatever the
/// platform makes a click in pixels. Slint hands the window 60 a
/// click, and a slider counting 40-pixel notches made the clicks
/// alternate one step and two. A touchpad's small deltas still add
/// up to a step.
#[test]
fn a_wheel_click_on_a_slider_is_one_step() {
    use crate::testing;
    use slint::platform::WindowEvent;

    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = testing::state_for(&app, testing::folder(1));
    app.invoke_select(0);
    app.set_panel_tab("Develop".into());
    app.set_exposure(0.0);
    let center = testing::track_middle(&app, "LightSection::exposure");
    // A click on the slider focuses it, so the wheel is its own.
    testing::click(&app, center.0, center.1);
    let scroll = |delta_y: f32| {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(center.0, center.1),
            delta_x: 0.0,
            delta_y,
        });
    };
    let exposure = || (app.get_exposure() * 100.0).round() / 100.0;

    for (i, want) in [0.05, 0.10, 0.15, 0.20].into_iter().enumerate() {
        scroll(60.0);
        assert_eq!(exposure(), want, "click {}", i + 1);
    }
    scroll(-60.0);
    assert_eq!(exposure(), 0.15, "a click back is one step back");
    // A platform giving a smaller click, still one step each.
    scroll(45.0);
    scroll(45.0);
    assert_eq!(exposure(), 0.25);
    // A touchpad's deltas add up: two of 15 are nothing, the third
    // is the step, and the remainder does not carry.
    scroll(15.0);
    scroll(15.0);
    assert_eq!(exposure(), 0.25);
    scroll(15.0);
    assert_eq!(exposure(), 0.30);
    scroll(15.0);
    assert_eq!(exposure(), 0.30);
}

/// A Mac turns a wheel's vertical scroll with Shift held into a
/// sideways one of the same sign: a sideways delta under Shift
/// reads as the vertical it was, and without Shift a move right
/// (content leftward, a negative delta) is an up.
#[test]
fn shift_with_a_sideways_wheel_reads_as_the_vertical_it_was() {
    use crate::testing;
    use slint::platform::{Key, WindowEvent};

    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = testing::state_for(&app, testing::folder(1));
    app.invoke_select(0);
    app.set_panel_tab("Develop".into());
    app.set_exposure(0.0);
    let center = testing::track_middle(&app, "LightSection::exposure");
    testing::click(&app, center.0, center.1);
    let sideways = |delta_x: f32| {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(center.0, center.1),
            delta_x,
            delta_y: 0.0,
        });
    };
    let shift = |down: bool| {
        let text = Key::Shift.into();
        app.window().dispatch_event(if down {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
    };
    let exposure = || (app.get_exposure() * 100.0).round() / 100.0;

    shift(true);
    sideways(60.0);
    assert_eq!(
        exposure(),
        0.05,
        "Shift and a wheel up, turned rightward, is an up"
    );
    sideways(-60.0);
    assert_eq!(
        exposure(),
        0.0,
        "Shift and a wheel down, turned leftward, is a down"
    );
    shift(false);
    sideways(-60.0);
    assert_eq!(
        exposure(),
        0.05,
        "a move right, the content leftward, is an up"
    );
}

#[test]
fn natural_scrolling_moves_a_slider_the_way_the_fingers_went() {
    use crate::testing;
    use slint::platform::{Key, WindowEvent};

    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = testing::state_for(&app, testing::folder(1));
    app.invoke_select(0);
    app.set_panel_tab("Develop".into());
    app.set_exposure(0.0);
    let center = testing::track_middle(&app, "LightSection::exposure");
    testing::click(&app, center.0, center.1);
    let vertical = |delta_y: f32| {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(center.0, center.1),
            delta_x: 0.0,
            delta_y,
        });
    };
    let exposure = || (app.get_exposure() * 100.0).round() / 100.0;

    vertical(60.0);
    assert_eq!(exposure(), 0.05, "a wheel up is an up");
    vertical(-60.0);
    assert_eq!(exposure(), 0.0, "a wheel down is a down");
    app.global::<crate::SliderKeys>().set_natural(true);
    vertical(-60.0);
    assert_eq!(
        exposure(),
        0.05,
        "a swipe up, come as a scroll down, is an up"
    );
    vertical(60.0);
    assert_eq!(
        exposure(),
        0.0,
        "a swipe down, come as a scroll up, is a down"
    );
    let sideways = |delta_x: f32| {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(center.0, center.1),
            delta_x,
            delta_y: 0.0,
        });
    };
    let shift = |down: bool| {
        let text = Key::Shift.into();
        app.window().dispatch_event(if down {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
    };
    shift(true);
    sideways(-60.0);
    assert_eq!(
        exposure(),
        0.05,
        "Shift and a wheel up, turned leftward, is an up"
    );
    sideways(60.0);
    assert_eq!(
        exposure(),
        0.0,
        "Shift and a wheel down, turned rightward, is a down"
    );
    shift(false);
    sideways(60.0);
    assert_eq!(
        exposure(),
        0.05,
        "a swipe right, the content rightward, is an up"
    );
    sideways(-60.0);
    assert_eq!(exposure(), 0.0, "a swipe left is a down");
}

/// FOLDERS keeps a fold for each view: folded in the Library, it
/// is open in the Loupe as it was left there, and folded again
/// back in the Library. A click on the header moves the fold; a
/// view's fold lands at once.
#[test]
fn the_folders_section_keeps_a_fold_for_each_view() {
    use crate::testing::{click, press, settle};
    use i_slint_backend_testing::ElementHandle;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    app.set_folder_tree_shown(true);
    let height = || {
        ElementHandle::find_by_element_id(&app, "App::folders-section")
            .next()
            .expect("FOLDERS in the left pane")
            .size()
            .height
    };
    // One frame of a fold's 120 ms, then the rest of it.
    let frame = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
    };
    let rest = || {
        settle(&app);
        settle(&app);
    };
    press(&app, "G");
    settle(&app);
    assert!(app.get_grid_open());
    assert!(!app.get_collapsed_folders());
    let header = crate::testing::section_part(&app, "FoldersSection", "Section::row")
        .expect("FOLDERS in the left pane");
    let (at, size) = (header.absolute_position(), header.size());
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    frame();
    let moving = height();
    rest();
    assert!(app.get_collapsed_folders(), "folded in the Library");
    assert!(moving > height(), "a click's fold moves");

    press(&app, "G");
    frame();
    let landed = height();
    rest();
    assert!(!app.get_grid_open());
    assert!(!app.get_collapsed_folders(), "open in the Loupe");
    assert_eq!(landed, height(), "a view's fold lands at once");
    assert_eq!(
        read_folds(&app)
            .iter()
            .filter(|f| f.starts_with("folders"))
            .collect::<Vec<_>>(),
        ["folders-library"]
    );

    press(&app, "G");
    settle(&app);
    assert!(app.get_grid_open());
    assert!(app.get_collapsed_folders(), "folded again in the Library");
}

/// Both folds go to the settings and come back, whichever view is
/// on screen; a file from before them, with FOLDERS's one fold,
/// folds it in both views.
#[test]
fn the_folders_folds_round_trip_through_the_settings() {
    use crate::testing::settle;
    let folders = |app: &App| -> Vec<String> {
        read_folds(app)
            .into_iter()
            .filter(|f| f.starts_with("folders"))
            .collect()
    };
    let app = window(3);
    for grid in [false, true] {
        app.set_grid_open(grid);
        settle(&app);
        for (library, loupe) in [(true, false), (false, true), (true, true), (false, false)] {
            let mut saved = Vec::new();
            if library {
                saved.push("folders-library".to_string());
            }
            if loupe {
                saved.push("folders-loupe".to_string());
            }
            show_folds(&app, &saved);
            assert_eq!(folders(&app), saved, "grid {grid}");
            assert_eq!(
                app.get_collapsed_folders(),
                if grid { library } else { loupe }
            );
            // Through a switch and back, still the same.
            app.set_grid_open(!grid);
            settle(&app);
            assert_eq!(
                app.get_collapsed_folders(),
                if grid { loupe } else { library }
            );
            assert_eq!(folders(&app), saved, "grid {grid}, switched");
            app.set_grid_open(grid);
            settle(&app);
        }
    }
    show_folds(&app, &["folders".to_string()]);
    assert_eq!(folders(&app), ["folders-library", "folders-loupe"]);
    show_folds(&app, &[]);
    assert!(folders(&app).is_empty());
    show_folds(&app, &["geometry".into()]);
    assert!(
        app.get_collapsed_crop() && app.get_collapsed_rotate() && app.get_collapsed_perspective(),
        "GEOMETRY's fold folds its three"
    );

    // Read between a switch and its handler, as the start's
    // baseline is: each view's fold is still its own.
    app.set_grid_open(false);
    settle(&app);
    show_folds(&app, &["folders-library".to_string()]);
    app.set_grid_open(true);
    assert_eq!(folders(&app), ["folders-library"]);
    settle(&app);
    assert!(app.get_collapsed_folders());
    assert_eq!(folders(&app), ["folders-library"]);
}

/// The panel at its sections' foot keeps to their reach when they
/// get shorter under it (sections folded, a mask deleted) or the
/// view gets taller (a taller window): never scrolled past their
/// foot with room left empty under it (issue 157).
#[test]
fn the_panel_keeps_to_its_sections_when_they_shrink_or_the_view_grows() {
    use crate::testing::{assert_panel_in_range, panel_to_end, settle};
    for (w, h) in [(1683.0, 1000.0), (1500.0, 800.0)] {
        let at = format!("{w}x{h}");
        let app = window(3);
        app.window().set_size(slint::LogicalSize::new(w, h));
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        for (_, _, set) in FOLDS {
            set(&app, false);
        }
        settle(&app);

        // A tall section folded.
        panel_to_end(&app);
        app.set_collapsed_light(true);
        assert_panel_in_range(&app, &format!("{at}: LIGHT folded"));
        // Every section folded.
        panel_to_end(&app);
        for (_, _, set) in FOLDS {
            set(&app, true);
        }
        assert_panel_in_range(&app, &format!("{at}: all folded"));
        for (_, _, set) in FOLDS {
            set(&app, false);
        }

        // A taller window.
        panel_to_end(&app);
        app.window().set_size(slint::LogicalSize::new(w, h + 400.0));
        assert_panel_in_range(&app, &format!("{at}: taller"));
        app.window().set_size(slint::LogicalSize::new(w, h));

        // A mask deleted, with three of them, from the Masks tab's foot.
        app.set_panel_tab("Masks".into());
        for _ in 0..3 {
            app.invoke_new_mask();
        }
        settle(&app);
        panel_to_end(&app);
        app.invoke_delete_adjustment();
        assert_panel_in_range(&app, &format!("{at}: a mask deleted"));
        // Global chosen from a mask, from the foot.
        app.invoke_target_changed(1);
        settle(&app);
        panel_to_end(&app);
        app.invoke_target_changed(0);
        assert_panel_in_range(&app, &format!("{at}: Global chosen"));

        // A short tab after a long one starts at its top.
        app.set_panel_tab("Develop".into());
        settle(&app);
        panel_to_end(&app);
        app.set_panel_tab("Crop".into());
        assert_panel_in_range(&app, &format!("{at}: Crop after Develop"));
        drop(state);
    }
}

/// Shrinks one after another, several in one frame, and a view
/// grown twice, with no scroll between them: each keeps the panel
/// in its sections' reach.
#[test]
fn the_panel_keeps_to_its_sections_through_shrinks_in_a_row() {
    use crate::testing::{assert_panel_in_range, panel_to_end, settle};
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 800.0));
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    for (_, _, set) in FOLDS {
        set(&app, false);
    }
    settle(&app);
    panel_to_end(&app);
    app.set_collapsed_light(true);
    assert_panel_in_range(&app, "first shrink");
    app.set_collapsed_color(true);
    assert_panel_in_range(&app, "second shrink");
    app.set_collapsed_curves(true);
    app.set_collapsed_mixer(true);
    app.set_collapsed_grading(true);
    assert_panel_in_range(&app, "three shrinks in one frame");
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 1000.0));
    assert_panel_in_range(&app, "then taller");
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 1300.0));
    assert_panel_in_range(&app, "then taller again");
}

/// One section open at a time, scrolled to the foot with LIGHT
/// open: a section below it opened by its header is brought into
/// view as LIGHT folds, though the panel, shorter, is pulled back
/// into its reach in the same frames.
#[test]
fn a_section_opened_at_the_foot_under_one_at_a_time_comes_into_view() {
    use crate::testing::{click, panel_scroll_and_end, panel_to_end, section_part, settle};
    use i_slint_backend_testing::ElementHandle;
    let mut missed = Vec::new();
    // The header that sits in the view at the foot, under LIGHT,
    // at each size: the one a click can reach there. (1014 tall, the
    // head's kept time line taken, is the panel 1000 was before it.)
    for (w, h, name) in [
        (1500.0, 800.0, "DetailSection"),
        (1683.0, 1014.0, "ColorSection"),
    ] {
        let app = window(3);
        app.window().set_size(slint::LogicalSize::new(w, h));
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        for (_, _, set) in FOLDS {
            set(&app, true);
        }
        app.set_collapsed_light(false);
        app.set_one_section(true);
        settle(&app);
        panel_to_end(&app);
        let view = ElementHandle::find_by_element_id(&app, "App::panel-view")
            .next()
            .expect("the panel's view");
        let (top, bottom) = (
            view.absolute_position().y,
            view.absolute_position().y + view.size().height,
        );
        let header = section_part(&app, name, "Section::row")
            .unwrap_or_else(|| panic!("{w}x{h}: {name}'s header"));
        let y = header.absolute_position().y + header.size().height / 2.0;
        assert!(
            y > top && y < bottom,
            "{w}x{h}: {name}'s header in the view at the foot, at {y} in {top}..{bottom}"
        );
        click(
            &app,
            header.absolute_position().x + header.size().width / 2.0,
            y,
        );
        for _ in 0..10 {
            settle(&app);
        }
        let (scroll, end) = panel_scroll_and_end(&app);
        let at =
            section_part(&app, name, "Section::row").map_or(f32::NAN, |h| h.absolute_position().y);
        if !(at >= top - 0.5 && at < bottom) {
            missed.push(format!(
                "{w}x{h} {name}: header at {at}, view {top}..{bottom}, scroll {scroll}, reach {end}"
            ));
        }
    }
    assert!(
        missed.is_empty(),
        "sections not brought into view: {missed:#?}"
    );
}

/// The right pane hidden (Tab, F8) at its sections' foot, the
/// sections folded meanwhile, then brought back: the pane, built
/// anew, keeps to what its sections now reach (issue 157, by way
/// of another photo with fewer masks).
#[test]
fn a_pane_hidden_and_back_keeps_to_its_sections() {
    use crate::testing::{assert_panel_in_range, panel_to_end, settle};
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 800.0));
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    for (_, _, set) in FOLDS {
        set(&app, false);
    }
    settle(&app);
    panel_to_end(&app);
    app.set_right_hidden(true);
    settle(&app);
    for (_, _, set) in FOLDS {
        set(&app, true);
    }
    settle(&app);
    app.set_right_hidden(false);
    for _ in 0..5 {
        settle(&app);
    }
    assert_panel_in_range(&app, "the pane back after folds while hidden");
}

/// A scroll the sections still reach comes back with the pane
/// unchanged, at their foot and short of it.
#[test]
fn a_pane_hidden_and_back_keeps_a_scroll_in_reach() {
    use crate::testing::{panel_scroll_and_end, panel_to_end, settle};
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 800.0));
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    for (_, _, set) in FOLDS {
        set(&app, false);
    }
    settle(&app);
    panel_to_end(&app);
    let (foot, _) = panel_scroll_and_end(&app);
    for kept in [foot, foot / 2.0] {
        app.set_panel_scroll(kept);
        settle(&app);
        app.set_right_hidden(true);
        settle(&app);
        app.set_right_hidden(false);
        for _ in 0..5 {
            settle(&app);
        }
        assert_eq!(app.get_panel_scroll(), kept, "the scroll kept");
    }
}

/// The right pane hidden and shown with only time between (the
/// lightbox and F8 set it from a timer, with no event to tear the
/// pane down first): the pane on its way out, squeezed for a frame,
/// is not kept in a reach it has only then, and the scroll comes
/// back exactly as it was, at the foot and short of it.
#[test]
fn a_pane_hidden_by_timers_alone_keeps_its_scroll() {
    use crate::testing::{panel_scroll_and_end, panel_to_end, settle};
    let step = |ms: u64| {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
    };
    for (w, h) in [(1500.0, 800.0), (1683.0, 1000.0)] {
        for part in [1.0f32, 0.99, 0.5, 0.1] {
            let app = window(3);
            app.window().set_size(slint::LogicalSize::new(w, h));
            let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
            for (_, _, set) in FOLDS {
                set(&app, false);
            }
            settle(&app);
            panel_to_end(&app);
            let (foot, _) = panel_scroll_and_end(&app);
            let kept = (foot * part).round();
            app.set_panel_scroll(kept);
            settle(&app);
            app.set_right_hidden(true);
            step(16);
            assert_eq!(app.get_panel_scroll(), kept, "{w}x{h} {part}: hidden");
            app.set_right_hidden(false);
            for ms in [1, 1, 16, 16] {
                step(ms);
            }
            assert_eq!(app.get_panel_scroll(), kept, "{w}x{h} {part}: shown");
            let (now, _) = panel_scroll_and_end(&app);
            assert_eq!(now, kept, "{w}x{h} {part}: laid out");
        }
    }
}

/// The pane shown and its look made with no event and no read of
/// the window between: the content is laid out by then, and the
/// foot is kept.
#[test]
fn a_pane_rebuilt_by_timers_alone_keeps_its_foot() {
    use crate::testing::{panel_scroll_and_end, panel_to_end, settle};
    let step = |ms: u64| {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
    };
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 800.0));
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    for (_, _, set) in FOLDS {
        set(&app, false);
    }
    settle(&app);
    panel_to_end(&app);
    let (foot, _) = panel_scroll_and_end(&app);
    app.set_right_hidden(true);
    step(20);
    app.set_right_hidden(false);
    for ms in [1, 1, 1, 20, 20] {
        step(ms);
    }
    let (now, _) = panel_scroll_and_end(&app);
    assert_eq!(now, foot, "kept with timers alone");
}

/// Into the lightbox and back out, the grid open before and after:
/// the right pane's scroll is where it was, at the foot and short
/// of it.
#[test]
fn the_lightbox_in_and_out_keeps_the_panels_scroll() {
    use crate::testing::{panel_scroll_and_end, panel_to_end, settle};
    let step = |ms: u64| {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
    };
    for (w, h) in [(1500.0, 800.0), (1683.0, 1000.0)] {
        for part in [1.0f32, 0.99, 0.5, 0.1] {
            let app = window(3);
            app.window().set_size(slint::LogicalSize::new(w, h));
            let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
            app.set_grid_open(true);
            for (_, _, set) in FOLDS {
                set(&app, false);
            }
            settle(&app);
            panel_to_end(&app);
            let (foot, _) = panel_scroll_and_end(&app);
            let kept = (foot * part).round();
            app.set_panel_scroll(kept);
            settle(&app);
            app.invoke_lightbox_enter();
            step(16);
            app.invoke_lightbox_leave();
            for ms in [1, 1, 16, 16] {
                step(ms);
            }
            let (now, _) = panel_scroll_and_end(&app);
            assert_eq!(now, kept, "{w}x{h} {part}: back from the lightbox");
        }
    }
}

/// A frame whose masks wait on a shape nothing can measure (§724) opens
/// read-only: a slider moved on it goes back to the state's value, the
/// panel holds the state, and no step is recorded.
#[test]
fn a_slider_moved_on_a_frame_that_waits_goes_back() {
    use crate::testing;
    use slint::platform::WindowEvent;

    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = testing::state_for(&app, testing::folder(1));
    state.borrow_mut().sidecars[0] = testing::waiting_sidecar();
    let steps = state.borrow().sidecars[0].history.len();
    app.invoke_select(0);
    app.set_panel_tab("Develop".into());
    let center = testing::track_middle(&app, "LightSection::exposure");
    testing::click(&app, center.0, center.1);
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(center.0, center.1),
        delta_x: 0.0,
        delta_y: 60.0,
    });
    assert_eq!(app.get_exposure(), 0.0, "the slider went back");
    let st = state.borrow();
    assert_eq!(
        st.develop.edit,
        st.sidecars[0].current().clone(),
        "the panel holds the state"
    );
    assert_eq!(st.sidecars[0].history.len(), steps, "no step");
}

/// The open frame's wait cleared by another path than its own develop
/// (an export, a copy, a merge measured it): when its develop lands the
/// panel takes the state as measured, unmarked, and a slider moved after
/// is recorded.
#[test]
fn a_frame_measured_while_open_takes_changes_again() {
    use crate::testing;

    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = testing::state_for(&app, testing::folder(1));
    crate::WORKER.with(|w| *w.borrow_mut() = Some(worker.clone()));
    state.borrow_mut().sidecars[0] = testing::waiting_sidecar();
    app.invoke_select(0);
    assert!(
        state.borrow().develop.edit.unturned,
        "the panel's copy waits"
    );
    // Measured elsewhere: the frame 1.5 wide before its quarter turn.
    state.borrow_mut().sidecars[0].frame_known(1.0 / 1.5);
    assert!(!state.borrow().sidecars[0].current_waits());
    testing::land_develop(
        &app,
        &state,
        crate::worker::LearnedReport::Off,
        crate::worker::FillReport::default(),
    );
    crate::panel::startup::land_pending(&mut state.borrow_mut(), &app);
    {
        let st = state.borrow();
        assert!(!st.develop.edit.unturned, "the panel unmarked");
        assert_eq!(st.develop.edit, st.sidecars[0].current().clone());
    }
    let steps = state.borrow().sidecars[0].history.len();
    app.set_exposure(0.8);
    {
        let mut st = state.borrow_mut();
        let edit = read_edit(&app, &st.develop.edit, st.develop.target);
        save_edit(&mut st, edit);
    }
    let st = state.borrow();
    assert_eq!(st.sidecars[0].history.len(), steps + 1, "the move recorded");
    assert_eq!(st.sidecars[0].current().light.exposure, 0.8);
    drop(st);
    crate::WORKER.with(|w| *w.borrow_mut() = None);
}

/// A frame whose masks wait on its shape is measured by its raw's header
/// when it is next in hand, never by a picture of it (§724): its 16:9
/// thumbnail does not say the 3:2 frame's shape. While its raw will not
/// say, a change is refused and the read-only line said; once the raw
/// says, the frame is measured on the first ask, and the line comes
/// down.
#[test]
fn a_waiting_frame_is_measured_by_its_header_not_its_thumbnail() {
    use crate::testing;

    let dir = std::env::temp_dir().join(format!("greycard-shape-header-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("IMG_0000.tif");
    // Not readable yet: its volume, say, not back.
    std::fs::write(&raw, b"not yet").unwrap();
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = testing::state_for(&app, vec![raw.clone()]);
    {
        let mut st = state.borrow_mut();
        st.sidecars[0] = testing::waiting_sidecar();
        st.views.thumb_base[0] = Some((16, 9, vec![0; 16 * 9 * 3]));
        // The panel holds the waiting copy, and a change was refused.
        st.current = Some(0);
        st.develop.edit = st.sidecars[0].current().clone();
        app.set_exposure(0.8);
        assert!(refused_waiting(&mut st, &app));
    }
    let words = "Read-only: IMG_0000.tif's masks cannot be placed until its raw is reachable";
    assert_eq!(app.get_status().as_str(), words);
    assert_eq!(app.get_exposure(), 0.0, "the slider went back");
    // The raw reachable: 1.5 wide for each unit of its height, as the
    // sidecar was turned by.
    greycard_library::fixture::write_frame_of(
        &raw,
        &greycard_library::fixture::R5,
        6,
        4,
        &[0; 6 * 4 * 3],
    );
    // The frame back in hand: measured by its header, on the first ask.
    let waits = crate::panel::browser::waits_on_shape(&mut state.borrow_mut(), &app, 0, "x");
    assert!(!waits, "measurable, so it does not wait");
    let st = state.borrow();
    assert_eq!(
        st.sidecars[0].shape,
        greycard_edit::Measured::new(1, 1.0 / 1.5),
        "the header's 3:2 turned, not the thumbnail's 16:9"
    );
    assert_eq!(app.get_status().as_str(), "", "the read-only line is down");
    drop(st);
    testing::remove_scratch(state, &dir);
}
