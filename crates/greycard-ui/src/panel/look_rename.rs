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
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use greycard_edit::look::{self, Entry};
use greycard_edit::{Placement, Sidecar};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::panel::assets::{refresh_presets, show_looks};
use crate::panel::browser::show_badges_of_file;
use crate::panel::history::show_history;
use crate::panel::look_remove::{files_of, list_in, store};
use crate::{App, Shared, State, tr};

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
    /// Where the pictures are, as the sheet's lines say it.
    place: String,
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
        return Some(tr!(
            "Only the case differs from its name now, and Windows and macOS take the two for \
             one name. Rename it to another name first, then to this one."
        ));
    }
    if folder.looks.contains(&lower) {
        return Some(tr!("There is a look called \"{new}\" already.", new = new));
    }
    for (_, to) in pairs(files, old, new) {
        let name = to
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if folder.files.contains(&name.to_lowercase()) {
            return Some(tr!(
                "\"{name}\" is in the look folder already.",
                name = name
            ));
        }
    }
    None
}

/// What would stop a rename now, in the status line's words: anything
/// else that writes looks or sidecars, or moves the files they sit
/// beside.
fn busy(st: &State) -> Option<String> {
    if st.sheets.look_renaming.is_some() {
        Some(tr!("a look is still being renamed"))
    } else if st.sheets.deleting.is_some() {
        Some(tr!(
            "pictures are being deleted; rename the look when that is done"
        ))
    } else if st.sheets.look_removing {
        Some(tr!("a look is still being removed"))
    } else if st.camera_match.running() {
        Some(tr!(
            "the camera match is running; rename the look when it is done"
        ))
    } else if st.work.exporting.is_some() || st.work.queue_run.is_some() {
        Some(tr!("an export is running; rename the look when it is done"))
    } else if st.archive.copying() {
        Some(tr!(
            "a copy to or from an archive is running; rename the look when it is done"
        ))
    } else if st.import.job.is_some() {
        Some(tr!("an import is running; rename the look when it is done"))
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
        if !crate::rows::file_is_loaded(st, i) {
            unread.push(file.clone());
            continue;
        }
        let mut looks = st
            .sidecars
            .get(i)
            .map(Sidecar::looks_named)
            .unwrap_or_default();
        if Some(i) == st.current && !st.develop.edit.look_lut.lut.is_none() {
            looks.insert(st.develop.edit.look_lut.lut.name().to_string());
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
        .filter_map(|p| match Sidecar::load_as_file(p) {
            Ok(Some(s)) => Some((p.clone(), s.looks_named())),
            _ => None,
        })
        .collect()
}

fn pictures(n: usize) -> String {
    tr!("{n} picture" | "{n} pictures" % n)
}

fn presets_of(n: usize) -> String {
    tr!("{n} preset" | "{n} presets" % n)
}

/// The sheet's line on what names the look and what it becomes.
pub(crate) fn uses_line(named: usize, pending: usize, presets: usize, place: &str) -> String {
    let mut out = match (named, pending) {
        (0, 0) => tr!("No picture {place} names it.", place = place),
        (0, _) => String::new(),
        (n, _) => tr!(
            "{n} picture {place} names it; its edit, history and snapshots will name the new \
             one."
                | "{n} pictures {place} name it; their edits, history and snapshots will name the \
                   new one."
                    % n,
            place = place
        ),
    };
    let mut add = |s: String| {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&s);
    };
    if pending > 0 {
        add(tr!(
            "Reading the edits of {n} more picture to count them..."
                | "Reading the edits of {n} more pictures to count them..." % pending
        ));
    }
    if presets > 0 {
        add(tr!("{n} preset names it and will name the new one."
            | "{n} presets name it and will name the new one."
                % presets));
    }
    out
}

/// The sheet's warning when the new name is one that edits or presets
/// already name, for a look that is not in the folder: they show it as
/// missing now, and will take this look once it has the name.
pub(crate) fn taken_line(new: &str, pictures: usize, presets: usize, place: &str) -> String {
    let who = match (pictures, presets) {
        (0, 0) => return String::new(),
        (p, 0) => tr!("{} {place}", self::pictures(p), place = place),
        (0, q) => presets_of(q),
        (p, q) => tr!(
            "{} {place} and {}",
            self::pictures(p),
            presets_of(q),
            place = place
        ),
    };
    tr!(
        "{who} already names \"{new}\", a look that is not in the folder, and will use this \
         look."
            | "{who} already name \"{new}\", a look that is not in the folder, and will use \
               this look."
                % pictures
                + presets,
        who = who,
        new = new
    )
}

/// The sheet's note: what the rename does not reach, and what the list
/// will show the look by when that is its title rather than its name.
fn note(old: &str, new: &str, own: Option<&Entry>) -> String {
    let mut out = tr!(
        "Edits in other folders that name {old} will show it as missing; renaming it back \
         to {old} brings them back.",
        old = old
    );
    if let Some(own) = own {
        let after = Entry {
            name: new.to_string(),
            ..own.clone()
        };
        let label = after.label();
        if label != new {
            out.push_str(&tr!(
                " The list shows it by the title inside its file, \"{label}\", which stays.",
                label = label
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
        app.set_status(tr!("no look is chosen to rename").into());
        return;
    }
    let Some(dir) = store() else {
        app.set_status(tr!("there is no look directory on this machine").into());
        return;
    };
    // The directory as it is now, as removal reads it.
    st.develop.looks = list_in(Some(dir.clone()));
    let files = files_of(&st.develop.looks, &name, &dir);
    if files.is_empty() {
        app.set_status(
            tr!(
                "{name} has no file in the look directory to rename",
                name = name
            )
            .into(),
        );
        return;
    }
    if let Some(link) = files.iter().find(|f| is_link(f)) {
        app.set_status(
            tr!(
                "{} is a link; rename the look in its folder by hand",
                link.file_name().unwrap_or_default().to_string_lossy()
            )
            .into(),
        );
        return;
    }
    let tables = look::tables_of(&st.develop.looks, &name);
    let own = tables
        .iter()
        .find(|e| e.variant.is_none())
        .map(|e| (*e).clone());
    let shown = tables
        .first()
        .map(|e| e.label().to_string())
        .unwrap_or_else(|| name.clone());
    let place = match st.views.view {
        crate::roots::View::Roots(_) => tr!("in the library"),
        _ => tr!("in this folder"),
    };
    let (in_memory, unread) = uses(st);
    let presets = st
        .develop
        .presets
        .iter()
        .filter(|e| !e.preset.edit.look_lut.lut.is_none())
        .map(|e| e.preset.edit.look_lut.lut.name().to_string())
        .collect();
    st.sheets.look_rename_generation += 1;
    let generation = st.sheets.look_rename_generation;
    let asked = Asked {
        name: name.clone(),
        folder: Folder::read(&dir, &st.develop.looks),
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
    app.set_look_rename_title(tr!("Rename the look {shown}", shown = shown).into());
    app.set_look_rename_name(name.as_str().into());
    app.set_look_rename_working(false);
    st.sheets.look_rename = Some(asked);
    show_sheet(st, app);
    app.set_look_rename_open(true);
    if !unread.is_empty() {
        count_unread(app, generation, unread);
    }
}

/// The sheet's lines for the name in its field.
fn show_sheet(st: &State, app: &App) {
    let Some(asked) = &st.sheets.look_rename else {
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
    app.set_look_rename_uses(uses_line(named, pending, presets, &asked.place).into());
    let taken = if fine {
        let (pictures, presets) = asked.naming(&new);
        taken_line(&new, pictures, presets, &asked.place)
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
    match &mut st.sheets.look_rename {
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
            return Err(tr!(
                "{} could not be made ({e})",
                to.file_name().unwrap_or_default().to_string_lossy(),
                e = e
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
            failed.push((from.clone(), tr!("its new copy is not whole")));
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
    // The window's copy's start, when it holds one, for a sidecar it
    // adopts or takes back written (§724); otherwise the file's.
    let start = memory
        .as_ref()
        .map_or_else(|| Sidecar::start_of(&path), |m| m.start.clone());
    let (mut sidecar, place) = match Sidecar::find(&path) {
        Some(file) => match Sidecar::read(&file, &start) {
            // A newer build's (§724): never written.
            Ok(read) if read.read_only() => {
                return Outcome::Failed(
                    read.writable()
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_default(),
                );
            }
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
                if let Some(m) = memory.filter(|m| m.current() == read.current()) {
                    read.redo = m.line.redo;
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
    let mut reached = vec![tr!("{n} file" | "{n} files" % files)];
    if done.pictures > 0 {
        reached.push(pictures(done.pictures));
    }
    if done.presets > 0 {
        reached.push(presets_of(done.presets));
    }
    let mut out = tr!(
        "renamed {old} to {new}: {}",
        reached.join(", "),
        old = old,
        new = new
    );
    let stale = done.stale + done.presets_stale;
    let mut why = Vec::new();
    if stale > 0 {
        let edits = |n: usize| tr!("{n} picture's edit" | "{n} pictures' edits" % n);
        let what = match (done.stale, done.presets_stale) {
            (p, 0) => edits(p),
            (0, p) => presets_of(p),
            (p, q) => tr!("{} and {}", edits(p), presets_of(q)),
        };
        why.push(tr!(
            "{what} could not be read or written and still name {old}",
            what = what,
            old = old
        ));
    }
    if done.unreachable > 0 {
        why.push(tr!(
            "{} could not be reached (offline, on an archive and not read yet, or sidecars \
             off) and may still name {old}",
            tr!("{n} picture's sidecar" | "{n} pictures' sidecars" % done.unreachable),
            old = old
        ));
    }
    if done.archived > 0 {
        why.push(tr!(
            "{} to an archive's copy, which is written later",
            tr!("{n} picture's edit goes" | "{n} pictures' edits go" % done.archived)
        ));
    }
    if !why.is_empty() {
        out.push_str(&tr!(
            "; {}, so {old}'s files stay under that name too until you remove them",
            why.join("; "),
            old = old
        ));
    } else if let Some((file, why)) = unlinked.first() {
        out.push_str(&tr!(
            "; {} stays under the old name ({why})",
            file.file_name().unwrap_or_default().to_string_lossy(),
            why = why
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
    // The thread reads and rewrites sidecars on disk: a batch of them
    // still being saved lands first.
    crate::panel::saves::finish(st, None);
    let old = asked.name.as_str();
    let looks = list_in(Some(asked.store.clone()));
    let files = files_of(&looks, old, &asked.store);
    if files.is_empty() {
        return Err(tr!(
            "{old} has no file in the look directory now; nothing was renamed",
            old = old
        ));
    }
    if new == old {
        return Err(tr!("{old} is its name already", old = old));
    }
    let folder = Folder::read(&asked.store, &looks);
    if let Some(why) = refusal(&folder, old, &files, new) {
        return Err(tr!("{old} was not renamed: {why}", old = old, why = why));
    }
    if let Some(link) = files.iter().find(|f| is_link(f)) {
        return Err(tr!(
            "{old} was not renamed: {} is a link",
            link.file_name().unwrap_or_default().to_string_lossy(),
            old = old
        ));
    }
    let pairs = pairs(&files, old, new);
    if let Err(why) = link_all(&pairs) {
        return Err(tr!(
            "{old} was not renamed: {why}; nothing changed",
            old = old,
            why = why
        ));
    }
    // The reads out now carry what the files held before the thread
    // renames them: those, and only those, are caught up as they land.
    let catch = st.views.loads.in_flight.clone();
    let catch_paths = catch.values().flatten().cloned().collect();
    let mut done = Rewritten::default();
    // With sidecars off none was read, so any of them may name the old
    // name: the old files stay.
    if !st.window.write_sidecars {
        done.unreachable = st.files.len();
    }
    // The open picture: the panel's edit and the sidecar in memory,
    // saved as any save of it is.
    let open = st
        .current
        .filter(|&c| crate::rows::panel_is_frames(st, c))
        .map(|c| st.file_of(c));
    let open_edit = st.develop.edit.look_lut.rename(old, new);
    if let Some(f) = open {
        // The file's whole sidecar: a rename reaches every line of it;
        // none of a read-only one's (§724).
        let renamed = !st.sidecars[f].read_only() && st.sidecars[f].rename_look(old, new);
        if renamed || open_edit {
            done.pictures += 1;
        }
        if renamed && st.window.write_sidecars {
            save_window_copy(st, f, &mut done);
        }
    }
    // The rest: those that go to an archive's copy, and with sidecars
    // off every one, from the window's copy; those it cannot reach,
    // counted; every other sidecar from disk, on the thread.
    let mut frames = Vec::new();
    for i in 0..st.files.len() {
        if Some(i) == open || crate::panel::delete::held_file(st, i) {
            continue;
        }
        let loaded = crate::rows::file_is_loaded(st, i);
        // A newer build's sidecar is read-only (§724): not renamed, and
        // counted as one left naming the old look.
        if loaded && st.sidecars[i].read_only() {
            if st.sidecars[i].names_look(old) {
                done.stale += 1;
            }
            continue;
        }
        if !st.window.write_sidecars {
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
        if crate::rows::file_is_offline(st, i) {
            done.unreachable += 1;
            continue;
        }
        frames.push(JobFrame {
            path: st.files[i].clone(),
            memory: loaded.then(|| st.sidecars[i].clone()),
        });
    }
    rename_elsewhere(st, app, old, new, &mut done);
    st.sheets.look_rename_generation += 1;
    let generation = st.sheets.look_rename_generation;
    let reached = Arc::new(Mutex::new(Vec::with_capacity(frames.len())));
    let stop = Arc::new(AtomicBool::new(false));
    st.sheets.look_renaming = Some(Renaming {
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
        placement: st.window.placement,
        reached,
        stop,
    };
    app.set_status(tr!("renaming {old} to {new}...", old = old, new = new).into());
    send_off(app, generation, job);
    Ok(())
}

/// Write the window's copy of frame `i`'s sidecar after a rename: to
/// an archive's copy for a frame the window follows there, which lands
/// later, or here; one that could not be written is counted stale.
fn save_window_copy(st: &mut State, i: usize, done: &mut Rewritten) {
    if crate::sync::followed(st, i) && crate::sidecars::file_writable(st, i) {
        crate::sidecars::save_file(st, i);
        done.archived += 1;
    } else if !(crate::sidecars::file_writable(st, i) && crate::sidecars::save_file(st, i)) {
        done.pictures -= 1;
        done.stale += 1;
    }
}

/// The edits the window keeps outside the sidecars: the settings
/// clipboard, the hovered and held states, the command line's
/// override, the export queue, and the presets in the store.
fn rename_elsewhere(st: &mut State, app: &App, old: &str, new: &str, done: &mut Rewritten) {
    if let Some(c) = &mut st.sheets.clipboard {
        c.edit.look_lut.rename(old, new);
    }
    for edit in [st.develop.peek.as_mut(), st.develop.held.as_mut()]
        .into_iter()
        .flatten()
    {
        edit.look_lut.rename(old, new);
    }
    if let Some((_, edit)) = &mut st.launch.overridden {
        edit.look_lut.rename(old, new);
    }
    let mut queued = false;
    for entry in &mut st.work.export_queue {
        for frame in &mut entry.frames {
            queued |= frame.edit.look_lut.rename(old, new);
        }
    }
    if queued {
        crate::panel::export_queue::keep(st);
    }
    if let Some(store) = st.develop.preset_store.clone() {
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
    let Some(r) = st.sheets.look_renaming.as_mut() else {
        return false;
    };
    if !r.catch_paths.remove(&path) {
        return false;
    }
    let (old, new) = (r.old.clone(), r.new.clone());
    // The old files are kept until the reads are in, so a look of the
    // old name is the old look; one listed again after they went is a
    // new look of that name, and not this one.
    if r.reported
        && st
            .develop
            .looks
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(&old))
    {
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
    let Some(r) = st.sheets.look_renaming.as_mut() else {
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
    let Some(mut r) = st
        .sheets
        .look_renaming
        .take_if(|r| r.generation == generation)
    else {
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
    st.sheets.look_renaming = Some(r);
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
            .filter(|&i| crate::rows::file_is_loaded(st, i));
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
            crate::sidecars::reread(st, &path);
            if crate::rows::versions_differ(st, i) {
                crate::versions::settle_later(app);
            }
            show_badges_of_file(st, app, i);
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
            && !(crate::sidecars::file_writable(st, i) && crate::sidecars::save_file(st, i))
            && !late
        {
            if wrote {
                r.done.pictures -= 1;
            }
            r.done.stale += 1;
        }
        show_badges_of_file(st, app, i);
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
        .sheets
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
    let Some(r) = st.sheets.look_renaming.as_mut() else {
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
        greycard_edit_io::looks::forget();
        st.develop.looks = list_in(store());
        st.develop.look_for = None;
        show_looks(st, &st.develop.edit.look_lut, app);
        crate::edited::looks_moved(st);
        show_history(st, app);
        app.set_look_rename_working(false);
        app.set_look_rename_open(false);
        app.set_status(status.into());
        app.window().request_redraw();
    }
    if st
        .sheets
        .look_renaming
        .as_ref()
        .is_some_and(|r| !r.thread_out)
    {
        st.sheets.look_renaming = None;
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
    if let Some(r) = &st.sheets.look_renaming {
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
    let Some(asked) = st.sheets.look_rename.take() else {
        close(app);
        return;
    };
    if !yes {
        close(app);
        return;
    }
    let new = app.get_look_rename_name().trim().to_string();
    let refused = if !st.window.deletes_allowed {
        Some(tr!(
            "not renamed: a capture, an export or a timing run never renames"
        ))
    } else if let Some(why) = busy(st) {
        Some(why)
    } else if asked.on_disk.is_none() {
        Some(tr!("not renamed: the folder's edits were still being read"))
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
                st.sheets.look_renaming.as_ref().map_or(0, |r| r.frames)
            );
            app.set_look_rename_working(true);
            app.set_look_rename_ready(false);
            app.set_look_rename_uses(
                tr!("Renaming: the edits are being read and written...").into(),
            );
        }
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>) {
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
mod tests;
