//! The library views' part of the window's state: [`Views`].

use crate::*;

/// The library views: what the browser lists and in what order, the
/// filter and the index behind it, the roots, collections, stacks and
/// folder tree, the selection beside the current frame, the
/// thumbnails of the grid and the strip, and the reads of the frames'
/// sidecars.
pub(crate) struct Views {
    /// The camera's JPEGs beside the listed raws, by the raw's path, as
    /// the last read of the list found them (`greycard_library::pairs`):
    /// the tile's mark, the name's line, the culling loupe's picture.
    /// A move or a delete asks the disk again at the moment it acts.
    /// Empty while `jpegs_apart`.
    pub(crate) companions: HashMap<PathBuf, Vec<PathBuf>>,
    /// Whether a camera's JPEG beside its raw is a frame of its own:
    /// the Settings sheet's switch, `settings.jpegs_apart`.
    pub(crate) jpegs_apart: bool,
    /// While a collection is the list: the frames it shows, each file
    /// and version, in its order. The cells are these, the ones whose
    /// file is listed and whose version its sidecar has, rather than
    /// every file's own and all its versions. None for any other list.
    pub(crate) collection_frames: Option<Vec<versions::Key>>,
    /// Each file's cells, by file, while the cells do not stand as
    /// `versions::cells_of` makes them (each file's together, its own
    /// first, the files in order): an open stack gathers its members
    /// behind its top, and the lookups by file then read this in place
    /// of searching. None while they stand so, a closed stack only
    /// leaving cells out.
    pub(crate) cells_by_file: Option<Vec<Vec<usize>>>,
    /// The stacks as the cells were last made from them: what a file's
    /// own question (`versions::wanted_of_file`) is answered by, without
    /// laying every stack over the whole list again.
    pub(crate) stacks_listed: Vec<stacks::Listed>,
    /// The stacks, kept beside the collections, and what the list
    /// shows of them.
    pub(crate) stacks: stacks::Store,
    /// The browser's selection, as cells: the current frame and
    /// whatever else Ctrl, Shift or Shift and an arrow put beside it.
    /// Read through `selection::frames`, which puts the current frame
    /// back in whatever this holds.
    pub(crate) picked: Vec<usize>,
    /// The browser's rows: the cells it lists, in order, the filter
    /// applied. `selected` on the window is a row; `current` here is
    /// a cell.
    pub(crate) shown: Vec<usize>,
    /// The rows the strip's cells and the grid's were for when their
    /// pictures were last put on and taken off (`settle_pictures`),
    /// first and how many.
    pub(crate) pictured: [(usize, usize); 2],
    pub(crate) filter: filter::Filter,
    /// The library index's thread, which indexes the open folder and
    /// each frame after its sidecar is written; none in a test, or
    /// when the library could not be opened.
    pub(crate) index: Option<library::Indexer>,
    /// The index read on this thread, read-only, for the filter's
    /// EXIF tests and the facets' counts; none until the indexer has
    /// opened the library.
    pub(crate) index_reader: Option<greycard_library::Library>,
    /// Each file's row in the index, where it has one yet.
    pub(crate) index_ids: Vec<Option<i64>>,
    /// Whether the index's tests passed each file when the browser's
    /// list was last made (`filter::Frame::index`).
    pub(crate) index_passed: Vec<bool>,
    /// The folder pass the window last asked for, so a pass over a
    /// folder since left says nothing.
    pub(crate) index_generation: u64,
    /// How far that pass is, while it runs.
    pub(crate) index_progress: Option<(usize, usize)>,
    /// Where the indexer opened the library, for the window's own
    /// read-only connection, tried again while it will not open.
    pub(crate) index_path: Option<PathBuf>,
    /// What the facets' row says instead of its note when the index
    /// is busy or unavailable.
    pub(crate) index_error: Option<String>,
    /// Passes over this folder that have failed in a row; asked
    /// again a few times before the window gives up on it.
    pub(crate) index_tries: u32,
    /// `index_passed` was just answered for this filter, and the list
    /// about to be made need not ask again.
    pub(crate) index_pass_ready: bool,
    /// The facets' counts as last read, kept when a read comes back
    /// busy.
    pub(crate) facets_last: RefCell<Vec<(filter::Facet, Vec<greycard_library::FacetCount>)>>,
    /// The library's roots, their counts and their watcher.
    pub(crate) library: roots::Library,
    /// The collections and smart collections, and what the browser
    /// marks with them.
    pub(crate) collections: collections::Store,
    /// The folder open, as the Open folder button names it, and the
    /// folders opened before it.
    pub(crate) recent: panel::recent::Recent,
    /// The open root's folders as a tree in the left pane, and the
    /// switch that opens a folder with those under it.
    pub(crate) tree: tree::Tree,
    /// What the browser lists: a folder, or the files under the
    /// roots.
    pub(crate) view: roots::View,
    /// The grid's order, each view's choice as kept in the settings:
    /// what a read of the list is sorted by (`sort`).
    pub(crate) sort: sort::Kept,
    /// The order was just chosen: the read it asked for, once merged,
    /// brings the frame on screen into view wherever it went.
    pub(crate) sort_reveal: bool,
    /// Counts every list put in the browser, so a list read on
    /// another thread that arrives after a later one is dropped.
    pub(crate) view_generation: u64,
    /// The requests for frames' sidecars out, in order, and the bar
    /// over their reads (`rows::request`).
    pub(crate) loads: rows::Loads,
    /// The frame picked whose sidecar is on its way: current already,
    /// its panel read-only, and opened when the read lands if it is
    /// still the pick. Nothing of the panel's is saved for it.
    pub(crate) pick_pending: Option<PathBuf>,
    /// The frame whose panel shows the edit standing in from its row,
    /// not its own: a pick whose sidecar was on its way, or a frame
    /// opened while its sidecar could not be had. Nothing of the panel
    /// is recorded for it (`rows::panel_is_frames`) until the frame is
    /// opened on its own sidecar, which `rows::take` does the moment
    /// that sidecar is in.
    pub(crate) panel_stand_in: Option<PathBuf>,
    /// The filmstrip's pictures as the worker made them, unturned, and
    /// the turns and mirror each is shown with on its row. Only the
    /// rows the strip or the grid has a cell for carry their picture;
    /// the rest are `None` here, their pictures kept in `thumb_base`.
    pub(crate) thumb_base: Vec<Option<(u32, u32, Vec<u8>)>>,
    pub(crate) thumb_shown: Vec<Option<(u8, bool)>>,
    /// The long edge each picture was made at, zero until one
    /// arrives, and the long edge each was last asked for; and the
    /// size the grid is asking for now, which follows the cell rather
    /// than climbing with it, so one look at the largest cell does
    /// not make every picture afterwards that size.
    pub(crate) thumb_made: Vec<u32>,
    /// Files no picture could be made of: their cells are marked, and
    /// a snapshot of the grid counts them as filled.
    pub(crate) thumb_failed: Vec<bool>,
    pub(crate) thumb_asked: Vec<u32>,
    pub(crate) thumb_want: u32,
    /// The folder's first round of thumbnails, counted as they come
    /// back, for one line in the log when the last one has.
    pub(crate) thumb_run: Option<panel::browser::ThumbRun>,
    /// When the folder on screen was opened, until the first frame
    /// that draws every row on screen with its picture.
    pub(crate) fill_clock: Option<std::time::Instant>,
    /// When the selection last moved in the develop view, until the
    /// frame that shows the frame it moved to.
    pub(crate) selected_at: Option<std::time::Instant>,
    /// The frames the grid last said it shows, first and last.
    pub(crate) grid_shown: Option<(i32, i32)>,
    /// The rows the filmstrip last said it shows, first and last.
    pub(crate) strip_shown: Option<(i32, i32)>,
    /// The frame the frame menu was opened over, for its Reveal.
    pub(crate) menu_file: Option<usize>,
}
