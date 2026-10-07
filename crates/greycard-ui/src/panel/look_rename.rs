//! Rename a look: the LOOK section's Rename button, a sheet with the
//! new name, and every file of the look renamed together in the look
//! folder (its own `<name>.cube` and each `<name>.<curve>.cube` that
//! declares its curve, as removal takes them), with the name rewritten
//! wherever this editor keeps an edit of the open folder's: each
//! picture's sidecar, every state of it (the history, what was undone,
//! the snapshots), the presets, the settings clipboard and the export
//! queue. The table's TITLE line, which the list shows when it has one,
//! is inside the file and stays as it is.
//!
//! The order is what keeps a stop midway from leaving an edit
//! "(missing)": every file is first made under the new name beside the
//! old (a hard link, or a copy where the folder takes none), then the
//! edits are rewritten, and only when every one of them was written do
//! the old names go. At any moment each edit names a look whose files
//! are there; a stop leaves at worst the look under both names. A
//! picture whose sidecar could not be read or written keeps the old
//! name, and so does one whose edit goes to an archive's copy that has
//! not been written yet; then the old files stay too, and the status
//! line says so.
//!
//! Every sidecar but the open picture's is read again from disk and
//! renamed there, on a thread of its own while the sheet says it is
//! working, and written back where it was found: what is on disk may be
//! newer than what the window holds, and the rename must not put the
//! window's copy over it. The open picture keeps the edit on the panel,
//! as any save of it does. A sidecar the rename cannot reach (offline,
//! followed to an archive and not read, sidecars off) keeps the old
//! files too; one whose read was out when the rename began takes the new
//! name as it lands (`caught_up`); and the thread is watched, so a share
//! that stops answering lands what was done and keeps the old files.
//!
//! Like removal, it is outside the history: the rename is the look's,
//! not a step in developing any picture, and every state is rewritten
//! in place so undo never goes back to a name that is not there. The
//! renamed states take new ids (`Sidecar::rename_look`), so a copy of a
//! sidecar elsewhere joins with this one without losing either.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use greycard_edit::look::{self, Entry};
use greycard_edit::{Placement, Sidecar};

use crate::panel::assets::{refresh_presets, show_looks};
use crate::panel::browser::show_badges;
use crate::panel::edit::{save_sidecar, writable};
use crate::panel::history::show_history;
use crate::panel::look_remove::{files_of, list_in, store};
use crate::*;

/// The sheet's question while it is up.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    /// The look's name now.
    pub(crate) name: String,
    /// Its files, its own first, as the folder had them at the ask.
    pub(crate) files: Vec<PathBuf>,
    pub(crate) store: PathBuf,
    /// What is in the folder already, for the refusals as the name is
    /// typed.
    folder: Folder,
    /// The look's own file's entry, for what the list will show.
    own: Option<Entry>,
    /// The looks each picture of the open list whose edits were in
    /// memory names, in any state.
    in_memory: Vec<BTreeSet<String>>,
    /// The same for the pictures whose sidecars were read off the
    /// window's thread; none while they are being read.
    pub(crate) on_disk: Option<Vec<(PathBuf, BTreeSet<String>)>>,
    pending: usize,
    /// The look each preset in the store names.
    presets: Vec<String>,
    place: &'static str,
    /// Counts the questions, so a count that lands after the sheet
    /// moved on is dropped.
    generation: u64,
}

impl Asked {
    /// How many pictures of the open list, and how many presets, name
    /// the look `name`.
    fn naming(&self, name: &str) -> (usize, usize) {
        let pictures = self
            .in_memory
            .iter()
            .chain(self.on_disk.iter().flatten().map(|(_, looks)| looks))
            .filter(|looks| looks.contains(name))
            .count();
        let presets = self.presets.iter().filter(|p| *p == name).count();
        (pictures, presets)
    }
}

/// A rename under way, from the Rename on its sheet until its thread
/// has said it is done and every read that was out when it began has
/// landed: until then `busy` holds every other rename off.
pub(crate) struct Renaming {
    generation: u64,
    old: String,
    new: String,
    pairs: Vec<(PathBuf, PathBuf)>,
    /// How many sidecars the thread was handed.
    frames: usize,
    /// What the rename reached: the window's own part, then the
    /// thread's as it lands.
    done: Rewritten,
    started: std::time::Instant,
    /// What the thread has done so far, and the word that stops it.
    reached: Arc<Mutex<Reached>>,
    stop: Arc<AtomicBool>,
    /// How many of `reached` the window has taken.
    taken: usize,
    /// The thread's outcomes have been landed (all of them, or what it
    /// had when it stalled or was stopped).
    landed: bool,
    /// The thread has not said it is done: after a stall or a Stop it
    /// may still write the sidecar it was on, which is taken when it
    /// says so.
    thread_out: bool,
    /// The reads that were out when the rename began, by request, and
    /// the frames they read: only these are caught up as they land
    /// (`caught_up`), and the rename is not over until each has.
    catch: HashMap<u64, Vec<PathBuf>>,
    catch_paths: BTreeSet<PathBuf>,
    /// The copies the thread wrote for frames whose read was still out
    /// when it landed: the read, landing later with what the file held
    /// before, is replaced by this.
    written: HashMap<PathBuf, Sidecar>,
    /// The reads out were waited on for as long as a write is, and the
    /// rest counted as not reached.
    gave_up: bool,
    /// The sheet has gone and the status line said what happened.
    reported: bool,
}

/// The names in the look folder, lower-cased, since the file systems
/// of Windows and macOS take two names that differ only in case for
/// one: the looks it lists and every file it holds.
#[derive(Debug, Clone, Default)]
pub(crate) struct Folder {
    looks: Vec<String>,
    files: Vec<String>,
}

impl Folder {
    pub(crate) fn read(store: &Path, looks: &[Entry]) -> Self {
        let files = std::fs::read_dir(store)
            .map(|dir| {
                dir.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            looks: looks.iter().map(|e| e.name.to_lowercase()).collect(),
            files,
        }
    }
}

/// Whether the look the panel has chosen can be renamed from the
/// section: it has files in the look folder and none of them is a link,
/// which would be renamed and not what it points at.
pub(crate) fn renamable(looks: &[Entry], name: &str) -> bool {
    name != look::NONE
        && store().is_some_and(|dir| {
            let files = files_of(looks, name, &dir);
            !files.is_empty() && !files.iter().any(|f| is_link(f))
        })
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Each file of the look named `old` and the file it becomes under
/// `new`: the same folder, the same ending (`.agx.cube`, `.png`).
pub(crate) fn pairs(files: &[PathBuf], old: &str, new: &str) -> Vec<(PathBuf, PathBuf)> {
    files
        .iter()
        .filter_map(|from| {
            let name = from.file_name()?.to_string_lossy().into_owned();
            let ending = name.strip_prefix(old)?;
            Some((from.clone(), from.with_file_name(format!("{new}{ending}"))))
        })
        .collect()
}

/// Why the look `old` cannot be called `new` in this folder, or none
/// when it can (or when `new` is its name already, which renames
/// nothing and is not refused).
pub(crate) fn refusal(folder: &Folder, old: &str, files: &[PathBuf], new: &str) -> Option<String> {
    if let Some(why) = look::name_refusal(new) {
        return Some(why);
    }
    if new == old {
        return None;
    }
    let lower = new.to_lowercase();
    if lower == old.to_lowercase() {
        return Some(
            "Only the case differs from its name now, and Windows and macOS take the two for \
             one name. Rename it to another name first, then to this one."
                .into(),
        );
    }
    if folder.looks.contains(&lower) {
        return Some(format!("There is a look called \"{new}\" already."));
    }
    for (_, to) in pairs(files, old, new) {
        let name = to
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if folder.files.contains(&name.to_lowercase()) {
            return Some(format!("\"{name}\" is in the look folder already."));
        }
    }
    None
}

/// What would stop a rename now, in the status line's words: anything
/// else that writes looks or sidecars, or moves the files they sit
/// beside.
fn busy(st: &State) -> Option<&'static str> {
    if st.look_renaming.is_some() {
        Some("a look is still being renamed")
    } else if st.deleting.is_some() {
        Some("pictures are being deleted; rename the look when that is done")
    } else if st.look_removing {
        Some("a look is still being removed")
    } else if st.camera_match.running() {
        Some("the camera match is running; rename the look when it is done")
    } else if st.exporting.is_some() || st.queue_run.is_some() {
        Some("an export is running; rename the look when it is done")
    } else if st.archive.copying() {
        Some("a copy to or from an archive is running; rename the look when it is done")
    } else if st.import.job.is_some() {
        Some("an import is running; rename the look when it is done")
    } else {
        None
    }
}

/// The looks each picture of the open list names in any state, by the
/// edits in memory (the open picture by the panel's edit too, which may
/// be ahead of its sidecar), and the paths whose edit still stands in
/// from the index's row, to be read off the window's thread.
pub(crate) fn uses(st: &State) -> (Vec<BTreeSet<String>>, Vec<PathBuf>) {
    let mut named = Vec::new();
    let mut unread = Vec::new();
    for (i, file) in st.files.iter().enumerate() {
        if !crate::rows::is_loaded(st, i) {
            unread.push(file.clone());
            continue;
        }
        let mut looks = st
            .sidecars
            .get(i)
            .map(Sidecar::looks_named)
            .unwrap_or_default();
        if Some(i) == st.current && !st.edit.look_lut.lut.is_none() {
            looks.insert(st.edit.look_lut.lut.name().to_string());
        }
        if !looks.is_empty() {
            named.push(looks);
        }
    }
    (named, unread)
}

/// The looks the sidecar beside each of `paths` names in any state,
/// read from disk; a path with no sidecar, or one that will not read,
/// is left out.
pub(crate) fn looks_on_disk(paths: &[PathBuf]) -> Vec<(PathBuf, BTreeSet<String>)> {
    paths
        .iter()
        .filter_map(|p| match Sidecar::load(p) {
            Ok(Some(s)) => Some((p.clone(), s.looks_named())),
            _ => None,
        })
        .collect()
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The sheet's line on what names the look and what it becomes.
pub(crate) fn uses_line(named: usize, pending: usize, presets: usize, place: &str) -> String {
    let mut out = match (named, pending) {
        (0, 0) => format!("No picture {place} names it."),
        (0, _) => String::new(),
        (1, _) => format!(
            "1 picture {place} names it; its edit, history and snapshots will name the new \
             one."
        ),
        (n, _) => format!(
            "{n} pictures {place} name it; their edits, history and snapshots will name the \
             new one."
        ),
    };
    let mut add = |s: String| {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&s);
    };
    if pending > 0 {
        add(format!(
            "Reading the edits of {} to count them...",
            count(pending, "more picture", "more pictures")
        ));
    }
    match presets {
        0 => {}
        1 => add("1 preset names it and will name the new one.".into()),
        n => add(format!("{n} presets name it and will name the new one.")),
    }
    out
}

/// The sheet's warning when the new name is one that edits or presets
/// already name, for a look that is not in the folder: they show it as
/// missing now, and will take this look once it has the name.
pub(crate) fn taken_line(new: &str, pictures: usize, presets: usize, place: &str) -> String {
    let who = match (pictures, presets) {
        (0, 0) => return String::new(),
        (p, 0) => format!("{} {place}", count(p, "picture", "pictures")),
        (0, q) => count(q, "preset", "presets"),
        (p, q) => format!(
            "{} {place} and {}",
            count(p, "picture", "pictures"),
            count(q, "preset", "presets")
        ),
    };
    let verb = if pictures + presets == 1 {
        "names"
    } else {
        "name"
    };
    format!(
        "{who} already {verb} \"{new}\", a look that is not in the folder, and will use this \
         look."
    )
}

/// The sheet's note: what the rename does not reach, and what the list
/// will show the look by when that is its title rather than its name.
fn note(old: &str, new: &str, own: Option<&Entry>) -> String {
    let mut out = format!(
        "Edits in other folders that name {old} will show it as missing; renaming it back \
         to {old} brings them back."
    );
    if let Some(own) = own {
        let after = Entry {
            name: new.to_string(),
            ..own.clone()
        };
        let label = after.label();
        if label != new {
            out.push_str(&format!(
                " The list shows it by the title inside its file, \"{label}\", which stays."
            ));
        }
    }
    out
}

/// Open the sheet for the look the panel has chosen, or say in the
/// status line why there is nothing to ask.
pub(crate) fn ask(st: &mut State, app: &App) {
    if let Some(why) = busy(st) {
        app.set_status(why.into());
        return;
    }
    let name = app.get_look_name().to_string();
    if name == look::NONE {
        app.set_status("no look is chosen to rename".into());
        return;
    }
    let Some(dir) = store() else {
        app.set_status("there is no look directory on this machine".into());
        return;
    };
    // The directory as it is now, as removal reads it.
    st.looks = list_in(Some(dir.clone()));
    let files = files_of(&st.looks, &name, &dir);
    if files.is_empty() {
        app.set_status(format!("{name} has no file in the look directory to rename").into());
        return;
    }
    if let Some(link) = files.iter().find(|f| is_link(f)) {
        app.set_status(
            format!(
                "{} is a link; rename the look in its folder by hand",
                link.file_name().unwrap_or_default().to_string_lossy()
            )
            .into(),
        );
        return;
    }
    let tables = look::tables_of(&st.looks, &name);
    let own = tables
        .iter()
        .find(|e| e.variant.is_none())
        .map(|e| (*e).clone());
    let shown = tables
        .first()
        .map(|e| e.label().to_string())
        .unwrap_or_else(|| name.clone());
    let place = match st.view {
        crate::roots::View::Roots(_) => "in the library",
        _ => "in this folder",
    };
    let (in_memory, unread) = uses(st);
    let presets = st
        .presets
        .iter()
        .filter(|e| !e.preset.edit.look_lut.lut.is_none())
        .map(|e| e.preset.edit.look_lut.lut.name().to_string())
        .collect();
    st.look_rename_generation += 1;
    let generation = st.look_rename_generation;
    let asked = Asked {
        name: name.clone(),
        folder: Folder::read(&dir, &st.looks),
        files,
        store: dir,
        own,
        in_memory,
        on_disk: unread.is_empty().then(Vec::new),
        pending: unread.len(),
        presets,
        place,
        generation,
    };
    app.set_look_rename_title(format!("Rename the look {shown}").into());
    app.set_look_rename_name(name.as_str().into());
    app.set_look_rename_working(false);
    st.look_rename = Some(asked);
    show_sheet(st, app);
    app.set_look_rename_open(true);
    if !unread.is_empty() {
        count_unread(app, generation, unread);
    }
}

/// The sheet's lines for the name in its field.
fn show_sheet(st: &State, app: &App) {
    let Some(asked) = &st.look_rename else {
        return;
    };
    let new = app.get_look_rename_name().trim().to_string();
    let why = refusal(&asked.folder, &asked.name, &asked.files, &new);
    let fine = why.is_none() && new != asked.name;
    let lines: Vec<slint::SharedString> = if fine {
        pairs(&asked.files, &asked.name, &new)
            .iter()
            .map(|(from, to)| {
                format!(
                    "{} → {}",
                    from.file_name().unwrap_or_default().to_string_lossy(),
                    to.file_name().unwrap_or_default().to_string_lossy()
                )
                .into()
            })
            .collect()
    } else {
        asked
            .files
            .iter()
            .map(|f| {
                f.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref()
                    .into()
            })
            .collect()
    };
    let counted = asked.on_disk.is_some();
    let (named, presets) = asked.naming(&asked.name);
    let pending = if counted { 0 } else { asked.pending };
    app.set_look_rename_files(ModelRc::new(VecModel::from(lines)));
    app.set_look_rename_refusal(why.clone().unwrap_or_default().into());
    app.set_look_rename_uses(uses_line(named, pending, presets, asked.place).into());
    let taken = if fine {
        let (pictures, presets) = asked.naming(&new);
        taken_line(&new, pictures, presets, asked.place)
    } else {
        String::new()
    };
    app.set_look_rename_warning(taken.into());
    let shown_new = if fine { &new } else { &asked.name };
    app.set_look_rename_note(note(&asked.name, shown_new, asked.own.as_ref()).into());
    app.set_look_rename_ready(fine && counted);
}

/// Read the edits that stood in from the index on a thread of their
/// own, and put what they say on the sheet when it is done.
#[cfg(not(test))]
fn count_unread(app: &App, generation: u64, unread: Vec<PathBuf>) {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard look uses".into())
        .spawn(move || {
            let found = looks_on_disk(&unread);
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                ) else {
                    return;
                };
                counted(&mut state.borrow_mut(), &app, generation, found);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("look uses: {e}");
    }
}

/// A test counts them itself, through [`looks_on_disk`] and
/// [`counted`].
#[cfg(test)]
fn count_unread(_app: &App, _generation: u64, _unread: Vec<PathBuf>) {}

/// The looks the pictures read from disk name, landed: the sheet counts
/// them and Rename is offered.
pub(crate) fn counted(
    st: &mut State,
    app: &App,
    generation: u64,
    found: Vec<(PathBuf, BTreeSet<String>)>,
) {
    match &mut st.look_rename {
        Some(asked) if asked.generation == generation => asked.on_disk = Some(found),
        _ => return,
    }
    show_sheet(st, app);
}

/// Make `to` hold what `from` does without touching `from`: a hard
/// link, or where the folder takes none (FAT, exFAT, some shares) a
/// copy, made under a name the look list does not read and put in
/// place once whole. Never over a file that is there.
fn link_one(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(e) => tracing::debug!("{}: no hard link ({e}), copied", to.display()),
    }
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::copy(from, &part)?;
    if std::fs::symlink_metadata(to).is_ok() {
        let _ = std::fs::remove_file(&part);
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(&part, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// The first stage: every file under its new name too. On a failure
/// the new names made so far go again and the old are as they were.
pub(crate) fn link_all(pairs: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    for (i, (from, to)) in pairs.iter().enumerate() {
        if let Err(e) = link_one(from, to) {
            for (_, made) in &pairs[..i] {
                let _ = std::fs::remove_file(made);
            }
            return Err(format!(
                "{} could not be made ({e})",
                to.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }
    Ok(())
}

/// The last stage: the old names go, the curves' tables first, so a
/// stop midway leaves the old look's own file rather than a curve's
/// table with no look. A name goes only while its new one is there and
/// as long. The ones that could not go, and why.
pub(crate) fn unlink_old(pairs: &[(PathBuf, PathBuf)]) -> Vec<(PathBuf, String)> {
    let mut failed = Vec::new();
    for (from, to) in pairs.iter().rev() {
        let whole = match (std::fs::metadata(from), std::fs::metadata(to)) {
            (Ok(a), Ok(b)) => a.len() == b.len(),
            _ => false,
        };
        if !whole {
            failed.push((from.clone(), "its new copy is not whole".to_string()));
            continue;
        }
        if let Err(e) = std::fs::remove_file(from) {
            failed.push((from.clone(), e.to_string()));
        }
    }
    failed
}

/// What the rewrite reached.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Rewritten {
    /// Pictures whose edit now names the new name.
    pub(crate) pictures: usize,
    /// Pictures that name the old name still: a sidecar that could not
    /// be read or written.
    pub(crate) stale: usize,
    /// Pictures whose edit goes to an archive's copy, which is written
    /// after the window has moved on: they name the old name until it
    /// lands.
    pub(crate) archived: usize,
    /// Pictures whose sidecars the rename cannot reach at all: under a
    /// root that is offline, followed to an archive and not read yet,
    /// or every one when sidecars are off and none was read.
    pub(crate) unreachable: usize,
    pub(crate) presets: usize,
    pub(crate) presets_stale: usize,
}

impl Rewritten {
    /// Whether anything may still name the old name, so its files stay.
    fn keeps_old(&self) -> bool {
        self.stale + self.archived + self.unreachable + self.presets_stale > 0
    }
}

/// One sidecar for the thread: the frame, the window's copy of its
/// sidecar when it holds one, and that copy's latest revision, which
/// tells at the landing whether the window's copy moved meanwhile.
pub(crate) struct JobFrame {
    path: PathBuf,
    memory: Option<Sidecar>,
}

/// What came of each sidecar the thread reached, in order: the frame,
/// the latest revision of the window's copy as it was sent (none when
/// the window held none), and the outcome.
type Reached = Vec<(PathBuf, Option<Option<String>>, Outcome)>;

/// The sidecars a rename reads, renames and writes off the window's
/// thread.
pub(crate) struct Job {
    old: String,
    new: String,
    frames: Vec<JobFrame>,
    /// Where a sidecar the frame has none of yet is written.
    placement: Placement,
    /// Where each outcome is put as it comes, so a landing that does not
    /// wait for the end (a stall, a Stop) has what was done.
    reached: Arc<Mutex<Reached>>,
    /// Set by such a landing: the thread takes no further sidecar.
    stop: Arc<AtomicBool>,
}

/// What came of one sidecar.
pub(crate) enum Outcome {
    /// Renamed and written: the window takes this copy.
    Written(Sidecar),
    /// The file names the look nowhere, though the window's copy did:
    /// the file was changed elsewhere, and the window takes it.
    Adopted(Sidecar),
    /// Nothing of the look in it.
    Untouched,
    /// It could not be read or written, and names the old name still.
    Failed(String),
}

impl Job {
    /// Every sidecar read from where it is and, when it names the look,
    /// renamed and written back there; one the frame has none of on
    /// disk is the window's copy, written where sidecars go. `beat` is
    /// called after each, for the watch on a stalled share; a panic is
    /// that one sidecar's failure, and the rest go on.
    pub(crate) fn run(self, beat: &dyn Fn()) {
        let Job {
            old,
            new,
            frames,
            placement,
            reached,
            stop,
        } = self;
        for frame in frames {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let path = frame.path.clone();
            let sent = frame
                .memory
                .as_ref()
                .map(|m| m.revisions.last().map(|r| r.hash.clone()));
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                one(frame, &old, &new, placement, &stop)
            }))
            .unwrap_or_else(|payload| {
                Outcome::Failed(format!(
                    "panicked: {}",
                    crate::worker::panic_message(payload.as_ref())
                ))
            });
            if let Ok(mut r) = reached.lock() {
                r.push((path, sent, outcome));
            }
            beat();
        }
    }
}

fn one(frame: JobFrame, old: &str, new: &str, placement: Placement, stop: &AtomicBool) -> Outcome {
    let JobFrame { path, memory } = frame;
    let held_names = memory.as_ref().is_some_and(|m| m.names_look(old));
    let (mut sidecar, place) = match Sidecar::find(&path) {
        Some(file) => match Sidecar::read(&file) {
            Ok(mut read) => {
                if !read.names_look(old) {
                    return if held_names {
                        Outcome::Adopted(read)
                    } else {
                        Outcome::Untouched
                    };
                }
                // What the window undid goes with it, when the file is
                // where the window left it.
                if let Some(m) = memory.filter(|m| m.current == read.current) {
                    read.redo = m.redo;
                }
                (read, Placement::at(&file))
            }
            Err(e) => return Outcome::Failed(e.to_string()),
        },
        // No sidecar found because the frame's folder is not there (a
        // share gone since the list was read): nothing says what its
        // sidecar names.
        None if !path.parent().is_some_and(Path::is_dir) => {
            return Outcome::Failed("its folder is not there".into());
        }
        None => match memory {
            Some(m) if held_names => (m, placement),
            _ => return Outcome::Untouched,
        },
    };
    sidecar.rename_look(old, new);
    // Landed without it meanwhile (a stall, a Stop): not written now.
    if stop.load(Ordering::Relaxed) {
        return Outcome::Failed("stopped before it was written".into());
    }
    match sidecar.save_in(&path, place) {
        Ok(()) => Outcome::Written(sidecar),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// What a rename did, in the status line's words.
fn summary(
    old: &str,
    new: &str,
    files: usize,
    done: &Rewritten,
    unlinked: &[(PathBuf, String)],
) -> String {
    let mut reached = vec![count(files, "file", "files")];
    if done.pictures > 0 {
        reached.push(count(done.pictures, "picture", "pictures"));
    }
    if done.presets > 0 {
        reached.push(count(done.presets, "preset", "presets"));
    }
    let mut out = format!("renamed {old} to {new}: {}", reached.join(", "));
    let stale = done.stale + done.presets_stale;
    let mut why = Vec::new();
    if stale > 0 {
        let what = match (done.stale, done.presets_stale) {
            (p, 0) => count(p, "picture's edit", "pictures' edits"),
            (0, p) => count(p, "preset", "presets"),
            (p, q) => format!(
                "{} and {}",
                count(p, "picture's edit", "pictures' edits"),
                count(q, "preset", "presets")
            ),
        };
        why.push(format!(
            "{what} could not be read or written and still name {old}"
        ));
    }
    if done.unreachable > 0 {
        why.push(format!(
            "{} could not be reached (offline, on an archive and not read yet, or sidecars \
             off) and may still name {old}",
            count(done.unreachable, "picture's sidecar", "pictures' sidecars")
        ));
    }
    if done.archived > 0 {
        why.push(format!(
            "{} to an archive's copy, which is written later",
            count(done.archived, "picture's edit goes", "pictures' edits go")
        ));
    }
    if !why.is_empty() {
        out.push_str(&format!(
            "; {}, so {old}'s files stay under that name too until you remove them",
            why.join("; ")
        ));
    } else if let Some((file, why)) = unlinked.first() {
        out.push_str(&format!(
            "; {} stays under the old name ({why})",
            file.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    out
}

/// The rename, once the sheet said yes: the folder read again and the
/// name checked against it, the files made under the new name, the
/// window's own edits renamed (the open picture's, those that go to an
/// archive's copy, the presets, the clipboard, the queue), and every
/// other sidecar handed to a thread. An error is the status line's
/// words, and nothing was changed.
pub(crate) fn start(st: &mut State, app: &App, asked: &Asked, new: &str) -> Result<(), String> {
    let old = asked.name.as_str();
    let looks = list_in(Some(asked.store.clone()));
    let files = files_of(&looks, old, &asked.store);
    if files.is_empty() {
        return Err(format!(
            "{old} has no file in the look directory now; nothing was renamed"
        ));
    }
    if new == old {
        return Err(format!("{old} is its name already"));
    }
    let folder = Folder::read(&asked.store, &looks);
    if let Some(why) = refusal(&folder, old, &files, new) {
        return Err(format!("{old} was not renamed: {why}"));
    }
    if let Some(link) = files.iter().find(|f| is_link(f)) {
        return Err(format!(
            "{old} was not renamed: {} is a link",
            link.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    let pairs = pairs(&files, old, new);
    if let Err(why) = link_all(&pairs) {
        return Err(format!("{old} was not renamed: {why}; nothing changed"));
    }
    // The reads out now carry what the files held before the thread
    // renames them: those, and only those, are caught up as they land.
    let catch = st.loads.in_flight.clone();
    let catch_paths = catch.values().flatten().cloned().collect();
    let mut done = Rewritten::default();
    // With sidecars off none was read, so any of them may name the old
    // name: the old files stay.
    if !st.write_sidecars {
        done.unreachable = st.files.len();
    }
    // The open picture: the panel's edit and the sidecar in memory,
    // saved as any save of it is.
    let open = st.current.filter(|&c| crate::rows::panel_is_frames(st, c));
    let open_edit = st.edit.look_lut.rename(old, new);
    if let Some(c) = open {
        let renamed = st.sidecars[c].rename_look(old, new);
        if renamed || open_edit {
            done.pictures += 1;
        }
        if renamed && st.write_sidecars {
            save_window_copy(st, c, &mut done);
        }
    }
    // The rest: those that go to an archive's copy, and with sidecars
    // off every one, from the window's copy; those it cannot reach,
    // counted; every other sidecar from disk, on the thread.
    let mut frames = Vec::new();
    for i in 0..st.files.len() {
        if Some(i) == open || crate::panel::delete::held(st, i) {
            continue;
        }
        let loaded = crate::rows::is_loaded(st, i);
        if !st.write_sidecars {
            if loaded && st.sidecars[i].rename_look(old, new) {
                done.pictures += 1;
            }
            continue;
        }
        if crate::sync::followed(st, i) {
            if !loaded {
                done.unreachable += 1;
            } else if st.sidecars[i].rename_look(old, new) {
                done.pictures += 1;
                save_window_copy(st, i, &mut done);
            }
            continue;
        }
        if crate::rows::is_offline(st, i) {
            done.unreachable += 1;
            continue;
        }
        frames.push(JobFrame {
            path: st.files[i].clone(),
            memory: loaded.then(|| st.sidecars[i].clone()),
        });
    }
    rename_elsewhere(st, app, old, new, &mut done);
    st.look_rename_generation += 1;
    let generation = st.look_rename_generation;
    let reached = Arc::new(Mutex::new(Vec::with_capacity(frames.len())));
    let stop = Arc::new(AtomicBool::new(false));
    st.look_renaming = Some(Renaming {
        generation,
        old: old.to_string(),
        new: new.to_string(),
        pairs,
        frames: frames.len(),
        done,
        started: std::time::Instant::now(),
        reached: reached.clone(),
        stop: stop.clone(),
        taken: 0,
        landed: false,
        thread_out: true,
        catch,
        catch_paths,
        written: HashMap::new(),
        gave_up: false,
        reported: false,
    });
    let job = Job {
        old: old.to_string(),
        new: new.to_string(),
        frames,
        placement: st.placement,
        reached,
        stop,
    };
    app.set_status(format!("renaming {old} to {new}...").into());
    send_off(app, generation, job);
    Ok(())
}

/// Write the window's copy of frame `i`'s sidecar after a rename: to
/// an archive's copy for a frame the window follows there, which lands
/// later, or here; one that could not be written is counted stale.
fn save_window_copy(st: &mut State, i: usize, done: &mut Rewritten) {
    if crate::sync::followed(st, i) {
        save_sidecar(st, i);
        done.archived += 1;
    } else if !(writable(st, i) && save_sidecar(st, i)) {
        done.pictures -= 1;
        done.stale += 1;
    }
}

/// The edits the window keeps outside the sidecars: the settings
/// clipboard, the hovered and held states, the command line's
/// override, the export queue, and the presets in the store.
fn rename_elsewhere(st: &mut State, app: &App, old: &str, new: &str, done: &mut Rewritten) {
    if let Some(c) = &mut st.clipboard {
        c.edit.look_lut.rename(old, new);
    }
    for edit in [st.peek.as_mut(), st.held.as_mut()].into_iter().flatten() {
        edit.look_lut.rename(old, new);
    }
    if let Some((_, edit)) = &mut st.overridden {
        edit.look_lut.rename(old, new);
    }
    let mut queued = false;
    for entry in &mut st.export_queue {
        for frame in &mut entry.frames {
            queued |= frame.edit.look_lut.rename(old, new);
        }
    }
    if queued {
        crate::panel::export_queue::keep(st);
    }
    if let Some(store) = st.preset_store.clone() {
        for mut entry in store.list() {
            if !entry.preset.edit.look_lut.rename(old, new) {
                continue;
            }
            match entry.preset.save_to(&entry.path) {
                Ok(()) => done.presets += 1,
                Err(e) => {
                    tracing::warn!("preset {}: {e}", entry.path.display());
                    done.presets_stale += 1;
                }
            }
        }
        refresh_presets(st, app);
    }
}

/// Run the sidecars' part on a thread of its own, watched as an
/// archive write is: one that goes quiet for `sync::WRITE_WAIT` on a
/// sidecar (a share that stopped answering) is landed with what it
/// had, the rest counted as not written; what it does after, it says
/// when it is done, and that is taken then.
#[cfg(not(test))]
fn send_off(app: &App, generation: u64, job: Job) {
    let app_weak = app.as_weak();
    let on_window = move |thread_done: bool, aside: bool| {
        let app_weak = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let (Some(app), Some(state)) = (
                app_weak.upgrade(),
                crate::STATE.with(|s| s.borrow().clone()),
            ) else {
                return;
            };
            if aside {
                tracing::warn!(
                    "rename look: a sidecar has not answered for {} s; landed without the rest",
                    crate::sync::WRITE_WAIT.as_secs()
                );
            }
            heard(&mut state.borrow_mut(), &app, generation, thread_done);
        });
    };
    let watch = {
        let on_window = on_window.clone();
        move |heard: crate::archive::Heard<()>| match heard {
            crate::archive::Heard::Aside => on_window(false, true),
            crate::archive::Heard::Done { .. } | crate::archive::Heard::Lost => {
                on_window(true, false)
            }
        }
    };
    let spawned = crate::archive::supervise(
        "rename look",
        crate::sync::WRITE_WAIT,
        move |beat| job.run(&|| beat()),
        watch,
    );
    if let Err(e) = spawned {
        tracing::warn!("rename look: the thread did not start: {e}");
        on_window(true, false);
    }
}

/// In a test the job is queued, and the test runs it and lands it.
#[cfg(test)]
fn send_off(_app: &App, generation: u64, job: Job) {
    tests::SENT.with(|s| s.borrow_mut().push((generation, job)));
}

/// A read that was out when the rename began has landed frame `i`'s
/// sidecar, as the file held it before the thread renamed it: it takes
/// the copy the thread wrote when the thread has landed one, else the
/// new name in memory (the thread's copy replaces it as it lands). No
/// other read is touched, and nothing is written here. True when the
/// frame was caught up.
pub(crate) fn caught_up(st: &mut State, i: usize) -> bool {
    let Some(path) = st.files.get(i).cloned() else {
        return false;
    };
    let Some(r) = st.look_renaming.as_mut() else {
        return false;
    };
    if !r.catch_paths.remove(&path) {
        return false;
    }
    let (old, new) = (r.old.clone(), r.new.clone());
    // The old files are kept until the reads are in, so a look of the
    // old name is the old look; one listed again after they went is a
    // new look of that name, and not this one.
    if r.reported && st.looks.iter().any(|e| e.name.eq_ignore_ascii_case(&old)) {
        return false;
    }
    match r.written.remove(&path) {
        Some(copy) => st.sidecars[i] = copy,
        None => {
            if !st.sidecars[i].rename_look(&old, &new) {
                return false;
            }
        }
    }
    tracing::info!(
        "rename look: {} came in from a read that was out, and takes {new}",
        path.display()
    );
    true
}

/// A read of the window's has landed (request `id`): if it was out when
/// the rename began, the rename has one fewer to wait on.
pub(crate) fn read_landed(st: &mut State, app: &App, id: u64) {
    let Some(r) = st.look_renaming.as_mut() else {
        return;
    };
    if let Some(paths) = r.catch.remove(&id) {
        // A path the read left out (a list since changed) is not waited
        // on: nothing more will bring it.
        for p in paths {
            r.catch_paths.remove(&p);
        }
        settle(st, app);
    }
}

/// The thread was heard from: done (`thread_done`), or gone quiet or
/// stopped. What it reached since the last time is taken: at the first
/// landing as the rename's outcome, after it (a sidecar written after
/// a stall or a Stop) as a late one, which keeps the window and the
/// file in step but changes nothing of what was said.
pub(crate) fn heard(st: &mut State, app: &App, generation: u64, thread_done: bool) {
    let Some(mut r) = st.look_renaming.take_if(|r| r.generation == generation) else {
        return;
    };
    let clock = std::time::Instant::now();
    if !thread_done {
        // A landing that does not wait for the end: the thread takes no
        // further sidecar, and checks again before it writes the one it
        // is on.
        r.stop.store(true, Ordering::Relaxed);
    }
    if thread_done {
        r.thread_out = false;
    }
    let reached: Reached = r
        .reached
        .lock()
        .map(|mut v| v.split_off(0))
        .unwrap_or_default();
    r.taken += reached.len();
    let late = r.landed;
    let written = take_outcomes(st, app, &mut r, reached, late);
    if !r.landed {
        r.landed = true;
        if r.thread_out {
            r.done.stale += r.frames.saturating_sub(r.taken);
        }
        if !r.catch.is_empty() {
            give_up_later(app, generation);
        }
    }
    after_saves(st, app, written);
    tracing::debug!(
        "rename look: {} landed in {:.3} s on the window's thread",
        if late { "a late outcome" } else { "the thread" },
        clock.elapsed().as_secs_f64()
    );
    st.look_renaming = Some(r);
    settle(st, app);
}

/// Take what the thread reached into the window: one pass over the
/// list, the outcomes that touched nothing passed over first. The paths
/// written, for the index and the archives' copies.
fn take_outcomes(
    st: &mut State,
    app: &App,
    r: &mut Renaming,
    reached: Reached,
    late: bool,
) -> Vec<PathBuf> {
    let at: HashMap<PathBuf, usize> = st
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.clone(), i))
        .collect();
    let mut written = Vec::new();
    for (path, sent, outcome) in reached {
        let (sidecar, wrote) = match outcome {
            Outcome::Untouched => continue,
            Outcome::Failed(why) => {
                tracing::warn!("rename look: {}: {why}", path.display());
                if !late {
                    r.done.stale += 1;
                }
                continue;
            }
            Outcome::Written(s) => {
                if !late {
                    r.done.pictures += 1;
                }
                written.push(path.clone());
                (s, true)
            }
            Outcome::Adopted(s) => (s, false),
        };
        let loaded = at
            .get(&path)
            .copied()
            .filter(|&i| crate::rows::is_loaded(st, i));
        let Some(i) = loaded else {
            // Its read is still out: the copy waits for it.
            if wrote && r.catch_paths.contains(&path) {
                r.written.insert(path, sidecar);
            }
            continue;
        };
        let now = st.sidecars[i].revisions.last().map(|r| r.hash.clone());
        if sent.is_none() || sent == Some(now) {
            st.sidecars[i] = sidecar;
            show_badges(st, app, i);
            continue;
        }
        // The window's copy moved while the thread had the file (a sync
        // with an archive's copy brought a newer one, or joined the
        // two): that copy is the one to keep, renamed and saved as any
        // save is, so the other machine's states stay.
        if wrote {
            written.retain(|p| *p != path);
        }
        if st.sidecars[i].rename_look(&r.old, &r.new)
            && !(writable(st, i) && save_sidecar(st, i))
            && !late
        {
            if wrote {
                r.done.pictures -= 1;
            }
            r.done.stale += 1;
        }
        show_badges(st, app, i);
    }
    written
}

/// The reads out when the rename began are waited on for as long as a
/// write is; then the frames they have not brought are counted as not
/// reached, and the old files stay for them.
fn give_up_later(app: &App, generation: u64) {
    #[cfg(not(test))]
    {
        let app_weak = app.as_weak();
        slint::Timer::single_shot(crate::sync::WRITE_WAIT, move || {
            let (Some(app), Some(state)) = (
                app_weak.upgrade(),
                crate::STATE.with(|s| s.borrow().clone()),
            ) else {
                return;
            };
            give_up(&mut state.borrow_mut(), &app, generation);
        });
    }
    #[cfg(test)]
    let _ = (app, generation);
}

/// The wait on the reads out is over: what they have not brought is
/// not reached.
pub(crate) fn give_up(st: &mut State, app: &App, generation: u64) {
    let Some(r) = st
        .look_renaming
        .as_mut()
        .filter(|r| r.generation == generation && !r.catch.is_empty())
    else {
        return;
    };
    tracing::warn!(
        "rename look: {} sidecar read(s) out when it began did not land in {} s",
        r.catch.len(),
        crate::sync::WRITE_WAIT.as_secs()
    );
    r.done.unreachable += r.catch_paths.len();
    r.catch.clear();
    r.catch_paths.clear();
    r.gave_up = true;
    settle(st, app);
}

/// Where the rename stands: once the thread's outcomes are in and every
/// read out when it began has landed (or been given up on), the old
/// files go when nothing may name them, the list and the panel are
/// shown again, the sheet goes and the status line says what happened;
/// once the thread has also said it is done, the rename is over and
/// another may begin.
fn settle(st: &mut State, app: &App) {
    let Some(r) = st.look_renaming.as_mut() else {
        return;
    };
    if !r.landed || !r.catch.is_empty() {
        return;
    }
    if !r.reported {
        r.reported = true;
        let unlinked = if r.done.keeps_old() {
            Vec::new()
        } else {
            unlink_old(&r.pairs)
        };
        let status = summary(&r.old, &r.new, r.pairs.len(), &r.done, &unlinked);
        tracing::info!(
            "rename look: {status} ({:.2} s in all)",
            r.started.elapsed().as_secs_f64()
        );
        look::forget();
        st.looks = list_in(store());
        st.look_for = None;
        show_looks(st, &st.edit.look_lut, app);
        show_history(st, app);
        app.set_look_rename_working(false);
        app.set_look_rename_open(false);
        app.set_status(status.into());
        app.window().request_redraw();
    }
    if st.look_renaming.as_ref().is_some_and(|r| !r.thread_out) {
        st.look_renaming = None;
    }
}

/// The index and the archives' copies told of the sidecars the thread
/// wrote, as a save tells them, a few hundred an event-loop turn: the
/// pairing with an archive is a query of the index each, which for a
/// library of thousands would hold the window.
fn after_saves(st: &mut State, app: &App, mut paths: Vec<PathBuf>) {
    const TURN: usize = 200;
    let rest = paths.split_off(paths.len().min(TURN));
    let at: HashMap<&PathBuf, usize> = st.files.iter().enumerate().map(|(i, f)| (f, i)).collect();
    let now: Vec<usize> = paths.iter().filter_map(|p| at.get(p).copied()).collect();
    for i in now {
        crate::library::sidecar_written(st, i);
    }
    if rest.is_empty() {
        return;
    }
    let app_weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let (Some(app), Some(state)) = (
            app_weak.upgrade(),
            crate::STATE.with(|s| s.borrow().clone()),
        ) else {
            return;
        };
        after_saves(&mut state.borrow_mut(), &app, rest);
    });
}

/// The sheet's answer: yes renames to the name in its field. While a
/// rename is out the sheet stays, says so, and its one answer is Stop,
/// which lands what is done.
pub(crate) fn answered(st: &mut State, app: &App, yes: bool) {
    if let Some(r) = &st.look_renaming {
        if !yes && !r.reported {
            let (generation, landed) = (r.generation, r.landed);
            tracing::info!("rename look: stopped");
            if landed {
                // Waiting on the reads out: no longer.
                give_up(st, app, generation);
            } else {
                heard(st, app, generation, false);
            }
        }
        return;
    }
    let close = |app: &App| app.set_look_rename_open(false);
    let Some(asked) = st.look_rename.take() else {
        close(app);
        return;
    };
    if !yes {
        close(app);
        return;
    }
    let new = app.get_look_rename_name().trim().to_string();
    let refused = if !st.deletes_allowed {
        Some("not renamed: a capture, an export or a timing run never renames".to_string())
    } else if let Some(why) = busy(st) {
        Some(why.to_string())
    } else if asked.on_disk.is_none() {
        Some("not renamed: the folder's edits were still being read".to_string())
    } else {
        start(st, app, &asked, &new).err()
    };
    match refused {
        Some(why) => {
            tracing::info!("rename look: {why}");
            close(app);
            app.set_status(why.into());
        }
        None => {
            tracing::info!(
                "rename look: begun, {} sidecars to their thread",
                st.look_renaming.as_ref().map_or(0, |r| r.frames)
            );
            app.set_look_rename_working(true);
            app.set_look_rename_ready(false);
            app.set_look_rename_uses("Renaming: the edits are being read and written...".into());
        }
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_rename_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ask(&mut state.borrow_mut(), &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_rename_edited(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            show_sheet(&state.borrow(), &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_rename_answered(move |yes| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            answered(&mut state.borrow_mut(), &app, yes);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{LOOK_STORE as STORE, click, press, state_for, type_text, window};
    use greycard_edit::look::{LookLut, LutChoice};
    use greycard_edit::{DisplayCurve, Edit, Preset, Section};
    use slint::platform::Key;

    thread_local! {
        pub(super) static SENT: RefCell<Vec<(u64, Job)>> = const { RefCell::new(Vec::new()) };
    }

    /// The job the sheet sent, taken.
    fn sent() -> (u64, Job) {
        let mut jobs: Vec<(u64, Job)> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        assert_eq!(jobs.len(), 1, "one job sent");
        jobs.remove(0)
    }

    /// The rename's thread, run here: the job the sheet sent, run and
    /// landed.
    fn run_sent(state: &Rc<RefCell<State>>, app: &App) {
        let (generation, job) = sent();
        job.run(&|| {});
        heard(&mut state.borrow_mut(), app, generation, true);
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = crate::testing::scratch_dir(&format!("look-rename-{what}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A one-node table, with the comment lines given above it.
    fn table(dir: &Path, file: &str, comments: &[&str]) -> PathBuf {
        let mut text = String::new();
        for c in comments {
            text.push_str(&format!("# {c}\n"));
        }
        text.push_str("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    text.push_str(&format!("{r} {g} {b}\n"));
                }
            }
        }
        let path = dir.join(file);
        std::fs::write(&path, text).unwrap();
        path
    }

    /// A store holding Neon (its own table and an AgX one), Neon Film
    /// (a look whose name starts with Neon's) and Mono.
    fn store_of(dir: &Path) -> PathBuf {
        let store = dir.join("looks");
        std::fs::create_dir_all(&store).unwrap();
        table(&store, "Neon.cube", &[]);
        table(&store, "Neon.agx.cube", &["display_curve: agx"]);
        table(&store, "Neon Film.cube", &[]);
        table(&store, "Mono.cube", &[]);
        store
    }

    fn there(store: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(store)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn named(name: &str) -> LookLut {
        LookLut {
            lut: LutChoice::Named(name.into()),
            strength: 0.8,
        }
    }

    fn neon_files(store: &Path) -> Vec<PathBuf> {
        files_of(&list_in(Some(store.to_path_buf())), "Neon", store)
    }

    /// Each file keeps its ending under the new name; a look whose name
    /// only starts with this one's is not among them.
    #[test]
    fn every_table_of_the_look_takes_the_new_name_and_its_own_ending() {
        let dir = scratch("pairs");
        let store = store_of(&dir);
        let names: Vec<(String, String)> = pairs(&neon_files(&store), "Neon", "Glow")
            .iter()
            .map(|(a, b)| {
                (
                    a.file_name().unwrap().to_string_lossy().into_owned(),
                    b.file_name().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("Neon.cube".into(), "Glow.cube".into()),
                ("Neon.agx.cube".into(), "Glow.agx.cube".into())
            ]
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A name the folder has already, as a look or as a file, is
    /// refused case aside; a change of case alone is refused with why;
    /// the platform's rules come through; the look's own name is no
    /// refusal, only nothing to do.
    #[test]
    fn a_name_the_folder_has_already_is_refused() {
        let dir = scratch("refused");
        let store = store_of(&dir);
        // A film preset that is only called Glow.agx: a look of its own,
        // whose file a rename to Glow would need for its AgX table.
        table(&store, "Glow.agx.cube", &[]);
        let looks = list_in(Some(store.clone()));
        let folder = Folder::read(&store, &looks);
        let files = files_of(&looks, "Neon", &store);
        let why = |new: &str| refusal(&folder, "Neon", &files, new);
        assert_eq!(
            why("Mono").as_deref(),
            Some("There is a look called \"Mono\" already.")
        );
        assert_eq!(
            why("neon film").as_deref(),
            Some("There is a look called \"neon film\" already.")
        );
        assert!(why("NEON").unwrap().starts_with("Only the case differs"));
        assert_eq!(
            why("Glow").as_deref(),
            Some("\"Glow.agx.cube\" is in the look folder already.")
        );
        assert!(why("a:b").unwrap().contains("Windows or macOS"));
        assert!(why("Neon.agx").unwrap().contains("AgX table"));
        assert!(why("").unwrap().contains("needs a name"));
        assert_eq!(why("Neon"), None);
        assert_eq!(why("Neon Glow"), None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The note says what the list will show the look by when its file's
    /// title, not its new name, is what the list shows: a film's own
    /// title, or a fitted look's title once its name no longer starts
    /// with the body.
    #[test]
    fn the_note_says_when_the_list_shows_the_title_instead() {
        let dir = scratch("note");
        let store = dir.join("looks");
        std::fs::create_dir_all(&store).unwrap();
        let plain = table(&store, "Plain.cube", &[]);
        let film = dir.join("looks/Film.cube");
        std::fs::write(
            &film,
            format!(
                "TITLE \"Kodak Portra\"\n{}",
                std::fs::read_to_string(&plain).unwrap()
            ),
        )
        .unwrap();
        let fitted = store.join("Canon EOS R6 Mark II Faithful.cube");
        std::fs::write(
            &fitted,
            format!(
                "TITLE \"{}\"\n{}",
                look::fitted_title(
                    "Canon EOS R6 Mark II Faithful",
                    "Canon EOS R6 Mark II",
                    Some(40)
                ),
                std::fs::read_to_string(&plain).unwrap()
            ),
        )
        .unwrap();
        let looks = list_in(Some(store.clone()));
        let own = |name: &str| look::tables_of(&looks, name)[0].clone();
        assert!(!note("Plain", "Flat", Some(&own("Plain"))).contains("title"));
        assert!(
            note("Film", "Portra", Some(&own("Film")))
                .ends_with("by the title inside its file, \"Kodak Portra\", which stays.")
        );
        let r6 = "Canon EOS R6 Mark II Faithful";
        assert!(!note(r6, r6, Some(&own(r6))).contains("title"));
        assert!(note(r6, "My R6", Some(&own(r6))).contains("(fitted on Canon EOS R6 Mark II"));
        crate::testing::remove_dir_retry(&dir);
    }

    /// The first stage makes every new name beside the old; when one
    /// cannot be made the ones made go again and nothing else moved.
    #[test]
    fn a_link_that_fails_takes_back_what_it_made() {
        let dir = scratch("link");
        let store = store_of(&dir);
        let files = neon_files(&store);
        // Something is in the way of the second.
        table(&store, "Glow.agx.cube", &["display_curve: channels"]);
        let before = there(&store);
        let why = link_all(&pairs(&files, "Neon", "Glow")).unwrap_err();
        assert!(why.starts_with("Glow.agx.cube could not be made"), "{why}");
        assert_eq!(there(&store), before);

        // Without it both are made, and the old stay until the last
        // stage, which takes them only while the new are whole.
        std::fs::remove_file(store.join("Glow.agx.cube")).unwrap();
        let pairs = pairs(&files, "Neon", "Glow");
        link_all(&pairs).unwrap();
        assert_eq!(
            there(&store),
            [
                "Glow.agx.cube",
                "Glow.cube",
                "Mono.cube",
                "Neon Film.cube",
                "Neon.agx.cube",
                "Neon.cube"
            ]
        );
        std::fs::remove_file(store.join("Glow.cube")).unwrap();
        let failed = unlink_old(&pairs);
        assert_eq!(failed.len(), 1);
        assert!(store.join("Neon.cube").exists());
        assert!(!store.join("Neon.agx.cube").exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A rename that stops between its stages leaves no edit naming a
    /// look that is not there: after the first stage, or partway into
    /// it, every edit names the old name, whose files are all there;
    /// partway through the rewrite each names one or the other, and
    /// both are there under every curve.
    #[test]
    fn a_rename_stopped_midway_leaves_nothing_missing() {
        let dir = scratch("midway");
        let store = store_of(&dir);
        let frames: Vec<PathBuf> = (0..4).map(|i| dir.join(format!("f{i}.tif"))).collect();
        for f in &frames {
            std::fs::write(f, b"x").unwrap();
            let mut sidecar = Sidecar::default();
            sidecar.record(Edit {
                look_lut: named("Neon"),
                ..Edit::default()
            });
            sidecar.save(f).unwrap();
        }
        let every_look_is_there = || {
            for f in &frames {
                let s = Sidecar::load(f).unwrap().unwrap();
                assert!(s.current.look_lut.is_there_in(&store), "{}", f.display());
                for curve in DisplayCurve::ALL {
                    assert!(
                        s.current.look_lut.look_under_in(&store, curve).is_some(),
                        "{} under {curve:?}",
                        f.display()
                    );
                }
            }
        };
        let pairs = pairs(&neon_files(&store), "Neon", "Glow");
        // Stopped partway into the first stage: one new name made.
        link_one(&pairs[0].0, &pairs[0].1).unwrap();
        every_look_is_there();
        std::fs::remove_file(&pairs[0].1).unwrap();
        // Stopped after it, and then halfway through the rewrite.
        link_all(&pairs).unwrap();
        every_look_is_there();
        for f in &frames[..2] {
            let mut s = Sidecar::load(f).unwrap().unwrap();
            assert!(s.rename_look("Neon", "Glow"));
            s.save(f).unwrap();
        }
        every_look_is_there();
        // The rest, then the last stage: every edit names Glow, and Neon
        // is gone.
        for f in &frames[2..] {
            let mut s = Sidecar::load(f).unwrap().unwrap();
            s.rename_look("Neon", "Glow");
            s.save(f).unwrap();
        }
        assert!(unlink_old(&pairs).is_empty());
        every_look_is_there();
        assert!(!named("Neon").is_there_in(&store));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A state over real frames in `dir`, sidecars on, the store the
    /// rename's, deletes (and so renames) allowed.
    fn renaming(
        app: &App,
        dir: &Path,
        store: &Path,
        frames: usize,
    ) -> (Rc<RefCell<State>>, Rc<crate::worker::Worker>, Vec<PathBuf>) {
        STORE.with(|s| *s.borrow_mut() = Some(store.to_path_buf()));
        let files: Vec<PathBuf> = (0..frames).map(|i| dir.join(format!("f{i}.tif"))).collect();
        for f in &files {
            std::fs::write(f, b"x").unwrap();
        }
        let (state, worker) = state_for(app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.deletes_allowed = true;
            st.looks = list_in(Some(store.to_path_buf()));
        }
        (state, worker, files)
    }

    /// The window, from the button to the files: the sheet names the
    /// look and its pictures, waits for the edits it is reading, takes
    /// a new name on Return, and the look's two tables, the open
    /// picture, a snapshot, a picture whose sidecar was only on disk,
    /// a preset, the clipboard and the export queue all name it after;
    /// a look whose name starts with the old one's, another look's
    /// preset and pictures naming another look are left as they were,
    /// and nothing shows "(missing)".
    #[test]
    fn the_sheet_renames_a_look_and_everything_of_the_folder_that_names_it() {
        let dir = scratch("window");
        let store = store_of(&dir);
        let presets = dir.join("presets");
        let queue_file = dir.join("export-queue.json");
        let app = window(5);
        let (state, _worker, files) = renaming(&app, &dir, &store, 5);
        {
            let mut st = state.borrow_mut();
            // Frame 0 is open on Neon; frame 1 keeps a snapshot on Neon
            // and has moved to Mono; frame 2 is on Neon Film; frame 3's
            // sidecar is on disk only, Neon in its history; frame 4 has
            // no look.
            st.current = Some(0);
            st.edit.look_lut = named("Neon");
            let open = st.edit.clone();
            st.sidecars[0].record(open);
            st.sidecars[0].save(&files[0]).unwrap();
            let mut e = Edit {
                look_lut: named("Neon"),
                ..Edit::default()
            };
            st.sidecars[1].record(e.clone());
            st.sidecars[1].take_snapshot("Night", 1_700_000_000);
            e.look_lut = named("Mono");
            st.sidecars[1].record(e.clone());
            st.sidecars[1].save(&files[1]).unwrap();
            // Frame 2 once had Ghost, a look no longer in the folder.
            e.look_lut = named("Ghost");
            st.sidecars[2].record(e.clone());
            e.look_lut = named("Neon Film");
            st.sidecars[2].record(e.clone());
            st.sidecars[2].save(&files[2]).unwrap();
            // Frame 3's sidecar is in the hidden folder, though sidecars
            // go beside the frames.
            let mut on_disk = Sidecar::default();
            e.look_lut = named("Neon");
            on_disk.record(e.clone());
            e.look_lut = named("Mono");
            on_disk.record(e.clone());
            on_disk
                .save_in(&files[3], greycard_edit::Placement::Folder)
                .unwrap();
            assert_eq!(st.placement, greycard_edit::Placement::Beside);
            st.from_row[3].read = false;
            st.from_row[4].read = false;
            let store_p = greycard_edit::preset::Store::at(presets.clone());
            let mut look = Edit {
                look_lut: named("Neon"),
                ..Edit::default()
            };
            store_p
                .save(&Preset::from_edit("Neon Warm", &look, &[Section::Look]))
                .unwrap();
            look.look_lut = named("Mono");
            store_p
                .save(&Preset::from_edit("Mono Cold", &look, &[Section::Look]))
                .unwrap();
            look.look_lut = named("Ghost");
            store_p
                .save(&Preset::from_edit("Ghost Warm", &look, &[Section::Look]))
                .unwrap();
            st.preset_store = Some(store_p);
            crate::panel::assets::refresh_presets(&mut st, &app);
            st.clipboard = Some(crate::clipboard::Clipboard::copy(
                st.edit.clone(),
                &files[0],
            ));
            st.export_queue_file = Some(queue_file.clone());
            st.export_queue = vec![crate::export_queue::Entry {
                queued: 0,
                folder: None,
                preset: None,
                sheet: Default::default(),
                frames: vec![crate::export_queue::QueuedFrame {
                    source: files[0].clone(),
                    edit: st.edit.clone(),
                    turn: 0,
                    seed_blend: false,
                }],
            }];
            show_looks(&st, &st.edit.look_lut, &app);
        }
        let mono_preset = std::fs::read(presets.join("mono-cold.gcp")).unwrap();
        let before_2 = std::fs::read(Sidecar::path_for(&files[2])).unwrap();
        assert!(app.get_look_renamable());
        // The section's body grows to its rows over a moment.
        for _ in 0..4 {
            crate::testing::count_labeled(&app, "x");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
            slint::platform::update_timers_and_animations();
        }
        let (at, size) = crate::testing::labeled(&app, "Rename…");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        slint::platform::update_timers_and_animations();
        assert!(app.get_look_rename_open());
        assert_eq!(app.get_look_rename_title(), "Rename the look Neon");
        assert_eq!(app.get_look_rename_name(), "Neon");
        assert!(!app.get_look_rename_ready());
        let uses = app.get_look_rename_uses();
        assert!(
            uses.starts_with("2 pictures in this folder name it;")
                && uses.contains("Reading the edits of 2 more pictures")
                && uses.contains("1 preset names it"),
            "{uses}"
        );
        assert!(
            app.get_look_rename_note()
                .starts_with("Edits in other folders that name Neon will show it as missing")
        );

        // A name taken is refused and Return does nothing.
        type_text(&app, "Mono");
        assert_eq!(app.get_look_rename_name(), "Mono");
        assert_eq!(
            app.get_look_rename_refusal(),
            "There is a look called \"Mono\" already."
        );
        press(&app, Key::Return);
        assert!(app.get_look_rename_open());

        // The edits read from disk land; frame 4's sidecar was never
        // written, so only frame 3 names it, and Ghost besides.
        let generation = state.borrow().look_rename_generation;
        let unread: Vec<PathBuf> = vec![files[3].clone(), files[4].clone()];
        let found = looks_on_disk(&unread);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, files[3]);
        counted(&mut state.borrow_mut(), &app, generation, found);
        assert!(
            app.get_look_rename_uses()
                .starts_with("3 pictures in this folder name it;")
        );
        for _ in 0.."Mono".len() {
            press(&app, Key::Backspace);
        }
        // A name edits and presets already have for a look that is not
        // there: said, and not refused.
        type_text(&app, "Ghost");
        assert_eq!(app.get_look_rename_refusal(), "");
        assert_eq!(
            app.get_look_rename_warning(),
            "1 picture in this folder and 1 preset already name \"Ghost\", a look that is \
             not in the folder, and will use this look."
        );
        assert!(app.get_look_rename_ready());
        for _ in 0.."Ghost".len() {
            press(&app, Key::Backspace);
        }
        type_text(&app, "Glow");
        assert_eq!(app.get_look_rename_warning(), "");
        assert_eq!(app.get_look_rename_refusal(), "");
        assert!(app.get_look_rename_ready());
        let lines: Vec<String> = app
            .get_look_rename_files()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            lines,
            ["Neon.cube → Glow.cube", "Neon.agx.cube → Glow.agx.cube"]
        );
        // Frame 1's sidecar is turned elsewhere after the window read it.
        {
            let mut outside = Sidecar::load(&files[1]).unwrap().unwrap();
            outside.turn = 1;
            outside.save(&files[1]).unwrap();
        }
        press(&app, Key::Return);
        // The sidecars go to their thread; the sheet stays, working, and
        // takes no answer meanwhile.
        assert!(app.get_look_rename_open());
        assert!(app.get_look_rename_working());
        press(&app, Key::Escape);
        assert!(app.get_look_rename_open());
        // The open picture's is written already.
        assert_eq!(
            Sidecar::load(&files[0]).unwrap().unwrap().current.look_lut,
            named("Glow")
        );
        run_sent(&state, &app);
        assert!(!app.get_look_rename_open());
        assert!(
            app.get_status()
                .starts_with("renamed Neon to Glow: 2 files, 3 pictures, 1 preset"),
            "{}",
            app.get_status()
        );

        // The folder.
        assert_eq!(
            there(&store),
            ["Glow.agx.cube", "Glow.cube", "Mono.cube", "Neon Film.cube"]
        );
        // The sidecars on disk.
        let read = |i: usize| Sidecar::load(&files[i]).unwrap().unwrap();
        assert_eq!(read(0).current.look_lut, named("Glow"));
        assert_eq!(read(1).current.look_lut, named("Mono"));
        assert_eq!(read(1).snapshots[0].edit.look_lut, named("Glow"));
        assert_eq!(read(2).current.look_lut, named("Neon Film"));
        assert_eq!(read(3).current.look_lut, named("Mono"));
        assert_eq!(read(3).history[1].edit.look_lut, named("Glow"));
        assert!(!(0..4).any(|i| read(i).names_look("Neon")));
        // The turn made elsewhere is kept, on disk and in the window: the
        // file was renamed, not the window's copy written over it.
        assert_eq!(read(1).turn, 1);
        assert_eq!(state.borrow().sidecars[1].turn, 1);
        // Frame 3's sidecar was in the hidden folder and stays there.
        let found = Sidecar::find(&files[3]).unwrap();
        assert_eq!(greycard_edit::Placement::at(&found), Placement::Folder);
        assert!(!Sidecar::path_for(&files[3]).exists());
        // Frame 2 names another look and was not written.
        assert_eq!(
            std::fs::read(Sidecar::path_for(&files[2])).unwrap(),
            before_2
        );
        // The presets, the clipboard, the queue, the panel.
        let st = state.borrow();
        let warm = Preset::load(&presets.join("neon-warm.gcp")).unwrap();
        assert_eq!(warm.edit.look_lut, named("Glow"));
        assert_eq!(
            std::fs::read(presets.join("mono-cold.gcp")).unwrap(),
            mono_preset
        );
        assert_eq!(st.clipboard.as_ref().unwrap().edit.look_lut, named("Glow"));
        let queued = crate::export_queue::load(&queue_file);
        assert_eq!(queued[0].frames[0].edit.look_lut, named("Glow"));
        assert_eq!(st.edit.look_lut, named("Glow"));
        assert_eq!(app.get_look_name(), "Glow");
        let labels: Vec<String> = app
            .get_look_rows()
            .iter()
            .map(|r| r.label.to_string())
            .collect();
        assert!(labels.iter().any(|l| l == "Glow"), "{labels:?}");
        assert!(!labels.iter().any(|l| l.contains("missing")), "{labels:?}");
        // Outside the history: the open frame's states are as many as
        // before, and none of them names Neon.
        assert_eq!(st.sidecars[0].history.len(), 1);
        assert!(!st.sidecars[0].names_look("Neon"));
        drop(st);
        STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A picture whose sidecar cannot be written keeps the old name, so
    /// the old files stay beside the new and the status line says why.
    #[test]
    fn a_sidecar_that_cannot_be_written_keeps_the_old_files() {
        let dir = scratch("stale");
        let store = store_of(&dir);
        let app = window(2);
        let (state, _worker, files) = renaming(&app, &dir, &store, 2);
        // Frame 1's sidecar cannot be written: a folder is in its place.
        std::fs::create_dir_all(Sidecar::path_for(&files[1])).unwrap();
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.edit.look_lut = named("Neon");
            let open = st.edit.clone();
            st.sidecars[0].record(open.clone());
            st.sidecars[1].record(open);
            show_looks(&st, &st.edit.look_lut, &app);
        }
        ask(&mut state.borrow_mut(), &app);
        assert!(app.get_look_rename_open());
        app.set_look_rename_name("Glow".into());
        app.invoke_look_rename_edited();
        assert!(app.get_look_rename_ready());
        app.invoke_look_rename_answered(true);
        run_sent(&state, &app);
        let status = app.get_status().to_string();
        assert!(
            status.contains("1 picture's edit could not be read or written and still name Neon"),
            "{status}"
        );
        assert_eq!(
            there(&store),
            [
                "Glow.agx.cube",
                "Glow.cube",
                "Mono.cube",
                "Neon Film.cube",
                "Neon.agx.cube",
                "Neon.cube"
            ]
        );
        assert_eq!(
            Sidecar::load(&files[0]).unwrap().unwrap().current.look_lut,
            named("Glow")
        );
        STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A picture the window follows on an archive's copy has its edit
    /// written there later, so until then it names the old name: the
    /// old files stay, and the status line says why.
    #[test]
    fn a_picture_followed_to_an_archive_keeps_the_old_files() {
        let dir = scratch("followed");
        let store = store_of(&dir);
        let app = window(2);
        let (state, _worker, files) = renaming(&app, &dir, &store, 2);
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            let neon = Edit {
                look_lut: named("Neon"),
                ..Edit::default()
            };
            st.sidecars[1].record(neon);
            crate::sync::follow_for_test(
                &mut st,
                files[1].clone(),
                dir.join("local").join("f1.tif"),
                dir.clone(),
            );
            show_looks(&st, &st.edit.look_lut, &app);
        }
        app.set_look_name("Neon".into());
        ask(&mut state.borrow_mut(), &app);
        app.set_look_rename_name("Glow".into());
        app.invoke_look_rename_edited();
        app.invoke_look_rename_answered(true);
        run_sent(&state, &app);
        let status = app.get_status().to_string();
        assert!(
            status.contains("1 picture's edit goes to an archive's copy"),
            "{status}"
        );
        assert!(store.join("Neon.cube").exists() && store.join("Glow.cube").exists());
        assert!(!state.borrow().sidecars[1].names_look("Neon"));
        STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// No sheet, or no rename, where one would be wrong: None, a look
    /// that is a link, a removal under way, a capture run; and Escape
    /// changes nothing and gives the window its keys back.
    #[test]
    fn a_rename_is_refused_where_it_cannot_be_made() {
        let dir = scratch("refusals");
        let store = store_of(&dir);
        let app = window(2);
        let (state, _worker, _files) = renaming(&app, &dir, &store, 2);
        let before = there(&store);
        show_looks(&state.borrow(), &Default::default(), &app);
        assert!(!app.get_look_renamable());
        ask(&mut state.borrow_mut(), &app);
        assert!(!app.get_look_rename_open());
        assert!(app.get_status().contains("no look is chosen"));

        app.set_look_name("Neon".into());
        state.borrow_mut().look_removing = true;
        ask(&mut state.borrow_mut(), &app);
        assert!(!app.get_look_rename_open());
        assert!(app.get_status().contains("still being removed"));
        state.borrow_mut().look_removing = false;

        // Escape: no, and the window has its keys.
        ask(&mut state.borrow_mut(), &app);
        assert!(app.get_look_rename_open());
        type_text(&app, "Glow");
        press(&app, Key::Escape);
        assert!(!app.get_look_rename_open());
        assert_eq!(there(&store), before);
        let toggles = Rc::new(RefCell::new(0));
        let seen = toggles.clone();
        app.on_cull_toggled(move || *seen.borrow_mut() += 1);
        press(&app, "c");
        assert_eq!(*toggles.borrow(), 1);

        // A capture never renames, whatever its keys answer.
        state.borrow_mut().deletes_allowed = false;
        ask(&mut state.borrow_mut(), &app);
        app.set_look_rename_name("Glow".into());
        app.invoke_look_rename_edited();
        app.invoke_look_rename_answered(true);
        assert!(app.get_status().contains("never renames"));
        assert_eq!(there(&store), before);

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(store.join("Mono.cube"), store.join("Linky.cube")).unwrap();
            let looks = list_in(Some(store.clone()));
            assert!(!renamable(&looks, "Linky"));
            assert!(renamable(&looks, "Neon"));
            app.set_look_name("Linky".into());
            ask(&mut state.borrow_mut(), &app);
            assert!(!app.get_look_rename_open());
            assert!(
                app.get_status().contains("Linky.cube is a link"),
                "{}",
                app.get_status()
            );
        }
        // Pictures being deleted: no rename until that is done.
        app.set_look_name("Neon".into());
        state.borrow_mut().deleting = Some(Default::default());
        ask(&mut state.borrow_mut(), &app);
        assert!(!app.get_look_rename_open());
        assert!(app.get_status().contains("being deleted"));
        STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The sheet asked and answered with `new` over the look the panel
    /// has chosen; the job sent, not run.
    fn answer(state: &Rc<RefCell<State>>, app: &App, new: &str) {
        ask(&mut state.borrow_mut(), app);
        assert!(app.get_look_rename_open(), "{}", app.get_status());
        let generation = state.borrow().look_rename_generation;
        let unread: Vec<PathBuf> = {
            let st = state.borrow();
            (0..st.files.len())
                .filter(|&i| !crate::rows::is_loaded(&st, i))
                .map(|i| st.files[i].clone())
                .collect()
        };
        counted(
            &mut state.borrow_mut(),
            app,
            generation,
            looks_on_disk(&unread),
        );
        app.set_look_rename_name(new.into());
        app.invoke_look_rename_edited();
        assert!(
            app.get_look_rename_ready(),
            "{}",
            app.get_look_rename_refusal()
        );
        app.invoke_look_rename_answered(true);
        assert!(app.get_look_rename_working(), "{}", app.get_status());
    }

    /// Frames 0 and 1 with sidecars on disk naming Neon, frame 0 open
    /// on it, frame 1 loaded.
    fn two_on_neon(dir: &Path, store: &Path) -> (App, Rc<RefCell<State>>, Vec<PathBuf>) {
        let app = window(2);
        let (state, _worker, files) = renaming(&app, dir, store, 2);
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.edit.look_lut = named("Neon");
            for (i, f) in files.iter().enumerate() {
                let edit = st.edit.clone();
                st.sidecars[i].record(edit);
                st.sidecars[i].save(f).unwrap();
            }
            show_looks(&st, &st.edit.look_lut, &app);
        }
        (app, state, files)
    }

    /// A read that was out when the rename began, landing with the
    /// sidecar as it was before: during the thread, the open frame's
    /// (whose panel was its row's) takes the new name, and the thread's
    /// copy as it lands; after the thread, another frame's takes the
    /// copy the thread wrote. The rename waits for it: the sheet stays
    /// and the old files with it until it is in.
    #[test]
    fn a_read_out_when_the_rename_began_is_caught_up() {
        let dir = scratch("late");
        let store = store_of(&dir);
        let (app, state, files) = two_on_neon(&dir, &store);
        let before: Vec<Sidecar> = files
            .iter()
            .map(|f| Sidecar::load(f).unwrap().unwrap())
            .collect();
        // Both frames' reads are out; frame 0's panel stands in.
        {
            let mut st = state.borrow_mut();
            st.from_row[0].read = false;
            st.from_row[1].read = false;
            st.panel_stand_in = Some(files[0].clone());
            st.loads.in_flight.insert(70, vec![files[0].clone()]);
            st.loads.in_flight.insert(71, vec![files[1].clone()]);
        }
        answer(&state, &app, "Glow");
        let (generation, job) = sent();
        // Frame 0's read lands while the thread has the files.
        {
            let mut st = state.borrow_mut();
            assert!(crate::rows::land_read(&mut st, &app, 0, before[0].clone()));
            assert!(st.sidecars[0].names_look("Glow"));
            read_landed(&mut st, &app, 70);
        }
        job.run(&|| {});
        heard(&mut state.borrow_mut(), &app, generation, true);
        // Frame 1's read is still out: the rename waits, the old files
        // with it.
        assert!(app.get_look_rename_open());
        assert!(state.borrow().look_renaming.is_some());
        assert!(store.join("Neon.cube").exists());
        {
            let mut st = state.borrow_mut();
            assert!(crate::rows::land_read(&mut st, &app, 1, before[1].clone()));
            read_landed(&mut st, &app, 71);
        }
        assert!(!app.get_look_rename_open());
        assert!(state.borrow().look_renaming.is_none());
        assert!(app.get_status().starts_with("renamed Neon to Glow"));
        assert!(!store.join("Neon.cube").exists());
        let st = state.borrow();
        for (i, f) in files.iter().enumerate() {
            assert!(!st.sidecars[i].names_look("Neon"), "{i}");
            let disk = Sidecar::load(f).unwrap().unwrap();
            assert!(!disk.names_look("Neon"), "{i}");
            // The window holds what the file holds.
            assert_eq!(st.sidecars[i].revisions, disk.revisions, "{i}");
        }
        drop(st);
        STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A read that was not out when a rename began is never rewritten by
    /// it: after Neon is renamed Glow and a new look called Neon is made
    /// and used, a sidecar naming it reads in as Neon and is not
    /// written; and a rename and back writes nothing to a sidecar read
    /// in later.
    #[test]
    fn a_read_not_out_when_the_rename_began_is_left_alone() {
        for back in [false, true] {
            let dir = scratch(if back { "back" } else { "new-neon" });
            let store = store_of(&dir);
            let (app, state, files) = two_on_neon(&dir, &store);
            answer(&state, &app, "Glow");
            run_sent(&state, &app);
            if back {
                app.set_look_name("Glow".into());
                answer(&state, &app, "Neon");
                run_sent(&state, &app);
                assert!(store.join("Neon.cube").exists() && !store.join("Glow.cube").exists());
            } else {
                // A new Neon, used on frame 1 (a sidecar written by hand, as
                // another folder's would be).
                table(&store, "Neon.cube", &[]);
                let mut s = Sidecar::load(&files[1]).unwrap().unwrap();
                s.record(Edit {
                    look_lut: named("Neon"),
                    ..exposed(1.0)
                });
                s.save(&files[1]).unwrap();
            }
            let bytes = std::fs::read(Sidecar::path_for(&files[1])).unwrap();
            {
                let mut st = state.borrow_mut();
                st.from_row[1].read = false;
                let read = Sidecar::load(&files[1]).unwrap().unwrap();
                assert!(crate::rows::land_read(&mut st, &app, 1, read));
                assert!(st.sidecars[1].current.look_lut.names("Neon"));
            }
            assert_eq!(std::fs::read(Sidecar::path_for(&files[1])).unwrap(), bytes);
            STORE.with(|s| *s.borrow_mut() = None);
            crate::testing::remove_scratch(state, &dir);
        }
    }

    /// After a Stop the rename's thread may still be on a sidecar: it
    /// checks again before writing it, and a write that was past that
    /// check is taken when the thread says it is done, so the window and
    /// the file agree. Until then no other rename begins.
    #[test]
    fn a_late_write_after_a_stop_is_taken_and_holds_the_next_rename() {
        let dir = scratch("late-write");
        let store = store_of(&dir);
        let (app, state, files) = two_on_neon(&dir, &store);
        answer(&state, &app, "Glow");
        let (generation, _job) = sent();
        app.invoke_look_rename_answered(false);
        assert!(!app.get_look_rename_open());
        assert!(app.get_status().contains("could not be read or written"));
        // A sidecar not yet written when the stop came is not written.
        let frame = |st: &State| JobFrame {
            path: files[1].clone(),
            memory: Some(st.sidecars[1].clone()),
        };
        let stopped = one(
            frame(&state.borrow()),
            "Neon",
            "Glow",
            Placement::Beside,
            &AtomicBool::new(true),
        );
        assert!(matches!(stopped, Outcome::Failed(_)));
        assert!(
            Sidecar::load(&files[1])
                .unwrap()
                .unwrap()
                .names_look("Neon")
        );
        // The thread is out: no other rename begins.
        app.set_look_name("Neon".into());
        ask(&mut state.borrow_mut(), &app);
        assert!(app.get_status().contains("still being renamed"));
        // One that was past the check is written, and taken when the
        // thread is done.
        let (sent_hash, late) = {
            let st = state.borrow();
            let f = frame(&st);
            let hash = f
                .memory
                .as_ref()
                .map(|m| m.revisions.last().map(|r| r.hash.clone()));
            (
                hash,
                one(
                    f,
                    "Neon",
                    "Glow",
                    Placement::Beside,
                    &AtomicBool::new(false),
                ),
            )
        };
        assert!(matches!(late, Outcome::Written(_)));
        {
            let st = state.borrow();
            let r = st.look_renaming.as_ref().unwrap();
            r.reached
                .lock()
                .unwrap()
                .push((files[1].clone(), sent_hash, late));
        }
        heard(&mut state.borrow_mut(), &app, generation, true);
        let st = state.borrow();
        assert!(st.look_renaming.is_none());
        assert!(!st.sidecars[1].names_look("Neon"));
        assert_eq!(
            st.sidecars[1].revisions,
            Sidecar::load(&files[1]).unwrap().unwrap().revisions
        );
        // The old files stay: the outcome said was that it was stopped.
        assert!(store.join("Neon.cube").exists());
        drop(st);
        STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A sync that lands on frame 1 while the thread has its file (the
    /// archive's copy newer, Behind(Local); or the two joined,
    /// Diverged) leaves the window a copy other than the one sent: that
    /// copy is the one kept, renamed and saved, so the other machine's
    /// states are not lost to the thread's.
    #[test]
    fn a_copy_moved_by_a_sync_during_the_rename_is_kept_and_renamed() {
        for diverged in [false, true] {
            let dir = scratch(if diverged { "diverged" } else { "behind" });
            let store = store_of(&dir);
            let (app, state, files) = two_on_neon(&dir, &store);
            answer(&state, &app, "Glow");
            let (generation, job) = sent();
            // What the sync brings: a state from the other machine.
            {
                let mut st = state.borrow_mut();
                let mut theirs = st.sidecars[1].clone();
                theirs.record(Edit {
                    look_lut: named("Neon"),
                    ..exposed(2.0)
                });
                theirs.advance(&greycard_edit::sync::Stamp {
                    at: 4_000_000_000,
                    host: "laptop".into(),
                });
                st.sidecars[1] = if diverged {
                    let mut ours = st.sidecars[1].clone();
                    ours.record(Edit {
                        look_lut: named("Neon"),
                        ..exposed(-1.0)
                    });
                    ours.advance(&greycard_edit::sync::Stamp {
                        at: 3_000_000_000,
                        host: "desk".into(),
                    });
                    greycard_edit::sync::join(&ours, &theirs)
                } else {
                    theirs
                };
            }
            job.run(&|| {});
            heard(&mut state.borrow_mut(), &app, generation, true);
            let status = app.get_status().to_string();
            assert!(status.starts_with("renamed Neon to Glow"), "{status}");
            let st = state.borrow();
            let disk = Sidecar::load(&files[1]).unwrap().unwrap();
            for s in [&st.sidecars[1], &disk] {
                assert!(!s.names_look("Neon"), "diverged {diverged}");
                let exposures: Vec<f32> = s
                    .history
                    .iter()
                    .map(|h| h.edit.light.exposure)
                    .chain(std::iter::once(s.current.light.exposure))
                    .collect();
                assert!(exposures.contains(&2.0), "{exposures:?}");
                if diverged {
                    assert!(exposures.contains(&-1.0), "{exposures:?}");
                }
            }
            drop(st);
            STORE.with(|s| *s.borrow_mut() = None);
            crate::testing::remove_scratch(state, &dir);
        }
    }

    fn exposed(stops: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = stops;
        e
    }

    /// A thread that went quiet, or a Stop, lands what was done: the
    /// sidecars reached take the new name, the rest are counted, and
    /// the old files stay.
    #[test]
    fn a_stall_or_a_stop_lands_what_was_done_and_keeps_the_old_files() {
        for stop_pressed in [false, true] {
            let dir = scratch(if stop_pressed { "stop" } else { "stall" });
            let store = store_of(&dir);
            let app = window(4);
            let (state, _worker, files) = renaming(&app, &dir, &store, 4);
            {
                let mut st = state.borrow_mut();
                st.current = Some(0);
                st.edit.look_lut = named("Neon");
                for (i, f) in files.iter().enumerate() {
                    let edit = st.edit.clone();
                    st.sidecars[i].record(edit);
                    st.sidecars[i].save(f).unwrap();
                }
                show_looks(&st, &st.edit.look_lut, &app);
            }
            answer(&state, &app, "Glow");
            let (generation, job) = sent();
            if stop_pressed {
                // Stop, before the thread has done anything.
                app.invoke_look_rename_answered(false);
                assert!(!app.get_look_rename_open());
                job.run(&|| {});
            } else {
                // The thread does one sidecar and goes quiet on the next:
                // the watch lands it.
                let stop = state.borrow().look_renaming.as_ref().unwrap().stop.clone();
                job.run(&|| stop.store(true, Ordering::Relaxed));
                heard(&mut state.borrow_mut(), &app, generation, false);
            }
            // Over only when the thread says it is done.
            assert!(state.borrow().look_renaming.is_some());
            heard(&mut state.borrow_mut(), &app, generation, true);
            assert!(state.borrow().look_renaming.is_none());
            let status = app.get_status().to_string();
            let left = if stop_pressed { 3 } else { 2 };
            assert!(
                status.contains(&format!(
                    "{left} pictures' edits could not be read or written and still name Neon"
                )),
                "{status}"
            );
            assert!(store.join("Neon.cube").exists() && store.join("Glow.cube").exists());
            let renamed = (1..4)
                .filter(|&i| {
                    !Sidecar::load(&files[i])
                        .unwrap()
                        .unwrap()
                        .names_look("Neon")
                })
                .count();
            assert_eq!(renamed, if stop_pressed { 0 } else { 1 });
            STORE.with(|s| *s.borrow_mut() = None);
            crate::testing::remove_scratch(state, &dir);
        }
    }

    /// Sidecars the rename cannot reach keep the old files: a frame
    /// under a root that is offline, a frame followed to an archive and
    /// not read yet, and every frame when sidecars are off.
    #[test]
    fn sidecars_out_of_reach_keep_the_old_files() {
        for case in ["offline", "followed", "off"] {
            let dir = scratch(case);
            let store = store_of(&dir);
            let (app, state, files) = two_on_neon(&dir, &store);
            {
                let mut st = state.borrow_mut();
                match case {
                    "offline" => {
                        st.from_row[1].read = false;
                        st.library.offline.insert(dir.clone());
                    }
                    "followed" => {
                        st.from_row[1].read = false;
                        crate::sync::follow_for_test(
                            &mut st,
                            files[1].clone(),
                            dir.join("local").join("f1.tif"),
                            dir.clone(),
                        );
                    }
                    _ => st.write_sidecars = false,
                }
            }
            answer(&state, &app, "Glow");
            run_sent(&state, &app);
            let status = app.get_status().to_string();
            assert!(status.contains("could not be reached"), "{case}: {status}");
            assert!(
                store.join("Neon.cube").exists() && store.join("Glow.cube").exists(),
                "{case}"
            );
            STORE.with(|s| *s.borrow_mut() = None);
            crate::testing::remove_scratch(state, &dir);
        }
    }

    /// The landing on the window's thread at ten thousand frames: one
    /// pass over the list, not one a sidecar. Timed, and held to well
    /// under the second a scan per sidecar took.
    #[test]
    fn landing_ten_thousand_sidecars_is_one_pass() {
        let dir = scratch("ten-thousand");
        let store = store_of(&dir);
        STORE.with(|s| *s.borrow_mut() = Some(store.clone()));
        let app = window(0);
        let files: Vec<PathBuf> = (0..10_000)
            .map(|i| PathBuf::from("/nowhere").join(format!("IMG_{i:05}.CR3")))
            .collect();
        let (state, _worker) = state_for(&app, files.clone());
        let mut written = Sidecar::default();
        written.record(Edit {
            look_lut: named("Glow"),
            ..Edit::default()
        });
        let reached: Reached = files
            .iter()
            .map(|f| (f.clone(), Some(None), Outcome::Written(written.clone())))
            .collect();
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.look_rename_generation = 7;
            st.look_renaming = Some(Renaming {
                generation: 7,
                old: "Neon".into(),
                new: "Glow".into(),
                pairs: Vec::new(),
                frames: files.len(),
                done: Rewritten::default(),
                started: std::time::Instant::now(),
                reached: Arc::new(Mutex::new(reached)),
                stop: Arc::new(AtomicBool::new(false)),
                taken: 0,
                landed: false,
                thread_out: true,
                catch: HashMap::new(),
                catch_paths: BTreeSet::new(),
                written: HashMap::new(),
                gave_up: false,
                reported: false,
            });
        }
        let clock = std::time::Instant::now();
        heard(&mut state.borrow_mut(), &app, 7, true);
        let took = clock.elapsed();
        eprintln!("landing 10,000 sidecars: {:.3} s", took.as_secs_f64());
        assert!(
            app.get_status()
                .starts_with("renamed Neon to Glow: 0 files, 10000 pictures"),
            "{}",
            app.get_status()
        );
        assert!(state.borrow().sidecars[9_999].names_look("Glow"));
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
        STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_scratch(state, &dir);
    }
}
