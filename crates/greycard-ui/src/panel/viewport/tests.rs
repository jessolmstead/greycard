use super::*;
use std::cell::Cell;
use std::sync::Arc;

use greycard_core::raw::{Orientation, RawFrame};
use greycard_edit::{Edit, WhiteBalance};

use crate::capture::{LightroomStep, RawTherapeeStep, Shown};
use crate::testing::{click, window};

/// A point of the camera's picture standing in, under random turns,
/// mirrors, straightening, keystones, crops, and camera frames, cut
/// or not, lands within a pixel of the same source point on the
/// develop's frame wherever the frame holds it.
#[test]
fn a_stand_ins_point_lands_on_the_same_source_point() {
    use crate::owners::{Stood, StoodView};
    use greycard_edit::geometry::Crop;
    let mut seed = 12345u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32
    };
    let (sw, sh) = (6000.0f32, 4000.0f32);
    for i in 0..2000 {
        let g = Geometry {
            turns: (rnd() * 4.0) as u8 % 4,
            flip: rnd() < 0.5,
            angle: if i % 3 == 0 {
                0.0
            } else {
                (rnd() - 0.5) * 20.0
            },
            vertical: if i % 4 == 0 {
                (rnd() - 0.5) * 60.0
            } else {
                0.0
            },
            horizontal: if i % 5 == 0 {
                (rnd() - 0.5) * 30.0
            } else {
                0.0
            },
            crop: (rnd() < 0.5).then(|| {
                let (w, h) = (0.2 + rnd() * 0.5, 0.2 + rnd() * 0.5);
                Crop {
                    x: rnd() * (1.0 - w),
                    y: rnd() * (1.0 - h),
                    w,
                    h,
                }
            }),
            ..Geometry::default()
        };
        let camera = (rnd() < 0.5).then_some(Crop {
            x: 0.0625,
            y: 0.0,
            w: 0.875,
            h: 1.0,
        });
        let cut = rnd() < 0.5;
        let frame = g.frame(sw, sh, camera);
        let plain = Geometry {
            turns: g.turns,
            flip: g.flip,
            ..Geometry::default()
        };
        let whole = plain.frame(sw, sh, camera.filter(|_| cut));
        let at = (rnd(), rnd());
        let stood = StoodView {
            generation: 1,
            zoom: 1.0,
            named: true,
            at,
            width: whole.size.0,
            shows: Stood::Camera { cut },
        };
        let (zoom, c) = stood_onto(&stood, &g, (sw, sh), &frame, camera);
        assert!(zoom == 1.0 && c.0.is_finite() && c.1.is_finite(), "{i}");
        let inside = c.0 > 0.0 && c.0 < frame.size.0 && c.1 > 0.0 && c.1 < frame.size.1;
        if inside {
            let on = (
                whole.origin.0 + at.0 * whole.size.0,
                whole.origin.1 + at.1 * whole.size.1,
            );
            let want = plain.to_source(on, sw, sh);
            let got = g.to_source((c.0 + frame.origin.0, c.1 + frame.origin.1), sw, sh);
            let miss = ((got.0 - want.0).powi(2) + (got.1 - want.1).powi(2)).sqrt();
            assert!(miss < 1.0, "case {i}: {miss} px off");
        }
    }
}

fn press_key(app: &App, text: &str) {
    use slint::platform::WindowEvent;
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
}

/// The pointer on the picture, so the keys reach the editor.
fn over_picture(app: &App) {
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(600.0, 400.0),
        });
}

/// The color pick's sample: 5 to start, the droppers' own 5x5, a
/// size it does not offer read as that, and the one chosen kept
/// in the settings.
#[test]
fn the_pick_sample_starts_at_five_and_is_kept_in_the_settings() {
    let app = window(1);
    assert_eq!(app.get_range_sample(), 5);
    assert_eq!(SAMPLE_SIZES.map(sample_reach), [2, 5, 10, 20]);
    assert_eq!((sample_size(7), sample_size(41)), (5, 41));
    app.set_range_sample(21);
    assert_eq!(crate::panel::startup::remember(&app).range_sample, 21);
}

/// The vectorscope's circle sits square in the middle of its box,
/// and the shot's lines fold away while it shows, so the sections
/// below move only by what the two heights differ; switching back
/// brings the lines back and the sections to where they were.
#[test]
fn the_vectorscope_folds_the_shot_away_and_back() {
    use i_slint_backend_testing::ElementHandle;
    let app = window(1);
    let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
    // The options, as the startup gives them, and a shot's lines.
    let names: Vec<slint::SharedString> =
        scope::Scope::ALL.iter().map(|s| s.name().into()).collect();
    app.set_scopes(slint::ModelRc::new(slint::VecModel::from(names)));
    app.set_shot_camera("Body and lens".into());
    app.set_shot_exposure("1/250 s".into());
    app.set_shot_size("6000 x 4000".into());
    let bounds = |id: &str| {
        let e = ElementHandle::find_by_element_id(&app, id)
            .next()
            .unwrap_or_else(|| panic!("no {id}"));
        (e.absolute_position(), e.size())
    };
    let choose = |name: &str| {
        crate::testing::click_segment(&app, "PanelHead::scope-row", name);
        assert_eq!(app.get_scope(), name, "the click chose {name}");
    };
    // The shot's exposure line, which says the test's "1/250 s".
    let shot = || {
        let n = crate::testing::count_id(&app, "PanelHead::shot-exposure-text");
        if n > 0 {
            assert_eq!(
                crate::testing::label_of(&app, "PanelHead::shot-exposure-text"),
                "1/250 s"
            );
        }
        n
    };
    for from in ["RGB", "Wave"] {
        choose(from);
        let (_, before) = bounds("PanelHead::scope-box");
        let top = bounds("App::panel-view").0.y;
        assert_eq!(shot(), 1, "{from}: the shot's lines");

        choose("Vector");
        assert_eq!(shot(), 0, "folded under the vectorscope");
        let (box_at, box_size) = bounds("PanelHead::scope-box");
        let (plot_at, plot_size) = bounds("PanelHead::scope-plot");
        assert_eq!(box_size.height, 180.0);
        assert_eq!((plot_size.width, plot_size.height), (180.0, 180.0));
        let left = plot_at.x - box_at.x;
        let right = box_at.x + box_size.width - (plot_at.x + plot_size.width);
        assert!((left - right).abs() <= 0.5, "centered: {left} and {right}");
        // Four 14 px lines (the time's kept when empty), a 6 px
        // spacer, and 2 px of net spacing fold away.
        let moved = bounds("App::panel-view").0.y - top;
        assert_eq!(
            moved,
            180.0 - before.height - (4.0 * 14.0 + 6.0 + 2.0),
            "from {from}"
        );

        choose(from);
        assert_eq!(shot(), 1, "{from}: the lines back");
        assert_eq!(bounds("App::panel-view").0.y, top, "{from}: back in place");
    }
}

#[test]
fn the_proof_badge_is_hidden_with_the_proof_off() {
    let app = window(1);
    assert_eq!(app.get_proof_badge(), "");
}

#[test]
fn s_over_the_picture_shows_the_badge_with_the_profiles_name() {
    let app = window(1);
    over_picture(&app);
    app.set_proof_profile("Display P3".into());
    press_key(&app, "s");
    assert!(app.get_proofing());
    assert_eq!(app.get_proof_badge(), "Proof: Display P3");
    press_key(&app, "s");
    assert_eq!(app.get_proof_badge(), "");
}

#[test]
fn a_profile_file_is_named_by_its_stem() {
    let app = window(1);
    assert_eq!(app.get_proof_badge(), "");
    set_proof_file(&app, "/somewhere/Fogra39 coated.icc");
    app.set_proof_profile("File".into());
    app.set_proofing(true);
    assert_eq!(app.get_proof_badge(), "Proof: Fogra39 coated");
}

#[test]
fn the_badge_says_gamut_only_with_the_warning_on() {
    let app = window(1);
    app.set_proof_profile("sRGB".into());
    app.set_proofing(true);
    assert!(!app.get_gamut_warning(), "off until switched on");
    assert_eq!(app.get_proof_badge(), "Proof: sRGB");
    app.set_gamut_warning(true);
    assert_eq!(app.get_proof_badge(), "Proof: sRGB \u{b7} gamut");
}

#[test]
fn a_click_on_the_badge_turns_the_proof_off() {
    let app = window(1);
    app.set_proof_profile("sRGB".into());
    app.set_proofing(true);
    let changed = Rc::new(Cell::new(0));
    {
        let changed = changed.clone();
        app.on_proof_changed(move || changed.set(changed.get() + 1));
    }
    let base = i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    click(&app, base.x + 20.0, base.y + 20.0);
    assert!(!app.get_proofing());
    assert_eq!(app.get_proof_badge(), "");
    assert_eq!(changed.get(), 1, "the engine is told");
}

#[test]
fn the_badge_is_hidden_in_culling_with_the_proof_on() {
    let app = window(1);
    app.set_proofing(true);
    assert_ne!(app.get_proof_badge(), "");
    app.set_culling(true);
    assert_eq!(app.get_proof_badge(), "");
}

/// A pick with a single-shot dropper (White, Range, Defringe) does
/// not zoom: the press puts the dropper down and, for these three,
/// lets go of `picking` right there — which used to leave the
/// release reading as a plain click, since it read `picking` fresh
/// rather than remembering the press. Stands in for
/// `viewport::install`'s own wiring of `pick_pressed`'s "Range"
/// arm (`app.invoke_stop_placing()` before the release), which
/// needs a developed picture on a real device to drive for real;
/// this drives the same pointer-event routing the `.slint` side
/// does, through the compiled window, with no GPU behind it.
#[test]
fn a_pick_with_a_single_shot_dropper_does_not_zoom() {
    let app = window(1);
    app.set_picking("Range".into());
    let zoomed = Rc::new(Cell::new(false));
    {
        let zoomed = zoomed.clone();
        app.on_toggle_zoom(move |_, _| zoomed.set(true));
    }
    {
        let app_weak = app.as_weak();
        app.on_pick_pressed(move |_, _| {
            let app = app_weak.upgrade().unwrap();
            // As the "Range" dropper's own press does: read the
            // color under the pointer (faked, here, as moving the
            // hue) and let go of `picking` on the press itself.
            app.set_range_hue(200.0);
            app.set_picking("".into());
        });
    }
    click(&app, 600.0, 400.0);
    assert!(!zoomed.get(), "a pick must not zoom the view");
    assert_eq!(app.get_range_hue(), 200.0, "the hue moved");
}

/// The mixer's dropper is one pick: the release that ends the
/// press (and any drag after it) puts the dropper down, so the
/// cursor is the hand again; its button and Esc put it down
/// before a pick. Drives the installed callbacks; the press itself
/// needs a developed picture to sample, so the test stands in for
/// it with what the press leaves behind.
#[test]
fn a_pick_puts_the_mixer_dropper_down() {
    let app = window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);

    app.invoke_pick_started("Mixer".into());
    assert_eq!(app.get_picking(), "Mixer", "the button puts it in hand");
    state.borrow_mut().develop.picking = Some(Picking::Mixer {
        band: 2,
        hue0: 0.0,
        sat0: 0.0,
        at: (10.0, 10.0),
    });
    app.invoke_pick_dragged(30.0, 0.0);
    assert!(app.get_mixer_hue() > 0.0, "the drag moved the band");
    app.invoke_pick_released();
    assert_eq!(app.get_picking(), "", "the pick put the dropper down");
    assert!(state.borrow().develop.picking.is_none());

    // Its button again, before a pick, takes it back.
    app.invoke_pick_started("Mixer".into());
    app.invoke_pick_started("Mixer".into());
    assert_eq!(app.get_picking(), "");

    // And Esc.
    app.invoke_pick_started("Mixer".into());
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert_eq!(app.get_picking(), "");

    // The Curve's dropper is a point a click and stays in hand.
    app.invoke_pick_started("Curve".into());
    state.borrow_mut().develop.picking = Some(Picking::Curve {
        channel: Channel::Rgb,
        index: 0,
        y0: 0.5,
        at: 10.0,
    });
    app.invoke_pick_released();
    assert_eq!(app.get_picking(), "Curve", "a curve pick leaves it in hand");
}

/// The pointer over the picture reaches the state, from the
/// `.slint` side's own report of it, and leaving takes the color
/// under it off the scope. The color itself needs an analysis
/// read back from a real device (`render::tests`).
#[test]
fn the_readout_follows_the_pointer_and_goes_with_it() {
    let app = window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(600.0, 400.0),
        });
    slint::platform::update_timers_and_animations();
    // In the viewport's own pixels, as the droppers' presses are:
    // the left pane's width in from the window's.
    let hover = state.borrow().develop.hover;
    assert!(hover.is_some_and(|(x, _)| x < 600.0), "{hover:?}");
    // Nothing to read yet, so nothing shown.
    assert_eq!(app.get_scope_readout(), "");
    // A diagonal move within the view, its one report of both.
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(640.0, 430.0),
        });
    slint::platform::update_timers_and_animations();
    let (x0, y0) = hover.unwrap();
    let moved = state.borrow().develop.hover;
    assert_eq!(moved, Some((x0 + 40.0, y0 + 30.0)));

    app.set_scope_readout("R 1  G 2  B 3  L 2".into());
    app.set_scope_marks(ModelRc::new(VecModel::from(vec![ScopeMark {
        x: 0.5,
        y: 0.5,
        channel: 0,
    }])));
    app.invoke_view_hovered(false, 600.0, 400.0);
    assert_eq!(state.borrow().develop.hover, None);
    assert_eq!(app.get_scope_readout(), "");
    assert_eq!(app.get_scope_marks().row_count(), 0);
}

/// A window at `scale` with frame 0 open and its own develop of
/// 6000 by 4000 on the GPU, at the fit: the state `view_to_frame`
/// and the readout measure by. Nothing is drawn.
fn hover_frame(scale: f32) -> (App, Rc<Shared>, Rc<crate::worker::Worker>) {
    let app = window(1);
    if scale != 1.0 {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
    }
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.set_placeholder(false);
    {
        let mut st = state.borrow_mut();
        st.develop.held = None;
        st.develop.peek = None;
        st.develop.shown_size = (6000, 4000);
        st.develop.source_size = (6000, 4000);
        st.develop.shown_turn = Some((0, current_turn(&st)));
        st.develop.image_size = (6000, 4000);
        st.develop.center = (3000.0, 2000.0);
        st.develop.zoom = 0.0;
    }
    (app, state, worker)
}

/// A Ctrl-click on a frame of one color with a wheel in hand sets
/// that wheel's hue to the color's as shown, at its own strength,
/// and zooms nothing; Ctrl+Shift the complement. Esc puts the
/// wheel down, and so does a control of another section, and a
/// Ctrl-click then sets no hue. Skipped without a GPU to sample.
#[test]
fn a_ctrl_click_with_a_wheel_in_hand_takes_the_hue_there() {
    use crate::render::Renderer;
    use slint::platform::{Key, WindowEvent};
    let Some((device, queue)) = greycard_viewport::testing::device("wheel_pick") else {
        return;
    };
    let (app, state, _worker) = hover_frame(1.0);
    let (w, h) = (192usize, 128usize);
    let color = [0.30f32, 0.12, 0.05];
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&greycard_render::halves::Halves::from_image(
        &greycard_core::image::WorkingImage {
            width: w,
            height: h,
            data: color.repeat(w * h),
        },
        None,
    ));
    {
        let mut st = state.borrow_mut();
        st.develop.renderer = Some(renderer);
        st.develop.shown_size = (w as u32, h as u32);
        st.develop.source_size = (w as u32, h as u32);
        st.develop.image_size = (w as u32, h as u32);
        st.develop.center = (w as f32 / 2.0, h as f32 / 2.0);
    }
    // The device goes with the test, even a failing one, not with
    // the thread's last timer.
    struct Unload(Rc<Shared>);
    impl Drop for Unload {
        fn drop(&mut self) {
            if let Ok(mut st) = self.0.try_borrow_mut() {
                st.develop.renderer = None;
            }
        }
    }
    let _unload = Unload(state.clone());
    // The color's hue with the wheels out: the same however far
    // the wheel in hand already pushes it.
    let want = wheel_pick(&mut state.borrow_mut(), &app, color).0;
    let shown = || pick_stages(&mut state.borrow_mut(), &app, color).shown_hue;
    // The view's middle, in the window.
    let base = i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    let (mx, my) = view_middle(&app);
    let at = (base.x + mx, base.y + my);
    let ctrl_click = |shift: bool| {
        let key = |k: Key, down: bool| {
            let text: slint::SharedString = k.into();
            app.window().dispatch_event(if down {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            });
        };
        key(Key::Control, true);
        if shift {
            key(Key::Shift, true);
        }
        click(&app, at.0, at.1);
        if shift {
            key(Key::Shift, false);
        }
        key(Key::Control, false);
    };
    let near = |a: f32, b: f32| ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs() < 1.0;
    let wheel = |app: &App| {
        let hues = crate::panel::color::grade_values(app.get_grade_hues());
        let sats = crate::panel::color::grade_values(app.get_grade_saturations());
        (hues[1], sats[1])
    };

    // The mid-tones wheel pressed right of its center: in hand, at
    // a hue of 0 and some strength.
    app.invoke_grade_press(1, 0.8, 0.5, false, false);
    app.invoke_grade_release();
    assert_eq!(app.get_wheel_hand(), 1);
    let (_, strength) = wheel(&app);
    assert!(strength > 0.0);
    assert!(!near(shown(), want), "the wheel pushes the color shown");
    ctrl_click(false);
    let (hue, kept) = wheel(&app);
    assert!(near(hue, want), "{hue} for {want}");
    assert_eq!(kept, strength, "at its own strength");
    assert_eq!(state.borrow().develop.zoom, 0.0, "no zoom");
    // Again on the same spot, the wheel now pushing toward it: the
    // same hue, not the pushed one.
    ctrl_click(false);
    assert!(near(wheel(&app).0, want), "the pick does not chase itself");
    ctrl_click(true);
    let hue = wheel(&app).0;
    assert!(near(hue, want + 180.0), "the complement: {hue} for {want}");

    // The tint's ring likewise.
    app.invoke_tint_press(0.8, 0.5, false, false);
    app.invoke_tint_release();
    assert_eq!(app.get_wheel_hand(), crate::panel::color::TINT_HAND);
    ctrl_click(false);
    assert!(near(app.get_tint_hue(), want), "{want}");
    // At nothing, the pick gives it an amount, the color's chroma
    // over its reference.
    app.set_tint_amount(0.0);
    ctrl_click(false);
    let chroma = wheel_pick(&mut state.borrow_mut(), &app, color).1;
    assert!(chroma > 0.0, "a colored frame");
    let amount = app.get_tint_amount();
    assert!(amount > 0.0, "the pick shows");
    assert!(
        (amount - (chroma / 0.1).min(1.0)).abs() < 1e-5,
        "{amount} for {chroma}"
    );

    // Esc puts it down: a Ctrl-click sets nothing.
    crate::testing::press(&app, Key::Escape);
    assert_eq!(app.get_wheel_hand(), -1);
    let was = app.get_tint_hue();
    app.set_tint_hue(10.0);
    ctrl_click(false);
    assert_eq!(app.get_tint_hue(), 10.0, "nothing in hand");
    app.set_tint_hue(was);

    // Another section's control puts it down too.
    app.invoke_grade_press(1, 0.8, 0.5, false, false);
    app.invoke_grade_release();
    assert_eq!(app.get_wheel_hand(), 1);
    app.set_exposure(0.5);
    app.invoke_view_changed();
    assert_eq!(app.get_wheel_hand(), -1, "the exposure moved");
}

/// The view's middle, logical pixels.
fn view_middle(app: &App) -> (f32, f32) {
    let scale = app.window().scale_factor();
    (
        app.get_view_width() as f32 / scale / 2.0,
        app.get_view_height() as f32 / scale / 2.0,
    )
}

/// The hand over a tool in hand: with the heal brush, or a mask's
/// shape waiting for its drag, Shift held makes a drag on the
/// picture pan it and lay down nothing, and the tool is still in
/// hand after. Space held does the same, its zoom kept for a tap.
#[test]
fn shift_or_space_held_over_a_tool_pans_rather_than_paints() {
    use crate::testing::{drag, press};
    use slint::platform::{Key, WindowEvent};
    let (app, state, _worker) = hover_frame(1.0);
    state.borrow_mut().develop.zoom = 1.0;
    let key = |text: slint::SharedString, down: bool| {
        app.window().dispatch_event(if down {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
    };
    let shift = |down: bool| key(Key::Shift.into(), down);
    let space = |down: bool| key(" ".into(), down);
    let patches = || state.borrow().develop.edit.retouch.patches.len();
    let center = || state.borrow().develop.center;
    let path = [(600.0, 400.0), (620.0, 400.0), (680.0, 420.0)];

    app.invoke_retouch_toggled("Heal".into());
    let before = center();
    shift(true);
    drag(&app, &path);
    shift(false);
    assert_eq!(patches(), 0, "no stroke under the hand");
    assert_ne!(center(), before, "the drag panned");
    assert_eq!(app.get_retouch_mode(), "Heal", "the brush is still in hand");
    // Let go of, the brush paints again, and pans nothing.
    let before = center();
    drag(&app, &path);
    assert_eq!(patches(), 1, "the stroke");
    assert_eq!(center(), before);

    // Space held: a pan, and no zoom at its release.
    let before = center();
    space(true);
    drag(&app, &path);
    space(false);
    assert_eq!(patches(), 1);
    assert_ne!(center(), before);
    assert_eq!(
        state.borrow().develop.zoom,
        1.0,
        "a hold that panned does not zoom"
    );
    // A tap still zooms.
    space(true);
    space(false);
    assert_eq!(state.borrow().develop.zoom, 0.0, "a tap goes to Fit");
    app.invoke_retouch_toggled("Heal".into());
    assert_eq!(app.get_retouch_mode(), "");

    // A Linear in hand from M, waiting for the drag that places it:
    // Space pans; Shift holds the shape instead, placed on a step.
    state.borrow_mut().develop.zoom = 1.0;
    press(&app, "m");
    assert_eq!(app.get_placing(), "Linear");
    let before = center();
    space(true);
    drag(&app, &path);
    space(false);
    assert_ne!(center(), before);
    assert_eq!(app.get_placing(), "Linear", "still in hand");
    assert!(
        state.borrow().develop.edit.adjustments[0]
            .mask
            .components
            .is_empty(),
        "nothing placed"
    );
    let before = center();
    shift(true);
    drag(&app, &path);
    shift(false);
    assert_eq!(center(), before, "no pan");
    let st = state.borrow();
    let greycard_edit::mask::Shape::Linear { from, to } =
        st.develop.edit.adjustments[0].mask.components[0].shape
    else {
        panic!("a linear placed")
    };
    let a = (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees();
    assert!((a - 15.0).abs() < 1e-3, "on the step: {a}");
}

/// The middle button pans whatever is in hand: with a mask's
/// brush, or the heal brush, a middle drag pans and lays down
/// nothing, the tool still in hand and painting again after; a
/// left press while the middle pans is nothing either.
#[test]
fn a_middle_drag_pans_under_a_brush_and_leaves_it_in_hand() {
    use crate::testing::{drag, drag_with};
    use slint::platform::{PointerEventButton, WindowEvent};
    let (app, state, _worker) = hover_frame(1.0);
    state.borrow_mut().develop.zoom = 1.0;
    let middle = PointerEventButton::Middle;
    let center = || state.borrow().develop.center;
    let path = [(600.0, 400.0), (620.0, 400.0), (680.0, 420.0)];
    let strokes = || {
        let st = state.borrow();
        st.develop.edit.adjustments.first().map_or(0, |a| {
            a.mask
                .components
                .iter()
                .map(|c| match &c.shape {
                    greycard_edit::mask::Shape::Brush { strokes } => strokes.len(),
                    _ => 0,
                })
                .sum()
        })
    };

    app.set_panel_tab("Masks".into());
    app.invoke_new_mask();
    app.invoke_add_shape("Brush".into(), "Add".into());
    assert_eq!(app.get_placing(), "Brush");
    let before = center();
    drag_with(&app, middle, &path);
    assert_ne!(center(), before, "the middle drag panned");
    assert_eq!(strokes(), 0, "and laid no stroke");
    assert_eq!(app.get_placing(), "Brush", "the brush is still in hand");
    let before = center();
    drag(&app, &path);
    assert_eq!(strokes(), 1, "the left drag paints");
    assert_eq!(center(), before, "and pans nothing");

    // A left press and release while the middle pans: no stroke,
    // no point on the last one, and the pan the middle's alone,
    // ended by the left release, which takes the window's hold.
    let points = || {
        let st = state.borrow();
        st.develop.edit.adjustments[0]
            .mask
            .components
            .iter()
            .map(|c| match &c.shape {
                greycard_edit::mask::Shape::Brush { strokes } => {
                    strokes.iter().map(|s| s.points.len()).sum::<usize>()
                }
                _ => 0,
            })
            .sum::<usize>()
    };
    let (was, before) = (points(), center());
    let at = |p: (f32, f32)| slint::LogicalPosition::new(p.0, p.1);
    let left = PointerEventButton::Left;
    for event in [
        WindowEvent::PointerMoved {
            position: at(path[0]),
        },
        WindowEvent::PointerPressed {
            position: at(path[0]),
            button: middle,
        },
        WindowEvent::PointerMoved {
            position: at(path[1]),
        },
        WindowEvent::PointerPressed {
            position: at(path[1]),
            button: left,
        },
        WindowEvent::PointerMoved {
            position: at(path[2]),
        },
        WindowEvent::PointerReleased {
            position: at(path[2]),
            button: left,
        },
        WindowEvent::PointerReleased {
            position: at(path[2]),
            button: middle,
        },
    ] {
        app.window().dispatch_event(event);
    }
    assert_eq!(strokes(), 1, "the left press in a middle pan painted");
    assert_eq!(points(), was, "nor did it add to the last stroke");
    let moved = (path[2].0 - path[0].0, path[2].1 - path[0].1);
    assert_eq!(
        center(),
        (before.0 - moved.0, before.1 - moved.1),
        "the middle's pan alone"
    );
    app.invoke_stop_placing();

    let patches = || state.borrow().develop.edit.retouch.patches.len();
    app.invoke_retouch_toggled("Heal".into());
    let before = center();
    drag_with(&app, middle, &path);
    assert_ne!(center(), before, "the middle drag panned");
    assert_eq!(patches(), 0, "no repair under the middle");
    assert_eq!(app.get_retouch_mode(), "Heal");
    drag(&app, &path);
    assert_eq!(patches(), 1, "the left drag repairs");
}

/// A middle click on the picture goes to 200% there and a second
/// one back to the zoom and the place it came from, fit or not; a
/// middle drag pans and zooms nothing; a left click is still 1:1.
#[test]
fn a_middle_click_toggles_200_percent_and_a_middle_drag_does_not() {
    use crate::testing::{click, click_with, drag_with};
    use slint::platform::PointerEventButton;
    let (app, state, _worker) = hover_frame(1.0);
    let middle = PointerEventButton::Middle;
    let zoom = || state.borrow().develop.zoom;
    let center = || state.borrow().develop.center;
    let (x, y) = view_middle(&app);
    let (x, y) = (x + 100.0, y + 50.0);

    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 2.0, "fit to 200%");
    assert_ne!(center(), (3000.0, 2000.0), "about the point clicked");
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 0.0, "back to fit");
    assert_eq!(center(), (3000.0, 2000.0));

    // From 1:1 somewhere off the middle, and back there.
    {
        let mut st = state.borrow_mut();
        st.develop.zoom = 1.0;
        st.develop.center = (2000.0, 1500.0);
    }
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 2.0);
    click_with(&app, middle, x, y);
    assert_eq!(
        (zoom(), center()),
        (1.0, (2000.0, 1500.0)),
        "back to 1:1 there"
    );

    // A middle drag pans, and its release zooms nothing.
    let before = center();
    drag_with(&app, middle, &[(x, y), (x + 10.0, y), (x + 60.0, y + 20.0)]);
    assert_eq!(zoom(), 1.0, "no toggle after a drag");
    assert_ne!(center(), before);

    // The left click's own toggle is as it was.
    state.borrow_mut().develop.zoom = 0.0;
    click(&app, x, y);
    assert_eq!(zoom(), 1.0);
}

/// The middle button held in a left drag holds the line: a linear
/// lands on Shift's step, and a brush's stroke is one straight
/// segment from its press to the release, however the pointer
/// wandered. Released first, the middle ends the stroke there,
/// and the left release after lays nothing more. A later plain
/// stroke is free again.
#[test]
fn the_middle_held_in_a_drag_holds_a_gradient_and_a_stroke_straight() {
    use crate::testing::press;
    use greycard_edit::mask::Shape;
    use slint::platform::{PointerEventButton, WindowEvent};
    let (app, state, _worker) = hover_frame(1.0);
    let (left, middle) = (PointerEventButton::Left, PointerEventButton::Middle);
    let at = |p: (f32, f32)| slint::LogicalPosition::new(p.0, p.1);
    let send = |events: &[(char, (f32, f32), PointerEventButton)]| {
        for &(kind, p, button) in events {
            let position = at(p);
            app.window().dispatch_event(match kind {
                'm' => WindowEvent::PointerMoved { position },
                'p' => WindowEvent::PointerPressed { position, button },
                _ => WindowEvent::PointerReleased { position, button },
            });
        }
    };
    let (a, b, c, d) = (
        (600.0, 400.0),
        (640.0, 380.0),
        (700.0, 450.0),
        (800.0, 437.0),
    );

    // A linear from M: the middle pressed partway, the release
    // off any step.
    press(&app, "m");
    assert_eq!(app.get_placing(), "Linear");
    send(&[
        ('m', a, left),
        ('p', a, left),
        ('m', b, left),
        ('p', b, middle),
        ('m', d, left),
        ('r', d, left),
        ('r', d, middle),
    ]);
    assert!(!app.get_place_line(), "let go with the gesture");
    let shape = |k: usize| {
        state.borrow().develop.edit.adjustments[0].mask.components[k]
            .shape
            .clone()
    };
    let Shape::Linear { from, to } = shape(0) else {
        panic!("a linear placed")
    };
    let angle = (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees();
    let r = angle.rem_euclid(15.0);
    assert!(r.min(15.0 - r) < 1e-3, "on Shift's step: {angle}");

    // Another linear, the middle let go first: placed where the
    // middle's release was, on the step, and the left release
    // after it places nothing more.
    press(&app, "m");
    send(&[
        ('m', a, left),
        ('p', a, left),
        ('m', b, left),
        ('p', b, middle),
        ('m', d, left),
        ('r', d, middle),
        ('m', c, left),
        ('r', c, left),
    ]);
    assert_eq!(app.get_placing(), "", "placed and put down");
    let linears: Vec<([f32; 2], [f32; 2])> = state
        .borrow()
        .develop
        .edit
        .adjustments
        .iter()
        .flat_map(|a| a.mask.components.iter())
        .filter_map(|c| match c.shape {
            Shape::Linear { from, to } => Some((from, to)),
            _ => None,
        })
        .collect();
    assert_eq!(linears.len(), 2, "one more, and only one");
    let (from, to) = linears[1];
    let angle = (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees();
    let r = angle.rem_euclid(15.0);
    assert!(r.min(15.0 - r) < 1e-3, "on Shift's step: {angle}");

    // A brush: a wandering stroke, the middle pressed partway.
    app.invoke_add_shape("Brush".into(), "Add".into());
    assert_eq!(app.get_placing(), "Brush");
    send(&[
        ('m', a, left),
        ('p', a, left),
        ('m', b, left),
        ('m', c, left),
        ('p', c, middle),
        ('m', b, left),
        ('m', d, left),
        ('r', d, left),
        ('r', d, middle),
    ]);
    // The brush placed: the last Brush shape of any mask.
    let strokes = |_: usize| {
        state
            .borrow()
            .develop
            .edit
            .adjustments
            .iter()
            .flat_map(|a| a.mask.components.iter())
            .filter_map(|c| match &c.shape {
                Shape::Brush { strokes } => Some(strokes.clone()),
                _ => None,
            })
            .next_back()
            .expect("a brush")
    };
    let points = |k: usize| strokes(k).last().unwrap().points.clone();
    let view = |p: [f32; 2]| {
        let st = state.borrow();
        source_to_view(&st, &app, p[0], p[1])
    };
    // The view's points are its own; the events', the window's.
    let base = i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    let near = |p: [f32; 2], q: (f32, f32)| {
        let v = view(p);
        (v.0 + base.x - q.0).abs() < 0.5 && (v.1 + base.y - q.1).abs() < 0.5
    };
    let line = points(1);
    assert_eq!(line.len(), 2, "one straight segment: {line:?}");
    assert!(near(line[0], a) && near(line[1], d), "{line:?}");

    // The middle let go first: the stroke ends where it was, and
    // the moves and the left release after it add nothing.
    send(&[
        ('m', a, left),
        ('p', a, left),
        ('m', b, left),
        ('p', b, middle),
        ('m', c, left),
        ('r', c, middle),
        ('m', d, left),
        ('r', d, left),
    ]);
    assert_eq!(strokes(1).len(), 2);
    let line = points(1);
    assert_eq!(line.len(), 2, "{line:?}");
    assert!(
        near(line[0], a) && near(line[1], c),
        "ended at the middle's release: {line:?}"
    );
    assert!(!app.get_place_line());

    // A plain stroke after: free, every point kept.
    send(&[
        ('m', a, left),
        ('p', a, left),
        ('m', b, left),
        ('m', c, left),
        ('m', d, left),
        ('r', d, left),
    ]);
    assert_eq!(strokes(1).len(), 3);
    assert!(points(1).len() > 2, "a free stroke");
    assert_eq!(app.get_placing(), "Brush", "the brush stays in hand");
}

/// With nothing in hand, a left press made while the middle pans
/// drives nothing: the pan is the middle's alone, however the
/// pointer goes after the left press.
#[test]
fn a_left_press_in_a_middle_pan_drives_nothing() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let (app, state, _worker) = hover_frame(1.0);
    state.borrow_mut().develop.zoom = 1.0;
    let (left, middle) = (PointerEventButton::Left, PointerEventButton::Middle);
    let at = |x: f32| slint::LogicalPosition::new(x, 400.0);
    let before = state.borrow().develop.center;
    for event in [
        WindowEvent::PointerMoved {
            position: at(600.0),
        },
        WindowEvent::PointerPressed {
            position: at(600.0),
            button: middle,
        },
        WindowEvent::PointerMoved {
            position: at(610.0),
        },
        WindowEvent::PointerPressed {
            position: at(610.0),
            button: left,
        },
        WindowEvent::PointerMoved {
            position: at(620.0),
        },
        WindowEvent::PointerReleased {
            position: at(620.0),
            button: left,
        },
        WindowEvent::PointerMoved {
            position: at(700.0),
        },
        WindowEvent::PointerReleased {
            position: at(700.0),
            button: middle,
        },
    ] {
        app.window().dispatch_event(event);
    }
    let after = state.borrow().develop.center;
    assert_eq!(
        after,
        (before.0 - 20.0, before.1),
        "20 px of the middle's pan"
    );
    assert_eq!(state.borrow().develop.zoom, 1.0, "and no click's zoom");
}

/// A middle drag pans from a mask's handle, a repair's handle, the
/// level, and the guide, and none of them takes the press as its
/// own; a pan whose area goes away mid-drag (a key put the level
/// down) ends, and a hover after it pans nothing.
#[test]
fn a_middle_drag_pans_from_a_handle_the_level_and_the_guide() {
    use crate::Pt;
    use crate::testing::{click, drag_with};
    use slint::platform::{PointerEventButton, WindowEvent};
    use std::cell::Cell;
    let (app, state, _worker) = hover_frame(1.0);
    state.borrow_mut().develop.zoom = 1.0;
    let middle = PointerEventButton::Middle;
    let base = i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    let center = || state.borrow().develop.center;
    let p = (base.x + 500.0, base.y + 300.0);
    let pans = || {
        let before = center();
        drag_with(
            &app,
            middle,
            &[p, (p.0 + 10.0, p.1), (p.0 + 40.0, p.1 + 20.0)],
        );
        center() == (before.0 - 40.0, before.1 - 20.0)
    };
    let took = Rc::new(Cell::new(false));
    let handles = || ModelRc::new(VecModel::from(vec![Pt { x: 500.0, y: 300.0 }]));

    app.set_mask_kind("Linear".into());
    app.set_mask_handles(handles());
    let t = took.clone();
    app.on_mask_grabbed(move |_, _| t.set(true));
    assert!(pans(), "from a mask's handle");
    assert!(!took.get());
    click(&app, p.0, p.1);
    assert!(took.get(), "the handle is there to press");
    app.set_mask_kind("".into());

    took.set(false);
    app.set_retouch_mode("Heal".into());
    app.set_patch_handles(ModelRc::new(VecModel::from(vec![
        Pt { x: 500.0, y: 300.0 },
        Pt { x: 700.0, y: 300.0 },
    ])));
    let t = took.clone();
    app.on_patch_grabbed(move |_| t.set(true));
    assert!(pans(), "from a repair's handle");
    assert!(!took.get());
    click(&app, p.0, p.1);
    assert!(took.get(), "the handle is there to press");
    app.set_retouch_mode("".into());

    took.set(false);
    let t = took.clone();
    app.on_level_end(move |_, _, _, _| t.set(true));
    app.set_level_mode(true);
    assert!(pans(), "from the level");
    assert!(!took.get(), "no level drawn");
    let t = took.clone();
    app.on_guide_stroke(move |_, _, _, _| t.set(true));
    app.set_level_mode(false);
    app.set_guide_mode("Vertical".into());
    assert!(pans(), "from the guide");
    assert!(!took.get(), "no guide drawn");
    app.set_guide_mode("".into());

    // The level taken away mid-pan: its release never comes.
    app.set_level_mode(true);
    let at = |x: f32| slint::LogicalPosition::new(x, p.1);
    for event in [
        WindowEvent::PointerMoved { position: at(p.0) },
        WindowEvent::PointerPressed {
            position: at(p.0),
            button: middle,
        },
        WindowEvent::PointerMoved {
            position: at(p.0 + 20.0),
        },
    ] {
        app.window().dispatch_event(event);
    }
    app.set_level_mode(false);
    let before = center();
    for x in [p.0 + 40.0, p.0 + 80.0] {
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position: at(x) });
    }
    assert_eq!(center(), before, "a hover after pans nothing");
}

/// The 200% click is the picture's: on the canvas round a fitted
/// picture, or in the compare view, it does nothing. Its memory is
/// the frame's: back on another frame it fits, and any other zoom
/// change between forgets it.
#[test]
fn the_200_percent_click_is_the_pictures_and_its_memory_the_frames() {
    use crate::testing::click_with;
    use slint::platform::PointerEventButton;
    let app = window(2);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    let set = |zoom: f32, center: (f32, f32)| {
        let mut st = state.borrow_mut();
        st.develop.image_size = (6000, 4000);
        st.develop.zoom = zoom;
        st.develop.center = center;
    };
    set(0.0, (3000.0, 2000.0));
    let middle = PointerEventButton::Middle;
    let base = i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    let zoom = || state.borrow().develop.zoom;
    let center = || state.borrow().develop.center;
    let (mx, my) = view_middle(&app);
    let (x, y) = (base.x + mx, base.y + my);

    click_with(&app, middle, base.x + 2.0, base.y + 2.0);
    assert_eq!(zoom(), 0.0, "the canvas takes none");

    // From 1:1 on the first frame, 200%; on the second frame, back
    // fits rather than land on the first frame's place.
    set(1.0, (2000.0, 1500.0));
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 2.0);
    app.invoke_select(1);
    set(2.0, (3000.0, 2000.0));
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 0.0, "another frame's memory fits");

    // Any other zoom change forgets it: the wheel at 200% and back.
    set(1.0, (2000.0, 1500.0));
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 2.0);
    app.invoke_zoom(1.25, x - base.x, y - base.y);
    app.invoke_zoom(0.8, x - base.x, y - base.y);
    assert!((zoom() - 2.0).abs() < 1e-3);
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 0.0, "forgotten: fit");
    // A pan at 200% keeps it.
    set(1.0, (2000.0, 1500.0));
    click_with(&app, middle, x, y);
    crate::testing::drag_with(&app, middle, &[(x, y), (x + 10.0, y), (x + 60.0, y)]);
    click_with(&app, middle, x, y);
    assert_eq!((zoom(), center()), (1.0, (2000.0, 1500.0)));
    // A turn between swaps the picture's size: back fits.
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 2.0);
    state.borrow_mut().develop.image_size = (4000, 6000);
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 0.0, "a turned picture's memory fits");
    set(1.0, (2000.0, 1500.0));

    // In the compare view a click is nobody's zoom.
    let mut cull = crate::panel::cull::Cull::new("".into(), 2048);
    cull.compare = 2;
    state.borrow_mut().culling.hold = Some(cull);
    click_with(&app, middle, x, y);
    assert_eq!(zoom(), 1.0, "the compare view takes none");
}

/// Under a Straighten, a Shift drag placing a Linear steps its
/// angle as the screen shows it: a near-level drag lands level on
/// the view, which in the picture's own units is tilted.
#[test]
fn a_shift_placed_linear_is_level_on_a_straightened_view() {
    use crate::testing::{drag, press};
    use slint::platform::{Key, WindowEvent};
    let (app, state, _worker) = hover_frame(1.0);
    app.set_geo_angle(5.0);
    press(&app, "m");
    assert_eq!(app.get_placing(), "Linear");
    let shift = |down: bool| {
        let text = Key::Shift.into();
        app.window().dispatch_event(if down {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
    };
    shift(true);
    drag(&app, &[(600.0, 400.0), (620.0, 400.0), (800.0, 410.0)]);
    shift(false);
    let st = state.borrow();
    let greycard_edit::mask::Shape::Linear { from, to } =
        st.develop.edit.adjustments[0].mask.components[0].shape
    else {
        panic!("a linear placed")
    };
    let (a, b) = (
        source_to_view(&st, &app, from[0], from[1]),
        source_to_view(&st, &app, to[0], to[1]),
    );
    assert!((a.1 - b.1).abs() < 0.01, "level on the view: {a:?} {b:?}");
    assert!(b.0 - a.0 > 100.0, "{a:?} {b:?}");
    let tilt = (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees();
    assert!(
        (tilt.abs() - 5.0).abs() < 0.01,
        "tilted in the picture: {tilt}"
    );
}

/// A focused repair slider passes Space and Shift on to the
/// window's keys, while their releases stay with the slider: such
/// a key never takes up the hand. Space zooms at once, as it always
/// has from a slider, and the next drag paints.
#[test]
fn a_key_a_slider_passes_on_never_leaves_the_hand_up() {
    use crate::testing::{click, drag, press};
    use slint::platform::{Key, WindowEvent};
    let (app, state, _worker) = hover_frame(1.0);
    state.borrow_mut().develop.zoom = 1.0;
    app.invoke_retouch_toggled("Heal".into());
    app.set_panel_tab("Retouch".into());
    let (at, size) = crate::testing::by_id(&app, "RetouchSection::size-slider");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert_ne!(
        app.global::<crate::SliderKeys>().get_holder(),
        0,
        "the slider has the keys"
    );
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    press(&app, " ");
    assert_eq!(state.borrow().develop.zoom, 0.0, "the tap zoomed at once");
    drag(&app, &[(600.0, 400.0), (620.0, 400.0), (680.0, 420.0)]);
    assert_eq!(
        state.borrow().develop.edit.retouch.patches.len(),
        1,
        "the drag painted"
    );
}

/// The view onto the frame as the scopes' analysis picture covers
/// it, pure geometry: the middle at the fit is the frame's middle,
/// the grey around the picture is on none of it, and at 1:1 an
/// off-center view, a crop, crop mode's whole plane and a quarter
/// turn (of the edit, or one the develop has yet to catch up with)
/// each give the fraction their arithmetic says, at a scale factor
/// of one and of two.
#[test]
fn the_view_maps_onto_the_frame_the_scopes_cover() {
    for scale in [1.0f32, 2.0] {
        let (app, state, _worker) = hover_frame(scale);
        assert_eq!(app.window().scale_factor(), scale);
        let (mx, my) = view_middle(&app);
        let near = |got: Option<(f32, f32)>, want: (f32, f32), what: &str| {
            let got = got.unwrap_or_else(|| panic!("{what} at {scale}: off the picture"));
            assert!(
                (got.0 - want.0).abs() < 1e-4 && (got.1 - want.1).abs() < 1e-4,
                "{what} at {scale}: {got:?}, not {want:?}"
            );
        };
        let at = |x: f32, y: f32| view_to_frame(&state.borrow(), &app, x, y).map(|(at, _)| at);
        near(at(mx, my), (0.5, 0.5), "the fit's middle");

        // Small in the view: grey all about it.
        state.borrow_mut().develop.zoom = 0.05;
        assert_eq!(at(2.0, 2.0), None, "the grey at {scale}");
        near(at(mx, my), (0.5, 0.5), "the small picture's middle");

        // At 1:1 off the frame's middle: a logical pixel is `scale`
        // of the frame's.
        {
            let mut st = state.borrow_mut();
            st.develop.zoom = 1.0;
            st.develop.center = (1000.0, 500.0);
        }
        let (x, y) = (mx + 30.0, my - 20.0);
        let (dx, dy) = (30.0 * scale, -20.0 * scale);
        near(
            at(x, y),
            ((1000.0 + dx) / 6000.0, (500.0 + dy) / 4000.0),
            "1:1 off the middle",
        );

        // A crop of the middle quarter: its frame is the crop, and
        // in crop mode the whole plane about it.
        app.set_geo_has_crop(true);
        app.set_geo_crop_x(0.25);
        app.set_geo_crop_y(0.25);
        app.set_geo_crop_w(0.5);
        app.set_geo_crop_h(0.5);
        near(
            at(x, y),
            ((1000.0 + dx) / 3000.0, (500.0 + dy) / 2000.0),
            "the crop",
        );
        app.set_crop_mode(true);
        near(
            at(x, y),
            ((1000.0 + dx) / 6000.0, (500.0 + dy) / 4000.0),
            "crop mode's bounds",
        );
        app.set_crop_mode(false);
        app.set_geo_has_crop(false);

        // A quarter turn stands the frame on end, the edit's own
        // and one the develop on the GPU lags by alike.
        let turned = ((1000.0 + dx) / 4000.0, (500.0 + dy) / 6000.0);
        app.set_geo_turns(1);
        near(at(x, y), turned, "the edit's quarter turn");
        app.set_geo_turns(0);
        {
            let mut st = state.borrow_mut();
            let turn = current_turn(&st);
            st.develop.shown_turn = Some((0, (turn + 3) % 4));
        }
        near(at(x, y), turned, "a turn the develop lags by");

        // Not the open frame's own develop: another frame's, or one
        // held while the next develops, which the scopes measure by
        // another edit than the panel's.
        state.borrow_mut().develop.shown_turn = Some((7, 0));
        assert_eq!(at(x, y), None, "another frame's develop");
        {
            let mut st = state.borrow_mut();
            st.develop.shown_turn = Some((0, current_turn(&st)));
            st.develop.held = Some(Edit::default());
        }
        assert_eq!(at(x, y), None, "a held develop");
    }
}

/// The readout and the marks are those of the analysis picture
/// under the pointer; a reading the same as the last leaves the
/// scope be; a zoom under the resting pointer reads the new spot;
/// and the camera's picture standing in, or a held develop, takes
/// them off.
#[test]
fn the_readout_reads_the_spot_under_the_pointer() {
    let (app, state, _worker) = hover_frame(1.0);
    // The analysis picture: red on its left half, blue on its
    // right, all selected.
    let picture: Vec<[u8; 4]> = (0..16 * 8)
        .map(|i| {
            let (r, b) = if i % 16 < 8 { (200, 40) } else { (40, 200) };
            [r, 40, b, 0xff]
        })
        .collect();
    HOVER_PICTURE.with(|p| {
        *p.borrow_mut() = Some(greycard_viewport::Rgba8 {
            width: 16,
            height: 8,
            pixels: picture,
        })
    });
    let red = scope::readout([200.0, 40.0, 40.0].map(|c| c / 255.0));
    let blue = scope::readout([40.0, 40.0, 200.0].map(|c| c / 255.0));
    let (mx, my) = view_middle(&app);
    let show = |x: f32, y: f32| {
        let mut st = state.borrow_mut();
        st.develop.hover = Some((x, y));
        show_hover(&mut st, &app);
    };
    // A quarter of the way across the fitted frame: red.
    let fit = effective_zoom(
        &state.borrow(),
        app.get_view_width() as u32,
        app.get_view_height() as u32,
    );
    let quarter = mx - 1500.0 * fit;
    show(quarter, my);
    assert_eq!(app.get_scope_readout(), red.as_str());
    assert_eq!(
        app.get_scope_marks().row_count(),
        4,
        "three channels and the luma"
    );
    assert!(!app.get_scope_marks_outside());

    // A move that reads the same pixel changes nothing shown.
    app.set_scope_readout("left be".into());
    show(quarter + 0.5, my + 0.5);
    assert_eq!(app.get_scope_readout(), "left be");
    state.borrow_mut().develop.hover_shown = None;
    show(quarter, my);
    assert_eq!(app.get_scope_readout(), red.as_str());

    // Zoomed in on the right half with the pointer resting: the
    // spot under it is blue now.
    {
        let mut st = state.borrow_mut();
        st.develop.zoom = 1.0;
        st.develop.center = (4500.0, 2000.0);
    }
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), blue.as_str());

    // The camera's picture standing in: none of it describes that.
    app.set_placeholder(true);
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), "");
    assert_eq!(app.get_scope_marks().row_count(), 0);
    app.set_placeholder(false);
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), blue.as_str());

    // This frame developed again under the resting pointer: the
    // upload drops the analysis, and what is shown stays until
    // the new one is back, rather than blink.
    let picture = HOVER_PICTURE.with(|p| p.borrow_mut().replace(Default::default()));
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), blue.as_str(), "a re-develop");
    assert_eq!(app.get_scope_marks().row_count(), 4);
    // Not on another scope, whose marks those are not.
    state.borrow_mut().develop.scope = scope::Scope::Vector;
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), "", "another scope");
    state.borrow_mut().develop.scope = scope::Scope::Rgb;
    HOVER_PICTURE.with(|p| *p.borrow_mut() = picture.clone());
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), blue.as_str());

    // A develop held under the last frame's edit: none of it.
    state.borrow_mut().develop.held = Some(Edit::default());
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), "");
    assert_eq!(app.get_scope_marks().row_count(), 0);
    state.borrow_mut().develop.held = None;
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), blue.as_str());

    // A step to another frame, its develop not landed: the one on
    // the GPU is the last frame's, and the readout goes.
    HOVER_PICTURE.with(|p| *p.borrow_mut() = Some(Default::default()));
    state.borrow_mut().develop.shown_turn = Some((7, 0));
    show_hover(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_scope_readout(), "", "a frame step");
    assert_eq!(app.get_scope_marks().row_count(), 0);
    HOVER_PICTURE.with(|p| p.borrow_mut().take());
}

/// The pointer leaving while the state is borrowed still takes the
/// color under it off the scope, a frame's time later.
#[test]
fn a_leave_while_the_state_is_borrowed_still_clears_the_readout() {
    let app = window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    state.borrow_mut().develop.hover = Some((10.0, 10.0));
    app.set_scope_readout("R 1  G 2  B 3  L 2".into());
    {
        let _held = state.borrow_mut();
        app.invoke_view_hovered(false, 10.0, 10.0);
    }
    assert_eq!(app.get_scope_readout(), "R 1  G 2  B 3  L 2", "not yet");
    // Not tried again at once, so a borrow held across a nested
    // event loop does not spin it.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    assert_eq!(app.get_scope_readout(), "R 1  G 2  B 3  L 2", "not at once");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    assert_eq!(state.borrow().develop.hover, None);
    assert_eq!(app.get_scope_readout(), "");
}

#[test]
fn a_sheet_or_a_tool_asked_for_is_on_screen() {
    let app = crate::testing::window(1);
    let (_state, _worker) = crate::testing::retouch_state(&app);
    assert_eq!(Shown::sheet("export"), Ok(Shown::Export));
    assert_eq!(Shown::tool("guide"), Ok(Shown::Guide));
    assert_eq!(Shown::sheet("sync"), Ok(Shown::Sync));
    assert_eq!(Shown::sheet("synced"), Ok(Shown::Synced));
    assert_eq!(Shown::sheet("preset-onto-set"), Ok(Shown::PresetOntoSet));
    assert_eq!(Shown::sheet("preset-remove"), Ok(Shown::PresetRemove));
    assert_eq!(Shown::sheet("paste"), Ok(Shown::Paste));
    assert_eq!(Shown::sheet("pasted"), Ok(Shown::Pasted));
    assert!(Shown::sheet("guide").is_err());
    assert!(Shown::tool("export").is_err());

    Shown::Export.open(&app);
    assert!(app.get_export_open());
    Shown::Fetch.open(&app);
    assert!(app.get_fetch_open());
    assert!(!app.get_fetch_title().is_empty());
    app.set_fetch_open(false);
    assert_eq!(Shown::sheet("lenses"), Ok(Shown::Lenses));
    assert_eq!(Shown::sheet("settings"), Ok(Shown::Settings));
    Shown::Lenses.open(&app);
    assert!(app.get_fetch_open());
    assert!(
        app.get_fetch_text()
            .starts_with(&crate::panel::assets::lenses_why())
    );

    app.set_fetch_open(false);
    Shown::Settings.open(&app);
    assert!(app.get_settings_open());
    // No presets, nothing asked; else the longest name.
    Shown::PresetRemove.open(&app);
    assert_eq!(app.get_preset_confirm_remove(), -1);
    app.set_preset_names(slint::ModelRc::new(slint::VecModel::from(vec![
        slint::SharedString::from("Muted Slide"),
        "Red-Filter Mono".into(),
        "Warm Negative".into(),
    ])));
    Shown::PresetRemove.open(&app);
    assert_eq!(app.get_preset_confirm_remove(), 1);
    app.set_preset_confirm_remove(-1);
    assert_eq!(Shown::sheet("preset-report"), Ok(Shown::PresetReport));
    assert_eq!(
        Shown::sheet("lightroom-written"),
        Ok(Shown::Lightroom(LightroomStep::Written))
    );
    assert_eq!(
        Shown::sheet("rawtherapee"),
        Ok(Shown::RawTherapee(RawTherapeeStep::Report))
    );
    assert_eq!(
        Shown::sheet("rawtherapee-offer"),
        Ok(Shown::RawTherapee(RawTherapeeStep::Offer))
    );
    assert_eq!(
        Shown::sheet("rawtherapee-written"),
        Ok(Shown::RawTherapee(RawTherapeeStep::Written))
    );
    // With no --lightroom catalog it opens nothing.
    Shown::Lightroom(LightroomStep::Roots).open(&app);
    assert!(!app.get_lr_open());
    Shown::PresetReport.open(&app);
    assert!(app.get_preset_report_open());
    assert_eq!(app.get_preset_report_title(), "Imported 2 of 3 presets");
    app.set_preset_report_open(false);

    // A tool brings the Crop tab with it.
    assert_eq!(app.get_panel_tab(), "Develop");
    Shown::Crop.open(&app);
    assert_eq!(app.get_panel_tab(), "Crop");
    assert!(app.get_crop_mode());
    Shown::Level.open(&app);
    assert!(app.get_level_mode());
    Shown::Guide.open(&app);
    assert_eq!(app.get_guide_mode(), "Vertical");
}

#[test]
fn tab_puts_the_panels_away_and_brings_them_back() {
    let app = crate::testing::window(3);
    let (_state, _worker) = crate::testing::retouch_state(&app);
    let tab = slint::platform::Key::Tab;
    let hidden = |app: &App| {
        (
            app.get_left_hidden(),
            app.get_right_hidden(),
            app.get_strip_hidden(),
        )
    };
    assert_eq!(hidden(&app), (false, false, false));
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (true, true, true), "Tab hides them");
    crate::testing::press(&app, tab);
    assert_eq!(
        hidden(&app),
        (false, false, false),
        "and Tab again shows them"
    );

    // One put away on its own: Tab puts the rest away with it,
    // and brings back only those it put away.
    use slint::platform::Key;
    for (key, alone) in [
        (Key::F7, (true, false, false)),
        (Key::F8, (false, true, false)),
        (Key::F6, (false, false, true)),
    ] {
        crate::testing::press(&app, key);
        assert_eq!(hidden(&app), alone, "{key:?} alone");
        crate::testing::press(&app, tab);
        assert_eq!(hidden(&app), (true, true, true));
        crate::testing::press(&app, tab);
        assert_eq!(hidden(&app), alone, "{key:?}'s pane stays away");
        crate::testing::press(&app, key);
    }
    assert_eq!(hidden(&app), (false, false, false));

    // The tester's: the strip put away by its button, Tab twice.
    app.set_strip_hidden(true);
    crate::testing::press(&app, tab);
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, false, true), "the strip stays away");
    app.set_strip_hidden(false);

    // All three put away by hand: Tab brings all three back.
    for key in [Key::F6, Key::F7, Key::F8] {
        crate::testing::press(&app, key);
    }
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, false, false), "never a dead key");

    // Put away by Tab, then by hand the right panel shown and put
    // away again: Tab leaves it away and brings back the rest.
    crate::testing::press(&app, tab);
    crate::testing::press(&app, Key::F8);
    crate::testing::press(&app, Key::F8);
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, true, false), "the hand since");
    crate::testing::press(&app, Key::F8);

    // Put away by Tab, one shown by hand: Tab puts it away with
    // the rest, and brings back all three.
    crate::testing::press(&app, tab);
    crate::testing::press(&app, Key::F8);
    assert_eq!(hidden(&app), (true, false, true));
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (true, true, true));
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, false, false), "all Tab put away");

    // A glance at the strip between two Tabs: all three back.
    crate::testing::press(&app, tab);
    crate::testing::press(&app, Key::F6);
    crate::testing::press(&app, tab);
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, false, false), "a glance at the strip");

    // Tab in the loupe, the grid's Tab bringing the left pane out,
    // a frame opened, Tab twice: all three back, nothing having
    // been put away by hand.
    crate::testing::press(&app, tab);
    app.set_grid_open(true);
    crate::testing::press(&app, tab);
    app.set_grid_open(false);
    assert_eq!(hidden(&app), (false, true, true));
    crate::testing::press(&app, tab);
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, false, false), "the grid's left pane");

    // The strip's arrow, a hand like F6: shown and put away again
    // while Tab has all three away, it stays away.
    crate::testing::press(&app, tab);
    let arrow_click = |app: &App| {
        let (at, arrow) = crate::testing::by_id(app, "App::strip-arrow");
        crate::testing::click(app, at.x + arrow.width / 2.0, at.y + arrow.height / 2.0);
    };
    arrow_click(&app);
    assert_eq!(hidden(&app), (true, true, false), "the arrow shows it");
    arrow_click(&app);
    assert_eq!(hidden(&app), (true, true, true));
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, false, true), "the arrow's hand kept");
    crate::testing::press(&app, Key::F6);

    // Grid and loupe: the strip away, Tab in the loupe, the grid's
    // Tab twice (the left pane out and away again, the hand's last
    // word), then Tab in the loupe: the panel back, the strip and
    // the left pane away.
    crate::testing::press(&app, Key::F6);
    crate::testing::press(&app, tab);
    app.set_grid_open(true);
    crate::testing::press(&app, tab);
    crate::testing::press(&app, tab);
    app.set_grid_open(false);
    assert_eq!(hidden(&app), (true, true, true));
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (true, false, true), "the grid's Tab a hand");
    crate::testing::press(&app, Key::F7);
    // Without the grid's Tab, the loupe's memory holds across.
    crate::testing::press(&app, tab);
    app.set_grid_open(true);
    app.set_grid_open(false);
    crate::testing::press(&app, tab);
    assert_eq!(hidden(&app), (false, false, true), "kept across the grid");
    crate::testing::press(&app, Key::F6);
    assert_eq!(hidden(&app), (false, false, false));

    // A sheet has the keys: Tab under it is not this one.
    app.set_export_open(true);
    crate::testing::press(&app, tab);
    crate::testing::press(&app, Key::F8);
    assert_eq!(hidden(&app), (false, false, false), "not under a sheet");
    app.set_export_open(false);

    // Over the grid, which has the window already, Tab is the
    // left pane's alone, and F6 nobody's.
    app.set_grid_open(true);
    crate::testing::press(&app, tab);
    crate::testing::press(&app, Key::F6);
    assert_eq!(
        hidden(&app),
        (true, false, false),
        "over the grid Tab is the left pane's alone"
    );
}

#[test]
fn each_panel_puts_away_on_its_own() {
    let app = crate::testing::window(3);
    let (_state, _worker) = crate::testing::retouch_state(&app);
    use slint::platform::Key;
    crate::testing::press(&app, Key::F8);
    assert!(app.get_right_hidden(), "F8 the develop panel");
    assert!(!app.get_left_hidden() && !app.get_strip_hidden());
    crate::testing::press(&app, Key::F6);
    assert!(app.get_strip_hidden(), "F6 the strip");
    assert!(!app.get_left_hidden());
    crate::testing::press(&app, Key::F8);
    crate::testing::press(&app, Key::F6);
    assert!(
        !app.get_right_hidden() && !app.get_strip_hidden(),
        "and back"
    );

    // What the settings keep is what the window shows.
    crate::testing::press(&app, Key::F7);
    let kept = crate::panel::startup::remember(&app);
    assert!(kept.hide_left && !kept.hide_right && !kept.hide_strip);
}

/// The viewport frames the open frame's develop by the camera's
/// frame as the export does (`stages::Facts::camera`), at each turn
/// of the frame and while a turn pressed since lags the develop;
/// a raw whose camera framed nothing, and a merge, are not framed.
#[test]
fn the_viewport_frames_by_the_cameras_frame_as_the_export() {
    use greycard_core::raw::{CameraFrame, Orientation, Rect};
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let area = Rect {
        x: 8,
        y: 8,
        width: 2,
        height: 2,
    };
    let mut frame = daylight_frame();
    frame.crop = Some(area);
    frame.orientation = Orientation::Rotate90;
    frame.shot.camera_frame = Some(CameraFrame {
        area,
        frame: Rect {
            x: 8,
            y: 8,
            width: 2,
            height: 1,
        },
        orientation: Orientation::Rotate90,
    });
    for (developed, now) in [(0u8, 0u8), (1, 1), (2, 2), (3, 3), (0, 1), (1, 3)] {
        let mut st = state.borrow_mut();
        st.sidecars[0].turn = now;
        st.develop.shown_turn = Some((0, developed));
        st.develop.drawn_camera = camera_of_frame(&frame);
        let viewport = camera_crop(&st);
        let export = greycard_engine::stages::Facts::raw(&frame, now).camera();
        assert_eq!(viewport, export, "developed at {developed}, now at {now}");
        assert!(viewport.is_some());
    }
    let mut whole = frame.clone();
    whole.shot.camera_frame = None;
    assert_eq!(camera_of_frame(&whole), None);
    let mut merged = frame.clone();
    merged.dng.merged = Some(Default::default());
    assert_eq!(camera_of_frame(&merged), None, "a merge opens whole");
}

/// The zoom keys' 1:1 and 3:1 from fit go to where the camera
/// focused, turned with the frame, and back to fit; from any other
/// zoom, with no focus point, one the crop leaves out, or another
/// frame's develop drawn, they zoom about the view's center as ever.
#[test]
fn the_zoom_key_goes_to_the_cameras_focus_point() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let (app, state, _worker) = hover_frame(1.0);
    let zoom = || state.borrow().develop.zoom;
    let center = || state.borrow().develop.center;
    let near = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5;
    // Right of and above the middle of the unturned 6000 by 4000.
    let point = FocusPoint {
        area: [0.7, 0.2, 0.1, 0.1],
        aspect: 1.5,
    };
    let fit = || {
        let mut st = state.borrow_mut();
        st.develop.zoom = 0.0;
        st.develop.center = (3000.0, 2000.0);
    };
    state.borrow_mut().develop.drawn_focus = Some((point, Orientation::Normal));
    app.invoke_zoom_key(1.0);
    assert_eq!(zoom(), 1.0);
    assert!(near(center(), (4500.0, 1000.0)), "{:?}", center());
    app.invoke_zoom_key(1.0);
    assert_eq!(zoom(), 0.0, "and back to fit");
    assert!(near(center(), (3000.0, 2000.0)));

    // 3:1 goes there from fit too, and back to fit.
    app.invoke_zoom_key(3.0);
    assert_eq!(zoom(), 3.0);
    assert!(near(center(), (4500.0, 1000.0)), "{:?}", center());
    app.invoke_zoom_key(3.0);
    assert_eq!(zoom(), 0.0);
    fit();

    // From any zoom but fit, either key zooms about the view's
    // center as it always did, keeping the place: from 3:1 panned
    // onto a detail, 1:1 there; from a scroll's 150%, 3:1 there.
    for (from, key) in [(3.0, 1.0), (1.5, 3.0), (1.5, 1.0)] {
        {
            let mut st = state.borrow_mut();
            st.develop.zoom = from;
            st.develop.center = (1200.0, 3100.0);
        }
        app.invoke_zoom_key(key);
        assert_eq!(zoom(), key, "from {from}");
        assert!(
            near(center(), (1200.0, 3100.0)),
            "from {from}: {:?}",
            center()
        );
        fit();
    }

    // A raw turned a quarter by its tag develops 4000 by 6000: the
    // point at the top right of the sensor is at the bottom right.
    {
        let mut st = state.borrow_mut();
        st.develop.drawn_focus = Some((point, Orientation::Rotate90));
        st.develop.source_size = (4000, 6000);
        st.develop.image_size = (4000, 6000);
        st.develop.center = (2000.0, 3000.0);
    }
    app.invoke_zoom_key(1.0);
    assert!(near(center(), (3000.0, 4500.0)), "{:?}", center());
    {
        let mut st = state.borrow_mut();
        st.develop.zoom = 0.0;
        st.develop.center = (2000.0, 3000.0);
    }

    // A crop of the bottom right quarter keeps it, nearer the
    // crop's corner.
    app.set_geo_has_crop(true);
    app.set_geo_crop_x(0.5);
    app.set_geo_crop_y(0.5);
    app.set_geo_crop_w(0.5);
    app.set_geo_crop_h(0.5);
    app.invoke_zoom_key(1.0);
    assert!(near(center(), (1000.0, 1500.0)), "{:?}", center());
    {
        let mut st = state.borrow_mut();
        st.develop.zoom = 0.0;
        st.develop.center = (1000.0, 1500.0);
    }

    // One of the top left leaves it out: the view's center.
    app.set_geo_crop_x(0.0);
    app.set_geo_crop_y(0.0);
    app.invoke_zoom_key(1.0);
    assert_eq!(zoom(), 1.0);
    assert!(near(center(), (1000.0, 1500.0)), "{:?}", center());
}

/// With no focus point, or another frame's develop on screen, the
/// zoom key's 1:1 is about the view's center.
#[test]
fn the_zoom_key_without_a_focus_point_zooms_about_the_middle() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let (app, state, _worker) = hover_frame(1.0);
    let center = || state.borrow().develop.center;
    app.invoke_zoom_key(1.0);
    assert_eq!(state.borrow().develop.zoom, 1.0);
    assert_eq!(center(), (3000.0, 2000.0));
    {
        let mut st = state.borrow_mut();
        st.develop.zoom = 0.0;
        st.develop.drawn_focus = Some((
            FocusPoint {
                area: [0.0, 0.0, 0.1, 0.1],
                aspect: 1.5,
            },
            Orientation::Normal,
        ));
        // The picture drawn is another frame's.
        st.develop.shown_turn = Some((5, 0));
    }
    app.invoke_zoom_key(1.0);
    assert_eq!(center(), (3000.0, 2000.0));
}

/// A key pressed before the frame's develop has landed zooms about
/// the middle; its develop landing with a point then takes the view
/// there, at 1:1 and at 3:1, but not once the view was panned or
/// zoomed, nor on another frame.
#[test]
fn a_focus_point_that_comes_late_takes_an_untouched_key_zoom_there() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let (app, state, _worker) = hover_frame(1.0);
    let center = || state.borrow().develop.center;
    let near = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5;
    let point = Some((
        FocusPoint {
            area: [0.7, 0.2, 0.1, 0.1],
            aspect: 1.5,
        },
        Orientation::Normal,
    ));
    // The frame's own develop lands, with its point.
    let arrive = || {
        let mut st = state.borrow_mut();
        st.develop.shown_turn = Some((0, current_turn(&st)));
        st.develop.drawn_focus = point;
        focus_arrived(&mut st, &app);
    };
    // At fit, another frame's develop still drawn.
    let reset = || {
        let mut st = state.borrow_mut();
        st.develop.zoom = 0.0;
        st.develop.center = (3000.0, 2000.0);
        st.develop.drawn_focus = None;
        st.develop.shown_turn = Some((5, 0));
    };
    for level in [1.0, 3.0] {
        reset();
        app.invoke_zoom_key(level);
        assert!(near(center(), (3000.0, 2000.0)));
        assert!(state.borrow().develop.focus_pending.is_some(), "waits");
        arrive();
        assert_eq!(state.borrow().develop.zoom, level);
        assert!(near(center(), (4500.0, 1000.0)), "{level}: {:?}", center());

        // Panned since: left where it is.
        reset();
        app.invoke_zoom_key(level);
        state.borrow_mut().develop.center = (2900.0, 2000.0);
        arrive();
        assert!(near(center(), (2900.0, 2000.0)), "{:?}", center());

        // Zoomed since: left.
        reset();
        app.invoke_zoom_key(level);
        state.borrow_mut().develop.zoom = level * 1.25;
        arrive();
        assert!(near(center(), (3000.0, 2000.0)));

        // Another frame since: left.
        reset();
        app.invoke_zoom_key(level);
        if let Some(pending) = state.borrow_mut().develop.focus_pending.as_mut() {
            pending.frame.path = "elsewhere.cr3".into();
        }
        arrive();
        assert!(near(center(), (3000.0, 2000.0)));
        assert!(state.borrow().develop.focus_pending.is_none());
    }
}

/// A develop that lands with no point the view can take (one the
/// edit's crop leaves out) is the answer to the press: the wait is
/// dropped, and a later re-center by a change of the crop or a turn
/// does not take the view anywhere. A develop already landed with
/// no point, and a compare view, never wait at all.
#[test]
fn a_develop_that_answers_with_no_point_drops_the_wait() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let (app, state, _worker) = hover_frame(1.0);
    let pending = || state.borrow().develop.focus_pending.clone();
    let point = Some((
        FocusPoint {
            area: [0.7, 0.2, 0.1, 0.1],
            aspect: 1.5,
        },
        Orientation::Normal,
    ));
    {
        let mut st = state.borrow_mut();
        st.develop.shown_turn = Some((5, 0));
    }
    // A crop of the bottom left quarter leaves the point out.
    app.set_geo_has_crop(true);
    app.set_geo_crop_x(0.0);
    app.set_geo_crop_y(0.5);
    app.set_geo_crop_w(0.5);
    app.set_geo_crop_h(0.5);
    app.invoke_zoom_key(1.0);
    assert!(pending().is_some());
    {
        let mut st = state.borrow_mut();
        st.develop.shown_turn = Some((0, current_turn(&st)));
        st.develop.drawn_focus = point;
        focus_arrived(&mut st, &app);
    }
    assert!(pending().is_none(), "answered: none it can take");
    // The crop taken off, the view re-centered as a draw does: no
    // jump minutes after the press.
    app.set_geo_has_crop(false);
    {
        let mut st = state.borrow_mut();
        st.develop.center = (3000.0, 2000.0);
        focus_arrived(&mut st, &app);
        assert_eq!(st.develop.center, (3000.0, 2000.0));
    }

    // The frame's develop on screen with no point: no wait.
    {
        let mut st = state.borrow_mut();
        st.develop.zoom = 0.0;
        st.develop.drawn_focus = None;
    }
    app.invoke_zoom_key(1.0);
    assert!(pending().is_none());
}

/// A raw of a camera that is Rec.2020 itself, calibrated under
/// D65, its as-shot gains a daylight's: enough of a frame for the
/// white balance to resolve against.
pub(crate) fn daylight_frame() -> RawFrame {
    use greycard_core::raw::{Calibration, CfaPattern, LevelPattern, Levels, Samples};
    let m = greycard_core::color::WORKING_SPACE
        .from_xyz_matrix()
        .expect("Rec.2020 inverts");
    RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        other_names: Vec::new(),
        width: 2,
        height: 2,
        channels: 1,
        layout: greycard_core::raw::SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(vec![500; 4]),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(1000.0, 1),
            black_deltas: None,
            saturation: None,
        },
        as_shot_coefficients: Some([1.0, 1.0, 1.0]),
        calibrations: vec![Calibration {
            illuminant: 21,
            color_matrix: m.rows.iter().flatten().map(|&v| v as f32).collect(),
            forward_matrix: None,
        }],
        crop: None,
        orientation: Orientation::Normal,
        shot: Default::default(),
        dng: Default::default(),
    }
}

/// The Temperature and Tint sliders are live under As shot, showing
/// the camera's white; a drag of Temperature turns As shot off by
/// itself, leaves the slider where the pointer let go, and is one
/// step of the history; As shot switched back on puts the sliders
/// and the picture back. Judged on the picture developed: a white
/// patch, neutral as shot, warms with the move and is neutral again
/// after.
#[test]
fn moving_temperature_turns_as_shot_off_and_as_shot_puts_it_back() {
    use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.set_panel_tab("Develop".into());
    // A white patch, 32 by 32, under daylight.
    let frame = RawFrame {
        width: 32,
        height: 32,
        samples: greycard_core::raw::Samples::U16(vec![500; 32 * 32]),
        ..daylight_frame()
    };
    let profile = profile_from_frame(&frame).unwrap();
    let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
    let as_shot = (shot.temp_tint.cct as f32, shot.temp_tint.duv as f32);
    {
        let mut st = state.borrow_mut();
        st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
        st.develop.frame = Some((Arc::new(frame.clone()), Box::new(profile)));
        st.develop.shown_turn = Some((0, current_turn(&st)));
        // As the decode's landing leaves the panel.
        st.develop.as_shot_white = Some(as_shot);
    }
    app.set_raw_input(true);
    app.set_temperature(as_shot.0);
    app.set_tint(as_shot.1);
    let wait = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ))
    };
    wait();
    assert!(app.get_as_shot());
    let steps = state.borrow().sidecars[0].history.len();
    // The white patch's middle as the edit on the panel develops it.
    let patch = |app: &App| {
        let edit = read_edit(app, &state.borrow().develop.edit, None);
        let developed = greycard_core::develop::develop(
            &frame,
            &edit.settings(&greycard_edit_io::Stores::user()),
        )
        .expect("the patch develops");
        developed.image.pixel(16, 16)
    };
    let neutral = patch(&app);
    assert!(
        (neutral[0] / neutral[2] - 1.0).abs() < 0.02,
        "as shot, the white is white: {neutral:?}"
    );

    // A drag of the Temperature handle, a third of the track to
    // the right: live under As shot, and the move turns it off.
    let (at, size) = crate::testing::by_id(&app, "WhiteBalanceSection::temperature-slider");
    let (left, right) = (at.x + 68.0, at.x + size.width - 60.0);
    let fraction = (as_shot.0 - 2000.0) / 10000.0;
    let x = left + fraction * (right - left);
    let y = at.y + size.height / 2.0;
    crate::testing::drag(&app, &[(x, y), (x + 10.0, y), (x + 60.0, y)]);
    wait();
    assert!(!app.get_as_shot(), "the move turned As shot off");
    let moved = app.get_temperature();
    assert!(moved > as_shot.0 + 2000.0, "{moved} from {}", as_shot.0);
    assert_eq!(app.get_tint(), as_shot.1, "the tint stays the camera's");
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::Custom {
            temperature: moved as f64,
            tint: as_shot.1 as f64,
        }
    );
    assert_eq!(
        state.borrow().sidecars[0].history.len(),
        steps + 1,
        "the move is one step"
    );
    let warmed = patch(&app);
    assert!(
        warmed[0] / warmed[2] > neutral[0] / neutral[2] * 1.1,
        "a higher temperature renders warmer: {warmed:?} against {neutral:?}"
    );

    // As shot back on: the camera's white on the sliders and on
    // the picture.
    crate::testing::click_switch_by_id(&app, "WhiteBalanceSection::as-shot-toggle");
    wait();
    assert!(app.get_as_shot());
    assert_eq!((app.get_temperature(), app.get_tint()), as_shot);
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::AsShot
    );
    assert_eq!(patch(&app), neutral);
    assert_eq!(state.borrow().sidecars[0].history.len(), steps + 2);

    // A custom pair, from a drag of each slider.
    let drag_slider = |id: &str, value: f32, low: f32, high: f32, by: f32| {
        let (at, size) = crate::testing::by_id(&app, id);
        let (left, right) = (at.x + 68.0, at.x + size.width - 60.0);
        let x = left + (value - low) / (high - low) * (right - left);
        let y = at.y + size.height / 2.0;
        crate::testing::drag(&app, &[(x, y), (x + by / 6.0, y), (x + by, y)]);
        wait();
    };
    drag_slider(
        "WhiteBalanceSection::temperature-slider",
        as_shot.0,
        2000.0,
        12000.0,
        60.0,
    );
    drag_slider(
        "WhiteBalanceSection::tint-slider",
        as_shot.1,
        -0.05,
        0.05,
        -40.0,
    );
    let custom = (app.get_temperature(), app.get_tint());
    assert!(!app.get_as_shot());
    assert!(custom.0 > as_shot.0 && custom.1 < as_shot.1, "{custom:?}");
    let steps = state.borrow().sidecars[0].history.len();
    // A double-click on a slider puts it alone back to the camera's
    // value, and nothing else: As shot stays off. One step each.
    let double_click = |id: &str| {
        // Clear of the last click, so the two here are a pair.
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(600));
        slint::platform::update_timers_and_animations();
        let (at, size) = crate::testing::by_id(&app, id);
        let (x, y) = (at.x + 30.0, at.y + size.height / 2.0);
        crate::testing::click(&app, x, y);
        crate::testing::click(&app, x, y);
        wait();
    };
    double_click("WhiteBalanceSection::temperature-slider");
    assert_eq!(
        (app.get_temperature(), app.get_tint()),
        (as_shot.0, custom.1)
    );
    assert!(!app.get_as_shot());
    assert_eq!(state.borrow().sidecars[0].history.len(), steps + 1);
    double_click("WhiteBalanceSection::tint-slider");
    assert_eq!((app.get_temperature(), app.get_tint()), as_shot);
    assert!(!app.get_as_shot(), "both at the camera's, and still custom");
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::Custom {
            temperature: as_shot.0 as f64,
            tint: as_shot.1 as f64,
        }
    );
    assert_eq!(state.borrow().sidecars[0].history.len(), steps + 2);

    // As shot switched on from a custom pair: both sliders, and the
    // picture, at the camera's.
    drag_slider(
        "WhiteBalanceSection::temperature-slider",
        as_shot.0,
        2000.0,
        12000.0,
        -60.0,
    );
    drag_slider(
        "WhiteBalanceSection::tint-slider",
        as_shot.1,
        -0.05,
        0.05,
        40.0,
    );
    assert!(!app.get_as_shot());
    assert_ne!(patch(&app), neutral);
    crate::testing::click_switch_by_id(&app, "WhiteBalanceSection::as-shot-toggle");
    wait();
    assert!(app.get_as_shot());
    assert_eq!((app.get_temperature(), app.get_tint()), as_shot);
    assert_eq!(patch(&app), neutral);
}

/// The Auto button reads the whole picture and sets the white it
/// finds the way the Neutral dropper does: As shot off, the
/// temperature and tint moved, one step of the history. A picture
/// that is all grey under a warm light, developed at daylight,
/// wants a lower temperature than the daylight it was shot at.
#[test]
fn auto_white_balance_sets_the_white_in_one_step() {
    use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let frame = daylight_frame();
    let profile = profile_from_frame(&frame).unwrap();
    let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
    {
        let mut st = state.borrow_mut();
        st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
        st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
    }
    let wait = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ))
    };
    wait();
    app.set_as_shot(true);
    let (temperature, tint) = (app.get_temperature(), app.get_tint());
    let steps = state.borrow().sidecars[0].history.len();

    // The picture on the GPU is not this frame's develop yet (a
    // frame was chosen and its develop has not landed): neither
    // Auto nor the Neutral dropper reads it.
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((vec![[0.3; 3]; 1000], 40)));
    app.invoke_wb_auto();
    assert_eq!(app.get_status(), "auto white balance: wait for the develop");
    assert_eq!(
        white_from_neutral(state.borrow_mut(), &app, [0.3, 0.3, 0.2]),
        Err(NeutralMiss::Waiting)
    );
    assert_eq!(
        NeutralMiss::Waiting.said(false),
        "white balance: wait for the develop"
    );
    assert!(app.get_as_shot());
    AUTO_WHITE_GRID.with(|g| g.borrow_mut().take());
    {
        let mut st = state.borrow_mut();
        st.develop.shown_turn = Some((0, current_turn(&st)));
    }
    // A neutral that names no white says so, in each one's words.
    assert_eq!(
        white_from_neutral(state.borrow_mut(), &app, [0.0, 0.3, 0.3]),
        Err(NeutralMiss::NoWhite)
    );
    assert_eq!(NeutralMiss::NoWhite.said(false), "nothing to read there");
    assert_eq!(
        NeutralMiss::NoWhite.said(true),
        "auto white balance: nothing neutral to read"
    );
    assert!(app.get_as_shot());

    // Too little to read: nothing moves, and the status says why.
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((vec![[0.99; 3]; 1000], 40)));
    app.invoke_wb_auto();
    assert_eq!(app.get_status(), auto_white_too_little());
    assert!(app.get_as_shot());

    let grid: Vec<[f32; 3]> = (0..2000)
        .map(|i| {
            let level = 0.05 + 0.5 * i as f32 / 2000.0;
            [1.25 * level, level, 0.7 * level]
        })
        .collect();
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid, 40)));
    app.invoke_wb_auto();
    wait();
    assert!(!app.get_as_shot());
    assert_ne!(app.get_tint(), tint);
    assert_ne!(app.get_temperature(), temperature);
    assert!(
        (app.get_temperature() as f64) < shot.temp_tint.cct - 500.0,
        "{} against {}",
        app.get_temperature(),
        shot.temp_tint.cct
    );
    assert_eq!(state.borrow().sidecars[0].history.len(), steps + 1);
}

/// A daylight raw open at As shot, its develop landed: the camera's
/// white as the sliders show it.
fn a_raw_up(app: &App, state: &Rc<Shared>) -> (f32, f32) {
    use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
    app.invoke_select(0);
    app.set_panel_tab("Develop".into());
    let frame = daylight_frame();
    let profile = profile_from_frame(&frame).unwrap();
    let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
    let camera = (shot.temp_tint.cct as f32, shot.temp_tint.duv as f32);
    {
        let mut st = state.borrow_mut();
        st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
        st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
        st.develop.shown_turn = Some((0, current_turn(&st)));
        st.develop.as_shot_white = Some(camera);
    }
    app.set_raw_input(true);
    app.set_as_shot(true);
    app.set_temperature(camera.0);
    app.set_tint(camera.1);
    camera
}

fn settle() {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::SAVE_MS + 100,
    ));
}

/// As shot's switch flipped, as a click does it.
fn switch_as_shot(app: &App, on: bool) {
    app.set_as_shot(on);
    app.invoke_as_shot_toggled();
    settle();
}

/// A slider moved, as the section's `changed` does it.
fn move_white(app: &App, temperature: f32, tint: f32) {
    app.set_as_shot(false);
    app.set_temperature(temperature);
    app.set_tint(tint);
    app.invoke_develop_changed();
    settle();
}

fn sliders(app: &App) -> (bool, f32, f32) {
    (app.get_as_shot(), app.get_temperature(), app.get_tint())
}

/// Discussion 59: As shot switched off gives back the custom white
/// its switch replaced, one step each way; with none replaced (the
/// frame opened at As shot) the sliders stay at the camera's.
#[test]
fn as_shot_off_gives_back_the_white_it_replaced() {
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let camera = a_raw_up(&app, &state);
    settle();
    let steps = || state.borrow().sidecars[0].history.len();
    let wb = || state.borrow().develop.edit.white_balance.clone();
    let custom = |t: f32, d: f32| WhiteBalance::Custom {
        temperature: t as f64,
        tint: d as f64,
    };

    // Opened at As shot: nothing to give back.
    let at = steps();
    switch_as_shot(&app, false);
    assert_eq!(sliders(&app), (false, camera.0, camera.1));
    assert_eq!(wb(), custom(camera.0, camera.1));
    assert_eq!(steps(), at + 1);

    // A custom white, As shot on and off: the custom white again.
    move_white(&app, 4000.0, 0.01);
    let at = steps();
    switch_as_shot(&app, true);
    assert_eq!(sliders(&app), (true, camera.0, camera.1));
    assert_eq!(wb(), WhiteBalance::AsShot);
    assert_eq!(steps(), at + 1, "on is one step");
    switch_as_shot(&app, false);
    assert_eq!(sliders(&app), (false, 4000.0, 0.01));
    assert_eq!(wb(), custom(4000.0, 0.01));
    assert_eq!(steps(), at + 2, "off is one step");
    assert!(state.borrow().develop.before_as_shot.is_none());
    // Given back once: on and off again remembers it anew.
    switch_as_shot(&app, true);
    switch_as_shot(&app, false);
    assert_eq!(sliders(&app), (false, 4000.0, 0.01));
}

/// Undo and redo across As shot's switch end on states that
/// existed: the undo of "on" shows the custom white and forgets it,
/// so the redo's As shot gives back nothing; the undo of the "off"
/// that gave it back shows As shot, and an undo past that the
/// custom white.
#[test]
fn as_shot_s_memory_and_the_history_agree() {
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let camera = a_raw_up(&app, &state);
    settle();
    move_white(&app, 4000.0, 0.01);
    let custom = (false, 4000.0, 0.01);
    let shot = (true, camera.0, camera.1);

    // On, off (given back), undo, undo, redo, redo.
    switch_as_shot(&app, true);
    switch_as_shot(&app, false);
    assert_eq!(sliders(&app), custom);
    app.invoke_undo();
    assert_eq!(sliders(&app), shot);
    app.invoke_undo();
    assert_eq!(sliders(&app), custom);
    app.invoke_redo();
    assert_eq!(sliders(&app), shot);
    app.invoke_redo();
    assert_eq!(sliders(&app), custom);

    // On, undo: the custom white, nothing remembered; redo: As
    // shot, and off then leaves the camera's.
    switch_as_shot(&app, true);
    app.invoke_undo();
    assert_eq!(sliders(&app), custom);
    assert!(state.borrow().develop.before_as_shot.is_none());
    app.invoke_redo();
    assert_eq!(sliders(&app), shot);
    switch_as_shot(&app, false);
    assert_eq!(sliders(&app), (false, camera.0, camera.1));

    // Undo that off, then on again from a custom white undone to:
    // remembered once, given back once.
    app.invoke_undo();
    app.invoke_undo();
    assert_eq!(sliders(&app), custom);
    switch_as_shot(&app, true);
    switch_as_shot(&app, false);
    assert_eq!(sliders(&app), custom);
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::Custom {
            temperature: 4000.0,
            tint: 0.01_f32 as f64,
        }
    );
}

/// Issue 128: each preset sets both sliders to its light and As
/// shot off, one step; an undo goes back to the white before it.
#[test]
fn a_white_balance_preset_sets_the_sliders_in_one_step() {
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let camera = a_raw_up(&app, &state);
    settle();
    let steps = || state.borrow().sidecars[0].history.len();
    let mut before = (true, camera.0, camera.1);
    for (key, temperature, tint) in crate::panel::color::WHITE_PRESETS {
        let at = steps();
        app.invoke_wb_preset(key.into());
        settle();
        assert_eq!(sliders(&app), (false, temperature, tint), "{key}");
        assert_eq!(
            state.borrow().develop.edit.white_balance,
            WhiteBalance::Custom {
                temperature: temperature as f64,
                tint: tint as f64,
            },
            "{key}"
        );
        assert_eq!(steps(), at + 1, "{key} is one step");
        app.invoke_undo();
        assert_eq!(sliders(&app), before, "{key} undone");
        app.invoke_redo();
        assert_eq!(sliders(&app), (false, temperature, tint), "{key} redone");
        before = sliders(&app);
    }
    // The six, in the menu's order.
    let keys: Vec<_> = crate::panel::color::WHITE_PRESETS
        .iter()
        .map(|p| p.0)
        .collect();
    assert_eq!(
        keys,
        [
            "Daylight",
            "Cloudy",
            "Shade",
            "Tungsten",
            "Fluorescent",
            "Flash"
        ]
    );
    // A key that is no preset changes nothing.
    let at = steps();
    app.invoke_wb_preset("Candle".into());
    settle();
    assert_eq!(sliders(&app), before);
    assert_eq!(steps(), at);
}

/// Auto's chevron opens the presets on a raw, and a pick from the
/// keys sets the white as a call does. A picture has no menu.
#[test]
fn autos_menu_holds_the_presets_on_a_raw_alone() {
    use slint::platform::Key;
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    a_raw_up(&app, &state);
    settle();
    let names = [
        "Daylight",
        "Cloudy",
        "Shade",
        "Tungsten",
        "Fluorescent",
        "Flash",
    ];
    let shown = |app: &App| {
        names
            .iter()
            .map(|n| crate::testing::count_menu_entries(app, n))
            .collect::<Vec<_>>()
    };
    assert_eq!(shown(&app), [0; 6], "no menu yet");

    // Asked open, as the chevron opens it: the six, once each;
    // the third from the keys is Shade.
    app.set_wb_presets_shown(true);
    assert_eq!(shown(&app), [1; 6]);
    for _ in 0..3 {
        crate::testing::press(&app, Key::DownArrow);
    }
    crate::testing::press(&app, Key::Return);
    slint::platform::update_timers_and_animations();
    settle();
    assert_eq!(shown(&app), [0; 6], "the menu is away");
    assert_eq!(sliders(&app), (false, 7500.0, 0.003));

    // A click on the chevron, at the right end of the section's
    // Auto button (the Light section has an Auto too), opens it.
    let auto = crate::testing::by_id(&app, "WhiteBalanceSection::auto-button");
    crate::testing::click(
        &app,
        auto.0.x + auto.1.width - 12.0,
        auto.0.y + auto.1.height / 2.0,
    );
    assert_eq!(shown(&app), [1; 6], "the chevron opens it");
    crate::testing::press(&app, Key::Escape);
    slint::platform::update_timers_and_animations();
    assert_eq!(shown(&app), [0; 6]);

    // A picture: no chevron, no menu, and a preset asked for
    // anyway changes nothing.
    app.set_picture_open(true);
    app.set_wb_presets_shown(true);
    assert_eq!(shown(&app), [0; 6]);
    let was = sliders(&app);
    app.invoke_wb_preset("Tungsten".into());
    settle();
    assert_eq!(sliders(&app), was);
}

/// Each of the presets' six has its icon drawn beside its words, in
/// the column Slint's menu keeps for one, in either theme; Fit's
/// zooms, which have none, leave that column bare, so the
/// words of every menu stand at the same place.
#[test]
fn each_white_balance_preset_has_its_icon_beside_it() {
    use i_slint_backend_testing::ElementHandle;
    crate::testing::drawing_backend();
    let app = window(1);
    app.window().set_size(slint::PhysicalSize::new(1500, 950));
    app.window()
        .dispatch_event(slint::platform::WindowEvent::Resized {
            size: slint::LogicalSize::new(1500.0, 950.0),
        });
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    a_raw_up(&app, &state);
    settle();
    // The ink in each open menu entry's icon column, top to bottom:
    // the pixels that differ from the entry's own ground, read at its
    // left padding. The column is the Fluent style's: 11 px of
    // padding, then 12 px. An entry's place is the menu's own, so
    // the menu's place in the window is added to it.
    let inks = |app: &App| {
        use i_slint_core::window::PopupWindowLocation;
        use slint::private_unstable_api::re_exports::WindowInner;
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        let origin = WindowInner::from_pub(app.window())
            .active_popups()
            .iter()
            .map(|p| match &p.location {
                PopupWindowLocation::ChildWindow(at) => (at.x, at.y),
                PopupWindowLocation::TopLevel(_) => panic!("a menu in a window of its own"),
            })
            .next_back();
        let Some((ox, oy)) = origin else {
            return Vec::new();
        };
        let shot = app.window().take_snapshot().expect("a drawing backend");
        let px = |x: f32, y: f32| shot.as_slice()[y as usize * shot.width() as usize + x as usize];
        let mut entries: Vec<_> = ElementHandle::find_by_element_type_name(app, "MenuItemBase")
            .map(|e| (e.absolute_position(), e.size()))
            .collect();
        entries.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
        entries
            .iter()
            .map(|(at, size)| {
                let (x0, y0) = (ox + at.x, oy + at.y);
                let ground = px(x0 + 3.0, y0 + size.height / 2.0);
                let mut ink = 0;
                for y in y0 as u32 + 2..(y0 + size.height) as u32 - 2 {
                    for x in x0 as u32 + 11..x0 as u32 + 23 {
                        let p = px(x as f32, y as f32);
                        let far = |a: u8, b: u8| a.abs_diff(b) > 40;
                        if far(p.r, ground.r) || far(p.g, ground.g) || far(p.b, ground.b) {
                            ink += 1;
                        }
                    }
                }
                ink
            })
            .collect::<Vec<usize>>()
    };
    for theme in ["Charcoal", "Light"] {
        app.global::<crate::Theme>().set_name(theme.into());
        app.set_wb_presets_shown(true);
        let ink = inks(&app);
        assert_eq!(ink.len(), 6, "{theme}: the presets' six");
        assert!(
            ink.iter().all(|&n| n >= 12),
            "{theme}: an icon each, {ink:?}"
        );
        crate::testing::press(&app, slint::platform::Key::Escape);
        slint::platform::update_timers_and_animations();
        assert!(inks(&app).is_empty(), "{theme}: the menu is away");

        app.set_zoom_menu_shown(true);
        let ink = inks(&app);
        assert!(!ink.is_empty(), "{theme}: the zooms are open");
        assert!(ink.iter().all(|&n| n == 0), "{theme}: no icons, {ink:?}");
        crate::testing::press(&app, slint::platform::Key::Escape);
        slint::platform::update_timers_and_animations();
    }
}

/// The presets' menu keeps the keys as Fit's does: the click that
/// closes it, on the strip or on the picture with the dropper in
/// hand, is no click; and after a pick Return and Space are the
/// window's, not Auto's.
#[test]
fn the_presets_menu_hands_the_keys_back() {
    use slint::platform::Key;
    use std::cell::Cell;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    a_raw_up(&app, &state);
    settle();
    let autos = Rc::new(Cell::new(0));
    let counted = autos.clone();
    app.on_wb_auto(move || counted.set(counted.get() + 1));
    let picks = Rc::new(Cell::new(0));
    let counted = picks.clone();
    app.on_pick_pressed(move |_, _| counted.set(counted.get() + 1));
    let chevron = || {
        let auto = crate::testing::by_id(&app, "WhiteBalanceSection::auto-button");
        crate::testing::click(
            &app,
            auto.0.x + auto.1.width - 12.0,
            auto.0.y + auto.1.height / 2.0,
        );
        // What every surface asks before taking a press: without
        // it the closing click below would be a click.
        assert!(app.get_menu_up(), "the menu took the keys' focus");
    };

    // Closed by a click on another frame: it stays on this one.
    chevron();
    let (x, y) = crate::testing::strip_cell(&app, 2);
    crate::testing::click(&app, x, y);
    assert!(!app.get_menu_up());
    assert_eq!(state.borrow().current, Some(0));

    // Closed by a click on the picture with the dropper in hand:
    // no neutral read.
    app.set_picking("White".into());
    chevron();
    crate::testing::click(&app, 600.0, 300.0);
    assert!(!app.get_menu_up());
    assert_eq!(picks.get(), 0, "the closing click picked nothing");
    app.set_picking("".into());

    // Daylight picked from the keys; then Return and Space press
    // no Auto.
    chevron();
    crate::testing::press(&app, Key::DownArrow);
    crate::testing::press(&app, Key::Return);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
    settle();
    assert_eq!(sliders(&app), (false, 5500.0, 0.003));
    crate::testing::press(&app, Key::Return);
    crate::testing::press(&app, " ");
    assert_eq!(autos.get(), 0, "the keys are not Auto's");
    assert_eq!(sliders(&app), (false, 5500.0, 0.003));
}

/// The masks panel's People and Landscape menus keep the keys as
/// Fit's does: the keys take the focus before either opens, and
/// after a pick, or Escape, Space is not the button's again and an
/// arrow steps the frame.
#[test]
fn the_masks_menus_hand_the_keys_back() {
    use slint::platform::Key;
    use std::cell::RefCell;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 1200.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    {
        let mut st = state.borrow_mut();
        st.develop.edit.adjustments.push(greycard_edit::Adjustment {
            id: 1,
            name: "Mask 1".into(),
            ..Default::default()
        });
        st.develop.target = Some(0);
    }
    app.set_panel_tab("Masks".into());
    app.set_target(1);
    let steps = Rc::new(Cell::new(0));
    let counted = steps.clone();
    app.on_step(move |by, _| counted.set(counted.get() + by));
    let parts = Rc::new(RefCell::new(Vec::<String>::new()));
    let picked = parts.clone();
    app.on_add_part(move |label, _| picked.borrow_mut().push(label.into()));
    let finds = Rc::new(Cell::new(0));
    let counted = finds.clone();
    app.on_find_landscape(move |_| counted.set(counted.get() + 1));
    let kinds = Rc::new(RefCell::new(Vec::<String>::new()));
    let picked = kinds.clone();
    app.on_add_landscape(move |kind| picked.borrow_mut().push(kind.into()));
    let after_the_menu = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        assert!(!app.get_menu_up(), "the menu is away");
    };
    let at = |id: &str| {
        let (p, size) = crate::testing::by_id(&app, id);
        (p.x + size.width / 2.0, p.y + size.height / 2.0)
    };
    let people = at("AdjustmentsSection::parts-button");
    let landscape = at("AdjustmentsSection::landscape-button");
    let people_up = || crate::testing::count_menu_entries(&app, "Whole person");

    // People, from a click on its button: Whole person picked from
    // the keys; then Space presses no People and Right steps.
    click(&app, people.0, people.1);
    assert_eq!(people_up(), 1, "the menu is up");
    assert!(app.get_menu_up(), "the menu took the keys' focus");
    crate::testing::press(&app, Key::DownArrow);
    crate::testing::press(&app, Key::Return);
    after_the_menu();
    assert_eq!(*parts.borrow(), ["Whole person"]);
    crate::testing::press(&app, " ");
    assert_eq!(people_up(), 0, "Space is not People's");
    crate::testing::press(&app, Key::RightArrow);
    assert_eq!(steps.get(), 1, "Right is the window's");

    // People put away with Escape: the same.
    click(&app, people.0, people.1);
    assert!(app.get_menu_up(), "the menu took the keys' focus");
    crate::testing::press(&app, Key::Escape);
    after_the_menu();
    crate::testing::press(&app, " ");
    assert_eq!(people_up(), 0, "Space is not People's");
    crate::testing::press(&app, Key::RightArrow);
    assert_eq!(steps.get(), 2, "Right is the window's");
    assert_eq!(parts.borrow().len(), 1);

    // Landscape: its button pressed, then the menu raised from
    // Rust once the answer is in, as `landscape::found` does.
    click(&app, landscape.0, landscape.1);
    assert_eq!(finds.get(), 1);
    let offers = |words: &[&str]| {
        let words: Vec<slint::SharedString> = words.iter().map(|&w| w.into()).collect();
        slint::ModelRc::new(slint::VecModel::from(words))
    };
    app.set_landscape_offers(offers(&["Foliage 33%", "Water 17%"]));
    // The menu's offer of water, as Rust words it: data here.
    let water_offered = || {
        crate::testing::menu_entries(&app)
            .iter()
            .filter(|e| e.accessible_label().as_deref() == Some("Water 17%"))
            .count()
    };
    app.set_landscape_kinds(offers(&["Foliage", "Water"]));
    // The offer `found` leaves, which the menu's timer asks after
    // as it opens, a turn later.
    state.borrow_mut().develop.landscape_offer = Some(crate::panel::landscape::Pending {
        id: 1,
        mode: greycard_edit::mask::Mode::Add,
    });
    let raise = || {
        app.set_landscape_menu_shown(true);
        for _ in 0..2 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
        }
    };
    raise();
    assert_eq!(water_offered(), 1);
    assert!(app.get_menu_up(), "the menu took the keys' focus");
    crate::testing::press(&app, Key::DownArrow);
    crate::testing::press(&app, Key::DownArrow);
    crate::testing::press(&app, Key::Return);
    after_the_menu();
    assert_eq!(*kinds.borrow(), ["Water"]);
    crate::testing::press(&app, " ");
    crate::testing::press(&app, Key::Return);
    assert_eq!(finds.get(), 1, "Space and Return are not Landscape's");
    crate::testing::press(&app, Key::RightArrow);
    assert_eq!(steps.get(), 3, "Right is the window's");

    // Landscape put away with Escape: the same.
    raise();
    assert_eq!(water_offered(), 1);
    assert!(app.get_menu_up(), "the menu took the keys' focus");
    crate::testing::press(&app, Key::Escape);
    after_the_menu();
    assert_eq!(water_offered(), 0);
    crate::testing::press(&app, Key::RightArrow);
    assert_eq!(steps.get(), 4, "Right is the window's");
    assert_eq!(kinds.borrow().len(), 1);
}

/// A new white chosen under As shot (a slider, Auto, the Neutral
/// dropper) turns it off and forgets the white it replaced; so
/// does another frame opened.
#[test]
fn a_new_white_or_another_frame_forgets_what_as_shot_replaced() {
    let app = window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let camera = a_raw_up(&app, &state);
    settle();
    let remembered = || state.borrow().develop.before_as_shot.is_some();

    // A slider.
    move_white(&app, 4000.0, 0.01);
    switch_as_shot(&app, true);
    assert!(remembered());
    move_white(&app, 7000.0, 0.01);
    assert!(!remembered());
    assert_eq!(sliders(&app), (false, 7000.0, 0.01));

    // Auto.
    switch_as_shot(&app, true);
    assert!(remembered());
    let grid: Vec<[f32; 3]> = (0..2000)
        .map(|i| {
            let level = 0.05 + 0.5 * i as f32 / 2000.0;
            [1.25 * level, level, 0.7 * level]
        })
        .collect();
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid, 40)));
    app.invoke_wb_auto();
    settle();
    assert!(!app.get_as_shot());
    assert!(!remembered());
    assert_ne!(app.get_temperature(), 7000.0);

    // The Neutral dropper.
    move_white(&app, 4000.0, 0.01);
    switch_as_shot(&app, true);
    assert!(remembered());
    assert_eq!(
        white_from_neutral(state.borrow_mut(), &app, [0.3, 0.3, 0.2]),
        Ok(())
    );
    settle();
    assert!(!app.get_as_shot());
    assert!(!remembered());
    assert_ne!(sliders(&app), (false, 4000.0, 0.01));

    // Another frame.
    move_white(&app, 4000.0, 0.01);
    switch_as_shot(&app, true);
    assert!(remembered());
    app.invoke_select(1);
    assert!(!remembered());
    // Back on the first, nothing to give back.
    app.invoke_select(0);
    state.borrow_mut().develop.as_shot_white = Some(camera);
    assert!(app.get_as_shot());
    switch_as_shot(&app, false);
    assert_ne!(sliders(&app), (false, 4000.0, 0.01));
}

/// A picture's As shot is the offset of none, and its switch gives
/// back the offset it replaced as a raw's gives back a light.
#[test]
fn a_pictures_as_shot_off_gives_back_its_offset() {
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, picture_and_raw());
    a_picture_up(&app, &state);
    settle();
    move_white(&app, 20.0, 0.005);
    switch_as_shot(&app, true);
    assert_eq!(sliders(&app), (true, 0.0, 0.0));
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::AsShot
    );
    switch_as_shot(&app, false);
    assert_eq!(sliders(&app), (false, 20.0, 0.005));
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::offset_of(20.0, 0.005_f32 as f64)
    );
}

/// The Light section's Auto sets the light sliders from the picture
/// through the history, as one step under its name: an undo takes
/// them back, and a second press changes nothing. Before the
/// frame's develop is up it does nothing.
#[test]
fn auto_light_sets_the_sliders_in_one_step() {
    use greycard_edit::auto::MIDDLE;
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let wait = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ))
    };
    wait();
    // A frame a stop darker than Auto's middle, grey.
    let grid = || -> Vec<[f32; 3]> {
        (0..4000)
            .map(|i| {
                let stops = MIDDLE - 3.0 + 4.0 * (i as f32 + 0.5) / 4000.0;
                [0.18 * 2f32.powf(stops); 3]
            })
            .collect()
    };
    let line = |state: &Rc<Shared>| {
        let st = state.borrow();
        let l = &st.sidecars[0];
        (
            l.history.len(),
            l.current().light,
            l.current_label().map(String::from),
        )
    };
    let (steps, light, _) = line(&state);

    // Not this frame's develop yet: nothing moves.
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid(), 40)));
    app.invoke_light_auto();
    wait();
    assert_eq!(line(&state).0, steps);
    assert_eq!(app.get_exposure(), 0.0);
    AUTO_WHITE_GRID.with(|g| g.borrow_mut().take());
    {
        let mut st = state.borrow_mut();
        st.develop.shown_turn = Some((0, current_turn(&st)));
    }

    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid(), 40)));
    app.invoke_light_auto();
    wait();
    let (after, auto, label) = line(&state);
    assert_eq!(after, steps + 1);
    assert_eq!(label.as_deref(), Some(greycard_edit::history::AUTO_LIGHT));
    assert_ne!(auto, light, "the first press moved the sliders");
    assert!((auto.exposure - 1.0).abs() < 0.05, "{auto:?}");
    assert_eq!(app.get_exposure(), auto.exposure);
    assert_eq!(app.get_whites(), auto.tone.whites);
    assert_eq!(auto.tone.contrast, light.tone.contrast);

    // Again: the same picture gives the same sliders, no new step.
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid(), 40)));
    app.invoke_light_auto();
    wait();
    assert_eq!(line(&state).0, steps + 1);
    assert_eq!(line(&state).1, auto);

    app.invoke_undo();
    wait();
    assert_eq!(line(&state).1, light);
    assert_eq!(app.get_exposure(), light.exposure);
}

/// Auto is a button under the Light section's sliders, there for
/// the picture and not for a mask, whose light it does not set.
#[test]
fn auto_light_is_a_button_under_the_light_sliders() {
    use greycard_edit::auto::MIDDLE;
    use i_slint_backend_testing::ElementHandle;
    let app = window(1);
    // Tall enough that Auto, under the Light section's last slider
    // and its highlight fill's switch, is in the panel's view.
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let wait = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ))
    };
    wait();
    {
        let mut st = state.borrow_mut();
        st.develop.shown_turn = Some((0, current_turn(&st)));
    }
    app.set_raw_input(true);
    let light = state.borrow().sidecars[0].current().light;

    // The white balance has an Auto of its own above: the Light
    // section's is under its last slider.
    let (blacks, _) = crate::testing::by_id(&app, "LightSection::blacks-slider");
    let (at, size) = crate::testing::by_id(&app, "LightSection::auto-button");
    assert!(at.y > blacks.y, "the Auto under the Blacks slider");
    let grid: Vec<[f32; 3]> = (0..4000)
        .map(|i| {
            let stops = MIDDLE - 3.0 + 4.0 * (i as f32 + 0.5) / 4000.0;
            [0.18 * 2f32.powf(stops); 3]
        })
        .collect();
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid, 40)));
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    wait();
    let auto = state.borrow().sidecars[0].current().light;
    assert_ne!(auto, light, "the click moved the sliders");
    assert!((auto.exposure - 1.0).abs() < 0.05, "{auto:?}");
    assert_eq!(app.get_exposure(), auto.exposure);

    // A mask's Light section has no Auto, and its white balance
    // none either.
    app.set_panel_tab("Masks".into());
    app.invoke_tab_changed("Masks".into());
    app.invoke_new_mask();
    app.invoke_target_changed(1);
    assert_eq!(app.get_target(), 1);
    let (blacks, _) = crate::testing::by_id(&app, "LightSection::blacks-slider");
    let panel = ElementHandle::find_by_element_id(&app, "App::panel-view")
        .next()
        .expect("the panel's view")
        .absolute_position();
    assert!(blacks.x >= panel.x);
    let autos = crate::testing::count_id(&app, "LightSection::auto-button")
        + crate::testing::count_id(&app, "WhiteBalanceSection::auto-button");
    assert_eq!(autos, 0);
}

/// A monochrome raw's masks' whites and the global slider's preview
/// move nothing: every white is the identity on it.
#[test]
fn no_white_moves_a_monochrome_raw_on_the_panel() {
    use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let frame = greycard_core::RawFrame {
        layout: greycard_core::raw::SensorLayout::Monochrome,
        as_shot_coefficients: Some([1.0; 3]),
        calibrations: Vec::new(),
        ..daylight_frame()
    };
    let profile = profile_from_frame(&frame).unwrap();
    let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
    let mut st = state.borrow_mut();
    st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
    st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let shift = crate::panel::color::local_white_shift(&mut st, (false, 3000.0, 0.01)).unwrap();
    let lamp = greycard_edit::white::LocalWhite::Absolute {
        temperature: 2500.0,
        tint: -0.01,
    };
    assert_eq!(shift.local(&lamp), Some(identity));
    let preview = crate::panel::color::preview_white(&mut st, (false, 3000.0, 0.01));
    for (r, row) in preview.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            assert!(
                (v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-6,
                "{preview:?}"
            );
        }
    }
}

/// A picture held up while the next frame develops keeps its own
/// masks' white balances: the frame, its profile and the develop's
/// white move on to the next file before its develop lands, and the
/// held picture's masks must not flash back to the global's white.
#[test]
fn a_held_picture_keeps_its_masks_whites() {
    use greycard_core::color::{WhitePoint, profile_from_frame, resolve_white_balance};
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let frame = daylight_frame();
    let profile = profile_from_frame(&frame).unwrap();
    let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
    let mut st = state.borrow_mut();
    st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
    st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
    let own = crate::panel::color::local_white_shift(&mut st, (true, 0.0, 0.0)).expect("a raw's");
    // An offset is from the panel's white: As shot, the develop's
    // own; a global slider ahead of the develop, that slider's,
    // with the develop's gains and matrix kept.
    assert_eq!(own.global, shot.temp_tint);
    let ahead = crate::panel::color::local_white_shift(&mut st, (false, 6100.0, 0.001)).unwrap();
    assert_eq!(ahead.global.cct, 6100.0);
    assert_eq!(ahead.global.duv, 0.001f32 as f64);
    assert_eq!(ahead.camera(), own.camera());
    let own = crate::panel::color::local_white_shift(&mut st, (true, 0.0, 0.0)).unwrap();
    // The next file chosen: the picture held, the frame gone and
    // the develop's white not yet the next one's.
    st.develop.held = Some(st.develop.edit.clone());
    st.develop.frame = None;
    st.develop.base_white = None;
    let held =
        crate::panel::color::local_white_shift(&mut st, (true, 0.0, 0.0)).expect("the held one's");
    assert_eq!(held.camera(), own.camera());
    // Its develop landed: the next frame's own, here none.
    st.develop.held = None;
    assert!(crate::panel::color::local_white_shift(&mut st, (true, 0.0, 0.0)).is_none());
    st.develop.held = Some(st.develop.edit.clone());
    assert!(crate::panel::color::local_white_shift(&mut st, (true, 0.0, 0.0)).is_none());
}

/// A mask's own white balance on the panel: switched on it starts
/// at the global's white, As shot read as its kelvin and Duv, and
/// the Neutral dropper on the Masks tab sets the mask's and leaves
/// the global's alone.
/// A daylight frame open with one mask, "Lamp", chosen on the Masks
/// tab: the frame's As shot white as a temperature and tint.
/// The open frame as a picture whose develop has landed at As
/// shot: its own white D65, the sliders at the offset of zero.
fn a_picture_up(app: &App, state: &Rc<Shared>) -> greycard_core::TempTint {
    app.invoke_select(0);
    let own = greycard_core::picture::white_temp_tint(greycard_core::illuminant::D65);
    {
        let mut st = state.borrow_mut();
        st.develop.base_white = Some(WhiteBase {
            clip: 1.0,
            picture: Some((own, own)),
            ..WhiteBase::IDENTITY
        });
        st.develop.frame = None;
        st.develop.shown_turn = Some((0, current_turn(&st)));
        st.develop.as_shot_white = Some((0.0, 0.0));
    }
    app.set_raw_input(false);
    // As the decode's landing sets them, from the picture's white.
    let ((m0, m1), (d0, d1)) = greycard_edit::white::picture_offset_range(own);
    app.set_picture_mired_min(m0 as f32);
    app.set_picture_mired_max(m1 as f32);
    app.set_picture_tint_min(d0 as f32);
    app.set_picture_tint_max(d1 as f32);
    app.set_panel_tab("Develop".into());
    app.set_as_shot(true);
    app.set_temperature(0.0);
    app.set_tint(0.0);
    own
}

/// A JPEG and a raw beside it, in a directory that does not exist.
fn picture_and_raw() -> Vec<std::path::PathBuf> {
    ["IMG_0000.JPG", "IMG_0001.CR3"]
        .iter()
        .map(|f| std::path::PathBuf::from("/nowhere").join(f))
        .collect()
}

/// A picture's WHITE BALANCE is live, its sliders an offset from
/// the picture's own white read in mired and Duv: a drag is an
/// offset in the edit and one step; the double-click puts a slider
/// back to none; As shot is the offset of zero; the dropper and
/// Auto store the offset that makes the pick grey, the preview
/// moves the picture on the GPU from the light it was developed at,
/// and a raw's section reads as it did.
#[test]
fn a_pictures_white_balance_is_an_offset_from_its_own_white() {
    use greycard_edit::white::{adaptation, picture_light};
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, picture_and_raw());
    let own = a_picture_up(&app, &state);
    let wait = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ))
    };
    wait();
    assert!(app.get_picture_open());
    let temperature_says = || {
        crate::testing::all_by_id(&app, "WhiteBalanceSection::temperature-slider")
            .first()
            .and_then(|e| e.accessible_value())
            .map(|v| v.to_string())
    };
    assert_eq!(temperature_says().as_deref(), Some("+0 mired"));
    let steps = state.borrow().sidecars[0].history.len();
    // The span reaches the panel's lights from D65: about -346 to
    // +70 mired.
    let (m0, m1) = (app.get_picture_mired_min(), app.get_picture_mired_max());
    assert!(
        (-347.0..-345.0).contains(&m0) && (70.0..71.0).contains(&m1),
        "{m0} {m1}"
    );
    // A drag from where none is on the track.
    let (at, size) = crate::testing::by_id(&app, "WhiteBalanceSection::temperature-slider");
    let (left, right) = (at.x + 68.0, at.x + size.width - 60.0);
    let x = left + (0.0 - m0) / (m1 - m0) * (right - left);
    let y = at.y + size.height / 2.0;
    crate::testing::drag(&app, &[(x, y), (x + 5.0, y), (x + 20.0, y)]);
    wait();
    assert!(!app.get_as_shot());
    let mired = app.get_temperature();
    assert!(mired > 0.0 && mired <= m1, "{mired}");
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::Offset {
            mired: mired as f64,
            tint: 0.0
        }
    );
    assert_eq!(state.borrow().sidecars[0].history.len(), steps + 1);
    let shown = format!("+{} mired", mired.round());
    assert_eq!(temperature_says(), Some(shown.clone()), "{shown}");
    // The preview: from the light developed at to the sliders'.
    let light = picture_light(
        &WhiteBalance::Offset {
            mired: mired as f64,
            tint: 0.0,
        },
        own,
    )
    .unwrap();
    let preview = crate::panel::color::preview_white(&mut state.borrow_mut(), (false, mired, 0.0));
    assert_eq!(preview, adaptation(own, light).unwrap());
    // The double-click: the slider alone back to none, As shot
    // left off; both at none is As shot, the offset of zero.
    slint::platform::update_timers_and_animations();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(600));
    slint::platform::update_timers_and_animations();
    let (at, size) = crate::testing::by_id(&app, "WhiteBalanceSection::temperature-slider");
    let (x, y) = (at.x + 30.0, at.y + size.height / 2.0);
    crate::testing::click(&app, x, y);
    crate::testing::click(&app, x, y);
    wait();
    assert_eq!((app.get_temperature(), app.get_tint()), (0.0, 0.0));
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::AsShot
    );

    // The dropper on a warm patch: the offset that makes it grey,
    // toward the lower kelvin, and the develop's light is shown.
    white_from_neutral(state.borrow_mut(), &app, [0.4, 0.3, 0.2]).unwrap();
    assert!(!app.get_as_shot());
    let (m, d) = (app.get_temperature(), app.get_tint());
    assert!(m < 0.0, "a warm patch wants a warmer light: {m}");
    let edit = read_edit(&app, &state.borrow().develop.edit, None);
    let light = picture_light(&edit.white_balance, own).unwrap();
    let grey = greycard_core::color::apply3(&adaptation(own, light).unwrap(), [0.4, 0.3, 0.2]);
    let mean = (grey[0] + grey[1] + grey[2]) / 3.0;
    assert!(
        grey.iter().all(|v| (v - mean).abs() < 2e-3),
        "{grey:?} at {m} {d}"
    );

    // Auto from a picture all of one warm cast, from As shot and
    // from the dropper's light: the same offset both times.
    app.set_as_shot(true);
    app.set_temperature(0.0);
    app.set_tint(0.0);
    let cast = vec![[0.36f32, 0.3, 0.22]; 1000];
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((cast.clone(), 40)));
    app.invoke_wb_auto();
    let first = (app.get_temperature(), app.get_tint());
    assert!(first.0 < 0.0, "{first:?}");
    // Developed at the first answer, the cast is the first answer's
    // picture: a second press finds the same light.
    let at = picture_light(
        &WhiteBalance::Offset {
            mired: first.0 as f64,
            tint: first.1 as f64,
        },
        own,
    )
    .unwrap();
    state.borrow_mut().develop.base_white = Some(WhiteBase {
        clip: 1.0,
        picture: Some((own, at)),
        ..WhiteBase::IDENTITY
    });
    let m = adaptation(own, at).unwrap();
    let shown: Vec<[f32; 3]> = cast
        .iter()
        .map(|px| greycard_core::color::apply3(&m, *px))
        .collect();
    AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((shown, 40)));
    app.invoke_wb_auto();
    let second = (app.get_temperature(), app.get_tint());
    assert!(
        (second.0 - first.0).abs() < 0.5 && (second.1 - first.1).abs() < 1e-4,
        "{first:?} then {second:?}"
    );
}

/// The kind the sliders are read as is the open file's, by its
/// name, from the moment it is chosen: a raw chosen after a picture
/// and moved before its develop lands takes a light, never an
/// offset, and a picture chosen after a raw takes an offset. A
/// picture holding a kelvin light (a paste from before) shows As
/// shot, and showing it writes nothing.
#[test]
fn the_sliders_are_read_as_the_open_files_kind_before_its_develop() {
    let app = window(2);
    let (state, _worker) = crate::testing::state_for(&app, picture_and_raw());
    a_picture_up(&app, &state);
    let wait = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ))
    };
    // The raw chosen; its decode has not landed, so the last
    // decode's word (a picture) and its develop still stand.
    app.invoke_select(1);
    assert!(!app.get_raw_input(), "the last decode's, still");
    assert!(!app.get_picture_open());
    app.set_as_shot(false);
    app.set_temperature(4300.0);
    app.invoke_develop_changed();
    wait();
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::Custom {
            temperature: 4300.0,
            tint: 0.0
        }
    );
    // And back to the picture, before its develop: an offset.
    app.set_raw_input(true);
    app.invoke_select(0);
    assert!(app.get_picture_open());
    app.set_as_shot(false);
    app.set_temperature(20.0);
    app.invoke_develop_changed();
    wait();
    assert_eq!(
        state.borrow().develop.edit.white_balance,
        WhiteBalance::Offset {
            mired: 20.0,
            tint: 0.0
        }
    );
    // A picture holding a kelvin light shows As shot at none, and
    // read back it is the light it was, not a step.
    let light = WhiteBalance::Custom {
        temperature: 3200.0,
        tint: 0.0,
    };
    let mut edit = state.borrow().develop.edit.clone();
    edit.white_balance = light.clone();
    crate::panel::edit::show_edit(&state.borrow(), &edit, &app, None);
    assert!(app.get_as_shot());
    assert_eq!((app.get_temperature(), app.get_tint()), (0.0, 0.0));
    assert_eq!(read_edit(&app, &edit, None).white_balance, light);
    // Moved, it is an offset.
    app.set_as_shot(false);
    app.set_temperature(-15.0);
    assert_eq!(
        read_edit(&app, &edit, None).white_balance,
        WhiteBalance::Offset {
            mired: -15.0,
            tint: 0.0
        }
    );
}

/// A mask on a picture: its white is an offset from the picture's
/// global, Follow global not offered and on from the start; the
/// dropper under it stores the offset from the global's light.
#[test]
fn a_masks_white_on_a_picture_adds_to_the_global() {
    let app = window(2);
    let (state, _worker) = crate::testing::state_for(&app, picture_and_raw());
    let own = a_picture_up(&app, &state);
    {
        let mut st = state.borrow_mut();
        st.develop.edit.adjustments.push(greycard_edit::Adjustment {
            id: 1,
            name: "Wall".into(),
            ..Default::default()
        });
        st.develop.target = Some(0);
    }
    app.set_panel_tab("Masks".into());
    app.set_target(1);
    // A warmer global, then the mask's white on.
    app.set_as_shot(false);
    app.set_temperature(30.0);
    app.set_tint(0.0);
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    assert!(app.get_local_wb_relative());
    assert_eq!(
        (app.get_local_temperature(), app.get_local_tint()),
        (0.0, 0.0)
    );
    assert_eq!(
        crate::testing::count_id(&app, "WhiteBalanceSection::follow-global-toggle"),
        0
    );
    // The mask's +20 is the picture's light moved by 50.
    app.set_local_temperature(20.0);
    let edit = read_edit(&app, &state.borrow().develop.edit, Some(0));
    let global = greycard_edit::white::picture_light(&edit.white_balance, own).unwrap();
    let mask = edit.adjustments[0].look.white_balance;
    let shift = crate::panel::color::local_white_shift(&mut state.borrow_mut(), (false, 30.0, 0.0))
        .expect("a picture's");
    assert_eq!(shift.global, global);
    let target = mask.temp_tint(global).unwrap();
    assert!((1e6 / own.cct - 1e6 / target.cct - 50.0).abs() < 1e-3);
    // The dropper under the mask: an offset from the global's, the
    // global left alone.
    white_from_neutral(state.borrow_mut(), &app, [0.4, 0.3, 0.2]).unwrap();
    assert!(app.get_local_wb_relative() && app.get_local_wb_on());
    assert!(app.get_local_temperature() < 0.0);
    assert_eq!(app.get_temperature(), 30.0);
}

fn a_lamp_mask(app: &App, state: &Rc<Shared>) -> greycard_core::TempTint {
    use greycard_core::color::profile_from_frame;
    app.invoke_select(0);
    let frame = daylight_frame();
    let profile = profile_from_frame(&frame).unwrap();
    let shot = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
    {
        let mut st = state.borrow_mut();
        st.develop.base_white = Some(WhiteBase::from(&shot, 1.0));
        st.develop.frame = Some((Arc::new(frame), Box::new(profile)));
        st.develop.shown_turn = Some((0, current_turn(&st)));
        st.develop.edit.adjustments.push(greycard_edit::Adjustment {
            id: 1,
            name: "Lamp".into(),
            ..Default::default()
        });
        st.develop.target = Some(0);
    }
    app.set_panel_tab("Masks".into());
    app.set_target(1);
    app.set_as_shot(true);
    shot.temp_tint
}

#[test]
fn a_masks_white_balance_starts_at_the_globals_and_takes_the_dropper() {
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let shot = a_lamp_mask(&app, &state);
    app.set_local_temperature(9000.0);
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    assert!(
        (app.get_local_temperature() as f64 - shot.cct).abs() < 1.0,
        "{} against {}",
        app.get_local_temperature(),
        shot.cct
    );
    assert!((app.get_local_tint() as f64 - shot.duv).abs() < 1e-4);
    // A custom global is taken as it stands, by a mask with no
    // light of its own, as a mask with none is shown.
    app.set_as_shot(false);
    app.set_temperature(4100.0);
    app.set_tint(0.003);
    crate::panel::edit::show_local_white(greycard_edit::LocalWhite::Off, &app);
    assert!(!app.get_local_wb_on() && !app.get_local_wb_kept());
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    assert_eq!(app.get_local_temperature(), 4100.0);
    assert_eq!(app.get_local_tint(), 0.003);
    let edit = read_edit(&app, &state.borrow().develop.edit, Some(0));
    assert_eq!(
        edit.adjustments[0].look.white_balance,
        greycard_edit::LocalWhite::Absolute {
            temperature: 4100.0,
            tint: 0.003f32 as f64
        }
    );
    // The dropper under a warm light, on the Masks tab: the mask's
    // white moves, the global's does not.
    app.set_local_wb_on(false);
    white_from_neutral(state.borrow_mut(), &app, [0.4, 0.3, 0.15]).unwrap();
    assert!(app.get_local_wb_on());
    assert!(
        app.get_local_temperature() < 4000.0,
        "{}",
        app.get_local_temperature()
    );
    assert_eq!(app.get_temperature(), 4100.0);
    assert_eq!(app.get_tint(), 0.003);
    // Off, the mask's light reaches nothing, and the global look
    // never carries one.
    app.set_local_wb_on(false);
    let edit = read_edit(&app, &state.borrow().develop.edit, Some(0));
    assert!(
        edit.adjustments[0]
            .look
            .white_balance
            .temp_tint(greycard_core::TempTint {
                cct: 5500.0,
                duv: 0.0
            })
            .is_none()
    );
    app.set_local_wb_on(true);
    let edit = read_edit(&app, &state.borrow().develop.edit, None);
    assert!(edit.look().white_balance.is_off());
}

/// A mask's white balance switched off keeps its light, and on
/// again brings it back rather than the global's, so the switch
/// compares the two; each switch is a step. A double-click on its
/// Temperature or Tint puts that slider alone back at the global's
/// white, a step, the switch left on; both back, the mask is the
/// global's.
#[test]
fn a_masks_white_balance_keeps_its_light_while_switched_off() {
    use greycard_edit::LocalWhite;
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    let shot = a_lamp_mask(&app, &state);
    let settle = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
            crate::panel::edit::SAVE_MS + 100,
        ));
        slint::platform::update_timers_and_animations();
    };
    settle();
    let stored = || {
        state.borrow().sidecars[0].current().adjustments[0]
            .look
            .white_balance
    };
    let steps = || state.borrow().sidecars[0].history.len();
    let lamp = LocalWhite::Absolute {
        temperature: 3300.0,
        tint: -0.004f32 as f64,
    };
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    app.set_local_temperature(3300.0);
    app.set_local_tint(-0.004);
    app.invoke_view_changed();
    settle();
    assert_eq!(stored(), lamp);
    let before = steps();

    // Off: the light is kept, and the picture is the global's.
    app.set_local_wb_on(false);
    app.invoke_local_wb_toggled();
    settle();
    let LocalWhite::Absolute { temperature, tint } = lamp else {
        unreachable!()
    };
    assert_eq!(stored(), LocalWhite::Kept { temperature, tint });
    assert!(
        stored()
            .temp_tint(greycard_core::TempTint {
                cct: 5500.0,
                duv: 0.0
            })
            .is_none()
    );
    assert_eq!(steps(), before + 1, "switching off is a step");
    assert_eq!(app.get_local_temperature(), 3300.0);

    // On: the same light, not the global's.
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    settle();
    assert_eq!(stored(), lamp);
    assert_eq!(steps(), before + 2, "switching on is a step");

    // Through the sidecar's file and a fresh panel: off, then shown
    // again, the panel holds the kept light and brings it back.
    app.set_local_wb_on(false);
    app.invoke_local_wb_toggled();
    settle();
    let json = state.borrow().sidecars[0].to_json();
    let read = greycard_edit::Sidecar::from_json(&json, &greycard_edit::Edit::default()).unwrap();
    assert_eq!(
        read.current().adjustments[0].look.white_balance,
        LocalWhite::Kept { temperature, tint }
    );
    app.set_local_temperature(9000.0);
    app.set_local_wb_kept(false);
    {
        let mut st = state.borrow_mut();
        st.develop.edit = read.current().clone();
        crate::panel::edit::show_edit(&st, read.current(), &app, Some(0));
    }
    assert!(!app.get_local_wb_on());
    assert!(app.get_local_wb_kept());
    assert_eq!(app.get_local_temperature(), 3300.0);
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    assert_eq!(app.get_local_temperature(), 3300.0);
    assert_eq!(app.get_local_tint(), -0.004);
    settle();
    assert_eq!(stored(), lamp);

    // A double-click on Temperature: that slider alone back to the
    // global's white, As shot read as the kelvin it means; the
    // tint, the switch, and Follow global stay. One step.
    let before = steps();
    app.invoke_local_wb_slider_reset("temperature".into());
    settle();
    assert!(app.get_local_wb_on() && app.get_local_wb_kept());
    assert!(!app.get_local_wb_relative());
    assert!((app.get_local_temperature() as f64 - shot.cct).abs() < 1.0);
    assert_eq!(app.get_local_tint(), -0.004);
    assert_eq!(steps(), before + 1, "a reset is a step");
    // On Tint: the global's tint, the temperature left.
    let temperature = app.get_local_temperature();
    app.invoke_local_wb_slider_reset("tint".into());
    settle();
    assert_eq!(app.get_local_temperature(), temperature);
    assert!((app.get_local_tint() as f64 - shot.duv).abs() < 1e-4);
    assert!(app.get_local_wb_on());
    assert_eq!(steps(), before + 2, "a reset is a step");
    // Both back: the light under the mask is the global's, so the
    // mask's matrix on the picture is the one following the global
    // makes, none.
    let shift = crate::panel::color::local_white_shift(
        &mut state.borrow_mut(),
        (true, app.get_temperature(), app.get_tint()),
    )
    .expect("a raw's");
    let m = shift.local(&stored()).expect("a light");
    for (i, row) in m.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            let identity = if i == j { 1.0 } else { 0.0 };
            assert!((v - identity).abs() < 1e-3, "{m:?}");
        }
    }
    // Switched off, the sliders take no hand: a reset reaching the
    // panel anyway leaves the kept light.
    app.set_local_temperature(3300.0);
    app.invoke_view_changed();
    app.set_local_wb_on(false);
    app.invoke_local_wb_toggled();
    settle();
    let before = (steps(), stored());
    app.invoke_local_wb_slider_reset("temperature".into());
    settle();
    assert!(!app.get_local_wb_on() && app.get_local_wb_kept());
    assert_eq!(app.get_local_temperature(), 3300.0);
    assert_eq!((steps(), stored()), before);
}

/// Follow global turns the mask's sliders into the offset from the
/// global's white and back, at the global as it stands, so nothing
/// moves; the dropper stores the light it finds less the global's;
/// a shown edit brings the switch back; and a double-click puts one
/// offset back to none.
#[test]
fn follow_global_turns_a_masks_light_into_an_offset_and_back() {
    use greycard_edit::LocalWhite;
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    a_lamp_mask(&app, &state);
    app.set_as_shot(false);
    app.set_temperature(5000.0);
    app.set_tint(0.002);
    let global = greycard_core::TempTint {
        cct: 5000.0,
        duv: 0.002f32 as f64,
    };
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    app.set_local_temperature(3200.0);
    app.set_local_tint(-0.004);
    let stored = |app: &App| {
        read_edit(app, &state.borrow().develop.edit, Some(0)).adjustments[0]
            .look
            .white_balance
    };
    let light = stored(&app).temp_tint(global).unwrap();
    app.set_local_wb_relative(true);
    app.invoke_local_wb_relative_toggled();
    // 200 mired at 5000 K, 312.5 at 3200 K: 112.5 toward the
    // lower kelvin.
    assert_eq!(app.get_local_temperature(), -112.5);
    assert!((app.get_local_tint() + 0.006).abs() < 1e-6);
    let offset = stored(&app);
    assert!(matches!(offset, LocalWhite::Relative { .. }), "{offset:?}");
    let same = offset.temp_tint(global).unwrap();
    assert!((same.cct - light.cct).abs() < 1e-3 && (same.duv - light.duv).abs() < 1e-6);
    // The global moves; the light under the mask moves with it,
    // the same 112.5 mired warmer.
    let higher = greycard_core::TempTint {
        cct: 6000.0,
        duv: global.duv,
    };
    let moved = offset.temp_tint(higher).unwrap().cct;
    assert!(
        (moved - 1e6 / (1e6 / 6000.0 + 112.5)).abs() < 1e-6,
        "{moved}"
    );
    // The dropper: the light that makes the patch grey, stored as
    // that light less the global's.
    white_from_neutral(state.borrow_mut(), &app, [0.4, 0.3, 0.15]).unwrap();
    assert!(app.get_local_wb_relative());
    let picked_offset = stored(&app).temp_tint(global).unwrap();
    app.set_local_wb_relative(false);
    white_from_neutral(state.borrow_mut(), &app, [0.4, 0.3, 0.15]).unwrap();
    let picked = stored(&app).temp_tint(global).unwrap();
    assert!(matches!(stored(&app), LocalWhite::Absolute { .. }));
    assert!(
        (picked.cct - picked_offset.cct).abs() < 0.5,
        "{picked:?} {picked_offset:?}"
    );
    assert!((picked.duv - picked_offset.duv).abs() < 1e-5);
    // Back on, and off again: the light it was, to the slider's
    // own precision.
    app.set_local_temperature(3200.0);
    app.set_local_tint(-0.004);
    app.set_local_wb_relative(true);
    app.invoke_local_wb_relative_toggled();
    app.set_local_wb_relative(false);
    app.invoke_local_wb_relative_toggled();
    assert!((app.get_local_temperature() - 3200.0).abs() < 1e-2);
    assert!((app.get_local_tint() + 0.004).abs() < 1e-6);
    // A shown edit with an offset, kept while switched off, brings
    // the switch back with it.
    let mut edit = state.borrow().develop.edit.clone();
    edit.adjustments[0].look.white_balance = LocalWhite::KeptRelative {
        mired: 40.0,
        tint: 0.0,
    };
    crate::panel::edit::show_edit(&state.borrow(), &edit, &app, Some(0));
    assert!(app.get_local_wb_relative() && app.get_local_wb_kept());
    assert!(!app.get_local_wb_on());
    assert_eq!(app.get_local_temperature(), 40.0);
    // On, a double-click puts that offset alone back to none, the
    // other and Follow global left.
    app.set_local_wb_on(true);
    app.invoke_local_wb_toggled();
    app.set_local_tint(0.002);
    app.invoke_local_wb_slider_reset("temperature".into());
    assert!(app.get_local_wb_on() && app.get_local_wb_relative());
    assert_eq!(
        stored(&app),
        LocalWhite::Relative {
            mired: 0.0,
            tint: 0.002f32 as f64
        }
    );
    app.set_local_temperature(40.0);
    app.invoke_local_wb_slider_reset("tint".into());
    assert_eq!(
        stored(&app),
        LocalWhite::Relative {
            mired: 40.0,
            tint: 0.0
        }
    );
    app.invoke_local_wb_slider_reset("temperature".into());
    assert_eq!(
        stored(&app),
        LocalWhite::Relative {
            mired: 0.0,
            tint: 0.0
        }
    );
}

/// A window with a 6000 by 4000 picture on the viewport, centered,
/// at `zoom` (0 fits).
fn scrolled_view(zoom: f32) -> (App, Rc<Shared>, Rc<crate::worker::Worker>) {
    let app = window(1);
    let (state, worker) = crate::testing::retouch_state(&app);
    {
        let mut st = state.borrow_mut();
        st.develop.image_size = (6000, 4000);
        st.develop.center = (3000.0, 2000.0);
        st.develop.zoom = zoom;
    }
    (app, state, worker)
}

/// The tester's report: a drag that starts on the canvas round a
/// fitted picture pans nothing, a drag that starts on the picture
/// pans it as before, and one that starts there and leaves it
/// keeps panning. Zoomed in, the picture covers the view and a
/// drag anywhere pans.
#[test]
fn only_a_drag_that_starts_on_the_picture_pans_it() {
    let (app, state, _worker) = scrolled_view(0.0);
    let base = i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    let (vw, vh) = (app.get_view_width() as f32, app.get_view_height() as f32);
    let scale = app.window().scale_factor();
    let (mx, my) = (vw / scale / 2.0, vh / scale / 2.0);
    // Fitted, the picture in the middle and canvas in the corner.
    let canvas = (2.0, 2.0);
    assert!(!app.invoke_picture_at(canvas.0, canvas.1));
    assert!(app.invoke_picture_at(mx, my));
    let at = |p: (f32, f32)| (base.x + p.0, base.y + p.1);
    let drag_from = |p: (f32, f32), by: f32| {
        let (x, y) = at(p);
        crate::testing::drag(
            &app,
            &[(x, y), (x + 10.0, y + by / 4.0), (x + 40.0, y + by)],
        );
    };
    let still = state.borrow().develop.center;
    drag_from(canvas, 0.0);
    assert_eq!(state.borrow().develop.center, still, "the canvas panned it");
    // From the picture, out onto the canvas: it pans all the way.
    drag_from((mx, my), -(my - 2.0));
    let moved = state.borrow().develop.center;
    assert_ne!(moved, still, "the picture did not pan");
    assert!(
        moved.1 > still.1,
        "the pan followed the drag off the picture"
    );
    // Dragged clean off the view, every point counts as the
    // picture's, so a drag from anywhere brings it back.
    state.borrow_mut().develop.center = (36000.0, 2000.0);
    assert!(app.invoke_picture_at(canvas.0, canvas.1));
    drag_from(canvas, 0.0);
    assert!(
        state.borrow().develop.center.0 < 36000.0,
        "the stranded picture stayed off"
    );
    // Zoomed in, the canvas's old place is the picture's.
    state.borrow_mut().develop.zoom = 1.0;
    state.borrow_mut().develop.center = (3000.0, 2000.0);
    assert!(app.invoke_picture_at(canvas.0, canvas.1));
    drag_from(canvas, 0.0);
    assert_ne!(state.borrow().develop.center, (3000.0, 2000.0));
}

/// A wheel event at the middle of the view.
fn scroll(app: &App, delta_x: f32, delta_y: f32) {
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(600.0, 400.0),
            delta_x,
            delta_y,
        });
}

/// Under natural scrolling a swipe up arrives as a negative delta,
/// and the brush and the patch, values rather than content, turn it
/// back as a slider does: a swipe up grows them. The picture's zoom
/// keeps the system's way.
#[test]
fn under_natural_scrolling_the_brush_grows_with_a_swipe_up() {
    let (app, _state, _worker) = scrolled_view(1.0);
    app.global::<crate::SliderKeys>().set_natural(true);
    app.set_brush_size(0.05);
    app.set_placing("Brush".into());
    scroll(&app, 0.0, -60.0);
    assert!(
        (app.get_brush_size() - 0.055).abs() < 1e-6,
        "a swipe up grows the brush"
    );
    scroll(&app, 0.0, 60.0);
    assert!(
        (app.get_brush_size() - 0.05).abs() < 1e-6,
        "and a swipe down shrinks it"
    );
    app.set_placing("".into());
    app.set_patch_size(0.05);
    app.set_retouch_mode("Heal".into());
    scroll(&app, 0.0, -60.0);
    assert!((app.get_patch_size() - 0.055).abs() < 1e-6, "the patch too");
}

/// A mouse wheel's click, a single vertical delta of 60 px, zooms a
/// step about the pointer, from the fit as from a zoom.
#[test]
fn a_wheel_click_over_the_picture_zooms() {
    let (app, state, _worker) = scrolled_view(1.0);
    scroll(&app, 0.0, 60.0);
    assert_eq!(state.borrow().develop.zoom, 1.25);
    scroll(&app, 0.0, -60.0);
    assert_eq!(state.borrow().develop.zoom, 1.0);

    let (app, state, _worker) = scrolled_view(0.0);
    let (vw, vh) = (app.get_view_width() as u32, app.get_view_height() as u32);
    let fit = effective_zoom(&state.borrow(), vw, vh);
    scroll(&app, 0.0, 60.0);
    assert!((state.borrow().develop.zoom - fit * 1.25).abs() < 1e-6);
}

/// With the brush or the retouch patch in hand the wheel sizes it
/// by 1.1x for each 60 px of delta, however it arrives: a click is
/// one step, a touchpad's small deltas add up to one, and a turn of
/// direction starts the count over.
#[test]
fn the_wheel_sizes_the_brush_and_the_patch_by_the_accumulated_delta() {
    type Get = fn(&App) -> f32;
    type Put = fn(&App);
    let brush: (Get, Put) = (App::get_brush_size, |a| a.set_placing("Brush".into()));
    let patch: (Get, Put) = (App::get_patch_size, |a| a.set_retouch_mode("Heal".into()));
    for (size, hold) in [brush, patch] {
        let (app, _state, _worker) = scrolled_view(1.0);
        app.set_brush_size(0.05);
        app.set_patch_size(0.05);
        hold(&app);
        let base = size(&app);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-6;

        scroll(&app, 0.0, 60.0);
        assert!(near(size(&app), base * 1.1), "a click is a step");
        scroll(&app, 0.0, -60.0);
        assert!(near(size(&app), base), "and back");

        for _ in 0..6 {
            scroll(&app, 0.0, 10.0);
        }
        assert!(
            near(size(&app), base * 1.1),
            "six small deltas are one step"
        );
        for _ in 0..6 {
            scroll(&app, 0.0, -10.0);
        }
        assert!(near(size(&app), base), "and six back");

        for _ in 0..3 {
            scroll(&app, 0.0, 10.0);
        }
        for _ in 0..3 {
            scroll(&app, 0.0, -10.0);
        }
        assert!(near(size(&app), base), "a reversal does not step");
    }
}

/// A touchpad's small deltas pan the picture with the fingers and
/// leave the zoom alone, adding up as they come; deltas on both
/// axes, or a tilted wheel's sideways click, pan too.
#[test]
fn a_touchpad_scroll_over_the_picture_pans() {
    let (app, state, _worker) = scrolled_view(1.0);
    let scale = app.window().scale_factor();
    scroll(&app, 0.0, 10.0);
    {
        let st = state.borrow();
        assert_eq!(st.develop.zoom, 1.0, "a small delta does not zoom");
        assert_eq!(st.develop.center, (3000.0, 2000.0 - 10.0 * scale));
    }
    for _ in 0..5 {
        scroll(&app, 3.0, -4.0);
    }
    {
        let st = state.borrow();
        assert_eq!(st.develop.zoom, 1.0);
        assert_eq!(
            st.develop.center,
            (3000.0 - 15.0 * scale, 2000.0 + 10.0 * scale)
        );
    }
    scroll(&app, 60.0, 60.0);
    assert_eq!(state.borrow().develop.zoom, 1.0);
    scroll(&app, 60.0, 0.0);
    assert_eq!(state.borrow().develop.zoom, 1.0);
    assert_eq!(state.borrow().develop.center.0, 3000.0 - 135.0 * scale);
}

/// A touchpad's fast flick carries deltas of a click's size and
/// more: one that is no whole number of clicks pans, and a whole
/// click within a quarter second of a pan is part of the pan. The
/// same click after a quiet spell is a wheel's, and zooms.
#[test]
fn a_flick_that_reaches_a_clicks_size_still_pans() {
    let (app, state, _worker) = scrolled_view(1.0);
    let scale = app.window().scale_factor();
    scroll(&app, 0.0, 72.0);
    {
        let st = state.borrow();
        assert_eq!(st.develop.zoom, 1.0, "72 px is no whole click");
        assert_eq!(st.develop.center.1, 2000.0 - 72.0 * scale);
    }
    // Well after that: a flick's ramp, which lands on a whole click
    // on its way up.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1000));
    for dy in [8.0, 24.0, 48.0, 60.0] {
        scroll(&app, 0.0, dy);
    }
    {
        let st = state.borrow();
        assert_eq!(st.develop.zoom, 1.0, "a click's size mid-pan pans");
        assert_eq!(st.develop.center.1, 2000.0 - 212.0 * scale);
    }
    // Still within the quarter second of the last pan.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
    scroll(&app, 0.0, -120.0);
    assert_eq!(state.borrow().develop.zoom, 1.0);
    // A quiet spell, and the same click zooms.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
    scroll(&app, 0.0, 60.0);
    assert_eq!(state.borrow().develop.zoom, 1.25);
}

/// A high-resolution wheel's fragments pan a little, and its whole
/// detent, close behind them, still zooms: fragments that small
/// are no flick.
#[test]
fn a_high_resolution_wheels_detent_still_zooms() {
    let (app, state, _worker) = scrolled_view(1.0);
    let scale = app.window().scale_factor();
    for _ in 0..3 {
        scroll(&app, 0.0, 15.0);
    }
    assert_eq!(state.borrow().develop.zoom, 1.0);
    assert_eq!(state.borrow().develop.center.1, 2000.0 - 45.0 * scale);
    scroll(&app, 0.0, 60.0);
    assert_eq!(state.borrow().develop.zoom, 1.25);
}

/// A zoom by nothing (Ctrl and a purely sideways scroll; a gesture
/// that only rotates) leaves a fitted picture fitted.
#[test]
fn a_zoom_by_nothing_keeps_the_fit() {
    use slint::platform::{Key, WindowEvent};
    let (app, state, _worker) = scrolled_view(0.0);
    app.invoke_zoom(1.0, 600.0, 400.0);
    assert_eq!(state.borrow().develop.zoom, 0.0);
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    scroll(&app, 60.0, 0.0);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    let st = state.borrow();
    assert_eq!(st.develop.zoom, 0.0, "still the fit");
    assert_eq!(st.develop.center, (3000.0, 2000.0));
}

/// Where the whole picture is in view a pan has nowhere to go: the
/// picture stays put, and nothing zooms instead.
#[test]
fn a_touchpad_scroll_on_a_fitted_picture_does_nothing() {
    let (app, state, _worker) = scrolled_view(0.0);
    for _ in 0..10 {
        scroll(&app, 7.0, -12.0);
    }
    let st = state.borrow();
    assert_eq!(st.develop.zoom, 0.0);
    assert_eq!(st.develop.center, (3000.0, 2000.0));
}

/// With Ctrl held a touchpad's delta zooms, by as much of a step as
/// the delta is of a click, and does not pan.
#[test]
fn ctrl_and_a_touchpad_scroll_zooms() {
    use slint::platform::{Key, WindowEvent};
    let (app, state, _worker) = scrolled_view(1.0);
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    scroll(&app, 0.0, 10.0);
    let zoomed = state.borrow().develop.zoom;
    assert!(
        (zoomed - 1.25f32.powf(10.0 / 60.0)).abs() < 1e-5,
        "zoomed to {zoomed}"
    );
    // Six of them make a click's step.
    for _ in 0..5 {
        scroll(&app, 0.0, 10.0);
    }
    assert!((state.borrow().develop.zoom - 1.25).abs() < 1e-4);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    // Let go of Ctrl and the same delta pans again.
    let before = state.borrow().develop.center;
    scroll(&app, 0.0, 10.0);
    assert!((state.borrow().develop.zoom - 1.25).abs() < 1e-4);
    assert_ne!(state.borrow().develop.center, before);
}

/// Library and Loupe at the left pane's top: the grid and the
/// one frame, the one on screen pressed, in the same place in
/// both. The Loupe's modes are `panel::cull`'s tests.
#[test]
fn library_and_loupe_switch_the_view_and_the_one_on_screen_is_pressed() {
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(6));
    app.invoke_select(0);
    let pressed = |id: &str| crate::testing::pressed_id(&app, id);
    let middle = |id: &str| {
        let (at, size) = crate::testing::by_id(&app, id);
        (at.x + size.width / 2.0, at.y + size.height / 2.0, at)
    };
    assert!(!app.get_grid_open());
    assert_eq!(pressed("App::loupe-view-button"), Some(true));
    assert_eq!(pressed("App::library-view-button"), Some(false));
    let (lx, ly, library_at) = middle("App::library-view-button");
    // The pane's top left, above everything that scrolls.
    assert!(
        library_at.x < 240.0 && library_at.y < 40.0,
        "{library_at:?}"
    );
    click(&app, lx, ly);
    assert!(app.get_grid_open());
    assert_eq!(pressed("App::library-view-button"), Some(true));
    assert_eq!(pressed("App::loupe-view-button"), Some(false));
    // Where it was in the loupe.
    let (_, _, there) = middle("App::library-view-button");
    assert_eq!((there.x, there.y), (library_at.x, library_at.y));
    // Library again is still the library; G agrees with the two.
    click(&app, lx, ly);
    assert!(app.get_grid_open());
    let (dx, dy, _) = middle("App::loupe-view-button");
    click(&app, dx, dy);
    assert!(!app.get_grid_open());
    crate::testing::press(&app, "g");
    assert!(app.get_grid_open());
    assert_eq!(pressed("App::library-view-button"), Some(true));
    // From the lightbox with the pane brought back: Loupe leaves
    // it for the frame, Library for the grid, as its own ways out.
    for (label, grid) in [
        ("App::loupe-view-button", false),
        ("App::library-view-button", true),
    ] {
        app.invoke_lightbox_enter();
        assert!(app.get_lightbox());
        app.set_left_hidden(false);
        let (x, y, _) = middle(label);
        click(&app, x, y);
        assert!(!app.get_lightbox(), "{label}");
        assert_eq!(app.get_grid_open(), grid, "{label}");
    }
}

/// The Help menu's two of its own: the keys sheet, and the guide,
/// which a test never opens a browser for.
#[test]
fn help_has_the_keys_and_the_guide_and_a_test_opens_no_browser() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    assert_eq!(crate::testing::count_id(&app, "App::help-button"), 1);
    app.invoke_guide_asked();
    assert_eq!(app.get_status(), "the guide is open in the browser");
    assert_eq!(Shown::sheet("keys"), Ok(Shown::Keys));
    assert_eq!(Shown::sheet("help"), Ok(Shown::Help));
    Shown::Keys.open(&app);
    assert!(app.get_keys_open());
    assert!(app.get_keys_rows().row_count() > 20);
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_keys_open());
    // The menu asked for opens it, and the ask is spent.
    slint::platform::update_timers_and_animations();
    Shown::Help.open(&app);
    crate::testing::count_id(&app, "App::help-button");
    slint::platform::update_timers_and_animations();
    assert!(!app.get_help_menu_shown());
}

/// The Fit button's menu, opened by a right-click on it: 200%
/// zooms to 2:1 about the view's center, Fit fits again, and a
/// plain click on the button still fits.
#[test]
fn the_fit_buttons_menu_chooses_a_zoom_and_a_click_still_fits() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    {
        let mut st = state.borrow_mut();
        st.develop.image_size = (6000, 4000);
        st.develop.center = (3000.0, 2000.0);
    }
    let zoom = || {
        let (vw, vh) = (app.get_view_width() as u32, app.get_view_height() as u32);
        effective_zoom(&state.borrow(), vw, vh)
    };
    let fitted = zoom();
    assert!(fitted < 1.0, "{fitted}");
    // The open menu's entries titled `title`, data here.
    let found = |title: &str| -> Vec<(f32, f32)> {
        crate::testing::menu_entries(&app)
            .into_iter()
            .filter(|e| e.accessible_label().as_deref() == Some(title))
            .map(|e| {
                let (at, size) = (e.absolute_position(), e.size());
                (at.x + size.width / 2.0, at.y + size.height / 2.0)
            })
            .collect()
    };
    // The footer's Fit.
    let button = {
        let (at, size) = crate::testing::by_id(&app, "App::fit-button");
        (at.x + size.width / 2.0, at.y + size.height / 2.0)
    };
    // An entry of the open menu by its place, from the keys: the
    // menu is a popup of its own, with places of its own. Its
    // last entry's words are nowhere else.
    let pick = |nth: usize| {
        assert_eq!(found("400%").len(), 1, "the menu is up");
        for _ in 0..nth {
            crate::testing::press(&app, slint::platform::Key::DownArrow);
        }
        crate::testing::press(&app, slint::platform::Key::Return);
        slint::platform::update_timers_and_animations();
    };
    assert!(found("400%").is_empty(), "no menu yet");
    let right_click = || {
        let position = slint::LogicalPosition::new(button.0, button.1);
        let right = PointerEventButton::Right;
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed {
                position,
                button: right,
            },
            WindowEvent::PointerReleased {
                position,
                button: right,
            },
        ] {
            app.window().dispatch_event(event);
        }
    };
    right_click();
    pick(6);
    assert_eq!(zoom(), 2.0);
    assert_eq!(state.borrow().develop.center, (3000.0, 2000.0));
    // The ask from Rust opens the same menu, and Fit in it fits.
    app.set_zoom_menu_shown(true);
    pick(1);
    assert!((zoom() - fitted).abs() < 1e-6);
    // Back at 2:1, a plain click on the button fits as before.
    app.set_zoom_menu_shown(true);
    pick(6);
    assert_eq!(zoom(), 2.0);
    crate::testing::click(&app, button.0, button.1);
    assert!((zoom() - fitted).abs() < 1e-6);
    // Escape puts the menu away and leaves the window's keys with
    // the focus, not the button: Space zooms to 1:1 rather than
    // pressing Fit.
    right_click();
    assert_eq!(found("400%").len(), 1, "the menu is up");
    crate::testing::press(&app, slint::platform::Key::Escape);
    slint::platform::update_timers_and_animations();
    assert!(found("400%").is_empty(), "the menu is away");
    crate::testing::press(&app, " ");
    assert_eq!(zoom(), 1.0);
    assert_eq!(Shown::sheet("zoom"), Ok(Shown::Zoom));
}

/// The edge arrows and the undo and redo buttons show a picture
/// and no text: each says its word at the pointer after a rest,
/// the word names what a press will do, and the press takes it
/// down.
#[test]
fn the_edge_arrows_and_undo_say_their_words_after_a_rest() {
    use crate::testing::rest_on_id;
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    app.set_can_undo(true);
    app.set_can_redo(true);
    let away = || {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(2.0, 2.0),
            });
        slint::platform::update_timers_and_animations();
    };
    // Each by its id, and the word it says, in the window's language.
    for (label, words) in [
        ("PanelHead::undo-button", "Undo"),
        ("PanelHead::redo-button", "Redo"),
        ("App::right-arrow", "Hide panel"),
        ("App::strip-arrow", "Hide filmstrip"),
    ] {
        assert_eq!(app.get_tip_text(), "", "no word before a rest on {label}");
        rest_on_id(&app, label);
        assert_eq!(app.get_tip_text(), crate::i18n::text(words), "{label}");
        away();
        assert_eq!(app.get_tip_text(), "", "{label} leaves with the pointer");
    }
    // The panel scrolled under a resting pointer takes the word
    // down; the Develop tab is long enough to scroll.
    app.set_panel_tab("Develop".into());
    crate::testing::rest_on_id(&app, "PanelHead::undo-button");
    assert_eq!(app.get_tip_text(), "Undo");
    app.set_panel_scroll(-40.0);
    assert_ne!(app.get_panel_scroll(), 0.0, "the panel scrolls");
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), "", "a scroll took the word down");
    away();
    // A key takes the word down, and a panel the key puts away
    // takes its controls with it: F8 over a resting Undo.
    away();
    crate::testing::rest_on_id(&app, "PanelHead::undo-button");
    assert_eq!(app.get_tip_text(), "Undo");
    crate::testing::press(&app, slint::platform::Key::F8);
    slint::platform::update_timers_and_animations();
    assert_eq!(app.get_tip_text(), "", "a key took the word down");
    crate::testing::press(&app, slint::platform::Key::F8);
    away();
    // The press: the word, then the panel gone and the word with it,
    // and the arrow's next word is the way back.
    let (x, y) = crate::testing::rest_on_id(&app, "App::right-arrow");
    assert_eq!(app.get_tip_text(), "Hide panel");
    click(&app, x, y);
    assert_eq!(app.get_tip_text(), "", "the press took the word down");
    assert!(app.get_right_hidden());
    away();
    crate::testing::rest_on_id(&app, "App::right-arrow");
    assert_eq!(app.get_tip_text(), "Show panel");
}

/// NAVIGATOR is one height, its header and a box the width inside
/// the section's box at 3:2, with a 3:2 frame, a 16:9 one, a portrait one, and a
/// chosen frame whose picture is on its way: nothing below it
/// moves. The frame sits centered in the box at its own shape,
/// and a press on it navigates by where it fell on the frame, not
/// on the box; on the empty box a press navigates nowhere. With
/// nothing chosen, the header alone.
#[test]
fn the_navigator_keeps_one_height_whatever_the_frame() {
    use crate::testing::{click, settle};
    use i_slint_backend_testing::ElementHandle;
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let asked = Rc::new(Cell::new((-1.0f32, -1.0f32)));
    let a = asked.clone();
    app.on_navigate(move |x, y| a.set((x, y)));
    let picture = |w: u32, h: u32| {
        slint::Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h))
    };
    let find = |id: &str| {
        let e = ElementHandle::find_by_element_id(&app, id)
            .next()
            .unwrap_or_else(|| panic!("{id} in the window"));
        (e.absolute_position(), e.size())
    };
    // A body's growth is the section's 120ms fold.
    let show = |image: slint::Image| {
        app.set_navigator(image);
        settle(&app);
        settle(&app);
    };
    app.set_selected(0);
    let mut heights = Vec::new();
    for (w, h) in [(300, 200), (320, 180), (200, 300)] {
        show(picture(w, h));
        let (section_at, section) = find("App::navigator-section");
        heights.push(section.height);
        let (held_at, held) = find("NavigatorSection::held");
        assert!(
            (held.height - held.width / 1.5).abs() < 0.5,
            "the box at 3:2: {held:?}"
        );
        // Inside the section's box: 8 px of padding under it.
        assert!(
            held_at.y > section_at.y
                && (held_at.y + held.height + 8.0 - section_at.y - section.height).abs() < 0.5,
            "the header, then the box: {held_at:?} {held:?} in {section_at:?} {section:?}"
        );
        let (at, shown) = find("NavigatorSection::shown");
        let shape = w as f32 / h as f32;
        assert!(
            (shown.width / shown.height - shape).abs() < 0.02,
            "{shown:?} at {shape}"
        );
        assert!(
            (at.x + shown.width / 2.0 - held_at.x - held.width / 2.0).abs() < 1.0
                && (at.y + shown.height / 2.0 - held_at.y - held.height / 2.0).abs() < 1.0,
            "centered: {at:?} {shown:?} in {held_at:?} {held:?}"
        );
        // A press a quarter in from the frame's left and top.
        asked.set((-1.0, -1.0));
        click(&app, at.x + shown.width / 4.0, at.y + shown.height / 4.0);
        let (x, y) = asked.get();
        assert!(
            (x - 0.25).abs() < 0.02 && (y - 0.25).abs() < 0.02,
            "{x} {y} at {w}x{h}"
        );
    }
    // Chosen, its picture on its way: the box holds its place and
    // a press on it navigates nowhere.
    show(slint::Image::default());
    heights.push(find("App::navigator-section").1.height);
    let (held_at, held) = find("NavigatorSection::held");
    asked.set((-1.0, -1.0));
    click(
        &app,
        held_at.x + held.width / 2.0,
        held_at.y + held.height / 2.0,
    );
    assert_eq!(asked.get(), (-1.0, -1.0), "no frame, no navigating");
    assert!(
        heights.iter().all(|h| (h - heights[0]).abs() < 0.5),
        "{heights:?}"
    );
    // Nothing chosen: the header alone, as before.
    app.set_selected(-1);
    settle(&app);
    settle(&app);
    assert!(
        ElementHandle::find_by_element_id(&app, "NavigatorSection::held")
            .next()
            .is_none(),
        "no box with nothing chosen"
    );
    let header = find("App::navigator-section").1.height;
    assert!(
        header > 0.0 && header < heights[0] - held.height + 1.0,
        "the header alone: {header} against {heights:?}"
    );
}

/// At a wide pane NAVIGATOR keeps the pane's width but stops
/// growing taller: a panorama takes the whole width, and a
/// portrait the capped height, the section one height for both.
#[test]
fn the_navigator_grows_no_taller_at_a_wide_pane() {
    use crate::testing::settle;
    use i_slint_backend_testing::ElementHandle;
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    app.set_left_size(500.0);
    app.set_selected(0);
    let find = |id: &str| {
        let e = ElementHandle::find_by_element_id(&app, id)
            .next()
            .unwrap_or_else(|| panic!("{id} in the window"));
        (e.absolute_position(), e.size())
    };
    let show = |w: u32, h: u32| {
        app.set_navigator(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
            slint::Rgba8Pixel,
        >::new(w, h)));
        settle(&app);
        settle(&app);
    };
    // The cap: a 3:2 box 260 px wide.
    let tallest = 260.0 / 1.5;
    show(1000, 200);
    let section = find("App::navigator-section").1;
    let (_, held) = find("NavigatorSection::held");
    let (_, shown) = find("NavigatorSection::shown");
    // The section's box, less its 6 px sides.
    assert!(
        held.width > 400.0 && (held.width + 12.0 - section.width).abs() < 0.5,
        "the box the pane's width: {held:?} in {section:?}"
    );
    assert!((held.height - tallest).abs() < 0.5, "capped: {held:?}");
    assert!(
        (shown.width - held.width).abs() < 0.5 && (shown.height - held.width / 5.0).abs() < 0.5,
        "the panorama across the whole width: {shown:?} in {held:?}"
    );
    show(200, 300);
    let (_, portrait) = find("NavigatorSection::held");
    let (_, shown) = find("NavigatorSection::shown");
    assert!(
        (portrait.height - tallest).abs() < 0.5,
        "the portrait's box no taller: {portrait:?}"
    );
    assert!(
        (shown.height - tallest).abs() < 0.5 && (shown.width - tallest * 2.0 / 3.0).abs() < 0.5,
        "the portrait at the capped height: {shown:?}"
    );
    assert!(
        (find("App::navigator-section").1.height - section.height).abs() < 0.5,
        "one height for both"
    );
}

/// 100% is one picture pixel a physical pixel at any UI scale: at a
/// scale of 1.5 the view is 1.5 physical pixels a logical one, the
/// zoom the viewport draws at 100% is 1, and that is not past 1:1,
/// so the picture is read as it always was; at 200% it is past, and
/// each picture pixel is a block (`magnify`).
///
/// What it pins is the window's half: the size the renderer draws at
/// (`view-width`, which the render hands `Renderer::render`) is the
/// view's physical pixels, not its logical ones, and the zoom the menu
/// names goes to the renderer as it is, not times the scale. Were
/// either logical, 100% at 1.5 would draw at 1.5 target pixels a
/// picture pixel and come out blocks. The renderer's half, strictly
/// past one target pixel a picture pixel, is `magnify`'s and the
/// viewport crate's tests'.
#[test]
fn a_one_and_a_half_ui_scale_at_100_is_not_past_1_to_1() {
    use greycard_viewport::render::magnify::magnified;
    let (app, state, _worker) = hover_frame(1.5);
    assert_eq!(app.window().scale_factor(), 1.5);
    let view = i_slint_backend_testing::ElementHandle::find_by_element_id(&app, "App::view")
        .next()
        .expect("the viewport is on screen")
        .size();
    let (vw, vh) = (
        app.get_view_width().max(1) as u32,
        app.get_view_height().max(1) as u32,
    );
    assert!(
        (vw as f32 - view.width * 1.5).abs() <= 1.0,
        "the view is {vw} physical pixels for {} logical",
        view.width
    );
    let (mx, my) = view_middle(&app);
    let drawn_at = |level: f32| {
        app.invoke_zoom_to(level, mx, my);
        let st = state.borrow();
        assert_eq!(st.develop.zoom, level);
        effective_zoom(&st, vw, vh)
    };
    let at_100 = drawn_at(1.0);
    assert_eq!(at_100, 1.0);
    assert!(!magnified(at_100, 1.0), "100% at a scale of 1.5");
    let at_200 = drawn_at(2.0);
    assert_eq!(at_200, 2.0);
    assert!(magnified(at_200, 1.0), "200% at a scale of 1.5");
}

/// The source pixel under a point of the leveled plane, in f64 and
/// written from the geometry's own description rather than its code:
/// the keystone about the plane's center, then the fine turn, the
/// quarter turns, and the mirror, about the source's center.
fn plane_to_source(g: &Geometry, (w, h): (f64, f64), r: (f64, f64)) -> (f64, f64) {
    let (pw, ph) = if g.turns % 2 == 1 { (h, w) } else { (w, h) };
    let kh = f64::from(g.horizontal).to_radians().tan() / (pw / 2.0);
    let kv = -f64::from(g.vertical).to_radians().tan() / (ph / 2.0);
    let (dx, dy) = (r.0 - pw / 2.0, r.1 - ph / 2.0);
    let d = 1.0 + kh * dx + kv * dy;
    let (x, y) = (dx / d, dy / d);
    let (s, c) = f64::from(g.angle).to_radians().sin_cos();
    let (x, y) = (c * x - s * y, s * x + c * y);
    let (x, y) = match g.turns % 4 {
        0 => (x, y),
        1 => (-y, x),
        2 => (-x, -y),
        _ => (y, -x),
    };
    let x = if g.flip { -x } else { x };
    (w / 2.0 + x, h / 2.0 + y)
}

/// A view's center carried from one frame laid out to another, under
/// random turns, mirrors, angles, keystones, and crops, shown cropped
/// or whole: wherever the new frame holds it unclamped, it is over
/// the same source pixel, measured by [`plane_to_source`].
#[test]
fn a_carried_center_stays_over_the_same_source_pixel() {
    use greycard_edit::geometry::{Aspect, Crop};
    let (sw, sh) = (6000.0f32, 4000.0f32);
    let mut seed = 0x5eedu64;
    let mut rnd = |a: f32, b: f32| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        a + (b - a) * ((seed >> 40) as f32 / (1u64 << 24) as f32)
    };
    let of = Some((PathBuf::from("/nowhere/IMG_0000.CR3"), None));
    let laid = |rnd: &mut dyn FnMut(f32, f32) -> f32| {
        let w = rnd(0.1, 0.6);
        let h = rnd(0.1, 0.6);
        let geometry = Geometry {
            turns: rnd(0.0, 4.0) as u8 % 4,
            flip: rnd(0.0, 1.0) < 0.5,
            angle: rnd(-45.0, 45.0),
            vertical: rnd(-20.0, 20.0),
            horizontal: rnd(-20.0, 20.0),
            crop: (rnd(0.0, 1.0) < 0.5).then(|| Crop {
                x: rnd(0.0, 1.0 - w),
                y: rnd(0.0, 1.0 - h),
                w,
                h,
            }),
            aspect: Aspect::Free,
            portrait: false,
        };
        let frame = if rnd(0.0, 1.0) < 0.3 {
            geometry.bounds(sw, sh)
        } else {
            geometry.frame(sw, sh, None)
        };
        owners::Laid {
            of: of.clone(),
            geometry,
            source: (sw, sh),
            turn: 0,
            frame,
        }
    };
    let source = |l: &owners::Laid, c: (f32, f32)| {
        plane_to_source(
            &l.geometry,
            (f64::from(sw), f64::from(sh)),
            (
                f64::from(l.frame.origin.0) + f64::from(c.0),
                f64::from(l.frame.origin.1) + f64::from(c.1),
            ),
        )
    };
    let (mut checked, mut worst) = (0, 0.0f64);
    for _ in 0..400 {
        let before = laid(&mut rnd);
        let now = laid(&mut rnd);
        let center = (rnd(0.0, before.frame.size.0), rnd(0.0, before.frame.size.1));
        let want = source(&before, center);
        if !(0.0..=f64::from(sw)).contains(&want.0) || !(0.0..=f64::from(sh)).contains(&want.1) {
            continue;
        }
        let Some(got) = carried_center(&before, &now, center) else {
            continue;
        };
        let held =
            got.0 > 0.0 && got.1 > 0.0 && got.0 < now.frame.size.0 && got.1 < now.frame.size.1;
        if !held {
            continue;
        }
        let at = source(&now, got);
        worst = worst.max(((at.0 - want.0).powi(2) + (at.1 - want.1).powi(2)).sqrt());
        checked += 1;
    }
    assert!(checked > 50, "only {checked} cases the new frame holds");
    assert!(worst < 0.05, "{worst} px off the source pixel");
}

/// A point turned with the frame's own turn lands where the shader
/// reads the texture of the develop that turn lags behind: the middle
/// of each texel is the middle of the pixel drawn from it.
#[test]
fn a_turned_point_is_the_shaders_turn() {
    let size = (6u32, 4u32);
    for lag in 0..4u8 {
        let turned = crate::placeholder::turned_size(size, lag);
        for y in 0..i64::from(turned.1) {
            for x in 0..i64::from(turned.0) {
                let (tx, ty) = crate::placeholder::texel_of((x, y), turned, lag);
                let at = turned_point(
                    (tx as f32 + 0.5, ty as f32 + 0.5),
                    (size.0 as f32, size.1 as f32),
                    lag,
                );
                assert_eq!(at, (x as f32 + 0.5, y as f32 + 0.5), "lag {lag}");
            }
        }
    }
}
