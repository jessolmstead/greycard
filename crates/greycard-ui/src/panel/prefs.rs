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

/// The settings' cap in megabytes as bytes, a number too large for
/// that being as good as no cap at all.
pub(crate) fn cap_bytes(mb: u64) -> u64 {
    mb.saturating_mul(1024 * 1024)
}

/// What the sheet can say about the thumbnail cache.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CacheShown {
    /// The platform names no cache directory.
    Missing,
    /// Not counted yet, or busy: the count is on its way.
    Counting,
    Known {
        usage: greycard_library::thumbs::Usage,
        cap: u64,
    },
}

impl CacheShown {
    fn entries(self) -> usize {
        match self {
            CacheShown::Known { usage, .. } => usage.entries,
            _ => 0,
        }
    }
}

/// The sheet's line about the thumbnail cache: what it holds against
/// its cap, or that it is off, or that there is none.
pub(crate) fn thumb_cache_words(shown: CacheShown) -> String {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    match shown {
        CacheShown::Missing => {
            "No cache folder on this machine: thumbnails are made each time.".into()
        }
        CacheShown::Counting => "Counting…".into(),
        CacheShown::Known { usage, cap: 0 } if usage.entries == 0 => {
            "Off: thumbnails are made each time a folder opens.".into()
        }
        CacheShown::Known { usage, cap: 0 } => format!(
            "Off. {} thumbnail{} from before, {:.1} MB.",
            usage.entries,
            plural(usage.entries),
            mb(usage.bytes)
        ),
        CacheShown::Known { usage, cap } => format!(
            "{} thumbnail{} kept, {:.1} MB of {:.0} MB.",
            usage.entries,
            plural(usage.entries),
            mb(usage.bytes),
            mb(cap)
        ),
    }
}

fn apply_thumb_cache(app: &App, shown: CacheShown) {
    app.set_thumb_cache_on(shown != CacheShown::Missing);
    app.set_thumb_cache_entries(shown.entries().min(i32::MAX as usize) as i32);
    app.set_thumb_cache_words(thumb_cache_words(shown).into());
}

/// The cache's line from the count the cache keeps, without touching
/// the disk or waiting on the worker: when the count is not known yet,
/// or the worker holds the cache in the middle of an eviction, the
/// line says so and a thread finds out and fills it in.
fn show_thumb_cache(app: &App, worker: &Worker) {
    let cache = worker.thumb_cache();
    let shown = match cache.try_lock() {
        Ok(held) => match held.as_ref() {
            None => CacheShown::Missing,
            Some(c) => match c.known_usage() {
                Some(usage) => CacheShown::Known {
                    usage,
                    cap: c.cap(),
                },
                None => CacheShown::Counting,
            },
        },
        Err(std::sync::TryLockError::WouldBlock) => CacheShown::Counting,
        Err(std::sync::TryLockError::Poisoned(_)) => CacheShown::Missing,
    };
    apply_thumb_cache(app, shown);
    if shown == CacheShown::Counting {
        on_the_cache(app, worker, |_| {});
    }
}

/// Do `work` to the thumbnail cache on a thread of its own — a clear
/// or an eviction over a large cache takes seconds, and a lock the
/// worker holds is waited for there — and show the cache as it is
/// after.
fn on_the_cache(
    app: &App,
    worker: &Worker,
    work: impl FnOnce(&mut greycard_library::Thumbs) + Send + 'static,
) {
    let cache = worker.thumb_cache();
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("thumbnail cache".into())
        .spawn(move || {
            let shown = match cache.lock() {
                Ok(mut held) => match held.as_mut() {
                    None => CacheShown::Missing,
                    Some(c) => {
                        work(c);
                        let usage = match c.known_usage() {
                            Some(u) => u,
                            None => {
                                let u = c.usage();
                                c.seed_usage(u);
                                u
                            }
                        };
                        CacheShown::Known {
                            usage,
                            cap: c.cap(),
                        }
                    }
                },
                Err(_) => CacheShown::Missing,
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app_weak.upgrade() {
                    apply_thumb_cache(&app, shown);
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("thumbnail cache: {e}");
    }
}

/// The cap field's text as megabytes: a whole number, spaces allowed
/// around it, or `None`.
pub(crate) fn cap_typed(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    // Clear the thumbnail cache: every folder's thumbnails are made
    // again the next time it opens.
    {
        let (worker, app_weak) = (worker.clone(), app.as_weak());
        app.on_thumb_cache_clear(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            apply_thumb_cache(&app, CacheShown::Counting);
            on_the_cache(&app, &worker, |c| {
                let gone = c.clear();
                tracing::info!(
                    "thumbnail cache cleared: {} entries, {} bytes",
                    gone.entries,
                    gone.bytes
                );
            });
        });
    }
    // A new cap: kept in the settings, and the cache evicted to it now
    // rather than at its next write. Zero is the cache off, and empties
    // it.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_thumb_cache_cap_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let kept = worker
                .thumb_cache()
                .try_lock()
                .ok()
                .and_then(|c| c.as_ref().map(|c| c.cap() / (1024 * 1024)))
                .unwrap_or_else(|| settings::Settings::load().thumb_cache_mb);
            let typed = app.get_thumb_cache_cap();
            let Some(mb) = cap_typed(&typed) else {
                // Not a number: the field goes back to what is kept.
                app.set_thumb_cache_cap(kept.to_string().into());
                return;
            };
            // The sheet closing asks too, changed or not.
            if mb == kept {
                return;
            }
            app.set_thumb_cache_cap(mb.to_string().into());
            keep(&state.borrow(), |s| s.thumb_cache_mb = mb);
            tracing::info!("thumbnail cache cap {mb} MB");
            apply_thumb_cache(&app, CacheShown::Counting);
            on_the_cache(&app, &worker, move |c| c.set_cap(cap_bytes(mb)));
        });
    }
    // Ctrl+, or the gear: the count first, then the sheet.
    {
        let (state, app_weak, worker) = (state.clone(), app.as_weak(), worker.clone());
        app.on_settings_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            show_thumb_cache(&app, &worker);
            let mb = worker
                .thumb_cache()
                .try_lock()
                .ok()
                .and_then(|c| c.as_ref().map(|c| c.cap() / (1024 * 1024)))
                .unwrap_or_else(|| settings::Settings::load().thumb_cache_mb);
            app.set_thumb_cache_cap(mb.to_string().into());
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

    #[test]
    fn the_thumbnail_cache_is_said_in_megabytes() {
        use greycard_library::thumbs::Usage;
        let mb = 1024 * 1024;
        let known = |bytes, entries, cap| CacheShown::Known {
            usage: Usage { bytes, entries },
            cap,
        };
        assert_eq!(
            thumb_cache_words(known(5 * mb / 2, 312, 300 * mb)),
            "312 thumbnails kept, 2.5 MB of 300 MB."
        );
        assert_eq!(
            thumb_cache_words(known(0, 1, mb)),
            "1 thumbnail kept, 0.0 MB of 1 MB."
        );
        // Off with nothing kept, and off with a cache left from before,
        // which the sheet still offers to clear.
        assert!(thumb_cache_words(known(0, 0, 0)).starts_with("Off:"));
        assert_eq!(
            thumb_cache_words(known(3 * mb, 40, 0)),
            "Off. 40 thumbnails from before, 3.0 MB."
        );
        assert_eq!(known(3 * mb, 40, 0).entries(), 40);
        assert_eq!(CacheShown::Counting.entries(), 0);
        assert_eq!(thumb_cache_words(CacheShown::Counting), "Counting…");
        assert!(thumb_cache_words(CacheShown::Missing).starts_with("No cache"));
    }

    #[test]
    fn the_cap_is_whole_megabytes_and_never_overflows() {
        assert_eq!(cap_typed(" 300 "), Some(300));
        assert_eq!(cap_typed("0"), Some(0));
        assert_eq!(cap_typed("2.5"), None);
        assert_eq!(cap_typed("-1"), None);
        assert_eq!(cap_typed(""), None);
        assert_eq!(cap_bytes(300), 300 * 1024 * 1024);
        assert_eq!(cap_bytes(1 << 44), u64::MAX);
        assert_eq!(cap_bytes(u64::MAX), u64::MAX);
    }
}
