//! Versions in the window: a version of a file is a cell of its own in
//! the browser's list, right after its file's.
//!
//! What is a fact about a file the window keeps once per file: `files`,
//! `sidecars`, `from_row`, the index's rows and answers.
//! Over those it lists cells, `State::cells`, in display order: a file's
//! own cell and then one for each version its sidecar keeps, by
//! position. The strip, the grid, the selection, the arrows, the rubber
//! band, the culling keys, the filter and the lightbox walk cells;
//! `current`, `picked`, `shown`, `menu_file`, `shown_turn` and the
//! thumbnail vectors are by cell. A cell reaches its file's facts
//! through [`State::file_of`] and its own line through [`State::line`]
//! and [`State::line_mut`], which is `sidecars[cell.file].line_mut(
//! cell.version)`. One parsed sidecar a file, so two cells of one file
//! never race their saves, and a save of any cell writes the file once.
//!
//! The cells are made from the sidecars ([`cells_of`]) and made again
//! when a file's versions change: one made, deleted or swapped, a join
//! that brought or took one, a read that found other versions than the
//! row said ([`settle`]). A relist carries each cell that stays (its
//! picture, its place in the selection), as a merge of the list does; a
//! cell whose version went gives its place to its file's own cell.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use greycard_edit::{Line, Sidecar};
#[cfg(not(test))]
use slint::ComponentHandle;

use crate::worker::Worker;
use crate::{App, State};

/// A cell of the browser's list: a file and which version of it, `None`
/// for the file's own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Cell {
    pub(crate) file: usize,
    pub(crate) version: Option<Arc<str>>,
}

/// A frame by what names it across lists and threads: its file's path
/// and its version's id, `None` for the file's own. What the thumbnails'
/// pool, the edit's pictures and a request for sidecars are keyed by.
/// The version's name rides along for the log and the status line,
/// and is no part of the key: two keys are the same frame by path and
/// id whatever name each was made with.
#[derive(Debug, Clone)]
pub(crate) struct Key {
    pub(crate) path: PathBuf,
    pub(crate) version: Option<Arc<str>>,
    pub(crate) name: Option<Arc<str>>,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.version == other.version
    }
}

impl Eq for Key {}

impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.version.hash(state);
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.path, &self.version).cmp(&(&other.path, &other.version))
    }
}

impl Key {
    /// The file's own frame.
    pub(crate) fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            version: None,
            name: None,
        }
    }

    /// Version `version` of the file at `path`, the file's own for none.
    pub(crate) fn of(path: impl Into<PathBuf>, version: Option<&str>) -> Self {
        Self {
            path: path.into(),
            version: version.map(Arc::from),
            name: None,
        }
    }

    /// The version's id, when the frame is one.
    pub(crate) fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// The file's name for the file's own frame, and for a version the
    /// file's stem and the version's name, "IMG_0001 (Warm)", the form
    /// the export names it by (`queue::label_of`); the id stands in for a
    /// name that was not had. What the log and the status line say of
    /// the frame.
    pub(crate) fn said(&self) -> String {
        match &self.version {
            None => crate::panel::browser::file_name(&self.path),
            Some(v) => {
                let stem = self
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!(
                    "{stem} ({})",
                    self.name.as_deref().filter(|n| !n.is_empty()).unwrap_or(v)
                )
            }
        }
    }
}

/// The cells `sidecars` make, in display order: each file's own, then
/// its versions in the sidecar's order.
pub(crate) fn cells_of(sidecars: &[Sidecar]) -> Vec<Cell> {
    let mut cells = Vec::with_capacity(sidecars.len());
    for (file, s) in sidecars.iter().enumerate() {
        cells.push(Cell {
            file,
            version: None,
        });
        cells.extend(s.versions.iter().map(|v| Cell {
            file,
            version: Some(Arc::from(v.id.as_str())),
        }));
    }
    cells
}

/// The cells a collection's frames make over `files` and their
/// `sidecars`, in the collection's order, each with the place in
/// `frames` it came from: each frame whose file is listed and whose
/// version the sidecar has, each once, or with `once` false each as
/// often as it is named (the export queue's, where each is an export of
/// its own). A file may have no cell (its one frame a version gone
/// since), and its cells need not stand together or start with its own.
pub(crate) fn frames_kept(
    files: &[PathBuf],
    sidecars: &[Sidecar],
    frames: &[Key],
    once: bool,
) -> Vec<(Cell, usize)> {
    let at: HashMap<&Path, usize> = files
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_path(), i))
        .collect();
    let mut seen: std::collections::HashSet<Cell> = std::collections::HashSet::new();
    let mut cells = Vec::with_capacity(frames.len());
    for (n, k) in frames.iter().enumerate() {
        let Some(&file) = at.get(k.path.as_path()) else {
            continue;
        };
        if let Some(v) = k.version()
            && sidecars.get(file).is_none_or(|s| s.version(v).is_none())
        {
            continue;
        }
        let cell = Cell {
            file,
            version: k.version.clone(),
        };
        if !once || seen.insert(cell.clone()) {
            cells.push((cell, n));
        }
    }
    cells
}

impl State {
    /// File `file`'s path changed to `to` (followed to an archive's copy,
    /// found moved, a move made): in the list, and in a collection's
    /// frames, which name their files by path, so its cells stay. The
    /// one place a listed file's path is written.
    pub(crate) fn move_file(&mut self, file: usize, to: PathBuf) {
        let Some(at) = self.files.get_mut(file) else {
            return;
        };
        let was = std::mem::replace(at, to.clone());
        // Its companions are taken to have gone with it, as every move
        // of ours takes them; the next read of the list says for sure.
        if let Some(with) = self.views.companions.remove(&was) {
            let with = with
                .iter()
                .filter_map(|c| Some(to.with_file_name(c.file_name()?)))
                .collect();
            self.views.companions.insert(to.clone(), with);
        }
        if let Some(frames) = self.views.collection_frames.as_mut() {
            for k in frames.iter_mut().filter(|k| k.path == was) {
                k.path = to.clone();
            }
        }
    }

    /// The cells the list wants now: a collection's frames while one is
    /// the list, else each file's own and all its versions, the stacks
    /// laid over them (a closed one's members behind its top).
    pub(crate) fn wanted_cells(&self) -> Vec<Cell> {
        self.wanted_cells_with(&crate::stacks::listed(self))
    }

    /// [`State::wanted_cells`] with the stacks `listed` as the list
    /// shows them.
    fn wanted_cells_with(&self, listed: &[crate::stacks::Listed]) -> Vec<Cell> {
        match &self.views.collection_frames {
            Some(frames) => frames_kept(
                &self.files,
                &self.sidecars,
                frames,
                self.views.view != crate::roots::View::Queue,
            )
            .into_iter()
            .map(|(cell, _)| cell)
            .collect(),
            None => crate::stacks::arrange(cells_of(&self.sidecars), listed),
        }
    }

    /// The versions the list wants of file `file`, in its order: none
    /// standing for the file's own.
    pub(crate) fn wanted_of_file(&self, file: usize) -> Vec<Option<Arc<str>>> {
        if self.views.collection_frames.is_some() {
            self.wanted_cells()
                .into_iter()
                .filter(|c| c.file == file)
                .map(|c| c.version)
                .collect()
        } else if self
            .views
            .stacks_listed
            .iter()
            .any(|l| l.members.iter().any(|m| m.file == file))
        {
            crate::stacks::wanted_of_file(self, file)
        } else {
            std::iter::once(None)
                .chain(
                    self.sidecars
                        .get(file)
                        .into_iter()
                        .flat_map(|s| s.versions.iter().map(|v| Some(Arc::from(v.id.as_str())))),
                )
                .collect()
        }
    }

    /// Cell `c`, when the list has one.
    pub(crate) fn cell(&self, c: usize) -> Option<&Cell> {
        self.cells.get(c)
    }

    /// The file cell `c` is of.
    pub(crate) fn file_of(&self, c: usize) -> usize {
        self.cells[c].file
    }

    /// The file cell `c` is of, when the list has the cell.
    pub(crate) fn file_at(&self, c: usize) -> Option<usize> {
        self.cells.get(c).map(|cell| cell.file)
    }

    /// The version cell `c` is, none for its file's own.
    pub(crate) fn version_of(&self, c: usize) -> Option<&str> {
        self.cells.get(c).and_then(|cell| cell.version.as_deref())
    }

    /// Cell `c` by its file's path and its version, the version's name
    /// along.
    pub(crate) fn key_of(&self, c: usize) -> Option<Key> {
        let cell = self.cells.get(c)?;
        Some(Key {
            path: self.files.get(cell.file)?.clone(),
            version: cell.version.clone(),
            name: self.version_name(c).map(Arc::from),
        })
    }

    /// The path of cell `c`'s file.
    pub(crate) fn path_of(&self, c: usize) -> &PathBuf {
        &self.files[self.cells[c].file]
    }

    /// The path of cell `c`'s file, when the list has the cell.
    pub(crate) fn path_at(&self, c: usize) -> Option<&PathBuf> {
        self.files.get(self.cells.get(c)?.file)
    }

    /// The sidecar of cell `c`'s file: the whole file, every line of it.
    pub(crate) fn sidecar(&self, c: usize) -> &Sidecar {
        &self.sidecars[self.cells[c].file]
    }

    /// The sidecar of cell `c`'s file, when the list has the cell.
    pub(crate) fn sidecar_at(&self, c: usize) -> Option<&Sidecar> {
        self.sidecars.get(self.cells.get(c)?.file)
    }

    pub(crate) fn sidecar_mut(&mut self, c: usize) -> &mut Sidecar {
        &mut self.sidecars[self.cells[c].file]
    }

    /// Cell `c`'s own line of its file's sidecar: the file's own for
    /// version 0, the version's otherwise. None when the list has no
    /// such cell, or the sidecar no longer has the version (taken out by
    /// a join or a read since the cells were made; the list follows at
    /// the next turn).
    pub(crate) fn line(&self, c: usize) -> Option<&Line> {
        let cell = self.cells.get(c)?;
        self.sidecars.get(cell.file)?.line(cell.version.as_deref())
    }

    /// In the export queue's view no line of the list is to be written
    /// (`queue_view::frozen`): none. An export's record is made on its
    /// sidecar's line directly, and is not asked.
    pub(crate) fn line_mut(&mut self, c: usize) -> Option<&mut Line> {
        if crate::panel::queue_view::refused(self, "a line of the edit") {
            return None;
        }
        let cell = self.cells.get(c)?.clone();
        self.sidecars
            .get_mut(cell.file)?
            .line_mut(cell.version.as_deref())
    }

    /// The name of the version cell `c` is, none for the file's own.
    pub(crate) fn version_name(&self, c: usize) -> Option<String> {
        let cell = self.cells.get(c)?;
        let id = cell.version.as_deref()?;
        self.sidecars
            .get(cell.file)?
            .version(id)
            .map(|v| v.name.clone())
    }

    /// The cells of file `file`, in the list's order. Outside a
    /// collection they stand together, its own first, unless an open
    /// stack gathered one behind its top (`cells_by_file` says then); in
    /// one they are the collection's frames of it, wherever its order
    /// puts them. Empty for a file not listed.
    pub(crate) fn cells_of_file(&self, file: usize) -> Vec<usize> {
        if self.views.collection_frames.is_none()
            && let Some(by_file) = &self.views.cells_by_file
        {
            return by_file.get(file).cloned().unwrap_or_default();
        }
        if self.views.collection_frames.is_none() {
            let first = self.cells.partition_point(|c| c.file < file);
            let end = self.cells.partition_point(|c| c.file <= file);
            return (first..end).collect();
        }
        (0..self.cells.len())
            .filter(|&c| self.cells[c].file == file)
            .collect()
    }

    /// The cell of version `version` of file `file`, when the list has
    /// it.
    pub(crate) fn cell_of(&self, file: usize, version: Option<&str>) -> Option<usize> {
        self.cells_of_file(file)
            .into_iter()
            .find(|&c| self.cells[c].version.as_deref() == version)
    }

    /// The file's own cell of file `file`, when the list has it: a
    /// collection may hold a version of a file and not the file's own.
    pub(crate) fn own_cell(&self, file: usize) -> Option<usize> {
        if self.views.collection_frames.is_some() || self.views.cells_by_file.is_some() {
            return self.cell_of(file, None);
        }
        // A closed stack may keep the file's own out and its versions in.
        let first = self.cells.partition_point(|c| c.file < file);
        let cell = self.cells.get(first)?;
        (cell.file == file && cell.version.is_none()).then_some(first)
    }

    /// A cell to stand for file `file` where what is done is the file's
    /// (its meta, its turn): its own, else the first of its versions the
    /// list has.
    pub(crate) fn file_cell(&self, file: usize) -> Option<usize> {
        self.own_cell(file)
            .or_else(|| self.cells_of_file(file).first().copied())
    }

    /// The cell `key` names, when the list has it.
    pub(crate) fn find_cell(&self, key: &Key) -> Option<usize> {
        let file = self.files.iter().position(|f| *f == key.path)?;
        self.cell_of(file, key.version.as_deref())
    }

    /// The file `path` is at, when the list has it.
    pub(crate) fn file_by_path(&self, path: &Path) -> Option<usize> {
        self.files.iter().position(|f| f == path)
    }

    /// The files of `cells`, each once, in the order first met: for what
    /// acts on the file and not on a line of it (a move, a delete of the
    /// file, a turn, a write to the archive, a culling key).
    pub(crate) fn files_of(&self, cells: &[usize]) -> Vec<usize> {
        let mut seen = std::collections::HashSet::with_capacity(cells.len());
        let mut out: Vec<usize> = Vec::new();
        for &c in cells {
            if let Some(f) = self.file_at(c)
                && seen.insert(f)
            {
                out.push(f);
            }
        }
        out
    }

    /// The cells of `cells` with their files, each file once by the
    /// first of its cells met, `prefer` standing for its file when it is
    /// among them: for what acts on the file once but is said from a
    /// cell (a turn from the panel's own frame).
    pub(crate) fn once_a_file(&self, cells: &[usize], prefer: Option<usize>) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for &c in cells {
            let Some(f) = self.file_at(c) else {
                continue;
            };
            match out.iter().position(|&o| self.file_at(o) == Some(f)) {
                Some(at) if Some(c) == prefer => out[at] = c,
                Some(_) => {}
                None => out.push(c),
            }
        }
        out
    }
}

/// The cells made again from the sidecars, the list's files where they
/// are: for a change to which versions a file has. Each cell that stays
/// keeps what it holds; the frame on screen stays on screen, or, its
/// version gone, its file's own cell is opened in its place, as a click
/// on it would. True when the list moved.
pub(crate) fn settle(st: &mut State, app: &App, worker: &Worker) -> bool {
    let wanted = st.wanted_cells();
    if wanted == st.cells {
        return false;
    }
    let was_current = st.current;
    let was_file = was_current.and_then(|c| st.file_at(c));
    let compared = compared_file(st);
    relist(st, app, worker, &Some);
    // Which cells show an edit, before any picture is asked for.
    crate::edited::note_list(st, worker);
    let next = crate::panel::browser::rebuild_browser(st, app);
    crate::roots::ask_owed(st, worker);
    if was_current.is_some() && st.current.is_none() {
        // The version on screen went: its file's own cell opens in its
        // place, as a click on it would.
        let row = was_file
            .and_then(|f| st.file_cell(f))
            .and_then(|c| crate::panel::browser::row_of(st, c))
            .or(next);
        if let Some(row) = row {
            crate::panel::browser::open_row(st, app, worker, row as i32, false);
        } else {
            app.set_selected(-1);
            crate::panel::browser::show_file_name(st, app, None);
        }
    } else if st.culling.cull.is_some()
        && let Some(c) = st.current
    {
        crate::panel::cull::cull_select(st, app, c);
    }
    refit_compare(st, app, compared);
    true
}

/// What a named compare is fitted to: a file and its versions, by the
/// file's path, or a stack's members, by the stack's id.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Compared {
    File(PathBuf),
    Stack(String),
}

/// What a versions or a stack's compare is on and how many tiles it
/// shows, to be taken before a relist, which renumbers the cells under
/// it.
pub(crate) fn compared_file(st: &State) -> Option<(Compared, usize)> {
    let cull = st.culling.cull.as_ref()?;
    if let Some(id) = &cull.stack_of {
        return Some((Compared::Stack(id.clone()), cull.compare));
    }
    Some((Compared::File(cull.versions_of.clone()?), cull.compare))
}

/// After a relist, a versions compare fitted to its file's cells as they
/// are now (`compared`, from [`compared_file`] before it), so it closes
/// up on them and never takes in a frame of another file; with the file
/// gone, the view is no longer a versions compare. A stack's compare is
/// fitted to its members the same way. The zoom stands when
/// the count comes back as it was. Every relist with a culling view calls
/// it: `settle`, the list's merge, a drop of files.
pub(crate) fn refit_compare(st: &mut State, app: &App, compared: Option<(Compared, usize)>) {
    match compared {
        Some(_) if st.culling.cull.is_none() => {}
        Some((Compared::File(path), was)) => {
            crate::panel::versions::fit_compare(st, app, &path, Some(was));
        }
        Some((Compared::Stack(id), was)) => {
            crate::panel::stacks::fit_compare(st, app, &id, Some(was));
        }
        None => {}
    }
}

/// [`settle`] at the next turn of the event loop, from a place that
/// holds cell numbers it goes on using (a read landing under a request,
/// a row from the index): the list is not renumbered under it.
pub(crate) fn settle_later(app: &App) {
    #[cfg(test)]
    {
        let _ = app;
        SETTLES_ASKED.with(|n| n.set(n.get() + 1));
    }
    #[cfg(not(test))]
    settle_at_next_turn(app);
}

#[cfg(test)]
thread_local! {
    /// How many settles were asked for the next turn, which a test has
    /// none of: `settle_pending` runs them.
    pub(crate) static SETTLES_ASKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A test's turn of the event loop for the settles asked (`settle_later`):
/// how many were asked, and the list put in step when any was.
#[cfg(test)]
pub(crate) fn settle_pending(st: &mut State, app: &App, worker: &Worker) -> usize {
    let asked = SETTLES_ASKED.with(|n| n.replace(0));
    if asked > 0 {
        settle(st, app, worker);
    }
    asked
}

#[cfg(not(test))]
fn settle_at_next_turn(app: &App) {
    let app_weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let (Some(app), Some(state), Some(worker)) = (
            app_weak.upgrade(),
            crate::STATE.with(|s| s.borrow().clone()),
            crate::WORKER.with(|w| w.borrow().clone()),
        ) else {
            return;
        };
        let mut st = state.borrow_mut();
        settle(&mut st, &app, &worker);
    });
}

/// The cells made again from the sidecars after the files moved:
/// `to_new_file` says where each file of the old list is in the new,
/// `None` for one gone. What each cell that stays holds (its picture,
/// its sizes, its place in the selection, the frame on screen) is
/// carried to its new number; a cell new to the list starts empty, and
/// one gone takes what it held with it. When the numbering moved, what
/// is keyed by it and cannot be carried cheaply (the culling pictures,
/// the camera picture standing in, the prefetch) is put down, for the
/// caller to ask for again. The browser's rows are the caller's to make
/// again. True when the numbering moved.
pub(crate) fn relist(
    st: &mut State,
    app: &App,
    _worker: &Worker,
    to_new_file: &dyn Fn(usize) -> Option<usize>,
) -> bool {
    // The export queue's cells carry by the queued frame each is.
    if let Some((cells, from)) = crate::panel::queue_view::by_id(st) {
        return carry(st, app, _worker, cells, &from);
    }
    let listed = crate::stacks::listed(st);
    let cells = st.wanted_cells_with(&listed);
    st.views.stacks_listed = listed;
    let at: HashMap<Cell, usize> = st
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            Some((
                Cell {
                    file: to_new_file(c.file)?,
                    version: c.version.clone(),
                },
                i,
            ))
        })
        .collect();
    let from: Vec<Option<usize>> = cells.iter().map(|c| at.get(c).copied()).collect();
    carry(st, app, _worker, cells, &from)
}

/// The cells `cells` put in place of the list's, each new cell taking
/// what the old cell `from` names held (its picture, its sizes, its
/// place in the selection, the frame on screen), none for a cell new to
/// the list: [`relist`]'s second half, for a list that knows its cells'
/// old numbers by something else than their file and version (the
/// export queue's, a file queued twice being two cells). True when the
/// numbering moved.
pub(crate) fn carry(
    st: &mut State,
    app: &App,
    _worker: &Worker,
    cells: Vec<Cell>,
    from: &[Option<usize>],
) -> bool {
    let old_len = std::mem::take(&mut st.cells).len();
    let renumbered = old_len != cells.len() || from.iter().enumerate().any(|(i, f)| *f != Some(i));
    let count = cells.len();
    let mut thumb_base = Vec::with_capacity(count);
    let mut thumb_shown = Vec::with_capacity(count);
    let mut thumb_made = Vec::with_capacity(count);
    let mut thumb_asked = Vec::with_capacity(count);
    let mut thumb_failed = Vec::with_capacity(count);
    for f in from {
        match *f {
            Some(i) => {
                thumb_base.push(st.views.thumb_base.get_mut(i).and_then(Option::take));
                thumb_shown.push(st.views.thumb_shown.get(i).copied().flatten());
                thumb_made.push(st.views.thumb_made.get(i).copied().unwrap_or(0));
                thumb_asked.push(
                    st.views
                        .thumb_asked
                        .get(i)
                        .copied()
                        .unwrap_or(crate::worker::THUMB_WIDTH),
                );
                thumb_failed.push(st.views.thumb_failed.get(i).copied().unwrap_or(false));
            }
            None => {
                thumb_base.push(None);
                thumb_shown.push(None);
                thumb_made.push(0);
                thumb_asked.push(crate::worker::THUMB_WIDTH);
                thumb_failed.push(false);
            }
        }
    }
    let mut new_of: Vec<Option<usize>> = vec![None; old_len];
    for (at, f) in from.iter().enumerate() {
        if let Some(i) = *f {
            new_of[i] = Some(at);
        }
    }
    let to_new = |i: usize| new_of.get(i).copied().flatten();
    if renumbered {
        if let Some(cull) = st.culling.cull.as_mut() {
            cull.cache.clear();
            cull.textures.clear();
            cull.clear_failures();
            cull.full_asked = None;
            // A count of the versions compare left out is not this
            // list's, nor its set; a caller that relists takes the
            // compared file first and fits the view to the new cells
            // after (`refit_compare`).
            cull.leave_versions(app);
        }
        crate::panel::cull::drop_placeholder(st, app);
        st.culling.hold = None;
        st.culling.prefetch.want(Vec::new());
        if let Some(run) = st.views.thumb_run.as_mut() {
            run.renumber(from);
        }
    }
    st.current = st.current.and_then(to_new);
    st.views.picked = st.views.picked.iter().filter_map(|&i| to_new(i)).collect();
    st.views.menu_file = st.views.menu_file.and_then(to_new);
    st.develop.shown_turn = st
        .develop
        .shown_turn
        .and_then(|(i, t)| Some((to_new(i)?, t)));
    st.views.thumb_base = thumb_base;
    st.views.thumb_shown = thumb_shown;
    st.views.thumb_made = thumb_made;
    st.views.thumb_asked = thumb_asked;
    st.views.thumb_failed = thumb_failed;
    st.views.cells_by_file = crate::stacks::by_file(&cells, st.files.len());
    st.cells = cells;
    st.views.queue_cell_ids = crate::panel::queue_view::cell_ids_made(st);
    renumbered
}

/// The cells for a new list, nothing carried: the thumbnail vectors
/// sized to them, every picture to be asked for.
pub(crate) fn list(st: &mut State) {
    let listed = crate::stacks::listed(st);
    st.cells = st.wanted_cells_with(&listed);
    st.views.stacks_listed = listed;
    st.views.cells_by_file = crate::stacks::by_file(&st.cells, st.files.len());
    st.views.queue_cell_ids = crate::panel::queue_view::cell_ids_made(st);
    let count = st.cells.len();
    st.views.thumb_base = vec![None; count];
    st.views.thumb_shown = vec![None; count];
    st.views.thumb_made = vec![0; count];
    st.views.thumb_failed = vec![false; count];
    st.views.thumb_asked = vec![crate::worker::THUMB_WIDTH; count];
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::Edit;

    fn with_versions(names: &[&str]) -> Sidecar {
        let mut s = Sidecar::default();
        for n in names {
            s.new_version(None, Some(n)).unwrap();
        }
        s
    }

    /// A file's cells stand together, its own first, then its versions
    /// in the sidecar's order; a file without versions is one cell.
    #[test]
    fn cells_follow_the_sidecars() {
        let sidecars = vec![
            Sidecar::default(),
            with_versions(&["Warm", "Cool"]),
            Sidecar::default(),
        ];
        let cells = cells_of(&sidecars);
        let files: Vec<usize> = cells.iter().map(|c| c.file).collect();
        assert_eq!(files, [0, 1, 1, 1, 2]);
        assert!(cells[0].version.is_none());
        assert!(cells[1].version.is_none());
        assert_eq!(
            cells[2].version.as_deref(),
            Some(sidecars[1].versions[0].id.as_str())
        );
        assert_eq!(
            cells[3].version.as_deref(),
            Some(sidecars[1].versions[1].id.as_str())
        );
        assert!(cells[4].version.is_none());
    }

    /// The lookups by file and by key find the cells where they stand.
    #[test]
    fn a_state_finds_its_cells() {
        let app = crate::testing::window(0);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        let mut st = state.borrow_mut();
        let id = st.sidecars[1].new_version(None, Some("Warm")).unwrap();
        st.cells = cells_of(&st.sidecars);
        assert_eq!(st.cells_of_file(1), (1..3).collect::<Vec<_>>());
        assert_eq!(st.cells_of_file(2), (3..4).collect::<Vec<_>>());
        assert_eq!(st.cells_of_file(7), (4..4).collect::<Vec<_>>());
        assert_eq!(st.own_cell(2), Some(3));
        assert_eq!(st.cell_of(1, Some(&id)), Some(2));
        assert_eq!(st.cell_of(1, Some("nope")), None);
        assert_eq!(st.version_of(2), Some(id.as_str()));
        assert_eq!(st.version_name(2).as_deref(), Some("Warm"));
        assert_eq!(st.file_of(3), 2);
        assert_eq!(st.files_of(&[0, 1, 2, 3, 2]), [0, 1, 2]);
        assert_eq!(st.once_a_file(&[1, 2, 3], Some(2)), [2, 3]);
        assert_eq!(st.once_a_file(&[1, 2, 3], None), [1, 3]);
        let key = st.key_of(2).unwrap();
        assert_eq!(key.version(), Some(id.as_str()));
        assert_eq!(st.find_cell(&key), Some(2));
        assert_eq!(st.find_cell(&Key::file("/nowhere/IMG_0002.CR3")), Some(3));
        let mut e = Edit::default();
        e.light.exposure = 0.5;
        st.line_mut(2).unwrap().record(e);
        assert_eq!(st.line(2).unwrap().current.light.exposure, 0.5);
        assert_eq!(st.line(1).unwrap().current.light.exposure, 0.0);
    }

    /// A collection's cells are its frames in its order: a file's cells
    /// need not stand together or start with its own, a version the
    /// sidecar does not have and a file not listed make none, and each
    /// frame comes once. The lookups find them where they stand.
    #[test]
    fn a_collection_s_cells_are_its_frames_in_its_order() {
        let app = crate::testing::window(0);
        let files = crate::testing::folder(3);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        let mut st = state.borrow_mut();
        let warm = st.sidecars[0].new_version(None, Some("Warm")).unwrap();
        let cool = st.sidecars[2].new_version(None, Some("Cool")).unwrap();
        st.views.collection_frames = Some(vec![
            Key::of(&files[0], Some(&warm)),
            Key::file(&files[1]),
            Key::of(&files[0], None),
            Key::of(&files[0], Some(&warm)),
            Key::of(&files[1], Some("gone00000000000")),
            Key::of(&files[2], Some(&cool)),
            Key::file("/nowhere/else.CR3"),
        ]);
        st.cells = st.wanted_cells();
        let cells: Vec<(usize, Option<&str>)> = st
            .cells
            .iter()
            .map(|c| (c.file, c.version.as_deref()))
            .collect();
        assert_eq!(
            cells,
            [
                (0, Some(warm.as_str())),
                (1, None),
                (0, None),
                (2, Some(cool.as_str()))
            ]
        );
        assert_eq!(st.cells_of_file(0), [0, 2]);
        assert_eq!(st.own_cell(0), Some(2));
        assert_eq!(st.cell_of(0, Some(&warm)), Some(0));
        assert_eq!(st.own_cell(2), None, "the collection holds Cool alone");
        assert_eq!(st.file_cell(2), Some(3));
        assert_eq!(st.wanted_of_file(0), [Some(Arc::from(warm.as_str())), None]);
        // Outside a collection: each file's own and all its versions.
        st.views.collection_frames = None;
        assert_eq!(st.wanted_cells(), cells_of(&st.sidecars));
    }

    /// A relist carries each cell's picture and its place in the
    /// selection to its new number; a cell new to the list starts
    /// empty, and one gone takes its picture with it.
    #[test]
    fn a_relist_carries_the_cells_that_stay() {
        let app = crate::testing::window(0);
        let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        let mut st = state.borrow_mut();
        for i in 0..3 {
            st.views.thumb_base[i] = Some((1, 1, vec![i as u8; 3]));
        }
        st.current = Some(2);
        st.views.picked = vec![1, 2];
        let id = st.sidecars[0].new_version(None, Some("Warm")).unwrap();
        assert!(relist(&mut st, &app, &worker, &Some));
        assert_eq!(st.cells.len(), 4);
        assert_eq!(st.current, Some(3));
        assert_eq!(st.views.picked, [2, 3]);
        assert!(
            st.views.thumb_base[1].is_none(),
            "the new cell has no picture"
        );
        assert_eq!(st.views.thumb_base[2].as_ref().unwrap().2, [1, 1, 1]);
        assert_eq!(st.views.thumb_base[3].as_ref().unwrap().2, [2, 2, 2]);
        // The version deleted: its cell goes, and the frame on screen,
        // another cell, keeps its number's picture.
        st.views.thumb_base[1] = Some((1, 1, vec![9, 9, 9]));
        st.current = Some(1);
        st.sidecars[0].delete_version(&id);
        assert!(relist(&mut st, &app, &worker, &Some));
        assert_eq!(st.cells.len(), 3);
        assert_eq!(st.current, None, "the cell on screen went");
        assert_eq!(st.views.picked, [1, 2]);
        assert_eq!(st.views.thumb_base[1].as_ref().unwrap().2, [1, 1, 1]);
        // Nothing changed: nothing moves.
        assert!(!relist(&mut st, &app, &worker, &Some));
    }

    /// A frame whose file's path changes (followed to its archive's
    /// copy, or found moved by a merge) keeps its cells in a collection:
    /// the frames follow the path with the list.
    #[test]
    fn a_followed_frame_keeps_its_cells_in_a_collection() {
        let app = crate::testing::window(0);
        let files = crate::testing::folder(2);
        let (state, worker) = crate::testing::state_for(&app, files.clone());
        let mut st = state.borrow_mut();
        st.views.collection_frames = Some(vec![Key::file(&files[0]), Key::file(&files[1])]);
        list(&mut st);
        assert_eq!(st.cells.len(), 2);
        let copy = PathBuf::from("/archive/shoot/a.tif");
        st.move_file(0, copy.clone());
        assert_eq!(st.files[0], copy);
        assert!(!crate::rows::versions_differ(&st, 0));
        assert!(!settle(&mut st, &app, &worker));
        assert_eq!(st.cells.len(), 2, "the followed frame's cell stays");
        assert_eq!(st.views.collection_frames.as_ref().unwrap()[0].path, copy);
    }
}
