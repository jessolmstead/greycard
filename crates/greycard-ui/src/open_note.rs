//! A launch that dies opening a frame, and the next launch's way
//! around it.
//!
//! The settings remember the file last open, and a launch opens it
//! again. A frame that kills the process as it is put on screen (a
//! picture the GPU cannot hold once did, for every launch after, since
//! each opened it again) would then kill every launch, and reinstalling
//! leaves the settings where they were. So, as [`gpu_marker`] does for
//! a GPU that dies at setup, a launch leaves a note: before a frame's
//! first picture goes up it names the file in `<config>/greycard/
//! opening`, and takes it off once the view on screen has shown the
//! frame (see [`shown`]), the user has moved on to another frame, or
//! the window has closed. A note found at a launch names the file the last one
//! died opening; a launch that would open it again opens its folder in
//! the Library instead, says why in the log, and takes the note off, so
//! the file can be opened by hand.
//!
//! The note is written only as a frame's picture is new: the launch's
//! restored file, once the window's renderer is set up (a death
//! before is the GPU marker's, and no picture is up yet), and each
//! frame whose develop lands as the file open after another (as
//! `last_file` changes to it). A develop of the same frame again (a
//! slider) writes nothing. `last_file` changes without a note in one
//! place: a frame chosen in Cull (`startup::keep_last_file`), which
//! develops nothing, written once Cull's loupe has drawn its camera
//! picture, or at once from a sheet of tiles, which puts up none (its
//! camera picture fetched after, on threads that catch a panic), so
//! what the note guards is past for it or never comes; a launch that
//! restores it notes it as any restored file. A launch whose list lands
//! under the grid takes its note off at once only when no develop of
//! the restored frame is sent; in Develop that develop's picture,
//! drawn, takes it off. A lock beside the note is held for the session, so a
//! second greycard beside a live one finds the note live, and neither
//! honors it nor writes one. A note for another file than the one to
//! be restored is let go; a launch that restores none (a file named on
//! the command line, or a `last_file` that is gone) leaves it for the
//! launch that would.
//!
//! [`gpu_marker`]: crate::gpu_marker

use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// The note's name in the settings' folder.
const NOTE: &str = "opening";

/// The note, its lock, and what it says, for the session: none where
/// there is no settings folder, or where another greycard holds it.
struct Held {
    path: PathBuf,
    /// The lock, held until the process ends.
    _lock: std::fs::File,
    /// The file the note on disk names, while there is one.
    pending: Option<PathBuf>,
    /// Its picture has gone up and waits to be drawn.
    up: bool,
    /// The last file whose picture was drawn, so a develop of it again
    /// is not noted again.
    settled: Option<PathBuf>,
    /// The restored file, noted once the window's renderer is set up.
    restoring: Option<PathBuf>,
}

thread_local! {
    /// The window's thread's: the launch notes and takes off from
    /// there alone, and each test's thread has its own.
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

/// What [`decide`] found at a launch.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// No note: a launch like any other.
    Nothing,
    /// A note, and nothing restored to weigh it against (a file named
    /// on the command line, or a `last_file` that is gone): left as it
    /// is, for the launch that would restore it.
    Kept(PathBuf),
    /// A note for a file this launch does not open again, or one gone:
    /// let go.
    Stale(PathBuf),
    /// The last launch died opening `0`, which this launch was to open
    /// again.
    Died(PathBuf),
}

/// What a note naming `noted` means to a launch that would restore
/// `restoring`: the same file (by its canonical path where both have
/// one) is the one the last launch died opening; another is stale.
pub(crate) fn decide(noted: Option<&Path>, restoring: Option<&Path>) -> Found {
    let Some(noted) = noted else {
        return Found::Nothing;
    };
    match restoring {
        None => Found::Kept(noted.to_path_buf()),
        Some(r) if same_file(noted, r) => Found::Died(noted.to_path_buf()),
        Some(_) => Found::Stale(noted.to_path_buf()),
    }
}

/// Whether two paths name one file: by their canonical forms where both
/// have one, else as written.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The note's first line; the path follows it, as it is.
const HEADER: &[u8] =
    b"# greycard: a frame being opened; a launch that finds this died opening it\n";

/// A path as bytes, losslessly: the bytes the system has it as on Unix,
/// the UTF-16 units little-endian on Windows. Not trimmed or decoded,
/// so a name with spaces at its ends or not in UTF-8 comes back the
/// same.
fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

/// [`path_bytes`] read back.
fn path_of(bytes: &[u8]) -> Option<PathBuf> {
    if bytes.is_empty() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let (units, rest) = bytes.as_chunks::<2>();
        if !rest.is_empty() {
            return None;
        }
        let units: Vec<u16> = units.iter().map(|b| u16::from_le_bytes(*b)).collect();
        Some(PathBuf::from(std::ffi::OsString::from_wide(&units)))
    }
}

/// The file a note names: what follows its first line.
fn parse(note: &[u8]) -> Option<PathBuf> {
    path_of(note.strip_prefix(HEADER)?)
}

/// The note's bytes for `file`.
fn text(file: &Path) -> Vec<u8> {
    [HEADER, &path_bytes(file)].concat()
}

/// Write the note at `path` naming `file`. Not synced: what it guards
/// against is the process ending, and a process that ends leaves what
/// it wrote in the system's cache, which the next launch reads; a sync
/// would only add a wait to every frame change on the window's thread
/// for the whole machine going down, which the note does not try to
/// tell from a death of ours.
fn write(path: &Path, file: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text(file))
}

fn remove(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("open note {}: {e}", path.display()),
    }
}

/// What became of the lock beside the note.
enum Lock {
    Taken(std::fs::File),
    /// Another process holds it.
    Held,
    /// It could not be opened (a settings folder that cannot be
    /// written): the note can be neither trusted nor kept.
    Failed(std::io::Error),
}

/// The lock beside the note. A file system that cannot lock takes the
/// file as ours, as it would with no lock at all.
fn take_lock(note: &Path) -> Lock {
    let path = note.with_extension("lock");
    let opened = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
        });
    let file = match opened {
        Ok(file) => file,
        Err(e) => return Lock::Failed(e),
    };
    match file.try_lock() {
        Ok(()) => Lock::Taken(file),
        Err(std::fs::TryLockError::WouldBlock) => Lock::Held,
        Err(std::fs::TryLockError::Error(e)) => {
            tracing::warn!(
                "open note lock {}: {e}; taken as this launch's",
                path.display()
            );
            Lock::Taken(file)
        }
    }
}

/// [`begin_in`] with the device made at once, as the tests have it.
#[cfg(test)]
pub(crate) fn start_in(dir: &Path, restoring: Option<&Path>) -> Option<PathBuf> {
    let skip = begin_in(dir, restoring);
    device_made();
    skip
}

/// The note in `dir` at a launch that would restore `restoring`: the
/// lock taken for the session, a note found read and let go, and,
/// unless the last launch died opening it, the restored file kept to
/// be noted once the window's renderer is set up ([`device_made`]),
/// before it opens: a death before that is the GPU marker's
/// (`gpu_marker`), and the two never both count one. The file not to
/// open again, where the last launch died opening it; none otherwise,
/// and none where another greycard holds the note.
fn begin_in(dir: &Path, restoring: Option<&Path>) -> Option<PathBuf> {
    let path = dir.join(NOTE);
    let lock = match take_lock(&path) {
        Lock::Taken(file) => file,
        Lock::Held => {
            tracing::info!(
                "open note {}: another greycard holds it; left alone",
                path.display()
            );
            return None;
        }
        Lock::Failed(e) => {
            tracing::warn!(
                "open note {}: its lock cannot be made ({e}); no note this launch",
                path.display()
            );
            return None;
        }
    };
    let noted = std::fs::read(&path).ok().and_then(|t| parse(&t));
    let found = decide(noted.as_deref(), restoring);
    let mut held = Held {
        path: path.clone(),
        _lock: lock,
        pending: None,
        up: false,
        settled: None,
        restoring: None,
    };
    let skip = match found {
        Found::Nothing => None,
        Found::Kept(file) => {
            tracing::info!(
                "open note: {} named; this launch restores nothing, so it is left",
                file.display()
            );
            None
        }
        Found::Stale(file) => {
            tracing::info!(
                "open note: {} named, which this launch does not open again; let go",
                file.display()
            );
            remove(&path);
            None
        }
        Found::Died(file) => {
            tracing::warn!(
                "the last launch died opening {}; not opened again: its folder is shown in the \
                 Library, and it opens by hand as any frame does (note {})",
                file.display(),
                path.display()
            );
            remove(&path);
            Some(file)
        }
    };
    if skip.is_none() {
        held.restoring = restoring.map(Path::to_path_buf);
    }
    HELD.with(|h| *h.borrow_mut() = Some(held));
    skip
}

/// [`begin_in`] in the settings' folder; nothing where there is none,
/// and nothing for a run the command line owns (`scripted`: a capture,
/// a key or menu script, a timing run): such a run neither reads the
/// user's note, nor takes it off, nor writes one, so it cannot use up
/// a note meant for the next launch by hand, hang waiting on a frame
/// the note kept from it, or leave a note of its own that keeps the
/// user's file from them.
pub(crate) fn start(restoring: Option<&Path>, scripted: bool) -> Option<PathBuf> {
    if scripted {
        tracing::debug!("open note: a run the command line owns; not read or written");
        return None;
    }
    let dir = crate::settings::path()?.parent()?.to_path_buf();
    begin_in(&dir, restoring)
}

/// The window's renderer is set up: the restored file noted, once,
/// unless a develop has noted or drawn a frame already.
pub(crate) fn device_made() {
    with(|held| {
        if let Some(file) = held.restoring.take()
            && held.pending.is_none()
            && held.settled.is_none()
        {
            note(held, &file);
        }
    });
}

/// Whether the command line owns this run, for [`start`]: a capture
/// (`--snapshot`, `--screenshot`), a timing run, or a script of keys,
/// a press, a menu, or a turn.
pub(crate) fn scripted(cli: &crate::Cli) -> bool {
    crate::panel::startup::scripted_start(cli)
        || cli.keys.is_some()
        || cli.press.is_some()
        || cli.menu.is_some()
        || cli.turn.is_some()
        || cli.cull_key.is_some()
        || cli.snapshot_placeholder
}

fn note(held: &mut Held, file: &Path) {
    match write(&held.path, file) {
        Ok(()) => {
            held.pending = Some(file.to_path_buf());
            held.up = false;
        }
        Err(e) => tracing::warn!("open note {}: {e}", held.path.display()),
    }
}

fn with(f: impl FnOnce(&mut Held)) {
    HELD.with(|h| {
        if let Some(held) = h.borrow_mut().as_mut() {
            f(held);
        }
    });
}

/// A develop of `file` landed, its picture to go up on the next frame:
/// noted, unless it is the file noted already or drawn last.
pub(crate) fn opening(file: &Path) {
    with(|held| {
        let known = |p: &Option<PathBuf>| p.as_deref().is_some_and(|p| p == file);
        if known(&held.pending) || known(&held.settled) {
            return;
        }
        note(held, file);
    });
}

/// The noted file's picture went up on the GPU.
pub(crate) fn put_up() {
    with(|held| {
        if held.pending.is_some() {
            held.up = true;
        }
    });
}

/// A frame was drawn with a developed picture on screen: a picture
/// put up since the note is drawn, and the note goes.
pub(crate) fn drawn() {
    with(|held| {
        if held.up
            && let Some(file) = held.pending.take()
        {
            remove(&held.path);
            held.up = false;
            held.settled = Some(file);
        }
    });
}

/// The view on screen is past what the note guards for the noted
/// frame, whatever went up of it: the Cull loupe drew it (its camera
/// picture, reduced to fit the device if need be), or the Library's
/// grid or lightbox is what shows, whose pictures are thumbnails and
/// never the frame's full picture. The note goes, and a develop of the
/// frame landing later writes none.
pub(crate) fn shown() {
    with(|held| {
        if let Some(file) = held.pending.take() {
            remove(&held.path);
            held.up = false;
            held.settled = Some(file);
        }
    });
}

/// Another frame than the noted one is now open: the user moved on
/// before its picture was drawn, and the note goes.
pub(crate) fn moved_to(file: &Path) {
    with(|held| {
        if held.pending.as_deref().is_some_and(|p| !same_file(p, file)) {
            remove(&held.path);
            held.pending = None;
            held.up = false;
        }
    });
}

/// The window closed as it closes: nothing died opening.
pub(crate) fn quit() {
    with(|held| {
        if held.pending.take().is_some() {
            remove(&held.path);
        }
    });
}

/// Let go of the session's note and its lock: for the tests, which
/// start one launch after another in a process.
#[cfg(test)]
pub(crate) fn forget() {
    HELD.with(|h| *h.borrow_mut() = None);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test's scratch folder, removed when the test ends, a failing
    /// one too.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            HELD.with(|h| *h.borrow_mut() = None);
            crate::testing::remove_dir_retry(&self.0);
        }
    }

    fn noted(dir: &Path) -> Option<PathBuf> {
        std::fs::read(dir.join(NOTE)).ok().and_then(|t| parse(&t))
    }

    /// A frame and the folder the settings live in, under a scratch
    /// folder.
    fn setup(name: &str) -> (Scratch, PathBuf, PathBuf) {
        let root = crate::testing::scratch_dir(&format!("open-note-{name}"));
        let config = root.join("config");
        let frame = root.join("pictures").join("wide.jpg");
        std::fs::create_dir_all(frame.parent().unwrap()).unwrap();
        std::fs::write(&frame, b"not a picture").unwrap();
        (Scratch(root), config, frame)
    }

    /// A launch that died opening its restored frame: the next does not
    /// open it, takes the note off, and the launch after that opens it
    /// again and notes it.
    #[test]
    fn a_launch_that_died_opening_its_frame_is_not_followed_into_it() {
        let (_root, config, frame) = setup("died");
        // The first launch notes it, and dies before it is drawn.
        assert_eq!(start_in(&config, Some(&frame)), None);
        assert_eq!(noted(&config), Some(frame.clone()));
        forget();
        // The next: not opened, the note gone.
        assert_eq!(start_in(&config, Some(&frame)), Some(frame.clone()));
        assert_eq!(noted(&config), None, "taken off for the next");
        forget();
        // The one after, the file opened by hand or restored, as any.
        assert_eq!(start_in(&config, Some(&frame)), None);
        assert_eq!(noted(&config), Some(frame.clone()));
        forget();
    }

    /// A launch that dies setting up the GPU, before the window's device
    /// is made, leaves no note: that death is the GPU marker's, and the
    /// next launch opens the frame again.
    #[test]
    fn a_death_before_the_device_is_made_leaves_no_note() {
        let (_root, config, frame) = setup("before-device");
        assert_eq!(begin_in(&config, Some(&frame)), None);
        assert_eq!(noted(&config), None, "not before the device");
        forget();
        assert_eq!(begin_in(&config, Some(&frame)), None, "opened again");
        device_made();
        assert_eq!(noted(&config), Some(frame.clone()), "noted once made");
        // A device made again (after a lost one) notes nothing anew.
        shown();
        device_made();
        assert_eq!(noted(&config), None);
        forget();
    }

    /// A frame drawn takes the note off, and the next launch opens it;
    /// a develop of it again writes no note; another frame's landing
    /// does, and its drawing takes it off.
    #[test]
    fn a_frame_drawn_takes_the_note_off() {
        let (_root, config, frame) = setup("drawn");
        assert_eq!(start_in(&config, Some(&frame)), None);
        // The develop lands, as the restored file.
        opening(&frame);
        assert_eq!(noted(&config), Some(frame.clone()));
        // Drawn before it went up: not yet.
        drawn();
        assert_eq!(noted(&config), Some(frame.clone()));
        put_up();
        drawn();
        assert_eq!(noted(&config), None, "drawn");
        // A slider's develop of it again: nothing written.
        opening(&frame);
        assert_eq!(noted(&config), None);
        // Another frame lands, goes up, and is drawn.
        let other = frame.with_file_name("other.jpg");
        std::fs::write(&other, b"x").unwrap();
        opening(&other);
        assert_eq!(noted(&config), Some(other.clone()));
        put_up();
        drawn();
        assert_eq!(noted(&config), None);
        forget();
        assert_eq!(start_in(&config, Some(&other)), None, "opened again");
        forget();
    }

    /// A window closed during a slow open, and a frame left for another
    /// before it was drawn, take the note off: neither died.
    #[test]
    fn a_clean_quit_or_a_move_mid_open_takes_the_note_off() {
        let (_root, config, frame) = setup("quit");
        assert_eq!(start_in(&config, Some(&frame)), None);
        put_up();
        quit();
        assert_eq!(noted(&config), None, "the window closed");
        forget();
        assert_eq!(start_in(&config, Some(&frame)), None, "opened again");
        let other = frame.with_file_name("other.jpg");
        moved_to(&frame);
        assert_eq!(noted(&config), Some(frame.clone()), "still on it");
        moved_to(&other);
        assert_eq!(noted(&config), None, "moved on");
        forget();
    }

    /// A launch that restores nothing (its `last_file` gone, or a file
    /// named on the command line) leaves a note as it is and opens as
    /// it would; one that restores another file than the note's lets
    /// the note go.
    #[test]
    fn a_note_for_a_missing_or_other_file_trips_nothing() {
        let (_root, config, frame) = setup("missing");
        let gone = frame.with_file_name("gone.jpg");
        write(&config.join(NOTE), &gone).unwrap();
        // The settings' `last_file` is gone too: nothing restored.
        assert_eq!(start_in(&config, None), None);
        assert_eq!(noted(&config), Some(gone.clone()), "left for its launch");
        forget();
        write(&config.join(NOTE), &gone).unwrap();
        assert_eq!(start_in(&config, Some(&frame)), None, "another file");
        assert_eq!(
            noted(&config),
            Some(frame.clone()),
            "the restored one noted"
        );
        forget();
    }

    /// A second greycard beside a live one finds the note held: it
    /// neither honors it nor takes it off, and writes none of its own.
    #[test]
    fn a_second_instance_leaves_the_note_alone() {
        let (_root, config, frame) = setup("second");
        // The live one, its frame noted and not drawn yet.
        assert_eq!(start_in(&config, Some(&frame)), None);
        let live = HELD.with(|h| h.borrow_mut().take());
        // The second, in the same process with its own lock attempt.
        assert_eq!(start_in(&config, Some(&frame)), None, "not taken as died");
        assert_eq!(noted(&config), Some(frame.clone()), "left alone");
        opening(&frame.with_file_name("other.jpg"));
        assert_eq!(noted(&config), Some(frame.clone()), "none of its own");
        drop(live);
        forget();
    }

    /// The launch's list whose frame to open is the one the last launch
    /// died opening opens in the Library with no frame open, and not
    /// from the rendering setup either; once, so the next list opens
    /// its frame as any does.
    #[test]
    fn a_list_whose_frame_died_opening_opens_in_the_library_without_it() {
        use crate::testing::{folder, state_for, window};
        let app = window(4);
        let files = folder(4);
        let (state, worker) = state_for(&app, files.clone());
        let held = |files: &[PathBuf]| -> Vec<crate::rows::Held> {
            files
                .iter()
                .map(|f| crate::rows::from_disk(f, None, Default::default()))
                .collect()
        };
        state.borrow_mut().launch.not_reopened = Some(files[2].clone());
        app.set_grid_open(false);
        crate::panel::browser::open_loaded(
            &state,
            &app,
            &worker,
            files.clone(),
            held(&files),
            true,
            2,
        );
        {
            let st = state.borrow();
            assert!(app.get_grid_open(), "the Library");
            assert_eq!(st.current, None, "nothing open");
            assert_eq!(st.launch.select_at_start, None, "nor left for the setup");
            assert_eq!(st.launch.not_reopened, None, "once");
        }
        // The next list opens its frame as before.
        app.set_grid_open(false);
        crate::panel::browser::open_loaded(
            &state,
            &app,
            &worker,
            files.clone(),
            held(&files),
            true,
            2,
        );
        assert_eq!(state.borrow().launch.select_at_start, Some(2));
        assert!(!app.get_grid_open());
        // A note for another frame than the list's opens the list's.
        state.borrow_mut().launch.not_reopened = Some(files[3].clone());
        crate::panel::browser::open_loaded(
            &state,
            &app,
            &worker,
            files.clone(),
            held(&files),
            true,
            1,
        );
        assert_eq!(state.borrow().launch.select_at_start, Some(1));
        assert!(!app.get_grid_open());
    }

    /// A run the command line owns neither honors a note, nor takes it
    /// off, nor writes one, whether it restores the noted file or
    /// another; and the frames it lands write nothing either. The
    /// flags that own a run are the captures, the scripts, and the
    /// timing runs; a plain launch and one naming a file are not.
    #[test]
    fn a_run_the_command_line_owns_leaves_the_note_alone() {
        let (_root, config, frame) = setup("scripted");
        crate::settings::use_file(Some(config.join("settings.json")));
        let other = frame.with_file_name("other.jpg");
        std::fs::write(&other, b"x").unwrap();
        // The user's note, planted as a death would leave it.
        write(&config.join(NOTE), &frame).unwrap();
        assert_eq!(start(Some(&frame), true), None, "not honored");
        assert_eq!(noted(&config), Some(frame.clone()), "not used up");
        opening(&other);
        put_up();
        drawn();
        quit();
        assert_eq!(noted(&config), Some(frame.clone()), "nothing written");
        forget();
        // No note, a capture restoring the user's last file and killed
        // before its first draw: none left behind.
        std::fs::remove_file(config.join(NOTE)).unwrap();
        assert_eq!(start(Some(&frame), true), None);
        opening(&frame);
        assert_eq!(noted(&config), None, "a capture writes no note");
        forget();
        // A launch by hand honors it as before.
        write(&config.join(NOTE), &frame).unwrap();
        assert_eq!(start(Some(&frame), false), Some(frame.clone()));
        forget();
        crate::settings::use_file(None);
        let cli = |args: &[&str]| {
            let all = std::iter::once("greycard-ui").chain(args.iter().copied());
            scripted(&<crate::Cli as clap::Parser>::try_parse_from(all).unwrap())
        };
        for owned in [
            &["--snapshot", "a.png"][..],
            &["--screenshot", "a.png"],
            &["--keys", "d"],
            &["--press", "d"],
            &["--menu", "1"],
            &["--turn", "1"],
            &["--time-select", "4"],
        ] {
            assert!(cli(owned), "{owned:?}");
        }
        assert!(!cli(&[]), "a plain launch");
        assert!(!cli(&["some/file.jpg"]), "a file named");
        assert!(!cli(&["--grid"]), "a view asked for");
    }

    /// A path comes back from the note as it went in: spaces at its
    /// ends, a newline in it, and (where the file system takes them)
    /// bytes that are not UTF-8.
    #[test]
    fn a_noted_path_comes_back_byte_for_byte() {
        let (_root, config, frame) = setup("lossless");
        let dir = frame.parent().unwrap();
        // Windows takes no name with a newline or a space at its end.
        #[cfg(not(unix))]
        let names = vec![dir.join(" spaced out.jpg"), dir.join("été.jpg")];
        #[cfg(unix)]
        let names = {
            use std::os::unix::ffi::OsStrExt;
            let mut names = vec![dir.join(" spaced .jpg "), dir.join("two\nlines.jpg")];
            // APFS refuses a name that is not UTF-8.
            if !cfg!(target_os = "macos") {
                names.push(dir.join(std::ffi::OsStr::from_bytes(b"latin-\xe9t\xe9.jpg")));
            }
            names
        };
        for name in names {
            std::fs::write(&name, b"x").unwrap();
            assert_eq!(start_in(&config, Some(&name)), None);
            assert_eq!(noted(&config).as_deref(), Some(name.as_path()), "{name:?}");
            forget();
            assert_eq!(
                start_in(&config, Some(&name)),
                Some(name.clone()),
                "{name:?}"
            );
            forget();
        }
    }

    /// A settings folder the note cannot be made in: no note, said as
    /// that, and the launch opens as it would.
    #[test]
    fn a_folder_that_cannot_hold_the_note_is_said_and_skipped() {
        let (_root, config, frame) = setup("unwritable");
        // A file where the folder would be.
        std::fs::write(&config, b"not a folder").unwrap();
        assert_eq!(start_in(&config, Some(&frame)), None);
        assert!(HELD.with(|h| h.borrow().is_none()), "nothing held");
        opening(&frame);
        assert!(config.is_file(), "nothing written");
    }

    /// A launch's frame through the window's own wiring, in Develop:
    /// its develop landing (`deliver` -> `remember_last_file`) notes it,
    /// its picture going up on the next frame (`land_pending` ->
    /// `picture_up`) marks it up, and the develop view's draw
    /// (`drawn`, which the window calls after handing its texture to
    /// Slint) takes the note off. And in the Library over Cull: the
    /// list opening in the grid takes the restored frame's note off at
    /// once; over Develop it stays for that frame's develop. Skipped
    /// without a GPU for the picture to go up on.
    #[test]
    fn a_launch_in_develop_or_the_library_leaves_no_note_once_settled() {
        use crate::testing::{state_for, window};
        let (_root, config, frame) = setup("wired");
        crate::settings::use_file(Some(config.join("settings.json")));
        let other = frame.with_file_name("other.jpg");
        std::fs::write(&other, b"x").unwrap();
        let files = vec![frame.clone(), other.clone()];
        let held = |files: &[PathBuf]| -> Vec<crate::rows::Held> {
            files
                .iter()
                .map(|f| crate::rows::from_disk(f, None, Default::default()))
                .collect()
        };

        // The Library: the restored frame noted at the start. Over
        // Cull, which develops nothing, the list opening in the grid
        // takes it off; over Develop the frame's develop runs under the
        // grid, and its note stays on until that picture is drawn.
        for cull in [true, false] {
            let app = window(2);
            let (state, worker) = state_for(&app, files.clone());
            if cull {
                let mut st = state.borrow_mut();
                st.current = Some(1);
                crate::panel::cull::enter_cull(&mut st, &app, 1);
            }
            assert_eq!(start_in(&config, Some(&frame)), None);
            assert_eq!(noted(&config), Some(frame.clone()));
            app.set_grid_open(true);
            crate::panel::browser::open_loaded(
                &state,
                &app,
                &worker,
                files.clone(),
                held(&files),
                true,
                0,
            );
            if cull {
                assert_eq!(noted(&config), None, "the Library shows no full picture");
            } else {
                assert_eq!(noted(&config), Some(frame.clone()), "its develop runs");
            }
            forget();
        }

        // Develop.
        let Some((device, queue)) = greycard_viewport::testing::device("the open note in Develop")
        else {
            crate::settings::use_file(None);
            return;
        };
        let app = window(2);
        let (state, _worker) = state_for(&app, files.clone());
        struct Unload(std::rc::Rc<crate::Shared>);
        impl Drop for Unload {
            fn drop(&mut self) {
                if let Ok(mut st) = self.0.try_borrow_mut() {
                    st.develop.renderer = None;
                }
            }
        }
        let _unload = Unload(state.clone());
        assert_eq!(start_in(&config, None), None);
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.develop.renderer = Some(crate::render::Renderer::new(&device, &queue));
        }
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::Developed {
                generation,
                turn: 0,
                kind: crate::finish::Source::SCENE,
                image: crate::worker::Developed::Halves(std::sync::Arc::new(
                    crate::worker::Halves {
                        width: 60,
                        height: 40,
                        pixels: vec![Default::default(); 60 * 40 * 4],
                    },
                )),
                guide: std::sync::Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.1,
                sharpen: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
        let canonical = std::fs::canonicalize(&frame).unwrap();
        assert_eq!(
            noted(&config),
            Some(canonical),
            "noted as its develop lands"
        );
        // Drawn before its picture went up: not yet.
        drawn();
        assert!(noted(&config).is_some(), "not before it went up");
        crate::panel::startup::land_pending(&mut state.borrow_mut(), &app);
        assert!(noted(&config).is_some(), "up, not drawn yet");
        drawn();
        assert_eq!(noted(&config), None, "drawn");
        forget();
        crate::settings::use_file(None);
    }

    /// Two spellings of one file are the one file.
    #[test]
    fn a_file_by_another_spelling_is_the_same_file() {
        let (_root, _, frame) = setup("spelling");
        let dotted = frame.parent().unwrap().join(".").join("wide.jpg");
        assert!(same_file(&frame, &dotted));
        assert_eq!(
            decide(Some(&dotted), Some(&frame)),
            Found::Died(dotted.clone())
        );
        assert!(matches!(decide(Some(&dotted), None), Found::Kept(_)));
        let other = frame.with_file_name("other.jpg");
        assert!(matches!(
            decide(Some(&dotted), Some(&other)),
            Found::Stale(_)
        ));
        assert_eq!(decide(None, Some(&frame)), Found::Nothing);
    }
}
