//! The makers of versions on the window: New version (Ctrl+', the
//! frame menu), New version from the original, Rename version..., Swap
//! with the original and Delete version... Each changes the file's one
//! sidecar through the edit crate's makers, writes it once through the
//! usual save, and puts the cells in step with it (`versions::settle`):
//! a version is a cell of its own right after its file's.

use std::path::Path;
use std::rc::Rc;

use crate::tr;
use greycard_edit::Edit;
use slint::{ComponentHandle, Model};

use crate::panel::browser::{file_name, open_row, row_of, show_badges, show_thumb, thumb_turns};
use crate::panel::edit::{read_edit, save_edit};
use crate::sidecars::write_file;
use crate::versions::Key;
use crate::worker::Worker;
use crate::{App, Shared, State};

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
        return Some(tr!("a version is made from the editor, not while culling"));
    }
    unready(st, app, c)
}

/// [`refused`] less culling: a version's delete touches no panel, and
/// under compare it is how the loser goes.
pub(crate) fn unready(st: &mut State, app: &App, c: usize) -> Option<String> {
    if crate::panel::delete::held(st, c) {
        return Some(tr!("a delete is under way"));
    }
    if !crate::rows::load_frame(st, app, c) {
        return Some(
            crate::rows::refused(st, c, &tr!("no version is made"))
                .unwrap_or_else(|| tr!("no version is made")),
        );
    }
    if st.sidecar_at(c).is_some_and(|s| s.read_only()) {
        return Some(crate::panel::browser::newer_words());
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
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    from_original: bool,
    from_menu: bool,
) {
    let mut st = state.borrow_mut();
    let Some(c) = cell_for(&st, from_menu) else {
        app.set_status(tr!("no frame to make a version of").into());
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
        st.sidecars[file].new_version_from_original(default, None)
    } else {
        let from = st.version_of(c).map(str::to_string);
        st.sidecars[file].new_version(from.as_deref(), None)
    };
    let Some(id) = id else {
        app.set_status(tr!("the version it was to be made from is gone").into());
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
    app.set_status(
        tr!(
            "made {name} of {file}",
            name = name,
            file = file_name(&path)
        )
        .into(),
    );
    ask_name(&mut st, app, &path, &id, &name, true);
}

/// The name sheet over version `id` of `path`: the collections' sheet,
/// its field holding `name`, all of it chosen.
fn ask_name(st: &mut State, app: &App, path: &Path, id: &str, name: &str, fresh: bool) {
    st.views.collections_held.asked = Some(crate::panel::collections::Asked::Version {
        path: path.to_path_buf(),
        id: id.to_string(),
    });
    app.set_collection_sheet_title(
        if fresh {
            tr!("Name the new version")
        } else {
            tr!("Rename the version")
        }
        .into(),
    );
    app.set_collection_sheet_note(
        tr!(
            "A version of {file}, with an edit of its own; the rating, flag, label and keywords are the file's",
            file = file_name(path)
        )
        .into(),
    );
    app.set_collection_sheet_asks_name(true);
    app.set_collection_sheet_verb(if fresh { tr!("Name") } else { tr!("Rename") }.into());
    app.set_collection_sheet_placeholder(tr!("The version's name").into());
    app.set_collection_sheet_name(name.into());
    app.set_collection_sheet_open(true);
}

/// Rename version... on the cell the menu was opened on.
pub(crate) fn ask_rename(st: &mut State, app: &App, from_menu: bool) {
    let Some(c) = cell_for(st, from_menu) else {
        return;
    };
    let (Some(id), Some(name)) = (st.version_of(c).map(str::to_string), st.version_name(c)) else {
        app.set_status(tr!("the file's own frame is renamed as a file, not as a version").into());
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
        app.set_status(tr!("that version is gone; nothing renamed").into());
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
    app.set_status(
        tr!(
            "{was} of {file} is {now} now",
            was = was,
            file = file_name(path),
            now = now
        )
        .into(),
    );
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
        app.set_status(tr!("the original is swapped from one of its versions").into());
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
            tr!(
                "that version of {file} is gone; nothing swapped",
                file = file_name(&path)
            )
            .into(),
        );
        return;
    }
    let name = st.version_name(c).unwrap_or_default();
    // The Fill's strokes waiting on either go with the line they were
    // painted over.
    crate::panel::painted::swapped(st, &path, &id);
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
    app.set_status(
        tr!(
            "{name} and the original of {file} swapped",
            name = name,
            file = file_name(&path)
        )
        .into(),
    );
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
        app.set_status(tr!("no version is chosen").into());
        return;
    };
    let title = match keys.len() {
        1 => tr!(
            "Delete the version {name} of {file}?",
            name = st.version_name(*first).unwrap_or_default(),
            file = file_name(st.path_of(*first))
        ),
        n => tr!("Delete {n} version?" | "Delete {n} versions?" % n),
    };
    let text = if keys.len() == 1 {
        tr!("The file and its other versions stay.")
    } else {
        tr!("The files and their other versions stay.")
    };
    app.set_delete_title(title.into());
    app.set_delete_text(text.into());
    app.set_delete_note(tr!("Its edit, history and snapshots go with it.").into());
    app.set_delete_archive("".into());
    app.set_delete_trash(true);
    app.set_delete_permanent(false);
    app.set_delete_confirm(
        if keys.len() == 1 {
            tr!("Delete version")
        } else {
            tr!("Delete versions")
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
            // Gone from its sidecar, the version leaves its stack, and
            // takes the Fill's strokes waiting on it.
            crate::stacks::version_left(st, file, id);
            crate::panel::painted::drop_frame(st, key);
            gone.push(tr!(
                "{name} of {file}",
                name = taken.name,
                file = file_name(&key.path)
            ));
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    // Each file written once; the cells then drop the versions taken.
    for &file in &files {
        write_file(st, file);
    }
    // The same list less some cells: the strip and the grid hold
    // their scroll, as they do when files leave it.
    if crate::versions::settle(st, app, worker) {
        app.set_rows_held(true);
    }
    let said = match gone.as_slice() {
        [] => tr!("nothing deleted"),
        [one] => tr!("deleted the version {name}", name = one),
        many => tr!("deleted {n} version" | "deleted {n} versions" % many.len()),
    };
    tracing::info!("versions: {said}");
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
        app.set_status(tr!("no frame to compare").into());
        return;
    };
    let Some(file) = st.file_at(c) else {
        return;
    };
    let path = st.files[file].clone();
    let name = file_name(&path);
    let (set, left) = compare_set(st, file);
    if set.len() < 2 {
        app.set_status(tr!("{name} has no versions to compare", name = name).into());
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
        tr!(
            "comparing {name} and {n} of its versions; {left}"
                | "comparing {name} and {n} of its versions; {left}" % set.len() - 1,
            name = name,
            left = left_out(left, &name)
        )
    } else {
        tr!("comparing {name} and its versions", name = name)
    };
    tracing::info!("versions: {said}");
    app.set_status(said.into());
}

/// The status line's word for `left` versions of `name` that compare
/// did not show.
fn left_out(left: usize, name: &str) -> String {
    tr!(
        "{n} more version of {name} left out" | "{n} more versions of {name} left out" % left,
        name = name
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

pub(crate) fn install(app: &App, state: &Rc<Shared>, worker: &Rc<Worker>) {
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_version_new_asked(move |from_original, from_menu| {
            if let Some(app) = app_weak.upgrade()
                && !crate::panel::queue_view::refused(&state.borrow(), "a new version")
            {
                new_version(&state, &app, &worker, from_original, from_menu);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_version_rename_asked(move || {
            if let Some(app) = app_weak.upgrade()
                && !crate::panel::queue_view::refused(&state.borrow(), "a version's name")
            {
                ask_rename(&mut state.borrow_mut(), &app, true);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_version_swap_asked(move || {
            if let Some(app) = app_weak.upgrade()
                && !crate::panel::queue_view::refused(&state.borrow(), "a version's swap")
            {
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
                if st.views.view == crate::roots::View::Queue {
                    crate::panel::queue_view::ask_out(&mut st, &app);
                    return;
                }
                if st.sheets.deleting.is_some() {
                    app.set_status(tr!("a delete is still under way").into());
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
mod tests;
