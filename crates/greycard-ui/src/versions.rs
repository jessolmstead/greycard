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

impl State {
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

    pub(crate) fn line_mut(&mut self, c: usize) -> Option<&mut Line> {
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

    /// The cells of file `file`, its own first: they stand together in
    /// the list. Empty for a file not listed.
    pub(crate) fn cells_of_file(&self, file: usize) -> std::ops::Range<usize> {
        let first = self.cells.partition_point(|c| c.file < file);
        let end = self.cells.partition_point(|c| c.file <= file);
        first..end
    }

    /// The cell of version `version` of file `file`, when the list has
    /// it.
    pub(crate) fn cell_of(&self, file: usize, version: Option<&str>) -> Option<usize> {
        self.cells_of_file(file)
            .find(|&c| self.cells[c].version.as_deref() == version)
    }

    /// The file's own cell of file `file`.
    pub(crate) fn own_cell(&self, file: usize) -> Option<usize> {
        let first = self.cells.partition_point(|c| c.file < file);
        (self.cells.get(first)?.file == file).then_some(first)
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
        let mut out: Vec<usize> = Vec::new();
        for &c in cells {
            if let Some(f) = self.file_at(c)
                && !out.contains(&f)
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
    let wanted = cells_of(&st.sidecars);
    if wanted == st.cells {
        return false;
    }
    let was_current = st.current;
    let was_file = was_current.and_then(|c| st.file_at(c));
    relist(st, app, worker, &Some);
    // Which cells show an edit, before any picture is asked for.
    crate::edited::note_list(st, worker);
    let next = crate::panel::browser::rebuild_browser(st, app);
    crate::roots::ask_owed(st, worker);
    if was_current.is_some() && st.current.is_none() {
        // The version on screen went: its file's own cell opens in its
        // place, as a click on it would.
        let row = was_file
            .and_then(|f| st.own_cell(f))
            .and_then(|c| crate::panel::browser::row_of(st, c))
            .or(next);
        if let Some(row) = row {
            crate::panel::browser::open_row(st, app, worker, row as i32, false);
        } else {
            app.set_selected(-1);
            app.set_file_name("".into());
        }
    } else if st.cull.is_some()
        && let Some(c) = st.current
    {
        crate::panel::cull::cull_select(st, app, c);
    }
    true
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
    let old = std::mem::take(&mut st.cells);
    let cells = cells_of(&st.sidecars);
    let at: HashMap<Cell, usize> = old
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
    let renumbered =
        old.len() != cells.len() || from.iter().enumerate().any(|(i, f)| *f != Some(i));
    let count = cells.len();
    let mut thumb_base = Vec::with_capacity(count);
    let mut thumb_shown = Vec::with_capacity(count);
    let mut thumb_made = Vec::with_capacity(count);
    let mut thumb_asked = Vec::with_capacity(count);
    let mut thumb_failed = Vec::with_capacity(count);
    for f in &from {
        match *f {
            Some(i) => {
                thumb_base.push(st.thumb_base.get_mut(i).and_then(Option::take));
                thumb_shown.push(st.thumb_shown.get(i).copied().flatten());
                thumb_made.push(st.thumb_made.get(i).copied().unwrap_or(0));
                thumb_asked.push(
                    st.thumb_asked
                        .get(i)
                        .copied()
                        .unwrap_or(crate::worker::THUMB_WIDTH),
                );
                thumb_failed.push(st.thumb_failed.get(i).copied().unwrap_or(false));
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
    let mut new_of: Vec<Option<usize>> = vec![None; old.len()];
    for (at, f) in from.iter().enumerate() {
        if let Some(i) = *f {
            new_of[i] = Some(at);
        }
    }
    let to_new = |i: usize| new_of.get(i).copied().flatten();
    if renumbered {
        if let Some(cull) = st.cull.as_mut() {
            cull.cache.clear();
            cull.textures.clear();
            cull.clear_failures();
            cull.full_asked = None;
        }
        crate::panel::cull::drop_placeholder(st, app);
        st.hold = None;
        st.prefetch.want(Vec::new());
        if let Some(run) = st.thumb_run.as_mut() {
            run.renumber(&from);
        }
    }
    st.current = st.current.and_then(to_new);
    st.picked = st.picked.iter().filter_map(|&i| to_new(i)).collect();
    st.menu_file = st.menu_file.and_then(to_new);
    st.shown_turn = st.shown_turn.and_then(|(i, t)| Some((to_new(i)?, t)));
    st.thumb_base = thumb_base;
    st.thumb_shown = thumb_shown;
    st.thumb_made = thumb_made;
    st.thumb_asked = thumb_asked;
    st.thumb_failed = thumb_failed;
    st.cells = cells;
    renumbered
}

/// The cells for a new list, nothing carried: the thumbnail vectors
/// sized to them, every picture to be asked for.
pub(crate) fn list(st: &mut State) {
    st.cells = cells_of(&st.sidecars);
    let count = st.cells.len();
    st.thumb_base = vec![None; count];
    st.thumb_shown = vec![None; count];
    st.thumb_made = vec![0; count];
    st.thumb_failed = vec![false; count];
    st.thumb_asked = vec![crate::worker::THUMB_WIDTH; count];
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
        assert_eq!(st.cells_of_file(1), 1..3);
        assert_eq!(st.cells_of_file(2), 3..4);
        assert_eq!(st.cells_of_file(7), 4..4);
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

    /// A relist carries each cell's picture and its place in the
    /// selection to its new number; a cell new to the list starts
    /// empty, and one gone takes its picture with it.
    #[test]
    fn a_relist_carries_the_cells_that_stay() {
        let app = crate::testing::window(0);
        let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        let mut st = state.borrow_mut();
        for i in 0..3 {
            st.thumb_base[i] = Some((1, 1, vec![i as u8; 3]));
        }
        st.current = Some(2);
        st.picked = vec![1, 2];
        let id = st.sidecars[0].new_version(None, Some("Warm")).unwrap();
        assert!(relist(&mut st, &app, &worker, &Some));
        assert_eq!(st.cells.len(), 4);
        assert_eq!(st.current, Some(3));
        assert_eq!(st.picked, [2, 3]);
        assert!(st.thumb_base[1].is_none(), "the new cell has no picture");
        assert_eq!(st.thumb_base[2].as_ref().unwrap().2, [1, 1, 1]);
        assert_eq!(st.thumb_base[3].as_ref().unwrap().2, [2, 2, 2]);
        // The version deleted: its cell goes, and the frame on screen,
        // another cell, keeps its number's picture.
        st.thumb_base[1] = Some((1, 1, vec![9, 9, 9]));
        st.current = Some(1);
        st.sidecars[0].delete_version(&id);
        assert!(relist(&mut st, &app, &worker, &Some));
        assert_eq!(st.cells.len(), 3);
        assert_eq!(st.current, None, "the cell on screen went");
        assert_eq!(st.picked, [1, 2]);
        assert_eq!(st.thumb_base[1].as_ref().unwrap().2, [1, 1, 1]);
        // Nothing changed: nothing moves.
        assert!(!relist(&mut st, &app, &worker, &Some));
    }
}
