//! Helpers the tests here share with the editor's own: a scratch
//! folder's removal, a camera profile for the masks' white balances,
//! a camera body by name and a watermark picture. Built for this
//! crate's tests and, behind the `testing` feature, for the editor's.

use std::path::PathBuf;

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
pub fn remove_dir_retry(dir: &std::path::Path) {
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

/// A camera profile with a warm and a daylight calibration, the
/// matrices of an ordinary sensor (a little wider than Rec.2020 in its
/// reds, a little narrower in its blues), for the masks' white
/// balances in tests that have no raw: the profile and a develop at
/// 5500 K, what [`greycard_edit::WhiteShift`] is made from.
pub fn white_shift() -> greycard_edit::WhiteShift {
    use greycard_core::raw::{
        Calibration, CfaPattern, LevelPattern, Levels, Samples, SensorLayout,
    };
    let calibration = |illuminant: u16, m: [f32; 9]| Calibration {
        illuminant,
        color_matrix: m.to_vec(),
        forward_matrix: None,
    };
    let frame = greycard_core::RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        width: 2,
        height: 2,
        channels: 1,
        layout: SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(vec![0; 4]),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(1.0, 1),
        },
        as_shot_coefficients: Some([2.0, 1.0, 1.6]),
        calibrations: vec![
            calibration(
                17,
                [
                    1.1564, -0.4626, -0.1081, -0.4229, 1.2260, 0.2187, -0.0711, 0.1500, 0.6268,
                ],
            ),
            calibration(
                21,
                [
                    0.9766, -0.2953, -0.1254, -0.4276, 1.2116, 0.2433, -0.0437, 0.1336, 0.5131,
                ],
            ),
        ],
        crop: None,
        orientation: Default::default(),
        shot: Default::default(),
    };
    let profile = greycard_core::color::profile_from_frame(&frame).expect("two calibrations");
    let base = greycard_core::color::white_balance_at(
        &profile,
        greycard_core::TempTint {
            cct: 5500.0,
            duv: 0.0,
        },
    )
    .expect("daylight resolves");
    greycard_edit::WhiteShift::new(profile, &base)
}

/// A picture `size` high and twice as wide, clear but for a white
/// bar across its middle half, written as a PNG under `dir`.
pub fn bar_png(dir: &std::path::Path, size: u32) -> PathBuf {
    let mut img = image::RgbaImage::new(size * 2, size);
    for (_, y, p) in img.enumerate_pixels_mut() {
        let inside = y >= size / 4 && y < size * 3 / 4;
        p.0 = if inside {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 0]
        };
    }
    let path = dir.join("mark.png");
    img.save(&path).unwrap();
    path
}

/// The body the GFX 100S II's raws name, as the decoder reads them.
pub const GFX: (&str, &str) = ("FUJIFILM", "GFX100S II");
