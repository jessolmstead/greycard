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
            seconds: 0.0,
        }
    }

    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }

    /// The preview is smaller than a camera's full frame would be.
    pub fn small(&self) -> bool {
        self.source.0.max(self.source.1) < SMALL_LONG_EDGE
    }

    /// This copy is the JPEG's every pixel: 1:1 needs no other.
    pub fn own_size(&self) -> bool {
        (self.width, self.height) == self.source
    }
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
    /// the view.
    pub fn has_own_size(&self, file: usize) -> bool {
        self.best(file).is_some_and(|p| p.own_size())
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
}

/// One decode wanted: the file, its path, and the long edge to make
/// it at, zero for the JPEG's own size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub file: usize,
    pub path: PathBuf,
    pub size: u32,
}

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
}

impl Prefetcher {
    /// Two or three threads: a decode is single-threaded and forty
    /// to a hundred milliseconds, and a run of arrows wants the
    /// window filled at a few times that rate.
    pub fn new(deliver: impl Fn(Loaded) + Send + Sync + 'static) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let deliver: Deliver = Arc::new(deliver);
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 4).clamp(1, 3))
            .unwrap_or(1);
        for k in 0..threads {
            let (q, d) = (queue.clone(), deliver.clone());
            std::thread::Builder::new()
                .name(format!("greycard cull {k}"))
                .spawn(move || run(q, d))
                .expect("spawning a cull thread");
        }
        Self { queue }
    }

    /// What to decode from now on, in this order: the whole of what
    /// is wanted and not yet had, every time. Whatever was queued
    /// before and is not here is forgotten; one a thread is already
    /// on is not started again, and arrives as it was going to.
    pub fn want(&self, list: Vec<Want>) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("cull queue");
        q.wanted = list
            .into_iter()
            .filter(|w| !q.in_flight.contains(w))
            .collect();
        cv.notify_all();
    }

    /// One decode before everything else wanted: the full-size copy
    /// of the frame just taken to 1:1.
    pub fn push_front(&self, want: Want) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("cull queue");
        if q.in_flight.contains(&want) {
            return;
        }
        q.wanted.retain(|w| *w != want);
        q.wanted.push_front(want);
        cv.notify_one();
    }
}

fn run(queue: Arc<(Mutex<Queue>, Condvar)>, deliver: Deliver) {
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
        let made = std::panic::catch_unwind(|| decode(&want.path, want.size));
        lock.lock()
            .expect("cull queue")
            .in_flight
            .retain(|w| *w != want);
        deliver(match made {
            Ok(Ok(preview)) => Loaded::Ok {
                file: want.file,
                path: want.path,
                size: want.size,
                preview: Arc::new(preview),
            },
            Ok(Err(e)) => Loaded::Failed {
                file: want.file,
                path: want.path,
                size: want.size,
                message: format!("{e:#}"),
            },
            Err(_) => Loaded::Failed {
                file: want.file,
                path: want.path,
                size: want.size,
                message: "the decode panicked".into(),
            },
        });
    }
}

/// The camera's JPEG (or, for a picture that is not a raw, the
/// picture itself) at `size` on its long edge, zero for its own,
/// turned as its orientation tag says.
pub fn decode(path: &Path, size: u32) -> anyhow::Result<Preview> {
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
    Ok(preview)
}

/// The camera's own rendering of a file, and the orientation it is
/// to be shown in.
fn camera_picture(path: &Path) -> anyhow::Result<(image::RgbImage, Orientation)> {
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
fn turn_rgb8(w: u32, h: u32, rgb: &[u8], orientation: Orientation) -> (u32, u32, Vec<u8>) {
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
    shoot.join("rejects")
}

/// What a move of the rejects did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// The files moved, by their index in the list given.
    pub files: Vec<usize>,
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
        match by_shoot.iter_mut().find(|(s, _)| *s == shoot) {
            Some((_, group)) => group.push(i),
            None => by_shoot.push((shoot, vec![i])),
        }
    }
    for (shoot, group) in by_shoot {
        let dir = rejects_dir(&shoot);
        std::fs::create_dir_all(&dir)?;
        move_into(files, &group, &dir, &mut moved);
    }
    Ok(moved)
}

/// The frames at `group`, all in one folder, into its rejects folder
/// `dir`.
fn move_into(files: &[PathBuf], group: &[usize], dir: &Path, moved: &mut Moved) {
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
        // A file of any of those names there already: the frame
        // stays whole where it is, rather than its raw going and
        // one of the files beside it not.
        let taken = if dest.exists() {
            Some(name.to_string_lossy().into_owned())
        } else {
            beside
                .iter()
                .find(|(_, to)| to.exists())
                .and_then(|(_, to)| to.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        };
        if let Some(taken) = taken {
            moved
                .skipped
                .push((i, format!("{taken} is in {} already", dir.display())));
            continue;
        }
        if let Err(e) = std::fs::rename(raw, &dest) {
            moved.skipped.push((i, e.to_string()));
            continue;
        }
        moved.files.push(i);
        for (from, to) in &beside {
            if to.parent().is_some_and(|p| !p.exists())
                && let Err(e) = greycard_edit::Sidecar::folder_under(dir)
            {
                tracing::warn!("{}: not moved: {e}", from.display());
                continue;
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
        assert_eq!(moved.files, vec![1, 2]);
        assert_eq!(moved.sidecars, 1);
        assert_eq!(moved.skipped.len(), 3);
        assert_eq!(moved.skipped[0].0, 3);
        assert_eq!(moved.skipped[1].0, 4);
        assert_eq!(moved.skipped[2].0, 5);
        // The kept frame and the clashes are where they were, E with
        // its sidecar; the moved ones and B's sidecar are in the
        // folder beside the shoot.
        assert!(files[0].exists() && files[3].exists());
        assert!(e.exists() && greycard_edit::Sidecar::path_for(&e).exists());
        assert!(!files[1].exists() && !files[2].exists());
        assert!(!greycard_edit::Sidecar::path_for(&files[1]).exists());
        let there = rejects_dir(&dir);
        assert!(there.join("B.CR3").exists() && there.join("C.CR3").exists());
        assert!(greycard_edit::Sidecar::path_for(&there.join("B.CR3")).exists());
        // Both of B's names went; F stayed whole, its XMP with it.
        assert!(there.join("B.xmp").exists() && !dir.join("B.xmp").exists());
        assert!(there.join("B.CR3.xmp").exists() && !dir.join("B.CR3.xmp").exists());
        assert!(f.exists() && dir.join("F.xmp").exists());
        assert_eq!(std::fs::read(there.join("D.CR3")).unwrap(), b"older");
        assert_eq!(std::fs::read(there.join("E.CR3.gcd")).unwrap(), b"older");
        assert_eq!(std::fs::read(there.join("F.xmp")).unwrap(), b"older");
        // Every file that was there is still somewhere.
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
                "E.CR3.gcd",
                "F.CR3",
                "F.xmp",
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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
