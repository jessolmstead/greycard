//! The settings sheet: the sidecars' place and the XMP switch, which
//! are the installation's rather than a picture's, and the move that
//! puts an open folder's sidecars where the place says.

#[cfg(test)]
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use greycard_edit::Sidecar;
use slint::ComponentHandle;

use crate::worker::Worker;
use crate::{App, Shared, State, chooser, grid, settings, tr};

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
            Ok(greycard_edit::Settled::InPlace | greycard_edit::Settled::Kept) => {}
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
        // Fills {place} in 'Moved N sidecars {place}'
        greycard_edit::Placement::Beside => tr!("beside their raws"),
        // Fills {place} in 'Moved N sidecars {place}'
        greycard_edit::Placement::Folder => tr!("into the hidden folder"),
    };
    let mut words = tr!(
        "Moved {n} sidecar {place}." | "Moved {n} sidecars {place}." % n,
        place = place
    );
    if moved.dropped > 0 {
        words.push_str(&tr!(" Removed {n} older copy left in the other place."
            | " Removed {n} older copies left in the other place." % moved.dropped));
    }
    if !moved.failed.is_empty() {
        let k = moved.failed.len();
        words.push_str(&tr!(" {k} could not be moved; the log says why.", k = k));
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
    settings::Settings::change(change);
}

/// The window's language, put in force once the first component is
/// made and before it shows: the setting's catalog (a regional tag
/// takes its language's), or the system's for System or for a setting
/// no catalog matches; English when neither has one. Both sides take
/// it, Slint's bundled catalogs and `tr!`, and Japanese, Chinese, or
/// Korean takes its region's typeface. The Language row is shown with
/// the setting.
pub(crate) fn start_language(app: &App, setting: &str) {
    let chosen = crate::i18n::pick(setting);
    let language = if setting == "en" {
        ""
    } else if chosen.is_empty() {
        crate::i18n::system()
    } else {
        chosen
    };
    crate::i18n::select(language);
    if let Err(e) = slint::select_bundled_translation(language) {
        tracing::warn!("language {language:?} not selected in the window: {e}");
    }
    tracing::info!(
        "language {:?} (setting {setting:?})",
        if language.is_empty() { "en" } else { language }
    );
    // Before anything is drawn: Slint keeps text it has shaped without
    // noting a change of fonts, so a string shaped earlier keeps its face.
    crate::i18n::face::apply(language);
    show_languages(app, setting);
}

/// The Language row's list: "System" in the window's language, English
/// (the source language), then each catalog's language in its own name.
/// A regional setting (`fr-CA`) shows its catalog's place; an unknown
/// one shows System, which is what it falls to.
pub(crate) fn show_languages(app: &App, setting: &str) {
    let languages = crate::i18n::languages();
    let mut names = vec![
        app.global::<crate::LanguageWords>().get_system(),
        "English".into(),
    ];
    names.extend(
        languages
            .iter()
            .map(|(_, name)| slint::SharedString::from(*name)),
    );
    app.set_languages(slint::ModelRc::new(slint::VecModel::from(names)));
    let chosen = crate::i18n::pick(setting);
    let at = if setting == "en" {
        1
    } else {
        languages
            .iter()
            .position(|(tag, _)| !chosen.is_empty() && *tag == chosen)
            .map_or(0, |i| i + 2)
    };
    app.set_language_index(i32::try_from(at).unwrap_or(0));
}

/// The setting for the Language row's choice at `index`: empty for
/// System, "en" for the source language, else the catalog's tag.
pub(crate) fn language_chosen(index: i32) -> String {
    match index {
        0 => String::new(),
        1 => "en".into(),
        i => usize::try_from(i)
            .ok()
            .and_then(|i| i.checked_sub(2))
            .and_then(|i| {
                crate::i18n::languages()
                    .get(i)
                    .map(|(tag, _)| tag.to_string())
            })
            .unwrap_or_default(),
    }
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
                settings::Settings::change(|s| s.start_folder = dir.clone());
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
        chooser::choose_folder(tr!("Start in"), start, move |chosen| {
            let _ = weak.upgrade_in_event_loop(move |app| take(&app, chosen));
        });
    }
}

/// The view a launch opens on, as clicked under Start in: kept at
/// once, a word the settings do not know taken as the first.
pub(crate) fn start_view_changed(st: &State, app: &App) {
    let picked = app.get_start_view();
    let name = settings::START_VIEWS
        .into_iter()
        .find(|v| *v == picked.as_str())
        .unwrap_or(settings::START_VIEWS[0]);
    app.set_start_view(name.into());
    keep(st, |s| s.start_view = name.into());
    tracing::info!("a launch opens on the {name}");
}

/// The filter bar shown or put away, over the grid and in culling's
/// section alike: kept as flipped. The filter it holds is the state's
/// and goes on applying either way.
pub(crate) fn filter_bar_changed(st: &State, app: &App) {
    let on = app.get_filter_bar();
    keep(st, |s| s.filter_bar = on);
    tracing::info!("filter bar {}", if on { "shown" } else { "hidden" });
}

/// The grid's cell as the Settings row shows it: whole logical
/// pixels. The wheel and the keys set it too, and the row follows.
pub(crate) fn show_grid_cell(app: &App) {
    app.set_grid_cell_typed(format!("{}", app.get_grid_cell().round()).into());
}

/// The cell size as typed in the sheet: held to the grid's range and
/// taken, the grid re-flowing; anything but a number puts the field
/// back. The size is kept with the window's other choices, as the
/// wheel's is.
pub(crate) fn grid_cell_typed_changed(app: &App) {
    let typed = app.get_grid_cell_typed();
    let px = typed.trim().parse::<f32>().ok().filter(|px| px.is_finite());
    if let Some(px) = px {
        let cell = grid::clamp_cell(px.round());
        if cell != app.get_grid_cell() {
            app.set_grid_cell(cell);
            tracing::info!("grid cell {cell} px");
        }
    }
    show_grid_cell(app);
}

/// The scale as the Settings row shows it, and the setting it holds.
pub(crate) fn show_ui_scale(app: &App, percent: u32) {
    let percent = crate::window_fit::scale::held(percent);
    app.set_ui_scale_percent(percent as i32);
    app.set_ui_scale(crate::window_fit::scale::name(percent).into());
}

/// The scale as typed in the sheet: held to its range and taken, for
/// the next start; anything but a number puts the field back. The
/// percentage taken, when it changed.
pub(crate) fn ui_scale_typed(app: &App) -> Option<u32> {
    let was = u32::try_from(app.get_ui_scale_percent()).unwrap_or(100);
    let now = crate::window_fit::scale::named(&app.get_ui_scale()).unwrap_or(was);
    show_ui_scale(app, now);
    (now != was).then_some(now)
}

/// Ctrl and the wheel sizing the grid's cells, or only scrolling:
/// kept as flipped.
pub(crate) fn wheel_resizes_changed(st: &State, app: &App) {
    let on = app.get_wheel_resizes();
    keep(st, |s| s.grid_wheel = on);
    tracing::info!("grid wheel {}", if on { "resizes" } else { "only scrolls" });
}

/// The Author and Copyright fields as typed: kept, ends trimmed, when
/// either changed, for the next export; empty is none and leaves the
/// key out of the file. The field shows what was kept.
pub(crate) fn rights_changed(st: &mut State, app: &App) {
    let author = app.get_settings_author().trim().to_string();
    let copyright = app.get_settings_copyright().trim().to_string();
    app.set_settings_author(author.as_str().into());
    app.set_settings_copyright(copyright.as_str().into());
    // The sheet closing asks too, changed or not.
    if (author.as_str(), copyright.as_str())
        == (st.window.author.as_str(), st.window.copyright.as_str())
    {
        return;
    }
    st.window.author.clone_from(&author);
    st.window.copyright.clone_from(&copyright);
    keep(st, |s| keep_rights(s, &author, &copyright));
    tracing::info!(
        "exports' author {}, copyright {}",
        if author.is_empty() { "none" } else { "set" },
        if copyright.is_empty() { "none" } else { "set" }
    );
}

/// The author and copyright into the settings file's fields.
fn keep_rights(s: &mut settings::Settings, author: &str, copyright: &str) {
    s.shared.author = author.to_string();
    s.shared.copyright = copyright.to_string();
}

/// A raw's new edit with the lens section on or off: kept as flipped.
pub(crate) fn lens_on_new_changed(st: &mut State, app: &App) {
    let on = app.get_lens_on_new();
    st.window.lens_on_new = on;
    keep(st, |s| s.shared.lens_on_new = on);
    tracing::info!(
        "lens corrections on new edits: {}",
        if on { "on" } else { "off" }
    );
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
    let one = |b: u64| format!("{:.1}", mb(b));
    let whole = |b: u64| format!("{:.0}", mb(b));
    match shown {
        CacheShown::Missing => tr!("No cache folder: thumbnails are rebuilt each time."),
        CacheShown::Counting => tr!("Counting..."),
        CacheShown::Known { usage, cap: 0, .. } if usage.total().entries == 0 => {
            tr!("Off: thumbnails are rebuilt each time.")
        }
        CacheShown::Known { usage, cap: 0, .. } => {
            let all = usage.total();
            tr!(
                "Off. {n} picture from before, {mb} MB."
                    | "Off. {n} pictures from before, {mb} MB." % all.entries,
                mb = one(all.bytes)
            )
        }
        CacheShown::Known {
            usage,
            cap,
            preview_cap,
        } => {
            let thumbs = tr!(
                "{n} thumbnail kept, {mb} MB of {cap} MB."
                    | "{n} thumbnails kept, {mb} MB of {cap} MB." % usage.thumbs.entries,
                mb = one(usage.thumbs.bytes),
                cap = whole(cap)
            );
            match (preview_cap, usage.previews.entries) {
                (0, 0) => thumbs,
                (0, n) => tr!(
                    "{thumbs} {n} preview from before, {mb} MB."
                        | "{thumbs} {n} previews from before, {mb} MB." % n,
                    thumbs = thumbs,
                    mb = one(usage.previews.bytes)
                ),
                (pcap, n) => tr!(
                    "{thumbs} {n} local preview, {mb} MB of {cap} MB."
                        | "{thumbs} {n} local previews, {mb} MB of {cap} MB." % n,
                    thumbs = thumbs,
                    mb = one(usage.previews.bytes),
                    cap = whole(pcap)
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
/// walked again here: the line says "Counting..." until the count is
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
pub(crate) fn jpegs_apart_changed(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, apart: bool) {
    {
        let mut st = state.borrow_mut();
        if st.views.jpegs_apart == apart {
            return;
        }
        st.views.jpegs_apart = apart;
        keep(&st, |s| s.shared.jpegs_apart = apart);
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

/// The folders the About category names, in its rows' order: the
/// settings', the library index's, the caches', and the log's, none
/// for one this machine has no place for.
fn about_paths() -> [Option<PathBuf>; 4] {
    let parent = |p: Option<PathBuf>| p.and_then(|p| p.parent().map(Path::to_path_buf));
    let caches = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::cache_dir)
        .map(|c| c.join("greycard"));
    [
        parent(settings::path()),
        parent(greycard_library::Library::user_path()),
        caches,
        crate::log::dir(),
    ]
}

/// The same, each as `path_shown` writes it, empty for none.
pub(crate) fn about_dirs() -> [String; 4] {
    about_paths().map(|p| p.map(|p| crate::report::path_shown(&p)).unwrap_or_default())
}

/// The About category's words, which hold for the run.
fn show_about(app: &App) {
    app.set_about_version(greycard_version::LABEL.into());
    let [settings, library, caches, log] = about_dirs();
    app.set_about_settings_dir(settings.into());
    app.set_about_library_dir(library.into());
    app.set_about_caches_dir(caches.into());
    app.set_about_log_dir(log.into());
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    show_about(app);
    // An About folder clicked opens in the file manager; one not made
    // yet (no log written, no cache kept) is made first, so the click
    // never lands on nothing.
    app.on_about_dir_clicked(|row| {
        let Some(dir) = usize::try_from(row)
            .ok()
            .and_then(|r| about_paths().into_iter().nth(r).flatten())
        else {
            return;
        };
        if let Err(e) =
            std::fs::create_dir_all(&dir).and_then(|()| crate::panel::menu::open_folder(&dir))
        {
            tracing::warn!("settings: {} would not open: {e}", dir.display());
        }
    });
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
    // The language, kept as it is chosen and in force from the next
    // start, as the row says.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_language_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let tag = language_chosen(app.get_language_index());
            tracing::info!("language {tag:?} chosen, from the next start");
            keep(&state.borrow(), |s| s.language = tag);
        });
    }
    // The scroll bars' way and size, as the palette is.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_scroll_bars_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let name = settings::scroll_bars_named(&app.get_scroll_bars());
            app.set_scroll_bars(name.into());
            keep(&state.borrow(), |s| s.scroll_bars = name.into());
            tracing::info!("scroll bars {name}");
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_scroll_bar_size_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let name = settings::scroll_bar_size_named(&app.get_scroll_bar_size());
            app.set_scroll_bar_size(name.into());
            keep(&state.borrow(), |s| s.scroll_bar_size = name.into());
            tracing::info!("scroll bar size {name}");
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
    // Fetch all, in the Models category.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_models_fetch_all(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            crate::panel::models::fetch_all(&mut state.borrow_mut(), &app);
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
            show_grid_cell(&app);
            app.set_sidecar_placement(placement_name(st.window.placement).into());
            app.set_xmp_sidecars(st.window.xmp_sidecars);
            app.set_jpegs_apart(st.views.jpegs_apart);
            app.set_crop_start(st.develop.crop_start.name().into());
            app.set_lens_on_new(st.window.lens_on_new);
            app.set_settings_author(st.window.author.as_str().into());
            app.set_settings_copyright(st.window.copyright.as_str().into());
            crate::panel::retouch::show_patches(&st, &st.develop.edit, &app);
            let kept = settings::Settings::load();
            show_start(&app, &kept.start_folder);
            app.set_start_view(kept.start_view.into());
            app.set_settings_note("".into());
            // A Check now still on its way keeps its "Checking...".
            if !app.get_update_checking() {
                app.set_update_note("".into());
            }
            show_elsewhere(&st, &app);
            crate::panel::assets::show_presets_missing(&st, &app);
            drop(st);
            crate::panel::models::show(&mut state.borrow_mut(), &app);
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
            keep(&st, |s| s.shared.sidecars_in_folder = folder);
            if let Some(index) = &st.views.index {
                index.set_lightroom_takes(st.window.sidecars());
            }
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
            keep(&st, |s| s.shared.xmp_sidecars = on);
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
    // The view a launch opens on: kept as clicked.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_start_view_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                start_view_changed(&state.borrow(), &app);
            }
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
    // The cell size typed, and the wheel's switch.
    {
        let app_weak = app.as_weak();
        app.on_grid_cell_typed_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                grid_cell_typed_changed(&app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_wheel_resizes_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            wheel_resizes_changed(&state.borrow(), &app);
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
    // A raw's new edit with the lens section on or off: kept as
    // flipped, for the next raw with no sidecar; one written keeps its
    // own.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_lens_on_new_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                lens_on_new_changed(&mut state.borrow_mut(), &app);
            }
        });
    }
    // The author and copyright every export carries: kept as typed,
    // and the next export writes them.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_rights_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                rights_changed(&mut state.borrow_mut(), &app);
            }
        });
    }
    // The interface's scale: kept as typed, for the next start, which
    // is when it applies; the sheet says so while it differs from the
    // one this window started at.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_ui_scale_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(percent) = ui_scale_typed(&app) else {
                return;
            };
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
            let land = move |app: &App, state: &Rc<Shared>, moved: Moved| {
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
            app.set_settings_note(tr!("Moving the sidecars...").into());
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
                app.set_settings_note(tr!("The sidecars couldn't be moved; see the log.").into());
            }
        });
    }
}

#[cfg(test)]
mod tests;
