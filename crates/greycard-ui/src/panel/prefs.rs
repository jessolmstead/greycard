//! The settings sheet: the sidecars' place and the XMP switch, which
//! are the installation's rather than a picture's, and the move that
//! puts an open folder's sidecars where the place says.

use crate::*;

/// The sheet's words for the two places, in the order its row shows
/// them.
pub(crate) const BESIDE: &str = "Beside the raw";
pub(crate) const FOLDER: &str = "Hidden folder";

pub(crate) fn placement_name(placement: greycard_edit::Placement) -> &'static str {
    match placement {
        greycard_edit::Placement::Beside => BESIDE,
        greycard_edit::Placement::Folder => FOLDER,
    }
}

/// The place the row names; beside for anything else, the default.
pub(crate) fn placement_named(name: &str) -> greycard_edit::Placement {
    if name == FOLDER {
        greycard_edit::Placement::Folder
    } else {
        greycard_edit::Placement::Beside
    }
}

/// How many of `files` have a sidecar in the place `placement` does
/// not name: what the sheet's move would settle.
pub(crate) fn sidecars_elsewhere(files: &[PathBuf], placement: greycard_edit::Placement) -> usize {
    files
        .iter()
        .filter(|f| Sidecar::misplaced(f, placement))
        .count()
}

/// What a move did: the sidecars renamed into place, the stale
/// copies removed from the other place (the one read was in place
/// already), and the frames that could not be settled, each with why.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Moved {
    pub(crate) moved: usize,
    pub(crate) dropped: usize,
    pub(crate) failed: Vec<(PathBuf, String)>,
}

/// Put every one of `files`' sidecars where `placement` says: a
/// [`Sidecar::settle`] a frame. One that fails is said and the rest go
/// on; nothing is read or rewritten, so an edit cannot be lost to it.
pub(crate) fn move_sidecars(files: &[PathBuf], placement: greycard_edit::Placement) -> Moved {
    let mut out = Moved::default();
    for f in files {
        match Sidecar::settle(f, placement) {
            Ok(greycard_edit::Settled::Moved) => out.moved += 1,
            Ok(greycard_edit::Settled::Dropped) => out.dropped += 1,
            Ok(greycard_edit::Settled::InPlace) => {}
            Err(e) => {
                tracing::warn!("{}: sidecar not moved: {e}", f.display());
                out.failed.push((f.clone(), e.to_string()));
            }
        }
    }
    out
}

/// The move's report, for the sheet.
pub(crate) fn moved_words(moved: &Moved, placement: greycard_edit::Placement) -> String {
    let n = moved.moved;
    let place = match placement {
        greycard_edit::Placement::Beside => "beside their raws",
        greycard_edit::Placement::Folder => "into the hidden folder",
    };
    let mut words = format!(
        "Moved {n} sidecar{} {place}.",
        if n == 1 { "" } else { "s" }
    );
    if moved.dropped > 0 {
        let k = moved.dropped;
        words.push_str(&format!(
            " Removed {k} older cop{} left in the other place.",
            if k == 1 { "y" } else { "ies" }
        ));
    }
    if !moved.failed.is_empty() {
        let k = moved.failed.len();
        words.push_str(&format!(" {k} could not be moved; the log says why."));
    }
    words
}

/// The sheet's count, over the open frames as they are on disk now:
/// the list the move runs over, a folder's or the one file opened on
/// its own. And whether a move may run at all.
pub(crate) fn show_elsewhere(st: &State, app: &App) {
    app.set_open_frames(st.files.len() as i32);
    app.set_sidecars_elsewhere(sidecars_elsewhere(&st.files, st.placement) as i32);
    app.set_sidecars_written(st.write_sidecars);
}

/// Change one field of the settings file, unless this is a batch run
/// or a test, which leave the user's settings alone. The close of the
/// window keeps these two from the file rather than the panel, so the
/// file is the place they live.
fn keep(st: &State, change: impl FnOnce(&mut settings::Settings)) {
    if st.batch || cfg!(test) {
        return;
    }
    let mut kept = settings::Settings::load();
    change(&mut kept);
    kept.save();
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    // Ctrl+, or the gear: the count first, then the sheet.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_settings_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            app.set_sidecar_placement(placement_name(st.placement).into());
            app.set_xmp_sidecars(st.xmp_sidecars);
            app.set_settings_note("".into());
            show_elsewhere(&st, &app);
            app.set_settings_open(true);
        });
    }
    // The place flipped: from now on each save writes there, and
    // nothing moves until it is asked to.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_placement_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.placement = placement_named(&app.get_sidecar_placement());
            let folder = st.placement == greycard_edit::Placement::Folder;
            keep(&st, |s| s.sidecars_in_folder = folder);
            app.set_settings_note("".into());
            show_elsewhere(&st, &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_xmp_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            st.xmp_sidecars = app.get_xmp_sidecars();
            let on = st.xmp_sidecars;
            keep(&st, |s| s.xmp_sidecars = on);
        });
    }
    // The folder's sidecars put where the setting says, confirmed.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_sidecars_move(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            // `--no-sidecars` promised not to touch them; the sheet
            // says so and offers nothing, and this holds to it too.
            if !st.write_sidecars {
                return;
            }
            let moved = move_sidecars(&st.files, st.placement);
            tracing::info!(
                "moved {} sidecars to {:?}, removed {} older copies, {} failed",
                moved.moved,
                st.placement,
                moved.dropped,
                moved.failed.len()
            );
            app.set_settings_note(moved_words(&moved, st.placement).into());
            show_elsewhere(&st, &app);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::Placement;

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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_sheet_opens_counts_and_moves() {
        let dir = scratch("sheet");
        let files = shoot(&dir);
        let app = crate::testing::window(files.len());
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        state.borrow_mut().write_sidecars = true;

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
        assert_eq!(state.borrow().placement, Placement::Folder);
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
        assert!(state.borrow().xmp_sidecars);

        // Escape closes it.
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_settings_open());
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_sidecars_moves_nothing() {
        let dir = scratch("nosidecars");
        let files = shoot(&dir);
        let app = crate::testing::window(files.len());
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = false;
            st.placement = Placement::Folder;
        }
        app.invoke_settings_asked();
        assert!(!app.get_sidecars_written(), "the sheet is told");
        app.invoke_sidecars_move();
        assert!(Sidecar::path_in(&files[0], Placement::Beside).exists());
        assert_eq!(app.get_sidecars_elsewhere(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rows_words_name_the_places() {
        for p in [Placement::Beside, Placement::Folder] {
            assert_eq!(placement_named(placement_name(p)), p);
        }
        assert_eq!(placement_named("anything"), Placement::Beside);
    }
}
