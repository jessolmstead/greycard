//! Helpers every crate's tests share: a scratch folder's removal that
//! Windows would also allow. Behind `cfg(test)` or the `testing`
//! feature, which the other crates' dev-dependencies turn on, so it
//! never reaches a release build. And what the DCP fuzz target asserts,
//! which the core's tests run too.

pub mod dcp;

/// A test's folder removed, tried again for a while when the system
/// refuses: on Windows a file a thread has just let go of can still be
/// held for a moment after the thread was joined. The last refusal is
/// the panic.
///
/// Linux removes a file that is still open, Windows does not, and a
/// test that leaves the index's reader or a mapped raw under its
/// folder passes here and fails there, twenty minutes later and
/// without a name. So on Linux the removal first waits for what
/// Windows would refuse, the SQLite files and the memory maps this
/// process still holds under the folder, and names them when they
/// are still held at the end of the wait. Plain open files are not
/// counted: Rust opens them with delete sharing, and Windows removes
/// those as Linux does.
///
/// Called while the thread unwinds (from a drop guard in a failing
/// test) the removal is tried once and never panics: a second panic
/// would abort the run and hide the first failure.
pub fn remove_dir_retry(dir: &std::path::Path) {
    if std::thread::panicking() {
        // remove_dir_all allowed: best effort while unwinding.
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    let start = std::time::Instant::now();
    let limit = std::time::Duration::from_secs(20);
    loop {
        let held = still_held_under(dir);
        if !held.is_empty() {
            if start.elapsed() < limit {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            panic!(
                "{} is still held by this process, which Windows would refuse to remove: \
                 {held:?}; close the index reader (testing::remove_scratch) or stop the \
                 worker before removing the folder",
                dir.display()
            );
        }
        // remove_dir_all allowed: the helper itself.
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if start.elapsed() < limit => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("{} could not be removed: {e}", dir.display()),
        }
    }
}

/// The files under `dir` this process holds in a way Windows would
/// refuse to delete: SQLite databases and their journals open as
/// descriptors (SQLite opens without delete sharing), and any file
/// mapped into memory (a mapped file cannot be deleted there). From
/// `/proc`, so Linux only; elsewhere the removal itself is the check.
#[cfg(target_os = "linux")]
fn still_held_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut held = Vec::new();
    if let Ok(fds) = std::fs::read_dir("/proc/self/fd") {
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let name = target.to_string_lossy();
            let sqlite = [".sqlite", ".sqlite-wal", ".sqlite-shm", ".sqlite-journal"]
                .iter()
                .any(|end| name.ends_with(end));
            if sqlite && target.starts_with(&dir) && !held.contains(&target) {
                held.push(target);
            }
        }
    }
    if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
        for line in maps.lines() {
            let Some(at) = line.find('/') else { continue };
            let path = std::path::PathBuf::from(line[at..].trim_end_matches(" (deleted)"));
            if path.starts_with(&dir) && !held.contains(&path) {
                held.push(path);
            }
        }
    }
    held
}

#[cfg(not(target_os = "linux"))]
fn still_held_under(_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    Vec::new()
}

/// A file or a folder renamed, tried again for a while when the system
/// refuses: on Windows a folder cannot be renamed while any file under
/// it is open, and a test's thumbnail pool or worker may still be
/// reading one when the test gets here. The same retry as
/// [`crate::fs::rename_into_place`], on any error and for 20 s rather
/// than two, since a test's own threads can hold a file longer than
/// another process's moment. The last refusal is the panic.
pub fn rename_retry(from: impl AsRef<std::path::Path>, to: impl AsRef<std::path::Path>) {
    let (from, to) = (from.as_ref(), to.as_ref());
    let waits = [std::time::Duration::from_millis(50); 400];
    if let Err(e) = crate::fs::retry(|| std::fs::rename(from, to), &waits, |_| true, from) {
        panic!("{} could not be renamed: {e}", from.display());
    }
}

/// Wait until the file system stamps a new file later than `path`'s
/// own clock, so a change made to it next moves that clock: a few
/// milliseconds where the clock is fine, up to its step where it is
/// coarse (two seconds on FAT, one on HFS+, a tick of 15.6 ms on NTFS).
/// The probe is written beside the folder `path` is or is in, never in
/// it.
pub fn a_moment(path: &std::path::Path) {
    let was = std::fs::metadata(path).unwrap().modified().unwrap();
    let folder = match path.is_dir() {
        true => path,
        false => path.parent().unwrap(),
    };
    let probe = folder.with_extension("probe");
    for _ in 0..300 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&probe, b"").unwrap();
        let now = std::fs::metadata(&probe).unwrap().modified().unwrap();
        std::fs::remove_file(&probe).unwrap();
        if now > was {
            return;
        }
    }
    panic!("the clock at {} did not move", path.display());
}

/// A camera's frame shot at 3:2 on a 4:3 sensor, as an Olympus body
/// keeps it: the 5184 by 3888 area at (8, 8) and the band of 3456 rows
/// 216 below its top, under the orientation tag `orientation`.
pub fn three_two_frame(orientation: crate::raw::Orientation) -> crate::raw::CameraFrame {
    use crate::raw::{CameraFrame, Rect};
    CameraFrame {
        area: Rect {
            x: 8,
            y: 8,
            width: 5184,
            height: 3888,
        },
        frame: Rect {
            x: 8,
            y: 224,
            width: 5184,
            height: 3456,
        },
        orientation,
    }
}

/// The head of an OM System raw as the camera's-frame reader takes it,
/// and nothing else: a little-endian TIFF whose IFD0 holds the Make and
/// the orientation tag (`orientation`, EXIF's 1 to 8), whose Exif IFD
/// holds a maker note with an Image Processing sub-IFD of the valid
/// area `area` (left, top, width, height) and the AspectFrame `frame`
/// (x0, y0, x1, y1, inclusive, in the area). No image data: a file the
/// index reads the maker's tags of, and the decoder will not open.
pub fn olympus_head(orientation: u16, area: [u32; 4], frame: [u16; 4]) -> Vec<u8> {
    fn entry(out: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: [u8; 4]) {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&typ.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&value);
    }
    let short2 = |a: u16, b: u16| {
        let mut v = [0u8; 4];
        v[..2].copy_from_slice(&a.to_le_bytes());
        v[2..].copy_from_slice(&b.to_le_bytes());
        v
    };
    let make = b"OM Digital Solutions\0";
    // IFD0 at 8: three entries.
    let make_at = 8 + 2 + 3 * 12 + 4;
    let exif_at = make_at + make.len() + make.len() % 2;
    let note_at = exif_at + 2 + 12 + 4;
    // The note: its header, its main IFD at 16, the Image Processing
    // IFD at 34, the AspectFrame's eight bytes after it; offsets from
    // the note's start.
    let ip = 34u32;
    let frame_at = ip + 2 + 5 * 12 + 4;
    let mut note = b"OM SYSTEM\0\0\0II\x04\0".to_vec();
    note.extend_from_slice(&1u16.to_le_bytes());
    entry(&mut note, 0x2040, 4, 1, ip.to_le_bytes());
    note.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(note.len() as u32, ip);
    note.extend_from_slice(&5u16.to_le_bytes());
    entry(&mut note, 0x0612, 3, 2, short2(area[0] as u16, 0));
    entry(&mut note, 0x0613, 3, 2, short2(area[1] as u16, 0));
    entry(&mut note, 0x0614, 4, 1, area[2].to_le_bytes());
    entry(&mut note, 0x0615, 4, 1, area[3].to_le_bytes());
    entry(&mut note, 0x1113, 3, 4, frame_at.to_le_bytes());
    note.extend_from_slice(&0u32.to_le_bytes());
    for v in frame {
        note.extend_from_slice(&v.to_le_bytes());
    }
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    entry(
        &mut out,
        0x010f,
        2,
        make.len() as u32,
        (make_at as u32).to_le_bytes(),
    );
    entry(&mut out, 0x0112, 3, 1, short2(orientation, 0));
    entry(&mut out, 0x8769, 4, 1, (exif_at as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(make);
    if make.len() % 2 == 1 {
        out.push(0);
    }
    assert_eq!(out.len(), exif_at);
    out.extend_from_slice(&1u16.to_le_bytes());
    entry(
        &mut out,
        0x927c,
        7,
        note.len() as u32,
        (note_at as u32).to_le_bytes(),
    );
    out.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(out.len(), note_at);
    out.extend_from_slice(&note);
    out
}
