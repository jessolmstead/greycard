//! Edit in another editor, from the frame menu, as Lightroom hands a
//! frame to one: the frame exported with its edit as a 16-bit TIFF
//! beside its file, under its name and "-edit", through the export's
//! own path as a set of one; the file handed to the app picked from the
//! Settings sheet's list (`crate::edit_in` builds the command); and the file
//! watched, so that once the app has written it the index has it and
//! the list shows it beside its raw, stacked with it.
//!
//! The export is the sheet's defaults but for the format, TIFF 16-bit,
//! and the color space and the metadata, which are the sheet's: what
//! the last export went out with unless they have been changed since,
//! so a sheet set to None hands over a file with none. The name goes through
//! the set's naming, so a file there already is never written over.
//! The frame right-clicked is the one exported, a version as itself.
//!
//! The watch is one thread over every file handed over, looking at
//! each file's time and size once a second, off the window's thread
//! since the folder may be a share that stops answering. A save is
//! taken once its stamp has held across two looks, so a writer still
//! at work is waited for; the file is hashed then, and its raw once,
//! at its first look, as is the file as the export left it. A save
//! lets go of what the window holds under the content before it, so
//! the save shows. A file is let go once it has been gone for a
//! while, or a while after its last save was taken, and every file is
//! let go with the window.

#[cfg(test)]
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use greycard_library::collections::Reference;
use slint::ComponentHandle;

use crate::edit_in::{self as how, Platform, Stamp};
use crate::export::{Format, OnExists};
use crate::sheet::Sheet;
use crate::worker::Worker;
use crate::{App, Shared, State, chooser, queue, tr};

/// How often a watched file is looked at.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// How many looks in a row a file may be gone before it is let go. A
/// look counts only when the file's folder answers, so a share that
/// has dropped out keeps its watches.
const GONE_LOOKS: u32 = 60;
/// How many times a raw that will not read is tried for its hash.
const RAW_TRIES: u8 = 3;

/// How long after its last save was taken a file is let go.
const KEEP_AFTER: Duration = Duration::from_secs(60 * 60);

/// What goes after the frame's name.
pub(crate) const SUFFIX: &str = "-edit";

/// Edit in as the window holds it.
#[derive(Default)]
pub(crate) struct EditIn {
    /// The apps added on the Settings sheet, in the order added, each
    /// with its name, worked out as it is added or read; none offers
    /// no Edit in.
    pub(crate) apps: Vec<(PathBuf, String)>,
    /// Those of them the frame menu offers, by place: the ones on the
    /// disk when it was last built.
    offered: Vec<PathBuf>,
    /// The set exporting a frame for an app, until its frame is done,
    /// and that frame: its file, its version's id, and the app.
    set: Option<Arc<queue::Set>>,
    frame: Option<(PathBuf, Option<String>, PathBuf)>,
    /// The files handed to the app, one a name, shared with the
    /// thread that watches them; it lets go when this does.
    pub(crate) watches: Arc<Mutex<Vec<Watch>>>,
    /// Whether that thread is running.
    watching: bool,
    /// A file the app wrote was given to the indexer: the list is read
    /// again once its row is in.
    relist: bool,
}

/// A file handed to the app.
pub(crate) struct Watch {
    /// The watch's own number, so a look that read the disk for one
    /// never lands on another handed over under its name meanwhile.
    id: u64,
    pub(crate) out: PathBuf,
    /// The file it was exported from, as the list names it, and the
    /// version, none for the file's own.
    pub(crate) source: PathBuf,
    pub(crate) version: Option<String>,
    /// The raw's hash, read at the file's first look.
    raw_hash: Option<String>,
    /// How many looks have tried to read the raw.
    raw_tries: u8,
    /// The key the window's pictures of the file are kept under, its
    /// content hash and its stamp (`worker::thumb_key`): the export's,
    /// read at the first look while the file is still as written, then
    /// each save's. What a save lets go of.
    held: Option<(String, u64)>,
    /// Whether the first look has tried for the export's key.
    held_tried: bool,
    settle: how::Settle,
    /// How many looks in a row the file has been gone.
    gone: u32,
    /// When its last save was taken.
    taken: Option<Instant>,
}

/// A save of a watched file taken: the file, what it was exported
/// from, its hash and its raw's, and the key the window's pictures of
/// it were kept under before the save.
pub(crate) struct Changed {
    pub(crate) out: PathBuf,
    pub(crate) source: PathBuf,
    pub(crate) version: Option<String>,
    pub(crate) raw_hash: Option<String>,
    pub(crate) edit_hash: Option<String>,
    pub(crate) was: Option<(String, u64)>,
}

/// `path`'s content hash, none (and said) when it will not read.
fn hash(path: &Path) -> Option<String> {
    greycard_library::hash_file(path)
        .map_err(|e| tracing::warn!("edit in: {}: {e}", path.display()))
        .ok()
}

/// What a file's pictures are kept under: its content hash and its
/// stamp (`worker::thumb_key`).
type ThumbKey = (String, u64);

/// One watched file as a look found it: the watch, the file, its stamp
/// (none when the file is gone, unknown when its folder does not
/// answer, as on a share that dropped out), the raw's hash when the
/// look read it, and the export's key when the look tried for it.
type Seen = (
    u64,
    PathBuf,
    Option<Option<Stamp>>,
    Option<Option<String>>,
    Option<Option<(String, u64)>>,
);

/// What a look asks of one watched file: the watch, the file, its
/// source, whether to hash the raw, and the stamp the export left when
/// the export's key is still to be read.
type Asked = (u64, PathBuf, PathBuf, bool, Option<Option<Stamp>>);

/// The key of `out` (`worker::thumb_key`) while it is still as the
/// export left it, `handed`: none once the app has written it, which a
/// stamp before and after the read tells.
fn export_key(out: &Path, handed: Option<Stamp>) -> Option<(String, u64)> {
    let handed = handed?;
    if how::stamp_of(out) != Some(handed) {
        return None;
    }
    let key = crate::worker::thumb_key(out)?;
    (how::stamp_of(out) == Some(handed)).then_some(key)
}

/// One look at every watched file: the saves that have settled, each
/// hashed, and the files gone too long or kept long enough let go. The
/// disk is asked with the list let go, so a share that is slow to
/// answer never holds the window's thread at the lock.
pub(crate) fn look(watches: &Mutex<Vec<Watch>>) -> Vec<Changed> {
    let asked: Vec<Asked> = watches
        .lock()
        .expect("edit in watches")
        .iter()
        .map(|w| {
            let raw = w.raw_hash.is_none() && w.raw_tries < RAW_TRIES;
            let export = (!w.held_tried && w.taken.is_none()).then(|| w.settle.taken());
            (w.id, w.out.clone(), w.source.clone(), raw, export)
        })
        .collect();
    let seen: Vec<Seen> = asked
        .into_iter()
        .map(|(id, out, source, raw, export)| {
            let raw = raw.then(|| hash(&source));
            let export = export.map(|handed| export_key(&out, handed));
            let stamp = match how::stamp_of(&out) {
                Some(stamp) => Some(Some(stamp)),
                None => out.parent().is_none_or(|p| p.is_dir()).then_some(None),
            };
            (id, out, stamp, raw, export)
        })
        .collect();
    let mut taken = Vec::new();
    {
        let mut list = watches.lock().expect("edit in watches");
        for (id, out, stamp, raw, export) in seen {
            // Let go, or handed over again, meanwhile.
            let Some(w) = list.iter_mut().find(|w| w.id == id) else {
                continue;
            };
            if let Some(raw) = raw {
                w.raw_tries += 1;
                w.raw_hash = raw;
            }
            if let Some(export) = export {
                w.held_tried = true;
                w.held = w.held.take().or(export);
            }
            let Some(stamp) = stamp else {
                continue;
            };
            w.gone = if stamp.is_none() { w.gone + 1 } else { 0 };
            if w.settle.look(stamp) {
                w.taken = Some(Instant::now());
                taken.push((
                    id,
                    out,
                    w.source.clone(),
                    w.version.clone(),
                    w.raw_hash.clone(),
                    w.held.clone(),
                ));
            }
        }
        list.retain(|w| {
            let keep = w.gone < GONE_LOOKS && w.taken.is_none_or(|t| t.elapsed() < KEEP_AFTER);
            if !keep {
                tracing::info!("edit in: {} no longer watched", w.out.display());
            }
            keep
        });
    }
    let taken: Vec<(u64, Changed, Option<ThumbKey>)> = taken
        .into_iter()
        .map(|(id, out, source, version, raw_hash, was)| {
            let key = crate::worker::thumb_key(&out);
            if key.is_none() {
                tracing::warn!("edit in: {}: not hashed", out.display());
            }
            let changed = Changed {
                edit_hash: key.as_ref().map(|(h, _)| h.clone()),
                out,
                source,
                version,
                raw_hash,
                was,
            };
            (id, changed, key)
        })
        .collect();
    // Each save's key is what the next lets go of.
    let mut list = watches.lock().expect("edit in watches");
    for (id, _, key) in &taken {
        if let Some(w) = list.iter_mut().find(|w| w.id == *id) {
            w.held = key.clone();
        }
    }
    drop(list);
    taken.into_iter().map(|(_, c, _)| c).collect()
}

/// The name `path` is shown by: a desktop entry's own when it reads.
fn name_of(path: &Path) -> String {
    let entry = how::is_desktop_entry(path)
        .then(|| std::fs::read_to_string(path).ok())
        .flatten();
    how::app_name(path, entry.as_deref())
}

/// The apps `paths`, in order, each named once here: the Settings
/// sheet's list, and the frame menu's offer.
pub(crate) fn set_apps(app: &App, edit_in: &mut EditIn, paths: Vec<PathBuf>) {
    edit_in.apps = paths
        .into_iter()
        .map(|p| {
            let name = name_of(&p);
            (p, name)
        })
        .collect();
    show_list(app, edit_in);
    offer(app, edit_in);
}

/// The Settings sheet's list: every app, by name.
fn show_list(app: &App, edit_in: &EditIn) {
    let list: Vec<slint::SharedString> = edit_in.apps.iter().map(|(_, n)| n.into()).collect();
    app.set_edit_in_list(Rc::new(slint::VecModel::from(list)).into());
}

/// The frame menu's apps, as it is built: only those still on the
/// disk, so a stale one is never offered but stays on the list until
/// it is taken off. A stat an app; the names were worked out before.
pub(crate) fn offer(app: &App, edit_in: &mut EditIn) {
    let (paths, names): (Vec<PathBuf>, Vec<slint::SharedString>) = edit_in
        .apps
        .iter()
        .filter(|(p, _)| p.exists())
        .map(|(p, n)| (p.clone(), n.into()))
        .unzip();
    edit_in.offered = paths;
    app.set_edit_in_apps(Rc::new(slint::VecModel::from(names)).into());
}

/// The export Edit in writes: the sheet's defaults as TIFF 16-bit, in
/// the color space and with the metadata the sheet holds.
pub(crate) fn settings_for(app: &App) -> crate::export::Settings {
    Sheet {
        format: Format::Tiff.name().into(),
        space: app.get_export_space().into(),
        metadata: app.get_export_metadata().into(),
        ..Sheet::default()
    }
    .settings()
}

/// The stem the file is written under: its source's and "-edit"; a
/// version's name joins it as for any export of a version.
pub(crate) fn stem_of(source: &Path) -> String {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    format!("{stem}{SUFFIX}")
}

/// Where the export of `source` (a version named `version`) goes, as
/// the set names it: beside the file, under [`stem_of`].
pub(crate) fn out_of(source: &Path, version: Option<&str>) -> PathBuf {
    queue::named_as(
        &[(source.to_path_buf(), version.map(str::to_string))],
        &[stem_of(source)],
        None,
        None,
        Format::Tiff,
        OnExists::Increment,
    )
    .into_iter()
    .next()
    .map(|(p, _)| p)
    .unwrap_or_default()
}

/// The frame menu's Edit in with the app at `which` of those it
/// offers: the frame right-clicked exported, once its sidecar is in
/// hand.
pub(crate) fn asked(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, which: usize) {
    let (key, chosen) = {
        let mut held = state.borrow_mut();
        let st: &mut State = &mut held;
        let Some(chosen) = st.work.edit_in.offered.get(which).cloned() else {
            return;
        };
        // Gone since the menu was built: nothing written for it, and the
        // menu offers it no more.
        if !chosen.exists() {
            tracing::warn!(
                "edit in: {} is not there; nothing exported",
                chosen.display()
            );
            offer(app, &mut st.work.edit_in);
            return;
        }
        if st.work.exporting.is_some() || st.work.export_choosing {
            app.set_status(tr!("not exported: another export is under way").into());
            return;
        }
        let Some(key) = st.views.menu_file.or(st.current).and_then(|c| st.key_of(c)) else {
            return;
        };
        (key, chosen)
    };
    crate::rows::request(
        state,
        app,
        worker,
        vec![key.clone()],
        crate::rows::Scope::Line,
        crate::rows::Request::Export,
        Box::new(move |state, app, _, at| {
            let frames: Vec<usize> = at.into_iter().flatten().collect();
            start(state, app, &key, &frames, chosen);
        }),
    );
}

/// The export of the frame `pressed` for `chosen`, its sidecar in hand.
fn start(
    state: &Rc<Shared>,
    app: &App,
    pressed: &crate::versions::Key,
    frames: &[usize],
    chosen: PathBuf,
) {
    {
        let st = state.borrow();
        if st.work.exporting.is_some() || st.work.export_choosing {
            return;
        }
    }
    let Some((_, _, frames, _)) =
        crate::panel::deliver::pressed_frames(state, app, pressed, frames)
    else {
        return;
    };
    let Some(frame) = frames.into_iter().next() else {
        return;
    };
    let mut st = state.borrow_mut();
    let source = frame.0.clone();
    let version = frame.4.clone();
    let out = out_of(&source, version.as_ref().map(|v| v.name.as_str()));
    tracing::info!("edit in: {} to {}", source.display(), out.display());
    let settings = settings_for(app);
    let set = crate::panel::deliver::start_set(
        &mut st,
        app,
        crate::panel::deliver::Pending {
            frames: vec![frame],
            folder: None,
            sub: None,
            settings,
            preset: None,
            stems: vec![stem_of(&source)],
        },
        OnExists::Increment,
    );
    st.work.edit_in.frame = set
        .is_some()
        .then(|| (source, version.map(|v| v.id), chosen));
    st.work.edit_in.set = set;
}

/// A frame of `set` done: when it is Edit in's and was written, the
/// file is handed to the app and watched.
pub(crate) fn frame_done(st: &mut State, app: &App, set: &Arc<queue::Set>, done: &queue::Done) {
    let edit_in = &mut st.work.edit_in;
    if !edit_in.set.as_ref().is_some_and(|s| Arc::ptr_eq(s, set)) {
        return;
    }
    edit_in.set = None;
    let (Some((source, version, chosen)), queue::Done::Exported { path, .. }) =
        (edit_in.frame.take(), done)
    else {
        return;
    };
    {
        // One watch a name: an earlier one's stamp would take this
        // export for a save.
        let mut list = edit_in.watches.lock().expect("edit in watches");
        list.retain(|w| w.out != *path);
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        list.push(Watch {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            out: path.clone(),
            source,
            version,
            raw_hash: None,
            raw_tries: 0,
            held: None,
            held_tried: false,
            settle: how::Settle::from(how::stamp_of(path)),
            gone: 0,
            taken: None,
        });
    }
    if !std::mem::replace(&mut edit_in.watching, true) {
        watch(app, Arc::downgrade(&edit_in.watches));
    }
    match hand_over(&chosen, path) {
        Ok(()) => tracing::info!("edit in: {} handed to {}", path.display(), chosen.display()),
        Err(e) => tracing::warn!(
            "edit in: {} would not start for {}: {e}",
            chosen.display(),
            path.display()
        ),
    }
}

#[cfg(test)]
thread_local! {
    /// The commands a test's Edit in would have started.
    pub(crate) static LAUNCHED: RefCell<Vec<how::Launch>> = const { RefCell::new(Vec::new()) };
}

/// `file` opened in `chosen`, without waiting on it. A desktop entry
/// with no `gio` to launch it opens the file in the desktop's own app
/// for it instead.
fn hand_over(chosen: &Path, file: &Path) -> std::io::Result<()> {
    let launch = how::launch(chosen, file, Platform::this());
    #[cfg(test)]
    {
        LAUNCHED.with(|l| l.borrow_mut().push(launch));
        Ok(())
    }
    #[cfg(not(test))]
    match spawn(&launch) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && how::is_desktop_entry(chosen) => {
            tracing::warn!(
                "edit in: no {} to launch {}; the file opened with xdg-open",
                launch.program.display(),
                chosen.display()
            );
            spawn(&how::fallback(file))
        }
        other => other,
    }
}

/// `launch` started and reaped on a thread of its own, so it is never
/// left a zombie and the window never waits on it.
#[cfg(not(test))]
fn spawn(launch: &how::Launch) -> std::io::Result<()> {
    let mut cmd = launch.command();
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    let mut child = crate::adapter::scrub(&mut cmd).spawn()?;
    let program = launch.program.clone();
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => {
            tracing::warn!("edit in: {} said {status}", program.display());
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("edit in: {}: {e}", program.display()),
    });
    Ok(())
}

/// The watched files looked at once a tick on a thread of its own
/// while the list is held, each save taken brought to the window's
/// thread.
fn watch(app: &App, watches: std::sync::Weak<Mutex<Vec<Watch>>>) {
    if cfg!(test) {
        return;
    }
    let weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard edit in".into())
        .spawn(move || {
            loop {
                std::thread::sleep(WATCH_EVERY);
                let Some(list) = watches.upgrade() else {
                    return;
                };
                let taken = look(&list);
                drop(list);
                for c in taken {
                    let sent = weak.upgrade_in_event_loop(move |app| {
                        let (Some(state), Some(worker)) = (
                            crate::STATE.with(|s| s.borrow().clone()),
                            crate::WORKER.with(|w| w.borrow().clone()),
                        ) else {
                            return;
                        };
                        changed(&state, &app, &worker, c);
                    });
                    if sent.is_err() {
                        return;
                    }
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("edit in: the watch would not start: {e}");
    }
}

/// A watched file written by the app: given to the indexer, what the
/// window held of it before the save let go and asked for again, the
/// frame read again when it is the one open (`edited::file_rewritten`:
/// the file is listed as soon as greycard writes it under a watched
/// root, so even the app's first save is a rewrite), stacked with its
/// raw (after it in the raw's stack, or in a new stack of the two) and
/// the stack opened, and the list read again so it shows.
pub(crate) fn changed(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>, c: Changed) {
    {
        let mut held = state.borrow_mut();
        let st: &mut State = &mut held;
        let (source, version) = (c.source, c.version);
        tracing::info!("edit in: {} written", c.out.display());
        if let Some(index) = &st.views.index {
            index.file(c.out.clone());
            st.work.edit_in.relist = true;
        }
        crate::edited::file_rewritten(
            st,
            app,
            worker,
            &c.out,
            c.was.as_ref(),
            // A save before the first look leaves the export's key
            // unread: a hash that moved misses what was kept under the
            // old one, and one that did not (a change past the hashed
            // head at the same length) is the old one.
            c.edit_hash.as_deref(),
            c.edit_hash.as_deref(),
        );
        match (c.raw_hash, c.edit_hash) {
            (Some(raw_hash), Some(edit_hash)) => {
                let with = Reference {
                    hash: raw_hash,
                    hint: match st.file_by_path(&source) {
                        Some(f) => crate::stacks::hint_of_file(st, f)
                            .unwrap_or_else(|| crate::stacks::hint_of_path(st, &source)),
                        None => crate::stacks::hint_of_path(st, &source),
                    },
                    version,
                };
                let adding = Reference {
                    hash: edit_hash,
                    hint: crate::stacks::hint_of_path(st, &c.out),
                    version: None,
                };
                if st.views.stacks.stack_beside(with, adding).is_some() {
                    tracing::info!(
                        "edit in: {} stacked with {}",
                        c.out.display(),
                        source.display()
                    );
                    crate::panel::stacks::settle(st, app, worker);
                }
            }
            _ => tracing::warn!("edit in: {} not stacked: unread", c.out.display()),
        }
    }
    crate::roots::refresh_view(state, app, worker);
}

/// The indexer has written rows: a file the app wrote among them is
/// listed now.
pub(crate) fn indexed(state: &Rc<Shared>, app: &App) {
    let relist = std::mem::take(&mut state.borrow_mut().work.edit_in.relist);
    if relist && let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        crate::roots::refresh_view(state, app, &worker);
    }
}

#[cfg(test)]
thread_local! {
    /// What the app's chooser answers in a test, in place of asking.
    pub(crate) static CHOSEN_IN_TEST: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The Settings sheet's Edit in list's Add...: an app asked of the
/// desktop's file chooser, opening where the last one added is or
/// where the platform keeps its apps, and added when one is picked.
fn choose(state: &Rc<Shared>, app: &App) {
    let start = state
        .borrow()
        .work
        .edit_in
        .apps
        .last()
        .map(|(p, _)| p.as_path())
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| how::apps_folder(Platform::this()));
    #[cfg(test)]
    {
        let _ = start;
        let answer = CHOSEN_IN_TEST.with(|c| c.borrow_mut().take());
        chosen(state, app, Ok(answer));
    }
    #[cfg(not(test))]
    {
        let weak = app.as_weak();
        chooser::choose_open(
            tr!("Edit in"),
            start,
            (String::new(), Vec::new()),
            move |picked| {
                let _ = weak.upgrade_in_event_loop(move |app| {
                    if let Some(state) = crate::STATE.with(|s| s.borrow().clone()) {
                        chosen(&state, &app, picked);
                    }
                });
            },
        );
    }
}

/// The apps as the list now holds them: shown, and written to the
/// settings.
fn kept(st: &mut State, app: &App) {
    show_list(app, &st.work.edit_in);
    offer(app, &mut st.work.edit_in);
    let kept: Vec<String> = st
        .work
        .edit_in
        .apps
        .iter()
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .collect();
    crate::panel::crop::keep_in_file(st, |s| s.edit_in = kept);
}

/// The chooser's answer: an app added at the list's end, shown, and
/// written to the settings; one on the list already stays where it is;
/// a cancel leaves the list as it was.
fn chosen(state: &Rc<Shared>, app: &App, picked: anyhow::Result<Option<PathBuf>>) {
    match picked {
        Ok(Some(path)) => {
            let mut st = state.borrow_mut();
            if st
                .work
                .edit_in
                .apps
                .iter()
                .any(|(p, _)| how::same_app(p, &path))
            {
                return;
            }
            tracing::info!("edit in: {} added", path.display());
            let name = name_of(&path);
            st.work.edit_in.apps.push((path, name));
            kept(&mut st, app);
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!("file chooser: {e:#}");
            app.set_settings_note(chooser::chooser_failed(&e).into());
        }
    }
}

/// The list's × on the app at `at`: taken off, shown, and written.
fn remove(state: &Rc<Shared>, app: &App, at: usize) {
    let mut st = state.borrow_mut();
    if at >= st.work.edit_in.apps.len() {
        return;
    }
    let (gone, _) = st.work.edit_in.apps.remove(at);
    tracing::info!("edit in: {} taken off", gone.display());
    kept(&mut st, app);
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_menu_edit_in(move |which| {
            if let (Some(app), Ok(which)) = (app_weak.upgrade(), usize::try_from(which)) {
                asked(&state, &app, &worker, which);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_edit_in_add(move || {
            if let Some(app) = app_weak.upgrade() {
                choose(&state, &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_edit_in_remove(move |at| {
            if let (Some(app), Ok(at)) = (app_weak.upgrade(), usize::try_from(at)) {
                remove(&state, &app, at);
            }
        });
    }
}

#[cfg(test)]
mod tests;
