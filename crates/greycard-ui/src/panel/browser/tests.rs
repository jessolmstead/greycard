use super::*;
use crate::testing::{press, window};
use greycard_edit::geometry::Geometry;
use greycard_edit_io::xmp;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;

/// The neighbors the worker is told of start from the frames the
/// right and the left keys reach, under the filter: a frame it hides
/// is passed over, and the ends of the strip have fewer. A hidden
/// frame's are those of the nearest row shown, as the keys step from
/// there. In order: the next ahead, the one behind, then two more
/// ahead. Each comes with its own line's edit and turn.
#[test]
fn the_neighbors_are_the_frames_the_arrow_keys_reach() {
    use crate::worker::Side::{Next, Previous};
    let shown = [0, 2, 5, 7];
    let right = |c| beside(&shown, c, Next);
    assert_eq!(right(2), vec![(5, Next, 1), (0, Previous, 1), (7, Next, 2)]);
    assert_eq!(right(0), vec![(2, Next, 1), (5, Next, 2), (7, Next, 3)]);
    assert_eq!(right(7), vec![(5, Previous, 1)]);
    assert_eq!(right(3), vec![(7, Next, 1), (2, Previous, 1)]);
    assert_eq!(right(9), vec![(7, Next, 1), (5, Previous, 1)]);
    assert_eq!(beside(&[4], 4, Next), vec![]);
    assert_eq!(beside(&[4], 3, Next), vec![(4, Next, 1)]);
    assert_eq!(beside(&[], 4, Next), vec![]);

    let left = |c| beside(&shown, c, Previous);
    assert_eq!(
        left(7),
        vec![(5, Previous, 1), (2, Previous, 2), (0, Previous, 3)]
    );
    assert_eq!(
        left(3),
        vec![(2, Previous, 1), (7, Next, 1), (0, Previous, 2)]
    );
    assert_eq!(left(0), vec![(2, Next, 1)]);

    let app = window(5);
    let files = crate::testing::folder(5);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let mut st = state.borrow_mut();
    st.views.shown = vec![0, 1, 3, 4];
    st.sidecars[3].turn = 1;
    {
        let mut e = st.line_mut(3).expect("a line").current().clone();
        e.light.exposure = 0.5;
        st.line_mut(3).expect("a line").record(e);
    }
    let near = neighbors_of(&st, 1);
    let got: Vec<_> = near
        .iter()
        .map(|n| (n.path.clone(), n.side, n.turn, n.edit.light.exposure))
        .collect();
    assert_eq!(
        got,
        vec![
            (files[3].clone(), Next, 1, 0.5),
            (files[0].clone(), Previous, 0, 0.0),
            (files[4].clone(), Next, 0, 0.0)
        ]
    );
    assert_eq!(
        near.iter().map(|n| n.steps).collect::<Vec<_>>(),
        vec![1, 1, 2]
    );
    assert_eq!(near[0].seed_blend, crate::rows::wants_seed(&st, 3));

    assert!(near.iter().all(|n| n.read_first.is_none()), "all read");

    // Listed from the index, the sidecars standing in from their
    // rows: one whose row says it holds no develop goes under the
    // stand-in's default edit at the turn its row shows taken back;
    // one whose row says it is edited is sent for the worker to read
    // first, under the stand-in's edit until then.
    st.window.write_sidecars = true;
    let stand_in = |edited: bool, turns: (u8, bool)| crate::rows::FromRow {
        read: false,
        turns,
        edited,
        ..crate::rows::FromRow::read()
    };
    st.from_row = (0..5).map(|_| crate::rows::FromRow::read()).collect();
    st.from_row[3] = stand_in(false, (3, false));
    st.from_row[0] = stand_in(true, (0, false));
    st.line_mut(3).expect("a line").record(Edit::default());
    let near = neighbors_of(&st, 1);
    assert_eq!(near.len(), 3);
    assert_eq!(near[2].path, files[4]);
    assert_eq!(
        (near[0].path.clone(), near[0].side, near[0].turn),
        (files[3].clone(), Next, 1)
    );
    assert_eq!(near[0].edit, Edit::default());
    assert!(near[0].seed_blend, "a raw never developed, by its row");
    assert!(near[0].read_first.is_none(), "the row's word taken");
    assert_eq!(
        (near[1].path.clone(), near[1].side),
        (files[0].clone(), Previous)
    );
    assert_eq!(
        near[1].read_first,
        Some(crate::worker::ReadFirst { version: None }),
        "the edited stand-in read first"
    );
    // A mirror the row shows is no default geometry: read first.
    st.from_row[3] = stand_in(false, (1, true));
    let near = neighbors_of(&st, 1);
    assert_eq!(near.len(), 3);
    assert!(near[0].read_first.is_some());
    // Not by the worker: with sidecars off, or after a read of it
    // failed. A network mount is the worker's to ask.
    let read_firsts = |st: &State| {
        neighbors_of(st, 1)
            .into_iter()
            .filter(|n| n.read_first.is_some())
            .map(|n| n.path)
            .collect::<Vec<_>>()
    };
    assert_eq!(read_firsts(&st), vec![files[3].clone(), files[0].clone()]);
    st.from_row[3].unread = Some("it did not read".into());
    assert_eq!(read_firsts(&st), vec![files[0].clone()]);
    assert_eq!(neighbors_of(&st, 1).len(), 2, "left out");
    st.window.write_sidecars = false;
    assert!(read_firsts(&st).is_empty());
}

/// The neighbors as sent and as they are now differ when one sent to
/// be read first is now read by the window, its edit's base the
/// same: the worker is told again.
#[test]
fn a_neighbor_read_first_then_read_is_sent_again() {
    let n = |read_first| crate::worker::Neighbor {
        path: "/nowhere/IMG_0001.CR3".into(),
        edit: Edit::default(),
        turn: 0,
        seed_blend: false,
        side: crate::worker::Side::Next,
        steps: 1,
        read_first,
    };
    let sent = NeighborsSent {
        generation: 1,
        open: "/nowhere/IMG_0000.CR3".into(),
        neighbors: vec![n(Some(crate::worker::ReadFirst { version: None }))],
        filter: String::new(),
        sort: crate::sort::Choice::Hand,
    };
    assert!(sent.serves(&[n(Some(crate::worker::ReadFirst { version: None }))]));
    assert!(!sent.serves(&[n(None)]));
    assert!(!sent.serves(&[n(Some(crate::worker::ReadFirst {
        version: Some("v".into())
    }))]));
}

/// A virtual copy's cell standing in from its row is read first from
/// its own line: the worker is told the version.
#[test]
fn a_version_standing_in_is_read_first_by_its_version() {
    let app = window(3);
    let files = crate::testing::folder(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let mut st = state.borrow_mut();
    st.window.write_sidecars = true;
    let id = st.sidecars[2]
        .new_version(None, Some("Warm"))
        .expect("a version");
    let copy = crate::versions::Cell {
        version: Some(std::sync::Arc::from(id.as_str())),
        ..st.cells[2].clone()
    };
    st.cells.push(copy);
    st.views.shown = vec![0, 1, 3];
    st.from_row = (0..3)
        .map(|_| crate::rows::FromRow {
            read: false,
            ..crate::rows::FromRow::read()
        })
        .collect();
    let near = neighbors_of(&st, 1);
    let copy = near
        .iter()
        .find(|n| n.path == files[2])
        .expect("the copy sent");
    assert_eq!(
        copy.read_first,
        Some(crate::worker::ReadFirst {
            version: Some(std::sync::Arc::from(id.as_str()))
        })
    );
}

/// A step is a move to the frame the right or the left key reaches;
/// one step back keeps the walk's direction, a second turns it, and a
/// back-and-forth compare never does. A jump keeps the direction and
/// forgets the steps against it.
#[test]
fn the_walk_turns_after_two_steps_the_other_way() {
    use crate::worker::Side::{Next, Previous};
    let shown = [0, 2, 5, 7];
    assert_eq!(step_side(&shown, 2, 5), Some(Next));
    assert_eq!(step_side(&shown, 5, 2), Some(Previous));
    assert_eq!(step_side(&shown, 2, 7), None);
    assert_eq!(step_side(&shown, 2, 2), None);

    let mut walk = Walk::default();
    assert_eq!(walk.ahead, Next);
    walk.step(Some(Previous));
    assert_eq!(walk.ahead, Next, "one step back is a look");
    walk.step(Some(Previous));
    assert_eq!(walk.ahead, Previous, "the second turns it");
    for side in [Next, Previous, Next, Previous, Next] {
        walk.step(Some(side));
        assert_eq!(walk.ahead, Previous, "a compare, back and forth");
    }
    walk.step(None);
    walk.step(Some(Next));
    assert_eq!(walk.ahead, Previous, "the jump forgot the step before it");
    walk.step(Some(Next));
    assert_eq!(walk.ahead, Next);

    // The streak a shelf for one base waits on: two steps ahead in a
    // row; a compare of two frames never reaches it.
    let mut walk = Walk::default();
    walk.step(Some(Next));
    assert_eq!(walk.heading().streak, 1);
    walk.step(Some(Next));
    assert_eq!(walk.heading().streak, 2);
    for side in [Previous, Next, Previous, Next] {
        walk.step(Some(side));
        assert!(walk.heading().streak < 2, "a compare");
        assert_eq!(walk.ahead, Next);
    }
    walk.step(Some(Previous));
    walk.step(Some(Previous));
    assert_eq!(walk.heading().ahead, Previous);
    assert_eq!(walk.heading().streak, 2, "turned by two steps");
    walk.step(None);
    assert_eq!(walk.heading().streak, 0, "a jump");

    // Opened in the editor: two steps left turn the neighbors sent.
    let app = window(6);
    let files = crate::testing::folder(6);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let mut st = state.borrow_mut();
    st.views.shown = (0..6).collect();
    for (i, ahead) in [(4, Next), (3, Next), (2, Previous), (3, Previous)] {
        walked_to(&mut st, i);
        assert_eq!(st.develop.walk.ahead, ahead, "at {i}");
    }
    let near: Vec<_> = neighbors_of(&st, 3)
        .into_iter()
        .map(|n| (n.path, n.side, n.steps))
        .collect();
    assert_eq!(
        near,
        vec![
            (files[2].clone(), Previous, 1),
            (files[4].clone(), Next, 1),
            (files[1].clone(), Previous, 2),
            (files[0].clone(), Previous, 3),
        ]
    );
    // The same frame opened again is no step and no jump: the step
    // right before it and the one after turn the walk.
    walked_to(&mut st, 3);
    assert_eq!(st.develop.walk.ahead, Previous);
    walked_to(&mut st, 4);
    assert_eq!(st.develop.walk.ahead, Next);
}

/// A filter change while a frame is open in the editor sends the
/// worker the neighbors the keys reach now, in place of those its
/// open sent, once: the same filter again, or a move that leaves
/// them be, sends nothing.
#[test]
fn a_filter_change_sends_the_neighbors_again() {
    use crate::worker::Side::{Next, Previous};
    let app = window(5);
    let files = crate::testing::folder(5);
    let (state, worker) = crate::testing::state_for(&app, files.clone());
    let generation = 4;
    // The frame is open before its neighbors arrive, as an open
    // job makes it: a list that finds no frame open is dropped at
    // once, and the worker's thread can run between the two.
    worker.set_open_frame(Some(crate::versions::Key::file(files[2].clone())));
    {
        let mut st = state.borrow_mut();
        rebuild_browser(&mut st, &app);
        st.current = Some(2);
        // What the open of frame 2 sent, as `open_frame` sends it.
        worker.neighbors(
            generation,
            crate::worker::Heading::steady(Next),
            neighbors_of(&st, 2),
        );
        st.develop.neighbors_sent = Some(NeighborsSent::of(&st, 2, generation));
    }
    assert_eq!(
        worker.neighbors_waiting(),
        Some((
            generation,
            vec![
                (files[3].clone(), Next),
                (files[1].clone(), Previous),
                (files[4].clone(), Next)
            ]
        ))
    );

    // Frames 1 and 3 rejected, and the filter to No rejects.
    {
        let mut st = state.borrow_mut();
        st.sidecars[1].meta.flag = meta::Flag::Reject;
        st.sidecars[3].meta.flag = meta::Flag::Reject;
        st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
    }
    crate::panel::cull::apply_filter(&state, &app);
    assert_eq!(state.borrow().views.shown, vec![0, 2, 4]);
    assert_eq!(
        worker.neighbors_waiting(),
        Some((
            generation,
            vec![(files[4].clone(), Next), (files[0].clone(), Previous)]
        ))
    );

    // Once per change: a list put in its place stays through the
    // same filter applied again.
    worker.neighbors(generation, crate::worker::Heading::steady(Next), Vec::new());
    crate::panel::cull::apply_filter(&state, &app);
    assert_eq!(worker.neighbors_waiting(), Some((generation, Vec::new())));
}

/// A row of the browser and a file of the folder are two
/// different numbers whenever a filter is on. The badges wrote
/// the file's number into the list, so a star set under "No
/// rejects" landed on whichever frame happened to be that many
/// rows down — someone else's thumbnail, not an index off the
/// end, which is why nothing ever panicked over it.
#[test]
fn a_badge_set_under_a_filter_lands_on_the_filtered_row() {
    use meta::{Change, Flag, Label};
    let app = window(0);
    let files: Vec<PathBuf> = ["a.CR3", "b.CR3", "c.CR3", "d.CR3", "e.CR3"]
        .iter()
        .map(|n| PathBuf::from("/nowhere").join(n))
        .collect();
    let mut st = State::empty(files, &app);
    // One reject at the front, so every row is the file one
    // along: row 0 is file 1, and the file numbers 1 to 4 are
    // all rows that exist. The old code wrote file 1's badge at
    // row 1, which is file 2 — a neighbor, in range, wrong.
    st.sidecars[0].meta.flag = Flag::Reject;
    st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
    rebuild_browser(&mut st, &app);
    assert_eq!(st.views.shown, vec![1, 2, 3, 4]);
    assert_eq!(app.get_thumbs().row_count(), 4);

    // Four stars on file 1: row 0, and nothing on row 1.
    set_meta(&mut st, &app, &[1], Change::Rating(4));
    let rows = app.get_thumbs();
    assert_eq!(rows.row_data(0).unwrap().name, "b.CR3");
    assert_eq!(rows.row_data(0).unwrap().rating, 4, "the file's own row");
    assert_eq!(
        rows.row_data(1).unwrap().rating,
        0,
        "and not its neighbor's"
    );
    assert!(
        (2..4).all(|r| rows.row_data(r).unwrap().rating == 0),
        "and nobody else's"
    );

    // A label on the last file shown, which is file 4 at row 3.
    set_meta(&mut st, &app, &[4], Change::Label(Label::Red));
    let rows = app.get_thumbs();
    assert_eq!(rows.row_data(3).unwrap().name, "e.CR3");
    assert_eq!(rows.row_data(3).unwrap().label, Label::Red.code());
    assert!(
        (0..3).all(|r| rows.row_data(r).unwrap().label == 0),
        "the label went to one row"
    );
    // And the star set a moment ago is still where it was put.
    assert_eq!(rows.row_data(0).unwrap().rating, 4);
}

#[test]
fn a_rating_from_another_tool_arrives_and_the_folder_is_left_alone() {
    let dir = std::env::temp_dir().join(format!("greycard-xmp-load-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let theirs = dir.join("IMG_0001.CR3");
    let mine = dir.join("IMG_0002.CR3");
    for f in [&theirs, &mine] {
        std::fs::write(f, b"raw").expect("a file");
    }
    let three = Meta {
        rating: 3,
        ..Meta::default()
    };

    // Rated in Lightroom, never opened here: the XMP is all
    // there is, so its word is taken.
    std::fs::write(xmp::short_path(&theirs), xmp::fresh(&three, None)).expect("an xmp");
    let (sidecars, _) = load_sidecars(std::slice::from_ref(&theirs), true);
    assert_eq!(sidecars[0].meta, three);
    // And nothing was written into somebody else's folder for
    // it: the meta rides in memory until the frame is saved.
    assert!(
        !Sidecar::path_for(&theirs).exists(),
        "opening a folder deposits nothing"
    );

    // Cleared here after the XMP was taken, and saved: the XMP
    // still says three and is older than nothing in particular,
    // but it is the same XMP that was read, so it has nothing
    // new to say and the three do not come back.
    let mut sidecar = Sidecar::default();
    std::fs::write(xmp::short_path(&mine), xmp::fresh(&three, None)).expect("an xmp");
    assert!(greycard_edit_io::xmp::adopt(&mine, &mut sidecar, || None).moved());
    sidecar.meta.set_rating(0);
    sidecar.save(&mine).expect("the sidecar writes");
    set_modified(&xmp::short_path(&mine), 2_000_000_000);
    set_modified(&Sidecar::path_for(&mine), 1_000_000_000);
    assert_eq!(
        load_sidecars(std::slice::from_ref(&mine), true).0[0]
            .meta
            .rating,
        0,
        "a newer mtime is not a newer opinion"
    );

    // With `--no-sidecars` neither file is read at all.
    assert_eq!(
        load_sidecars(&[theirs, mine], false).0[0].meta,
        Meta::default()
    );
    crate::testing::remove_dir_retry(&dir);
}

/// A file's modification time, set, so the newer-wins rule can
/// be tested without sleeping through a filesystem's resolution.
fn set_modified(path: &Path, secs: u64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("the file opens")
        .set_modified(t)
        .expect("the time is set");
}

#[test]
fn the_badges_read_the_meta() {
    use meta::{Flag, Label};
    let meta = Meta {
        rating: 200,
        flag: Flag::Reject,
        label: Label::Purple,
        ..Meta::default()
    };
    // A rating past the range does not draw two hundred stars.
    let thumb = thumb_for(Path::new("/tmp/IMG_0001.CR3"), &meta, (true, false));
    assert_eq!(thumb.name, "IMG_0001.CR3");
    assert_eq!(thumb.rating, 5);
    assert_eq!(thumb.flag, 2);
    assert_eq!(thumb.label, 5);
    assert_eq!((thumb.edited, thumb.exported), (true, false));
    let bare = thumb_for(Path::new("/tmp/a.CR3"), &Meta::default(), (false, false));
    assert_eq!((bare.rating, bare.flag, bare.label), (0, 0, 0));
    assert_eq!((bare.edited, bare.exported), (false, false));
    assert_eq!(Flag::Pick.code(), 1);
    assert_eq!(Label::Red.code(), 1);
    assert_eq!(Label::Blue.code(), 4);
}

/// A two-pixel picture for file `index`, as the pool delivers one.
/// A picture for frame `index`, square, so a row of them is laid
/// out as the square cells were and the scroll arithmetic below
/// reads as it did.
fn picture_for(files: &[PathBuf], index: usize) -> crate::worker::Outcome {
    picture_shaped(files, index, 2, 2)
}

/// A portrait picture, 2:3, for frame `index`.
fn portrait_for(files: &[PathBuf], index: usize) -> crate::worker::Outcome {
    picture_shaped(files, index, 2, 3)
}

/// A picture `width` by `height` for frame `index`.
fn picture_shaped(
    files: &[PathBuf],
    index: usize,
    width: u32,
    height: u32,
) -> crate::worker::Outcome {
    crate::worker::Outcome::Thumbnail {
        index,
        frame: crate::versions::Key::file(files[index].clone()),
        size: 176,
        width,
        height,
        rgb: vec![0; (width * height * 3) as usize],
        cached: false,
        seconds: 0.0,
    }
}

/// A file no picture can be made of marks its cell, on the grid
/// and after the rows are made again, and a snapshot of the grid
/// counts it as filled; a picture that does come clears it.
#[test]
fn a_file_with_no_picture_marks_its_cell_and_fills_the_grid() {
    use crate::panel::deliver::deliver;
    let app = window(3);
    let files = crate::testing::folder(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    app.set_grid_open(true);
    app.invoke_grid_range(0.0, 773.0, 8, 0.0);
    assert_eq!(state.borrow().views.grid_shown, Some((0, 2)));
    let failed = |row: usize| app.get_thumbs().row_data(row).unwrap().failed;
    deliver(&app, picture_for(&files, 0));
    deliver(&app, picture_for(&files, 2));
    assert!(!grid_filled(&state.borrow(), &app));
    deliver(
        &app,
        crate::worker::Outcome::NoThumbnail {
            index: 1,
            frame: crate::versions::Key::file(files[1].clone()),
        },
    );
    assert!(failed(1));
    assert!(!failed(0) && !failed(2));
    assert!(grid_filled(&state.borrow(), &app));
    rebuild_browser(&mut state.borrow_mut(), &app);
    assert!(failed(1), "kept when the rows are made again");
    // Another folder's failure, by the path it carries, is not
    // this one's.
    deliver(
        &app,
        crate::worker::Outcome::NoThumbnail {
            index: 0,
            frame: crate::versions::Key::file("/elsewhere/IMG_0000.CR3"),
        },
    );
    assert!(!failed(0));
    deliver(&app, picture_for(&files, 1));
    assert!(!failed(1));
    assert!(!state.borrow().views.thumb_failed[1]);
    // A larger picture that fails leaves the one there, unmarked,
    // and the cell filled.
    state.borrow_mut().views.thumb_want = 360;
    assert!(!grid_filled(&state.borrow(), &app));
    deliver(
        &app,
        crate::worker::Outcome::NoThumbnail {
            index: 0,
            frame: crate::versions::Key::file(files[0].clone()),
        },
    );
    assert!(!failed(0));
    assert!(state.borrow().views.thumb_base[0].is_some());
    state.borrow_mut().views.thumb_made[1] = 360;
    state.borrow_mut().views.thumb_made[2] = 360;
    assert!(grid_filled(&state.borrow(), &app));
}

/// A row carries its picture while the grid or the strip has a cell
/// for it, and only then: one delivered for a row without a cell is
/// kept in hand, goes on when a view comes to the row, and comes
/// off when the views have left it. A batch from the pool is taken
/// in one call.
#[test]
fn a_row_carries_its_picture_while_a_view_has_a_cell_for_it() {
    use crate::panel::deliver::deliver;
    use crate::worker::Outcome;
    let app = window(1000);
    let files = crate::testing::folder(1000);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let pictured = |row: usize| app.get_thumbs().row_data(row).unwrap().image.size().width;
    app.set_grid_open(true);
    // Eight columns of frames with no picture yet, laid out as
    // 3:2, 146 apart: five rows and a part on screen, cells for rows
    // 0 to 8, frames 0 to 71.
    app.invoke_grid_range(0.0, 773.0, 8, 0.0);
    assert_eq!(cells::window(&app, cells::View::Grid), (0, 72));
    deliver(
        &app,
        Outcome::Thumbnails(crate::thumbpool::Batch::of(
            [5, 300, 900].map(|i| picture_for(&files, i)).into(),
        )),
    );
    {
        let st = state.borrow();
        assert!(
            [5, 300, 900]
                .iter()
                .all(|&i| st.views.thumb_base[i].is_some())
        );
    }
    assert_eq!(pictured(5), 2, "a cell: on its row");
    assert_eq!(pictured(300), 0, "no cell: kept for later");
    assert_eq!(pictured(900), 0);
    // The grid scrolled to 900's screenful: on, and 5 off.
    app.invoke_grid_range(row_top(&app, 8, 888), 773.0, 8, 0.0);
    assert_eq!(pictured(900), 2);
    assert_eq!(pictured(5), 0);
    assert!(state.borrow().views.thumb_shown[5].is_none());
    assert!(
        state.borrow().views.thumb_base[5].is_some(),
        "still in hand"
    );
    assert_eq!(pictured(300), 0);
    // The strip, through its own report.
    app.invoke_strip_range(295, 305);
    assert_eq!(pictured(300), 2);
    // Back to the top: 5 on again, 900 off, 300 kept by the strip.
    app.invoke_grid_range(0.0, 773.0, 8, 0.0);
    assert_eq!((pictured(5), pictured(900), pictured(300)), (2, 0, 2));
    // A larger picture for a row without a cell waits in hand, and
    // is the one put on when the grid comes back to it.
    deliver(
        &app,
        Outcome::Thumbnail {
            index: 900,
            frame: crate::versions::Key::file(files[900].clone()),
            size: 360,
            width: 4,
            height: 2,
            rgb: vec![0; 24],
            cached: true,
            seconds: 0.0,
        },
    );
    assert_eq!(pictured(900), 0);
    app.invoke_grid_range(row_top(&app, 8, 888), 773.0, 8, 0.0);
    assert_eq!(pictured(900), 4);
}

/// A picture that came while the filter hid its frame goes on its
/// row when the filter is cleared only if a view has a cell for the
/// row: a cleared filter over thousands would otherwise put them
/// all on in one call.
#[test]
fn pictures_that_came_while_hidden_wait_for_a_cell_when_the_filter_clears() {
    use crate::panel::deliver::deliver;
    let app = window(0);
    let files = crate::testing::folder(1000);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        for s in &mut st.sidecars[500..1000] {
            s.meta.flag = meta::Flag::Reject;
        }
        st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
        rebuild_browser(&mut st, &app);
        assert_eq!(st.views.shown.len(), 500);
    }
    app.set_grid_open(true);
    app.invoke_grid_range(0.0, 773.0, 8, 0.0);
    deliver(&app, picture_for(&files, 5));
    deliver(&app, picture_for(&files, 900));
    let pictured = |row: usize| app.get_thumbs().row_data(row).unwrap().image.size().width;
    {
        let mut st = state.borrow_mut();
        assert!(st.views.thumb_base[900].is_some() && st.views.thumb_shown[900].is_none());
        st.views.filter = filter::Filter::from_name("All").expect("it parses");
        rebuild_browser(&mut st, &app);
    }
    assert_eq!(pictured(5), 2, "a cell: made again on its row");
    assert_eq!(pictured(900), 0, "no cell: kept for later");
    app.invoke_grid_range(row_top(&app, 8, 888), 773.0, 8, 0.0);
    assert_eq!(pictured(900), 2);
}

/// A batch larger than a turn takes: the first turn takes one slice
/// and leaves the rest to a timer, whose turn takes them.
#[test]
fn a_batch_left_over_a_turn_is_taken_on_the_next() {
    use crate::panel::deliver::take_thumbnails;
    let app = window(100);
    let files = crate::testing::folder(100);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    // A sheet tall enough for a cell for every frame.
    app.invoke_grid_range(0.0, 3000.0, 8, 0.0);
    let batch = crate::thumbpool::Batch::of((0..100).map(|i| picture_for(&files, i)).collect());
    take_thumbnails(&app, &state, batch, std::time::Duration::ZERO);
    let taken = |st: &State| st.views.thumb_base.iter().filter(|b| b.is_some()).count();
    assert_eq!(taken(&state.borrow()), 32, "one slice in a turn of nothing");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    slint::platform::update_timers_and_animations();
    assert_eq!(taken(&state.borrow()), 100, "the rest on the timer's turn");
    assert!((0..100).all(|r| app.get_thumbs().row_data(r).unwrap().image.size().width == 2));
}

/// How many cells of the strip, or of the grid, are on screen: the
/// testing backend finds only elements its clip leaves showing.
/// Every cell a view's model holds is made, a `for` outside a
/// `ListView` making one for each row, so the model's count is the
/// count made.
/// The grid of the tests' window, six 176 cells across, over
/// `count` frames with no picture yet, each laid out as 3:2.
fn shaped(count: i32) -> grid::Layout {
    grid::Layout::new(176.0, 6, count, &[], |_| 0.0)
}

/// The top of the grid row frame `frame` is on, from the sheet's
/// top, as the window lays the rows out now in `cols` columns.
fn row_top(app: &App, cols: i32, frame: i32) -> f32 {
    let count = app.get_thumbs().row_count() as i32;
    let layout = cells::layout(app, app.get_tile_cell(), cols, count, &[], grid_shape(app));
    grid::PAD + layout.place(frame).expect("a frame").y
}

fn cells_on_screen(app: &App, id: &str) -> usize {
    // A cell a model change has just brought in is built on the
    // next event: a pointer move at the corner, where nothing is.
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, id).count()
}

/// The rows a view's cells show, sorted.
fn cell_rows(model: slint::ModelRc<crate::ThumbCell>) -> Vec<i32> {
    let mut rows: Vec<i32> = model.iter().map(|c| c.row).collect();
    rows.sort();
    rows
}

/// A browser of five thousand frames in a 1500 by 950 window,
/// opened on the first, with pictures in hand for every frame.
fn five_thousand() -> (App, Rc<Shared>, Vec<PathBuf>) {
    use crate::panel::deliver::deliver;
    let app = window(5000);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let files = crate::testing::folder(5000);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    state.borrow_mut().current = Some(0);
    app.set_selected(0);
    press(&app, Key::Shift);
    deliver(
        &app,
        crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(
            (0..5000).map(|i| picture_for(&files, i)).collect(),
        )),
    );
    // The whole batch, over as many turns as it takes.
    for _ in 0..500 {
        if state.borrow().views.thumb_base.iter().all(Option::is_some) {
            break;
        }
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
    }
    assert!(state.borrow().views.thumb_base.iter().all(Option::is_some));
    // The rows laid out by the pictures' shapes, a turn after the
    // last came in.
    slint::platform::update_timers_and_animations();
    (app, state, files)
}

/// The pictures' shapes lay the grid's rows out: landscapes close
/// the rows up by the plate a square cell had, a portrait raises its
/// row alone, its neighbors at the row's top, and a shape that comes
/// in above the screen moves the scroll with it, so what is on
/// screen stays where it was.
#[test]
fn the_rows_take_the_pictures_shapes_and_the_screen_stays_put() {
    use crate::panel::deliver::deliver;
    let app = window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let files = crate::testing::folder(120);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    state.borrow_mut().current = Some(0);
    app.set_selected(0);
    press(&app, "g");
    assert_eq!(grid::columns(app.get_grid_w(), 176.0), 6);
    deliver(
        &app,
        crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(
            (0..120)
                .map(|i| match i {
                    8 => portrait_for(&files, i),
                    _ => picture_shaped(&files, i, 3, 2),
                })
                .collect(),
        )),
    );
    for _ in 0..10 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
    }
    let place =
        |i: i32| app.invoke_grid_place(i, 176.0, 6, ModelRc::default(), app.get_grid_shape());
    // A row of landscapes: 120 of picture where the square was 176.
    assert_eq!((place(0).y, place(0).pic), (0.0, 120.0));
    // The portrait's row: its picture the square, its neighbor's
    // the landscape's, both at the row's top, 146 down.
    assert_eq!((place(8).y, place(8).pic), (146.0, 176.0));
    assert_eq!((place(7).y, place(7).pic), (146.0, 120.0));
    // The row under it, a square row's pitch further.
    assert_eq!(place(12).y, 146.0 + 202.0);
    // The last row: eighteen rows of 146 and one of 202 above it.
    assert_eq!(row_top(&app, 6, 114), grid::PAD + 18.0 * 146.0 + 202.0);
    // Scrolled to the eleventh row, a portrait comes for the first
    // frame: the first row grows by the plate, and the scroll with
    // it.
    let to = row_top(&app, 6, 60);
    app.invoke_scroll_by(to - app.get_grid_scroll());
    let was = app.get_grid_scroll();
    assert_eq!(state.borrow().views.grid_shown.map(|s| s.0), Some(60));
    deliver(&app, portrait_for(&files, 0));
    slint::platform::update_timers_and_animations();
    assert_eq!(place(0).pic, 176.0);
    assert_eq!(app.get_grid_scroll(), was + 56.0);
    assert_eq!(state.borrow().views.grid_shown.map(|s| s.0), Some(60));
}

/// Rows that grow under a selected frame on screen keep it on
/// screen, whole; and a sheet at its end stays at its end.
#[test]
fn shapes_that_come_later_keep_the_selection_and_the_end_in_view() {
    use crate::panel::deliver::deliver;
    let app = window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let files = crate::testing::folder(120);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    state.borrow_mut().current = Some(0);
    app.set_selected(0);
    press(&app, "g");
    let settle = || {
        for _ in 0..10 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        }
    };
    let batch = |which: &dyn Fn(usize) -> bool, portrait: bool| {
        let pictures = (0..120)
            .filter(|&i| which(i))
            .map(|i| match portrait {
                true => portrait_for(&files, i),
                false => picture_shaped(&files, i, 3, 2),
            })
            .collect();
        deliver(
            &app,
            crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(pictures)),
        );
        settle();
    };
    // Whole on screen: its row's top and its name's foot inside the
    // sheet.
    let whole = |i: i32| {
        let p = app.invoke_grid_place(i, 176.0, 6, ModelRc::default(), app.get_grid_shape());
        let (y, h, s) = (grid::PAD + p.y, app.get_grid_h(), app.get_grid_scroll());
        assert!(
            s <= y && y + p.pic + grid::LABEL <= s + h,
            "frame {i} at {y} to {}, the sheet {s} to {}",
            y + p.pic + grid::LABEL,
            s + h
        );
    };
    let max = || {
        let count = app.get_thumbs().row_count() as i32;
        cells::layout(&app, 176.0, 6, count, &[], grid_shape(&app))
            .max_scroll(app.get_grid_h(), 0.0)
    };
    batch(&|_| true, false);
    // Frame 60 chosen and revealed among landscapes, then every
    // third frame turns out a portrait: rows above and its own grow.
    app.set_selected(60);
    press(&app, Key::Shift);
    whole(60);
    batch(&|i| i % 3 == 0, true);
    whole(60);
    // End, then more portraits: still at the end, the last frame
    // whole.
    press(&app, Key::End);
    assert_eq!(app.get_selected(), 119);
    assert_eq!(app.get_grid_scroll(), max());
    batch(&|i| i % 3 == 1, true);
    assert_eq!(app.get_grid_scroll(), max());
    whole(119);
    // The last four rows landscapes again, End, and up three rows
    // to the first of them, the sheet still at its end; then those
    // rows turn portrait, its own and the three under it growing.
    // The end moves down, and the frame stays whole on screen.
    batch(&|i| i >= 96, false);
    press(&app, Key::End);
    for _ in 0..3 {
        press(&app, Key::UpArrow);
    }
    assert_eq!(app.get_selected(), 101);
    assert_eq!(app.get_grid_scroll(), max());
    whole(101);
    batch(&|i| i >= 96 && i % 3 == 0, true);
    whole(101);
}

/// A notch of the wheel that cuts the selected row at the top is
/// the user's own: shapes that come in after it hold the sheet
/// where the wheel put it, rather than bring the row back whole.
#[test]
fn shapes_that_come_after_a_scroll_leave_the_sheet_where_it_was() {
    use crate::panel::deliver::deliver;
    let app = window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let files = crate::testing::folder(120);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    state.borrow_mut().current = Some(0);
    app.set_selected(0);
    press(&app, "g");
    let settle = || {
        for _ in 0..10 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        }
    };
    deliver(
        &app,
        crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(
            (0..60).map(|i| picture_shaped(&files, i, 3, 2)).collect(),
        )),
    );
    settle();
    assert_eq!(app.get_grid_scroll(), 0.0);
    // A third of a row down: the selected frame's row cut at the
    // top, and still on screen.
    app.invoke_scroll_by(48.0);
    assert_eq!(app.get_grid_scroll(), 48.0);
    assert_eq!(state.borrow().views.grid_shown.map(|s| s.0), Some(0));
    // Shapes for rows below, one of them a portrait: the sheet
    // stays where the wheel put it.
    deliver(
        &app,
        crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(
            (60..120)
                .map(|i| match i {
                    66 => portrait_for(&files, i),
                    _ => picture_shaped(&files, i, 3, 2),
                })
                .collect(),
        )),
    );
    settle();
    assert_eq!(app.get_grid_scroll(), 48.0, "the shapes undid the scroll");
    // Another notch, and the selected row's own shape changes: its
    // row grows under the cut, and the sheet still holds.
    app.invoke_scroll_by(48.0);
    deliver(&app, portrait_for(&files, 3));
    settle();
    assert_eq!(
        app.get_grid_scroll(),
        96.0,
        "the row's growth undid the scroll"
    );
}

/// The strip's cells take their pictures' shapes, a portrait narrow
/// and a landscape wide, and a shape that comes in for a frame left
/// of the view moves nothing on screen: a click where a frame was
/// is still that frame. A frame chosen past the right edge comes in
/// with the pad beside it.
#[test]
fn the_strips_cells_take_their_shapes_and_a_late_one_moves_nothing_in_view() {
    use crate::panel::deliver::deliver;
    use i_slint_backend_testing::ElementHandle;
    let app = window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let files = crate::testing::folder(120);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    state.borrow_mut().current = Some(0);
    app.set_selected(0);
    press(&app, Key::Shift);
    let settle = || {
        for _ in 0..10 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        }
    };
    let clicks = Rc::new(RefCell::new(Vec::new()));
    let c = clicks.clone();
    app.on_frame_clicked(move |row, _, _| c.borrow_mut().push(row));
    // Before any shape: every cell the 3:2 landscape's.
    let (_, was_last) = state.borrow().views.strip_shown.unwrap();
    assert_eq!(
        Some((0, was_last)),
        cells::strip_layout(&app).shown(0.0, app.get_strip_view())
    );
    // The first twenty, alternately portrait and landscape.
    deliver(
        &app,
        crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(
            (0..20)
                .map(|i| match i % 2 {
                    0 => portrait_for(&files, i),
                    _ => picture_shaped(&files, i, 3, 2),
                })
                .collect(),
        )),
    );
    settle();
    assert_eq!(app.get_strip_scroll(), 0.0);
    // The portraits narrowed the row at the same scroll: the rows
    // that came on screen at the right are said, so they have cells
    // and their pictures are asked for (the worker is handed the
    // range and the rows near it as the strip says it).
    let (first, last) = state.borrow().views.strip_shown.unwrap();
    assert_eq!(
        Some((first, last)),
        cells::strip_layout(&app).shown(0.0, app.get_strip_view())
    );
    assert!(last > was_last, "{last} after {was_last}");
    assert_eq!(
        state.borrow().views.near_rows,
        Some((first as usize, last as usize))
    );
    for row in was_last + 1..=last {
        assert!(cells::has_cell(&app, row as usize), "row {row} has no cell");
    }
    // The cells drawn are the layout's: a portrait's the floor's 96
    // wide around its picture's 61, a landscape's its picture's 137
    // in its ring.
    let drawn: Vec<(f32, f32)> = ElementHandle::find_by_element_id(&app, "Filmstrip::cell")
        .map(|e| (e.absolute_position().x, e.size().width))
        .collect();
    for row in 0..4 {
        let (left, width) = cells::strip_layout(&app).place(row).unwrap();
        assert!(drawn.contains(&(left, width)), "row {row}: {drawn:?}");
    }
    assert_eq!(
        cells::strip_layout(&app).place(0).unwrap().1,
        strip::cell_width(1.5, strip::PHOTO)
    );
    assert_eq!(
        cells::strip_layout(&app).place(1).unwrap().1,
        strip::cell_width(2.0 / 3.0, strip::PHOTO)
    );
    // Along the strip, past frames whose pictures have not come.
    assert!(app.invoke_scroll_by(5000.0));
    settle();
    let scroll = app.get_strip_scroll();
    assert_eq!(scroll, 5000.0);
    let (first, last) = state.borrow().views.strip_shown.unwrap();
    assert!(first > 30, "{first}");
    let on_screen = |row: i32| {
        let (left, width) = cells::strip_layout(&app).place(row).unwrap();
        (left - app.get_strip_scroll(), width)
    };
    let was: Vec<_> = (first..=last).map(on_screen).collect();
    let (x, y) = crate::testing::strip_cell(&app, first as usize + 2);
    // Frames 21 to 29, left of the view, come in as portraits: the
    // row is shorter before the view, and the view moves with it.
    deliver(
        &app,
        crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(
            (21..30).map(|i| portrait_for(&files, i)).collect(),
        )),
    );
    settle();
    assert!(app.get_strip_scroll() < scroll, "the row before shrank");
    let now: Vec<_> = (first..=last).map(on_screen).collect();
    assert_eq!(now, was, "the frames on screen moved");
    crate::testing::click(&app, x, y);
    assert_eq!(*clicks.borrow(), vec![first + 2]);
    // The first frame past the right edge, chosen: in view with the
    // pad after it, at the strip's right.
    let past = last + 1;
    app.set_selected(past);
    press(&app, Key::Shift);
    let (left, width) = cells::strip_layout(&app).place(past).unwrap();
    assert_eq!(
        left + width + strip::PAD,
        app.get_strip_scroll() + app.get_strip_view()
    );
}

/// A portrait's cell is the floor's width, wide enough for its
/// twelve-character name, with the picture and its ring centered in
/// it; a landscape's cell is its picture's, the ring at its edges.
/// Either way the name sits under the ring, outside it.
#[test]
fn a_portraits_cell_shows_its_name_whole_and_a_landscapes_is_its_picture() {
    use crate::panel::deliver::deliver;
    use i_slint_backend_testing::ElementHandle;
    let app = window(10);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    // Two cameras' names, the twelve characters the floor is for.
    let names = ["4Z4A2978.CR3", "DSCF0438.RAF"];
    cells::set_rows(
        &app,
        (0..10)
            .map(|i| Thumb {
                name: names.get(i).map_or("IMG_0000.CR3", |n| n).into(),
                image: slint::Image::default(),
                ..Default::default()
            })
            .collect(),
    );
    let files = crate::testing::folder(10);
    let (_state, _worker) = crate::testing::state_for(&app, files.clone());
    press(&app, Key::Shift);
    deliver(
        &app,
        crate::worker::Outcome::Thumbnails(crate::thumbpool::Batch::of(vec![
            portrait_for(&files, 0),
            picture_shaped(&files, 1, 3, 2),
        ])),
    );
    for _ in 0..10 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
    }
    press(&app, Key::Shift);
    assert_eq!(app.get_strip_scroll(), 0.0);
    let layout = cells::strip_layout(&app);
    let (p_left, p_width) = layout.place(0).unwrap();
    let (l_left, l_width) = layout.place(1).unwrap();
    assert_eq!(p_width, strip::FLOOR);
    assert_eq!(
        l_width,
        137.0 + 2.0 * strip::RING,
        "the landscape as before"
    );
    // The ring around each picture: the portrait's in the middle of
    // its cell, the landscape's the cell's own.
    let (p_inset, p_photo) = layout.photo(0).unwrap();
    assert!(p_inset > 0.0);
    let frames: Vec<_> = ElementHandle::find_by_element_id(&app, "Filmstrip::frame")
        .map(|e| (e.absolute_position().x, e.size().width, e))
        .collect();
    let frame = |x: f32, w: f32| {
        frames
            .iter()
            .find(|f| (f.0, f.1) == (x, w))
            .map(|f| f.2.clone())
            .unwrap_or_else(|| panic!("no ring at {x} {w} wide"))
    };
    let rings = [
        frame(p_left + p_inset, p_photo + 2.0 * strip::RING),
        frame(l_left, l_width),
    ];
    // Each ring holds the picture and the band's room under it,
    // the theme's `space-1`, and ends above the name.
    let band = app.global::<crate::Theme>().get_space_1();
    for ring in &rings {
        assert_eq!(ring.size().height, strip::PHOTO + band + 2.0 * strip::RING);
    }
    let ring_foot = rings[0].absolute_position().y + rings[0].size().height;
    // Each name's box is its cell's width, a portrait's the floor,
    // which these names fit in the theme's face (see `strip::FLOOR`):
    // no longer cut to the picture's width, and under the ring.
    for (name, left, width) in [(names[0], p_left, p_width), (names[1], l_left, l_width)] {
        let text = crate::testing::cell_names(&app, name)
            .into_iter()
            .find(|e| e.absolute_position().x == left)
            .unwrap_or_else(|| panic!("{name} under its cell"));
        assert_eq!(text.size().width, width);
        assert!(
            text.absolute_position().y >= ring_foot,
            "{name} in the ring"
        );
    }
}

/// The strip and the grid make cells for the rows on screen and a
/// margin, however long the list: five thousand frames are a strip
/// of a screen's width and a grid of seven rows, and no more rows
/// than those carry a picture.
#[test]
fn five_thousand_frames_make_cells_for_the_screen_alone() {
    let (app, state, _files) = five_thousand();
    // The strip, 1500 wide over square pictures, the floor's 96 a
    // cell and 8 between: fourteen and a part on screen, and cells for a part
    // more and the margin either side.
    assert_eq!(cells_on_screen(&app, "Filmstrip::cell"), 15);
    let strip = app.get_strip_cells().row_count();
    assert!(strip <= 17 + 2 * cells::STRIP_MARGIN, "{strip}");
    assert_eq!(app.get_cells_made() as usize, strip, "the grid is closed");
    assert_eq!(cells_on_screen(&app, "GridSheet::cell"), 0);
    press(&app, "g");
    // Six columns beside the left pane; three rows on screen at
    // the top and most of a fourth, as the layout test has it, a
    // fifth's part as it scrolls; and cells for a row either side.
    assert_eq!(cells_on_screen(&app, "GridSheet::cell"), 4 * 6);
    let grid = app.get_grid_cells().row_count();
    assert_eq!(grid, 7 * 6);
    assert_eq!(app.get_cells_made() as usize, strip + grid);
    assert_eq!(cell_rows(app.get_grid_cells()), (0..42).collect::<Vec<_>>());
    // What carries a picture is what has a cell.
    let rows = app.get_thumbs();
    let pictured = (0..5000)
        .filter(|&r| rows.row_data(r).unwrap().image.size().width > 0)
        .count();
    assert_eq!(pictured, 42, "the grid's cells take in the strip's");
    let st = state.borrow();
    assert_eq!(
        st.views.thumb_shown.iter().filter(|s| s.is_some()).count(),
        42
    );
}

/// Scrolled to the far end and back, the grid and the strip still
/// have the current frame chosen, ringed and pictured, and a
/// picture that came for it while it was off screen is the one it
/// shows.
#[test]
fn scrolling_to_the_end_and_back_keeps_the_current_frame_and_its_picture() {
    use crate::panel::deliver::deliver;
    use crate::worker::Outcome;
    let (app, state, files) = five_thousand();
    let cell_of =
        |model: slint::ModelRc<crate::ThumbCell>, row: i32| model.iter().find(|c| c.row == row);
    for grid in [true, false] {
        if grid {
            press(&app, "g");
        }
        let cells = || {
            if grid {
                app.get_grid_cells()
            } else {
                app.get_strip_cells()
            }
        };
        let made = app.get_cells_made();
        assert!(cell_of(cells(), 0).is_some());
        assert!(app.invoke_scroll_by(1e9));
        press(&app, Key::Shift);
        assert!(
            cell_of(cells(), 4999).is_some(),
            "the last frame has a cell"
        );
        assert!(cell_of(cells(), 0).is_none());
        assert_eq!(app.get_cells_made(), made, "as many cells at the end");
        assert_eq!(app.get_selected(), 0);
        // Its picture is on its row while the other view has a cell
        // for it, and off when neither has.
        assert_eq!(
            state.borrow().views.thumb_shown[0].is_some(),
            cells::has_cell(&app, 0)
        );
        // A new picture for it while it is away.
        let (w, h) = if grid { (6, 3) } else { (8, 4) };
        deliver(
            &app,
            Outcome::Thumbnail {
                index: 0,
                frame: crate::versions::Key::file(files[0].clone()),
                size: 360,
                width: w,
                height: h,
                rgb: vec![0; (w * h * 3) as usize],
                cached: true,
                seconds: 0.0,
            },
        );
        assert!(app.invoke_scroll_by(-1e9));
        press(&app, Key::Shift);
        let back = cell_of(cells(), 0).expect("the first frame's cell again");
        assert_eq!(back.thumb.image.size().width, w, "the newer picture");
        assert_eq!(app.get_selected(), 0, "still the current frame, ringed");
        assert_eq!(state.borrow().current, Some(0));
        if grid {
            press(&app, "g");
        }
    }
}

/// A frame far down the folder, made current, is scrolled into
/// view in the grid and in the strip, and has a cell on screen in
/// each.
#[test]
fn a_far_frame_made_current_is_revealed_with_a_cell() {
    let (app, _state, _files) = five_thousand();
    press(&app, "g");
    app.set_selected(4321);
    press(&app, Key::Shift);
    let rows = cell_rows(app.get_grid_cells());
    assert!(rows.contains(&4321), "{rows:?}");
    let (first, count) = cells::window(&app, cells::View::Grid);
    assert!(first <= 4321 && 4321 < first + count);
    assert_eq!(
        app.get_thumbs().row_data(4321).unwrap().image.size().width,
        2
    );
    assert_eq!(cells_on_screen(&app, "GridSheet::cell"), 5 * 6);
    // The strip, out of the grid.
    press(&app, "g");
    app.set_selected(3210);
    press(&app, Key::Shift);
    let rows = cell_rows(app.get_strip_cells());
    assert!(rows.contains(&3210), "{rows:?}");
    assert_eq!(
        app.get_thumbs().row_data(3210).unwrap().image.size().width,
        2
    );
    assert!(cells_on_screen(&app, "Filmstrip::cell") >= 8);
}

/// The window's share of a turn: as long as it spent drawing and
/// listening since the last, within its bounds.
#[test]
fn a_turn_takes_pictures_in_for_as_long_as_the_frame_took() {
    use crate::panel::deliver::{THUMB_TURN, THUMB_TURN_MOST, thumb_turn};
    use std::time::Duration;
    assert_eq!(thumb_turn(None), THUMB_TURN);
    assert_eq!(thumb_turn(Some(Duration::from_millis(1))), THUMB_TURN);
    assert_eq!(
        thumb_turn(Some(Duration::from_millis(60))),
        Duration::from_millis(60)
    );
    assert_eq!(thumb_turn(Some(Duration::from_secs(2))), THUMB_TURN_MOST);
}

/// A snapshot of a grid whose pictures have not all come stops
/// waiting at its limit and takes the grid as it stands; without
/// a snapshot, or with the grid filled, there is nothing to give
/// up.
#[test]
fn a_snapshot_of_the_grid_stops_waiting_at_its_limit() {
    use crate::panel::deliver::deliver;
    let app = window(3);
    let files = crate::testing::folder(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    app.set_grid_open(true);
    state.borrow_mut().views.grid_shown = Some((0, 2));
    deliver(&app, picture_for(&files, 0));
    assert!(!give_up_on_grid(&mut state.borrow_mut(), &app));
    assert!(!grid_filled(&state.borrow(), &app));
    state.borrow_mut().launch.snapshot = Some(PathBuf::from("grid.png"));
    assert!(give_up_on_grid(&mut state.borrow_mut(), &app));
    assert!(grid_filled(&state.borrow(), &app));
    // Every picture in, but one too small for the cells, its
    // larger one not come: still waited for, and given up on.
    let app = window(2);
    let files = crate::testing::folder(2);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    app.set_grid_open(true);
    state.borrow_mut().views.grid_shown = Some((0, 1));
    state.borrow_mut().launch.snapshot = Some(PathBuf::from("grid.png"));
    deliver(&app, picture_for(&files, 0));
    deliver(&app, picture_for(&files, 1));
    state.borrow_mut().views.thumb_want = 360;
    assert!(give_up_on_grid(&mut state.borrow_mut(), &app));
    // A grid filled in time is not given up on.
    let app = window(2);
    let files = crate::testing::folder(2);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    app.set_grid_open(true);
    state.borrow_mut().views.grid_shown = Some((0, 1));
    state.borrow_mut().launch.snapshot = Some(PathBuf::from("grid.png"));
    deliver(&app, picture_for(&files, 0));
    deliver(&app, picture_for(&files, 1));
    assert!(!give_up_on_grid(&mut state.borrow_mut(), &app));
}

/// The filmstrip's picture, the grid's and the culling loupe's are
/// the camera's own, so a frame's turn rides in the same quarter
/// turns the edit's do: what comes out is the picture the develop
/// would make of the turned frame.
#[test]
fn a_thumbnail_follows_the_frames_own_turn() {
    use greycard_core::WorkingImage;
    use greycard_core::develop::orient;
    use greycard_core::raw::Orientation;
    let rgb: Vec<u8> = (0..6).flat_map(|i| [i, i, i]).collect();
    let plane = |w: u32, h: u32, rgb: &[u8]| {
        WorkingImage::from_data(
            w as usize,
            h as usize,
            rgb.iter().map(|&b| b as f32).collect(),
        )
        .unwrap()
    };
    for geometry in [
        Geometry::default(),
        Geometry {
            turns: 1,
            ..Geometry::default()
        },
        Geometry {
            flip: true,
            ..Geometry::default()
        },
        Geometry {
            turns: 3,
            flip: true,
            ..Geometry::default()
        },
    ] {
        for turn in 0..4u8 {
            // The develop: the source turned by the frame's own
            // quarter turns, then the edit's geometry on top.
            let turned = orient(
                plane(3, 2, &rgb),
                Orientation::from_exif(match turn {
                    1 => 6,
                    2 => 3,
                    3 => 8,
                    _ => 1,
                })
                .unwrap(),
            );
            let bytes: Vec<u8> = turned.data.iter().map(|&v| v as u8).collect();
            let under = geometry.under_turned_source(i32::from(turn));
            let want = greycard_edit::geometry::turn_pixels(
                turned.width as u32,
                turned.height as u32,
                &bytes,
                under.turns,
                under.flip,
            );
            // The strip: the camera's picture, with the turn
            // folded into the quarter turns.
            let (turns, flip) = under.shown_turns(turn);
            let got = greycard_edit::geometry::turn_pixels(3, 2, &rgb, turns, flip);
            assert_eq!(got, want, "{geometry:?} + {turn}");
        }
    }
}

#[test]
fn g_opens_the_grid_and_three_keys_leave_it() {
    let app = window(11);
    assert!(!app.get_grid_open());
    press(&app, "g");
    assert!(app.get_grid_open());
    press(&app, Key::Return);
    assert!(!app.get_grid_open());
    press(&app, "G");
    assert!(app.get_grid_open());
    press(&app, Key::Escape);
    assert!(!app.get_grid_open());
    press(&app, "g");
    press(&app, "g");
    assert!(!app.get_grid_open());
}

#[test]
fn the_arrows_step_the_strip_in_one_dimension_and_the_grid_in_two() {
    let app = window(11);
    let steps = Rc::new(RefCell::new(Vec::new()));
    let flat = steps.clone();
    app.on_step(move |by, _| flat.borrow_mut().push((by, 0)));
    let two = steps.clone();
    app.on_grid_step(move |dx, dy, cols, _| {
        assert!(cols >= 1, "the grid always has a column");
        two.borrow_mut().push((dx, dy));
    });
    // In the loupe the arrows are the strip's, along the folder.
    press(&app, Key::LeftArrow);
    press(&app, Key::RightArrow);
    press(&app, Key::UpArrow);
    assert_eq!(*steps.borrow(), vec![(-1, 0), (1, 0)]);
    steps.borrow_mut().clear();
    // In the grid they are the grid's, and the vertical pair
    // reaches it too.
    press(&app, "g");
    press(&app, Key::LeftArrow);
    press(&app, Key::RightArrow);
    press(&app, Key::UpArrow);
    press(&app, Key::DownArrow);
    assert_eq!(*steps.borrow(), vec![(-1, 0), (1, 0), (0, -1), (0, 1)]);
}

/// A mouse's back and forward buttons over the grid ask for the
/// folder visited before and after, over a cell and over the sheet
/// between the cells, and step no frame; the left button there
/// asks for nothing.
#[test]
fn the_mouse_side_buttons_go_back_and_forward_over_the_grid() {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{PointerEventButton, WindowEvent};
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let files = crate::testing::folder(3);
    let (_state, _worker) = crate::testing::state_for(&app, files);
    let steps = Rc::new(RefCell::new(Vec::new()));
    let seen = steps.clone();
    app.on_grid_step(move |dx, dy, _, extend| seen.borrow_mut().push((dx, dy, extend)));
    let visits = Rc::new(RefCell::new(Vec::new()));
    let asked = visits.clone();
    app.on_visit_folder(move |back| asked.borrow_mut().push(back));
    app.set_grid_open(true);
    app.invoke_grid_range(0.0, 773.0, 8, 0.0);
    let press = |at: slint::LogicalPosition, button| {
        for event in [
            WindowEvent::PointerMoved { position: at },
            WindowEvent::PointerPressed {
                position: at,
                button,
            },
            WindowEvent::PointerReleased {
                position: at,
                button,
            },
        ] {
            app.window().dispatch_event(event);
        }
    };
    let cell = ElementHandle::find_by_element_id(&app, "GridSheet::ct")
        .next()
        .expect("a cell on the sheet");
    let (at, size) = (cell.absolute_position(), cell.size());
    let middle = slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
    press(middle, PointerEventButton::Forward);
    press(middle, PointerEventButton::Back);
    // Below the one row of three: the sheet, no cell.
    let below = slint::LogicalPosition::new(middle.x, 900.0);
    press(below, PointerEventButton::Forward);
    press(below, PointerEventButton::Left);
    assert_eq!(*visits.borrow(), [false, true, false]);
    assert!(steps.borrow().is_empty(), "{:?}", steps.borrow());
}

#[test]
fn g_at_a_sized_window_reads_the_sheet_it_is_born_at() {
    // Opened by G with the window already at its size, the sheet
    // is created at that size and `changed width` has nothing to
    // say; the first release laid one column down the left.
    let app = window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let reports = seen.clone();
    app.on_grid_range(move |scroll, height, cols, _top| {
        reports.borrow_mut().push((scroll, height, cols));
    });
    app.set_selected(0);
    press(&app, "g");
    assert!(
        app.get_grid_w() > 1000.0,
        "sheet width {}",
        app.get_grid_w()
    );
    assert!(
        app.get_grid_h() > 500.0,
        "sheet height {}",
        app.get_grid_h()
    );
    // Beside the left pane, 240 of the 1500.
    let cols = grid::columns(app.get_grid_w(), 176.0);
    assert_eq!(cols, 6);
    assert_eq!(seen.borrow().last().map(|r| r.2), Some(6));
}

#[test]
fn the_grid_lays_out_and_reports_what_it_shows() {
    let app = window(120);
    // The sheet a 1500 by 950 window leaves.
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let reports = seen.clone();
    app.on_grid_range(move |scroll, height, cols, _top| {
        reports.borrow_mut().push((scroll, height, cols));
    });
    let steps = Rc::new(RefCell::new(0));
    let cols = steps.clone();
    app.on_grid_step(move |_, _, c, _| *cols.borrow_mut() = c);
    app.set_selected(0);
    press(&app, "g");
    press(&app, Key::DownArrow);
    // Six 176 cells across the 1,260 the left pane leaves of
    // 1500, rows of frames with no picture yet laid out as 3:2,
    // 146 apart: under a header of the selection's line, the
    // filter's chips with its words, and its facets on two rows
    // the sheet is 805 tall and shows five rows and part of a
    // sixth, so the first thirty-six frames are what the worker is
    // told to make first.
    assert_eq!(*steps.borrow(), 6);
    assert_eq!(*seen.borrow(), vec![(0.0, 805.0, 6)]);
    assert_eq!(shaped(120).visible(0.0, 805.0, 0.0), Some((0, 35)));
    // A frame at the foot of the sheet scrolls it there, and what
    // it says it shows follows the scroll.
    seen.borrow_mut().clear();
    app.set_selected(119);
    // Slint runs the changed handlers with the next event, which
    // in a window is the next frame.
    press(&app, Key::Shift);
    let scrolled = shaped(120).reveal(0.0, 805.0, 119, 0.0);
    assert_eq!(scrolled, shaped(120).max_scroll(805.0, 0.0));
    assert_eq!(*seen.borrow(), vec![(scrolled, 805.0, 6)]);
    assert_eq!(shaped(120).visible(scrolled, 805.0, 0.0), Some((84, 119)));
    // The pane put away, by F7 as in the loupe: the sheet takes
    // the whole width and eight cells a row, as before the pane.
    seen.borrow_mut().clear();
    press(&app, Key::F7);
    assert!(app.get_left_hidden());
    press(&app, Key::Shift);
    assert_eq!(seen.borrow().last().map(|r| (r.1, r.2)), Some((805.0, 8)));
    // And back by Tab, which over the grid is the pane's alone.
    press(&app, Key::Tab);
    press(&app, Key::Shift);
    assert!(!app.get_left_hidden());
    assert!(!app.get_right_hidden() && !app.get_strip_hidden());
    assert_eq!(seen.borrow().last().map(|r| r.2), Some(6));
}

#[test]
fn a_sheet_owns_the_keys_over_the_grid_and_a_new_folder_re_flows_it() {
    let app = window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    app.set_selected(0);
    press(&app, "g");
    assert!(app.get_grid_open());
    // A sheet is drawn over the grid, so the keys are its own:
    // neither G nor Return reaches the grid under it.
    app.set_export_open(true);
    press(&app, "g");
    press(&app, Key::Return);
    assert!(app.get_grid_open());
    app.set_export_open(false);
    press(&app, "g");
    assert!(!app.get_grid_open());
    // Another folder while the grid is up: it re-flows and says
    // afresh what it shows, rather than keeping the old scroll.
    press(&app, "g");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let reports = seen.clone();
    app.on_grid_range(move |scroll, _, cols, _| reports.borrow_mut().push((scroll, cols)));
    let twelve: Vec<Thumb> = (0..12)
        .map(|i| Thumb {
            name: format!("n{i:02}.CR3").into(),
            image: slint::Image::default(),
            ..Default::default()
        })
        .collect();
    cells::set_rows(&app, twelve);
    press(&app, Key::Shift);
    assert_eq!(*seen.borrow(), vec![(0.0, 6)]);
}

/// The shape a turn maps masks by is the frame's own, and a
/// frame whose develop has not landed has no shape on the panel
/// to borrow: the last frame's would map this one's masks by an
/// aspect that is not theirs, and say nothing about it.
#[test]
fn a_frame_without_its_own_develop_does_not_borrow_the_last_ones_shape() {
    let app = window(2);
    let files = vec![
        PathBuf::from("/nowhere/a.CR3"),
        PathBuf::from("/nowhere/b.CR3"),
    ];
    let mut st = State::empty(files, &app);
    // Nothing developed yet, and nothing else to measure by.
    assert_eq!(st.develop.source_size, (0, 0));
    st.current = Some(0);
    assert_eq!(frame_aspect(&mut st, 0), None);
    // The first frame's develop lands: landscape.
    st.develop.source_size = (6000, 4000);
    assert_eq!(frame_aspect(&mut st, 0), Some(1.5));
    // The selection moves, as `on_select` leaves it: the size is
    // forgotten, and the second frame has no shape of its own to
    // be had (its file is not there), so nothing is claimed.
    st.current = Some(1);
    st.develop.source_size = (0, 0);
    assert_eq!(
        frame_aspect(&mut st, 1),
        None,
        "the last frame's shape is not this one's"
    );
    // Its thumbnail arrives: a picture of the frame is never its shape
    // (§724), so nothing is claimed still.
    st.views.thumb_base[1] = Some((400, 600, Vec::new()));
    assert_eq!(frame_aspect(&mut st, 1), None);
}

/// The frame's own turn: a bracket either way, unmodified, and
/// nobody else's when a modifier is down or a sheet is over the
/// window.
#[test]
fn the_brackets_turn_the_frame_and_a_modifier_takes_them_away() {
    let app = window(11);
    let turns = Rc::new(RefCell::new(Vec::new()));
    let seen = turns.clone();
    app.on_frame_turned(move |q| seen.borrow_mut().push(q));
    press(&app, "[");
    press(&app, "]");
    assert_eq!(*turns.borrow(), vec![-1, 1]);
    // In culling and on the grid alike.
    app.set_culling(true);
    press(&app, "]");
    app.set_culling(false);
    app.set_grid_open(true);
    press(&app, "[");
    app.set_grid_open(false);
    assert_eq!(*turns.borrow(), vec![-1, 1, 1, -1]);
    // Ctrl+bracket is somebody else's, and a sheet keeps its own
    // keys.
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(&app, "[");
    press(&app, "]");
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(turns.borrow().len(), 4);
    app.set_export_open(true);
    press(&app, "[");
    press(&app, "]");
    assert_eq!(turns.borrow().len(), 4);
}

#[test]
fn the_culling_keys_reach_the_browser_in_both_views() {
    let app = window(11);
    let pressed = Rc::new(RefCell::new(Vec::new()));
    let seen = pressed.clone();
    app.on_meta_key(move |key| {
        // What the window sends is the key's text; what it means
        // is `meta::Change`'s to say, as it is in the editor.
        match meta::Change::from_key(&key) {
            Some(_) => {
                seen.borrow_mut().push(key.to_string());
                true
            }
            None => false,
        }
    });
    let zooms = Rc::new(RefCell::new(0));
    let counted = zooms.clone();
    app.on_zoom_key(move |_| *counted.borrow_mut() += 1);
    // The loupe: the strip is the browser there.
    for key in ["1", "5", "0", "p", "X", "u", "6", "9"] {
        press(&app, key);
    }
    assert_eq!(*pressed.borrow(), ["1", "5", "0", "p", "X", "u", "6", "9"]);
    // And the grid, where the plus and minus are still its own.
    press(&app, "g");
    press(&app, "3");
    press(&app, "+");
    assert_eq!(pressed.borrow().last().map(String::as_str), Some("3"));
    // A sheet over the window takes every key it is shown.
    app.set_export_open(true);
    press(&app, "4");
    assert_eq!(pressed.borrow().last().map(String::as_str), Some("3"));
    app.set_export_open(false);
    // The keys that are not the browser's are untouched: Z still
    // zooms, and G still closes the grid.
    press(&app, "g");
    press(&app, "z");
    assert_eq!(*zooms.borrow(), 1);
    assert!(!app.get_grid_open());
    assert_eq!(pressed.borrow().len(), 9);
}

/// A rating, a flag, or a label key in the develop view puts the
/// frame's badge up over the picture, and its timer takes it down;
/// a second key while it is up starts the timer again. Library,
/// Cull, and the lightbox show badges of their own and get no
/// flash, and culling's is as it was. With the strip put away the
/// badge is up in the develop view without a key.
#[test]
fn a_key_in_the_develop_view_puts_the_badge_up_for_a_moment() {
    use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
    use std::time::Duration;
    let app = window(0);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(4));
    // How many of the element `id` are on the window, once the
    // layout has settled into what the last change asked for.
    let here = |id: &str| {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        ElementHandle::find_by_element_id(&app, id).count()
    };
    app.invoke_select(1);
    assert_eq!(state.borrow().current, Some(1));
    assert!(!app.get_badge_flash() && !app.get_develop_badge());

    app.invoke_meta_key("p".into());
    assert_eq!(app.get_frame_flag(), meta::Flag::Pick.code());
    assert!(app.get_badge_flash() && app.get_develop_badge());
    // The badge is the develop view's; a badge with nothing to
    // say is not drawn, so it is looked for once the key landed.
    assert_eq!(here("Viewport::develop-badge"), 1);
    assert_eq!(here("Viewport::cull-badge"), 0);
    mock_elapsed_time(Duration::from_millis(1500));
    app.invoke_meta_key("3".into());
    assert_eq!(app.get_frame_rating(), 3);
    mock_elapsed_time(Duration::from_millis(1500));
    assert!(app.get_badge_flash(), "the second key started it again");
    mock_elapsed_time(Duration::from_millis(600));
    assert!(!app.get_badge_flash() && !app.get_develop_badge());

    // Library, the lightbox and Cull: the key lands, no flash.
    app.set_grid_open(true);
    app.invoke_meta_key("x".into());
    assert_eq!(app.get_frame_flag(), meta::Flag::Reject.code());
    app.set_grid_open(false);
    app.set_lightbox(true);
    app.invoke_meta_key("u".into());
    app.set_lightbox(false);
    assert_eq!(app.get_frame_flag(), 0);
    app.invoke_cull_toggled();
    assert!(app.get_cull_mode());
    app.invoke_meta_key("6".into());
    assert!(app.get_frame_label() > 0);
    assert!(!app.get_badge_flash());
    // Culling's own badge, in place of this one, and the strip
    // put away there changes nothing of it.
    app.set_strip_hidden(true);
    assert!(!app.get_develop_badge());
    assert_eq!(here("Viewport::cull-badge"), 1);
    assert_eq!(here("Viewport::develop-badge"), 0);

    // Back in Develop with the strip still away: up, no key.
    app.invoke_cull_leave();
    assert!(!app.get_cull_mode());
    assert!(!app.get_badge_flash() && app.get_develop_badge());
    assert_eq!(here("Viewport::develop-badge"), 1);
    app.set_strip_hidden(false);
    assert!(!app.get_develop_badge());

    // P, then an arrow within the moment: the flash answered the
    // last frame, and goes with it.
    app.invoke_select(0);
    app.invoke_meta_key("p".into());
    assert!(app.get_badge_flash());
    app.invoke_step(1, false);
    assert_eq!(state.borrow().current, Some(1));
    assert!(!app.get_badge_flash() && !app.get_develop_badge());

    // P, then Library and back within the moment: nothing of it
    // is left to see.
    app.invoke_meta_key("p".into());
    assert!(app.get_badge_flash());
    app.set_grid_open(true);
    mock_elapsed_time(Duration::from_millis(100));
    app.set_grid_open(false);
    mock_elapsed_time(Duration::from_millis(100));
    assert!(!app.get_badge_flash() && !app.get_develop_badge());

    // X under the Picks filter, 1 and 3 the picks and 1 open: 1
    // leaves the list and the selection moves on to 3, whose pick
    // is not the key's answer, so nothing flashes.
    {
        let mut st = state.borrow_mut();
        for (i, flag) in [
            meta::Flag::None,
            meta::Flag::Pick,
            meta::Flag::None,
            meta::Flag::Pick,
        ]
        .into_iter()
        .enumerate()
        {
            st.sidecars[i].meta.flag = flag;
        }
    }
    app.invoke_filter_flag_toggled(1, false);
    assert_eq!(state.borrow().views.shown, vec![1, 3]);
    assert_eq!(state.borrow().current, Some(1));
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().current, Some(3), "the selection moved on");
    assert_eq!(app.get_frame_flag(), meta::Flag::Pick.code());
    assert!(!app.get_badge_flash() && !app.get_develop_badge());
}

#[test]
fn plus_and_minus_size_the_cells_only_in_the_grid() {
    let app = window(11);
    let zooms = Rc::new(RefCell::new(Vec::new()));
    let seen = zooms.clone();
    app.on_grid_zoom(move |by| seen.borrow_mut().push(by));
    press(&app, "+");
    assert!(zooms.borrow().is_empty());
    press(&app, "g");
    press(&app, "+");
    press(&app, "=");
    press(&app, "-");
    assert_eq!(*zooms.borrow(), vec![1, 1, -1]);
}

#[test]
fn a_frame_rated_but_never_developed_still_gets_its_iso_blend() {
    let dir = std::env::temp_dir().join(format!("greycard-seed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let rated = dir.join("IMG_0001.CR3");
    let developed = dir.join("IMG_0002.CR3");
    let untouched = dir.join("IMG_0003.CR3");
    let picture = dir.join("IMG_0004.JPG");

    // A star and nothing else: a sidecar with a default edit.
    let mut sidecar = Sidecar::default();
    sidecar.meta.set_rating(4);
    sidecar.save(&rated).expect("the sidecar writes");
    assert!(files::never_developed(&sidecar));
    // One that has been developed, however lightly.
    let mut sidecar = Sidecar::default();
    let mut edit = Edit::default();
    edit.light.exposure = 0.3;
    sidecar.record(edit);
    sidecar.save(&developed).expect("the sidecar writes");
    assert!(!files::never_developed(&sidecar));
    // And one rated in a picture that is not a raw, which has no
    // learned denoiser to seed.
    let mut sidecar = Sidecar {
        line: greycard_edit::Line::of(Edit::for_picture(), 0).unwrap(),
        ..Sidecar::default()
    };
    sidecar.meta.set_rating(1);
    sidecar.save(&picture).expect("the sidecar writes");

    let files = vec![rated, developed, untouched, picture];
    let (sidecars, seed) = load_sidecars(&files, true);
    assert_eq!(sidecars[0].meta.rating, 4, "the meta came back");
    assert_eq!(seed, vec![true, false, true, false]);
    // With sidecars off nothing is read and nothing is seeded.
    assert_eq!(load_sidecars(&files, false).1, vec![false; 4]);
    crate::testing::remove_dir_retry(&dir);
}

/// The folder's round of thumbnails closes on the last first
/// arrival, a failure counting as one, and a larger picture made
/// again for the grid not counting at all.
#[test]
fn a_folders_thumbnails_are_counted_once_each() {
    let mut run = ThumbRun::new(3);
    assert_eq!(run.arrived(0, Some(true), 0.001), None);
    assert_eq!(run.arrived(0, Some(false), 0.2), None, "the grid's second");
    assert_eq!(run.arrived(7, Some(false), 0.2), None, "not this folder's");
    assert_eq!(run.arrived(2, None, 0.0), None);
    let done = run.arrived(1, Some(false), 0.05).expect("the last one");
    assert_eq!(
        (done.files, done.of, done.cached, done.made, done.failed),
        (3, 3, 1, 1, 1)
    );
    assert!((done.work - 0.051).abs() < 1e-9);
    assert_eq!(run.arrived(1, Some(true), 0.0), None);
}

/// Closed with the pool idle, a cold root's far frames never come:
/// the run says what has, of how many.
#[test]
fn a_folders_round_closed_early_says_what_came_of_how_many() {
    let mut run = ThumbRun::new(5);
    run.arrived(3, Some(false), 0.1);
    run.arrived(4, Some(true), 0.0);
    let t = run.totals();
    assert_eq!((t.files, t.of, t.made, t.cached), (2, 5, 1, 1));
}

/// A run closes early only with the pool idle after making pictures
/// for it, nothing finished still waiting for the window, and the
/// rows on screen filled; a pass's end heard before anything was
/// made for it is the last folder's, and counts for nothing.
#[test]
fn a_folders_round_closes_early_only_with_all_taken_in() {
    let mut run = ThumbRun::new(5);
    run.arrived(0, Some(true), 0.0);
    run.pool_idle();
    assert!(!run.may_close(true), "the last folder's pass");
    run.arrived(1, Some(false), 0.1);
    assert!(!run.may_close(true), "the pool not idle");
    run.pool_idle();
    run.outbox_left = true;
    assert!(!run.may_close(true), "pictures still in the outbox");
    run.outbox_left = false;
    assert!(!run.may_close(false), "the rows on screen not filled");
    assert!(run.may_close(true));
}

/// For the numbers: every sidecar under `GREYCARD_SIDECAR_BENCH`,
/// read one after another as a folder's open reads them and on the
/// pool as the all-roots view does. `cargo test --release --
/// --ignored sidecars_for_the_numbers --nocapture`.
#[test]
#[ignore = "reads a tree named by GREYCARD_SIDECAR_BENCH"]
fn sidecars_for_the_numbers() {
    let Some(top) = std::env::var_os("GREYCARD_SIDECAR_BENCH") else {
        return;
    };
    let mut files = Vec::new();
    let mut dirs = vec![PathBuf::from(top)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    dirs.push(p);
                }
            } else if greycard_core::decode::is_raw_path(&p) {
                files.push(p);
            }
        }
    }
    files.sort();
    for round in 0..2 {
        let t = std::time::Instant::now();
        let (one, _) = load_sidecars(&files, true);
        let serial = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let none = vec![None; files.len()];
        let pool = crate::rows::bring(
            &files,
            &none,
            Some(greycard_edit::Placement::Beside),
            Default::default(),
            None,
            greycard_edit_io::on_disk::frame_shape,
        );
        let parallel = t.elapsed().as_secs_f64();
        assert_eq!(one.len(), pool.len());
        eprintln!(
            "round {round}: {} sidecars, {serial:.3} s one after another, {parallel:.3} s on the pool",
            files.len()
        );
    }
}
/// What the grid reported, in order: scroll, height, columns.
type Reports = Rc<RefCell<Vec<(f32, f32, i32)>>>;

/// The grid at 1500 by 950 over `count` frames, opened on the
/// first, with what it reports kept: the sheet is 805 tall beside
/// the pane, six columns.
fn grid_with_reports(count: usize) -> (App, Reports) {
    let app = window(count);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let seen = cells_from_reports(&app);
    app.set_selected(0);
    press(&app, "g");
    (app, seen)
}

/// The grid's reports kept, and its cells moved by them as the
/// browser moves them.
fn cells_from_reports(app: &App) -> Reports {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let (reports, app_weak) = (seen.clone(), app.as_weak());
    app.on_grid_range(move |scroll, height, cols, top| {
        reports.borrow_mut().push((scroll, height, cols));
        let app = app_weak.upgrade().unwrap();
        let count = app.get_thumbs().row_count() as i32;
        let layout = cells::layout(
            &app,
            app.get_grid_cell(),
            cols,
            count,
            &[],
            grid_shape(&app),
        );
        let (at, wanted) = layout.window(scroll, height, top);
        cells::show(&app, cells::View::Grid, at, wanted);
    });
    seen
}

fn last_scroll(seen: &Reports) -> f32 {
    seen.borrow().last().map(|r| r.0).expect("a report")
}

fn wheel(app: &App, x: f32, y: f32, by: f32) {
    app.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(x, y),
        delta_x: 0.0,
        delta_y: by,
    });
}

#[test]
fn the_scroll_bar_sits_at_the_sheets_edge_and_its_thumb_drags() {
    let (app, seen) = grid_with_reports(120);
    let max = shaped(120).max_scroll(805.0, 0.0);
    let (at, size) = crate::testing::by_id(&app, "GridSheet::band");
    // The band down the sheet's right edge, under the header.
    assert_eq!((at.x, size.width), (1500.0 - 14.0, 14.0));
    assert_eq!((at.y, size.height), (950.0 - 805.0, 805.0));
    // A drag of the thumb, taken 10 px into it, 150 px down in
    // steps with no time between them, as a fast mouse sends them
    // within a frame: the moves are gathered into one step, which
    // the release applies, where the arithmetic puts it, and
    // reports the range it shows, the cells following.
    let top = at.y + grid::bar_offset(805.0, max, 0.0);
    let x = at.x + 7.0;
    let path: Vec<(f32, f32)> = (0..=15)
        .map(|k| (x, top + 10.0 + 10.0 * k as f32))
        .collect();
    seen.borrow_mut().clear();
    crate::testing::drag(&app, &path);
    let to = grid::bar_drag(805.0, max, 0.0, 150.0);
    assert!(to > 0.0 && to < max);
    assert_eq!(seen.borrow().len(), 1, "{:?}", seen.borrow());
    assert!((last_scroll(&seen) - to).abs() < 0.01);
    // Moves a frame apart are a step each.
    seen.borrow_mut().clear();
    let button = slint::platform::PointerEventButton::Left;
    let at_y = |y: f32| slint::LogicalPosition::new(x, y);
    let grab = at.y + grid::bar_offset(805.0, max, to) + 10.0;
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at_y(grab),
        button,
    });
    for k in 1..=3 {
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: at_y(grab + 20.0 * k as f32),
        });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
        press(&app, Key::Shift);
    }
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at_y(grab + 60.0),
        button,
    });
    assert_eq!(seen.borrow().len(), 3, "{:?}", seen.borrow());
    let to = grid::bar_drag(805.0, max, to, 60.0);
    assert!((last_scroll(&seen) - to).abs() < 0.01);
    let (first, last) = shaped(120).visible(to, 805.0, 0.0).unwrap();
    let held: Vec<i32> = app.get_grid_cells().iter().map(|c| c.row).collect();
    assert!(
        held.contains(&first) && held.contains(&last),
        "{first}..{last} in {held:?}"
    );
    assert_eq!(app.get_selected(), 0);
    // The keys kept their focus: G closes the grid.
    press(&app, "g");
    assert!(!app.get_grid_open());
}

#[test]
fn a_press_in_the_track_pages_and_the_wheel_over_the_bar_scrolls() {
    let (app, seen) = grid_with_reports(120);
    let max = shaped(120).max_scroll(805.0, 0.0);
    let (at, _) = crate::testing::by_id(&app, "GridSheet::band");
    let x = at.x + 7.0;
    // Below the thumb: a sheet's height down.
    crate::testing::click(&app, x, 950.0 - 20.0);
    assert_eq!(last_scroll(&seen), 805.0);
    crate::testing::click(&app, x, 950.0 - 20.0);
    assert_eq!(last_scroll(&seen), 2.0 * 805.0);
    // Above it: a sheet's height back up, and no further than the
    // top.
    crate::testing::click(&app, x, at.y + 6.0);
    assert_eq!(last_scroll(&seen), 805.0);
    crate::testing::click(&app, x, at.y + 6.0);
    crate::testing::click(&app, x, at.y + 6.0);
    assert_eq!(last_scroll(&seen), 0.0);
    // Down to the end and held there.
    for _ in 0..6 {
        crate::testing::click(&app, x, 950.0 - 6.0);
    }
    assert_eq!(last_scroll(&seen), max);
    // The wheel over the bar is the sheet's wheel.
    wheel(&app, x, 500.0, 120.0);
    assert!(last_scroll(&seen) < max);
    assert_eq!(app.get_selected(), 0);
}

#[test]
fn a_press_on_the_bar_never_reaches_the_cell_under_it() {
    // A sheet exactly a row of eight wide, the pane put away: the
    // last column's right edge lies 12 px in, under the band.
    let clicks = Rc::new(RefCell::new(Vec::new()));
    let menus = Rc::new(RefCell::new(Vec::new()));
    let run = |count: usize| {
        let app = window(count);
        app.window()
            .set_size(slint::LogicalSize::new(1460.0, 950.0));
        cells_from_reports(&app);
        app.set_selected(0);
        press(&app, "g");
        press(&app, Key::F7);
        press(&app, Key::Shift);
        assert_eq!(app.get_grid_w(), 1460.0);
        let c = clicks.clone();
        app.on_frame_clicked(move |row, _, _| c.borrow_mut().push(row));
        let m = menus.clone();
        app.on_frame_menu_asked(move |row| m.borrow_mut().push(row));
        // In frame 7's cell, in the band.
        let (x, y) = (1460.0 - 13.0, 950.0 - 805.0 + 100.0);
        crate::testing::click(&app, x, y);
        let position = slint::LogicalPosition::new(x, y);
        let button = slint::platform::PointerEventButton::Right;
        app.window()
            .dispatch_event(WindowEvent::PointerPressed { position, button });
        app.window()
            .dispatch_event(WindowEvent::PointerReleased { position, button });
        app
    };
    // A folder that fits has no bar, and the press is the cell's.
    let app = run(8);
    assert_eq!(*clicks.borrow(), vec![7]);
    assert_eq!(*menus.borrow(), vec![7]);
    drop(app);
    clicks.borrow_mut().clear();
    menus.borrow_mut().clear();
    // One that scrolls has the bar there, and the press is its.
    let _app = run(400);
    assert!(clicks.borrow().is_empty(), "{:?}", clicks.borrow());
    assert!(menus.borrow().is_empty(), "{:?}", menus.borrow());
}

#[test]
fn the_bar_wakes_with_a_scroll_and_sleeps_at_rest() {
    let (app, seen) = grid_with_reports(120);
    // Opened at the top, nothing has scrolled: asleep.
    assert!(!app.get_grid_bar_awake());
    wheel(&app, 700.0, 500.0, -120.0);
    assert!(last_scroll(&seen) > 0.0);
    press(&app, Key::Shift);
    assert!(app.get_grid_bar_awake());
    let tick = |ms: u64| {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        press(&app, Key::Shift);
    };
    // Awake while the scroll keeps coming.
    for _ in 0..4 {
        tick(400);
        wheel(&app, 700.0, 500.0, -120.0);
        press(&app, Key::Shift);
        assert!(app.get_grid_bar_awake());
    }
    // A tick with no scroll since the last, and it sleeps: about a
    // second at rest. The timer stops with it, and nothing changes
    // after.
    tick(700);
    tick(700);
    assert!(!app.get_grid_bar_awake());
    let reports = seen.borrow().len();
    for _ in 0..5 {
        tick(1000);
        assert!(!app.get_grid_bar_awake());
    }
    assert_eq!(seen.borrow().len(), reports);
}

#[test]
fn no_bar_when_the_folder_fits() {
    let (app, seen) = grid_with_reports(12);
    assert_eq!(shaped(12).max_scroll(805.0, 0.0), 0.0);
    let visible = |app: &App| {
        use i_slint_backend_testing::ElementHandle;
        press(app, Key::Shift);
        ElementHandle::find_by_element_id(app, "GridSheet::band").count() > 0
    };
    assert!(!visible(&app));
    // Neither a press nor the wheel down the band moves anything.
    crate::testing::click(&app, 1493.0, 900.0);
    wheel(&app, 1493.0, 500.0, -120.0);
    assert!(seen.borrow().iter().all(|r| r.0 == 0.0));
    // A longer folder brings it.
    let more: Vec<Thumb> = (0..120)
        .map(|i| Thumb {
            name: format!("n{i:03}.CR3").into(),
            image: slint::Image::default(),
            ..Default::default()
        })
        .collect();
    cells::set_rows(&app, more);
    assert!(visible(&app));
}
#[test]
fn a_thumb_dragged_past_the_end_and_back_follows_without_a_jump() {
    let (app, seen) = grid_with_reports(120);
    let max = shaped(120).max_scroll(805.0, 0.0);
    let (at, _) = crate::testing::by_id(&app, "GridSheet::band");
    let x = at.x + 7.0;
    let grab = at.y + grid::bar_offset(805.0, max, 0.0) + 10.0;
    let to = |y: f32| slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
    let next_frame = || {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
        press(&app, Key::Shift);
    };
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: to(grab) });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: to(grab),
        button,
    });
    // Far past the window's foot: held at the end.
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: to(grab + 2000.0),
    });
    next_frame();
    assert_eq!(last_scroll(&seen), max);
    // Back to 50 px under the grab: where 50 px from the start
    // puts it, not 50 px from where the end was reached.
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: to(grab + 50.0),
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: to(grab + 50.0),
        button,
    });
    assert_eq!(last_scroll(&seen), grid::bar_drag(805.0, max, 0.0, 50.0));
}

#[test]
fn a_press_on_the_bar_with_a_menu_up_only_closes_the_menu() {
    let (app, seen) = grid_with_reports(120);
    let (at, _) = crate::testing::by_id(&app, "GridSheet::band");
    // The frame menu up over the first frame.
    let position = slint::LogicalPosition::new(400.0, 250.0);
    let button = slint::platform::PointerEventButton::Right;
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    app.window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    app.window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
    assert!(app.get_menu_up(), "the frame menu");
    let reports = seen.borrow().len();
    // A press in the track below the thumb: the menu closes and
    // the sheet does not page.
    crate::testing::click(&app, at.x + 7.0, 950.0 - 20.0);
    press(&app, Key::Shift);
    assert!(!app.get_menu_up(), "the press closed the menu");
    assert!(
        seen.borrow()[reports..].iter().all(|r| r.0 == 0.0),
        "{:?}",
        seen.borrow()
    );
    assert_eq!(app.get_selected(), 0);
    // The next press pages.
    crate::testing::click(&app, at.x + 7.0, 950.0 - 20.0);
    assert_eq!(last_scroll(&seen), 805.0);
}

#[test]
fn a_click_on_a_frame_with_a_menu_up_only_closes_the_menu() {
    let (app, _seen) = grid_with_reports(120);
    let clicks = Rc::new(std::cell::Cell::new(0));
    {
        let clicks = clicks.clone();
        app.on_frame_clicked(move |_, _, _| clicks.set(clicks.get() + 1));
    }
    let position = slint::LogicalPosition::new(1000.0, 250.0);
    let button = slint::platform::PointerEventButton::Right;
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    app.window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    app.window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
    assert!(app.get_menu_up(), "the frame menu");
    // On another frame, clear of the menu, which opens down and
    // right of the press.
    crate::testing::click(&app, 400.0, 250.0);
    assert!(!app.get_menu_up(), "the press closed the menu");
    assert_eq!(clicks.get(), 0, "and picked no frame");
    crate::testing::click(&app, 400.0, 250.0);
    assert_eq!(clicks.get(), 1, "the next click picks it");
}

/// The strip's bar, if it is there: its top left, its size, the
/// scroll it reads and the scroll's maximum.
fn strip_bar(app: &App) -> Option<(slint::LogicalPosition, slint::LogicalSize, f32, f32)> {
    use i_slint_backend_testing::ElementHandle;
    press(app, Key::Shift);
    let bar = ElementHandle::find_by_element_id(app, "Filmstrip::scroll-bar").next()?;
    let value = bar.accessible_value()?.parse().ok()?;
    let max = bar.accessible_value_maximum()?;
    Some((bar.absolute_position(), bar.size(), value, max))
}

/// What the strip reported, in order: its first frame and its last.
type StripReports = Rc<RefCell<Vec<(i32, i32)>>>;

/// The strip at 1500 by 950 over `count` frames, on the first, with
/// what it reports kept and its cells moved by them as the browser
/// moves them.
fn strip_with(count: usize) -> (App, StripReports) {
    let app = window(count);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let (reports, app_weak) = (seen.clone(), app.as_weak());
    app.on_strip_range(move |first, last| {
        reports.borrow_mut().push((first, last));
        let (at, wanted) = cells::strip_window(first as usize, last as usize);
        cells::show(&app_weak.upgrade().unwrap(), cells::View::Strip, at, wanted);
    });
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    app.set_selected(0);
    press(&app, Key::Shift);
    (app, seen)
}

/// The row the strip scrolls along, as the window lays it out now:
/// its cells and the pad at either end.
fn strip_row(app: &App) -> f32 {
    cells::strip_layout(app).length()
}

#[test]
fn the_strips_bar_runs_along_its_foot_and_a_press_in_the_track_pages() {
    let (app, reports) = strip_with(40);
    let (at, size, value, max) = strip_bar(&app).expect("a bar on a long strip");
    // The band along the strip's foot, the strip's width.
    assert_eq!((at.y, size.height), (950.0 - 12.0, 12.0));
    let w = size.width;
    assert!(w > 1000.0, "{w}");
    assert_eq!(max, (strip_row(&app) - w).round());
    assert_eq!(value, 0.0);
    // Past the thumb: a strip's width along, then two, and the
    // strip says what it shows from there.
    let y = at.y + 6.0;
    let past = at.x + w - 20.0;
    crate::testing::click(&app, past, y);
    assert_eq!(strip_bar(&app).unwrap().2, w.round());
    crate::testing::click(&app, past, y);
    assert_eq!(strip_bar(&app).unwrap().2, (2.0 * w).round());
    let first = cells::strip_layout(&app).shown(2.0 * w, w).unwrap().0;
    assert_eq!(reports.borrow().last().map(|r| r.0), Some(first));
    // Before it: back a width, and no further than the start.
    crate::testing::click(&app, at.x + 6.0, y);
    assert_eq!(strip_bar(&app).unwrap().2, w.round());
    crate::testing::click(&app, at.x + 6.0, y);
    crate::testing::click(&app, at.x + 6.0, y);
    assert_eq!(strip_bar(&app).unwrap().2, 0.0);
    // To the end and held there.
    for _ in 0..8 {
        crate::testing::click(&app, past, y);
    }
    assert_eq!(strip_bar(&app).unwrap().2, max);
    // None of it chose a frame.
    assert_eq!(app.get_selected(), 0);
}

#[test]
fn the_strips_thumb_drags_and_the_wheel_over_the_bar_moves_it() {
    let (app, _) = strip_with(40);
    let clicks = Rc::new(RefCell::new(Vec::new()));
    let c = clicks.clone();
    app.on_frame_clicked(move |row, _, _| c.borrow_mut().push(row));
    let (at, size, _, _) = strip_bar(&app).unwrap();
    let w = size.width;
    let max = strip_row(&app) - w;
    // Taken 10 px into the thumb and moved 150 px along: the thumb
    // follows the pointer, and the strip comes to where the grid's
    // arithmetic puts it.
    let x = at.x + grid::bar_offset(w, max, 0.0) + 10.0;
    let y = at.y + 6.0;
    let path: Vec<(f32, f32)> = (0..=15).map(|k| (x + 10.0 * k as f32, y)).collect();
    crate::testing::drag(&app, &path);
    let to = grid::bar_drag(w, max, 0.0, 150.0);
    assert!(to > 0.0 && to < max);
    assert_eq!(strip_bar(&app).unwrap().2, to.round());
    // Far past the end: held there.
    let grab = at.x + grid::bar_offset(w, max, to) + 10.0;
    crate::testing::drag(&app, &[(grab, y), (grab + 5000.0, y)]);
    assert_eq!(strip_bar(&app).unwrap().2, max.round());
    // A wheel notch over the band moves the strip back, as over
    // the frames.
    wheel(&app, at.x + w / 2.0, y, 120.0);
    assert_eq!(strip_bar(&app).unwrap().2, (max - 120.0).round());
    assert!(clicks.borrow().is_empty(), "{:?}", clicks.borrow());
    assert_eq!(app.get_selected(), 0);
    // The keys kept their focus: G opens the grid.
    press(&app, "g");
    assert!(app.get_grid_open());
}

#[test]
fn one_frame_has_no_strip_bar_and_a_folder_loaded_brings_one() {
    use i_slint_backend_testing::ElementHandle;
    let (app, _) = strip_with(1);
    let clicks = Rc::new(RefCell::new(Vec::new()));
    let c = clicks.clone();
    app.on_frame_clicked(move |row, _, _| c.borrow_mut().push(row));
    assert!(strip_bar(&app).is_none());
    // The frame ends where the band begins, the strip's padding
    // above the foot: its last pixel row is its own.
    let cell = ElementHandle::find_by_element_id(&app, "Filmstrip::tt")
        .next()
        .expect("the frame's cell");
    let foot = cell.absolute_position().y + cell.size().height;
    assert_eq!(foot, 950.0 - 12.0);
    crate::testing::click(&app, 100.0, foot - 1.0);
    assert_eq!(*clicks.borrow(), vec![0]);
    clicks.borrow_mut().clear();
    // A folder of forty swapped in while the strip is on screen:
    // the bar comes with the strip's new length, at the start.
    let more: Vec<Thumb> = (0..40)
        .map(|i| Thumb {
            name: format!("n{i:02}.CR3").into(),
            image: slint::Image::default(),
            ..Default::default()
        })
        .collect();
    cells::set_rows(&app, more);
    let (_, size, value, max) = strip_bar(&app).expect("a bar for forty");
    assert_eq!(max, (strip_row(&app) - size.width).round());
    assert_eq!(value, 0.0);
    // With the bar there the frame's last row is still the frame's,
    // and a press in the band below it is the bar's.
    crate::testing::click(&app, 100.0, foot - 1.0);
    assert_eq!(*clicks.borrow(), vec![0]);
    clicks.borrow_mut().clear();
    crate::testing::click(&app, 100.0, foot + 1.0);
    assert!(clicks.borrow().is_empty(), "{:?}", clicks.borrow());
    // Back to a folder that fits: gone again.
    cells::set_rows(&app, Vec::new());
    assert!(strip_bar(&app).is_none());
}

#[test]
fn the_strips_bar_follows_the_window_when_it_is_resized() {
    let (app, _) = strip_with(40);
    let (_, wide, _, wide_max) = strip_bar(&app).unwrap();
    app.window()
        .set_size(slint::LogicalSize::new(1000.0, 950.0));
    let (at, size, value, max) = strip_bar(&app).unwrap();
    assert_eq!(size.width, wide.width - 500.0);
    assert_eq!(max, wide_max + 500.0);
    assert_eq!(value, 0.0);
    // The thumb is where the arithmetic puts it on the narrower
    // strip: a press on its far end takes it, and one just past
    // pages.
    let (w, max) = (size.width, strip_row(&app) - size.width);
    let end = at.x + grid::bar_offset(w, max, 0.0) + grid::bar_length(w, max);
    crate::testing::click(&app, end - 2.0, at.y + 6.0);
    assert_eq!(strip_bar(&app).unwrap().2, 0.0);
    crate::testing::click(&app, end + 2.0, at.y + 6.0);
    assert_eq!(strip_bar(&app).unwrap().2, w.round());
    // Taller, the band keeps to the foot.
    app.window()
        .set_size(slint::LogicalSize::new(1000.0, 1100.0));
    let (at, _, _, _) = strip_bar(&app).unwrap();
    assert_eq!(at.y, 1100.0 - 12.0);
}

#[test]
fn the_strips_bar_wakes_with_a_scroll_and_sleeps_at_rest() {
    let (app, _) = strip_with(40);
    assert!(!app.get_strip_bar_awake());
    let (at, size, _, _) = strip_bar(&app).unwrap();
    wheel(&app, 600.0, at.y - 60.0, -120.0);
    assert_eq!(strip_bar(&app).unwrap().2, 120.0);
    assert!(app.get_strip_bar_awake());
    let tick = |ms: u64| {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        press(&app, Key::Shift);
    };
    tick(700);
    tick(700);
    assert!(!app.get_strip_bar_awake());
    // A page wakes it too.
    crate::testing::click(&app, at.x + size.width - 20.0, at.y + 6.0);
    assert!(app.get_strip_bar_awake());
}

/// Shift held around a key, as a hand holds it.
fn press_shifted(app: &App, key: Key) {
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    press(app, key);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
}

#[test]
fn home_and_end_go_to_the_ends_of_the_strip_and_choose_the_frame_there() {
    let (app, _) = strip_with(40);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(40));
    app.invoke_select(5);
    press(&app, Key::Shift);
    assert_eq!(app.get_selected(), 5);
    let (_, _, _, max) = strip_bar(&app).unwrap();
    press(&app, Key::End);
    assert_eq!(app.get_selected(), 39);
    assert_eq!(strip_bar(&app).unwrap().2, max);
    press(&app, Key::Home);
    assert_eq!(app.get_selected(), 0);
    assert_eq!(strip_bar(&app).unwrap().2, 0.0);
    // The first frame chosen and the strip wheeled away from it:
    // Home still brings the strip back.
    wheel(&app, 600.0, 860.0, -600.0);
    assert!(strip_bar(&app).unwrap().2 > 0.0);
    press(&app, Key::Home);
    assert_eq!(strip_bar(&app).unwrap().2, 0.0);
    assert_eq!(app.get_set_count(), 1);
    // With Shift, from the sixth frame: the set takes the whole run
    // to the last, as Shift held on the right arrow would, and the
    // last is the current frame.
    app.invoke_select(5);
    press_shifted(&app, Key::End);
    assert_eq!(app.get_selected(), 39);
    assert_eq!(app.get_set_count(), 35);
    assert_eq!(chosen_frames(&state.borrow()), (5..40).collect::<Vec<_>>());
    assert_eq!(state.borrow().current, Some(39));
    // And back with Shift+Home: the run to the first joins it.
    press_shifted(&app, Key::Home);
    assert_eq!(app.get_selected(), 0);
    assert_eq!(app.get_set_count(), 40);
}

#[test]
fn home_and_end_under_a_filter_go_to_the_first_and_last_shown() {
    let (app, _) = strip_with(0);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(40));
    // The first two frames and the last two rejected, and only
    // the rest shown: files 2 to 37 on rows 0 to 35.
    {
        let mut st = state.borrow_mut();
        for i in [0, 1, 38, 39] {
            st.sidecars[i].meta.flag = meta::Flag::Reject;
        }
        st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
        rebuild_browser(&mut st, &app);
        assert_eq!(st.views.shown, (2..38).collect::<Vec<_>>());
    }
    app.invoke_select(10);
    press(&app, Key::End);
    assert_eq!(app.get_selected(), 35);
    assert_eq!(state.borrow().current, Some(37));
    assert_eq!(strip_bar(&app).unwrap().2, strip_bar(&app).unwrap().3);
    press(&app, Key::Home);
    assert_eq!(app.get_selected(), 0);
    assert_eq!(state.borrow().current, Some(2));
    // In the grid the same.
    press(&app, "g");
    press(&app, Key::End);
    assert_eq!(state.borrow().current, Some(37));
    // The run Shift takes is of shown frames only.
    press_shifted(&app, Key::Home);
    assert_eq!(state.borrow().current, Some(2));
    assert_eq!(chosen_frames(&state.borrow()), (2..38).collect::<Vec<_>>());
}

/// Ctrl held around a key.
fn press_ctrl(app: &App, key: impl Into<slint::SharedString>) {
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(app, key);
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}

#[test]
fn ctrl_a_chooses_every_frame_shown_and_leaves_the_current_one() {
    let (app, _) = strip_with(0);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(40));
    // Files 2 to 37 shown on rows 0 to 35, as under Home and End.
    {
        let mut st = state.borrow_mut();
        for i in [0, 1, 38, 39] {
            st.sidecars[i].meta.flag = meta::Flag::Reject;
        }
        st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
        rebuild_browser(&mut st, &app);
    }
    app.invoke_select(10);
    assert_eq!(state.borrow().current, Some(12));
    let all: Vec<usize> = (2..38).collect();
    press_ctrl(&app, "a");
    assert_eq!(chosen_frames(&state.borrow()), all);
    assert_eq!(app.get_set_count(), 36);
    assert_eq!(state.borrow().current, Some(12));
    assert_eq!(app.get_selected(), 10);
    // Again: nothing changes.
    press_ctrl(&app, "a");
    assert_eq!(chosen_frames(&state.borrow()), all);
    assert_eq!(state.borrow().current, Some(12));
    // A Shift+click after measures from the current frame.
    app.invoke_frame_clicked(0, false, true);
    assert_eq!(chosen_frames(&state.borrow()), (2..13).collect::<Vec<_>>());
    assert_eq!(state.borrow().current, Some(12));
    // In the grid the same.
    press(&app, "g");
    assert!(app.get_grid_open());
    press_ctrl(&app, "a");
    assert_eq!(chosen_frames(&state.borrow()), all);
    assert_eq!(state.borrow().current, Some(12));
    assert!(app.get_grid_open(), "and the grid stays");
    // In the lightbox the same.
    press(&app, Key::Escape);
    assert_eq!(app.get_set_count(), 1);
    press(&app, "l");
    assert!(app.get_lightbox());
    press_ctrl(&app, "a");
    assert_eq!(chosen_frames(&state.borrow()), all);
    assert_eq!(state.borrow().current, Some(12));
    assert!(app.get_lightbox(), "and the lightbox stays");
    // And in Cull.
    press(&app, Key::Escape);
    assert_eq!(app.get_set_count(), 1);
    press(&app, "l");
    assert!(!app.get_lightbox());
    press(&app, "c");
    assert!(app.get_culling());
    press_ctrl(&app, "a");
    assert_eq!(chosen_frames(&state.borrow()), all);
    assert_eq!(state.borrow().current, Some(12));
    assert!(app.get_culling(), "and Cull stays");
    // The filter field with the focus keeps Ctrl+A for its text.
    press(&app, Key::Escape);
    assert_eq!(app.get_set_count(), 1);
    app.set_filter_focus(true);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    press(&app, "x");
    press_ctrl(&app, "a");
    press(&app, "y");
    assert_eq!(app.get_filter_text(), "y");
    assert_eq!(app.get_set_count(), 1);
    // And the keys sheet lists it.
    let rows = crate::keys::rows("Ctrl+A", false);
    assert!(
        rows.iter()
            .any(|r| !r.heading && r.does == crate::i18n::text("Select every frame shown")),
        "{:?}",
        rows.iter().map(|r| r.does.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn home_and_end_go_to_the_ends_of_the_grid_and_choose_the_frame_there() {
    // The browser's own handlers first, then the grid's reports
    // kept over theirs.
    let app = window(120);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = crate::testing::state_for(&app, crate::testing::folder(120));
    let seen = cells_from_reports(&app);
    app.invoke_select(0);
    press(&app, "g");
    let max = shaped(120).max_scroll(805.0, 0.0);
    press(&app, Key::End);
    assert_eq!(app.get_selected(), 119);
    assert_eq!(last_scroll(&seen), max);
    press(&app, Key::Home);
    assert_eq!(app.get_selected(), 0);
    assert_eq!(last_scroll(&seen), 0.0);
    // Chosen already and scrolled away: Home brings the sheet back.
    wheel(&app, 700.0, 500.0, -600.0);
    assert!(last_scroll(&seen) > 0.0);
    press(&app, Key::Home);
    assert_eq!(last_scroll(&seen), 0.0);
    assert!(app.get_grid_open(), "and the grid stays");
}

#[test]
fn home_and_end_in_the_filter_field_are_the_fields() {
    let (app, seen) = grid_with_reports(120);
    let ends = Rc::new(RefCell::new(Vec::new()));
    let e = ends.clone();
    app.on_step_end(move |end, _| e.borrow_mut().push(end));
    // The field takes the focus as the filter key sends it there.
    app.set_filter_focus(true);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    press(&app, "a");
    press(&app, Key::Home);
    press(&app, "b");
    press(&app, Key::End);
    press(&app, "c");
    assert!(ends.borrow().is_empty(), "{:?}", ends.borrow());
    assert!(seen.borrow().iter().all(|r| r.0 == 0.0));
    // Home put the cursor before the "a", End after it. Slint leaves
    // the cursor where it is on Apple systems, as a Mac field does.
    let typed = if cfg!(target_vendor = "apple") {
        "abc"
    } else {
        "bac"
    };
    assert_eq!(app.get_filter_text(), typed);
}

/// The strip's marks for the set and the current frame, five
/// frames with the middle three in the set and the middle one
/// current, over dark and over bright pictures (synthetic), in
/// each theme at scales 1 and 2, drawn by Slint's software
/// renderer. Run with `GREYCARD_STRIP_CAPTURES=DIR` and
/// `--ignored`.
#[test]
#[ignore]
fn capture_the_strips_set() {
    use crate::testing;
    use slint::platform::WindowEvent;
    let Some(dir) = std::env::var_os("GREYCARD_STRIP_CAPTURES") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A night street and a pale sky, 3:2: the ring against a
    // picture darker than every theme's strip and one lighter.
    let picture = |bright: bool| {
        let (w, h) = (240u32, 160u32);
        let mut px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        for (i, p) in px.make_mut_slice().iter_mut().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let v = if bright {
                255 - (y * 30 / h) as u8 - (x * 10 / w) as u8
            } else {
                4 + (y * 24 / h) as u8 + if (x / 30 + y / 40) % 3 == 0 { 10 } else { 0 }
            };
            let b = if bright { v - 6 } else { v.saturating_add(4) };
            *p = slint::Rgba8Pixel::new(v, v, b, 255);
        }
        slint::Image::from_rgba8(px)
    };
    testing::drawing_backend();
    for theme in ["Charcoal", "Grey", "Light"] {
        for scale in [1.0f32, 2.0] {
            for current_bright in [false, true] {
                let app = testing::window(5);
                app.window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged {
                        scale_factor: scale,
                    });
                let (w, h) = (1200.0, 800.0);
                app.window().set_size(slint::PhysicalSize::new(
                    (w * scale) as u32,
                    (h * scale) as u32,
                ));
                app.window().dispatch_event(WindowEvent::Resized {
                    size: slint::LogicalSize::new(w, h),
                });
                app.global::<crate::Theme>().set_name(theme.into());
                let thumbs = (0..5)
                    .map(|i| Thumb {
                        name: format!("f{i:02}.CR3").into(),
                        // The current frame's picture bright or
                        // dark, the others alternating from it.
                        image: picture((i % 2 == 0) == current_bright),
                        chosen: (1..=3).contains(&i),
                        ..Default::default()
                    })
                    .collect();
                crate::cells::set_rows(&app, thumbs);
                // The strip's cells, which its report asks for once
                // a folder's state is there to answer it.
                crate::cells::show(&app, crate::cells::View::Strip, 0, 5);
                app.set_selected(2);
                i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
                let shot = app.window().take_snapshot().expect("a drawing backend");
                let full = image::RgbaImage::from_raw(
                    shot.width(),
                    shot.height(),
                    shot.as_bytes().to_vec(),
                )
                .unwrap();
                let (left, width) = crate::cells::strip_layout(&app)
                    .place(4)
                    .expect("five cells");
                let px = |v: f32| (v * scale).max(0.0).round() as u32;
                let top = h - app.get_strip_h();
                let part = image::imageops::crop_imm(
                    &full,
                    0,
                    px(top),
                    px(left + width + crate::strip::PAD),
                    px(app.get_strip_h()),
                )
                .to_image();
                let name = format!(
                    "{}-x{}-current-{}.png",
                    theme.to_lowercase(),
                    scale as u32,
                    if current_bright { "bright" } else { "dark" },
                );
                part.save(dir.join(&name)).unwrap();
                // The same five on the Library grid, to
                // `GREYCARD_GRID_CAPTURES` when it is set.
                let Some(grid_dir) = std::env::var_os("GREYCARD_GRID_CAPTURES") else {
                    continue;
                };
                let grid_dir = std::path::PathBuf::from(grid_dir);
                std::fs::create_dir_all(&grid_dir).unwrap();
                app.set_grid_open(true);
                crate::cells::show(&app, crate::cells::View::Grid, 0, 5);
                app.invoke_grid_range(0.0, 600.0, 5, 0.0);
                i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
                let cells: Vec<_> = i_slint_backend_testing::ElementHandle::find_by_element_id(
                    &app,
                    "GridSheet::ct",
                )
                .map(|c| testing::bounds_of(&c))
                .collect();
                assert_eq!(cells.len(), 5, "five cells on the sheet");
                let x0 = cells.iter().map(|b| b.0.x).fold(f32::MAX, f32::min) - 12.0;
                let y0 = cells.iter().map(|b| b.0.y).fold(f32::MAX, f32::min) - 12.0;
                let x1 = cells.iter().map(|b| b.0.x + b.1.width).fold(0.0, f32::max) + 12.0;
                let y1 = cells.iter().map(|b| b.0.y + b.1.height).fold(0.0, f32::max) + 12.0;
                let shot = app.window().take_snapshot().expect("a drawing backend");
                let full = image::RgbaImage::from_raw(
                    shot.width(),
                    shot.height(),
                    shot.as_bytes().to_vec(),
                )
                .unwrap();
                image::imageops::crop_imm(&full, px(x0), px(y0), px(x1 - x0), px(y1 - y0))
                    .to_image()
                    .save(grid_dir.join(&name))
                    .unwrap();
            }
        }
    }
}

/// What the session learned of a file by its path follows the file
/// when it moves, and goes when the file leaves the list: a frame
/// decoded as monochrome stays so at its new path, an archive's copy
/// followed stays followed, and nothing of it is left for another
/// file at the old one.
#[test]
fn what_is_learned_by_path_follows_a_move_and_goes_with_the_file() {
    let app = window(3);
    let files = crate::testing::folder(3);
    let (state, worker) = crate::testing::state_for(&app, files.clone());
    let to = PathBuf::from("/archive/shoot/IMG_0000.CR3");
    let stance = greycard_core::decode::Stance::default();
    {
        let mut st = state.borrow_mut();
        st.develop.monochromes.insert(files[0].clone());
        st.views.reframed.insert(files[0].clone());
        st.views.panel_stand_in = Some(files[0].clone());
        crate::sync::follow_for_test(
            &mut st,
            files[0].clone(),
            "/local/IMG_0000.CR3".into(),
            "/archive".into(),
        );
        st.move_file(0, to.clone());
        assert!(st.develop.monochromes.contains(&to));
        assert!(!st.develop.monochromes.contains(&files[0]));
        assert!(st.views.reframed.contains(&to));
        assert_eq!(st.views.panel_stand_in, Some(to.clone()));
        assert!(crate::sync::followed(&st, 0), "the copy is followed still");
        assert_eq!(
            crate::sync::local_of(&st, &to),
            Some(std::path::Path::new("/local/IMG_0000.CR3"))
        );
        assert!(crate::sync::local_of(&st, &files[0]).is_none());
        st.develop.monochromes.insert(files[1].clone());
        st.views.reframed.insert(files[1].clone());
        st.develop.stances.insert(files[1].clone(), stance);
        st.develop
            .mosaics
            .insert(files[1].clone(), OpenMosaic::NotMosaic);
    }
    crate::panel::cull::drop_files(
        &mut state.borrow_mut(),
        &app,
        &worker,
        &[0, 1],
        "gone".into(),
    );
    let st = state.borrow();
    assert!(st.develop.monochromes.is_empty());
    assert!(st.views.reframed.is_empty());
    assert!(st.develop.stances.is_empty());
    assert!(st.develop.mosaics.is_empty());
}

/// A file moved onto a path that something was learned of for a
/// file gone from there takes none of it: its stance, its mosaic,
/// and its monochrome flag were the other file's.
#[test]
fn a_file_moved_onto_a_gone_files_path_inherits_nothing() {
    let app = window(2);
    let files = crate::testing::folder(2);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let mut st = state.borrow_mut();
    let to = PathBuf::from("/nowhere/IMG_9999.CR3");
    st.develop
        .stances
        .insert(to.clone(), greycard_core::decode::Stance::default());
    st.develop.mosaics.insert(to.clone(), OpenMosaic::NotMosaic);
    st.develop.monochromes.insert(to.clone());
    st.views.reframed.insert(to.clone());
    st.move_file(0, to.clone());
    assert!(!st.develop.stances.contains_key(&to));
    assert!(!st.develop.mosaics.contains_key(&to));
    assert!(!st.develop.monochromes.contains(&to));
    assert!(!st.views.reframed.contains(&to));
}

/// A turn from the window writes no state (§724): four quarter turns of
/// a frame with a crop and a mask on it, its shape known, leave every
/// state as it was written, show the mask turned with the frame, and
/// show each state as written again once round. A frame with a mask that
/// nothing can measure is not turned at all, rather than leave its mask
/// a quarter turn off, and the status line says why.
#[test]
fn a_turn_from_the_window_changes_no_written_state_and_needs_the_shape() {
    use greycard_edit::mask::{Component, Mask, Shape};
    let app = window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let mut placed = greycard_edit::Edit::default();
    placed.geometry.crop = Some(greycard_edit::geometry::Crop::of([0.1, 0.2, 0.5, 0.6]));
    placed.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        mask: Mask {
            components: vec![Component {
                shape: Shape::Radial {
                    center: [0.3, 0.4],
                    radius: [0.1, 0.05],
                    angle: 10.0,
                    feather: 0.5,
                },
                ..Component::default()
            }],
            ..Mask::default()
        },
        ..Default::default()
    });
    for f in 0..2 {
        assert!(state.borrow_mut().sidecar_mut(f).record(placed.clone()));
    }
    // Frame 0's shape is known; nothing can measure frame 1, whose file
    // is not there.
    state.borrow_mut().sidecar_mut(0).frame_known(1.5);
    let written = |f: usize| state.borrow().sidecar(f).current_written().into_owned();
    let before = [written(0), written(1)];
    for k in 1..=4u8 {
        turn_frames(&mut state.borrow_mut(), &app, &worker, &[0, 1], 1);
        for (f, was) in before.iter().enumerate() {
            assert_eq!(written(f), *was, "frame {f}, turn {k}");
        }
        let st = state.borrow();
        // Turned, its mask with it by the shape.
        assert_eq!(st.sidecar(0).turn, k % 4, "turn {k}");
        assert!(!st.sidecar(0).needs_frame());
        assert_eq!(
            st.sidecar(0).current().adjustments == placed.adjustments,
            k % 4 == 0,
            "turn {k}"
        );
        // Not turned at all: a mask a quarter turn off is worse.
        assert_eq!(st.sidecar(1).turn, 0, "turn {k}");
        assert_eq!(*st.sidecar(1).current(), placed, "turn {k}");
        assert_eq!(
            app.get_status().as_str(),
            "Read-only: IMG_0001.CR3's masks cannot be placed until its raw is reachable",
            "turn {k}"
        );
    }
    for f in 0..2 {
        assert_eq!(*state.borrow().sidecar(f).current(), placed);
    }
}

/// A frame with a mask on it whose XMP, from another program, says it
/// is turned: the window's read makes the turn by the frame's header, as
/// the library's pass does, so the cell stands as its row does and does
/// not flip when the sidecar loads (§724). A sidecar read with the turn
/// owed (by a reader that measured nothing) has it made as soon as the
/// window measures the frame.
#[test]
fn an_xmp_turn_on_a_frame_with_a_mask_is_made_in_the_window() {
    use greycard_core::raw::Orientation;
    use greycard_edit::mask::{Component, Mask, Shape};
    let dir = std::env::temp_dir().join(format!("greycard-xmp-window-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("IMG_0000.tif");
    greycard_library::fixture::write_frame_of(
        &raw,
        &greycard_library::fixture::R5,
        6,
        4,
        &[0; 6 * 4 * 3],
    );
    let mut placed = Edit::default();
    placed.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        mask: Mask {
            components: vec![Component {
                shape: Shape::Radial {
                    center: [0.3, 0.4],
                    radius: [0.1, 0.05],
                    angle: 10.0,
                    feather: 0.5,
                },
                ..Component::default()
            }],
            ..Mask::default()
        },
        ..Default::default()
    });
    let mut s = Sidecar::default();
    assert!(s.record(placed));
    s.save(&raw).unwrap();
    std::fs::write(
        xmp::long_path(&raw),
        xmp::fresh(
            &Meta::default(),
            Some(xmp::Turn::new(Orientation::Rotate90, 0)),
        ),
    )
    .unwrap();
    let mut lib = greycard_library::Library::open_in_memory().unwrap();
    lib.index_folder(&dir, &mut |_| {}).unwrap();
    let row = lib.by_path(&raw).unwrap().unwrap().turns;
    assert_ne!(row, 0, "the library's row turns it");
    // The window's own read, on its thread: the cell as the row has it.
    let held = crate::rows::from_disk(
        &raw,
        Some(greycard_edit::Placement::Beside),
        Default::default(),
    );
    let shown =
        |s: &Sidecar| greycard_library::pack_turns(s.current().geometry.shown_turns(s.turn));
    assert_eq!(shown(&held.sidecar), row);
    assert_eq!(held.sidecar.turn_owed, None);
    let turn = held.sidecar.turn;
    assert_ne!(turn, 0);
    // Read by one that measured nothing: owed, then made by the window.
    let owed = greycard_library::held::from_disk(
        &raw,
        Some(greycard_edit::Placement::Beside),
        Default::default(),
        |_, _| None,
    )
    .sidecar;
    assert_eq!((owed.turn, owed.turn_owed), (0, Some(turn)));
    let app = window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![raw.clone()]);
    state.borrow_mut().sidecars[0] = owed;
    migrate_frame(&mut state.borrow_mut(), 0);
    {
        let st = state.borrow();
        assert_eq!(
            (st.sidecars[0].turn, st.sidecars[0].turn_owed),
            (turn, None)
        );
        assert_eq!(shown(&st.sidecars[0]), row, "the cell does not flip");
        assert_eq!(st.sidecars[0].current(), held.sidecar.current());
    }
    drop(lib);
    crate::testing::remove_scratch(state, &dir);
}

/// A selection turned with the open frame in it and a frame nothing can
/// measure: the open frame develops turned, and the status line keeps
/// the words for the one not turned, over "developing..." and through
/// the develop's landing (§724).
#[test]
fn a_refused_turn_in_a_selection_keeps_its_words_over_the_develop() {
    use greycard_edit::mask::{Component, Mask, Shape};
    let app = window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let mut placed = Edit::default();
    placed.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        mask: Mask {
            components: vec![Component {
                shape: Shape::Radial {
                    center: [0.3, 0.4],
                    radius: [0.1, 0.05],
                    angle: 10.0,
                    feather: 0.5,
                },
                ..Component::default()
            }],
            ..Mask::default()
        },
        ..Default::default()
    });
    assert!(state.borrow_mut().sidecar_mut(1).record(placed));
    app.invoke_select(0);
    turn_frames(&mut state.borrow_mut(), &app, &worker, &[0, 1], 1);
    let words = "Read-only: IMG_0001.CR3's masks cannot be placed until its raw is reachable";
    assert_eq!(state.borrow().sidecar(0).turn, 1, "the open frame turned");
    assert_eq!(state.borrow().sidecar(1).turn, 0, "the other not");
    assert_eq!(app.get_status().as_str(), words);
    let landed = crate::testing::land_develop(
        &app,
        &state,
        crate::worker::LearnedReport::Off,
        crate::worker::FillReport::default(),
    );
    assert_eq!(landed, words);
}

/// The open frame with a mask on it, its XMP's turn owed (its header
/// would not read when it opened), and its develop landed: the window,
/// the frame's raw, and the edit placed on it.
fn owed_under_the_panel(name: &str) -> (App, Rc<crate::Shared>, Rc<Worker>, PathBuf, Edit) {
    use greycard_edit::mask::{Component, Mask, Shape};
    let dir = crate::testing::scratch_dir(name);
    let raw = dir.join("IMG_0000.CR3");
    std::fs::write(&raw, b"not a raw").unwrap();
    let mut placed = Edit::default();
    placed.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        mask: Mask {
            components: vec![Component {
                shape: Shape::Radial {
                    center: [0.3, 0.4],
                    radius: [0.1, 0.05],
                    angle: 10.0,
                    feather: 0.5,
                },
                ..Component::default()
            }],
            ..Mask::default()
        },
        ..Default::default()
    });
    let app = window(1);
    let (state, worker) = crate::testing::state_for(&app, vec![raw.clone()]);
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        assert!(st.sidecar_mut(0).record(placed.clone()));
        st.sidecar_mut(0).turn_owed = Some(1);
    }
    app.invoke_select(0);
    let mut st = state.borrow_mut();
    assert_eq!(st.sidecar(0).turn_owed, Some(1), "nothing measured it");
    // Its develop lands: the open frame is measured by it.
    st.develop.source_size = (60, 40);
    drop(st);
    (app, state, worker, raw, placed)
}

/// Turn pressed on the open frame whose XMP's turn is owed: the frame
/// ends where the XMP's turn and the press say together, nothing is
/// recorded, the mask is carried round by both, and the file says so.
/// Before, the owed turn was made under the panel and the panel's edit,
/// still shown under the turn before, was then recorded a quarter turn
/// off (§724).
#[test]
fn turn_on_an_open_frame_with_a_turn_owed_makes_both_and_records_nothing() {
    let (app, state, worker, raw, placed) = owed_under_the_panel("owed-under-panel-turn");
    let steps = state.borrow().line(0).unwrap().history.len();
    turn_frames(&mut state.borrow_mut(), &app, &worker, &[0], 1);
    let mut expected = Sidecar::default();
    assert!(expected.record(placed));
    expected.frame_known(1.5);
    expected.turn_by(2, Some(1.5)).unwrap();
    {
        let st = state.borrow();
        let s = st.sidecar(0);
        assert_eq!((s.turn, s.turn_owed), (2, None));
        assert_eq!(s.history.len(), steps, "a turn records nothing");
        assert_eq!(s.current(), expected.current(), "the mask carried round");
        assert_eq!(st.develop.edit, *expected.current(), "the panel's too");
        assert_eq!(st.develop.source_size, (60, 40), "two quarters");
    }
    let file = Sidecar::load_as_file(&raw).unwrap().unwrap();
    assert_eq!((file.turn, file.turn_owed), (2, None));
    assert_eq!(file.history.len(), steps);
    let dir = raw.parent().unwrap().to_path_buf();
    crate::testing::remove_scratch(state, &dir);
}

/// An export pressed over that open frame takes the panel's edit with
/// the turn the panel shows it under: the owed turn is not made under
/// the panel, so the pair is the picture on screen, and nothing is
/// recorded (§724).
#[test]
fn an_export_over_an_open_frame_with_a_turn_owed_takes_the_picture_shown() {
    let (app, state, _worker, raw, placed) = owed_under_the_panel("owed-under-panel-export");
    let steps = state.borrow().line(0).unwrap().history.len();
    let frames = crate::panel::deliver::set_frames_of(&mut state.borrow_mut(), &app, &[0]);
    assert_eq!(frames.len(), 1);
    let (_, edit, turn, _, _) = &frames[0];
    assert_eq!((edit, *turn), (&placed, 0), "the picture on screen");
    let st = state.borrow();
    assert_eq!((st.sidecar(0).turn, st.sidecar(0).turn_owed), (0, Some(1)));
    assert_eq!(st.line(0).unwrap().history.len(), steps);
    drop(st);
    let dir = raw.parent().unwrap().to_path_buf();
    crate::testing::remove_scratch(state, &dir);
}

/// Frames read together for a press, on the pool while the window waits,
/// are measured by the window's mapped header read, never the unmapped
/// one that reads a raw whole (§724).
#[test]
fn frames_read_while_the_window_waits_are_measured_by_the_mapped_header() {
    let dir = crate::testing::scratch_dir("load-frames-header");
    let files: Vec<PathBuf> = (0..2)
        .map(|i| dir.join(format!("IMG_{i:04}.tif")))
        .collect();
    for f in &files {
        greycard_library::fixture::write_frame_of(
            f,
            &greycard_library::fixture::R5,
            6,
            4,
            &[0; 6 * 4 * 3],
        );
        crate::testing::waiting_sidecar().save(f).unwrap();
    }
    let app = window(2);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let mut st = state.borrow_mut();
    st.window.write_sidecars = true;
    st.from_row = (0..2)
        .map(|_| crate::rows::FromRow {
            read: false,
            ..crate::rows::FromRow::read()
        })
        .collect();
    let kept = crate::rows::load_frames(&mut st, &app, &[0, 1]);
    assert_eq!(kept, [0, 1]);
    let asked = HEADER_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    for (c, f) in files.iter().enumerate() {
        assert!(asked.contains_key(f), "{}", f.display());
        assert!(!st.sidecar(c).current_waits(), "measured");
    }
    drop(st);
    crate::testing::remove_scratch(state, &dir);
}
