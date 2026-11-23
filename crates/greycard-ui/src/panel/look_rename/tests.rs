use std::cell::RefCell;

use super::*;
use crate::testing::{LOOK_STORE as STORE, click, press, state_for, type_text, window};
use greycard_edit::look::{LookLut, LutChoice};
use greycard_edit::{DisplayCurve, Edit, Preset, Section};
use slint::Model;
use slint::platform::Key;

/// A newer greycard's sidecar (§724) is never written by the rename's
/// thread: the frame fails, and the file is as it was.
#[test]
fn a_rename_writes_no_newer_build_s_sidecar() {
    let dir = std::env::temp_dir().join(format!("greycard-rename-newer-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("IMG_0001.CR3");
    let path = greycard_edit::Sidecar::path_for(&raw);
    let newer = r#"{"floors": {"read": 10, "write": 11}, "current": {"version": 11, "look_lut": {"lut": "Neon", "strength": 1.0}}}"#;
    std::fs::write(&path, newer).unwrap();
    let frame = JobFrame {
        path: raw,
        memory: None,
    };
    let stop = AtomicBool::new(false);
    assert!(matches!(
        one(
            frame,
            "Neon",
            "Glow",
            greycard_edit::Placement::Beside,
            &stop
        ),
        Outcome::Failed(_)
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    crate::testing::remove_dir_retry(&dir);
}

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
fn run_sent(state: &Rc<Shared>, app: &App) {
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
            let s = Sidecar::load_as_file(f).unwrap().unwrap();
            assert!(
                greycard_edit_io::looks::is_there_in(&store, &s.current().look_lut),
                "{}",
                f.display()
            );
            for curve in DisplayCurve::ALL {
                assert!(
                    greycard_edit_io::looks::look_under_in(&store, &s.current().look_lut, curve)
                        .is_some(),
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
        let mut s = Sidecar::load_as_file(f).unwrap().unwrap();
        assert!(s.rename_look("Neon", "Glow"));
        s.save(f).unwrap();
    }
    every_look_is_there();
    // The rest, then the last stage: every edit names Glow, and Neon
    // is gone.
    for f in &frames[2..] {
        let mut s = Sidecar::load_as_file(f).unwrap().unwrap();
        s.rename_look("Neon", "Glow");
        s.save(f).unwrap();
    }
    assert!(unlink_old(&pairs).is_empty());
    every_look_is_there();
    assert!(!greycard_edit_io::looks::is_there_in(
        &store,
        &named("Neon")
    ));
    crate::testing::remove_dir_retry(&dir);
}

/// A state over real frames in `dir`, sidecars on, the store the
/// rename's, deletes (and so renames) allowed.
fn renaming(
    app: &App,
    dir: &Path,
    store: &Path,
    frames: usize,
) -> (Rc<Shared>, Rc<crate::worker::Worker>, Vec<PathBuf>) {
    STORE.with(|s| *s.borrow_mut() = Some(store.to_path_buf()));
    let files: Vec<PathBuf> = (0..frames).map(|i| dir.join(format!("f{i}.tif"))).collect();
    for f in &files {
        std::fs::write(f, b"x").unwrap();
    }
    let (state, worker) = state_for(app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.deletes_allowed = true;
        st.develop.looks = list_in(Some(store.to_path_buf()));
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
        st.develop.edit.look_lut = named("Neon");
        let open = st.develop.edit.clone();
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
        assert_eq!(st.window.placement, greycard_edit::Placement::Beside);
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
        st.develop.preset_store = Some(store_p);
        crate::panel::assets::refresh_presets(&mut st, &app);
        st.sheets.clipboard = Some(crate::clipboard::Clipboard::copy_file(
            st.develop.edit.clone(),
            &files[0],
        ));
        st.work.export_queue_file = Some(queue_file.clone());
        st.work.export_queue = vec![crate::export_queue::Entry {
            queued: 0,
            folder: None,
            preset: None,
            sheet: Default::default(),
            frames: vec![crate::export_queue::QueuedFrame {
                source: files[0].clone(),
                edit: st.develop.edit.clone(),
                turn: 0,
                seed_blend: false,
                version: None,
                id: crate::export_queue::fresh_id(),
            }],
        }];
        show_looks(&st, &st.develop.edit.look_lut, &app);
    }
    let mono_preset = std::fs::read(presets.join("mono-cold.gcp")).unwrap();
    let before_2 = std::fs::read(Sidecar::path_for(&files[2])).unwrap();
    assert!(app.get_look_renamable());
    // The section's body grows to its rows over a moment.
    for _ in 0..4 {
        crate::testing::count_id(&app, "x");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
        slint::platform::update_timers_and_animations();
    }
    let (at, size) = crate::testing::by_id(&app, "LookSection::rename-look-button");
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
    let generation = state.borrow().sheets.look_rename_generation;
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
        let mut outside = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
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
        Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap()
            .current()
            .look_lut,
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
    let read = |i: usize| Sidecar::load_as_file(&files[i]).unwrap().unwrap();
    assert_eq!(read(0).current().look_lut, named("Glow"));
    assert_eq!(read(1).current().look_lut, named("Mono"));
    assert_eq!(read(1).snapshots[0].edit.look_lut, named("Glow"));
    assert_eq!(read(2).current().look_lut, named("Neon Film"));
    assert_eq!(read(3).current().look_lut, named("Mono"));
    assert_eq!(read(3).history[1].edit().look_lut, named("Glow"));
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
    assert_eq!(
        st.sheets.clipboard.as_ref().unwrap().edit.look_lut,
        named("Glow")
    );
    let queued = crate::export_queue::load(&queue_file);
    assert_eq!(queued[0].frames[0].edit.look_lut, named("Glow"));
    assert_eq!(st.develop.edit.look_lut, named("Glow"));
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
        st.develop.edit.look_lut = named("Neon");
        let open = st.develop.edit.clone();
        st.sidecars[0].record(open.clone());
        st.sidecars[1].record(open);
        show_looks(&st, &st.develop.edit.look_lut, &app);
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
        Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap()
            .current()
            .look_lut,
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
        show_looks(&st, &st.develop.edit.look_lut, &app);
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
    state.borrow_mut().sheets.look_removing = true;
    ask(&mut state.borrow_mut(), &app);
    assert!(!app.get_look_rename_open());
    assert!(app.get_status().contains("still being removed"));
    state.borrow_mut().sheets.look_removing = false;

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
    state.borrow_mut().window.deletes_allowed = false;
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
    state.borrow_mut().sheets.deleting = Some(Default::default());
    ask(&mut state.borrow_mut(), &app);
    assert!(!app.get_look_rename_open());
    assert!(app.get_status().contains("being deleted"));
    STORE.with(|s| *s.borrow_mut() = None);
    crate::testing::remove_scratch(state, &dir);
}

/// The sheet asked and answered with `new` over the look the panel
/// has chosen; the job sent, not run.
fn answer(state: &Rc<Shared>, app: &App, new: &str) {
    ask(&mut state.borrow_mut(), app);
    assert!(app.get_look_rename_open(), "{}", app.get_status());
    let generation = state.borrow().sheets.look_rename_generation;
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
fn two_on_neon(dir: &Path, store: &Path) -> (App, Rc<Shared>, Vec<PathBuf>) {
    let app = window(2);
    let (state, _worker, files) = renaming(&app, dir, store, 2);
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.develop.edit.look_lut = named("Neon");
        for (i, f) in files.iter().enumerate() {
            let edit = st.develop.edit.clone();
            st.sidecars[i].record(edit);
            st.sidecars[i].save(f).unwrap();
        }
        show_looks(&st, &st.develop.edit.look_lut, &app);
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
        .map(|f| Sidecar::load_as_file(f).unwrap().unwrap())
        .collect();
    // Both frames' reads are out; frame 0's panel stands in.
    {
        let mut st = state.borrow_mut();
        st.from_row[0].read = false;
        st.from_row[1].read = false;
        st.views.panel_stand_in = Some(files[0].clone());
        st.views.loads.in_flight.insert(70, vec![files[0].clone()]);
        st.views.loads.in_flight.insert(71, vec![files[1].clone()]);
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
    assert!(state.borrow().sheets.look_renaming.is_some());
    assert!(store.join("Neon.cube").exists());
    {
        let mut st = state.borrow_mut();
        assert!(crate::rows::land_read(&mut st, &app, 1, before[1].clone()));
        read_landed(&mut st, &app, 71);
    }
    assert!(!app.get_look_rename_open());
    assert!(state.borrow().sheets.look_renaming.is_none());
    assert!(app.get_status().starts_with("renamed Neon to Glow"));
    assert!(!store.join("Neon.cube").exists());
    let st = state.borrow();
    for (i, f) in files.iter().enumerate() {
        assert!(!st.sidecars[i].names_look("Neon"), "{i}");
        let disk = Sidecar::load_as_file(f).unwrap().unwrap();
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
            let mut s = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
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
            let read = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
            assert!(crate::rows::land_read(&mut st, &app, 1, read));
            assert!(st.sidecars[1].current().look_lut.names("Neon"));
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
        Sidecar::load_as_file(&files[1])
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
        let r = st.sheets.look_renaming.as_ref().unwrap();
        r.reached
            .lock()
            .unwrap()
            .push((files[1].clone(), sent_hash, late));
    }
    heard(&mut state.borrow_mut(), &app, generation, true);
    let st = state.borrow();
    assert!(st.sheets.look_renaming.is_none());
    assert!(!st.sidecars[1].names_look("Neon"));
    assert_eq!(
        st.sidecars[1].revisions,
        Sidecar::load_as_file(&files[1]).unwrap().unwrap().revisions
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
        let disk = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
        for s in [&st.sidecars[1], &disk] {
            assert!(!s.names_look("Neon"), "diverged {diverged}");
            let exposures: Vec<f32> = s
                .history
                .iter()
                .map(|h| h.edit().light.exposure)
                .chain(std::iter::once(s.current().light.exposure))
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
            st.develop.edit.look_lut = named("Neon");
            for (i, f) in files.iter().enumerate() {
                let edit = st.develop.edit.clone();
                st.sidecars[i].record(edit);
                st.sidecars[i].save(f).unwrap();
            }
            show_looks(&st, &st.develop.edit.look_lut, &app);
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
            let stop = state
                .borrow()
                .sheets
                .look_renaming
                .as_ref()
                .unwrap()
                .stop
                .clone();
            job.run(&|| stop.store(true, Ordering::Relaxed));
            heard(&mut state.borrow_mut(), &app, generation, false);
        }
        // Over only when the thread says it is done.
        assert!(state.borrow().sheets.look_renaming.is_some());
        heard(&mut state.borrow_mut(), &app, generation, true);
        assert!(state.borrow().sheets.look_renaming.is_none());
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
                !Sidecar::load_as_file(&files[i])
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
                    st.views.library.offline.insert(dir.clone());
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
                _ => st.window.write_sidecars = false,
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
        st.window.write_sidecars = true;
        st.sheets.look_rename_generation = 7;
        st.sheets.look_renaming = Some(Renaming {
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
