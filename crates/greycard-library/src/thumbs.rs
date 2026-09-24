//! The thumbnail cache: the browser's small pictures, kept on disk
//! and named by the file's content hash rather than its path (notes
//! §72), so a shoot moved or renamed does not render again.
//!
//! An entry is the picture the browser holds in memory for a frame:
//! the camera's preview, sized to a long edge and turned by the
//! camera's own orientation tag, and nothing of the frame's edit. A
//! turn or a mirror from the sidecar is applied as the picture is
//! drawn, so it is not in the entry and cannot make one stale; the
//! orientation tag is in the file's head, which the hash covers.
//!
//! The key is [`hash_file`](crate::hash::hash_file) — the index's
//! `hash` column for the same file — and the long edge asked for, one
//! file an entry at `<root>/<first two hex>/<hash>-<size>.thumb`. The
//! file is a small header and a JPEG:
//!
//! ```text
//! "GCTH"  format u16  recipe u16  width u32  height u32
//! stamp u64  length u32  checksum [u8; 16]  JPEG (length bytes)
//! ```
//!
//! all little-endian. `recipe` is the caller's version of how it
//! makes a picture, so a change there turns every entry into a miss;
//! `stamp` is the caller's too: the editor's is zero for most raws
//! and the modification time for a picture file, whose head can stay
//! the same through a re-export (an uncompressed TIFF retouched in
//! its lower half is the same size and the same first 64 KB), and for
//! a DNG, whose directories and previews can sit past the head and be
//! rewritten in place. The checksum is BLAKE3
//! of the JPEG, cut to 16 bytes. An entry that fails any of it — cut
//! short, another format, a checksum that does not match, a JPEG that
//! will not decode or decodes to another size — is a miss, and is
//! removed.
//!
//! Entries are written to a temporary name and renamed into place,
//! so a reader never sees half of one. A hit sets the entry's
//! modification time to now, and that is its recency: when the
//! entries pass the cap, the least recently used are removed until
//! they are under nine tenths of it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::Error;

/// The layout of an entry this build reads and writes.
pub const FORMAT: u16 = 1;

/// The cap when the settings name none: 300 MB, about thirty thousand
/// strip-sized pictures.
pub const DEFAULT_CAP: u64 = 300 * 1024 * 1024;

/// What the entries are cut down to when they pass the cap, as a
/// fraction of it, so a cache at its cap does not evict on every
/// write.
const LOW_WATER: f64 = 0.9;

const MAGIC: &[u8; 4] = b"GCTH";
const HEADER: usize = 4 + 2 + 2 + 4 + 4 + 8 + 4 + 16;
/// The JPEG's quality: the preview it came from was a JPEG already,
/// and at a few hundred pixels 90 is indistinguishable from the
/// picture in memory.
const QUALITY: u8 = 90;
/// Beyond any thumbnail's side, so a header that claims more is
/// refused before anything is allocated for it.
const MAX_SIDE: u32 = 8192;

/// A picture as the browser holds it: 8-bit RGB, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// What an entry is for beyond its key: see the module's words on
/// `recipe` and `stamp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tag {
    pub recipe: u16,
    pub stamp: u64,
}

/// How much the cache holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub bytes: u64,
    pub entries: usize,
}

/// The cache under one directory, with a cap on its size.
#[derive(Debug)]
pub struct Thumbs {
    root: PathBuf,
    cap: u64,
    /// The bytes the entries take, counted on the first write and
    /// kept up after it; `None` until then.
    used: Option<u64>,
}

impl Thumbs {
    /// The user's cache: `$XDG_CACHE_HOME/greycard/thumbs` when that
    /// is set, else `greycard/thumbs` under the platform's cache
    /// directory: `~/.cache` on Linux, `~/Library/Caches` on macOS,
    /// `%LOCALAPPDATA%` on Windows. A cache cleaner may wipe it, which
    /// costs a render a frame and nothing else.
    pub fn user(cap: u64) -> crate::Result<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(dirs::cache_dir)
            .ok_or(Error::NoCacheDir)?;
        Ok(Self::at(base.join("greycard").join("thumbs"), cap))
    }

    /// The cache under `root`, which is made on the first write.
    pub fn at(root: impl Into<PathBuf>, cap: u64) -> Self {
        Self {
            root: root.into(),
            cap,
            used: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// A new cap, taking effect at the next write.
    pub fn set_cap(&mut self, cap: u64) {
        self.cap = cap;
    }

    /// Where the entry for `hash` at `size` lives.
    pub fn entry_path(&self, hash: &str, size: u32) -> PathBuf {
        let fan = hash.get(..2).unwrap_or("__");
        self.root.join(fan).join(format!("{hash}-{size}.thumb"))
    }

    /// The picture kept for `hash` at `size` under `tag`, or `None`.
    /// A damaged entry, or one kept under another tag, is removed and
    /// answers `None`: the caller renders and puts, as for any miss.
    pub fn get(&self, hash: &str, size: u32, tag: Tag) -> Option<Thumb> {
        let path = self.entry_path(hash, size);
        let bytes = std::fs::read(&path).ok()?;
        match decode_entry(&bytes, tag) {
            Some(thumb) => {
                // Its recency, for the eviction. A failure here costs
                // an early eviction at worst.
                if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Some(thumb)
            }
            None => {
                log::debug!("thumbnail cache: {} unusable, removed", path.display());
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Keep `thumb` for `hash` at `size` under `tag`, replacing what
    /// was there, and evict if that takes the cache past its cap.
    pub fn put(&mut self, hash: &str, size: u32, tag: Tag, thumb: &Thumb) -> std::io::Result<()> {
        let bytes = encode_entry(thumb, tag)?;
        let path = self.entry_path(hash, size);
        let dir = path.parent().expect("an entry is under its fan-out folder");
        std::fs::create_dir_all(dir)?;
        let before = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // A name of this process's and this moment's, so two editors
        // writing one entry do not write into each other's file.
        let tmp = dir.join(format!(
            ".{hash}-{size}.{}.{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let written = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            drop(f);
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        let used = match self.used {
            Some(u) => u.saturating_sub(before) + bytes.len() as u64,
            None => self.usage().bytes,
        };
        self.used = Some(used);
        if used > self.cap {
            self.evict();
        }
        Ok(())
    }

    /// What the cache holds now, counted on disk.
    pub fn usage(&self) -> Usage {
        let mut usage = Usage::default();
        for (_, len, _) in self.entries() {
            usage.bytes += len;
            usage.entries += 1;
        }
        usage
    }

    /// Remove every entry. Answers what was removed.
    pub fn clear(&mut self) -> Usage {
        let mut gone = Usage::default();
        for (path, len, _) in self.entries() {
            if std::fs::remove_file(&path).is_ok() {
                gone.bytes += len;
                gone.entries += 1;
            }
        }
        self.remove_empty_folders();
        self.used = Some(0);
        gone
    }

    /// Remove the least recently used entries until the cache is
    /// under nine tenths of its cap.
    fn evict(&mut self) {
        let mut entries = self.entries();
        let mut used: u64 = entries.iter().map(|(_, len, _)| len).sum();
        let target = (self.cap as f64 * LOW_WATER) as u64;
        entries.sort_by_key(|(_, _, when)| *when);
        let mut removed = 0usize;
        for (path, len, _) in &entries {
            if used <= target {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                used = used.saturating_sub(*len);
                removed += 1;
            }
        }
        log::info!(
            "thumbnail cache: evicted {removed} entries, {} MB kept of a {} MB cap",
            used / (1024 * 1024),
            self.cap / (1024 * 1024)
        );
        self.used = Some(used);
    }

    /// Every entry: its path, its length and when it was last used.
    fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut out = Vec::new();
        let Ok(fans) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for fan in fans.flatten() {
            if !fan.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(fan.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "thumb") {
                    continue;
                }
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                let when = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, meta.len(), when));
            }
        }
        out
    }

    fn remove_empty_folders(&self) {
        if let Ok(fans) = std::fs::read_dir(&self.root) {
            for fan in fans.flatten() {
                // Fails, and is meant to, on a folder with anything
                // left in it.
                let _ = std::fs::remove_dir(fan.path());
            }
        }
    }
}

/// An entry's bytes: the header and the JPEG.
fn encode_entry(thumb: &Thumb, tag: Tag) -> std::io::Result<Vec<u8>> {
    let expected = thumb.width as usize * thumb.height as usize * 3;
    if thumb.width == 0 || thumb.height == 0 || thumb.rgb.len() != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "a {}x{} picture of {} bytes",
                thumb.width,
                thumb.height,
                thumb.rgb.len()
            ),
        ));
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
        .encode(
            &thumb.rgb,
            thumb.width,
            thumb.height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(std::io::Error::other)?;
    let mut out = Vec::with_capacity(HEADER + jpeg.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&tag.recipe.to_le_bytes());
    out.extend_from_slice(&thumb.width.to_le_bytes());
    out.extend_from_slice(&thumb.height.to_le_bytes());
    out.extend_from_slice(&tag.stamp.to_le_bytes());
    out.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
    out.extend_from_slice(&checksum(&jpeg));
    out.extend_from_slice(&jpeg);
    Ok(out)
}

/// The picture in an entry's bytes, or `None` for anything amiss.
fn decode_entry(bytes: &[u8], tag: Tag) -> Option<Thumb> {
    let head = bytes.get(..HEADER)?;
    let u16_at = |at: usize| u16::from_le_bytes(head[at..at + 2].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
    if &head[..4] != MAGIC || u16_at(4) != FORMAT || u16_at(6) != tag.recipe {
        return None;
    }
    let (width, height) = (u32_at(8), u32_at(12));
    let stamp = u64::from_le_bytes(head[16..24].try_into().unwrap());
    let length = u32_at(24) as usize;
    if stamp != tag.stamp
        || width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || bytes.len() != HEADER + length
    {
        return None;
    }
    let jpeg = &bytes[HEADER..];
    if checksum(jpeg) != head[28..44] {
        return None;
    }
    let decoded = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    let rgb = decoded.to_rgb8();
    if rgb.width() != width || rgb.height() != height {
        return None;
    }
    Some(Thumb {
        width,
        height,
        rgb: rgb.into_raw(),
    })
}

fn checksum(bytes: &[u8]) -> [u8; 16] {
    let hash = blake3::hash(bytes);
    hash.as_bytes()[..16].try_into().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_file;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-thumbs-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A smooth picture, so the JPEG is near it and small.
    fn picture(width: u32, height: u32, seed: u8) -> Thumb {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.push((x * 255 / width) as u8);
                rgb.push((y * 255 / height) as u8);
                rgb.push(seed);
            }
        }
        Thumb { width, height, rgb }
    }

    /// Within what a JPEG at quality 90 does to a smooth picture.
    fn near(a: &Thumb, b: &Thumb) -> bool {
        a.width == b.width
            && a.height == b.height
            && a.rgb
                .iter()
                .zip(&b.rgb)
                .all(|(x, y)| (*x as i32 - *y as i32).abs() <= 8)
    }

    fn raw_file(dir: &Path, name: &str, seed: u8) -> PathBuf {
        let path = dir.join(name);
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8 ^ seed).collect();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn the_key_is_the_index_hash() {
        let dir = scratch("key");
        let raw = raw_file(&dir, "IMG_0001.CR3", 1);
        let mut lib = crate::Library::open_in_memory().unwrap();
        lib.index_file(&raw).unwrap();
        let row = lib.by_path(&raw).unwrap().expect("indexed");
        let key = hash_file(&raw).unwrap();
        assert_eq!(key, row.hash);
        let cache = Thumbs::at(dir.join("thumbs"), DEFAULT_CAP);
        let entry = cache.entry_path(&key, 170);
        assert!(entry.to_string_lossy().contains(&row.hash));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hit_needs_nothing_of_the_raw_and_a_moved_file_hits() {
        let dir = scratch("hit");
        let raw = raw_file(&dir, "IMG_0002.CR3", 2);
        let mut cache = Thumbs::at(dir.join("thumbs"), DEFAULT_CAP);
        let key = hash_file(&raw).unwrap();
        let thumb = picture(170, 113, 40);
        let tag = Tag::default();
        cache.put(&key, 170, tag, &thumb).unwrap();

        // Moved to another folder under another name: the same key.
        let elsewhere = dir.join("renamed shoot");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let moved = elsewhere.join("wedding-0002.CR3");
        std::fs::rename(&raw, &moved).unwrap();
        let moved_key = hash_file(&moved).unwrap();
        assert_eq!(moved_key, key);
        // And gone altogether: the entry is all a hit reads.
        std::fs::remove_file(&moved).unwrap();
        let got = cache.get(&moved_key, 170, tag).expect("a hit");
        assert!(near(&got, &thumb));
        // Another size is another entry.
        assert!(cache.get(&key, 256, tag).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_changes_the_picture_changes_the_key_or_misses() {
        let dir = scratch("stale");
        let raw = raw_file(&dir, "IMG_0003.CR3", 3);
        let mut cache = Thumbs::at(dir.join("thumbs"), DEFAULT_CAP);
        let key = hash_file(&raw).unwrap();
        let tag = Tag {
            recipe: 1,
            stamp: 0,
        };
        cache.put(&key, 170, tag, &picture(170, 113, 0)).unwrap();
        // The orientation tag rewritten in place, a byte in the head:
        // another key, so a miss.
        let mut body = std::fs::read(&raw).unwrap();
        body[40] ^= 0x08;
        std::fs::write(&raw, &body).unwrap();
        let turned = hash_file(&raw).unwrap();
        assert_ne!(turned, key);
        assert!(cache.get(&turned, 170, tag).is_none());
        // The caller's recipe changed: a miss, and the entry gone.
        let newer = Tag {
            recipe: 2,
            stamp: 0,
        };
        assert!(cache.get(&key, 170, newer).is_none());
        assert!(!cache.entry_path(&key, 170).exists());
        // A picture file re-exported over itself with its head the
        // same: the stamp tells them apart.
        let stamped = Tag {
            recipe: 1,
            stamp: 1_700_000_000,
        };
        cache
            .put(&key, 170, stamped, &picture(170, 113, 0))
            .unwrap();
        let later = Tag {
            recipe: 1,
            stamp: 1_700_000_500,
        };
        assert!(cache.get(&key, 170, later).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_entry_is_a_miss() {
        let dir = scratch("damaged");
        let mut cache = Thumbs::at(dir.join("thumbs"), DEFAULT_CAP);
        let key = "ab".repeat(32);
        let tag = Tag::default();
        let thumb = picture(96, 64, 9);
        let path = cache.entry_path(&key, 96);
        cache.put(&key, 96, tag, &thumb).unwrap();
        let whole = std::fs::read(&path).unwrap();
        // Cut short at every length that matters: inside the header,
        // at its end, inside the JPEG, one byte short.
        for cut in [0, 3, HEADER - 1, HEADER, HEADER + 10, whole.len() - 1] {
            std::fs::write(&path, &whole[..cut]).unwrap();
            assert!(cache.get(&key, 96, tag).is_none(), "cut at {cut}");
            assert!(!path.exists(), "the damaged entry is removed");
        }
        // A byte of the JPEG flipped: the checksum catches it.
        let mut flipped = whole.clone();
        let at = HEADER + (whole.len() - HEADER) / 2;
        flipped[at] ^= 0xff;
        std::fs::write(&path, &flipped).unwrap();
        assert!(cache.get(&key, 96, tag).is_none());
        // A header claiming a size the JPEG is not.
        let mut lying = whole.clone();
        lying[8] = lying[8].wrapping_add(1);
        std::fs::write(&path, &lying).unwrap();
        assert!(cache.get(&key, 96, tag).is_none());
        // Garbage of the right length.
        std::fs::write(&path, vec![0x5au8; whole.len()]).unwrap();
        assert!(cache.get(&key, 96, tag).is_none());
        // And a good one is still a hit.
        cache.put(&key, 96, tag, &thumb).unwrap();
        assert!(cache.get(&key, 96, tag).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn eviction_keeps_the_cap_and_the_recent() {
        let dir = scratch("evict");
        // One entry to find its size, then a cap of ten of them.
        let mut probe = Thumbs::at(dir.join("probe"), DEFAULT_CAP);
        let thumb = |seed: u8| picture(128, 96, seed);
        probe
            .put(&"00".repeat(32), 128, Tag::default(), &thumb(0))
            .unwrap();
        let one = probe.usage().bytes;
        let cap = one * 10;
        let mut cache = Thumbs::at(dir.join("thumbs"), cap);
        let keys: Vec<String> = (0..30u8).map(|i| format!("{i:02x}").repeat(32)).collect();
        let t0 = SystemTime::now() - std::time::Duration::from_secs(3600);
        for (i, key) in keys.iter().enumerate() {
            cache
                .put(key, 128, Tag::default(), &thumb(i as u8))
                .unwrap();
            // Recency by the clock, one second apart, so the order
            // does not rest on the filesystem's timestamp grain.
            let f = std::fs::File::options()
                .write(true)
                .open(cache.entry_path(key, 128))
                .unwrap();
            f.set_modified(t0 + std::time::Duration::from_secs(i as u64))
                .unwrap();
            // The first one used again each time, which makes it the
            // most recent: it stays.
            if i > 0 {
                assert!(cache.get(&keys[0], 128, Tag::default()).is_some(), "at {i}");
            }
            assert!(cache.usage().bytes <= cap, "over the cap at {i}");
        }
        let usage = cache.usage();
        assert!(usage.bytes <= cap);
        assert!(
            usage.entries >= 8,
            "evicted down to nine tenths, not to nothing"
        );
        assert!(cache.get(&keys[0], 128, Tag::default()).is_some());
        // The newest are kept, the oldest gone.
        assert!(cache.get(&keys[29], 128, Tag::default()).is_some());
        assert!(cache.get(&keys[1], 128, Tag::default()).is_none());
        // And clear empties it.
        let gone = cache.clear();
        assert_eq!(gone.entries, usage.entries);
        assert_eq!(cache.usage(), Usage::default());
        assert!(cache.get(&keys[0], 128, Tag::default()).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bad_picture_is_refused_not_written() {
        let dir = scratch("refused");
        let mut cache = Thumbs::at(dir.join("thumbs"), DEFAULT_CAP);
        let bad = Thumb {
            width: 10,
            height: 10,
            rgb: vec![0; 5],
        };
        assert!(
            cache
                .put(&"cd".repeat(32), 10, Tag::default(), &bad)
                .is_err()
        );
        assert_eq!(cache.usage(), Usage::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
