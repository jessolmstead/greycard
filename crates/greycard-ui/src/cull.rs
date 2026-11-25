//! Culling from the camera's JPEG (notes §80): the previews decoded
//! ahead of the arrow on their own threads, the window of them kept,
//! the rows a filtered browser lists, and the compare view's layout;
//! the move of the rejects to a folder beside the shoot is `rejects`'s.
//!
//! Everything that decides something is pure and lives here rather
//! than in the window's callbacks, so it can be tested without a
//! window: which frames are kept as the index moves, which row a
//! file is on, where a tile goes, which files a move touches.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use greycard_core::raw::Orientation;

use crate::tr;

// The compare view's tiles are laid out where the viewport's fit reads
// them, in the library.
pub use crate::zoom::tile_rects;
#[cfg(test)]
pub(crate) use greycard_engine::camera::FILE_READS;
pub use greycard_engine::camera::camera_picture;
pub(crate) use greycard_engine::camera::{turn_rgb8, turned_size};
// The rejects' move, in the library beside the archive's.
pub use crate::rejects::{
    Moved, Orphans, REJECTS, above_rejects, move_back, move_rejects_as, rejects_dir,
};

pub mod lightbox;

/// [`move_rejects_as`] as the shoot's own folder has it, for the tests:
/// an orphaned sidecar written over, and the camera's JPEG beside a raw
/// going with it. Built whatever the build: the binary's tests use it,
/// and they build this library without cfg(test).
#[doc(hidden)]
pub fn move_rejects(files: &[PathBuf], rejected: &[usize]) -> anyhow::Result<Moved> {
    move_rejects_as(files, rejected, Orphans::WriteOver, false)
}

/// How many frames either side of the current one are decoded ahead
/// and kept: a dozen, so a run of arrows in either direction finds
/// the next frame ready and the memory stays at a few dozen screen-
/// size pictures.
pub const REACH: usize = 12;

/// A preview's long edge under which it is called small: the Canons,
/// the Nikons and the GFX embed the full frame; a Sony ARW carries
/// 1616 wide, a phone's DNG about a thousand. At 1:1 such a preview
/// is shown at its own pixels and the status line says so, rather
/// than its being blown up to the frame's size in silence.
pub const SMALL_LONG_EDGE: u32 = 3000;

/// The rows kept decoded about `current`, held within the folder.
pub fn kept(current: usize, count: usize, reach: usize) -> std::ops::Range<usize> {
    if count == 0 {
        return 0..0;
    }
    let current = current.min(count - 1);
    current.saturating_sub(reach)..(current + reach + 1).min(count)
}

/// The order to decode in: the current row first, then outward one
/// step at a time, the direction of travel before the other at each
/// distance, so the frame the next arrow lands on is the one made
/// next.
pub fn order(current: usize, count: usize, reach: usize, forward: bool) -> Vec<usize> {
    let range = kept(current, count, reach);
    if range.is_empty() {
        return Vec::new();
    }
    let current = current.min(count - 1);
    let mut out = vec![current];
    for d in 1..=reach {
        let (first, second) = if forward {
            (current.checked_add(d), current.checked_sub(d))
        } else {
            (current.checked_sub(d), current.checked_add(d))
        };
        for i in [first, second].into_iter().flatten() {
            if range.contains(&i) {
                out.push(i);
            }
        }
    }
    out
}

/// The memory the screen-size previews may take together, so the
/// window shrinks under a large view rather than the process growing
/// with it: a dozen either side at 1410 px is about 130 MB at most,
/// and the same count at a 2560 px view would be over 400.
pub const BUDGET_BYTES: usize = 256 << 20;

/// Previews are numbered as they are made, so a texture made from
/// one can tell it from the next copy of the same frame.
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The camera's picture, turned as the camera says, as the GPU takes
/// it: RGBA bytes, encoded sRGB.
pub struct Preview {
    /// Its number, unique for the process.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The JPEG's own size, turned: what 1:1 means for this frame,
    /// whatever size this copy was made at.
    pub source: (u32, u32),
    /// This copy was asked for at the JPEG's own size, and goes in
    /// its frame's full-size slot of the cache. A screen-size request
    /// on a small JPEG comes back at its own size too, and is a window
    /// preview all the same: the slot is the request's, not the
    /// picture's, or a frame no larger than the view would never be
    /// shown (see `Cache`).
    pub full: bool,
    /// This is the frame's local preview from the cache, not the
    /// camera's JPEG read from the file: the file is out of reach, or
    /// still on its way over the network.
    pub local: bool,
    /// This is the picture made from the frame's edit, kept in the
    /// cache (`edited`): what a frame with an edit stands in with for
    /// its develop and is compared with its versions on. It
    /// stands where the camera's JPEG would at the view's size, and is
    /// never taken for the JPEG's every pixel, so 1:1 still opens the
    /// camera's full JPEG for focus.
    pub edited: bool,
    /// Decoded from the camera's JPEG beside the raw rather than the
    /// preview inside it: the camera's finished picture, full size.
    pub companion: bool,
    /// Asked for as the camera's picture alone (`Want::camera`): never
    /// the edit's, whether or not one is kept. A picture asked the other
    /// way is the edit's where one is kept and the camera's where not.
    pub camera_only: bool,
    /// Made by a quick develop of a raw that carries no camera picture
    /// to read, as its local preview is made. Named as the local
    /// preview is, never as the camera's JPEG, and its own every pixel.
    pub developed: bool,
    /// Cut to the camera's frame (`cut_to_camera`): not the picture of
    /// the whole sensor a frame with a crop of its own stands in with.
    pub cut: bool,
    /// How long the decode took, for the log.
    pub seconds: f64,
}

impl Preview {
    /// A preview of `width` by `height` of a JPEG of `source`, the
    /// next number.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>, source: (u32, u32), full: bool) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            width,
            height,
            rgba,
            source,
            full,
            local: false,
            edited: false,
            companion: false,
            camera_only: false,
            developed: false,
            cut: false,
            seconds: 0.0,
        }
    }

    /// The picture made from a frame's edit, as the cache keeps it.
    pub fn edited(thumb: greycard_library::thumbs::Thumb) -> Self {
        let mut preview = Self::local(thumb, false);
        preview.local = false;
        preview.edited = true;
        preview
    }

    /// Whether this is the kind of picture a cell wants: the camera's
    /// (`camera`) is any picture not the edit's, and the other kind
    /// any asked for the edit's, which is the camera's where no edit's
    /// is kept. A picture of the wrong kind stays up until the right
    /// one lands, and is not taken in in its place.
    pub fn fits(&self, camera: bool) -> bool {
        if camera {
            !self.edited
        } else {
            !self.camera_only
        }
    }

    /// Where this picture came from, for the words that name it.
    pub fn origin(&self) -> Origin {
        if self.edited {
            Origin::Edit
        } else if self.local || self.developed {
            Origin::Local
        } else if self.companion {
            Origin::Companion
        } else {
            Origin::Camera
        }
    }

    /// A frame's local preview as the cache keeps it, as the GPU takes
    /// it: its own pixels are all there is of it, so 1:1 is those.
    pub fn local(thumb: greycard_library::thumbs::Thumb, full: bool) -> Self {
        let mut rgba = Vec::with_capacity(thumb.rgb.len() / 3 * 4);
        for px in thumb.rgb.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        let mut preview = Self::new(
            thumb.width,
            thumb.height,
            rgba,
            (thumb.width, thumb.height),
            full,
        );
        preview.local = true;
        preview
    }

    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }

    /// The preview is smaller than a camera's full frame would be. A
    /// local preview, or a develop made as one is, is small by design
    /// and says so in its own words.
    pub fn small(&self) -> bool {
        !self.local
            && !self.edited
            && !self.developed
            && self.source.0.max(self.source.1) < SMALL_LONG_EDGE
    }

    /// This copy is the JPEG's every pixel: 1:1 needs no other.
    pub fn own_size(&self) -> bool {
        (self.width, self.height) == self.source
    }
}

/// Where a picture the loupe shows came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The camera's JPEG, or the picture itself, from the file.
    Camera,
    /// The camera's JPEG beside the raw, from that file.
    Companion,
    /// The frame's local preview, from the cache.
    Local,
    /// The picture made from the frame's edit, from the cache.
    Edit,
}

/// How many screen-size previews of `size` on the long edge a budget
/// holds: a frame's RGBA is under three times the square of its long
/// edge whichever way it lies.
pub fn frames_within(budget: usize, size: u32) -> usize {
    let each = 3 * size as usize * size as usize;
    (budget / each.max(1)).max(1)
}

/// The reach either side of the selection the budget allows, no
/// more than [`REACH`].
pub fn reach_for(size: u32) -> usize {
    (frames_within(BUDGET_BYTES, size).saturating_sub(1) / 2).clamp(1, REACH)
}

/// The previews kept in memory, by file index: the screen-size ones
/// of the window about the current frame, and at 1:1 the full-size
/// ones of the frames the view shows, at most one a tile.
#[derive(Default)]
pub struct Cache {
    previews: HashMap<usize, Arc<Preview>>,
    full: HashMap<usize, Arc<Preview>>,
}

/// The full-size copies a cache let go, by cell and the copy's number:
/// the textures made from them go too.
pub type Dropped = Vec<(usize, u64)>;

impl Cache {
    pub fn get(&self, cell: usize) -> Option<&Arc<Preview>> {
        self.previews.get(&cell)
    }

    /// The full-size preview of `file`, if one is held.
    pub fn full(&self, cell: usize) -> Option<&Arc<Preview>> {
        self.full.get(&cell)
    }

    /// The best copy there is of `file`: its full one when one is
    /// held, else the screen-size one.
    pub fn best(&self, cell: usize) -> Option<&Arc<Preview>> {
        self.full(cell).or_else(|| self.get(cell))
    }

    /// Whether a copy of `file` at the JPEG's every pixel is held:
    /// a full-size one, or a window preview of a JPEG no larger than
    /// the view. A local preview is its own every pixel and not the
    /// JPEG's, so it is not: at 1:1 the file's is still asked for
    /// where the file can be read.
    pub fn has_own_size(&self, cell: usize) -> bool {
        self.best(cell)
            .is_some_and(|p| p.own_size() && !p.local && !p.edited)
    }

    pub fn insert(&mut self, cell: usize, preview: Arc<Preview>) {
        if preview.full {
            self.full.insert(cell, preview);
        } else {
            self.previews.insert(cell, preview);
        }
    }

    /// Drop everything outside `files`, and the full-size copies
    /// outside `shown`, the frames the view shows: the window moved on.
    pub fn keep(&mut self, files: &[usize], shown: &[usize]) -> Dropped {
        self.previews.retain(|f, _| files.contains(f));
        self.keep_full(shown)
    }

    /// Drop the full-size copies but those of `shown`: the selection
    /// moved, and a frame no longer shown needs none.
    pub fn keep_full(&mut self, shown: &[usize]) -> Dropped {
        let gone: Dropped = self
            .full
            .iter()
            .filter(|(f, _)| !shown.contains(f))
            .map(|(&f, p)| (f, p.id))
            .collect();
        for (f, _) in &gone {
            self.full.remove(f);
        }
        gone
    }

    /// Drop every full-size copy: the view is fitted again.
    pub fn drop_full(&mut self) -> Dropped {
        self.keep_full(&[])
    }

    pub fn clear(&mut self) {
        self.previews.clear();
        self.full.clear();
    }

    /// Drop `file`'s view-size copy: its picture changed (its edit's
    /// was kept), and the next refresh asks for it again.
    pub fn forget(&mut self, cell: usize) {
        self.previews.remove(&cell);
    }

    /// Drop both of `cell`'s copies, the view-size one and the full one:
    /// its file was written again, and the camera's picture with it.
    /// The full copy let go, for its texture.
    pub fn forget_file(&mut self, cell: usize) -> Dropped {
        self.previews.remove(&cell);
        self.full
            .remove(&cell)
            .map(|p| vec![(cell, p.id)])
            .unwrap_or_default()
    }

    /// Keep the screen-size previews `keep` says yes to, by cell.
    pub fn retain(&mut self, mut keep: impl FnMut(usize, &Preview) -> bool) {
        self.previews.retain(|&c, p| keep(c, p));
    }

    /// What is held, in bytes.
    pub fn bytes(&self) -> usize {
        self.previews.values().map(|p| p.bytes()).sum::<usize>()
            + self.full.values().map(|p| p.bytes()).sum::<usize>()
    }

    /// How many screen-size previews are held.
    pub fn count(&self) -> usize {
        self.previews.len()
    }
}

/// The browser's row of `file`, when the filter shows it at all:
/// `shown` is the files it shows, in order, so the row is where the
/// file sits in it. None for a file the filter hides.
pub fn row_of_shown(shown: &[usize], file: usize) -> Option<usize> {
    shown.binary_search(&file).ok()
}

/// The row of `file` in `shown`, or the row of the nearest file that
/// is shown: the one after it, else the one before. None for an
/// empty list.
pub fn nearest_row(shown: &[usize], file: usize) -> Option<usize> {
    if shown.is_empty() {
        return None;
    }
    let at = shown.partition_point(|&f| f < file);
    Some(at.min(shown.len() - 1))
}

/// The tile at a point of the view, physical pixels.
pub fn tile_at(width: u32, height: u32, n: usize, x: f32, y: f32) -> Option<usize> {
    tile_rects(width, height, n)
        .iter()
        .position(|&(tx, ty, tw, th)| {
            x >= tx as f32 && y >= ty as f32 && x < (tx + tw) as f32 && y < (ty + th) as f32
        })
}

/// The first row of a compare set of `n` that holds `current`: the
/// set as it was when the selection is still inside it, else slid
/// the least that brings the selection in, and never past the end of
/// the folder.
pub fn anchor_for(anchor: usize, current: usize, n: usize, count: usize) -> usize {
    let n = n.max(1);
    if count == 0 {
        return 0;
    }
    let last_start = count.saturating_sub(n);
    let anchor = anchor.min(last_start);
    if current < anchor {
        current.min(last_start)
    } else if current >= anchor + n {
        (current + 1).saturating_sub(n).min(last_start)
    } else {
        anchor
    }
}

/// The rows a compare set shows, from its anchor.
pub fn compare_rows(anchor: usize, n: usize, count: usize) -> Vec<usize> {
    (anchor..(anchor + n.max(1)).min(count)).collect()
}

/// The frames a view shows in the order their full-size copies are
/// asked for at 1:1: the chosen one, then the others in tile order.
pub fn chosen_first(chosen: usize, shown: &[usize]) -> Vec<usize> {
    std::iter::once(chosen)
        .chain(shown.iter().copied().filter(|&f| f != chosen))
        .collect()
}

/// What a decode came back with.
pub enum Loaded {
    Ok {
        file: usize,
        frame: crate::version_key::Key,
        /// The long edge it was asked at, zero for the JPEG's own.
        size: u32,
        preview: Arc<Preview>,
    },
    Failed {
        file: usize,
        frame: crate::version_key::Key,
        size: u32,
        message: String,
    },
    /// A local preview asked for is not kept.
    NoPreview {
        file: usize,
        frame: crate::version_key::Key,
    },
}

/// One decode wanted: the file, its path, the long edge to make it
/// at (zero for the JPEG's own size), and where the picture comes
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub file: usize,
    pub path: PathBuf,
    /// The version of the file the cell is, none for the file's own:
    /// which line's edit the picture is of.
    pub version: Option<std::sync::Arc<str>>,
    pub size: u32,
    pub from: Source,
    /// A decode for the culling loupe, which makes the frame's local
    /// preview from the picture in hand when it is owed one. The
    /// develop view's is not: its cores are the develop's.
    pub make_preview: bool,
    /// A tile of the lightbox: the picture of the frame's edit when it
    /// has one, and never the camera's then (`lightbox::fetch`).
    pub lightbox: bool,
    /// The camera's picture alone, never the picture of the frame's
    /// edit: the loupe in Cull on the camera's JPEG, which is what its
    /// switch says it shows. The develop view's stand-in and a versions
    /// compare's tiles take the edit's where one is kept.
    pub camera: bool,
    /// The file the camera's picture is read from when it is not
    /// `path`: the camera's JPEG beside the raw, full size and the
    /// camera's finished picture, where the raw's own preview may be a
    /// small one.
    pub picture: Option<PathBuf>,
    /// A raw with no camera picture in it is developed quickly for
    /// this want ([`developed_picture`]): the culling loupe's and the
    /// lightbox's. The develop view's stand-in is not: the develop it
    /// stands in for is on its way.
    // Its link names this module's private items, for a reader here.
    #[allow(rustdoc::private_intra_doc_links)]
    pub develop: bool,
    /// The camera's picture whole, never cut to the camera's frame: the
    /// frame's edit stores a crop of its own, made on the whole sensor,
    /// which the picture standing in for its develop must show.
    pub whole: bool,
}

impl Want {
    /// A decode of the file itself: the camera's JPEG, or the picture.
    pub fn of_file(file: usize, path: PathBuf, size: u32) -> Self {
        Self {
            file,
            path,
            version: None,
            size,
            from: Source::File,
            make_preview: false,
            lightbox: false,
            camera: false,
            picture: None,
            develop: false,
            whole: false,
        }
    }

    /// The same want, of the camera's picture alone when `camera`.
    pub fn camera_only(self, camera: bool) -> Self {
        Self { camera, ..self }
    }

    /// The same want, the camera's picture read from `picture` (the
    /// JPEG beside the raw) when there is one.
    pub fn with_picture(self, picture: Option<PathBuf>) -> Self {
        Self { picture, ..self }
    }

    /// The same want, the camera's picture whole when `whole`
    /// ([`Want::whole`]).
    pub fn whole(self, whole: bool) -> Self {
        Self { whole, ..self }
    }

    /// The same want, of version `version` of the file.
    pub fn of_version(self, version: Option<std::sync::Arc<str>>) -> Self {
        Self { version, ..self }
    }

    /// The frame the picture is of: the file and its version.
    pub fn frame(&self) -> crate::version_key::Key {
        crate::version_key::Key {
            path: self.path.clone(),
            version: self.version.clone(),
            name: None,
        }
    }

    /// A decode of the file for the culling loupe.
    pub fn for_loupe(file: usize, path: PathBuf, size: u32) -> Self {
        Self {
            make_preview: true,
            develop: true,
            ..Self::of_file(file, path, size)
        }
    }

    /// The local preview alone, for the loupe.
    pub fn local(file: usize, path: PathBuf, size: u32, (hash, stamp): (String, u64)) -> Self {
        Self {
            from: Source::Preview { hash, stamp },
            ..Self::of_file(file, path, size)
        }
    }

    /// The same picture of the same file from the same place: one a
    /// thread is on is not begun again.
    fn same(&self, other: &Want) -> bool {
        self.file == other.file
            && self.size == other.size
            && self.path == other.path
            && self.version == other.version
            && self.from == other.from
            && self.lightbox == other.lightbox
            && self.camera == other.camera
            && self.picture == other.picture
            && self.develop == other.develop
            && self.whole == other.whole
    }

    /// Whether a thread on `in_flight` will deliver what this want
    /// asks for, so this one is not begun: the same want, or the same
    /// but for a develop of a raw with no camera picture, which the
    /// one in flight makes and this one would not. One way only: a
    /// want that would develop is begun beside one that would not.
    fn covered_by(&self, in_flight: &Want) -> bool {
        self.same(&Want {
            develop: self.develop,
            ..in_flight.clone()
        }) && (in_flight.develop || !self.develop)
    }
}

/// Where the loupe's picture of a frame comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The file: its camera JPEG, or the picture itself.
    File,
    /// The local preview alone, under the file's hash and stamp:
    /// nothing of the file is read.
    Preview { hash: String, stamp: u64 },
}

/// Where the loupe takes a frame's picture from: the local preview
/// under this key, the file, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub preview: Option<(String, u64)>,
    pub file: bool,
}

/// Where a frame's picture comes from, by its root: the file on a
/// local disk; the local preview alone under an offline root (nothing
/// at all when there is no key to find it by); the local preview and
/// the file under a root on a network mount, the preview wanted ahead
/// of every file there, so the window's previews are all up before any
/// read goes over the share. With no key there, only the file: the
/// key would be read from the file's head over the share, which is the
/// read the preview is there to go ahead of.
pub fn source_for(offline: bool, remote: bool, key: Option<(String, u64)>) -> Plan {
    Plan {
        preview: if offline || remote { key } else { None },
        file: !offline,
    }
}

/// What the loupe says of a frame out of reach with no local preview
/// kept, in place of a decode's failure.
/// In English, marked for the catalog: kept and compared as it is,
/// translated where it is shown.
pub const NO_LOCAL_PREVIEW: &str =
    crate::marked::tr!("its root is offline, and no local preview of it is kept");

#[derive(Default)]
struct Queue {
    wanted: VecDeque<Want>,
    /// The decodes a thread has taken and not yet delivered: a list
    /// that names one again does not start it twice.
    in_flight: Vec<Want>,
    /// The prefetcher was dropped: the threads end at their next look.
    closed: bool,
}

type Deliver = Arc<dyn Fn(Loaded) + Send + Sync>;

/// The previews decoded on threads of their own: the worker is busy
/// with the folder's thumbnails, and a frame under the arrow cannot
/// wait behind them. The list of what is wanted is replaced whole on
/// every move; a thread takes the front of it.
pub struct Prefetcher {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    previews: Previewed,
    /// Whether a test armed the two below ([`Prefetcher::record`]):
    /// never in a shipping build, which keeps nothing of either.
    recording: AtomicBool,
    /// The last list wanted, for the tests to read what was asked.
    asked: Mutex<Vec<Want>>,
    /// Every decode put at the front, for the tests.
    pushed: Mutex<Vec<Want>>,
}

/// The local previews, once the editor has a cache to keep them in.
type Previewed = Arc<Mutex<Option<Arc<crate::previews::Previews>>>>;

impl Prefetcher {
    /// Two or three threads: a decode is single-threaded and forty
    /// to a hundred milliseconds, and a run of arrows wants the
    /// window filled at a few times that rate.
    pub fn new(deliver: impl Fn(Loaded) + Send + Sync + 'static) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let deliver: Deliver = Arc::new(deliver);
        let previews: Previewed = Arc::new(Mutex::new(None));
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 4).clamp(1, 3))
            .unwrap_or(1);
        for k in 0..threads {
            let (q, d, p) = (queue.clone(), deliver.clone(), previews.clone());
            std::thread::Builder::new()
                .name(format!("greycard cull {k}"))
                .spawn(move || run(q, d, p))
                .expect("spawning a cull thread");
        }
        Self {
            queue,
            previews,
            recording: AtomicBool::new(false),
            asked: Mutex::new(Vec::new()),
            pushed: Mutex::new(Vec::new()),
        }
    }

    /// Whether every thread has let go of the queue, asked at any time
    /// after: true once they have ended.
    #[cfg(test)]
    pub(crate) fn threads_gone(&self) -> impl Fn() -> bool + 'static {
        let queue = Arc::downgrade(&self.queue);
        move || queue.upgrade().is_none()
    }

    /// Keeps what is asked of this prefetcher from now on, for
    /// [`Prefetcher::asked`] and [`Prefetcher::pushed`]. For the tests;
    /// built whatever the build, since the binary's tests use it and
    /// they build this library without cfg(test). Nothing arms it
    /// outside a test, so a culling session keeps nothing.
    #[doc(hidden)]
    pub fn record(&self) {
        self.recording.store(true, Ordering::Relaxed);
    }

    /// The last list wanted since [`Prefetcher::record`], which a test
    /// must have called: one that did not is told so, rather than
    /// reading an empty list.
    #[doc(hidden)]
    pub fn asked(&self) -> Vec<Want> {
        assert!(self.recording(), "Prefetcher::record was not called");
        self.asked.lock().expect("cull asked").clone()
    }

    /// Every decode put at the front since [`Prefetcher::record`], as
    /// [`Prefetcher::asked`] has it.
    #[doc(hidden)]
    pub fn pushed(&self) -> Vec<Want> {
        assert!(self.recording(), "Prefetcher::record was not called");
        self.pushed.lock().expect("cull pushed").clone()
    }

    fn recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// The local previews to read and to make: the worker's.
    pub fn set_previews(&self, previews: Arc<crate::previews::Previews>) {
        *self.previews.lock().expect("cull previews") = Some(previews);
    }

    /// What to decode from now on, in this order: the whole of what
    /// is wanted and not yet had, every time. Whatever was queued
    /// before and is not here is forgotten; one a thread is already
    /// on is not started again, and arrives as it was going to.
    pub fn want(&self, list: Vec<Want>) {
        if self.recording() {
            *self.asked.lock().expect("cull asked") = list.clone();
        }
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("cull queue");
        q.wanted = list
            .into_iter()
            .filter(|w| !q.in_flight.iter().any(|f| w.covered_by(f)))
            .collect();
        cv.notify_all();
    }

    /// One decode before everything else wanted: a full-size copy of
    /// a frame just taken to 1:1.
    pub fn push_front(&self, want: Want) {
        if self.recording() {
            self.pushed.lock().expect("cull pushed").push(want.clone());
        }
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("cull queue");
        if q.in_flight.iter().any(|f| want.covered_by(f)) {
            return;
        }
        q.wanted.retain(|w| !w.same(&want));
        q.wanted.push_front(want);
        cv.notify_one();
    }
}

/// A prefetcher dropped closes its queue: each thread finishes the
/// decode in hand and ends, and what was still wanted is dropped, since
/// nobody is left to show it. Without it every prefetcher made leaves
/// its threads asleep on the queue for the life of the process, and a
/// run of tests that each make one meets the thread cap macOS sets per
/// process. No join: a decode can take a second, and the window's
/// thread is the one dropping it.
impl Drop for Prefetcher {
    fn drop(&mut self) {
        let (lock, cv) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        cv.notify_all();
    }
}

fn run(queue: Arc<(Mutex<Queue>, Condvar)>, deliver: Deliver, previews: Previewed) {
    let (lock, cv) = &*queue;
    loop {
        let want = {
            let mut q = lock.lock().expect("cull queue");
            loop {
                if q.closed {
                    return;
                }
                if let Some(w) = q.wanted.pop_front() {
                    q.in_flight.push(w.clone());
                    break w;
                }
                q = cv.wait(q).expect("cull queue");
            }
        };
        let previews = previews.lock().expect("cull previews").clone();
        let delivered = std::cell::Cell::new(false);
        let fetched = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fetch(&want, previews.as_ref(), &|loaded| {
                delivered.set(true);
                deliver(loaded)
            })
        }));
        lock.lock()
            .expect("cull queue")
            .in_flight
            .retain(|w| !w.same(&want));
        // A panic after a picture was delivered leaves that picture up.
        if fetched.is_err() && !delivered.get() {
            deliver(Loaded::Failed {
                file: want.file,
                frame: want.frame(),
                size: want.size,
                message: tr!("the decode panicked"),
            });
        }
    }
}

/// The camera's picture of the raw at `path` cut to the camera's frame,
/// where the camera framed less than the raw keeps, as the thumbnails
/// are (`worker::thumbs::cut_to_camera`): what the loupe, the lightbox,
/// and the camera's picture standing in for a develop show of a frame,
/// so the develop framed by the same frame lands where it stood. The
/// camera's JPEG beside the raw is the camera's own cut already, and the
/// edit's picture is cut by its edit; both are as they are, as is any
/// picture not of the sensor's shape (`CameraFrame::cut_shown`), and
/// one asked for `whole`. The frame is read from the file's head only
/// with `read` (the file was read for the picture); a local preview
/// takes it as the index's row told it. Its 1:1 size is cut with it.
pub(crate) fn cut_to_camera(preview: Preview, path: &Path, read: bool, whole: bool) -> Preview {
    if preview.edited || preview.companion || whole {
        return preview;
    }
    let camera = if read {
        crate::edited_pictures::camera_of(path)
    } else {
        crate::edited_pictures::camera_known(path)
    };
    let Some(frame) = camera else {
        return preview;
    };
    if !frame.shows_whole_area(preview.width, preview.height) {
        return preview;
    }
    let (w, h, rgba) = frame.cut_shown(preview.width, preview.height, &preview.rgba, 4);
    let (sw, sh, _) = frame.cut_shown(preview.source.0, preview.source.1, &[], 0);
    Preview {
        width: w,
        height: h,
        rgba,
        source: (sw, sh),
        cut: true,
        ..preview
    }
}

/// Whether the picture decoded for `path` is kept as its local
/// preview: not the camera's JPEG beside a raw the camera framed, as
/// far as that is known (nothing of the raw is read for it, only the
/// JPEG was), which is the camera's cut and not the raw's picture of
/// the whole sensor the local preview is cut from. That one is left
/// owed, for the pool to make from the raw's own picture.
fn keeps_as_preview(companion: bool, path: &Path) -> bool {
    !(companion && crate::edited_pictures::camera_known(path).is_some())
}

/// One picture wanted, from where it says, handed to `deliver`: the
/// local preview from the cache, which reads nothing of the file, or
/// the file's. A picture decoded from the file for the loupe makes the
/// frame's local preview when it is owed one, on another thread, so
/// this one goes on to the next decode at once.
pub fn fetch(
    want: &Want,
    previews: Option<&Arc<crate::previews::Previews>>,
    deliver: &dyn Fn(Loaded),
) {
    if want.lightbox {
        return lightbox::fetch(want, previews, deliver);
    }
    let edited = previews.and_then(|p| p.edited()).filter(|_| !want.camera);
    if let Source::Preview { hash, stamp } = &want.from {
        // An edited frame out of reach shows the last picture of its
        // edit kept for its content, ahead of the camera's.
        let from_edit = edited.filter(|_| want.size > 0).and_then(|e| {
            e.lookup(
                hash,
                crate::edited_pictures::BIG,
                e.shows_offline(&want.frame()),
            )
        });
        if let Some(thumb) = from_edit {
            deliver(Loaded::Ok {
                file: want.file,
                frame: want.frame(),
                size: want.size,
                preview: Arc::new(Preview::edited(thumb)),
            });
            return;
        }
        deliver(
            match previews.and_then(|p| p.get(&want.path, hash, *stamp)) {
                Some(thumb) => Loaded::Ok {
                    file: want.file,
                    frame: want.frame(),
                    size: want.size,
                    preview: Arc::new(cut_to_camera(
                        Preview {
                            camera_only: want.camera,
                            ..Preview::local(thumb, want.size == 0)
                        },
                        &want.path,
                        false,
                        want.whole,
                    )),
                },
                None => Loaded::NoPreview {
                    file: want.file,
                    frame: want.frame(),
                },
            },
        );
        return;
    }
    // A frame with an edit stands in for its develop, or is compared
    // with its versions, on the picture of its edit at the view's size;
    // 1:1 is the camera's JPEG all the same, for focus.
    if want.size > 0
        && let Some(edited) = edited
        && let shows @ crate::edited_pictures::Shows::Edit(_) = edited.shows(&want.frame())
        && let Ok(hash) = greycard_library::hash_file(&want.path)
        && let Some(thumb) = edited.lookup(&hash, crate::edited_pictures::BIG, shows)
    {
        deliver(Loaded::Ok {
            file: want.file,
            frame: want.frame(),
            size: want.size,
            preview: Arc::new(Preview::edited(thumb)),
        });
        return;
    }
    let fallback = if want.develop {
        Fallback::Develop(previews)
    } else {
        Fallback::Nothing
    };
    let decoded = match &want.picture {
        Some(jpeg) => decode_paired(&want.path, jpeg, want.size, fallback),
        None => decode_keeping(&want.path, want.size, fallback).map(|d| (d, false)),
    };
    match decoded {
        Ok(((mut preview, picture, orientation), companion)) => {
            preview.companion = companion;
            preview.camera_only = want.camera;
            deliver(Loaded::Ok {
                file: want.file,
                frame: want.frame(),
                size: want.size,
                preview: Arc::new(cut_to_camera(preview, &want.path, true, want.whole)),
            });
            // Where the camera focused, from the head of the file just
            // read, once the picture is out: for the zoom keys on it.
            crate::focus_points::read_and_wake(&want.path);
            if want.make_preview
                && keeps_as_preview(companion, &want.path)
                && let Some(previews) = previews
                && let Some((owed, in_hand)) = previews.take_for_picture(&want.path)
            {
                let path = want.path.clone();
                rayon::spawn(move || {
                    // Caught here: a panic on rayon's pool with no
                    // handler takes the editor down with it.
                    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        in_hand
                            .previews()
                            .make_taken(&path, &owed, &picture, orientation);
                    }));
                    if let Err(payload) = made {
                        tracing::warn!(
                            "preview {}: the making panicked: {}",
                            path.display(),
                            crate::worker::panic_message(payload.as_ref())
                        );
                    }
                    drop(in_hand);
                });
            }
        }
        Err(e) => deliver(Loaded::Failed {
            file: want.file,
            frame: want.frame(),
            size: want.size,
            message: format!("{e:#}"),
        }),
    }
}

/// The camera's JPEG (or, for a picture that is not a raw, the
/// picture itself) at `size` on its long edge, zero for its own,
/// turned as its orientation tag says; and the camera's picture it was
/// made from with that orientation, for a local preview to be made
/// from. A raw with no camera picture in it is developed quickly
/// instead ([`developed_picture`]).
fn decode_keeping(
    path: &Path,
    size: u32,
    fallback: Fallback,
) -> anyhow::Result<(Preview, image::RgbImage, Orientation)> {
    let started = Instant::now();
    // Of a raw with several previews, the largest only for 1:1; for a
    // view the smallest that fills it and the local preview made from
    // the same picture (`previews::SIZE`).
    let picture = match size {
        0 => camera_picture(path),
        _ => {
            greycard_engine::camera::camera_picture_at_least(path, size.max(crate::previews::SIZE))
        }
    };
    match picture {
        Ok((image, orientation)) => Ok(shaped(image, orientation, size, started)),
        Err(e) => develop_instead(path, e, fallback, size, started),
    }
}

/// What `fallback` makes of a raw whose camera picture would not read
/// (`why`): its quick develop at `size`, or `why` itself. A picture
/// file is never developed: it is its own picture, and one that does
/// not read is broken.
fn develop_instead(
    path: &Path,
    why: anyhow::Error,
    fallback: Fallback,
    size: u32,
    started: Instant,
) -> anyhow::Result<(Preview, image::RgbImage, Orientation)> {
    let Fallback::Develop(previews) = fallback else {
        return Err(why);
    };
    if greycard_core::picture::is_picture_path(path) {
        return Err(why);
    }
    let (image, orientation) = developed_picture(path, &why, previews)?;
    let (mut preview, image, orientation) = shaped(image, orientation, size, started);
    preview.developed = true;
    Ok((preview, image, orientation))
}

/// What a want shows of a raw with no camera picture in it.
#[derive(Clone, Copy)]
enum Fallback<'a> {
    /// Nothing: the decode fails.
    Nothing,
    /// A quick develop, or the local preview kept of one.
    Develop(Option<&'a Arc<crate::previews::Previews>>),
}

/// A raw with no camera picture to read (`why`), developed quickly at
/// the local preview's size the way its local preview is made, already
/// turned as the camera says: the local preview itself when one is
/// kept, which is that develop made before. A raw that will not decode
/// at all is the develop's failure.
fn developed_picture(
    path: &Path,
    why: &anyhow::Error,
    previews: Option<&Arc<crate::previews::Previews>>,
) -> anyhow::Result<(image::RgbImage, Orientation)> {
    let kept = previews.and_then(|p| {
        let (hash, stamp) = crate::worker::thumb_key(path)?;
        p.get(path, &hash, stamp)
    });
    if let Some(thumb) = kept
        && let Some(image) = image::RgbImage::from_raw(thumb.width, thumb.height, thumb.rgb)
    {
        tracing::debug!("preview {}: {why:#}; its local preview", path.display());
        return Ok((image, Orientation::Normal));
    }
    // The local preview owed, taken before the develop, so the
    // previews' pool does not make the same develop beside this one.
    let owed = previews.and_then(|p| Some((p.clone(), p.take(path)?)));
    let started = Instant::now();
    if RECORDING_DEVELOPS.load(Ordering::Relaxed) {
        DEVELOPS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(path.to_path_buf());
    }
    let (width, height, rgb) =
        crate::worker::thumbnail(path, crate::previews::SIZE).inspect_err(|_| {
            tracing::debug!("preview {}: {why:#}; no develop either", path.display());
        })?;
    tracing::info!(
        "preview {}: {why:#}; a quick develop, {width}x{height} in {:.0} ms",
        path.display(),
        started.elapsed().as_secs_f64() * 1e3
    );
    let image = image::RgbImage::from_raw(width, height, rgb)
        .ok_or_else(|| anyhow::anyhow!("the develop's picture is not {width}x{height}"))?;
    if let Some((previews, owed)) = owed {
        // Kept on another thread, as the loupe's are, so the picture
        // goes up first.
        let (path, picture) = (path.to_path_buf(), image.clone());
        rayon::spawn(move || {
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                previews.make_taken(&path, &owed, &picture, Orientation::Normal);
            }));
            if let Err(payload) = made {
                tracing::warn!(
                    "preview {}: the making panicked: {}",
                    path.display(),
                    crate::worker::panic_message(payload.as_ref())
                );
            }
        });
    }
    Ok((image, Orientation::Normal))
}

/// The raws developed for want of a camera picture, for the tests that
/// say a kept local preview is read instead: by path, since the tests
/// run beside each other. Kept only once a test has armed it
/// ([`record_develops`]), so a shipping build keeps nothing.
static DEVELOPS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Whether a test armed [`DEVELOPS`].
static RECORDING_DEVELOPS: AtomicBool = AtomicBool::new(false);

/// Keeps the raws developed for want of a camera picture from now on,
/// for [`developed`]. For the tests; built whatever the build, since
/// the binary's tests use it and they build this library without
/// cfg(test). Nothing arms it outside a test.
// Its links name this module's private items, for a reader here.
#[allow(rustdoc::private_intra_doc_links)]
#[doc(hidden)]
pub fn record_develops() {
    RECORDING_DEVELOPS.store(true, Ordering::Relaxed);
}

/// The raws developed since [`record_develops`], which a test must have
/// called: one that did not is told so, rather than reading none.
#[doc(hidden)]
pub fn developed() -> Vec<PathBuf> {
    assert!(
        RECORDING_DEVELOPS.load(Ordering::Relaxed),
        "cull::record_develops was not called"
    );
    DEVELOPS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// [`decode_keeping`] for a raw with the camera's JPEG beside it, and
/// whether the picture is the JPEG's: the JPEG, the camera's finished
/// picture, unless the raw's own preview is the larger (a JPEG shot
/// small beside a full-size preview) or the JPEG will not read (gone
/// since the list was read, or cut off), when the raw's own is shown.
/// A JPEG as large as the frame itself is taken without the raw's
/// preview read, since none inside the raw is larger.
fn decode_paired(
    raw: &Path,
    jpeg: &Path,
    size: u32,
    fallback: Fallback,
) -> anyhow::Result<((Preview, image::RgbImage, Orientation), bool)> {
    let started = Instant::now();
    let read = image::image_dimensions(jpeg)
        .ok()
        .filter(|_| jpeg_whole(jpeg));
    let Some((jw, jh)) = read else {
        tracing::debug!(
            "{}: will not read, or is not all there; the raw's own preview",
            jpeg.display()
        );
        return decode_keeping(raw, size, fallback).map(|d| (d, false));
    };
    let long = jw.max(jh);
    let frame = greycard_core::decode::stance_path_unmapped(raw)
        .ok()
        .and_then(|s| s.size)
        .map(|(w, h)| w.max(h));
    // Within a few percent: a sensor's frame runs a little past the
    // camera's JPEG of it.
    if frame.is_some_and(|f| u64::from(long) * 100 >= u64::from(f) * 95)
        && let Ok(d) = decode_keeping(jpeg, size, Fallback::Nothing)
    {
        return Ok((d, true));
    }
    let own = camera_picture(raw);
    if let Ok((image, _)) = &own
        && image.width().max(image.height()) > long
    {
        let (image, orientation) = own?;
        return Ok((shaped(image, orientation, size, started), false));
    }
    match decode_keeping(jpeg, size, Fallback::Nothing) {
        Ok(d) => Ok((d, true)),
        Err(e) => match own {
            Ok((image, orientation)) => Ok((shaped(image, orientation, size, started), false)),
            // Neither reads: the raw developed, if it decodes.
            Err(own) => develop_instead(raw, own, fallback, size, started)
                .map(|d| (d, false))
                .map_err(|_| e),
        },
    }
}

/// Whether the JPEG at `path` ends with its end-of-image marker, past
/// any padding of zeros or `FF`s after it. A file cut off (a card's copy
/// still being written) reads its header and decodes, its missing rows
/// grey, so the header alone does not say the picture is there.
fn jpeg_whole(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return false;
    };
    let tail = len.min(4096);
    if file.seek(SeekFrom::End(-(tail as i64))).is_err() {
        return false;
    }
    let mut end = Vec::with_capacity(tail as usize);
    if file.read_to_end(&mut end).is_err() {
        return false;
    }
    match end.iter().rposition(|&b| b != 0x00 && b != 0xFF) {
        Some(i) => i > 0 && end[i] == 0xD9 && end[i - 1] == 0xFF,
        None => false,
    }
}

/// A camera's picture at `size` on its long edge, zero for its own,
/// turned as `orientation` says; with the picture, for a local preview.
fn shaped(
    image: image::RgbImage,
    orientation: Orientation,
    size: u32,
    started: Instant,
) -> (Preview, image::RgbImage, Orientation) {
    let (pw, ph) = (image.width(), image.height());
    let long = pw.max(ph);
    // The nearest whole factor: within a few percent of the size
    // asked, a little over as often as a little under, so the picture
    // on screen is as often a slight downscale as a slight upscale.
    let factor = if size == 0 {
        1
    } else {
        ((long as f32 / size as f32).round() as u32).max(1)
    };
    let small = box_down(pw, ph, image.as_raw(), factor);
    let (w, h, rgb) = turn_rgb8(small.0, small.1, &small.2, orientation);
    let source = turned_size(pw, ph, orientation);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for px in rgb.as_chunks::<3>().0 {
        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    // The slot is the request's: a full-size request fills its frame's
    // full slot, a screen-size one is a window preview even when the
    // JPEG was no larger than the view and came back whole.
    let mut preview = Preview::new(w, h, rgba, source, size == 0);
    preview.seconds = started.elapsed().as_secs_f64();
    (preview, image, orientation)
}

/// RGB bytes box-downscaled by a whole `factor`, the partial cells at
/// the right and the bottom dropped. A factor past the picture's
/// short side is held to it, so the smallest result is a pixel.
fn box_down(w: u32, h: u32, rgb: &[u8], factor: u32) -> (u32, u32, Vec<u8>) {
    let factor = factor.min(w.max(1)).min(h.max(1));
    if factor <= 1 {
        return (w, h, rgb.to_vec());
    }
    let (w, h, f) = (w as usize, h as usize, factor as usize);
    let (ow, oh) = ((w / f).max(1), (h / f).max(1));
    let mut out = vec![0u8; ow * oh * 3];
    let n = (f * f) as u32;
    for ty in 0..oh {
        for tx in 0..ow {
            let mut sum = [0u32; 3];
            for y in ty * f..(ty + 1) * f {
                let row = &rgb[(y * w + tx * f) * 3..(y * w + (tx + 1) * f) * 3];
                for px in row.as_chunks::<3>().0 {
                    sum[0] += px[0] as u32;
                    sum[1] += px[1] as u32;
                    sum[2] += px[2] as u32;
                }
            }
            let o = (ty * ow + tx) * 3;
            out[o] = ((sum[0] + n / 2) / n) as u8;
            out[o + 1] = ((sum[1] + n / 2) / n) as u8;
            out[o + 2] = ((sum[2] + n / 2) / n) as u8;
        }
    }
    (ow as u32, oh as u32, out)
}

#[cfg(test)]
mod tests;
