//! The import sheet: where the frames come from and go, and the
//! import itself on a thread of its own, its progress on the status
//! line; and `--import`, the same without a window.
//!
//! The window's thread never touches the card: looking for one, reading
//! what it holds and the copy all run on their own threads and post
//! back, so a slow or sleeping mount holds up nothing but itself.

use crate::import::{self, Job, Options, Report, Scan};
use crate::naming::Fields;
use crate::panel::browser::open_folder;
use crate::*;

/// The sheet's side of the state.
#[derive(Default)]
pub(crate) struct Sheet {
    pub(crate) source: Option<PathBuf>,
    pub(crate) destination: Option<PathBuf>,
    pub(crate) backup: Option<PathBuf>,
    /// What the source holds, and its first frame's tokens, once read.
    pub(crate) scan: Option<Scan>,
    pub(crate) first: Option<Fields>,
    /// The source reading the window last asked for, so an older one
    /// that answers late says nothing.
    pub(crate) generation: u64,
    /// The remembered choices are on the sheet already.
    pub(crate) filled: bool,
    /// The import running, held to stop it.
    pub(crate) job: Option<Arc<Job>>,
    /// Its thread, joined when the window closes mid-import.
    pub(crate) thread: Option<std::thread::JoinHandle<()>>,
    /// When the sheet was asked for, for the log's timing of the card
    /// search and the reading.
    pub(crate) asked_at: Option<std::time::Instant>,
    /// Whether the destination and the backup are there, looked at on
    /// a thread (a remembered `/mnt/backup` may be a drive not
    /// mounted, and a stat of a network one can wait); none until
    /// looked at. And whether the two are on one drive.
    pub(crate) destination_there: Option<bool>,
    pub(crate) backup_there: Option<bool>,
    pub(crate) same_drive: bool,
    pub(crate) folders_generation: u64,
    /// The frame being copied, for the words while the window waits
    /// on it to close.
    pub(crate) in_hand: String,
    /// The window was asked to close and waits for the frame in hand;
    /// or gave up waiting on it.
    pub(crate) closing: bool,
    pub(crate) gave_up: bool,
    /// The folder the browser showed when the import began: the
    /// import opens its own at the end only if the user is still there.
    pub(crate) folder_at_start: Option<PathBuf>,
}

/// How long a closing window waits for the frame in hand before it
/// gives up and says so in the log. A 100 MB raw off a slow card
/// reader at 10 MB/s is ten seconds; a read that hangs is forever.
pub(crate) const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The picker's word for no preset.
const NONE: &str = "None";

fn path_text(p: &Option<PathBuf>) -> slint::SharedString {
    p.as_deref().map(tilde).unwrap_or_default().into()
}

/// A path as the sheet shows it: the home folder as `~`, so the end
/// of a long path, which is the part that differs, has the room.
fn tilde(p: &Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) if !rest.as_os_str().is_empty() => {
            format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
        }
        _ => p.display().to_string(),
    }
}

/// What the note under the source says of what it holds.
pub(crate) fn note_text(scan: &Scan) -> String {
    let n = scan.items.len();
    if n == 0 {
        return "no frames here the editor opens".into();
    }
    let files = scan.files();
    let mut s = import::frames(n);
    if files > n {
        s.push_str(&format!(
            ", {files} files with the JPEGs and XMPs that go with their raws"
        ));
    }
    if !scan.left.is_empty() {
        s.push_str(&format!(
            "; {} left on the card (videos, and files the editor does not open)",
            scan.left.len()
        ));
    }
    s
}

/// The options the sheet makes, when it makes any.
fn options(st: &State, app: &App) -> Option<Options> {
    let preset_name = app.get_import_preset();
    let preset = st
        .presets
        .iter()
        .find(|e| e.preset.name == preset_name.as_str())
        .map(|e| e.preset.clone());
    Some(Options {
        source: st.import.source.clone()?,
        destination: st.import.destination.clone()?,
        subfolder: app.get_import_subfolder().to_string(),
        name: app.get_import_name().to_string(),
        preset,
        backup: st.import.backup.clone(),
        library: st.index_path.clone(),
        verify: app.get_import_verify(),
        placement: st.write_sidecars.then_some(st.placement),
        profiles: st.profiles.clone(),
    })
}

/// Where the first frame would go, or why nothing can; and whether
/// the sheet is ready to import.
pub(crate) fn preview(
    source: Option<&Path>,
    destination: Option<&Path>,
    subfolder: &str,
    name: &str,
    scan: Option<&Scan>,
    first: Option<&Fields>,
) -> (String, bool) {
    let Some(_) = source else {
        return (String::new(), false);
    };
    let (Some(scan), Some(first)) = (scan, first) else {
        return (String::new(), false);
    };
    let Some(item) = scan.items.first() else {
        return (String::new(), false);
    };
    let opts = Options {
        source: PathBuf::new(),
        destination: destination.map(Path::to_path_buf).unwrap_or_default(),
        subfolder: subfolder.into(),
        name: name.into(),
        preset: None,
        backup: None,
        library: None,
        placement: None,
        profiles: Vec::new(),
        verify: false,
    };
    match import::relative(&opts, &item.file, first) {
        Err(e) => (format!("Not a pattern: {e}"), false),
        Ok(rel) => match destination {
            None => (
                format!("{} becomes {}", item_name(&item.file), rel.display()),
                false,
            ),
            Some(d) => (
                format!("{} becomes {}", item_name(&item.file), tilde(&d.join(rel))),
                true,
            ),
        },
    }
}

fn item_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Put what the state knows on the sheet: the folders, the note, the
/// preview and whether Import is on.
pub(crate) fn show(st: &State, app: &App) {
    // A folder that is not there is said so, and never made: a
    // remembered backup drive that is not plugged in would otherwise
    // be recreated as a folder on the system disk.
    // Said on a line of its own: the end of a long path is elided.
    let missing = |p: &Option<PathBuf>, there: Option<bool>| p.is_some() && there == Some(false);
    app.set_import_source(path_text(&st.import.source));
    app.set_import_destination(path_text(&st.import.destination));
    app.set_import_backup(path_text(&st.import.backup));
    let note = if missing(&st.import.destination, st.import.destination_there) {
        "The destination folder is not there: is its drive connected? It is not made anew here."
    } else if missing(&st.import.backup, st.import.backup_there) {
        "The backup folder is not there: is its drive connected? It is not made anew here."
    } else if st.import.backup.is_some() && st.import.same_drive {
        "The backup is on the same drive as the destination: one failing drive takes both."
    } else {
        ""
    };
    app.set_import_folders_note(note.into());
    let count = st.import.scan.as_ref().map_or(0, |s| s.items.len());
    app.set_import_count(count as i32);
    let (text, ready) = preview(
        st.import.source.as_deref(),
        st.import.destination.as_deref(),
        &app.get_import_subfolder(),
        &app.get_import_name(),
        st.import.scan.as_ref(),
        st.import.first.as_ref(),
    );
    app.set_import_preview(text.into());
    let folders = st.import.destination_there == Some(true)
        && (st.import.backup.is_none() || st.import.backup_there == Some(true));
    app.set_import_ready(ready && folders && count > 0 && st.import.job.is_none());
}

/// Look at whether the destination and the backup are there, and on
/// which drive, on a thread; the answer is heard by [`folders_looked`].
pub(crate) fn look_at_folders(st: &mut State, app: &App) {
    st.import.folders_generation += 1;
    let generation = st.import.folders_generation;
    st.import.destination_there = None;
    st.import.backup_there = None;
    st.import.same_drive = false;
    show(st, app);
    let (dest, backup) = (st.import.destination.clone(), st.import.backup.clone());
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let looked = look(dest.as_deref(), backup.as_deref());
        let _ = weak.upgrade_in_event_loop(move |app| {
            if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                folders_looked(&mut state.borrow_mut(), &app, generation, looked);
            }
        });
    });
}

/// What [`look_at_folders`] finds: the destination there, the backup
/// there, the two on one drive.
pub(crate) type Looked = (Option<bool>, Option<bool>, bool);

pub(crate) fn look(dest: Option<&Path>, backup: Option<&Path>) -> Looked {
    let there = |p: Option<&Path>| p.map(Path::is_dir);
    let same = match (dest, backup) {
        (Some(d), Some(b)) if d.is_dir() && b.is_dir() => {
            import::same_device(d, b).unwrap_or(false)
        }
        _ => false,
    };
    (there(dest), there(backup), same)
}

pub(crate) fn folders_looked(st: &mut State, app: &App, generation: u64, looked: Looked) {
    if generation != st.import.folders_generation {
        return;
    }
    (
        st.import.destination_there,
        st.import.backup_there,
        st.import.same_drive,
    ) = looked;
    if st.import.same_drive {
        tracing::warn!("import: the backup is on the same drive as the destination");
    }
    show(st, app);
}

/// Read what `source` holds, on a thread; the answer is heard by
/// [`scanned`] unless another reading was asked for since.
fn read_source(st: &mut State, app: &App, source: PathBuf) {
    st.import.generation += 1;
    let generation = st.import.generation;
    st.import.source = Some(source.clone());
    st.import.scan = None;
    st.import.first = None;
    app.set_import_note(format!("reading {}...", source.display()).into());
    show(st, app);
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let read = import::scan(&source).map(|scan| {
            let first = scan
                .items
                .first()
                .map(|item| import::fields_of(&item.file, 1));
            (scan, first)
        });
        let seconds = started.elapsed().as_secs_f64();
        let _ = weak.upgrade_in_event_loop(move |app| {
            if let Some(state) = STATE.with(|s| s.borrow().clone()) {
                scanned(&mut state.borrow_mut(), &app, generation, read, seconds);
            }
        });
    });
}

/// What the source reading found, on the window's thread.
pub(crate) fn scanned(
    st: &mut State,
    app: &App,
    generation: u64,
    read: Result<(Scan, Option<Fields>)>,
    seconds: f64,
) {
    if generation != st.import.generation {
        return;
    }
    match read {
        Ok((scan, first)) => {
            tracing::info!(
                "import: {} holds {} ({} files, {} left) read in {:.2} s{}",
                st.import
                    .source
                    .as_deref()
                    .unwrap_or(Path::new(""))
                    .display(),
                import::frames(scan.items.len()),
                scan.files(),
                scan.left.len(),
                seconds,
                st.import
                    .asked_at
                    .map(|t| format!(
                        ", {:.2} s after the sheet was asked for",
                        t.elapsed().as_secs_f64()
                    ))
                    .unwrap_or_default()
            );
            app.set_import_note(note_text(&scan).into());
            st.import.scan = Some(scan);
            st.import.first = first;
        }
        Err(e) => {
            app.set_import_note(format!("{e:#}").into());
            st.import.scan = None;
            st.import.first = None;
        }
    }
    show(st, app);
}

/// Open the sheet: the remembered choices the first time, a card
/// looked for when no source is chosen yet, the source read again
/// when one is (a card taken out and another put in).
pub(crate) fn ask(st: &mut State, app: &App) {
    if st.import.job.is_some() {
        return;
    }
    st.import.asked_at = Some(std::time::Instant::now());
    if !st.import.filled {
        st.import.filled = true;
        // From the file this run writes to, so a snapshot or a test
        // shows the defaults and never the user's.
        let kept = st
            .settings_file
            .as_deref()
            .map(settings::Settings::load_from)
            .unwrap_or_default()
            .import;
        let path = |s: &str| (!s.is_empty()).then(|| PathBuf::from(s));
        // Where the last import went, else the desktop's pictures.
        if st.import.destination.is_none() {
            st.import.destination =
                path(&kept.destination)
                    .or_else(dirs::picture_dir)
                    .or_else(|| {
                        // A desktop with no user-dirs file still has one.
                        dirs::home_dir()
                            .map(|h| h.join("Pictures"))
                            .filter(|p| p.is_dir())
                    });
        }
        if st.import.backup.is_none() {
            st.import.backup = path(&kept.backup);
        }
        app.set_import_subfolder(kept.subfolder.into());
        app.set_import_name(kept.name.into());
        app.set_import_preset(if kept.preset.is_empty() {
            NONE.into()
        } else {
            kept.preset.into()
        });
    }
    let mut names = vec![slint::SharedString::from(NONE)];
    names.extend(
        st.presets
            .iter()
            .map(|e| slint::SharedString::from(e.preset.name.as_str())),
    );
    if !names.iter().any(|n| *n == app.get_import_preset()) {
        app.set_import_preset(NONE.into());
    }
    app.set_import_presets(ModelRc::new(VecModel::from(names)));
    app.set_import_open(true);
    look_at_folders(st, app);
    match st.import.source.clone() {
        Some(source) => read_source(st, app, source),
        None => {
            app.set_import_note("looking for a card...".into());
            show(st, app);
            let weak = app.as_weak();
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                let volumes = import::volumes();
                let card = import::find_card(&volumes);
                let seconds = started.elapsed().as_secs_f64();
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                        return;
                    };
                    let mut st = state.borrow_mut();
                    found(&mut st, &app, card, volumes.len(), seconds);
                });
            });
        }
    }
}

/// What the card search found, on the window's thread. A source
/// chosen by hand meanwhile wins.
pub(crate) fn found(
    st: &mut State,
    app: &App,
    card: Option<PathBuf>,
    volumes: usize,
    seconds: f64,
) {
    tracing::info!(
        "import: {} volumes looked at in {:.3} s: {}",
        volumes,
        seconds,
        card.as_deref()
            .map_or_else(|| "no card".to_string(), |c| c.display().to_string())
    );
    if st.import.source.is_some() {
        return;
    }
    match card {
        Some(card) => read_source(st, app, card),
        None => {
            app.set_import_note("no card found; choose the folder to import from".into());
            show(st, app);
        }
    }
}

/// Start the import the sheet describes, on a thread; the sheet goes.
pub(crate) fn start(st: &mut State, app: &App) {
    let (Some(opts), Some(scan)) = (options(st, app), st.import.scan.clone()) else {
        return;
    };
    if let Err(e) = import::check(&opts) {
        app.set_import_preview(format!("{e:#}").into());
        app.set_import_ready(false);
        return;
    }
    // Kept for next time now, not at the close: a run that ends
    // badly still remembers where its frames went.
    if let Some(file) = &st.settings_file {
        let mut kept = settings::Settings::load_from(file);
        kept.import = settings::ImportChoices {
            destination: opts.destination.to_string_lossy().into_owned(),
            subfolder: opts.subfolder.clone(),
            name: opts.name.clone(),
            backup: opts
                .backup
                .as_deref()
                .map(|b| b.to_string_lossy().into_owned())
                .unwrap_or_default(),
            preset: opts
                .preset
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        };
        kept.save_to(file);
    }
    let job = Arc::new(Job::default());
    st.import.job = Some(job.clone());
    st.import.folder_at_start = browsed_folder(st);
    st.import.closing = false;
    st.import.gave_up = false;
    app.set_import_open(false);
    app.set_import_running(true);
    app.set_import_stopping(false);
    say(
        app,
        format!("importing {}...", import::frames(scan.items.len())),
    );
    let weak = app.as_weak();
    st.import.thread = Some(std::thread::spawn(move || {
        let progress_weak = weak.clone();
        let report = import::run(&opts, &scan, &job, |i, n, name| {
            let line = import::progress_line(i, n, name);
            let name = name.to_string();
            let _ = progress_weak.upgrade_in_event_loop(move |app| {
                let closing = STATE.with(|s| {
                    s.borrow().as_ref().is_some_and(|state| {
                        let mut st = state.borrow_mut();
                        st.import.in_hand = name;
                        st.import.closing
                    })
                });
                if app.get_import_running() && !closing {
                    say(&app, line);
                }
            });
        });
        import::log_report(&opts, &scan, &report);
        let _ = weak.upgrade_in_event_loop(move |app| {
            let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            finished(&state, &app, &opts, report);
        });
    }));
}

/// The import is done, however it went: the line said, and the folder
/// that took the most frames opened in the browser.
pub(crate) fn finished(state: &Rc<RefCell<State>>, app: &App, opts: &Options, report: Report) {
    let (closing, moved) = {
        let mut st = state.borrow_mut();
        st.import.job = None;
        // Its last act was to post this; it ends on its own.
        st.import.thread = None;
        let moved = browsed_folder(&st) != st.import.folder_at_start;
        (std::mem::take(&mut st.import.closing), moved)
    };
    app.set_import_running(false);
    app.set_import_stopping(false);
    if closing {
        // The window was asked to close and waited for this.
        tracing::info!("import: the frame in hand landed; closing");
        let _ = slint::quit_event_loop();
        return;
    }
    let mut line = report.line(Path::new(&tilde(&opts.destination)));
    // Nothing new is nothing to open; and a user who went to another
    // folder while it ran is not taken away from it.
    if report.frames > 0
        && let Some(folder) = report.main_folder().map(Path::to_path_buf)
    {
        if moved {
            line.push_str(&format!("; open {} to see them", tilde(&folder)));
        } else if let Some(worker) = WORKER.with(|w| w.borrow().clone()) {
            open_folder(state, app, &worker, &folder);
        }
    }
    // After the open, whose own words would stand over these.
    say(app, line.clone());
    // The grid's header says it for a while and then goes back to
    // the selection; the status plate keeps it until the next words.
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(20), move || {
        if let Some(app) = weak.upgrade()
            && !app.get_import_running()
            && app.get_import_status() == line.as_str()
        {
            app.set_import_status("".into());
        }
    });
    let st = state.borrow();
    show(&st, app);
}

/// The folder the browser shows: its first file's.
fn browsed_folder(st: &State) -> Option<PathBuf> {
    st.files
        .first()
        .and_then(|f| f.parent())
        .map(Path::to_path_buf)
}

/// The event loop has ended with an import still running (a batch
/// run's capture, or a close that gave up): stop it after the file in
/// hand and wait for that, but never longer than `limit`, and say so
/// when it gives up. A read that hangs on a dying card must not keep
/// a process alive with no window.
pub(crate) fn leave(sheet: &mut Sheet, limit: std::time::Duration) {
    if let Some(job) = &sheet.job {
        job.cancel();
        tracing::info!("import: the window closed; stopping after the file in hand");
    }
    let Some(thread) = sheet.thread.take() else {
        return;
    };
    if sheet.gave_up {
        return;
    }
    let started = std::time::Instant::now();
    while !thread.is_finished() {
        if started.elapsed() >= limit {
            tracing::warn!(
                "import: gave up after {:.0} s waiting on {}; its temporary is left, and \
                 a later import into that folder sweeps it",
                limit.as_secs_f64(),
                sheet.in_hand
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let _ = thread.join();
}

/// The window's close asked for while an import runs: the import is
/// stopped after the frame in hand, the window stays up saying so, and
/// closes itself once that frame lands ([`finished`]); if it has not
/// landed after [`CLOSE_WAIT`], the window closes anyway and the log
/// says what was left.
pub(crate) fn close_asked(st: &mut State, app: &App) -> bool {
    let Some(job) = &st.import.job else {
        return false;
    };
    job.cancel();
    st.import.closing = true;
    app.set_import_stopping(true);
    say(
        app,
        format!(
            "finishing {}... the window closes once it has landed",
            st.import.in_hand
        ),
    );
    slint::Timer::single_shot(CLOSE_WAIT, move || {
        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
            return;
        };
        let mut st = state.borrow_mut();
        if !st.import.closing {
            return;
        }
        st.import.gave_up = true;
        tracing::warn!(
            "import: {} had not landed after {:.0} s; closing without it",
            st.import.in_hand,
            CLOSE_WAIT.as_secs_f64()
        );
        let _ = slint::quit_event_loop();
    });
    true
}

/// `--sheet imported`: press Import once the source has been read,
/// looking every 20 ms for up to `tries` looks.
pub(crate) fn start_when_read(app: &App, tries: u32) {
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(20), move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if app.get_import_ready() {
            app.invoke_import_started();
        } else if tries > 0 {
            start_when_read(&app, tries - 1);
        } else {
            tracing::warn!("--sheet imported: the sheet never became ready");
        }
    });
}

/// The import's words: on the status line, and in the grid's header,
/// which covers it.
fn say(app: &App, line: String) {
    app.set_status(line.as_str().into());
    app.set_import_status(line.into());
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.window().on_close_requested(move || {
            let Some(app) = weak.upgrade() else {
                return slint::CloseRequestResponse::HideWindow;
            };
            if close_asked(&mut state.borrow_mut(), &app) {
                slint::CloseRequestResponse::KeepWindowShown
            } else {
                slint::CloseRequestResponse::HideWindow
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_import_asked(move || {
            let app = weak.unwrap();
            ask(&mut state.borrow_mut(), &app);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_import_pattern_changed(move || {
            let app = weak.unwrap();
            show(&state.borrow(), &app);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_import_backup_cleared(move || {
            let app = weak.unwrap();
            let mut st = state.borrow_mut();
            st.import.backup = None;
            look_at_folders(&mut st, &app);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_import_started(move || {
            let app = weak.unwrap();
            start(&mut state.borrow_mut(), &app);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_import_stop(move || {
            let app = weak.unwrap();
            if let Some(job) = &state.borrow().import.job {
                job.cancel();
                app.set_import_stopping(true);
                say(
                    &app,
                    "stopping the import after the frame in hand...".into(),
                );
            }
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_import_choose(move |which| {
            let app = weak.unwrap();
            let st = state.borrow();
            let (title, now) = match which.as_str() {
                "source" => ("Import from", &st.import.source),
                "destination" => ("Import to", &st.import.destination),
                _ => ("Back up to", &st.import.backup),
            };
            let start = now
                .clone()
                .or_else(dirs::picture_dir)
                .or_else(dirs::home_dir)
                .unwrap_or_default();
            let which = which.to_string();
            let weak = app.as_weak();
            export::choose_folder(title, start, move |chosen| {
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                        return;
                    };
                    let mut st = state.borrow_mut();
                    match chosen {
                        Ok(Some(folder)) => match which.as_str() {
                            "source" => read_source(&mut st, &app, folder),
                            "destination" => {
                                st.import.destination = Some(folder);
                                look_at_folders(&mut st, &app);
                            }
                            _ => {
                                st.import.backup = Some(folder);
                                look_at_folders(&mut st, &app);
                            }
                        },
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("folder chooser: {e:#}");
                            app.set_import_note("the desktop offered no folder chooser".into());
                        }
                    }
                    show(&st, &app);
                });
            });
        });
    }
}

/// `--import SRC --to DEST`: the import with no window, its progress
/// and its end in the log and on the terminal. A failure is the exit
/// code's. `opts` as the flags give it; the preset is named by
/// `preset` and looked up here, and the camera profiles listed here.
pub(crate) fn headless(opts: Options, preset: Option<&str>) -> Result<std::process::ExitCode> {
    let preset = match preset {
        None => None,
        Some(name) => Some(
            preset::Store::user()
                .and_then(|s| {
                    // The film presets, as the window's first run
                    // puts them there.
                    s.seed();
                    s.find(name)
                })
                .with_context(|| format!("no preset called {name}"))?
                .preset,
        ),
    };
    let opts = Options {
        source: import::resolve(&opts.source),
        destination: import::resolve(&opts.destination),
        backup: opts.backup.as_deref().map(import::resolve),
        preset,
        profiles: greycard_edit::camera::list(),
        ..opts
    };
    // Checked before anything is made: a destination on the card is
    // refused, not made and then refused.
    import::check(&opts)?;
    // The command line named them, so they are made; the sheet only
    // takes folders that are there.
    std::fs::create_dir_all(&opts.destination)
        .with_context(|| format!("making {}", opts.destination.display()))?;
    if let Some(b) = &opts.backup {
        std::fs::create_dir_all(b).with_context(|| format!("making {}", b.display()))?;
    }
    let scan = import::scan(&opts.source)?;
    let report = import::run(&opts, &scan, &Job::default(), |i, n, file| {
        tracing::info!("{}", import::progress_line(i, n, file));
    });
    import::log_report(&opts, &scan, &report);
    // The status line's words, for whoever ran it: the log is a file.
    eprintln!("{}", report.line(&opts.destination));
    Ok(if report.error.is_some() {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::Day;

    fn fields() -> Fields {
        Fields {
            day: Day {
                year: 2026,
                month: 9,
                day: 24,
            },
            name: "IMG_0001".into(),
            camera: "Canon EOS R6".into(),
            seq: 1,
        }
    }

    /// The options `--import SRC --to DEST [--backup DIR]` makes.
    fn flags(source: &Path, to: &Path, backup: Option<&Path>) -> Options {
        Options {
            source: source.to_path_buf(),
            destination: to.to_path_buf(),
            subfolder: String::new(),
            name: "{name}".into(),
            preset: None,
            backup: backup.map(Path::to_path_buf),
            library: None,
            placement: None,
            profiles: Vec::new(),
            verify: false,
        }
    }

    fn scan_of(names: &[&str]) -> Scan {
        Scan {
            items: names
                .iter()
                .map(|n| import::Item {
                    file: PathBuf::from("card").join(n),
                    companions: Vec::new(),
                })
                .collect(),
            left: Vec::new(),
        }
    }

    #[test]
    fn the_preview_names_the_first_frame_s_path_or_why_not() {
        let scan = scan_of(&["IMG_0001.CR3"]);
        let dest = Path::new("photos");
        let (text, ready) = preview(
            Some(Path::new("card")),
            Some(dest),
            "{yyyy}/{date}",
            "{camera}-{seq}",
            Some(&scan),
            Some(&fields()),
        );
        assert!(ready);
        let want = dest
            .join("2026")
            .join("2026-09-24")
            .join("Canon EOS R6-0001.CR3");
        assert_eq!(text, format!("IMG_0001.CR3 becomes {}", want.display()));
        // No destination yet: the name, and not ready.
        let (text, ready) = preview(
            Some(Path::new("card")),
            None,
            "",
            "{name}",
            Some(&scan),
            Some(&fields()),
        );
        assert!(!ready);
        assert_eq!(text, "IMG_0001.CR3 becomes IMG_0001.CR3");
        // A pattern that is not one says so.
        let (text, ready) = preview(
            Some(Path::new("card")),
            Some(dest),
            "",
            "{lens}",
            Some(&scan),
            Some(&fields()),
        );
        assert!(!ready);
        assert!(text.starts_with("Not a pattern: {lens}"), "{text}");
        // Nothing read yet: nothing to say.
        assert_eq!(
            preview(
                Some(Path::new("card")),
                Some(dest),
                "",
                "{name}",
                None,
                None
            ),
            (String::new(), false)
        );
    }

    #[test]
    fn the_note_counts_frames_companions_and_what_is_left() {
        let mut scan = scan_of(&["a.CR3", "b.JPG"]);
        assert_eq!(note_text(&scan), "2 frames");
        scan.items[0].companions.push(PathBuf::from("card/a.JPG"));
        scan.left.push(PathBuf::from("card/MVI.MP4"));
        assert_eq!(
            note_text(&scan),
            "2 frames, 3 files with the JPEGs and XMPs that go with their raws; \
             1 left on the card (videos, and files the editor does not open)"
        );
        assert_eq!(
            note_text(&Scan::default()),
            "no frames here the editor opens"
        );
    }

    /// Ctrl+Shift+I opens the sheet with nothing open, the remembered
    /// patterns on it and the presets offered; a card found fills the
    /// source, and a source picked by hand first is not replaced.
    #[test]
    fn the_sheet_opens_on_the_key_and_takes_a_card_found() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        state.borrow_mut().presets = vec![Entry {
            path: PathBuf::from("x.gcp"),
            preset: Preset::from_edit("Warm", &edit, &[Section::Light]),
        }];
        let control = slint::platform::Key::Control;
        let shift = slint::platform::Key::Shift;
        use slint::platform::WindowEvent;
        for k in [control, shift] {
            app.window()
                .dispatch_event(WindowEvent::KeyPressed { text: k.into() });
        }
        crate::testing::press(&app, "I");
        for k in [shift, control] {
            app.window()
                .dispatch_event(WindowEvent::KeyReleased { text: k.into() });
        }
        assert!(app.get_import_open());
        assert_eq!(app.get_import_presets().row_count(), 2);
        assert_eq!(app.get_import_preset(), NONE);
        assert!(!app.get_import_ready());

        // The search's answer, as its thread would post it.
        let dir =
            std::env::temp_dir().join(format!("greycard-import-sheet-{}", std::process::id()));
        let card = dir.join("EOS_DIGITAL/DCIM");
        std::fs::create_dir_all(card.join("100CANON")).unwrap();
        found(&mut state.borrow_mut(), &app, Some(card.clone()), 3, 0.001);
        assert_eq!(
            state.borrow().import.source.as_deref(),
            Some(card.as_path())
        );
        let generation = state.borrow().import.generation;
        // And the reading's, with a frame and a destination: ready.
        state.borrow_mut().import.destination = Some(dir.join("photos"));
        scanned(
            &mut state.borrow_mut(),
            &app,
            generation,
            Ok((scan_of(&["IMG_0001.CR3"]), Some(fields()))),
            0.01,
        );
        // Not until the destination has been looked at and is there.
        assert!(!app.get_import_ready());
        let folders = state.borrow().import.folders_generation;
        folders_looked(
            &mut state.borrow_mut(),
            &app,
            folders,
            (Some(true), None, false),
        );
        assert!(app.get_import_ready());
        assert_eq!(app.get_import_count(), 1);
        assert_eq!(app.get_import_note(), "1 frame");
        // An older reading that answers late says nothing.
        scanned(
            &mut state.borrow_mut(),
            &app,
            generation - 1,
            Ok((Scan::default(), None)),
            0.01,
        );
        assert_eq!(app.get_import_count(), 1);
        // A card found after one was chosen by hand is not taken.
        found(
            &mut state.borrow_mut(),
            &app,
            Some(dir.join("other")),
            3,
            0.001,
        );
        assert_eq!(
            state.borrow().import.source.as_deref(),
            Some(card.as_path())
        );
        // Escape closes the sheet.
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_import_open());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--import SRC --to DEST` as the command line parses it, run
    /// with no window: the destination made, the frames in it, a
    /// second run all already there; `--to` alone is refused.
    #[test]
    fn the_command_line_imports_with_no_window() {
        use clap::Parser;
        let dir = std::env::temp_dir().join(format!("greycard-import-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let card = dir.join("card/DCIM/100CANON");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("IMG_0001.CR3"), b"a raw's bytes").unwrap();
        std::fs::write(card.join("IMG_0002.JPG"), b"a camera JPEG").unwrap();
        let dest = dir.join("photos/new");
        let cli = crate::Cli::try_parse_from([
            "greycard-ui".as_ref(),
            "--import".as_ref(),
            dir.join("card").as_os_str(),
            "--to".as_ref(),
            dest.as_os_str(),
            "--name".as_ref(),
            "trip-{seq}".as_ref(),
        ])
        .unwrap();
        let run = || {
            let mut o = flags(
                cli.import.as_deref().unwrap(),
                cli.to.as_deref().unwrap(),
                None,
            );
            o.name = cli.name.clone().unwrap();
            headless(o, None).unwrap()
        };
        assert_eq!(run(), std::process::ExitCode::SUCCESS);
        assert_eq!(
            std::fs::read(dest.join("trip-0001.CR3")).unwrap(),
            b"a raw's bytes"
        );
        assert_eq!(
            std::fs::read(dest.join("trip-0002.JPG")).unwrap(),
            b"a camera JPEG"
        );
        assert_eq!(run(), std::process::ExitCode::SUCCESS);
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 2);
        // `--to` alone is the sheet's, for `--sheet import`.
        assert!(crate::Cli::try_parse_from(["greycard-ui", "--to", "x"]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review item 1: `--import` checked before it made anything. A
    /// destination under the card's DCIM was refused with the folder
    /// already made on the card.
    #[test]
    fn the_command_line_refuses_the_card_before_it_makes_a_folder() {
        let dir =
            std::env::temp_dir().join(format!("greycard-import-first-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let card = dir.join("card2/EOS_DIGITAL");
        std::fs::create_dir_all(card.join("DCIM/100CANON")).unwrap();
        std::fs::write(card.join("DCIM/100CANON/IMG_0001.CR3"), b"raw").unwrap();
        let source = card.join("DCIM");
        let refused = |to: &Path, backup: Option<&Path>| {
            let e = headless(flags(&source, to, backup), None).unwrap_err();
            format!("{e:#}")
        };
        let e = refused(&source.join("out"), None);
        assert!(e.contains("is on the card"), "{e}");
        assert!(!source.join("out").exists());
        let e = refused(&card.join("Imported"), None);
        assert!(e.contains("is on the card"), "{e}");
        assert!(!card.join("Imported").exists());
        let e = refused(&dir.join("photos"), Some(&card.join("backup")));
        assert!(e.contains("backup"), "{e}");
        assert!(!card.join("backup").exists());
        assert!(
            !dir.join("photos").exists(),
            "nothing made before the check"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review item 14: a remembered folder that is not there (a backup
    /// drive not plugged in) is said so and not made; and a backup on
    /// the destination's drive is warned of.
    #[test]
    fn a_folder_that_is_not_there_is_said_and_refused() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        let dir = std::env::temp_dir().join(format!("greycard-import-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        let gone = dir.join("mnt/backup");
        let looked = look(Some(&dir.join("photos")), Some(&gone));
        assert_eq!((looked.0, looked.1), (Some(true), Some(false)));
        {
            let mut st = state.borrow_mut();
            st.import.source = Some(dir.join("card"));
            st.import.destination = Some(dir.join("photos"));
            st.import.backup = Some(gone.clone());
            st.import.scan = Some(scan_of(&["IMG_0001.CR3"]));
            st.import.first = Some(fields());
            let generation = st.import.folders_generation;
            folders_looked(&mut st, &app, generation, looked);
        }
        assert!(!app.get_import_ready());
        assert!(
            app.get_import_folders_note()
                .contains("backup folder is not there"),
            "{}",
            app.get_import_folders_note()
        );
        assert!(!gone.exists());
        // Cleared, it is ready; the same drive is warned of.
        {
            let mut st = state.borrow_mut();
            st.import.backup = Some(dir.join("backup"));
            let generation = st.import.folders_generation;
            folders_looked(&mut st, &app, generation, (Some(true), Some(true), true));
        }
        assert!(app.get_import_ready());
        assert!(app.get_import_folders_note().contains("same drive"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review item 12: Escape leaves the grid (or culling, or a set)
    /// before it stops an import; only when nothing else takes it does
    /// it stop one.
    #[test]
    fn escape_leaves_the_grid_before_it_stops_an_import() {
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        let job = Arc::new(Job::default());
        state.borrow_mut().import.job = Some(job.clone());
        app.set_import_running(true);
        app.set_grid_open(true);
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_grid_open());
        assert!(!job.is_canceled());
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(job.is_canceled());
        assert!(app.get_import_stopping());
    }

    /// Review item 13: a close while an import runs keeps the window up
    /// saying what it waits on; and the wait after the event loop has
    /// ended gives up after its limit rather than outliving the window.
    #[test]
    fn a_close_waits_for_the_frame_in_hand_but_not_forever() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        assert!(
            !close_asked(&mut state.borrow_mut(), &app),
            "nothing running"
        );
        let job = Arc::new(Job::default());
        {
            let mut st = state.borrow_mut();
            st.import.job = Some(job.clone());
            st.import.in_hand = "IMG_0042.CR3".into();
        }
        assert!(close_asked(&mut state.borrow_mut(), &app));
        assert!(job.is_canceled());
        assert!(state.borrow().import.closing);
        assert!(
            app.get_status().contains("finishing IMG_0042.CR3"),
            "{}",
            app.get_status()
        );

        let mut sheet = Sheet {
            job: Some(Arc::new(Job::default())),
            thread: Some(std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(3))
            })),
            in_hand: "IMG_0043.CR3".into(),
            ..Sheet::default()
        };
        let started = std::time::Instant::now();
        leave(&mut sheet, std::time::Duration::from_millis(100));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        // One that finishes in time is joined.
        let mut sheet = Sheet {
            thread: Some(std::thread::spawn(|| {})),
            ..Sheet::default()
        };
        leave(&mut sheet, std::time::Duration::from_secs(5));
        assert!(sheet.thread.is_none());
    }
}
