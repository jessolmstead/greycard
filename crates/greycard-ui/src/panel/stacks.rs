//! The stacks in the window (`crate::stacks` keeps them and lays them
//! over the list): Stack and Unstack, Set as top and Remove from stack
//! in the frame menu and on their keys, a stack opened and closed by
//! its badge or by S, and Compare stack.
//!
//! The keys are Lightroom's: Ctrl+G stacks the selection, Ctrl+Shift+G
//! unstacks, S opens or closes, Shift+S puts the frame on top. S and
//! Shift+S are the grid's, as in Lightroom's Library; everywhere else S
//! stays the proof, and in culling a stack opens and closes by its badge
//! on the strip.
//!
//! What acts on a closed stack acts on what is seen of it, its top:
//! a rating, a flag, an export, a delete, a collection's Add. What acts
//! on the stack (Unstack, S, Compare stack) takes the stack through any
//! of its frames shown.

use std::collections::{BTreeMap, HashSet};

use greycard_library::Rehash;
use greycard_library::collections::Reference;

use crate::panel::browser::{chosen_frames, open_row, row_of};
use crate::panel::collections::frames_of;
use crate::stacks::{Listed, listed, stack_of};
use crate::*;

/// Most frames side by side; a stack of more compares its first four.
const COMPARE_MOST: usize = 4;

/// The cell the menu was opened on, or the frame on screen for a key.
fn cell_for(st: &State, from_menu: bool) -> Option<usize> {
    if from_menu {
        st.menu_file.or(st.current)
    } else {
        st.current
    }
}

/// The stacks the chosen cells are members of, each once, in the order
/// first met.
fn chosen_stacks(st: &State) -> Vec<Listed> {
    let all = listed(st);
    let mut out: Vec<Listed> = Vec::new();
    for c in chosen_frames(st) {
        let Some(cell) = st.cell(c) else {
            continue;
        };
        if let Some(l) = all.iter().find(|l| l.members.contains(cell))
            && !out.iter().any(|o| o.id == l.id)
        {
            out.push(l.clone());
        }
    }
    out
}

/// The list made again after a change to the stacks, and the browser's
/// rows with it.
fn settle(st: &mut State, app: &App, worker: &Worker) {
    if !crate::versions::settle(st, app, worker) {
        crate::panel::browser::rebuild_browser(st, app);
    }
}

/// The name a stack goes by in the status line: its top's.
fn top_name(st: &State, l: &Listed) -> String {
    let Some(top) = l.members.first() else {
        return String::new();
    };
    let key = crate::versions::Key {
        path: st.files[top.file].clone(),
        version: top.version.clone(),
        name: top.version.as_deref().and_then(|v| {
            st.sidecars
                .get(top.file)?
                .version(v)
                .map(|x| std::sync::Arc::from(x.name.as_str()))
        }),
    };
    key.said()
}

/// A member Stack would make: a cell chosen, or a reference of a closed
/// stack whose top was chosen.
enum Planned {
    Cell(usize),
    Ref(Reference),
}

/// What Stack would make of the frames chosen: the frame on screen first
/// and the rest in the list's order, a closed stack's top standing for
/// its stack and bringing its other references after it, each frame
/// once by its path and version. The menu counts it before it is made.
fn planned(st: &State) -> Vec<Planned> {
    let chosen = chosen_frames(st);
    let all = listed(st);
    let mut order: Vec<usize> = Vec::with_capacity(chosen.len());
    if let Some(c) = st.current.filter(|c| chosen.contains(c)) {
        order.push(c);
    }
    order.extend(chosen.iter().copied().filter(|c| Some(*c) != st.current));
    let mut seen: HashSet<(String, Option<String>)> = HashSet::new();
    let mut out = Vec::new();
    for c in order {
        let cell = &st.cells[c];
        let Some(hint) = crate::stacks::hint_of_file(st, cell.file) else {
            continue;
        };
        if seen.insert((hint, cell.version.as_deref().map(str::to_string))) {
            out.push(Planned::Cell(c));
        }
        if let Some(l) = all
            .iter()
            .find(|l| !l.open && l.members.first() == Some(cell))
            && let Some(s) = st.stacks.stack(&l.id)
        {
            for (i, r) in s.refs.iter().enumerate() {
                if i != l.refs[0] && seen.insert((r.hint.clone(), r.version.clone())) {
                    out.push(Planned::Ref(r.clone()));
                }
            }
        }
    }
    out
}

/// Stack (Ctrl+G): the frames chosen made a stack, the frame on screen
/// on top and the rest after it in the list's order (`planned`). A
/// frame in another stack leaves that one. The frames must be of one
/// folder, and a collection's list, which is its own order, makes none.
/// A member is kept at the path the list finds it by, a frame followed
/// on an archive's copy at its local one.
pub(crate) fn make(st: &mut State, app: &App, worker: &Worker) {
    if st.collection_frames.is_some() {
        app.set_status("a stack is made in a folder's list, not in a collection's".into());
        return;
    }
    let plan = planned(st);
    let cells: Vec<usize> = plan
        .iter()
        .filter_map(|p| match p {
            Planned::Cell(c) => Some(*c),
            Planned::Ref(_) => None,
        })
        .collect();
    let folders: HashSet<Option<PathBuf>> = cells
        .iter()
        .map(|&c| crate::stacks::folder_of(st, st.file_of(c)))
        .collect();
    if folders.len() > 1 {
        app.set_status("a stack is made of frames in one folder".into());
        return;
    }
    let frames = frames_of(st, &cells);
    if frames.len() != cells.len() {
        app.set_status("not stacked: a frame chosen could not be read".into());
        return;
    }
    let mut hashes = frames.into_iter();
    let mut refs: Vec<Reference> = Vec::with_capacity(plan.len());
    for p in plan {
        match p {
            Planned::Cell(c) => {
                let Some((hash, _, version)) = hashes.next() else {
                    break;
                };
                let Some(hint) = crate::stacks::hint_of_file(st, st.file_of(c)) else {
                    continue;
                };
                refs.push(Reference {
                    hash,
                    hint,
                    version,
                });
            }
            Planned::Ref(r) => refs.push(r),
        }
    }
    if refs.len() < 2 {
        app.set_status("choose two frames or more to stack".into());
        return;
    }
    let n = refs.len();
    st.stacks.make(refs);
    let name = st.current.and_then(|c| st.key_of(c)).map(|k| k.said());
    settle(st, app, worker);
    let said = format!(
        "{} stacked{}",
        crate::queue::frames(n),
        name.map(|n| format!(" under {n}")).unwrap_or_default()
    );
    tracing::info!("{said}");
    app.set_status(said.into());
}

/// Unstack (Ctrl+Shift+G): each stack a frame chosen is in, no more.
pub(crate) fn unmake(st: &mut State, app: &App, worker: &Worker) {
    let stacks = chosen_stacks(st);
    if stacks.is_empty() {
        app.set_status("no stack chosen".into());
        return;
    }
    let said = match stacks.as_slice() {
        [one] => format!(
            "{}'s stack of {} unstacked",
            top_name(st, one),
            one.members.len()
        ),
        many => format!("{} stacks unstacked", many.len()),
    };
    for l in &stacks {
        st.stacks.unstack(&l.id);
    }
    settle(st, app, worker);
    tracing::info!("{said}");
    app.set_status(said.into());
}

/// A stack opened, or closed: the stack of browser row `row` (its
/// badge), or, for none, those of the frames chosen (S), opened all when
/// any is closed and closed all otherwise. A stack closed with the frame
/// on screen behind its top opens its top in its place.
pub(crate) fn toggle(st: &mut State, app: &App, worker: &Worker, row: Option<usize>) {
    let stacks = match row {
        Some(r) => st
            .shown
            .get(r)
            .and_then(|&c| stack_of(st, c))
            .map(|(l, _)| vec![l])
            .unwrap_or_default(),
        None => chosen_stacks(st),
    };
    if stacks.is_empty() {
        app.set_status("no stack chosen".into());
        return;
    }
    let open = stacks.iter().any(|l| !l.open);
    if !open
        && let Some(c) = st.current
        && let Some(cell) = st.cell(c).cloned()
        && let Some(l) = stacks.iter().find(|l| l.members[1..].contains(&cell))
        && let Some(top) = st.cell_of(l.members[0].file, l.members[0].version.as_deref())
        && let Some(row) = row_of(st, top)
    {
        open_row(st, app, worker, row as i32, false);
    }
    for l in &stacks {
        st.stacks.set_open(&l.id, open);
    }
    settle(st, app, worker);
}

/// Set as top (Shift+S): the frame right-clicked, or the one on
/// screen, on top of its stack.
pub(crate) fn set_top(st: &mut State, app: &App, worker: &Worker, from_menu: bool) {
    let Some((l, at)) = cell_for(st, from_menu).and_then(|c| stack_of(st, c)) else {
        app.set_status("not in a stack".into());
        return;
    };
    if at == 0 {
        return;
    }
    st.stacks.set_top_at(&l.id, l.refs[at]);
    settle(st, app, worker);
    let top = Listed {
        members: vec![l.members[at].clone()],
        ..l
    };
    app.set_status(format!("{} on top of its stack", top_name(st, &top)).into());
}

/// Remove from stack: the frames chosen that are members out of their
/// stacks; a stack left with one goes. A closed stack's top chosen is
/// its top alone, and the next member comes up.
pub(crate) fn remove(st: &mut State, app: &App, worker: &Worker) {
    let all = listed(st);
    let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for c in chosen_frames(st) {
        let Some(cell) = st.cell(c) else {
            continue;
        };
        for l in &all {
            if let Some(at) = l.members.iter().position(|m| m == cell) {
                out.entry(l.id.clone()).or_default().push(l.refs[at]);
            }
        }
    }
    if out.is_empty() {
        app.set_status("no frame chosen is in a stack".into());
        return;
    }
    let n: usize = out.iter().map(|(id, at)| st.stacks.remove_at(id, at)).sum();
    settle(st, app, worker);
    app.set_status(
        format!(
            "{} taken out of {}",
            crate::queue::frames(n),
            stacks_word(out.len())
        )
        .into(),
    );
}

fn stacks_word(n: usize) -> &'static str {
    if n == 1 { "its stack" } else { "their stacks" }
}

/// The cells Compare stack shows of stack `id`: its members listed and
/// shown, four at most, and how many more it has.
fn compare_set(st: &State, id: &str) -> Option<(Vec<usize>, usize)> {
    let l = listed(st).into_iter().find(|l| l.id == id)?;
    let all: Vec<usize> = l
        .members
        .iter()
        .filter_map(|m| st.cell_of(m.file, m.version.as_deref()))
        .filter(|&c| row_of(st, c).is_some())
        .collect();
    let left = all.len().saturating_sub(COMPARE_MOST);
    Some((all.into_iter().take(COMPARE_MOST).collect(), left))
}

/// Compare stack: culling with the stack's members side by side, its
/// top first, two, three or four up by how many there are; a closed
/// stack is opened for it. The view stays the stack's as the list
/// changes under it, as Compare versions' stays its file's.
pub(crate) fn compare(st: &mut State, app: &App, worker: &Worker, from_menu: bool) {
    let Some((l, at)) = cell_for(st, from_menu).and_then(|c| stack_of(st, c)) else {
        app.set_status("not in a stack".into());
        return;
    };
    if !l.open {
        st.stacks.set_open(&l.id, true);
        settle(st, app, worker);
    }
    let Some((set, left)) = compare_set(st, &l.id) else {
        return;
    };
    if set.len() < 2 {
        app.set_status("the stack's frames are hidden by the filter".into());
        return;
    }
    let focus = st
        .cell_of(l.members[at].file, l.members[at].version.as_deref())
        .filter(|c| set.contains(c))
        .unwrap_or(set[0]);
    if st.cull.is_none() {
        crate::panel::cull::enter_cull(st, app, 1);
    }
    if st.current != Some(focus) {
        if let Some(row) = row_of(st, focus) {
            open_row(st, app, worker, row as i32, false);
        }
    } else {
        st.picked = vec![focus];
        crate::panel::browser::show_set(st, app);
    }
    fit_compare(st, app, &l.id, None);
    let name = top_name(st, &l);
    let said = if left > 0 {
        format!("comparing {name}'s stack; {left} more of it left out")
    } else {
        format!("comparing {name}'s stack")
    };
    tracing::info!("{said}");
    app.set_status(said.into());
}

/// The culling view fitted to stack `id`'s members as the cells are
/// now, as `versions::fit_compare` fits it to a file's: as many tiles
/// as are shown, four at most, by name. With fewer than two left (the
/// stack gone, closed, or its members filtered out) the view is no
/// longer the stack's.
pub(crate) fn fit_compare(st: &mut State, app: &App, id: &str, was: Option<usize>) {
    let set = compare_set(st, id)
        .filter(|(set, _)| set.len() > 1)
        .and_then(|(set, left)| Some((row_of(st, *set.first()?)?, set, left)));
    let Some((first, set, left)) = set else {
        if let Some(cull) = st.cull.as_mut() {
            cull.leave_versions(app);
        }
        if let Some(c) = st.current {
            crate::panel::cull::cull_select(st, app, c);
        }
        return;
    };
    let up = set.len();
    let Some(cull) = st.cull.as_mut() else {
        return;
    };
    let changed = was.unwrap_or(cull.compare) != up;
    cull.compare = up;
    cull.anchor = first;
    cull.note = (left > 0).then(|| format!("{left} more of the stack left out"));
    cull.versions_of = None;
    cull.stack_of = Some(id.to_string());
    cull.set = Some(set);
    cull.status.clear();
    app.set_compare(up as i32);
    app.set_compare_text(up.to_string().into());
    if changed {
        st.zoom = 0.0;
        st.image_size = (0, 0);
    }
    crate::panel::cull::cull_refresh(st);
    app.window().request_redraw();
}

/// What the frame menu says of the stack under the cell right-clicked:
/// how many frames it shows (none for no stack), whether it is open,
/// whether the cell is its top; and whether the frames chosen can be
/// stacked.
pub(crate) fn menu_asked(st: &State, app: &App, c: usize) {
    let of = stack_of(st, c);
    app.set_menu_stack(of.as_ref().map_or(0, |(l, _)| l.members.len() as i32));
    app.set_menu_stack_open(of.as_ref().is_some_and(|(l, _)| l.open));
    app.set_menu_stack_top(of.as_ref().is_some_and(|&(_, at)| at == 0));
    // Counted as it would be made: a closed top brings its stack.
    let holds = planned(st).len();
    app.set_menu_can_stack(st.collection_frames.is_none() && chosen_frames(st).len() > 1);
    app.set_menu_stack_holds(holds as i32);
}

/// The index found members moved or rewritten: the stacks follow, and
/// the list with them.
pub(crate) fn healed(
    state: &Rc<RefCell<State>>,
    app: &App,
    moved: &[(PathBuf, PathBuf)],
    rehashed: &[Rehash],
) {
    let mut st = state.borrow_mut();
    if !st.stacks.heal(moved, rehashed) {
        return;
    }
    if let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) {
        settle(&mut st, app, &worker);
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, worker: &Rc<Worker>) {
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_stack_make(move || {
            if let Some(app) = app_weak.upgrade() {
                make(&mut state.borrow_mut(), &app, &worker);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_stack_unmake(move || {
            if let Some(app) = app_weak.upgrade() {
                unmake(&mut state.borrow_mut(), &app, &worker);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_stack_toggle(move || {
            if let Some(app) = app_weak.upgrade() {
                toggle(&mut state.borrow_mut(), &app, &worker, None);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_stack_badge(move |row| {
            if let Some(app) = app_weak.upgrade() {
                toggle(
                    &mut state.borrow_mut(),
                    &app,
                    &worker,
                    usize::try_from(row).ok(),
                );
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_stack_top(move |from_menu| {
            if let Some(app) = app_weak.upgrade() {
                set_top(&mut state.borrow_mut(), &app, &worker, from_menu);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_stack_remove(move || {
            if let Some(app) = app_weak.upgrade() {
                remove(&mut state.borrow_mut(), &app, &worker);
            }
        });
    }
    {
        let (state, worker, app_weak) = (state.clone(), worker.clone(), app.as_weak());
        app.on_stack_compare(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let in_stack = {
                let st = state.borrow();
                cell_for(&st, true).is_some_and(|c| stack_of(&st, c).is_some())
            };
            if in_stack {
                // The loupe is where compare shows, as Compare versions
                // has it.
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
    use crate::roots::{View, land_sent, open_view};
    use crate::testing::{press, state_for, window};
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    use slint::Model;
    use slint::platform::{Key, WindowEvent};

    struct Setup {
        dir: PathBuf,
        /// x0 to x4 in the shoot.
        files: Vec<PathBuf>,
        writer: greycard_library::Library,
        app: App,
        state: Rc<RefCell<State>>,
        worker: Rc<Worker>,
    }

    /// A root of a shoot of five frames, indexed, on a window with the
    /// grid up over every root, its collections and stacks kept in the
    /// test's own folder.
    fn setup(what: &str) -> Setup {
        let dir = crate::testing::scratch_dir(&format!("stack-{what}"));
        let root = dir.join("photos");
        let shoot = root.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let files: Vec<PathBuf> = (0..5)
            .map(|i| {
                let p = shoot.join(format!("x{i}.tif"));
                write_frame(&p, [&R5, &R6, &A7][i % 3], i as u16 + 1);
                p
            })
            .collect();
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
            st.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
            // By name over the folders: the frames are told apart by their
            // place in the list this built, in that order.
            st.sort = crate::sort::Kept::by_name();
            st.library.roots.add(&root).unwrap();
            let collections = dir.join(crate::collections::DIR);
            st.collections.dir = Some(collections.clone());
            st.stacks = crate::stacks::Store::load(greycard_library::stacks::dir_in(&collections));
        }
        open_view(&state, &app, &worker, View::Roots(None));
        land_sent(&state, &app, &worker);
        Setup {
            dir,
            files,
            writer,
            app,
            state,
            worker,
        }
    }

    fn teardown(s: Setup) {
        let Setup {
            dir,
            writer,
            app,
            state,
            worker,
            ..
        } = s;
        drop(writer);
        state.borrow_mut().index_reader = None;
        drop(app);
        drop(worker);
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The browser's rows: each frame's name, a version's after a slash,
    /// and its stack's mark, "[3]" on a closed stack's top and "[2/3]" on
    /// an open one's members.
    fn rows(app: &App) -> Vec<String> {
        let thumbs = app.get_thumbs();
        (0..thumbs.row_count())
            .map(|r| {
                let t = thumbs.row_data(r).unwrap();
                let mut s = t.name.to_string();
                if !t.version.is_empty() {
                    s = format!("{s}/{}", t.version);
                }
                match (t.stack, t.stack_open) {
                    (0, _) => s,
                    (n, false) => format!("{s} [{n}]"),
                    (n, true) => format!("{s} [{}/{n}]", t.stack_place),
                }
            })
            .collect()
    }

    /// The own cell of shoot frame `i`.
    fn cell(s: &Setup, i: usize) -> usize {
        let st = s.state.borrow();
        let f = st.file_by_path(&s.files[i]).unwrap();
        st.own_cell(f).unwrap()
    }

    fn row(s: &Setup, c: usize) -> i32 {
        row_of(&s.state.borrow(), c).unwrap() as i32
    }

    /// Frame `on` opened and the frames `also` chosen beside it.
    fn choose(s: &Setup, on: usize, also: &[usize]) {
        let c = cell(s, on);
        s.app.invoke_select(row(s, c));
        let mut picked: Vec<usize> = also.iter().map(|&i| cell(s, i)).collect();
        picked.push(c);
        picked.sort_unstable();
        s.state.borrow_mut().picked = picked;
    }

    /// Each file's own answer to which of its frames the list wants is
    /// the whole list's: what a read landing asks of one file.
    fn wants_agree(s: &Setup) {
        let st = s.state.borrow();
        let all = st.wanted_cells();
        assert_eq!(all, st.cells);
        for f in 0..st.files.len() {
            let whole: Vec<_> = all
                .iter()
                .filter(|c| c.file == f)
                .map(|c| c.version.clone())
                .collect();
            assert_eq!(st.wanted_of_file(f), whole, "{}", st.files[f].display());
            assert!(!crate::rows::versions_differ(&st, f));
        }
    }

    fn with_modifier(app: &App, key: Key, f: impl FnOnce()) {
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        f();
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }

    /// The stacks' files, each as its members' names in order.
    fn on_disk(s: &Setup) -> Vec<Vec<String>> {
        let dir = greycard_library::stacks::dir_in(&s.dir.join(crate::collections::DIR));
        let mut out: Vec<Vec<String>> =
            greycard_library::stacks::read_folder(&dir, greycard_library::stacks::VERSION)
                .list
                .iter()
                .map(|st| {
                    st.refs
                        .iter()
                        .map(|r| {
                            let name = Path::new(&r.hint).file_name().unwrap().to_string_lossy();
                            match &r.version {
                                Some(_) => format!("{name}/v"),
                                None => name.into_owned(),
                            }
                        })
                        .collect()
                })
                .collect();
        out.sort();
        out
    }

    /// Ctrl+G stacks the selection under the frame on screen, closed: its
    /// top alone is listed, counted, and is the selection. S opens it,
    /// the members after the top; Set as top from the menu moves the
    /// stack to where its new top stands; Remove takes a member out, and
    /// a stack down to one goes with its file; Ctrl+Shift+G unstacks.
    /// Each change is on disk, and a new session reads it back.
    #[test]
    fn stack_open_top_remove_and_unstack() {
        let s = setup("make");
        let (app, state) = (&s.app, &s.state);
        assert_eq!(
            rows(app),
            ["x0.tif", "x1.tif", "x2.tif", "x3.tif", "x4.tif"]
        );
        choose(&s, 1, &[3, 4]);
        with_modifier(app, Key::Control, || press(app, "g"));
        assert_eq!(app.get_status(), "3 frames stacked under x1.tif");
        assert_eq!(rows(app), ["x0.tif", "x1.tif [3]", "x2.tif"]);
        assert_eq!(on_disk(&s), [["x1.tif", "x3.tif", "x4.tif"]]);
        wants_agree(&s);
        assert_eq!(chosen_frames(&state.borrow()), [cell(&s, 1)]);

        // S in the grid opens it, the members after the top.
        press(app, "s");
        assert_eq!(
            rows(app),
            [
                "x0.tif",
                "x1.tif [1/3]",
                "x3.tif [2/3]",
                "x4.tif [3/3]",
                "x2.tif"
            ]
        );
        {
            // Out of the folder's order: the lookups by file read the
            // index made with the cells.
            let st = state.borrow();
            assert!(st.cells_by_file.is_some());
            let x3 = st.file_by_path(&s.files[3]).unwrap();
            assert_eq!(st.cells_of_file(x3), [2]);
            assert_eq!(st.own_cell(x3), Some(2));
        }
        wants_agree(&s);

        // x4 on top from its menu: the stack stands where x4 does.
        let x4 = row(&s, cell(&s, 4));
        app.invoke_select(x4);
        app.invoke_frame_menu_asked(x4);
        assert_eq!(app.get_menu_stack(), 3);
        assert!(app.get_menu_stack_open());
        assert!(!app.get_menu_stack_top());
        app.invoke_stack_top(true);
        assert_eq!(
            rows(app),
            [
                "x0.tif",
                "x2.tif",
                "x4.tif [1/3]",
                "x1.tif [2/3]",
                "x3.tif [3/3]"
            ]
        );
        assert_eq!(on_disk(&s), [["x4.tif", "x1.tif", "x3.tif"]]);
        wants_agree(&s);

        // x1 out: a plain frame where it stands.
        app.invoke_select(row(&s, cell(&s, 1)));
        app.invoke_stack_remove();
        assert_eq!(app.get_status(), "1 frame taken out of its stack");
        assert_eq!(
            rows(app),
            ["x0.tif", "x1.tif", "x2.tif", "x4.tif [1/2]", "x3.tif [2/2]"]
        );
        // x3 out: one left is no stack, and its file goes.
        app.invoke_select(row(&s, cell(&s, 3)));
        app.invoke_stack_remove();
        assert_eq!(
            rows(app),
            ["x0.tif", "x1.tif", "x2.tif", "x3.tif", "x4.tif"]
        );
        assert!(on_disk(&s).is_empty());

        // From the menu, and Ctrl+Shift+G.
        choose(&s, 2, &[0]);
        app.invoke_frame_menu_asked(row(&s, cell(&s, 2)));
        assert!(app.get_menu_can_stack());
        app.invoke_stack_make();
        assert_eq!(rows(app), ["x1.tif", "x2.tif [2]", "x3.tif", "x4.tif"]);
        let back = crate::stacks::Store::load(greycard_library::stacks::dir_in(
            &s.dir.join(crate::collections::DIR),
        ));
        assert_eq!(back.list, state.borrow().stacks.list);
        with_modifier(app, Key::Control, || {
            with_modifier(app, Key::Shift, || press(app, "G"))
        });
        assert_eq!(app.get_status(), "x2.tif's stack of 2 unstacked");
        assert_eq!(
            rows(app),
            ["x0.tif", "x1.tif", "x2.tif", "x3.tif", "x4.tif"]
        );
        assert!(on_disk(&s).is_empty());
        teardown(s);
    }

    /// Culling's arrows step over a closed stack as one frame and through
    /// an open one's members; closing it from a member opens its top.
    #[test]
    fn culling_steps_over_a_closed_stack_and_through_an_open_one() {
        let s = setup("cull");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        choose(&s, 1, &[3]);
        app.invoke_stack_make();
        app.set_grid_open(false);
        crate::panel::cull::enter_cull(&mut state.borrow_mut(), app, 1);
        app.invoke_select(row(&s, cell(&s, 0)));
        let on = |st: &State| {
            st.current
                .and_then(|c| st.path_at(c))
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap()
        };
        let mut seen = Vec::new();
        for _ in 0..3 {
            app.invoke_step(1, false);
            seen.push(on(&state.borrow()));
        }
        assert_eq!(seen, ["x1.tif", "x2.tif", "x4.tif"]);
        // S in culling is the proof; the badge opens the stack.
        app.invoke_select(row(&s, cell(&s, 1)));
        press(app, "s");
        assert_eq!(rows(app), ["x0.tif", "x1.tif [2]", "x2.tif", "x4.tif"]);
        app.invoke_stack_badge(row(&s, cell(&s, 1)));
        seen.clear();
        for _ in 0..2 {
            app.invoke_step(1, false);
            seen.push(on(&state.borrow()));
        }
        assert_eq!(seen, ["x3.tif", "x2.tif"]);
        // Closed from x3, a member: x1, its top, is on screen.
        app.invoke_select(row(&s, cell(&s, 3)));
        toggle(&mut state.borrow_mut(), app, worker, None);
        assert_eq!(on(&state.borrow()), "x1.tif");
        assert_eq!(rows(app), ["x0.tif", "x1.tif [2]", "x2.tif", "x4.tif"]);
        teardown(s);
    }

    /// A closed stack is its top to everything that acts on the frames
    /// shown: the filter passes it when its top passes, and the
    /// selection an export or a collection takes is its top alone.
    #[test]
    fn a_closed_stack_is_its_top_to_the_filter_and_the_selection() {
        let s = setup("filter");
        let (app, state) = (&s.app, &s.state);
        choose(&s, 1, &[3]);
        app.invoke_stack_make();
        // The bar counts the frames shown: x3, behind the top, is not.
        assert_eq!(app.get_filter_total(), 4);
        // Add to collection takes the top's frame, not the stack's.
        app.invoke_collection_menu_add(-1);
        assert!(matches!(
            &state.borrow().collections.asked,
            Some(crate::panel::collections::Asked::New { add }) if add.len() == 1
        ));
        state.borrow_mut().collections.asked = None;
        app.set_collection_sheet_open(false);
        // x3, behind the top, rated; the filter at three stars.
        {
            let mut st = state.borrow_mut();
            let x3 = st.file_by_path(&s.files[3]).unwrap();
            st.sidecars[x3].meta.rating = 3;
            st.filter.stars = crate::filter::Stars::AtLeast(3);
        }
        crate::panel::cull::apply_filter(state, app);
        assert!(rows(app).is_empty(), "{:?}", rows(app));
        // Opened, x3 passes on its own.
        let mut st = state.borrow_mut();
        let top = st.own_cell(st.file_by_path(&s.files[1]).unwrap()).unwrap();
        let id = stack_of(&st, top).unwrap().0.id;
        st.stacks.set_open(&id, true);
        settle(&mut st, app, &s.worker);
        assert_eq!(rows(app), ["x3.tif [2/2]"]);
        // Closed again with the top rated: the stack, by its top.
        st.stacks.set_open(&id, false);
        let x1 = st.file_by_path(&s.files[1]).unwrap();
        st.sidecars[x1].meta.rating = 3;
        settle(&mut st, app, &s.worker);
        assert_eq!(rows(app), ["x1.tif [2]"]);
        drop(st);
        teardown(s);
    }

    /// A member gone from the disk is not shown or counted and stays in
    /// the file; the first member there is the top. A frame the window
    /// moves out of the folder (a rejects move, a delete) leaves its
    /// stack. A pass that finds a member renamed carries its path.
    #[test]
    fn a_member_gone_moved_out_or_renamed() {
        let mut s = setup("gone");
        choose(&s, 1, &[2, 4]);
        s.app.invoke_stack_make();
        // x1, the top, deleted by another hand, and the index's pass.
        std::fs::remove_file(&s.files[1]).unwrap();
        let shoot = s.files[0].parent().unwrap().to_path_buf();
        s.writer.index_folder(&shoot, &mut |_| {}).unwrap();
        open_view(&s.state, &s.app, &s.worker, View::Roots(None));
        land_sent(&s.state, &s.app, &s.worker);
        assert_eq!(rows(&s.app), ["x0.tif", "x2.tif [2]", "x3.tif"]);
        assert_eq!(on_disk(&s), [["x1.tif", "x2.tif", "x4.tif"]]);

        // x2 moved out of the folder by the window: it leaves; the
        // stack shows one member and is no stack in the list, kept.
        {
            let mut st = s.state.borrow_mut();
            let x2 = st.file_by_path(&s.files[2]).unwrap();
            crate::panel::cull::drop_files(&mut st, &s.app, &s.worker, &[x2], "moved".into());
        }
        assert_eq!(rows(&s.app), ["x0.tif", "x3.tif", "x4.tif"]);
        assert_eq!(on_disk(&s), [["x1.tif", "x4.tif"]]);

        // x4 renamed by another hand: the pass's move carries it.
        let renamed = s.files[4].with_file_name("x5.tif");
        std::fs::rename(&s.files[4], &renamed).unwrap();
        let report = s.writer.index_folder(&shoot, &mut |_| {}).unwrap();
        assert_eq!(report.moved_files.len(), 1, "{report:?}");
        healed(&s.state, &s.app, &report.moved_files, &report.rehashed);
        assert_eq!(on_disk(&s), [["x1.tif", "x5.tif"]]);
        teardown(s);
    }

    /// A version is a member by its own: on top, it shows beside its
    /// file's own; its file's own stacked under it is behind it.
    #[test]
    fn versions_in_a_stack() {
        let s = setup("versions");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let warm = {
            let mut st = state.borrow_mut();
            let x0 = st.file_by_path(&s.files[0]).unwrap();
            let id = st.sidecars[x0].new_version(None, Some("Warm")).unwrap();
            crate::versions::settle(&mut st, app, worker);
            id
        };
        assert_eq!(
            rows(app),
            [
                "x0.tif",
                "x0.tif/Warm",
                "x1.tif",
                "x2.tif",
                "x3.tif",
                "x4.tif"
            ]
        );
        let warm_cell = || {
            let st = state.borrow();
            let x0 = st.file_by_path(&s.files[0]).unwrap();
            st.cell_of(x0, Some(&warm)).unwrap()
        };
        // Warm on top, with x2.
        let w = warm_cell();
        app.invoke_select(row(&s, w));
        state.borrow_mut().picked = vec![w, cell(&s, 2)];
        app.invoke_stack_make();
        assert_eq!(
            rows(app),
            ["x0.tif", "x0.tif/Warm [2]", "x1.tif", "x3.tif", "x4.tif"]
        );
        assert_eq!(on_disk(&s), [["x0.tif/v", "x2.tif"]]);
        // The file's own stacked with Warm, the closed top, which brings
        // its stack along: x0's own goes behind them.
        let w = warm_cell();
        app.invoke_select(row(&s, w));
        state.borrow_mut().picked = vec![cell(&s, 0), w];
        app.invoke_stack_make();
        assert_eq!(on_disk(&s), [["x0.tif/v", "x2.tif", "x0.tif"]]);
        assert_eq!(rows(app), ["x0.tif/Warm [3]", "x1.tif", "x3.tif", "x4.tif"]);
        wants_agree(&s);
        {
            let st = state.borrow();
            let x0 = st.file_by_path(&s.files[0]).unwrap();
            assert_eq!(st.own_cell(x0), None, "the file's own is behind");
            assert_eq!(st.file_cell(x0), Some(0));
        }
        // Warm gone from the sidecar (a join, a read): its cell asks to
        // go, and the first member left, x2, comes up.
        {
            let mut st = state.borrow_mut();
            let x0 = st.file_by_path(&s.files[0]).unwrap();
            st.sidecars[x0].delete_version(&warm);
            assert!(crate::rows::versions_differ(&st, x0));
            crate::versions::settle(&mut st, app, worker);
        }
        assert_eq!(rows(app), ["x1.tif", "x2.tif [2]", "x3.tif", "x4.tif"]);
        wants_agree(&s);
        teardown(s);
    }

    /// Compare stack opens a closed stack and shows its members side by
    /// side in culling, the view kept to the stack as it changes.
    #[test]
    fn compare_stack_shows_the_members_side_by_side() {
        let s = setup("compare");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        choose(&s, 1, &[2, 3]);
        app.invoke_stack_make();
        let x1 = row(&s, cell(&s, 1));
        app.invoke_frame_menu_asked(x1);
        assert_eq!(app.get_menu_stack(), 3);
        app.invoke_stack_compare();
        let id = {
            let st = state.borrow();
            let cull = st.cull.as_ref().expect("culling");
            assert_eq!(cull.compare, 3);
            cull.stack_of.clone().unwrap()
        };
        assert_eq!(
            state.borrow().cull.as_ref().unwrap().set.as_deref(),
            Some([cell(&s, 1), cell(&s, 2), cell(&s, 3)].as_slice())
        );
        assert!(!app.get_grid_open());
        assert_eq!(app.get_status(), "comparing x1.tif's stack");
        // x2 out of the stack: the view closes up on the two left.
        {
            let mut st = state.borrow_mut();
            st.stacks.remove_at(&id, &[1]);
            crate::versions::settle(&mut st, app, worker);
            let cull = st.cull.as_ref().unwrap();
            assert_eq!(cull.compare, 2);
            assert_eq!(cull.stack_of.as_deref(), Some(id.as_str()));
        }
        assert_eq!(
            state.borrow().cull.as_ref().unwrap().set.as_deref(),
            Some([cell(&s, 1), cell(&s, 3)].as_slice())
        );
        // Unstacked: the view is no longer the stack's.
        {
            let mut st = state.borrow_mut();
            st.stacks.unstack(&id);
            crate::versions::settle(&mut st, app, worker);
            assert_eq!(st.cull.as_ref().unwrap().stack_of, None);
        }
        teardown(s);
    }

    /// Delete version on a member takes its reference out of the stack's
    /// file, as a delete of a file does; the stack's next member comes up.
    #[test]
    fn delete_version_takes_it_out_of_its_stack() {
        let s = setup("delete-version");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let (x0, warm) = {
            let mut st = state.borrow_mut();
            let x0 = st.file_by_path(&s.files[0]).unwrap();
            // Its sidecar in hand, as a version's delete asks.
            st.from_row[x0] = crate::rows::FromRow::read();
            let id = st.sidecars[x0].new_version(None, Some("Warm")).unwrap();
            crate::versions::settle(&mut st, app, worker);
            (x0, id)
        };
        let w = state.borrow().cell_of(x0, Some(&warm)).unwrap();
        app.invoke_select(row(&s, w));
        state.borrow_mut().picked = vec![w, cell(&s, 1), cell(&s, 2)];
        app.invoke_stack_make();
        assert_eq!(on_disk(&s), [["x0.tif/v", "x1.tif", "x2.tif"]]);
        {
            let mut st = state.borrow_mut();
            let w = st.cell_of(x0, Some(&warm)).unwrap();
            crate::panel::versions::ask_delete(&mut st, app, &[w]);
            crate::panel::versions::answered(&mut st, app, worker, 1);
        }
        assert_eq!(on_disk(&s), [["x1.tif", "x2.tif"]]);
        assert_eq!(rows(app), ["x0.tif", "x1.tif [2]", "x3.tif", "x4.tif"]);
        teardown(s);
    }

    /// Two stack files claiming one frame (two editors): the older has
    /// it, the newer is left with one and is no stack, as the command
    /// line has it; a new stack takes its frames out of the others.
    /// The menu counts what Stack would make: a closed top chosen brings
    /// its stack.
    #[test]
    fn a_frame_two_stacks_claim_is_the_older_s() {
        let s = setup("claimed");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        {
            let mut st = state.borrow_mut();
            let hint = |i: usize| greycard_library::collections::hint_of(&s.files[i]);
            let r = |i: usize| Reference {
                hash: format!("h{i}"),
                hint: hint(i),
                version: None,
            };
            let mut older = greycard_library::stacks::Stack::new(vec![r(0), r(2)]);
            let mut newer = greycard_library::stacks::Stack::new(vec![r(2), r(3)]);
            older.made = 10;
            newer.made = 20;
            let dir = st.stacks.dir.clone().unwrap();
            newer.write(&dir).unwrap();
            older.write(&dir).unwrap();
            st.stacks = crate::stacks::Store::load(dir);
            crate::versions::settle(&mut st, app, worker);
        }
        assert_eq!(rows(app), ["x0.tif [2]", "x1.tif", "x3.tif", "x4.tif"]);
        wants_agree(&s);
        // x0's closed stack and x4 chosen: the stack would hold three.
        choose(&s, 0, &[4]);
        app.invoke_frame_menu_asked(row(&s, cell(&s, 0)));
        assert!(app.get_menu_can_stack());
        assert_eq!(app.get_menu_stack_holds(), 3);
        app.invoke_stack_make();
        assert_eq!(app.get_status(), "3 frames stacked under x0.tif");
        // Both gave their frames up and, left with one or none, went.
        assert_eq!(on_disk(&s), [["x0.tif", "x2.tif", "x4.tif"]]);
        assert_eq!(rows(app), ["x0.tif [3]", "x1.tif", "x3.tif"]);
        teardown(s);
    }

    /// A frame the window follows on an archive's copy is stacked, and
    /// found, by its local path.
    #[test]
    fn a_followed_frame_is_stacked_by_its_local_path() {
        let s = setup("followed");
        let (app, state, worker) = (&s.app, &s.state, &s.worker);
        let copy = s.dir.join("archive").join("x1.tif");
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::copy(&s.files[1], &copy).unwrap();
        {
            let mut st = state.borrow_mut();
            let x1 = st.file_by_path(&s.files[1]).unwrap();
            st.move_file(x1, copy.clone());
            crate::sync::follow_for_test(
                &mut st,
                copy.clone(),
                s.files[1].clone(),
                s.dir.join("archive"),
            );
            crate::versions::settle(&mut st, app, worker);
        }
        let x1 = {
            let st = state.borrow();
            st.own_cell(st.file_by_path(&copy).unwrap()).unwrap()
        };
        app.invoke_select(row(&s, x1));
        state.borrow_mut().picked = vec![x1, cell(&s, 2)];
        app.invoke_stack_make();
        assert_eq!(on_disk(&s), [["x1.tif", "x2.tif"]], "{}", app.get_status());
        let hints: Vec<String> = state.borrow().stacks.list[0]
            .refs
            .iter()
            .map(|r| r.hint.clone())
            .collect();
        assert_eq!(
            hints[0],
            greycard_library::collections::hint_of(&s.files[1])
        );
        assert_eq!(rows(app), ["x0.tif", "x1.tif [2]", "x3.tif", "x4.tif"]);
        teardown(s);
    }

    /// A stack is made of one folder's frames, and not in a
    /// collection's list.
    #[test]
    fn a_stack_is_of_one_folder() {
        let mut s = setup("folders");
        let other = s.dir.join("photos").join("other");
        std::fs::create_dir_all(&other).unwrap();
        write_frame(&other.join("y.tif"), &R5, 40);
        s.writer.index_folder(&other, &mut |_| {}).unwrap();
        open_view(&s.state, &s.app, &s.worker, View::Roots(None));
        land_sent(&s.state, &s.app, &s.worker);
        let y = {
            let st = s.state.borrow();
            st.own_cell(st.file_by_path(&other.join("y.tif")).unwrap())
                .unwrap()
        };
        s.app.invoke_select(row(&s, cell(&s, 0)));
        s.state.borrow_mut().picked = vec![cell(&s, 0), y];
        s.app.invoke_stack_make();
        assert_eq!(
            s.app.get_status(),
            "a stack is made of frames in one folder"
        );
        assert!(on_disk(&s).is_empty());
        s.state.borrow_mut().collection_frames = Some(Vec::new());
        s.app.invoke_stack_make();
        assert_eq!(
            s.app.get_status(),
            "a stack is made in a folder's list, not in a collection's"
        );
        s.state.borrow_mut().collection_frames = None;
        teardown(s);
    }
}
