use super::*;
use crate::roots::{View, land_sent, open_view};
use crate::testing::{state_for, window};
use greycard_core::merge::Noise;
use greycard_core::merge::testing::{mosaic, write_dng};
use slint::Model;

/// A finished merge whose frames went without the learned tier
/// asked says why after its line, the card's or X-Trans; one with
/// nothing skipped says its line alone.
#[test]
fn a_merge_that_went_without_the_tier_says_why() {
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    let out = std::env::temp_dir().join("IMG_0-HDR.dng");
    let landed = |learned_skipped| {
        land(
            &state,
            &app,
            &worker,
            Done {
                frames: 3,
                made: Ok(Made {
                    out: out.clone(),
                    learned_skipped,
                    hash: None,
                    edit: None,
                }),
                lands: Lands::Beside { stack: None },
            },
        );
        app.get_status().to_string()
    };
    assert_eq!(
        landed(Some(merge::LearnedSkipped::OffCard)),
        "merged 3 frames into IMG_0-HDR.dng; no learned denoise on this graphics card"
    );
    assert_eq!(
        landed(Some(merge::LearnedSkipped::XTrans)),
        "merged 3 frames into IMG_0-HDR.dng; no learned denoise in an X-Trans merge"
    );
    assert_eq!(landed(None), "merged 3 frames into IMG_0-HDR.dng");
}

/// A frame the window has not read takes the setting for new edits
/// when it has no sidecar, sidecars on or off, or one that holds a
/// rating and no state (§724), and its own when one is recorded.
#[test]
fn a_frame_merged_unread_starts_as_the_setting_says() {
    let off = Fresh { lens: false };
    let none = Path::new("/nowhere/IMG_0001.CR3");
    assert!(!edit_on_disk(none, true, off).unwrap().lens.enabled);
    assert!(!edit_on_disk(none, false, off).unwrap().lens.enabled);
    assert_eq!(
        edit_on_disk(none, true, Fresh::default()).unwrap(),
        Edit::default()
    );
    let dir = crate::testing::scratch_dir("merge-fresh");
    let raw = dir.join("IMG_0002.CR3");
    std::fs::write(&raw, b"raw").unwrap();
    let mut written = Sidecar::default();
    written.meta.rating = 2;
    written.save(&raw).unwrap();
    assert!(
        !edit_on_disk(&raw, true, off).unwrap().lens.enabled,
        "a rating and no state: the setting's"
    );
    written.line = greycard_edit::Line::of(Edit::default(), 0).unwrap();
    written.save(&raw).unwrap();
    assert!(
        edit_on_disk(&raw, true, off).unwrap().lens.enabled,
        "the file's own"
    );
    crate::testing::remove_dir_retry(&dir);
}

/// A shoot of three bracketed frames, IMG_0 to IMG_2 (x0.26, x1,
/// x3.8), the middle one edited (an exposure, a demosaic), indexed,
/// on a window with the grid up over the root.
#[test]
fn a_bracket_is_merged_named_stacked_and_given_its_reference_edit() {
    let dir = crate::testing::scratch_dir("merge-hdr");
    let root = dir.join("photos");
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let (w, h) = (192, 128);
    let gains = [1.0 / 3.8, 1.0, 3.8];
    let shutters = [(1, 400), (1, 100), (1, 25)];
    let files: Vec<PathBuf> = (0..3)
        .map(|k| {
            let p = shoot.join(format!("IMG_{k}.dng"));
            write_dng(
                &p,
                mosaic(w, h, gains[k], Noise::ASSUMED, 9 + k as u64),
                w,
                h,
                shutters[k],
                "Bracket",
            );
            p
        })
        .collect();
    let mut edited = Edit::default();
    edited.light.exposure = 1.5;
    edited.demosaic = greycard_edit::Demosaic::Bilinear;
    Sidecar {
        line: Line::of(edited, 0).unwrap(),
        ..Sidecar::default()
    }
    .save(&files[1])
    .unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1400.0, 900.0));
    app.set_grid_open(true);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        // Sidecars read and written, as a user's window has them.
        st.window.write_sidecars = true;
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        let collections = dir.join(crate::collections::DIR);
        st.views.collections.dir = Some(collections.clone());
        st.views.stacks =
            crate::stacks::Store::load(greycard_library::stacks::dir_in(&collections));
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    assert_eq!(app.get_thumbs().row_count(), 3);

    // One frame chosen is no bracket.
    let cell = |i: usize| {
        let st = state.borrow();
        let f = st.file_by_path(&files[i]).unwrap();
        st.own_cell(f).unwrap()
    };
    let row = |c: usize| crate::panel::browser::row_of(&state.borrow(), c).unwrap() as i32;
    app.invoke_select(row(cell(0)));
    state.borrow_mut().views.picked = vec![cell(0)];
    app.invoke_merge_hdr();
    assert_eq!(app.get_status(), "choose two frames or more to merge");

    // All three, on a card the learned denoiser lost: an HDR asks
    // for no tier, so its line says nothing of it.
    let all = vec![cell(0), cell(1), cell(2)];
    state.borrow_mut().views.picked = all;
    crate::ai::set_test_off_card(Some(true));
    app.invoke_merge_hdr();
    crate::ai::set_test_off_card(None);
    let out = shoot.join("IMG_0-HDR.dng");
    assert_eq!(app.get_status(), "merged 3 frames into IMG_0-HDR.dng");
    assert!(out.is_file());
    assert!(!with_queue(|q| q.running));

    // Its edit: the reference's, less the exposure.
    let sidecar = Sidecar::load_as_file(&out)
        .unwrap()
        .expect("a sidecar beside it");
    assert_eq!(sidecar.current().light.exposure, 0.0);
    assert_eq!(
        sidecar.current().demosaic,
        greycard_edit::Demosaic::Bilinear
    );

    // On top of a stack of its sources, in the list's order.
    let stacks = state.borrow().views.stacks.list.clone();
    assert_eq!(stacks.len(), 1);
    let names: Vec<String> = stacks[0]
        .refs
        .iter()
        .map(|r| {
            Path::new(&r.hint)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        ["IMG_0-HDR.dng", "IMG_0.dng", "IMG_1.dng", "IMG_2.dng"]
    );

    // The stack is closed, its top the merge: the list shows it
    // alone.
    assert_eq!(app.get_thumbs().row_count(), 1);

    // Opened, and merged again: beside it, numbered, and stacked
    // anew, the sources leaving the first stack.
    {
        let mut st = state.borrow_mut();
        st.views.stacks.set_open(&stacks[0].id, true);
        crate::panel::stacks::settle(&mut st, &app, &worker);
    }
    state.borrow_mut().views.picked = vec![cell(0), cell(1), cell(2)];
    app.invoke_merge_hdr();
    assert_eq!(app.get_status(), "merged 3 frames into IMG_0-HDR-1.dng");
    assert!(shoot.join("IMG_0-HDR-1.dng").is_file());
    let stacks = state.borrow().views.stacks.list.clone();
    assert_eq!(stacks.len(), 1);
    assert!(stacks[0].refs[0].hint.ends_with("IMG_0-HDR-1.dng"));

    // Asked while another runs: it waits, and says so.
    {
        let mut st = state.borrow_mut();
        st.views.stacks.set_open(&stacks[0].id, true);
        crate::panel::stacks::settle(&mut st, &app, &worker);
    }
    with_queue(|q| q.running = true);
    state.borrow_mut().views.picked = vec![cell(0), cell(1)];
    app.invoke_merge_hdr();
    assert_eq!(app.get_status(), queued());
    with_queue(|q| {
        assert_eq!(q.waiting.len(), 1);
        q.waiting.clear();
        q.running = false;
    });

    // A session that writes no sidecars: none beside the file, its
    // edit held in memory and given to it once it is read.
    let all = vec![cell(0), cell(1), cell(2)];
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = false;
        st.views.picked = all;
        // The reference edited in the session, in memory alone.
        let f = st.file_by_path(&files[1]).unwrap();
        assert!(crate::rows::load_file(&mut st, &app, f));
        st.sidecars[f].line = Line::of(
            Edit {
                demosaic: greycard_edit::Demosaic::Vng4,
                ..Edit::default()
            },
            0,
        )
        .unwrap();
    }
    app.invoke_merge_hdr();
    let third = shoot.join("IMG_0-HDR-2.dng");
    assert_eq!(app.get_status(), "merged 3 frames into IMG_0-HDR-2.dng");
    assert!(Sidecar::load_as_file(&third).unwrap().is_none());
    assert!(state.borrow().work.merge_edits.contains_key(&third));
    writer.index_tree(&root, &mut |_| {}).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    let mut st = state.borrow_mut();
    let f = st.file_by_path(&third).expect("listed");
    assert!(crate::rows::load_file(&mut st, &app, f));
    assert!(st.work.merge_edits.is_empty());
    assert_eq!(st.sidecars[f].current().light.exposure, 0.0);
    assert_eq!(
        st.sidecars[f].current().demosaic,
        greycard_edit::Demosaic::Vng4
    );
    drop(st);

    drop(writer);
    drop(app);
    drop(worker);
    crate::testing::remove_scratch(state, &dir);
}

/// Two frames of one subject: Merge to Focus stack is offered only
/// with more than one frame chosen, refuses one, and with two
/// writes `<first> stack.dng` beside them, on top of a stack of
/// them, with the reference's edit less the learned denoiser.
#[test]
fn a_focus_stack_is_merged_named_stacked_and_given_its_reference_edit() {
    // The item, guarded as the other merges are.
    let menu =
        std::fs::read_to_string(Path::new(greycard_slint::PANEL_DIR).join("frame-menu.slint"))
            .unwrap();
    let item = menu
        .split("MenuItem {")
        .zip(menu.split("MenuItem {").skip(1))
        .find(|(_, body)| body.contains("root.merge-focus()"))
        .map(|(guard, _)| guard.rsplit('\n').next().unwrap().to_string())
        .expect("the item");
    assert!(item.contains("!root.queue && root.count > 1"), "{item}");

    let dir = crate::testing::scratch_dir("merge-focus");
    let root = dir.join("photos");
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let (w, h) = (192, 128);
    let files: Vec<PathBuf> = (0..2)
        .map(|k| {
            let p = shoot.join(format!("IMG_{k}.dng"));
            write_dng(
                &p,
                mosaic(w, h, 0.25, Noise::ASSUMED, 30 + k as u64),
                w,
                h,
                (1, 100),
                "Focus",
            );
            p
        })
        .collect();
    let mut edited = Edit::default();
    edited.light.exposure = 0.5;
    edited.noise.learned = Learned::Fast;
    for f in &files {
        Sidecar {
            line: Line::of(edited.clone(), 0).unwrap(),
            ..Sidecar::default()
        }
        .save(f)
        .unwrap();
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1400.0, 900.0));
    app.set_grid_open(true);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        let collections = dir.join(crate::collections::DIR);
        st.views.collections.dir = Some(collections.clone());
        st.views.stacks =
            crate::stacks::Store::load(greycard_library::stacks::dir_in(&collections));
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    assert_eq!(app.get_thumbs().row_count(), 2);
    let cell = |i: usize| {
        let st = state.borrow();
        let f = st.file_by_path(&files[i]).unwrap();
        st.own_cell(f).unwrap()
    };

    let row = |c: usize| crate::panel::browser::row_of(&state.borrow(), c).unwrap() as i32;

    // One frame chosen is no stack.
    app.invoke_select(row(cell(0)));
    state.borrow_mut().views.picked = vec![cell(0)];
    app.invoke_merge_focus();
    assert_eq!(app.get_status(), "choose two frames or more to merge");
    let out = shoot.join("IMG_0 stack.dng");
    assert!(!out.exists());

    // Both: each frame said as it is read, the file written.
    let heard = Rc::new(RefCell::new(Vec::<String>::new()));
    let into = heard.clone();
    HEARD.with(|h| {
        *h.borrow_mut() = Some(Box::new(move |line: &str| {
            into.borrow_mut().push(line.to_string())
        }))
    });
    state.borrow_mut().views.picked = vec![cell(0), cell(1)];
    app.invoke_merge_focus();
    HEARD.with(|h| *h.borrow_mut() = None);
    assert_eq!(app.get_status(), "merged 2 frames into IMG_0 stack.dng");
    assert!(out.is_file());
    assert_eq!(
        heard.borrow()[..2],
        ["merging frame 1 of 2", "merging frame 2 of 2"]
    );
    assert_eq!(
        heard.borrow().last().map(String::as_str),
        Some("merging: writing the file")
    );

    // Its edit: the reference's, its exposure kept, no learned
    // denoiser.
    let sidecar = Sidecar::load_as_file(&out)
        .unwrap()
        .expect("a sidecar beside it");
    assert_eq!(sidecar.current().light.exposure, 0.5);
    assert_eq!(sidecar.current().noise.learned, Learned::Off);

    // On top of a stack of its sources.
    let stacks = state.borrow().views.stacks.list.clone();
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].refs.len(), 3);
    assert!(stacks[0].refs[0].hint.ends_with("IMG_0 stack.dng"));

    drop(writer);
    drop(app);
    drop(worker);
    crate::testing::remove_scratch(state, &dir);
}

/// Two frames of a camera turned about its lens, the second chosen
/// first and the reference edited: Merge to Panorama names the file
/// after the first in the list, stacks it on top of its sources,
/// and gives it the reference's edit less the exposure and the crop.
#[test]
fn a_panorama_is_merged_named_stacked_and_given_its_reference_edit() {
    use greycard_core::merge::testing::{mosaic_of, turned, write_dng_shot};
    let dir = crate::testing::scratch_dir("merge-pano");
    let root = dir.join("photos");
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let (w, h) = (960, 640);
    let files: Vec<PathBuf> = [-0.2, 0.2]
        .iter()
        .enumerate()
        .map(|(k, &yaw)| {
            let p = shoot.join(format!("IMG_{k}.dng"));
            let samples = mosaic_of(w, h, 1.0, Noise::ASSUMED, 70 + k as u64, |x, y, c| {
                turned(x, y, c, w, h, 800.0, yaw)
            });
            write_dng_shot(&p, samples, (w, h), (1, 100), 100, "Pano", Some(30));
            p
        })
        .collect();
    // The reference of two is the first.
    let mut edited = Edit::default();
    edited.light.exposure = 0.7;
    edited.demosaic = greycard_edit::Demosaic::Bilinear;
    edited.geometry.angle = 2.0;
    Sidecar {
        line: Line::of(edited, 0).unwrap(),
        ..Sidecar::default()
    }
    .save(&files[0])
    .unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1400.0, 900.0));
    app.set_grid_open(true);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        let collections = dir.join(crate::collections::DIR);
        st.views.collections.dir = Some(collections.clone());
        st.views.stacks =
            crate::stacks::Store::load(greycard_library::stacks::dir_in(&collections));
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    assert_eq!(app.get_thumbs().row_count(), 2);
    let cell = |i: usize| {
        let st = state.borrow();
        let f = st.file_by_path(&files[i]).unwrap();
        st.own_cell(f).unwrap()
    };
    let row = |c: usize| crate::panel::browser::row_of(&state.borrow(), c).unwrap() as i32;
    app.invoke_select(row(cell(0)));
    state.borrow_mut().views.picked = vec![cell(0), cell(1)];
    app.invoke_merge_pano();
    let out = shoot.join("IMG_0-pano.dng");
    assert_eq!(app.get_status(), "merged 2 frames into IMG_0-pano.dng");
    assert!(out.is_file());
    let sidecar = Sidecar::load_as_file(&out)
        .unwrap()
        .expect("a sidecar beside it");
    assert_eq!(sidecar.current().light.exposure, 0.0);
    assert_eq!(
        sidecar.current().demosaic,
        greycard_edit::Demosaic::Bilinear
    );
    assert_eq!(sidecar.current().geometry, Default::default());
    let stacks = state.borrow().views.stacks.list.clone();
    assert_eq!(stacks.len(), 1);
    let names: Vec<String> = stacks[0]
        .refs
        .iter()
        .map(|r| {
            Path::new(&r.hint)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["IMG_0-pano.dng", "IMG_0.dng", "IMG_1.dng"]);
    assert_eq!(app.get_thumbs().row_count(), 1);
    drop(writer);
    drop(app);
    drop(worker);
    crate::testing::remove_scratch(state, &dir);
}

/// Four frames shot right to left in uneven steps, IMG_0 to IMG_3:
/// the list's middle, walked first, is IMG_1, the turn's middle
/// and the reference IMG_2. Each frame given in `edits` has that
/// edit in its sidecar. Merged to a panorama on a window, what
/// `heard` makes of the shoot's folder told each progress line, and
/// what is left to look at.
struct Turned {
    dir: PathBuf,
    out: PathBuf,
    folder: PathBuf,
    writer: greycard_library::Library,
    app: App,
    state: Rc<Shared>,
    worker: Rc<Worker>,
}

fn turned_merged(
    name: &str,
    edits: &[(usize, Edit)],
    heard: impl FnOnce(&Path) -> Box<dyn FnMut(&str)>,
) -> Turned {
    use greycard_core::merge::testing::{mosaic_of, turned, write_dng_shot};
    let dir = crate::testing::scratch_dir(name);
    let root = dir.join("photos");
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let (w, h) = (960, 640);
    let files: Vec<PathBuf> = [0.5, 0.3, 0.0, -0.3]
        .iter()
        .enumerate()
        .map(|(k, &yaw)| {
            let p = shoot.join(format!("IMG_{k}.dng"));
            let samples = mosaic_of(w, h, 1.0, Noise::ASSUMED, 70 + k as u64, |x, y, c| {
                turned(x, y, c, w, h, 800.0, yaw)
            });
            write_dng_shot(&p, samples, (w, h), (1, 100), 100, "Pano", Some(30));
            p
        })
        .collect();
    for (k, edit) in edits {
        Sidecar {
            line: Line::of(edit.clone(), 0).unwrap(),
            ..Sidecar::default()
        }
        .save(&files[*k])
        .unwrap();
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1400.0, 900.0));
    app.set_grid_open(true);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        let collections = dir.join(crate::collections::DIR);
        st.views.collections.dir = Some(collections.clone());
        st.views.stacks =
            crate::stacks::Store::load(greycard_library::stacks::dir_in(&collections));
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    let cells: Vec<usize> = {
        let st = state.borrow();
        files
            .iter()
            .map(|p| st.own_cell(st.file_by_path(p).unwrap()).unwrap())
            .collect()
    };
    let row = crate::panel::browser::row_of(&state.borrow(), cells[0]).unwrap() as i32;
    app.invoke_select(row);
    state.borrow_mut().views.picked = cells;
    let heard = heard(&shoot);
    HEARD.with(|h| *h.borrow_mut() = Some(heard));
    app.invoke_merge_pano();
    HEARD.with(|h| *h.borrow_mut() = None);
    Turned {
        out: shoot.join("IMG_0-pano.dng"),
        folder: shoot.join(".IMG_0-pano.dng.merging"),
        dir,
        writer,
        app,
        state,
        worker,
    }
}

impl Turned {
    /// The merged file's sidecar edit, and the names its stack
    /// holds, the file's first.
    fn landed(&self) -> (Edit, Vec<String>) {
        let sidecar = Sidecar::load_as_file(&self.out)
            .unwrap()
            .expect("a sidecar beside it");
        let stacks = self.state.borrow().views.stacks.list.clone();
        assert_eq!(stacks.len(), 1);
        let names = stacks[0]
            .refs
            .iter()
            .map(|r| {
                Path::new(&r.hint)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        (sidecar.current().clone(), names)
    }

    fn done(self) {
        drop(self.writer);
        drop(self.app);
        drop(self.worker);
        crate::testing::remove_scratch(self.state, &self.dir);
    }
}

/// Each progress line heard, and at each what the merge's folder
/// held (sorted) and whether the name had a file.
type Heard = Rc<RefCell<Vec<(String, Vec<String>, bool)>>>;

fn listening(shoot: &Path, heard: &Heard) -> Box<dyn FnMut(&str)> {
    let (folder, out) = (
        shoot.join(".IMG_0-pano.dng.merging"),
        shoot.join("IMG_0-pano.dng"),
    );
    let into = heard.clone();
    Box::new(move |line: &str| {
        let mut held: Vec<String> = std::fs::read_dir(&folder)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        held.sort();
        into.borrow_mut()
            .push((line.to_string(), held, out.exists()));
    })
}

fn writes(heard: &Heard) -> usize {
    heard
        .borrow()
        .iter()
        .filter(|(line, _, _)| line == "merging: writing the file")
        .count()
}

/// The look the reference gives the file: a contrast, kept by a
/// merge, and with `apart` a demosaic, which walks the frames
/// differently from the anchor's.
fn reference_edit(apart: bool) -> Edit {
    let mut edit = Edit::default();
    edit.light.tone.contrast = 20.0;
    if apart {
        edit.demosaic = greycard_edit::Demosaic::Bilinear;
    }
    edit
}

const STACKED: [&str; 5] = [
    "IMG_0-pano.dng",
    "IMG_0.dng",
    "IMG_1.dng",
    "IMG_2.dng",
    "IMG_3.dng",
];

/// The reference's edit walks as the anchor's: the panorama is
/// written once, under the folder, and renamed onto the name, with
/// the reference's look.
#[test]
fn a_panorama_whose_edits_walk_alike_is_written_once() {
    let heard = Heard::default();
    let t = turned_merged("merge-pano-alike", &[(2, reference_edit(false))], |shoot| {
        listening(shoot, &heard)
    });
    assert_eq!(t.app.get_status(), "merged 4 frames into IMG_0-pano.dng");
    assert_eq!(writes(&heard), 1);
    assert!(t.out.is_file());
    assert!(!t.folder.exists());
    let (look, stacked) = t.landed();
    assert_eq!(look.light.tone.contrast, 20.0);
    assert_eq!(stacked, STACKED);
    t.done();
}

/// The reference's edit walks apart from the anchor's: the first
/// run is written under the folder, nothing at the name, then the
/// second beside it, and only the second is renamed onto the name,
/// with the reference's look; the folder is gone after.
#[test]
fn a_panorama_whose_edits_walk_apart_is_written_under_its_folder() {
    let heard = Heard::default();
    let t = turned_merged("merge-pano-apart", &[(2, reference_edit(true))], |shoot| {
        listening(shoot, &heard)
    });
    assert_eq!(t.app.get_status(), "merged 4 frames into IMG_0-pano.dng");
    assert_eq!(writes(&heard), 2);
    // The second run's first line: the first file under the folder,
    // the name still free.
    let heard = heard.borrow();
    let second = heard
        .iter()
        .position(|(line, _, _)| line == "merging: writing the file")
        .map(|k| k + 1)
        .unwrap();
    let (line, held, at_name) = &heard[second];
    assert_eq!(line, "merging frame 1 of 4");
    assert!(held.iter().any(|n| n == FIRST), "{held:?}");
    assert!(!at_name);
    assert!(heard.iter().all(|(_, _, at_name)| !at_name));
    assert!(t.out.is_file());
    assert!(!t.folder.exists());
    let tag = greycard_core::decode::merge_tag_path(&t.out)
        .unwrap()
        .unwrap();
    assert_eq!(tag.reference, 2);
    let (look, stacked) = t.landed();
    assert_eq!(look.light.tone.contrast, 20.0);
    assert_eq!(look.demosaic, greycard_edit::Demosaic::Bilinear);
    assert_eq!(stacked, STACKED);
    t.done();
}

/// The second run fails (its file cannot be made: a folder stands
/// where it goes): the first is renamed onto the name instead,
/// with the anchor's look, the status says the merge as ever, and
/// the stack is made.
#[test]
fn a_panorama_whose_second_run_fails_keeps_its_first() {
    let heard = Heard::default();
    let t = turned_merged("merge-pano-fails", &[(2, reference_edit(true))], |shoot| {
        let folder = shoot.join(".IMG_0-pano.dng.merging");
        let mut hear = listening(shoot, &heard);
        Box::new(move |line: &str| {
            hear(line);
            let _ = std::fs::create_dir(folder.join(SECOND));
        })
    });
    assert_eq!(t.app.get_status(), "merged 4 frames into IMG_0-pano.dng");
    assert_eq!(writes(&heard), 2);
    assert!(t.out.is_file());
    assert!(!t.folder.exists());
    let (look, stacked) = t.landed();
    assert_eq!(
        look.light.tone.contrast,
        Edit::default().light.tone.contrast
    );
    assert_eq!(look.demosaic, Edit::default().demosaic);
    assert_eq!(stacked, STACKED);
    t.done();
}

/// A bracket of three merged, its file and sidecar beside the
/// frames and stacked on them, indexed and listed, on a window
/// whose worker opens frames for real and says so to the test, and
/// a merge of the old kind beside it, with no recipe.
struct Rig {
    dir: PathBuf,
    root: PathBuf,
    shoot: PathBuf,
    files: Vec<PathBuf>,
    out: PathBuf,
    old: PathBuf,
    writer: greycard_library::Library,
    app: App,
    state: Rc<Shared>,
    worker: Rc<Worker>,
    heard: std::sync::mpsc::Receiver<crate::worker::Outcome>,
}

fn rig(name: &str) -> Rig {
    let dir = crate::testing::scratch_dir(name);
    let root = dir.join("photos");
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let (w, h) = (192, 128);
    let gains = [1.0 / 3.8, 1.0, 3.8];
    let shutters = [(1, 400), (1, 100), (1, 25)];
    let files: Vec<PathBuf> = (0..3)
        .map(|k| {
            let p = shoot.join(format!("IMG_{k}.dng"));
            write_dng(
                &p,
                mosaic(w, h, gains[k], Noise::ASSUMED, 9 + k as u64),
                w,
                h,
                shutters[k],
                "Bracket",
            );
            p
        })
        .collect();
    let old = shoot.join("OLD-HDR.dng");
    greycard_core::merge::testing::write_merged(&files[0], &old);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    app.window()
        .set_size(slint::LogicalSize::new(1400.0, 900.0));
    app.set_grid_open(true);
    let (tx, heard) = std::sync::mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let worker = Rc::new(Worker::new(move |o| {
        let _ = tx.lock().unwrap().send(o);
    }));
    worker.set_thumb_cache(Some(greycard_library::thumbs::Thumbs::at(
        dir.join("thumbs"),
        greycard_library::thumbs::DEFAULT_CAP,
    )));
    let state = crate::testing::state_with(&app, Vec::new(), worker.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.sort = crate::sort::Kept::by_name();
        st.views.library.roots.add(&root).unwrap();
        let collections = dir.join(crate::collections::DIR);
        st.views.collections.dir = Some(collections.clone());
        st.views.stacks =
            crate::stacks::Store::load(greycard_library::stacks::dir_in(&collections));
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    let cells: Vec<usize> = {
        let st = state.borrow();
        files
            .iter()
            .map(|p| st.own_cell(st.file_by_path(p).unwrap()).unwrap())
            .collect()
    };
    let row = crate::panel::browser::row_of(&state.borrow(), cells[0]).unwrap() as i32;
    app.invoke_select(row);
    state.borrow_mut().views.picked = cells;
    app.invoke_merge_hdr();
    app.set_grid_open(false);
    let out = shoot.join("IMG_0-HDR.dng");
    assert_eq!(app.get_status(), "merged 3 frames into IMG_0-HDR.dng");
    writer.index_tree(&root, &mut |_| {}).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_sent(&state, &app, &worker);
    Rig {
        dir,
        root,
        shoot,
        files,
        out,
        old,
        writer,
        app,
        state,
        worker,
        heard,
    }
}

impl Rig {
    /// File `path` opened, its sidecar read, and the worker's open
    /// of it taken in as the window takes it.
    fn open(&self, path: &Path) {
        let c = {
            let mut st = self.state.borrow_mut();
            let f = st.file_by_path(path).expect("listed");
            assert!(crate::rows::load_file(&mut st, &self.app, f));
            st.own_cell(f).expect("a cell of its own")
        };
        crate::panel::browser::open_frame(&mut self.state.borrow_mut(), &self.app, &self.worker, c);
        assert!(!self.app.get_merge_again_shown(), "not until it is read");
        self.opened();
    }

    /// The worker's open of the frame the window opened last taken
    /// in as the window takes it: its open, its develop, and for a
    /// merge that keeps a recipe the word on making it again after
    /// them. What else it says is not this test's.
    fn opened(&self) {
        use crate::worker::Outcome;
        let (generation, path) = {
            let st = self.state.borrow();
            (
                st.develop.generation,
                st.path_at(st.current.unwrap()).unwrap().clone(),
            )
        };
        let asked = greycard_core::decode::merge_tag_path(&path)
            .ok()
            .flatten()
            .is_some_and(|t| t.recipe.is_some());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            assert!(std::time::Instant::now() < deadline, "waiting for the open");
            let Ok(outcome) = self
                .heard
                .recv_timeout(std::time::Duration::from_millis(100))
            else {
                continue;
            };
            let last = match &outcome {
                Outcome::Opened { generation: g, .. } if *g == generation => false,
                Outcome::Developed { generation: g, .. } if *g == generation => !asked,
                Outcome::MergeAgain { path: p, .. } if *p == path => true,
                Outcome::Failed { message, .. } => panic!("{message}"),
                _ => continue,
            };
            crate::panel::deliver::deliver(&self.app, outcome);
            if last {
                return;
            }
        }
    }

    fn done(self) {
        crate::WORKER.with(|w| *w.borrow_mut() = None);
        crate::STATE.with(|s| *s.borrow_mut() = None);
        // The pool's threads too, or one still making a picture
        // writes into the folder after it is removed.
        assert!(self.worker.stop_all(std::time::Duration::from_secs(60)));
        drop(self.writer);
        drop(self.app);
        drop(self.worker);
        crate::testing::remove_scratch(self.state, &self.dir);
        let left = crate::testing::left_in(&self.dir);
        assert!(left.is_empty(), "nothing left behind: {left:?}");
    }
}

/// The Noise section's row is offered on a merge that keeps its
/// recipe, its frames beside it, in a session that writes sidecars,
/// once the worker has said so after the develop; and on nothing
/// else: a merge written before the recipe, the same merge with
/// sidecars off, or with a frame gone.
#[test]
fn the_row_is_offered_on_a_merge_it_can_make_again() {
    let r = rig("merge-again-row");
    let row = "NoiseSection::merge-again-button";
    r.app.set_collapsed_noise(false);
    r.open(&r.out);
    assert!(r.app.get_merge_again_shown());
    // On the panel, below the learned tier's sliders.
    // In the panel's own terms: each lookup scrolls it its own way.
    let (at, _) = crate::testing::by_id(&r.app, row);
    let at = at.y - r.app.get_panel_scroll();
    let (blend, _) = crate::testing::by_id(&r.app, "NoiseSection::chroma-slider");
    let blend = blend.y - r.app.get_panel_scroll();
    assert!(at > blend, "{at} under {blend}");

    r.open(&r.old);
    assert!(!r.app.get_merge_again_shown());

    r.state.borrow_mut().window.write_sidecars = false;
    r.open(&r.out);
    assert!(!r.app.get_merge_again_shown());
    r.state.borrow_mut().window.write_sidecars = true;

    let aside = r.shoot.join("aside.dng");
    greycard_core::testing::rename_retry(&r.files[2], &aside);
    r.open(&r.out);
    assert!(!r.app.get_merge_again_shown());
    greycard_core::testing::rename_retry(&aside, &r.files[2]);
    r.open(&r.out);
    assert!(r.app.get_merge_again_shown());
    let _ = &r.root;
    r.done();
}

/// Pressed, the row makes the open merge again with its frames
/// denoised (the network the plain demosaic), over its file: the
/// status says the merge's lines, the file keeps its name, its
/// sidecar, and its place on top of its stack, the old file's
/// pictures leave the cache, and the frame is opened again under
/// the edit it had, the done line kept before its develop's. The
/// row's tier is the window's: the edit keeps no learned tier, and
/// the develop has no denoise to complain of. With a frame gone it
/// is refused in the engine's words and nothing is written.
#[test]
fn the_row_makes_the_merge_again_and_the_picture_reloads() {
    let r = rig("merge-again-press");
    {
        let mut sidecar = Sidecar::load_as_file(&r.out).unwrap().unwrap();
        let mut edit = sidecar.current().clone();
        edit.light.exposure = 0.4;
        sidecar.line = Line::of(edit, 0).unwrap();
        sidecar.save(&r.out).unwrap();
    }
    let sidecar = Sidecar::path_for(&r.out);
    let sidecar_before = std::fs::read(&sidecar).unwrap();
    r.open(&r.out);
    assert!(r.app.get_merge_again_shown());
    assert_eq!(r.app.get_merge_again_tier(), "off", "the recipe ran none");
    let generation = r.state.borrow().develop.generation;

    // A picture of the old file in the cache, under its key.
    let (old_hash, old_stamp) = crate::worker::thumb_key(&r.out).unwrap();
    let tag = greycard_library::thumbs::Tag {
        recipe: crate::worker::thumb_recipe(&r.out),
        stamp: old_stamp,
    };
    let thumb = greycard_library::thumbs::Thumb {
        width: 1,
        height: 1,
        rgb: vec![9, 9, 9],
    };
    let has = |hash: &str| {
        r.worker
            .thumb_cache()
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .has(hash, 77, tag)
    };
    r.worker
        .thumb_cache()
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .put(&old_hash, 77, tag, &thumb)
        .unwrap();
    assert!(has(&old_hash));

    r.app.set_merge_again_tier("balanced".into());
    r.app.invoke_merge_again();
    assert_eq!(r.app.get_status(), "merged 3 frames into IMG_0-HDR.dng");
    assert!(!r.app.get_merge_running());
    let made = greycard_core::decode::merge_tag_path(&r.out)
        .unwrap()
        .expect("our tag");
    assert!(made.denoised);
    assert_eq!(
        made.recipe.expect("a recipe").denoise.as_deref(),
        Some("balanced")
    );
    let names: Vec<String> = std::fs::read_dir(&r.shoot)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.') || n.contains("-HDR-"))
        .collect();
    assert!(names.is_empty(), "{names:?}");
    assert_eq!(std::fs::read(&sidecar).unwrap(), sidecar_before);
    assert!(!has(&old_hash));

    // On top of its stack under its new hash.
    let new_hash = greycard_library::hash_file(&r.out).unwrap();
    assert_ne!(new_hash, old_hash);
    let stacks = r.state.borrow().views.stacks.list.clone();
    assert_eq!(stacks.len(), 1);
    assert!(stacks[0].refs[0].hint.ends_with("IMG_0-HDR.dng"));
    assert_eq!(stacks[0].refs[0].hash, new_hash);

    // Opened again, under the edit it had.
    assert!(r.state.borrow().develop.generation > generation);
    r.opened();
    assert!(r.app.get_merge_again_shown());
    // The merge's words, put back by the develop that landed, and
    // no word of a denoise the recipe left off.
    assert_eq!(r.app.get_status(), "merged 3 frames into IMG_0-HDR.dng");
    assert_eq!(r.app.get_merge_again_tier(), "balanced", "the recipe's");
    assert_eq!(
        Sidecar::load_as_file(&r.out)
            .unwrap()
            .unwrap()
            .current()
            .noise
            .learned,
        Learned::Off
    );
    assert_eq!(r.app.get_denoise_learned(), "off");
    {
        let st = r.state.borrow();
        assert_eq!(st.develop.edit.light.exposure, 0.4);
        assert_eq!(
            st.path_at(st.current.unwrap()).unwrap().as_path(),
            r.out.as_path()
        );
    }
    assert_eq!(std::fs::read(&sidecar).unwrap(), sidecar_before);

    // A frame gone: refused, and the file as it was.
    let before = std::fs::read(&r.out).unwrap();
    let aside = r.shoot.join("aside.dng");
    greycard_core::testing::rename_retry(&r.files[0], &aside);
    r.app.invoke_merge_again();
    assert_eq!(
        r.app.get_status(),
        format!(
            "not merged: IMG_0.dng is not beside {}: a merge is made again from its \
             frames, in its folder",
            r.out.display()
        )
    );
    assert!(!r.app.get_merge_running());
    assert_eq!(std::fs::read(&r.out).unwrap(), before);
    greycard_core::testing::rename_retry(&aside, &r.files[0]);
    r.done();
}

/// A frame whose masks wait on a shape its raw's header cannot say, the
/// one a merge's new file would start from, is not merged, and the
/// status line says why in the approved words (§724).
#[test]
fn a_merge_from_a_frame_whose_masks_wait_says_why_it_was_not_merged() {
    let dir = std::env::temp_dir().join(format!("greycard-merge-waits-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("IMG_0001.CR3");
    std::fs::write(&raw, b"not a raw").unwrap();
    let mut s = crate::testing::waiting_sidecar();
    s.save(&raw).unwrap();
    let e = edit_on_disk(&raw, true, Fresh::default()).unwrap_err();
    assert_eq!(
        tr!("not merged: {why}", why = format!("{e:#}")),
        format!(
            "not merged: {}: its masks cannot be placed until its raw is reachable",
            raw.display()
        )
    );
    crate::testing::remove_dir_retry(&dir);
}
