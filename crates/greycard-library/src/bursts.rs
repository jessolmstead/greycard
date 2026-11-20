//! Bursts: frames of one folder taken seconds apart whose previews are
//! near-identical, found so that each can be put in a stack with its
//! sharpest frame on top.
//!
//! What a frame is judged by is its look ([`Look`]), made once from the
//! camera's embedded preview (a picture's own pixels for a JPEG or a
//! TIFF) and kept in the index's `looks` table beside the frame's row:
//! a signature, the preview's grey at 16 by 16, which two frames of one
//! burst share and two pictures of different things do not; and its
//! focus score (`greycard_core::sharpness::focus_score`), the one the
//! index keeps in the frame's row and the sort reads, higher the
//! sharper the frame. A look is kept with the file's
//! mtime and read back only while the row's mtime is the same, so a
//! file rewritten in place (a DNG's previews updated) is looked at
//! again, and nothing the index's pass writes has to know of it.
//!
//! [`group`] is the rule, over frames as plain values; [`bursts_in`]
//! applies it to a folder of the index, making the looks it lacks.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{TransactionBehavior, params};

use crate::{Library, Result, path_bytes, path_from_bytes};

/// The most seconds between one frame of a burst and the next. A
/// camera's capture time is whole seconds, so two frames 1.9 s apart
/// can read 2 s apart; at 2 every burst and every quick second press
/// (one to five frames a second) is caught, and a frame recomposed or
/// waited for, three seconds and more later, starts a burst of its own.
pub const GAP_SECONDS: i64 = 2;

/// The signature's side: 256 cells, enough to tell a recomposed frame
/// from a held one, too few for focus or a small movement to matter.
pub const SIDE: usize = 16;

/// The most two frames' signatures may differ and still be one burst:
/// the mean absolute difference of their cells, in 8-bit grey levels,
/// after each signature's own mean is taken out (so a burst whose
/// exposure flickers stays one). Set on real frames: neighbors in
/// panning bursts at an airshow measured 0.7 to 7.7, a held burst 1.4,
/// and a sharp picture against its own blur under 2; two different
/// pictures measured 11 at the least (two night skies) and mostly over
/// 30, and the same subject a minute later 36. Frames of open sky
/// minutes apart measured as little as 5.8: there the time parts them.
pub const NEAR: f32 = 8.0;

/// A frame's grey at [`SIDE`] by [`SIDE`], row by row.
pub type Signature = [u8; SIDE * SIDE];

/// What a frame is judged by: see the module.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub signature: Signature,
    /// The focus score, nought to one; higher is sharper. Comparable
    /// between frames of one camera; across cameras only roughly.
    pub focus: f32,
}

/// The `looks` table, part of the schema and the step from schema 12:
/// a row a frame, going with the frame's row, and the file's mtime the
/// look was made from.
pub(crate) const LOOKS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS looks (
    file      INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    mtime     INTEGER NOT NULL,
    signature BLOB NOT NULL,
    focus     REAL NOT NULL
);
";

/// A pixel's grey: Rec. 709's weights over the encoded values, which is
/// near enough to lightness for comparing two frames.
fn grey(p: &image::Rgb<u8>) -> u8 {
    let [r, g, b] = p.0.map(u32::from);
    ((54 * r + 183 * g + 19 * b + 128) >> 8) as u8
}

/// A picture's look.
pub fn look_of(rgb: &image::RgbImage) -> Look {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let grey: Vec<u8> = rgb.pixels().map(grey).collect();
    Look {
        signature: signature_of(&grey, w, h),
        focus: greycard_core::sharpness::focus_score(rgb.as_raw(), w, h).unwrap_or(0.0),
    }
}

/// The grey averaged over a [`SIDE`] by [`SIDE`] grid. A picture
/// narrower than the grid repeats its pixels.
fn signature_of(grey: &[u8], w: usize, h: usize) -> Signature {
    let mut out = [0u8; SIDE * SIDE];
    if w == 0 || h == 0 {
        return out;
    }
    let span = |i: usize, n: usize| {
        let from = (i * n / SIDE).min(n - 1);
        (from, ((i + 1) * n / SIDE).max(from + 1))
    };
    for cy in 0..SIDE {
        let (y0, y1) = span(cy, h);
        for cx in 0..SIDE {
            let (x0, x1) = span(cx, w);
            let mut sum = 0u64;
            for y in y0..y1 {
                sum += grey[y * w + x0..y * w + x1]
                    .iter()
                    .map(|&v| u64::from(v))
                    .sum::<u64>();
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            out[cy * SIDE + cx] = ((sum + n / 2) / n) as u8;
        }
    }
    out
}

/// How far apart two signatures are: the mean absolute difference of
/// their cells, each signature's mean taken out first. See [`NEAR`].
pub fn distance(a: &Signature, b: &Signature) -> f32 {
    let mean = |s: &Signature| s.iter().map(|&v| f32::from(v)).sum::<f32>() / s.len() as f32;
    let shift = mean(a) - mean(b);
    a.iter()
        .zip(b)
        .map(|(&x, &y)| (f32::from(x) - f32::from(y) - shift).abs())
        .sum::<f32>()
        / a.len() as f32
}

/// Whether two signatures are of one burst.
pub fn near(a: &Signature, b: &Signature) -> bool {
    distance(a, b) <= NEAR
}

/// The look of the file at `path`, from the camera's preview in a raw
/// and from the picture itself in a JPEG or a TIFF; none for a file
/// with neither, or one that will not read. The preview is taken as
/// the camera stored it, not turned: the signature compares pixels as
/// they are stored, and a camera held through a burst stores them alike
/// even when its orientation tag flips between frames, which a turn
/// would make two different pictures.
pub fn read_look(path: &Path) -> Option<Look> {
    Some(look_of(&read_preview(path)?))
}

/// The camera's preview in a raw, the picture itself in a JPEG or a
/// TIFF, as stored; none for a file with neither, or one that will not
/// read.
pub(crate) fn read_preview(path: &Path) -> Option<image::RgbImage> {
    Some(if greycard_core::picture::is_picture_path(path) {
        image::ImageReader::open(path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()?
            .to_rgb8()
    } else {
        // Of a raw with several previews, the smallest at the focus
        // score's edge. The score downscales by a whole factor to no
        // more than that edge, so one just past it (2112 pixels) is
        // scored at half the edge: a step of a few hundredths from a
        // full-size preview's, the same for every frame of the camera,
        // which is what a burst and a sort compare.
        greycard_core::decode::preview_path_at_least(
            path,
            greycard_core::sharpness::SCORE_EDGE as u32,
        )
        .ok()??
        .0
    })
}

/// A frame as [`group`] takes it.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    /// When it was taken, in seconds; none for a frame with no time,
    /// which is in no burst.
    pub taken: Option<i64>,
    /// None for a frame with no preview, or one the caller leaves out
    /// (already in a stack): it is in no burst, and a run of frames
    /// either side of it is two runs.
    pub look: Option<Look>,
}

/// One burst: its frames by their place in what [`group`] was given,
/// the sharpest first and the rest in the order they were taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Burst {
    pub frames: Vec<usize>,
}

/// The bursts among one folder's frames: runs in capture order where
/// each frame is at most [`GAP_SECONDS`] after the last and near it by
/// signature ([`near`]), two frames or more. Frames taken in the same
/// second keep the order they were given in, which for a folder in
/// name order is the camera's count.
pub fn group(frames: &[Frame]) -> Vec<Burst> {
    let mut order: Vec<usize> = (0..frames.len())
        .filter(|&i| frames[i].taken.is_some())
        .collect();
    order.sort_by_key(|&i| frames[i].taken);
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for i in order {
        let joins = run.last().is_some_and(|&last| {
            let (a, b) = (&frames[last], &frames[i]);
            let close = b
                .taken
                .zip(a.taken)
                .is_some_and(|(b, a)| b - a <= GAP_SECONDS);
            let alike = a
                .look
                .as_ref()
                .zip(b.look.as_ref())
                .is_some_and(|(a, b)| near(&a.signature, &b.signature));
            close && alike
        });
        if !joins {
            close_run(frames, &mut run, &mut out);
        }
        run.push(i);
    }
    close_run(frames, &mut run, &mut out);
    out
}

/// A run of two or more made a burst, its sharpest frame moved to the
/// front; the earlier frame wins a tie.
fn close_run(frames: &[Frame], run: &mut Vec<usize>, out: &mut Vec<Burst>) {
    if run.len() > 1 {
        let focus = |i: usize| frames[i].look.as_ref().map_or(0.0, |l| l.focus);
        let top = (0..run.len())
            .reduce(|best, j| {
                if focus(run[j]) > focus(run[best]) {
                    j
                } else {
                    best
                }
            })
            .unwrap_or(0);
        let first = run.remove(top);
        run.insert(0, first);
        out.push(Burst {
            frames: std::mem::take(run),
        });
    }
    run.clear();
}

/// One frame of a folder as the index holds it for the bursts.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// As the index keys it.
    pub path: PathBuf,
    /// The content hash, which a stack's reference keeps.
    pub hash: String,
    pub mtime: i64,
    /// The capture time, the sidecar's shift applied, as the index's
    /// `taken_at` has it.
    pub taken: Option<String>,
    /// The look kept for the file as it is now; none when there is
    /// none yet, or the file changed since.
    pub look: Option<Look>,
}

impl Row {
    /// The frame [`group`] takes.
    fn frame(&self) -> Frame {
        Frame {
            // A date with no time cannot be seconds from another.
            taken: self
                .taken
                .as_deref()
                .filter(|t| t.len() > 10)
                .and_then(greycard_core::exif::seconds_of),
            look: self.look.clone(),
        }
    }
}

impl Library {
    /// The frames directly in one folder (canonical, as the index keys
    /// it), the missing and the camera's JPEGs beside raws left out, by
    /// name, each with its look when one is kept for its mtime.
    pub fn burst_rows(&self, folder: &Path) -> Result<Vec<Row>> {
        let sql = format!(
            "SELECT files.path, files.hash, files.mtime, files.taken_at, looks.signature, \
             coalesce({}, looks.focus) FROM files \
             LEFT JOIN looks ON looks.file = files.id AND looks.mtime = files.mtime \
             WHERE files.folder = ?1 AND files.missing_since IS NULL{} ORDER BY files.name",
            crate::FOCUS_SQL,
            self.frames_only_in_folder()
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![path_bytes(folder)], |r| {
            let signature: Option<Vec<u8>> = r.get(4)?;
            let focus: Option<f64> = r.get(5)?;
            Ok(Row {
                path: path_from_bytes(&r.get::<_, Vec<u8>>(0)?),
                hash: r.get(1)?,
                mtime: r.get(2)?,
                taken: r.get(3)?,
                look: signature
                    .and_then(|s| Signature::try_from(s.as_slice()).ok())
                    .zip(focus)
                    .map(|(signature, focus)| Look {
                        signature,
                        focus: focus as f32,
                    }),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Keep these looks, each against the file's row at its path while
    /// the row's mtime is still the one the look was made from: a file
    /// that changed meanwhile keeps none, and is looked at again.
    pub fn keep_looks(&mut self, looks: &[(PathBuf, i64, Look)]) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO looks (file, mtime, signature, focus) \
                 SELECT id, mtime, ?, ? FROM files WHERE path = ? AND mtime = ?",
            )?;
            for (path, mtime, look) in looks {
                stmt.execute(params![
                    look.signature.as_slice(),
                    f64::from(look.focus),
                    path_bytes(path),
                    mtime
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// What [`bursts_in`] found.
#[derive(Debug, Default)]
pub struct Found {
    /// Each burst's rows, the sharpest first.
    pub bursts: Vec<Vec<Row>>,
    /// How many frames were looked at for the first time.
    pub looked: usize,
}

/// The bursts of one folder of the index (canonical): the looks it
/// lacks made from the files on `threads` threads and kept, then
/// [`group`]. A frame `leave` says yes to (one already in a stack) is
/// in none, and splits a run it stands in. `beat` is called after each
/// preview read, from the reading threads, so a caller can tell a long
/// run from one stuck on a file that stopped answering.
pub fn bursts_in(
    lib: &mut Library,
    folder: &Path,
    leave: &dyn Fn(&Path) -> bool,
    beat: &(dyn Fn() + Sync),
    threads: usize,
) -> Result<Found> {
    let mut rows = lib.burst_rows(folder)?;
    let wanted: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].look.is_none() && !leave(&rows[i].path))
        .collect();
    let made = read_looks(&rows, &wanted, beat, threads);
    let keep: Vec<(PathBuf, i64, Look)> = made
        .iter()
        .map(|(i, look)| (rows[*i].path.clone(), rows[*i].mtime, look.clone()))
        .collect();
    lib.keep_looks(&keep)?;
    let looked = made.len();
    for (i, look) in made {
        rows[i].look = Some(look);
    }
    let frames: Vec<Frame> = rows
        .iter()
        .map(|r| {
            let mut f = r.frame();
            if leave(&r.path) {
                f.look = None;
            }
            f
        })
        .collect();
    let bursts = group(&frames)
        .into_iter()
        .map(|b| b.frames.into_iter().map(|i| rows[i].clone()).collect())
        .collect();
    Ok(Found { bursts, looked })
}

/// The looks of rows `wanted`, read on up to `threads` threads, `beat`
/// called after each; a file with no preview is left out.
fn read_looks(
    rows: &[Row],
    wanted: &[usize],
    beat: &(dyn Fn() + Sync),
    threads: usize,
) -> Vec<(usize, Look)> {
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(wanted.len()));
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, wanted.len().max(1)) {
            s.spawn(|| {
                while let Some(&i) = wanted.get(next.fetch_add(1, Ordering::Relaxed)) {
                    match read_look(&rows[i].path) {
                        Some(look) => out.lock().expect("looks").push((i, look)),
                        None => log::debug!("{}: no preview to look at", rows[i].path.display()),
                    }
                    beat();
                }
            });
        }
    });
    let mut out = out.into_inner().expect("looks");
    out.sort_by_key(|(i, _)| *i);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with detail at every scale: rings and a checker.
    fn scene(w: u32, h: u32, phase: f32) -> image::RgbImage {
        image::RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32, y as f32);
            let r = ((fx - w as f32 * 0.4).hypot(fy - h as f32 * 0.5) * 0.15 + phase).sin();
            let check = if (x / 8 + y / 8) % 2 == 0 {
                40.0
            } else {
                -40.0
            };
            let v = (128.0 + 70.0 * r + check).clamp(0.0, 255.0) as u8;
            image::Rgb([v, v.saturating_sub(10), v / 2 + 60])
        })
    }

    /// A box blur of radius `r`, twice: near enough to a lens's.
    fn blurred(img: &image::RgbImage, r: i64) -> image::RgbImage {
        let pass = |src: &image::RgbImage, dx: i64, dy: i64| {
            let (w, h) = (src.width() as i64, src.height() as i64);
            image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
                let mut sum = [0u32; 3];
                for k in -r..=r {
                    let sx = (x as i64 + k * dx).clamp(0, w - 1) as u32;
                    let sy = (y as i64 + k * dy).clamp(0, h - 1) as u32;
                    for (s, v) in sum.iter_mut().zip(src.get_pixel(sx, sy).0) {
                        *s += u32::from(v);
                    }
                }
                let n = (2 * r + 1) as u32;
                image::Rgb(sum.map(|s| (s / n) as u8))
            })
        };
        let once = pass(&pass(img, 1, 0), 0, 1);
        pass(&pass(&once, 1, 0), 0, 1)
    }

    /// The sharp frame scores higher than its blur; the two are one
    /// burst by signature, and a picture of something else is not.
    #[test]
    fn the_sharp_frame_scores_higher_and_its_blur_is_near() {
        let sharp = scene(600, 400, 0.0);
        let soft = blurred(&sharp, 2);
        let (a, b) = (look_of(&sharp), look_of(&soft));
        assert!(a.focus > b.focus + 0.05, "{} against {}", a.focus, b.focus);
        let d = distance(&a.signature, &b.signature);
        assert!(d < 2.0, "{d}");
        assert!(near(&a.signature, &b.signature));
        // Brighter by a stop's worth of levels: still one burst.
        let brighter = image::RgbImage::from_fn(600, 400, |x, y| {
            image::Rgb(sharp.get_pixel(x, y).0.map(|v| v.saturating_add(20)))
        });
        assert!(near(&a.signature, &look_of(&brighter).signature));
        // Another picture: the scene turned over.
        let other =
            image::imageops::flip_vertical(&image::imageops::rotate180(&scene(600, 400, 2.0)));
        let d = distance(&a.signature, &look_of(&other).signature);
        assert!(d > 1.5 * NEAR, "{d}");
        // A flat picture has no focus, and a tiny one no Laplacian.
        let flat = image::RgbImage::from_pixel(64, 48, image::Rgb([90, 90, 90]));
        assert_eq!(look_of(&flat).focus, 0.0);
        assert_eq!(look_of(&image::RgbImage::new(2, 2)).focus, 0.0);
    }

    fn sig(v: u8) -> Signature {
        let mut s = [0u8; SIDE * SIDE];
        for (i, c) in s.iter_mut().enumerate() {
            *c = if (i + v as usize).is_multiple_of(3) {
                200
            } else {
                30
            };
        }
        s
    }

    fn frame(taken: i64, scene: u8, focus: f32) -> Frame {
        Frame {
            taken: Some(taken),
            look: Some(Look {
                signature: sig(scene),
                focus,
            }),
        }
    }

    #[test]
    fn a_burst_is_close_in_time_and_alike() {
        let frames = vec![
            // A burst of three, the middle sharpest.
            frame(100, 0, 5.0),
            frame(101, 0, 9.0),
            frame(103, 0, 7.0),
            // Three seconds on: a burst of its own with the next.
            frame(106, 0, 1.0),
            frame(106, 0, 1.0),
            // Within the gap but another picture.
            frame(107, 1, 1.0),
            // Alone.
            frame(200, 0, 1.0),
        ];
        let got: Vec<Vec<usize>> = group(&frames).into_iter().map(|b| b.frames).collect();
        // The sharpest first, the rest in time; a tie keeps the first.
        assert_eq!(got, vec![vec![1, 0, 2], vec![3, 4]]);
    }

    #[test]
    fn a_frame_without_a_look_or_a_time_splits_or_stays_out() {
        let mut frames = vec![
            frame(10, 0, 1.0),
            frame(11, 0, 1.0),
            // Already stacked, or no preview: the run breaks here.
            Frame {
                taken: Some(12),
                look: None,
            },
            frame(13, 0, 1.0),
            frame(14, 0, 2.0),
            // No time: in nothing, and not in the way either.
            Frame {
                taken: None,
                ..frame(0, 0, 1.0)
            },
        ];
        let got: Vec<Vec<usize>> = group(&frames).into_iter().map(|b| b.frames).collect();
        assert_eq!(got, vec![vec![0, 1], vec![4, 3]]);
        // Given out of order: sorted by time, a same-second pair in the
        // order given.
        frames.swap(0, 1);
        frames.reverse();
        let got: Vec<Vec<usize>> = group(&frames).into_iter().map(|b| b.frames).collect();
        assert_eq!(got, vec![vec![4, 5], vec![1, 2]]);
        assert!(group(&[]).is_empty());
    }

    /// A schema 12 library gains the looks table in place, its rows
    /// kept; a look is read back while the file's mtime holds and not
    /// after, and goes with its row.
    #[test]
    fn the_looks_table_comes_with_schema_13() {
        use crate::fixture::{R5, write_frame};
        let dir = crate::index::tests::scratch("looks");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let db = dir.join("library.sqlite");
        let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
        write_frame(&a, &R5, 1);
        write_frame(&b, &R5, 2);
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_tree(&shoot, &mut |_| {}).unwrap();
            lib.conn
                .execute_batch(
                    "DROP TABLE looks; ALTER TABLE files DROP COLUMN focus; \
                     PRAGMA user_version = 12;",
                )
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let version: i32 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        let rows = lib.burst_rows(&shoot).unwrap();
        assert_eq!(rows.len(), 2, "the rows kept");
        assert!(rows.iter().all(|r| r.look.is_none()));
        assert_eq!(rows[0].taken.as_deref(), Some("2024-08-24 15:32:39"));

        // Made once, read back.
        let beats = AtomicUsize::new(0);
        let beat = || {
            beats.fetch_add(1, Ordering::Relaxed);
        };
        let found = bursts_in(&mut lib, &shoot, &|_| false, &beat, 2).unwrap();
        assert_eq!(found.looked, 2);
        assert_eq!(beats.into_inner(), 2, "a beat a preview read");
        assert_eq!(found.bursts.len(), 1, "one second, one picture");
        let again = bursts_in(&mut lib, &shoot, &|_| false, &|| {}, 2).unwrap();
        assert_eq!(again.looked, 0);
        assert_eq!(found.bursts, again.bursts);
        // A frame left out is in none.
        let left = bursts_in(&mut lib, &shoot, &|p| p == a, &|| {}, 2).unwrap();
        assert!(left.bursts.is_empty());

        // The file rewritten: its look is stale until made again.
        write_frame(&b, &R5, 3);
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&b)
            .unwrap()
            .set_modified(later)
            .unwrap();
        lib.index_tree(&shoot, &mut |_| {}).unwrap();
        let rows = lib.burst_rows(&shoot).unwrap();
        assert!(rows[0].look.is_some() && rows[1].look.is_none());
        // Gone with its row.
        std::fs::remove_file(&a).unwrap();
        lib.index_tree(&shoot, &mut |_| {}).unwrap();
        lib.prune_missing().unwrap();
        let count = |sql: &str| -> i64 { lib.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT count(*) FROM looks WHERE file NOT IN (SELECT id FROM files)"),
            0,
            "a's went with its row"
        );
        assert_eq!(
            count("SELECT count(*) FROM looks"),
            1,
            "b's stale one waits"
        );
        drop(lib);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The consecutive frames of a folder of raws, as the rule sees
    /// them: the seconds between, the signatures' distance, and each
    /// focus score. For setting [`NEAR`] on real bursts:
    /// `GREYCARD_BURSTS=<folder> cargo test -p greycard-library
    /// measure_bursts -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_bursts() {
        let Ok(dir) = std::env::var("GREYCARD_BURSTS") else {
            return;
        };
        let dir = crate::canonical(Path::new(&dir)).unwrap();
        let scratch = crate::index::tests::scratch("measure-bursts");
        let mut lib = Library::open(&scratch.join("library.sqlite")).unwrap();
        lib.index_tree(&dir, &mut |_| {}).unwrap();
        let rows = lib.burst_rows(&dir).unwrap();
        let mut last: Option<(&Row, Option<i64>, Look)> = None;
        for row in &rows {
            let started = std::time::Instant::now();
            let name = row.path.file_name().unwrap().to_string_lossy();
            let Some(look) = read_look(&row.path) else {
                println!("{name}: no preview");
                continue;
            };
            let ms = started.elapsed().as_millis();
            let taken = row.frame().taken;
            if let Some((lr, lt, ll)) = &last {
                println!(
                    "{} -> {name}: {:?} s, distance {:.2}",
                    lr.path.file_name().unwrap().to_string_lossy(),
                    taken.zip(*lt).map(|(a, b)| a - b),
                    distance(&ll.signature, &look.signature)
                );
            }
            println!("{name}: focus {:.1} ({ms} ms)", look.focus);
            last = Some((row, taken, look));
        }
        drop(lib);
        greycard_core::testing::remove_dir_retry(&scratch);
    }

    /// What the focus score costs over a folder: a fresh pass, which
    /// scores nothing; the scorer after it; and the bytes read, the
    /// pass and the scorer apart, with the folder taken for a network
    /// mount's. `GREYCARD_FOCUS_PASS=<folder> cargo test -p
    /// greycard-library --release measure_focus_pass -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn measure_focus_pass() {
        let Ok(dir) = std::env::var("GREYCARD_FOCUS_PASS") else {
            return;
        };
        let dir = crate::canonical(Path::new(&dir)).unwrap();
        // Bytes this process has read, cached or not.
        let read = || -> u64 {
            std::fs::read_to_string("/proc/self/io")
                .unwrap()
                .lines()
                .find_map(|l| l.strip_prefix("rchar: "))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        for far in [false, true] {
            let scratch = crate::index::tests::scratch("measure-focus-pass");
            let mut lib = Library::open(&scratch.join("library.sqlite")).unwrap();
            if far {
                lib.set_network(|_| true, Default::default());
            }
            let (bytes, started) = (read(), std::time::Instant::now());
            let report = lib.index_tree(&dir, &mut |_| {}).unwrap();
            let pass = (started.elapsed().as_secs_f64(), read() - bytes);
            let (bytes, started) = (read(), std::time::Instant::now());
            let scored =
                crate::score::score(&mut lib, crate::score::THREADS, &|| false, &mut |_| {})
                    .unwrap()
                    .scored;
            let scoring = (started.elapsed().as_secs_f64(), read() - bytes);
            println!(
                "{}: pass {:.2} s, {} MB, {} added; scorer {:.2} s, {} MB, {scored} scored",
                if far { "network" } else { "local" },
                pass.0,
                pass.1 >> 20,
                report.added,
                scoring.0,
                scoring.1 >> 20,
            );
            if !far {
                let mut stmt = lib
                    .conn
                    .prepare("SELECT name, focus FROM files ORDER BY focus DESC")
                    .unwrap();
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))
                    .unwrap();
                for row in rows {
                    let (name, focus) = row.unwrap();
                    println!("{name}: {focus:.3}");
                }
            }
            drop(lib);
            greycard_core::testing::remove_dir_retry(&scratch);
        }
    }
}
