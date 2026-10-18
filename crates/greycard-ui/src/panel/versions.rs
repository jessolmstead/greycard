//! The makers of versions on the window: New version (Ctrl+', the
//! frame menu), New version from the original, Rename version..., Swap
//! with the original and Delete version... Each changes the file's one
//! sidecar through the edit crate's makers, writes it once through the
//! usual save, and puts the cells in step with it (`versions::settle`):
//! a version is a cell of its own right after its file's.

use crate::panel::browser::{file_name, open_row, row_of, show_badges, show_thumb, thumb_turns};
use crate::panel::edit::{read_edit, save_edit, write_file};
use crate::versions::Key;
use crate::*;

/// The cell the menu's items act on: the cell right-clicked, from the
/// menu, else the cell on screen.
fn cell_for(st: &State, from_menu: bool) -> Option<usize> {
    if from_menu {
        st.views.menu_file.or(st.current)
    } else {
        st.current
    }
}

/// The panel's edit made a state of the frame on screen first, so what
/// a maker copies or swaps is what the panel shows.
fn settle_panel(st: &mut State, app: &App) {
    if st.culling.cull.is_none() && st.current.is_some() {
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        save_edit(st, edit);
    }
}

/// Why no version can be made of cell `c` now, for the status line:
/// culling (the panel is a stand-in there), a delete under way, or a
/// sidecar that is not in hand. None when it can.
fn refused(st: &mut State, app: &App, c: usize) -> Option<String> {
    if st.culling.cull.is_some() {
        return Some("a version is made from the editor, not while culling".into());
    }
    unready(st, app, c)
}

/// [`refused`] less culling: a version's delete touches no panel, and
/// under compare it is how the loser goes.
fn unready(st: &mut State, app: &App, c: usize) -> Option<String> {
    if crate::panel::delete::held(st, c) {
        return Some("a delete is under way".into());
    }
    if !crate::rows::load_frame(st, app, c) {
        return Some(
            crate::rows::refused(st, c, "no version is made")
                .unwrap_or_else(|| "no version is made".into()),
        );
    }
    None
}

/// New version of cell `c`'s frame (the frame on screen, or the one
/// the menu was opened on): its edit copied as the new line's one
/// state, or with `from_original` the frame's default edit. The sidecar
/// is written once, the new cell comes in after its file's others, it
/// is opened, and its name is asked for with "Version N" in the field,
/// all of it chosen, so typing names it and Escape keeps the name it
/// was made with.
pub(crate) fn new_version(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    from_original: bool,
    from_menu: bool,
) {
    let mut st = state.borrow_mut();
    let Some(c) = cell_for(&st, from_menu) else {
        app.set_status("no frame to make a version of".into());
        return;
    };
    if let Some(why) = refused(&mut st, app, c) {
        app.set_status(why.into());
        return;
    }
    if Some(c) == st.current {
        settle_panel(&mut st, app);
    }
    let file = st.file_of(c);
    let path = st.files[file].clone();
    let picture = greycard_core::picture::is_picture_path(&path);
    let id = if from_original {
        let default = if picture {
            Edit::for_picture()
        } else {
            Edit::default()
        };
        Some(st.sidecars[file].new_version_from_original(default, None))
    } else {
        let from = st.version_of(c).map(str::to_string);
        st.sidecars[file].new_version(from.as_deref(), None)
    };
    let Some(id) = id else {
        app.set_status("the version it was to be made from is gone".into());
        return;
    };
    let name = st.sidecars[file]
        .version(&id)
        .map(|v| v.name.clone())
        .unwrap_or_default();
    write_file(&mut st, file);
    // Made in a collection's view: it goes into the collection beside
    // the frame it came from, or its cell would have no place there.
    crate::panel::collections::version_made(&mut st, app, c, &id);
    crate::versions::settle(&mut st, app, worker);
    let Some(at) = st.cell_of(file, Some(&id)) else {
        return;
    };
    tracing::info!(
        "{}: {name} made{}",
        file_name(&path),
        if from_original {
            " from the defaults"
        } else {
            ""
        }
    );
    // The new cell, opened as a click on it would: the frame left is
    // saved on the way, as any click saves it.
    if let Some(row) = row_of(&st, at) {
        open_row(&mut st, app, worker, row as i32, false);
    }
    app.set_status(format!("made {name} of {}", file_name(&path)).into());
    ask_name(&mut st, app, &path, &id, &name, true);
}

/// The name sheet over version `id` of `path`: the collections' sheet,
/// its field holding `name`, all of it chosen.
fn ask_name(st: &mut State, app: &App, path: &Path, id: &str, name: &str, fresh: bool) {
    st.views.collections.asked = Some(crate::panel::collections::Asked::Version {
        path: path.to_path_buf(),
        id: id.to_string(),
    });
    app.set_collection_sheet_title(
        if fresh {
            "Name the new version"
        } else {
            "Rename the version"
        }
        .into(),
    );
    app.set_collection_sheet_note(
        format!(
            "A version of {}, with an edit of its own; the rating, flag, label and keywords are the file's",
            file_name(path)
        )
        .into(),
    );
    app.set_collection_sheet_asks_name(true);
    app.set_collection_sheet_verb(if fresh { "Name" } else { "Rename" }.into());
    app.set_collection_sheet_placeholder("The version's name".into());
    app.set_collection_sheet_name(name.into());
    app.set_collection_sheet_open(true);
}

/// Rename version... on the cell the menu was opened on.
pub(crate) fn ask_rename(st: &mut State, app: &App, from_menu: bool) {
    let Some(c) = cell_for(st, from_menu) else {
        return;
    };
    let (Some(id), Some(name)) = (st.version_of(c).map(str::to_string), st.version_name(c)) else {
        app.set_status("the file's own frame is renamed as a file, not as a version".into());
        return;
    };
    let path = st.path_of(c).clone();
    ask_name(st, app, &path, &id, &name, false);
}

/// The name sheet answered for version `id` of `path` with `typed`: the
/// version called that, written, and its cell's tag and label with it.
/// A blank name, or the name it has, changes nothing.
pub(crate) fn renamed(st: &mut State, app: &App, path: &Path, id: &str, typed: &str) {
    let Some(c) = st.find_cell(&Key::of(path, Some(id))) else {
        app.set_status("that version is gone; nothing renamed".into());
        return;
    };
    if let Some(why) = refused(st, app, c) {
        app.set_status(why.into());
        return;
    }
    let file = st.file_of(c);
    let was = st.sidecars[file]
        .version(id)
        .map(|v| v.name.clone())
        .unwrap_or_default();
    if !st.sidecars[file].rename_version(id, typed) {
        return;
    }
    let now = typed.trim().to_string();
    write_file(st, file);
    show_names(st, app, file);
    tracing::info!("{}: {was} renamed {now}", file_name(path));
    app.set_status(format!("{was} of {} is {now} now", file_name(path)).into());
}

/// The version names on the cells of file `file`, as the sidecar says
/// them now, and the history's head when the frame on screen is one of
/// them.
pub(crate) fn show_names(st: &State, app: &App, file: usize) {
    let model = app.get_thumbs();
    for c in st.cells_of_file(file) {
        let Some(row) = row_of(st, c) else {
            continue;
        };
        let name: slint::SharedString = st.version_name(c).unwrap_or_default().into();
        if let Some(mut t) = model.row_data(row)
            && t.version != name
        {
            t.version = name;
            model.set_row_data(row, t);
        }
    }
    if st.current.is_some_and(|c| st.file_at(c) == Some(file)) {
        crate::panel::history::show_history(st, app);
    }
}

/// Swap with the original on the version the menu was opened on: the
/// two lines exchanged whole, the version keeping its id and its name.
/// The two cells trade pictures at once, and the frame on screen, when
/// it is one of them, shows its line as it is now.
pub(crate) fn swap(st: &mut State, app: &App, worker: &Worker, from_menu: bool) {
    let Some(c) = cell_for(st, from_menu) else {
        return;
    };
    let Some(id) = st.version_of(c).map(str::to_string) else {
        app.set_status("the original is swapped from one of its versions".into());
        return;
    };
    if let Some(why) = refused(st, app, c) {
        app.set_status(why.into());
        return;
    }
    let file = st.file_of(c);
    let on_screen = st.current.is_some_and(|x| st.file_at(x) == Some(file));
    if on_screen {
        settle_panel(st, app);
    }
    let path = st.files[file].clone();
    // A collection may list the version and not the original.
    let own = st.own_cell(file);
    if !st.sidecars[file].swap_with_original(&id) {
        // The read the load brought had no such version: deleted
        // elsewhere, and the cells follow at the next turn.
        app.set_status(
            format!(
                "that version of {} is gone; nothing swapped",
                file_name(&path)
            )
            .into(),
        );
        return;
    }
    let name = st.version_name(c).unwrap_or_default();
    write_file(st, file);
    // The pictures trade places now; each is asked for again after,
    // from its line as it now is.
    match own {
        Some(own) => {
            st.views.thumb_base.swap(own, c);
            st.views.thumb_made.swap(own, c);
            st.views.thumb_failed.swap(own, c);
        }
        // The original's picture is not in the list to trade: the
        // version's is made again.
        None => {
            st.views.thumb_base[c] = None;
            st.views.thumb_made[c] = 0;
            st.views.thumb_failed[c] = false;
        }
    }
    for g in own.into_iter().chain([c]) {
        let (turns, flip) = thumb_turns(st, app, g);
        show_thumb(st, app, g, turns, flip);
        show_badges(st, app, g);
        crate::edited::refresh(st, g);
    }
    if let Some(x) = st.current.filter(|_| on_screen) {
        // The panel's line is another now: opened afresh on it, with
        // nothing of the panel saved over the line it held.
        st.current = None;
        if let Some(row) = row_of(st, x) {
            open_row(st, app, worker, row as i32, false);
        }
    }
    tracing::info!("{}: {name} swapped with the original", file_name(&path));
    app.set_status(format!("{name} and the original of {} swapped", file_name(&path)).into());
}

/// What the delete sheet asks over versions while it is up: the frames,
/// by file and version id.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    pub(crate) frames: Vec<Key>,
}

/// The delete sheet over `cells`, every one of them a version: in the
/// version's words for one ("Delete the version Warm of IMG_0001.CR3?"),
/// counted for more. Answered by [`answered`]. A cell that is a file's
/// own is left out.
pub(crate) fn ask_delete(st: &mut State, app: &App, cells: &[usize]) {
    let keys: Vec<Key> = cells
        .iter()
        .filter(|&&c| st.version_of(c).is_some())
        .filter_map(|&c| st.key_of(c))
        .collect();
    let Some(first) = cells.iter().find(|&&c| st.version_of(c).is_some()) else {
        app.set_status("no version is chosen".into());
        return;
    };
    let title = match keys.len() {
        1 => format!(
            "Delete the version {} of {}?",
            st.version_name(*first).unwrap_or_default(),
            file_name(st.path_of(*first))
        ),
        n => format!("Delete {n} versions?"),
    };
    let text = if keys.len() == 1 {
        "The file and its other versions stay."
    } else {
        "The files and their other versions stay."
    };
    app.set_delete_title(title.into());
    app.set_delete_text(text.into());
    app.set_delete_note("Its edit, history and snapshots go with it.".into());
    app.set_delete_archive("".into());
    app.set_delete_trash(true);
    app.set_delete_permanent(false);
    app.set_delete_confirm(
        if keys.len() == 1 {
            "Delete version"
        } else {
            "Delete versions"
        }
        .into(),
    );
    st.sheets.version_delete = Some(Asked { frames: keys });
    app.set_delete_open(true);
}

/// The sheet's answer over versions: 1 deletes them, anything else
/// leaves them. Each file's sidecar is written once, whatever number of
/// its versions went, and the cells follow.
pub(crate) fn answered(st: &mut State, app: &App, worker: &Worker, answer: i32) {
    app.set_delete_open(false);
    app.set_delete_confirm("".into());
    let Some(asked) = st.sheets.version_delete.take() else {
        return;
    };
    if answer != 1 {
        return;
    }
    // The panel's edit a state first, as a delete of the file's makes
    // it: what the frame on screen holds is never lost to a delete of
    // another line of it.
    settle_panel(st, app);
    let mut gone = Vec::new();
    let mut files: Vec<usize> = Vec::new();
    for key in &asked.frames {
        let Some(id) = key.version() else {
            continue;
        };
        let Some(c) = st.find_cell(key) else {
            continue;
        };
        if let Some(why) = unready(st, app, c) {
            app.set_status(why.into());
            continue;
        }
        let file = st.file_of(c);
        if let Some(taken) = st.sidecars[file].delete_version(id) {
            // Gone from its sidecar, the version leaves its stack.
            crate::stacks::version_left(st, file, id);
            gone.push(format!("{} of {}", taken.name, file_name(&key.path)));
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    // Each file written once; the cells then drop the versions taken.
    for &file in &files {
        write_file(st, file);
    }
    crate::versions::settle(st, app, worker);
    let said = match gone.as_slice() {
        [] => "nothing deleted".to_string(),
        [one] => format!("deleted the version {one}"),
        many => format!("deleted {} versions", many.len()),
    };
    tracing::info!("{said}");
    app.set_status(said.into());
}

/// The most cells compare shows at once, the compare grid's most.
const COMPARE_MOST: usize = 4;

/// The cells Compare versions shows for file `file`, in the list's
/// order wherever they stand (in a folder the file's own first and its
/// versions after it; in a collection as it lists them, perhaps apart),
/// and how many of its cells were left out: all of them where they are
/// two, three or four, else the first four. Only cells the browser
/// lists count.
pub(crate) fn compare_set(st: &State, file: usize) -> (Vec<usize>, usize) {
    let all: Vec<usize> = st
        .cells_of_file(file)
        .into_iter()
        .filter(|&c| row_of(st, c).is_some())
        .collect();
    let left = all.len().saturating_sub(COMPARE_MOST);
    let set = all.into_iter().take(COMPARE_MOST).collect();
    (set, left)
}

/// Compare versions on the cell the menu was opened on (or the frame
/// on screen): culling with the file and its versions alone side by
/// side, each its own picture, two, three or four up by how many they
/// are (`fit_compare`). The focus is the cell right-clicked when it is
/// in the set, else the file's own, and the selection collapses to it,
/// so Delete there is that one frame's delete: a version's, by its
/// sheet. A file with more than three versions shows its first three
/// and the status line says how many were left out.
pub(crate) fn compare(st: &mut State, app: &App, worker: &Worker, from_menu: bool) {
    let Some(c) = cell_for(st, from_menu) else {
        app.set_status("no frame to compare".into());
        return;
    };
    let Some(file) = st.file_at(c) else {
        return;
    };
    let path = st.files[file].clone();
    let name = file_name(&path);
    let (set, left) = compare_set(st, file);
    if set.len() < 2 {
        app.set_status(format!("{name} has no versions to compare").into());
        return;
    }
    let focus = if set.contains(&c) { c } else { set[0] };
    if st.culling.cull.is_none() {
        // At one up; the set's own count is put on it below.
        crate::panel::cull::enter_cull(st, app, 1);
    }
    if st.current != Some(focus) {
        if let Some(row) = row_of(st, focus) {
            open_row(st, app, worker, row as i32, false);
        }
    } else {
        st.views.picked = vec![focus];
        crate::panel::browser::show_set(st, app);
    }
    fit_compare(st, app, &path, None);
    let said = if left > 0 {
        format!(
            "comparing {name} and {} of its versions; {}",
            set.len() - 1,
            left_out(left, &name)
        )
    } else {
        format!("comparing {name} and its versions")
    };
    tracing::info!("{said}");
    app.set_status(said.into());
}

/// The status line's word for `left` versions of `name` that compare
/// did not show.
fn left_out(left: usize, name: &str) -> String {
    format!(
        "{left} more version{} of {name} left out",
        if left == 1 { "" } else { "s" }
    )
}

/// The culling view fitted to the file at `path` and its versions as
/// the cells are now: as many tiles as there are of them, four at most,
/// and the tiles those cells by name (`Cull::set`), wherever they stand
/// in the list, so the view is those cells and no other frame's. With
/// the versions gone (a delete under compare took the last) it is one
/// up on the file. What the view is fitted to is kept
/// (`Cull::versions_of`), so a later change to the list fits it again.
/// `was` is the count the view had before a relist put it down, for a
/// refit; the zoom stands when the count comes back the same.
pub(crate) fn fit_compare(st: &mut State, app: &App, path: &Path, was: Option<usize>) {
    // The file gone from the list, or every cell of it filtered out:
    // the view is no longer its compare.
    let first = st.file_by_path(path).and_then(|file| {
        let (set, left) = compare_set(st, file);
        let first = row_of(st, *set.first()?)?;
        Some((set, left, first))
    });
    let Some((set, left, first)) = first else {
        if let Some(cull) = st.culling.cull.as_mut() {
            cull.leave_versions(app);
        }
        // The set placed again about the frame on screen, as an arrow
        // places it.
        if let Some(c) = st.current {
            crate::panel::cull::cull_select(st, app, c);
        }
        return;
    };
    // The tiles are the set's cells themselves, not a run of rows from
    // an anchor: in a collection they may stand apart. The anchor is the
    // first of them, where V's views start from if the compare is left.
    let up = set.len().max(1);
    let note = (left > 0).then(|| left_out(left, &file_name(path)));
    let Some(cull) = st.culling.cull.as_mut() else {
        return;
    };
    let changed = was.unwrap_or(cull.compare) != up;
    cull.compare = up;
    cull.anchor = first;
    cull.note = note;
    cull.versions_of = (up > 1).then(|| path.to_path_buf());
    cull.stack_of = None;
    cull.set = (up > 1).then_some(set);
    cull.status.clear();
    app.set_compare(up as i32);
    app.set_compare_text(up.to_string().into());
    if changed {
        // The tiles are another size: fitted afresh.
        st.develop.zoom = 0.0;
        st.develop.image_size = (0, 0);
    }
    crate::panel::cull::cull_refresh(st);
    app.window().request_redraw();
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_version_new_asked(move |from_original, from_menu| {
            if let Some(app) = app_weak.upgrade() {
                new_version(&state, &app, &worker, from_original, from_menu);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_version_rename_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                ask_rename(&mut state.borrow_mut(), &app, true);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_version_swap_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                swap(&mut state.borrow_mut(), &app, &worker, true);
            }
        });
    }
    {
        // The menu's Delete version: the cell right-clicked, whatever the
        // selection holds beside it.
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_version_delete_asked(move || {
            if let Some(app) = app_weak.upgrade() {
                let mut st = state.borrow_mut();
                if st.sheets.deleting.is_some() {
                    app.set_status("a delete is still under way".into());
                    return;
                }
                let Some(c) = cell_for(&st, true) else {
                    return;
                };
                ask_delete(&mut st, &app, &[c]);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_version_compare_asked(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let has_versions = {
                let st = state.borrow();
                cell_for(&st, true)
                    .and_then(|c| st.file_at(c))
                    .is_some_and(|f| compare_set(&st, f).0.len() > 1)
            };
            if has_versions {
                // The loupe is where compare shows: out of the lightbox
                // and the grid first, before the state is held, since
                // both answer the window's own callbacks.
                if app.get_lightbox() {
                    app.invoke_lightbox_leave();
                }
                app.set_grid_open(false);
            }
            compare(&mut state.borrow_mut(), &app, &worker, true);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{folder, state_for, window};
    use slint::Model;

    /// A version named `name` of file `file`, made in its sidecar as the
    /// edit crate makes one, and the cells put in step: its id.
    fn version_of(
        state: &Rc<RefCell<State>>,
        app: &App,
        worker: &Worker,
        file: usize,
        name: &str,
    ) -> String {
        let mut st = state.borrow_mut();
        let id = st.sidecars[file].new_version(None, Some(name)).unwrap();
        crate::versions::settle(&mut st, app, worker);
        id
    }

    /// What the strip and the grid list: each row's file name and the
    /// version's tag on it.
    fn rows(app: &App) -> Vec<(String, String)> {
        let thumbs = app.get_thumbs();
        (0..thumbs.row_count())
            .map(|r| {
                let t = thumbs.row_data(r).unwrap();
                (t.name.to_string(), t.version.to_string())
            })
            .collect()
    }

    fn save(app: &App, state: &Rc<RefCell<State>>) {
        let mut st = state.borrow_mut();
        let edit = read_edit(app, &st.develop.edit, st.develop.target);
        save_edit(&mut st, edit);
    }

    /// Each cell as (file, version id).
    fn cells(st: &State) -> Vec<(usize, Option<String>)> {
        st.cells
            .iter()
            .map(|c| (c.file, c.version.as_deref().map(str::to_string)))
            .collect()
    }

    /// A folder of `n` frames on disk, nothing written yet.
    fn disk_folder(what: &str, n: usize) -> (PathBuf, Vec<PathBuf>) {
        let dir = crate::testing::scratch_dir(what);
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..n)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        (dir, files)
    }

    /// A version is a cell of its own right after its file's, wearing
    /// its name and the file's name under it; the arrows walk onto it
    /// and past it as onto any frame, and the file's facts are kept
    /// once.
    #[test]
    fn a_version_cell_comes_after_its_file_and_the_arrows_walk_it() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        let id = version_of(&state, &app, &worker, 1, "Warm");
        {
            let st = state.borrow();
            assert_eq!(st.files.len(), 3, "one path a file");
            assert_eq!(st.sidecars.len(), 3, "one sidecar a file");
            assert_eq!(
                cells(&st),
                [(0, None), (1, None), (1, Some(id.clone())), (2, None)]
            );
            assert_eq!(st.version_name(2).as_deref(), Some("Warm"));
            assert_eq!(st.views.shown, [0, 1, 2, 3]);
        }
        assert_eq!(
            rows(&app),
            [
                ("IMG_0000.CR3".to_string(), String::new()),
                ("IMG_0001.CR3".to_string(), String::new()),
                ("IMG_0001.CR3".to_string(), "Warm".to_string()),
                ("IMG_0002.CR3".to_string(), String::new()),
            ]
        );
        assert_eq!(crate::testing::count_labeled(&app, "Version Warm"), 1);
        app.invoke_select(0);
        for want in [1, 2, 3] {
            app.invoke_step(1, false);
            assert_eq!(state.borrow().current, Some(want));
        }
        app.invoke_step(-1, false);
        assert_eq!(state.borrow().current, Some(2), "back onto the version");
        // Shift and an arrow take it into the set as a frame.
        app.invoke_step(-1, true);
        let chosen = crate::panel::browser::chosen_frames(&state.borrow());
        assert_eq!(chosen, [1, 2]);
    }

    /// New version (Ctrl+') copies the frame's edit as the new line's
    /// one state, makes a cell for it after the file's, opens it, and
    /// asks its name with "Version 2" in the field; the name typed is
    /// the version's, on its cell.
    #[test]
    fn new_version_makes_a_cell_selects_it_and_opens_its_name() {
        let app = window(2);
        let (state, _worker) = state_for(&app, folder(2));
        app.invoke_select(0);
        app.set_exposure(0.5);
        save(&app, &state);
        app.invoke_version_new_asked(false, false);
        {
            let st = state.borrow();
            assert_eq!(st.cells.len(), 3);
            assert_eq!(st.files.len(), 2);
            assert_eq!(st.current, Some(1), "the new cell is open");
            let id = st.version_of(1).expect("a version").to_string();
            let line = st.line(1).unwrap();
            assert_eq!(line.current.light.exposure, 0.5, "the edit copied");
            assert_eq!(line.history.len(), 0, "and nothing else");
            assert_eq!(st.sidecars[0].versions.len(), 1);
            assert_eq!(st.sidecars[0].versions[0].id, id);
        }
        assert_eq!(app.get_selected(), 1);
        assert!(app.get_collection_sheet_open(), "its name asked for");
        assert_eq!(app.get_collection_sheet_name(), "Version 2");
        assert_eq!(rows(&app)[1].1, "Version 2");
        assert_eq!(app.get_history_version(), "Version 2");
        app.set_collection_sheet_name("Warm".into());
        app.invoke_collection_sheet_done();
        assert!(!app.get_collection_sheet_open());
        assert_eq!(state.borrow().sidecars[0].versions[0].name, "Warm");
        assert_eq!(rows(&app)[1].1, "Warm");
        assert_eq!(app.get_history_version(), "Warm");
        // From the original: the default edit.
        app.invoke_select(0);
        assert_eq!(app.get_history_version(), "");
        app.invoke_version_new_asked(true, false);
        let st = state.borrow();
        assert_eq!(st.cells.len(), 4);
        assert_eq!(st.current, Some(2), "after the file's other version");
        assert_eq!(st.line(2).unwrap().current, Edit::default());
        assert_eq!(app.get_collection_sheet_name(), "Version 3");
    }

    /// Rename version... asks with the version's name in the field and
    /// gives the version the name typed; the file's own frame offers no
    /// rename as a version.
    #[test]
    fn a_version_is_renamed_from_the_menu() {
        let app = window(2);
        let (state, worker) = state_for(&app, folder(2));
        let id = version_of(&state, &app, &worker, 0, "Warm");
        app.invoke_select(1);
        app.invoke_frame_menu_asked(1);
        assert_eq!(app.get_menu_version(), "Warm");
        app.invoke_version_rename_asked();
        assert!(app.get_collection_sheet_open());
        assert_eq!(app.get_collection_sheet_name(), "Warm");
        app.set_collection_sheet_name("  ".into());
        app.invoke_collection_sheet_done();
        assert!(app.get_collection_sheet_open(), "a blank name is no name");
        app.set_collection_sheet_name("Cool".into());
        app.invoke_collection_sheet_done();
        let st = state.borrow();
        assert_eq!(st.sidecars[0].version(&id).unwrap().name, "Cool");
        assert_eq!(rows(&app)[1].1, "Cool");
        drop(st);
        app.invoke_frame_menu_asked(0);
        assert_eq!(app.get_menu_version(), "", "the file's own is no version");
    }

    /// Swap with the original exchanges the two lines whole, edits and
    /// pictures, and the version keeps its id and its name.
    #[test]
    fn a_swap_exchanges_the_lines_and_the_version_keeps_its_id() {
        let app = window(2);
        let (state, worker) = state_for(&app, folder(2));
        app.invoke_select(0);
        app.set_exposure(0.5);
        save(&app, &state);
        let id = version_of(&state, &app, &worker, 0, "Warm");
        app.invoke_select(1);
        app.set_exposure(1.0);
        save(&app, &state);
        {
            let mut st = state.borrow_mut();
            st.views.thumb_base[0] = Some((1, 1, vec![0, 0, 0]));
            st.views.thumb_base[1] = Some((1, 1, vec![9, 9, 9]));
        }
        app.invoke_frame_menu_asked(1);
        app.invoke_version_swap_asked();
        let st = state.borrow();
        assert_eq!(
            st.sidecars[0].current.light.exposure, 1.0,
            "the file's own is the version's"
        );
        assert_eq!(st.line(1).unwrap().current.light.exposure, 0.5);
        assert_eq!(st.version_of(1), Some(id.as_str()), "the id stays");
        assert_eq!(st.version_name(1).as_deref(), Some("Warm"));
        assert_eq!(
            st.views.thumb_base[0].as_ref().unwrap().2,
            [9, 9, 9],
            "the pictures trade"
        );
        assert_eq!(st.views.thumb_base[1].as_ref().unwrap().2, [0, 0, 0]);
        assert_eq!(st.current, Some(1));
        assert_eq!(
            app.get_exposure(),
            0.5,
            "the panel shows the line it is on now"
        );
        assert_eq!(
            st.line(1).unwrap().history.len(),
            st.sidecars[0].version(&id).unwrap().line.history.len(),
            "nothing recorded over the swap"
        );
    }

    /// Delete on a version's cell asks first, in the version's words; a
    /// no keeps it, a yes takes the cell and the version and leaves the
    /// file and its other version. The file's own cell opens in the
    /// gone cell's place.
    #[test]
    fn a_version_is_deleted_after_the_sheet_asks() {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        let warm = version_of(&state, &app, &worker, 0, "Warm");
        let cool = version_of(&state, &app, &worker, 0, "Cool");
        assert_eq!(state.borrow().cells.len(), 3);
        app.invoke_select(1);
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_open());
        assert_eq!(
            app.get_delete_title(),
            "Delete the version Warm of IMG_0000.CR3?"
        );
        assert_eq!(
            app.get_delete_text(),
            "The file and its other versions stay."
        );
        assert_eq!(app.get_delete_confirm(), "Delete version");
        assert!(!app.get_delete_permanent());
        app.invoke_delete_answered(0);
        assert!(!app.get_delete_open());
        assert_eq!(state.borrow().cells.len(), 3, "a no keeps it");
        app.invoke_delete_asked("selection".into());
        app.invoke_delete_answered(1);
        let st = state.borrow();
        assert_eq!(cells(&st), [(0, None), (0, Some(cool.clone()))]);
        assert!(st.sidecars[0].version(&warm).is_none());
        assert_eq!(st.current, Some(0), "the file's own frame in its place");
        assert_eq!(app.get_delete_confirm(), "");
        drop(st);
        assert_eq!(rows(&app)[1].1, "Cool");
    }

    /// Two versions chosen and deleted at once both go, and the file's
    /// sidecar is written once for the two.
    #[test]
    fn two_versions_deleted_at_once_both_go() {
        let app = window(1);
        let (dir, files) = disk_folder("versions-delete-two", 1);
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        let warm = version_of(&state, &app, &worker, 0, "Warm");
        let cool = version_of(&state, &app, &worker, 0, "Cool");
        app.invoke_select(1);
        state.borrow_mut().views.picked = vec![1, 2];
        crate::panel::edit::write_file(&mut state.borrow_mut(), 0);
        let saved = state.borrow().sidecars[0].saved;
        app.invoke_delete_asked("selection".into());
        assert_eq!(app.get_delete_title(), "Delete 2 versions?");
        assert_eq!(app.get_delete_confirm(), "Delete versions");
        app.invoke_delete_answered(1);
        {
            let st = state.borrow();
            assert!(st.sidecars[0].version(&warm).is_none(), "Warm went");
            assert!(st.sidecars[0].version(&cool).is_none(), "Cool went");
            assert_eq!(cells(&st), [(0, None)]);
            assert_eq!(st.sidecars[0].saved, saved + 1, "one write for the two");
            assert_eq!(st.current, Some(0));
        }
        let disk = Sidecar::load(&files[0]).unwrap().unwrap();
        assert!(disk.versions.is_empty());
        crate::testing::remove_scratch(state, &dir);
    }

    /// The menu's Delete version... acts on the cell right-clicked and
    /// not on the selection: with the file's own cell and another
    /// version chosen beside it, the one under the pointer alone goes.
    #[test]
    fn the_menu_s_delete_version_acts_on_the_cell_right_clicked() {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        let warm = version_of(&state, &app, &worker, 0, "Warm");
        let cool = version_of(&state, &app, &worker, 0, "Cool");
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0, 1, 2];
        app.invoke_frame_menu_asked(2);
        assert_eq!(app.get_menu_version(), "Cool");
        app.invoke_version_delete_asked();
        assert_eq!(
            app.get_delete_title(),
            "Delete the version Cool of IMG_0000.CR3?"
        );
        app.invoke_delete_answered(1);
        let st = state.borrow();
        assert!(st.sidecars[0].version(&cool).is_none(), "Cool went");
        assert!(st.sidecars[0].version(&warm).is_some(), "Warm stays");
        assert_eq!(cells(&st), [(0, None), (0, Some(warm.clone()))]);
        drop(st);
        // On the file's own cell the menu offers no version to delete.
        app.invoke_frame_menu_asked(0);
        assert_eq!(app.get_menu_version(), "");
    }

    /// The panel on a version's cell edits that version's line: its
    /// history, its undo; the file's own line is untouched, and stays
    /// so through a step back to the file's own cell.
    #[test]
    fn the_panel_on_a_version_edits_that_line_alone() {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        let id = version_of(&state, &app, &worker, 0, "Warm");
        app.invoke_select(1);
        app.set_exposure(1.0);
        save(&app, &state);
        app.set_exposure(1.5);
        save(&app, &state);
        {
            let st = state.borrow();
            let line = &st.sidecars[0].version(&id).unwrap().line;
            assert_eq!(line.current.light.exposure, 1.5);
            assert_eq!(
                st.sidecars[0].current,
                Edit::default(),
                "the file's own untouched"
            );
            assert!(st.sidecars[0].history.is_empty());
        }
        app.invoke_undo();
        assert_eq!(
            state.borrow().line(1).unwrap().current.light.exposure,
            1.0,
            "the undo on the version's line"
        );
        app.invoke_select(0);
        assert_eq!(app.get_exposure(), 0.0, "the file's own panel");
        assert_eq!(state.borrow().sidecars[0].current, Edit::default());
        app.invoke_select(1);
        assert_eq!(app.get_exposure(), 1.0);
    }

    /// An export from a version's cell carries the version: its name
    /// joins the file's, IMG_0000-Warm.jpg, beside the file's own, and
    /// the set's lines name it "IMG_0000 (Warm)".
    #[test]
    fn an_export_from_a_version_is_named_for_it() {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        let id = version_of(&state, &app, &worker, 0, "Warm");
        app.invoke_select(0);
        app.invoke_step(1, true);
        let frames = crate::panel::deliver::set_frames(&mut state.borrow_mut(), &app);
        assert_eq!(frames.len(), 2);
        assert!(frames[0].4.is_none());
        let v = frames[1].4.clone().unwrap();
        assert_eq!((v.id.as_str(), v.name.as_str()), (id.as_str(), "Warm"));
        let sources: Vec<(PathBuf, Option<String>)> = frames
            .iter()
            .map(|f| (f.0.clone(), f.4.as_ref().map(|v| v.name.clone())))
            .collect();
        let out = std::path::Path::new("/out");
        let named = crate::queue::named_of(
            &sources,
            Some(out),
            None,
            crate::export::Format::Jpeg,
            crate::export::OnExists::Increment,
        );
        assert_eq!(named[0].0, out.join("IMG_0000.jpg"));
        assert_eq!(named[1].0, out.join("IMG_0000-Warm.jpg"));
        let frame = crate::queue::Frame {
            source: frames[1].0.clone(),
            edit: frames[1].1.clone(),
            version: frames[1].4.clone(),
            turn: 0,
            time_shift: 0,
            seed_blend: false,
            out: named[1].0.clone(),
            moved_from: None,
        };
        assert_eq!(crate::queue::label(&frame), "IMG_0000 (Warm)");
    }

    /// Two frames on disk, the first's sidecar with a version "Warm" and
    /// a rating: the version's id.
    fn shoot(dir: &Path) -> (PathBuf, PathBuf, String) {
        use greycard_library::fixture::{R5, R6, write_frame};
        std::fs::create_dir_all(dir).unwrap();
        let (a, b) = (dir.join("a.tif"), dir.join("b.tif"));
        write_frame(&a, &R5, 1);
        write_frame(&b, &R6, 2);
        let mut s = Sidecar {
            line: greycard_edit::Line::of(Edit::for_picture()),
            ..Sidecar::default()
        };
        s.meta.rating = 2;
        let id = s.new_version(None, Some("Warm")).unwrap();
        s.save(&a).unwrap();
        (a, b, id)
    }

    /// A folder opened from the disk reads each sidecar's versions as
    /// it reads the meta: a cell each after its file's. An edit on the
    /// version's cell is written into the version's line of the one
    /// sidecar, the file's own line as it was; a rating on it is the
    /// file's, written once.
    #[test]
    fn a_folder_from_the_disk_lists_versions_and_a_save_keeps_them() {
        let dir = crate::testing::scratch_dir("versions-disk");
        let (a, _b, id) = shoot(&dir);
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        state.borrow_mut().window.write_sidecars = true;
        crate::panel::browser::open_folder(&state, &app, &worker, &dir);
        crate::roots::land_sent(&state, &app, &worker);
        {
            let st = state.borrow();
            assert_eq!(cells(&st), [(0, None), (0, Some(id.clone())), (1, None)]);
            assert_eq!(st.sidecar(1).meta.rating, 2, "the file's meta");
        }
        assert_eq!(rows(&app)[1], ("a.tif".to_string(), "Warm".to_string()));
        app.invoke_select(1);
        app.set_exposure(0.8);
        save(&app, &state);
        let saved = state.borrow().sidecars[0].saved;
        assert!(app.invoke_meta_key("4".into()));
        let on_disk = Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(
            on_disk.version(&id).unwrap().line.current.light.exposure,
            0.8
        );
        assert_eq!(on_disk.current.light.exposure, 0.0, "the file's own line");
        assert!(on_disk.history.is_empty());
        assert_eq!(on_disk.meta.rating, 4);
        assert_eq!(on_disk.saved, saved + 1, "the key wrote the file once");
        assert_eq!(rows(&app).len(), 3);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A view of the roots lists a file's versions from its row in the
    /// index, a cell each standing in with the version's own marks,
    /// nothing read; the version's cell opened reads the sidecar, and
    /// what is edited there is the version's.
    #[test]
    fn a_roots_view_lists_versions_from_the_index() {
        let dir = crate::testing::scratch_dir("versions-index");
        let root = dir.join("shoot");
        let (a, _b, id) = shoot(&root);
        // The version edited, so its row says so and the file's does not.
        let mut s = Sidecar::load(&a).unwrap().unwrap();
        s.line_mut(Some(&id)).unwrap().record({
            let mut e = Edit::for_picture();
            e.light.exposure = 0.6;
            e
        });
        s.save(&a).unwrap();
        let db = dir.join("index").join("library.sqlite");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_tree(&root, &mut |_| {})
            .unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.window.write_sidecars = true;
            st.views.library.roots.add(&root).unwrap();
            crate::roots::recount(&mut st);
        }
        crate::roots::open_view(
            &state,
            &app,
            &worker,
            crate::roots::View::Roots(Some(root.clone())),
        );
        crate::roots::land_sent(&state, &app, &worker);
        {
            let st = state.borrow();
            assert_eq!(cells(&st), [(0, None), (0, Some(id.clone())), (1, None)]);
            assert!(!crate::rows::is_loaded(&st, 1), "stood in from its row");
            assert_eq!(crate::panel::browser::frame_marks(&st, 0), (false, false));
            assert_eq!(crate::panel::browser::frame_marks(&st, 1), (true, false));
        }
        assert_eq!(rows(&app)[1].1, "Warm");
        let thumbs = app.get_thumbs();
        assert!(!thumbs.row_data(0).unwrap().edited);
        assert!(
            thumbs.row_data(1).unwrap().edited,
            "the version's own pencil"
        );
        app.invoke_select(1);
        crate::rows::land_pending(&state, &app, &worker);
        assert!(crate::rows::is_loaded(&state.borrow(), 1));
        assert_eq!(app.get_exposure(), 0.6, "the version's edit on the panel");
        app.set_exposure(0.9);
        save(&app, &state);
        let on_disk = Sidecar::load(&a).unwrap().unwrap();
        assert_eq!(
            on_disk.version(&id).unwrap().line.current.light.exposure,
            0.9
        );
        assert_eq!(on_disk.current.light.exposure, 0.0);
        crate::testing::remove_scratch(state, &dir);
    }

    /// Review probe: the row lists an edited version, the file's own line
    /// unedited; the `.gcd` is not found at the read (a share's hiccup,
    /// as the guard in `rows::take` is for). The guard asks the file's own
    /// marks alone, so the default is taken, the version's cell goes, and
    /// a rating then writes a sidecar without the version.
    #[test]
    fn a_missing_sidecar_with_a_version_on_record_is_not_taken() {
        let dir = crate::testing::scratch_dir("versions-probe-missing");
        let root = dir.join("shoot");
        let (a, _b, id) = shoot(&root);
        let mut s = Sidecar::load(&a).unwrap().unwrap();
        s.meta.rating = 0;
        s.line_mut(Some(&id)).unwrap().record({
            let mut e = Edit::for_picture();
            e.light.exposure = 0.6;
            e
        });
        s.save(&a).unwrap();
        let db = dir.join("index").join("library.sqlite");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_tree(&root, &mut |_| {})
            .unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.window.write_sidecars = true;
            st.views.library.roots.add(&root).unwrap();
            crate::roots::recount(&mut st);
        }
        crate::roots::open_view(
            &state,
            &app,
            &worker,
            crate::roots::View::Roots(Some(root.clone())),
        );
        crate::roots::land_sent(&state, &app, &worker);
        assert_eq!(
            cells(&state.borrow()),
            [(0, None), (0, Some(id.clone())), (1, None)]
        );
        // The sidecar is not there at the read.
        let gcd = Sidecar::find(&a).unwrap();
        let aside = dir.join("aside.gcd");
        std::fs::rename(&gcd, &aside).unwrap();
        let took = crate::rows::load_frame(&mut state.borrow_mut(), &app, 1);
        std::fs::rename(&aside, &gcd).unwrap();
        crate::versions::settle_pending(&mut state.borrow_mut(), &app, &worker);
        let left = cells(&state.borrow());
        crate::testing::remove_scratch(state, &dir);
        assert!(
            !took && left.len() == 3,
            "a read that found no sidecar where the row lists an edited \
             version was taken: took {took}, cells {left:?}"
        );
    }

    /// The sidecar deleted on purpose: the read is refused while the
    /// row lists the version; once the index's row says no sidecar and
    /// no versions, the next read takes the default and the version's
    /// cell goes.
    #[test]
    fn a_deleted_sidecar_recovers_when_the_row_follows() {
        let dir = crate::testing::scratch_dir("versions-deleted-sidecar-recovers");
        let root = dir.join("shoot");
        let (a, _b, id) = shoot(&root);
        let db = dir.join("index").join("library.sqlite");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_tree(&root, &mut |_| {})
            .unwrap();
        let app = window(0);
        let (state, worker) = state_for(&app, Vec::new());
        {
            let mut st = state.borrow_mut();
            st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            st.window.write_sidecars = true;
            st.views.library.roots.add(&root).unwrap();
            crate::roots::recount(&mut st);
        }
        crate::roots::open_view(
            &state,
            &app,
            &worker,
            crate::roots::View::Roots(Some(root.clone())),
        );
        crate::roots::land_sent(&state, &app, &worker);
        assert_eq!(
            cells(&state.borrow()),
            [(0, None), (0, Some(id.clone())), (1, None)]
        );
        std::fs::remove_file(Sidecar::find(&a).unwrap()).unwrap();
        let took = crate::rows::load_frame(&mut state.borrow_mut(), &app, 1);
        assert!(!took, "refused");
        greycard_library::Library::open(&db)
            .unwrap()
            .index_tree(&root, &mut |_| {})
            .unwrap();
        let row = greycard_library::Library::open_read_only(&db)
            .unwrap()
            .rows_of_with(std::slice::from_ref(&a), &mut |p| p.to_path_buf())
            .unwrap()
            .remove(0)
            .expect("a row");
        assert!(row.versions.is_empty(), "the pass saw no sidecar");
        {
            let mut st = state.borrow_mut();
            crate::rows::apply_row(&mut st, 0, &row);
            let took = crate::rows::load_frame(&mut st, &app, 0);
            crate::versions::settle_pending(&mut st, &app, &worker);
            assert!(took);
            assert_eq!(cells(&st), [(0, None), (1, None)]);
        }
        crate::testing::remove_scratch(state, &dir);
    }

    /// A version's cell asks for its picture by its own frame, the
    /// file's path and its id: the worker holds it as edited by its own
    /// line, the file's own cell as the file's, and a save on the
    /// version's cell is said for its frame.
    #[test]
    fn a_version_s_picture_is_asked_by_its_own_frame() {
        let app = window(1);
        let (dir, files) = disk_folder("versions-picture", 1);
        let (state, worker) = state_for(&app, files);
        state.borrow_mut().window.write_sidecars = true;
        let id = version_of(&state, &app, &worker, 0, "Warm");
        let frame_of = |st: &State, c: usize| match crate::rows::thumb_job(st, c) {
            crate::worker::Job::Thumbnail { index, frame } => (index, frame),
            _ => panic!("a thumbnail job"),
        };
        {
            let st = state.borrow();
            let (index, frame) = frame_of(&st, 1);
            assert_eq!(index, 1);
            assert_eq!(frame.version(), Some(id.as_str()));
            assert_eq!(frame.path, st.files[0]);
            assert_eq!(frame_of(&st, 0).1, Key::file(&st.files[0]));
            assert!(!worker.edited().holds(&frame));
        }
        // The version's line edited and the file written while the
        // file's own cell is on screen (a paste onto the version, say):
        // the version's frame is held as edited by its own line, the
        // file's own not.
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            let mut warm = Edit::default();
            warm.light.exposure = 0.5;
            st.line_mut(1).unwrap().record(warm);
            crate::panel::edit::write_file(&mut st, 0);
        }
        let st = state.borrow();
        let frame = frame_of(&st, 1).1;
        assert!(
            worker.edited().holds(&frame),
            "the version's frame is held as edited by its save"
        );
        assert!(
            !worker.edited().holds(&Key::file(&st.files[0])),
            "the file's own is not"
        );
        drop(st);
        crate::testing::remove_scratch(state, &dir);
    }

    /// An export of a version landing is recorded on the version's line
    /// and marks its cell alone; the file's own cell stays unmarked.
    #[test]
    fn an_export_of_a_version_marks_its_cell_alone() {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        let id = version_of(&state, &app, &worker, 0, "Warm");
        let (source, edit) = {
            let st = state.borrow();
            (st.files[0].clone(), st.line(1).unwrap().current.clone())
        };
        let set = std::sync::Arc::new(crate::queue::Set::new(
            1,
            None,
            crate::export::Settings::default(),
            crate::export::OnExists::Increment,
        ));
        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::SetFrameDone {
                set,
                index: 0,
                source,
                edit,
                version: Some(id.clone()),
                done: crate::queue::Done::Exported {
                    path: PathBuf::from("/out/IMG_0000-Warm.jpg"),
                    seconds: 1.0,
                    note: None,
                    left_out: Vec::new(),
                },
            },
        );
        let st = state.borrow();
        let line = &st.sidecars[0].version(&id).unwrap().line;
        assert_eq!(
            line.last_export.as_ref().map(|e| e.file.as_str()),
            Some("/out/IMG_0000-Warm.jpg")
        );
        assert!(st.sidecars[0].last_export.is_none());
        let thumbs = app.get_thumbs();
        assert!(!thumbs.row_data(0).unwrap().exported);
        assert!(thumbs.row_data(1).unwrap().exported);
    }

    /// A culling key on a version's cell is the file's: the meta is
    /// shared, so the file's own cell and every version's wear it, and
    /// a key over the file and its version chosen together writes the
    /// file once, one revision.
    #[test]
    fn a_culling_key_on_a_version_marks_the_file_and_writes_it_once() {
        let app = window(2);
        let (dir, files) = disk_folder("versions-key", 2);
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        version_of(&state, &app, &worker, 0, "Warm");
        crate::panel::edit::write_file(&mut state.borrow_mut(), 0);
        app.invoke_select(1);
        assert!(app.invoke_meta_key("3".into()));
        assert!(app.invoke_meta_key("p".into()));
        {
            let st = state.borrow();
            assert_eq!(st.sidecars[0].meta.rating, 3);
            assert_eq!(st.sidecars[0].meta.flag, greycard_edit::meta::Flag::Pick);
            assert_eq!(st.sidecars[1].meta.rating, 0, "the other file is its own");
            let thumbs = app.get_thumbs();
            assert_eq!(thumbs.row_data(0).unwrap().rating, 3);
            assert_eq!(thumbs.row_data(1).unwrap().rating, 3);
            assert_eq!(thumbs.row_data(2).unwrap().rating, 0);
        }
        // The file and its version chosen together: one write.
        let saved = state.borrow().sidecars[0].saved;
        state.borrow_mut().views.picked = vec![0, 1];
        assert!(app.invoke_meta_key("5".into()));
        let st = state.borrow();
        assert_eq!(st.sidecars[0].meta.rating, 5);
        assert_eq!(st.sidecars[0].saved, saved + 1, "written once");
        let disk = Sidecar::load(&files[0]).unwrap().unwrap();
        assert_eq!(disk.meta.rating, 5);
        assert_eq!(disk.saved, saved + 1);
        drop(st);
        crate::testing::remove_scratch(state, &dir);
    }

    /// Settings copied from a file's own frame paste onto its version,
    /// which is a frame of its own; and a paste over a file and its two
    /// versions together lays a step on each line and writes the file
    /// once.
    #[test]
    fn a_paste_reaches_a_version_and_its_file_and_writes_once() {
        let app = window(2);
        let (dir, files) = disk_folder("versions-paste", 2);
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        let warm = version_of(&state, &app, &worker, 0, "Warm");
        let cool = version_of(&state, &app, &worker, 0, "Cool");
        app.invoke_select(0);
        app.set_exposure(-0.6);
        app.invoke_copy_asked();
        app.invoke_select(1);
        app.invoke_paste_asked();
        assert_eq!(app.get_sync_text(), "Onto IMG_0000 (Warm).");
        app.invoke_sync_applied();
        assert_eq!(app.get_exposure(), -0.6);
        assert_eq!(app.get_history_version(), "Warm");
        {
            let st = state.borrow();
            let line = &st.sidecars[0].version(&warm).unwrap().line;
            assert_eq!(line.current.light.exposure, -0.6);
            assert_eq!(
                line.current_label.as_deref(),
                Some("Paste from IMG_0000.CR3")
            );
            assert_eq!(
                st.sidecars[0]
                    .version(&cool)
                    .unwrap()
                    .line
                    .current
                    .light
                    .exposure,
                0.0,
                "Cool not chosen"
            );
        }
        // Copied from the other file, pasted onto the file's own and its
        // two versions together: three steps, one write.
        app.invoke_select(3);
        app.set_exposure(0.4);
        app.invoke_copy_asked();
        app.invoke_select(0);
        let saved = state.borrow().sidecars[0].saved;
        state.borrow_mut().views.picked = vec![0, 1, 2];
        app.invoke_paste_asked();
        assert_eq!(app.get_sync_text(), "Onto 3 frames.");
        app.invoke_sync_applied();
        let st = state.borrow();
        let exposures: Vec<f32> = st.sidecars[0]
            .lines()
            .map(|l| l.current.light.exposure)
            .collect();
        assert_eq!(exposures, [0.4, 0.4, 0.4]);
        assert_eq!(
            st.sidecars[0].current_label.as_deref(),
            Some("Paste from IMG_0001.CR3")
        );
        assert_eq!(st.sidecars[0].saved, saved + 1, "one write for the three");
        let disk = Sidecar::load(&files[0]).unwrap().unwrap();
        assert_eq!(disk.saved, saved + 1);
        assert_eq!(
            disk.version(&cool).unwrap().line.current.light.exposure,
            0.4
        );
        drop(st);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A read that lands with other versions than the row listed (one
    /// deleted on another machine) moves the list once the caller is
    /// done with its numbers: the cell goes, the file's own opens in
    /// its place, and nothing is written back.
    #[test]
    fn a_read_without_the_version_takes_its_cell_and_writes_nothing() {
        let app = window(1);
        let (dir, files) = disk_folder("versions-read", 1);
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        let id = version_of(&state, &app, &worker, 0, "Warm");
        crate::panel::edit::write_file(&mut state.borrow_mut(), 0);
        let saved = state.borrow().sidecars[0].saved;
        app.invoke_select(1);
        // The other machine's copy, without Warm, landing as a read of
        // the file does.
        let mut theirs = Sidecar::load(&files[0]).unwrap().unwrap();
        theirs.delete_version(&id).unwrap();
        theirs.meta.rating = 5;
        {
            let mut st = state.borrow_mut();
            st.from_row[0].read = false;
            assert!(crate::rows::land_read(&mut st, &app, 0, theirs));
            assert_eq!(cells(&st).len(), 2, "the cells wait for the next turn");
            assert!(st.line(1).is_none(), "the version's cell has no line now");
            assert_eq!(crate::versions::settle_pending(&mut st, &app, &worker), 1);
            assert_eq!(cells(&st), [(0, None)]);
            assert_eq!(st.current, Some(0), "the file's own cell in its place");
            assert_eq!(st.sidecars[0].meta.rating, 5);
            assert_eq!(st.sidecars[0].saved, saved, "nothing written");
        }
        assert_eq!(rows(&app).len(), 1);
        let disk = Sidecar::load(&files[0]).unwrap().unwrap();
        assert_eq!(disk.saved, saved);
        assert!(disk.version(&id).is_some(), "the disk as it was");
        crate::testing::remove_scratch(state, &dir);
    }

    /// The row from the index lists a version the cells lack, or lacks
    /// one they have: the stand-in takes the row's list and the cells
    /// follow at the next turn.
    #[test]
    fn a_row_with_other_versions_moves_the_cells() {
        let app = window(2);
        let (state, worker) = state_for(&app, folder(2));
        {
            let mut st = state.borrow_mut();
            st.from_row[0].read = false;
        }
        let row = greycard_library::RowMeta {
            id: 1,
            hash: "ab".repeat(32),
            mtime: 1,
            meta: Default::default(),
            edited: false,
            turns: 0,
            exported: false,
            iso: None,
            sidecar_hash: None,
            develop_key: None,
            versions: vec![greycard_library::VersionRow {
                id: "0123456789abcdef".into(),
                name: "Warm".into(),
                made: 7,
                edited: true,
                exported: false,
                develop_key: Some(9),
            }],
            taken: None,
            edited_at: None,
        };
        let mut st = state.borrow_mut();
        assert!(crate::rows::apply_row(&mut st, 0, &row));
        assert_eq!(st.sidecars[0].versions.len(), 1, "the stand-in lists it");
        assert_eq!(crate::versions::settle_pending(&mut st, &app, &worker), 1);
        assert_eq!(
            cells(&st),
            [(0, None), (0, Some("0123456789abcdef".into())), (1, None)]
        );
        assert_eq!(
            crate::panel::browser::frame_marks(&st, 1),
            (true, false),
            "the version's own marks from the row"
        );
        assert_eq!(crate::panel::browser::frame_marks(&st, 0), (false, false));
        assert_eq!(st.version_name(1).as_deref(), Some("Warm"));
    }

    /// A version whose line a read has found gone (the sidecar rewritten
    /// outside, or the version deleted from another cell), before the
    /// cells have followed: a paste, a sync and a preset onto its cell
    /// each say the frame is gone and lay nothing on it, and nothing is
    /// claimed of it or written.
    #[test]
    fn a_paste_a_sync_and_a_preset_onto_a_vanished_version_say_so() {
        let app = window(2);
        let (dir, files) = disk_folder("versions-vanished", 2);
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        let id = version_of(&state, &app, &worker, 0, "Warm");
        crate::panel::edit::write_file(&mut state.borrow_mut(), 0);
        // The version taken out of the sidecar behind the cells' back.
        state.borrow_mut().sidecars[0].delete_version(&id).unwrap();
        let saved = state.borrow().sidecars[0].saved;
        assert!(state.borrow().line(1).is_none());
        // Copied from the other file, pasted onto the vanished cell.
        app.invoke_select(2);
        app.set_exposure(-0.6);
        app.invoke_copy_asked();
        app.invoke_select(0);
        state.borrow_mut().views.picked = vec![0, 1];
        app.invoke_paste_asked();
        app.invoke_sync_applied();
        let status = app.get_status().to_string();
        // The version went before the paste was pressed, so its name is
        // not to be had: the frame is said by its id.
        assert!(
            status.contains("IMG_0000 (") && status.contains(" moved or is gone"),
            "{status}"
        );
        assert!(status.contains("paste was not applied"), "{status}");
        {
            let st = state.borrow();
            assert_eq!(
                st.sidecars[0].current.light.exposure, -0.6,
                "the file's own took it"
            );
            assert!(st.sidecars[0].versions.is_empty());
        }
        // A sync from the file's own onto the vanished cell.
        state.borrow_mut().views.picked = vec![0, 1];
        app.invoke_sync_open_asked();
        app.invoke_sync_applied();
        let status = app.get_status().to_string();
        assert!(status.contains("moved or is gone"), "{status}");
        assert!(status.contains("sync was not applied"), "{status}");
        // A preset over the two.
        let mut edit = Edit::default();
        edit.light.exposure = 1.5;
        let preset =
            greycard_edit::Preset::from_edit("Bright", &edit, &[greycard_edit::Section::Light]);
        state.borrow_mut().views.picked = vec![0, 1];
        crate::panel::sync::preset_pressed(&state, &app, &worker, preset);
        let status = app.get_status().to_string();
        assert!(status.contains("moved or is gone"), "{status}");
        assert!(status.contains("preset was not applied"), "{status}");
        let st = state.borrow();
        assert_eq!(st.sidecars[0].current.light.exposure, 1.5);
        // The paste and the preset wrote the file's own line (the sync had
        // nothing left to lay), and nothing was written for the version.
        assert_eq!(st.sidecars[0].saved, saved + 2);
        assert!(
            Sidecar::load(&files[0])
                .unwrap()
                .unwrap()
                .versions
                .is_empty()
        );
        drop(st);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The ISO's seed is a line's: a version with a strength of its own
    /// keeps it when the file's seed lands on the open cell, a new
    /// version from an unseeded original waits on its own line and takes
    /// the seed there when it is the one open, and the file's own line
    /// is left as it was.
    #[test]
    fn the_seed_lands_on_the_open_line_alone() {
        let app = window(1);
        let (state, worker) = state_for(&app, folder(1));
        let warm = version_of(&state, &app, &worker, 0, "Warm");
        let cool = version_of(&state, &app, &worker, 0, "Cool");
        {
            let mut st = state.borrow_mut();
            let mut own = Edit::default();
            own.noise.learned_strength = 0.77;
            st.sidecars[0].line_mut(Some(&warm)).unwrap().record(own);
            assert!(crate::rows::wants_seed(&st, 0), "the original waits");
            assert!(
                !crate::rows::wants_seed(&st, 1),
                "Warm has its own strength"
            );
            assert!(
                crate::rows::wants_seed(&st, 2),
                "Cool, from the unseeded original, waits"
            );
            // The seed lands with the file's own cell open.
            st.current = Some(0);
            assert!(crate::rows::seed_landed(&mut st, 0, 0.3));
            assert_eq!(st.sidecars[0].current.noise.learned_strength, 0.3);
            let strength = |st: &State, v: &str| {
                st.sidecars[0]
                    .version(v)
                    .unwrap()
                    .line
                    .current
                    .noise
                    .learned_strength
            };
            assert_eq!(strength(&st, &warm), 0.77, "kept its own");
            assert_eq!(
                strength(&st, &cool),
                Edit::default().noise.learned_strength,
                "still waits"
            );
            assert!(!crate::rows::wants_seed(&st, 0));
            assert!(crate::rows::wants_seed(&st, 2));
            // Cool open: its own line takes it.
            assert!(crate::rows::seed_landed(&mut st, 2, 0.5));
            assert_eq!(strength(&st, &cool), 0.5);
            assert_eq!(
                st.sidecars[0].current.noise.learned_strength, 0.3,
                "the file's own as it was"
            );
            // Warm, with a strength of its own, takes none.
            assert!(!crate::rows::seed_landed(&mut st, 1, 0.9));
            assert_eq!(strength(&st, &warm), 0.77);
        }
        // What the open job asks of the worker is the line's own wait.
        let st = state.borrow();
        assert!(!crate::rows::wants_seed(&st, 0) && !crate::rows::wants_seed(&st, 1));
    }

    /// The meta and the turn are the file's: a culling key or a turn on
    /// a version cell whose line a read has just found gone still rates
    /// and turns the file, where a paste onto it would say the frame is
    /// gone.
    #[test]
    fn a_culling_key_on_a_vanished_version_cell_rates_the_file() {
        let app = window(2);
        let (dir, files) = disk_folder("versions-vanished-key", 2);
        let (state, worker) = state_for(&app, files.clone());
        state.borrow_mut().window.write_sidecars = true;
        let id = version_of(&state, &app, &worker, 0, "Warm");
        crate::panel::edit::write_file(&mut state.borrow_mut(), 0);
        state.borrow_mut().sidecars[0].delete_version(&id).unwrap();
        {
            let mut st = state.borrow_mut();
            st.current = Some(1);
            st.views.picked = vec![1];
        }
        assert!(app.invoke_meta_key("4".into()));
        {
            let st = state.borrow();
            assert_eq!(st.sidecars[0].meta.rating, 4, "the file's rating");
            assert!(!app.get_status().contains("gone"), "{}", app.get_status());
        }
        // The panel's edit, and the develop's generation, before a turn
        // from the vanished cell: neither moves, since there is no line
        // to show or develop.
        let (panel, generation) = {
            let mut st = state.borrow_mut();
            st.develop.edit.light.exposure = 0.25;
            (st.develop.edit.clone(), st.develop.generation)
        };
        app.invoke_frame_turned(1);
        {
            let st = state.borrow();
            assert_eq!(st.sidecars[0].turn, 1, "the file's turn");
            assert_eq!(st.develop.edit, panel, "the panel's edit as it was");
            assert_eq!(st.develop.generation, generation, "no develop asked");
        }
        let disk = Sidecar::load(&files[0]).unwrap().unwrap();
        assert_eq!((disk.meta.rating, disk.turn), (4, 1));
        assert!(disk.versions.is_empty(), "nothing of the version written");
        crate::testing::remove_scratch(state, &dir);
    }

    /// The cells the compare view shows now, in its order.
    fn compared(st: &State) -> Vec<usize> {
        let cull = st.culling.cull.as_ref().expect("culling");
        cull.rows(&st.views.shown)
            .into_iter()
            .map(|r| st.views.shown[r])
            .collect()
    }

    /// Compare versions is offered on a file with versions and on each
    /// of its versions' cells, and on no file without one.
    #[test]
    fn compare_versions_is_offered_on_a_file_with_versions_and_its_versions() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        version_of(&state, &app, &worker, 1, "Warm");
        // Cells: 0, 1, 1 Warm, 2.
        for (row, offered) in [(0, false), (1, true), (2, true), (3, false)] {
            app.invoke_frame_menu_asked(row);
            assert_eq!(app.get_menu_compare(), offered, "row {row}");
        }
        // The versions gone, the file is one without.
        let id = state.borrow().version_of(2).unwrap().to_string();
        {
            let mut st = state.borrow_mut();
            st.sidecars[1].delete_version(&id).unwrap();
            crate::versions::settle(&mut st, &app, &worker);
        }
        app.invoke_frame_menu_asked(1);
        assert!(!app.get_menu_compare());
    }

    /// Compare versions shows the file and its versions and no other
    /// frame: from a file with one version, two up on the two, the file
    /// in focus; from a version of a file with two, the last file of the
    /// folder, three up on the three (the 2×2 with its fourth tile
    /// empty), anchored on the file with no slide at the list's end, the
    /// version in focus; V goes on from three to four, as from two. A
    /// file with four versions compares its first three, four up, and the
    /// status line says one was left out.
    #[test]
    fn compare_versions_sets_the_file_and_its_versions_side_by_side() {
        let app = window(4);
        let (state, worker) = state_for(&app, folder(4));
        version_of(&state, &app, &worker, 1, "Warm");
        version_of(&state, &app, &worker, 3, "Warm");
        version_of(&state, &app, &worker, 3, "Cool");
        // Cells: 0, 1, 1 Warm, 2, 3, 3 Warm, 3 Cool.
        app.invoke_select(0);
        app.invoke_frame_menu_asked(1);
        app.invoke_version_compare_asked();
        {
            let st = state.borrow();
            let cull = st.culling.cull.as_ref().expect("culling entered");
            assert_eq!(cull.compare, 2);
            assert_eq!(cull.anchor, 1, "anchored on the file");
            assert_eq!(compared(&st), [1, 2]);
            assert_eq!(st.current, Some(1), "the file in focus");
            assert_eq!(cull.note, None);
        }
        assert_eq!(app.get_compare(), 2);
        assert!(app.get_culling());
        // From a version, culling already on.
        app.invoke_frame_menu_asked(5);
        app.invoke_version_compare_asked();
        {
            let st = state.borrow();
            let cull = st.culling.cull.as_ref().unwrap();
            assert_eq!(cull.compare, 3);
            assert_eq!(cull.anchor, 4, "on the file's own cell, no slide");
            assert_eq!(compared(&st), [4, 5, 6], "the three and no other");
            assert_eq!(crate::cull::tile_grid(3), (2, 2), "the fourth empty");
            assert_eq!(st.current, Some(5), "the version right-clicked in focus");
            assert_eq!(crate::panel::browser::chosen_frames(&st), [5]);
        }
        assert_eq!(app.get_compare(), 3);
        assert_eq!(app.get_compare_text(), "3");
        // V from three is four, the next step of its cycle.
        crate::testing::press(&app, "v");
        {
            let st = state.borrow();
            let cull = st.culling.cull.as_ref().unwrap();
            assert_eq!(cull.compare, 4);
            assert_eq!(cull.versions_of, None, "the view is V's now");
        }
        assert_eq!(app.get_compare_text(), "4");
        // A file with four versions: the first three, and one left out.
        for name in ["A", "B", "C", "D"] {
            version_of(&state, &app, &worker, 0, name);
        }
        // Cells: 0, 0 A, 0 B, 0 C, 0 D, 1, ...
        app.invoke_frame_menu_asked(4);
        app.invoke_version_compare_asked();
        let st = state.borrow();
        let cull = st.culling.cull.as_ref().unwrap();
        assert_eq!(cull.compare, 4);
        assert_eq!(compared(&st), [0, 1, 2, 3]);
        assert_eq!(
            st.current,
            Some(0),
            "D is out of the set: the file in focus"
        );
        assert_eq!(
            cull.note.as_deref(),
            Some("1 more version of IMG_0000.CR3 left out")
        );
        assert!(
            app.get_status()
                .contains("1 more version of IMG_0000.CR3 left out"),
            "{}",
            app.get_status()
        );
    }

    /// A version's tile names the version on the plate, before what the
    /// picture is, and at its top the frame's label; the file's own tile
    /// keeps today's words.
    #[test]
    fn a_version_s_tile_names_the_version_and_the_file_s_does_not() {
        use crate::panel::cull::{Cull, Named, cull_status, plate_words, tile_name};
        let named = |file, version: Option<&str>| Named {
            file,
            size: (2048, 1365),
            small: false,
            own: false,
            local: crate::cull::Origin::Edit,
            version: version.map(str::to_string),
        };
        let names = [named(0, None), named(1, Some("Warm"))];
        assert_eq!(plate_words(&names[1]), "Warm, the edit, 2048 \u{d7} 1365");
        assert_eq!(plate_words(&names[0]), "the edit, 2048 \u{d7} 1365");
        let mut cull = Cull::new("".into(), 2048);
        cull.compare = 2;
        assert_eq!(
            cull_status(&cull, 1, 0.0, true, &names),
            "culling, 2 up: Warm, the edit, 2048 \u{d7} 1365, fitted; Enter develops"
        );
        assert_eq!(
            cull_status(&cull, 0, 0.0, true, &names),
            "culling, 2 up: the edit, 2048 \u{d7} 1365, fitted; Enter develops"
        );
        // The camera's JPEG where the version's picture is not made yet.
        let camera = Named {
            local: crate::cull::Origin::Camera,
            ..named(1, Some("Warm"))
        };
        assert_eq!(
            plate_words(&camera),
            "Warm, the camera JPEG, 2048 \u{d7} 1365"
        );
        cull.note = Some("1 more version of IMG_0001.CR3 left out".into());
        assert!(
            cull_status(&cull, 1, 0.0, true, &names)
                .ends_with("; Enter develops; 1 more version of IMG_0001.CR3 left out")
        );
        let path = Path::new("/nowhere/IMG_0001.CR3");
        let id: Arc<str> = Arc::from("0123456789abcdef");
        assert_eq!(tile_name(path, None, None), "IMG_0001.CR3");
        assert_eq!(tile_name(path, Some(&id), Some("Warm")), "IMG_0001 (Warm)");
        // A version with no name says its id, as every other label does.
        assert_eq!(
            tile_name(path, Some(&id), Some("")),
            "IMG_0001 (0123456789abcdef)"
        );
        // The state's own lookup of the name a tile carries.
        let app = window(2);
        let (state, worker) = state_for(&app, folder(2));
        version_of(&state, &app, &worker, 0, "Warm");
        let st = state.borrow();
        assert_eq!(
            crate::panel::cull::version_name_of(&st.sidecars, &st.cells, 1).as_deref(),
            Some("Warm")
        );
        assert_eq!(
            crate::panel::cull::version_name_of(&st.sidecars, &st.cells, 0),
            None
        );
    }

    /// Delete under compare on a version's tile is the version's delete,
    /// by its sheet; after it the file's own cell is in focus, culling
    /// stays, and the set closes up on the file's cells as they are now:
    /// three up becomes two, never taking in the next frame; with the
    /// last version gone it is one up on the file.
    #[test]
    fn delete_under_compare_takes_the_version_and_the_set_closes_up() {
        let app = window(4);
        let (state, worker) = state_for(&app, folder(4));
        let warm = version_of(&state, &app, &worker, 1, "Warm");
        let cool = version_of(&state, &app, &worker, 1, "Cool");
        // Cells: 0, 1, 1 Warm, 1 Cool, 2, 3.
        app.invoke_select(0);
        app.invoke_frame_menu_asked(2);
        app.invoke_version_compare_asked();
        assert_eq!(compared(&state.borrow()), [1, 2, 3]);
        assert_eq!(state.borrow().current, Some(2));
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_open());
        assert_eq!(
            app.get_delete_title(),
            "Delete the version Warm of IMG_0001.CR3?"
        );
        app.invoke_delete_answered(1);
        let st = state.borrow();
        assert!(st.sidecars[1].version(&warm).is_none(), "Warm went");
        assert!(st.sidecars[1].version(&cool).is_some(), "Cool stays");
        assert_eq!(
            cells(&st),
            [
                (0, None),
                (1, None),
                (1, Some(cool.clone())),
                (2, None),
                (3, None)
            ]
        );
        assert!(st.culling.cull.is_some(), "still culling");
        assert_eq!(st.current, Some(1), "the file's own frame in focus");
        assert_eq!(st.culling.cull.as_ref().unwrap().anchor, 1);
        assert_eq!(st.culling.cull.as_ref().unwrap().compare, 2);
        assert_eq!(compared(&st), [1, 2], "the set closed up, no stranger");
        assert_eq!(app.get_compare(), 2);
        assert_eq!(app.get_status(), "deleted the version Warm of IMG_0001.CR3");
        drop(st);
        // The last version deleted from the menu: one up on the file.
        app.invoke_frame_menu_asked(2);
        app.invoke_version_delete_asked();
        app.invoke_delete_answered(1);
        let st = state.borrow();
        assert!(st.sidecars[1].versions.is_empty());
        let cull = st.culling.cull.as_ref().expect("still culling");
        assert_eq!(cull.compare, 1);
        assert_eq!(cull.versions_of, None);
        assert_eq!(st.current, Some(1));
        assert_eq!(compared(&st), [1]);
    }

    /// The develop view's camera picture standing in for a version
    /// cell is asked for that version's frame, so its delivery is kept
    /// by the key check and does not hold a culling thread for nothing.
    #[test]
    fn a_version_s_placeholder_is_asked_for_the_version() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        let warm = version_of(&state, &app, &worker, 1, "Warm");
        app.invoke_select(0);
        app.invoke_select(2);
        let st = state.borrow();
        assert_eq!(st.current, Some(2));
        let asked = st.culling.prefetch.asked();
        let w = asked.iter().find(|w| w.file == 2).expect("cell 2 asked");
        assert_eq!(w.version.as_deref(), Some(warm.as_str()), "{asked:?}");
        assert_eq!(Some(w.frame()), st.key_of(2), "the delivery is kept");
    }

    /// Three up stands through every culling key and a click on a tile
    /// of the file; the field's 3 changes nothing and New version is
    /// refused. An arrow off the file leaves the compare: three up
    /// becomes four, the field no longer offers 3, and the set is placed
    /// about the frame as V's four up places it.
    #[test]
    fn three_up_stands_through_the_culling_keys_and_leaves_with_the_file() {
        let app = window(4);
        let (state, worker) = state_for(&app, folder(4));
        version_of(&state, &app, &worker, 3, "Warm");
        version_of(&state, &app, &worker, 3, "Cool");
        // Cells: 0, 1, 2, 3, 3 Warm, 3 Cool.
        app.invoke_select(0);
        app.invoke_frame_menu_asked(4);
        app.invoke_version_compare_asked();
        assert_eq!(compared(&state.borrow()), [3, 4, 5]);
        for key in ["1", "3", "p", "x", "u", "6", "[", "]", " ", " ", "z", "z"] {
            crate::testing::press(&app, key);
            let st = state.borrow();
            let cull = st.culling.cull.as_ref().expect("culling");
            assert_eq!(cull.compare, 3, "key {key}");
            assert_eq!(compared(&st), [3, 4, 5], "key {key}");
        }
        app.invoke_select(3);
        assert!(
            state
                .borrow()
                .culling
                .cull
                .as_ref()
                .unwrap()
                .versions_of
                .is_some()
        );
        app.invoke_compare_changed(3);
        assert_eq!(state.borrow().culling.cull.as_ref().unwrap().compare, 3);
        app.invoke_version_new_asked(false, false);
        assert_eq!(state.borrow().cells.len(), 6, "no version made in culling");
        // The arrow off the file.
        app.invoke_select(2);
        let st = state.borrow();
        let cull = st.culling.cull.as_ref().unwrap();
        assert_eq!(cull.versions_of, None);
        assert_eq!(cull.note, None);
        assert_eq!(cull.compare, 4, "three up is Compare versions' alone");
        assert_eq!(compared(&st), [2, 3, 4, 5]);
        assert_eq!(app.get_compare(), 4);
        assert_eq!(app.get_compare_text(), "4");
    }

    /// A settle from another cause than a delete refits the compare: a
    /// version of a file before the compared one (a join) moves the
    /// cells by one, and one of the compared file makes it three up.
    #[test]
    fn a_settle_from_a_join_refits_the_compare() {
        let app = window(4);
        let (state, worker) = state_for(&app, folder(4));
        version_of(&state, &app, &worker, 2, "Warm");
        // Cells: 0, 1, 2, 2 Warm, 3.
        app.invoke_select(0);
        app.invoke_frame_menu_asked(3);
        app.invoke_version_compare_asked();
        assert_eq!(compared(&state.borrow()), [2, 3]);
        version_of(&state, &app, &worker, 0, "Other");
        {
            let st = state.borrow();
            assert_eq!(compared(&st), [3, 4]);
            assert_eq!(st.current, Some(4));
        }
        version_of(&state, &app, &worker, 2, "Cool");
        let st = state.borrow();
        assert_eq!(compared(&st), [3, 4, 5]);
        assert_eq!(st.culling.cull.as_ref().unwrap().compare, 3);
    }

    /// Another file taken out of the list under a versions compare (a
    /// reject moved out, a delete) renumbers the cells outside `settle`:
    /// the set follows the file's cells to their new numbers. The
    /// compared file itself taken out ends the compare, three up
    /// becoming four.
    #[test]
    fn a_drop_of_files_under_compare_keeps_the_set_on_the_file() {
        let app = window(4);
        let (state, worker) = state_for(&app, folder(4));
        version_of(&state, &app, &worker, 2, "Warm");
        version_of(&state, &app, &worker, 2, "Cool");
        // Cells: 0, 1, 2, 2 Warm, 2 Cool, 3.
        app.invoke_select(0);
        app.invoke_frame_menu_asked(3);
        app.invoke_version_compare_asked();
        assert_eq!(compared(&state.borrow()), [2, 3, 4]);
        // At 1:1: the renumber under it keeps the zoom, the set coming
        // back at the count it had.
        state.borrow_mut().develop.zoom = 1.0;
        crate::panel::cull::drop_files(&mut state.borrow_mut(), &app, &worker, &[0], "gone".into());
        {
            let st = state.borrow();
            // The file is file 1 now: its cells 1, 2, 3.
            assert_eq!(compared(&st), [1, 2, 3]);
            assert_eq!(st.culling.cull.as_ref().unwrap().compare, 3);
            assert_eq!(st.current, Some(2), "Warm still in focus");
            assert_eq!(st.develop.zoom, 1.0, "the zoom stands");
        }
        crate::panel::cull::drop_files(&mut state.borrow_mut(), &app, &worker, &[1], "gone".into());
        let st = state.borrow();
        let cull = st.culling.cull.as_ref().unwrap();
        assert_eq!(cull.versions_of, None);
        assert_eq!(cull.compare, 4);
        assert_eq!(app.get_compare_text(), "4");
    }

    /// Compare versions from the grid or the lightbox shows the compare:
    /// the grid closes, and the lightbox is left first.
    #[test]
    fn compare_versions_from_the_grid_or_the_lightbox_closes_it() {
        let app = window(3);
        let (state, worker) = state_for(&app, folder(3));
        version_of(&state, &app, &worker, 1, "Warm");
        // Cells: 0, 1, 1 Warm, 2.
        app.invoke_select(0);
        app.set_grid_open(true);
        app.invoke_frame_menu_asked(1);
        app.invoke_version_compare_asked();
        assert!(!app.get_grid_open(), "the grid closed");
        assert!(app.get_culling());
        assert_eq!(compared(&state.borrow()), [1, 2]);
        app.invoke_cull_leave();
        app.invoke_lightbox_enter();
        assert!(app.get_lightbox() && app.get_grid_open());
        app.invoke_frame_menu_asked(2);
        app.invoke_version_compare_asked();
        assert!(!app.get_lightbox(), "the lightbox left");
        assert!(!app.get_grid_open());
        assert!(app.get_culling());
        assert_eq!(compared(&state.borrow()), [1, 2]);
        assert_eq!(state.borrow().current, Some(2));
    }

    /// In a collection a file's cells need not stand together: one that
    /// lists IMG_0000 (Warm), IMG_0001, IMG_0000 compares Warm and its
    /// file in two tiles, with IMG_0001 between them in the list and in
    /// none of the tiles. A click between the two keeps the set; an
    /// arrow onto IMG_0001 leaves the compare. Delete under it takes the
    /// version, and the file alone is one up.
    #[test]
    fn compare_in_a_collection_takes_the_file_s_cells_where_they_stand() {
        use crate::versions::Key;
        let app = window(3);
        let files = folder(3);
        let (state, worker) = state_for(&app, files.clone());
        let warm = version_of(&state, &app, &worker, 0, "Warm");
        {
            let mut st = state.borrow_mut();
            st.views.collection_frames = Some(vec![
                Key::of(&files[0], Some(&warm)),
                Key::file(&files[1]),
                Key::of(&files[0], None),
            ]);
            crate::versions::settle(&mut st, &app, &worker);
            assert_eq!(cells(&st), [(0, Some(warm.clone())), (1, None), (0, None)]);
        }
        app.invoke_select(0);
        app.invoke_frame_menu_asked(0);
        assert!(app.get_menu_compare());
        app.invoke_version_compare_asked();
        {
            let st = state.borrow();
            let cull = st.culling.cull.as_ref().expect("culling");
            assert_eq!(cull.compare, 2);
            assert_eq!(compared(&st), [0, 2], "the file's cells and no other");
            assert_eq!(st.current, Some(0), "Warm in focus");
        }
        // A click on the file's own tile: still its compare.
        app.invoke_select(2);
        assert_eq!(compared(&state.borrow()), [0, 2]);
        assert!(state.borrow().culling.cull.as_ref().unwrap().set.is_some());
        // An arrow onto IMG_0001: the compare is left, V's two up.
        app.invoke_select(1);
        {
            let st = state.borrow();
            let cull = st.culling.cull.as_ref().unwrap();
            assert_eq!(cull.set, None);
            assert_eq!(cull.versions_of, None);
            assert_eq!(cull.compare, 2);
            assert!(compared(&st).contains(&1));
        }
        // Back to a compare, and Warm deleted under it.
        app.invoke_frame_menu_asked(0);
        app.invoke_version_compare_asked();
        assert_eq!(compared(&state.borrow()), [0, 2]);
        app.invoke_delete_asked("selection".into());
        assert_eq!(
            app.get_delete_title(),
            "Delete the version Warm of IMG_0000.CR3?"
        );
        app.invoke_delete_answered(1);
        let st = state.borrow();
        assert!(st.sidecars[0].versions.is_empty());
        let cull = st.culling.cull.as_ref().expect("still culling");
        assert_eq!(cull.compare, 1);
        assert_eq!(cull.set, None);
        let own = st.cell_of(0, None).unwrap();
        assert_eq!(st.current, Some(own), "the file's own frame in focus");
    }

    /// Another folder opened while a versions compare stands: culling
    /// goes on over the new list, the compare does not. Its cells, its
    /// file and its note go, and three up becomes four.
    #[test]
    fn a_new_list_ends_a_versions_compare() {
        let app = window(4);
        let (state, worker) = state_for(&app, folder(4));
        version_of(&state, &app, &worker, 0, "Warm");
        version_of(&state, &app, &worker, 0, "Cool");
        // Cells: 0, 0 Warm, 0 Cool, 1, 2, 3.
        app.invoke_select(0);
        app.invoke_frame_menu_asked(1);
        app.invoke_version_compare_asked();
        assert_eq!(compared(&state.borrow()), [0, 1, 2]);
        let files: Vec<PathBuf> = (0..5)
            .map(|i| PathBuf::from(format!("/other/B_{i}.CR3")))
            .collect();
        let held = files
            .iter()
            .map(|f| crate::rows::from_disk(f, false))
            .collect();
        crate::panel::browser::open_loaded(&state, &app, &worker, files, held, true, 0);
        let st = state.borrow();
        let cull = st.culling.cull.as_ref().expect("still culling");
        assert_eq!(cull.versions_of, None);
        assert_eq!(cull.set, None);
        assert_eq!(cull.note, None);
        assert_eq!(cull.compare, 4);
        assert_eq!(app.get_compare_text(), "4");
    }
}
