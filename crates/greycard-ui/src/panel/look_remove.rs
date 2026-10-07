//! Remove a look: the LOOK section's Remove button, a sheet that names
//! the look and every file of it that will go, and says how many
//! pictures in the open folder name it. A look is a name with up to one
//! table per display curve (§235), so the files are the look's own
//! `<name>.cube` and every `<name>.<curve>.cube` that declares its
//! curve, and nothing else: a film preset called `Neon.agx` is a look
//! of its own and stays when `Neon` goes.
//!
//! The files go to the system trash through `crate::delete`, which
//! hands the trash in as a function so no test ever touches the user's
//! trash; where there is none the sheet says so and deletes only on an
//! explicit click, as the delete sheet does. No sidecar is rewritten:
//! a picture that names a removed look shows it as "(missing)" and is
//! rendered without it, as one naming a look from another machine is.

use std::path::{Path, PathBuf};

use greycard_edit::look::{self, Entry};

use crate::delete::{self, Deleted, Doomed, How, Offer, TRASH_SUPPORTED};
use crate::panel::assets::show_looks;
use crate::*;

/// The sheet's question while it is up.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    pub(crate) name: String,
    /// The files to remove, the look's own first.
    pub(crate) files: Vec<PathBuf>,
    /// The look folder, the only place a file may go from.
    pub(crate) store: PathBuf,
    /// What the sheet offered; an answer it did not offer is a no.
    pub(crate) offer: Offer,
    /// Counts the questions, so a count of pictures that lands after
    /// the sheet moved on is dropped.
    #[cfg_attr(test, allow(dead_code))]
    generation: u64,
}

/// A removal handed to its thread.
pub(crate) struct Job {
    name: String,
    files: Vec<PathBuf>,
    how: How,
    allowed: Vec<PathBuf>,
}

/// What came back from the thread.
pub(crate) struct Done {
    name: String,
    how: How,
    total: usize,
    /// How many of the files are no longer in the folder.
    gone: usize,
    deleted: Deleted,
    panicked: Option<String>,
}

/// Where the looks are kept, which the tests point at a scratch folder
/// of their own.
pub(crate) fn store() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = crate::testing::LOOK_STORE.with(|s| s.borrow().clone()) {
        return Some(dir);
    }
    look::store_dir()
}

/// The files of the look named, as `looks` lists them: its own file
/// and each curve's table, those in `store` and nothing else.
pub(crate) fn files_of(looks: &[Entry], name: &str, store: &Path) -> Vec<PathBuf> {
    look::tables_of(looks, name)
        .into_iter()
        .map(|e| e.path.clone())
        .filter(|p| p.parent() == Some(store))
        .collect()
}

/// Whether the look the panel has chosen has files to remove: a link is
/// not one.
pub(crate) fn removable(looks: &[Entry], name: &str) -> bool {
    name != "none"
        && store().is_some_and(|dir| {
            files_of(looks, name, &dir)
                .iter()
                .any(|f| !std::fs::symlink_metadata(f).is_ok_and(|m| m.file_type().is_symlink()))
        })
}

/// How many pictures of the list name a look: those whose edit is in
/// memory, counted now, and the paths whose edit still stands in from
/// the index's row, which says nothing of the look and is read off the
/// window's thread.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Uses {
    pub(crate) named: usize,
    pub(crate) unread: Vec<PathBuf>,
}

/// The pictures of the open list that name `name`. The open picture is
/// asked of the panel's edit, which may be ahead of its sidecar.
pub(crate) fn uses(st: &State, name: &str) -> Uses {
    let mut out = Uses::default();
    for (i, file) in st.files.iter().enumerate() {
        if !crate::rows::is_loaded(st, i) {
            out.unread.push(file.clone());
            continue;
        }
        let edit = match (Some(i) == st.current, st.sidecars.get(i)) {
            (true, _) => &st.edit,
            (false, Some(s)) => &s.current,
            (false, None) => continue,
        };
        if edit.look_lut.lut.name() == name {
            out.named += 1;
        }
    }
    out
}

/// How many of the sidecars beside `paths` name the look, read from
/// disk: the pictures whose edits were not in memory.
pub(crate) fn named_on_disk(paths: &[PathBuf], name: &str) -> usize {
    paths
        .iter()
        .filter(|p| {
            matches!(
                greycard_edit::Sidecar::load(p),
                Ok(Some(s)) if s.current.look_lut.lut.name() == name
            )
        })
        .count()
}

/// The sheet's line on the pictures that name the look. `pending` is
/// how many are still being read; `place` is where the pictures are,
/// "in this folder" or "in the library".
pub(crate) fn uses_line(named: usize, pending: usize, place: &str) -> String {
    let mut out = match (named, pending) {
        (0, 0) => format!("No picture {place} uses it."),
        (0, _) => String::new(),
        (1, _) => format!(
            "1 picture {place} uses it; it will show the look as missing and render \
             without it."
        ),
        (n, _) => format!(
            "{n} pictures {place} use it; they will show the look as missing and render \
             without it."
        ),
    };
    if pending > 0 {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!(
            "Reading the edits of {pending} more picture{} to count them...",
            if pending == 1 { "" } else { "s" }
        ));
    }
    out
}

/// What the sheet says will happen, by what it offers.
fn offer_note(offer: Offer, files: usize) -> &'static str {
    let one = files == 1;
    match (offer.trash, offer.permanent) {
        (true, false) if one => "The file goes to the system trash, where it can be restored from.",
        (true, false) => "The files go to the system trash, where they can be restored from.",
        (true, true) => {
            "The trash refused files from the look folder earlier. Try the trash again, or \
             delete them permanently. A permanent delete cannot be undone."
        }
        _ if one => {
            "There is no system trash here, so the file will be deleted permanently. This \
             cannot be undone."
        }
        _ => {
            "There is no system trash here, so the files will be deleted permanently. This \
             cannot be undone."
        }
    }
}

/// Open the sheet for the look the panel has chosen, or say in the
/// status line why there is nothing to ask.
pub(crate) fn ask(st: &mut State, app: &App) {
    if st.look_removing {
        app.set_status("a look is still being removed".into());
        return;
    }
    let name = app.get_look_name().to_string();
    if name == "none" {
        app.set_status("no look is chosen to remove".into());
        return;
    }
    let Some(dir) = store() else {
        app.set_status("there is no look directory on this machine".into());
        return;
    };
    // The directory as it is now, not as it was when the section was
    // opened: a table added or moved since is the look's too.
    st.looks = list_in(Some(dir.clone()));
    let all = files_of(&st.looks, &name, &dir);
    if all.is_empty() {
        app.set_status(format!("{name} has no file in the look directory to remove").into());
        return;
    }
    // A link is never removed (the move refuses one, and a link's
    // target is not the look folder's): said, and left out.
    let (links, files): (Vec<PathBuf>, Vec<PathBuf>) = all
        .into_iter()
        .partition(|f| std::fs::symlink_metadata(f).is_ok_and(|m| m.file_type().is_symlink()));
    let link_names: Vec<String> = links
        .iter()
        .filter_map(|f| f.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    if files.is_empty() {
        app.set_status(
            format!(
                "{} {} a link and stays; nothing is removed through one",
                link_names.join(", "),
                if link_names.len() == 1 { "is" } else { "are" }
            )
            .into(),
        );
        return;
    }
    let canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let offer = delete::offer(TRASH_SUPPORTED, st.trash_refused.contains(&canonical));
    let counted = uses(st, &name);
    let names: Vec<slint::SharedString> = files
        .iter()
        .filter_map(|f| f.file_name())
        .map(|n| n.to_string_lossy().as_ref().into())
        .collect();
    let shown = look::tables_of(&st.looks, &name)
        .first()
        .map(|e| e.label().to_string())
        .unwrap_or_else(|| name.clone());
    let place = match st.view {
        crate::roots::View::Roots(_) => "in the library",
        _ => "in this folder",
    };
    let mut note = offer_note(offer, files.len()).to_string();
    let stays = !link_names.is_empty();
    let (title, uses) = if stays {
        // Something of the look stays, so pictures keep it: the sheet
        // removes some of its tables, not the look.
        note = format!(
            "{} {} a link and stays. {note}",
            link_names.join(", "),
            if link_names.len() == 1 { "is" } else { "are" }
        );
        (
            format!(
                "Remove {} of {shown}'s {} tables?",
                files.len(),
                files.len() + link_names.len()
            ),
            format!(
                "The look stays for pictures through {}, so none of them shows it as missing.",
                link_names.join(", ")
            ),
        )
    } else {
        (
            format!("Remove the look {shown}?"),
            uses_line(counted.named, counted.unread.len(), place),
        )
    };
    app.set_look_remove_title(title.into());
    app.set_look_remove_files(ModelRc::new(VecModel::from(names)));
    app.set_look_remove_uses(uses.into());
    app.set_look_remove_note(note.into());
    app.set_look_remove_trash(offer.trash);
    app.set_look_remove_permanent(offer.permanent);
    st.look_remove_generation += 1;
    let generation = st.look_remove_generation;
    st.look_remove = Some(Asked {
        name: name.clone(),
        files,
        store: dir,
        offer,
        generation,
    });
    app.set_look_remove_open(true);
    if !counted.unread.is_empty() && !stays {
        count_unread(app, generation, counted, name, place);
    }
}

/// Read the edits that stood in from the index on a thread of their
/// own, and put the count on the sheet when it is done.
#[cfg(not(test))]
fn count_unread(app: &App, generation: u64, counted: Uses, name: String, place: &'static str) {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard look uses".into())
        .spawn(move || {
            let on_disk = named_on_disk(&counted.unread, &name);
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                ) else {
                    return;
                };
                let st = state.borrow();
                if st.look_remove.as_ref().map(|a| a.generation) == Some(generation) {
                    app.set_look_remove_uses(uses_line(counted.named + on_disk, 0, place).into());
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("look uses: {e}");
    }
}

/// A test counts them itself, through [`named_on_disk`].
#[cfg(test)]
fn count_unread(_app: &App, _generation: u64, _counted: Uses, _name: String, _place: &str) {}

impl Job {
    /// The removal, `trash` the system trash for [`How::Trash`]: every
    /// file of the look in one call, as the delete does a frame's.
    pub(crate) fn run_with(self, trash: impl Fn(&[PathBuf]) -> Result<(), String>) -> Done {
        let total = self.files.len();
        let plan = delete::Plan {
            frames: self
                .files
                .first()
                .map(|first| Doomed {
                    frame: first.clone(),
                    sidecars: self.files[1..].to_vec(),
                })
                .into_iter()
                .collect(),
            refused: Vec::new(),
        };
        let deleted = delete::delete(&plan, self.how, &self.allowed, trash);
        let gone = self
            .files
            .iter()
            .filter(|f| std::fs::symlink_metadata(f).is_err())
            .count();
        Done {
            name: self.name,
            how: self.how,
            total,
            gone,
            deleted,
            panicked: None,
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    /// [`Job::run_with`] the system trash, a panic caught: the trash
    /// crate panics when it cannot set up COM on Windows.
    pub(crate) fn run_guarded(self) -> Done {
        let (name, how, files) = (self.name.clone(), self.how, self.files.clone());
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_with(delete::system_trash)
        })) {
            Ok(done) => done,
            Err(payload) => Done {
                name,
                how,
                total: files.len(),
                gone: files
                    .iter()
                    .filter(|f| std::fs::symlink_metadata(f).is_err())
                    .count(),
                deleted: Deleted::default(),
                panicked: Some(crate::worker::panic_message(payload.as_ref())),
            },
        }
    }
}

/// The sheet's answer: 0 no, 1 the trash, 2 permanently. An answer the
/// sheet did not offer is a no. A yes goes to a thread of its own, and
/// [`land`] finishes it.
pub(crate) fn answered(st: &mut State, app: &App, answer: i32) {
    app.set_look_remove_open(false);
    let Some(asked) = st.look_remove.take() else {
        return;
    };
    let how = match answer {
        1 if asked.offer.trash => How::Trash,
        2 if asked.offer.permanent => How::Permanent,
        _ => return,
    };
    if !st.deletes_allowed {
        let why = "not removed: a capture, an export or a timing run never deletes";
        tracing::info!("{why}");
        app.set_status(why.into());
        return;
    }
    if st.look_removing {
        app.set_status("a look is still being removed".into());
        return;
    }
    st.look_removing = true;
    app.set_status(format!("removing {}...", asked.name).into());
    let job = Job {
        name: asked.name,
        files: asked.files,
        how,
        allowed: delete::allowed(&[asked.store]),
    };
    if !send_off(app, job) {
        st.look_removing = false;
        app.set_status("the removal could not be started; nothing was removed".into());
    }
}

/// Run a removal on a thread of its own (the delete's reason: the
/// trash crate wants a thread with no COM set up before it). False
/// when it could not start.
#[cfg(not(test))]
fn send_off(app: &App, job: Job) -> bool {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard remove look".into())
        .spawn(move || {
            let done = job.run_guarded();
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                ) else {
                    return;
                };
                land(&mut state.borrow_mut(), &app, done);
            });
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("remove look: {e}");
            false
        }
    }
}

/// In a test the job is queued, and the test runs it through a fake
/// trash and lands it.
#[cfg(test)]
fn send_off(_app: &App, job: Job) -> bool {
    tests::SENT.with(|s| s.borrow_mut().push(job));
    true
}

/// What a removal did, in the status line's words.
fn summary(done: &Done) -> String {
    if let Some(why) = &done.panicked {
        return format!(
            "the removal of {} failed ({why}); {} of {} went before it did",
            done.name,
            done.gone,
            delete::count(done.total, "file")
        );
    }
    let how = match done.how {
        How::Trash => "moved to the trash",
        How::Permanent => "deleted permanently",
    };
    let mut out = if done.gone == done.total {
        format!(
            "removed {}: {} {how}",
            done.name,
            delete::count(done.total, "file")
        )
    } else {
        format!(
            "{}: {} of {} {how}",
            done.name,
            done.gone,
            delete::count(done.total, "file")
        )
    };
    if let Some(r) = &done.deleted.trash_refused {
        out.push_str(&format!(
            "; the trash refused {}: {}; ask again to delete permanently",
            // The own table when it is what was refused; else the
            // table that was left after it went.
            (if r.frame_gone { r.left.first() } else { None })
                .unwrap_or(&r.frame)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            r.why
        ));
    }
    if let Some((f, why)) = done.deleted.failed.first() {
        out.push_str(&format!(
            "; {} not removed: {why}",
            f.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
    }
    out
}

/// A removal is done: the look list read again, the viewport's cached
/// look and the tables kept in memory dropped, the status said. The
/// open picture that named the look shows it as "(missing)" and renders
/// without it, as it would one already missing; no sidecar is touched.
pub(crate) fn land(st: &mut State, app: &App, done: Done) {
    st.look_removing = false;
    if let Some(r) = &done.deleted.trash_refused
        && let Some(dir) = r.frame.parent()
    {
        let dir = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        st.trash_refused.insert(dir);
    }
    let status = summary(&done);
    tracing::info!("remove look: {status}");
    app.set_status(status.into());
    st.looks = list_in(store());
    look::forget();
    st.look_for = None;
    show_looks(st, &st.edit.look_lut, app);
    app.window().request_redraw();
}

/// The look list, from the store the sheet works on.
pub(crate) fn list_in(dir: Option<PathBuf>) -> Vec<Entry> {
    let Some(dir) = dir else {
        return Vec::new();
    };
    greycard_core::lut::list_dir(&dir)
        .into_iter()
        .filter_map(|(stem, path, info)| info.ok().map(|i| look::entry_of(&stem, path, i)))
        .collect()
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_remove_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ask(&mut state.borrow_mut(), &app);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_remove_answered(move |answer| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            answered(&mut state.borrow_mut(), &app, answer);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{LOOK_STORE, click, press, state_for, window};
    use slint::platform::Key;

    thread_local! {
        pub(super) static SENT: RefCell<Vec<Job>> = const { RefCell::new(Vec::new()) };
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-look-remove-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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

    /// A store holding Neon (per channel and AgX), a look of its own
    /// called `Neon.agx` that is no variant, and Mono.
    fn store_of(dir: &Path) -> PathBuf {
        let store = dir.join("looks");
        std::fs::create_dir_all(&store).unwrap();
        table(&store, "Neon.cube", &[]);
        table(&store, "Neon.agx.cube", &["display_curve: agx"]);
        table(&store, "Neon.agx.cube.keep", &[]);
        table(&store, "Mono.cube", &[]);
        store
    }

    /// The files a name's removal takes, as the sheet lists them.
    fn names_of(store: &Path, name: &str) -> Vec<String> {
        files_of(&list_in(Some(store.to_path_buf())), name, store)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn there(store: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(store)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// Every table of the look, and nothing else: the `.agx.cube` that
    /// declares its curve goes with `Neon`; one that does not is a look
    /// of its own and stays.
    #[test]
    fn the_sheets_files_are_every_table_of_the_look_and_nothing_else() {
        let dir = scratch("files");
        let store = store_of(&dir);
        assert_eq!(names_of(&store, "Neon"), ["Neon.cube", "Neon.agx.cube"]);
        assert_eq!(names_of(&store, "Mono"), ["Mono.cube"]);
        assert!(names_of(&store, "Absent").is_empty());
        // A film preset that is only called Neon.agx, with no
        // declaration in its header, is a look of its own.
        table(&store, "Neon.agx.cube", &[]);
        assert_eq!(names_of(&store, "Neon"), ["Neon.cube"]);
        assert_eq!(names_of(&store, "Neon.agx"), ["Neon.agx.cube"]);
        // A file whose folder is not the store is never listed.
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let stray = table(&elsewhere, "Stray.cube", &[]);
        let looks = list_in(Some(elsewhere.clone()));
        assert!(files_of(&looks, "Stray", &store).is_empty());
        assert_eq!(files_of(&looks, "Stray", &elsewhere), [stray]);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_line_counts_the_pictures_that_name_the_look() {
        assert_eq!(
            uses_line(0, 0, "in this folder"),
            "No picture in this folder uses it."
        );
        assert!(
            uses_line(1, 0, "in this folder")
                .starts_with("1 picture in this folder uses it; it will show")
        );
        assert!(
            uses_line(3, 0, "in this folder").starts_with(
                "3 pictures in this folder use it; they will show the look as missing"
            )
        );
        let pending = uses_line(2, 4, "in this folder");
        assert!(
            pending.contains("2 pictures") && pending.contains("4 more"),
            "{pending}"
        );
        assert!(
            uses_line(0, 4, "in this folder").starts_with("Reading the edits of 4 more pictures")
        );
    }

    /// The count, from the edits in memory: the open picture by the
    /// panel's edit, the others by their sidecars, and a frame whose
    /// edit stands in from its row apart, to be read.
    #[test]
    fn the_count_reads_the_edits_in_memory_and_sets_the_rest_apart() {
        let app = window(5);
        let (state, _worker) = state_for(&app, crate::testing::folder(5));
        let mut st = state.borrow_mut();
        let named = |n: &str| greycard_edit::look::LookLut {
            lut: greycard_edit::look::LutChoice::Named(n.into()),
            strength: 1.0,
        };
        st.sidecars[1].current.look_lut = named("Neon");
        st.sidecars[2].current.look_lut = named("Neon");
        st.sidecars[3].current.look_lut = named("Mono");
        st.current = Some(0);
        st.edit.look_lut = named("Neon");
        // Frame 4's edit stands in from its row.
        st.from_row[4].read = false;
        let counted = uses(&st, "Neon");
        assert_eq!(counted.named, 3);
        assert_eq!(counted.unread, [st.files[4].clone()]);
        assert_eq!(uses(&st, "Mono").named, 1);
        assert_eq!(uses(&st, "Absent").named, 0);
    }

    /// The pictures whose edit was not in memory are read from their
    /// sidecars, off the window's thread.
    #[test]
    fn the_unread_pictures_are_counted_from_their_sidecars() {
        let dir = scratch("unread");
        let frames: Vec<PathBuf> = (0..3).map(|i| dir.join(format!("f{i}.tif"))).collect();
        for (i, f) in frames.iter().enumerate() {
            std::fs::write(f, b"x").unwrap();
            let mut sidecar = greycard_edit::Sidecar::default();
            sidecar.current.look_lut = greycard_edit::look::LookLut {
                lut: greycard_edit::look::LutChoice::Named(
                    if i == 1 { "Mono" } else { "Neon" }.into(),
                ),
                strength: 1.0,
            };
            sidecar.save(f).unwrap();
        }
        assert_eq!(named_on_disk(&frames, "Neon"), 2);
        assert_eq!(named_on_disk(&frames, "Mono"), 1);
        assert_eq!(named_on_disk(&frames, "Absent"), 0);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The move, through a fake trash that takes the files and records
    /// the call: one call with the look's every file, the others left.
    #[test]
    fn the_move_hands_every_table_to_the_trash_in_one_call() {
        let dir = scratch("move");
        let store = store_of(&dir);
        let files = files_of(&list_in(Some(store.clone())), "Neon", &store);
        let taken = RefCell::new(Vec::new());
        let fake = |paths: &[PathBuf]| {
            for p in paths {
                std::fs::remove_file(p).map_err(|e| e.to_string())?;
            }
            taken.borrow_mut().push(paths.to_vec());
            Ok(())
        };
        let done = Job {
            name: "Neon".into(),
            files: files.clone(),
            how: How::Trash,
            allowed: delete::allowed(std::slice::from_ref(&store)),
        }
        .run_with(fake);
        assert_eq!(taken.borrow().len(), 1);
        let mut called = taken.borrow()[0].clone();
        called.sort();
        let mut want = files;
        want.sort();
        assert_eq!(called, want);
        assert_eq!((done.gone, done.total), (2, 2));
        assert_eq!(summary(&done), "removed Neon: 2 files moved to the trash");
        assert_eq!(there(&store), ["Mono.cube", "Neon.agx.cube.keep"]);

        // A trash that refuses leaves the files where they were, and the
        // permanent delete is not taken in its place.
        let dir2 = scratch("refused");
        let store = store_of(&dir2);
        let files = files_of(&list_in(Some(store.clone())), "Neon", &store);
        let done = Job {
            name: "Neon".into(),
            files,
            how: How::Trash,
            allowed: delete::allowed(std::slice::from_ref(&store)),
        }
        .run_with(|_| Err("no trash on this disk".into()));
        assert_eq!(done.gone, 0);
        assert!(
            summary(&done).contains("the trash refused"),
            "{}",
            summary(&done)
        );
        assert_eq!(names_of(&store, "Neon").len(), 2);
        crate::testing::remove_dir_retry(&dir2);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A file that has become a link, or is not in the store, is left
    /// where it is.
    #[test]
    fn a_file_outside_the_store_is_never_removed() {
        let dir = scratch("outside");
        let store = store_of(&dir);
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let stray = table(&elsewhere, "Stray.cube", &[]);
        let done = Job {
            name: "Stray".into(),
            files: vec![stray.clone()],
            how: How::Permanent,
            allowed: delete::allowed(&[store]),
        }
        .run_with(|_| Ok(()));
        assert!(stray.exists());
        assert_eq!(done.gone, 0);
        assert!(!done.deleted.failed.is_empty());
        crate::testing::remove_dir_retry(&dir);
    }

    /// The window: the Remove button is there for a look with files and
    /// not for None; the sheet lists the files and counts; Return asks
    /// for the trash once, the files go through the fake, the list
    /// reads again and the open picture shows the look missing; Escape
    /// closes the sheet and the window has its keys after.
    #[test]
    fn the_sheet_removes_a_look_and_takes_the_keys() {
        let dir = scratch("window");
        let store = store_of(&dir);
        LOOK_STORE.with(|s| *s.borrow_mut() = Some(store.clone()));
        let app = window(3);
        let (state, _worker) = state_for(&app, crate::testing::folder(3));
        {
            let mut st = state.borrow_mut();
            st.deletes_allowed = true;
            st.looks = list_in(Some(store.clone()));
            st.current = Some(0);
            st.edit.look_lut = greycard_edit::look::LookLut {
                lut: greycard_edit::look::LutChoice::Named("Neon".into()),
                strength: 1.0,
            };
            st.sidecars[1].current.look_lut = st.edit.look_lut.clone();
            show_looks(&st, &st.edit.look_lut, &app);
        }
        assert!(app.get_look_removable());
        assert_eq!(app.get_look_name(), "Neon");
        // The section's body grows to its rows over a moment.
        for _ in 0..4 {
            crate::testing::count_labeled(&app, "x");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
            slint::platform::update_timers_and_animations();
        }
        let remove = |app: &App| {
            let (at, size) = crate::testing::labeled(app, "Remove this look…");
            click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
            slint::platform::update_timers_and_animations();
            assert!(app.get_look_remove_open());
        };
        remove(&app);
        assert_eq!(app.get_look_remove_title(), "Remove the look Neon?");
        assert!(app.get_look_remove_uses().contains("in this folder"));
        let listed: Vec<String> = app
            .get_look_remove_files()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(listed, ["Neon.cube", "Neon.agx.cube"]);
        let uses = app.get_look_remove_uses();
        assert!(
            uses.starts_with("2 pictures in this folder use it; they will show"),
            "{uses}"
        );

        // Escape: no, nothing sent, and the window has its keys back.
        press(&app, Key::Escape);
        assert!(!app.get_look_remove_open());
        assert!(SENT.with(|s| s.borrow().is_empty()));
        let toggles = Rc::new(RefCell::new(0));
        let seen = toggles.clone();
        app.on_cull_toggled(move || *seen.borrow_mut() += 1);
        press(&app, "c");
        assert_eq!(*toggles.borrow(), 1);

        // Return: yes, once; a second Return reaches nothing.
        remove(&app);
        press(&app, Key::Return);
        assert!(!app.get_look_remove_open());
        press(&app, Key::Return);
        let jobs: Vec<Job> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        assert_eq!(jobs.len(), 1);
        for job in jobs {
            let done = job.run_with(|paths| {
                for p in paths {
                    std::fs::remove_file(p).map_err(|e| e.to_string())?;
                }
                Ok(())
            });
            land(&mut state.borrow_mut(), &app, done);
        }
        assert_eq!(there(&store), ["Mono.cube", "Neon.agx.cube.keep"]);
        assert!(
            app.get_status()
                .starts_with("removed Neon: 2 files moved to the trash"),
            "{}",
            app.get_status()
        );
        // The list reads again; the open picture still names Neon, which
        // the list shows as missing, and no sidecar was touched.
        let labels: Vec<String> = app
            .get_look_rows()
            .iter()
            .map(|r| r.label.to_string())
            .collect();
        assert!(labels.iter().any(|l| l == "Neon (missing)"), "{labels:?}");
        assert!(!app.get_look_removable());
        assert!(state.borrow().look_for.is_none());
        assert_eq!(state.borrow().edit.look_lut.lut.name(), "Neon");
        LOOK_STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A table added since the section was opened is listed and goes with
    /// the look: the sheet reads the folder again rather than trust the
    /// list it was handed.
    #[test]
    fn the_sheet_reads_the_folder_again() {
        let dir = scratch("stale");
        let store = store_of(&dir);
        LOOK_STORE.with(|s| *s.borrow_mut() = Some(store.clone()));
        let app = window(2);
        let (state, _worker) = state_for(&app, crate::testing::folder(2));
        // The list the panel holds is from before the AgX table.
        let before = store.join("Neon.agx.cube");
        let held = std::fs::read(&before).unwrap();
        std::fs::remove_file(&before).unwrap();
        state.borrow_mut().looks = list_in(Some(store.clone()));
        std::fs::write(&before, held).unwrap();
        app.set_look_name("Neon".into());
        ask(&mut state.borrow_mut(), &app);
        let listed: Vec<String> = app
            .get_look_remove_files()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(listed, ["Neon.cube", "Neon.agx.cube"]);
        LOOK_STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A table that is a link is not offered and is said to stay; a look
    /// of nothing but links has no sheet.
    #[cfg(unix)]
    #[test]
    fn a_link_is_said_to_stay_and_is_never_offered() {
        let dir = scratch("links");
        let store = store_of(&dir);
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let target = table(&elsewhere, "Target.cube", &[]);
        std::os::unix::fs::symlink(&target, store.join("Linky.cube")).unwrap();
        std::fs::remove_file(store.join("Neon.cube")).unwrap();
        std::os::unix::fs::symlink(&target, store.join("Neon.cube")).unwrap();
        LOOK_STORE.with(|s| *s.borrow_mut() = Some(store.clone()));
        let app = window(2);
        let (state, _worker) = state_for(&app, crate::testing::folder(2));
        app.set_look_name("Linky".into());
        ask(&mut state.borrow_mut(), &app);
        assert!(!app.get_look_remove_open());
        assert!(
            app.get_status().contains("Linky.cube is a link and stays"),
            "{}",
            app.get_status()
        );
        // Neon has a link and a real AgX table: the sheet offers the
        // one and says the other stays.
        app.set_look_name("Neon".into());
        ask(&mut state.borrow_mut(), &app);
        assert!(app.get_look_remove_open());
        let listed: Vec<String> = app
            .get_look_remove_files()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(listed, ["Neon.agx.cube"]);
        assert_eq!(app.get_look_remove_title(), "Remove 1 of Neon's 2 tables?");
        assert!(
            app.get_look_remove_uses()
                .starts_with("The look stays for pictures through Neon.cube"),
            "{}",
            app.get_look_remove_uses()
        );
        assert!(
            app.get_look_remove_note()
                .contains("The file goes to the system")
        );
        // A look of nothing but links has no button.
        let looks = list_in(Some(store.clone()));
        assert!(!removable(&looks, "Linky"));
        assert!(removable(&looks, "Neon"));
        assert!(
            app.get_look_remove_note()
                .starts_with("Neon.cube is a link and stays."),
            "{}",
            app.get_look_remove_note()
        );
        LOOK_STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A refused trash names the table that was refused: the variant when
    /// the look's own file went.
    #[test]
    fn a_refusal_names_the_table_refused() {
        let refusal = |frame_gone, left: Vec<PathBuf>| Done {
            name: "Neon".into(),
            how: How::Trash,
            total: 2,
            gone: 1,
            deleted: Deleted {
                trash_refused: Some(delete::Refusal {
                    frame: PathBuf::from("/looks/Neon.cube"),
                    why: "no trash".into(),
                    frame_gone,
                    was_gone: false,
                    left,
                    sidecars_gone: 0,
                }),
                ..Deleted::default()
            },
            panicked: None,
        };
        let own = summary(&refusal(false, Vec::new()));
        assert!(own.contains("the trash refused Neon.cube:"), "{own}");
        let variant = summary(&refusal(true, vec![PathBuf::from("/looks/Neon.agx.cube")]));
        assert!(
            variant.contains("the trash refused Neon.agx.cube:"),
            "{variant}"
        );
    }

    /// None has nothing to remove, and says so.
    #[test]
    fn none_cannot_be_removed() {
        let dir = scratch("none");
        let store = store_of(&dir);
        LOOK_STORE.with(|s| *s.borrow_mut() = Some(store));
        let app = window(2);
        let (state, _worker) = state_for(&app, crate::testing::folder(2));
        show_looks(&state.borrow(), &Default::default(), &app);
        assert!(!app.get_look_removable());
        ask(&mut state.borrow_mut(), &app);
        assert!(!app.get_look_remove_open());
        assert!(app.get_status().contains("no look is chosen"));
        LOOK_STORE.with(|s| *s.borrow_mut() = None);
        crate::testing::remove_dir_retry(&dir);
    }
}
