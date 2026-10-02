//! Local previews: a mid-size picture of each frame under a root, kept
//! in the thumbnail cache under the thumbnails' own key with a long
//! edge of [`SIZE`], so the culling loupe and the compare view can show
//! a frame whose file is out of reach (its root offline) or slow to
//! read (on a network mount) without reading it.
//!
//! A preview is what the thumbnails are made from, larger: the camera's
//! embedded JPEG, or the picture itself for a JPEG, PNG or TIFF, or the
//! raw's quick develop when the file has no preview in it, turned by
//! the camera's orientation tag and nothing of the edit. It is made
//! once a frame and made again only when the file's stamp moves, which
//! is the key's to see; the eviction takes it with the rest.
//!
//! Who makes it: the thumbnails' pool, behind the thumbnails. A
//! thumbnail's lookup already has the file's hash and stamp in hand,
//! and notes here a frame under a root with no preview kept
//! ([`Previews::note`]); the pool then makes it at its lowest priority
//! ([`Previews::make`]). A frame the culling loupe decodes from its file
//! meanwhile has its preview made from the picture in hand
//! ([`Previews::make_from`]), with no second read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use greycard_core::raw::Orientation;
use greycard_library::thumbs::{Tag, Thumb};

use crate::worker::{THUMB_RECIPE, ThumbCache, file_stat, thumb_tag, thumbnail};

/// A preview's long edge. Past a 1440p loupe's fitted view and half a
/// 4K one, so a fitted frame is not magnified on a desktop screen; at
/// 1:1 it is a preview and says so. A decision for the user to move:
/// 2560 would fill a 4K loupe at about 1.6 times the bytes.
pub const SIZE: u32 = 2048;

// A thumbnail is never made at a preview's size, so the two can never
// be each other's entry.
const _: () = assert!(SIZE > crate::grid::MAX_RENDER);

/// A preview's JPEG quality: a little under the thumbnails' 90, where
/// the bytes of a picture this size start to count (about a third of a
/// megabyte for a 24 MP frame's preview), and still well past where
/// culling for focus and expression can see the difference.
pub const QUALITY: u8 = 88;

/// The share of the previews' own cap (`preview_cache_mb`, apart from
/// the thumbnails') a preview may be made into: past it, none is made,
/// so the previews never evict one another round and round to make room
/// for themselves at every open of the roots' view. At the default 8 GB
/// that is 6 GB of previews, some 13,000 frames at the 472 KB a frame
/// measured on the samples; the rest of a library larger than that
/// culls from its files as before.
pub const ROOM: f64 = 0.75;

/// How many previews made from a picture the loupe decoded may be in
/// hand at once on rayon's pool. Each holds the whole camera picture
/// (72 MB at 24 MP, 135 at 45), and a develop begun meanwhile queues its
/// work behind them on the same pool; past this many the loupe skips
/// the preview, leaves it owed, and the thumbnails' pool makes it
/// later from the file.
pub const FROM_PICTURES_AT_ONCE: usize = 2;

/// A preview owed: the key the thumbnail's lookup found, and the
/// file's stat then, which a making checks again so a file changed
/// since is not kept under its old key.
#[derive(Debug, Clone)]
pub(crate) struct Owed {
    hash: String,
    tag: Tag,
    stat: (u64, u64),
}

/// A preview made and kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Made {
    pub width: u32,
    pub height: u32,
    /// The entry's bytes on disk.
    pub bytes: u64,
    pub seconds: f64,
}

/// The local previews: the cache they are kept in, the roots whose
/// frames get one, and the frames owed one.
pub(crate) struct Previews {
    cache: ThumbCache,
    roots: Mutex<Vec<PathBuf>>,
    owed: Mutex<HashMap<PathBuf, Owed>>,
    /// Previews being made from a picture in hand: see
    /// [`FROM_PICTURES_AT_ONCE`].
    from_pictures: AtomicUsize,
}

/// One preview being made from a picture in hand, counted while it is
/// held.
pub(crate) struct InHand(Arc<Previews>);

impl InHand {
    pub(crate) fn previews(&self) -> &Previews {
        &self.0
    }
}

impl Drop for InHand {
    fn drop(&mut self) {
        self.0.from_pictures.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Previews {
    pub(crate) fn new(cache: ThumbCache) -> Self {
        Self {
            cache,
            roots: Mutex::new(Vec::new()),
            owed: Mutex::new(HashMap::new()),
            from_pictures: AtomicUsize::new(0),
        }
    }

    /// The preview owed `path`, taken to be made from a picture in hand
    /// on another thread, with its place among the
    /// [`FROM_PICTURES_AT_ONCE`]; `None`, and the preview left owed for
    /// the pool, when that many are in hand already.
    pub(crate) fn take_for_picture(self: &Arc<Self>, path: &Path) -> Option<(Owed, InHand)> {
        self.from_pictures
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < FROM_PICTURES_AT_ONCE).then_some(n + 1)
            })
            .ok()?;
        let in_hand = InHand(self.clone());
        let owed = self.take(path)?;
        Some((owed, in_hand))
    }

    /// The roots whose frames get a preview: the ones online when the
    /// list was opened. A frame of a folder under no root gets none;
    /// its file is where the user put it and read from there.
    pub(crate) fn set_roots(&self, roots: Vec<PathBuf>) {
        *self.roots.lock().expect("preview roots") = roots;
        // What was owed was the last list's; this list's thumbnails
        // note their own.
        self.owed.lock().expect("previews owed").clear();
    }

    fn under_a_root(&self, path: &Path) -> bool {
        self.roots
            .lock()
            .expect("preview roots")
            .iter()
            .any(|r| path.starts_with(r))
    }

    /// A thumbnail's lookup found `path`'s key: note the frame as owed
    /// a preview when it is under a root and the cache has none for
    /// that key. One stat of the cache's own entry; nothing of the file.
    pub(crate) fn note(&self, path: &Path, stat: (u64, u64), hash: &str) {
        if !self.under_a_root(path) {
            return;
        }
        let tag = thumb_tag(stat);
        let kept = self
            .cache
            .lock()
            .expect("thumbnail cache")
            .as_ref()
            .is_none_or(|c| c.cap() == 0 || c.preview_cap() == 0 || c.has(hash, SIZE, tag));
        let mut owed = self.owed.lock().expect("previews owed");
        if kept {
            owed.remove(path);
        } else {
            owed.insert(
                path.to_path_buf(),
                Owed {
                    hash: hash.to_string(),
                    tag,
                    stat,
                },
            );
        }
    }

    /// Whether `path` is owed a preview: a question of memory.
    pub(crate) fn owes(&self, path: &Path) -> bool {
        self.owed.lock().expect("previews owed").contains_key(path)
    }

    /// The preview owed `path`, taken off the list, when it is still
    /// under a root and the cache is known to have room for it: see
    /// [`ROOM`]. Until the cache's startup count is in, nothing is
    /// made, so no preview's write walks the cache under its lock.
    pub(crate) fn take(&self, path: &Path) -> Option<Owed> {
        let owed = self.owed.lock().expect("previews owed").remove(path)?;
        if !self.under_a_root(path) {
            return None;
        }
        let room = self
            .cache
            .lock()
            .expect("thumbnail cache")
            .as_ref()
            .is_some_and(|c| {
                c.cap() > 0
                    && c.preview_cap() > 0
                    && c.known_split()
                        .is_some_and(|u| (u.previews.bytes as f64) < c.preview_cap() as f64 * ROOM)
            });
        if !room {
            tracing::debug!(
                "preview {}: the cache has no room for previews",
                path.display()
            );
            return None;
        }
        Some(owed)
    }

    /// Make `path`'s preview from its file, if it is owed one: the
    /// pool's work, behind the thumbnails. `None` when none was owed,
    /// the cache had no room, one was kept meanwhile, or the file
    /// changed under it.
    pub(crate) fn make(&self, path: &Path) -> anyhow::Result<Option<Made>> {
        let Some(owed) = self.take(path) else {
            return Ok(None);
        };
        if self.kept_already(&owed) || file_stat(path) != Some(owed.stat) {
            return Ok(None);
        }
        let started = Instant::now();
        // The camera's picture, or the picture file itself; a raw with
        // neither goes the thumbnail's way, through a quick develop.
        let thumb = match crate::cull::camera_picture(path) {
            Ok((picture, orientation)) => sized(&picture, orientation),
            Err(_) => {
                let (width, height, rgb) = thumbnail(path, SIZE)?;
                Thumb { width, height, rgb }
            }
        };
        Ok(self.keep(path, &owed, thumb, started))
    }

    /// Make `path`'s preview from the camera's picture decoded for
    /// something else (the culling loupe), if it is owed one: no read
    /// of the file beyond a stat to see it has not changed.
    #[cfg(test)]
    pub(crate) fn make_from(
        &self,
        path: &Path,
        picture: &image::RgbImage,
        orientation: Orientation,
    ) -> Option<Made> {
        let owed = self.take(path)?;
        self.make_taken(path, &owed, picture, orientation)
    }

    /// Make a preview [`take`](Self::take)n off the list from a picture
    /// in hand: the cull threads take it, which is a question of
    /// memory, and hand this to another thread, so the next decode is
    /// not held up by the resize and the encode.
    pub(crate) fn make_taken(
        &self,
        path: &Path,
        owed: &Owed,
        picture: &image::RgbImage,
        orientation: Orientation,
    ) -> Option<Made> {
        if self.kept_already(owed) || file_stat(path) != Some(owed.stat) {
            return None;
        }
        let started = Instant::now();
        self.keep(path, owed, sized(picture, orientation), started)
    }

    fn kept_already(&self, owed: &Owed) -> bool {
        self.cache
            .lock()
            .expect("thumbnail cache")
            .as_ref()
            .is_some_and(|c| c.has(&owed.hash, SIZE, owed.tag))
    }

    /// Put a preview made under `owed`'s key, unless the file changed
    /// while it was made. The JPEG is encoded outside the cache's lock,
    /// which every thumbnail lookup and the window's delete take; only
    /// the write, the rename and the count are under it.
    fn keep(&self, path: &Path, owed: &Owed, thumb: Thumb, started: Instant) -> Option<Made> {
        if file_stat(path) != Some(owed.stat) {
            tracing::debug!(
                "preview {}: the file changed while it was made; not kept",
                path.display()
            );
            return None;
        }
        let entry = match greycard_library::thumbs::encode(&thumb, owed.tag, QUALITY) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::debug!("preview {}: not encoded: {e}", path.display());
                return None;
            }
        };
        let mut cache = self.cache.lock().expect("thumbnail cache");
        let cache = cache.as_mut()?;
        if let Err(e) = cache.put_bytes(&owed.hash, SIZE, owed.tag, &entry) {
            tracing::debug!("preview {}: not kept: {e}", path.display());
            return None;
        }
        let bytes = entry.len() as u64;
        let made = Made {
            width: thumb.width,
            height: thumb.height,
            bytes,
            seconds: started.elapsed().as_secs_f64(),
        };
        tracing::debug!(
            "preview {}: {}x{}, {} KB in {:.0} ms",
            path.display(),
            made.width,
            made.height,
            made.bytes / 1024,
            made.seconds * 1e3
        );
        Some(made)
    }

    /// The preview kept under a file's hash and stamp, from the cache
    /// alone: nothing of the file is read. The entry is read under the
    /// cache's lock and decoded outside it.
    pub(crate) fn get(&self, hash: &str, stamp: u64) -> Option<Thumb> {
        let tag = Tag {
            recipe: THUMB_RECIPE,
            stamp,
        };
        let bytes = self
            .cache
            .lock()
            .expect("thumbnail cache")
            .as_ref()?
            .read(hash, SIZE, tag)?;
        let thumb = greycard_library::thumbs::decode(&bytes, tag);
        if let Some(cache) = self.cache.lock().expect("thumbnail cache").as_mut() {
            cache.settle(hash, SIZE, tag, thumb.is_some(), bytes.len() as u64);
        }
        thumb
    }

    /// The hash and stamp a file's preview is kept under: a stat and a
    /// read of its first 64 KB, for a frame with no row to say them.
    #[cfg(test)]
    pub(crate) fn key_of(path: &Path) -> Option<(String, u64)> {
        crate::worker::thumb_key(path)
    }
}

/// A picture brought to [`SIZE`] on its long edge (area-averaged, so
/// the long edge is the size and not the nearest whole factor under
/// it: a 24 MP frame's 6000 would be 1500 by a whole factor), left as
/// it is when smaller, and turned as the camera says.
fn sized(picture: &image::RgbImage, orientation: Orientation) -> Thumb {
    let (w, h) = (picture.width(), picture.height());
    let long = w.max(h);
    let small;
    let picture = if long > SIZE {
        let scale = SIZE as f64 / long as f64;
        let (sw, sh) = (
            ((w as f64 * scale).round() as u32).clamp(1, SIZE),
            ((h as f64 * scale).round() as u32).clamp(1, SIZE),
        );
        small = image::imageops::thumbnail(picture, sw, sh);
        &small
    } else {
        picture
    };
    let (width, height, rgb) = crate::cull::turn_rgb8(
        picture.width(),
        picture.height(),
        picture.as_raw(),
        orientation,
    );
    Thumb { width, height, rgb }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_library::thumbs::Thumbs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-previews-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A PNG of `w` by `h`, a smooth ramp: a picture file the
    /// thumbnail's maker reads whole, as it reads a camera's preview.
    fn png(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 90])
        });
        img.save(path).unwrap();
    }

    /// Previews over a cache of the test's own, its count seeded as the
    /// editor's startup seeds it.
    fn previews_at(dir: &Path, cap: u64) -> Previews {
        let mut thumbs = Thumbs::at(dir.join("thumbs"), cap).with_previews(SIZE, cap);
        thumbs.seed_split(greycard_library::thumbs::split_at(thumbs.root(), SIZE));
        let cache: ThumbCache = Arc::new(Mutex::new(Some(thumbs)));
        Previews::new(cache)
    }

    /// The key is the thumbnails' own with a long edge of `SIZE`: the
    /// file's content hash, the recipe and the file's mtime stamp.
    #[test]
    fn a_preview_is_kept_under_the_thumbnails_key_at_its_own_size() {
        let dir = scratch("key");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("a.png");
        png(&file, 4000, 3000);
        let previews = previews_at(&dir, greycard_library::thumbs::DEFAULT_CAP);
        previews.set_roots(vec![root.clone()]);
        let (hash, stamp) = Previews::key_of(&file).unwrap();
        previews.note(&file, file_stat(&file).unwrap(), &hash);
        assert!(previews.owes(&file));
        let made = previews.make(&file).unwrap().expect("made");
        assert_eq!(
            (made.width, made.height),
            (2048, 1536),
            "to the size, not a whole factor"
        );
        assert!(made.bytes > 0);
        let cache = previews.cache.lock().unwrap();
        let entry = cache
            .as_ref()
            .unwrap()
            .entry_path(&hash, SIZE, thumb_tag((0, stamp)));
        assert!(
            entry.ends_with(format!(
                "{hash}-2048-r{}-{stamp}.thumb",
                crate::worker::THUMB_RECIPE
            )),
            "{}",
            entry.display()
        );
        assert_eq!(std::fs::metadata(&entry).unwrap().len(), made.bytes);
        drop(cache);
        assert!(!previews.owes(&file), "made once");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Made, then read back from the cache alone with the file gone:
    /// the round trip the offline loupe rests on. Noted again, a frame
    /// with its preview kept is owed nothing; and a file changed since
    /// its note is not kept under the old key.
    #[test]
    fn a_preview_made_is_read_back_with_the_file_gone() {
        let dir = scratch("round");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("b.png");
        png(&file, 3000, 2000);
        let previews = previews_at(&dir, greycard_library::thumbs::DEFAULT_CAP);
        previews.set_roots(vec![root.clone()]);
        let stat = file_stat(&file).unwrap();
        let (hash, stamp) = Previews::key_of(&file).unwrap();
        previews.note(&file, stat, &hash);
        previews.make(&file).unwrap().expect("made");
        previews.note(&file, stat, &hash);
        assert!(!previews.owes(&file), "kept already");
        std::fs::remove_file(&file).unwrap();
        let got = previews.get(&hash, stamp).expect("read from the cache");
        assert_eq!((got.width, got.height), (2048, 1365));
        assert!(previews.get(&hash, stamp + 1).is_none(), "another stamp");

        // A second file, changed after its note: not kept.
        let other = root.join("c.png");
        png(&other, 1000, 800);
        let (hash, _) = Previews::key_of(&other).unwrap();
        previews.note(&other, (1, 1), &hash);
        assert_eq!(previews.make(&other).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The cost of a preview on the frames of `GREYCARD_SAMPLES`: made
    /// from each file as the pool makes it, into a cache of the test's
    /// own, the bytes and the time of each and the whole folder's.
    /// Nothing is written beside the samples. Run with `--release
    /// --ignored --nocapture`.
    #[test]
    #[ignore = "reads the folder named by GREYCARD_SAMPLES"]
    fn the_samples_previews() {
        let Some(samples) = std::env::var_os("GREYCARD_SAMPLES").map(PathBuf::from) else {
            eprintln!("set GREYCARD_SAMPLES to a folder of raws");
            return;
        };
        let dir = scratch("samples");
        let previews = previews_at(&dir, greycard_library::thumbs::DEFAULT_CAP);
        previews.set_roots(vec![samples.clone()]);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&samples)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && (greycard_core::picture::is_picture_path(p)
                        || p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                            greycard_core::decode::RAW_EXTENSIONS
                                .iter()
                                .any(|r| r.eq_ignore_ascii_case(e))
                        }))
            })
            .collect();
        files.sort();
        let (mut bytes, mut seconds, mut made, mut read) = (0u64, 0.0f64, 0usize, 0.0f64);
        for f in &files {
            let (hash, stamp) = Previews::key_of(f).unwrap();
            previews.note(f, file_stat(f).unwrap(), &hash);
            match previews.make(f) {
                Ok(Some(m)) => {
                    // Read back as the offline loupe reads it.
                    let started = Instant::now();
                    previews.get(&hash, stamp).expect("kept");
                    let back = started.elapsed().as_secs_f64();
                    println!(
                        "{}: {}x{}, {} KB, made in {:.0} ms, read in {:.1} ms",
                        f.file_name().unwrap().to_string_lossy(),
                        m.width,
                        m.height,
                        m.bytes / 1024,
                        m.seconds * 1e3,
                        back * 1e3
                    );
                    bytes += m.bytes;
                    seconds += m.seconds;
                    read += back;
                    made += 1;
                }
                other => println!("{}: {other:?}", f.display()),
            }
        }
        let n = made.max(1) as f64;
        println!(
            "{made} of {} made: {:.1} MB, {:.0} KB, {:.0} ms to make and {:.1} ms to read a frame, \
             {:.2} s in all",
            files.len(),
            bytes as f64 / 1e6,
            bytes as f64 / n / 1024.0,
            seconds * 1e3 / n,
            read * 1e3 / n,
            seconds
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Only frames under a root are owed one; none is made with the
    /// cache off or past its share of the cap; one made from a picture
    /// in hand reads nothing of the file but its stat.
    #[test]
    fn previews_are_owed_under_a_root_and_made_only_with_room() {
        let dir = scratch("room");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let (inside, outside) = (root.join("in.png"), dir.join("out.png"));
        png(&inside, 600, 400);
        png(&outside, 600, 400);
        let previews = previews_at(&dir, greycard_library::thumbs::DEFAULT_CAP);
        previews.set_roots(vec![root.clone()]);
        for f in [&inside, &outside] {
            let (hash, _) = Previews::key_of(f).unwrap();
            previews.note(f, file_stat(f).unwrap(), &hash);
        }
        assert!(previews.owes(&inside));
        assert!(!previews.owes(&outside), "under no root");

        // From a picture in hand: nothing of the file read for it.
        let picture = image::RgbImage::from_pixel(600, 400, image::Rgb([10, 20, 30]));
        let made = previews
            .make_from(&inside, &picture, Orientation::Rotate90)
            .expect("made from the picture in hand");
        assert_eq!((made.width, made.height), (400, 600), "turned");
        assert!(
            !crate::cull::FILE_READS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&inside),
            "the file's picture is not read"
        );

        // The cache off: owed nothing.
        let off = previews_at(&dir.join("off"), 0);
        off.set_roots(vec![root.clone()]);
        let (hash, _) = Previews::key_of(&outside).unwrap();
        off.note(&inside, file_stat(&inside).unwrap(), &hash);
        assert!(!off.owes(&inside));

        // A cache whose count is not in yet: owed, and not made, so no
        // write walks the cache under its lock; and one past its share
        // of the cap: owed, and not made.
        let unknown = |cap: u64| {
            let cache: ThumbCache = Arc::new(Mutex::new(Some(
                Thumbs::at(dir.join(format!("fresh-{cap}")), cap).with_previews(SIZE, cap),
            )));
            let previews = Previews::new(cache);
            previews.set_roots(vec![root.clone()]);
            previews
        };
        let (hash, _) = Previews::key_of(&inside).unwrap();
        let uncounted = unknown(greycard_library::thumbs::DEFAULT_CAP);
        uncounted.note(&inside, file_stat(&inside).unwrap(), &hash);
        assert!(uncounted.owes(&inside));
        assert_eq!(uncounted.make(&inside).unwrap(), None, "not counted yet");
        let tiny = unknown(4);
        tiny.cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .seed_split(greycard_library::thumbs::Split {
                previews: greycard_library::thumbs::Usage {
                    bytes: 3,
                    entries: 1,
                },
                ..Default::default()
            });
        tiny.note(&inside, file_stat(&inside).unwrap(), &hash);
        assert!(tiny.owes(&inside));
        assert_eq!(tiny.make(&inside).unwrap(), None, "no room");
        // The previews' room is their own cap's: thumbnails filling the
        // thumbnails' cap leave it, and previews filling theirs take it,
        // whatever the thumbnails hold.
        let own = |thumbs_mb: u64, previews_mb: u64, thumbs_held: u64, previews_held: u64| {
            let mb = 1024 * 1024;
            let mut cache = Thumbs::at(
                dir.join(format!("own-{thumbs_mb}-{previews_mb}")),
                thumbs_mb * mb,
            )
            .with_previews(SIZE, previews_mb * mb);
            cache.seed_split(greycard_library::thumbs::Split {
                thumbs: greycard_library::thumbs::Usage {
                    bytes: thumbs_held * mb,
                    entries: 1,
                },
                previews: greycard_library::thumbs::Usage {
                    bytes: previews_held * mb,
                    entries: 1,
                },
            });
            let previews = Previews::new(Arc::new(Mutex::new(Some(cache))));
            previews.set_roots(vec![root.clone()]);
            previews.note(&inside, file_stat(&inside).unwrap(), &hash);
            previews.take(&inside).is_some()
        };
        assert!(
            own(300, 8192, 299, 0),
            "a full thumbnail cap leaves the previews room"
        );
        assert!(own(300, 8192, 0, 6000), "under three quarters of their own");
        assert!(!own(300, 8192, 0, 6200), "past three quarters of their own");
        assert!(!own(300, 0, 0, 0), "a previews' cap of nothing keeps none");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Past `FROM_PICTURES_AT_ONCE` previews in hand from pictures, the
    /// next is skipped and left owed for the pool; one let go makes room.
    #[test]
    fn previews_from_pictures_past_the_limit_are_left_for_the_pool() {
        let dir = scratch("at-once");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let previews = Arc::new(previews_at(&dir, greycard_library::thumbs::DEFAULT_CAP));
        previews.set_roots(vec![root.clone()]);
        let files: Vec<PathBuf> = (0..3).map(|i| root.join(format!("{i}.png"))).collect();
        for f in &files {
            png(f, 64, 48);
            let (hash, _) = Previews::key_of(f).unwrap();
            previews.note(f, file_stat(f).unwrap(), &hash);
        }
        let first = previews.take_for_picture(&files[0]).expect("room");
        let second = previews.take_for_picture(&files[1]).expect("room");
        assert_eq!(FROM_PICTURES_AT_ONCE, 2);
        assert!(previews.take_for_picture(&files[2]).is_none(), "skipped");
        assert!(previews.owes(&files[2]), "left owed for the pool");
        drop(first);
        assert!(previews.take_for_picture(&files[2]).is_some(), "room again");
        drop(second);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What was owed is the last list's: new roots clear it, and a
    /// frame whose root is no longer one is not made, whoever takes it.
    #[test]
    fn a_preview_owed_under_a_root_gone_is_not_made() {
        let dir = scratch("stale");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("s.png");
        png(&file, 500, 300);
        let previews = previews_at(&dir, greycard_library::thumbs::DEFAULT_CAP);
        previews.set_roots(vec![root.clone()]);
        let (hash, _) = Previews::key_of(&file).unwrap();
        previews.note(&file, file_stat(&file).unwrap(), &hash);
        assert!(previews.owes(&file));
        previews.set_roots(vec![dir.join("elsewhere")]);
        assert!(!previews.owes(&file), "cleared with the roots");
        // Noted under the old roots and taken under the new.
        previews.set_roots(vec![root.clone()]);
        previews.note(&file, file_stat(&file).unwrap(), &hash);
        *previews.roots.lock().unwrap() = Vec::new();
        assert!(previews.take(&file).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
