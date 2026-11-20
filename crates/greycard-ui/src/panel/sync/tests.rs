use std::sync::Arc;

use super::*;

/// A preset, a paste, or a sync laying a DCP with the tone curve's
/// switch on keeps the switch only where the profile list says that
/// DCP carries a curve: not on one without, nor on one not listed.
#[test]
fn the_tone_curve_switch_is_laid_only_where_the_list_has_a_curve() {
    use greycard_edit::camera::{Camera, Entry, ProfileChoice};
    let entry = |name: &str, tone_curve| Entry {
        name: name.into(),
        path: PathBuf::from(format!("/nowhere/{name}.dcp")),
        title: None,
        unique_camera_model: None,
        copyright: None,
        same_as: None,
        tone_curve,
    };
    let profiles = [entry("Curved", true), entry("Flat", false)];
    let preset = |name: &str| {
        Preset::from_edit(
            "P",
            &Edit {
                camera: Camera {
                    profile: ProfileChoice::Named(name.into()),
                    look_table: true,
                    tone_curve: true,
                },
                ..Edit::default()
            },
            &[Section::Camera],
        )
    };
    let kept = |name: &str| {
        curve_settled(&preset(name), &profiles)
            .edit
            .camera
            .tone_curve
    };
    assert!(kept("Curved"));
    assert!(!kept("Flat"));
    assert!(!kept("Missing"));
}

/// A paste onto the frame on screen settles the switch as a paste
/// onto the rest does: a copy with it on, onto a frame already on
/// the same curveless DCP, is no change at all.
#[test]
fn a_paste_onto_the_open_frame_keeps_no_switch_its_profile_lacks() {
    use greycard_edit::camera::{Camera, Entry as Profile, ProfileChoice};
    let profiles = vec![Profile {
        name: "Flat".into(),
        path: PathBuf::from("/nowhere/Flat.dcp"),
        title: None,
        unique_camera_model: None,
        copyright: None,
        same_as: None,
        tone_curve: false,
    }];
    let on = |tone_curve| Edit {
        camera: Camera {
            profile: ProfileChoice::Named("Flat".into()),
            look_table: true,
            tone_curve,
        },
        ..Edit::default()
    };
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    {
        let mut st = state.borrow_mut();
        crate::panel::assets::hand_profiles(&mut st, profiles.clone());
        st.sidecars[0].record(on(true));
        st.sidecars[2].record(on(false));
    }
    app.invoke_select(2);
    assert_eq!(app.get_camera_profile(), "Flat");
    let mut st = state.borrow_mut();
    copy_settings_of(&mut st, &app, 0);
    assert!(
        st.sheets.clipboard.as_ref().unwrap().edit.camera.tone_curve,
        "the copy carries the switch"
    );
    let steps = st.line(2).unwrap().history.len();
    let synced = paste_onto(
        &mut st,
        &app,
        &worker,
        &[2],
        &Choice::whole(&[Section::Camera]),
        &profiles,
        |_, _| None,
    );
    assert!(synced.moved.is_empty());
    let line = st.line(2).unwrap();
    assert_eq!(line.history.len(), steps);
    assert!(!line.current.camera.tone_curve);
    assert!(!app.get_camera_tone_curve());
}
use crate::filter;
use crate::finish::MAX_LOCALS;
use crate::panel::browser::open_row;
use crate::testing::{click, folder, press, state_for, strip_cell, window};
use greycard_edit::mask::{Component, Mask, Shape};
use greycard_edit::preset::Entry;
use greycard_edit::{Learned, Sidecar, preset};
use slint::Model;
use slint::platform::{Key, WindowEvent};

/// The highlight fill goes with the Demosaic section: a sync of every
/// section the sheet ticks lays it, on, over each frame, a JPEG that
/// cannot use it among them, which keeps it; a paste without Demosaic
/// leaves each frame's own, on or off, while the rest of the look
/// lands; one with Demosaic lays the copy's, off as off.
#[test]
fn a_sync_and_a_paste_carry_the_highlight_fill_with_the_demosaic() {
    let with = |ev: f32, fill: Option<&str>| {
        let mut e = Edit::default();
        e.light.exposure = ev;
        e.highlight_fill = fill.map(str::to_string);
        e
    };
    let mut files = folder(3);
    files[2] = PathBuf::from("/nowhere/IMG_0002.JPG");
    let app = window(3);
    let (state, worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars[0].record(with(1.25, Some("hl-1")));
        st.sidecars[2].record(with(0.0, Some("hl-9")));
    }
    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0, 1, 2];
    app.invoke_sync_open_asked();
    app.invoke_sync_applied();
    {
        let st = state.borrow();
        assert_eq!(st.sidecars[0].current, with(1.25, Some("hl-1")));
        for f in [1, 2] {
            let current = &st.sidecars[f].current;
            assert_eq!(current.light.exposure, 1.25, "{f}");
            assert_eq!(current.highlight_fill.as_deref(), Some("hl-1"), "{f}");
        }
    }

    let mut st = state.borrow_mut();
    st.sidecars[1].record(with(-0.5, None));
    st.sidecars[2].record(with(0.0, Some("hl-9")));
    copy_settings_of(&mut st, &app, 0);
    let others: Vec<Section> = Section::ALL
        .iter()
        .copied()
        .filter(|s| *s != Section::Demosaic)
        .collect();
    paste_onto(
        &mut st,
        &app,
        &worker,
        &[1, 2],
        &Choice::whole(&others),
        &[],
        |_, _| None,
    );
    assert_eq!(st.line(1).unwrap().current.light.exposure, 1.25);
    assert_eq!(st.line(1).unwrap().current.highlight_fill, None);
    assert_eq!(
        st.line(2).unwrap().current.highlight_fill.as_deref(),
        Some("hl-9")
    );
    paste_onto(
        &mut st,
        &app,
        &worker,
        &[1, 2],
        &Choice::whole(&[Section::Demosaic]),
        &[],
        |_, _| None,
    );
    for f in [1, 2] {
        assert_eq!(
            st.line(f).unwrap().current.highlight_fill.as_deref(),
            Some("hl-1"),
            "{f}"
        );
    }
    st.sidecars[1].record(with(1.25, None));
    copy_settings_of(&mut st, &app, 1);
    paste_onto(
        &mut st,
        &app,
        &worker,
        &[2],
        &Choice::whole(&[Section::Demosaic]),
        &[],
        |_, _| None,
    );
    assert_eq!(st.line(2).unwrap().current.highlight_fill, None, "off");
}

/// The sheet's headings as its rows read: each section's title
/// and how its checks stand, 0 off, 1 some, 2 on.
fn headings(app: &App) -> Vec<(String, i32)> {
    let rows = app.get_sync_rows();
    (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .filter(|r| r.kind == 0)
        .map(|r| (r.label.to_string(), r.state))
        .collect()
}

/// The headings as a sync's sheet opens on them: all on but the
/// masks.
fn defaults() -> Vec<(String, i32)> {
    Section::ALL
        .iter()
        .map(|s| {
            (
                s.title().to_string(),
                if s.syncs_by_default() { 2 } else { 0 },
            )
        })
        .collect()
}

/// The sections `on` names checked whole and nothing else, as the
/// sheet's Uncheck all and a click on each heading do.
fn choose(app: &App, on: impl Fn(Section) -> bool) {
    app.invoke_sync_all(false);
    let rows = app.get_sync_rows();
    for i in 0..rows.row_count() {
        let r = rows.row_data(i).unwrap();
        if r.kind == 0 && Section::ALL.iter().any(|s| r.label == s.title() && on(*s)) {
            app.invoke_sync_row_toggled(i as i32);
        }
    }
}

fn with_modifier(app: &App, key: Key, f: impl FnOnce()) {
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    f();
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
}

#[test]
fn ctrl_click_grows_the_set_and_the_sync_sheet_opens_on_it() {
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(6));
    // A frame open, the set that frame alone.
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(app.get_set_count(), 1);
    // Nothing to sync onto yet: the key does nothing.
    press(&app, Key::Escape);
    with_modifier(&app, Key::Control, || {
        with_modifier(&app, Key::Shift, || press(&app, "S"));
    });
    assert!(!app.get_sync_open());

    // Ctrl+click two more: the set grows, the frame on screen
    // does not move, and the rows say which are chosen.
    with_modifier(&app, Key::Control, || {
        let (x, y) = strip_cell(&app, 3);
        click(&app, x, y);
        let (x, y) = strip_cell(&app, 4);
        click(&app, x, y);
    });
    assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(app.get_selected(), 1);
    assert_eq!(app.get_set_count(), 3);
    let rows = app.get_thumbs();
    let chosen: Vec<bool> = (0..6).map(|r| rows.row_data(r).unwrap().chosen).collect();
    assert_eq!(chosen, [false, true, false, true, true, false]);

    // Ctrl+Shift+S opens the sheet, everything on but the masks.
    with_modifier(&app, Key::Control, || {
        with_modifier(&app, Key::Shift, || press(&app, "S"));
    });
    assert!(app.get_sync_open());
    assert_eq!(
        app.get_sync_text(),
        "From IMG_0001.CR3 onto 2 other frames."
    );
    let heads = headings(&app);
    assert_eq!(heads.len(), Section::ALL.len());
    for ((name, on), s) in heads.iter().zip(Section::ALL) {
        assert_eq!(name, s.title());
        assert_eq!(*on, if *s == Section::Adjustments { 0 } else { 2 }, "{s:?}");
    }
    // Escape leaves the sheet, then a second one the set.
    press(&app, Key::Escape);
    assert!(!app.get_sync_open());
    assert_eq!(app.get_set_count(), 3);
    press(&app, Key::Escape);
    assert_eq!(app.get_set_count(), 1);
    assert_eq!(chosen_frames(&state.borrow()), vec![1]);
}

#[test]
fn shift_click_and_shift_arrows_extend_and_a_plain_arrow_collapses() {
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(6));
    app.invoke_select(1);
    // Shift+click four rows along: the run, the anchor kept.
    with_modifier(&app, Key::Shift, || {
        let (x, y) = strip_cell(&app, 4);
        click(&app, x, y);
    });
    assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3, 4]);
    assert_eq!(state.borrow().current, Some(1));
    // A plain click on the frame on screen: back to it alone.
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    assert_eq!(chosen_frames(&state.borrow()), vec![1]);
    // Shift and the arrow: the current frame moves and the set
    // keeps what it had.
    with_modifier(&app, Key::Shift, || {
        press(&app, Key::RightArrow);
        press(&app, Key::RightArrow);
    });
    assert_eq!(state.borrow().current, Some(3));
    assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3]);
    // The meta keys reach the whole set.
    press(&app, "4");
    let st = state.borrow();
    let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
    assert_eq!(ratings, [0, 4, 4, 4, 0, 0]);
    drop(st);
    // A plain arrow collapses to the frame it lands on.
    press(&app, Key::RightArrow);
    assert_eq!(state.borrow().current, Some(4));
    assert_eq!(chosen_frames(&state.borrow()), vec![4]);
    assert_eq!(app.get_set_count(), 1);
}

#[test]
fn a_sync_onto_unindexed_frames_reads_their_mosaics_off_the_window() {
    let dir = std::env::temp_dir().join(format!("greycard-sync-mosaics-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let files: Vec<PathBuf> = (0..4)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    for f in &files {
        std::fs::write(f, b"not a raw").expect("a file");
    }
    let app = window(4);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.placement = greycard_edit::Placement::Folder;
    }
    crate::rows::HOLD_MOSAICS.with(|h| h.set(true));
    app.invoke_select(0);
    app.set_exposure(1.25);
    state.borrow_mut().views.picked = vec![0, 1, 2, 3];
    app.invoke_sync_open_asked();
    app.invoke_sync_applied();
    // The press returns with the reads out and nothing laid yet.
    assert_eq!(app.get_status(), "reading 4 frames...");
    assert!(
        state.borrow().sidecars[1..]
            .iter()
            .all(|s| s.history.is_empty())
    );
    assert_eq!(state.borrow().views.loads.whats(), ["sync"]);
    // In, the sync runs on what they said.
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    assert_eq!(app.get_status(), "synced 19 sections onto 3 frames");
    {
        let st = state.borrow();
        for s in &st.sidecars[1..] {
            assert_eq!(s.current.light.exposure, 1.25);
        }
        assert!(st.views.loads.queue.is_empty());
    }
    for f in &files[1..] {
        let back = Sidecar::load(f).unwrap().expect("the sidecar written");
        assert_eq!(back.current.light.exposure, 1.25);
    }
    // Read once a session: the next sync runs at once.
    app.set_exposure(-0.5);
    app.invoke_sync_open_asked();
    app.invoke_sync_applied();
    assert_eq!(app.get_status(), "synced 19 sections onto 3 frames");
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 0);
    assert_eq!(state.borrow().sidecars[3].current.light.exposure, -0.5);
    crate::rows::HOLD_MOSAICS.with(|h| h.set(false));
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn a_sidecar_read_landing_under_a_mosaics_read_leaves_no_bar_behind() {
    let dir = std::env::temp_dir().join(format!("greycard-sync-bar-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let files: Vec<PathBuf> = (0..3)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    for f in &files {
        std::fs::write(f, b"not a raw").expect("a file");
    }
    let app = window(3);
    let (state, worker) = state_for(&app, files.clone());
    crate::rows::HOLD_MOSAICS.with(|h| h.set(true));
    app.invoke_select(0);
    app.set_exposure(1.25);
    state.borrow_mut().views.picked = vec![0, 1];
    // A frame standing in from its row, for a request behind the
    // sync that reads its sidecar.
    state.borrow_mut().from_row[2].read = false;
    app.invoke_sync_open_asked();
    app.invoke_sync_applied();
    let ran = Rc::new(std::cell::Cell::new(false));
    let r = ran.clone();
    let key = state.borrow().key_of(2).unwrap();
    crate::rows::request(
        &state,
        &app,
        &worker,
        vec![key],
        crate::rows::Scope::File,
        crate::rows::Request::RatingKey,
        Box::new(move |_, _, _, _| r.set(true)),
    );
    assert_eq!(state.borrow().views.loads.whats(), ["sync", "rating key"]);
    // The sidecar read lands first, the request waiting its turn;
    // then the mosaics, and both run, the bar gone with them.
    let mosaics = crate::rows::read_held_mosaics();
    crate::rows::land_pending(&state, &app, &worker);
    assert!(!ran.get());
    crate::rows::land_mosaics(&state, &app, &worker, mosaics);
    assert!(ran.get());
    assert_eq!(app.get_status(), "synced 19 sections onto 1 frame");
    assert!(state.borrow().views.loads.since.is_none());
    let later = std::time::Instant::now() + std::time::Duration::from_secs(90);
    crate::rows::bar_tick(&state, &app, &worker, later);
    assert!(!app.get_loading_shown());
    assert_eq!(app.get_status(), "synced 19 sections onto 1 frame");
    crate::rows::HOLD_MOSAICS.with(|h| h.set(false));
    crate::testing::remove_dir_retry(&dir);
}

/// The window's longest stall over a sync from the first frame in
/// `GREYCARD_SYNC_FRAMES` onto the rest, unindexed: the press, and
/// the run once the mosaics are in, with the read in place (as
/// before) and held off the window.
#[test]
#[ignore]
fn measure_a_sync_onto_unindexed_frames() {
    let dir = PathBuf::from(std::env::var("GREYCARD_SYNC_FRAMES").expect("a folder"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "CR3"))
        .collect();
    files.sort();
    for hold in [false, true, false, true] {
        let app = window(files.len());
        let (state, worker) = state_for(&app, files.clone());
        crate::rows::HOLD_MOSAICS.with(|h| h.set(hold));
        app.invoke_select(0);
        app.set_exposure(0.5);
        state.borrow_mut().views.picked = (0..files.len()).collect();
        app.invoke_sync_open_asked();
        let t = std::time::Instant::now();
        app.invoke_sync_applied();
        let press = t.elapsed();
        let t = std::time::Instant::now();
        let read = crate::rows::read_held_mosaics();
        let off: usize = read.iter().map(|(_, r)| r.len()).sum();
        let read_took = t.elapsed();
        let t = std::time::Instant::now();
        crate::rows::land_mosaics(&state, &app, &worker, read);
        let run = t.elapsed();
        eprintln!(
            "hold {hold}: press {:.1} ms, {off} mosaics off the window {:.1} ms, run {:.1} ms; {}",
            press.as_secs_f64() * 1e3,
            read_took.as_secs_f64() * 1e3,
            run.as_secs_f64() * 1e3,
            app.get_status()
        );
    }
    crate::rows::HOLD_MOSAICS.with(|h| h.set(false));
}

#[test]
fn apply_lays_the_current_frame_over_the_others_and_saves_them() {
    let dir = std::env::temp_dir().join(format!("greycard-sync-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let files: Vec<PathBuf> = (0..3)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    let app = window(3);
    let (state, _worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.placement = greycard_edit::Placement::Folder;
        // The frame the sync goes onto has a look of its own and
        // a crop.
        let mut own = Edit::default();
        own.sharpen.radius = 2.0;
        own.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.8,
            h: 0.8,
        });
        st.sidecars[2].record(own);
    }
    app.invoke_select(0);
    // The panel is the frame's: an exposure moved on it.
    app.set_exposure(1.25);
    {
        let mut st = state.borrow_mut();
        st.views.picked = vec![0, 2];
    }
    app.invoke_sync_open_asked();
    assert!(app.get_sync_open());
    app.invoke_sync_applied();
    assert!(!app.get_sync_open());
    assert_eq!(app.get_status(), "synced 19 sections onto 1 frame");

    let st = state.borrow();
    // The frame synced from kept what it had and recorded the
    // panel's state; the frame outside the set is untouched.
    assert_eq!(st.sidecars[0].current.light.exposure, 1.25);
    assert_eq!(st.sidecars[1], Sidecar::default());
    // The frame synced onto has the exposure, keeps its crop,
    // and has one step more.
    let s = &st.sidecars[2];
    assert_eq!(s.current.light.exposure, 1.25);
    assert_eq!(s.current.sharpen, Edit::default().sharpen);
    assert!(s.current.geometry.crop.is_some());
    assert_eq!(s.history.len(), 2);
    // Its step is named for the frame it came from; the frame
    // synced from recorded its panel as a plain step.
    assert_eq!(s.current_label.as_deref(), Some("Sync from IMG_0000.CR3"));
    assert_eq!(st.sidecars[0].current_label, None);
    // Written where the setting says, and it reads back the same.
    let on_disk = Sidecar::path_in(&files[2], greycard_edit::Placement::Folder);
    assert!(on_disk.exists(), "{}", on_disk.display());
    let back = Sidecar::load(&files[2]).unwrap().unwrap();
    assert_eq!(back.current, s.current);
    assert_eq!(back.current_label, s.current_label);
    assert!(!Sidecar::path_in(&files[1], greycard_edit::Placement::Folder).exists());
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

fn entry(name: &str, made_for: Option<&str>) -> camera::Entry {
    camera::Entry {
        name: name.into(),
        path: PathBuf::new(),
        title: None,
        unique_camera_model: made_for.map(str::to_string),
        copyright: None,
        same_as: None,
        tone_curve: false,
    }
}

fn body(make: &str, model: &str, iso: u32) -> greycard_core::decode::Probe {
    greycard_core::decode::Probe {
        make: make.into(),
        model: model.into(),
        other_names: Vec::new(),
        iso: Some(iso),
        exposure_time: None,
        fnumber: None,
        ..Default::default()
    }
}

#[test]
fn a_profile_fits_only_the_body_it_was_made_for() {
    let r5 = entry("r5", Some("Canon EOS R5"));
    let fits = |e: Option<&camera::Entry>, body: Option<(&str, &str)>| {
        profile_fits(
            e,
            body.map(|(make, model)| camera::Body::new(make, model, &[])),
        )
    };
    assert!(fits(Some(&r5), Some(("Canon", "EOS R5"))));
    assert!(!fits(Some(&r5), Some(("Fujifilm", "X-T5"))));
    // A frame that cannot say what took it does not get it.
    assert!(!fits(Some(&r5), None));
    // A profile that names no camera, or one the directory has
    // not got, cannot be checked, and goes as a preset's would.
    assert!(fits(Some(&entry("any", None)), None));
    assert!(fits(Some(&entry("any", Some(" "))), None));
    assert!(fits(None, Some(("Fujifilm", "X-T5"))));
}

/// A preset's profile made under a body's other name goes to the
/// body's frames: the probe carries the names the decoder gives.
#[test]
fn a_preset_s_profile_fits_a_body_by_its_other_name() {
    let mut edit = Edit::default();
    edit.camera.profile = ProfileChoice::Named("t8i".into());
    let preset = Preset::from_edit("Rebel", &edit, &[Section::Camera]);
    let profiles = [entry("t8i", Some("Canon EOS Rebel T8i"))];
    let mut probe = body("Canon", "Canon EOS 850D", 100);
    probe.other_names =
        greycard_core::decode::rawler_backend::other_names("Canon", "Canon EOS 850D");
    let (_, left_off) = preset_fitted(&preset, &profiles, || Some(probe.clone()));
    assert!(!left_off);
    probe.other_names.clear();
    let (_, left_off) = preset_fitted(&preset, &profiles, || Some(probe));
    assert!(left_off);
}

/// A body whose EXIF make is not its clean make: the open frame's
/// names (clean) and a probe's (as the EXIF spells them) fit the
/// same profile under the body's other name.
#[test]
fn a_body_fits_by_its_other_name_whichever_make_names_it() {
    use greycard_core::decode::rawler_backend::other_names;
    let d800e = entry("d800e", Some("Nikon D800E"));
    for (make, model) in [("Nikon", "D800"), ("NIKON CORPORATION", "NIKON D800")] {
        let mut probe = body(make, model, 100);
        probe.other_names = other_names(make, model);
        assert!(
            profile_fits(Some(&d800e), Some(probed_body(&probe))),
            "{make} {model}"
        );
        assert!(d800e.fits(probed_body(&probe)), "{make} {model}");
    }
}

/// A Canon DCP synced over a Canon frame and a Fuji frame lands
/// on the Canon alone, and the Fuji takes everything else; a
/// frame still waiting on its ISO's learned blend is given it
/// before the sync makes its edit no longer the default.
#[test]
fn a_profile_skips_another_body_and_a_waiting_blend_is_seeded() {
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    {
        let mut st = state.borrow_mut();
        st.sidecars[0].current.camera.profile = ProfileChoice::Named("r5".into());
        st.sidecars[0].current.light.exposure = 0.5;
        // Frames 1 and 2 are raws never developed: each waits for
        // its ISO's blend, as its own line says.
        assert!(!crate::rows::wants_seed(&st, 0));
        assert!(crate::rows::wants_seed(&st, 1));
        assert!(crate::rows::wants_seed(&st, 2));
    }
    open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    state.borrow_mut().views.picked = vec![0, 1, 2];
    let profiles = [entry("r5", Some("Canon EOS R5"))];
    let bodies = |_: &State, f: usize| match f {
        1 => Some(body("Canon", "EOS R5", 3200)),
        2 => Some(body("Fujifilm", "X-T5", 100)),
        _ => None,
    };
    let sections = [Section::Camera, Section::Light];
    let synced = sync_selection(&mut state.borrow_mut(), &app, &sections, &profiles, bodies);
    assert_eq!(synced.moved, [1, 2]);
    assert_eq!(synced.profile_left_off, [2]);
    let st = state.borrow();
    assert_eq!(st.sidecars[1].current.camera.profile.name(), "r5");
    assert!(st.sidecars[2].current.camera.profile.is_embedded());
    assert_eq!(st.sidecars[1].current.light.exposure, 0.5);
    assert_eq!(st.sidecars[2].current.light.exposure, 0.5);
    // Noise was not synced, so each frame's own ISO seeded its
    // blend, under the sync's step, and is not seeded again.
    let blend = |iso| greycard_edit::Noise::blend_for_iso(Some(iso));
    assert_eq!(
        st.sidecars[1].history[0].edit.noise.learned_strength,
        blend(3200)
    );
    assert_eq!(st.sidecars[2].current.noise.learned_strength, blend(100));
    assert!((0..3).all(|c| !crate::rows::wants_seed(&st, c)));
    assert!(
        synced_words(
            &st,
            &Choice::whole(&[Section::Light, Section::Camera]),
            2,
            &synced
        )
        .ends_with("; the camera profile left off IMG_0002.CR3 (made for another camera)")
    );
    drop(st);

    // With Noise synced the blend is the source's, and nothing
    // is seeded over it on a first open: the lines are at their
    // defaults again, waiting.
    {
        let mut st = state.borrow_mut();
        for f in [1, 2] {
            st.sidecars[f] = Sidecar::default();
        }
        assert!(crate::rows::wants_seed(&st, 1) && crate::rows::wants_seed(&st, 2));
    }
    app.set_denoise_learned_strength(0.6);
    let synced = sync_selection(
        &mut state.borrow_mut(),
        &app,
        &[Section::Noise],
        &profiles,
        |_, _| panic!("nothing to read a frame for"),
    );
    assert_eq!(synced.moved, [1, 2]);
    let st = state.borrow();
    assert!((0..3).all(|c| !crate::rows::wants_seed(&st, c)));
    assert_eq!(
        st.sidecars[2].current.noise.learned_strength,
        st.sidecars[0].current.noise.learned_strength
    );
}

/// A sync of the Light section, or of its exposure alone, from an
/// older edit that keeps the zero it was made against takes that
/// zero with it, so the target lands where the source sits; from a
/// frame read off its sidecar as from the one on screen. A new edit
/// has none to give, and the target's exposure counts from its own
/// body's default. A shift moves the target's own exposure, so the
/// target keeps its own zero.
#[test]
fn a_sync_carries_the_exposure_s_zero_with_the_exposure() {
    use greycard_edit::preset::{Choice, Part};
    let older = Some(greycard_edit::baseline::BEFORE_BODIES);
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    {
        let mut st = state.borrow_mut();
        st.sidecars[0].current.light.exposure = 0.3;
        st.sidecars[0].current.light.baseline = older;
    }
    let none = |_: &State, _: usize| None;
    let exposure = Choice {
        sections: Vec::new(),
        parts: vec![Part::LightExposure],
        masks: None,
        masks_land: Default::default(),
    };
    for choice in [Choice::whole(&[Section::Light]), exposure] {
        {
            let mut st = state.borrow_mut();
            for f in [1, 2] {
                st.sidecars[f] = Sidecar::default();
            }
        }
        let (synced, _) = sync_onto(
            &mut state.borrow_mut(),
            &app,
            0,
            &[1],
            &choice,
            Default::default(),
            &[],
            none,
        );
        assert_eq!(synced.moved, [1]);
        let st = state.borrow();
        assert_eq!(st.sidecars[1].current.light.exposure, 0.3);
        assert_eq!(st.sidecars[1].current.light.baseline, older, "{choice:?}");
    }
    // From the frame on screen, through the panel.
    open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    state.borrow_mut().sidecars[2] = Sidecar::default();
    let choice = Choice::whole(&[Section::Light]);
    sync_onto(
        &mut state.borrow_mut(),
        &app,
        0,
        &[2],
        &choice,
        Default::default(),
        &[],
        none,
    );
    assert_eq!(state.borrow().sidecars[2].current.light.baseline, older);
    // A new edit carries none, over an older one's.
    {
        let mut st = state.borrow_mut();
        st.sidecars[1].current.light.baseline = older;
        st.sidecars[2].current.light.baseline = None;
        st.sidecars[2].current.light.exposure = 0.5;
    }
    sync_onto(
        &mut state.borrow_mut(),
        &app,
        2,
        &[1],
        &choice,
        Default::default(),
        &[],
        none,
    );
    {
        let st = state.borrow();
        assert_eq!(st.sidecars[1].current.light.exposure, 0.5);
        assert_eq!(st.sidecars[1].current.light.baseline, None);
    }
    // Shifted: the older source's +0.3 moves a new frame's own +0.2
    // to +0.5 from that frame's own zero, not the source's.
    {
        let mut st = state.borrow_mut();
        st.sidecars[1] = Sidecar::default();
        st.sidecars[1].current.light.exposure = 0.2;
    }
    let shift = SyncShift {
        exposure: true,
        ..Default::default()
    };
    let (synced, moved) = sync_onto(
        &mut state.borrow_mut(),
        &app,
        0,
        &[1],
        &choice,
        shift,
        &[],
        none,
    );
    assert_eq!(synced.moved, [1]);
    assert_eq!(moved.exposure, Some(0.3));
    let st = state.borrow();
    assert!((st.sidecars[1].current.light.exposure - 0.5).abs() < 1e-6);
    assert_eq!(st.sidecars[1].current.light.baseline, None);
}

/// The demosaic goes as the source frame runs it, and only to the
/// frames of a mosaic that takes it: an X-Trans frame's stored
/// Bayer choice, never picked there, travels as Markesteijn's three
/// passes and is left off a Bayer frame; a Bayer frame's RCD is left
/// off an X-Trans one.
#[test]
fn a_demosaic_syncs_as_it_runs_and_only_to_its_own_mosaic() {
    use greycard_core::raw::Mosaic;
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    state.borrow_mut().sidecars[0].current.demosaic = greycard_edit::Demosaic::Amaze;
    open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    state.borrow_mut().views.picked = vec![0, 1, 2];
    // What the worker has said of each file's mosaic.
    {
        use crate::panel::browser::OpenMosaic;
        let mut st = state.borrow_mut();
        for (f, m) in [(0, Mosaic::XTrans), (1, Mosaic::XTrans), (2, Mosaic::Bayer)] {
            let path = st.path_of(f).clone();
            st.develop.mosaics.insert(path, OpenMosaic::Known(m));
        }
    }
    // The bodies are not asked about: no profile goes, and a
    // frame waiting for its ISO's blend is left waiting.
    let mosaics = |_: &State, _: usize| -> Option<greycard_core::decode::Probe> { None };
    let synced = sync_selection(
        &mut state.borrow_mut(),
        &app,
        &[Section::Demosaic],
        &[],
        mosaics,
    );
    assert_eq!(synced.demosaic_left_off, [2]);
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[1].current.demosaic,
            greycard_edit::Demosaic::Markesteijn3
        );
        assert_eq!(st.sidecars[2].current.demosaic, Edit::default().demosaic);
        assert!(
            profile_left_off_words(&st, &synced)
                .ends_with("; the demosaic left off IMG_0002.CR3 (a method of another mosaic)")
        );
    }

    // From a Bayer frame: its RCD to the Bayer frame alone.
    state.borrow_mut().sidecars[2].current.demosaic = greycard_edit::Demosaic::Rcd;
    state.borrow_mut().sidecars[1] = Sidecar::default();
    open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    state.borrow_mut().views.picked = vec![2, 1, 0];
    let synced = sync_selection(
        &mut state.borrow_mut(),
        &app,
        &[Section::Demosaic],
        &[],
        mosaics,
    );
    assert_eq!(synced.demosaic_left_off, [0, 1]);
    let st = state.borrow();
    assert_eq!(st.sidecars[1].current.demosaic, Edit::default().demosaic);
    assert_eq!(
        st.sidecars[0].current.demosaic,
        greycard_edit::Demosaic::Amaze
    );
    // And the rule alone.
    let preset = Preset::from_edit(
        "",
        &Edit {
            demosaic: greycard_edit::Demosaic::Markesteijn1,
            ..Edit::default()
        },
        &[Section::Demosaic],
    );
    assert!(demosaic_fits(&preset, Some(Mosaic::XTrans)));
    assert!(!demosaic_fits(&preset, Some(Mosaic::Bayer)));
    assert!(demosaic_fits(&preset, None));
}

/// The library's index says each file's mosaic, so a sync over
/// frames never opened reads none of them: the files are gone from
/// the disk here, and the demosaic still goes only where it fits.
#[test]
fn a_sync_takes_the_mosaics_from_the_library_and_reads_no_file() {
    use greycard_core::raw::Mosaic;
    use greycard_library::fixture::write_mosaic_dng;
    const XTRANS: &str = "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG";
    let dir = crate::testing::scratch_dir("sync-mosaics");
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<PathBuf> = (0..3)
        .map(|i| dir.join(format!("IMG_{i:04}.dng")))
        .collect();
    write_mosaic_dng(&files[0], XTRANS);
    write_mosaic_dng(&files[1], XTRANS);
    write_mosaic_dng(&files[2], "RGGB");
    let db = dir.join("library.sqlite");
    greycard_library::Library::open(&db)
        .unwrap()
        .index_folder(&dir, &mut |_| {})
        .unwrap();
    for f in &files {
        std::fs::remove_file(f).unwrap();
    }
    let app = window(3);
    let (state, worker) = state_for(&app, files.clone());
    state.borrow_mut().views.index_reader =
        Some(greycard_library::Library::open_read_only(&db).unwrap());
    {
        let mut st = state.borrow_mut();
        assert_eq!(
            crate::panel::browser::mosaic_of(&mut st, 1),
            Some(Mosaic::XTrans)
        );
        assert_eq!(
            crate::panel::browser::mosaic_of(&mut st, 2),
            Some(Mosaic::Bayer)
        );
        // From the index, not a read: nothing was kept as read.
        assert!(st.develop.mosaics.is_empty() && st.develop.stances.is_empty());
        st.sidecars[0].current.demosaic = greycard_edit::Demosaic::Amaze;
    }
    open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    state.borrow_mut().views.picked = vec![0, 1, 2];
    let synced = sync_selection(
        &mut state.borrow_mut(),
        &app,
        &[Section::Demosaic],
        &[],
        |_: &State, _: usize| None,
    );
    assert_eq!(synced.demosaic_left_off, [2]);
    let st = state.borrow();
    assert_eq!(
        st.sidecars[1].current.demosaic,
        greycard_edit::Demosaic::Markesteijn3
    );
    assert!(
        st.develop.mosaics.is_empty() && st.develop.stances.is_empty(),
        "no file was read"
    );
    drop(st);
    crate::testing::remove_scratch(state, &dir);
}

/// The sheet reads the frame behind it, so nothing under it may
/// move that frame; and an Apply on a selection that moved anyway
/// does nothing rather than sync what the sheet did not name.
#[test]
fn nothing_moves_the_selection_under_the_sheet_and_a_moved_one_is_refused() {
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(6));
    let undone = Rc::new(std::cell::Cell::new(0));
    {
        let undone = undone.clone();
        app.on_undo(move || undone.set(undone.get() + 1));
    }
    app.invoke_select(1);
    with_modifier(&app, Key::Control, || {
        for i in [3, 4] {
            let (x, y) = strip_cell(&app, i);
            click(&app, x, y);
        }
    });
    app.invoke_sync_open_asked();
    assert!(app.get_sync_open());
    // The arrows, the zoom and proof keys, undo: none reach the
    // frame behind.
    press(&app, Key::RightArrow);
    with_modifier(&app, Key::Shift, || press(&app, Key::RightArrow));
    with_modifier(&app, Key::Control, || press(&app, "z"));
    press(&app, "s");
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
    assert_eq!(undone.get(), 0);
    assert!(!app.get_proofing());
    // The set moves anyway (a click that got through): refused.
    state.borrow_mut().views.picked = vec![1, 3];
    app.invoke_sync_applied();
    assert!(!app.get_sync_open());
    assert_eq!(
        app.get_status(),
        "the selection changed while the sheet was open; nothing synced"
    );
    assert!(state.borrow().sidecars[3].history.is_empty());
    // And no sheet over a tool in hand.
    app.set_level_mode(true);
    with_modifier(&app, Key::Control, || {
        with_modifier(&app, Key::Shift, || press(&app, "S"));
    });
    app.invoke_sync_open_asked();
    assert!(!app.get_sync_open());
    // Escape puts the sheet away before the tool.
    app.set_level_mode(false);
    app.invoke_sync_open_asked();
    app.set_level_mode(true);
    press(&app, Key::Escape);
    assert!(!app.get_sync_open());
    assert!(
        app.get_level_mode(),
        "the tool behind the sheet stays in hand"
    );
}

/// A frame the filter has just hidden is out of the set, current
/// or not: a key does not reach it.
#[test]
fn a_hidden_current_frame_takes_no_key() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    state.borrow_mut().views.picked = vec![1, 2];
    {
        let mut st = state.borrow_mut();
        st.sidecars[1].meta.flag = greycard_edit::meta::Flag::Reject;
        st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
        st.views.shown = vec![0, 2, 3];
    }
    let st = state.borrow();
    assert_eq!(chosen_frames(&st), vec![2]);
    assert_eq!(st.current, Some(1));
}

/// A preset click with three frames selected lays it over each,
/// through `lay_over_targets` the same way a sync does: one step
/// per frame that did not have it, a frame that already matches
/// left alone, and the frame outside the set untouched.
#[test]
fn a_preset_click_with_three_selected_lays_it_over_each_and_leaves_the_rest() {
    let dir = std::env::temp_dir().join(format!("greycard-preset-set-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let files: Vec<PathBuf> = (0..4)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    let app = window(4);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, files.clone());
    let mut edit = Edit::default();
    edit.light.exposure = 0.5;
    let preset = Preset::from_edit("Portra 400", &edit, &[Section::Light]);
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.placement = greycard_edit::Placement::Folder;
        st.develop.presets = vec![Entry {
            path: PathBuf::new(),
            preset: preset.clone(),
        }];
        // Frame 2 already has the preset's Light on it: it is left
        // out of what moves.
        st.sidecars[2].record(preset.applied(&Edit::default()));
    }
    app.invoke_select(0);
    with_modifier(&app, Key::Control, || {
        let (x, y) = strip_cell(&app, 1);
        click(&app, x, y);
        let (x, y) = strip_cell(&app, 2);
        click(&app, x, y);
    });
    assert_eq!(chosen_frames(&state.borrow()), vec![0, 1, 2]);

    app.invoke_preset_applied(0);

    let st = state.borrow();
    // The current frame took it, through the panel, as it always
    // has.
    assert_eq!(st.sidecars[0].current.light.exposure, 0.5);
    assert_eq!(st.sidecars[0].history.len(), 1);
    // The other selected frame took it, through the sidecar, as a
    // sync's target does.
    assert_eq!(st.sidecars[1].current.light.exposure, 0.5);
    assert_eq!(st.sidecars[1].history.len(), 1);
    // Already had it: no new step.
    assert_eq!(st.sidecars[2].history.len(), 1);
    // Every frame that took it, the one on screen too, has the
    // step under the set's words; the one that had it, none.
    for f in [0, 1] {
        assert_eq!(
            st.sidecars[f].current_label.as_deref(),
            Some("Preset ×3: Portra 400"),
            "frame {f}"
        );
    }
    assert_eq!(st.sidecars[2].current_label, None);
    assert_eq!(
        app.get_history_rows()
            .row_data(0)
            .map(|r| r.name.to_string())
            .as_deref(),
        Some("Preset ×3: Portra 400")
    );
    // Outside the set: untouched.
    assert_eq!(st.sidecars[3], Sidecar::default());
    assert_eq!(
        app.get_status(),
        "Portra 400 onto 2 frames; 1 had it already"
    );

    // Written where the setting says.
    let on_disk = Sidecar::path_in(&files[1], greycard_edit::Placement::Folder);
    assert!(on_disk.exists(), "{}", on_disk.display());
    let back = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(back.current, st.sidecars[1].current);
    assert_eq!(back.current_label, st.sidecars[1].current_label);
    assert!(!Sidecar::path_in(&files[3], greycard_edit::Placement::Folder).exists());
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// With one frame selected, a preset click is unchanged: the
/// current frame alone, "applied" or "is on already", exactly as
/// before the set existed.
#[test]
fn a_preset_click_with_one_frame_selected_is_unchanged() {
    let dir = std::env::temp_dir().join(format!("greycard-preset-one-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let files: Vec<PathBuf> = (0..2)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    let app = window(2);
    let (state, _worker) = state_for(&app, files.clone());
    let mut edit = Edit::default();
    edit.light.exposure = 0.5;
    let preset = Preset::from_edit("Portra 400", &edit, &[Section::Light]);
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.placement = greycard_edit::Placement::Folder;
        st.develop.presets = vec![Entry {
            path: PathBuf::new(),
            preset,
        }];
    }
    app.invoke_select(0);
    assert_eq!(chosen_frames(&state.borrow()), vec![0]);

    app.invoke_preset_applied(0);
    assert_eq!(state.borrow().sidecars[0].current.light.exposure, 0.5);
    assert_eq!(state.borrow().sidecars[0].history.len(), 1);
    assert_eq!(app.get_status(), "Portra 400 applied");
    assert_eq!(
        state.borrow().sidecars[0].current_label.as_deref(),
        Some("Preset: Portra 400")
    );
    assert_eq!(state.borrow().sidecars[1], Sidecar::default());

    // Applied again: already on, and nothing else moves.
    app.invoke_preset_applied(0);
    assert_eq!(app.get_status(), "Portra 400 is on already");
    assert_eq!(state.borrow().sidecars[0].history.len(), 1);

    crate::testing::remove_dir_retry(&dir);
}

/// A store preset carrying Noise never brings the learned tier
/// the way a sync's does: `lay_over_targets` runs with
/// `learned_from: None` for a preset click, so a frame still
/// waiting on its first open takes its own ISO's blend, not the
/// preset's.
#[test]
fn a_preset_carrying_noise_leaves_the_learned_tier_to_each_frames_iso() {
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    let mut edit = Edit::default();
    edit.noise.profiled = true;
    edit.noise.strength = 1.8;
    // Whatever tier the edit it was saved from had, a preset
    // never carries it.
    edit.noise.learned = Learned::Best;
    edit.noise.learned_strength = 0.9;
    let preset = Preset::from_edit("Clean", &edit, &[Section::Noise]);
    assert_eq!(
        preset.edit.noise.learned,
        Learned::Off,
        "a preset leaves it"
    );

    // Frame 1 at its default again waits for its ISO; frame 0,
    // edited, has a blend of its own that no seed touches.
    {
        let mut st = state.borrow_mut();
        st.sidecars[1] = Sidecar::default();
        st.sidecars[0].current.noise.learned_strength = 0.123;
        assert!(!crate::rows::wants_seed(&st, 0));
        assert!(crate::rows::wants_seed(&st, 1));
    }
    let bodies = |_: &State, f: usize| match f {
        0 => Some(body("Canon", "EOS R5", 100)),
        1 => Some(body("Canon", "EOS R5", 3200)),
        _ => None,
    };
    let synced = lay_over_targets(
        &mut state.borrow_mut(),
        &app,
        &preset,
        &[0, 1],
        None,
        Some("Preset ×2: Film"),
        &[],
        bodies,
    );
    assert_eq!(synced.moved, [0, 1]);
    let st = state.borrow();
    let blend = |iso| greycard_edit::Noise::blend_for_iso(Some(iso));
    for f in [0, 1] {
        assert!(st.sidecars[f].current.noise.profiled, "frame {f}");
        assert_eq!(st.sidecars[f].current.noise.strength, 1.8, "frame {f}");
        assert_eq!(
            st.sidecars[f].current.noise.learned,
            Learned::Off,
            "the preset's Noise never carries a tier, frame {f}"
        );
    }
    assert_eq!(
        st.sidecars[0].current.noise.learned_strength, 0.123,
        "its own blend, not the ISO's"
    );
    assert_eq!(st.sidecars[1].current.noise.learned_strength, blend(3200));
    assert!(!crate::rows::wants_seed(&st, 1), "seeded");
    assert!(crate::rows::wants_seed(&st, 2), "not a target: still waits");
}

/// A preset naming a camera profile checks every frame's body the
/// same way, the open frame included: `preset_for_body` (what a
/// preset click gives its own frame) and `lay_over_targets` (what
/// it gives the rest of the set) agree over a mixed selection.
#[test]
fn a_preset_click_checks_every_bodys_fit_the_open_frame_included() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let mut edit = Edit::default();
    edit.camera.profile = ProfileChoice::Named("r5".into());
    edit.light.exposure = 0.4;
    let preset = Preset::from_edit("Portra 400", &edit, &[Section::Camera, Section::Light]);
    let profiles = [entry("r5", Some("Canon EOS R5"))];
    let bodies = |_: &State, f: usize| match f {
        0 => Some(body("Canon", "EOS R5", 100)), // the open frame: fits
        1 => Some(body("Canon", "EOS R5", 200)), // a target: fits
        2 => Some(body("Fujifilm", "X-T5", 400)), // a target: does not
        _ => None,
    };

    // The open frame (0) fits: the fit check leaves it whole.
    let (current_preset, current_left_off) =
        preset_for_body(&state.borrow(), &preset, 0, &profiles, bodies);
    assert!(!current_left_off);
    assert_eq!(current_preset.sections, preset.sections);

    // The rest of the set, through the same loop a sync runs.
    let mut synced = lay_over_targets(
        &mut state.borrow_mut(),
        &app,
        &preset,
        &[1, 2],
        None,
        Some("Preset ×2: Film"),
        &profiles,
        bodies,
    );
    assert_eq!(synced.moved, [1, 2]);
    assert_eq!(synced.profile_left_off, [2]);
    if current_left_off {
        synced.profile_left_off.push(0);
        synced.profile_left_off.sort_unstable();
    }
    assert_eq!(
        synced.profile_left_off,
        [2],
        "the open frame fit, so it stands alone"
    );
    let st = state.borrow();
    assert_eq!(st.sidecars[1].current.camera.profile.name(), "r5");
    assert!(st.sidecars[2].current.camera.profile.is_embedded());
    assert_eq!(st.sidecars[1].current.light.exposure, 0.4);
    assert_eq!(
        st.sidecars[2].current.light.exposure, 0.4,
        "Light still lands"
    );
    drop(st);

    // Now the open frame is the one of another body: it gets the
    // same protection the mismatched target got, not the profile
    // just because it is the one on screen.
    let (current_preset, current_left_off) =
        preset_for_body(&state.borrow(), &preset, 2, &profiles, bodies);
    assert!(current_left_off, "the open frame gets the same check");
    assert!(!current_preset.sections.contains(&Section::Camera));
    assert!(
        current_preset.sections.contains(&Section::Light),
        "the rest of the preset still reaches it"
    );
}

/// `apply_preset` is what `on_preset_applied` calls: a left-off
/// open frame is folded into the same report a left-off target
/// gets, not just the two helpers it is built from
/// (`preset_for_body` and `lay_over_targets`, tested apart above).
#[test]
fn apply_preset_names_the_open_frame_among_a_left_off_profile() {
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    let edit = Edit {
        camera: greycard_edit::Camera {
            profile: ProfileChoice::Named("r5".into()),
            ..Default::default()
        },
        light: greycard_edit::Light {
            exposure: 0.4,
            ..Default::default()
        },
        ..Edit::default()
    };
    let preset = Preset::from_edit("Portra 400", &edit, &[Section::Camera, Section::Light]);
    let profiles = [entry("r5", Some("Canon EOS R5"))];
    let bodies = |_: &State, f: usize| match f {
        0 => Some(body("Fujifilm", "X-T5", 100)), // the open frame: does not fit
        1 => Some(body("Canon", "EOS R5", 200)),  // a target: fits
        _ => None,
    };
    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0, 1];

    apply_preset(
        &mut state.borrow_mut(),
        &app,
        &worker,
        &preset,
        &profiles,
        bodies,
    );

    let status = app.get_status();
    assert!(status.contains("Portra 400 onto 2 frames"), "{status}");
    assert!(
        status.contains("the camera profile left off IMG_0000.CR3 (made for another camera)"),
        "{status}: the open frame, not just the target"
    );
    let st = state.borrow();
    assert!(
        st.sidecars[0].current.camera.profile.is_embedded(),
        "left off the open frame"
    );
    assert_eq!(
        st.sidecars[0].current.light.exposure, 0.4,
        "Light still lands"
    );
    assert_eq!(st.sidecars[1].current.camera.profile.name(), "r5");
}

/// A single-frame click that leaves the camera profile off says
/// so too, the same tail a set's report gives a left-off frame,
/// and — outside culling — those words survive a real develop
/// landing the way a set's do.
#[test]
fn a_single_frame_click_names_a_left_off_profile_and_the_words_ride_the_develop() {
    let app = window(1);
    let (state, worker) = state_for(&app, folder(1));
    let edit = Edit {
        camera: greycard_edit::Camera {
            profile: ProfileChoice::Named("r5".into()),
            ..Default::default()
        },
        white_balance: WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.0,
        },
        ..Edit::default()
    };
    let preset = Preset::from_edit(
        "Portra 400",
        &edit,
        &[Section::Camera, Section::WhiteBalance],
    );
    let profiles = [entry("r5", Some("Canon EOS R5"))];
    let bodies = |_: &State, _: usize| Some(body("Fujifilm", "X-T5", 100));

    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0];
    assert!(sync_targets(&state.borrow()).is_empty(), "one frame alone");

    apply_preset(
        &mut state.borrow_mut(),
        &app,
        &worker,
        &preset,
        &profiles,
        bodies,
    );
    assert_eq!(
        app.get_status(),
        "developing...",
        "take_current's own word, for now"
    );
    let generation = state.borrow().develop.generation;
    let said = "Portra 400 applied; the camera profile left off IMG_0000.CR3 \
                 (made for another camera)";
    assert_eq!(
        state.borrow().develop.status_after_develop,
        Some((generation, said.to_string()))
    );
    // The profile was left off; the rest of the preset still
    // landed.
    let st = state.borrow();
    assert!(st.sidecars[0].current.camera.profile.is_embedded());
    assert_eq!(
        st.sidecars[0].current.white_balance,
        WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.0
        }
    );
    drop(st);

    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Developed {
            generation,
            turn: 0,
            kind: crate::finish::Source::SCENE,
            image: crate::worker::Developed::Halves(std::sync::Arc::new(crate::worker::Halves {
                width: 60,
                height: 40,
                pixels: Vec::new(),
            })),
            guide: std::sync::Arc::new(crate::finish::Guide::NONE),
            planes: None,
            white: crate::worker::WhiteBase::IDENTITY,
            seconds: 0.1,
            sharpen: None,
            sources: Vec::new(),
            learned: crate::worker::LearnedReport::Off,
            fills: crate::worker::FillReport::default(),
        },
    );
    assert_eq!(app.get_status(), said);
    assert!(
        state.borrow().develop.status_after_develop.is_none(),
        "taken"
    );
}

/// In culling, `leave_cull(.., None)` reads the edit to leave with
/// off the very sidecar a preset click just recorded onto, so the
/// two never differ and its own "write once the panel rests"
/// never fires: the click saves the sidecar itself first, both
/// with one frame open and with a set of two or more. Each of the
/// two branches gets its own folder and its own app, so neither
/// leaves anything behind in the panel for the other to read.
#[test]
fn a_preset_click_in_culling_saves_the_open_frames_sidecar() {
    // `also`: the rest of the set beside frame 0, empty for the
    // single-frame branch.
    for (tag, also) in [("one", vec![]), ("set", vec![1])] {
        let dir =
            std::env::temp_dir().join(format!("greycard-preset-cull-{tag}-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let preset = Preset::from_edit("Portra 400", &edit, &[Section::Light]);
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = greycard_edit::Placement::Folder;
            st.develop.presets = vec![Entry {
                path: PathBuf::new(),
                preset,
            }];
        }
        let on_disk = Sidecar::path_in(&files[0], greycard_edit::Placement::Folder);

        app.invoke_select(0);
        app.invoke_cull_toggled();
        assert!(state.borrow().culling.cull.is_some(), "{tag}");
        // The set is made once the frame is open: opening it
        // makes the set that frame alone.
        let set = !also.is_empty();
        state.borrow_mut().views.picked = also;
        assert_eq!(
            chosen_frames(&state.borrow()).len(),
            if set { 2 } else { 1 },
            "{tag}"
        );
        app.invoke_preset_applied(0);
        assert!(
            state.borrow().culling.cull.is_none(),
            "{tag}: the click left culling"
        );
        assert_eq!(
            state.borrow().sidecars[0].current.light.exposure,
            0.5,
            "{tag}"
        );
        assert!(
            on_disk.exists(),
            "{tag}: the step was saved, not just recorded in memory"
        );
        assert_eq!(
            Sidecar::load(&files[0]).unwrap().unwrap().current,
            state.borrow().sidecars[0].current,
            "{tag}"
        );
        let words = if tag == "one" {
            "Preset: Portra 400"
        } else {
            "Preset ×2: Portra 400"
        };
        assert_eq!(
            Sidecar::load(&files[0])
                .unwrap()
                .unwrap()
                .current_label
                .as_deref(),
            Some(words),
            "{tag}"
        );
        if tag == "set" {
            assert_eq!(
                state.borrow().sidecars[1].current_label.as_deref(),
                Some(words)
            );
        }
        crate::testing::remove_dir_retry(&dir);
    }
}

/// A preset click over a set whose current frame needs a real
/// develop leaves its own words for that develop to carry: the
/// develop's "developing..." does not take them away for good.
#[test]
fn a_multi_frame_presets_words_ride_the_develop_that_lands() {
    let app = window(2);
    let (state, _worker) = state_for(&app, folder(2));
    // A white balance, not a light control: the history's
    // `same_develop` reads the raw develop's own inputs, and
    // exposure is the finish's, so only this asks the worker for a
    // fresh one.
    let edit = Edit {
        white_balance: WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.0,
        },
        ..Edit::default()
    };
    let preset = Preset::from_edit("Portra 400", &edit, &[Section::WhiteBalance]);
    state.borrow_mut().develop.presets = vec![Entry {
        path: PathBuf::new(),
        preset,
    }];
    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0, 1];
    app.invoke_preset_applied(0);
    assert_eq!(
        app.get_status(),
        "developing...",
        "take_current's own word, for now"
    );
    let generation = state.borrow().develop.generation;
    assert_eq!(
        state.borrow().develop.status_after_develop,
        Some((generation, "Portra 400 onto 2 frames".to_string()))
    );

    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Developed {
            generation,
            turn: 0,
            kind: crate::finish::Source::SCENE,
            image: crate::worker::Developed::Halves(std::sync::Arc::new(crate::worker::Halves {
                width: 100,
                height: 80,
                pixels: Vec::new(),
            })),
            guide: std::sync::Arc::new(crate::finish::Guide::NONE),
            planes: None,
            white: crate::worker::WhiteBase::IDENTITY,
            seconds: 0.42,
            sharpen: None,
            sources: Vec::new(),
            learned: crate::worker::LearnedReport::Off,
            fills: crate::worker::FillReport::default(),
        },
    );
    // The set's words alone: the develop's own are the log's.
    assert_eq!(app.get_status(), "Portra 400 onto 2 frames");
    assert!(
        state.borrow().develop.status_after_develop.is_none(),
        "taken"
    );
}

/// The history panel names a preset's step by the preset, a
/// snapshot restored by the snapshot, and the rest by what moved;
/// an undo keeps the preset's row as a redo, and a new step after
/// it drops the row and its words.
#[test]
fn the_history_rows_read_a_named_step_by_its_words() {
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    let mut edit = Edit::default();
    edit.light.exposure = 0.5;
    edit.light.tone.contrast = 0.25;
    let preset = Preset::from_edit("Faded film", &edit, &[Section::Light]);
    state.borrow_mut().develop.presets = vec![Entry {
        path: PathBuf::new(),
        preset,
    }];
    app.invoke_select(0);
    let rows = |app: &App| -> Vec<String> {
        let names = app.get_history_rows();
        (0..names.row_count())
            .map(|r| names.row_data(r).unwrap().name.to_string())
            .collect()
    };

    app.invoke_preset_applied(0);
    assert_eq!(rows(&app), ["Preset: Faded film", "Original"]);

    // Undone, the row is still there to redo, under its words.
    app.invoke_undo();
    assert_eq!(rows(&app), ["Preset: Faded film", "Original"]);
    assert_eq!(app.get_history_current(), 1);
    app.invoke_redo();
    assert_eq!(
        state.borrow().sidecars[0].current_label.as_deref(),
        Some("Preset: Faded film")
    );

    // A snapshot of it, a slider moved, the snapshot restored:
    // the restore is named for the snapshot even though a single
    // section moved.
    app.invoke_snapshot_taken();
    app.set_exposure(1.5);
    app.invoke_snapshot_restored(0);
    assert_eq!(
        rows(&app),
        [
            "Snapshot: Snapshot 1",
            "Exposure +1.50",
            "Preset: Faded film",
            "Original"
        ]
    );

    // Back past the preset, and a new step: the preset's state
    // and its words are gone with the redo stack.
    app.invoke_undo();
    app.invoke_undo();
    app.invoke_undo();
    assert_eq!(app.get_history_current(), 3);
    app.set_exposure(-1.0);
    app.invoke_snapshot_taken();
    assert_eq!(rows(&app), ["Exposure -1.00", "Original"]);
    let st = state.borrow();
    assert!(st.sidecars[0].redo.is_empty());
    assert!((0..st.sidecars[0].states()).all(|i| st.sidecars[0].label(i).is_none()));
}

/// Ctrl (a Mac's Command, to Slint) held over `f`.
fn with_control(app: &App, f: impl FnOnce()) {
    with_modifier(app, Key::Control, f);
}

fn history_rows(app: &App) -> Vec<String> {
    let names = app.get_history_rows();
    (0..names.row_count())
        .map(|r| names.row_data(r).unwrap().name.to_string())
        .collect()
}

#[test]
fn copy_and_paste_lay_the_copy_over_the_set_and_leave_its_source() {
    let dir = std::env::temp_dir().join(format!("greycard-paste-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let files: Vec<PathBuf> = (0..4)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    let app = window(4);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.placement = greycard_edit::Placement::Folder;
        // A frame the paste goes onto, with a crop of its own.
        let mut own = Edit::default();
        own.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        });
        st.sidecars[3].record(own);
    }
    // Nothing copied: Paste is grayed and the key says so.
    assert_eq!(app.get_clip_from(), "");
    app.invoke_select(1);
    with_control(&app, || press(&app, "v"));
    assert!(!app.get_sync_open());
    assert!(app.get_status().starts_with("nothing copied yet"));

    // Frame 1's panel, copied with Ctrl+C, unrecorded as it is.
    app.set_exposure(0.8);
    with_control(&app, || press(&app, "c"));
    assert_eq!(app.get_clip_from(), "IMG_0001.CR3");
    {
        let st = state.borrow();
        let clip = st.sheets.clipboard.as_ref().expect("a copy");
        assert_eq!(clip.from.path, files[1]);
        assert_eq!(clip.edit.light.exposure, 0.8);
    }
    // Onto the frame it came from alone: nothing to do.
    with_control(&app, || press(&app, "v"));
    assert!(!app.get_sync_open());
    assert_eq!(
        app.get_status(),
        "IMG_0001.CR3 is the frame these came from; choose the frames to paste onto"
    );

    // Two more chosen: the sheet, over the two, the source said.
    with_control(&app, || {
        let (x, y) = strip_cell(&app, 0);
        click(&app, x, y);
        let (x, y) = strip_cell(&app, 3);
        click(&app, x, y);
    });
    assert_eq!(chosen_frames(&state.borrow()), vec![0, 1, 3]);
    with_control(&app, || press(&app, "v"));
    assert!(app.get_sync_open());
    assert!(app.get_sync_paste());
    assert_eq!(
        app.get_sync_text(),
        "Onto 2 frames; IMG_0001.CR3, which they came from, is left as it is."
    );
    // Nobody synced yet this session: the sync's defaults.
    assert_eq!(headings(&app), defaults());
    // Light alone, then Paste.
    choose(&app, |s| s == Section::Light);
    app.invoke_sync_applied();
    assert!(!app.get_sync_open());
    assert_eq!(
        app.get_status(),
        "pasted 1 section from IMG_0001.CR3 onto 2 frames"
    );
    {
        let st = state.borrow();
        for f in [0, 3] {
            let s = &st.sidecars[f];
            assert_eq!(s.current.light.exposure, 0.8, "{f}");
            assert_eq!(s.current_label.as_deref(), Some("Paste from IMG_0001.CR3"));
            let back = Sidecar::load(&files[f]).unwrap().unwrap();
            assert_eq!(back.current, s.current);
            assert_eq!(back.current_label, s.current_label);
        }
        // The crop stayed the frame's own; one step more.
        assert!(st.sidecars[3].current.geometry.crop.is_some());
        assert_eq!(st.sidecars[3].history.len(), 2);
        // The source is untouched by the paste; the frame outside
        // the set is untouched altogether.
        assert_eq!(st.sidecars[1].current_label, None);
        assert_eq!(st.sidecars[2], Sidecar::default());
        assert_eq!(
            st.sheets.sync_last.as_ref().map(|c| c.choice()),
            Some(Choice::whole(&[Section::Light]))
        );
    }
    // A second paste opens on the choice the first made, and with
    // nothing new to lay over, records nothing.
    with_control(&app, || press(&app, "v"));
    for ((name, on), s) in headings(&app).iter().zip(Section::ALL) {
        assert_eq!(name, s.title());
        assert_eq!(*on, if *s == Section::Light { 2 } else { 0 }, "{s:?}");
    }
    app.invoke_sync_applied();
    assert_eq!(app.get_status(), "the 2 frames had these already");
    assert_eq!(state.borrow().sidecars[3].history.len(), 2);
    // A paste's sheet left by Escape leaves no paste behind it.
    with_control(&app, || press(&app, "v"));
    assert!(app.get_sync_paste());
    press(&app, Key::Escape);
    slint::platform::update_timers_and_animations();
    assert!(!app.get_sync_open());
    assert!(!app.get_sync_paste());
    // The sync's own sheet still opens on its defaults, as a sync.
    app.invoke_sync_open_asked();
    assert!(!app.get_sync_paste());
    assert_eq!(headings(&app), defaults());
    press(&app, Key::Escape);
    crate::testing::remove_dir_retry(&dir);
}

/// Four frames, the third a JPEG: the display curve's switch shows
/// only on a raw, so a paste never lays a curve on a picture nor
/// takes one from it.
fn frames_with_a_jpeg() -> Vec<PathBuf> {
    let mut files = folder(4);
    files[2] = PathBuf::from("/nowhere/IMG_0002.JPG");
    files
}

/// The display curve is a row of the paste sheet, on by default:
/// left on, the source's curve lands on each raw, one step each
/// under the paste's label, and a JPEG keeps its default; turned
/// off, each frame keeps its own.
#[test]
fn a_paste_carries_the_display_curve_when_its_row_is_on() {
    use greycard_edit::DisplayCurve;
    let app = window(4);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, frames_with_a_jpeg());
    // Frame 3 per channel, frame 0 AgX: each its own.
    state.borrow_mut().sidecars[0].record(Edit {
        display_curve: DisplayCurve::Agx,
        ..Edit::default()
    });
    // Frame 1 under AgX, a touch brighter, copied.
    app.invoke_select(1);
    app.set_display_curve(DisplayCurve::Agx.name().into());
    app.set_exposure(0.4);
    with_control(&app, || press(&app, "c"));
    with_control(&app, || {
        let (x, y) = strip_cell(&app, 2);
        click(&app, x, y);
        let (x, y) = strip_cell(&app, 3);
        click(&app, x, y);
    });
    assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3]);

    // The row is there, under its panel name, beside Light, and on.
    with_control(&app, || press(&app, "v"));
    assert!(app.get_sync_paste());
    let heads = headings(&app);
    let at = heads
        .iter()
        .position(|(name, _)| name == "Display curve")
        .expect("a Display curve row");
    assert_eq!(heads[at - 1].0, "Light");
    assert_eq!(heads[at].1, 2);
    // Light alone: frame 3 keeps per channel.
    choose(&app, |s| s == Section::Light);
    app.invoke_sync_applied();
    {
        let st = state.borrow();
        let s = &st.sidecars[3];
        assert_eq!(s.current.light.exposure, 0.4);
        assert_eq!(s.current.display_curve, DisplayCurve::Channels);
    }

    // Light and the curve: frame 3 takes AgX, in one more step
    // under the paste's label; the JPEG takes the light alone.
    with_control(&app, || press(&app, "v"));
    choose(&app, |s| s == Section::Light || s == Section::DisplayCurve);
    app.invoke_sync_applied();
    let st = state.borrow();
    let s = &st.sidecars[3];
    assert_eq!(s.current.display_curve, DisplayCurve::Agx);
    assert_eq!(s.current.light.exposure, 0.4);
    assert_eq!(s.current_label.as_deref(), Some("Paste from IMG_0001.CR3"));
    assert_eq!(s.history.len(), 2);
    let jpeg = &st.sidecars[2];
    assert_eq!(jpeg.current.light.exposure, 0.4);
    assert_eq!(jpeg.current.display_curve, DisplayCurve::Channels);
    // Frame 0, outside the set, is as it was.
    assert_eq!(st.sidecars[0].current.display_curve, DisplayCurve::Agx);
    assert_eq!(st.sidecars[0].current.light.exposure, 0.0);
}

/// Pasted onto the frame on screen, the curve reaches the panel's
/// switch, and is undone as any step is; copied from a JPEG, there
/// is no curve to carry, and an AgX raw keeps AgX.
#[test]
fn a_pasted_display_curve_shows_on_the_frame_on_screen() {
    use greycard_edit::DisplayCurve;
    let app = window(4);
    let (_state, _worker) = state_for(&app, frames_with_a_jpeg());
    let agx = DisplayCurve::Agx.name();
    let channels = DisplayCurve::Channels.name();
    app.invoke_select(1);
    app.set_display_curve(agx.into());
    app.invoke_copy_asked();
    app.invoke_select(3);
    assert_eq!(app.get_display_curve(), channels);
    app.invoke_paste_asked();
    assert_eq!(app.get_sync_text(), "Onto IMG_0003.CR3.");
    app.invoke_sync_applied();
    assert_eq!(app.get_display_curve(), agx);
    assert_eq!(history_rows(&app), ["Paste from IMG_0001.CR3", "Original"]);
    app.invoke_undo();
    assert_eq!(app.get_display_curve(), channels);
    app.invoke_redo();
    assert_eq!(app.get_display_curve(), agx);

    // From the JPEG, the last paste's checks, all on: the curve's
    // row is off, the status counts what went, and frame 3 stays
    // AgX.
    app.invoke_select(2);
    app.set_exposure(0.3);
    app.invoke_copy_asked();
    app.invoke_select(3);
    app.invoke_paste_asked();
    let heads = headings(&app);
    assert!(
        heads.contains(&("Display curve".to_string(), 0)),
        "{heads:?}"
    );
    assert!(heads.contains(&("Light".to_string(), 2)), "{heads:?}");
    app.invoke_sync_applied();
    assert_eq!(app.get_exposure(), 0.3);
    assert_eq!(app.get_display_curve(), agx);
    assert!(
        app.get_status()
            .starts_with("pasted 18 sections from IMG_0002.JPG"),
        "{}",
        app.get_status()
    );
    // The row checked by hand: still nothing goes.
    app.invoke_paste_asked();
    app.invoke_sync_all(true);
    app.invoke_sync_applied();
    assert_eq!(app.get_display_curve(), agx);
}

/// A preset saved from a JPEG, every row checked, does not name
/// the display curve, which would set an AgX raw back to per
/// channel.
#[test]
fn a_preset_saved_from_a_picture_carries_no_display_curve() {
    let app = window(4);
    let (state, _worker) = state_for(&app, frames_with_a_jpeg());
    let dir = crate::testing::scratch_dir("preset-curve");
    state.borrow_mut().develop.preset_store = Some(preset::Store::at(dir.clone()));
    app.invoke_select(2);
    app.set_exposure(0.3);
    app.invoke_preset_save_open();
    app.invoke_preset_all(true);
    app.set_preset_name("Bright".into());
    app.invoke_preset_saved();
    assert!(
        app.get_status().starts_with("saved Bright (19 sections)"),
        "{}",
        app.get_status()
    );
    let saved = preset::Store::at(dir.clone())
        .find("Bright")
        .expect("saved")
        .preset;
    assert!(!saved.sections.contains(&Section::DisplayCurve));
    assert!(saved.sections.contains(&Section::Light));
    let text = std::fs::read_to_string(dir.join("bright.gcp")).expect("the file");
    assert!(!text.contains("display-curve"), "{text}");
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn a_paste_onto_the_frame_on_screen_goes_through_the_panel_and_undoes() {
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_exposure(-0.6);
    app.invoke_copy_asked();
    // Another frame opened: the paste is onto it, through the
    // panel, as one step with the paste's words.
    app.invoke_select(2);
    assert_eq!(app.get_exposure(), 0.0);
    app.invoke_paste_asked();
    assert_eq!(app.get_sync_text(), "Onto IMG_0002.CR3.");
    app.invoke_sync_applied();
    assert_eq!(app.get_exposure(), -0.6);
    assert_eq!(history_rows(&app), ["Paste from IMG_0000.CR3", "Original"]);
    assert_eq!(
        state.borrow().sidecars[2].current_label.as_deref(),
        Some("Paste from IMG_0000.CR3")
    );
    // Undone as any step is: the frame back as it was.
    app.invoke_undo();
    assert_eq!(app.get_exposure(), 0.0);
    assert_eq!(state.borrow().sidecars[2].current.light.exposure, 0.0);
    // The frame it was copied from never took a step of it.
    assert!(state.borrow().sidecars[0].current_label.is_none());
}

/// The paste sheet from the keys alone (issue 78): it opens with
/// Paste holding the keys, so Enter takes the checks as they
/// stand.
#[test]
fn the_paste_sheet_opens_on_paste_and_enter_pastes() {
    let app = window(3);
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_exposure(-0.6);
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    assert!(app.get_sync_open());
    press(&app, Key::Return);
    assert!(!app.get_sync_open());
    assert_eq!(app.get_exposure(), -0.6);
    assert_eq!(history_rows(&app), ["Paste from IMG_0000.CR3", "Original"]);
    // The keys are the window's again: Tab hides the panels.
    assert!(!app.get_left_hidden());
    press(&app, Key::Tab);
    assert!(app.get_left_hidden(), "Tab reached the window");
}

/// The states of the sheet's rows, in their order.
fn row_states(app: &App) -> Vec<i32> {
    let rows = app.get_sync_rows();
    (0..rows.row_count())
        .map(|i| rows.row_data(i).unwrap().state)
        .collect()
}

/// Tab goes from Paste round to Check all, Uncheck all, and the
/// checks, every one a stop, then Cancel and Paste; Space turns a
/// check over, and Shift+Tab goes back the same way.
#[test]
fn tab_and_shift_tab_go_round_the_paste_sheet() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = state_for(&app, folder(3));
    let shift_tab = |app: &App| with_modifier(app, Key::Shift, || press(app, Key::Tab));
    app.invoke_select(0);
    app.set_exposure(-0.6);
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    let checks = {
        let rows = app.get_sync_rows();
        (0..rows.row_count())
            .filter(|&i| rows.row_data(i).unwrap().kind != 2)
            .count()
    };
    assert!(checks > 2, "{checks}");

    // Past Paste to Check all, then Uncheck all, then the first
    // check: Space turns it over and nothing else.
    let before = row_states(&app);
    press(&app, Key::Tab);
    press(&app, Key::Tab);
    press(&app, Key::Tab);
    press(&app, " ");
    let after = row_states(&app);
    assert_ne!(before[0], after[0], "the first check turned over");
    assert!(app.get_sync_open());
    // Back once to Uncheck all, then to Check all: each pressed.
    shift_tab(&app);
    press(&app, " ");
    assert!(
        row_states(&app).iter().all(|&s| s == 0),
        "{:?}",
        row_states(&app)
    );
    shift_tab(&app);
    press(&app, " ");
    assert!(row_states(&app).iter().any(|&s| s != 0));
    let all = row_states(&app);
    // Back from Check all round to Paste, forward round to Check
    // all again, and back: Enter pastes.
    shift_tab(&app);
    press(&app, Key::Tab);
    assert_eq!(row_states(&app), all, "Tab moved and toggled nothing");
    shift_tab(&app);
    press(&app, Key::Return);
    assert!(!app.get_sync_open());
    assert_eq!(app.get_exposure(), -0.6);

    // Again, and forward through every check: the stop after the
    // last is Cancel, which leaves the sheet and pastes nothing.
    app.invoke_undo();
    assert_eq!(app.get_exposure(), 0.0);
    app.invoke_paste_asked();
    assert!(app.get_sync_open());
    let before = row_states(&app);
    // Paste to Check all, Uncheck all, the checks and the masks'
    // choice (every row a stop), then Cancel.
    for _ in 0..2 + app.get_sync_rows().row_count() + 1 {
        press(&app, Key::Tab);
    }
    press(&app, Key::Return);
    assert!(!app.get_sync_open(), "Enter on Cancel closed the sheet");
    assert_eq!(app.get_exposure(), 0.0, "and pasted nothing");
    // Shift+Tab from Paste is Cancel too; Escape still cancels.
    app.invoke_paste_asked();
    assert_eq!(row_states(&app), before);
    shift_tab(&app);
    press(&app, " ");
    assert!(!app.get_sync_open());
    app.invoke_paste_asked();
    shift_tab(&app);
    press(&app, Key::Escape);
    assert!(!app.get_sync_open());
    assert_eq!(app.get_exposure(), 0.0);
    // Enter on a check is Paste, never the check turned over.
    app.invoke_paste_asked();
    for _ in 0..3 {
        press(&app, Key::Tab);
    }
    assert_eq!(row_states(&app), before);
    press(&app, Key::Return);
    assert!(!app.get_sync_open());
    assert_eq!(app.get_exposure(), -0.6);
}

/// The frame menu's Paste leaves the keys on the sheet's Paste:
/// the menu's own hand-back, a moment later, takes nothing from it.
#[test]
fn the_menus_paste_opens_on_paste_too() {
    use i_slint_backend_testing::ElementQuery;
    use slint::platform::PointerEventButton;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_exposure(-0.6);
    app.invoke_copy_asked();
    app.invoke_select(2);
    let (x, y) = strip_cell(&app, 2);
    let position = slint::LogicalPosition::new(x, y);
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
    let item = ElementQuery::from_root(&app)
        .match_descendants()
        .match_predicate(|e| {
            e.accessible_label()
                .is_some_and(|l| l.starts_with("Paste settings from IMG_0000.CR3"))
        })
        .find_first()
        .expect("the menu's Paste");
    item.mock_single_click(PointerEventButton::Left);
    assert!(
        app.get_sync_open(),
        "{:?} {:?}",
        item.accessible_label(),
        app.get_status()
    );
    // Past the menu's hand-back of the keys to the window.
    for _ in 0..5 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(5));
        slint::platform::update_timers_and_animations();
    }
    press(&app, Key::Return);
    assert!(!app.get_sync_open());
    assert_eq!(app.get_exposure(), -0.6);
}

/// A check reached by Tab below the list's view is brought into
/// it, above the sheet's buttons.
#[test]
fn a_check_tabbed_to_is_scrolled_into_view() {
    use i_slint_backend_testing::ElementHandle;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 420.0));
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_exposure(-0.6);
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    let rows = app.get_sync_rows();
    let checks: Vec<_> = (0..rows.row_count())
        .map(|i| rows.row_data(i).unwrap())
        .filter(|r| r.kind != 2)
        .collect();
    // The lowest part's check, near the foot of the longer column:
    // a part's label names its section too, so no tab or heading
    // elsewhere in the window shares it.
    let (at, low) = checks
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == 1)
        .max_by_key(|(_, r)| r.line)
        .unwrap();
    let low = low.a11y.to_string();
    // A button's words: its own label.
    let cancel = ElementHandle::find_by_element_id(&app, "SyncSheet::cancel")
        .next()
        .unwrap()
        .absolute_position()
        .y;
    // Where the check's foot is, if it is drawn at all: one
    // scrolled out of the list's view is not.
    let bottom = |label: &str| {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        ElementHandle::find_by_accessible_label(&app, label)
            .next()
            .map(|e| e.absolute_position().y + e.size().height)
    };
    assert!(
        bottom(&low).is_none_or(|b| b > cancel),
        "the list scrolls in this window"
    );
    // Paste, round to Check all, Uncheck all, then the checks.
    for _ in 0..3 + at {
        press(&app, Key::Tab);
    }
    // Slint scrolls the check that took the focus into view, and
    // since 1.19 animates the scroll: let it end.
    for _ in 0..60 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        slint::platform::update_timers_and_animations();
    }
    let b = bottom(&low).expect("the check in view");
    assert!(b <= cancel, "{b} > {cancel}");
    press(&app, Key::Escape);
}

/// The primary button's ring is for the keys' focus: Tab onto it
/// rings it, a click on it does not.
#[test]
fn a_primary_buttons_ring_is_for_the_keys() {
    use i_slint_backend_testing::ElementHandle;
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = state_for(&app, folder(3));
    let shift_tab = |app: &App| with_modifier(app, Key::Shift, || press(app, Key::Tab));
    let rings = |app: &App| {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        ElementHandle::find_by_element_id(app, "PanelButton::ring").count()
    };
    app.invoke_select(0);
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    // Open with the keys on Paste: a ring.
    assert_eq!(rings(&app), 1);
    // A click on Uncheck all: the focus there, no ring anywhere.
    let words = ElementHandle::find_by_element_id(&app, "CarryList::uncheck-all")
        .next()
        .unwrap();
    let (at, size) = (words.absolute_position(), words.size());
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(app.get_sync_open());
    assert_eq!(rings(&app), 0);
    // Back to Paste by the keys: Shift+Tab twice from Uncheck all
    // goes round through Check all; the ring is back.
    shift_tab(&app);
    shift_tab(&app);
    assert_eq!(rings(&app), 1);
    press(&app, Key::Escape);
}

/// A Subject mask pasted onto another frame has the same key there,
/// adjustment and component, and the same shape: the panel's raster
/// of one frame is never the other's. Going back to the frame it
/// came from asks the worker for that frame's own, and a raster the
/// worker made for a frame no longer open is not taken for the one
/// open now.
#[test]
fn a_pasted_subject_mask_is_made_for_each_frame_and_never_shared() {
    use crate::panel::mask::bake_locals;
    use greycard_edit::brush::Raster;
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    let files = state.borrow().files.clone();
    let raster = |fill: u8| Arc::new(Raster::from_data(1.5, 4, vec![fill; 24]));
    let made = |file: &PathBuf, fill: u8| crate::worker::Outcome::Mask {
        key: (1, 0),
        shape: Shape::Subject {},
        file: Some(file.clone()),
        raster: raster(fill),
        provider: None,
        seconds: 0.0,
        note: None,
        part: None,
    };
    // What the viewport would draw for (1, 0) now, and what it
    // would ask the worker for.
    let baked = |state: &Rc<Shared>| {
        let st = &mut *state.borrow_mut();
        let edit = read_edit(&app, &st.develop.edit, st.develop.target);
        let (_, wants) = bake_locals(
            &edit,
            &mut st.develop.rasters,
            &mut st.develop.learned,
            1.5,
            None,
        );
        let held = st.develop.learned.get(&(1, 0)).map(|(_, r)| r.data()[0]);
        (held, wants.iter().any(|(k, _)| *k == (1, 0)))
    };
    app.invoke_select(0);
    state
        .borrow_mut()
        .develop
        .edit
        .adjustments
        .push(greycard_edit::Adjustment {
            id: 1,
            name: "Subject 1".into(),
            enabled: true,
            mask: Mask {
                components: vec![Component {
                    shape: Shape::Subject {},
                    ..Default::default()
                }],
                invert: false,
                ..Default::default()
            },
            look: Default::default(),
            opacity: 1.0,
        });
    crate::panel::deliver::deliver(&app, made(&files[0], 10));
    assert_eq!(baked(&state), (Some(10), false), "the source's own");
    app.invoke_copy_asked();
    // Pasted onto the frame opened next, before any frame is drawn.
    app.invoke_select(2);
    app.invoke_paste_asked();
    choose(&app, |s| s == Section::Adjustments);
    app.invoke_sync_applied();
    assert_eq!(
        state.borrow().sidecars[2].current.adjustments[0].id,
        1,
        "the same key"
    );
    assert_eq!(baked(&state), (None, true), "asked for, not the source's");
    crate::panel::deliver::deliver(&app, made(&files[2], 20));
    assert_eq!(baked(&state), (Some(20), false), "the target's own");
    // Back to the source: its own raster is asked for, and the
    // target's, arriving late, is not taken for it.
    app.invoke_select(0);
    assert_eq!(baked(&state), (None, true), "asked for, not the target's");
    crate::panel::deliver::deliver(&app, made(&files[2], 20));
    assert_eq!(baked(&state), (None, true), "a late one of the target's");
    crate::panel::deliver::deliver(&app, made(&files[0], 10));
    assert_eq!(baked(&state), (Some(10), false), "the source's own again");
}

/// A Part carries its person: pasted onto another frame it is the
/// same person, signature and place, for that frame's raster to
/// look for, and nothing here resolves it.
#[test]
fn a_pasted_part_keeps_its_person() {
    use greycard_edit::mask::{Person, Route, Signature};
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    let part = Shape::Part {
        phrase: "lips".into(),
        route: Route::Mouth,
        person: Some(Person {
            signature: Signature {
                kind: "colors-1".into(),
                values: (0..22).map(|i| i as f32 * 0.5).collect(),
            },
            at: [0.42, 0.31],
            picture: Some("5e1d".into()),
        }),
    };
    app.invoke_select(0);
    {
        // On the panel's edit, as a part chosen there lands.
        let mut st = state.borrow_mut();
        st.develop.edit.adjustments.push(greycard_edit::Adjustment {
            id: 1,
            name: "Lips 1".into(),
            enabled: true,
            mask: Mask {
                components: vec![Component {
                    shape: part.clone(),
                    ..Default::default()
                }],
                invert: false,
                ..Default::default()
            },
            look: Default::default(),
            opacity: 1.0,
        });
    }
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    choose(&app, |s| s == Section::Adjustments);
    app.invoke_sync_applied();
    assert_eq!(
        app.get_status(),
        "pasted 1 section from IMG_0000.CR3 onto 1 frame"
    );
    let st = state.borrow();
    let pasted = &st.sidecars[2].current.adjustments[0].mask.components[0].shape;
    assert_eq!(pasted, &part);
}

#[test]
fn a_paste_in_culling_goes_onto_the_sidecars_and_stays_in_culling() {
    let dir = std::env::temp_dir().join(format!("greycard-paste-cull-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let files: Vec<PathBuf> = (0..3)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    let app = window(3);
    let (state, _worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.placement = greycard_edit::Placement::Folder;
        let mut edit = Edit::default();
        edit.light.exposure = 1.1;
        st.sidecars[2].record(edit);
    }
    app.invoke_select(0);
    app.invoke_cull_toggled();
    assert!(state.borrow().culling.cull.is_some());
    // In culling the copy is the sidecar's, not the panel's.
    state.borrow_mut().current = Some(2);
    app.invoke_copy_asked();
    assert_eq!(
        state
            .borrow()
            .sheets
            .clipboard
            .as_ref()
            .map(|c| c.edit.light.exposure),
        Some(1.1)
    );
    // Back on frame 0, with frame 1 beside it: both take it on
    // their sidecars, the frame on screen as any other.
    state.borrow_mut().current = Some(0);
    state.borrow_mut().views.picked = vec![0, 1];
    app.invoke_paste_asked();
    app.invoke_sync_applied();
    assert!(
        state.borrow().culling.cull.is_some(),
        "the paste left culling alone"
    );
    for f in [0, 1] {
        let st = state.borrow();
        assert_eq!(st.sidecars[f].current.light.exposure, 1.1, "{f}");
        let back = Sidecar::load(&files[f]).unwrap().unwrap();
        assert_eq!(
            back.current_label.as_deref(),
            Some("Paste from IMG_0002.CR3")
        );
    }
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn a_selection_moved_under_the_paste_sheet_is_refused() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(0);
    app.set_exposure(0.3);
    app.invoke_copy_asked();
    app.invoke_select(1);
    state.borrow_mut().views.picked = vec![1, 2];
    app.invoke_paste_asked();
    assert!(app.get_sync_open());
    state.borrow_mut().views.picked = vec![1, 3];
    app.invoke_sync_applied();
    assert_eq!(
        app.get_status(),
        "the selection changed while the sheet was open; nothing pasted"
    );
    let st = state.borrow();
    assert!(
        st.sidecars
            .iter()
            .skip(1)
            .all(|s| s.current_label.is_none())
    );
}

#[test]
fn the_filter_field_keeps_its_own_copy_and_paste() {
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    // The grid's filter field, with the focus, typed into: the
    // key opens the grid, whose field takes the focus on the next
    // turn of the loop.
    press(&app, "/");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    press(&app, "a");
    assert_eq!(app.get_filter_text(), "a");
    with_control(&app, || press(&app, "c"));
    with_control(&app, || press(&app, "v"));
    assert!(state.borrow().sheets.clipboard.is_none());
    assert!(!app.get_sync_open());
    // Given the keys back, the same Ctrl+C copies the frame.
    press(&app, Key::Escape);
    with_control(&app, || press(&app, "c"));
    assert!(state.borrow().sheets.clipboard.is_some());
}

/// Where to click the one element on screen a screen reader knows
/// as `label`, of `role` when one is given: on a check, its box;
/// otherwise its middle. It must be inside the window, so a click
/// there lands on it and on nothing else.
fn spot(
    app: &App,
    label: &str,
    role: Option<i_slint_backend_testing::AccessibleRole>,
) -> (f32, f32) {
    use i_slint_backend_testing::ElementHandle;
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    let found: Vec<_> = ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| role.is_none() || e.accessible_role() == role)
        .collect();
    assert_eq!(found.len(), 1, "one {label:?} on screen");
    let (at, size) = (found[0].absolute_position(), found[0].size());
    let window = app.window().size().to_logical(app.window().scale_factor());
    assert!(
        at.x >= 0.0
            && at.y >= 0.0
            && at.x + size.width <= window.width
            && at.y + size.height <= window.height,
        "{label:?} is off the window"
    );
    match role {
        Some(_) => (at.x + 7.0, at.y + size.height / 2.0),
        None => (at.x + size.width / 2.0, at.y + size.height / 2.0),
    }
}

/// A click on the sheet's check `label` ("Detail: Dehaze").
fn tick(app: &App, label: &str) {
    let (x, y) = spot(
        app,
        label,
        Some(i_slint_backend_testing::AccessibleRole::Checkbox),
    );
    click(app, x, y);
}

/// A click at the middle of the sheet's control `id`, the one on
/// screen, inside the window.
fn press_id(app: &App, id: &str) {
    use i_slint_backend_testing::ElementHandle;
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    let found: Vec<_> = ElementHandle::find_by_element_id(app, id).collect();
    assert_eq!(found.len(), 1, "one {id} on screen");
    let (at, size) = (found[0].absolute_position(), found[0].size());
    assert_in_window(app, at, size, id);
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// A click on the masks' row's Add to masks when `add`, its Replace
/// masks when not.
fn press_join(app: &App, add: bool) {
    const JOIN: [&str; 2] = ["Replace masks", "Add to masks"];
    let option = JOIN[usize::from(add)];
    assert_eq!(crate::testing::count_id(app, "CarryList::join"), 1);
    let (at, size) = crate::testing::segment(app, "CarryList::join", &JOIN, option);
    assert_in_window(app, at, size, option);
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

fn assert_in_window(app: &App, at: slint::LogicalPosition, size: slint::LogicalSize, what: &str) {
    let window = app.window().size().to_logical(app.window().scale_factor());
    assert!(
        at.x >= 0.0
            && at.y >= 0.0
            && at.x + size.width <= window.width
            && at.y + size.height <= window.height,
        "{what:?} is off the window"
    );
}

fn heading(app: &App, title: &str) -> i32 {
    headings(app)
        .into_iter()
        .find(|(name, _)| name == title)
        .unwrap_or_else(|| panic!("no heading {title:?}"))
        .1
}

fn mask(id: u64, name: &str, shape: Shape) -> greycard_edit::Adjustment {
    greycard_edit::Adjustment {
        id,
        name: name.into(),
        mask: Mask {
            components: vec![Component {
                shape,
                ..Default::default()
            }],
            invert: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn mask_names(edit: &Edit) -> Vec<String> {
    edit.adjustments.iter().map(|a| a.name.clone()).collect()
}

/// Frame 0 on the panel with a look and two masks, frame 2 with a
/// look and a mask of its own, in a window tall enough for the
/// sheet's whole list.
fn two_frames() -> (App, Rc<Shared>, Rc<Worker>) {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 1200.0));
    let (state, worker) = state_for(&app, folder(3));
    {
        let mut st = state.borrow_mut();
        let mut own = Edit::default();
        own.light.tone.shadows = -0.5;
        own.detail.texture = 0.2;
        own.adjustments = vec![mask(1, "Mine 1", Shape::Subject {})];
        st.sidecars[2].record(own);
    }
    app.invoke_select(0);
    app.set_exposure(0.7);
    app.set_contrast(1.3);
    app.set_detail_texture(0.6);
    app.set_detail_dehaze(0.4);
    state.borrow_mut().develop.edit.adjustments = vec![
        mask(1, "Sky 1", Shape::Sky { picks: Vec::new() }),
        mask(2, "Face 2", Shape::Subject {}),
    ];
    (app, state, worker)
}

#[test]
fn the_sheet_sends_one_part_and_one_mask_and_the_rest_stays() {
    let (app, state, _worker) = two_frames();
    state.borrow_mut().views.picked = vec![0, 2];
    app.invoke_sync_open_asked();
    assert!(app.get_sync_open());
    // The frame's masks listed by name under Masks, unchecked as
    // a sync's masks start.
    assert_eq!(heading(&app, "Masks"), 0);
    // Check all, then Uncheck all: every heading follows.
    press_id(&app, "CarryList::check-all");
    assert!(headings(&app).iter().all(|(_, on)| *on == 2));
    press_id(&app, "CarryList::uncheck-all");
    assert!(headings(&app).iter().all(|(_, on)| *on == 0));
    // Light's Exposure, Detail's Dehaze and the one mask, joining.
    tick(&app, "Light: Exposure");
    tick(&app, "Detail: Dehaze");
    tick(&app, "Masks: Face 2");
    press_join(&app, true);
    assert_eq!(heading(&app, "Light"), 1);
    assert_eq!(heading(&app, "Detail"), 1);
    assert_eq!(heading(&app, "Masks"), 1);
    assert_eq!(heading(&app, "Color"), 0);
    app.invoke_sync_applied();
    assert_eq!(
        app.get_status(),
        "synced 1 section and 2 parts onto 1 frame"
    );
    let st = state.borrow();
    let got = &st.sidecars[2].current;
    assert_eq!(got.light.exposure, 0.7);
    assert_eq!(got.detail.dehaze, 0.4);
    // The rest of Light and Detail is the frame's own.
    assert_eq!(got.light.tone.contrast, 1.0);
    assert_eq!(got.light.tone.shadows, -0.5);
    assert_eq!(got.detail.texture, 0.2);
    // The mask joined the frame's own, after it.
    assert_eq!(mask_names(got), ["Mine 1", "Face 2"]);
    assert_eq!(got.adjustments[1].id, 2);
    assert_eq!(st.sidecars[1], Sidecar::default());
}

#[test]
fn a_check_takes_space_and_tab_moves_on_and_return_applies() {
    let (app, state, _worker) = two_frames();
    state.borrow_mut().views.picked = vec![0, 2];
    app.invoke_sync_open_asked();
    let dehaze = |app: &App| {
        let rows = app.get_sync_rows();
        (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .find(|r| r.a11y == "Detail: Dehaze")
            .unwrap()
            .state
    };
    // A click takes the focus as it turns the check over; the keys
    // then turn it back and again.
    tick(&app, "Detail: Dehaze");
    assert_eq!(dehaze(&app), 0);
    assert_eq!(heading(&app, "Detail"), 1);
    press(&app, " ");
    assert_eq!(dehaze(&app), 2);
    assert_eq!(heading(&app, "Detail"), 2);
    press(&app, " ");
    assert_eq!(dehaze(&app), 0);
    // Tab leaves it: Space then turns over whatever is next, not
    // the Dehaze.
    press(&app, Key::Tab);
    press(&app, " ");
    assert_eq!(dehaze(&app), 0);
    // Escape from a check still leaves the sheet.
    tick(&app, "Detail: Texture");
    press(&app, Key::Escape);
    assert!(!app.get_sync_open());
    // Return on a check is the sheet's Apply, the check left as
    // it was.
    app.invoke_sync_open_asked();
    tick(&app, "Detail: Dehaze");
    let before = dehaze(&app);
    press(&app, Key::Return);
    assert!(!app.get_sync_open());
    assert_eq!(dehaze(&app), before);
}

#[test]
fn a_mask_pasted_alone_replaces_the_frames_and_the_sheet_remembers() {
    let (app, state, _worker) = two_frames();
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    assert!(app.get_sync_paste());
    press_id(&app, "CarryList::uncheck-all");
    tick(&app, "Masks: Sky 1");
    app.invoke_sync_applied();
    assert_eq!(
        app.get_status(),
        "pasted 1 section from IMG_0000.CR3 onto 1 frame"
    );
    {
        let st = state.borrow();
        let got = &st.sidecars[2].current;
        // Replacing, as masks always have: the frame's own gone,
        // the Sky carried as its ask, to be found on this frame.
        assert_eq!(mask_names(got), ["Sky 1"]);
        assert_eq!(
            got.adjustments[0].mask.components[0].shape,
            Shape::Sky { picks: Vec::new() }
        );
        assert_eq!(got.light.exposure, 0.0, "no Light was chosen");
        assert_eq!(got.light.tone.shadows, -0.5);
    }
    // The next paste opens on that choice: the masks partly on,
    // the Sky checked, Face not, the rest off.
    app.invoke_paste_asked();
    assert_eq!(heading(&app, "Masks"), 1);
    assert_eq!(heading(&app, "Light"), 0);
    let rows = app.get_sync_rows();
    let checked: Vec<String> = (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .filter(|r| r.kind == 1 && r.state == 2)
        .map(|r| r.a11y.to_string())
        .collect();
    assert_eq!(checked, ["Masks: Sky 1"]);
    press(&app, Key::Escape);
}

/// How the sheet's masks land: 0 replacing the frame's, 1 joining.
fn joins(app: &App) -> i32 {
    let rows = app.get_sync_rows();
    (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .find(|r| r.kind == 2)
        .expect("the masks' line")
        .state
}

/// The headings with Masks alone on, as Copy masks opens a paste.
fn masks_alone() -> Vec<(String, i32)> {
    Section::ALL
        .iter()
        .map(|s| {
            let on = *s == Section::Adjustments;
            (s.title().to_string(), if on { 2 } else { 0 })
        })
        .collect()
}

/// A mask row's Copy masks: a paste's sheet opens on every mask
/// and nothing else, replacing as a first paste does, and carries
/// all of them; a Copy settings after it opens as it always did.
#[test]
fn copy_masks_opens_the_paste_on_every_mask_and_nothing_else() {
    let (app, state, _worker) = two_frames();
    app.invoke_masks_copied();
    assert!(state.borrow().sheets.clipboard.is_some());
    assert!(!app.get_mask_held(), "the mask clipboard is apart");
    app.invoke_select(2);
    app.invoke_paste_asked();
    assert!(app.get_sync_paste());
    assert_eq!(headings(&app), masks_alone());
    assert_eq!(joins(&app), 0);
    app.invoke_sync_applied();
    assert_eq!(
        app.get_status(),
        "pasted 1 section from IMG_0000.CR3 onto 1 frame"
    );
    {
        let st = state.borrow();
        let got = &st.sidecars[2].current;
        assert_eq!(mask_names(got), ["Sky 1", "Face 2"]);
        assert_eq!(got.light.exposure, 0.0, "no Light was chosen");
        assert_eq!(got.light.tone.shadows, -0.5);
        assert_eq!(got.detail.texture, 0.2);
    }
    // The same copy pasted again opens on the masks again.
    app.invoke_paste_asked();
    assert_eq!(headings(&app), masks_alone());
    press(&app, Key::Escape);
    // Copy settings is its own again: the sheet as a first paste
    // opens it, the masks paste not remembered as a choice.
    app.invoke_select(0);
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    assert_eq!(headings(&app), defaults());
    press(&app, Key::Escape);
}

/// Copy masks keeps how the last paste's masks landed, and a
/// choice made on its own sheet is kept for the next paste while
/// the sections stay as that paste had them.
#[test]
fn copy_masks_keeps_the_join_and_leaves_the_sections_chosen() {
    let (app, state, _worker) = two_frames();
    // A paste of Light alone, the masks joining.
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    press_id(&app, "CarryList::uncheck-all");
    tick(&app, "Light: Exposure");
    press_join(&app, true);
    app.invoke_sync_applied();
    // Copy masks: every mask, joining as that paste had them.
    app.invoke_select(0);
    app.invoke_masks_copied();
    app.invoke_select(1);
    app.invoke_paste_asked();
    assert_eq!(headings(&app), masks_alone());
    assert_eq!(joins(&app), 1);
    // Replacing, chosen here, is what the next paste opens with.
    press_join(&app, false);
    app.invoke_sync_applied();
    assert_eq!(
        mask_names(&state.borrow().sidecars[1].current),
        ["Sky 1", "Face 2"]
    );
    app.invoke_select(0);
    app.invoke_copy_asked();
    app.invoke_select(2);
    app.invoke_paste_asked();
    assert_eq!(joins(&app), 0);
    // The sections as the Light paste chose them, not the masks.
    assert_eq!(heading(&app, "Light"), 1);
    assert_eq!(heading(&app, "Masks"), 0);
    press(&app, Key::Escape);
}

/// Copy mask and Paste mask: the one mask, apart from the settings
/// clipboard, joined to every selected frame that lacks it, one
/// step each; onto the open frame through the panel, and undone in
/// one.
#[test]
fn a_mask_copied_alone_joins_the_selection_and_the_open_frame() {
    let (app, state, _worker) = two_frames();
    assert!(!app.get_mask_held());
    app.invoke_mask_copied(1);
    assert!(app.get_mask_held());
    assert!(state.borrow().sheets.clipboard.is_none());
    // Onto the selection, the frame it came from among them,
    // which has it already and is left as it is.
    state.borrow_mut().views.picked = vec![0, 1, 2];
    app.invoke_mask_pasted();
    assert_eq!(
        app.get_status(),
        "pasted 1 section from IMG_0000.CR3 onto 2 frames"
    );
    {
        let st = state.borrow();
        assert_eq!(mask_names(&st.sidecars[1].current), ["Face 2"]);
        let got = &st.sidecars[2].current;
        assert_eq!(mask_names(got), ["Mine 1", "Face 2"]);
        assert_eq!(got.adjustments[1].id, 2);
        assert_eq!(
            got.adjustments[1].mask.components[0].shape,
            Shape::Subject {},
            "the subject to be found on this frame"
        );
        assert_eq!(got.light.tone.shadows, -0.5, "its own look kept");
        assert_eq!(got.light.exposure, 0.0, "nothing but the mask");
        assert_eq!(
            st.sidecars[2].current_label.as_deref(),
            Some("Paste from IMG_0000.CR3")
        );
        assert_eq!(mask_names(&st.develop.edit), ["Sky 1", "Face 2"]);
    }
    // Frame 2's own mask copied, and pasted onto frame 0 alone,
    // open on the panel.
    app.invoke_select(2);
    app.invoke_mask_copied(0);
    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0];
    app.invoke_mask_pasted();
    let names = |state: &Rc<Shared>| mask_names(&state.borrow().develop.edit);
    assert_eq!(names(&state), ["Sky 1", "Face 2", "Mine 1"]);
    assert_eq!(app.get_adjustment_names().row_count(), 4);
    assert_eq!(
        state.borrow().sidecars[0].current_label.as_deref(),
        Some("Paste from IMG_0002.CR3")
    );
    app.invoke_undo();
    assert_eq!(names(&state), ["Sky 1", "Face 2"]);
    assert_eq!(app.get_exposure(), 0.7, "the look before it kept");
}

/// A frame holding as many masks as a picture may takes no more
/// from Paste mask: the 17th would be listed and never drawn.
#[test]
fn paste_mask_leaves_a_frame_at_the_most_masks_as_it_is() {
    let (app, state, _worker) = two_frames();
    let full: Vec<_> = (1..=MAX_LOCALS as u64)
        .map(|id| mask(id, &format!("Mine {id}"), Shape::Subject {}))
        .collect();
    {
        let mut st = state.borrow_mut();
        let mut own = st.sidecars[2].current.clone();
        own.adjustments = full.clone();
        st.sidecars[2].record(own);
    }
    app.invoke_mask_copied(0);
    state.borrow_mut().views.picked = vec![0, 2];
    app.invoke_mask_pasted();
    assert_eq!(app.get_status(), "the 2 frames had these already");
    assert_eq!(state.borrow().sidecars[2].current.adjustments, full);
}

/// Enter on a check of the preset sheet is its Save, once there
/// is a name; with none, nothing.
#[test]
fn enter_on_a_preset_check_saves_once_named() {
    let (app, state, _worker) = two_frames();
    let dir = crate::testing::scratch_dir("preset-enter");
    state.borrow_mut().develop.preset_store = Some(preset::Store::at(dir.clone()));
    app.invoke_preset_save_open();
    assert!(app.get_preset_open());
    let saves = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = saves.clone();
    app.on_preset_saved(move || seen.set(seen.get() + 1));
    let rows = app.get_preset_rows();
    let check = (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .find(|r| r.kind != 2)
        .unwrap()
        .a11y
        .to_string();
    app.set_preset_name("".into());
    tick(&app, &check);
    press(&app, Key::Return);
    assert_eq!(saves.get(), 0, "no name, no save");
    app.set_preset_name("Warm".into());
    tick(&app, &check);
    press(&app, Key::Return);
    assert_eq!(saves.get(), 1);
    drop(app);
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn the_preset_sheet_saves_a_single_part_and_a_single_mask() {
    let (app, state, _worker) = two_frames();
    let dir = crate::testing::scratch_dir("preset-parts");
    state.borrow_mut().develop.preset_store = Some(preset::Store::at(dir.clone()));
    app.invoke_preset_save_open();
    assert!(app.get_preset_open());
    // On to start: only the parts the edit changed.
    let rows = app.get_preset_rows();
    let on: Vec<String> = (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .filter(|r| r.kind == 1 && r.state == 2)
        .map(|r| r.a11y.to_string())
        .collect();
    assert_eq!(
        on,
        [
            "Light: Exposure",
            "Light: Contrast",
            "Detail: Texture",
            "Detail: Dehaze"
        ]
    );
    press_id(&app, "CarryList::uncheck-all");
    tick(&app, "Detail: Dehaze");
    tick(&app, "Masks: Face 2");
    app.set_preset_name("Haze".into());
    app.invoke_preset_saved();
    assert!(
        app.get_status()
            .starts_with("saved Haze (1 section and 1 part)")
    );
    let saved = preset::Store::at(dir.clone())
        .find("Haze")
        .expect("saved")
        .preset;
    assert!(saved.sections == [Section::Adjustments]);
    assert_eq!(saved.parts, [preset::Part::DetailDehaze]);
    assert_eq!(mask_names(&saved.edit), ["Face 2"]);
    // Laid over frame 2's edit: the dehaze, the mask in place of
    // its own, and nothing else.
    let out = saved.applied(&state.borrow().sidecars[2].current);
    assert_eq!(out.detail.dehaze, 0.4);
    assert_eq!(out.detail.texture, 0.2);
    assert_eq!(mask_names(&out), ["Face 2"]);
    crate::testing::remove_dir_retry(&dir);
}

// The relative sync: shifts, set against shift, and what the
// frames look like after.

/// A linear DNG of our own writing, a flat grey card under a light
/// the camera's `gains` neutralize: a raw the engine opens, whose
/// as-shot white is the one asked for.
fn grey_card(path: &Path, gains: [f32; 3]) {
    use greycard_core::raw::{
        Calibration, LevelPattern, Levels, Orientation, Samples, SensorLayout,
    };
    let m = greycard_core::color::WORKING_SPACE
        .from_xyz_matrix()
        .expect("the working space has a matrix");
    let (width, height) = (24usize, 16usize);
    let frame = greycard_core::raw::RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        other_names: Vec::new(),
        width,
        height,
        channels: 3,
        layout: SensorLayout::Linear,
        samples: Samples::U16(Vec::new()),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 3),
            white: LevelPattern::uniform(65535.0, 3),
            black_deltas: None,
            saturation: None,
        },
        as_shot_coefficients: Some(gains.map(f64::from)),
        calibrations: vec![Calibration {
            illuminant: 21,
            color_matrix: m.rows.iter().flatten().map(|v| *v as f32).collect(),
            forward_matrix: None,
        }],
        crop: None,
        orientation: Orientation::Normal,
        shot: Default::default(),
        dng: Default::default(),
    };
    // The card as the camera saw it: grey, under the light's color.
    let pixel = [0.18 / gains[0], 0.18 / gains[1], 0.18 / gains[2]];
    let data: Vec<f32> = (0..width * height).flat_map(|_| pixel).collect();
    let image = greycard_core::CameraImage::from_data(width, height, data).unwrap();
    let mut file = std::fs::File::create(path).unwrap();
    greycard_core::dng::write_linear_dng(&mut file, &frame, &image, gains, &Default::default())
        .unwrap();
}

/// The light `path`'s As shot means, read the way the develop
/// reads it: the whole frame decoded, its own profile.
fn decoded_shot(path: &Path) -> TempTint {
    let frame = greycard_core::decode::decode_path(path).unwrap();
    let profile = greycard_core::color::profile_from_frame(&frame).unwrap();
    greycard_core::color::as_shot_temp_tint(&frame, &profile).unwrap()
}

/// The card's middle as `edit` renders it from the raw at `path`:
/// the develop and the export's finish, sixteen bits, RGB.
fn card(path: &Path, edit: &Edit) -> [f64; 3] {
    let frame = greycard_core::decode::decode_path(path).unwrap();
    let developed =
        greycard_core::develop::develop(&frame, &edit.settings(&greycard_edit_io::Stores::user()))
            .unwrap();
    let image = developed.image;
    let settings = crate::export::Settings {
        format: crate::export::Format::Tiff,
        sharpen: crate::export::Sharpen::Off,
        ..Default::default()
    };
    let rendered = crate::export::render(
        &image,
        edit,
        &greycard_edit_io::Stores::user(),
        (image.width as u32, image.height as u32),
        &settings,
        &Default::default(),
        developed.clip_level,
        None,
        crate::finish::Source::SCENE,
        None,
        None,
    );
    let crate::export::Pixels::Sixteen(px) = rendered.pixels else {
        panic!("a TIFF is sixteen bits");
    };
    let (w, h) = (rendered.width as usize, rendered.height as usize);
    let at = ((h / 2) * w + w / 2) * 3;
    [px[at] as f64, px[at + 1] as f64, px[at + 2] as f64]
}

fn custom(temperature: f64, tint: f64) -> WhiteBalance {
    WhiteBalance::Custom { temperature, tint }
}

/// The warmth a card renders with: red over blue.
fn warmth(c: [f64; 3]) -> f64 {
    c[0] / c[2]
}

fn at(rgb: [f64; 3], other: [f64; 3]) -> f64 {
    (0..3)
        .map(|i| (rgb[i] - other[i]).abs())
        .fold(0.0, f64::max)
}

/// A preset carrying the white balance and the light, as a sync on
/// the sheet's defaults does.
fn white_and_light(edit: &Edit) -> Preset {
    Preset::from_choice(
        "",
        edit,
        &Choice::whole(&[Section::WhiteBalance, Section::Light]),
    )
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn a_shift_moves_each_frame_from_its_own_value_and_a_set_does_not() {
    let mut source = Edit::default();
    let shot = TempTint {
        cct: 4800.0,
        duv: 0.002,
    };
    source.white_balance = custom(5100.0, 0.004);
    source.light.exposure = 0.5;
    let preset = white_and_light(&source);
    let all = SyncShift {
        temperature: true,
        tint: true,
        exposure: true,
    };
    let shift = shift_of(all, &preset, &source, Some(shot));
    // The source's difference from its as-shot, in mired.
    let by = 1e6 / 4800.0 - 1e6 / 5100.0;
    assert!(close(shift.temperature.unwrap(), by));
    assert!((shift.tint.unwrap() - 0.002).abs() < 1e-12);
    assert_eq!(shift.exposure, Some(0.5 - Edit::default().light.exposure));
    assert_eq!(shift.from_kelvin, Some(4800.0));
    assert_eq!(
        shift.words(),
        "Temperature +12 mired, Tint +0.002, Exposure +0.50 EV"
    );
    let sheet = shift.sheet_words();
    assert!(sheet.contains("+300 K at this frame's 4800 K"), "{sheet}");
    // Nothing asked, nothing shifted; a section left behind is not
    // shifted either.
    assert!(shift_of(SyncShift::default(), &preset, &source, Some(shot)).is_none());
    let light_only = Preset::from_choice("", &source, &Choice::whole(&[Section::Light]));
    let s = shift_of(all, &light_only, &source, Some(shot));
    assert_eq!((s.temperature, s.tint), (None, None));
    assert!(s.exposure.is_some());
    // The source's As shot unread: its move cannot be known.
    assert_eq!(shift_of(all, &preset, &source, None).temperature, None);

    let a = Edit {
        white_balance: custom(4200.0, 0.0),
        light: greycard_edit::Light {
            exposure: -0.25,
            ..Default::default()
        },
        ..Default::default()
    };
    let out = shifted(&preset, &shift, &a, None);
    let WhiteBalance::Custom { temperature, tint } = out.edit.white_balance else {
        panic!("a shifted custom white is custom");
    };
    // The same mired move from its own light: fewer kelvin than the
    // source's 300, the frame being warmer.
    assert!(close(1e6 / 4200.0 - 1e6 / temperature, by), "{temperature}");
    assert!(temperature - 4200.0 < 300.0);
    assert!((tint - 0.002).abs() < 1e-12);
    assert_eq!(out.edit.light.exposure, -0.25 + shift.exposure.unwrap());
    // Temperature shifted, Tint set: the source's tint.
    let temp_only = shift_of(
        SyncShift {
            temperature: true,
            ..Default::default()
        },
        &preset,
        &source,
        Some(shot),
    );
    let out = shifted(&preset, &temp_only, &a, None);
    assert_eq!(
        out.edit.white_balance,
        custom(kelvin_moved(4200.0, by), 0.004)
    );
    assert_eq!(
        out.edit.light.exposure, source.light.exposure,
        "exposure set"
    );
    // An As shot frame becomes custom at its own as-shot light,
    // moved.
    let b = Edit::default();
    let own = TempTint {
        cct: 3300.0,
        duv: -0.001,
    };
    let out = shifted(&preset, &shift, &b, Some(own));
    let WhiteBalance::Custom { temperature, tint } = out.edit.white_balance else {
        panic!("As shot shifted is custom");
    };
    assert!(close(temperature, kelvin_moved(3300.0, by)));
    assert!((tint - 0.001).abs() < 1e-12);
    // One whose raw will not say keeps its As shot.
    let out = shifted(&preset, &shift, &b, None);
    assert_eq!(out.edit.white_balance, WhiteBalance::AsShot);
    // The sliders' ends hold.
    let hot = Edit {
        white_balance: custom(11900.0, 0.049),
        light: greycard_edit::Light {
            exposure: 4.8,
            ..Default::default()
        },
        ..Default::default()
    };
    let out = shifted(&preset, &shift, &hot, None);
    assert_eq!(out.edit.white_balance, custom(12000.0, 0.05));
    assert_eq!(out.edit.light.exposure, 5.0);
}

/// A mired is more kelvin the cooler the light: the one shift
/// moves a warm frame fewer kelvin than a cool one.
#[test]
fn a_temperature_shift_is_the_same_mired_at_every_light() {
    let by = 1e6 / 4800.0 - 1e6 / 5100.0;
    let at = |k: f64| kelvin_moved(k, by) - k;
    assert!((at(4800.0) - 300.0).abs() < 1e-6);
    assert!(at(3000.0) < at(4800.0) && at(4800.0) < at(6500.0));
    assert!((at(3000.0) - 114.5).abs() < 0.5, "{}", at(3000.0));
    // Back by as much is where it was.
    assert!(close(kelvin_moved(kelvin_moved(3000.0, by), -by), 3000.0));
    assert_eq!(kelvin_moved(3000.0, 0.0), 3000.0);
}

/// A source at As shot moved nowhere: whatever the other field's
/// choice, every frame's white is its own, custom or As shot, and
/// a frame's raw is never read for it.
#[test]
fn a_source_at_as_shot_leaves_every_frames_white_whatever_is_set() {
    let mut source = Edit::default();
    source.light.exposure = 0.5;
    let preset = white_and_light(&source);
    let custom_frame = Edit {
        white_balance: custom(4200.0, 0.0),
        ..Default::default()
    };
    let as_shot_frame = Edit::default();
    let read = TempTint {
        cct: 3300.0,
        duv: -0.001,
    };
    for (temperature, tint) in [(true, false), (false, true), (true, true)] {
        let modes = SyncShift {
            temperature,
            tint,
            exposure: true,
        };
        let shift = shift_of(modes, &preset, &source, None);
        assert!(!shift.reads_targets(&preset), "{modes:?}");
        for frame in [&custom_frame, &as_shot_frame] {
            let out = shifted(&preset, &shift, frame, Some(read));
            assert_eq!(out.edit.white_balance, frame.white_balance, "{modes:?}");
            assert_eq!(out.edit.light.exposure, 0.5, "the exposure still shifts");
        }
    }
    // Both set: the source's As shot, as a sync always laid it.
    let out = shifted(
        &preset,
        &shift_of(SyncShift::default(), &preset, &source, None),
        &custom_frame,
        None,
    );
    assert_eq!(out.edit.white_balance, WhiteBalance::AsShot);
}

/// Lightroom's Quick Develop, end to end: a batch synced from a
/// frame warmed 300 K and brightened half a stop from its as-shot
/// goes as many mired warmer and half a stop brighter each, from
/// where each was, and the cards they render say so; where a set
/// would have made the two custom frames' cards one.
#[test]
fn a_relative_sync_moves_each_frames_rendered_card_by_the_shift() {
    let dir = crate::testing::scratch_dir("relative-sync");
    let files: Vec<PathBuf> = ["S", "A", "B", "C"]
        .iter()
        .map(|n| dir.join(format!("{n}.dng")))
        .collect();
    grey_card(&files[0], [2.0, 1.0, 1.6]);
    grey_card(&files[1], [2.0, 1.0, 1.6]);
    grey_card(&files[2], [2.0, 1.0, 1.6]);
    grey_card(&files[3], [1.6, 1.0, 2.2]);
    let source_shot = decoded_shot(&files[0]);
    let c_shot = decoded_shot(&files[3]);
    let app = window(4);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, files.clone());
    // The light read without a sample is the develop's own, on the
    // pool as on the window's thread.
    let read = frame_shot(&state.borrow(), 3, &Edit::default().camera);
    assert_eq!(read.map(|t| (t.cct, t.duv)), Some((c_shot.cct, c_shot.duv)));
    let pooled = read_whites(&[(3, files[3].clone(), Edit::default().camera)]);
    assert_eq!(pooled.get(&3).copied().flatten(), read);
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.placement = greycard_edit::Placement::Folder;
        st.sidecars[1].record(Edit {
            white_balance: custom(4200.0, 0.0),
            ..Default::default()
        });
        st.sidecars[2].record(Edit {
            white_balance: custom(5000.0, 0.0),
            ..Default::default()
        });
    }
    let before: Vec<Edit> = (0..4)
        .map(|c| state.borrow().sidecars[c].current.clone())
        .collect();
    app.invoke_select(0);
    // The source warmed 300 K from its as-shot and half a stop up.
    app.set_as_shot(false);
    app.set_temperature((source_shot.cct + 300.0) as f32);
    app.set_tint(source_shot.duv as f32);
    app.set_exposure(0.5);
    state.borrow_mut().views.picked = vec![0, 1, 2, 3];
    app.set_set_count(4);
    app.set_sync_shift_temperature(true);
    app.set_sync_shift_tint(true);
    app.set_sync_shift_exposure(true);
    app.invoke_sync_open_asked();
    assert!(app.get_sync_open());
    assert!(app.get_sync_shift_shown());
    let words = app.get_sync_shift_words().to_string();
    assert!(words.contains("mired (+300 K at this frame's"), "{words}");
    assert!(words.contains("Exposure +0.50 EV"), "{words}");
    app.invoke_sync_applied();
    assert!(!app.get_sync_open());
    let status = app.get_status().to_string();
    assert!(
        status.contains("each shifted Temperature +") && status.contains("mired"),
        "{status}"
    );

    let after: Vec<Edit> = (0..4)
        .map(|c| state.borrow().sidecars[c].current.clone())
        .collect();
    // Each from its own by the source's mired move (300 K at its
    // own light, the f32 of the panel's slider aside) and half a
    // stop.
    let temperature = |e: &Edit| match e.white_balance {
        WhiteBalance::Custom { temperature, .. } => temperature,
        WhiteBalance::AsShot | WhiteBalance::Offset { .. } => panic!("As shot after a shift"),
    };
    let by = 1e6 / source_shot.cct - 1e6 / (source_shot.cct + 300.0);
    for (c, own) in [(1, 4200.0), (2, 5000.0), (3, c_shot.cct)] {
        let want = kelvin_moved(own, by);
        assert!((temperature(&after[c]) - want).abs() < 0.05, "{c}");
    }
    for e in &after[1..] {
        assert_eq!(e.light.exposure, 0.5);
    }
    // Geometry and the rest are each frame's own.
    for c in 1..4 {
        assert_eq!(after[c].geometry, before[c].geometry);
        assert_eq!(after[c].adjustments, before[c].adjustments);
    }

    // What the cards render. The As shot frame first: custom at its
    // own as-shot light renders as As shot did, so the shift is the
    // only move.
    let mut c_at_shot = before[3].clone();
    c_at_shot.white_balance = custom(c_shot.cct, c_shot.duv);
    let c_before = card(&files[3], &before[3]);
    assert!(
        at(card(&files[3], &c_at_shot), c_before) <= 40.0,
        "custom at the as-shot light renders as As shot"
    );
    let c_after = card(&files[3], &after[3]);
    assert!(
        warmth(c_after) > warmth(c_before) * 1.01,
        "{c_before:?} {c_after:?}"
    );
    // Every target warmer and brighter than it was, by the shift:
    // each renders as its own light the same mired up, half a stop
    // up.
    for c in 1..4 {
        let (was, now) = (card(&files[c], &before[c]), card(&files[c], &after[c]));
        assert!(warmth(now) > warmth(was), "{c}: {was:?} {now:?}");
        let mut moved = before[c].clone();
        moved.white_balance = after[c].white_balance.clone();
        let same_light = card(&files[c], &moved);
        assert!(now[1] > same_light[1] * 1.05, "{c}: half a stop up");
    }
    // The two custom frames shot under one light stay as far apart
    // as they were; set, they would have rendered one card.
    let (a, b) = (card(&files[1], &after[1]), card(&files[2], &after[2]));
    assert!(warmth(a) < warmth(b), "{a:?} {b:?}");
    let set = white_and_light(&after[0]);
    let set_a = card(&files[1], &set.edit);
    let set_b = card(&files[2], &set.edit);
    assert_eq!(set_a, set_b);
    assert!(at(a, b) > 100.0, "{a:?} {b:?}");
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn the_shift_choices_grey_out_with_their_section_and_hide_for_a_paste() {
    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0, 1, 2];
    app.set_set_count(3);
    app.invoke_sync_open_asked();
    assert!(app.get_sync_shift_shown());
    assert!(app.get_sync_shift_white() && app.get_sync_shift_light());
    // Only the white balance unchecked: its two grey, Exposure not.
    choose(&app, |s| s.syncs_by_default() && s != Section::WhiteBalance);
    assert!(!app.get_sync_shift_white());
    assert!(app.get_sync_shift_light());
    choose(&app, |s| s == Section::WhiteBalance);
    assert!(app.get_sync_shift_white());
    assert!(!app.get_sync_shift_light());
    // A choice made says what a frame moves by.
    app.set_sync_shift_temperature(true);
    app.invoke_sync_shift_changed();
    let words = app.get_sync_shift_words().to_string();
    assert!(words.contains("Temperature +0 mired"), "{words}");
    press(&app, Key::Escape);
    assert!(!app.get_sync_open());
    // A paste's sheet has none.
    app.invoke_copy_asked();
    app.invoke_select(1);
    state.borrow_mut().views.picked = vec![1, 2];
    app.invoke_paste_asked();
    assert!(app.get_sync_open() && app.get_sync_paste());
    assert!(!app.get_sync_shift_shown());
}

/// The develop panel's head offers the sync sheet over the
/// selection, with as many frames named as the sheet goes onto.
#[test]
fn the_panels_sync_button_opens_the_sheet_on_the_selection() {
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(6));
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    assert_eq!(
        crate::testing::count_id(&app, "PanelHead::sync-button"),
        0,
        "one frame, no button"
    );
    with_modifier(&app, Key::Control, || {
        let (x, y) = strip_cell(&app, 3);
        click(&app, x, y);
        let (x, y) = strip_cell(&app, 4);
        click(&app, x, y);
    });
    assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
    assert_eq!(crate::testing::count_id(&app, "PanelHead::sync-button"), 1);
    let (at, size) = crate::testing::by_id(&app, "PanelHead::sync-button");
    // In the panel's head, on the right, above the tabs.
    assert!(at.x > 1500.0 - crate::panel::sizes::RIGHT, "{at:?}");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(app.get_sync_open());
    assert!(!app.get_sync_paste());
    assert_eq!(
        app.get_sync_text(),
        "From IMG_0001.CR3 onto 2 other frames."
    );
    // The frame menu names what it does to the selection.
    app.invoke_frame_menu_asked(-1);
    assert_eq!(app.get_menu_count(), 3);
    assert_eq!(app.get_menu_paste_count(), 0, "nothing copied");
}

/// A middle click on the panel's Sync button syncs at once, the
/// sheet never up, on the choice the last sync took (here Light
/// alone, so the color stays); a left click still opens the sheet.
#[test]
fn a_middle_click_on_the_panels_sync_syncs_without_the_sheet() {
    use slint::platform::PointerEventButton;
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, folder(6));
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    with_modifier(&app, Key::Control, || {
        let (x, y) = strip_cell(&app, 3);
        click(&app, x, y);
        let (x, y) = strip_cell(&app, 4);
        click(&app, x, y);
    });
    assert_eq!(chosen_frames(&state.borrow()), vec![1, 3, 4]);
    app.set_exposure(0.6);
    app.set_vibrance(0.3);
    app.invoke_view_changed();
    app.invoke_sync_open_asked();
    choose(&app, |s| s == Section::Light);
    app.invoke_sync_applied();
    crate::rows::land_pending(&state, &app, &worker);
    let edits = || {
        let st = state.borrow();
        [3, 4].map(|f| {
            let e = &st.sidecars[f].current;
            (e.light.exposure, e.color.vibrance)
        })
    };
    assert_eq!(edits(), [(0.6, 0.0); 2], "the sheet's sync, Light alone");

    app.set_exposure(-0.4);
    app.invoke_view_changed();
    let (at, size) = crate::testing::by_id(&app, "PanelHead::sync-button");
    let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
    crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
    assert!(!app.get_sync_open(), "no sheet");
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(edits(), [(-0.4, 0.0); 2], "Light again, and only Light");
    assert_eq!(app.get_status(), "synced 1 section onto 2 frames");

    // The left click is the sheet's, as before.
    click(&app, x, y);
    assert!(app.get_sync_open());
}

/// Frame `c`'s line given an exposure and a vibrance of its own.
fn own_edit(state: &Rc<Shared>, c: usize, exposure: f32, vibrance: f32) {
    let mut st = state.borrow_mut();
    let mut e = st.line(c).unwrap().current.clone();
    e.light.exposure = exposure;
    e.color.vibrance = vibrance;
    assert!(st.line_mut(c).unwrap().record(e));
}

/// The status line, or the words waiting on the develop the sync
/// onto the open frame asked for.
fn said(state: &Rc<Shared>, app: &App) -> String {
    match &state.borrow().develop.status_after_develop {
        Some((_, said)) => said.clone(),
        None => app.get_status().to_string(),
    }
}

/// A middle click on a frame of the set that is not open syncs it
/// onto the rest, the open frame among them through its panel, on
/// the last sync's choice (Light alone); the set and the open frame
/// stay, the source takes nothing, and an undo takes the open
/// frame's step back.
#[test]
fn a_middle_click_on_a_frame_syncs_it_onto_the_rest_of_the_set() {
    use slint::platform::PointerEventButton;
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, folder(6));
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    with_modifier(&app, Key::Control, || {
        let (x, y) = strip_cell(&app, 3);
        click(&app, x, y);
        let (x, y) = strip_cell(&app, 4);
        click(&app, x, y);
    });
    app.set_exposure(0.6);
    app.invoke_view_changed();
    app.invoke_sync_open_asked();
    choose(&app, |s| s == Section::Light);
    app.invoke_sync_applied();
    crate::rows::land_pending(&state, &app, &worker);
    own_edit(&state, 3, -1.0, 0.5);
    let source_steps = state.borrow().sidecars[3].history.len();

    let (x, y) = strip_cell(&app, 3);
    crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
    assert!(!app.get_sync_open(), "no sheet");
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(chosen_frames(&st), vec![1, 3, 4], "the set stays");
        assert_eq!(st.current, Some(1), "the open frame stays");
        let e = &st.sidecars[4].current;
        assert_eq!((e.light.exposure, e.color.vibrance), (-1.0, 0.0));
        assert_eq!(st.sidecars[1].current.light.exposure, -1.0);
        assert_eq!(
            st.sidecars[3].history.len(),
            source_steps,
            "no step on the source"
        );
        assert_eq!(st.sidecars[3].current.color.vibrance, 0.5);
        assert_eq!(
            st.sidecars[4].current_label.as_deref(),
            Some("Sync from IMG_0003.CR3")
        );
    }
    assert_eq!(app.get_selected(), 1);
    assert_eq!(
        (app.get_exposure(), app.get_vibrance()),
        (-1.0, 0.0),
        "the panel"
    );
    assert_eq!(history_rows(&app)[0], "Sync from IMG_0003.CR3");
    assert_eq!(said(&state, &app), "synced 1 section onto 2 frames");
    // The open frame's step is undone as any is.
    app.invoke_undo();
    assert_eq!(app.get_exposure(), 0.6);
    assert_eq!(state.borrow().sidecars[1].current.light.exposure, 0.6);
}

/// A middle click on a frame outside the set lands on the whole
/// set, here the open frame alone, which keeps its place; on the
/// one frame the set holds there is nothing to sync onto. A middle
/// press let go on another frame, a tool in hand, and a sheet up
/// each sync nothing.
#[test]
fn a_middle_click_outside_the_set_syncs_onto_the_set() {
    use slint::platform::PointerEventButton;
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, folder(6));
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    own_edit(&state, 2, -1.0, 0.0);
    let middle = |c: usize| {
        let (x, y) = strip_cell(&app, c);
        crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
        crate::rows::land_pending(&state, &app, &worker);
    };
    let untouched = || {
        assert_eq!(app.get_exposure(), 0.0);
        assert!(state.borrow().sidecars[1].history.is_empty());
    };

    // Pressed on frame 2 and let go on frame 3: no click.
    app.set_status("".into());
    crate::testing::drag_with(
        &app,
        PointerEventButton::Middle,
        &[strip_cell(&app, 2), strip_cell(&app, 3)],
    );
    crate::rows::land_pending(&state, &app, &worker);
    untouched();
    assert_eq!(app.get_status(), "");
    // On the frame the set is: nothing to sync onto.
    middle(1);
    assert_eq!(
        app.get_status(),
        "choose the frames to sync onto: Ctrl+click or Shift+click"
    );
    untouched();
    // With a tool in hand.
    app.set_level_mode(true);
    middle(2);
    assert_eq!(app.get_status(), "put the tool down first (Esc), then sync");
    untouched();
    app.set_level_mode(false);
    // Under a sheet.
    app.set_status("".into());
    app.set_sync_open(true);
    middle(2);
    untouched();
    assert_eq!(app.get_status(), "");
    app.set_sync_open(false);

    middle(2);
    assert_eq!(app.get_exposure(), -1.0, "onto the open frame");
    let st = state.borrow();
    assert_eq!(chosen_frames(&st), vec![1]);
    assert_eq!(st.current, Some(1));
    assert_eq!(st.sidecars[1].current.light.exposure, -1.0);
    drop(st);
    let said = said(&state, &app);
    assert!(
        said.starts_with("synced ") && said.ends_with(" onto 1 frame"),
        "{said}"
    );
}

/// A middle release inside a left press is no middle click, and
/// neither is one after the pointer left the window mid-press.
#[test]
fn a_middle_release_in_a_left_press_or_after_a_cancel_syncs_nothing() {
    use slint::platform::PointerEventButton;
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, folder(6));
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    own_edit(&state, 2, -1.0, 0.0);
    let position = {
        let (x, y) = strip_cell(&app, 2);
        slint::LogicalPosition::new(x, y)
    };
    let send = |events: &[WindowEvent]| {
        for e in events {
            app.window().dispatch_event(e.clone());
        }
        crate::rows::land_pending(&state, &app, &worker);
    };
    let pressed = |button| WindowEvent::PointerPressed { position, button };
    let released = |button| WindowEvent::PointerReleased { position, button };
    // Left and middle crossed: the left click opens frame 2.
    send(&[
        WindowEvent::PointerMoved { position },
        pressed(PointerEventButton::Middle),
        pressed(PointerEventButton::Left),
        released(PointerEventButton::Middle),
        released(PointerEventButton::Left),
    ]);
    assert_eq!(
        state.borrow().sidecars[1].current.light.exposure,
        0.0,
        "no sync"
    );
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    assert_eq!(state.borrow().current, Some(1));
    // A middle press, the pointer out of the window, a release.
    send(&[
        WindowEvent::PointerMoved { position },
        pressed(PointerEventButton::Middle),
        WindowEvent::PointerExited,
    ]);
    send(&[
        WindowEvent::PointerMoved { position },
        released(PointerEventButton::Middle),
    ]);
    assert_eq!(app.get_exposure(), 0.0, "no sync");
    assert!(state.borrow().sidecars[1].history.is_empty());
}

/// The last choice a middle click takes, set as a sheet's apply
/// leaves it.
fn last_is(state: &Rc<Shared>, on: impl Fn(Section) -> bool) {
    state.borrow_mut().sheets.sync_last = Some(carry::Checks::new(Vec::new(), on));
}

/// A slider moved on the open frame and not yet saved (the save
/// timer still out) reaches its sidecar on disk when a middle click
/// syncs onto it, whether the sync moved it or found it had the
/// sections already.
#[test]
fn a_middle_click_onto_the_open_frame_writes_what_its_panel_held() {
    let app = window(3);
    let dir = crate::testing::scratch_dir("middle-unsaved");
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<PathBuf> = (0..3)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    let (state, worker) = state_for(&app, files.clone());
    state.borrow_mut().window.write_sidecars = true;
    last_is(&state, |s| s == Section::Light);
    own_edit(&state, 1, 0.5, 0.0);
    app.invoke_select(0);
    let on_disk = || {
        Sidecar::load(&files[0])
            .unwrap()
            .map(|s| (s.current.light.exposure, s.current.color.vibrance))
    };
    // The same Light as the source, unsaved: nothing moves.
    app.set_exposure(0.5);
    app.invoke_view_changed();
    assert_ne!(on_disk(), Some((0.5, 0.0)), "the timer has not saved it");
    app.invoke_frame_sync_asked(1);
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(app.get_status(), "the other 1 frame had these already");
    assert_eq!(on_disk(), Some((0.5, 0.0)), "written all the same");
    // A Color slider unsaved, and Light from another value: the
    // frame moves, and both are on disk.
    own_edit(&state, 1, -0.5, 0.0);
    app.set_vibrance(0.3);
    app.invoke_view_changed();
    app.invoke_frame_sync_asked(1);
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(app.get_exposure(), -0.5);
    assert_eq!(on_disk(), Some((-0.5, 0.3)));
    crate::testing::remove_scratch(state, &dir);
}

/// A middle click on a frame sets, whatever shift the sheet was
/// left on: twice over, the targets are the source's value and the
/// second click takes no step.
#[test]
fn a_second_middle_click_changes_nothing_whatever_the_sheets_shift() {
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    last_is(&state, |s| s == Section::Light);
    app.set_sync_shift_exposure(true);
    own_edit(&state, 1, 1.0, 0.0);
    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0, 2];
    for _ in 0..2 {
        app.invoke_frame_sync_asked(1);
        crate::rows::land_pending(&state, &app, &worker);
    }
    let st = state.borrow();
    assert_eq!(
        st.sidecars[2].current.light.exposure, 1.0,
        "set, not shifted"
    );
    assert_eq!(st.sidecars[0].current.light.exposure, 1.0);
    assert_eq!(st.sidecars[2].history.len(), 1, "one step");
    drop(st);
    assert_eq!(app.get_exposure(), 1.0);
    assert_eq!(app.get_status(), "the other 2 frames had these already");
}

/// The Sync button's middle click sets as a frame's does, whatever
/// shift the sheet was left on: twice over, the target is the open
/// frame's value, one step, and the remembered choice and shift
/// are as they were. The sheet's own apply still shifts.
#[test]
fn the_buttons_middle_click_sets_and_the_sheet_still_shifts() {
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    last_is(&state, |s| s == Section::Light);
    app.set_sync_shift_exposure(true);
    own_edit(&state, 2, 0.5, 0.0);
    app.invoke_select(0);
    state.borrow_mut().views.picked = vec![0, 2];
    app.set_exposure(1.0);
    app.invoke_view_changed();
    let last = state.borrow().sheets.sync_last.clone();
    for _ in 0..2 {
        app.invoke_sync_quick_asked();
        crate::rows::land_pending(&state, &app, &worker);
    }
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[2].current.light.exposure, 1.0,
            "set, not shifted"
        );
        assert_eq!(st.sidecars[2].history.len(), 2, "its own and one sync");
        assert_eq!(st.sheets.sync_last, last, "the last choice unchanged");
    }
    assert!(app.get_sync_shift_exposure(), "the shift unchanged");
    assert_eq!(app.get_status(), "the other 1 frame had these already");

    // The sheet, Light shifted: the target moves by the open
    // frame's +1 EV from its own value.
    app.invoke_sync_open_asked();
    choose(&app, |s| s == Section::Light);
    app.invoke_sync_applied();
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().sidecars[2].current.light.exposure, 2.0);
}

/// A middle click onto the open frame with the crop up leaves the
/// crop up and the frame's own crop, as a paste does; one with a
/// mask chosen whose masks the sync replaces with none goes back to
/// the global look, as a paste does.
#[test]
fn a_middle_click_onto_the_open_frame_under_the_crop_and_a_mask_does_as_a_paste() {
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    let crop = greycard_edit::geometry::Crop {
        x: 0.1,
        y: 0.1,
        w: 0.8,
        h: 0.8,
    };
    {
        let mut st = state.borrow_mut();
        let mut own = Edit::default();
        own.geometry.crop = Some(crop);
        own.adjustments = vec![mask(1, "Sky 1", Shape::Subject {})];
        st.sidecars[0].record(own);
    }
    own_edit(&state, 1, -1.0, 0.0);
    app.invoke_select(0);
    // The crop up: Light goes, the crop stays as it was.
    last_is(&state, |s| s == Section::Light);
    app.set_crop_mode(true);
    app.invoke_crop_toggled();
    let check_crop = |what: &str| {
        assert!(app.get_crop_mode(), "{what}: the crop still up");
        assert_eq!(app.get_exposure(), -1.0, "{what}");
        let st = state.borrow();
        assert_eq!(st.sidecars[0].current.geometry.crop, Some(crop), "{what}");
    };
    app.invoke_frame_sync_asked(1);
    crate::rows::land_pending(&state, &app, &worker);
    check_crop("the sync");
    app.invoke_undo();
    assert_eq!(app.get_exposure(), 0.0);
    copy_settings_of(&mut state.borrow_mut(), &app, 1);
    app.invoke_paste_asked();
    app.invoke_sync_applied();
    check_crop("the paste");
    app.set_crop_mode(false);
    app.invoke_crop_toggled();

    // The mask chosen, the masks replaced by the source's none.
    app.invoke_undo();
    last_is(&state, |s| s == Section::Light || s == Section::Adjustments);
    app.invoke_target_changed(1);
    assert_eq!(state.borrow().develop.target, Some(0));
    let check_mask = |what: &str| {
        let st = state.borrow();
        assert_eq!(st.develop.target, None, "{what}: back to the global look");
        assert!(st.sidecars[0].current.adjustments.is_empty(), "{what}");
        assert_eq!(st.develop.edit.light.exposure, -1.0, "{what}");
    };
    app.invoke_frame_sync_asked(1);
    crate::rows::land_pending(&state, &app, &worker);
    check_mask("the sync");
    app.invoke_undo();
    app.invoke_target_changed(1);
    assert_eq!(state.borrow().develop.target, Some(0));
    app.invoke_paste_asked();
    app.invoke_sync_applied();
    check_mask("the paste");
}

/// In Cull drawn developed a middle click onto the open frame
/// leaves Cull for Develop, as a paste onto it does: the panel takes
/// the frame's new state.
#[test]
fn a_middle_click_onto_the_open_frame_in_developed_cull_leaves_it_as_a_paste_does() {
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    last_is(&state, |s| s == Section::Light);
    own_edit(&state, 1, -1.0, 0.0);
    app.invoke_select(0);
    crate::panel::cull::enter_developed(&mut state.borrow_mut(), &app);
    assert!(state.borrow().culling.developed.is_some());
    app.invoke_frame_sync_asked(1);
    crate::rows::land_pending(&state, &app, &worker);
    assert!(
        state.borrow().culling.developed.is_none(),
        "the sync left Cull"
    );
    assert_eq!(app.get_exposure(), -1.0);

    app.invoke_undo();
    crate::panel::cull::enter_developed(&mut state.borrow_mut(), &app);
    copy_settings_of(&mut state.borrow_mut(), &app, 1);
    app.invoke_paste_asked();
    app.invoke_sync_applied();
    assert!(
        state.borrow().culling.developed.is_none(),
        "the paste left Cull"
    );
    assert_eq!(app.get_exposure(), -1.0);
}

/// A middle click on a cell of the grid syncs as one in the strip.
#[test]
fn a_middle_click_in_the_grid_syncs_as_in_the_strip() {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::PointerEventButton;
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, folder(6));
    let (x, y) = strip_cell(&app, 1);
    click(&app, x, y);
    with_modifier(&app, Key::Control, || {
        let (x, y) = strip_cell(&app, 3);
        click(&app, x, y);
    });
    own_edit(&state, 0, -1.0, 0.0);
    app.set_grid_open(true);
    slint::platform::update_timers_and_animations();
    let cell = ElementHandle::find_by_element_id(&app, "GridSheet::ct")
        .next()
        .expect("a cell on the sheet");
    let (at, size) = (cell.absolute_position(), cell.size());
    let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
    crate::testing::click_with(&app, PointerEventButton::Middle, x, y);
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(chosen_frames(&st), vec![1, 3]);
    assert_eq!(st.current, Some(1));
    assert_eq!(st.sidecars[3].current.light.exposure, -1.0);
    assert_eq!(st.sidecars[1].current.light.exposure, -1.0);
    assert_eq!(
        st.sidecars[3].current_label.as_deref(),
        Some("Sync from IMG_0000.CR3")
    );
}

/// The window's hold for a paste onto 300 frames, timed: the frames
/// under `GREYCARD_BULK_DIR` (a folder on the disk to be measured),
/// their sidecars beside them. Run by hand with `--ignored
/// --nocapture`.
#[test]
#[ignore]
fn time_a_paste_onto_300_frames() {
    let Some(dir) = std::env::var_os("GREYCARD_BULK_DIR").map(PathBuf::from) else {
        return;
    };
    const N: usize = 300;
    for write in [false, true] {
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("the folder");
        let files: Vec<PathBuf> = (0..N)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(N);
        let (state, _worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = write;
        app.invoke_select(0);
        app.set_exposure(0.8);
        with_control(&app, || press(&app, "c"));
        state.borrow_mut().views.picked = (0..N).collect();
        with_control(&app, || press(&app, "v"));
        assert!(app.get_sync_open());
        choose(&app, |s| s == Section::Light);
        crate::panel::saves::tests::HOLD.set(true);
        let t = std::time::Instant::now();
        app.invoke_sync_applied();
        let held = t.elapsed();
        crate::panel::saves::tests::HOLD.set(false);
        // The batch's thread let finish, then its landing on the
        // window's thread timed alone.
        let t = std::time::Instant::now();
        let landing = loop {
            let l = std::time::Instant::now();
            let zero = Some(std::time::Duration::ZERO);
            if crate::panel::saves::finish(&mut state.borrow_mut(), zero) {
                break l.elapsed();
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        println!(
            "paste onto {} frames, sidecars {}: window held {:.1} ms, the batch's \
             thread {:.1} ms, its landing {:.1} ms",
            N - 1,
            if write { "written" } else { "off" },
            held.as_secs_f64() * 1e3,
            t.elapsed().as_secs_f64() * 1e3,
            landing.as_secs_f64() * 1e3
        );
        assert!(
            app.get_status().starts_with("pasted"),
            "{}",
            app.get_status()
        );
    }
    crate::testing::remove_dir_retry(&dir);
}
