//! Where the camera focused on each raw, read from the file's head
//! (`decode::focus_point_path`) with no decode, for the culling loupe
//! and the lightbox's loupe, which draw the camera's picture and have
//! no develop to carry it. Read after the picture on the decode thread
//! that reads the file anyway, or on a thread of its own when the zoom
//! key asks for a frame whose picture came from the local previews;
//! kept for the session by path, with the file's modified time and
//! length it was read at, so a file replaced at the same path is read
//! again. A read that finds nothing, or fails, is kept too: a missing
//! or unreadable file is asked once.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::SystemTime;

use greycard_core::raw::{FocusPoint, Orientation};

/// A raw's point and the orientation its picture is turned by, or
/// none.
pub(crate) type Point = Option<(FocusPoint, Orientation)>;

/// What a file was when it was read: its modified time and length;
/// none for a file that could not be looked at.
type Stamp = Option<(Option<SystemTime>, u64)>;

static POINTS: LazyLock<Mutex<HashMap<PathBuf, (Stamp, Point)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The paths a thread of [`read_then`] is reading now.
static READING: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// What the window does when a read off its thread has found
/// something new: a redraw, which lays the frame out and takes a key's
/// zoom waiting on the point there (`viewport::focus_arrived`).
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// The most points kept: past it the map starts again, as a session
/// that steps through more frames than this reads them again.
const HELD: usize = 1 << 16;

/// The most [`read_then`] threads at once: a folder that has stopped
/// answering holds at most these, and a key pressed past them waits
/// for no point.
const READERS: usize = 4;

/// Sets what a read off the window's thread wakes: once, at start.
pub fn set_wake(wake: impl Fn() + Send + Sync + 'static) {
    let _ = WAKE.set(Box::new(wake));
}

/// The point of the file at `path` when it is known, read or told,
/// with nothing read or looked at: for the window's thread.
pub fn known(path: &Path) -> Option<Point> {
    POINTS
        .lock()
        .expect("focus points")
        .get(path)
        .map(|(_, p)| *p)
}

/// Whether reading the file at `path` could find a point: a raw of a
/// maker read, nothing known of it yet.
pub fn wanted(path: &Path) -> bool {
    greycard_core::decode::may_have_focus_point(path) && known(path).is_none()
}

/// Keeps `point` as the word on the file at `path` as `stamp` found it.
fn keep(path: &Path, stamp: Stamp, point: Point) {
    let mut points = POINTS.lock().expect("focus points");
    if points.len() >= HELD {
        points.clear();
    }
    points.insert(path.to_path_buf(), (stamp, point));
}

/// Keeps `point` as the word on the file at `path`, as it is now. For
/// the tests; built whatever the build, since the binary's tests use
/// it and they build this library without cfg(test).
#[doc(hidden)]
pub fn tell(path: &Path, point: Point) {
    keep(path, stamp_of(path), point);
}

fn stamp_of(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

#[cfg(test)]
static HEAD_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The point of the file at `path`, read from its head unless it was
/// read before as the file is now (its modified time and length, or
/// the same failure to look at it): for a thread that may block on the
/// file. A second value says whether this read found something not
/// known before.
pub(crate) fn read(path: &Path) -> (Point, bool) {
    if !greycard_core::decode::may_have_focus_point(path) {
        return (None, false);
    }
    let stamp = stamp_of(path);
    if let Some((was, point)) = POINTS.lock().expect("focus points").get(path)
        && *was == stamp
    {
        return (*point, false);
    }
    #[cfg(test)]
    HEAD_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let at = std::time::Instant::now();
    // A file that cannot be looked at is not opened.
    let point = match stamp {
        None => None,
        Some(_) => match greycard_core::decode::focus_point_path(path) {
            Ok(point) => point,
            Err(e) => {
                tracing::debug!("{}: the focus point: {e}", path.display());
                None
            }
        },
    };
    tracing::debug!(
        "{}: the focus point read in {:.2} ms: {point:?}",
        path.display(),
        at.elapsed().as_secs_f64() * 1e3
    );
    keep(path, stamp, point);
    (point, true)
}

/// [`read`] on the decode thread after its picture went out: the
/// window is woken when the read found something new.
// Its links name this module's private items, for a reader here.
#[allow(rustdoc::private_intra_doc_links)]
pub fn read_and_wake(path: &Path) {
    if read(path).1
        && let Some(wake) = WAKE.get()
    {
        wake();
    }
}

/// [`read`] on a thread of its own, then the window woken. One read a
/// path at a time: a second ask for a path being read waits on the
/// first. True when a read is on its way, false when [`READERS`] are
/// busy and none was started.
// Its links name this module's private items, for a reader here.
#[allow(rustdoc::private_intra_doc_links)]
pub fn read_then(path: PathBuf) -> bool {
    {
        let mut reading = READING.lock().expect("focus reads");
        if reading.contains(&path) {
            return true;
        }
        if reading.len() >= READERS {
            return false;
        }
        reading.insert(path.clone());
    }
    let started = std::thread::Builder::new()
        .name("focus point".into())
        .spawn({
            let path = path.clone();
            move || {
                read(&path);
                READING.lock().expect("focus reads").remove(&path);
                if let Some(wake) = WAKE.get() {
                    wake();
                }
            }
        });
    if let Err(e) = started {
        tracing::debug!("the focus point read: {e}");
        READING.lock().expect("focus reads").remove(&path);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// A file's point is read once while it stays as it was; one
    /// replaced at the same path is read again; a missing file, and one
    /// that holds no point, are asked once and kept as none.
    #[test]
    fn a_point_is_kept_by_the_file_as_it_was() {
        let dir = greycard_engine::testing::scratch_dir("focus-points");
        let raw = dir.join("IMG_0001.CR3");
        let seed = include_bytes!("../../../fuzz/corpus/focus_point/seed-cr3-two-in-focus");
        std::fs::write(&raw, seed).unwrap();
        let reads = || HEAD_READS.load(Ordering::Relaxed);
        let before = reads();
        let (first, new) = read(&raw);
        assert!(first.is_some() && new);
        assert_eq!(read(&raw), (first, false), "kept");
        assert_eq!(known(&raw), Some(first));
        // Replaced by a file of another length: read again, none.
        std::fs::write(&raw, b"not a raw at all").unwrap();
        assert_eq!(read(&raw), (None, true));
        assert_eq!(read(&raw), (None, false), "none, kept");
        // Missing: asked once.
        let missing = dir.join("IMG_0002.CR3");
        assert_eq!(read(&missing), (None, true));
        assert_eq!(read(&missing), (None, false));
        assert_eq!(known(&missing), Some(None));
        // Another test may read at once; at least these three did.
        assert!(reads() - before >= 3);
        greycard_render::testing::remove_dir_retry(&dir);
    }

    /// One thread a path, and no more than [`READERS`] at once.
    #[test]
    fn reads_off_the_window_are_one_a_path_and_bounded() {
        // As if a slow folder held these reads: the set filled by hand.
        let held: Vec<PathBuf> = (0..READERS)
            .map(|i| PathBuf::from(format!("/nowhere/focus-held/IMG_{i:04}.CR3")))
            .collect();
        let mut reading = READING.lock().unwrap();
        let others = reading.clone();
        reading.clear();
        reading.extend(held.iter().cloned());
        drop(reading);
        assert!(read_then(held[0].clone()), "already on its way: waits");
        let one_more = PathBuf::from("/nowhere/focus-held/IMG_9999.CR3");
        assert!(!read_then(one_more.clone()), "past the bound: none");
        assert!(known(&one_more).is_none(), "nothing read");
        let mut reading = READING.lock().unwrap();
        assert_eq!(reading.len(), READERS, "no thread started");
        reading.clear();
        reading.extend(others);
    }
}
