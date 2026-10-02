//! The archive roots on the window (notes §197, §216): the mark on a
//! root's chip, the grid header's Back up or Bring back with its count,
//! the frame menu's, the sheet that says what a copy would do before it
//! does it, the card over the copy with its Cancel, and the Delete
//! sheet's line on whether the frames are on an archive.
//!
//! Everything that asks the disk (what is on the other side, the copy,
//! the counts) is `crate::archive`'s, run off the window's thread on a
//! thread of its own whose beats are watched: an archive that answered
//! its look and then stopped answering sets the job aside, and the
//! window goes on (`archive::supervise`, §215's pattern). What lands is
//! numbered, so a later ask overtakes an earlier one.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use greycard_library::{Change, Pairing, Roots};

use crate::archive::{self, Ask, Beat, Direction, Frames, Heard, Plan, Report};

pub(crate) mod rejects;
use crate::panel::browser::{chosen_frames, file_name};
use crate::roots::{ROOT_WAIT, View};
use crate::*;

/// The window's side of the archives.
#[derive(Default)]
pub(crate) struct Archive {
    /// What the sheet is over, while it is up.
    asked: Option<Asked>,
    /// The sheet's plan, once its look is in.
    plan: Option<Plan>,
    /// Numbers every look: one that lands under an older number is
    /// dropped.
    look_token: u64,
    /// A copy under way.
    running: Option<Running>,
    /// Numbers every copy, so one set aside and back late is told from
    /// the one under way.
    run_token: u64,
    /// The copies set aside and not back yet, by number, with the
    /// archive each was to or from: no other copy to or from that
    /// archive starts until it lands, as no pass goes where one set
    /// aside is (§215); it may still be writing into the folders a new
    /// one would. Another archive is not held by it.
    aside: Vec<(u64, PathBuf)>,
    /// Fills the card's bar from the copy's count.
    timer: slint::Timer,
    /// The header's counts: the list they were taken over (the view's
    /// generation), and for each archive the source is paired with, how
    /// many of the frames a Back up would walk are not on it (none when
    /// it did not answer).
    count: Option<Counts>,
    count_token: u64,
    /// Stops the count out when a newer one is asked for.
    count_stop: Option<Arc<AtomicBool>>,
    /// Numbers the Delete sheet's looks.
    delete_token: u64,
    /// Remove rejects from an archive: its sheet and its queue.
    pub(crate) rejects: rejects::Rejects,
}

/// The header's counts: the view's generation they were taken over,
/// and each paired archive with how many frames are not on it and how
/// many could not be told without reading them whole (none when it did
/// not answer).
type Counts = (u64, Vec<(PathBuf, Option<(usize, usize)>)>);

/// A sheet's question: the ask, and what it is said with.
#[derive(Debug, Clone)]
struct Asked {
    ask: Ask,
    /// The archive a backup goes to, or the local root a bring-back
    /// comes to: what the destination must be under.
    other: PathBuf,
    /// That root's name, as the sheet and the status line say it.
    label: String,
    /// For a backup, the source folder whose pairing is kept.
    remember: Option<PathBuf>,
    /// For a bring-back the pairing did not unwind: the sheet asks.
    unpaired: bool,
    /// The archive the copy goes to or comes from.
    archive: PathBuf,
    /// The local side's base (a root, or a folder of no root's): the
    /// pairings between it and `archive` are the ones the look checks.
    local: PathBuf,
    /// While the field is still the default: what the look settles it
    /// from, once it knows which pairings are gone (§219).
    settle: Option<Settle>,
}

/// Where the sheet's destination comes from, while it is the default.
#[derive(Debug, Clone)]
enum Settle {
    /// Back up's default for this folder under this base.
    BackUp { folder: PathBuf, base: PathBuf },
    /// Bring back's unwinding of this archive folder to the local root
    /// asked.
    BringBack { from: PathBuf },
}

/// What a sheet's look found: the plan, whether a bring-back's pairing
/// did not unwind, and the pairings whose destinations are gone.
struct Looked {
    plan: Plan,
    unpaired: bool,
    gone: Vec<Pairing>,
}

/// A job on an archive under way: a copy, or the rejects' move or
/// delete there.
struct Running {
    cancel: Arc<AtomicBool>,
    /// Bytes copied so far, or frames done for the rejects.
    bytes: Arc<AtomicU64>,
    total: u64,
    /// What the card says it is doing: "backing up to", "moving the
    /// rejects on".
    doing: &'static str,
    /// The bar is by frames, not bytes: the rejects' moves are renames,
    /// over in no time whatever the size.
    by_files: bool,
    label: String,
    token: u64,
    archive: PathBuf,
}

/// Where a list or a selection is, for the archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Side {
    /// Here: under a local root, or a folder open from outside any
    /// root. `base` is the root, or that folder; `folder` is what a
    /// copy over the whole list walks.
    Local {
        base: PathBuf,
        folder: Option<PathBuf>,
    },
    /// On an archive.
    Archive {
        archive: PathBuf,
        folder: Option<PathBuf>,
    },
    /// Neither, or more than one root: nothing to copy from as one.
    None,
}

/// A file's path as the index spells it: its folder canonical, by what
/// the window's reads kept.
fn keyed(st: &State, f: &Path) -> PathBuf {
    match (f.parent(), f.file_name()) {
        (Some(dir), Some(name)) => st
            .library
            .canonical
            .get(dir)
            .map(|c| c.join(name))
            .unwrap_or_else(|| f.to_path_buf()),
        _ => f.to_path_buf(),
    }
}

/// The folder open in a folder's view, canonical as the reads have it.
fn open_folder(st: &State) -> Option<PathBuf> {
    let dir = st.files.first()?.parent()?;
    Some(
        st.library
            .canonical
            .get(dir)
            .cloned()
            .unwrap_or_else(|| dir.to_path_buf()),
    )
}

/// Where a path is: under an archive, a local root, or a folder of no
/// root's (`folder`, its own base).
fn side_of_path(st: &State, path: &Path, folder: Option<PathBuf>) -> Side {
    let roots = &st.library.roots;
    if let Some(archive) = roots.archive_of(path) {
        return Side::Archive {
            archive: archive.to_path_buf(),
            folder,
        };
    }
    match roots.root_of(path) {
        Some(root) => Side::Local {
            base: root.to_path_buf(),
            folder,
        },
        None => match &folder {
            Some(f) => Side::Local {
                base: f.clone(),
                folder,
            },
            None => Side::None,
        },
    }
}

/// Where the view is.
pub(crate) fn side(st: &State) -> Side {
    match &st.view {
        View::Folder => match open_folder(st) {
            Some(dir) => side_of_path(st, &dir.clone(), Some(dir)),
            None => Side::None,
        },
        View::Roots(None) => Side::None,
        View::Roots(Some(r)) => side_of_path(st, r, Some(r.clone())),
        View::Branch { folder, .. } => side_of_path(st, folder, Some(folder.clone())),
    }
}

/// Where these frames are, as one: the same side and the same root or
/// archive for every one, else none.
fn side_of_frames(st: &State, frames: &[PathBuf]) -> Side {
    let view = side(st);
    let mut out: Option<Side> = None;
    for f in frames {
        let here = match (&view, st.library.roots.root_of(f)) {
            // A folder of no root's: its own base.
            (Side::Local { base, .. }, None) if f.starts_with(base) => Side::Local {
                base: base.clone(),
                folder: None,
            },
            _ => match side_of_path(st, f, None) {
                Side::Local { base, .. } => Side::Local { base, folder: None },
                Side::Archive { archive, .. } => Side::Archive {
                    archive,
                    folder: None,
                },
                Side::None => return Side::None,
            },
        };
        match &out {
            None => out = Some(here),
            Some(s) if *s == here => {}
            Some(_) => return Side::None,
        }
    }
    out.unwrap_or(Side::None)
}

/// What a side can be copied to: the archives from here, the local
/// roots from an archive.
fn choices(st: &State, side: &Side) -> Vec<PathBuf> {
    match side {
        Side::Local { .. } => st.library.roots.archives().to_vec(),
        Side::Archive { .. } => st.library.roots.locals(),
        Side::None => Vec::new(),
    }
}

fn labels(st: &State, roots: &[PathBuf]) -> ModelRc<slint::SharedString> {
    let names: Vec<slint::SharedString> = roots
        .iter()
        .map(|r| st.library.roots.label(r).into())
        .collect();
    ModelRc::new(VecModel::from(names))
}

fn frames_word(n: usize) -> String {
    if n == 1 {
        "1 frame".into()
    } else {
        format!("{n} frames")
    }
}

/// The header's button and the frame menu's items, for the view as it
/// is: no disk.
pub(crate) fn show(st: &State, app: &App) {
    let side = side(st);
    let choices = choices(st, &side);
    let roots = &st.library.roots;
    // The counts as last taken over this list, by archive.
    let counts: Vec<(PathBuf, (usize, usize))> = st
        .archive
        .count
        .as_ref()
        .filter(|(generation, _)| *generation == st.view_generation)
        .map(|(_, c)| {
            c.iter()
                .filter_map(|(a, n)| n.map(|n| (a.clone(), n)))
                .filter(|(a, _)| roots.is_archive(a))
                .collect()
        })
        .unwrap_or_default();
    let count_of = |a: &Path| counts.iter().find(|(c, _)| c == a).map(|(_, n)| *n);
    let not_on = |(n, u): (usize, usize), label: &str| header_words(n, u, label);
    let (header, verb, items) = match &side {
        _ if choices.is_empty() => (String::new(), "", Vec::new()),
        Side::Local { .. } => {
            // The first archive with a count names the button; each
            // choice in the menu carries its own.
            let words = counts
                .first()
                .map(|(a, n)| not_on(*n, &roots.label(a)))
                .unwrap_or_else(|| "Back up...".into());
            let items = choices
                .iter()
                .map(|a| {
                    let label = roots.label(a);
                    match count_of(a) {
                        Some(n) => format!("Back up to {label} ({})...", not_on(n, &label)),
                        None => format!("Back up to {label}..."),
                    }
                })
                .collect();
            (words, "Back up", items)
        }
        Side::Archive { .. } => (
            "Bring back...".into(),
            "Bring back",
            choices
                .iter()
                .map(|r| format!("Bring back to {}...", roots.label(r)))
                .collect(),
        ),
        Side::None => (String::new(), "", Vec::new()),
    };
    app.set_archive_header(header.into());
    app.set_archive_verb(verb.into());
    app.set_archive_choices(labels(st, &choices));
    let items: Vec<slint::SharedString> = items.into_iter().map(Into::into).collect();
    app.set_archive_header_choices(ModelRc::new(VecModel::from(items)));
    rejects::show(st, app);
}

/// The header's words for an archive: how many frames are not on it, and
/// how many could not be told without reading them whole, which the
/// count never does (the Back up sheet's look does). "All on" only when
/// every frame is known to be there.
pub(crate) fn header_words(missing: usize, unknown: usize, label: &str) -> String {
    match (missing, unknown) {
        (0, 0) => format!("All on {label}"),
        (n, 0) => format!("{} not on {label}", frames_word(n)),
        (0, u) => format!("{} not known on {label}", frames_word(u)),
        (n, u) => format!("{} not on {label}, {u} not known", frames_word(n)),
    }
}

/// The frame menu over a selection: its Back up or Bring back, by
/// where the selection is.
pub(crate) fn menu_asked(st: &State, app: &App) {
    let frames: Vec<PathBuf> = chosen_frames(st)
        .into_iter()
        .filter_map(|i| st.files.get(i))
        .map(|f| keyed(st, f))
        .collect();
    let side = side_of_frames(st, &frames);
    let choices = choices(st, &side);
    let verb = match (&side, choices.is_empty()) {
        (_, true) => "",
        (Side::Local { .. }, _) => "Back up",
        (Side::Archive { .. }, _) => "Bring back",
        (Side::None, _) => "",
    };
    app.set_archive_verb(verb.into());
    app.set_archive_choices(labels(st, &choices));
}

/// Where a copy is pressed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pressed {
    /// The grid's header: the open folder, or the selection when there
    /// is one.
    Header,
    /// The frame menu: the selection.
    Menu,
}

/// A copy asked for: what it is over worked out here, from the window's
/// state alone, and the look at what is on the other side sent off the
/// window's thread. The sheet opens when the look lands.
pub(crate) fn ask(state: &Rc<RefCell<State>>, app: &App, from: Pressed, choice: usize) {
    let mut st = state.borrow_mut();
    if st.archive.running.is_some() {
        app.set_status("a backup is still under way".into());
        return;
    }
    let chosen = chosen_frames(&st);
    let (offline, here): (Vec<usize>, Vec<usize>) = chosen
        .into_iter()
        .partition(|&i| crate::rows::is_offline(&st, i));
    let selection: Vec<PathBuf> = here
        .iter()
        .filter_map(|&i| st.files.get(i))
        .map(|f| keyed(&st, f))
        .collect();
    let many = selection.len() + offline.len() > 1;
    let (side, frames) = match from {
        Pressed::Menu => (
            side_of_frames(&st, &selection),
            Frames::Chosen(selection.clone()),
        ),
        Pressed::Header if many => (
            side_of_frames(&st, &selection),
            Frames::Chosen(selection.clone()),
        ),
        Pressed::Header => {
            let side = side(&st);
            let folder = match &side {
                Side::Local { folder, .. } | Side::Archive { folder, .. } => folder.clone(),
                Side::None => None,
            };
            match folder {
                Some(f) => (side, Frames::Folder(f)),
                None => (side, Frames::Chosen(selection.clone())),
            }
        }
    };
    if let Frames::Chosen(f) = &frames
        && f.is_empty()
    {
        if let Some(&i) = offline.first() {
            crate::rows::say_offline_for(&st, app, i, ": nothing to copy");
        } else {
            app.set_status("nothing is chosen to copy".into());
        }
        return;
    }
    let choices = choices(&st, &side);
    let Some(other) = choices.get(choice).cloned() else {
        app.set_status("there is no archive to copy to: mark a root as one from its menu".into());
        return;
    };
    let archive = match &side {
        Side::Archive { archive, .. } => archive.clone(),
        _ => other.clone(),
    };
    if st.archive.aside.iter().any(|(_, a)| *a == archive) {
        app.set_status(
            format!(
                "the copy set aside earlier has not come back from {} yet; another starts once \
                 it has",
                st.library.roots.label(&archive)
            )
            .into(),
        );
        return;
    }
    let roots = &st.library.roots;
    let label = roots.label(&other);
    // The folder the copy is over (§219: the pairing is its): the
    // folder walked, or the chosen frames' common folder.
    let over = |fallback: &Path| {
        match &frames {
            Frames::Folder(d) => Some(d.clone()),
            Frames::Chosen(f) => common_folder(f),
        }
        .unwrap_or_else(|| fallback.to_path_buf())
    };
    let asked = match &side {
        Side::Local { base, .. } => {
            let folder = over(base);
            // The default as the file has it; the look settles it again
            // once it knows which pairings are gone.
            let (dest, _) = roots.backup_default(&folder, base, &other);
            Asked {
                ask: Ask {
                    direction: Direction::BackUp,
                    frames,
                    include_rejects: false,
                    sidecars: st.write_sidecars,
                    base: folder.clone(),
                    dest,
                    other_side: vec![other.clone()],
                    skip: roots.archives().to_vec(),
                    index: st.index_path.clone(),
                },
                other,
                label,
                remember: Some(folder.clone()),
                unpaired: false,
                archive: archive.clone(),
                local: base.clone(),
                settle: Some(Settle::BackUp {
                    folder,
                    base: base.clone(),
                }),
            }
        }
        Side::Archive { archive, folder } => {
            let from = folder.clone().unwrap_or_else(|| over(archive));
            // Unwound by the look, which asks the disk whether a
            // pairing above the folder holds it.
            let (base, dest, unpaired) = unpaired(roots, &from, &other, archive);
            Asked {
                ask: Ask {
                    direction: Direction::BringBack,
                    frames,
                    include_rejects: false,
                    sidecars: st.write_sidecars,
                    base,
                    dest,
                    other_side: roots.locals(),
                    skip: roots.locals(),
                    index: st.index_path.clone(),
                },
                other: other.clone(),
                label,
                remember: None,
                unpaired,
                archive: archive.clone(),
                local: other,
                settle: Some(Settle::BringBack { from }),
            }
        }
        Side::None => {
            app.set_status(
                "these frames are under more than one root: back up a root's or a folder's at a time"
                    .into(),
            );
            return;
        }
    };
    app.set_status(format!("looking at what is on {}...", asked.label).into());
    st.archive.asked = Some(asked);
    st.archive.plan = None;
    look(&mut st, app);
}

/// Bring back with no pairing to unwind: from the archive folder `from`
/// to `home` joined with its name, the sheet asking. The ask's base,
/// its destination, and true.
fn unpaired(roots: &Roots, from: &Path, home: &Path, archive: &Path) -> (PathBuf, PathBuf, bool) {
    let name = from
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| roots.label(archive).into());
    (from.to_path_buf(), home.join(name), true)
}

/// The pairings between the sheet's local side and its archive whose
/// destination is no longer on the archive, off the window's thread,
/// after both roots answered. A pairing whose source folder is gone is
/// kept: a shoot is brought back because its folder here was deleted,
/// and the pairing is where home was. An archive that lists nothing at
/// all (a share's mount point with the share not on it) says nothing
/// about the folders under it, and none is taken for gone.
fn gone_pairings(roots: &Roots, local: &Path, archive: &Path, beat: &Beat) -> Vec<Pairing> {
    let lists = std::fs::read_dir(archive)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if !lists {
        return Vec::new();
    }
    roots
        .backups_between(local, archive)
        .filter(|p| {
            beat();
            !p.dest.is_dir()
        })
        .cloned()
        .collect()
}

/// The sheet's destination settled while it is the default, off the
/// window's thread (§219). Back up's from the pairings less those gone.
/// Bring back's from the pairings over the archive folder, the deepest
/// first: one whose destination is the folder is exact, and unwinds to
/// its source even when that folder is gone here (the copy makes it
/// again, as it does any destination); one above it only when its
/// source has the
/// same folder under it, so a shoot under a paired year folder that was
/// never backed up from here is not that pairing's. Whether the
/// bring-back's pairing did not unwind.
fn settle(
    ask: &mut Ask,
    settle: &Settle,
    roots: &Roots,
    gone: &[Pairing],
    home: &Path,
    archive: &Path,
    beat: &Beat,
) -> bool {
    match settle {
        Settle::BackUp { folder, base } => {
            let mut left = roots.clone();
            for p in gone {
                left.drop_backup(p);
            }
            ask.base = folder.clone();
            ask.dest = left.backup_default(folder, base, archive).0;
            false
        }
        Settle::BringBack { from } => {
            let found = roots.unwind(from, home, archive).into_iter().find(|p| {
                beat();
                match from.strip_prefix(&p.dest) {
                    Ok(rel) if rel.as_os_str().is_empty() => true,
                    Ok(rel) => p.source.join(rel).is_dir(),
                    Err(_) => false,
                }
            });
            let (base, dest, unpaired) = match found {
                Some(p) => (p.dest, p.source, false),
                None => unpaired(roots, from, home, archive),
            };
            ask.base = base;
            ask.dest = dest;
            unpaired
        }
    }
}

/// The status line's words for pairings dropped because their
/// destination is gone: said once, since they are gone from the file
/// after.
fn gone_words(gone: &[Pairing], archive_label: &str) -> String {
    match gone {
        [] => String::new(),
        [p] => format!(
            "forgot that {} was backed up to {}: that folder is no longer on {archive_label}",
            p.source.display(),
            p.dest.display(),
        ),
        many => format!(
            "forgot {} backups whose folders are no longer on {archive_label}: {}",
            many.len(),
            some_names(&many.iter().map(|p| p.source.as_path()).collect::<Vec<_>>())
        ),
    }
}

/// The folder every one of `frames` is under.
fn common_folder(frames: &[PathBuf]) -> Option<PathBuf> {
    let mut common = frames.first()?.parent()?.to_path_buf();
    for f in &frames[1..] {
        while !f.starts_with(&common) {
            common = common.parent()?.to_path_buf();
        }
    }
    Some(common)
}

/// The look at what the sheet's ask would do, off the window's thread:
/// the two sides' roots looked at first with the roots' 3 s look, then
/// the plan.
fn look(st: &mut State, app: &App) {
    let Some(asked) = st.archive.asked.clone() else {
        return;
    };
    st.archive.look_token += 1;
    let token = st.archive.look_token;
    let roots = &st.library.roots;
    let mut looked: Vec<(PathBuf, String)> = vec![(asked.other.clone(), asked.label.clone())];
    let source = match asked.ask.direction {
        Direction::BackUp => roots.root_of(&asked.ask.base),
        Direction::BringBack => roots.archive_of(&asked.ask.base),
    };
    if let Some(r) = source {
        looked.push((r.to_path_buf(), roots.label(r)));
    }
    let mut ask = asked.ask.clone();
    let (pairings, local, archive) = (roots.clone(), asked.local.clone(), asked.archive.clone());
    let (to_settle, mut unpaired) = (asked.settle.clone(), asked.unpaired);
    let sent = send(
        app,
        "archive look",
        token,
        move |beat| {
            let roots: Vec<PathBuf> = looked.iter().map(|(r, _)| r.clone()).collect();
            let answered = look_at(&roots, beat);
            if let Some((_, name)) = looked
                .iter()
                .zip(&answered)
                .find(|(_, a)| !**a)
                .map(|(l, _)| l)
            {
                return Err(format!(
                    "{name} is not answering; is its drive or share there? Nothing was copied"
                ));
            }
            let gone = gone_pairings(&pairings, &local, &archive, beat);
            if let Some(how) = &to_settle {
                unpaired = settle(&mut ask, how, &pairings, &gone, &local, &archive, beat);
            }
            Ok(Looked {
                plan: archive::plan(&ask, beat),
                unpaired,
                gone,
            })
        },
        land_look,
    );
    if !sent {
        app.set_status("the look at the archive could not be started".into());
        st.archive.asked = None;
    }
}

/// The look landed: the sheet filled and opened, or what went wrong
/// said.
fn land_look(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    token: u64,
    heard: Heard<Result<Looked, String>>,
) {
    let mut st = state.borrow_mut();
    if token != st.archive.look_token {
        return;
    }
    let Some(mut asked) = st.archive.asked.clone() else {
        return;
    };
    match heard {
        Heard::Aside => {
            app.set_status(
                format!(
                    "{} has said nothing for {} s; the look is set aside and nothing was copied",
                    asked.label,
                    ROOT_WAIT.as_secs()
                )
                .into(),
            );
            close(&mut st, app);
        }
        Heard::Lost => {
            app.set_status("the look at the archive failed; the log has why".into());
            close(&mut st, app);
        }
        Heard::Done { late: true, .. } => {}
        Heard::Done {
            result: Err(why), ..
        } => {
            app.set_status(why.into());
            close(&mut st, app);
        }
        Heard::Done {
            result: Ok(looked), ..
        } => {
            // The pairings found gone are dropped, through the file, and
            // said once: the next look does not find them.
            let gone = looked.gone.clone();
            if !gone.is_empty()
                && let Err(e) = crate::roots::edit_roots(&mut st, |r| {
                    for p in &gone {
                        r.drop_backup(p);
                    }
                })
            {
                tracing::warn!("library: roots not saved: {e}");
            }
            // The field as settled is what the sheet holds from now on.
            asked.ask = looked.plan.ask.clone();
            asked.unpaired = looked.unpaired;
            asked.settle = None;
            fill(app, &asked, &looked.plan);
            let archive_label = st.library.roots.label(&asked.archive);
            st.archive.asked = Some(asked);
            st.archive.plan = Some(looked.plan);
            app.set_status(gone_words(&looked.gone, &archive_label).into());
            app.set_archive_open(true);
        }
    }
}

fn close(st: &mut State, app: &App) {
    st.archive.asked = None;
    st.archive.plan = None;
    app.set_archive_open(false);
    rejects::run_due(st, app);
}

/// A few names, and how many more.
fn some_names(paths: &[&Path]) -> String {
    const SHOWN: usize = 6;
    let mut words: Vec<String> = paths.iter().take(SHOWN).map(|p| file_name(p)).collect();
    if paths.len() > SHOWN {
        words.push(format!("and {} more", paths.len() - SHOWN));
    }
    words.join(", ")
}

/// The sheet's words for a plan.
fn fill(app: &App, asked: &Asked, plan: &Plan) {
    let label = &asked.label;
    let (verb, title) = match asked.ask.direction {
        Direction::BackUp => ("Back up", format!("Back up to {label}")),
        Direction::BringBack => ("Bring back", format!("Bring back to {label}")),
    };
    let (n, bytes) = plan.copies();
    let sidecars = plan.sidecars_only();
    let (same, elsewhere) = plan.same();
    let mut lines = Vec::new();
    let with = if plan.ask.sidecars {
        ", with their sidecars"
    } else {
        ""
    };
    if n > 0 {
        lines.push(format!(
            "{} to copy, {}{with}.",
            frames_word(n),
            archive::size_words(bytes)
        ));
    }
    if !plan.ask.sidecars {
        lines.push(
            "Sidecars are off for this run: the frames go without them, and no sidecar is \
             compared or written."
                .into(),
        );
    }
    if sidecars > 0 {
        let here = match asked.ask.direction {
            Direction::BackUp => "here",
            Direction::BringBack => "on the archive",
        };
        lines.push(format!(
            "{} already there {} newer sidecars {here}: the sidecars alone go.",
            frames_word(sidecars),
            if sidecars == 1 { "has" } else { "have" }
        ));
    }
    if same > 0 {
        let at = match asked.ask.direction {
            Direction::BackUp => format!("on {label}"),
            Direction::BringBack => "here".into(),
        };
        let mut line = format!("{} already {at}", frames_word(same));
        if elsewhere > 0 {
            line.push_str(&format!(", {elsewhere} under a different folder"));
        }
        line.push_str(", skipped.");
        lines.push(line);
    }
    if n == 0 && sidecars == 0 {
        lines.push("Nothing to copy.".into());
    }
    let mut named = Vec::new();
    let theirs = plan.theirs_newer();
    if !theirs.is_empty() {
        let whose = match asked.ask.direction {
            Direction::BackUp => format!("{label}'s"),
            Direction::BringBack => "the one here".into(),
        };
        named.push(format!(
            "Skipped, {whose} sidecar is the newer: {}.",
            some_names(&theirs)
        ));
    }
    let taken = plan.taken();
    if !taken.is_empty() {
        named.push(format!(
            "Left, another file has the name, or a sidecar's name, there: {}.",
            some_names(&taken)
        ));
    }
    let unreadable = plan.unreadable();
    if !unreadable.is_empty() {
        let paths: Vec<&Path> = unreadable.iter().map(|(p, _)| *p).collect();
        named.push(format!("Could not be read: {}.", some_names(&paths)));
    }
    app.set_archive_title(title.into());
    app.set_archive_text(lines.join("\n").into());
    app.set_archive_named(named.join("\n").into());
    app.set_archive_dest(plan.ask.dest.to_string_lossy().into_owned().into());
    let note = match (asked.ask.direction, asked.unpaired) {
        (Direction::BackUp, _) => format!(
            "Under {label}; kept for the next backup of this folder. Each frame goes at its path under it."
        ),
        (Direction::BringBack, true) => format!(
            "This folder of the archive was not backed up from {label}: choose where under {label} it goes."
        ),
        (Direction::BringBack, false) => {
            format!("Where it was backed up from, under {label}.")
        }
    };
    app.set_archive_dest_note(note.into());
    app.set_archive_rejects(match plan.ask.frames {
        Frames::Folder(_) if plan.rejects > 0 => plan.rejects as i32,
        _ => -1,
    });
    app.set_archive_include_rejects(plan.ask.include_rejects);
    app.set_archive_confirm(
        if n > 0 {
            format!("{verb} {}", frames_word(n))
        } else if sidecars > 0 {
            format!("Copy the sidecars of {}", frames_word(sidecars))
        } else {
            verb.to_string()
        }
        .into(),
    );
    app.set_archive_can_confirm(plan.has_work());
}

/// The folder field or the rejects' checkbox changed: looked at again,
/// the folder held to the root it must be under.
pub(crate) fn look_again(state: &Rc<RefCell<State>>, app: &App) {
    let mut st = state.borrow_mut();
    let Some(asked) = st.archive.asked.as_mut() else {
        return;
    };
    // Kept as typed less a trailing separator: `/mnt/x/2026/` is the
    // folder `/mnt/x/2026`.
    let typed = greycard_library::roots::tidy(Path::new(app.get_archive_dest().trim()));
    if !typed.is_absolute()
        || !typed.starts_with(&asked.other)
        || typed
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        app.set_archive_dest_note(
            format!(
                "The folder has to be under {} ({}).",
                asked.label,
                asked.other.display()
            )
            .into(),
        );
        app.set_archive_can_confirm(false);
        return;
    }
    asked.ask.dest = typed;
    // The field is the user's now: the look keeps it as it is.
    asked.settle = None;
    asked.ask.include_rejects = app.get_archive_include_rejects();
    app.set_archive_can_confirm(false);
    app.set_archive_confirm("Looking...".into());
    look(&mut st, app);
}

/// The sheet answered. A yes over a folder typed and not looked at yet
/// looks first; otherwise the copy goes off the window's thread.
pub(crate) fn answered(state: &Rc<RefCell<State>>, app: &App, yes: bool) {
    if !yes {
        let mut st = state.borrow_mut();
        st.archive.look_token += 1;
        close(&mut st, app);
        return;
    }
    let typed_changed = {
        let st = state.borrow();
        let Some(plan) = &st.archive.plan else {
            return;
        };
        Path::new(app.get_archive_dest().trim()) != plan.ask.dest
            || app.get_archive_include_rejects() != plan.ask.include_rejects
    };
    if typed_changed {
        look_again(state, app);
        return;
    }
    let mut st = state.borrow_mut();
    let (Some(asked), Some(plan)) = (st.archive.asked.take(), st.archive.plan.take()) else {
        return;
    };
    app.set_archive_open(false);
    if !plan.has_work() {
        return;
    }
    if st.archive.running.is_some() || st.archive.aside.iter().any(|(_, a)| *a == asked.archive) {
        app.set_status(
            "not copied: another job on an archive started meanwhile; ask again once it is done"
                .into(),
        );
        return;
    }
    if !st.deletes_allowed {
        let why = "not copied: a capture, an export or a timing run never copies";
        tracing::info!("{why}");
        app.set_status(why.into());
        return;
    }
    // The folder chosen is kept as this folder's pairing (§219).
    if let Some(source) = &asked.remember {
        let (archive, dest) = (asked.other.clone(), plan.ask.dest.clone());
        if let Err(e) =
            crate::roots::edit_roots(&mut st, |r| r.set_backup_folder(source, &archive, &dest))
        {
            tracing::warn!("library: roots not saved: {e}");
        }
    }
    start(&mut st, app, asked, plan);
}

/// The copy sent off, its card up.
fn start(st: &mut State, app: &App, asked: Asked, plan: Plan) {
    st.archive.run_token += 1;
    let token = st.archive.run_token;
    let (n, total) = plan.copies();
    let cancel = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(AtomicU64::new(0));
    st.archive.running = Some(Running {
        cancel: cancel.clone(),
        bytes: bytes.clone(),
        total,
        doing: match asked.ask.direction {
            Direction::BackUp => "backing up to",
            Direction::BringBack => "bringing back to",
        },
        by_files: false,
        label: asked.label.clone(),
        token,
        archive: asked.archive.clone(),
    });
    let doing = match asked.ask.direction {
        Direction::BackUp => "backing up",
        Direction::BringBack => "bringing back",
    };
    tracing::info!(
        "archive: {doing} {} ({} bytes) to {}",
        frames_word(n),
        total,
        plan.ask.dest.display()
    );
    app.set_archive_fraction(0.0);
    app.set_archive_line(format!("{doing} {} to {}...", frames_word(n), asked.label).into());
    app.set_archive_running(true);
    let index = plan.ask.index.clone();
    let (direction, label) = (asked.ask.direction, asked.label.clone());
    let sent = send(
        app,
        "archive copy",
        token,
        move |beat| {
            let hooks = archive::Hooks::new(&**beat, &cancel, &bytes);
            let report = archive::run(&plan, &hooks);
            if let Some(index) = &index {
                archive::record(index, &report, beat);
            }
            Copied {
                report,
                direction,
                label,
            }
        },
        land_run,
    );
    if !sent {
        st.archive.running = None;
        app.set_archive_running(false);
        app.set_status("the copy could not be started; nothing was copied".into());
        return;
    }
    start_timer(st, app);
}

/// The card's bar filled from the job's count, five times a second.
fn start_timer(st: &State, app: &App) {
    let app_weak = app.as_weak();
    st.archive.timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(200),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(state) = crate::STATE.with(|s| s.borrow().clone()) else {
                return;
            };
            tick(&state.borrow(), &app);
        },
    );
}

/// The card's bar and words from the copy's count.
fn tick(st: &State, app: &App) {
    let Some(r) = &st.archive.running else {
        return;
    };
    let done = r.bytes.load(Ordering::Relaxed);
    let fraction = if r.total == 0 {
        1.0
    } else {
        done as f32 / r.total as f32
    };
    app.set_archive_fraction(fraction);
    let doing = r.doing;
    let canceling = if r.cancel.load(Ordering::Relaxed) {
        "; stopping after this frame"
    } else {
        ""
    };
    app.set_archive_line(
        format!(
            "{doing} {}: {} of {}{canceling}",
            r.label,
            if r.by_files {
                done.to_string()
            } else {
                archive::size_words(done)
            },
            if r.by_files {
                frames_word(r.total as usize)
            } else {
                archive::size_words(r.total)
            }
        )
        .into(),
    );
}

/// Cancel: the copy stops once the frame it is on is done.
pub(crate) fn cancel(st: &State, app: &App) {
    if let Some(r) = &st.archive.running {
        r.cancel.store(true, Ordering::SeqCst);
        tick(st, app);
    }
}

/// What a copy's job hands back: its report, and which way it went and
/// where, so a late landing is said as what it was.
pub(crate) struct Copied {
    report: Report,
    direction: Direction,
    label: String,
}

/// The copy landed, or went quiet: said, its folders handed to the
/// indexer, the count taken again.
fn land_run(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    token: u64,
    heard: Heard<Copied>,
) {
    let mut st = state.borrow_mut();
    let ours = st
        .archive
        .running
        .as_ref()
        .is_some_and(|r| r.token == token);
    match heard {
        Heard::Aside => {
            if !ours {
                return;
            }
            if let Some(r) = st.archive.running.take() {
                // It goes on alone: the frame it is on, and no more. No
                // other copy starts until it lands.
                r.cancel.store(true, Ordering::SeqCst);
                st.archive.aside.push((token, r.archive.clone()));
                st.archive.timer.stop();
                app.set_archive_running(false);
                app.set_status(
                    format!(
                        "{} has said nothing for {} s: the copy is set aside, and stops after \
                         the frame it is on; what it did is said if it answers",
                        r.label,
                        ROOT_WAIT.as_secs()
                    )
                    .into(),
                );
            }
        }
        Heard::Lost => {
            // Its thread ended with nothing to say: let go of it either
            // way, so nothing waits on it for the session.
            if ours {
                st.archive.running = None;
                st.archive.timer.stop();
                app.set_archive_running(false);
            }
            st.archive.aside.retain(|(t, _)| *t != token);
            tracing::error!("archive: the copy's thread ended without a report");
            app.set_status(
                "the copy stopped without a report; what it copied is checked by the next Back up \
                 (the log has why)"
                    .into(),
            );
            rejects::run_due(&mut st, app);
        }
        Heard::Done { result, late } => {
            if ours {
                st.archive.running = None;
                st.archive.timer.stop();
                app.set_archive_running(false);
            }
            st.archive.aside.retain(|(t, _)| *t != token);
            let Copied {
                report,
                direction,
                label,
            } = result;
            let mut words = summary(&report, direction, &label);
            if late {
                words = format!("the copy set aside earlier came back: {words}");
            }
            tracing::info!("archive: {words}");
            for (f, why) in &report.failed {
                tracing::warn!("archive: {}: {why}", f.display());
            }
            app.set_status(words.into());
            // The folders written to, passed over now rather than at the
            // next poll.
            if let Some(indexer) = &st.index
                && !report.folders.is_empty()
            {
                indexer
                    .asker()
                    .changes(report.folders.iter().cloned().map(Change::Folder).collect());
            }
            want_count(&mut st, app);
            // A queue heard due while the card was this copy's.
            rejects::run_due(&mut st, app);
        }
    }
}

/// What a copy did, in a line.
pub(crate) fn summary(report: &Report, direction: Direction, label: &str) -> String {
    let mut parts = Vec::new();
    let to = if label.is_empty() {
        String::new()
    } else {
        format!(" to {label}")
    };
    let verb = match direction {
        Direction::BackUp => "backed up",
        Direction::BringBack => "brought back",
    };
    let bytes: u64 = report.copied.iter().map(|c| c.2).sum();
    parts.push(format!(
        "{verb} {}{to} ({})",
        frames_word(report.copied.len()),
        archive::size_words(bytes)
    ));
    if report.sidecars_only > 0 {
        parts.push(format!(
            "the sidecars of {} already there",
            frames_word(report.sidecars_only)
        ));
    }
    if report.same > 0 {
        parts.push(format!("{} already there", report.same));
    }
    if !report.theirs_newer.is_empty() {
        let paths: Vec<&Path> = report.theirs_newer.iter().map(PathBuf::as_path).collect();
        parts.push(format!(
            "skipped, the other side's sidecar is newer: {}",
            some_names(&paths)
        ));
    }
    if !report.taken.is_empty() {
        let paths: Vec<&Path> = report.taken.iter().map(PathBuf::as_path).collect();
        parts.push(format!(
            "left, another file has the name: {}",
            some_names(&paths)
        ));
    }
    if !report.failed.is_empty() {
        let (f, why) = &report.failed[0];
        parts.push(format!(
            "{} failed: {} ({why}{})",
            report.failed.len(),
            file_name(f),
            if report.failed.len() > 1 {
                "; the log has the rest"
            } else {
                ""
            }
        ));
    }
    if !report.stale.is_empty() {
        parts.push(format!(
            "removed {} left by an earlier copy that did not finish",
            if report.stale.len() == 1 {
                "a temporary file".to_string()
            } else {
                format!("{} temporary files", report.stale.len())
            }
        ));
    }
    if report.canceled > 0 {
        parts.push(format!(
            "canceled, {} not copied",
            frames_word(report.canceled)
        ));
    }
    parts.join("; ")
}

/// The header's counts taken again, off the window's thread, when the
/// view is a source that has been backed up once (a folder under its
/// root, or under the folder of no root's, has a pairing to the archive:
/// the pairing is the mark, §219): for each archive so paired,
/// how many of the frames a Back up of the view would walk (its folder,
/// every folder down, the rejects left out) are not on it. A count
/// overtaken by a newer one stops between frames.
pub(crate) fn want_count(st: &mut State, app: &App) {
    show(st, app);
    let Side::Local {
        base,
        folder: Some(folder),
    } = side(st)
    else {
        return;
    };
    let roots = &st.library.roots;
    let paired: Vec<PathBuf> = roots
        .archives()
        .iter()
        .filter(|a| roots.backed_up_under(&base, a))
        .cloned()
        .collect();
    if paired.is_empty() {
        return;
    }
    let skip = roots.archives().to_vec();
    if let Some(old) = st.archive.count_stop.take() {
        old.store(true, Ordering::SeqCst);
    }
    let stop = Arc::new(AtomicBool::new(false));
    st.archive.count_stop = Some(stop.clone());
    st.archive.count_token += 1;
    let token = st.archive.count_token;
    let generation = st.view_generation;
    let index = st.index_path.clone();
    send(
        app,
        "archive count",
        token,
        move |beat| {
            let answered = look_at(&paired, beat);
            let halt = || stop.load(Ordering::SeqCst);
            let (frames, _) = archive::walk(&folder, false, &skip, &**beat, &halt);
            if halt() {
                return None;
            }
            let lib = index
                .as_deref()
                .and_then(|p| archive::open_index(p, false, beat));
            let mut counts = Vec::new();
            for (a, there) in paired.iter().zip(answered) {
                if !there {
                    counts.push((a.clone(), None));
                    continue;
                }
                let on = archive::on_archives(
                    &frames,
                    std::slice::from_ref(a),
                    lib.as_ref(),
                    &**beat,
                    &halt,
                )?;
                let missing = on
                    .iter()
                    .filter(|w| w.on.is_empty() && w.unknown.is_empty())
                    .count();
                let unknown = on
                    .iter()
                    .filter(|w| w.on.is_empty() && !w.unknown.is_empty())
                    .count();
                counts.push((a.clone(), Some((missing, unknown))));
            }
            Some((generation, counts))
        },
        land_count,
    );
}

fn land_count(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    token: u64,
    heard: Heard<Option<Counts>>,
) {
    let mut st = state.borrow_mut();
    if token != st.archive.count_token {
        return;
    }
    if let Heard::Done {
        result: Some(count),
        ..
    } = heard
    {
        st.archive.count = Some(count);
        st.archive.count_stop = None;
        show(&st, app);
    }
}

/// The Delete sheet's line: whether the frames are on an archive, by
/// the same lookup, off the window's thread. A line, not a refusal.
pub(crate) fn delete_line(st: &mut State, app: &App, frames: &[PathBuf]) {
    let mut archives = st.library.roots.archives().to_vec();
    if archives.is_empty() || frames.is_empty() {
        app.set_delete_archive("".into());
        return;
    }
    // Frames that are themselves on an archive (its chip view) are the
    // archive's copies: the question for them is whether a copy is
    // anywhere else, on a local root or another archive, since the
    // lookup never finds a frame at its own path. The roots looked at
    // are then every root but the archives the frames are on.
    let own: Vec<PathBuf> = frames
        .iter()
        .filter_map(|f| st.library.roots.archive_of(f).map(Path::to_path_buf))
        .collect();
    let elsewhere = frames.len() == own.len();
    if elsewhere {
        archives = st
            .library
            .roots
            .list()
            .iter()
            .filter(|r| !own.contains(r))
            .cloned()
            .collect();
        if archives.is_empty() {
            app.set_delete_archive(if frames.len() == 1 {
                "It is on no other root.".into()
            } else {
                format!("All {} are on no other root.", frames.len()).into()
            });
            return;
        }
    }
    app.set_delete_archive(
        if elsewhere {
            "Looking for copies of these on the other roots..."
        } else {
            "Looking for these on the archives..."
        }
        .into(),
    );
    st.archive.delete_token += 1;
    let token = st.archive.delete_token;
    let frames: Vec<PathBuf> = frames.iter().map(|f| keyed(st, f)).collect();
    let labels: Vec<String> = archives.iter().map(|a| st.library.roots.label(a)).collect();
    let index = st.index_path.clone();
    send(
        app,
        "archive delete look",
        token,
        move |beat| {
            // Every archive looked at together, the wait beating, so one
            // that does not answer is said for itself alone.
            let answered = look_at(&archives, beat);
            let looked: Vec<PathBuf> = archives
                .iter()
                .zip(&answered)
                .filter(|(_, a)| **a)
                .map(|(r, _)| r.clone())
                .collect();
            let lib = index
                .as_deref()
                .and_then(|p| archive::open_index(p, false, beat));
            let on = archive::on_archives(&frames, &looked, lib.as_ref(), &**beat, &|| false)
                .unwrap_or_default();
            // Back to indices into every archive.
            let into: Vec<usize> = answered
                .iter()
                .enumerate()
                .filter(|(_, a)| **a)
                .map(|(i, _)| i)
                .collect();
            let on: Vec<archive::Where> = on
                .into_iter()
                .map(|w| archive::Where {
                    on: w.on.into_iter().map(|k| into[k]).collect(),
                    unknown: w.unknown.into_iter().map(|k| into[k]).collect(),
                })
                .collect();
            delete_words(&labels, &answered, &on, elsewhere)
        },
        land_delete,
    );
}

fn land_delete(
    state: &Rc<RefCell<State>>,
    app: &App,
    _worker: &Rc<Worker>,
    token: u64,
    heard: Heard<String>,
) {
    let st = state.borrow();
    if token != st.archive.delete_token || !app.get_delete_open() {
        return;
    }
    match heard {
        Heard::Done {
            result,
            late: false,
        } => app.set_delete_archive(result.into()),
        Heard::Done { late: true, .. } => {}
        Heard::Aside | Heard::Lost => app.set_delete_archive(
            "An archive stopped answering; whether these are on it is not known.".into(),
        ),
    }
}

/// The Delete sheet's words: all the frames on one archive, all on
/// some archive, or how many are on none; how many could not be told
/// without reading them whole (the look never does: the Back up sheet's
/// does); and the archives that did not answer. With `elsewhere` the
/// frames are an archive's own and the roots looked at are the others,
/// so the words say "also on" and "no other root".
pub(crate) fn delete_words(
    labels: &[String],
    answered: &[bool],
    on: &[archive::Where],
    elsewhere: bool,
) -> String {
    let n = on.len();
    let unknown = on
        .iter()
        .filter(|w| w.on.is_empty() && !w.unknown.is_empty())
        .count();
    let (also, none_of, an) = if elsewhere {
        ("also on", "no other root", "another root")
    } else {
        ("on", "no archive", "an archive")
    };
    let mut words = if n == 0 {
        String::new()
    } else if let Some(k) = (0..labels.len()).find(|k| on.iter().all(|w| w.on.contains(k))) {
        if n == 1 {
            format!("It is {also} {}.", labels[k])
        } else {
            format!("All {n} are {also} {}.", labels[k])
        }
    } else {
        let none = on
            .iter()
            .filter(|w| w.on.is_empty() && w.unknown.is_empty())
            .count();
        match none {
            0 if unknown == 0 => format!("All {n} are {also} {an}."),
            0 => String::new(),
            1 if n == 1 => format!("It is on {none_of}."),
            1 => format!("1 of these is on {none_of}."),
            none => format!("{none} of these are on {none_of}."),
        }
    };
    match unknown {
        0 => {}
        1 if n == 1 => words.push_str(&format!(
            " Whether it is {also} {an} could not be told without reading it whole \
             (Back up's sheet reads it)."
        )),
        u => words.push_str(&format!(
            " {u} could not be told without reading them whole (Back up's sheet reads them)."
        )),
    }
    let quiet: Vec<&str> = labels
        .iter()
        .zip(answered)
        .filter(|(_, a)| !**a)
        .map(|(l, _)| l.as_str())
        .collect();
    if !quiet.is_empty() {
        words.push_str(&format!(
            " {} did not answer, and {} not looked at.",
            quiet.join(", "),
            if quiet.len() == 1 { "was" } else { "were" }
        ));
    }
    words.trim().to_string()
}

/// A root's archive mark turned on or off, from its chip's menu.
pub(crate) fn toggle(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, dir: &Path) {
    let refresh = {
        let mut st = state.borrow_mut();
        let on = !st.library.roots.is_archive(dir);
        match crate::roots::edit_roots(&mut st, |r| r.set_archive(dir, on)) {
            Ok(true) => {}
            Ok(false) => {
                app.set_status(format!("{} is not in the library", dir.display()).into());
                return;
            }
            Err(e) => {
                tracing::warn!("library: roots not saved: {e}");
                app.set_status(format!("not changed: {e}").into());
                return;
            }
        }
        let name = crate::roots::said(&st.library.roots, dir);
        tracing::info!(
            "library: {} {}",
            dir.display(),
            if on {
                "is an archive"
            } else {
                "is no longer an archive"
            }
        );
        app.set_status(
            if on {
                format!("{name} is an archive: Back up copies to it, Bring back from it")
            } else {
                format!("{name} is a plain root again; its files are where they were")
            }
            .into(),
        );
        st.archive.count = None;
        crate::roots::show(&st, app);
        want_count(&mut st, app);
        matches!(st.view, View::Roots(None))
    };
    // All roots leaves the archive's copies of local frames out.
    if refresh {
        crate::roots::refresh_view(state, app, worker);
    }
}

/// Each of `roots` looked at with the roots' 3 s look, all together,
/// the wait beating: whether each answered. One that does not answer in
/// the look's time is no for itself alone.
fn look_at(roots: &[PathBuf], beat: &Beat) -> Vec<bool> {
    // In a test, in place, as the roots' own looks are.
    #[cfg(test)]
    return roots
        .iter()
        .map(|r| {
            beat();
            crate::roots::answer_of(r, ROOT_WAIT) == Some(true)
        })
        .collect();
    #[cfg_attr(test, allow(unreachable_code))]
    let (tx, rx) = std::sync::mpsc::channel();
    for (i, r) in roots.iter().enumerate() {
        let (tx, r) = (tx.clone(), r.clone());
        let _ = std::thread::Builder::new()
            .name("greycard archive look".into())
            .spawn(move || {
                let _ = tx.send((i, crate::roots::answer_of(&r, ROOT_WAIT) == Some(true)));
            });
    }
    drop(tx);
    let mut out = vec![false; roots.len()];
    let until = std::time::Instant::now() + ROOT_WAIT + std::time::Duration::from_secs(1);
    loop {
        beat();
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok((i, there)) => out[i] = there,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if std::time::Instant::now() > until {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    out
}

/// Where a job's end lands on the window's thread.
type Landing<R> = fn(&Rc<RefCell<State>>, &App, &Rc<Worker>, u64, Heard<R>);

/// Run `job` off the window's thread with its beats watched, and land
/// what it says on the window's thread. False when no thread could be
/// had.
#[cfg(not(test))]
fn send<R: Send + 'static>(
    app: &App,
    name: &str,
    token: u64,
    job: impl FnOnce(&Beat) -> R + Send + 'static,
    land: Landing<R>,
) -> bool {
    let app_weak = app.as_weak();
    let watched = archive::supervise(name, ROOT_WAIT, job, move |heard| {
        let app_weak = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let (Some(app), Some(state), Some(worker)) = (
                app_weak.upgrade(),
                crate::STATE.with(|s| s.borrow().clone()),
                crate::WORKER.with(|w| w.borrow().clone()),
            ) else {
                return;
            };
            land(&state, &app, &worker, token, heard);
        });
    });
    match watched {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("archive: {e}");
            false
        }
    }
}

/// In a test the job is queued, and the test runs and lands it in place
/// (`tests::land_sent`): there is no event loop to land it on. A test
/// that asks for it (`tests::ASIDE_NEXT`) has the next job heard as set
/// aside first, and its end land later, late, as the watch would.
#[cfg(test)]
fn send<R: Send + 'static>(
    _app: &App,
    _name: &str,
    token: u64,
    job: impl FnOnce(&Beat) -> R + Send + 'static,
    land: Landing<R>,
) -> bool {
    let aside = tests::ASIDE_NEXT.with(|a| a.replace(false));
    tests::SENT.with(|s| {
        s.borrow_mut().push(Box::new(move |state, app, worker| {
            if aside {
                land(state, app, worker, token, Heard::Aside);
                tests::SENT.with(|s| {
                    s.borrow_mut().push(Box::new(move |state, app, worker| {
                        let result = job(&archive::no_beat());
                        land(
                            state,
                            app,
                            worker,
                            token,
                            Heard::Done { result, late: true },
                        );
                    }))
                });
            } else {
                let result = job(&archive::no_beat());
                land(
                    state,
                    app,
                    worker,
                    token,
                    Heard::Done {
                        result,
                        late: false,
                    },
                );
            }
        }))
    });
    true
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    rejects::install(app, state);
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_library_root_archive(move |path| {
            if let Some(app) = app_weak.upgrade() {
                toggle(&state, &app, &worker, Path::new(path.as_str()));
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_archive_header_pressed(move |i| {
            if let Some(app) = app_weak.upgrade() {
                ask(&state, &app, Pressed::Header, i.max(0) as usize);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_archive_menu_picked(move |i| {
            if let Some(app) = app_weak.upgrade() {
                ask(&state, &app, Pressed::Menu, i.max(0) as usize);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_archive_answered(move |yes| {
            if let Some(app) = app_weak.upgrade() {
                answered(&state, &app, yes);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_archive_look_again(move || {
            if let Some(app) = app_weak.upgrade() {
                look_again(&state, &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_archive_cancel(move || {
            if let Some(app) = app_weak.upgrade() {
                cancel(&state.borrow(), &app);
            }
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    type Sent = Box<dyn FnOnce(&Rc<RefCell<State>>, &App, &Rc<Worker>)>;

    thread_local! {
        pub(crate) static SENT: RefCell<Vec<Sent>> = RefCell::new(Vec::new());
        /// The next job sent is heard as set aside first, then late.
        pub(crate) static ASIDE_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Run the jobs sent off and land them, as the threads and the event
    /// loop would, until none are left; how many there were.
    pub(crate) fn land_sent(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
        let mut n = 0;
        loop {
            let jobs: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
            if jobs.is_empty() {
                return n;
            }
            for job in jobs {
                job(state, app, worker);
                n += 1;
            }
        }
    }

    /// The jobs sent off so far run and landed, once: what one of them
    /// sends meanwhile waits for the next turn.
    fn land_once(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
        let jobs: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        let n = jobs.len();
        for job in jobs {
            job(state, app, worker);
        }
        n
    }

    #[test]
    fn the_delete_line_says_where_the_frames_are() {
        let on = |v: &[&[usize]]| -> Vec<archive::Where> {
            v.iter()
                .map(|o| archive::Where {
                    on: o.to_vec(),
                    unknown: Vec::new(),
                })
                .collect()
        };
        let labels = vec!["Archive".to_string(), "Cloud".to_string()];
        let both = [true, true];
        assert_eq!(
            delete_words(&labels, &both, &on(&[&[0], &[0, 1], &[0]]), false),
            "All 3 are on Archive."
        );
        assert_eq!(
            delete_words(&labels, &both, &on(&[&[0], &[1]]), false),
            "All 2 are on an archive."
        );
        assert_eq!(
            delete_words(&labels, &both, &on(&[&[0], &[], &[]]), false),
            "2 of these are on no archive."
        );
        assert_eq!(
            delete_words(&labels, &both, &on(&[&[1]]), false),
            "It is on Cloud."
        );
        assert_eq!(
            delete_words(&labels, &both, &on(&[&[]]), false),
            "It is on no archive."
        );
        assert_eq!(
            delete_words(&labels, &[true, false], &on(&[&[], &[0]]), false),
            "1 of these is on no archive. Cloud did not answer, and was not looked at."
        );
        // Unknowns: never counted as on, and said.
        let mut some = on(&[&[0], &[], &[]]);
        some[1].unknown = vec![0];
        assert_eq!(
            delete_words(&labels, &both, &some, false),
            "1 of these is on no archive. 1 could not be told without reading them whole \
             (Back up's sheet reads them)."
        );
        let mut all = on(&[&[0], &[]]);
        all[1].unknown = vec![0];
        assert_eq!(
            delete_words(&labels, &both, &all, false),
            "1 could not be told without reading them whole (Back up's sheet reads them)."
        );
        // The archive's own frames: the other roots looked at, and the
        // words say so.
        let others = vec!["Laptop".to_string(), "Cloud".to_string()];
        assert_eq!(
            delete_words(&others, &both, &on(&[&[0], &[0]]), true),
            "All 2 are also on Laptop."
        );
        assert_eq!(
            delete_words(&others, &both, &on(&[&[]]), true),
            "It is on no other root."
        );
        assert_eq!(
            delete_words(&others, &both, &on(&[&[0], &[], &[1]]), true),
            "1 of these is on no other root."
        );
        assert_eq!(header_words(0, 0, "nas"), "All on nas");
        assert_eq!(header_words(0, 2, "nas"), "2 frames not known on nas");
        assert_eq!(
            header_words(3, 1, "nas"),
            "3 frames not on nas, 1 not known"
        );
    }

    use crate::testing::{state_for, window};
    use greycard_library::fixture::{A7, R5, R6, write_frame};

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-archives-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    /// Frames of their own: no two alike, in this test or another.
    fn frames(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        static SEED: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(100);
        std::fs::create_dir_all(dir).unwrap();
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let p = dir.join(n);
                let seed = SEED.fetch_add(1, Ordering::Relaxed);
                write_frame(&p, [&R5, &R6, &A7][i % 3], seed);
                p
            })
            .collect()
    }

    fn index(db: &Path, roots: &[&Path]) {
        let mut lib = greycard_library::Library::open(db).unwrap();
        for r in roots {
            lib.index_tree(r, &mut |_| {}).unwrap();
        }
    }

    /// A window over `files` with a library of two roots, `local` and
    /// `nas`, the second marked an archive, kept in a roots file of the
    /// test's own; the index at `db`.
    fn opened(
        app: &App,
        dir: &Path,
        files: Vec<PathBuf>,
        local: &Path,
        nas: &Path,
        db: &Path,
    ) -> (Rc<RefCell<State>>, Rc<Worker>) {
        let (state, worker) = state_for(app, files);
        {
            let mut st = state.borrow_mut();
            st.deletes_allowed = true;
            st.current = (!st.files.is_empty()).then_some(0);
            st.index_path = Some(db.to_path_buf());
            st.index_reader = Some(greycard_library::Library::open_read_only(db).unwrap());
            let file = dir.join("roots.json");
            let mut roots = greycard_library::Roots::default();
            roots.add(local).unwrap();
            roots.add(nas).unwrap();
            roots.set_archive(nas, true);
            roots.save(&file).unwrap();
            st.library.roots = roots;
            st.library.file = Some(file);
            crate::panel::browser::rebuild_browser(&mut st, app);
            show(&st, app);
        }
        (state, worker)
    }

    #[test]
    fn a_roots_archive_mark_is_kept_and_shown_on_its_chip() {
        let dir = scratch("mark");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let files = frames(&local.join("shoot"), &["a.tif"]);
        std::fs::create_dir_all(&nas).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local]);
        let app = window(1);
        let (state, _worker) = opened(&app, &dir, files, &local, &nas, &db);
        // Off, from the chip's menu, and on again.
        app.invoke_library_root_archive(nas.to_string_lossy().into_owned().into());
        assert!(!state.borrow().library.roots.is_archive(&nas));
        let text = std::fs::read_to_string(dir.join("roots.json")).unwrap();
        assert!(!text.contains("\"archives\""), "{text}");
        app.invoke_library_root_archive(nas.to_string_lossy().into_owned().into());
        assert!(state.borrow().library.roots.is_archive(&nas));
        let text = std::fs::read_to_string(dir.join("roots.json")).unwrap();
        assert!(text.contains("\"archives\""), "{text}");
        let chips: Vec<RootChip> = state.borrow().library.rows.iter().collect();
        assert_eq!(
            chips.iter().map(|c| c.archive).collect::<Vec<_>>(),
            [false, true]
        );
        assert!(app.get_status().contains("is an archive"));
        crate::testing::remove_scratch(state, &dir);
    }

    /// The whole of a backup through the window: the header's button,
    /// the sheet's words, the rejects' checkbox, the copy, the pairing
    /// remembered, the header's count after; and from the archive's
    /// view, Bring back finds the frames home already.
    #[test]
    fn a_backup_through_the_window_copies_remembers_and_counts() {
        let dir = scratch("backup");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot");
        let files = frames(&shoot, &["a.tif", "b.tif", "c.tif"]);
        frames(&shoot.join(crate::cull::REJECTS), &["r.tif"]);
        std::fs::create_dir_all(&nas).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(files.len());
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        assert_eq!(app.get_archive_header(), "Back up...");
        assert_eq!(
            app.get_archive_choices().iter().collect::<Vec<_>>(),
            ["nas"]
        );
        // The frame menu offers the same, over the selection.
        crate::panel::menu::menu_asked(&state, &app, 0);
        assert_eq!(app.get_archive_verb(), "Back up");

        app.invoke_archive_header_pressed(0);
        assert!(!app.get_archive_open(), "not until the look is in");
        assert_eq!(land_sent(&state, &app, &worker), 1);
        assert!(app.get_archive_open());
        assert_eq!(app.get_archive_title(), "Back up to nas");
        assert!(
            app.get_archive_text().starts_with("3 frames to copy"),
            "{}",
            app.get_archive_text()
        );
        assert_eq!(app.get_archive_rejects(), 1);
        assert_eq!(
            app.get_archive_dest(),
            nas.join("local").join("shoot").to_string_lossy()
        );
        // A folder not under the archive is refused before any look.
        app.set_archive_dest(dir.join("elsewhere").to_string_lossy().into_owned().into());
        app.invoke_archive_look_again();
        assert!(!app.get_archive_can_confirm());
        assert!(app.get_archive_dest_note().contains("has to be under nas"));
        assert_eq!(land_sent(&state, &app, &worker), 0);
        // Another folder under it, and the rejects asked for.
        let chosen = nas.join("clients").join("shoot");
        app.set_archive_dest(chosen.to_string_lossy().into_owned().into());
        app.set_archive_include_rejects(true);
        app.invoke_archive_look_again();
        assert_eq!(land_sent(&state, &app, &worker), 1);
        assert!(app.get_archive_text().starts_with("4 frames to copy"));
        assert_eq!(app.get_archive_confirm(), "Back up 4 frames");

        app.invoke_archive_answered(true);
        assert!(!app.get_archive_open());
        assert!(app.get_archive_running());
        // The copy, then the count it asks for.
        assert_eq!(land_sent(&state, &app, &worker), 2);
        assert!(!app.get_archive_running());
        let there = chosen.clone();
        for f in &files {
            assert!(there.join(f.file_name().unwrap()).is_file());
        }
        assert!(there.join(crate::cull::REJECTS).join("r.tif").is_file());
        let status = app.get_status();
        assert!(status.starts_with("backed up 4 frames to nas"), "{status}");
        // The folder chosen is kept as the folder's pairing (§219).
        let kept = greycard_library::Roots::load(&dir.join("roots.json")).unwrap();
        assert_eq!(kept.backup_folder(&shoot, &nas), Some(chosen.as_path()));
        // The header counts what is not on the archive: nothing now.
        assert_eq!(app.get_archive_header(), "All on nas");
        // The copies' rows carry the whole file's hash.
        let lib = greycard_library::Library::open_read_only(&db).unwrap();
        assert!(lib.whole_hash(&there.join("a.tif")).unwrap().is_some());
        drop(lib);

        // A new frame here: counted as not on the archive.
        let d_here = shoot.join("d.tif");
        write_frame(&d_here, &A7, 42);
        {
            let mut st = state.borrow_mut();
            st.files.push(d_here.clone());
            want_count(&mut st, &app);
        }
        assert_eq!(land_sent(&state, &app, &worker), 1);
        assert_eq!(app.get_archive_header(), "1 frame not on nas");

        // From the archive's own view: Bring back, the pairing unwound
        // to the root it came from, and every frame already home.
        index(&db, &[&nas]);
        crate::roots::open_view(
            &state,
            &app,
            &worker,
            View::Branch {
                root: nas.clone(),
                folder: there.clone(),
                deep: true,
            },
        );
        crate::roots::land_sent(&state, &app, &worker);
        land_sent(&state, &app, &worker);
        assert_eq!(app.get_archive_header(), "Bring back...");
        {
            let mut st = state.borrow_mut();
            st.picked.clear();
        }
        app.invoke_archive_header_pressed(0);
        assert_eq!(land_sent(&state, &app, &worker), 1);
        assert_eq!(app.get_archive_title(), "Bring back to local");
        assert_eq!(app.get_archive_dest(), shoot.to_string_lossy());
        assert!(
            app.get_archive_text().contains("3 frames already here"),
            "{}",
            app.get_archive_text()
        );
        assert!(!app.get_archive_can_confirm());
        app.invoke_archive_answered(false);
        assert!(!app.get_archive_open());
        crate::testing::remove_scratch(state, &dir);
    }

    /// The Delete sheet says whether the frames are on an archive, from
    /// the same lookup: a line, and the sheet as it was.
    #[test]
    fn the_delete_sheet_says_whether_the_frames_are_on_an_archive() {
        let dir = scratch("delete-line");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let files = frames(&local.join("shoot"), &["a.tif", "b.tif"]);
        std::fs::create_dir_all(nas.join("by hand")).unwrap();
        std::fs::copy(&files[0], nas.join("by hand").join("a-copy.tif")).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(files.len());
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        state.borrow_mut().picked = vec![0, 1];
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_open());
        assert_eq!(
            app.get_delete_archive(),
            "Looking for these on the archives..."
        );
        assert_eq!(land_sent(&state, &app, &worker), 1);
        assert_eq!(app.get_delete_archive(), "1 of these is on no archive.");
        assert!(app.get_delete_trash(), "a line, not a refusal");
        app.invoke_delete_answered(0);

        state.borrow_mut().picked = vec![0];
        app.invoke_delete_asked("selection".into());
        land_sent(&state, &app, &worker);
        assert_eq!(app.get_delete_archive(), "It is on nas.");
        app.invoke_delete_answered(0);
        crate::testing::remove_scratch(state, &dir);
    }

    /// All roots keeps the archive's own frames and leaves out its
    /// copies of the frames here; the archive's view shows all of it.
    #[test]
    fn all_roots_shows_the_archive_less_its_copies_of_local_frames() {
        let dir = scratch("all-roots");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let here = frames(&local.join("shoot"), &["a.tif", "b.tif"]);
        let old = frames(&nas.join("2019"), &["z.tif"]);
        let copy = nas.join("local").join("shoot").join("a.tif");
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::copy(&here[0], &copy).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(0);
        let (state, worker) = opened(&app, &dir, Vec::new(), &local, &nas, &db);
        crate::roots::open_view(&state, &app, &worker, View::Roots(None));
        crate::roots::land_sent(&state, &app, &worker);
        let mut listed = state.borrow().files.clone();
        listed.sort();
        let mut want = vec![here[0].clone(), here[1].clone(), old[0].clone()];
        want.sort();
        assert_eq!(listed, want);
        // The archive alone: every frame on it.
        crate::roots::open_view(&state, &app, &worker, View::Roots(Some(nas.clone())));
        crate::roots::land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().files.len(), 2);
        // Not an archive: All roots lists the copy too.
        app.invoke_library_root_archive(nas.to_string_lossy().into_owned().into());
        crate::roots::open_view(&state, &app, &worker, View::Roots(None));
        crate::roots::land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().files.len(), 4);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A window over a shoot of three frames, a rejects folder and a
    /// subfolder of exports, under `local`, with `nas` an archive.
    fn shoot_window(
        what: &str,
    ) -> (
        PathBuf,
        PathBuf,
        PathBuf,
        App,
        Rc<RefCell<State>>,
        Rc<Worker>,
    ) {
        let dir = scratch(what);
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot");
        let files = frames(&shoot, &["a.tif", "b.tif", "c.tif"]);
        frames(&shoot.join(crate::cull::REJECTS), &["r.tif"]);
        frames(&shoot.join("exports"), &["e.tif"]);
        std::fs::create_dir_all(&nas).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(files.len());
        let (state, worker) = opened(&app, &dir, files, &local, &nas, &db);
        (dir, local, nas, app, state, worker)
    }

    /// A copy that goes quiet is set aside through the window: its card
    /// down, said, no other copy started meanwhile, and its end said
    /// when it lands late, as what it was.
    #[test]
    fn a_copy_set_aside_holds_the_next_until_it_lands_late() {
        let (dir, _local, nas, app, state, worker) = shoot_window("aside");
        app.invoke_archive_header_pressed(0);
        assert_eq!(land_sent(&state, &app, &worker), 1);
        ASIDE_NEXT.with(|a| a.set(true));
        app.invoke_archive_answered(true);
        assert!(app.get_archive_running());
        assert_eq!(land_once(&state, &app, &worker), 1);
        assert!(!app.get_archive_running());
        assert!(
            app.get_status().contains("set aside"),
            "{}",
            app.get_status()
        );
        // Meanwhile, no other copy to that archive.
        app.invoke_archive_header_pressed(0);
        assert!(
            app.get_status().contains("has not come back from nas yet"),
            "{}",
            app.get_status()
        );
        assert!(!app.get_archive_open());
        // Another archive is not held by it.
        let cloud = dir.join("cloud");
        std::fs::create_dir_all(&cloud).unwrap();
        {
            let mut st = state.borrow_mut();
            crate::roots::edit_roots(&mut st, |r| {
                r.add(&cloud).unwrap();
                r.set_archive(&cloud, true);
            })
            .unwrap();
        }
        // (The copy set aside is still out: kept back while the look goes.)
        let held: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        app.invoke_archive_header_pressed(1);
        assert_eq!(land_once(&state, &app, &worker), 1);
        assert_eq!(app.get_archive_title(), "Back up to cloud");
        app.invoke_archive_answered(false);
        SENT.with(|s| s.borrow_mut().extend(held));
        // It lands, late, said as the backup it was: told to stop when
        // it was set aside, it began no frame. Then the next can go.
        land_once(&state, &app, &worker);
        let status = app.get_status();
        assert!(
            status.starts_with("the copy set aside earlier came back: backed up 0 frames to nas"),
            "{status}"
        );
        assert!(status.contains("canceled, 4 frames not copied"), "{status}");
        land_sent(&state, &app, &worker);
        app.invoke_archive_header_pressed(0);
        assert_eq!(land_sent(&state, &app, &worker), 1);
        assert!(app.get_archive_open());
        assert!(app.get_archive_text().starts_with("4 frames to copy"));
        assert!(!nas.join("local").exists());
        crate::testing::remove_scratch(state, &dir);
    }

    /// The header counts what a Back up of the folder would walk (the
    /// subfolders in, the rejects out), per archive: the first paired
    /// archive's on the button, each in its own menu item.
    #[test]
    fn the_header_counts_the_walk_per_archive() {
        let (dir, local, nas, app, state, worker) = shoot_window("counts");
        let cloud = dir.join("cloud");
        std::fs::create_dir_all(&cloud).unwrap();
        let shoot = local.join("shoot");
        // On nas already: a and the export.
        let on_nas = nas.join("local").join("shoot");
        std::fs::create_dir_all(on_nas.join("exports")).unwrap();
        std::fs::copy(shoot.join("a.tif"), on_nas.join("a.tif")).unwrap();
        std::fs::copy(
            shoot.join("exports").join("e.tif"),
            on_nas.join("exports").join("e.tif"),
        )
        .unwrap();
        index(&dir.join("library.sqlite"), &[&nas]);
        {
            let mut st = state.borrow_mut();
            crate::roots::edit_roots(&mut st, |r| {
                r.add(&cloud).unwrap();
                r.set_archive(&cloud, true);
                r.set_backup_folder(&local, &nas, &nas.join("local"));
                r.set_backup_folder(&local, &cloud, &cloud.join("local"));
            })
            .unwrap();
            want_count(&mut st, &app);
        }
        assert_eq!(land_sent(&state, &app, &worker), 1);
        // a, b, c and the export; the rejects' frame left out.
        assert_eq!(app.get_archive_header(), "2 frames not on nas");
        assert_eq!(
            app.get_archive_header_choices().iter().collect::<Vec<_>>(),
            [
                "Back up to nas (2 frames not on nas)...",
                "Back up to cloud (4 frames not on cloud)..."
            ]
        );
        crate::testing::remove_scratch(state, &dir);
    }

    /// Under `--no-sidecars` the sheet says the frames go without them.
    #[test]
    fn with_sidecars_off_the_sheet_says_so() {
        let (dir, _local, _nas, app, state, worker) = shoot_window("no-sidecars");
        state.borrow_mut().write_sidecars = false;
        app.invoke_archive_header_pressed(0);
        land_sent(&state, &app, &worker);
        let text = app.get_archive_text();
        assert!(text.starts_with("4 frames to copy, "), "{text}");
        assert!(!text.contains("with their sidecars"), "{text}");
        assert!(text.contains("Sidecars are off"), "{text}");
        app.invoke_archive_answered(false);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A copy whose thread ends with nothing to say lets go of the
    /// window: the card down, and the archive free for the next copy.
    #[test]
    fn a_copy_lost_lets_go_of_the_window() {
        let (dir, _local, _nas, app, state, worker) = shoot_window("lost");
        app.invoke_archive_header_pressed(0);
        land_sent(&state, &app, &worker);
        // Set aside first, then lost: nothing should hold on to it.
        ASIDE_NEXT.with(|a| a.set(true));
        app.invoke_archive_answered(true);
        let token = state.borrow().archive.run_token;
        SENT.with(|s| s.borrow_mut().clear());
        land_run(&state, &app, &worker, token, Heard::Aside);
        assert_eq!(state.borrow().archive.aside.len(), 1);
        land_run(&state, &app, &worker, token, Heard::Lost);
        assert!(state.borrow().archive.aside.is_empty());
        assert!(state.borrow().archive.running.is_none());
        assert!(!app.get_archive_running());
        assert!(app.get_status().contains("stopped without a report"));
        app.invoke_archive_header_pressed(0);
        assert_eq!(land_sent(&state, &app, &worker), 1);
        assert!(app.get_archive_open());

        // Lost while under way, never set aside: the card comes down too.
        app.invoke_archive_answered(true);
        let token = state.borrow().archive.run_token;
        SENT.with(|s| s.borrow_mut().clear());
        assert!(app.get_archive_running());
        land_run(&state, &app, &worker, token, Heard::Lost);
        assert!(!app.get_archive_running());
        assert!(state.borrow().archive.running.is_none());
        crate::testing::remove_scratch(state, &dir);
    }

    /// A folder's view opened, its lists and counts landed, nothing
    /// picked.
    fn open_branch(
        state: &Rc<RefCell<State>>,
        app: &App,
        worker: &Rc<Worker>,
        root: &Path,
        folder: &Path,
    ) {
        crate::roots::open_view(
            state,
            app,
            worker,
            View::Branch {
                root: root.to_path_buf(),
                folder: folder.to_path_buf(),
                deep: true,
            },
        );
        crate::roots::land_sent(state, app, worker);
        land_sent(state, app, worker);
        state.borrow_mut().picked.clear();
    }

    /// The sheet opened from the header and its look landed: the field.
    fn sheet_dest(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> String {
        app.invoke_archive_header_pressed(0);
        assert_eq!(land_sent(state, app, worker), 1);
        assert!(app.get_archive_open(), "{}", app.get_status());
        app.get_archive_dest().to_string()
    }

    /// §219 through the window: the mirror the first time; one Back up
    /// of a shoot to the archive's year folder (typed with a trailing
    /// separator, kept without), after which the next shoot from the
    /// root opens beside it by its own name and counts on the header;
    /// and the first shoot opens at its own pairing again.
    #[test]
    fn back_ups_default_is_the_folders_then_a_siblings_then_the_mirror() {
        let dir = scratch("defaults");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let (a, b) = (
            local.join("2026").join("shoot-a"),
            local.join("2026").join("shoot-b"),
        );
        let files = frames(&a, &["a.tif", "b.tif"]);
        frames(&b, &["c.tif"]);
        std::fs::create_dir_all(&nas).unwrap();
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(files.len());
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        assert_eq!(
            app.get_archive_header(),
            "Back up...",
            "no count before a pairing"
        );

        // The mirror, the path under the root kept.
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            nas.join("local")
                .join("2026")
                .join("shoot-a")
                .to_string_lossy()
        );
        let year = nas.join("2026");
        let typed = format!("{}/", year.join("shoot-a").display());
        app.set_archive_dest(typed.clone().into());
        app.invoke_archive_look_again();
        assert_eq!(land_sent(&state, &app, &worker), 1);
        app.invoke_archive_answered(true);
        land_sent(&state, &app, &worker);
        assert!(year.join("shoot-a").join("a.tif").is_file());
        let text = std::fs::read_to_string(dir.join("roots.json")).unwrap();
        assert!(!text.contains(&typed), "{text}");
        let kept = greycard_library::Roots::load(&dir.join("roots.json")).unwrap();
        assert_eq!(
            kept.backup_folder(&a, &nas).unwrap().to_string_lossy(),
            year.join("shoot-a").to_string_lossy()
        );
        assert_eq!(
            kept.backup_folder(&local, &nas),
            None,
            "the root is not paired"
        );

        // The next shoot: beside it, and counted, since a folder of its
        // root has been backed up.
        index(&db, &[&nas]);
        open_branch(&state, &app, &worker, &local, &b);
        assert_eq!(app.get_archive_header(), "1 frame not on nas");
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            year.join("shoot-b").to_string_lossy()
        );
        app.invoke_archive_answered(false);
        // The first again: its own pairing.
        open_branch(&state, &app, &worker, &local, &a);
        assert_eq!(app.get_archive_header(), "All on nas");
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            year.join("shoot-a").to_string_lossy()
        );
        app.invoke_archive_answered(false);
        crate::testing::remove_scratch(state, &dir);
    }

    /// §219's story: a shoot backed up to the archive's year folder
    /// itself. Bring back of the year folder, or of a folder of that
    /// shoot's under it, unwinds to the shoot; another shoot under the
    /// year folder, never backed up from here, is not that pairing's,
    /// and the sheet asks.
    #[test]
    fn bring_back_unwinds_only_the_pairing_that_holds_the_folder() {
        let dir = scratch("unwind");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot-a");
        let files = frames(&shoot, &["a.tif", "b.tif"]);
        frames(&shoot.join("exports"), &["e.tif"]);
        let year = nas.join("2026");
        frames(&year.join("other-shoot"), &["z.tif"]);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(files.len());
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        sheet_dest(&state, &app, &worker);
        app.set_archive_dest(year.to_string_lossy().into_owned().into());
        app.invoke_archive_look_again();
        assert_eq!(land_sent(&state, &app, &worker), 1);
        app.invoke_archive_answered(true);
        land_sent(&state, &app, &worker);
        assert!(year.join("a.tif").is_file());
        assert!(year.join("exports").join("e.tif").is_file());
        index(&db, &[&nas]);

        // The shoot never backed up from here: not the root's, asked.
        open_branch(&state, &app, &worker, &nas, &year.join("other-shoot"));
        assert_eq!(app.get_archive_header(), "Bring back...");
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            local.join("other-shoot").to_string_lossy()
        );
        assert!(
            app.get_archive_dest_note()
                .contains("was not backed up from local"),
            "{}",
            app.get_archive_dest_note()
        );
        app.invoke_archive_answered(false);
        // The year folder: the pairing's own destination, exact.
        open_branch(&state, &app, &worker, &nas, &year);
        assert_eq!(sheet_dest(&state, &app, &worker), shoot.to_string_lossy());
        assert!(
            app.get_archive_dest_note()
                .starts_with("Where it was backed up from")
        );
        app.invoke_archive_answered(false);
        // The shoot's own subfolder there: the shoot, its path kept.
        open_branch(&state, &app, &worker, &nas, &year.join("exports"));
        assert_eq!(sheet_dest(&state, &app, &worker), shoot.to_string_lossy());
        assert!(
            app.get_archive_text().contains("1 frame already here"),
            "{}",
            app.get_archive_text()
        );
        app.invoke_archive_answered(false);
        crate::testing::remove_scratch(state, &dir);
    }

    /// §219's story as the user hit it: the ROOT backed up once from its
    /// own view, the field changed to the archive's year folder. Bring
    /// back of another shoot under the year folder, never here, asks
    /// with `<root>/<name>` and never offers the root as home; and a Back
    /// up of a second shoot of the root, deeper down, defaults into the
    /// year folder by its own name, not with its path under the root.
    #[test]
    fn a_root_backed_up_to_the_year_folder_does_not_claim_the_year() {
        let dir = scratch("story");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let files = frames(&local.join("shoot-a"), &["a.tif"]);
        let second = local.join("2025").join("shoot-b");
        frames(&second, &["b.tif"]);
        let year = nas.join("2026");
        frames(&year.join("never-here"), &["z.tif"]);
        let db = dir.join("library.sqlite");
        index(&db, &[&local, &nas]);
        let app = window(files.len());
        let (state, worker) = opened(&app, &dir, files, &local, &nas, &db);
        crate::roots::open_view(&state, &app, &worker, View::Roots(Some(local.clone())));
        crate::roots::land_sent(&state, &app, &worker);
        land_sent(&state, &app, &worker);
        state.borrow_mut().picked.clear();
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            nas.join("local").to_string_lossy()
        );
        app.set_archive_dest(year.to_string_lossy().into_owned().into());
        app.invoke_archive_look_again();
        assert_eq!(land_sent(&state, &app, &worker), 1);
        app.invoke_archive_answered(true);
        land_sent(&state, &app, &worker);
        assert!(year.join("shoot-a").join("a.tif").is_file());
        let kept = greycard_library::Roots::load(&dir.join("roots.json")).unwrap();
        assert_eq!(kept.backup_folder(&local, &nas), Some(year.as_path()));
        index(&db, &[&nas]);

        // Bring back of a shoot under the year folder never here: asked.
        open_branch(&state, &app, &worker, &nas, &year.join("never-here"));
        let dest = sheet_dest(&state, &app, &worker);
        assert_eq!(dest, local.join("never-here").to_string_lossy());
        assert_ne!(dest, local.to_string_lossy());
        assert!(
            app.get_archive_dest_note()
                .contains("was not backed up from local"),
            "{}",
            app.get_archive_dest_note()
        );
        app.invoke_archive_answered(false);

        // Back up of a second shoot of the root: into the year folder,
        // by its own name.
        open_branch(&state, &app, &worker, &local, &second);
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            year.join("shoot-b").to_string_lossy()
        );
        app.invoke_archive_answered(false);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The sibling rule stands aside when the most recent source is under
    /// the folder backed up: a root's view after one of its shoots, or a
    /// selection whose common folder is above that shoot, opens at the
    /// mirror.
    #[test]
    fn a_folder_above_the_last_backed_up_opens_at_the_mirror() {
        let (dir, local, nas, app, state, worker) = shoot_window("above");
        let shoot = local.join("shoot");
        frames(&local.join("other"), &["o.tif"]);
        index(&dir.join("library.sqlite"), &[&local]);
        crate::roots::edit_roots(&mut state.borrow_mut(), |r| {
            r.set_backup_folder(&shoot, &nas, &nas.join("2026").join("shoot"))
        })
        .unwrap();
        // The root's view.
        crate::roots::open_view(&state, &app, &worker, View::Roots(Some(local.clone())));
        crate::roots::land_sent(&state, &app, &worker);
        land_sent(&state, &app, &worker);
        state.borrow_mut().picked.clear();
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            nas.join("local").to_string_lossy()
        );
        app.invoke_archive_answered(false);
        // A selection across the shoot and another folder: its common
        // folder is the root, above the shoot.
        let (in_shoot, in_other) = {
            let st = state.borrow();
            let at = |d: &Path| st.files.iter().position(|f| f.starts_with(d)).unwrap();
            (at(&shoot), at(&local.join("other")))
        };
        state.borrow_mut().picked = vec![in_shoot.min(in_other), in_shoot.max(in_other)];
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            nas.join("local").to_string_lossy()
        );
        app.invoke_archive_answered(false);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A pairing whose destination is no longer on the archive is
    /// dropped by the sheet's look and said once; the default is then
    /// settled without it. One whose source folder is no longer here is
    /// kept, and Bring back unwinds to it, the folder made again. An
    /// archive that lists nothing at all (a mount point with no share on
    /// it) drops nothing.
    #[test]
    fn a_pairing_whose_destination_is_gone_is_dropped_and_said_once() {
        let (dir, local, nas, app, state, worker) = shoot_window("gone");
        let shoot = local.join("shoot");
        let mirror = nas.join("local").join("shoot");
        let (moved, kept) = (nas.join("moved"), nas.join("kept"));
        std::fs::create_dir_all(&kept).unwrap();
        // The pairings the file keeps, and the window agrees.
        let pairings = |state: &Rc<RefCell<State>>| {
            let file = greycard_library::Roots::load(&dir.join("roots.json")).unwrap();
            assert_eq!(file.backups(), state.borrow().library.roots.backups());
            file.backups().len()
        };
        // The destination gone from the archive.
        crate::roots::edit_roots(&mut state.borrow_mut(), |r| {
            r.set_backup_folder(&shoot, &nas, &moved)
        })
        .unwrap();
        assert_eq!(sheet_dest(&state, &app, &worker), mirror.to_string_lossy());
        assert_eq!(
            app.get_status(),
            format!(
                "forgot that {} was backed up to {}: that folder is no longer on nas",
                shoot.display(),
                moved.display()
            )
        );
        assert_eq!(pairings(&state), 0);
        app.invoke_archive_answered(false);
        sheet_dest(&state, &app, &worker);
        assert_eq!(app.get_status(), "", "said once");
        app.invoke_archive_answered(false);

        // The source folder gone from here: kept, and nothing said. The
        // next shoot opens inside its destination, a container (§219).
        let vanished = local.join("vanished");
        let backed = frames(&kept, &["k.tif"]);
        crate::roots::edit_roots(&mut state.borrow_mut(), |r| {
            r.set_backup_folder(&vanished, &nas, &kept)
        })
        .unwrap();
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            kept.join("shoot").to_string_lossy()
        );
        assert_eq!(app.get_status(), "");
        assert_eq!(pairings(&state), 1);
        app.invoke_archive_answered(false);
        // Bring back of its destination unwinds to it, and the copy
        // makes the folder again.
        index(&dir.join("library.sqlite"), &[&nas]);
        open_branch(&state, &app, &worker, &nas, &kept);
        assert_eq!(
            sheet_dest(&state, &app, &worker),
            vanished.to_string_lossy()
        );
        assert!(
            app.get_archive_dest_note()
                .starts_with("Where it was backed up from")
        );
        assert_eq!(app.get_status(), "");
        app.invoke_archive_answered(true);
        land_sent(&state, &app, &worker);
        assert!(vanished.join(backed[0].file_name().unwrap()).is_file());
        assert_eq!(pairings(&state), 1);

        // An archive with nothing on it says nothing of its folders.
        crate::testing::remove_dir_retry(&kept);
        crate::roots::edit_roots(&mut state.borrow_mut(), |r| {
            r.set_backup_folder(&shoot, &nas, &moved)
        })
        .unwrap();
        open_branch(&state, &app, &worker, &local, &shoot);
        assert_eq!(sheet_dest(&state, &app, &worker), moved.to_string_lossy());
        assert_eq!(app.get_status(), "");
        assert_eq!(pairings(&state), 2);
        app.invoke_archive_answered(false);
        crate::testing::remove_scratch(state, &dir);
    }
}
