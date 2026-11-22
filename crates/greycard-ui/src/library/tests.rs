use std::cell::RefCell;

use super::*;
use greycard_edit::{Sidecar, meta};
use greycard_library::fixture::{A7, R5, R6, write_frame, write_mosaic_dng};
use slint::Model;

/// The window's folder pass says what the library found in it, a
/// folder's rename among it: the second half of a rename whose
/// first the watcher's pass over the old folder saw, which the
/// library pairs once and forgets.
#[test]
fn the_window_pass_says_a_rename_it_completes() {
    let dir = crate::testing::scratch_dir("window-pass-rename");
    let (shoot, renamed) = (dir.join("shoot"), dir.join("renamed"));
    std::fs::create_dir_all(&shoot).unwrap();
    for (i, frame) in [&R5, &R6, &A7].into_iter().enumerate() {
        write_frame(&shoot.join(format!("x{i}.tif")), frame, i as u16 + 1);
    }
    let mut lib = Library::open(&dir.join("library.sqlite")).unwrap();
    lib.index_tree(&dir, &mut |_| {}).unwrap();
    let was = greycard_library::key_folder(&shoot);
    greycard_core::testing::rename_retry(&shoot, &renamed);
    let report = lib.index_tree(&shoot, &mut |_| {}).unwrap();
    assert!(report.moved_folders.is_empty(), "{report:?}");
    let waits = Waits {
        wanted: Arc::new(AtomicU64::new(1)),
        files: Arc::new(AtomicUsize::new(0)),
        folders: Arc::new(AtomicBool::new(false)),
        lightroom: Arc::new(Mutex::new(None)),
        busy: Arc::new(Busy::default()),
        score: mpsc::channel().0,
    };
    let pass = Pass {
        dirs: vec![renamed.clone()],
        generation: 1,
        started: Instant::now(),
    };
    let said = std::cell::RefCell::new(None);
    let told = |t: Told| {
        if let Told::Indexed { report, .. } = t {
            *said.borrow_mut() = Some(report);
        }
    };
    assert!(window_pass(&mut lib, pass, &dir, &told, &|_| {}, &waits).is_none());
    let report = said.into_inner().expect("the pass said its report");
    assert_eq!(
        report.moved_folders,
        [(was, greycard_library::key_folder(&renamed))],
        "{report:?}"
    );
    assert_eq!(report.added, 3, "{report:?}");
    drop(lib);
    crate::testing::remove_dir_retry(&dir);
}

fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-ui-library-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // Canonical, as the indexer makes a root before it walks it:
    // on macOS the temp dir is a symlink, and a hang hook's prefix
    // check on the uncanonical path would never match.
    dunce::canonicalize(&dir).unwrap()
}

/// A folder of five frames on a library of the test's own: two
/// R6s, an R5, an A7, and a JPEG-like frame the index has not
/// reached. The index is written through a connection of its
/// own, as the indexer's thread would, and read through a
/// read-only one, as the window does.
fn folder(dir: &Path) -> (Vec<PathBuf>, Vec<Sidecar>, Library) {
    let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif", "d.tif", "e.tif"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    write_frame(&files[0], &R6, 1);
    write_frame(&files[1], &R6, 2);
    write_frame(&files[2], &R5, 3);
    write_frame(&files[3], &A7, 4);
    let mut sidecars = vec![Sidecar::default(); files.len()];
    sidecars[0].meta.rating = 3;
    sidecars[0].meta.set_keywords(vec!["Harbor".into()]);
    sidecars[2].meta.rating = 4;
    sidecars[2]
        .meta
        .set_keywords(vec!["harbor".into(), "dusk".into()]);
    sidecars[3].meta.flag = meta::Flag::Pick;
    for (f, s) in files.iter().zip(&mut sidecars).take(4) {
        s.save(f).unwrap();
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = Library::open(&db).unwrap();
    writer.index_folder(dir, &mut |_| {}).unwrap();
    greycard_library::score::score(&mut writer, 2, &|| false, &mut |_| {}).unwrap();
    // The fifth arrives after the pass: no row yet.
    write_frame(&files[4], &R6, 5);
    drop(writer);
    let reader = Library::open_read_only(&db).unwrap();
    (files, sidecars, reader)
}

/// A state over a folder of three, all indexed: an R5 at 23.9 mm
/// and ISO 100, an R6 at ISO 6400, and a mosaic DNG whose EXIF has
/// neither an ISO nor a focal length.
fn odd_state(app: &App, dir: &Path) -> State {
    let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.dng"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    write_frame(
        &files[0],
        &greycard_library::fixture::Frame {
            focal: (239, 10),
            ..R5
        },
        1,
    );
    write_frame(&files[1], &R6, 2);
    write_mosaic_dng(&files[2], "RGGB");
    let db = dir.join("index").join("library.sqlite");
    let mut writer = Library::open(&db).unwrap();
    writer.index_folder(dir, &mut |_| {}).unwrap();
    drop(writer);
    let mut st = State::empty(files, app);
    st.sidecars = vec![Sidecar::default(); 3];
    st.views.index_reader = Some(Library::open_read_only(&db).unwrap());
    refresh_ids(&mut st);
    assert!(
        st.views.index_ids.iter().all(Option::is_some),
        "all indexed"
    );
    st
}

/// An older build's held-out ISO chip let a frame with no ISO
/// through; loaded now it is dropped, rather than turned into an
/// `iso!=` that would hide that frame.
#[test]
fn a_held_out_iso_chip_kept_by_an_older_build_hides_no_frame_without_one() {
    use crate::filter::SavedBar;
    let dir = scratch("held-out");
    let app = crate::testing::window(3);
    let mut st = odd_state(&app, &dir);
    let kept: filter::Saved = serde_json::from_str(
        r#"{"stars": 0, "exact": false, "flags": [], "labels": [], "text": "",
                "not_facets": {"iso": ["6400"], "focal": ["50"]}}"#,
    )
    .unwrap();
    st.views.filter = kept.filter();
    assert_eq!(st.views.filter.text, "");
    assert_eq!(shown(&st), [0, 1, 2], "the frame with no ISO stays");
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// A focal length off the whole millimeter is a stop of its own, and
/// the terms the range writes for it pass it at either end, through
/// the index's own SQL.
#[test]
fn a_focal_stop_off_the_whole_millimeter_passes_its_own_terms() {
    let dir = scratch("focal-tenth");
    let app = crate::testing::window(3);
    let mut st = odd_state(&app, &dir);
    let counts = folder_values(&st)
        .into_iter()
        .find(|(f, _)| *f == Facet::Focal)
        .map(|(_, c)| c)
        .unwrap();
    let focal = range_of(Facet::Focal, &counts, "");
    let keys: Vec<String> = focal.keys.iter().map(|k| k.to_string()).collect();
    assert_eq!(keys, ["23.9", "50"]);
    for text in ["focal>=23.9", "focal<=23.9", "focal>=23.9 focal<=23.9"] {
        st.views.filter.text = text.into();
        st.views.index_passed = index_pass(&st);
        let shown = shown(&st);
        assert!(shown.contains(&0), "{text}: {shown:?}");
        assert!(!shown.contains(&2), "{text}: no focal length, no pass");
    }
    st.views.filter.text = "focal>=23.9 focal<=23.9".into();
    assert_eq!(shown(&st), [0]);
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// A state with the folder's files, sidecars and index, as the
/// window would have them after the first pass.
fn state(app: &App, dir: &Path) -> State {
    let (files, sidecars, reader) = folder(dir);
    let mut st = State::empty(files, app);
    st.sidecars = sidecars;
    st.views.index_reader = Some(reader);
    refresh_ids(&mut st);
    st
}

fn shown(st: &State) -> Vec<usize> {
    let pass = index_pass(st);
    let frames: Vec<filter::Frame> = st
        .files
        .iter()
        .zip(&st.sidecars)
        .zip(&pass)
        .map(|((path, s), &index)| filter::Frame {
            path,
            meta: &s.meta,
            index,
        })
        .collect();
    st.views.filter.apply(&frames)
}

/// A facet's counts, the index's answer brought up to the filter
/// first, as `rebuild_browser` brings it before the chips are
/// shown.
fn counts(st: &mut State, facet: Facet) -> Vec<(String, usize)> {
    st.views.index_passed = index_pass(st);
    facet_counts(st)
        .into_iter()
        .find(|(f, _)| *f == facet)
        .map(|(_, c)| c.into_iter().map(|c| (c.value, c.count)).collect())
        .unwrap_or_default()
}

fn pairs(want: &[(&str, usize)]) -> Vec<(String, usize)> {
    want.iter().map(|(v, n)| (v.to_string(), *n)).collect()
}

/// A camera chip is answered from the index, and a frame the
/// index has not reached yet stays until it has.
#[test]
fn a_facet_chip_filters_by_the_index_and_a_frame_without_a_row_stays() {
    let dir = scratch("chip");
    let app = crate::testing::window(5);
    let mut st = state(&app, &dir);
    assert_eq!(st.views.index_ids.iter().filter(|i| i.is_some()).count(), 4);
    assert_eq!(st.views.index_ids[4], None, "the fifth has no row");
    assert_eq!(shown(&st), [0, 1, 2, 3, 4]);

    st.views
        .filter
        .toggle_facet(Facet::Camera, "Canon EOS R6m2");
    // The two R6s, and the fifth, which the index cannot yet say
    // is an R6 or not.
    assert_eq!(shown(&st), [0, 1, 4]);
    // Two chips of a facet are either.
    st.views.filter.toggle_facet(Facet::Camera, "Canon EOS R5");
    assert_eq!(shown(&st), [0, 1, 2, 4]);
    // A second facet narrows.
    st.views.filter.toggle_facet(Facet::Iso, "100");
    assert_eq!(shown(&st), [2, 4]);

    // Once the fifth has a row the index's word stands.
    let db = st.views.index_reader.as_ref().unwrap().path().to_path_buf();
    Library::open(&db)
        .unwrap()
        .index_file(&st.files[4])
        .unwrap();
    refresh_ids(&mut st);
    assert!(st.views.index_ids[4].is_some());
    assert_eq!(shown(&st), [2]);
    st.views.filter.toggle_facet(Facet::Iso, "100");
    assert_eq!(shown(&st), [0, 1, 2, 4]);
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// A facet's count is how many frames hold each value among the
/// frames the other groups leave, its own set aside: the meta
/// rows' rule, and what one chip pressed alone would list.
#[test]
fn a_facet_counts_what_the_other_tests_leave_and_its_own_is_set_aside() {
    let dir = scratch("counts");
    let app = crate::testing::window(5);
    let mut st = state(&app, &dir);
    let sony = st
        .views
        .index_reader
        .as_ref()
        .unwrap()
        .by_path(&st.files[3])
        .unwrap()
        .unwrap()
        .exif
        .camera;
    assert_eq!(
        counts(&mut st, Facet::Camera),
        pairs(&[("Canon EOS R6m2", 2), ("Canon EOS R5", 1), (&sony, 1)])
    );
    assert_eq!(
        counts(&mut st, Facet::Lens),
        pairs(&[
            ("RF50mm F1.8 STM", 2),
            ("FE 24-70mm F2.8 GM II", 1),
            ("RF35mm F1.4 L VCM", 1)
        ])
    );
    // ISO and the focal length are ranges, and are not counted.
    assert!(counts(&mut st, Facet::Iso).is_empty());
    assert!(counts(&mut st, Facet::Focal).is_empty());
    assert_eq!(
        counts(&mut st, Facet::Keyword),
        pairs(&[("harbor", 2), ("dusk", 1)])
    );

    // Three stars or more: the meta test narrows every facet.
    st.views.filter.stars = filter::Stars::AtLeast(3);
    assert_eq!(
        counts(&mut st, Facet::Camera),
        pairs(&[("Canon EOS R5", 1), ("Canon EOS R6m2", 1)])
    );
    st.views.filter.stars = filter::Stars::default();

    // An R6 chip on: the camera row is unmoved, since its own
    // group is set aside; the others narrow to the R6s.
    st.views
        .filter
        .toggle_facet(Facet::Camera, "Canon EOS R6m2");
    assert_eq!(
        counts(&mut st, Facet::Camera),
        pairs(&[("Canon EOS R6m2", 2), ("Canon EOS R5", 1), (&sony, 1)])
    );
    assert_eq!(
        counts(&mut st, Facet::Lens),
        pairs(&[("RF50mm F1.8 STM", 2)])
    );
    assert_eq!(counts(&mut st, Facet::Keyword), pairs(&[("harbor", 1)]));

    // And the rule itself, chip by chip: a count is what that
    // chip alone, the other groups as they stand, would list of
    // the frames the index has rows for.
    st.views.filter.stars = filter::Stars::AtLeast(1);
    st.views.index_passed = index_pass(&st);
    for facet in Facet::ALL
        .into_iter()
        .filter(|f| filter::ranged(*f).is_none())
    {
        let rows = facet_counts(&st);
        let (_, chips) = rows.iter().find(|(f, _)| *f == facet).unwrap();
        for chip in chips {
            let mut alone = st.views.filter.clone();
            alone.facets[filter::facet_slot(facet)] = vec![chip.value.clone()];
            let saved = std::mem::replace(&mut st.views.filter, alone);
            let listed = shown(&st)
                .into_iter()
                .filter(|&i| st.views.index_ids[i].is_some())
                .count();
            st.views.filter = saved;
            assert_eq!(listed, chip.count, "{facet:?} {}", chip.value);
        }
    }

    // The keyword is counted from the sidecars in hand: a keyword
    // taken off in memory is off the count before any row knows,
    // which is also what `--no-sidecars` needs.
    st.views.filter = filter::Filter::default();
    st.sidecars[2].meta.set_keywords(Vec::new());
    assert_eq!(counts(&mut st, Facet::Keyword), pairs(&[("harbor", 1)]));
    // And it needs no index at all.
    st.views.index_reader = None;
    refresh_ids(&mut st);
    assert_eq!(counts(&mut st, Facet::Keyword), pairs(&[("harbor", 1)]));
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// The text field takes the §160 grammar: an EXIF term goes to
/// the index, a meta term is answered from the sidecar, and a
/// word is the word it always was.
#[test]
fn a_typed_term_is_answered_where_it_can_be() {
    let dir = scratch("typed");
    let app = crate::testing::window(5);
    let mut st = state(&app, &dir);
    st.views.filter.text = "camera:R6".into();
    assert_eq!(shown(&st), [0, 1, 4]);
    st.views.filter.text = "camera:R6 rating>=3".into();
    assert_eq!(shown(&st), [0]);
    st.views.filter.text = "iso>=3200 harbor".into();
    assert_eq!(shown(&st), [0]);
    // The focus score is the index's: every frame it scored, and
    // the one it has no row for, as for every term of the index.
    st.views.filter.text = "focus>=0".into();
    assert_eq!(shown(&st), [0, 1, 2, 3, 4]);
    st.views.filter.text = "focus>=0 rating>=3".into();
    assert_eq!(shown(&st), [0, 2]);
    // The sidecar's word, not the row's: a rating set in memory
    // counts before any index_file has run.
    st.sidecars[1].meta.rating = 5;
    st.views.filter.text = "rating=5".into();
    assert_eq!(shown(&st), [1]);
    // A term that does not parse is the word it was, and says so.
    st.views.filter.text = "rating>=9".into();
    assert_eq!(st.views.filter.typed().errors.len(), 1);
    assert_eq!(shown(&st), Vec::<usize>::new());
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// A facet row's key, its `code`: the facet's slot.
fn slot(facet: Facet) -> i32 {
    crate::filter::facet_slot(facet) as i32
}

/// The kind's row: counted from the paths, left out of a folder of
/// one kind until its chip is on, first in the short row of a
/// folder of more. Its chip, or `--filter kind:raw`, holds every
/// frame to it from the start: one the index has no row for, and
/// every one with no index at all.
#[test]
fn the_kind_row_is_counted_from_the_paths_and_holds_a_frame_with_no_row() {
    let kind_row = |rows: &[FacetRow]| rows.iter().find(|r| r.code == slot(Facet::Kind)).cloned();
    let chips = |row: &FacetRow| -> Vec<(String, i32, bool)> {
        (0..row.chips.row_count())
            .map(|i| {
                let c = row.chips.row_data(i).unwrap();
                (c.text.to_string(), c.count, c.on)
            })
            .collect()
    };
    // All TIFFs: a row of one chip says nothing, and is left out;
    // with that chip on it stays, to be pressed off.
    let dir = scratch("onekind");
    let app = crate::testing::window(5);
    let mut st = state(&app, &dir);
    let rows = facet_rows(&st);
    assert!(rows.iter().any(|r| r.code == slot(Facet::Camera)));
    assert!(kind_row(&rows).is_none());
    st.views.filter.toggle_facet(Facet::Kind, "tiff");
    let row = kind_row(&facet_rows(&st)).expect("a kind on");
    // The fifth, which the index has not reached, among them.
    assert_eq!(chips(&row), [("TIFF".to_string(), 5, true)]);
    drop(st);
    crate::testing::remove_dir_retry(&dir);

    // A synthetic raw beside two pictures, and a JPEG that
    // arrives after the pass: no row.
    let dir = scratch("twokinds");
    let files = vec![
        dir.join("a.dng"),
        dir.join("b.tif"),
        dir.join("c.tif"),
        dir.join("d.jpg"),
    ];
    write_mosaic_dng(&files[0], "RGGB");
    write_frame(&files[1], &R6, 1);
    write_frame(&files[2], &R5, 2);
    let db = dir.join("index").join("library.sqlite");
    Library::open(&db)
        .unwrap()
        .index_folder(&dir, &mut |_| {})
        .unwrap();
    image::RgbImage::from_pixel(4, 3, image::Rgb([90, 120, 150]))
        .save(&files[3])
        .unwrap();
    let app = crate::testing::window(4);
    let mut st = State::empty(files, &app);
    st.sidecars = vec![Sidecar::default(); 4];
    st.views.index_reader = Some(Library::open_read_only(&db).unwrap());
    refresh_ids(&mut st);
    assert_eq!(st.views.index_ids[3], None, "the JPEG has no row");
    let rows = facet_rows(&st);
    let (long, short) = split_facet_rows(&rows);
    assert!(kind_row(&long).is_none());
    assert_eq!(short[0].name, "Kind", "first in the short row");
    assert_eq!(short[0].code, filter::facet_slot(Facet::Kind) as i32);
    assert_eq!(
        chips(&short[0]),
        [
            ("TIFF".to_string(), 2, false),
            ("JPEG".to_string(), 1, false),
            ("Raw".to_string(), 1, false)
        ]
    );
    // While the index is at work its progress stays up beside the
    // kind's row, which a mixed folder has from the start; with
    // rows and the index done, there is nothing to say.
    st.views.index_progress = Some((1, 4));
    crate::panel::cull::show_filter(&st, &app);
    assert!(
        app.get_filter_facets()
            .iter()
            .any(|r| r.code == slot(Facet::Kind))
    );
    assert_eq!(app.get_filter_facet_note(), "Indexing the folder: 1 of 4");
    st.views.index_progress = None;
    crate::panel::cull::show_filter(&st, &app);
    assert_eq!(app.get_filter_facet_note(), "");
    assert_eq!(
        facet_note(&st, false),
        "No camera, lens or date in these files' EXIF"
    );

    // `--filter kind:raw` has the chip on before any frame is read,
    // and the JPEG the index has not reached is not shown.
    let (text, wanted) = filter::from_cli("kind:raw");
    assert!(text.is_empty());
    st.views.filter = filter::Filter::default();
    assert!(st.views.filter.choose_at_once(wanted).is_empty());
    assert_eq!(st.views.filter.chosen(Facet::Kind), ["raw".to_string()]);
    assert_eq!(shown(&st), [0]);
    // The JPEG chip the bar hands back, the same.
    st.views.filter.toggle_facet(Facet::Kind, "raw");
    st.views.filter.toggle_facet(Facet::Kind, "jpeg");
    assert_eq!(shown(&st), [3]);

    // With no index at all the row is there and the chip holds.
    st.views.index_reader = None;
    refresh_ids(&mut st);
    let row = kind_row(&facet_rows(&st)).expect("a kind row with no index");
    assert_eq!(row.chips.row_count(), 3);
    assert_eq!(shown(&st), [3]);
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// The window's side: a facet chip pressed on the bar narrows the
/// browser, and the bar shows the chip on with its count.
#[test]
fn a_chip_pressed_on_the_bar_narrows_the_browser() {
    use crate::testing::{state_for, window};
    let dir = scratch("bar");
    let app = window(0);
    let (files, sidecars, reader) = folder(&dir);
    let (state, _worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
        crate::panel::browser::rebuild_browser(&mut st, &app);
    }
    assert_eq!(app.get_filter_shown(), 5);
    let camera = app.get_filter_facets().row_data(0).expect("a camera row");
    assert_eq!(camera.name, "Camera");
    assert_eq!(camera.chips.row_count(), 3);
    app.invoke_filter_facet_toggled(
        filter::facet_slot(Facet::Camera) as i32,
        "Canon EOS R6m2".into(),
        false,
    );
    assert_eq!(state.borrow().views.shown, [0, 1, 4]);
    assert_eq!(app.get_filter_shown(), 3);
    assert!(app.get_filter_on());
    let camera = app.get_filter_facets().row_data(0).unwrap();
    let chip = camera.chips.row_data(0).unwrap();
    assert_eq!(
        (chip.key.as_str(), chip.count, chip.on),
        ("Canon EOS R6m2", 2, true)
    );
    // Clear lets it go with the rest.
    app.invoke_filter_cleared();
    assert_eq!(state.borrow().views.shown, [0, 1, 2, 3, 4]);
    // The callbacks hold the state too; its connection goes here.
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The indexer says it is busy from its start until it has nothing
/// in hand, and goes idle again after a pass it is asked for: what
/// keeps Forget missing from taking a frame a pass still to come
/// would find moved.
#[test]
fn the_indexer_is_busy_from_an_ask_until_its_pass_is_done() {
    let dir = scratch("busy");
    let shoot = dir.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    write_frame(&shoot.join("a.tif"), &R6, 1);
    let db = dir.join("library.sqlite");
    let (tx, rx) = std::sync::mpsc::channel();
    let indexer = Indexer::start(db, move |told| {
        let _ = tx.send(told);
    })
    .expect("the indexer starts");
    let idle = |indexer: &Indexer| {
        let since = Instant::now();
        while indexer.busy(&HashSet::new(), &[]) {
            assert!(since.elapsed() < Duration::from_secs(20), "never idle");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    idle(&indexer);
    indexer.roots(vec![dir.clone()]);
    loop {
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Told::Background { launch: true, .. }) => break,
            Ok(_) => {}
            Err(e) => panic!("{e}"),
        }
    }
    idle(&indexer);
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// A panic on the indexer's thread is said to the window as a
/// failure, not left as a pass that never ends.
#[test]
fn a_panic_on_the_indexer_thread_is_a_failure_the_window_hears() {
    let dir = scratch("panic");
    let db = dir.join("library.sqlite");
    let (tx, rx) = std::sync::mpsc::channel();
    let indexer = Indexer::start(db, move |told| {
        if matches!(told, Told::Opened(_)) {
            panic!("a bug");
        }
        let _ = tx.send(told);
    })
    .expect("the indexer starts");
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Told::Failed(message)) => assert!(message.contains("a bug"), "{message}"),
        other => panic!("{other:?}"),
    }
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// A library another process holds locked past the writer's
/// timeout: the pass says so in `Indexed`, so the window can say
/// busy and ask again, rather than report a pass that did
/// nothing as a folder with no EXIF.
#[test]
fn a_locked_library_is_an_error_in_the_pass_not_an_empty_folder() {
    let dir = scratch("locked");
    let shoot = dir.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    write_frame(&shoot.join("a.tif"), &R6, 1);
    let db = dir.join("library.sqlite");
    drop(Library::open(&db).unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let indexer = Indexer::start(db.clone(), move |told| {
        let _ = tx.send(told);
    })
    .expect("the indexer starts");
    let wait = || rx.recv_timeout(Duration::from_secs(30)).expect("an answer");
    assert!(matches!(wait(), Told::Opened(_)));
    let holder = rusqlite::Connection::open(&db).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    indexer.folders(vec![shoot.clone()], 1);
    loop {
        match wait() {
            Told::Indexed { error, .. } => {
                assert!(error.is_some(), "the lock was said");
                break;
            }
            Told::Progress { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);
    indexer.folders(vec![shoot], 2);
    match wait() {
        Told::Indexed { error, report, .. } => {
            assert_eq!(error, None);
            assert_eq!(report.added, 1);
        }
        other => panic!("{other:?}"),
    }
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// The CULLING section's ISO and focal length are ranges over the
/// folder's values: an end moved writes its term into the text and
/// filters by it, a term typed shows on the ends, and the whole
/// range again takes the terms out.
#[test]
fn a_range_writes_its_terms_and_shows_the_typed_ones() {
    use slint::Model;

    use crate::testing::{state_for, window};
    let dir = scratch("ranges");
    let app = window(0);
    let (files, sidecars, reader) = folder(&dir);
    let (state, _worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
        crate::panel::browser::rebuild_browser(&mut st, &app);
    }
    let shown =
        |r: &FacetRange| -> Vec<String> { r.values.iter().map(|v| v.to_string()).collect() };
    let iso = app.get_filter_iso_range();
    assert_eq!(shown(&iso), ["100", "3200", "6400"]);
    assert_eq!((iso.low, iso.high, iso.on), (0, 2, false));
    assert_eq!(
        shown(&app.get_filter_focal_range()),
        ["24 mm", "35 mm", "50 mm"]
    );
    // No chips for either, in the panel or the header.
    let rows: Vec<String> = app
        .get_filter_facets()
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    assert!(!rows.iter().any(|n| n == "ISO" || n == "Focal"), "{rows:?}");

    // The low end to 3200: the two R6s, the A7, and the fifth,
    // which has no row yet.
    app.invoke_filter_range_moved(iso.code, 0, "3200".into(), "".into());
    assert_eq!(state.borrow().views.filter.text, "iso>=3200");
    assert_eq!(app.get_filter_text(), "iso>=3200");
    assert_eq!(app.get_filter_shown(), 4);
    let iso = app.get_filter_iso_range();
    assert_eq!((iso.low, iso.high, iso.on), (1, 2, true));
    // Its values stay the folder's, the range's own term aside.
    assert_eq!(shown(&iso), ["100", "3200", "6400"]);

    // A value the folder lacks, typed: the ends on the nearest it
    // has, the text as typed.
    app.invoke_filter_text_edited("harbor iso:5000".into());
    let iso = app.get_filter_iso_range();
    assert_eq!((iso.low, iso.high, iso.on), (2, 2, true));
    assert_eq!(state.borrow().views.filter.text, "harbor iso:5000");
    // The high end moved up from there to the folder's: the exact
    // term goes, and the low end is written as shown.
    app.invoke_filter_range_moved(iso.code, 1, "6400".into(), "".into());
    assert_eq!(app.get_filter_text(), "harbor iso>=6400");

    // The whole range again.
    app.invoke_filter_range_reset(iso.code);
    assert_eq!(app.get_filter_text(), "harbor");
    assert!(!app.get_filter_iso_range().on);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The grid's header has room for every row its filter bar draws: with
/// the ranges' row up, side by side at a wide window and one under the
/// other at a narrow one, the second facet row (kind, day, and keyword)
/// still sits whole above the sheet rather than clipped under it.
#[test]
fn the_grid_header_holds_both_facet_rows_under_the_ranges() {
    use i_slint_backend_testing::ElementHandle;
    use slint::Model;

    use crate::testing::{settle, state_for, window};
    let dir = scratch("header-rows");
    let app = window(0);
    let (files, sidecars, reader) = folder(&dir);
    let (state, _worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
        crate::panel::browser::rebuild_browser(&mut st, &app);
    }
    assert!(app.get_filter_facets_long().row_count() > 0, "camera, lens");
    assert!(
        app.get_filter_facets_short().row_count() > 0,
        "day, keyword"
    );
    app.set_filter_bar(true);
    app.set_grid_open(true);
    // The ranges side by side, then one under the other.
    for (width, ranges_rows) in [(1500.0, 1), (420.0, 2)] {
        app.window().set_size(slint::LogicalSize::new(width, 900.0));
        settle(&app);
        let sheet = ElementHandle::find_by_element_id(&app, "GridSheet::grid-sheet")
            .next()
            .expect("the sheet");
        let (sheet_at, sheet_size) = crate::testing::bounds_of(&sheet);
        let in_header = |e: &ElementHandle| {
            let at = e.absolute_position();
            at.x >= sheet_at.x && at.x < sheet_at.x + sheet_size.width && at.y < sheet_at.y
        };
        let mut ranges: Vec<f32> = ElementHandle::find_by_element_type_name(&app, "RangeRow")
            .filter(in_header)
            .map(|r| r.absolute_position().y)
            .collect();
        ranges.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        assert_eq!(ranges.len(), ranges_rows, "at {width} px: {ranges:?}");
        let rows: Vec<(f32, f32)> = ElementHandle::find_by_element_id(&app, "FacetChips::flick")
            .filter(in_header)
            .map(|f| (f.absolute_position().y, f.size().height))
            .collect();
        assert_eq!(rows.len(), 2, "at {width} px: {rows:?}");
        for (y, h) in rows {
            assert!(
                y + h <= sheet_at.y + 0.5,
                "at {width} px a facet row at {y}..{} runs under the sheet at {}",
                y + h,
                sheet_at.y
            );
        }
    }
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// ISO and the focal length have no chips: `--filter iso:3200` at
/// launch, and an older build's chips kept in a filter, come in as
/// the range's terms in the text, and the range shows them and
/// clears them as a range set by hand.
#[test]
fn launch_and_kept_chips_show_on_the_range() {
    use crate::filter::SavedBar;
    use crate::testing::{state_for, window};
    let dir = scratch("chosen");
    let app = window(0);
    let (files, sidecars, reader) = folder(&dir);
    let (state, _worker) = state_for(&app, files);
    let rebuild = || {
        let mut st = state.borrow_mut();
        crate::panel::browser::rebuild_browser(&mut st, &app);
    };
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
    }
    rebuild();
    let ends = |r: FacetRange| (r.low, r.high, r.on);
    let iso = app.get_filter_iso_range().code;
    let focal = app.get_filter_focal_range().code;

    // `--filter iso:3200` at launch: the term goes to the text.
    let (rest, wanted) = filter::from_cli("iso:3200");
    assert!(wanted.is_empty());
    app.invoke_filter_text_edited(rest.into());
    assert_eq!(ends(app.get_filter_iso_range()), (1, 1, true));
    assert!(app.get_filter_on());
    // Clear, as for any filter.
    app.invoke_filter_cleared();
    assert_eq!(ends(app.get_filter_iso_range()), (0, 2, false));
    assert!(!app.get_filter_on());

    // A filter kept from the last session or a smart collection by
    // a build that had the chips.
    let kept: filter::Saved = serde_json::from_str(
        r#"{"stars": 0, "exact": false, "flags": [], "labels": [], "text": "",
                "facets": {"iso": ["6400", "100"], "focal": ["50"]}}"#,
    )
    .unwrap();
    let read = kept.filter();
    app.set_filter_text(read.text.clone().into());
    state.borrow_mut().views.filter = read;
    rebuild();
    assert_eq!(app.get_filter_text(), "iso>=100 iso<=6400 focal:50");
    assert_eq!(ends(app.get_filter_iso_range()), (0, 2, true));
    assert_eq!(ends(app.get_filter_focal_range()), (2, 2, true));
    // The whole ISO range: its terms go, the focal length's stay.
    app.invoke_filter_range_reset(iso);
    assert_eq!(ends(app.get_filter_iso_range()), (0, 2, false));
    assert_eq!(app.get_filter_text(), "focal:50");
    // The focal length's low end down a stop: the exact term goes,
    // both ends written as shown.
    app.invoke_filter_range_moved(focal, 0, "35".into(), "".into());
    assert_eq!(app.get_filter_text(), "focal>=35");
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A range's drag filters nothing until release: a scrub past the
/// open frame's ISO and back, a click on the track, and a
/// double-click on a handle leave
/// the open frame, the set, the filter, and the grid's rows alone.
#[test]
fn a_scrub_and_a_double_click_leave_the_open_frame_and_the_set() {
    use crate::testing::{settle, state_for, window};
    use slint::platform::{PointerEventButton, WindowEvent};
    let dir = scratch("scrub");
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 900.0));
    let (files, sidecars, reader) = folder(&dir);
    let (state, _worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
        // The R5 at ISO 100 open, the A7 at 3200 with it.
        st.current = Some(2);
        st.views.picked = vec![2, 3];
        crate::panel::browser::rebuild_browser(&mut st, &app);
    }
    app.set_culling(true);
    app.set_panel_tab("Cull".into());
    settle(&app);
    settle(&app);
    assert_eq!(app.get_filter_iso_range().values.row_count(), 3);
    let (at, s) =
        i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "RangeSlider")
            .next()
            .map(|e| crate::testing::bounds_of(&e))
            .expect("the ISO range");
    let y = at.y + s.height / 2.0;
    let stop = |i: f32| slint::LogicalPosition::new(at.x + 8.0 + (s.width - 16.0) * i / 2.0, y);
    let rows = app.get_thumbs();
    let alone = |when: &str| {
        let st = state.borrow();
        assert_eq!(st.current, Some(2), "{when}: the open frame");
        assert_eq!(st.views.picked, [2, 3], "{when}: the set");
        assert_eq!(st.views.filter.text, "", "{when}: the filter");
        assert!(app.get_thumbs() == rows, "{when}: no rebuild");
    };
    let pointer = |event: WindowEvent| app.window().dispatch_event(event);
    let button = PointerEventButton::Left;
    // The low end to 6400 and back to 100.
    pointer(WindowEvent::PointerMoved {
        position: stop(0.0),
    });
    pointer(WindowEvent::PointerPressed {
        position: stop(0.0),
        button,
    });
    for i in [1.0, 2.0, 1.0, 0.0] {
        pointer(WindowEvent::PointerMoved { position: stop(i) });
        alone(&format!("held at {i}"));
    }
    pointer(WindowEvent::PointerReleased {
        position: stop(0.0),
        button,
    });
    settle(&app);
    alone("scrubbed and back");
    // A click on the track past the open frame's ISO, and a
    // double-click on the low handle.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    crate::testing::click(&app, stop(1.5).x, y);
    settle(&app);
    alone("clicked off the handles");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    crate::testing::click(&app, stop(0.0).x, y);
    crate::testing::click(&app, stop(0.0).x, y);
    settle(&app);
    alone("double-clicked");
    // The ranges sit above the panel's facet rows, which come and go
    // with the filter a range sets: applied, to the R6s alone, the
    // ranges keep their place in the section. (Measured from the
    // section's top: the panel's head, above the sections, is not
    // the filter's, and here it is drawn again at the first rebuild
    // since the frame was set open without opening it.)
    use i_slint_backend_testing::ElementHandle;
    let bounds = |kind: &str| -> Vec<(f32, f32, f32)> {
        let top = ElementHandle::find_by_element_type_name(&app, "CullSection")
            .next()
            .map(|e| crate::testing::bounds_of(&e).0.y)
            .expect("the CULLING section");
        ElementHandle::find_by_element_type_name(&app, kind)
            .map(|e| crate::testing::bounds_of(&e))
            .map(|(p, s)| (p.x, p.y - top, s.height))
            .collect()
    };
    let ranges = bounds("RangeRow");
    assert_eq!(ranges.len(), 2);
    let (panel_x, bottom) = (ranges[0].0, ranges[1].1 + ranges[1].2);
    let facets: Vec<_> = (bounds("FacetChips").into_iter())
        .filter(|f| f.0 >= panel_x - 1.0)
        .collect();
    assert!(!facets.is_empty(), "the panel's facet rows");
    for f in &facets {
        assert!(
            f.1 >= bottom,
            "a facet row at {} above a range ending at {bottom}",
            f.1
        );
    }
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    crate::testing::drag(
        &app,
        &[(stop(0.0).x, y), (stop(1.0).x, y), (stop(2.0).x, y)],
    );
    settle(&app);
    settle(&app);
    assert_eq!(state.borrow().views.filter.text, "iso>=6400");
    assert_eq!(bounds("RangeRow"), ranges);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The grid's header has two facet rows: camera and lens on the
/// first, whatever else the index counts on the second.
#[test]
fn the_headers_facets_split_into_camera_and_lens_then_the_rest() {
    use crate::testing::{state_for, window};
    let dir = scratch("rows");
    let app = window(0);
    let (files, sidecars, reader) = folder(&dir);
    let (state, _worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
        crate::panel::browser::rebuild_browser(&mut st, &app);
    }
    let names = |rows: slint::ModelRc<FacetRow>| -> Vec<String> {
        rows.iter().map(|r| r.name.to_string()).collect()
    };
    let long = names(app.get_filter_facets_long());
    let short = names(app.get_filter_facets_short());
    assert_eq!(long, ["Camera", "Lens"]);
    assert!(!short.is_empty(), "{short:?}");
    assert!(
        short.iter().all(|n| n != "Camera" && n != "Lens"),
        "{short:?}"
    );
    // ISO and the focal length are ranges, here as in the CULLING
    // section, with no chips.
    assert!(
        short.iter().all(|n| n != "ISO" && n != "Focal"),
        "{short:?}"
    );
    // Together they are the rows the stacked bar shows.
    assert_eq!(
        long.len() + short.len(),
        app.get_filter_facets().row_count()
    );
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The grid's header has the ranges too, side by side on a row of
/// their own under the top row and above the facets' rows, a fair
/// width at a laptop's window, the default, and a wide one, and no
/// taller than a chip row. A drag applies on release, as in Cull,
/// and the ranges stay put when the facets' rows change with it. A
/// facet with one value has no range here.
#[test]
fn the_grids_header_has_the_ranges_where_the_filter_cannot_move_them() {
    use crate::testing::{settle, state_for, window};
    use i_slint_backend_testing::ElementHandle;
    let dir = scratch("header-ranges");
    let app = window(0);
    let (files, sidecars, reader) = folder(&dir);
    let (state, _worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
        st.current = Some(2);
        crate::panel::browser::rebuild_browser(&mut st, &app);
    }
    app.set_grid_open(true);
    // The header's, inside the grid's sheet, by kind: left, top,
    // width, and height.
    let header = |kind: &str| -> Vec<(f32, f32, f32, f32)> {
        let (sheet, size) = ElementHandle::find_by_element_type_name(&app, "GridSheet")
            .next()
            .map(|e| crate::testing::bounds_of(&e))
            .expect("the grid");
        ElementHandle::find_by_element_type_name(&app, kind)
            .map(|e| crate::testing::bounds_of(&e))
            .filter(|(p, _)| p.x >= sheet.x && p.x < sheet.x + size.width)
            .map(|(p, s)| (p.x, p.y, s.width, s.height))
            .collect()
    };
    // The window at its least and on up to wide, the left pane open
    // and put away: the ranges side by side where both fit, else one
    // under the other, whole inside the grid either way.
    let sheet_right = || {
        ElementHandle::find_by_element_type_name(&app, "GridSheet")
            .next()
            .map(|e| {
                let (p, s) = crate::testing::bounds_of(&e);
                p.x + s.width
            })
            .expect("the grid")
    };
    let sizes = [
        (574.0, 768.0),
        (640.0, 768.0),
        (700.0, 768.0),
        (736.0, 768.0),
        (1366.0, 768.0),
        (1500.0, 800.0),
        (2560.0, 1440.0),
    ];
    for hidden in [false, true] {
        app.set_left_hidden(hidden);
        for (w, h) in sizes {
            app.window().set_size(slint::LogicalSize::new(w, h));
            settle(&app);
            settle(&app);
            let at = format!(
                "{w} wide, left pane {}",
                if hidden { "away" } else { "open" }
            );
            let rows = header("RangeRow");
            assert_eq!(rows.len(), 2, "{at}: both ranges");
            let right = sheet_right().min(w);
            if w >= 1366.0 {
                assert_eq!(rows[0].1, rows[1].1, "{at}: on one row");
            }
            let chip_row = header("FacetChips")
                .iter()
                .map(|f| f.3)
                .fold(f32::MAX, f32::min);
            for r in &rows {
                assert!(
                    r.3 <= chip_row,
                    "{at}: a range {} tall, a chip row {chip_row}",
                    r.3
                );
                assert!(
                    r.2 <= 420.5 || rows[0].1 != rows[1].1,
                    "{at}: a range {} wide",
                    r.2
                );
                assert!(
                    r.0 + r.2 <= right + 0.5,
                    "{at}: a range to {}, the grid to {right}",
                    r.0 + r.2
                );
            }
            for t in header("RangeSlider") {
                assert!(t.2 >= 100.0, "{at}: a track {} wide", t.2);
                assert!(t.0 + t.2 <= right + 0.5, "{at}: a track past the grid");
            }
            let bottom = rows.iter().map(|r| r.1 + r.3).fold(0.0, f32::max);
            for f in header("FacetChips") {
                assert!(
                    f.1 >= bottom,
                    "{at}: a facet row at {} above the ranges",
                    f.1
                );
            }
        }
    }
    app.set_left_hidden(false);
    // A drag of the ISO's low end, to 6400: nothing during it, the
    // filter on release, and the ranges where they were.
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 800.0));
    settle(&app);
    settle(&app);
    let rows = header("RangeRow");
    let track = header("RangeSlider")[0];
    let y = track.1 + track.3 / 2.0;
    let stop = |i: f32| track.0 + 8.0 + (track.2 - 16.0) * i / 2.0;
    let pointer = |e: slint::platform::WindowEvent| app.window().dispatch_event(e);
    let at = |x: f32| slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
    use slint::platform::WindowEvent::{PointerMoved, PointerPressed, PointerReleased};
    pointer(PointerMoved {
        position: at(stop(0.0)),
    });
    pointer(PointerPressed {
        position: at(stop(0.0)),
        button,
    });
    pointer(PointerMoved {
        position: at(stop(1.0)),
    });
    pointer(PointerMoved {
        position: at(stop(2.0)),
    });
    assert_eq!(
        state.borrow().views.filter.text,
        "",
        "nothing during a drag"
    );
    pointer(PointerReleased {
        position: at(stop(2.0)),
        button,
    });
    settle(&app);
    settle(&app);
    assert_eq!(state.borrow().views.filter.text, "iso>=6400");
    assert_eq!(app.get_filter_text(), "iso>=6400");
    assert_eq!(header("RangeRow"), rows, "the ranges stay put");
    // Typed, the other way: the handles follow the text.
    app.invoke_filter_text_edited("focal:35".into());
    settle(&app);
    let focal = app.get_filter_focal_range();
    assert_eq!((focal.low, focal.high, focal.on), (1, 1, true));
    // One value: no range in the header.
    let mut one = app.get_filter_iso_range();
    one.values = ModelRc::new(VecModel::from(vec![slint::SharedString::from("100")]));
    one.keys = one.values.clone();
    (one.low, one.high) = (0, 0);
    app.set_filter_iso_range(one);
    settle(&app);
    assert_eq!(header("RangeRow").len(), 1);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A facet row longer than the header scrolls sideways under a
/// plain wheel, which is the only wheel most mice have.
#[test]
fn a_plain_wheel_scrolls_a_long_facet_row_sideways() {
    use slint::platform::WindowEvent;
    let app = crate::testing::window(0);
    app.window().set_size(slint::LogicalSize::new(800.0, 600.0));
    app.set_grid_open(true);
    let chips: Vec<FacetChip> = (0..40)
        .map(|i| FacetChip {
            text: format!("{}", 100 * (i + 1)).into(),
            key: format!("k{i}").into(),
            count: 1,
            on: false,
            out: false,
        })
        .collect();
    app.set_filter_facets_short(ModelRc::new(VecModel::from(vec![FacetRow {
        name: "ISO".into(),
        code: 2,
        chips: ModelRc::new(VecModel::from(chips)),
    }])));
    let pressed = Rc::new(RefCell::new(Vec::new()));
    let seen = pressed.clone();
    app.on_filter_facet_toggled(move |_, key, _| seen.borrow_mut().push(key.to_string()));
    // With no camera or lens row, the short facets take the
    // first row's place: the header's third, under the selection's
    // line and the meta chips with the words; the sheet starts at
    // the left pane's edge, 240 in.
    let (x, y) = (440.0, 92.0);
    crate::testing::click(&app, x, y);
    let position = slint::LogicalPosition::new(x, y);
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position,
        delta_x: 0.0,
        delta_y: -400.0,
    });
    // A pointer that has not moved since the wheel still points
    // at the chip that was under it, as far as Slint's hover is
    // concerned; a hand moves, and so does this.
    crate::testing::click(&app, x + 5.0, y);
    let pressed = pressed.borrow();
    assert_eq!(pressed.len(), 2, "{pressed:?}");
    let at = |k: &str| k[1..].parse::<i32>().unwrap();
    assert!(
        at(&pressed[1]) > at(&pressed[0]),
        "down the wheel is along the row"
    );
}

/// The indexer's thread: it makes the library, indexes the folders
/// asked for, and brings a row up to date after a save, saying
/// each as it goes — on a database of the test's own.
#[test]
fn the_indexer_indexes_a_folder_and_a_saved_sidecar() {
    let dir = scratch("indexer");
    let shoot = dir.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
    write_frame(&a, &R6, 1);
    write_frame(&b, &R5, 2);
    let db = dir.join("data").join("library.sqlite");
    let (tx, rx) = std::sync::mpsc::channel();
    let indexer = Indexer::start(db.clone(), move |told| {
        let _ = tx.send(told);
    })
    .expect("the indexer starts");
    let wait = |want: &dyn Fn(&Told) -> bool| loop {
        let told = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the indexer answers");
        if want(&told) {
            return told;
        }
    };
    assert!(matches!(wait(&|t| matches!(t, Told::Opened(_))), Told::Opened(p) if p == db));
    indexer.folders(folders_of(&[a.clone(), b.clone()]), 7);
    match wait(&|t| matches!(t, Told::Indexed { .. })) {
        Told::Indexed {
            generation, report, ..
        } => {
            assert_eq!(generation, 7);
            assert_eq!(report.added, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    let reader = Library::open_read_only(&db).unwrap();
    assert_eq!(reader.by_path(&a).unwrap().unwrap().meta.rating, 0);
    // The pass scored nothing; the scorer behind it scores both and
    // says where.
    match wait(&|t| matches!(t, Told::Scored(_))) {
        Told::Scored(folders) => assert_eq!(folders.len(), 1, "{folders:?}"),
        other => panic!("{other:?}"),
    }
    for p in [&a, &b] {
        assert!(reader.by_path(p).unwrap().unwrap().exif.focus.is_some());
    }

    // A rating saved, and the row follows once the indexer says so.
    let mut s = Sidecar::default();
    s.meta.rating = 4;
    s.save(&a).unwrap();
    indexer.file(a.clone());
    wait(&|t| matches!(t, Told::FilesIndexed(_)));
    assert_eq!(reader.by_path(&a).unwrap().unwrap().meta.rating, 4);
    indexer.stop(Duration::from_secs(20));
    drop(reader);
    crate::testing::remove_dir_retry(&dir);
}

/// The indexer's pass over a folder of raws, and the thumbnail and
/// the local preview the pool makes of each, map no raw: they walk
/// shares, and a mapped raw whose volume drops while the machine
/// sleeps is a SIGBUS nothing catches. The test DNGs carry no
/// preview, so the thumbnail is the quick develop, read whole; it
/// decodes, and stops at the color matrix the DNGs do not carry.
#[test]
fn the_indexers_pass_and_the_thumbnails_map_no_raw() {
    let dir = scratch("unmapped");
    let shoot = dir.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let (a, b) = (shoot.join("a.dng"), shoot.join("b.DNG"));
    write_mosaic_dng(&a, "RGGB");
    write_mosaic_dng(&b, "RGGB");
    greycard_core::decode::watch_maps_under(&shoot);
    let db = dir.join("data").join("library.sqlite");
    let (tx, rx) = std::sync::mpsc::channel();
    let indexer = Indexer::start(db.clone(), move |told| {
        let _ = tx.send(told);
    })
    .expect("the indexer starts");
    indexer.folders(folders_of(&[a.clone(), b.clone()]), 1);
    loop {
        match rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the indexer answers")
        {
            Told::Indexed { report, .. } => {
                assert_eq!(report.added, 2, "{report:?}");
                break;
            }
            _ => continue,
        }
    }
    indexer.stop(Duration::from_secs(20));
    for p in [&a, &b] {
        let made = crate::worker::thumbnail(p, crate::worker::THUMB_WIDTH);
        let e = made.expect_err("no color matrix").to_string();
        assert!(e.contains("calibration"), "{e}");
        assert!(crate::cull::camera_picture(p).is_err(), "no preview in it");
    }
    assert_eq!(greycard_core::decode::maps_under(&shoot), 0, "raws mapped");
    crate::testing::remove_dir_retry(&dir);
}

/// Two roots of two frames each under `dir`, `a` and `b`, and an
/// indexer on a library there with `guard`; what it tells, on a
/// channel.
/// The guard's wait in the tests that hang a pass. The hang is the
/// test's own, held until it lets go, so any wait sets that pass
/// aside; the wait is for the passes meant to land, which must never
/// go this long without a word. 300 ms did on a Windows runner (a
/// fresh TIFF scanned before it is read), and a pass meant to land
/// was set aside.
const SET_ASIDE_WAIT: Duration = Duration::from_secs(2);

fn two_roots(dir: &Path, guard: Guard) -> (PathBuf, PathBuf, Indexer, mpsc::Receiver<Told>) {
    let (a, b) = (dir.join("a"), dir.join("b"));
    for (root, seed) in [(&a, 1), (&b, 3)] {
        std::fs::create_dir_all(root.join("day")).unwrap();
        write_frame(&root.join("day").join("x.tif"), &R6, seed);
        write_frame(&root.join("day").join("y.tif"), &R5, seed + 1);
    }
    let (tx, rx) = mpsc::channel();
    let indexer = Indexer::start_with(
        dir.join("data").join("library.sqlite"),
        move |told| {
            let _ = tx.send(told);
        },
        guard,
    )
    .expect("the indexer starts");
    (a, b, indexer, rx)
}

fn wait_for(rx: &mpsc::Receiver<Told>, want: &dyn Fn(&Told) -> bool) -> Told {
    loop {
        let told = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the indexer answers");
        if want(&told) {
            return told;
        }
    }
}

/// A root that does not answer the look before its pass is skipped,
/// said once, its rows left as they are, and the other root's pass
/// and report come all the same.
#[test]
fn a_pass_over_a_root_that_does_not_answer_is_skipped_and_the_next_lands() {
    let dir = scratch("skipped");
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let guard = Guard {
        answers: Box::new(move |root| {
            counted.fetch_add(1, Ordering::SeqCst);
            !root.ends_with("a")
        }),
        ..Guard::default()
    };
    let (a, b, indexer, rx) = two_roots(&dir, guard);
    wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
    indexer.roots(vec![a.clone(), b.clone()]);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Background { .. })
    }) {
        Told::Skipped {
            path,
            root,
            launch,
            generation,
            first,
        } => {
            assert_eq!((path, root), (a.clone(), a.clone()));
            assert!(launch && first);
            assert_eq!(generation, None);
        }
        other => panic!("the root that did not answer comes first: {other:?}"),
    }
    match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
        Told::Background {
            path,
            report,
            error,
            ..
        } => {
            assert_eq!(path, b);
            assert_eq!(error, None);
            assert_eq!(report.added, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    // Asked again: skipped again, and not said again.
    indexer.asker().poll(vec![a.clone()]);
    match wait_for(&rx, &|t| matches!(t, Told::Skipped { .. })) {
        Told::Skipped { first, launch, .. } => assert!(!first && !launch),
        other => panic!("{other:?}"),
    }
    assert_eq!(asked.load(Ordering::SeqCst), 3, "one look a pass begun");
    let reader = Library::open_read_only(&dir.join("data").join("library.sqlite")).unwrap();
    assert_eq!(
        reader.count_under(&a).unwrap(),
        0,
        "nothing indexed under a"
    );
    assert_eq!(reader.count_under(&b).unwrap(), 2);
    drop(reader);
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// A root that answers the look and then hangs in the middle of its
/// walk: the pass is set aside once it has said nothing for the
/// wait, said, and the other root's pass and report come while it
/// is still stuck. The stuck root is skipped without a look while
/// its pass is out; let go, that pass finishes and its report lands
/// as any other.
#[test]
fn a_root_that_hangs_mid_walk_never_stops_the_other_roots_reports() {
    let dir = scratch("hangs");
    let released = Arc::new(AtomicBool::new(false));
    let let_go = released.clone();
    let hang_under = dir.join("a");
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let guard = Guard {
        answers: Box::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            true
        }),
        wait: SET_ASIDE_WAIT,
        each_file: Some(Arc::new(move |file: &Path| {
            while file.starts_with(&hang_under) && !let_go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        })),
    };
    let (a, b, indexer, rx) = two_roots(&dir, guard);
    wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
    let begun = Instant::now();
    indexer.roots(vec![a.clone(), b.clone()]);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Background { .. })
    }) {
        Told::Skipped {
            root,
            launch,
            first,
            ..
        } => {
            assert_eq!(root, a);
            assert!(launch && first);
        }
        other => panic!("{other:?}"),
    }
    match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
        Told::Background { path, report, .. } => {
            assert_eq!(path, b);
            assert_eq!(report.added, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !released.load(Ordering::SeqCst),
        "b's report came while a's walk was still stuck"
    );
    assert!(begun.elapsed() < Duration::from_secs(10));
    // While its thread is out: skipped at once, with no look.
    let looks = asked.load(Ordering::SeqCst);
    indexer.asker().poll(vec![a.clone()]);
    match wait_for(&rx, &|t| matches!(t, Told::Skipped { .. })) {
        Told::Skipped { first, .. } => assert!(!first),
        other => panic!("{other:?}"),
    }
    assert_eq!(asked.load(Ordering::SeqCst), looks);
    // Let go: the pass set aside finishes, and its report lands.
    released.store(true, Ordering::SeqCst);
    match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
        Told::Background {
            path,
            launch,
            report,
            error,
            ..
        } => {
            assert_eq!(path, a);
            assert!(!launch, "counted when it was set aside");
            assert_eq!(error, None);
            assert_eq!(report.added, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    // And the root is passed over again at the next tick, looked at.
    indexer.asker().poll(vec![a.clone()]);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Background { .. })
    }) {
        Told::Background { path, report, .. } => {
            assert_eq!(path, a);
            assert_eq!(report.unchanged, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(asked.load(Ordering::SeqCst), looks + 1);
    let reader = Library::open_read_only(&dir.join("data").join("library.sqlite")).unwrap();
    assert_eq!(reader.count_under(&a).unwrap(), 2);
    drop(reader);
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// The window hears a skipped pass: said on the status line once, a
/// launch pass skipped counts as done for a capture waiting on the
/// roots, and its own folder's pass skipped is said in the filter's
/// line, its wait over.
#[test]
fn the_window_says_a_skipped_root_once_and_counts_its_launch_pass() {
    use crate::testing::{state_for, window};
    let dir = scratch("skipped-said");
    let root = dir.join("archive");
    std::fs::create_dir_all(&root).unwrap();
    let app = window(0);
    let (state, _worker) = state_for(&app, Vec::new());
    let root = {
        let mut st = state.borrow_mut();
        st.views.library.roots.add(&root).unwrap();
        let root = st.views.library.roots.list()[0].clone();
        st.views.library.roots.set_name(&root, "Archive");
        st.views.library.launch_left = 1;
        st.views.library.awaiting = true;
        root
    };
    let skipped = |first: bool, launch: bool, generation: Option<u64>| Told::Skipped {
        path: root.clone(),
        root: root.clone(),
        launch,
        generation,
        first,
    };
    told(&app, skipped(true, true, None));
    assert!(
        app.get_status().starts_with("Archive isn't reachable"),
        "{}",
        app.get_status()
    );
    assert_eq!(state.borrow().views.library.launch_left, 0);
    assert!(!state.borrow().views.library.awaiting);
    app.set_status("something else".into());
    told(&app, skipped(false, false, None));
    assert_eq!(app.get_status(), "something else", "said once");
    {
        let mut st = state.borrow_mut();
        st.views.index_generation = 9;
        st.launch.awaiting_index = true;
        st.views.index_progress = Some((0, 3));
    }
    told(&app, skipped(false, false, Some(9)));
    let st = state.borrow();
    assert!(!st.launch.awaiting_index);
    assert_eq!(st.views.index_progress, None);
    assert!(
        st.views
            .index_error
            .as_deref()
            .is_some_and(|e| e.starts_with("Archive isn't reachable"))
    );
    drop(st);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A share mounted below a local root that hangs holds its own pass
/// and not the local root's: the local root's walk leaves it out, its
/// changes are passed over, and only the share's tick is set aside.
#[test]
fn a_hung_share_below_a_local_root_holds_only_its_own_pass() {
    let dir = scratch("below-hangs");
    let hang_under = dir.join("a").join("nas");
    let held = hang_under.clone();
    let released = Arc::new(AtomicBool::new(false));
    let let_go = released.clone();
    let guard = Guard {
        answers: Box::new(|_| true),
        wait: SET_ASIDE_WAIT,
        each_file: Some(Arc::new(move |file: &Path| {
            while file.starts_with(&held) && !let_go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        })),
    };
    let (a, b, indexer, rx) = two_roots(&dir, guard);
    std::fs::create_dir_all(&hang_under).unwrap();
    write_frame(&hang_under.join("n.tif"), &A7, 7);
    let nas = hang_under;
    wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
    // The mounts left out come in the launch's own ask.
    indexer.launch(vec![a.clone(), b.clone()], vec![nas.clone()]);
    for root in [&a, &b] {
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Background { path, report, .. } => {
                assert_eq!(&path, root);
                assert_eq!(report.added, 2, "the share left out: {report:?}");
            }
            other => panic!("nothing set aside: {other:?}"),
        }
    }
    // The share's tick hangs, and is set aside alone.
    indexer.asker().poll(vec![nas.clone()]);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Background { .. })
    }) {
        Told::Skipped { root, .. } => assert_eq!(root, nas),
        other => panic!("{other:?}"),
    }
    // A change in the local root, and the root's own tree, still pass.
    write_frame(&a.join("day").join("w.tif"), &R6, 8);
    indexer
        .asker()
        .changes(vec![Change::Folder(a.join("day")), Change::Tree(a.clone())]);
    for want in [a.join("day"), a.clone()] {
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Background { path, error, .. } => {
                assert_eq!(path, want);
                assert_eq!(error, None);
            }
            other => panic!("{other:?}"),
        }
    }
    released.store(true, Ordering::SeqCst);
    match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
        Told::Background { path, report, .. } => {
            assert_eq!(path, nas);
            assert_eq!(report.added, 1, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// A pass set aside on a tree below a root (a share's tick, the share
/// not known to be left out of the root's walk) keeps a pass over the
/// root from walking into it: that pass is skipped until the share's
/// is back, and then made.
#[test]
fn a_pass_over_a_tree_with_one_set_aside_in_it_waits() {
    let dir = scratch("tree-aside");
    let nas = dir.join("a").join("nas");
    let held = nas.clone();
    let released = Arc::new(AtomicBool::new(false));
    let let_go = released.clone();
    let guard = Guard {
        answers: Box::new(|_| true),
        wait: SET_ASIDE_WAIT,
        each_file: Some(Arc::new(move |file: &Path| {
            while file.starts_with(&held) && !let_go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        })),
    };
    let (a, _b, indexer, rx) = two_roots(&dir, guard);
    std::fs::create_dir_all(&nas).unwrap();
    write_frame(&nas.join("n.tif"), &A7, 7);
    wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
    indexer.asker().poll(vec![nas.clone()]);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Background { .. })
    }) {
        Told::Skipped { root, .. } => assert_eq!(root, nas),
        other => panic!("{other:?}"),
    }
    indexer.roots(vec![a.clone()]);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Background { .. })
    }) {
        Told::Skipped { path, .. } => assert_eq!(path, a, "not walked into the share"),
        other => panic!("{other:?}"),
    }
    released.store(true, Ordering::SeqCst);
    match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
        Told::Background { path, .. } => assert_eq!(path, nas),
        other => panic!("{other:?}"),
    }
    indexer.roots(vec![a.clone()]);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Background { .. })
    }) {
        Told::Background { path, report, .. } => {
            assert_eq!(path, a);
            assert_eq!(report.added, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// A launch pass set aside is counted once: the word that it was set
/// aside says launch, and when it lands it does not, so three roots,
/// the first set aside, are three launch passes done and no more.
#[test]
fn a_launch_pass_set_aside_is_counted_once() {
    let dir = scratch("launch-once");
    let released = Arc::new(AtomicBool::new(false));
    let let_go = released.clone();
    let hang_under = dir.join("a");
    let guard = Guard {
        answers: Box::new(|_| true),
        wait: SET_ASIDE_WAIT,
        each_file: Some(Arc::new(move |file: &Path| {
            while file.starts_with(&hang_under) && !let_go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        })),
    };
    let (a, b, indexer, rx) = two_roots(&dir, guard);
    let c = dir.join("c");
    std::fs::create_dir_all(&c).unwrap();
    write_frame(&c.join("z.tif"), &R6, 9);
    wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
    indexer.roots(vec![a.clone(), b.clone(), c.clone()]);
    let mut launches = Vec::new();
    let mut a_landed = None;
    while a_landed.is_none() {
        match wait_for(&rx, &|t| {
            matches!(t, Told::Skipped { .. } | Told::Background { .. })
        }) {
            Told::Skipped { path, launch, .. } => {
                if launch {
                    launches.push(path.clone());
                }
            }
            Told::Background { path, launch, .. } => {
                if launch {
                    launches.push(path.clone());
                }
                if path == a {
                    a_landed = Some(launch);
                } else if path == c {
                    // Every other root done: let a go.
                    released.store(true, Ordering::SeqCst);
                }
            }
            _ => {}
        }
    }
    assert_eq!(a_landed, Some(false), "a's pass lands as no launch pass");
    assert_eq!(launches, vec![a.clone(), b, c], "each root counted once");
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

/// The window's own folder pass, stuck the same way, is said to the
/// window with its generation, and the indexer goes on with the
/// passes in the background.
#[test]
fn a_folder_pass_that_hangs_is_said_and_the_indexer_goes_on() {
    let dir = scratch("folder-hangs");
    let released = Arc::new(AtomicBool::new(false));
    let let_go = released.clone();
    let hang_under = dir.join("a");
    let guard = Guard {
        answers: Box::new(|_| true),
        wait: SET_ASIDE_WAIT,
        each_file: Some(Arc::new(move |file: &Path| {
            while file.starts_with(&hang_under) && !let_go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        })),
    };
    let (a, b, indexer, rx) = two_roots(&dir, guard);
    wait_for(&rx, &|t| matches!(t, Told::Opened(_)));
    indexer.folders(vec![a.join("day")], 3);
    match wait_for(&rx, &|t| {
        matches!(t, Told::Skipped { .. } | Told::Indexed { .. })
    }) {
        Told::Skipped {
            path,
            generation,
            first,
            launch,
            ..
        } => {
            assert_eq!(path, a.join("day"));
            assert_eq!(generation, Some(3));
            assert!(first && !launch);
        }
        other => panic!("{other:?}"),
    }
    indexer.roots(vec![b.clone()]);
    match wait_for(&rx, &|t| matches!(t, Told::Background { .. })) {
        Told::Background { path, report, .. } => {
            assert_eq!(path, b);
            assert_eq!(report.added, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!released.load(Ordering::SeqCst));
    // Let go: the folder's pass set aside finishes, and says so with
    // its own generation.
    released.store(true, Ordering::SeqCst);
    match wait_for(&rx, &|t| matches!(t, Told::Indexed { .. })) {
        Told::Indexed {
            generation,
            report,
            error,
            ..
        } => {
            assert_eq!(generation, 3);
            assert_eq!(error, None);
            assert_eq!(report.added, 2, "{report:?}");
        }
        other => panic!("{other:?}"),
    }
    indexer.stop(Duration::from_secs(20));
    crate::testing::remove_dir_retry(&dir);
}

#[test]
fn a_facet_row_keeps_its_chips_on_and_caps_the_rest() {
    let counts: Vec<FacetCount> = (0..20)
        .map(|i| FacetCount {
            value: (100 * (i + 1)).to_string(),
            label: (100 * (i + 1)).to_string(),
            count: if i == 3 { 1 } else { 20 - i },
        })
        .collect();
    let on = vec!["400".to_string(), "99999".to_string()];
    let chips = chips_of(Facet::Iso, &counts, &on, &[], &|v| v.to_string());
    // Twelve by count, the one on kept past the cut, and a value
    // on that no frame holds any more, so it can be let go.
    assert_eq!(chips.len(), CHIPS_A_FACET + 2);
    assert!(chips.iter().any(|c| c.key == "400" && c.on));
    assert!(
        chips
            .iter()
            .any(|c| c.key == "99999" && c.on && c.count == 0)
    );
    // In the facet's own order, not the count's.
    let keys: Vec<i32> = chips
        .iter()
        .filter(|c| c.count > 0)
        .map(|c| c.key.parse().unwrap())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert_eq!(
        chips_of(Facet::Focal, &counts[..1], &[], &[], &|v| v.to_string())[0].text,
        "100 mm"
    );
    // A chip held out is kept past the cut as one on is, and says
    // which it is.
    let chips = chips_of(Facet::Iso, &counts, &[], &["2000".into()], &|v| {
        v.to_string()
    });
    assert_eq!(chips.len(), CHIPS_A_FACET + 1);
    assert!(chips.iter().any(|c| c.key == "2000" && c.out && !c.on));
}

/// Ties at the cut do not push out the values held more often:
/// a day's many one-frame focal lengths, first in the facet's
/// order, leave the 200 mm held three times on the row.
#[test]
fn a_facet_row_keeps_the_most_held_when_the_cut_is_a_tie() {
    let mut counts: Vec<FacetCount> = (1..=16)
        .map(|mm| FacetCount {
            value: mm.to_string(),
            label: mm.to_string(),
            count: 1,
        })
        .collect();
    for (mm, n) in [(120, 2), (200, 3), (400, 6)] {
        counts.push(FacetCount {
            value: mm.to_string(),
            label: mm.to_string(),
            count: n,
        });
    }
    let chips = chips_of(Facet::Focal, &counts, &[], &[], &|v| v.to_string());
    assert_eq!(chips.len(), CHIPS_A_FACET);
    for held in ["120", "200", "400"] {
        assert!(chips.iter().any(|c| c.key == held), "{held} mm kept");
    }
    // The rest of the room goes to the ties, in the facet's order.
    let ones: Vec<&str> = chips
        .iter()
        .filter(|c| c.count == 1)
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(ones, ["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
    // A keyword on that no frame holds is named as written.
    let chips = chips_of(Facet::Keyword, &[], &["harbor".into()], &[], &|_| {
        "Harbor".into()
    });
    assert_eq!(chips[0].text, "Harbor");
}

/// The timer's tick goes behind the launch pass, and is skipped for
/// a root whose whole pass is still waiting or stopped partway:
/// the launch's, or the last tick's.
#[test]
fn a_tick_waits_behind_the_launch_and_is_never_doubled() {
    let (share, local) = (PathBuf::from("/mnt/nas"), PathBuf::from("/home/p"));
    let mut q: VecDeque<Background> = VecDeque::new();
    queue(&mut q, Background::Root(share.clone()));
    queue(&mut q, Background::Root(local.clone()));
    // During the share's launch walk (in the queue, stopped or not
    // yet taken up): skipped.
    assert!(!queue_poll(&mut q, share.clone()));
    assert_eq!(q.len(), 2);
    // The launch walk done: a tick is queued, behind the local
    // root's launch pass.
    q.pop_front();
    assert!(queue_poll(&mut q, share.clone()));
    assert_eq!(
        Vec::from(q.clone()),
        vec![
            Background::Root(local.clone()),
            Background::Poll(share.clone())
        ]
    );
    // The next tick while that one is outstanding: skipped.
    assert!(!queue_poll(&mut q, share.clone()));
    assert_eq!(q.len(), 2);
    // A folder pass under the share is not a pass over the whole.
    q.clear();
    queue(
        &mut q,
        Background::Change(Change::Folder(share.join("day"))),
    );
    assert!(queue_poll(&mut q, share.clone()));
    assert_eq!(q.len(), 2);
    // A watcher's change still goes ahead of the tick.
    queue(&mut q, Background::Change(Change::Folder(local.join("x"))));
    assert_eq!(q.back(), Some(&Background::Poll(share)));
    // A share mounted below a local root: its tick is skipped while
    // the local root's launch pass, which walks it, is queued.
    q.clear();
    let below = local.join("NAS");
    queue(&mut q, Background::Root(local.clone()));
    assert!(!queue_poll(&mut q, below.clone()));
    q.clear();
    assert!(queue_poll(&mut q, below.clone()));
    assert_eq!(Vec::from(q), vec![Background::Poll(below)]);
}

/// A pass over the window's own folder says how far it has got at
/// every batch, and each word puts the filter on the window again:
/// the CULLING section holds still through them. A facet row scrolled
/// sideways stays scrolled, which a row made again at each word would
/// not (it would snap back to its start), and nothing under the rows
/// moves up or down, read from the handle found before with no walk
/// first, then at each frame of the next 160 ms. The pass's note comes
/// with its first word and stays while it runs: that line is the one
/// move, and it is made before the rest are watched.
#[test]
fn a_pass_ticking_moves_nothing_in_the_culling_section() {
    use crate::testing::{press, state_for, window};
    use i_slint_backend_testing::ElementHandle;
    let dir = scratch("cull-still");
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (files, mut sidecars, reader) = folder(&dir);
    // Keywords enough that their row runs past the panel and scrolls.
    let words: Vec<String> = (0..10).map(|k| format!("a long keyword {k}")).collect();
    sidecars[1].meta.set_keywords(words);
    let (state, _worker) = state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.sidecars = sidecars;
        st.views.index_reader = Some(reader);
        refresh_ids(&mut st);
        crate::panel::browser::rebuild_browser(&mut st, &app);
    }
    crate::STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    app.set_filter_bar(true);
    app.invoke_select(0);
    press(&app, "c");
    assert!(app.get_culling());
    crate::testing::settle(&app);
    assert!(
        app.get_filter_facets().row_count() > 1,
        "facet rows to keep"
    );
    // Under the chips and the facet rows, in the same section.
    let below = ElementHandle::find_by_element_id(&app, "CullSection::compare-words")
        .next()
        .expect("the Compare row");
    let generation = state.borrow().views.index_generation;
    let word = |done: usize| {
        told(
            &app,
            Told::Progress {
                generation,
                done,
                total: 8,
            },
        )
    };
    word(1);
    crate::testing::settle(&app);
    assert_ne!(app.get_filter_facet_note(), "", "the pass's note");
    // The facet rows in the panel, each its chips' left edge, top first:
    // where its sideways scroll has it.
    let panel_x = below.absolute_position().x;
    let rows = || -> Vec<(f32, f32)> {
        crate::testing::settle(&app);
        let mut found: Vec<(f32, f32)> = ElementHandle::find_by_element_id(&app, "FacetChips::row")
            .map(|r| (r.absolute_position().y, r.absolute_position().x))
            .filter(|(_, x)| *x >= panel_x - 40.0)
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found
    };
    // The row that runs past the panel: wider than its scroller.
    let flicks: Vec<ElementHandle> = ElementHandle::find_by_element_id(&app, "FacetChips::flick")
        .filter(|f| f.absolute_position().x >= panel_x - 40.0)
        .collect();
    let long = flicks
        .iter()
        .find(|f| {
            let y = f.absolute_position().y;
            ElementHandle::find_by_element_id(&app, "FacetChips::row").any(|r| {
                (r.absolute_position().y - y).abs() < 1.0 && r.size().width > f.size().width + 10.0
            })
        })
        .expect("a facet row past the panel");
    let unscrolled = rows();
    // Turned sideways by a wheel short of its end.
    let (at, size) = crate::testing::bounds_of(long);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(
                at.x + size.width / 2.0,
                at.y + size.height / 2.0,
            ),
            delta_x: 0.0,
            delta_y: -40.0,
        });
    let scrolled = rows();
    assert!(
        scrolled
            .iter()
            .zip(&unscrolled)
            .any(|(now, was)| now.1 < was.1 - 1.0),
        "no facet row scrolled sideways: {unscrolled:?} then {scrolled:?}"
    );
    // A row at its end hands the wheel on to the panel, which may have
    // scrolled too: the section is watched from where it is now.
    let before = below.absolute_position().y;
    for done in 2..=6 {
        word(done);
        for tick in 0..=10 {
            assert_eq!(
                below.absolute_position().y,
                before,
                "word {done}: the Compare row moved {} ms after it",
                tick * 16
            );
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
            slint::platform::update_timers_and_animations();
        }
        assert_eq!(rows(), scrolled, "word {done}: a facet row's scroll");
    }
    crate::STATE.with(|s| *s.borrow_mut() = None);
    drop(flicks);
    drop(below);
    crate::testing::remove_scratch(state, &dir);
}
