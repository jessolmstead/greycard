//! The Lightroom catalog import from the window: a catalog chosen, the
//! sheet asking where each of its roots is now and the set its
//! collections go under, the import's dry run shown as the command
//! prints it, then the write on a thread of its own, its progress on
//! the status line and under the pane's button, and the report again
//! with what was written.
//!
//! The sheet decides nothing the command does not: every answer it
//! takes is a field of [`lrcat::Options`], each with the command's flag
//! (`--root`, `--set`, `--force`, `--no-creations`), and what it shows
//! is the report's own lines ([`lrcat::Report::lines`]). The guess at a
//! root's folder is the command's too: where the catalog says, else a
//! folder of the root's name beside the catalog or under one of the
//! library's roots.
//!
//! The window's thread never reads the catalog or the disk: each dry
//! run and the write run on a thread and post back, a dry run asked
//! for later winning over one that answers late.

use greycard_library::lightroom_catalog::{self as lrcat, Found, Progress, Report, Voice};

use crate::files::tilde;
use crate::*;

/// What the sheet says when the run writes no sidecars: the report is
/// shown, and Write stays off.
pub(crate) const NO_SIDECARS: &str =
    "This run writes no sidecars (--no-sidecars), so nothing can be imported.";

/// The sheet's side of the state.
#[derive(Default)]
pub(crate) struct Sheet {
    pub(crate) catalog: Option<PathBuf>,
    /// Where the user said each root is: its path as the catalog has
    /// it, and the folder here.
    pub(crate) roots: Vec<(String, PathBuf)>,
    /// The last report: the dry run's, or the write's.
    pub(crate) report: Option<Report>,
    /// The dry run the window last asked for.
    pub(crate) generation: u64,
    /// What the sheet does once the next dry run lands, for a capture.
    pub(crate) then: Then,
    /// The write's thread, while it runs.
    pub(crate) thread: Option<std::thread::JoinHandle<()>>,
    /// A dry run asked for and not landed.
    pub(crate) reading: bool,
    /// The roots the next catalog opened starts from, as if chosen on
    /// the sheet: `--root` with `--lightroom`.
    pub(crate) seed: Vec<(String, PathBuf)>,
}

impl Sheet {
    /// Whether anything is under way: a dry run reading, a write, or a
    /// step waiting on one. What a capture waits out.
    pub(crate) fn busy(&self) -> bool {
        self.reading || self.thread.is_some() || self.then != Then::Nothing
    }
}

/// What follows a dry run without a click: for `--sheet`'s steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Then {
    #[default]
    Nothing,
    /// The report shown, as Check shows it.
    Report,
    /// The write started, as Write starts it.
    Write,
}

/// The import's options as the sheet has them: the roots the user
/// chose, the set as typed, the switches.
fn options(st: &State, app: &App, write: bool) -> Option<lrcat::Options> {
    Some(lrcat::Options {
        roots: st.lightroom.roots.clone(),
        write,
        force: app.get_lr_force(),
        // The field starts at the catalog's name; empty is none.
        set: Some(app.get_lr_set().to_string()),
        library: st.index_path.clone()?,
        sidecars_in_folder: st.placement == greycard_edit::Placement::Folder,
        no_creations: !app.get_lr_creations(),
    })
}

/// A catalog chosen: the sheet opened on it, its roots read.
pub(crate) fn open(st: &mut State, app: &App, catalog: PathBuf) {
    if st.lightroom.thread.is_some() {
        return;
    }
    let name = catalog
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    st.lightroom.catalog = Some(catalog.clone());
    st.lightroom.roots = std::mem::take(&mut st.lightroom.seed);
    st.lightroom.report = None;
    app.set_lr_title(format!("Import {name} from Lightroom").into());
    app.set_lr_subtitle(tilde(&catalog).into());
    app.set_lr_note("".into());
    app.set_lr_set(name.into());
    app.set_lr_creations(true);
    app.set_lr_force(false);
    app.set_lr_step(0);
    app.set_lr_open(true);
    if st.index_path.is_none() {
        app.set_lr_note(
            "No library here to bring the collections and the roots into: this \
             installation has no data folder."
                .into(),
        );
        show(st, app);
        return;
    }
    dry_run(st, app);
}

/// The dry run the sheet stands for, on a thread.
pub(crate) fn dry_run(st: &mut State, app: &App) {
    let (Some(catalog), Some(opts)) = (st.lightroom.catalog.clone(), options(st, app, false))
    else {
        return;
    };
    st.lightroom.generation += 1;
    st.lightroom.reading = true;
    let generation = st.lightroom.generation;
    app.set_lr_busy(true);
    app.set_lr_busy_text("reading the catalog...".into());
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let read = lrcat::import(&catalog, &opts).map_err(|e| e.to_string());
        let _ = weak.upgrade_in_event_loop(move |app| {
            let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            landed(&state, &app, generation, read);
        });
    });
}

/// A dry run's answer, on the window's thread. One asked for before
/// the last says nothing.
pub(crate) fn landed(
    state: &Rc<RefCell<State>>,
    app: &App,
    generation: u64,
    read: std::result::Result<Report, String>,
) {
    let then = {
        let mut st = state.borrow_mut();
        if generation != st.lightroom.generation {
            return;
        }
        st.lightroom.reading = false;
        app.set_lr_busy(false);
        match read {
            Ok(report) => {
                app.set_lr_note("".into());
                st.lightroom.report = Some(report);
            }
            Err(e) => {
                tracing::warn!("lightroom import: {e}");
                app.set_lr_note(e.into());
                st.lightroom.report = None;
            }
        }
        show(&st, app);
        std::mem::take(&mut st.lightroom.then)
    };
    match then {
        Then::Nothing => {}
        Then::Report => app.set_lr_step(1),
        Then::Write => {
            app.set_lr_step(1);
            write(state, app);
        }
    }
}

/// What a root's line says of where it was looked for.
fn root_note(r: &lrcat::RootReport) -> String {
    if !r.confirmed
        && let Some(m) = &r.near_miss
    {
        let mut m = m.clone();
        m.dir = PathBuf::from(tilde(&m.dir));
        return m.said(Voice::Sheet);
    }
    let Some(_) = &r.local else {
        return "not where the catalog says, and no folder of its name near the catalog or \
                under the library's roots: choose it"
            .into();
    };
    let how = match &r.how {
        Found::Given => "as chosen".to_string(),
        Found::BesideCatalog => "found beside the catalog".to_string(),
        Found::UnderRoot(root) => format!("found under the library's root {}", tilde(root)),
        Found::AsCatalogSays | Found::Not => "where the catalog says".to_string(),
    };
    if r.confirmed {
        format!("{how}: {} of its {} files found", r.found, r.files)
    } else {
        format!(
            "{how}: none of the {} files looked for there; choose where it is",
            r.checked
        )
    }
}

/// The sheet as the state has it.
pub(crate) fn show(st: &State, app: &App) {
    let report = st.lightroom.report.as_ref();
    let roots: Vec<LightroomRoot> = report
        .map(|r| {
            r.roots
                .iter()
                .map(|root| LightroomRoot {
                    catalog: root.catalog.as_str().into(),
                    name: root.name.as_str().into(),
                    local: root.local.as_deref().map(tilde).unwrap_or_default().into(),
                    folder: root
                        .local
                        .as_deref()
                        .and_then(Path::file_name)
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                        .into(),
                    note: root_note(root).into(),
                    taken: root.confirmed,
                })
                .collect()
        })
        .unwrap_or_default();
    app.set_lr_roots(ModelRc::new(VecModel::from(roots)));
    let mut lines: Vec<ReportLine> = report
        .map(|r| {
            r.lines(Voice::Sheet)
                .into_iter()
                .map(|(label, text)| ReportLine {
                    label: label.into(),
                    text: text.into(),
                })
                .collect()
        })
        .unwrap_or_default();
    // What a write did is said first, over the report it ends.
    let done = match lines.iter().position(|l| l.label == "written") {
        Some(at) => lines.split_off(at),
        None => Vec::new(),
    };
    app.set_lr_lines(ModelRc::new(VecModel::from(lines)));
    app.set_lr_done(ModelRc::new(VecModel::from(done)));
    app.set_lr_has_creations(report.is_some_and(|r| {
        r.collections.iter().any(|c| c.creation.is_some()) || !r.creations_left.is_empty()
    }));
    app.set_lr_already(report.map_or(0, |r| r.already) as i32);
    app.set_lr_ready(
        report.is_some_and(|r| r.found > 0) && st.index_path.is_some() && st.write_sidecars,
    );
    if !st.write_sidecars && app.get_lr_note().is_empty() {
        app.set_lr_note(NO_SIDECARS.into());
    }
    if let Some(r) = report {
        app.set_lr_subtitle(
            format!(
                "{}; {}, {} images in {} {}",
                st.lightroom
                    .catalog
                    .as_deref()
                    .map(tilde)
                    .unwrap_or_default(),
                r.written_by,
                r.images,
                r.roots.len(),
                if r.roots.len() == 1 {
                    "folder"
                } else {
                    "folders"
                }
            )
            .into(),
        );
    }
}

/// A root's folder chosen: the mapping taken, the dry run again.
pub(crate) fn root_chosen(st: &mut State, app: &App, row: usize, folder: PathBuf) {
    let Some(catalog) = st
        .lightroom
        .report
        .as_ref()
        .and_then(|r| r.roots.get(row))
        .map(|r| r.catalog.clone())
    else {
        return;
    };
    st.lightroom.roots.retain(|(from, _)| *from != catalog);
    st.lightroom.roots.push((catalog, folder));
    dry_run(st, app);
}

/// Write: the sheet goes, the import runs on its thread, its progress
/// said as it goes; the sheet comes back with the report when done.
pub(crate) fn write(state: &Rc<RefCell<State>>, app: &App) {
    let mut st = state.borrow_mut();
    if st.lightroom.thread.is_some() {
        return;
    }
    if !st.write_sidecars {
        app.set_lr_note(NO_SIDECARS.into());
        return;
    }
    let (Some(catalog), Some(opts)) = (st.lightroom.catalog.clone(), write_options(&mut st, app))
    else {
        return;
    };
    // The sheet stays up while it writes, so nothing is edited under
    // it; its progress is on the status line and under the button.
    app.set_lr_busy(true);
    app.set_lr_busy_text("writing...".into());
    app.set_lr_running(true);
    say(app, "importing the catalog: reading it...".into());
    let weak = app.as_weak();
    st.lightroom.thread = Some(std::thread::spawn(move || {
        let told = weak.clone();
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let report = lrcat::import_with(&catalog, &opts, &mut |p| {
            // A tenth of a second apart at most: the window is not
            // asked to redraw for every file.
            let now = std::time::Instant::now();
            if !matches!(p, Progress::Finishing)
                && now - last < std::time::Duration::from_millis(100)
            {
                return;
            }
            last = now;
            let line = progress_line(p);
            let _ = told.upgrade_in_event_loop(move |app| {
                if app.get_lr_running() {
                    say(&app, line);
                }
            });
        })
        .map_err(|e| e.to_string());
        // The written frames as the index keys them too, asked of the
        // disk here rather than on the window's thread.
        let keys = keys_of(&report);
        let _ = weak.upgrade_in_event_loop(move |app| {
            let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            wrote(&state, &app, report, keys);
        });
    }));
}

/// The options a write runs under, the window made ready for it. The
/// folders the report showed, guessed or chosen, are the ones written
/// to: a write never guesses again, and cannot land where the report
/// did not say. And the frame on screen, its panel moved and not yet
/// saved, is saved first, so the import reads what the window holds.
pub(crate) fn write_options(st: &mut State, app: &App) -> Option<lrcat::Options> {
    let mut opts = options(st, app, true)?;
    if let Some(report) = &st.lightroom.report {
        opts.roots = resolved(report, &opts.roots);
    }
    // A panel moved and not yet saved (its save waiting on the panel
    // to rest) is saved now: the frame then has an edit of its own, and
    // the import leaves it alone unless told to write over it. A panel
    // at rest is the sidecar already, and nothing is written.
    if st.current.is_some() && st.cull.is_none() && st.save_timer.running() {
        st.save_timer.stop();
        let outgoing = crate::panel::edit::read_edit(app, &st.edit, st.target);
        crate::panel::edit::save_edit(st, outgoing);
    }
    Some(opts)
}

/// The frames a write gave sidecars to, as the import found them and as
/// the index keys them: what the window's list may hold them as.
pub(crate) fn keys_of(report: &std::result::Result<Report, String>) -> Vec<PathBuf> {
    let Some(w) = report.as_ref().ok().and_then(|r| r.written.as_ref()) else {
        return Vec::new();
    };
    w.frames
        .iter()
        .flat_map(|p| [p.clone(), greycard_library::key_path(p)])
        .collect()
}

/// The roots a write is given: each the report took, at the folder it
/// took it at, and the user's own for the rest.
fn resolved(report: &Report, given: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let mut roots: Vec<(String, PathBuf)> = report
        .roots
        .iter()
        .filter(|r| r.confirmed)
        .filter_map(|r| Some((r.catalog.clone(), r.local.clone()?)))
        .collect();
    for (from, to) in given {
        let taken = report
            .roots
            .iter()
            .any(|r| r.confirmed && (r.catalog == *from || r.name == *from));
        if !taken {
            roots.push((from.clone(), to.clone()));
        }
    }
    roots
}

/// The status line's words for how far the import has come.
pub(crate) fn progress_line(p: Progress) -> String {
    match p {
        Progress::Looking { done, of } => {
            format!("importing the catalog: looking for its files, {done} of {of}")
        }
        Progress::Writing { done, of } => {
            format!("importing the catalog: writing {done} of {of} frames")
        }
        Progress::Finishing => {
            "importing the catalog: the collections, the stacks and the roots".into()
        }
    }
}

/// The write done, however it went: the line said, the window's
/// collections, stacks and roots read again from what it wrote, and
/// the sheet back with the report.
pub(crate) fn wrote(
    state: &Rc<RefCell<State>>,
    app: &App,
    report: std::result::Result<Report, String>,
    keys: Vec<PathBuf>,
) {
    app.set_lr_running(false);
    app.set_lr_busy(false);
    let line = {
        let mut st = state.borrow_mut();
        st.lightroom.thread = None;
        match report {
            Ok(mut report) => {
                // The write was given the report's folders as mappings;
                // each root says how the report found it, as it did.
                if let Some(before) = &st.lightroom.report {
                    for r in &mut report.roots {
                        if let Some(b) = before.roots.iter().find(|b| b.catalog == r.catalog) {
                            r.how = b.how.clone();
                        }
                    }
                }
                let line = match &report.written {
                    Some(w) => {
                        let n = |k: usize, one: &str, many: &str| {
                            format!("{k} {}", if k == 1 { one } else { many })
                        };
                        format!(
                            "imported the catalog: {} written, {} left alone, {}, {}",
                            n(w.sidecars, "frame", "frames"),
                            w.left_alone,
                            n(
                                w.collections + w.joined + w.smart,
                                "collection",
                                "collections"
                            ),
                            n(w.stacks, "stack", "stacks")
                        )
                    }
                    None => "imported the catalog".to_string(),
                };
                app.set_lr_note("".into());
                st.lightroom.report = Some(report);
                line
            }
            Err(e) => {
                tracing::warn!("lightroom import: {e}");
                app.set_lr_note(format!("Not imported: {e}").into());
                format!("the catalog was not imported: {e}")
            }
        }
    };
    took_up(state, app);
    reread_written(state, app, keys, "lightroom import");
    say(app, line.clone());
    {
        let st = state.borrow();
        show(&st, app);
    }
    app.set_lr_step(2);
    app.set_lr_open(true);
    // The pane's line goes after a while; the status plate keeps it
    // until the next words.
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(20), move || {
        if let Some(app) = weak.upgrade()
            && !app.get_lr_running()
            && app.get_lr_status() == line.as_str()
        {
            app.set_lr_status("".into());
        }
    });
}

/// What the write changed on disk taken up by the window: the
/// collections and the stacks read again, and each root it added
/// kept, watched and passed over, as the roots pane's own add does.
fn took_up(state: &Rc<RefCell<State>>, app: &App) {
    let added: Vec<PathBuf> = {
        let mut st = state.borrow_mut();
        let Some(path) = st.index_path.clone() else {
            return;
        };
        let (rows, names) = (st.collections.rows.clone(), st.collections.names.clone());
        st.collections =
            crate::collections::Store::load(crate::collections::Store::dir_beside(&path));
        (st.collections.rows, st.collections.names) = (rows, names);
        crate::panel::collections::show(&st, app);
        st.stacks = crate::stacks::Store::load(greycard_library::stacks::dir_in(
            &crate::collections::Store::dir_beside(&path),
        ));
        st.lightroom
            .report
            .as_ref()
            .and_then(|r| r.written.as_ref())
            .map(|w| {
                w.roots
                    .iter()
                    .filter(|(_, said)| said.starts_with("added"))
                    .map(|(dir, _)| dir.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    if !added.is_empty() {
        let worker = WORKER.with(|w| w.borrow().clone());
        crate::roots::taken_from_file(state, app, worker.as_ref(), added);
    }
}

/// The frames the write gave sidecars to, as the window holds them,
/// read again from what was written: each file's sidecar taken from the
/// disk, its cells' pictures asked again, and the frame on screen opened
/// again on its sidecar with nothing of its old panel saved over it, so
/// the next key or slider moves from the import's edit and not over it.
/// `whose` names the import in the log. The RawTherapee profiles'
/// import takes up its write the same way.
pub(crate) fn reread_written(
    state: &Rc<RefCell<State>>,
    app: &App,
    keys: Vec<PathBuf>,
    whose: &str,
) {
    let mut st = state.borrow_mut();
    let written: std::collections::HashSet<PathBuf> = keys.into_iter().collect();
    let files: Vec<usize> = st
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| written.contains(*f))
        .map(|(i, _)| i)
        .collect();
    for &file in &files {
        crate::rows::reread_file(&mut st, app, file);
        for c in st.cells_of_file(file) {
            crate::edited::changed(&mut st, c);
        }
    }
    let on_screen = st
        .current
        .filter(|&c| st.cull.is_none() && st.file_at(c).is_some_and(|f| files.contains(&f)));
    if let Some(c) = on_screen
        && let Some(worker) = WORKER.with(|w| w.borrow().clone())
    {
        st.current = None;
        crate::panel::browser::open_frame(&mut st, app, &worker, c);
    }
    tracing::info!(
        "{whose}: {} of the frames written held by the window, read again",
        files.len()
    );
}

fn say(app: &App, line: String) {
    app.set_status(line.as_str().into());
    app.set_lr_status(line.into());
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let weak = app.as_weak();
        app.on_lr_asked(move || {
            let app = weak.unwrap();
            let start = dirs::picture_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_default();
            app.set_status("choosing a Lightroom catalog...".into());
            let weak = app.as_weak();
            // A test opens no chooser: it answers as a cancel.
            export::choose_open(
                "Import a Lightroom catalog",
                start,
                (
                    "Lightroom catalogs".to_string(),
                    vec!["*.lrcat".into(), "*.LRCAT".into()],
                ),
                move |chosen| {
                    let _ = weak.upgrade_in_event_loop(move |app| {
                        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                            return;
                        };
                        match chosen {
                            Ok(Some(catalog)) => open(&mut state.borrow_mut(), &app, catalog),
                            Ok(None) => app.set_status("import canceled".into()),
                            Err(e) => {
                                tracing::warn!("file chooser: {e:#}");
                                app.set_status("the desktop offered no file chooser".into());
                            }
                        }
                    });
                },
            );
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_lr_choose_root(move |row| {
            let app = weak.unwrap();
            let st = state.borrow();
            let Some(root) = st
                .lightroom
                .report
                .as_ref()
                .and_then(|r| r.roots.get(row as usize))
            else {
                return;
            };
            let start = root
                .local
                .clone()
                .or_else(|| {
                    st.lightroom
                        .catalog
                        .as_deref()
                        .and_then(Path::parent)
                        .map(Path::to_path_buf)
                })
                .or_else(dirs::home_dir)
                .unwrap_or_default();
            let weak = app.as_weak();
            export::choose_folder("Where the catalog's folder is now", start, move |chosen| {
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
                        return;
                    };
                    match chosen {
                        Ok(Some(folder)) => {
                            root_chosen(&mut state.borrow_mut(), &app, row as usize, folder)
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("folder chooser: {e:#}");
                            app.set_lr_note("the desktop offered no folder chooser".into());
                        }
                    }
                });
            });
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_lr_check(move || {
            let app = weak.unwrap();
            let mut st = state.borrow_mut();
            // The set, the switches as they are now: the report is of
            // what a write would do with them.
            st.lightroom.then = Then::Report;
            dry_run(&mut st, &app);
        });
    }
    {
        let weak = app.as_weak();
        app.on_lr_back(move || {
            weak.unwrap().set_lr_step(0);
        });
    }
    {
        let state = state.clone();
        let weak = app.as_weak();
        app.on_lr_write(move || {
            let app = weak.unwrap();
            write(&state, &app);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_library::lightroom_catalog::Options;

    /// A catalog of two frames under one root, the files a folder of
    /// the root's name beside it, written as Lightroom keeps one; one
    /// frame rated and developed, the other rated, both in a collection.
    fn catalog(dir: &Path) -> PathBuf {
        let path = dir.join("Shoot.lrcat");
        let shots = dir.join("Shoots");
        std::fs::create_dir_all(&shots).unwrap();
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            std::fs::write(shots.join(n), format!("not a raw, {n}")).unwrap();
        }
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE Adobe_variablesTable (id_local INTEGER PRIMARY KEY, name, value);
             INSERT INTO Adobe_variablesTable (name, value) VALUES ('Adobe_DBVersion', '1504001');
             CREATE TABLE AgLibraryRootFolder (id_local INTEGER PRIMARY KEY, id_global,
                 absolutePath, name, relativePathFromCatalog);
             CREATE TABLE AgLibraryFolder (id_local INTEGER PRIMARY KEY, id_global, parentId,
                 pathFromRoot, rootFolder);
             CREATE TABLE AgLibraryFile (id_local INTEGER PRIMARY KEY, id_global, baseName,
                 extension, folder);
             CREATE TABLE Adobe_images (id_local INTEGER PRIMARY KEY, id_global, rootFile,
                 masterImage, copyName, rating, pick, colorLabels NOT NULL DEFAULT '');
             CREATE TABLE AgLibraryKeyword (id_local INTEGER PRIMARY KEY, id_global, name,
                 parent);
             CREATE TABLE AgLibraryKeywordImage (id_local INTEGER PRIMARY KEY, image, tag);
             CREATE TABLE AgLibraryIPTC (id_local INTEGER PRIMARY KEY, image, caption);
             CREATE TABLE Adobe_AdditionalMetadata (id_local INTEGER PRIMARY KEY, image, xmp);
             CREATE TABLE Adobe_libraryImageDevelopHistoryStep (id_local INTEGER PRIMARY KEY,
                 image, name);
             CREATE TABLE Adobe_imageDevelopSettings (id_local INTEGER PRIMARY KEY, image, text,
                 processVersion, hasDevelopAdjustmentsEx);
             CREATE TABLE AgLibraryCollection (id_local INTEGER PRIMARY KEY, creationId, name,
                 parent, systemOnly);
             CREATE TABLE AgLibraryCollectionImage (id_local INTEGER PRIMARY KEY, collection,
                 image, positionInCollection);
             CREATE TABLE AgLibraryCollectionContent (id_local INTEGER PRIMARY KEY, collection,
                 content, owningModule);
             CREATE TABLE AgLibraryFolderStack (id_local INTEGER PRIMARY KEY, id_global);
             INSERT INTO AgLibraryRootFolder VALUES (1, 'r', 'D:/Away/Shoots/', 'Shoots', NULL);
             INSERT INTO AgLibraryFolder VALUES (2, 'f', NULL, '', 1);
             INSERT INTO AgLibraryFile VALUES (10, 'a', 'IMG_0001', 'CR3', 2);
             INSERT INTO AgLibraryFile VALUES (11, 'b', 'IMG_0002', 'CR3', 2);
             INSERT INTO Adobe_images VALUES (100, 'i', 10, NULL, NULL, 3.0, 1.0, '');
             INSERT INTO Adobe_images VALUES (101, 'j', 11, NULL, NULL, 2.0, 0.0, '');
             INSERT INTO Adobe_imageDevelopSettings (image, text, processVersion,
                 hasDevelopAdjustmentsEx) VALUES (100, 's = { Exposure2012 = 0.5 }', '15.4', 1.0);
             INSERT INTO AgLibraryCollection VALUES (1, 'com.adobe.ag.library.collection',
                 'Picks', NULL, 0.0);
             INSERT INTO AgLibraryCollection VALUES (2, 'com.adobe.ag.bookExport',
                 'Album', NULL, 0.0);
             INSERT INTO AgLibraryCollectionImage VALUES (1, 1, 100, 'a');
             INSERT INTO AgLibraryCollectionImage VALUES (2, 1, 101, 'b');
             INSERT INTO AgLibraryCollectionImage VALUES (3, 2, 100, 'a');",
        )
        .unwrap();
        path
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-lightroom-sheet-{what}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The dry run the sheet asked for, run here and landed as its
    /// thread would land it.
    fn land_now(state: &Rc<RefCell<State>>, app: &App) {
        let (catalog, opts, generation) = {
            let st = state.borrow();
            (
                st.lightroom.catalog.clone().unwrap(),
                options(&st, app, false).unwrap(),
                st.lightroom.generation,
            )
        };
        let read = lrcat::import(&catalog, &opts).map_err(|e| e.to_string());
        landed(state, app, generation, read);
    }

    #[test]
    fn the_sheet_asks_the_roots_and_the_set_then_shows_the_command_s_report() {
        let dir = scratch("steps");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().write_sidecars = true;
        let library = dir.join("library").join("library.sqlite");
        state.borrow_mut().index_path = Some(library.clone());

        // The roots: the folder of the root's name beside the catalog,
        // guessed, and its files found.
        open(&mut state.borrow_mut(), &app, catalog.clone());
        assert!(app.get_lr_open());
        assert_eq!(app.get_lr_step(), 0);
        assert_eq!(app.get_lr_set(), "Shoot", "the catalog's name");
        assert!(app.get_lr_busy());
        land_now(&state, &app);
        assert!(!app.get_lr_busy());
        let roots = app.get_lr_roots();
        assert_eq!(roots.row_count(), 1);
        let r = roots.row_data(0).unwrap();
        assert_eq!(r.catalog, "D:/Away/Shoots/");
        assert_eq!(r.name, "Shoots");
        assert!(r.taken);
        assert!(r.note.contains("found beside the catalog"), "{}", r.note);
        assert!(r.note.contains("2 of its 2 files found"), "{}", r.note);
        assert!(app.get_lr_has_creations(), "the book");
        assert!(app.get_lr_ready());

        // A folder chosen in its place: the mapping is the command's
        // --root, and the dry run says so.
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        root_chosen(&mut state.borrow_mut(), &app, 0, elsewhere.clone());
        land_now(&state, &app);
        let r = app.get_lr_roots().row_data(0).unwrap();
        assert!(!r.taken);
        assert!(r.note.contains("as chosen"), "{}", r.note);
        assert!(!app.get_lr_ready(), "nothing found, nothing to write");
        root_chosen(&mut state.borrow_mut(), &app, 0, dir.join("Shoots"));
        land_now(&state, &app);
        assert!(app.get_lr_ready());

        // The set emptied and the book left out, then Check: the
        // report is the command's own lines for those options.
        app.set_lr_set("".into());
        app.set_lr_creations(false);
        state.borrow_mut().lightroom.then = Then::Report;
        state.borrow_mut().lightroom.generation += 1;
        land_now(&state, &app);
        assert_eq!(app.get_lr_step(), 1);
        let opts = Options {
            roots: vec![("D:/Away/Shoots/".into(), dir.join("Shoots"))],
            set: Some(String::new()),
            library: library.clone(),
            no_creations: true,
            ..Options::default()
        };
        let command = lrcat::import(&catalog, &opts).unwrap();
        let lines: Vec<(String, String)> = app
            .get_lr_lines()
            .iter()
            .map(|l| (l.label.to_string(), l.text.to_string()))
            .collect();
        assert_eq!(lines, command.lines(Voice::Sheet));
        let text: Vec<String> = lines.iter().map(|(_, t)| t.clone()).collect();
        assert!(
            text.iter().any(|t| t.starts_with("Picks: 2 frames")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|t| t.contains("left out: Album")),
            "{text:?}"
        );
        assert!(text.iter().any(|t| t == "nothing written yet"), "{text:?}");
        // The sheet's words where the command names its flags.
        assert!(!text.iter().any(|t| t.contains("--")), "{text:?}");
        assert_eq!(
            command
                .lines(Voice::Command)
                .iter()
                .filter(|(_, t)| t.contains("--write"))
                .count(),
            1
        );

        // A dry run that answers after a later one was asked says
        // nothing.
        let stale = state.borrow().lightroom.generation - 1;
        landed(&state, &app, stale, Err("late".into()));
        assert_eq!(app.get_lr_note(), "");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The catalog's two frames as small TIFFs the window opens, the
    /// first developed with a turned crop.
    fn pictures(dir: &Path) -> PathBuf {
        use greycard_library::fixture::{R5, R6, write_frame};
        let path = catalog(dir);
        let shots = dir.join("Shoots");
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            std::fs::remove_file(shots.join(n)).unwrap();
        }
        write_frame(&shots.join("a.tif"), &R5, 1);
        write_frame(&shots.join("b.tif"), &R6, 2);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "UPDATE AgLibraryFile SET baseName = 'a', extension = 'tif' WHERE id_local = 10;
             UPDATE AgLibraryFile SET baseName = 'b', extension = 'tif' WHERE id_local = 11;
             UPDATE Adobe_imageDevelopSettings SET text =
                 's = { Exposure2012 = 0.5, CropLeft = 0.25, CropRight = 0.75, CropAngle = 1 }'
                 WHERE image = 100;",
        )
        .unwrap();
        path
    }

    /// The frame open in the window when the write lands is opened again
    /// on what was written: a key pressed after it writes the import's
    /// edit with the rating, never the panel from before the write.
    #[test]
    fn the_frame_on_screen_takes_the_write_and_a_key_after_it_keeps_it() {
        let dir = crate::testing::scratch_dir("lightroom-on-screen");
        let catalog = pictures(&dir);
        let shots = dir.join("Shoots");
        let app = crate::testing::window(0);
        let (state, worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().write_sidecars = true;
        crate::panel::browser::open_folder(&state, &app, &worker, &shots);
        crate::roots::land_sent(&state, &app, &worker);
        WORKER.with(|w| *w.borrow_mut() = Some(worker.clone()));
        app.invoke_select(0);
        assert_eq!(state.borrow().current, Some(0));
        let a = shots.join("a.tif");
        assert!(greycard_edit::Sidecar::load(&a).unwrap().is_none());
        state.borrow_mut().index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog.clone());
        land_now(&state, &app);
        // The write as Write runs it, landed.
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        let report = lrcat::import_with(&catalog, &opts, &mut |_| {}).map_err(|e| e.to_string());
        let keys = keys_of(&report);
        wrote(&state, &app, report, keys);
        let written = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert!(!written.history.is_empty(), "the import's state");
        assert!(written.current.geometry.crop.is_some());
        // The panel is the written edit's.
        assert_eq!(app.get_exposure(), written.current.light.exposure);
        // A rating after it: the import's edit kept, the rating with it.
        assert!(app.invoke_meta_key("3".into()));
        let after = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(after.meta.rating, 3);
        assert_eq!(after.history.len(), written.history.len());
        assert_eq!(after.current.light.exposure, written.current.light.exposure);
        assert_eq!(after.current.geometry, written.current.geometry);
        WORKER.with(|w| *w.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A panel moved and waiting to be saved when Write is pressed is
    /// saved first, and the frame, an edit of its own now, is left alone.
    #[test]
    fn a_panel_not_yet_saved_is_saved_and_its_frame_left_alone() {
        let dir = crate::testing::scratch_dir("lightroom-unsaved");
        let catalog = pictures(&dir);
        let shots = dir.join("Shoots");
        let app = crate::testing::window(0);
        let (state, worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().write_sidecars = true;
        crate::panel::browser::open_folder(&state, &app, &worker, &shots);
        crate::roots::land_sent(&state, &app, &worker);
        app.invoke_select(0);
        state.borrow_mut().index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog.clone());
        land_now(&state, &app);
        // The slider moved, its save waiting on the panel to rest.
        app.set_exposure(0.8);
        state.borrow().save_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_secs(60),
            || {},
        );
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        assert!(!state.borrow().save_timer.running());
        let a = shots.join("a.tif");
        let saved = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(saved.current.light.exposure, 0.8);
        let report = lrcat::import_with(&catalog, &opts, &mut |_| {}).unwrap();
        let w = report.written.unwrap();
        assert_eq!(w.left_alone, 1, "the frame with the user's edit");
        let after = greycard_edit::Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(after.current.light.exposure, 0.8);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A run told to write no sidecars shows the report with Write off,
    /// says why, and a Write pressed anyway writes nothing.
    #[test]
    fn with_no_sidecars_the_report_shows_and_write_is_off() {
        let dir = scratch("no-sidecars");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        let library = dir.join("library").join("library.sqlite");
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = false;
            st.index_path = Some(library.clone());
        }
        open(&mut state.borrow_mut(), &app, catalog);
        land_now(&state, &app);
        assert_eq!(app.get_lr_roots().row_count(), 1);
        assert!(!app.get_lr_ready());
        assert_eq!(app.get_lr_note(), NO_SIDECARS);
        write(&state, &app);
        assert!(state.borrow().lightroom.thread.is_none());
        assert!(!app.get_lr_running());
        for n in ["IMG_0001.CR3", "IMG_0002.CR3"] {
            assert!(greycard_edit::Sidecar::find(&dir.join("Shoots").join(n)).is_none());
        }
        assert!(!crate::collections::Store::dir_beside(&library).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_goes_to_the_folders_the_report_showed() {
        let dir = scratch("resolved");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().write_sidecars = true;
        state.borrow_mut().index_path = Some(dir.join("library").join("library.sqlite"));
        open(&mut state.borrow_mut(), &app, catalog);
        land_now(&state, &app);
        // The guess beside the catalog becomes the write's mapping.
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        assert_eq!(
            opts.roots,
            vec![("D:/Away/Shoots/".to_string(), dir.join("Shoots"))]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_lands_with_the_report_and_the_window_takes_up_what_it_wrote() {
        let dir = scratch("write");
        let catalog = catalog(&dir);
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, Vec::new());
        state.borrow_mut().write_sidecars = true;
        let library = dir.join("library").join("library.sqlite");
        state.borrow_mut().index_path = Some(library.clone());
        open(&mut state.borrow_mut(), &app, catalog.clone());
        land_now(&state, &app);
        // The write as its thread runs it, landed: given the report's
        // folders, as Write gives them.
        let opts = write_options(&mut state.borrow_mut(), &app).unwrap();
        let mut seen = Vec::new();
        let report = lrcat::import_with(&catalog, &opts, &mut |p| seen.push(progress_line(p)));
        assert!(
            seen.iter()
                .any(|l| l == "importing the catalog: writing 2 of 2 frames"),
            "{seen:?}"
        );
        let report = report.map_err(|e| e.to_string());
        let keys = keys_of(&report);
        wrote(&state, &app, report, keys);
        assert!(app.get_lr_open());
        assert_eq!(app.get_lr_step(), 2);
        assert!(!app.get_lr_running());
        assert!(
            app.get_status()
                .starts_with("imported the catalog: 2 frames written"),
            "{}",
            app.get_status()
        );
        let done: Vec<String> = app
            .get_lr_done()
            .iter()
            .map(|l| l.text.to_string())
            .collect();
        assert!(done[0].starts_with("2 sidecars"), "{done:?}");
        // The root says how the report found it, not "as given" for the
        // mapping the write was handed.
        let lines: Vec<String> = app
            .get_lr_lines()
            .iter()
            .map(|l| l.text.to_string())
            .collect();
        assert!(
            lines.iter().any(|t| t.contains("found beside the catalog")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|t| t.contains("as given")), "{lines:?}");
        // The report itself goes on below, the command's, less those.
        let text: Vec<String> = app
            .get_lr_lines()
            .iter()
            .map(|l| l.text.to_string())
            .collect();
        assert!(
            text.iter().any(|t| t.starts_with("2 files: 2 found")),
            "{text:?}"
        );
        assert!(
            !text.iter().any(|t| t.starts_with("2 sidecars")),
            "{text:?}"
        );
        // The window's collections are the ones it wrote: Picks and
        // the book, under the catalog's set.
        let st = state.borrow();
        let names: Vec<String> = st.collections.list.iter().map(|c| c.name.clone()).collect();
        assert!(names.iter().any(|n| n == "Picks"), "{names:?}");
        assert!(names.iter().any(|n| n == "Album"), "{names:?}");
        assert!(
            st.library
                .roots
                .list()
                .iter()
                .any(|r| r.ends_with("Shoots")),
            "{:?}",
            st.library.roots.list()
        );
        drop(st);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
