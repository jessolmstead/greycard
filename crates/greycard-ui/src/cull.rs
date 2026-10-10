//! Culling from the camera's JPEG (notes §80): the previews decoded
//! ahead of the arrow on their own threads, the window of them kept,
//! the rows a filtered browser lists, the compare view's layout, and
//! the move of the rejects to a folder beside the shoot.
//!
//! Everything that decides something is pure and lives here rather
//! than in the window's callbacks, so it can be tested without a
//! window: which frames are kept as the index moves, which row a
//! file is on, where a tile goes, which files a move touches.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use greycard_core::raw::Orientation;

/// How many frames either side of the current one are decoded ahead
/// and kept: a dozen, so a run of arrows in either direction finds
/// the next frame ready and the memory stays at a few dozen screen-
/// size pictures.
pub const REACH: usize = 12;

/// The gap between compared frames, physical pixels.
pub const TILE_GAP: u32 = 4;

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
    /// the cache's one full-size slot. A screen-size request on a
    /// small JPEG comes back at its own size too, and is a window
    /// preview all the same: the slot is the request's, not the
    /// picture's, or a frame no larger than the view would never be
    /// shown (see `Cache`).
    pub full: bool,
    /// This is the frame's local preview from the cache, not the
    /// camera's JPEG read from the file: the file is out of reach, or
    /// still on its way over the network.
    pub local: bool,
    /// This is the picture made from the frame's edit, kept in the
    /// cache (`edited`): what a frame with an edit is culled on. It
    /// stands where the camera's JPEG would at the view's size, and is
    /// never taken for the JPEG's every pixel, so 1:1 still opens the
    /// camera's full JPEG for focus.
    pub edited: bool,
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

    /// Where this picture came from, for the words that name it.
    pub fn origin(&self) -> Origin {
        if self.edited {
            Origin::Edit
        } else if self.local {
            Origin::Local
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
    /// local preview is small by design and says so in its own words.
    pub fn small(&self) -> bool {
        !self.local && !self.edited && self.source.0.max(self.source.1) < SMALL_LONG_EDGE
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
/// of the window about the current frame, and the full-size one of
/// the current frame when 1:1 asked for it.
#[derive(Default)]
pub struct Cache {
    previews: HashMap<usize, Arc<Preview>>,
    full: Option<(usize, Arc<Preview>)>,
}

impl Cache {
    pub fn get(&self, file: usize) -> Option<&Arc<Preview>> {
        self.previews.get(&file)
    }

    /// The full-size preview of `file`, if it is the one held.
    pub fn full(&self, file: usize) -> Option<&Arc<Preview>> {
        self.full
            .as_ref()
            .filter(|(f, _)| *f == file)
            .map(|(_, p)| p)
    }

    /// The best copy there is of `file`: the full one when it is the
    /// one held, else the screen-size one.
    pub fn best(&self, file: usize) -> Option<&Arc<Preview>> {
        self.full(file).or_else(|| self.get(file))
    }

    /// Whether a copy of `file` at the JPEG's every pixel is held:
    /// the full slot's, or a window preview of a JPEG no larger than
    /// the view. A local preview is its own every pixel and not the
    /// JPEG's, so it is not: at 1:1 the file's is still asked for
    /// where the file can be read.
    pub fn has_own_size(&self, file: usize) -> bool {
        self.best(file)
            .is_some_and(|p| p.own_size() && !p.local && !p.edited)
    }

    pub fn insert(&mut self, file: usize, preview: Arc<Preview>) {
        if preview.full {
            self.full = Some((file, preview));
        } else {
            self.previews.insert(file, preview);
        }
    }

    /// Drop everything outside `files`, and the full-size copy unless
    /// it is `current`'s: the window moved on.
    pub fn keep(&mut self, files: &[usize], current: usize) {
        self.previews.retain(|f, _| files.contains(f));
        if self.full.as_ref().is_some_and(|(f, _)| *f != current) {
            self.full = None;
        }
    }

    /// Drop the full-size copy unless it is `file`'s: the selection
    /// moved.
    pub fn drop_full_unless(&mut self, file: usize) {
        if self.full.as_ref().is_some_and(|(f, _)| *f != file) {
            self.full = None;
        }
    }

    /// Drop the full-size copy: the view is fitted again.
    pub fn drop_full(&mut self) {
        self.full = None;
    }

    pub fn clear(&mut self) {
        self.previews.clear();
        self.full = None;
    }

    /// Drop `file`'s view-size copy: its picture changed (its edit's
    /// was kept), and the next refresh asks for it again.
    pub fn forget(&mut self, file: usize) {
        self.previews.remove(&file);
    }

    /// What is held, in bytes.
    pub fn bytes(&self) -> usize {
        self.previews.values().map(|p| p.bytes()).sum::<usize>()
            + self.full.as_ref().map_or(0, |(_, p)| p.bytes())
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

/// The compare view's grid: one frame, two side by side, or three or
/// four in a square (a set of four at the end of a folder of three
/// is three tiles, and the fourth cell is empty).
pub fn tile_grid(n: usize) -> (u32, u32) {
    match n {
        0 | 1 => (1, 1),
        2 => (2, 1),
        _ => (2, 2),
    }
}

/// Where each of `n` tiles goes in a view of `width` by `height`
/// physical pixels: x, y, width, height, with [`TILE_GAP`] between.
pub fn tile_rects(width: u32, height: u32, n: usize) -> Vec<(u32, u32, u32, u32)> {
    let (cols, rows) = tile_grid(n);
    let w = (width.saturating_sub(TILE_GAP * (cols - 1)) / cols).max(1);
    let h = (height.saturating_sub(TILE_GAP * (rows - 1)) / rows).max(1);
    (0..n.max(1))
        .map(|k| {
            let (c, r) = (k as u32 % cols, k as u32 / cols);
            (c * (w + TILE_GAP), r * (h + TILE_GAP), w, h)
        })
        .collect()
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

/// What a decode came back with.
pub enum Loaded {
    Ok {
        file: usize,
        path: PathBuf,
        /// The long edge it was asked at, zero for the JPEG's own.
        size: u32,
        preview: Arc<Preview>,
    },
    Failed {
        file: usize,
        path: PathBuf,
        size: u32,
        message: String,
    },
    /// A local preview asked for is not kept.
    NoPreview { file: usize, path: PathBuf },
}

/// One decode wanted: the file, its path, the long edge to make it
/// at (zero for the JPEG's own size), and where the picture comes
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub file: usize,
    pub path: PathBuf,
    pub size: u32,
    pub from: Source,
    /// A decode for the culling loupe, which makes the frame's local
    /// preview from the picture in hand when it is owed one. The
    /// develop view's is not: its cores are the develop's.
    pub make_preview: bool,
}

impl Want {
    /// A decode of the file itself: the camera's JPEG, or the picture.
    pub fn of_file(file: usize, path: PathBuf, size: u32) -> Self {
        Self {
            file,
            path,
            size,
            from: Source::File,
            make_preview: false,
        }
    }

    /// A decode of the file for the culling loupe.
    pub fn for_loupe(file: usize, path: PathBuf, size: u32) -> Self {
        Self {
            make_preview: true,
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
            && self.from == other.from
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
pub const NO_LOCAL_PREVIEW: &str = "its root is offline, and no local preview of it is kept";

#[derive(Default)]
struct Queue {
    wanted: VecDeque<Want>,
    /// The decodes a thread has taken and not yet delivered: a list
    /// that names one again does not start it twice.
    in_flight: Vec<Want>,
}

type Deliver = Arc<dyn Fn(Loaded) + Send + Sync>;

/// The previews decoded on threads of their own: the worker is busy
/// with the folder's thumbnails, and a frame under the arrow cannot
/// wait behind them. The list of what is wanted is replaced whole on
/// every move; a thread takes the front of it.
pub struct Prefetcher {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    previews: Previewed,
    /// The last list wanted, for the tests to read what was asked.
    #[cfg(test)]
    asked: Mutex<Vec<Want>>,
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
            #[cfg(test)]
            asked: Mutex::new(Vec::new()),
        }
    }

    /// The last list wanted.
    #[cfg(test)]
    pub(crate) fn asked(&self) -> Vec<Want> {
        self.asked.lock().expect("cull asked").clone()
    }

    /// The local previews to read and to make: the worker's.
    pub(crate) fn set_previews(&self, previews: Arc<crate::previews::Previews>) {
        *self.previews.lock().expect("cull previews") = Some(previews);
    }

    /// What to decode from now on, in this order: the whole of what
    /// is wanted and not yet had, every time. Whatever was queued
    /// before and is not here is forgotten; one a thread is already
    /// on is not started again, and arrives as it was going to.
    pub fn want(&self, list: Vec<Want>) {
        #[cfg(test)]
        {
            *self.asked.lock().expect("cull asked") = list.clone();
        }
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("cull queue");
        q.wanted = list
            .into_iter()
            .filter(|w| !q.in_flight.iter().any(|f| f.same(w)))
            .collect();
        cv.notify_all();
    }

    /// One decode before everything else wanted: the full-size copy
    /// of the frame just taken to 1:1.
    pub fn push_front(&self, want: Want) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("cull queue");
        if q.in_flight.iter().any(|f| f.same(&want)) {
            return;
        }
        q.wanted.retain(|w| !w.same(&want));
        q.wanted.push_front(want);
        cv.notify_one();
    }
}

fn run(queue: Arc<(Mutex<Queue>, Condvar)>, deliver: Deliver, previews: Previewed) {
    let (lock, cv) = &*queue;
    loop {
        let want = {
            let mut q = lock.lock().expect("cull queue");
            loop {
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
                path: want.path,
                size: want.size,
                message: "the decode panicked".into(),
            });
        }
    }
}

/// One picture wanted, from where it says, handed to `deliver`: the
/// local preview from the cache, which reads nothing of the file, or
/// the file's. A picture decoded from the file for the loupe makes the
/// frame's local preview when it is owed one, on another thread, so
/// this one goes on to the next decode at once.
pub(crate) fn fetch(
    want: &Want,
    previews: Option<&Arc<crate::previews::Previews>>,
    deliver: &dyn Fn(Loaded),
) {
    let edited = previews.and_then(|p| p.edited());
    if let Source::Preview { hash, stamp } = &want.from {
        // An edited frame out of reach shows the last picture of its
        // edit kept for its content, ahead of the camera's.
        let from_edit = edited
            .filter(|_| want.size > 0)
            .and_then(|e| e.lookup(hash, crate::edited::BIG, e.shows_offline(&want.path)));
        if let Some(thumb) = from_edit {
            deliver(Loaded::Ok {
                file: want.file,
                path: want.path.clone(),
                size: want.size,
                preview: Arc::new(Preview::edited(thumb)),
            });
            return;
        }
        deliver(match previews.and_then(|p| p.get(hash, *stamp)) {
            Some(thumb) => Loaded::Ok {
                file: want.file,
                path: want.path.clone(),
                size: want.size,
                preview: Arc::new(Preview::local(thumb, want.size == 0)),
            },
            None => Loaded::NoPreview {
                file: want.file,
                path: want.path.clone(),
            },
        });
        return;
    }
    // A frame with an edit is culled on the picture of its edit at the
    // view's size; 1:1 is the camera's JPEG all the same, for focus.
    if want.size > 0
        && let Some(edited) = edited
        && let shows @ crate::edited::Shows::Edit(_) = edited.shows(&want.path)
        && let Ok(hash) = greycard_library::hash_file(&want.path)
        && let Some(thumb) = edited.lookup(&hash, crate::edited::BIG, shows)
    {
        deliver(Loaded::Ok {
            file: want.file,
            path: want.path.clone(),
            size: want.size,
            preview: Arc::new(Preview::edited(thumb)),
        });
        return;
    }
    match decode_keeping(&want.path, want.size) {
        Ok((preview, picture, orientation)) => {
            deliver(Loaded::Ok {
                file: want.file,
                path: want.path.clone(),
                size: want.size,
                preview: Arc::new(preview),
            });
            if want.make_preview
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
            path: want.path.clone(),
            size: want.size,
            message: format!("{e:#}"),
        }),
    }
}

/// The camera's JPEG (or, for a picture that is not a raw, the
/// picture itself) at `size` on its long edge, zero for its own,
/// turned as its orientation tag says; and the camera's picture it was
/// made from with that orientation, for a local preview to be made
/// from.
fn decode_keeping(
    path: &Path,
    size: u32,
) -> anyhow::Result<(Preview, image::RgbImage, Orientation)> {
    let started = Instant::now();
    let (image, orientation) = camera_picture(path)?;
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
    // The slot is the request's: a full-size request fills the one
    // full slot, a screen-size one is a window preview even when the
    // JPEG was no larger than the view and came back whole.
    let mut preview = Preview::new(w, h, rgba, source, size == 0);
    preview.seconds = started.elapsed().as_secs_f64();
    Ok((preview, image, orientation))
}

/// The files read for a camera picture, for the tests that say a frame
/// out of reach reads none: by path, since the tests run beside each
/// other.
#[cfg(test)]
pub(crate) static FILE_READS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The camera's own rendering of a file, and the orientation it is
/// to be shown in.
pub(crate) fn camera_picture(path: &Path) -> anyhow::Result<(image::RgbImage, Orientation)> {
    #[cfg(test)]
    FILE_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(path.to_path_buf());
    if greycard_core::picture::is_picture_path(path) {
        // The picture module's reader for the tag: one answer a file,
        // so the loupe, the strip and the develop cannot disagree
        // about which way up a multi-directory TIFF stands.
        let orientation = greycard_core::picture::orientation_path(path)?;
        let decoder = image::ImageReader::open(path)?
            .with_guessed_format()?
            .into_decoder()?;
        let rgb = image::DynamicImage::from_decoder(decoder)?.to_rgb8();
        return Ok((rgb, orientation));
    }
    match greycard_core::decode::preview_path(path)? {
        Some(found) => Ok(found),
        None => anyhow::bail!("no camera preview in the file"),
    }
}

/// The size of a `w` by `h` picture once turned.
fn turned_size(w: u32, h: u32, orientation: Orientation) -> (u32, u32) {
    if matches!(
        orientation,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    ) {
        (h, w)
    } else {
        (w, h)
    }
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

/// RGB bytes turned as the engine's `develop::orient` turns a
/// picture, so the loupe agrees with the strip and the develop.
pub(crate) fn turn_rgb8(
    w: u32,
    h: u32,
    rgb: &[u8],
    orientation: Orientation,
) -> (u32, u32, Vec<u8>) {
    if orientation == Orientation::Normal {
        return (w, h, rgb.to_vec());
    }
    let (w, h) = (w as usize, h as usize);
    let (ow, oh) = turned_size(w as u32, h as u32, orientation);
    let (ow, oh) = (ow as usize, oh as usize);
    let source = |ox: usize, oy: usize| -> (usize, usize) {
        match orientation {
            Orientation::Normal => (ox, oy),
            Orientation::FlipHorizontal => (w - 1 - ox, oy),
            Orientation::Rotate180 => (w - 1 - ox, h - 1 - oy),
            Orientation::FlipVertical => (ox, h - 1 - oy),
            Orientation::Transpose => (oy, ox),
            Orientation::Rotate90 => (oy, h - 1 - ox),
            Orientation::Transverse => (w - 1 - oy, h - 1 - ox),
            Orientation::Rotate270 => (w - 1 - oy, ox),
        }
    };
    let mut out = vec![0u8; ow * oh * 3];
    for oy in 0..oh {
        for ox in 0..ow {
            let (sx, sy) = source(ox, oy);
            let s = (sy * w + sx) * 3;
            let d = (oy * ow + ox) * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (ow as u32, oh as u32, out)
}

/// The folder the rejects go to: beside the frames, named for what
/// they are.
pub fn rejects_dir(shoot: &Path) -> PathBuf {
    shoot.join(REJECTS)
}

/// The rejects folder's name.
pub const REJECTS: &str = "rejects";

/// What a move of the rejects did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// The files moved, by their index in the list given.
    pub files: Vec<usize>,
    /// Where each of `files` went, in the same order.
    pub to: Vec<PathBuf>,
    /// How many of them had a sidecar that went with them.
    pub sidecars: usize,
    /// The files left where they were, and why.
    pub skipped: Vec<(usize, String)>,
}

/// Move the files at `rejected` (indices into `files`), each with its
/// sidecar when it has one, into the rejects folder beside them.
/// Never a delete: a file of the same name already there is left
/// alone and the frame stays, said so in `skipped`. The folder is
/// made if it is not there.
pub fn move_rejects(files: &[PathBuf], rejected: &[usize]) -> anyhow::Result<Moved> {
    move_rejects_as(files, rejected, Orphans::WriteOver)
}

/// What a move does with a sidecar already in the rejects folder under
/// one of the frame's names, with no raw of the name beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orphans {
    /// The frame's own sidecar goes over it: the shoot's folder is the
    /// user's working folder, and the orphan is an earlier cull's.
    WriteOver,
    /// The frame stays whole where it is and the clash is named: on an
    /// archive nothing is ever written over.
    Keep,
}

/// [`move_rejects`], with what to do about an orphaned sidecar in the
/// rejects folder said.
pub fn move_rejects_as(
    files: &[PathBuf],
    rejected: &[usize],
    orphans: Orphans,
) -> anyhow::Result<Moved> {
    let mut moved = Moved::default();
    // Each frame into the rejects folder of its own folder: a list
    // from more than one (the library's all-roots view) sends each
    // shoot's rejects to that shoot's, never all to the first one's.
    let mut by_shoot: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for &i in rejected {
        let Some(raw) = files.get(i) else {
            continue;
        };
        let shoot = raw
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // Already out: a frame in a rejects folder (the all-roots view
        // lists those too) stays there, rather than going one folder
        // deeper each time the rejects are moved.
        if above_rejects(raw).is_some() {
            moved
                .skipped
                .push((i, "it is in a rejects folder already".into()));
            continue;
        }
        match by_shoot.iter_mut().find(|(s, _)| *s == shoot) {
            Some((_, group)) => group.push(i),
            None => by_shoot.push((shoot, vec![i])),
        }
    }
    for (shoot, group) in by_shoot {
        let dir = rejects_dir(&shoot);
        // A shoot whose rejects folder cannot be made (a folder that
        // is not the user's to write) keeps its rejects where they are,
        // said; the other shoots' go on.
        if let Err(e) = std::fs::create_dir_all(&dir) {
            let why = format!("{}: {e}", dir.display());
            moved
                .skipped
                .extend(group.iter().map(|&i| (i, why.clone())));
            continue;
        }
        move_into(files, &group, &dir, orphans, &mut moved);
    }
    Ok(moved)
}

/// Move the frames at `chosen` (indices into `files`) that are in a
/// rejects folder back into the folder above it, each with its
/// sidecar and its XMPs: the inverse of [`move_rejects`]. A sidecar
/// under the rejects folder's hidden folder goes under the folder
/// above's own, one beside stays beside. Never a delete and never a
/// file of another frame's written over: a raw of the name in the
/// folder above, or a sidecar of one of its names there with a raw it
/// belongs to beside it, leaves the frame whole where it is, said in
/// `skipped`. A sidecar there of its names with no raw beside it is
/// this frame's own, left when it was culled, and the frame's sidecar
/// goes over it, as [`Orphans::WriteOver`] has it. A frame not in a
/// rejects folder is left and said.
pub fn move_back(files: &[PathBuf], chosen: &[usize]) -> Moved {
    let mut moved = Moved::default();
    let mut by_folder: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for &i in chosen {
        let Some(raw) = files.get(i) else {
            continue;
        };
        let Some(above) = above_rejects(raw) else {
            moved
                .skipped
                .push((i, "it is not in a rejects folder".into()));
            continue;
        };
        match by_folder.iter_mut().find(|(d, _)| *d == above) {
            Some((_, group)) => group.push(i),
            None => by_folder.push((above, vec![i])),
        }
    }
    for (above, group) in by_folder {
        move_into(files, &group, &above, Orphans::WriteOver, &mut moved);
    }
    moved
}

/// The folder above the rejects folder `raw` is in, where Move back
/// takes it; None when `raw` is not in a rejects folder.
pub fn above_rejects(raw: &Path) -> Option<PathBuf> {
    let dir = raw.parent()?;
    if dir.file_name() != Some(std::ffi::OsStr::new(REJECTS)) {
        return None;
    }
    Some(
        dir.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
    )
}

/// The file in `dir` that answers to the stem of the short XMP name
/// `xmp` (`IMG.xmp`), by its name: a picture there of that stem, the
/// camera's `IMG.JPG` beside a raw, which the XMP is then that file's
/// or nobody's. Stems are matched without regard to case, as the disks
/// of two of the three platforms match them, and sidecars of ours and
/// files part-written (`.tmp`) are no picture. A folder that cannot be
/// read is taken to hold one. Only the short name has this question:
/// a `.gcd` and the long XMP name (`IMG.CR3.xmp`) carry their raw's
/// whole name, and no other file answers to it.
///
/// The files this same move brought in (`arrived`) are passed over:
/// a raw and the camera's JPEG of one shot moved together share the
/// short XMP as they did where they were, and the one that went first
/// is no other frame's claim on it.
fn stem_owner(xmp: &Path, dir: &Path, arrived: &[PathBuf]) -> Option<String> {
    let stem = xmp.file_stem()?.to_string_lossy().to_lowercase();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Some(format!("{} (it cannot be read)", dir.display()));
    };
    entries.flatten().find_map(|e| {
        let name = e.file_name();
        if arrived
            .iter()
            .any(|a| a.file_name() == Some(name.as_os_str()))
        {
            return None;
        }
        let path = Path::new(&name);
        let ours = path
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "xmp" | "gcd" | "tmp"));
        let same = path
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem);
        (same && !ours).then(|| name.to_string_lossy().into_owned())
    })
}

/// Whether `from`, one of `raw`'s XMPs, is its short name (`IMG.xmp`)
/// rather than its long one (`IMG.CR3.xmp`).
fn is_short_xmp(from: &Path, raw: &Path) -> bool {
    from.extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("xmp"))
        && from.file_stem() == raw.file_stem()
}

/// The frames at `group`, all in one folder, into the folder `dir`:
/// its rejects folder, or for Move back the folder above.
fn move_into(files: &[PathBuf], group: &[usize], dir: &Path, orphans: Orphans, moved: &mut Moved) {
    // The raws this call has moved into `dir` so far.
    let mut arrived: Vec<PathBuf> = Vec::new();
    for &i in group {
        let Some(raw) = files.get(i) else {
            continue;
        };
        let Some(name) = raw.file_name() else {
            moved.skipped.push((i, "no file name".into()));
            continue;
        };
        let dest = dir.join(name);
        // Everything that belongs to this frame and has to travel
        // with it: the `.gcd`, wherever the frame keeps it, and the
        // XMPs whose names are this frame's (both, when a folder has
        // been through two tools). A sidecar under the hidden folder
        // goes under the rejects' own, so the placement survives the
        // cull; the XMPs are always beside.
        let sidecar = greycard_edit::Sidecar::find(raw);
        let hidden = std::ffi::OsStr::new(greycard_edit::SIDECAR_FOLDER);
        let beside: Vec<(PathBuf, PathBuf)> = sidecar
            .iter()
            .cloned()
            .chain(greycard_edit::xmp::paths_of(raw))
            .filter_map(|from| {
                let file = from.file_name()?;
                let to = if from.parent().and_then(Path::file_name) == Some(hidden) {
                    dir.join(hidden).join(file)
                } else {
                    dir.join(file)
                };
                Some((from, to))
            })
            .collect();
        // The frame's `.gcd` names there, under either placement: a
        // reader takes the newer of the two (`Sidecar::find`), so one
        // left under the other placement would shadow the frame's own.
        let gcd = greycard_edit::Sidecar::path_for(&dest);
        let gcds = [
            gcd.clone(),
            dir.join(hidden).join(gcd.file_name().unwrap_or_default()),
        ];
        // The raw's name there already: the frame stays whole where
        // it is, rather than its raw going and one of the files beside
        // it not. A sidecar of its names there with no raw of the name
        // beside it is what an earlier move left when the frame was
        // dragged out by hand (the sidecar under the hidden folder is
        // easy to miss): an orphan of this very frame, which its own
        // sidecar, the one that carries the flag now, writes over. The
        // folder is the truth, and a sidecar with no frame is nothing.
        // The short XMP name is the one a file there can answer to (the
        // camera's JPEG of the stem): written over it would be that
        // file's lost, and moved in beside it with none there it would
        // be nobody's, the frame's ratings lost to every reader; either
        // way the frame stays whole and the file is named.
        let taken = if dest.exists() {
            Some(format!(
                "{} is in {} already",
                name.to_string_lossy(),
                dir.display()
            ))
        } else {
            let keep = orphans == Orphans::Keep;
            let there = |to: &Path| {
                format!(
                    "{} is in {} already",
                    to.file_name().unwrap_or_default().to_string_lossy(),
                    dir.display()
                )
            };
            gcds.iter()
                .find(|to| keep && to.exists())
                .map(|to| there(to))
                .or_else(|| {
                    beside.iter().find_map(|(from, to)| {
                        if is_short_xmp(from, raw)
                            && let Some(owner) = stem_owner(to, dir, &arrived)
                        {
                            return Some(if to.exists() {
                                there(to)
                            } else {
                                format!(
                                    "{owner} there answers to {} too",
                                    to.file_name().unwrap_or_default().to_string_lossy()
                                )
                            });
                        }
                        (keep && to.exists()).then(|| there(to))
                    })
                })
        };
        if let Some(why) = taken {
            moved.skipped.push((i, why));
            continue;
        }
        if let Err(e) = std::fs::rename(raw, &dest) {
            moved.skipped.push((i, e.to_string()));
            continue;
        }
        moved.files.push(i);
        moved.to.push(dest.clone());
        arrived.push(dest.clone());
        // An orphan under the placement the frame's own sidecar does
        // not take goes, so it cannot shadow it; one under the same
        // placement is written over below. A frame with no sidecar of
        // its own leaves an orphan there alone, and takes it up: it is
        // this frame's, and the only edit it has.
        if let Some(own) = sidecar.as_ref() {
            let lands = beside
                .iter()
                .find(|(from, _)| from == own)
                .map(|(_, to)| to);
            for other in gcds.iter().filter(|g| Some(*g) != lands && g.exists()) {
                match std::fs::remove_file(other) {
                    Ok(()) => tracing::info!(
                        "{}: an orphan of this frame under the other placement, taken away",
                        other.display()
                    ),
                    Err(e) => tracing::warn!("{}: orphan not taken away: {e}", other.display()),
                }
            }
        }
        for (from, to) in &beside {
            if to.parent().is_some_and(|p| !p.exists())
                && let Err(e) = greycard_edit::Sidecar::folder_under(dir)
            {
                tracing::warn!("{}: not moved: {e}", from.display());
                continue;
            }
            if to.exists() {
                tracing::info!("{}: an orphan of this frame, written over", to.display());
            }
            match std::fs::rename(from, to) {
                Ok(()) if Some(from) == sidecar.as_ref() => moved.sidecars += 1,
                Ok(()) => {}
                Err(e) => tracing::warn!("{}: not moved: {e}", from.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_keeps_a_dozen_either_side_and_drops_the_rest_as_it_moves() {
        // A folder of forty; the window about frame 5 reaches back to
        // the start and forward to 17.
        assert_eq!(kept(5, 40, REACH), 0..18);
        assert_eq!(kept(20, 40, REACH), 8..33);
        assert_eq!(kept(39, 40, REACH), 27..40);
        assert_eq!(kept(0, 0, REACH), 0..0);
        // Past the end clamps to the last frame.
        assert_eq!(kept(50, 40, REACH), 27..40);
        // Decoded in the order an arrow wants them: the frame itself,
        // then the next one forward, then the one behind, and so on.
        assert_eq!(order(20, 40, 2, true), vec![20, 21, 19, 22, 18]);
        assert_eq!(order(20, 40, 2, false), vec![20, 19, 21, 18, 22]);
        assert_eq!(order(0, 40, 2, false), vec![0, 1, 2]);
        assert_eq!(order(39, 40, 2, true), vec![39, 38, 37]);
        assert!(order(3, 0, 2, true).is_empty());
        // The cache follows: as the index moves from 5 to 20 the
        // frames behind fall out and the memory is bounded by the
        // window, not the folder.
        let preview = |full: bool| Arc::new(Preview::new(2, 1, vec![0; 8], (6000, 4000), full));
        let mut cache = Cache::default();
        for i in kept(5, 40, REACH) {
            cache.insert(i, preview(false));
        }
        cache.insert(5, preview(true));
        assert_eq!(cache.count(), 18);
        assert_eq!(cache.bytes(), 19 * 8);
        assert!(cache.full(5).is_some() && cache.best(5).unwrap().full);
        let window: Vec<usize> = kept(20, 40, REACH).collect();
        cache.keep(&window, 20);
        // Frames 8 to 17 survive the move, 0 to 7 do not, and the
        // full-size copy of frame 5 goes with it.
        assert_eq!(cache.count(), 10);
        assert!(cache.get(7).is_none() && cache.get(8).is_some() && cache.get(17).is_some());
        assert!(cache.full(5).is_none() && cache.best(20).is_none());
        for i in 18..33 {
            cache.insert(i, preview(false));
        }
        assert_eq!(cache.count(), 25);
        assert_eq!(cache.bytes(), 25 * 8);
        // A full-size copy of the current frame stays while it is
        // current.
        cache.insert(20, preview(true));
        cache.keep(&window, 20);
        assert!(cache.full(20).is_some());
        cache.clear();
        assert!(cache.count() == 0 && cache.bytes() == 0);
        // Each preview is numbered, so a texture made from one is
        // told from the next copy of the same frame.
        let (a, b) = (preview(false), preview(false));
        assert_ne!(a.id, b.id);
        // A screen-size request on a JPEG no larger than the view
        // comes back whole and is a window preview all the same; it
        // is also every pixel there is, so 1:1 asks for nothing more.
        let whole = Arc::new(Preview::new(1080, 1616, Vec::new(), (1080, 1616), false));
        assert!(!whole.full && whole.own_size() && whole.small());
        cache.insert(3, whole);
        assert!(cache.get(3).is_some() && cache.full(3).is_none());
        assert!(cache.has_own_size(3));
        // The budget: a dozen either side at a 1410 px view, and
        // fewer under a larger one.
        assert_eq!(reach_for(1410), REACH);
        assert!(reach_for(2560) < REACH && reach_for(2560) >= 4);
        assert_eq!(reach_for(20_000), 1);
        assert!(frames_within(BUDGET_BYTES, 1410) * 3 * 1410 * 1410 <= BUDGET_BYTES);
    }

    #[test]
    fn a_row_is_a_file_only_when_nothing_is_filtered_out() {
        // The files a filter left, and what the browser makes of
        // them: the rejects are gone, so rows 0, 1 and 2 are files
        // 0, 1 and 3.
        let shown = [0usize, 1, 3];
        assert_eq!(row_of_shown(&shown, 1), Some(1));
        assert_eq!(row_of_shown(&shown, 3), Some(2));
        assert_eq!(row_of_shown(&shown, 2), None);
        assert_eq!(row_of_shown(&[], 0), None);
        // A frame the filter hid: the selection goes to the next one
        // shown, or the last when there is none after it.
        assert_eq!(nearest_row(&shown, 1), Some(1));
        assert_eq!(nearest_row(&shown, 2), Some(2));
        assert_eq!(nearest_row(&shown, 4), Some(2));
        assert_eq!(nearest_row(&[], 4), None);
    }

    #[test]
    fn the_compare_set_slides_with_the_selection() {
        // Two up: the pair holds while the selection is in it.
        assert_eq!(anchor_for(4, 4, 2, 10), 4);
        assert_eq!(anchor_for(4, 5, 2, 10), 4);
        // Stepping past the pair slides it by one.
        assert_eq!(anchor_for(4, 6, 2, 10), 5);
        assert_eq!(anchor_for(4, 3, 2, 10), 3);
        // Four up at the end of the folder: the set stops at the end
        // and the selection moves within it.
        assert_eq!(anchor_for(0, 9, 4, 10), 6);
        assert_eq!(anchor_for(6, 8, 4, 10), 6);
        assert_eq!(compare_rows(6, 4, 10), vec![6, 7, 8, 9]);
        // A folder shorter than the set.
        assert_eq!(anchor_for(0, 1, 4, 2), 0);
        assert_eq!(compare_rows(0, 4, 2), vec![0, 1]);
        assert_eq!(anchor_for(3, 0, 2, 0), 0);
        // The tiles: two across, four in a square, with the gap
        // between and none at the edges.
        assert_eq!(tile_grid(1), (1, 1));
        assert_eq!(tile_grid(2), (2, 1));
        assert_eq!(tile_grid(3), (2, 2));
        assert_eq!(tile_grid(4), (2, 2));
        // Three frames in a set of four (a folder of three): three
        // tiles, each within the view, the fourth cell empty.
        let three = tile_rects(1000, 600, 3);
        assert_eq!(three.len(), 3);
        assert!(
            three
                .iter()
                .all(|&(x, y, w, h)| x + w <= 1000 && y + h <= 600)
        );
        assert_eq!(three[2], (0, 302, 498, 298));
        assert_eq!(tile_rects(1000, 600, 1), vec![(0, 0, 1000, 600)]);
        assert_eq!(
            tile_rects(1000, 600, 2),
            vec![(0, 0, 498, 600), (502, 0, 498, 600)]
        );
        let four = tile_rects(1000, 600, 4);
        assert_eq!(four[0], (0, 0, 498, 298));
        assert_eq!(four[3], (502, 302, 498, 298));
        assert_eq!(tile_at(1000, 600, 4, 600.0, 400.0), Some(3));
        assert_eq!(tile_at(1000, 600, 4, 499.0, 100.0), None);
    }

    /// The loupe's source by the frame's root: the file on a local
    /// disk, the local preview alone under an offline root (nothing
    /// without a key to find it by), the preview and the file on a
    /// network mount, and the file alone there with no key.
    #[test]
    fn the_loupe_takes_the_file_the_preview_or_both_by_the_root() {
        let key = Some(("ab".repeat(32), 7));
        let plan = |preview: Option<(String, u64)>, file| Plan { preview, file };
        assert_eq!(source_for(false, false, key.clone()), plan(None, true));
        assert_eq!(source_for(false, false, None), plan(None, true));
        assert_eq!(
            source_for(true, false, key.clone()),
            plan(key.clone(), false)
        );
        assert_eq!(source_for(true, true, None), plan(None, false), "nothing");
        assert_eq!(
            source_for(false, true, key.clone()),
            plan(key.clone(), true)
        );
        assert_eq!(source_for(false, true, None), plan(None, true));
        // The preview and the file of one frame are two wants, and a
        // thread on one does not keep the other from being begun.
        let a = Want::of_file(3, PathBuf::from("/s/a.CR3"), 1024);
        let b = Want::local(3, PathBuf::from("/s/a.CR3"), 1024, ("ab".repeat(32), 7));
        assert!(!a.same(&b));
        assert!(a.same(&Want::for_loupe(3, PathBuf::from("/s/a.CR3"), 1024)));
        assert!(!a.same(&Want {
            size: 0,
            ..a.clone()
        }));
        // A local preview is never the JPEG's every pixel.
        let mut cache = Cache::default();
        let thumb = greycard_library::thumbs::Thumb {
            width: 4,
            height: 2,
            rgb: vec![0; 24],
        };
        cache.insert(3, Arc::new(Preview::local(thumb, false)));
        assert!(cache.best(3).unwrap().own_size());
        assert!(!cache.has_own_size(3), "1:1 still asks for the file's");
    }

    /// A local preview asked for comes from the cache, with no read of
    /// the file, and one not kept says so; the develop view's decode of
    /// a frame makes no preview, and the loupe's makes the one owed, on
    /// another thread, from the picture in hand.
    #[test]
    fn the_local_preview_reads_no_file_and_a_decode_makes_one() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-cull-fetch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("frame.png");
        image::RgbImage::from_fn(3000, 2000, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 40])
        })
        .save(&file)
        .unwrap();
        let mut thumbs =
            greycard_library::Thumbs::at(dir.join("thumbs"), greycard_library::thumbs::DEFAULT_CAP)
                .with_previews(
                    crate::previews::SIZE,
                    greycard_library::thumbs::DEFAULT_PREVIEW_CAP,
                );
        thumbs.seed_split(Default::default());
        let cache: crate::worker::ThumbCache = Arc::new(Mutex::new(Some(thumbs)));
        let previews = Arc::new(crate::previews::Previews::new(cache));
        previews.set_roots(vec![dir.clone()]);
        let (hash, stamp) = crate::previews::Previews::key_of(&file).unwrap();
        let stat = crate::worker::file_stat(&file).unwrap();
        previews.note(&file, stat, &hash);
        assert!(previews.owes(&file));

        let run = |want: &Want| {
            let got = std::cell::RefCell::new(Vec::new());
            fetch(want, Some(&previews), &|l| got.borrow_mut().push(l));
            got.into_inner()
        };
        // None kept yet: said so, and nothing of the file read.
        let local = Want::local(0, file.clone(), 1000, (hash.clone(), stamp));
        assert!(matches!(&run(&local)[..], [Loaded::NoPreview { .. }]));
        assert!(
            !FILE_READS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&file)
        );

        let got = run(&Want::of_file(0, file.clone(), 1000));
        assert!(matches!(&got[..], [Loaded::Ok { preview, .. }] if !preview.local));
        assert!(previews.owes(&file), "the develop view's decode makes none");
        let got = run(&Want::for_loupe(0, file.clone(), 1000));
        assert!(matches!(&got[..], [Loaded::Ok { preview, .. }] if !preview.local));
        assert!(!previews.owes(&file), "taken by the loupe's decode");
        let started = Instant::now();
        let kept = loop {
            if let Some(kept) = previews.get(&hash, stamp) {
                break kept;
            }
            assert!(started.elapsed().as_secs() < 10, "made off the thread");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!((kept.width, kept.height), (2048, 1365));

        // Kept now: the preview alone comes back marked local, with the
        // file gone.
        std::fs::remove_file(&file).unwrap();
        let got = run(&local);
        assert!(matches!(&got[..], [Loaded::Ok { preview, .. }] if preview.local));
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_picture_is_boxed_down_and_turned_as_the_engine_turns_it() {
        // 4x2, each pixel its own red; a factor of 2 averages quads.
        let rgb: Vec<u8> = (0..8u8).flat_map(|i| [i * 10, 0, 0]).collect();
        let (w, h, small) = box_down(4, 2, &rgb, 2);
        assert_eq!((w, h), (2, 1));
        // (0 + 10 + 40 + 50) / 4 = 25; (20 + 30 + 60 + 70) / 4 = 45.
        assert_eq!(small, vec![25, 0, 0, 45, 0, 0]);
        // A factor past the short side is held to it.
        let (w, h, tiny) = box_down(4, 2, &rgb, 7);
        assert_eq!((w, h), (2, 1));
        assert_eq!(tiny, small);
        // Rotate270 (the tag's 8) turns a landscape to a portrait as
        // `develop::orient` does: the first output row is the
        // source's last column.
        let (w, h, turned) = turn_rgb8(4, 2, &rgb, Orientation::Rotate270);
        assert_eq!((w, h), (2, 4));
        assert_eq!(&turned[0..6], &[30, 0, 0, 70, 0, 0]);
        assert_eq!(turned_size(4, 2, Orientation::Rotate90), (2, 4));
        assert_eq!(turned_size(4, 2, Orientation::Rotate180), (4, 2));
    }

    #[test]
    fn a_reject_takes_its_sidecar_under_the_folder_along() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-rejects-folder-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("A.CR3");
        let b = dir.join("B.CR3");
        std::fs::write(&a, b"raw").unwrap();
        std::fs::write(&b, b"raw").unwrap();
        let mut sidecar = greycard_edit::Sidecar::default();
        sidecar.meta.flag = greycard_edit::meta::Flag::Reject;
        sidecar
            .save_in(&a, greycard_edit::Placement::Folder)
            .unwrap();
        sidecar.save(&b).unwrap();

        let moved = move_rejects(&[a.clone(), b.clone()], &[0, 1]).unwrap();
        assert_eq!(moved.files, vec![0, 1]);
        assert_eq!(moved.sidecars, 2);
        let there = rejects_dir(&dir);
        // Each sidecar keeps the placement it had: A's under the
        // rejects' own hidden folder, B's beside B.
        assert!(
            there
                .join(greycard_edit::SIDECAR_FOLDER)
                .join("A.CR3.gcd")
                .exists()
        );
        assert!(!there.join("A.CR3.gcd").exists());
        assert!(there.join("B.CR3.gcd").exists());
        assert!(
            !dir.join(greycard_edit::SIDECAR_FOLDER)
                .join("A.CR3.gcd")
                .exists()
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A raw of the name there already keeps the frame whole where it
    /// is; a sidecar of its names there with no raw beside it is an
    /// orphan of an earlier cull and goes under the frame's own.
    #[test]
    fn the_rejects_move_with_their_sidecars_and_nothing_is_deleted() {
        let dir = std::env::temp_dir().join(format!("greycard-rejects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let names = ["A.CR3", "B.CR3", "C.CR3", "D.CR3"];
        let files: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        for f in &files {
            std::fs::write(f, b"raw").unwrap();
        }
        // B has a sidecar, C has none, D's name is taken already,
        // E's sidecar's name is, and F's XMP's name is.
        let e = dir.join("E.CR3");
        let f = dir.join("F.CR3");
        std::fs::write(&e, b"raw").unwrap();
        std::fs::write(&f, b"raw").unwrap();
        let mut sidecar = greycard_edit::Sidecar::default();
        sidecar.meta.flag = greycard_edit::meta::Flag::Reject;
        sidecar.save(&files[1]).unwrap();
        sidecar.save(&e).unwrap();
        // B has an XMP under both names, which must not be left
        // pointing at a frame that has gone.
        greycard_edit::xmp::save(&files[1], &sidecar.meta, None).unwrap();
        std::fs::write(
            greycard_edit::xmp::long_path(&files[1]),
            greycard_edit::xmp::fresh(&sidecar.meta, None),
        )
        .unwrap();
        greycard_edit::xmp::save(&f, &sidecar.meta, None).unwrap();
        std::fs::create_dir_all(rejects_dir(&dir)).unwrap();
        std::fs::write(rejects_dir(&dir).join("D.CR3"), b"older").unwrap();
        std::fs::write(rejects_dir(&dir).join("E.CR3.gcd"), b"older").unwrap();
        std::fs::write(rejects_dir(&dir).join("F.xmp"), b"older").unwrap();
        let mut files = files;
        files.push(e.clone());
        files.push(f.clone());

        let moved = move_rejects(&files, &[1, 2, 3, 4, 5]).unwrap();
        // D's raw is there already: D stays whole. E's sidecar and F's
        // XMP there are orphans of an earlier cull, with no raw of the
        // name beside them: E and F go, their own sidecars over them.
        assert_eq!(moved.files, vec![1, 2, 4, 5]);
        assert_eq!(moved.sidecars, 2);
        assert_eq!(moved.skipped.len(), 1);
        assert_eq!(moved.skipped[0].0, 3);
        // The kept frame and the clash are where they were; the moved
        // ones and their sidecars are in the folder beside the shoot.
        assert!(files[0].exists() && files[3].exists());
        assert!(!files[1].exists() && !files[2].exists());
        assert!(!e.exists() && !greycard_edit::Sidecar::path_for(&e).exists());
        assert!(!f.exists() && !dir.join("F.xmp").exists());
        assert!(!greycard_edit::Sidecar::path_for(&files[1]).exists());
        let there = rejects_dir(&dir);
        assert!(there.join("B.CR3").exists() && there.join("C.CR3").exists());
        assert!(greycard_edit::Sidecar::path_for(&there.join("B.CR3")).exists());
        // Both of B's names went.
        assert!(there.join("B.xmp").exists() && !dir.join("B.xmp").exists());
        assert!(there.join("B.CR3.xmp").exists() && !dir.join("B.CR3.xmp").exists());
        assert_eq!(std::fs::read(there.join("D.CR3")).unwrap(), b"older");
        // The orphans are gone under E's and F's own.
        assert_ne!(std::fs::read(there.join("E.CR3.gcd")).unwrap(), b"older");
        assert_ne!(std::fs::read(there.join("F.xmp")).unwrap(), b"older");
        assert!(there.join("E.CR3").exists() && there.join("F.CR3").exists());
        // Every file that was there is still somewhere, the two
        // orphans excepted.
        let mut all: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .chain(std::fs::read_dir(&there).unwrap())
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        all.sort();
        assert_eq!(
            all,
            [
                "A.CR3",
                "B.CR3",
                "B.CR3.gcd",
                "B.CR3.xmp",
                "B.xmp",
                "C.CR3",
                "D.CR3",
                "D.CR3",
                "E.CR3",
                "E.CR3.gcd",
                "F.CR3",
                "F.xmp"
            ]
        );
        // Nothing rejected: nothing done, no folder made.
        let empty =
            std::env::temp_dir().join(format!("greycard-rejects-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        let none = move_rejects(&[empty.join("E.CR3")], &[]).unwrap();
        assert_eq!(none, Moved::default());
        assert!(!rejects_dir(&empty).exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    /// A list from more than one folder, as the library's all-roots
    /// view has: each shoot's rejects go to that shoot's own rejects
    /// folder, never all to the first one's.
    #[test]
    fn rejects_from_two_shoots_go_to_their_own_folders() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-rejects-two-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (one, two) = (dir.join("one"), dir.join("two"));
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        let files = vec![one.join("A.CR3"), one.join("B.CR3"), two.join("C.CR3")];
        for f in &files {
            std::fs::write(f, b"raw").unwrap();
        }
        let moved = move_rejects(&files, &[1, 2]).unwrap();
        assert_eq!(moved.files, vec![1, 2]);
        assert!(rejects_dir(&one).join("B.CR3").exists());
        assert!(rejects_dir(&two).join("C.CR3").exists());
        assert!(!rejects_dir(&one).join("C.CR3").exists());
        assert!(files[0].exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A shoot whose rejects folder cannot be made keeps its rejects,
    /// said in `skipped`, and the other shoots' go on; and a frame in
    /// a rejects folder already (the all-roots view lists those) is
    /// never moved a folder deeper.
    #[cfg(unix)]
    #[test]
    fn a_shoot_that_cannot_take_a_rejects_folder_keeps_its_own_and_none_nest() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "greycard-rejects-locked-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(rejects_dir(&dir.join("c"))).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let files = vec![
            a.join("A.CR3"),
            b.join("B.CR3"),
            rejects_dir(&dir.join("c")).join("C.CR3"),
        ];
        for f in &files {
            std::fs::write(f, b"raw").unwrap();
        }
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = std::fs::create_dir(b.join("probe")).is_ok();
        let moved = move_rejects(&files, &[0, 1, 2]).unwrap();
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
        if writable {
            // Run as root, which writes anyway.
            crate::testing::remove_dir_retry(&dir);
            return;
        }
        assert_eq!(moved.files, vec![0]);
        assert!(rejects_dir(&a).join("A.CR3").exists());
        assert!(files[1].exists(), "B stays where it was");
        assert!(files[2].exists(), "C stays in its rejects folder");
        assert!(!rejects_dir(&rejects_dir(&dir.join("c"))).exists());
        let skipped: Vec<usize> = moved.skipped.iter().map(|(i, _)| *i).collect();
        assert_eq!(skipped, vec![2, 1]);
        assert!(
            moved.skipped[1].1.contains("rejects"),
            "{:?}",
            moved.skipped
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A shoot with a rejects folder in it, for Move back: the shoot,
    /// and its rejects folder.
    fn shoot_with_rejects(what: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "greycard-back-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let rejects = rejects_dir(&dir);
        std::fs::create_dir_all(&rejects).unwrap();
        (dir, rejects)
    }

    /// A raw at `path` with a sidecar flagged reject, at `placement`.
    fn rejected_frame(path: &Path, placement: greycard_edit::Placement) {
        std::fs::write(path, b"raw").unwrap();
        let mut sidecar = greycard_edit::Sidecar::default();
        sidecar.meta.flag = greycard_edit::meta::Flag::Reject;
        sidecar.save_in(path, placement).unwrap();
    }

    /// The plain move back: the raw into the folder above, its sidecar
    /// beside it still, and both of its XMPs along.
    #[test]
    fn move_back_takes_a_frame_home_with_its_sidecar_and_xmps() {
        let (dir, rejects) = shoot_with_rejects("plain");
        let a = rejects.join("A.CR3");
        rejected_frame(&a, greycard_edit::Placement::Beside);
        let meta = greycard_edit::Sidecar::load(&a).unwrap().unwrap().meta;
        greycard_edit::xmp::save(&a, &meta, None).unwrap();
        std::fs::write(
            greycard_edit::xmp::long_path(&a),
            greycard_edit::xmp::fresh(&meta, None),
        )
        .unwrap();
        assert_eq!(above_rejects(&a), Some(dir.clone()));

        let moved = move_back(std::slice::from_ref(&a), &[0]);
        assert_eq!(moved.files, vec![0]);
        assert_eq!(moved.sidecars, 1);
        assert!(moved.skipped.is_empty());
        assert!(dir.join("A.CR3").exists() && !a.exists());
        assert!(dir.join("A.CR3.gcd").exists());
        assert!(!rejects.join("A.CR3.gcd").exists());
        assert!(dir.join("A.xmp").exists() && !rejects.join("A.xmp").exists());
        assert!(dir.join("A.CR3.xmp").exists() && !rejects.join("A.CR3.xmp").exists());
        // Nothing made under the hidden folder for a sidecar that was
        // beside.
        assert!(!dir.join(greycard_edit::SIDECAR_FOLDER).exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A sidecar under the rejects folder's hidden folder goes under
    /// the folder above's own, not beside.
    #[test]
    fn move_back_keeps_a_sidecar_under_the_hidden_folder() {
        let (dir, rejects) = shoot_with_rejects("hidden");
        let a = rejects.join("A.CR3");
        rejected_frame(&a, greycard_edit::Placement::Folder);
        let moved = move_back(std::slice::from_ref(&a), &[0]);
        assert_eq!(moved.files, vec![0]);
        assert_eq!(moved.sidecars, 1);
        let hidden = greycard_edit::SIDECAR_FOLDER;
        assert!(dir.join(hidden).join("A.CR3.gcd").exists());
        assert!(!dir.join("A.CR3.gcd").exists());
        assert!(!rejects.join(hidden).join("A.CR3.gcd").exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A raw of the name in the folder above, or an XMP of one of the
    /// frame's names there that a frame there answers to, leaves the
    /// frame whole in the rejects folder, the clash named; the files
    /// there are as they were.
    #[test]
    fn move_back_leaves_a_frame_whose_name_is_taken_whole() {
        let (dir, rejects) = shoot_with_rejects("taken");
        let (a, b) = (rejects.join("A.CR3"), rejects.join("B.CR3"));
        rejected_frame(&a, greycard_edit::Placement::Beside);
        rejected_frame(&b, greycard_edit::Placement::Beside);
        let meta = greycard_edit::Sidecar::load(&b).unwrap().unwrap().meta;
        greycard_edit::xmp::save(&b, &meta, None).unwrap();
        assert!(rejects.join("B.xmp").exists(), "B's own short XMP");
        // A's raw name is taken above; B's short XMP name is the
        // camera JPEG's there.
        std::fs::write(dir.join("A.CR3"), b"other").unwrap();
        std::fs::write(dir.join("B.JPG"), b"jpeg").unwrap();
        std::fs::write(dir.join("B.xmp"), b"the jpeg's").unwrap();

        let moved = move_back(&[a.clone(), b.clone()], &[0, 1]);
        assert!(moved.files.is_empty());
        let skipped: Vec<usize> = moved.skipped.iter().map(|(i, _)| *i).collect();
        assert_eq!(skipped, vec![0, 1]);
        assert!(moved.skipped[0].1.contains("A.CR3"), "{:?}", moved.skipped);
        assert!(moved.skipped[1].1.contains("B.xmp"), "{:?}", moved.skipped);
        // Whole where they were, and nothing above written over.
        assert!(a.exists() && rejects.join("A.CR3.gcd").exists());
        assert!(b.exists() && rejects.join("B.CR3.gcd").exists());
        assert!(rejects.join("B.xmp").exists());
        assert_eq!(std::fs::read(dir.join("A.CR3")).unwrap(), b"other");
        assert_eq!(std::fs::read(dir.join("B.xmp")).unwrap(), b"the jpeg's");
        crate::testing::remove_dir_retry(&dir);
    }

    /// A sidecar in the folder above under the frame's name with no
    /// raw of the name beside it is the frame's own, left when it was
    /// culled; the frame's sidecar, which carries the flag and the
    /// newer edits, goes over it.
    #[test]
    fn move_back_writes_the_frames_sidecar_over_its_own_orphan() {
        let (dir, rejects) = shoot_with_rejects("orphan");
        let a = rejects.join("A.CR3");
        rejected_frame(&a, greycard_edit::Placement::Folder);
        let hidden = dir.join(greycard_edit::SIDECAR_FOLDER);
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("A.CR3.gcd"), b"older").unwrap();

        let moved = move_back(std::slice::from_ref(&a), &[0]);
        assert_eq!(moved.files, vec![0]);
        assert_eq!(moved.sidecars, 1);
        assert!(dir.join("A.CR3").exists());
        let now = greycard_edit::Sidecar::load(&dir.join("A.CR3"))
            .unwrap()
            .expect("the frame's own sidecar");
        assert_eq!(now.meta.flag, greycard_edit::meta::Flag::Reject);
        assert_ne!(std::fs::read(hidden.join("A.CR3.gcd")).unwrap(), b"older");
        crate::testing::remove_dir_retry(&dir);
    }

    /// A frame not in a rejects folder stays where it is, said; the
    /// rest of the list goes on.
    #[test]
    fn move_back_leaves_a_frame_not_in_a_rejects_folder() {
        let (dir, rejects) = shoot_with_rejects("not");
        let (a, b) = (dir.join("A.CR3"), rejects.join("B.CR3"));
        rejected_frame(&a, greycard_edit::Placement::Beside);
        rejected_frame(&b, greycard_edit::Placement::Beside);
        assert_eq!(above_rejects(&a), None);
        let moved = move_back(&[a.clone(), b.clone()], &[0, 1]);
        assert_eq!(moved.files, vec![1]);
        assert_eq!(moved.skipped.len(), 1);
        assert_eq!(moved.skipped[0].0, 0);
        assert!(moved.skipped[0].1.contains("not in a rejects folder"));
        assert!(a.exists() && dir.join("A.CR3.gcd").exists());
        assert!(dir.join("B.CR3").exists() && !b.exists());
        // Nothing went up a folder from the shoot itself.
        assert!(!dir.parent().unwrap().join("A.CR3").exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// An orphan under the other placement than the frame's own
    /// sidecar takes: written over, it goes, so it cannot shadow the
    /// frame's own by a higher saved count; kept, on an archive, the
    /// frame stays whole and it is named.
    #[test]
    fn an_orphan_under_the_other_placement_goes_or_keeps_the_frame_whole() {
        let (dir, rejects) = shoot_with_rejects("other-placement");
        let hidden = greycard_edit::SIDECAR_FOLDER;
        // A's own beside it in the rejects folder; an older copy, saved
        // more times, under the shoot's hidden folder.
        let a = rejects.join("A.CR3");
        rejected_frame(&a, greycard_edit::Placement::Beside);
        let mut old = greycard_edit::Sidecar::default();
        for n in 0..5 {
            old.meta.rating = n;
            old.save_in(&dir.join("A.CR3"), greycard_edit::Placement::Folder)
                .unwrap();
        }
        std::fs::remove_file(dir.join("A.CR3")).unwrap_or(());
        assert!(dir.join(hidden).join("A.CR3.gcd").exists());

        let moved = move_back(std::slice::from_ref(&a), &[0]);
        assert_eq!(moved.files, vec![0]);
        assert_eq!(moved.to, vec![dir.join("A.CR3")]);
        assert!(dir.join("A.CR3.gcd").exists());
        assert!(
            !dir.join(hidden).join("A.CR3.gcd").exists(),
            "the orphan under the other placement went"
        );
        let now = greycard_edit::Sidecar::load(&dir.join("A.CR3"))
            .unwrap()
            .unwrap();
        assert_eq!(now.meta.flag, greycard_edit::meta::Flag::Reject);
        assert_eq!(now.meta.rating, 0, "the frame's own, not the orphan's");

        // Kept: the same shape the other way, into the rejects folder.
        let b = dir.join("B.CR3");
        rejected_frame(&b, greycard_edit::Placement::Beside);
        std::fs::create_dir_all(rejects.join(hidden)).unwrap();
        std::fs::write(rejects.join(hidden).join("B.CR3.gcd"), b"older").unwrap();
        let kept = move_rejects_as(std::slice::from_ref(&b), &[0], Orphans::Keep).unwrap();
        assert!(kept.files.is_empty());
        assert!(
            kept.skipped[0].1.contains("B.CR3.gcd"),
            "{:?}",
            kept.skipped
        );
        assert!(b.exists() && dir.join("B.CR3.gcd").exists());
        assert_eq!(
            std::fs::read(rejects.join(hidden).join("B.CR3.gcd")).unwrap(),
            b"older"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The `.gcd` and the long XMP name carry the raw's whole name, so
    /// an unrelated file of the stem there owns neither: a hidden
    /// orphan beside the camera's JPEG of the frame is still an orphan.
    #[test]
    fn a_jpeg_of_the_stem_owns_no_gcd() {
        let (dir, rejects) = shoot_with_rejects("jpeg-gcd");
        let a = rejects.join("A.CR3");
        rejected_frame(&a, greycard_edit::Placement::Folder);
        let hidden = dir.join(greycard_edit::SIDECAR_FOLDER);
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("A.CR3.gcd"), b"older").unwrap();
        std::fs::write(dir.join("A.CR3.jpg"), b"jpeg").unwrap();
        std::fs::write(dir.join("A.JPG"), b"jpeg").unwrap();
        let moved = move_back(std::slice::from_ref(&a), &[0]);
        assert_eq!(moved.files, vec![0], "{:?}", moved.skipped);
        assert_ne!(std::fs::read(hidden.join("A.CR3.gcd")).unwrap(), b"older");
        crate::testing::remove_dir_retry(&dir);
    }

    /// Move rejects with the camera's JPEG of a frame's stem in the
    /// rejects folder: its short XMP there is the JPEG's and is never
    /// written over, and with none there a short XMP moved in would be
    /// nobody's; either way the frame stays whole and the file is
    /// named. The stem is matched without regard to case. The long
    /// name has no such question and goes.
    #[test]
    fn a_jpegs_short_xmp_in_the_rejects_folder_keeps_the_frame_whole() {
        let (dir, rejects) = shoot_with_rejects("jpeg-xmp");
        let meta = greycard_edit::meta::Meta {
            flag: greycard_edit::meta::Flag::Reject,
            ..Default::default()
        };
        // B with its short XMP; the rejects folder has B.JPG and its XMP.
        let b = dir.join("B.CR3");
        rejected_frame(&b, greycard_edit::Placement::Beside);
        std::fs::write(dir.join("B.xmp"), greycard_edit::xmp::fresh(&meta, None)).unwrap();
        assert_eq!(greycard_edit::xmp::paths_of(&b), vec![dir.join("B.xmp")]);
        std::fs::write(rejects.join("B.JPG"), b"jpeg").unwrap();
        std::fs::write(rejects.join("B.xmp"), b"the jpeg's").unwrap();
        // C with its short XMP; the rejects folder has c.jpg, no XMP.
        let c = dir.join("C.CR3");
        rejected_frame(&c, greycard_edit::Placement::Beside);
        std::fs::write(dir.join("C.xmp"), greycard_edit::xmp::fresh(&meta, None)).unwrap();
        std::fs::write(rejects.join("c.jpg"), b"jpeg").unwrap();
        // D with only its long XMP, and a d.jpg there: D goes.
        let d = dir.join("D.CR3");
        rejected_frame(&d, greycard_edit::Placement::Beside);
        std::fs::write(
            greycard_edit::xmp::long_path(&d),
            greycard_edit::xmp::fresh(&meta, None),
        )
        .unwrap();
        std::fs::write(rejects.join("D.jpg"), b"jpeg").unwrap();

        let files = [b.clone(), c.clone(), d.clone()];
        let moved = move_rejects(&files, &[0, 1, 2]).unwrap();
        assert_eq!(moved.files, vec![2]);
        let why: Vec<&str> = moved.skipped.iter().map(|(_, w)| w.as_str()).collect();
        assert!(why[0].starts_with("B.xmp is in"), "{why:?}");
        assert_eq!(why[1], "c.jpg there answers to C.xmp too");
        assert!(b.exists() && dir.join("B.xmp").exists());
        assert_eq!(std::fs::read(rejects.join("B.xmp")).unwrap(), b"the jpeg's");
        assert!(c.exists() && dir.join("C.xmp").exists() && !rejects.join("C.xmp").exists());
        assert!(rejects.join("D.CR3").exists() && rejects.join("D.CR3.xmp").exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A raw and the camera's JPEG of one shot, each with its `.gcd`,
    /// and one short XMP between them, both chosen: the three move
    /// together, out and back, in either order, the one that goes
    /// first being no claim of another frame's on the XMP. With only
    /// the raw chosen, the XMP was nobody's while the pair was together
    /// and stays with the JPEG left behind, whose it then is.
    #[test]
    fn a_raw_and_its_jpeg_move_with_their_short_xmp_together() {
        let meta = greycard_edit::meta::Meta {
            flag: greycard_edit::meta::Flag::Reject,
            ..Default::default()
        };
        let names = |d: &Path| {
            let mut all: Vec<String> = std::fs::read_dir(d)
                .unwrap()
                .flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            all.sort();
            all
        };
        let trio = ["A.CR3", "A.CR3.gcd", "A.JPG", "A.JPG.gcd", "A.xmp"];
        for order in [[0usize, 1], [1, 0]] {
            let (dir, rejects) = shoot_with_rejects("pair");
            let pair = [dir.join("A.CR3"), dir.join("A.JPG")];
            for f in &pair {
                rejected_frame(f, greycard_edit::Placement::Beside);
            }
            std::fs::write(dir.join("A.xmp"), greycard_edit::xmp::fresh(&meta, None)).unwrap();

            let out = move_rejects(&pair, &order).unwrap();
            assert_eq!(out.files.len(), 2, "{:?}", out.skipped);
            assert_eq!(names(&rejects), trio, "out, in the order {order:?}");
            assert!(names(&dir).is_empty());

            let back_from = [rejects.join("A.CR3"), rejects.join("A.JPG")];
            let back = move_back(&back_from, &order);
            assert_eq!(back.files.len(), 2, "{:?}", back.skipped);
            assert_eq!(names(&dir), trio, "back, in the order {order:?}");
            assert!(names(&rejects).is_empty());
            crate::testing::remove_dir_retry(&dir);
        }

        // Only the raw: the XMP stays with the JPEG.
        let (dir, rejects) = shoot_with_rejects("pair-raw");
        let pair = [dir.join("A.CR3"), dir.join("A.JPG")];
        for f in &pair {
            rejected_frame(f, greycard_edit::Placement::Beside);
        }
        std::fs::write(dir.join("A.xmp"), greycard_edit::xmp::fresh(&meta, None)).unwrap();
        let out = move_rejects(&pair, &[0]).unwrap();
        assert_eq!(out.files, vec![0]);
        assert_eq!(names(&rejects), ["A.CR3", "A.CR3.gcd"]);
        assert_eq!(names(&dir), ["A.JPG", "A.JPG.gcd", "A.xmp"]);
        assert_eq!(
            greycard_edit::xmp::paths_of(&pair[1]),
            vec![dir.join("A.xmp")],
            "the JPEG's now"
        );
        // And back, the raw alone: the JPEG answers to A.xmp in the
        // shoot, which is not the raw's, so nothing stands in its way.
        let back = move_back(&[rejects.join("A.CR3")], &[0]);
        assert_eq!(back.files, vec![0], "{:?}", back.skipped);
        assert_eq!(names(&dir), trio);
        crate::testing::remove_dir_retry(&dir);
    }
}
