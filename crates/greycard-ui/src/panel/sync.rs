//! Sync settings: the current frame's chosen sections laid over the
//! rest of the browser's selection, each frame's history taking it
//! as one step and each sidecar written where the frame keeps it.
//!
//! The sheet is the preset sheet's twin and the work is a preset's:
//! `greycard_edit::sync_into` builds a `Preset` from the current
//! frame and applies it, so there is no second copy of what a
//! section is.

use crate::panel::browser::{
    chosen_frames, file_name, migrate_frame, show_badges, show_thumb, thumb_turns,
};
use crate::panel::edit::{read_edit, save_edit, write_sidecar};
use crate::panel::history::show_history;
use crate::*;

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

/// Lay `sections` of the current frame over the rest of the set:
/// one step on each frame's history, each sidecar written with the
/// placement the setting asks for, each row's picture and badges put
/// out again. The current frame's edit is not touched. The frames
/// that took it come back; a frame that had every section already
/// is left out, and its sidecar is not rewritten.
pub(crate) fn sync_selection(st: &mut State, app: &App, sections: &[Section]) -> Vec<usize> {
    let Some(from) = sync_source(st, app) else {
        return Vec::new();
    };
    let targets = sync_targets(st);
    // As a turn does: a sidecar from an older build is brought up to
    // date before anything acts on it, when its shape can be had.
    for &f in &targets {
        migrate_frame(st, f);
    }
    let moved = greycard_edit::sync_into(&mut st.sidecars, &from, &targets, sections);
    for &f in &moved {
        write_sidecar(st, f);
        let (turns, flip) = thumb_turns(st, app, f);
        show_thumb(st, app, f, turns, flip);
        show_badges(st, app, f);
    }
    show_history(st, app);
    moved
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

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    // The sheet, on everything but the masks.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sync_open_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if sync_targets(&st).is_empty() {
                app.set_status("choose the frames to sync onto: Ctrl+click or Shift+click".into());
                return;
            }
            for (i, s) in Section::ALL.iter().enumerate() {
                st.sync_sections.set_row_data(i, s.syncs_by_default());
            }
            app.set_sync_section_names(ModelRc::new(VecModel::from(
                Section::ALL
                    .iter()
                    .map(|s| slint::SharedString::from(s.title()))
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
            let asked = sync_targets(&st).len();
            let moved = sync_selection(&mut st, &app, &sections);
            let said = if moved.is_empty() {
                format!(
                    "the other {asked} frame{} had these already",
                    if asked == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "synced {} section{} onto {} frame{}",
                    sections.len(),
                    if sections.len() == 1 { "" } else { "s" },
                    moved.len(),
                    if moved.len() == 1 { "" } else { "s" }
                )
            };
            tracing::info!("{said}");
            app.set_status(said.into());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            assert_eq!(names.row_data(i).unwrap(), s.title());
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
}
