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
/// The count of sidecars out of place, asked for and shown. It is a
/// stat a frame over the open list, which for a root on a share is
/// thousands of round trips (11,711 froze the window on opening the
/// sheet), so it runs on a thread of its own; the sheet says
/// "Counting..." (a count below zero) until it lands, and a count for
/// an older list or place is dropped when a newer one has been asked
/// for. A test counts in place.
pub(crate) fn show_elsewhere(st: &State, app: &App) {
    app.set_open_frames(st.files.len() as i32);
    app.set_sidecars_written(st.window.write_sidecars);
    let ask = ELSEWHERE_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let (files, placement) = (st.files.clone(), st.window.placement);
    if cfg!(test) {
        app.set_sidecars_elsewhere(sidecars_elsewhere(&files, placement) as i32);
        return;
    }
    app.set_sidecars_elsewhere(-1);
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("sidecars count".into())
        .spawn(move || {
            let n = sidecars_elsewhere(&files, placement) as i32;
            let _ = slint::invoke_from_event_loop(move || {
                if ELSEWHERE_ASKED.load(std::sync::atomic::Ordering::SeqCst) != ask {
                    return;
                }
                if let Some(app) = app_weak.upgrade() {
                    app.set_sidecars_elsewhere(n);
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("sidecars count: {e}");
        app.set_sidecars_elsewhere(0);
    }
}

/// The number of the last count asked for, so a count that lands after
/// a newer one was asked for is dropped.
static ELSEWHERE_ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// A move of the sidecars is out; a second is refused until it lands.
static MOVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Change one field of the settings file, unless this is a batch run
/// or a test, which leave the user's settings alone. The close of the
/// window keeps these fields (the sidecars' two and culling's move-on
/// switch) from the file rather than the panel, so the file is the
/// place they live.
pub(crate) fn keep(st: &State, change: impl FnOnce(&mut settings::Settings)) {
    if st.window.batch || cfg!(test) {
        return;
    }
    let mut kept = settings::Settings::load();
    change(&mut kept);
    kept.save();
}

/// The Start in row's words for its two choices, in its order.
pub(crate) const START_LAST: &str = "Last folder";
pub(crate) const START_CHOSEN: &str = "Chosen folder";

/// The Start in row as the settings keep it: its choice, and the
/// chosen folder as shown, the home folder short.
pub(crate) fn show_start(app: &App, chosen: &str) {
    if chosen.is_empty() {
        app.set_start_choice(START_LAST.into());
        app.set_start_folder("".into());
    } else {
        app.set_start_choice(START_CHOSEN.into());
        app.set_start_folder(crate::report::path_shown(Path::new(chosen)).into());
    }
}

#[cfg(test)]
thread_local! {
    /// What the Start in chooser answers in a test, in place of
    /// asking: `Some(folder)`, or none (and when nothing is set) a
    /// cancel.
    pub(crate) static START_CHOSEN_IN_TEST: RefCell<Option<Option<PathBuf>>> =
        const { RefCell::new(None) };
}

/// A click on the Start in row: the last folder taken at once; a
/// chosen one asked of the desktop's folder chooser, opening on the
/// folder chosen before, and taken when one is picked. A cancel leaves
/// the row as it was.
fn start_picked(st: &State, app: &App, choice: &str) {
    if choice != START_CHOSEN {
        keep(st, |s| s.start_folder.clear());
        show_start(app, "");
        tracing::info!("a launch opens the last folder");
        return;
    }
    let batch = st.window.batch;
    let take = move |app: &App, chosen: anyhow::Result<Option<PathBuf>>| match chosen {
        Ok(Some(dir)) => {
            let dir = dir.to_string_lossy().into_owned();
            if !batch && !cfg!(test) {
                let mut kept = settings::Settings::load();
                kept.start_folder = dir.clone();
                kept.save();
            }
            tracing::info!("a launch opens {dir}");
            show_start(app, &dir);
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!("file chooser: {e:#}");
            app.set_settings_note(chooser::chooser_failed(&e).into());
        }
    };
    #[cfg(test)]
    {
        let answer = START_CHOSEN_IN_TEST
            .with(|c| c.borrow_mut().take())
            .flatten();
        take(app, Ok(answer));
    }
    #[cfg(not(test))]
    {
        let kept = settings::Settings::load().start_folder;
        let start = Some(PathBuf::from(kept))
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let weak = app.as_weak();
        chooser::choose_folder("Start in", start, move |chosen| {
            let _ = weak.upgrade_in_event_loop(move |app| take(&app, chosen));
        });
    }
}

/// The filter bar shown or put away, over the grid and in culling's
/// section alike: kept as flipped. The filter it holds is the state's
/// and goes on applying either way.
pub(crate) fn filter_bar_changed(st: &State, app: &App) {
    let on = app.get_filter_bar();
    keep(st, |s| s.filter_bar = on);
    tracing::info!("filter bar {}", if on { "shown" } else { "hidden" });
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
        usage: greycard_library::thumbs::Split,
        cap: u64,
        preview_cap: u64,
    },
}

impl CacheShown {
    /// Every entry, thumbnails and previews: what Clear takes.
    fn entries(self) -> usize {
        match self {
            CacheShown::Known { usage, .. } => usage.total().entries,
            _ => 0,
        }
    }

    /// The cache's line as it holds now, from the count it keeps.
    fn of(c: &greycard_library::Thumbs) -> CacheShown {
        match c.known_split() {
            Some(usage) => CacheShown::Known {
                usage,
                cap: c.cap(),
                preview_cap: c.preview_cap(),
            },
            None => CacheShown::Counting,
        }
    }
}

/// The sheet's line about the thumbnail cache: what it holds against
/// its cap, the previews against theirs, or that it is off, or that
/// there is none.
pub(crate) fn thumb_cache_words(shown: CacheShown) -> String {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    match shown {
        CacheShown::Missing => "No cache folder: thumbnails are rebuilt each time.".into(),
        CacheShown::Counting => "Counting…".into(),
        CacheShown::Known { usage, cap: 0, .. } if usage.total().entries == 0 => {
            "Off: thumbnails are rebuilt each time.".into()
        }
        CacheShown::Known { usage, cap: 0, .. } => {
            let all = usage.total();
            format!(
                "Off. {} picture{} from before, {:.1} MB.",
                all.entries,
                plural(all.entries),
                mb(all.bytes)
            )
        }
        CacheShown::Known {
            usage,
            cap,
            preview_cap,
        } => {
            let thumbs = format!(
                "{} thumbnail{} kept, {:.1} MB of {:.0} MB.",
                usage.thumbs.entries,
                plural(usage.thumbs.entries),
                mb(usage.thumbs.bytes),
                mb(cap)
            );
            match (preview_cap, usage.previews.entries) {
                (0, 0) => thumbs,
                (0, n) => format!(
                    "{thumbs} {n} preview{} from before, {:.1} MB.",
                    plural(n),
                    mb(usage.previews.bytes)
                ),
                (pcap, n) => format!(
                    "{thumbs} {n} local preview{}, {:.1} MB of {:.0} MB.",
                    plural(n),
                    mb(usage.previews.bytes),
                    mb(pcap)
                ),
            }
        }
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
            Some(c) => CacheShown::of(c),
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
/// after. A cache the startup count has not finished with is not
/// walked again here: the line says "Counting…" until the count is
/// seeded, looked for a few times a second without holding the lock.
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
            let mut work = Some(work);
            let started = std::time::Instant::now();
            let shown = loop {
                let shown = match cache.lock() {
                    Ok(mut held) => match held.as_mut() {
                        None => CacheShown::Missing,
                        Some(c) => {
                            if let Some(work) = work.take() {
                                work(c);
                            }
                            CacheShown::of(c)
                        }
                    },
                    Err(_) => CacheShown::Missing,
                };
                // A count that never arrives (its thread could not be
                // started) leaves the line saying so, not a thread
                // spinning.
                if shown != CacheShown::Counting || started.elapsed().as_secs() >= 120 {
                    break shown;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
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

/// The thumbnails' cap in megabytes as last handed over or set, else
/// as the settings keep it. Never the cache's lock: a count or an
/// eviction holds that, and the file can be behind the live value.
fn thumb_cap_mb(worker: &Worker) -> u64 {
    worker
        .thumb_caps()
        .map(|(cap, _)| cap / (1024 * 1024))
        .unwrap_or_else(|| settings::Settings::load().thumb_cache_mb)
}

/// The previews' cap in megabytes, the same way.
fn preview_cap_mb(worker: &Worker) -> u64 {
    worker
        .thumb_caps()
        .map(|(_, cap)| cap / (1024 * 1024))
        .unwrap_or_else(|| settings::Settings::load().preview_cache_mb)
}

/// The minutes between the passes over the network roots, as the
/// window runs them now: zero for never.
fn poll_minutes(st: &State) -> u64 {
    st.views
        .library
        .poll_every
        .map_or(0, |every| every.as_secs().div_ceil(60))
}

/// The network roots' minutes as typed in the sheet: kept and put to
/// work when they are a whole number and changed, the field put back
/// when they are not a number.
pub(crate) fn network_poll_changed(st: &mut State, app: &App) {
    let kept = poll_minutes(st);
    let Some(minutes) = cap_typed(&app.get_network_poll_minutes()) else {
        app.set_network_poll_minutes(kept.to_string().into());
        return;
    };
    // The sheet closing asks too, changed or not.
    if minutes == kept {
        app.set_network_poll_minutes(kept.to_string().into());
        return;
    }
    app.set_network_poll_minutes(minutes.to_string().into());
    keep(st, |s| s.network_poll_minutes = minutes);
    st.views.library.poll_every = crate::roots::poll_every(minutes);
    tracing::info!(
        "library: network roots passed over {}",
        match minutes {
            0 => "at launch only".to_string(),
            m => format!("every {m} min"),
        }
    );
    crate::roots::restart_poll(st);
}

/// A JPEG beside its raw made a frame of its own, or the raw's
/// companion again: the choice kept, the index's reader told, the
/// roots' counts and the tree made again by it, and the open list read
/// again, so the JPEGs come into it or go out of it as a merge brings
/// any file in or takes one out. Nothing on disk is touched.
pub(crate) fn jpegs_apart_changed(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    apart: bool,
) {
    {
        let mut st = state.borrow_mut();
        if st.views.jpegs_apart == apart {
            return;
        }
        st.views.jpegs_apart = apart;
        keep(&st, |s| s.jpegs_apart = apart);
        tracing::info!(
            "a JPEG beside its raw is {}",
            if apart {
                "a frame of its own"
            } else {
                "the raw's companion"
            }
        );
        if let Some(lib) = st.views.index_reader.as_mut() {
            lib.set_jpegs_apart(apart);
        }
        crate::roots::recount(&mut st);
        crate::tree::recounted(&mut st, app);
        crate::roots::show(&st, app);
    }
    if !crate::roots::refresh_view(state, app, worker) {
        tracing::info!("the list is files given by hand, and is not read again");
    }
}

/// The folders the About category names: the settings', the library
/// index's, the caches' and the log's, each as `path_shown` writes it,
/// empty for one this machine has no place for.
pub(crate) fn about_dirs() -> [String; 4] {
    let shown = |p: Option<PathBuf>| p.map(|p| crate::report::path_shown(&p)).unwrap_or_default();
    let parent = |p: Option<PathBuf>| p.and_then(|p| p.parent().map(Path::to_path_buf));
    let caches = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::cache_dir)
        .map(|c| c.join("greycard"));
    [
        shown(parent(settings::path())),
        shown(parent(greycard_library::Library::user_path())),
        shown(caches),
        shown(crate::log::dir()),
    ]
}

/// The About category's words, which hold for the run.
fn show_about(app: &App) {
    app.set_about_version(crate::update::VERSION.into());
    let [settings, library, caches, log] = about_dirs();
    app.set_about_settings_dir(settings.into());
    app.set_about_library_dir(library.into());
    app.set_about_caches_dir(caches.into());
    app.set_about_log_dir(log.into());
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    show_about(app);
    // The category the sheet shows, kept for its next opening.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_settings_category_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let name = settings::category_named(&app.get_settings_category());
            keep(&state.borrow(), |s| s.settings_category = name.into());
        });
    }
    // The palette, in force as the row is clicked: the name put back
    // as the settings read it, and kept.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_theme_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let name = settings::theme_named(&app.get_theme());
            app.set_theme(name.into());
            keep(&state.borrow(), |s| s.theme = name.into());
            tracing::info!("theme {name}");
        });
    }
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
            let kept = thumb_cap_mb(&worker);
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
            worker.note_thumbs_cap(cap_bytes(mb));
            apply_thumb_cache(&app, CacheShown::Counting);
            // Two edits' threads can take the lock out of order: each
            // applies the newest cap, so the last one typed wins.
            let caps = worker.thumb_caps_shared();
            on_the_cache(&app, &worker, move |c| {
                if let Some((cap, _)) = *caps.lock().expect("thumbnail caps") {
                    c.set_cap(cap);
                }
            });
        });
    }
    // How often the roots on a network mount are passed over: kept in
    // the settings, and the timer started again on it now. Zero is
    // never; anything but a whole number puts the field back.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_network_poll_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            network_poll_changed(&mut state.borrow_mut(), &app);
        });
    }
    // A new cap for the local previews, apart from the thumbnails':
    // kept in the settings, and the previews evicted to it now. Zero
    // keeps none, and empties them.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_preview_cache_cap_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let kept = preview_cap_mb(&worker);
            let Some(mb) = cap_typed(&app.get_preview_cache_cap()) else {
                app.set_preview_cache_cap(kept.to_string().into());
                return;
            };
            if mb == kept {
                app.set_preview_cache_cap(kept.to_string().into());
                return;
            }
            app.set_preview_cache_cap(mb.to_string().into());
            keep(&state.borrow(), |s| s.preview_cache_mb = mb);
            tracing::info!("local previews' cap {mb} MB");
            worker.note_previews_cap(cap_bytes(mb));
            apply_thumb_cache(&app, CacheShown::Counting);
            let caps = worker.thumb_caps_shared();
            on_the_cache(&app, &worker, move |c| {
                if let Some((_, cap)) = *caps.lock().expect("thumbnail caps") {
                    c.set_preview_cap(cap);
                }
            });
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
            let mb = thumb_cap_mb(&worker);
            app.set_thumb_cache_cap(mb.to_string().into());
            app.set_preview_cache_cap(preview_cap_mb(&worker).to_string().into());
            let st = state.borrow();
            app.set_network_poll_minutes(poll_minutes(&st).to_string().into());
            app.set_sidecar_placement(placement_name(st.window.placement).into());
            app.set_xmp_sidecars(st.window.xmp_sidecars);
            app.set_jpegs_apart(st.views.jpegs_apart);
            app.set_crop_start(st.develop.crop_start.name().into());
            show_start(&app, &settings::Settings::load().start_folder);
            app.set_settings_note("".into());
            // A Check now still on its way keeps its "Checking...".
            if !app.get_update_checking() {
                app.set_update_note("".into());
            }
            show_elsewhere(&st, &app);
            crate::panel::assets::show_presets_missing(&st, &app);
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
            st.window.placement = placement_named(&app.get_sidecar_placement());
            let folder = st.window.placement == greycard_edit::Placement::Folder;
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
            st.window.xmp_sidecars = app.get_xmp_sidecars();
            let on = st.window.xmp_sidecars;
            keep(&st, |s| s.xmp_sidecars = on);
        });
    }
    // A JPEG beside its raw a frame of its own, or the raw's companion:
    // kept, and the open list read again under the new rule.
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_jpegs_apart_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            jpegs_apart_changed(&state, &app, &worker, app.get_jpegs_apart());
        });
    }
    // Where a launch opens: the last folder, or one chosen.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_start_picked(move |choice| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            start_picked(&state.borrow(), &app, &choice);
        });
    }
    // The filter bar shown or not: kept as flipped.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_filter_bar_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            filter_bar_changed(&state.borrow(), &app);
        });
    }
    // One section of the panel open at a time: kept as flipped.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_one_section_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let on = app.get_one_section();
            keep(&state.borrow(), |s| s.one_section = on);
        });
    }
    // The aspect an uncropped frame's crop opens at: kept as chosen.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_start_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            crate::panel::crop::start_chosen(&mut state.borrow_mut(), &app.get_crop_start());
        });
    }
    // The interface's scale: kept as chosen, for the next start, which
    // is when it applies; the sheet says so while it differs from the
    // one this window started at.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_ui_scale_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let percent = crate::window_fit::scale::named(&app.get_ui_scale());
            keep(&state.borrow(), |s| s.ui_scale = percent);
            tracing::info!("interface scale {percent}%, from the next start");
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
            if !st.window.write_sidecars {
                return;
            }
            // The renames are one round trip a frame, so on a thread
            // of their own, the sheet saying so meanwhile; a test
            // moves in place.
            if MOVING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let (files, placement) = (st.files.clone(), st.window.placement);
            drop(st);
            let land = move |app: &App, state: &Rc<RefCell<State>>, moved: Moved| {
                MOVING.store(false, std::sync::atomic::Ordering::SeqCst);
                tracing::info!(
                    "moved {} sidecars to {:?}, removed {} older copies, {} failed",
                    moved.moved,
                    placement,
                    moved.dropped,
                    moved.failed.len()
                );
                app.set_settings_note(moved_words(&moved, placement).into());
                show_elsewhere(&state.borrow(), app);
            };
            if cfg!(test) {
                land(&app, &state, move_sidecars(&files, placement));
                return;
            }
            app.set_settings_note("Moving the sidecars...".into());
            let app_weak = app.as_weak();
            let spawned = std::thread::Builder::new()
                .name("sidecars move".into())
                .spawn(move || {
                    let moved = move_sidecars(&files, placement);
                    let _ = slint::invoke_from_event_loop(move || {
                        // The state is the window thread's own; a
                        // closure holding an Rc could not cross here.
                        let (Some(app), Some(state)) = (
                            app_weak.upgrade(),
                            crate::STATE.with(|s| s.borrow().clone()),
                        ) else {
                            return;
                        };
                        land(&app, &state, moved);
                    });
                });
            if let Err(e) = spawned {
                tracing::warn!("sidecars move: {e}");
                MOVING.store(false, std::sync::atomic::Ordering::SeqCst);
                app.set_settings_note("The sidecars couldn't be moved; see the log.".into());
            }
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
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_sheet_opens_counts_and_moves() {
        let dir = scratch("sheet");
        let files = shoot(&dir);
        let app = crate::testing::window(files.len());
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;

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
        assert_eq!(state.borrow().window.placement, Placement::Folder);
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
        assert!(state.borrow().window.xmp_sidecars);

        // Escape closes it.
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_settings_open());
        crate::testing::remove_dir_retry(&dir);
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
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn no_sidecars_moves_nothing() {
        let dir = scratch("nosidecars");
        let files = shoot(&dir);
        let app = crate::testing::window(files.len());
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = false;
            st.window.placement = Placement::Folder;
        }
        app.invoke_settings_asked();
        assert!(!app.get_sidecars_written(), "the sheet is told");
        app.invoke_sidecars_move();
        assert!(Sidecar::path_in(&files[0], Placement::Beside).exists());
        assert_eq!(app.get_sidecars_elsewhere(), 2);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The scale row: on the sheet with the five choices at the one the
    /// window started at; a click on another says it waits for the next
    /// start, and the start's own again takes the line away. A factor
    /// set by hand says it wins instead.
    #[test]
    fn the_scale_row_says_a_change_waits_for_the_next_start() {
        use i_slint_backend_testing::ElementHandle;
        const NEXT: &str = "Applies at the next start.";
        const BY_HAND: &str = "SLINT_SCALE_FACTOR is set, and wins over this.";
        let app = crate::testing::window(0);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_ui_scale("150%".into());
        app.set_ui_scale_started("150%".into());
        app.invoke_settings_asked();
        assert!(app.get_settings_open());
        assert_eq!(crate::testing::count_labeled(&app, "INTERFACE"), 1);
        for choice in ["125%", "150%", "175%", "200%"] {
            assert_eq!(crate::testing::count_labeled(&app, choice), 1, "{choice}");
        }
        assert_eq!(crate::testing::count_labeled(&app, NEXT), 0);
        let click_on = |label: &str| {
            crate::testing::count_labeled(&app, label);
            let segment = ElementHandle::find_by_accessible_label(&app, label)
                .next()
                .unwrap_or_else(|| panic!("{label:?} on the sheet"));
            let (at, size) = (segment.absolute_position(), segment.size());
            crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        };
        click_on("200%");
        assert_eq!(app.get_ui_scale(), "200%");
        assert_eq!(crate::testing::count_labeled(&app, NEXT), 1);
        assert_eq!(crate::window_fit::scale::named(&app.get_ui_scale()), 200);
        click_on("150%");
        assert_eq!(app.get_ui_scale(), "150%");
        assert_eq!(crate::testing::count_labeled(&app, NEXT), 0);
        // By hand: the variable's line, and not the next start's.
        app.set_ui_scale_by_hand(true);
        click_on("125%");
        assert_eq!(crate::testing::count_labeled(&app, BY_HAND), 1);
        assert_eq!(crate::testing::count_labeled(&app, NEXT), 0);
    }

    /// Start in: the last folder until a folder is chosen; a click on
    /// Chosen folder asks, a cancel leaves the row as it was, a folder
    /// picked is shown, and Last folder goes back.
    #[test]
    fn the_start_row_takes_a_chosen_folder_or_the_last() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_settings_category("Library".into());
        app.invoke_settings_asked();
        assert_eq!(app.get_start_choice(), START_LAST);
        assert_eq!(crate::testing::count_labeled(&app, "Start in"), 1);
        assert_eq!(crate::testing::count_labeled(&app, START_CHOSEN), 1);
        // Canceled: still the last folder.
        app.invoke_start_picked(START_CHOSEN.into());
        assert_eq!(app.get_start_choice(), START_LAST);
        // A folder picked, shown under the row.
        let dir = scratch("start");
        START_CHOSEN_IN_TEST.with(|c| *c.borrow_mut() = Some(Some(dir.clone())));
        app.invoke_start_picked(START_CHOSEN.into());
        assert_eq!(app.get_start_choice(), START_CHOSEN);
        let shown = crate::report::path_shown(&dir);
        assert_eq!(app.get_start_folder(), shown.as_str());
        assert_eq!(crate::testing::count_labeled(&app, &shown), 1);
        // Back to the last folder: the path goes.
        app.invoke_start_picked(START_LAST.into());
        assert_eq!(app.get_start_choice(), START_LAST);
        assert_eq!(crate::testing::count_labeled(&app, &shown), 0);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The filter bar's switch: off, the bar is gone from the grid's
    /// header and from culling's section, and the filter set still
    /// holds; on, it is back in both.
    #[test]
    fn the_filter_bar_switch_hides_the_bar_in_the_grid_and_in_culling() {
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_filter_on(true);
        const TIP: &str = "Sort: Name, A to Z";
        app.set_sort_menu(SortMenu {
            tip: TIP.into(),
            ..Default::default()
        });
        // "Save..." is the grid's bar's alone; the Sort button is the
        // bar's, and the header's top row's while the bar is away.
        app.set_grid_open(true);
        let grid_on = crate::testing::count_labeled(&app, "Save...");
        assert_eq!(grid_on, 1);
        assert_eq!(crate::testing::count_labeled(&app, TIP), 1);
        let (bar_sort, _) = crate::testing::labeled(&app, TIP);
        app.set_filter_bar(false);
        app.invoke_filter_bar_changed();
        assert_eq!(crate::testing::count_labeled(&app, "Save..."), 0);
        assert_eq!(crate::testing::count_labeled(&app, TIP), 1, "Sort stays");
        let (row_sort, _) = crate::testing::labeled(&app, TIP);
        assert!(row_sort.y < bar_sort.y, "on the top row: {row_sort:?}");
        assert!(app.get_filter_on(), "the filter still applies");
        app.set_filter_bar(true);
        app.invoke_filter_bar_changed();
        assert_eq!(crate::testing::count_labeled(&app, "Save..."), 1);
        assert_eq!(crate::testing::count_labeled(&app, TIP), 1);
        // Culling's section, with its "Show" over a stacked bar.
        app.set_grid_open(false);
        app.set_panel_tab(crate::panel::cull::CULL_TAB.into());
        let cull_on = crate::testing::count_labeled(&app, "Stars");
        assert!(cull_on >= 1, "the stacked bar's Stars");
        app.set_filter_bar(false);
        app.invoke_filter_bar_changed();
        assert_eq!(crate::testing::count_labeled(&app, "Stars"), cull_on - 1);
        assert!(app.get_filter_on(), "the filter still applies");
        // The filter key, with no bar to send it to, opens nothing.
        app.invoke_filter_asked();
        assert!(!app.get_grid_open());
        drop(state);
    }

    /// The theme row: a click puts its palette in force at once, the
    /// window's `Theme` with it, and a name the settings do not know
    /// goes back to Charcoal.
    #[test]
    fn the_theme_row_sets_the_palette() {
        let app = crate::testing::window(0);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.invoke_settings_asked();
        assert_eq!(app.global::<crate::Theme>().get_name(), "Charcoal");
        use i_slint_backend_testing::ElementHandle;
        let charcoal = app.global::<crate::Theme>().get_surface();
        let chosen = |t: &crate::Theme| (t.get_selection(), t.get_focus_ring());
        let charcoal_chosen = chosen(&app.global::<crate::Theme>());
        assert_eq!(crate::testing::count_labeled(&app, "Theme"), 1);
        let row_y = ElementHandle::find_by_accessible_label(&app, "Theme")
            .next()
            .unwrap()
            .absolute_position()
            .y;
        // The row's segment, not a panel section's header of the same
        // word under the sheet.
        let click_on = |label: &str| {
            crate::testing::count_labeled(&app, label);
            let segment = ElementHandle::find_by_accessible_label(&app, label)
                .min_by(|a, b| {
                    let off = |e: &ElementHandle| (e.absolute_position().y - row_y).abs();
                    off(a).total_cmp(&off(b))
                })
                .unwrap_or_else(|| panic!("{label:?} on the sheet"));
            let (at, size) = (segment.absolute_position(), segment.size());
            crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        };
        click_on("Light");
        assert_eq!(app.get_theme(), "Light");
        let theme = app.global::<crate::Theme>();
        assert_eq!(theme.get_name(), "Light");
        assert!(theme.get_light());
        assert_ne!(theme.get_surface(), charcoal);
        // A chosen row and the focus show on Light's grounds too.
        let light_chosen = chosen(&theme);
        assert_ne!(light_chosen.0, charcoal_chosen.0);
        assert_ne!(light_chosen.1, charcoal_chosen.1);
        click_on("Grey");
        assert_eq!(app.global::<crate::Theme>().get_name(), "Grey");
        click_on("Charcoal");
        assert_eq!(app.global::<crate::Theme>().get_surface(), charcoal);
        app.set_theme("Mauve".into());
        app.invoke_theme_changed();
        assert_eq!(app.global::<crate::Theme>().get_name(), "Charcoal");
    }

    /// About: the version, and the folders the run keeps its files in,
    /// the home folder short.
    #[test]
    fn the_about_category_says_the_version_and_the_folders() {
        let app = crate::testing::window(0);
        let (_state, _worker) = crate::testing::state_for(&app, Vec::new());
        app.set_settings_category("About".into());
        app.invoke_settings_asked();
        assert_eq!(app.get_about_version(), crate::update::VERSION);
        assert_eq!(
            crate::testing::count_labeled(&app, crate::update::VERSION),
            1
        );
        assert_eq!(crate::testing::count_labeled(&app, "Check now"), 1);
        // Each folder greycard's own; on Windows the logs go in a
        // folder of their own under the library's, and the library and
        // the caches share `%LOCALAPPDATA%\greycard`, so a folder is
        // said once for each row that names it.
        let shown = about_dirs();
        let [_, library, caches, log] = &shown;
        for dir in [library, caches, log] {
            let logs = cfg!(windows) && dir.ends_with(r"greycard\logs");
            assert!(dir.ends_with("greycard") || logs, "{dir}");
            let rows = shown.iter().filter(|d| *d == dir).count();
            assert_eq!(crate::testing::count_labeled(&app, dir), rows, "{dir}");
        }
        if let Some(home) = dirs::home_dir() {
            assert!(!log.contains(&*home.to_string_lossy()), "{log}");
        }
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
        use greycard_library::thumbs::{Split, Usage};
        let mb = 1024 * 1024;
        let known = |bytes, entries, cap| CacheShown::Known {
            usage: Split {
                thumbs: Usage { bytes, entries },
                previews: Usage::default(),
            },
            cap,
            preview_cap: 0,
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
            "Off. 40 pictures from before, 3.0 MB."
        );
        assert_eq!(known(3 * mb, 40, 0).entries(), 40);
        // The previews against their own cap, counted in what Clear
        // takes.
        let both = CacheShown::Known {
            usage: Split {
                thumbs: Usage {
                    bytes: 5 * mb / 2,
                    entries: 312,
                },
                previews: Usage {
                    bytes: 18 * mb,
                    entries: 40,
                },
            },
            cap: 300 * mb,
            preview_cap: 8192 * mb,
        };
        assert_eq!(
            thumb_cache_words(both),
            "312 thumbnails kept, 2.5 MB of 300 MB. 40 local previews, 18.0 MB of 8192 MB."
        );
        assert_eq!(both.entries(), 352);
        assert_eq!(CacheShown::Counting.entries(), 0);
        assert_eq!(thumb_cache_words(CacheShown::Counting), "Counting…");
        assert!(thumb_cache_words(CacheShown::Missing).starts_with("No cache"));
    }

    /// The network roots' minutes: shown as the window runs them, taken
    /// when a whole number and changed (the timer's every with it), and
    /// put back when not a number; zero is never.
    #[test]
    fn the_network_poll_field_takes_whole_minutes() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().views.library.poll_every = crate::roots::poll_every(10);
        app.invoke_settings_asked();
        assert_eq!(app.get_network_poll_minutes(), "10");
        app.set_network_poll_minutes(" 25 ".into());
        app.invoke_network_poll_changed();
        assert_eq!(app.get_network_poll_minutes(), "25");
        assert_eq!(
            state.borrow().views.library.poll_every,
            Some(std::time::Duration::from_secs(25 * 60))
        );
        for bad in ["soon", "2.5", "-1", ""] {
            app.set_network_poll_minutes(bad.into());
            app.invoke_network_poll_changed();
            assert_eq!(app.get_network_poll_minutes(), "25", "{bad:?} put back");
            assert_eq!(
                state.borrow().views.library.poll_every,
                Some(std::time::Duration::from_secs(25 * 60))
            );
        }
        app.set_network_poll_minutes("0".into());
        app.invoke_network_poll_changed();
        assert_eq!(
            state.borrow().views.library.poll_every,
            None,
            "zero is never"
        );
        app.invoke_settings_asked();
        assert_eq!(app.get_network_poll_minutes(), "0");
        // Past what a clock holds: saturates, as the launch's does.
        app.set_network_poll_minutes(u64::MAX.to_string().into());
        app.invoke_network_poll_changed();
        assert!(state.borrow().views.library.poll_every.is_some());
    }

    /// The previews' cap field: shown as the cache has it, taken when a
    /// whole number and changed (the cache's previews capped by it, the
    /// thumbnails' cap left), and put back when not a number.
    #[test]
    fn the_previews_cap_field_takes_whole_megabytes() {
        let dir = scratch("preview-cap");
        let app = crate::testing::window(0);
        let (_state, worker) = crate::testing::state_for(&app, Vec::new());
        let mb = 1024 * 1024;
        let mut cache = greycard_library::Thumbs::at(dir.join("thumbs"), 300 * mb)
            .with_previews(crate::previews::SIZE, 8192 * mb);
        cache.seed_split(Default::default());
        worker.set_thumb_cache(Some(cache));
        app.invoke_settings_asked();
        assert_eq!(app.get_preview_cache_cap(), "8192");
        assert_eq!(app.get_thumb_cache_cap(), "300");
        let caps = || {
            let held = worker.thumb_cache();
            let held = held.lock().unwrap();
            let c = held.as_ref().unwrap();
            (c.cap() / mb, c.preview_cap() / mb)
        };
        let until = |want: (u64, u64)| {
            let start = std::time::Instant::now();
            while caps() != want {
                assert!(start.elapsed().as_secs() < 10, "{:?} for {want:?}", caps());
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        app.set_preview_cache_cap(" 2048 ".into());
        app.invoke_preview_cache_cap_changed();
        assert_eq!(app.get_preview_cache_cap(), "2048");
        until((300, 2048));
        for bad in ["lots", "1.5", "-3", ""] {
            app.set_preview_cache_cap(bad.into());
            app.invoke_preview_cache_cap_changed();
            assert_eq!(app.get_preview_cache_cap(), "2048", "{bad:?} put back");
        }
        assert_eq!(caps(), (300, 2048));
        app.set_preview_cache_cap("0".into());
        app.invoke_preview_cache_cap_changed();
        until((300, 0));
        crate::testing::remove_dir_retry(&dir);
    }

    /// The caps the sheet shows are the ones last set, even while a
    /// count or an eviction holds the cache: never the settings file,
    /// which a held lock used to fall back to.
    #[test]
    fn the_caps_shown_are_live_while_the_cache_is_held() {
        let dir = scratch("caps-held");
        let app = crate::testing::window(0);
        let (_state, worker) = crate::testing::state_for(&app, Vec::new());
        let mb = 1024 * 1024;
        let mut cache = greycard_library::Thumbs::at(dir.join("thumbs"), 300 * mb)
            .with_previews(crate::previews::SIZE, 8192 * mb);
        cache.seed_split(Default::default());
        worker.set_thumb_cache(Some(cache));
        app.set_preview_cache_cap("2048".into());
        app.invoke_preview_cache_cap_changed();
        app.set_thumb_cache_cap("500".into());
        app.invoke_thumb_cache_cap_changed();
        let shared = worker.thumb_cache();
        let held = shared.lock().unwrap();
        app.invoke_settings_asked();
        assert_eq!(app.get_preview_cache_cap(), "2048");
        assert_eq!(app.get_thumb_cache_cap(), "500");
        app.set_preview_cache_cap("lots".into());
        app.invoke_preview_cache_cap_changed();
        assert_eq!(
            app.get_preview_cache_cap(),
            "2048",
            "put back to the live cap"
        );
        app.set_thumb_cache_cap("lots".into());
        app.invoke_thumb_cache_cap_changed();
        assert_eq!(app.get_thumb_cache_cap(), "500", "put back to the live cap");
        drop(held);
        crate::testing::remove_dir_retry(&dir);
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

    /// A window with a frame open on the Develop tab, the panel's
    /// MONITOR and SOFT PROOF sections showing.
    fn develop_window() -> (App, Rc<RefCell<State>>, Rc<Worker>) {
        let app = crate::testing::window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        app.invoke_select(0);
        (app, state, worker)
    }

    /// The Settings sheet's row `id`: what it shows as chosen.
    fn row_shows(app: &App, id: &str) -> String {
        use i_slint_backend_testing::ElementHandle;
        crate::testing::count_labeled(app, id);
        ElementHandle::find_by_element_id(app, &format!("SettingsSheet::{id}"))
            .next()
            .unwrap_or_else(|| panic!("{id} on the sheet"))
            .accessible_value()
            .unwrap_or_default()
            .to_string()
    }

    /// A click on `option` in the sheet's row `id`.
    fn click_row(app: &App, id: &str, option: &str) {
        use i_slint_backend_testing::ElementHandle;
        crate::testing::count_labeled(app, option);
        let row = ElementHandle::find_by_element_id(app, &format!("SettingsSheet::{id}"))
            .next()
            .unwrap_or_else(|| panic!("{id} on the sheet"));
        let (top, foot) = (
            row.absolute_position().y,
            row.absolute_position().y + row.size().height,
        );
        let (left, right) = (
            row.absolute_position().x,
            row.absolute_position().x + row.size().width,
        );
        let segment = ElementHandle::find_by_accessible_label(app, option)
            .find(|e| {
                let (at, size) = (e.absolute_position(), e.size());
                let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
                (left..right).contains(&x) && (top..foot).contains(&y)
            })
            .unwrap_or_else(|| panic!("{option:?} in {id}"));
        let (at, size) = (segment.absolute_position(), segment.size());
        crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    }

    /// A click on the one `option` the panel shows, the sheet closed.
    fn click_panel(app: &App, option: &str) {
        let (at, size) = crate::testing::labeled(app, option);
        assert_eq!(crate::testing::count_labeled(app, option), 1, "{option}");
        crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    }

    fn open_editing(app: &App) {
        app.set_settings_category("Editing".into());
        app.invoke_settings_asked();
        assert!(app.get_settings_open());
    }

    /// The canvas, the monitor's profile, and the soft proof's profile
    /// and intent: a row of the sheet changes the setting the panel's
    /// control changes, and the panel's control shows on the row.
    #[test]
    fn the_editing_rows_are_the_panels_own_choices() {
        let (app, _state, _worker) = develop_window();
        assert_eq!(
            app.get_canvas_names()
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>(),
            crate::render::CANVAS_NAMES
        );
        let kept = || crate::panel::startup::remember(&app);
        // The sheet's rows, read back from the settings kept.
        open_editing(&app);
        for name in crate::render::CANVAS_NAMES.iter() {
            click_row(&app, "canvas-row", name);
            assert_eq!(kept().canvas_color, *name);
            assert_eq!(row_shows(&app, "canvas-row"), *name);
        }
        click_row(&app, "monitor-row", "Adobe RGB");
        assert_eq!(app.get_display_profile(), "Adobe RGB");
        assert_eq!(kept().display_profile, "Adobe RGB");
        click_row(&app, "proof-row", "Display P3");
        assert_eq!(kept().proof_profile, "Display P3");
        click_row(&app, "intent-row", "Relative");
        assert_eq!(kept().proof_intent, "Relative");
        // A file chosen shows its path under the row, as the panel's.
        app.set_display_file("/profiles/monitor.icc".into());
        click_row(&app, "monitor-row", "File");
        assert_eq!(kept().display_profile, "/profiles/monitor.icc");
        assert_eq!(
            crate::testing::count_labeled(&app, "/profiles/monitor.icc"),
            1
        );
        // The panel's controls, the sheet closed; the rows show them.
        app.set_settings_open(false);
        click_panel(&app, "System");
        assert_eq!(kept().display_profile, "System");
        click_panel(&app, "Rec.2020");
        assert_eq!(kept().proof_profile, "Rec.2020");
        click_panel(&app, "Perceptual");
        assert_eq!(kept().proof_intent, "Perceptual");
        // The canvas's first swatch, right of its word.
        let (at, size) = crate::testing::labeled(&app, "Canvas");
        crate::testing::click(&app, at.x + 68.0 + 8.0 + 4.0, at.y + size.height / 2.0);
        assert_eq!(kept().canvas_color, crate::render::CANVAS_NAMES[0]);
        open_editing(&app);
        assert_eq!(
            row_shows(&app, "canvas-row"),
            crate::render::CANVAS_NAMES[0]
        );
        assert_eq!(row_shows(&app, "monitor-row"), "System");
        assert_eq!(row_shows(&app, "proof-row"), "Rec.2020");
        assert_eq!(row_shows(&app, "intent-row"), "Perceptual");
    }

    /// The export's format and color space: a row of the sheet is the
    /// export sheet's choice, and the export sheet's shows on the row.
    #[test]
    fn the_export_rows_are_the_export_sheets_own() {
        use i_slint_backend_testing::ElementHandle;
        let (app, _state, _worker) = develop_window();
        let kept = || crate::panel::startup::remember(&app);
        open_editing(&app);
        assert_eq!(crate::testing::count_labeled(&app, "EXPORT"), 1);
        click_row(&app, "format-row", "PNG");
        assert_eq!(kept().export.format, "PNG");
        click_row(&app, "space-row", "Adobe RGB");
        assert_eq!(kept().export.space, "Adobe RGB");
        app.set_settings_open(false);
        // The export sheet shows them, and its clicks come back.
        app.set_export_open(true);
        assert_eq!(app.get_export_format(), "PNG");
        let near = |word: &str, option: &str| {
            crate::testing::count_labeled(&app, option);
            let at = ElementHandle::find_by_accessible_label(&app, word)
                .next()
                .unwrap_or_else(|| panic!("{word:?} on the export sheet"))
                .absolute_position();
            let segment = ElementHandle::find_by_accessible_label(&app, option)
                .min_by(|a, b| {
                    let off = |e: &ElementHandle| {
                        let p = e.absolute_position();
                        (p.y - at.y).abs() + (p.x - at.x).abs()
                    };
                    off(a).total_cmp(&off(b))
                })
                .unwrap_or_else(|| panic!("{option:?} on the export sheet"));
            let (p, size) = (segment.absolute_position(), segment.size());
            crate::testing::click(&app, p.x + size.width / 2.0, p.y + size.height / 2.0);
        };
        near("Format", "TIFF 16-bit");
        near("Color", "Rec.2020");
        assert_eq!(kept().export.format, "TIFF 16-bit");
        assert_eq!(kept().export.space, "Rec.2020");
        app.set_export_open(false);
        open_editing(&app);
        assert_eq!(row_shows(&app, "format-row"), "TIFF 16-bit");
        assert_eq!(row_shows(&app, "space-row"), "Rec.2020");
    }

    /// In the smallest window the fit allows, each category keeps the
    /// card in the window and Done in reach.
    #[test]
    fn every_category_fits_the_smallest_window() {
        use i_slint_backend_testing::ElementHandle;
        let (app, _state, _worker) = develop_window();
        app.window()
            .set_size(slint::LogicalSize::new(1024.0, 640.0));
        for category in ["Interface", "Library", "Editing", "Keys", "About"] {
            app.set_settings_open(false);
            app.set_settings_category(category.into());
            app.invoke_settings_asked();
            assert!(
                crate::testing::count_labeled(&app, "Done") > 0,
                "{category}"
            );
            for done in ElementHandle::find_by_accessible_label(&app, "Done") {
                let at = done.absolute_position();
                let foot = at.y + done.size().height;
                assert!(at.y >= 0.0 && foot <= 640.0, "{category}: Done at {foot}");
            }
        }
    }
}
