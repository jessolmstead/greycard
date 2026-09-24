//! Sync settings: the current frame's chosen sections laid over the
//! rest of the browser's selection, each frame's history taking it
//! as one step and each sidecar written where the frame keeps it.
//!
//! The sheet is the preset sheet's twin and the work is a preset's:
//! `greycard_edit::sync_into` builds a `Preset` from the current
//! frame and applies it, so there is no second copy of what a
//! section is. Two things are the sync's own, because a sync reads
//! the frames it lands on and a preset cannot: a named camera
//! profile goes only to frames of the body it was made for, and a
//! frame whose learned-denoiser blend was still waiting on its ISO
//! is given it before the sync makes its edit no longer the default.

use crate::panel::browser::{
    chosen_frames, file_name, migrate_frame, show_badges, show_thumb, thumb_turns,
};
use crate::panel::edit::{read_edit, save_edit, write_sidecar};
use crate::panel::history::show_history;
use crate::*;
use greycard_edit::camera::{self, ProfileChoice};

/// The frames a sync goes onto: the set, less the frame it comes
/// from.
pub(crate) fn sync_targets(st: &State) -> Vec<usize> {
    selection::others(&chosen_frames(st), st.current)
}

/// What the sheet says it will do.
fn sync_words(st: &State) -> String {
    let n = sync_targets(st).len();
    let from = st
        .current
        .map(|c| file_name(&st.files[c]))
        .unwrap_or_default();
    format!(
        "From {from} onto {n} other frame{}.",
        if n == 1 { "" } else { "s" }
    )
}

/// The sheet's word for a section: its panel title, and for the
/// masks what ticking them does to the frames synced onto.
fn sync_title(s: Section) -> String {
    match s {
        Section::Adjustments => "Adjustments (replaces masks)".to_string(),
        s => s.title().to_string(),
    }
}

/// The current frame's edit as a sync reads it. Outside culling the
/// panel is the frame's, and what it holds is recorded and saved
/// first, as a preset's apply does, so the frame synced from has the
/// state that went out in its own history. In culling the panel is
/// not the frame's and the sidecar is the truth.
fn sync_source(st: &mut State, app: &App) -> Option<Edit> {
    let c = st.current?;
    if st.cull.is_some() {
        return Some(st.sidecars[c].current.clone());
    }
    let edit = read_edit(app, &st.edit, st.target);
    save_edit(st, edit.clone());
    Some(edit)
}

/// A raw frame's make, model and ISO, read from its metadata without
/// a pixel decoded; `None` for a picture that is not a raw, or a file
/// that will not say.
fn probe(st: &State, file: usize) -> Option<greycard_core::decode::Probe> {
    let path = st.files.get(file)?;
    if greycard_core::picture::is_picture_path(path) {
        return None;
    }
    greycard_core::decode::probe_path(path)
        .inspect_err(|e| tracing::warn!("{}: metadata: {e}", file_name(path)))
        .ok()
}

/// Whether a camera profile may go to a frame of `body` (make and
/// model). A profile the directory has not got, or one that names no
/// camera, cannot be checked and goes, as it would in a preset; one
/// made for a camera goes only to that camera's frames, and a frame
/// that cannot say what took it does not get it.
pub(crate) fn profile_fits(entry: Option<&camera::Entry>, body: Option<(&str, &str)>) -> bool {
    let Some(entry) = entry else {
        return true;
    };
    if entry
        .unique_camera_model
        .as_deref()
        .is_none_or(|u| u.trim().is_empty())
    {
        return true;
    }
    body.is_some_and(|(make, model)| entry.fits(make, model))
}

/// What a sync did: the frames it moved, and the frames the camera
/// profile was left off because it was made for another body.
#[derive(Debug, Default)]
pub(crate) struct Synced {
    pub(crate) moved: Vec<usize>,
    pub(crate) profile_left_off: Vec<usize>,
}

/// Lay `sections` of the current frame over the rest of the set:
/// one step on each frame's history, each sidecar written with the
/// placement the setting asks for, each row's picture and badges put
/// out again. The current frame's edit is not touched.
///
/// `profiles` is the profile directory as `camera::list` reads it:
/// with Camera chosen and a named profile on the source, each target
/// is read for its body and a target the profile was not made for
/// takes every other section and keeps its own profile.
///
/// A target still waiting on its first open for the learned blend
/// its ISO asks for (`seed_blend`) is given that blend now when the
/// sync does not bring one: the sync makes its edit no longer the
/// default, and the next launch would take that to mean the blend
/// was seeded already.
pub(crate) fn sync_selection(
    st: &mut State,
    app: &App,
    sections: &[Section],
    profiles: &[camera::Entry],
    probe: impl Fn(&State, usize) -> Option<greycard_core::decode::Probe>,
) -> Synced {
    let Some(from) = sync_source(st, app) else {
        return Synced::default();
    };
    let targets = sync_targets(st);
    // As a turn does: a sidecar from an older build is brought up to
    // date before anything acts on it, when its shape can be had.
    for &f in &targets {
        migrate_frame(st, f);
    }
    let named = match &from.camera.profile {
        ProfileChoice::Named(name) if sections.contains(&Section::Camera) => {
            Some(profiles.iter().find(|e| &e.name == name))
        }
        _ => None,
    };
    let seeds = !sections.contains(&Section::Noise);
    let mut fits = Vec::new();
    let mut left_off = Vec::new();
    let mut seeded = Vec::new();
    for &f in &targets {
        let wants_seed = seeds && st.seed_blend.get(f).copied().unwrap_or(false);
        let probed = (named.is_some() || wants_seed)
            .then(|| probe(st, f))
            .flatten();
        if wants_seed && let Some(p) = &probed {
            st.sidecars[f].current.noise.learned_strength =
                greycard_edit::Noise::blend_for_iso(p.iso);
            st.seed_blend[f] = false;
            seeded.push(f);
        }
        match named {
            Some(entry)
                if !profile_fits(
                    entry,
                    probed.as_ref().map(|p| (p.make.as_str(), p.model.as_str())),
                ) =>
            {
                left_off.push(f)
            }
            _ => fits.push(f),
        }
    }
    let mut moved = greycard_edit::sync_into(&mut st.sidecars, &from, &fits, sections);
    let without: Vec<Section> = sections
        .iter()
        .copied()
        .filter(|s| *s != Section::Camera)
        .collect();
    moved.extend(greycard_edit::sync_into(
        &mut st.sidecars,
        &from,
        &left_off,
        &without,
    ));
    moved.sort_unstable();
    if !seeds {
        // The sync brought the learned blend, which is a choice now
        // and not the ISO's to seed over on the frame's first open.
        for &f in &moved {
            if let Some(seed) = st.seed_blend.get_mut(f) {
                *seed = false;
            }
        }
    }
    let written = selection::union(&moved, &seeded);
    for &f in &written {
        write_sidecar(st, f);
        let (turns, flip) = thumb_turns(st, app, f);
        show_thumb(st, app, f, turns, flip);
        show_badges(st, app, f);
    }
    show_history(st, app);
    Synced {
        moved,
        profile_left_off: left_off,
    }
}

/// The status line after a sync: how much went where, and which
/// frames kept their own camera profile.
fn synced_words(st: &State, sections: usize, asked: usize, synced: &Synced) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut said = if synced.moved.is_empty() {
        format!("the other {asked} frame{} had these already", plural(asked))
    } else {
        format!(
            "synced {sections} section{} onto {} frame{}",
            plural(sections),
            synced.moved.len(),
            plural(synced.moved.len())
        )
    };
    if !synced.profile_left_off.is_empty() {
        let names: Vec<String> = synced
            .profile_left_off
            .iter()
            .take(3)
            .map(|&f| file_name(&st.files[f]))
            .collect();
        let more = synced.profile_left_off.len().saturating_sub(3);
        said.push_str(&format!(
            "; the camera profile left off {}{} (made for another camera)",
            names.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ));
    }
    said
}

/// The sections checked on the sheet, in `Section::ALL`'s order.
fn checked(st: &State) -> Vec<Section> {
    Section::ALL
        .iter()
        .enumerate()
        .filter(|(i, _)| st.sync_sections.row_data(*i).unwrap_or(false))
        .map(|(_, s)| *s)
        .collect()
}

/// Whether a viewport tool is in hand, as the window's
/// `tool-in-hand` has it: the sheet is not opened over one.
fn tool_in_hand(app: &App) -> bool {
    !app.get_placing().is_empty()
        || !app.get_picking().is_empty()
        || !app.get_guide_mode().is_empty()
        || app.get_level_mode()
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    // The sheet, on everything but the masks.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_open_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if tool_in_hand(&app) {
                app.set_status("put the tool down first (Esc), then sync".into());
                return;
            }
            let (Some(c), targets) = (st.current, sync_targets(&st)) else {
                return;
            };
            if targets.is_empty() {
                app.set_status("choose the frames to sync onto: Ctrl+click or Shift+click".into());
                return;
            }
            st.sync_asked = Some((c, targets));
            for (i, s) in Section::ALL.iter().enumerate() {
                st.sync_sections.set_row_data(i, s.syncs_by_default());
            }
            app.set_sync_section_names(ModelRc::new(VecModel::from(
                Section::ALL
                    .iter()
                    .map(|s| slint::SharedString::from(sync_title(*s)))
                    .collect::<Vec<_>>(),
            )));
            app.set_sync_section_on(ModelRc::from(st.sync_sections.clone()));
            app.set_sync_text(sync_words(&st).into());
            app.set_sync_open(true);
        });
    }
    {
        let state = state.clone();
        app.on_sync_section_toggled(move |i, on| {
            state.borrow().sync_sections.set_row_data(i as usize, on);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_applied(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let sections = checked(&st);
            if sections.is_empty() {
                app.set_status("choose what goes across".into());
                return;
            }
            app.set_sync_open(false);
            // What the sheet said is what is done, or nothing: a
            // selection that moved while the sheet was up is not the
            // one its words named.
            let asked = st.sync_asked.take();
            let targets = sync_targets(&st);
            let now = st.current.map(|c| (c, targets.clone()));
            if asked.is_none() || asked != now {
                tracing::warn!("sync refused: the selection moved while the sheet was open");
                app.set_status(
                    "the selection changed while the sheet was open; nothing synced".into(),
                );
                return;
            }
            if targets.is_empty() {
                app.set_status("nothing to sync onto".into());
                return;
            }
            let profiles = camera::list();
            let start = std::time::Instant::now();
            let synced = sync_selection(&mut st, &app, &sections, &profiles, probe);
            let took = start.elapsed();
            let said = synced_words(&st, sections.len(), targets.len(), &synced);
            tracing::info!("{said} in {:.1} ms", took.as_secs_f64() * 1e3);
            app.set_status(said.into());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::browser::open_row;
    use crate::testing::{click, folder, press, state_for, window};
    use slint::platform::{Key, WindowEvent};

    /// Where row `i` of the strip is, in a 1500 by 950 window: the
    /// strip is the window's foot, 148 high, its cells 178 wide and
    /// 186 apart after a pad of 12.
    fn strip_cell(i: usize) -> (f32, f32) {
        (12.0 + i as f32 * 186.0 + 89.0, 950.0 - 148.0 + 60.0)
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
        let (x, y) = strip_cell(1);
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
            let (x, y) = strip_cell(3);
            click(&app, x, y);
            let (x, y) = strip_cell(4);
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
        let on = app.get_sync_section_on();
        let names = app.get_sync_section_names();
        assert_eq!(on.row_count(), Section::ALL.len());
        for (i, s) in Section::ALL.iter().enumerate() {
            assert_eq!(names.row_data(i).unwrap(), sync_title(*s));
            assert_eq!(on.row_data(i).unwrap(), *s != Section::Adjustments, "{s:?}");
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
            let (x, y) = strip_cell(4);
            click(&app, x, y);
        });
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 2, 3, 4]);
        assert_eq!(state.borrow().current, Some(1));
        // A plain click on the frame on screen: back to it alone.
        let (x, y) = strip_cell(1);
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
    fn apply_lays_the_current_frame_over_the_others_and_saves_them() {
        let dir = std::env::temp_dir().join(format!("greycard-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.placement = greycard_edit::Placement::Folder;
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
            st.picked = vec![0, 2];
        }
        app.invoke_sync_open_asked();
        assert!(app.get_sync_open());
        app.invoke_sync_applied();
        assert!(!app.get_sync_open());
        assert_eq!(app.get_status(), "synced 17 sections onto 1 frame");

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
        // Written where the setting says, and it reads back the same.
        let on_disk = Sidecar::path_in(&files[2], greycard_edit::Placement::Folder);
        assert!(on_disk.exists(), "{}", on_disk.display());
        let back = Sidecar::load(&files[2]).unwrap().unwrap();
        assert_eq!(back.current, s.current);
        assert!(!Sidecar::path_in(&files[1], greycard_edit::Placement::Folder).exists());
        drop(st);
        std::fs::remove_dir_all(&dir).expect("the temp dir goes");
    }

    fn entry(name: &str, made_for: Option<&str>) -> camera::Entry {
        camera::Entry {
            name: name.into(),
            path: PathBuf::new(),
            title: None,
            unique_camera_model: made_for.map(str::to_string),
            copyright: None,
        }
    }

    fn body(make: &str, model: &str, iso: u32) -> greycard_core::decode::Probe {
        greycard_core::decode::Probe {
            make: make.into(),
            model: model.into(),
            iso: Some(iso),
            exposure_time: None,
            fnumber: None,
            ..Default::default()
        }
    }

    #[test]
    fn a_profile_fits_only_the_body_it_was_made_for() {
        let r5 = entry("r5", Some("Canon EOS R5"));
        assert!(profile_fits(Some(&r5), Some(("Canon", "EOS R5"))));
        assert!(!profile_fits(Some(&r5), Some(("Fujifilm", "X-T5"))));
        // A frame that cannot say what took it does not get it.
        assert!(!profile_fits(Some(&r5), None));
        // A profile that names no camera, or one the directory has
        // not got, cannot be checked, and goes as a preset's would.
        assert!(profile_fits(Some(&entry("any", None)), None));
        assert!(profile_fits(Some(&entry("any", Some(" "))), None));
        assert!(profile_fits(None, Some(("Fujifilm", "X-T5"))));
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
            st.seed_blend = vec![false, true, true];
        }
        open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        state.borrow_mut().picked = vec![0, 1, 2];
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
            st.sidecars[1].history[0].noise.learned_strength,
            blend(3200)
        );
        assert_eq!(st.sidecars[2].current.noise.learned_strength, blend(100));
        assert_eq!(st.seed_blend, [false, false, false]);
        assert!(
            synced_words(&st, 2, 2, &synced)
                .ends_with("; the camera profile left off IMG_0002.CR3 (made for another camera)")
        );
        drop(st);

        // With Noise synced the blend is the source's, and nothing
        // is seeded over it on a first open.
        state.borrow_mut().seed_blend = vec![false, true, true];
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
        assert_eq!(st.seed_blend, [false, false, false]);
        assert_eq!(
            st.sidecars[2].current.noise.learned_strength,
            st.sidecars[0].current.noise.learned_strength
        );
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
                let (x, y) = strip_cell(i);
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
        state.borrow_mut().picked = vec![1, 3];
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
        state.borrow_mut().picked = vec![1, 2];
        {
            let mut st = state.borrow_mut();
            st.sidecars[1].meta.flag = greycard_edit::meta::Flag::Reject;
            st.filter = filter::Filter::from_name("No rejects").expect("it parses");
            st.shown = vec![0, 2, 3];
        }
        let st = state.borrow();
        assert_eq!(chosen_frames(&st), vec![2]);
        assert_eq!(st.current, Some(1));
    }
}
