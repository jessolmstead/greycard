//! A frame a newer greycard made is read-only here (§724): read and
//! shown, and every write from the window refused, each said in one
//! line ("Read-only: made with a newer greycard") and logged by name.
//! The file on disk, and the XMP beside it, are left as the newer
//! build wrote them.

use std::path::PathBuf;
use std::rc::Rc;

use greycard_edit::{Edit, Placement, Preset, Section, meta};

use crate::testing::{remove_scratch, state_for, window};
use crate::{App, Shared};

/// A sidecar a build at schema 11 wrote: floors read 10, write 11, a
/// state at 11 with a field this build does not know, a rating, the
/// reject flag, and a turn.
const NEWER: &str = r#"{"floors": {"read": 10, "write": 11}, "current": {"version": 11, "light": {"exposure": 0.5, "sparkle": 1}}, "meta": {"rating": 3, "flag": "reject"}, "turn": 1}"#;

const WORDS: &str = "Read-only: made with a newer greycard";

/// `count` frames with a sidecar each at an exposure of 0.25, the
/// first's then written over by a newer build, all read by the window,
/// sidecars and XMPs written beside them.
fn newer(
    name: &str,
    count: usize,
) -> (
    App,
    Rc<Shared>,
    Rc<crate::worker::Worker>,
    PathBuf,
    Vec<PathBuf>,
) {
    let dir = std::env::temp_dir().join(format!("greycard-newer-{name}-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<PathBuf> = (0..count)
        .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
        .collect();
    for f in &files {
        let mut s = greycard_edit::Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 0.25;
        s.record(e);
        s.save_in(f, Placement::Beside).unwrap();
    }
    std::fs::write(greycard_edit::Sidecar::path_for(&files[0]), NEWER).unwrap();
    let app = window(count);
    let (state, worker) = state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.xmp_sidecars = true;
        st.window.placement = Placement::Beside;
        for f in 0..count {
            assert!(crate::rows::reread_file(&mut st, &app, f));
        }
        assert!(st.sidecars[0].read_only());
        assert!(!st.sidecars[1].read_only());
    }
    (app, state, worker, dir, files)
}

/// The newer build's file, as it left it, and no XMP beside the frame.
fn untouched(files: &[PathBuf]) {
    let gcd = greycard_edit::Sidecar::path_for(&files[0]);
    assert_eq!(std::fs::read_to_string(gcd).unwrap(), NEWER);
    assert!(
        greycard_edit_io::xmp::paths_of(&files[0]).is_empty(),
        "no XMP"
    );
}

/// The frame opens with what this build knows of it, and neither the
/// single save, the batch, nor a retry writes it or an XMP beside it.
#[test]
fn a_newer_build_s_frame_is_read_and_never_saved() {
    let (_app, state, _worker, dir, files) = newer("save", 2);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.sidecars[0].current().light.exposure, 0.5);
        assert_eq!(st.sidecars[0].meta.rating, 3);
        assert!(!crate::sidecars::file_writable(&st, 0));
        crate::sidecars::write_file(&mut st, 0);
        assert!(!crate::sidecars::save_file(&mut st, 0) || !crate::sidecars::file_writable(&st, 0));
        crate::panel::saves::save(&mut st, &[0, 1]);
    }
    untouched(&files);
    remove_scratch(state, &dir);
}

/// A step on it, by any panel action that asks for its line (a slider,
/// an undo, a preset on the open frame, an export's record), is
/// refused and said.
#[test]
fn a_step_on_a_newer_build_s_frame_is_refused_and_said() {
    let (app, state, _worker, dir, files) = newer("step", 2);
    {
        let mut st = state.borrow_mut();
        assert!(st.line_mut(0).is_none());
        assert_eq!(app.get_status(), WORDS);
        app.set_status("".into());
        assert!(st.line_mut(1).is_some(), "a frame of this build's is not");
        assert_eq!(app.get_status(), "");
        let edit = st.sidecars[0].current().clone();
        crate::panel::history::record_export(
            &mut st,
            &app,
            &files[0].clone(),
            &edit,
            None,
            &dir.join("out.jpg"),
            None,
        );
        assert!(
            st.sidecars[0].current_exports.is_empty(),
            "no export recorded"
        );
    }
    untouched(&files);
    remove_scratch(state, &dir);
}

/// A rating, a flag, or a label key over a set with a newer build's
/// frame in it: the others take it, the newer one keeps its own, and
/// the status line says why.
#[test]
fn a_key_over_a_newer_build_s_frame_leaves_it_and_says_so() {
    let (app, state, _worker, dir, files) = newer("keys", 2);
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::set_meta(&mut st, &app, &[0, 1], meta::Change::Rating(5));
        assert_eq!(st.sidecars[0].meta.rating, 3);
        assert_eq!(st.sidecars[1].meta.rating, 5);
        crate::panel::browser::set_meta(&mut st, &app, &[0], meta::Change::Flag(meta::Flag::Pick));
        assert_eq!(st.sidecars[0].meta.flag, meta::Flag::Reject);
        assert_eq!(app.get_status(), WORDS);
        // The reject's flag taken off as a frame comes back from the
        // rejects: left, in memory and on disk.
        crate::panel::cull::unreject(&mut st, 0);
        assert_eq!(st.sidecars[0].meta.flag, meta::Flag::Reject);
        // An undo of a key on a frame a newer build saved since: left.
        st.sidecars[1].newer = Some(11);
        crate::panel::browser::step_tags(&mut st, &app, true);
        assert_eq!(st.sidecars[1].meta.rating, 5);
        st.sidecars[1].newer = None;
    }
    untouched(&files);
    remove_scratch(state, &dir);
}

/// A turn, a paste, a sync to the selection, or a preset on a set: the
/// newer build's frame is left out of each, and said.
#[test]
fn a_turn_paste_sync_or_preset_leaves_a_newer_build_s_frame() {
    let (app, state, worker, dir, files) = newer("lay", 2);
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::turn_frames(&mut st, &app, &worker, &[0, 1], 1);
        assert_eq!(st.sidecars[0].turn, 1, "as the newer build left it");
        assert_eq!(st.sidecars[1].turn, 1);
        app.set_status("".into());
        let mut grain = Edit::default();
        grain.grain.amount = 0.4;
        let preset = Preset::from_edit("Fine Grain", &grain, &[Section::Grain]);
        let synced = crate::panel::sync::lay_over_targets(
            &mut st,
            &app,
            &preset,
            &[0, 1],
            None,
            Some("Preset ×2: Fine Grain"),
            &[],
            |_, _| None,
        );
        assert_eq!(synced.moved, [1]);
        assert_eq!(
            st.sidecars[0].current().grain.amount,
            Edit::default().grain.amount
        );
        assert_eq!(app.get_status(), WORDS);
    }
    untouched(&files);
    remove_scratch(state, &dir);
}

/// The versions panel makes, renames, swaps, and deletes no version of
/// a newer build's frame.
#[test]
fn no_version_is_made_of_a_newer_build_s_frame() {
    let (app, state, _worker, dir, files) = newer("versions", 2);
    {
        let mut st = state.borrow_mut();
        assert_eq!(
            crate::panel::versions::unready(&mut st, &app, 0),
            Some(WORDS.to_string())
        );
        assert!(st.sidecars[0].versions.is_empty());
    }
    untouched(&files);
    remove_scratch(state, &dir);
}
