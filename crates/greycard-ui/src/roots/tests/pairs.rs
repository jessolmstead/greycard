//! Tests of RAW+JPEG pairs in the list: the camera's JPEG beside its
//! raw marked on the raw's tile and named under its name, and the
//! setting that lists it apart.

use super::*;

/// Each row's name and whether its tile carries the pair's mark.
fn marks(app: &App) -> Vec<(String, bool)> {
    let model = app.get_thumbs();
    (0..model.row_count())
        .map(|r| {
            let t = model.row_data(r).unwrap();
            (t.name.to_string(), t.paired)
        })
        .collect()
}

/// A folder of pairs opens with each raw's JPEG its companion: not in
/// the list, a mark on the raw's tile and a line under its name. The
/// setting flipped lists the JPEGs as frames of their own and takes the
/// marks away; flipped back, the JPEGs leave the list again.
#[test]
fn a_folders_pairs_are_one_frame_each_until_listed_apart() {
    let dir = scratch("pairs");
    for name in ["A.CR3", "A.JPG", "B.JPG", "C.CR3", "c.jpeg"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Folder(dir.clone()),
        Select::Row(0),
    );
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(
            st.files,
            [dir.join("A.CR3"), dir.join("B.JPG"), dir.join("C.CR3")]
        );
        assert_eq!(
            st.views.companions.get(&dir.join("A.CR3")),
            Some(&vec![dir.join("A.JPG")])
        );
        assert_eq!(
            st.views.companions.get(&dir.join("C.CR3")),
            Some(&vec![dir.join("c.jpeg")])
        );
    }
    let row = |name: &str, paired: bool| (name.to_string(), paired);
    assert_eq!(
        marks(&app),
        [row("A.CR3", true), row("B.JPG", false), row("C.CR3", true)]
    );
    // The line under the name of the frame on screen.
    crate::panel::browser::show_file_name(&state.borrow(), &app, Some(&dir.join("A.CR3")));
    assert_eq!(app.get_file_companion(), "With the camera's JPEG, A.JPG");
    crate::panel::browser::show_file_name(&state.borrow(), &app, Some(&dir.join("B.JPG")));
    assert_eq!(app.get_file_companion(), "");

    // Apart: the JPEGs are frames, and nothing is marked.
    crate::panel::prefs::jpegs_apart_changed(&state, &app, &worker, true);
    land_all(&state, &app, &worker);
    {
        let st = state.borrow();
        assert!(st.views.jpegs_apart);
        assert_eq!(st.files.len(), 5, "{:?}", st.files);
        assert!(st.views.companions.is_empty());
    }
    assert!(
        marks(&app).iter().all(|(_, paired)| !paired),
        "{:?}",
        marks(&app)
    );

    // And back.
    crate::panel::prefs::jpegs_apart_changed(&state, &app, &worker, false);
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files.len(), 3);
    assert_eq!(
        marks(&app),
        [row("A.CR3", true), row("B.JPG", false), row("C.CR3", true)]
    );
    crate::testing::remove_scratch(state, &dir);
}

/// A JPEG that comes beside a listed raw after the folder opened (the
/// second file of a pair being copied in) changes no frame: the next
/// read of the list marks the raw, and the JPEG stays out of it.
#[test]
fn a_jpeg_arriving_beside_a_listed_raw_marks_it_and_stays_out() {
    let dir = scratch("pair-late");
    for name in ["A.CR3", "B.CR3"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Folder(dir.clone()),
        Select::Row(0),
    );
    land_all(&state, &app, &worker);
    assert!(marks(&app).iter().all(|(_, paired)| !paired));
    std::fs::write(dir.join("B.JPG"), b"x").unwrap();
    assert!(refresh_view(&state, &app, &worker));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, [dir.join("A.CR3"), dir.join("B.CR3")]);
    assert_eq!(
        marks(&app),
        [("A.CR3".to_string(), false), ("B.CR3".to_string(), true)]
    );
    crate::testing::remove_scratch(state, &dir);
}

/// A list holding a raw and its JPEG both as frames (a collection the
/// JPEG was put in while the JPEGs were listed apart) marks neither:
/// the JPEG is a frame of its own there, and nothing of the raw's takes
/// it.
#[test]
fn a_jpeg_listed_as_a_frame_is_no_companion_of_its_raw() {
    let dir = scratch("pair-both");
    for name in ["A.CR3", "A.JPG", "B.CR3", "B.JPG"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(vec![
            dir.join("A.CR3"),
            dir.join("A.JPG"),
            dir.join("B.CR3"),
        ]),
        Select::Row(0),
    );
    land_all(&state, &app, &worker);
    {
        let st = state.borrow();
        assert!(!st.views.companions.contains_key(&dir.join("A.CR3")));
        assert_eq!(
            st.views.companions.get(&dir.join("B.CR3")),
            Some(&vec![dir.join("B.JPG")])
        );
    }
    assert_eq!(
        marks(&app),
        [
            ("A.CR3".to_string(), false),
            ("A.JPG".to_string(), false),
            ("B.CR3".to_string(), true)
        ]
    );
    crate::testing::remove_scratch(state, &dir);
}
