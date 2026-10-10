//! Thumbnails from the edit: a frame with an edit shows its edit in the
//! strip, the grid, the culling loupe and compare, and a frame without
//! one keeps the camera's JPEG.
//!
//! The pictures made from an edit are kept in the thumbnail cache
//! beside the camera's, under the file's content hash, at each made size
//! (`grid::MADE`) and at the local previews' 2048, with the edit's
//! develop key ([`greycard_edit::Edit::develop_key`]) in the place of the
//! camera's stamp and [`EDITED_RECIPE`] in the place of its recipe. The
//! camera's entries stay: they stand in until the edited ones land, and
//! they are the picture again when the edit is reset. Like the camera's,
//! an edited picture is kept as the camera's tag turns it and no
//! further: its crop and straighten are in it, and the frame's own
//! quarter turns and mirror are taken back out, so the strip and the
//! loupe turn it at draw time exactly as they turn the camera's.
//!
//! Which frames show an edit is [`Edited`]'s to say. The window tells
//! it, with each list, which frames it holds as edited (the index's
//! `edited` for a frame standing in from its row, the sidecar's edit
//! against the frame's default for one read); the key of each is found
//! the first time a picture of it is looked up, by reading its sidecar
//! on the thumbnail's own thread, and kept. A frame whose sidecar cannot
//! be read (its root offline) shows the last edited picture kept of its
//! content, since nothing else can say which it is.
//!
//! This part makes them one way only: from the open frame, for nothing.
//! When the window's develop has landed and the edit is saved, or the
//! frame is left with its develop in hand, the worker takes the picture
//! it already has (on the CPU, or read back from the viewport's texture
//! a whole factor smaller), and a thread of its own reduces it,
//! straightens and crops it, brings it to 2048 and finishes it as an
//! export at that size is finished, then shrinks that to the made sizes
//! and keeps them all. The cells showing the frame are then asked again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use greycard_core::image::WorkingImage;
use greycard_edit::Edit;
use greycard_edit::brush::Raster;
use greycard_edit::geometry::Geometry;
use greycard_library::thumbs::{Tag, Thumb};
use rayon::prelude::*;

use crate::worker::ThumbCache;

/// The recipe the pictures made from an edit are kept under, where the
/// camera's carry `worker::THUMB_RECIPE`: the high bit says "made from
/// the edit", so the two never name one entry. Raise the low bits when
/// the shrink, the un-turn or the encode below make another picture
/// from the same finished one; a change to the develop or the finish
/// is `greycard_edit::key::DEVELOP_RECIPE`'s.
pub(crate) const EDITED_RECIPE: u16 = 0x8001;

/// The long edge of the larger picture, the one the culling loupe and
/// compare show: the local previews' own.
pub(crate) const BIG: u32 = crate::previews::SIZE;

/// The thumbnails' JPEG quality, the cache's own for them.
const THUMB_QUALITY: u8 = 90;

/// A frame's ISO as the window has it from its row (`Some(None)`: the
/// camera gave none; `None`: no row stood in for it), which the learned
/// blend's seed is judged against (`greycard_library::is_default_edit`).
pub(crate) type Iso = Option<Option<u32>>;

/// What is known of a frame the window holds as edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// Its key is still to be read from its sidecar.
    Read,
    /// Its current edit's key.
    Key(u64),
    /// Its sidecar could not be read: the last picture kept of its
    /// content is the best there is.
    Unread,
}

/// A frame held as edited: what is known of its key, and its ISO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    want: Want,
    iso: Iso,
}

impl Held {
    fn read(iso: Iso) -> Self {
        Self {
            want: Want::Read,
            iso,
        }
    }
}

/// What a frame shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shows {
    /// The camera's JPEG: no edit.
    Camera,
    /// The pictures kept under this key, the camera's until they are.
    Edit(u64),
    /// The picture of the edit kept last for its content, whatever its
    /// key.
    Newest,
}

/// Which frames show an edit and under what key, with the cache their
/// pictures are kept in and the thread that makes them.
pub(crate) struct Edited {
    cache: ThumbCache,
    wants: Mutex<HashMap<PathBuf, Held>>,
    keeper: Keeper,
    /// The pictures being made or waiting to be, by content hash and
    /// key, so a second save of the same picture before the first is
    /// kept (a develop landing, then a rating) makes it once.
    in_hand: InHand,
    /// The lens database the develop reads, by where it was found and
    /// when it was made ([`lens_database`]): part of every key whose
    /// edit corrects the lens by its profile.
    lenses: Mutex<Option<String>>,
}

type InHand = Arc<Mutex<std::collections::HashSet<(String, u64)>>>;

/// A picture in hand on the keeping thread, let go when the making that
/// holds it is done or dropped unbegun.
pub(crate) struct Marked {
    in_hand: InHand,
    which: (String, u64),
}

impl Drop for Marked {
    fn drop(&mut self) {
        if let Ok(mut set) = self.in_hand.lock() {
            set.remove(&self.which);
        }
    }
}

/// The lens database a develop reads now, named by the directory it is
/// found in (the first of `greycard_lens::Store::candidates` with any
/// XML in it, which is the one `Store::load` reads) and the fetch's
/// `timestamp.txt` there: a fetch or an update moves it. `None` with
/// none. A listing or two and one small read, never the database.
pub(crate) fn lens_database() -> Option<String> {
    let store = greycard_lens::Store::user().ok()?;
    let dir = store.candidates().into_iter().find(|dir| {
        std::fs::read_dir(dir).is_ok_and(|mut entries| {
            entries.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "xml")))
        })
    })?;
    let made = std::fs::read_to_string(dir.join("timestamp.txt")).unwrap_or_default();
    Some(format!("{} {}", dir.display(), made.trim()))
}

impl Edited {
    pub(crate) fn new(cache: ThumbCache) -> Self {
        Self {
            cache,
            wants: Mutex::new(HashMap::new()),
            keeper: Keeper::default(),
            in_hand: Arc::default(),
            lenses: Mutex::new(lens_database()),
        }
    }

    fn wants(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Held>> {
        self.wants.lock().expect("edited frames")
    }

    /// The lens database's name for the keys ([`lens_database`]).
    pub(crate) fn lenses(&self) -> Option<String> {
        self.lenses.lock().expect("edited lenses").clone()
    }

    /// The lens database changed (fetched, or updated): every key found
    /// is read again, since a key of an edit that corrects the lens
    /// names the database. Whether it changed.
    pub(crate) fn set_lenses(&self, now: Option<String>) -> bool {
        let mut lenses = self.lenses.lock().expect("edited lenses");
        if *lenses == now {
            return false;
        }
        *lenses = now;
        drop(lenses);
        for held in self.wants().values_mut() {
            held.want = Want::Read;
        }
        true
    }

    /// The key of `edit` under the lens database the develop reads now.
    pub(crate) fn key_of(&self, edit: &Edit) -> u64 {
        edit.develop_key(self.lenses().as_deref())
    }

    /// A new list: these frames are held as edited (true) or not, each
    /// with its ISO. Each edited one's key is read again from its
    /// sidecar when it is next looked up; the frames of other lists are
    /// forgotten.
    pub(crate) fn note_all(&self, list: impl IntoIterator<Item = (PathBuf, bool, Iso)>) {
        let wants: HashMap<PathBuf, Held> = list
            .into_iter()
            .filter(|(_, edited, _)| *edited)
            .map(|(path, _, iso)| (path, Held::read(iso)))
            .collect();
        *self.wants() = wants;
    }

    /// One frame saved, or found changed on disk: held as edited or not,
    /// its key read again when next looked up. Whether it was or is
    /// edited, which is when its pictures may have changed.
    pub(crate) fn note(&self, path: &Path, edited: bool, iso: Iso) -> bool {
        let mut wants = self.wants();
        let was = if edited {
            wants.insert(path.to_path_buf(), Held::read(iso))
        } else {
            wants.remove(path)
        };
        edited || was.is_some()
    }

    /// What the window holds of one frame changed (its row came in, or
    /// its sidecar was read): held as edited or not from now on, a key
    /// already found kept. Whether that turned it from one to the other,
    /// which is when its pictures change.
    pub(crate) fn hold(&self, path: &Path, edited: bool, iso: Iso) -> bool {
        let mut wants = self.wants();
        match (edited, wants.get_mut(path)) {
            (true, None) => {
                wants.insert(path.to_path_buf(), Held::read(iso));
                true
            }
            (true, Some(held)) => {
                held.iso = iso;
                false
            }
            (false, Some(_)) => {
                wants.remove(path);
                true
            }
            (false, None) => false,
        }
    }

    /// Whether the frame is held as edited.
    pub(crate) fn holds(&self, path: &Path) -> bool {
        self.wants().contains_key(path)
    }

    /// The key this frame was last shown under, when one was found.
    fn shown_key(&self, path: &Path) -> Option<u64> {
        match self.wants().get(path) {
            Some(Held {
                want: Want::Key(k), ..
            }) => Some(*k),
            _ => None,
        }
    }

    /// The frame's pictures are kept under `key`: it shows them from now
    /// on, and the pictures of the key it showed before are taken out of
    /// the cache. Only that key's: another copy of the same content may
    /// show its own. Whether that changed what it shows.
    pub(crate) fn settle_key(&self, path: &Path, hash: &str, key: u64, iso: Iso) -> bool {
        let was = self.shown_key(path);
        if let Some(old) = was.filter(|&old| old != key) {
            self.remove_key(hash, old);
        }
        self.wants().insert(
            path.to_path_buf(),
            Held {
                want: Want::Key(key),
                iso,
            },
        );
        was != Some(key)
    }

    fn remove_key(&self, hash: &str, key: u64) -> usize {
        let tag = Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        match self.cache.lock().expect("thumbnail cache").as_mut() {
            Some(cache) => cache.remove_tag(hash, tag),
            None => 0,
        }
    }

    /// What `path` shows, its sidecar read here for its key when that is
    /// not known yet: on a thumbnail's thread or a loupe's, never the
    /// window's.
    pub(crate) fn shows(&self, path: &Path) -> Shows {
        let held = self.wants().get(path).copied();
        let Some(held) = held else {
            return Shows::Camera;
        };
        match held.want {
            Want::Key(k) => Shows::Edit(k),
            Want::Unread => Shows::Newest,
            Want::Read => {
                let read = read_key(path, held.iso, self.lenses().as_deref());
                let mut wants = self.wants();
                // A save or a keep since the read began has the last word.
                if wants.get(path) != Some(&held) {
                    drop(wants);
                    return self.shows(path);
                }
                let (want, shows) = match read {
                    Read::Key(k) => (Want::Key(k), Shows::Edit(k)),
                    Read::Default => {
                        wants.remove(path);
                        return Shows::Camera;
                    }
                    Read::Unread => (Want::Unread, Shows::Newest),
                };
                wants.insert(
                    path.to_path_buf(),
                    Held {
                        want,
                        iso: held.iso,
                    },
                );
                shows
            }
        }
    }

    /// What a frame whose file and sidecar are out of reach shows:
    /// the edit's picture kept last for its content when the window
    /// holds it as edited. Nothing of it is read.
    pub(crate) fn shows_offline(&self, path: &Path) -> Shows {
        if self.wants().contains_key(path) {
            Shows::Newest
        } else {
            Shows::Camera
        }
    }

    /// The picture of the edit kept for content `hash` at `size`, as
    /// `shows` says, or `None`: the camera's then stands in. The JPEG is
    /// decoded outside the cache's lock.
    pub(crate) fn lookup(&self, hash: &str, size: u32, shows: Shows) -> Option<Thumb> {
        let tag = match shows {
            Shows::Camera => return None,
            Shows::Edit(k) => Tag {
                recipe: EDITED_RECIPE,
                stamp: k,
            },
            Shows::Newest => self
                .cache
                .lock()
                .expect("thumbnail cache")
                .as_ref()?
                .newest(hash, EDITED_RECIPE)?,
        };
        let bytes = {
            let cache = self.cache.lock().expect("thumbnail cache");
            let cache = cache.as_ref().filter(|c| c.cap() > 0)?;
            cache.read(hash, size, tag)?
        };
        let thumb = greycard_library::thumbs::decode(&bytes, tag);
        if let Some(cache) = self.cache.lock().expect("thumbnail cache").as_mut() {
            cache.settle(hash, size, tag, thumb.is_some(), bytes.len() as u64);
        }
        thumb
    }

    /// Whether every picture of the edit is kept for `hash` under `key`:
    /// a save that changed nothing of the picture (a rating) makes none.
    pub(crate) fn all_kept(&self, hash: &str, key: u64) -> bool {
        let tag = Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        let cache = self.cache.lock().expect("thumbnail cache");
        cache.as_ref().is_some_and(|c| {
            c.cap() > 0
                && crate::grid::MADE
                    .iter()
                    .chain([BIG].iter())
                    .all(|&size| c.has(hash, size, tag))
        })
    }

    /// The picture of content `hash` under `key` taken in hand, or
    /// `None` when it is in hand already.
    pub(crate) fn mark(&self, hash: &str, key: u64) -> Option<Marked> {
        let which = (hash.to_string(), key);
        let fresh = self
            .in_hand
            .lock()
            .expect("edited in hand")
            .insert(which.clone());
        fresh.then(|| Marked {
            in_hand: self.in_hand.clone(),
            which,
        })
    }

    /// Whether there is a cache to keep pictures in.
    pub(crate) fn cache_on(&self) -> bool {
        self.cache
            .lock()
            .expect("thumbnail cache")
            .as_ref()
            .is_some_and(|c| c.cap() > 0)
    }

    /// The frame's edit was reset: it shows the camera's, and the
    /// pictures of the key it showed are taken out (another copy's of
    /// the same content are left). Whether it showed anything else.
    pub(crate) fn reset(&self, path: &Path, hash: Option<&str>) -> bool {
        let old = self.shown_key(path);
        let held = self.wants().remove(path).is_some();
        let removed = match (old, hash) {
            (Some(old), Some(hash)) => self.remove_key(hash, old),
            _ => 0,
        };
        held || removed > 0
    }

    /// Keep `made` for content `hash` under `key` as frame `path`'s, and
    /// take the pictures of the key it showed before out of the cache.
    /// The encoding is done outside the cache's lock. Answers the bytes
    /// kept.
    pub(crate) fn store(
        &self,
        path: &Path,
        hash: &str,
        key: u64,
        iso: Iso,
        made: &Made,
    ) -> std::io::Result<u64> {
        let tag = Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        let mut encoded = Vec::with_capacity(made.thumbs.len() + 1);
        for (size, thumb) in &made.thumbs {
            encoded.push((
                *size,
                greycard_library::thumbs::encode(thumb, tag, THUMB_QUALITY)?,
            ));
        }
        encoded.push((
            BIG,
            greycard_library::thumbs::encode(&made.big, tag, crate::previews::QUALITY)?,
        ));
        let mut bytes = 0;
        {
            let mut cache = self.cache.lock().expect("thumbnail cache");
            let Some(cache) = cache.as_mut().filter(|c| c.cap() > 0) else {
                return Ok(0);
            };
            for (size, entry) in &encoded {
                cache.put_bytes(hash, *size, tag, entry)?;
                bytes += entry.len() as u64;
            }
        }
        self.settle_key(path, hash, key, iso);
        Ok(bytes)
    }

    /// Run `task` on the keeping thread, in place of one still waiting:
    /// the newest save's pictures are the ones wanted.
    pub(crate) fn keep_later(&self, task: Task) {
        self.keeper.put(task);
    }
}

/// What reading a frame's sidecar for its key found.
enum Read {
    Key(u64),
    /// Its edit is the frame's default: no edit.
    Default,
    /// The sidecar could not be read.
    Unread,
}

/// A frame's key, from its sidecar on disk as the window reads it (the
/// XMP beside it included), or why there is none.
fn read_key(path: &Path, iso: Iso, lenses: Option<&str>) -> Read {
    let (sidecar, _, trouble) = crate::panel::browser::load_sidecar_said(path, true);
    if trouble.is_some() {
        return Read::Unread;
    }
    if !is_edited(path, &sidecar.current, iso) {
        return Read::Default;
    }
    Read::Key(sidecar.develop_key(lenses))
}

/// Whether `edit` is an edit of the frame at `path`, by the rule the
/// cell's edited mark and the index's `edited` go by
/// (`greycard_library::is_default_edit`), the learned blend judged
/// against `iso`: a frame only opened, or reset to its original, shows
/// the camera's JPEG.
pub(crate) fn is_edited(path: &Path, edit: &Edit, iso: Iso) -> bool {
    !greycard_library::is_default_edit(edit, path, iso)
}

/// A job for the keeping thread.
pub(crate) type Task = Box<dyn FnOnce() + Send>;

/// One thread that makes the pictures of an edit, one at a time: a
/// making holds a reduced copy of a frame and runs the finish on rayon,
/// and two at once would only race each other to the same answer.
#[derive(Default)]
struct Keeper {
    inner: Arc<(Mutex<KeeperState>, Condvar)>,
}

#[derive(Default)]
struct KeeperState {
    waiting: Option<Task>,
    started: bool,
}

impl Keeper {
    fn put(&self, task: Task) {
        let (lock, cv) = &*self.inner;
        let mut state = lock.lock().expect("keeper");
        state.waiting = Some(task);
        if !state.started {
            state.started = true;
            let inner = self.inner.clone();
            let spawned = std::thread::Builder::new()
                .name("greycard edited pictures".into())
                .spawn(move || {
                    let (lock, cv) = &*inner;
                    loop {
                        let task = {
                            let mut state = lock.lock().expect("keeper");
                            loop {
                                if let Some(task) = state.waiting.take() {
                                    break task;
                                }
                                state = cv.wait(state).expect("keeper");
                            }
                        };
                        // A panic costs this picture and nothing else.
                        if let Err(payload) =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                        {
                            tracing::warn!(
                                "the edit's pictures: the making panicked: {}",
                                crate::worker::panic_message(payload.as_ref())
                            );
                        }
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!("the edit's pictures: no thread to make them on: {e}");
                state.started = false;
                state.waiting = None;
            }
        }
        cv.notify_one();
    }
}

/// What the finish of the edit's pictures takes beside the picture: the
/// export's inputs, as the worker has them for the open frame.
pub(crate) struct Finish {
    pub(crate) edit: Edit,
    /// The frame's own quarter turns, the sidecar's.
    pub(crate) turn: u8,
    /// The developed picture's size before the geometry, which the
    /// masks and the guide plane are placed by.
    pub(crate) source: (u32, u32),
    pub(crate) rasters: HashMap<(u64, usize), Arc<Raster>>,
    pub(crate) clip_level: f32,
    pub(crate) guide: Option<Arc<crate::finish::Guide>>,
    pub(crate) kind: crate::finish::Source,
    pub(crate) white: Option<greycard_edit::WhiteShift>,
}

/// The pictures of an edit, as they are kept: the larger one and each
/// made size, all turned as the camera's tag turns the frame and no
/// further.
pub(crate) struct Made {
    pub(crate) big: Thumb,
    pub(crate) thumbs: Vec<(u32, Thumb)>,
}

/// The whole factor a developed picture of `source` is reduced by before
/// its geometry, so its frame is still at least [`BIG`] on its long edge
/// and is brought to it by one resize that is less than a halving.
pub(crate) fn factor_for(source: (u32, u32), geometry: &Geometry) -> usize {
    let frame = geometry.frame(source.0 as f32, source.1 as f32);
    let long = frame.size.0.max(frame.size.1);
    ((long / BIG as f32).floor() as usize).max(1)
}

/// `image` box-averaged by `factor` each way, in linear light; the
/// pixels past the last whole block are left out.
pub(crate) fn reduce(image: &WorkingImage, factor: usize) -> WorkingImage {
    let factor = factor.max(1);
    let (w, h) = (
        (image.width / factor).max(1),
        (image.height / factor).max(1),
    );
    let mut data = vec![0.0f32; w * h * 3];
    let n = (factor * factor) as f32;
    data.par_chunks_mut(w * 3).enumerate().for_each(|(y, out)| {
        for (x, px) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let mut sum = [0.0f32; 3];
            for sy in y * factor..((y + 1) * factor).min(image.height) {
                let row = &image.data[sy * image.width * 3..][..image.width * 3];
                for sx in x * factor..((x + 1) * factor).min(image.width) {
                    for (s, v) in sum.iter_mut().zip(&row[sx * 3..sx * 3 + 3]) {
                        *s += v;
                    }
                }
            }
            *px = sum.map(|s| s / n);
        }
    });
    WorkingImage {
        width: w,
        height: h,
        data,
    }
}

/// Rows of a viewport texture as the GPU gives them back (RGBA half
/// floats, eight bytes a texel), `width` texels each and a whole number
/// of `factor`s of them, box-averaged by `factor` into `out`, its alpha
/// dropped.
fn reduce_halves(rows: &[u8], width: usize, factor: usize, out: &mut Vec<f32>) {
    let line = width * 8;
    let bands = rows.len() / line / factor;
    let w = width / factor;
    let n = (factor * factor) as f32;
    let start = out.len();
    out.resize(start + bands * w * 3, 0.0);
    out[start..]
        .par_chunks_mut(w * 3)
        .enumerate()
        .for_each(|(band, o)| {
            for (x, px) in o.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                for r in 0..factor {
                    let row = &rows[(band * factor + r) * line..][..line];
                    for sx in x * factor..(x + 1) * factor {
                        let texel = &row[sx * 8..sx * 8 + 6];
                        for (c, s) in sum.iter_mut().enumerate() {
                            let bits = u16::from_le_bytes([texel[c * 2], texel[c * 2 + 1]]);
                            *s += half::f16::from_bits(bits).to_f32();
                        }
                    }
                }
                *px = sum.map(|s| s / n);
            }
        });
}

/// The viewport's texture read back `factor` times smaller each way, a
/// band of rows at a time so the whole picture is never held at full
/// size on the CPU.
pub(crate) fn read_back_reduced(
    ctx: &greycard_gpu::Context,
    texture: &greycard_gpu::wgpu::Texture,
    factor: usize,
) -> Result<WorkingImage, String> {
    let factor = factor.max(1);
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let (w, h) = (width / factor, height / factor);
    if w == 0 || h == 0 {
        return Err(format!("a {width}x{height} picture reduced by {factor}"));
    }
    // About 64 MB of halves a band, a whole number of factors of rows.
    let per_band = ((64 << 20) / (width * 8 * factor)).max(1) * factor;
    let mut data = Vec::with_capacity(w * h * 3);
    let mut y = 0;
    while y < h * factor {
        let rows = per_band.min(h * factor - y);
        let bytes = ctx
            .read_viewport_rows(texture, y as u32, rows as u32)
            .map_err(|e| e.to_string())?;
        reduce_halves(&bytes, width, factor, &mut data);
        y += rows;
    }
    Ok(WorkingImage {
        width: w,
        height: h,
        data,
    })
}

/// The edit's pictures from a developed picture already reduced by a
/// whole factor (`reduced`, of the developed picture `finish.source`
/// before its geometry): the geometry, a resize to [`BIG`] on the long
/// edge, the finish as an export at that size is finished (with no
/// output sharpening), the frame's own turns taken back out, and the
/// made sizes shrunk from that.
pub(crate) fn make(reduced: &WorkingImage, finish: &Finish) -> Made {
    let edit = &finish.edit;
    let framed;
    let image = if edit.geometry.is_identity() {
        reduced
    } else {
        framed = crate::geometry::apply(reduced, &edit.geometry);
        &framed
    };
    let fitted = crate::export::fit(image, Some(BIG));
    let image = fitted.as_ref().unwrap_or(image);
    let (sw, sh) = (finish.source.0 as f32, finish.source.1 as f32);
    let frame_width = edit.geometry.frame(sw, sh).size.0;
    let settings = crate::export::Settings {
        format: crate::export::Format::Jpeg,
        long_edge: None,
        space: crate::export::Space::Srgb,
        sharpen: crate::export::Sharpen::Off,
        watermark: None,
        ..crate::export::Settings::default()
    };
    let rendered = crate::export::render_framed(
        image,
        frame_width,
        edit,
        finish.source,
        &settings,
        &finish.rasters,
        finish.clip_level,
        finish.guide.as_deref(),
        finish.kind,
        finish.white.as_ref(),
    );
    let crate::export::Pixels::Eight(rgb) = rendered.pixels else {
        unreachable!("a JPEG's pixels are eight bits")
    };
    let big = unturned(
        Thumb {
            width: rendered.width,
            height: rendered.height,
            rgb,
        },
        edit.geometry.shown_turns(finish.turn),
    );
    let thumbs = crate::grid::MADE
        .iter()
        .map(|&size| (size, shrunk(&big, size)))
        .collect();
    Made { big, thumbs }
}

/// A finished picture turned back by the frame's shown turns, so the
/// strip's and the loupe's own turn at draw time puts it back: a turn is
/// undone by the turn the other way, and a mirror with a turn is its own
/// undoing.
pub(crate) fn unturned(thumb: Thumb, (turns, flip): (u8, bool)) -> Thumb {
    let turns = turns % 4;
    if turns == 0 && !flip {
        return thumb;
    }
    let back = if flip { turns } else { (4 - turns) % 4 };
    let (width, height, rgb) =
        greycard_edit::geometry::turn_pixels(thumb.width, thumb.height, &thumb.rgb, back, flip);
    Thumb { width, height, rgb }
}

/// `picture` area-averaged to `size` on its long edge; left as it is
/// when it is no larger.
fn shrunk(picture: &Thumb, size: u32) -> Thumb {
    let (w, h) = (picture.width, picture.height);
    let long = w.max(h);
    if long <= size {
        return picture.clone();
    }
    let scale = size as f64 / long as f64;
    let (sw, sh) = (
        ((w as f64 * scale).round() as u32).clamp(1, size),
        ((h as f64 * scale).round() as u32).clamp(1, size),
    );
    let image =
        image::RgbImage::from_raw(w, h, picture.rgb.clone()).expect("a thumb's bytes are its size");
    let small = image::imageops::thumbnail(&image, sw, sh);
    Thumb {
        width: sw,
        height: sh,
        rgb: small.into_raw(),
    }
}

/// The developed picture the edit's pictures are made from.
pub(crate) enum Developed {
    /// The develop's own picture on the CPU, at full size.
    Full(Arc<WorkingImage>),
    /// Already reduced, read back from the viewport's texture.
    Reduced(WorkingImage),
}

/// One keeping: the frame, its content hash and key, the picture and
/// what finishes it.
pub(crate) struct Keeping {
    pub(crate) path: PathBuf,
    pub(crate) hash: String,
    pub(crate) key: u64,
    pub(crate) developed: Developed,
    /// The whole factor the developed picture is reduced by: here for a
    /// full-size one, already for one read back reduced.
    pub(crate) factor: usize,
    /// How long the read back took, when there was one.
    pub(crate) read_back: Option<f64>,
    /// The frame's ISO from its row, as the frame is held.
    pub(crate) iso: Iso,
    pub(crate) finish: Finish,
    /// The picture's place in hand, let go with the keeping.
    pub(crate) marked: Marked,
}

/// Make and keep the edit's pictures of one frame, then say so to
/// `done` with the frame's path: on the keeping thread.
pub(crate) fn keep(edited: &Edited, job: Keeping, done: impl FnOnce(PathBuf)) {
    let started = Instant::now();
    let reduced;
    let image = match &job.developed {
        Developed::Full(full) if job.factor > 1 => {
            reduced = reduce(full, job.factor);
            &reduced
        }
        Developed::Full(full) => &**full,
        Developed::Reduced(r) => r,
    };
    let (rw, rh) = (image.width, image.height);
    let reducing = started.elapsed().as_secs_f64();
    let made = make(image, &job.finish);
    let making = started.elapsed().as_secs_f64() - reducing;
    let stored = edited.store(&job.path, &job.hash, job.key, job.iso, &made);
    // Kept, or not: a save of the same picture from now on looks for it
    // in the cache rather than waiting on this one.
    drop(job.marked);
    let total = started.elapsed().as_secs_f64();
    let name = crate::panel::browser::file_name(&job.path);
    match stored {
        Ok(bytes) => {
            tracing::info!(
                "the edit's pictures of {name}: {}x{} and {} thumbnails, {} KB, in {:.0} ms \
                 ({}reduced to {rw}x{rh} by {} in {:.0} ms, made in {:.0} ms, kept in {:.0} ms)",
                made.big.width,
                made.big.height,
                made.thumbs.len(),
                bytes / 1024,
                (total + job.read_back.unwrap_or(0.0)) * 1e3,
                job.read_back
                    .map(|s| format!("read back in {:.0} ms, ", s * 1e3))
                    .unwrap_or_default(),
                job.factor,
                reducing * 1e3,
                making * 1e3,
                (total - reducing - making) * 1e3,
            );
            done(job.path);
        }
        Err(e) => tracing::warn!("the edit's pictures of {name}: not kept: {e}"),
    }
}

/// Frame `i`'s ISO as its row gave it (`rows::FromRow::iso`).
fn iso_of(st: &crate::State, i: usize) -> Iso {
    st.from_row.get(i).and_then(|r| r.iso)
}

/// Whether the window holds frame `i` as edited: by its row while it
/// stands in from one (the index's `edited`), by its sidecar's edit once
/// that is read, under the rule the cell's edited mark goes by.
fn held_edited(st: &crate::State, i: usize) -> bool {
    match st.from_row.get(i) {
        Some(row) if !row.read => row.edited,
        _ => st
            .sidecars
            .get(i)
            .zip(st.files.get(i))
            .is_some_and(|(s, path)| is_edited(path, &s.current, iso_of(st, i))),
    }
}

/// A new list in the window: the worker is told which of its frames
/// are edited, before their thumbnails are asked for.
pub(crate) fn note_list(st: &crate::State, worker: &crate::worker::Worker) {
    let list: Vec<(PathBuf, bool, Iso)> = (0..st.files.len())
        .map(|i| (st.files[i].clone(), held_edited(st, i), iso_of(st, i)))
        .collect();
    worker.edited().note_all(list);
}

/// Frame `file`'s sidecar was written, or taken whole from its archive
/// copy. The open frame's edit goes to the worker, which keeps the
/// pictures of it from the develop in hand (or when that lands, or when
/// the frame is left); any other frame (rated in the browser, culled,
/// or joined) is held as edited or not by what it is now, its key read
/// again, and its cells asked again when that may change their picture.
pub(crate) fn saved(st: &crate::State, file: usize) {
    let (Some(path), Some(sidecar)) = (st.files.get(file), st.sidecars.get(file)) else {
        return;
    };
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    let iso = iso_of(st, file);
    if st.current == Some(file) && st.cull.is_none() {
        worker.send(crate::worker::Job::Keep {
            path: path.clone(),
            edit: sidecar.current.clone(),
            turn: sidecar.turn,
            iso,
        });
        return;
    }
    if worker
        .edited()
        .note(path, is_edited(path, &sidecar.current, iso), iso)
    {
        worker.send(crate::rows::thumb_job(st, file));
    }
}

/// The window's develop of the open frame landed at `turn`: when what it
/// developed is the frame's saved edit, the worker keeps its pictures
/// from it (or finds them kept already, a key and a few stats), so a
/// frame edited before its pictures were kept has them once it is
/// opened. A frame with no edit, and none held, asks nothing.
pub(crate) fn landed(st: &crate::State, turn: u8) {
    let Some(c) = st.current.filter(|_| st.cull.is_none()) else {
        return;
    };
    let (Some(path), Some(sidecar)) = (st.files.get(c), st.sidecars.get(c)) else {
        return;
    };
    // The sidecar on disk is the frame's own, and holds what was
    // developed: nothing on the panel is unsaved.
    if !crate::panel::edit::writable(st, c) || sidecar.current != st.edit || sidecar.turn != turn {
        return;
    }
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    let iso = iso_of(st, c);
    if !is_edited(path, &sidecar.current, iso) && !worker.edited().holds(path) {
        return;
    }
    worker.send(crate::worker::Job::Keep {
        path: path.clone(),
        edit: sidecar.current.clone(),
        turn,
        iso,
    });
}

/// What the window holds of frame `i` changed (its row came in, or its
/// sidecar was read): the worker holds it as edited or not to match, and
/// when that turned, its cells ask for their pictures again.
pub(crate) fn refresh(st: &mut crate::State, i: usize) {
    let Some(path) = st.files.get(i).cloned() else {
        return;
    };
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    if worker
        .edited()
        .hold(&path, held_edited(st, i), iso_of(st, i))
    {
        kept(st, &worker, &path);
    }
}

/// Frame `i`'s sidecar changed on disk under the window, as its row now
/// says (another machine's save joined, a pass that found it rewritten):
/// its key is read again, and its cells ask for their pictures again.
pub(crate) fn changed(st: &mut crate::State, i: usize) {
    let Some(path) = st.files.get(i).cloned() else {
        return;
    };
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    if worker
        .edited()
        .note(&path, held_edited(st, i), iso_of(st, i))
    {
        kept(st, &worker, &path);
    }
}

/// The pictures `path` shows may have changed (`Outcome::EditedKept`):
/// its cells ask for theirs again, and the loupe drops its copy so its
/// next refresh asks too.
pub(crate) fn kept(st: &mut crate::State, worker: &crate::worker::Worker, path: &Path) {
    let Some(i) = st.files.iter().position(|f| f == path) else {
        return;
    };
    worker.send(crate::rows::thumb_job(st, i));
    let mut culling = false;
    for cull in [st.cull.as_mut(), st.hold.as_mut()].into_iter().flatten() {
        cull.cache.forget(i);
        culling = true;
    }
    if culling && st.cull.is_some() {
        crate::panel::cull::cull_refresh(st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32) -> Thumb {
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&[x as u8, y as u8, (x * 7 + y * 3) as u8]);
            }
        }
        Thumb {
            width: w,
            height: h,
            rgb,
        }
    }

    #[test]
    fn a_picture_unturned_and_turned_at_draw_time_is_the_picture() {
        let p = picture(5, 3);
        for turns in 0..4 {
            for flip in [false, true] {
                let (w, h, rgb) =
                    greycard_edit::geometry::turn_pixels(p.width, p.height, &p.rgb, turns, flip);
                let back = unturned(
                    Thumb {
                        width: w,
                        height: h,
                        rgb,
                    },
                    (turns, flip),
                );
                assert_eq!(back, p, "{turns} turns, mirrored {flip}");
            }
        }
    }

    #[test]
    fn the_reduction_averages_whole_blocks_and_the_halves_agree() {
        let (w, h) = (7, 5);
        let data: Vec<f32> = (0..w * h * 3).map(|i| (i % 11) as f32 * 0.125).collect();
        let image = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let r = reduce(&image, 2);
        assert_eq!((r.width, r.height), (3, 2));
        let at = |x: usize, y: usize, c: usize| image.data[(y * w + x) * 3 + c];
        let want = (at(2, 2, 1) + at(3, 2, 1) + at(2, 3, 1) + at(3, 3, 1)) / 4.0;
        assert!((r.data[(3 + 1) * 3 + 1] - want).abs() < 1e-6);
        // The same picture as the viewport's texture holds it.
        let mut halves = Vec::new();
        for y in 0..4 {
            for x in 0..w {
                for c in 0..4 {
                    let v = if c == 3 { 1.0 } else { at(x, y, c) };
                    halves.extend_from_slice(&half::f16::from_f32(v).to_bits().to_le_bytes());
                }
            }
        }
        let mut out = Vec::new();
        reduce_halves(&halves, w, 2, &mut out);
        assert_eq!(out.len(), r.data.len());
        for (a, b) in out.iter().zip(&r.data) {
            assert!((a - b).abs() < 1e-3, "{a} against {b}");
        }
        assert_eq!(reduce(&image, 1).data, image.data);
    }

    #[test]
    fn the_factor_keeps_the_frame_at_the_big_size_or_more() {
        let g = Geometry::default();
        assert_eq!(factor_for((6000, 4000), &g), 2);
        assert_eq!(factor_for((8192, 5464), &g), 4);
        assert_eq!(factor_for((1600, 1200), &g), 1);
        let cropped = Geometry {
            crop: Some(greycard_edit::geometry::Crop {
                x: 0.25,
                y: 0.25,
                w: 0.5,
                h: 0.5,
            }),
            ..Geometry::default()
        };
        assert_eq!(factor_for((8192, 5464), &cropped), 2);
    }

    /// What the worker said, as the test reads it.
    #[derive(Debug)]
    enum Heard {
        Developed,
        Kept(PathBuf),
        Thumb {
            size: u32,
            w: u32,
            h: u32,
            mean: f64,
            slope: (f64, f64),
        },
    }

    fn mean(rgb: &[u8]) -> f64 {
        rgb.iter().map(|&v| v as f64).sum::<f64>() / rgb.len().max(1) as f64
    }

    /// Which way the test picture runs: red from the left column to the
    /// right, green from the top row to the bottom. Both rise in the
    /// picture as the camera's tag turns it.
    fn slope(w: usize, h: usize, rgb: &[u8]) -> (f64, f64) {
        let at = |x: usize, y: usize, c: usize| rgb[(y * w + x) * 3 + c] as f64;
        (
            at(w - 1, h / 2, 0) - at(0, h / 2, 0),
            at(w / 2, h - 1, 1) - at(w / 2, 0, 1),
        )
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-edited-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Rig {
        worker: crate::worker::Worker,
        heard: std::sync::mpsc::Receiver<Heard>,
        dir: PathBuf,
        png: PathBuf,
        /// The last thumbnail's [`slope`].
        slope: std::cell::Cell<(f64, f64)>,
    }

    /// A worker with a cache of its own and a PNG of a dim gradient,
    /// 600 by 400, opened and developed on the CPU.
    fn rig(name: &str) -> Rig {
        use crate::worker::{Job, Outcome, Worker};
        let dir = scratch(name);
        let png = dir.join("frame.png");
        let picture = image::RgbImage::from_fn(600, 400, |x, y| {
            image::Rgb([(30 + x / 12) as u8, (40 + y / 10) as u8, 50])
        });
        picture.save(&png).unwrap();
        let (tx, heard) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = Worker::new(move |o| {
            let said = match o {
                Outcome::Developed { .. } => Heard::Developed,
                Outcome::EditedKept { path } => Heard::Kept(path),
                Outcome::Thumbnail {
                    size,
                    width,
                    height,
                    rgb,
                    ..
                } => Heard::Thumb {
                    size,
                    w: width,
                    h: height,
                    mean: mean(&rgb),
                    slope: slope(width as usize, height as usize, &rgb),
                },
                _ => return,
            };
            let _ = tx.lock().unwrap().send(said);
        });
        worker.set_thumb_cache(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        ));
        worker.send(Job::Open {
            path: png.clone(),
            edit: Edit::for_picture(),
            generation: 1,
            seed_blend: false,
            turn: 0,
        });
        let rig = Rig {
            worker,
            heard,
            dir,
            png,
            slope: std::cell::Cell::new((0.0, 0.0)),
        };
        assert!(matches!(rig.next(), Heard::Developed));
        rig
    }

    impl Rig {
        fn next(&self) -> Heard {
            self.heard
                .recv_timeout(std::time::Duration::from_secs(120))
                .expect("the worker answers")
        }

        /// The strip's picture of the frame, asked for as a cell asks.
        fn thumb(&self) -> (u32, u32, f64) {
            self.worker.send(crate::worker::Job::Thumbnail {
                index: 0,
                path: self.png.clone(),
            });
            loop {
                if let Heard::Thumb {
                    size,
                    w,
                    h,
                    mean,
                    slope,
                } = self.next()
                {
                    assert_eq!(size, crate::grid::made_size(crate::worker::THUMB_WIDTH));
                    self.slope.set(slope);
                    return (w, h, mean);
                }
            }
        }

        fn save(&self, edit: &Edit, turn: u8) {
            self.worker.send(crate::worker::Job::Keep {
                path: self.png.clone(),
                edit: edit.clone(),
                turn,
                iso: None,
            });
        }

        fn kept(&self) {
            loop {
                match self.next() {
                    Heard::Kept(path) => {
                        assert_eq!(path, self.png);
                        return;
                    }
                    Heard::Thumb { .. } | Heard::Developed => {}
                }
            }
        }

        /// The culling loupe's picture of the frame at a view's size:
        /// whether it is the edit's, whether it is a local preview, and
        /// its size.
        fn loupe(&self) -> (bool, bool, u32, u32) {
            let previews = self.worker.previews();
            let got = std::cell::RefCell::new(None);
            crate::cull::fetch(
                &crate::cull::Want::for_loupe(0, self.png.clone(), 1000),
                Some(&previews),
                &|loaded| {
                    if let crate::cull::Loaded::Ok { preview, .. } = loaded {
                        *got.borrow_mut() = Some(preview);
                    }
                },
            );
            let p = got.into_inner().expect("a picture");
            (p.edited, p.local, p.width, p.height)
        }

        /// Nothing said of the frame for a second.
        fn quiet(&self) {
            let said = self.heard.recv_timeout(std::time::Duration::from_secs(1));
            assert!(
                !matches!(said, Ok(Heard::Kept(_))),
                "a save that changed nothing asked the cells again"
            );
        }

        fn entries(&self) -> usize {
            greycard_library::thumbs::usage_at(&self.dir.join("thumbs")).entries
        }
    }

    /// The open frame brightened and saved: the strip's picture of it is
    /// made from the edit, brighter than the camera's, at the made size,
    /// and so is the loupe's at 2048; saved again unchanged (a rating)
    /// nothing is made again; and reset, the camera's is the picture
    /// again and the edit's are gone from the cache.
    #[test]
    fn a_saved_edit_shows_in_the_strip_and_a_reset_shows_the_camera_s() {
        let rig = rig("bright");
        let (cw, ch, camera) = rig.thumb();
        assert_eq!(
            (cw, ch),
            (150, 100),
            "the camera's picture, a whole factor down"
        );
        assert!(!rig.loupe().0, "no edit, the camera's");
        let mut bright = Edit::for_picture();
        bright.light.exposure = 1.0;
        rig.save(&bright, 0);
        rig.kept();
        let (w, h, edited) = rig.thumb();
        assert_eq!((w, h), (176, 117), "the edit's, to the made size");
        assert!(
            edited > camera + 20.0,
            "a stop up: {edited:.1} against the camera's {camera:.1}"
        );
        let (edited_up, local, lw, lh) = rig.loupe();
        assert!(edited_up && !local);
        assert_eq!((lw, lh), (600, 400), "no larger than it is");
        let kept = rig.entries();
        assert!(kept >= 6, "the camera's and the edit's five: {kept}");
        // A save that changes nothing of the picture (a rating) makes
        // nothing, and asks no cell again.
        rig.save(&bright, 0);
        rig.quiet();
        assert_eq!(rig.entries(), kept);
        // Another edit replaces the last one's pictures, not adds to them.
        bright.light.exposure = 1.5;
        rig.save(&bright, 0);
        rig.kept();
        assert_eq!(rig.entries(), kept);
        assert!(rig.thumb().2 > edited);
        // The crop is in the picture: the left half of the frame.
        bright.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        });
        rig.save(&bright, 0);
        rig.kept();
        let (w, h, _) = rig.thumb();
        assert_eq!((w, h), (132, 176), "300 by 400, to the made size");
        assert_eq!(rig.entries(), kept);
        // Reset: the camera's again, the edit's gone.
        rig.save(&Edit::for_picture(), 0);
        rig.kept();
        let (w, h, again) = rig.thumb();
        assert_eq!((w, h), (cw, ch), "the camera's again");
        assert!(
            (again - camera).abs() < 1.0,
            "{again:.1} against {camera:.1}"
        );
        assert_eq!(rig.entries(), kept - 5);
        assert!(!rig.loupe().0);
        crate::testing::remove_dir_retry(&rig.dir);
    }

    /// A frame turned a quarter is kept as the camera's tag turns it,
    /// for the strip to turn at draw time; a save whose develop is not
    /// in hand waits for it; and a frame left before it lands shows the
    /// camera's.
    #[test]
    fn a_turned_frame_is_kept_unturned_and_a_save_waits_for_its_develop() {
        use crate::worker::Job;
        let rig = rig("turned");
        let (_, _, camera) = rig.thumb();
        let (right, down) = rig.slope.get();
        assert!(
            right > 10.0 && down > 10.0,
            "the camera's runs right and down"
        );
        // The frame turned: the window develops it again at the turn.
        let mut bright = Edit::for_picture();
        bright.light.exposure = 1.0;
        rig.worker.send(Job::Develop {
            edit: bright.clone(),
            generation: 2,
            turn: 1,
        });
        assert!(matches!(rig.next(), Heard::Developed));
        rig.save(&bright, 1);
        rig.kept();
        let (w, h, edited) = rig.thumb();
        assert_eq!((w, h), (176, 117), "unturned, as the camera's is");
        assert!(edited > camera + 20.0);
        let (right, down) = rig.slope.get();
        assert!(
            right > 10.0 && down > 10.0,
            "and runs the camera's way: {right:.0} right, {down:.0} down"
        );
        // Turned a quarter and mirrored by the crop panel's own turn.
        let mut mirrored = bright.clone();
        mirrored.geometry.flip = true;
        mirrored.geometry.turns = 1;
        rig.worker.send(Job::Develop {
            edit: mirrored.clone(),
            generation: 5,
            turn: 1,
        });
        assert!(matches!(rig.next(), Heard::Developed));
        rig.save(&mirrored, 1);
        rig.kept();
        let (w, h, _) = rig.thumb();
        assert_eq!((w, h), (176, 117));
        let (right, down) = rig.slope.get();
        assert!(
            right > 10.0 && down > 10.0,
            "{right:.0} right, {down:.0} down"
        );
        // Texture is a develop's: the save comes before the develop
        // lands, and its pictures with it.
        let mut textured = bright.clone();
        textured.detail.texture = 30.0;
        rig.save(&textured, 1);
        rig.worker.send(Job::Develop {
            edit: textured.clone(),
            generation: 3,
            turn: 1,
        });
        rig.kept();
        assert!(rig.thumb().2 > camera + 20.0);
        // Saved again, darker, and the frame left before a develop of
        // it: the camera's stands in, the key having moved on.
        let mut dark = textured.clone();
        dark.detail.texture = 0.0;
        dark.light.exposure = -1.0;
        let mut sidecar = greycard_edit::Sidecar {
            current: dark.clone(),
            turn: 1,
            ..Default::default()
        };
        sidecar.save(&rig.png).unwrap();
        rig.save(&dark, 1);
        let other = rig.dir.join("other.png");
        image::RgbImage::from_pixel(40, 30, image::Rgb([9, 9, 9]))
            .save(&other)
            .unwrap();
        rig.worker.send(Job::Open {
            path: other,
            edit: Edit::for_picture(),
            generation: 4,
            seed_blend: false,
            turn: 0,
        });
        rig.kept();
        let (_, _, left) = rig.thumb();
        assert!((left - camera).abs() < 1.0, "{left:.1} against {camera:.1}");
        crate::testing::remove_dir_retry(&rig.dir);
    }

    /// A picture in hand is not taken in hand again until its making is
    /// done or dropped; another key, or another frame's content, is.
    #[test]
    fn a_picture_in_hand_is_made_once() {
        let edited = Edited::new(Arc::new(Mutex::new(None)));
        let first = edited.mark("ab", 7).expect("free");
        assert!(edited.mark("ab", 7).is_none(), "in hand");
        let other = edited.mark("ab", 8).expect("another key");
        let elsewhere = edited.mark("cd", 7).expect("another content");
        drop(first);
        assert!(edited.mark("ab", 7).is_some(), "let go");
        drop((other, elsewhere));
    }

    /// A frame held as edited from its row or its sidecar, and let go,
    /// says when that turned; a key kept and found again is no change.
    #[test]
    fn holding_a_frame_says_when_it_turned() {
        let edited = Edited::new(Arc::new(Mutex::new(None)));
        let path = Path::new("/x/IMG_0001.CR3");
        assert!(!edited.holds(path));
        assert!(edited.hold(path, true, None));
        assert!(!edited.hold(path, true, None));
        assert!(edited.settle_key(path, "ab", 5, None));
        assert!(!edited.settle_key(path, "ab", 5, None));
        assert!(!edited.hold(path, true, None), "a key found stays");
        assert_eq!(edited.shows(path), Shows::Edit(5));
        assert_eq!(edited.shows_offline(path), Shows::Newest);
        assert!(edited.hold(path, false, None));
        assert_eq!(edited.shows(path), Shows::Camera);
        assert!(!edited.note(path, false, None), "neither was nor is");
        assert!(edited.note(path, true, None));
        edited.settle_key(path, "ab", 6, None);
        // The lens database moved on: the key is read again.
        assert!(edited.set_lenses(Some("another".into())));
        assert!(!edited.set_lenses(Some("another".into())));
        assert_eq!(
            edited.shows(path),
            Shows::Newest,
            "read again, from a sidecar that is not there"
        );
        edited.note_all([(PathBuf::from("/x/IMG_0002.CR3"), true, None)]);
        assert!(!edited.holds(path), "another list's frames go");
    }

    /// A picture's mean, and the strip's drawn copy of it when the row
    /// has a cell: what frame `i` shows in the window.
    fn shown(st: &crate::State, app: &crate::App, i: usize) -> (f64, Option<f64>) {
        use slint::Model;
        let (_, _, rgb) = st.thumb_base[i].as_ref().expect("a picture held");
        let drawn = crate::panel::browser::row_of(st, i)
            .and_then(|row| app.get_thumbs().row_data(row))
            .and_then(|t| t.image.to_rgb8())
            .map(|buf| mean(buf.as_bytes()));
        (mean(rgb), drawn)
    }

    /// The window's own hooks, headless: a frame opened, brightened and
    /// saved through the panel's save shows its edit in the strip; a
    /// reset saved the same way shows the camera's; and a frame not open
    /// turned in the strip keeps its edit's picture, the turn being drawn
    /// and not kept.
    #[test]
    fn the_window_shows_a_saved_edit_and_a_turned_frame_keeps_it() {
        use crate::worker::{Job, Outcome, Worker};
        use std::time::{Duration, Instant};
        let dir = scratch("window");
        let files: Vec<PathBuf> = ["a.png", "b.png"]
            .iter()
            .map(|name| {
                let path = dir.join(name);
                image::RgbImage::from_fn(300, 200, |x, y| {
                    image::Rgb([(30 + x / 6) as u8, (40 + y / 5) as u8, 50])
                })
                .save(&path)
                .unwrap();
                path
            })
            .collect();
        let app = crate::testing::window(2);
        let (state, _) = crate::testing::state_for(&app, files.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = std::rc::Rc::new(Worker::new(move |o| {
            let _ = tx.lock().unwrap().send(o);
        }));
        worker.set_thumb_cache(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        ));
        crate::WORKER.with(|w| *w.borrow_mut() = Some(worker.clone()));
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            for s in &mut st.sidecars {
                s.current = Edit::for_picture();
            }
            st.current = Some(0);
            crate::panel::browser::rebuild_browser(&mut st, &app);
            note_list(&st, &worker);
            for i in 0..2 {
                worker.send(crate::rows::thumb_job(&st, i));
            }
        }
        // The window takes in what comes, as its event loop would; a
        // develop's own landing is the viewport's and not this test's.
        let developed = std::cell::Cell::new(0);
        let pump = |until: &dyn Fn(&crate::State) -> bool, what: &str| {
            let deadline = Instant::now() + Duration::from_secs(120);
            while !until(&state.borrow()) {
                assert!(Instant::now() < deadline, "waiting for {what}");
                let Ok(outcome) = rx.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                if matches!(outcome, Outcome::Developed { .. }) {
                    developed.set(developed.get() + 1);
                }
                if matches!(
                    outcome,
                    Outcome::Thumbnail { .. }
                        | Outcome::NoThumbnail { .. }
                        | Outcome::Thumbnails(_)
                        | Outcome::EditedKept { .. }
                ) {
                    crate::panel::deliver::deliver(&app, outcome);
                }
            }
        };
        pump(
            &|st| st.thumb_base.iter().all(Option::is_some),
            "the camera's",
        );
        let camera = shown(&state.borrow(), &app, 0);
        // Open the frame: its develop in the worker's hand.
        let opened = |path: &Path, generation: u64| {
            worker.send(Job::Open {
                path: path.to_path_buf(),
                edit: Edit::for_picture(),
                generation,
                seed_blend: false,
                turn: 0,
            });
        };
        opened(&files[0], 1);
        pump(&|_| developed.get() == 1, "the develop");
        // Brightened on the panel and saved by the panel's own save.
        {
            let mut st = state.borrow_mut();
            st.sidecars[0].current.light.exposure = 1.0;
            assert!(crate::panel::edit::save_sidecar(&mut st, 0));
        }
        pump(
            &|st| shown(st, &app, 0).0 > camera.0 + 20.0,
            "the edit's picture in the strip",
        );
        let edited = shown(&state.borrow(), &app, 0);
        if let (Some(drawn), Some(before)) = (edited.1, camera.1) {
            assert!(
                drawn > before + 20.0,
                "drawn: {drawn:.1} against {before:.1}"
            );
        }
        // Reset and saved: the camera's again.
        {
            let mut st = state.borrow_mut();
            st.sidecars[0].current = Edit::for_picture();
            assert!(crate::panel::edit::save_sidecar(&mut st, 0));
        }
        pump(
            &|st| (shown(st, &app, 0).0 - camera.0).abs() < 1.0,
            "the camera's again",
        );
        // Brightened and kept again, then left for the other frame.
        {
            let mut st = state.borrow_mut();
            st.sidecars[0].current.light.exposure = 1.0;
            assert!(crate::panel::edit::save_sidecar(&mut st, 0));
        }
        pump(
            &|st| shown(st, &app, 0).0 > camera.0 + 20.0,
            "the edit's picture again",
        );
        state.borrow_mut().current = Some(1);
        opened(&files[1], 2);
        // Turned in the strip while it is not open: its sidecar written,
        // its cell asked again, and it keeps the edit's picture.
        {
            let mut st = state.borrow_mut();
            crate::panel::browser::turn_frames(&mut st, &app, &worker, &[0], 1);
            assert_eq!(st.sidecars[0].turn, 1);
            st.thumb_base[0] = None;
            worker.send(crate::rows::thumb_job(&st, 0));
        }
        pump(
            &|st| st.thumb_base[0].is_some(),
            "the turned frame's picture",
        );
        let turned = shown(&state.borrow(), &app, 0).0;
        assert!(
            (turned - edited.0).abs() < 1.0,
            "the turned frame shows its edit: {turned:.1}, the edit's {:.1}, the camera's {:.1}",
            edited.0,
            camera.0
        );
        crate::WORKER.with(|w| *w.borrow_mut() = None);
        crate::STATE.with(|s| *s.borrow_mut() = None);
        worker.stop();
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A picture without something its edit asks for (here a look not in
    /// the look directory) is not kept under the edit's key: the frame
    /// shows the camera's.
    #[test]
    fn a_picture_missing_what_its_edit_asks_for_is_not_kept() {
        let rig = rig("left-out");
        let (_, _, camera) = rig.thumb();
        let mut bright = Edit::for_picture();
        bright.light.exposure = 1.0;
        bright.look_lut = greycard_edit::LookLut {
            lut: greycard_edit::look::LutChoice::Named("Not here at all".into()),
            strength: 1.0,
        };
        rig.save(&bright, 0);
        rig.kept();
        let (_, _, shown) = rig.thumb();
        assert!(
            (shown - camera).abs() < 1.0,
            "{shown:.1} against {camera:.1}"
        );
        assert!(!rig.loupe().0);
        crate::testing::remove_dir_retry(&rig.dir);
    }

    #[test]
    fn the_made_sizes_are_shrunk_from_the_big_one() {
        let big = picture(300, 200);
        assert_eq!(shrunk(&big, 128).width, 128);
        assert_eq!(shrunk(&big, 128).height, 85);
        assert_eq!(shrunk(&big, 360), big);
    }
}
