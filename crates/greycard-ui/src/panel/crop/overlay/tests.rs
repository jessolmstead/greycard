use std::rc::Rc;

use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};

use super::{Overlay, PHI, QUARTERS, spiral, spiral_commands};
use crate::App;
use crate::testing::{count_id, press, state_for, window};

/// Every guide's element in the viewport, by its id, as the guide
/// it draws.
const ELEMENTS: [(Overlay, &str); 5] = [
    (Overlay::Thirds, "Viewport::thirds"),
    (Overlay::GoldenRatio, "Viewport::golden-ratio"),
    (Overlay::GoldenSpiral, "Viewport::golden-spiral"),
    (Overlay::Diagonals, "Viewport::diagonals"),
    (Overlay::Triangles, "Viewport::triangles"),
];

/// The crop's rectangle on the view, as the render step would set it:
/// left, top, width, height.
const CROP: [f32; 4] = [120.0, 90.0, 300.0, 200.0];

/// A frame open in crop mode, the crop's rectangle set by hand as
/// nothing here drives the render step that sets it.
fn cropping() -> (App, Rc<crate::Shared>, Rc<crate::worker::Worker>) {
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    press(&app, "r");
    assert!(app.get_crop_mode());
    app.set_crop_left(CROP[0]);
    app.set_crop_top(CROP[1]);
    app.set_crop_width(CROP[2]);
    app.set_crop_height(CROP[3]);
    (app, state, worker)
}

/// `key` pressed with Shift held.
fn shifted(app: &App, key: &str) {
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    press(app, key);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
}

/// A guide chosen from the CROP section's list, as its `selected`
/// chooses it.
fn choose(app: &App, overlay: Overlay) {
    app.set_crop_overlay(overlay.name().into());
    app.invoke_crop_overlay_chosen();
}

/// Which guides' elements are on the view.
fn drawn(app: &App) -> Vec<Overlay> {
    ELEMENTS
        .iter()
        .filter(|(_, id)| count_id(app, id) > 0)
        .map(|(o, _)| *o)
        .collect()
}

/// The rectangles of the element `id`'s lines, each as left, top,
/// width, height on the viewport, sorted.
fn lines(app: &App, id: &str) -> Vec<[f32; 4]> {
    count_id(app, id);
    let base = ElementHandle::find_by_element_type_name(app, "Viewport")
        .next()
        .expect("the viewport is on screen")
        .absolute_position();
    let guide = ElementHandle::find_by_element_id(app, id)
        .next()
        .expect("the guide is drawn");
    let mut out: Vec<[f32; 4]> = guide
        .query_descendants()
        .match_type_name("Rectangle")
        .find_all()
        .into_iter()
        .map(|e| {
            let (at, size) = (e.absolute_position(), e.size());
            [at.x - base.x, at.y - base.y, size.width, size.height]
        })
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

/// Lines across the crop at fractions `fx` and down it at `fy`, as
/// `lines` reads them.
fn grid_at(fx: [f32; 2], fy: [f32; 2]) -> Vec<[f32; 4]> {
    let [l, t, w, h] = CROP;
    let mut out = vec![
        [l + w * fx[0], t, 1.0, h],
        [l + w * fx[1], t, 1.0, h],
        [l, t + h * fy[0], w, 1.0],
        [l, t + h * fy[1], w, 1.0],
    ];
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

fn assert_lines(got: &[[f32; 4]], want: &[[f32; 4]]) {
    assert_eq!(got.len(), want.len(), "{got:?}");
    for (g, w) in got.iter().zip(want) {
        assert!(
            g.iter().zip(w).all(|(a, b)| (a - b).abs() < 0.01),
            "{got:?} against {want:?}"
        );
    }
}

/// The thirds are the guide on a fresh install, drawn as they always
/// were: four lines of one pixel at the thirds of the crop.
#[test]
fn the_thirds_are_the_default_and_drawn_as_before() {
    assert_eq!(
        crate::settings::Settings::default().crop_overlay,
        Overlay::Thirds.name()
    );
    let (app, _state, _worker) = cropping();
    assert_eq!(app.get_crop_overlay(), Overlay::Thirds.name());
    assert_eq!(drawn(&app), vec![Overlay::Thirds]);
    assert_lines(
        &lines(&app, "Viewport::thirds"),
        &grid_at([1.0 / 3.0, 2.0 / 3.0], [1.0 / 3.0, 2.0 / 3.0]),
    );
    // They follow the crop as a drag moves it.
    app.set_crop_left(CROP[0] + 30.0);
    let moved = lines(&app, "Viewport::thirds");
    let x = CROP[0] + 30.0 + CROP[2] / 3.0;
    assert!(
        moved.iter().any(|l| (l[0] - x).abs() < 0.01 && l[2] == 1.0),
        "{moved:?}"
    );
    // Out of crop mode, no guide at all.
    press(&app, "r");
    assert!(drawn(&app).is_empty());
}

/// Each guide chosen is drawn alone, and None draws nothing.
#[test]
fn each_guide_chosen_shows_alone() {
    let (app, _state, _worker) = cropping();
    for overlay in Overlay::ALL {
        choose(&app, overlay);
        let want: Vec<Overlay> = if overlay == Overlay::None {
            Vec::new()
        } else {
            vec![overlay]
        };
        assert_eq!(drawn(&app), want, "{overlay:?}");
    }
    choose(&app, Overlay::GoldenRatio);
    let minor = 1.0 / (PHI * PHI) as f32;
    let major = 1.0 / PHI as f32;
    assert_lines(
        &lines(&app, "Viewport::golden-ratio"),
        &grid_at([minor, major], [minor, major]),
    );
    // The spiral, the diagonals, and the triangles lie over the crop.
    for (overlay, id, paths) in [
        (Overlay::GoldenSpiral, "Viewport::golden-spiral", 1),
        (Overlay::Diagonals, "Viewport::diagonals", 2),
        (Overlay::Triangles, "Viewport::triangles", 3),
    ] {
        choose(&app, overlay);
        count_id(&app, id);
        let e = ElementHandle::find_by_element_id(&app, id).next().unwrap();
        let base = ElementHandle::find_by_element_type_name(&app, "Viewport")
            .next()
            .unwrap()
            .absolute_position();
        let (at, size) = (e.absolute_position(), e.size());
        assert_eq!(
            [at.x - base.x, at.y - base.y, size.width, size.height],
            CROP,
            "{overlay:?}"
        );
        let drawn_paths = if paths == 1 {
            1
        } else {
            e.query_descendants()
                .match_type_name("Path")
                .find_all()
                .len()
        };
        assert_eq!(drawn_paths, paths, "{overlay:?}");
    }
}

/// O steps through the guides in the list's order and from the last
/// back to the first; held, it steps once. Out of crop mode it is the
/// mask's key and leaves the guide.
#[test]
fn o_steps_through_the_guides_and_wraps() {
    let (app, _state, _worker) = cropping();
    for overlay in Overlay::ALL.iter().skip(1).chain([&Overlay::Thirds]) {
        press(&app, "o");
        assert_eq!(app.get_crop_overlay(), overlay.name());
    }
    // Held: the repeats do nothing more.
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: "o".into() });
    app.window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text: "o".into() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: "o".into() });
    assert_eq!(app.get_crop_overlay(), Overlay::GoldenRatio.name());
    press(&app, "r");
    assert!(!app.get_crop_mode());
    press(&app, "o");
    assert_eq!(app.get_crop_overlay(), Overlay::GoldenRatio.name());
}

/// Shift+O turns the spiral four ways and the triangles two, round
/// to the first, and leaves the guides that look the same every way.
#[test]
fn shift_o_turns_the_spiral_and_the_triangles_alone() {
    let (app, _state, _worker) = cropping();
    choose(&app, Overlay::GoldenSpiral);
    for want in [1, 2, 3, 0] {
        shifted(&app, "o");
        assert_eq!(app.get_crop_overlay_turn(), want);
        assert_eq!(app.get_crop_overlay(), Overlay::GoldenSpiral.name());
    }
    choose(&app, Overlay::Triangles);
    for want in [1, 0] {
        shifted(&app, "o");
        assert_eq!(app.get_crop_overlay_turn(), want);
    }
    shifted(&app, "o");
    for overlay in [
        Overlay::Thirds,
        Overlay::GoldenRatio,
        Overlay::Diagonals,
        Overlay::None,
    ] {
        choose(&app, overlay);
        shifted(&app, "o");
        assert_eq!(app.get_crop_overlay_turn(), 1, "{overlay:?}");
        assert_eq!(app.get_crop_overlay(), overlay.name());
    }
}

/// The guide and its turn are kept in the settings file as they
/// change, a run started on that file opens with them, and a close
/// keeps what the file says.
#[test]
fn the_guide_outlives_the_session() {
    let dir = crate::testing::scratch_dir("crop-overlay");
    let file = dir.join("settings.json");
    {
        let (app, state, _worker) = cropping();
        state.borrow_mut().window.settings_file = Some(file.clone());
        choose(&app, Overlay::Diagonals);
        let kept = crate::settings::Settings::load_from(&file);
        assert_eq!(kept.crop_overlay, Overlay::Diagonals.name());
        press(&app, "o");
        shifted(&app, "o");
        shifted(&app, "o");
        shifted(&app, "o");
    }
    let kept = crate::settings::Settings::load_from(&file);
    assert_eq!(
        (kept.crop_overlay.as_str(), kept.crop_overlay_turn),
        (Overlay::Triangles.name(), 1)
    );
    // The next run, as its launch reads the file.
    let app = window(1);
    super::show(&app, &kept);
    assert_eq!(app.get_crop_overlay(), Overlay::Triangles.name());
    assert_eq!(app.get_crop_overlay_turn(), 1);
    // A name the build does not know, or none, is the thirds.
    let mut odd = kept.clone();
    odd.crop_overlay = "Fibonacci".into();
    super::show(&app, &odd);
    assert_eq!(app.get_crop_overlay(), Overlay::Thirds.name());
    let read: crate::settings::Settings =
        serde_json::from_str(r#"{"crop_overlay": 7, "crop_overlay_turn": 2}"#).unwrap();
    assert_eq!(read.crop_overlay_turn, 2);
    assert_eq!(Overlay::from_name(&read.crop_overlay), Overlay::Thirds);
    crate::testing::remove_dir_retry(&dir);
}

/// A guide is a view's: choosing one, stepping, or turning it stores
/// nothing in the edit and takes no step of history.
#[test]
fn a_guide_is_no_part_of_the_edit() {
    let (app, state, _worker) = cropping();
    let before = state.borrow().develop.edit.clone();
    choose(&app, Overlay::GoldenSpiral);
    shifted(&app, "o");
    press(&app, "o");
    choose(&app, Overlay::None);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::SAVE_MS + 100,
    ));
    let st = state.borrow();
    assert_eq!(st.develop.edit, before);
    assert!(st.sidecars[0].history.is_empty(), "no step");
}

/// Each way of the spiral, wide or tall: it starts at a corner of the
/// crop and stays inside it, each arc starts where the last ended and
/// joins two opposite corners of its square, and each square's sides
/// are the last one's over the golden ratio.
#[test]
fn the_spiral_is_the_golden_rectangles() {
    let mut starts = Vec::new();
    for tall in [false, true] {
        for turn in 0..4 {
            let quarters = spiral(turn, tall);
            assert_eq!(quarters.len(), QUARTERS);
            let first = quarters[0].from;
            assert!(
                [0.0, 1.0].contains(&first.0) && [0.0, 1.0].contains(&first.1),
                "{turn} {tall}: starts at {first:?}"
            );
            starts.push(first);
            let inside = |(x, y): (f64, f64)| {
                (-1e-9..=1.0 + 1e-9).contains(&x) && (-1e-9..=1.0 + 1e-9).contains(&y)
            };
            for (k, q) in quarters.iter().enumerate() {
                let [l, t, r, b] = q.square;
                assert!(inside((l, t)) && inside((r, b)), "{turn} {tall} {k}");
                assert!(inside(q.from) && inside(q.to));
                let corners = [(l, t), (r, t), (r, b), (l, b)];
                let near = |p: (f64, f64), c: (f64, f64)| {
                    (p.0 - c.0).abs() < 1e-9 && (p.1 - c.1).abs() < 1e-9
                };
                let i = corners.iter().position(|c| near(q.from, *c)).unwrap();
                assert!(near(q.to, corners[(i + 2) % 4]), "{turn} {tall} {k}");
                if k > 0 {
                    let last = quarters[k - 1];
                    assert!(near(q.from, last.to), "{turn} {tall} {k}: joined");
                    let lw = last.square[2] - last.square[0];
                    let lh = last.square[3] - last.square[1];
                    assert!(((r - l) * PHI - lw).abs() < 1e-9, "{turn} {tall} {k}");
                    assert!(((b - t) * PHI - lh).abs() < 1e-9, "{turn} {tall} {k}");
                }
                // A square of the golden rectangle, stretched with it.
                let (w, h) = if tall { (1.0, PHI) } else { (PHI, 1.0) };
                assert!(((r - l) * w - (b - t) * h).abs() < 1e-9);
            }
            // The first square is the crop's short side across.
            let [l, t, r, b] = quarters[0].square;
            let short = if tall { b - t } else { r - l };
            assert!((short - 1.0 / PHI).abs() < 1e-9 && (r - l).max(b - t) == 1.0);
        }
    }
    // The four ways start at the four corners.
    for set in starts.chunks(4) {
        let mut corners = set.to_vec();
        corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
        corners.dedup();
        assert_eq!(corners.len(), 4, "{set:?}");
    }
    // The path draws them: one move, an arc a quarter, the sweep
    // reversed by a mirror.
    let path = spiral_commands(0, false);
    assert!(path.starts_with("M 0.000000 1.000000 A "), "{path}");
    assert_eq!(path.matches(" A ").count(), QUARTERS);
    assert!(path.contains(" 0 0 1 "));
    assert!(spiral_commands(1, false).contains(" 0 0 0 "));
    assert!(spiral_commands(2, false).contains(" 0 0 1 "));
    assert_ne!(spiral_commands(0, false), spiral_commands(0, true));
}
