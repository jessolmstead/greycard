//! Reset to Original: a frame's edit put back to the state it started
//! from, as one step its history can go back from. The history's head
//! offers it for the frame on screen, the frame menu for the whole
//! selection; both record the same step ([`reset_line`]), and a frame
//! already there is left as it is.

use std::rc::Rc;

use greycard_edit::Edit;
use slint::ComponentHandle;

use crate::panel::browser::{chosen_frames, migrate_frame, show_badges, show_thumb, thumb_turns};
use crate::panel::cull::leave_cull;
use crate::panel::edit::record_panel;
use crate::panel::history::{show_history, take_current, take_current_with};
use crate::panel::queue_view::refused;
use crate::worker::Worker;
use crate::{App, Shared, State};

/// The words the step is recorded with, which its history row shows.
pub(crate) const LABEL: &str = "Reset to Original";

/// The edit cell `c`'s line started from: its earliest state when that
/// is the frame's default, which is what the history's Original row
/// holds, seeded blend and all; else, the earliest state being one a
/// long history kept in its place, the frame's default afresh, a raw's
/// with its ISO's learned blend (the earliest state's when the ISO is
/// not known).
pub(crate) fn original_of(st: &State, c: usize) -> Option<Edit> {
    let line = st.line(c)?;
    let path = st.path_at(c)?;
    let iso = st
        .file_at(c)
        .and_then(|f| st.from_row.get(f))
        .and_then(|r| r.iso);
    let first = line.state(0)?;
    if greycard_library::is_default_edit(first, path, iso) {
        return Some(first.clone());
    }
    if greycard_core::picture::is_picture_path(path) {
        return Some(Edit::for_picture());
    }
    let mut edit = Edit::default();
    edit.noise.learned_strength = match iso {
        Some(iso) => greycard_edit::Noise::blend_for_iso(iso),
        None => first.noise.learned_strength,
    };
    Some(edit)
}

/// Whether cell `c`'s current state is its Original ([`original_of`]);
/// true for a cell with no line, which has nothing to reset.
pub(crate) fn at_original(st: &State, c: usize) -> bool {
    match (st.line(c), original_of(st, c)) {
        (Some(line), Some(original)) => line.current == original,
        _ => true,
    }
}

/// Offer the history's Reset to Original by what the panel holds now
/// against the frame's Original, so a slider moved shows it before the
/// save records the step. Nothing in culling, or while the panel is not
/// the frame's own; the history's rows say it then (`show_history`).
pub(crate) fn show_panel_resettable(st: &State, app: &App) {
    let Some(c) = st.current else {
        return;
    };
    if st.culling.cull.is_some() || !crate::rows::panel_is_frames(st, c) {
        return;
    }
    let Some(original) = original_of(st, c) else {
        return;
    };
    let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    app.set_history_resettable(edit != original);
    // And each header's reset, by the same panel.
    crate::panel::section_reset::show_section_resets(st, app, &edit, st.develop.target);
}

/// Record cell `c`'s Original as its current state, one step named
/// [`LABEL`], without writing the sidecar; false when it was there
/// already.
pub(crate) fn reset_line(st: &mut State, c: usize) -> bool {
    let Some(original) = original_of(st, c) else {
        return false;
    };
    st.line_mut(c)
        .is_some_and(|line| line.record_as(original, Some(LABEL.to_string())))
}

/// Reset every one of `chosen` to its Original: the frame on screen,
/// outside culling, through the panel, what it holds recorded first,
/// as a paste onto it is; the rest on their sidecars, each written once
/// and its cell put out again. A frame at its Original already is left
/// alone, history and file. The cells that moved, sorted.
pub(crate) fn reset_onto(
    st: &mut State,
    app: &App,
    worker: &Worker,
    chosen: &[usize],
) -> Vec<usize> {
    if refused(st, "a reset") {
        return Vec::new();
    }
    let panel_frame = st.current.filter(|&c| {
        st.culling.cull.is_none() && chosen.contains(&c) && crate::rows::panel_is_frames(st, c)
    });
    let current_changed = panel_frame.is_some_and(|c| {
        let recorded = record_panel(st, app);
        reset_line(st, c) || recorded
    });
    let rest: Vec<usize> = chosen
        .iter()
        .copied()
        .filter(|&f| Some(f) != panel_frame)
        .collect();
    let rest = crate::rows::load_frames(st, app, &rest);
    for &f in &rest {
        migrate_frame(st, f);
    }
    let mut moved: Vec<usize> = rest.into_iter().filter(|&f| reset_line(st, f)).collect();
    let files = st.files_of(&moved);
    // One batch whatever the count, so its landing says in the log how
    // many reached the disk and names any that did not.
    if !files.is_empty() {
        crate::panel::saves::save_for(st, &files, Some("reset to original"));
    }
    for &f in &moved {
        let (turns, flip) = thumb_turns(st, app, f);
        show_thumb(st, app, f, turns, flip);
        show_badges(st, app, f);
    }
    match panel_frame.filter(|_| current_changed) {
        Some(c) => {
            // Written with the rest when a version of its file moved
            // too: one write, one revision.
            let written = st.file_at(c).is_some_and(|f| files.contains(&f));
            st.develop.status_kept = None;
            take_current_with(st, app, worker, !written);
            moved.push(c);
            moved.sort_unstable();
        }
        None => show_history(st, app),
    }
    tracing::info!(
        "reset to original: {} of {} frames moved",
        moved.len(),
        chosen.len()
    );
    moved
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    // The history's head: the frame on screen.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_history_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            if refused(&st, "a reset") {
                return;
            }
            let Some(c) = st.current else {
                return;
            };
            if let Some(why) = crate::rows::not_kept(&st, c) {
                app.set_status(why.into());
                return;
            }
            // In culling the panel is not the frame's: the sidecar
            // takes the step on its own, and the leaving develops it.
            if st.culling.cull.is_some() {
                if reset_line(&mut st, c) {
                    leave_cull(&mut st, &app, &worker, None);
                }
                return;
            }
            let recorded = record_panel(&mut st, &app);
            if reset_line(&mut st, c) || recorded {
                st.develop.status_kept = None;
                take_current(&mut st, &app, &worker);
            }
        });
    }
    // The frame menu: the selection, once its sidecars are in.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_menu_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            if refused(&st, "a reset") {
                return;
            }
            let keys: Vec<crate::versions::Key> = chosen_frames(&st)
                .into_iter()
                .filter_map(|c| st.key_of(c))
                .collect();
            drop(st);
            crate::rows::request(
                &state,
                &app,
                &worker,
                keys,
                crate::rows::Scope::Line,
                crate::rows::Request::Reset,
                Box::new(move |state, app, worker, at| {
                    let chosen: Vec<usize> = at.into_iter().flatten().collect();
                    reset_onto(&mut state.borrow_mut(), app, worker, &chosen);
                }),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use greycard_edit::Sidecar;
    use slint::Model;

    use super::*;
    use crate::panel::edit::read_edit;
    use crate::testing::{folder, state_for, window};

    fn history_rows(app: &App) -> Vec<String> {
        let rows = app.get_history_rows();
        (0..rows.row_count())
            .map(|r| rows.row_data(r).unwrap().name.to_string())
            .collect()
    }

    /// The panel's edit made a state, as the save timer makes it.
    fn save(app: &App, state: &Rc<Shared>) {
        let mut st = state.borrow_mut();
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
        show_history(&st, app);
    }

    fn exposed(ev: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = ev;
        e
    }

    #[test]
    fn the_head_resets_the_frame_on_screen_as_one_step_undone_by_one_undo() {
        let app = window(2);
        let (state, _worker) = state_for(&app, folder(2));
        app.invoke_select(0);
        // At the Original, nothing to offer.
        assert!(!app.get_history_resettable());
        // Offered as the slider moves, before the save records it, and
        // taken back as it returns.
        app.set_exposure(0.7);
        app.invoke_view_changed();
        assert!(app.get_history_resettable());
        assert!(state.borrow().sidecars[0].history.is_empty());
        app.set_exposure(0.0);
        app.invoke_view_changed();
        assert!(!app.get_history_resettable());
        app.set_exposure(0.7);
        save(&app, &state);
        assert!(app.get_history_resettable());
        let rows_before = history_rows(&app);

        app.invoke_history_reset();
        assert_eq!(app.get_exposure(), 0.0);
        assert!(!app.get_history_resettable());
        let mut want = vec![LABEL.to_string()];
        want.extend(rows_before);
        assert_eq!(history_rows(&app), want);
        {
            let st = state.borrow();
            assert_eq!(st.sidecars[0].current, Edit::default());
            assert_eq!(st.sidecars[0].current_label.as_deref(), Some(LABEL));
        }

        app.invoke_undo();
        assert_eq!(app.get_exposure(), 0.7);
        assert_eq!(state.borrow().sidecars[0].current.light.exposure, 0.7);
        assert!(app.get_history_resettable());
        // The other frame was never touched.
        assert_eq!(state.borrow().sidecars[1], Sidecar::default());
    }

    #[test]
    fn the_menu_resets_the_edited_of_a_selection_and_leaves_the_one_at_original() {
        let dir = std::env::temp_dir().join(format!("greycard-reset-{}", std::process::id()));
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
            st.sidecars[1].record(exposed(0.5));
            st.sidecars[2].record(exposed(-1.0));
            st.sidecars[2].record(exposed(1.5));
        }
        // The open frame is one of the edited, its panel the edit's.
        app.invoke_select(1);
        assert_eq!(app.get_exposure(), 0.5);
        assert!(app.get_history_resettable());
        state.borrow_mut().views.picked = vec![0, 1, 2];
        let untouched = state.borrow().sidecars[0].clone();

        app.invoke_menu_reset();
        {
            let st = state.borrow();
            for f in [1, 2] {
                let s = &st.sidecars[f];
                assert_eq!(s.current, Edit::default(), "{f}");
                assert_eq!(s.current_label.as_deref(), Some(LABEL), "{f}");
                let back = Sidecar::load(&files[f]).unwrap().expect("written");
                assert_eq!(back.current, s.current, "{f}");
            }
            // One step each, undoable.
            assert_eq!(st.sidecars[1].history.len(), 2);
            assert_eq!(st.sidecars[2].history.len(), 3);
            // The frame at Original: its history and its file alone.
            assert_eq!(st.sidecars[0], untouched);
            assert!(st.sidecars[0].history.is_empty());
            assert!(Sidecar::load(&files[0]).unwrap().is_none());
        }
        // The open frame's panel and history follow.
        assert_eq!(app.get_exposure(), 0.0);
        assert!(!app.get_history_resettable());
        assert_eq!(history_rows(&app)[0], LABEL);
        app.invoke_undo();
        assert_eq!(app.get_exposure(), 0.5);
        crate::testing::remove_dir_retry(&dir);
    }
}
