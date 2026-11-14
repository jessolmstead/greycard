//! Thumbnails from the edit: a frame with an edit shows its edit in the
//! strip, the grid, the lightbox, a versions compare and the develop
//! view's stand-in for its develop, and a frame without one keeps the
//! camera's JPEG. Cull on the camera's JPEG shows the camera's JPEG,
//! as its switch says; Developed is the switch for culling on the edit.
//!
//! The pictures made from an edit are kept in the thumbnail cache
//! beside the camera's, under the file's content hash, at each made size
//! (`made::MADE`) and at the local previews' 2048, with the edit's
//! develop key ([`greycard_edit::Edit::develop_key`]) in the place of the
//! camera's stamp and [`EDITED_RECIPE`] in the place of its recipe. The
//! camera's entries stay: they stand in until the edited ones land, and
//! they are the picture again when the edit is reset. Like the camera's,
//! an edited picture is kept as the camera's tag turns it and no
//! further: its crop and straighten are in it, and the frame's own
//! quarter turns and mirror are taken back out, so the strip and the
//! loupe turn it at draw time exactly as they turn the camera's.
//!
//! Which frames show an edit is [`Edited`]'s to say. A frame here is a
//! [`Key`]: a file's path and the version of it, the file's own for
//! none, since each version of a file is a line of its own with an
//! edit of its own, kept under its own key beside the others' under
//! the one content hash. The window tells it, with each list, which
//! frames it holds as edited (the index's `edited` for a frame standing
//! in from its row, the sidecar's edit against the frame's default for
//! one read); the key of each is found the first time a picture of it
//! is looked up, by reading its sidecar on the thumbnail's own thread,
//! and kept. A frame whose sidecar cannot
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

pub use greycard_render::reduce::reduce;
use greycard_render::reduce::reduce_halves;

use crate::version_key::Key;
use crate::worker::ThumbCache;

/// The recipe the pictures made from an edit are kept under, where the
/// camera's carry `worker::THUMB_RECIPE`: the high bit says "made from
/// the edit", so the two never name one entry. Raise the low bits when
/// the shrink, the un-turn or the encode below make another picture
/// from the same finished one; a change to the develop or the finish
/// is `greycard_edit::key::DEVELOP_RECIPE`'s.
pub const EDITED_RECIPE: u16 = 0x8001;

/// The recipe the pictures the reduced develop made are kept under
/// (`reduced`): the edit's too, and looked up after
/// [`EDITED_RECIPE`]'s, but made from the binned mosaic with no capture
/// sharpen and no denoise. The open frame's develop makes the edit's
/// pictures under [`EDITED_RECIPE`] when the frame is opened, and they
/// take these ones' place.
pub const REDUCED_RECIPE: u16 = 0x8002;

/// Both recipes of the edit's pictures, in the order they are looked
/// up.
const EDIT_RECIPES: [u16; 2] = [EDITED_RECIPE, REDUCED_RECIPE];

/// The long edge of the larger picture, the one the culling loupe and
/// compare show: the local previews' own.
pub const BIG: u32 = crate::previews::SIZE;

/// The thumbnails' JPEG quality, the cache's own for them.
const THUMB_QUALITY: u8 = 90;

/// A frame's ISO as the window has it from its row (`Some(None)`: the
/// camera gave none; `None`: no row stood in for it), which the learned
/// blend's seed is judged against (`greycard_library::is_default_edit`).
pub type Iso = Option<Option<u32>>;

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

/// A frame held as edited: what is known of its key, its ISO, and the
/// key its row in the index gave, if any.
///
/// The row's key is a first guess only: the index made it when it last
/// read the sidecar, under the looks, the lens database and the recipe
/// as they were then. Online the sidecar's own key is read the first
/// time a picture of the frame is looked up and wins; the row's stands
/// only for a frame whose sidecar cannot be read (its root offline),
/// where it is better than the picture kept last for the content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    want: Want,
    iso: Iso,
    row: Option<u64>,
    /// The key the frame's line is at as far as the window knows while
    /// its key is to be read again: the key its save computed (`note`),
    /// or the one last found. A hint of what it may show, for the guard
    /// on another frame's pictures (`wanted_elsewhere`); none when the
    /// line changed under the window and nobody knows its key.
    hint: Option<u64>,
}

impl Held {
    fn read(iso: Iso) -> Self {
        Self {
            want: Want::Read,
            iso,
            row: None,
            hint: None,
        }
    }

    /// Held with its key to be read, and the key its row gives as the
    /// guess for when it cannot be.
    fn from_row(iso: Iso, key: Option<u64>) -> Self {
        Self {
            want: Want::Read,
            iso,
            row: key,
            hint: None,
        }
    }

    /// Whether the frame may show `key`: it does, or its own key is not
    /// found yet and its hint is `key`, or it has none. A frame still to
    /// read its key whose line is known to be at another key is not
    /// holding `key`'s pictures.
    fn may_show(&self, key: u64) -> bool {
        match self.want {
            Want::Key(k) => k == key,
            Want::Read | Want::Unread => self.hint.is_none_or(|h| h == key),
        }
    }

    /// What it shows when its sidecar is not read: the key found, else
    /// the row's, else the picture kept last for its content.
    fn unread(&self) -> Shows {
        match (self.want, self.row) {
            (Want::Key(k), _) => Shows::Edit(k),
            (_, Some(k)) => Shows::Edit(k),
            _ => Shows::Newest,
        }
    }
}

/// What a frame shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shows {
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
pub struct Edited {
    cache: ThumbCache,
    wants: Mutex<HashMap<Key, Held>>,
    keeper: Keeper,
    /// The pictures being made or waiting to be, by content hash and
    /// key, so a second save of the same picture before the first is
    /// kept (a develop landing, then a rating) makes it once.
    in_hand: InHand,
    /// The lens database the develop reads, by where it was found and
    /// when it was made ([`lens_database`]): part of every key whose
    /// edit corrects the lens by its profile.
    lenses: Mutex<Option<String>>,
    /// The frame open in the editor, whose pictures the worker keeps
    /// from its own develop: the reduced develop never takes it, and
    /// runs one at a time while there is one (`thumbpool`). None while
    /// culling, in the grid, or with no frame open.
    open: Mutex<Option<Key>>,
    /// When the window last asked for a develop or one ended: the
    /// reduced develops wait for the editor to be quiet
    /// (`reduced::begin`).
    touched: Mutex<Instant>,
    /// The worker is making a neighbor's base onto its shelf: the
    /// reduced develops wait for it, so the two decoded frames beside
    /// the open one are never in hand at once.
    prefetching: std::sync::atomic::AtomicBool,
    /// The look and profile directories the keys read: the user's
    /// (`None`), or a test's.
    dirs: Mutex<Option<(PathBuf, PathBuf)>>,
    /// What the keys found so far were made under
    /// (`greycard_edit_io::key::keys_made_under`): a look or a profile
    /// written, refitted, renamed or removed since moves it
    /// ([`Edited::files_moved`]).
    made_under: Mutex<String>,
}

type InHand = Arc<Mutex<std::collections::HashSet<(String, u64, u16)>>>;

/// A picture in hand on the keeping thread, let go when the making that
/// holds it is done or dropped unbegun.
pub struct Marked {
    in_hand: InHand,
    which: (String, u64, u16),
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
pub fn lens_database() -> Option<String> {
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
    pub fn new(cache: ThumbCache) -> Self {
        let this = Self {
            cache,
            wants: Mutex::new(HashMap::new()),
            keeper: Keeper::default(),
            in_hand: Arc::default(),
            lenses: Mutex::new(lens_database()),
            open: Mutex::new(None),
            touched: Mutex::new(Instant::now()),
            prefetching: std::sync::atomic::AtomicBool::new(false),
            dirs: Mutex::new(None),
            made_under: Mutex::new(String::new()),
        };
        *this.made_under.lock().expect("made under") = this.under_now();
        // The index makes its keys with the same name.
        greycard_library::set_lens_database(this.lenses());
        this
    }

    /// The frame open in the editor, or none (culling, the grid, no
    /// frame). Whether that changed.
    pub fn set_open(&self, frame: Option<Key>) -> bool {
        let mut open = self.open.lock().expect("open frame");
        let changed = *open != frame;
        *open = frame;
        changed
    }

    /// Whether `frame` is the one open in the editor: another version
    /// of the open file is not.
    pub fn is_open(&self, frame: &Key) -> bool {
        self.open.lock().expect("open frame").as_ref() == Some(frame)
    }

    /// A develop was asked for, or ended.
    pub fn touch(&self) {
        *self.touched.lock().expect("touched") = Instant::now();
    }

    /// How long since a develop was last asked for or ended.
    pub fn idle_for(&self) -> std::time::Duration {
        self.touched.lock().expect("touched").elapsed()
    }

    /// The worker's prefetch of a neighbor's base begins (`true`) or
    /// ends.
    pub fn set_prefetching(&self, on: bool) {
        self.prefetching
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether the worker is making a neighbor's base.
    pub fn prefetching(&self) -> bool {
        self.prefetching.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether any frame is open in the editor.
    pub fn any_open(&self) -> bool {
        self.open.lock().expect("open frame").is_some()
    }

    /// The ISO the frame is held with, as its row gave it.
    pub fn iso_of(&self, frame: &Key) -> Iso {
        self.wants().get(frame).and_then(|h| h.iso)
    }

    /// The key the frame at `path` wants the reduced develop of its
    /// pictures under: held as edited under a key none of whose pictures
    /// are all kept, not the open frame, with a cache to keep them in;
    /// `None` otherwise. Reads the sidecar for the key when that is not
    /// known yet, so on a thumbnail's thread.
    pub fn wants_develop(&self, frame: &Key) -> Option<u64> {
        if self.is_open(frame) || !self.holds(frame) || !self.cache_on() {
            return None;
        }
        let Shows::Edit(key) = self.shows(frame) else {
            return None;
        };
        let hash = greycard_library::hash_file(&frame.path).ok()?;
        (!self.all_kept(&hash, key)).then_some(key)
    }

    fn wants(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Held>> {
        self.wants.lock().expect("edited frames")
    }

    /// The lens database's name for the keys ([`lens_database`]).
    pub fn lenses(&self) -> Option<String> {
        self.lenses.lock().expect("edited lenses").clone()
    }

    /// The lens database changed (fetched, or updated): every key found
    /// is read again, since a key of an edit that corrects the lens
    /// names the database. Whether it changed.
    pub fn set_lenses(&self, now: Option<String>) -> bool {
        let mut lenses = self.lenses.lock().expect("edited lenses");
        if *lenses == now {
            return false;
        }
        *lenses = now.clone();
        drop(lenses);
        greycard_library::set_lens_database(now);
        *self.made_under.lock().expect("made under") = self.under_now();
        self.read_all_again();
        true
    }

    /// Every frame held under a key found has its key read again from
    /// its sidecar when it is next looked up.
    fn read_all_again(&self) {
        for held in self.wants().values_mut() {
            held.want = Want::Read;
        }
    }

    /// What keys are made under now: the recipe, the lens database, the
    /// look and profile directories as they stand.
    fn under_now(&self) -> String {
        let lenses = self.lenses();
        match &*self.dirs.lock().expect("key dirs") {
            Some((looks, profiles)) => greycard_edit_io::key::keys_made_under_in(
                Some(looks),
                Some(profiles),
                lenses.as_deref(),
            ),
            None => greycard_edit_io::key::keys_made_under(lenses.as_deref()),
        }
    }

    /// A look or a profile may have been written, refitted, renamed or
    /// removed (the camera match's fit, a look removed or renamed, the
    /// LOOK or CAMERA PROFILE section read again): when what the keys
    /// are made under moved, every frame held under a key has it read
    /// again, as a new lens database has (`set_lenses`). Whether it
    /// moved, which is when the cells ask again and the index's keys go.
    /// A listing of the two directories; nothing is read.
    pub fn files_moved(&self) -> bool {
        let now = self.under_now();
        let mut was = self.made_under.lock().expect("made under");
        if *was == now {
            return false;
        }
        *was = now;
        drop(was);
        self.read_all_again();
        true
    }

    /// The frames held as edited.
    pub fn held_frames(&self) -> Vec<Key> {
        self.wants().keys().cloned().collect()
    }

    /// The keys read with the looks and profiles in these directories:
    /// for a test.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_dirs(&self, looks: PathBuf, profiles: PathBuf) {
        *self.dirs.lock().expect("key dirs") = Some((looks, profiles));
        *self.made_under.lock().expect("made under") = self.under_now();
    }

    /// The key of `edit` under the lens database the develop reads now.
    pub fn key_of(&self, edit: &Edit) -> u64 {
        let lenses = self.lenses();
        match &*self.dirs.lock().expect("key dirs") {
            Some((looks, profiles)) => greycard_edit_io::key::develop_key_in(
                edit,
                Some(looks),
                Some(profiles),
                lenses.as_deref(),
            ),
            None => greycard_edit_io::key::develop_key(edit, lenses.as_deref()),
        }
    }

    /// A new list: these frames are held as edited (true) or not, each
    /// with its ISO and the key its row gives, if any. An edited one
    /// without a key from its row has it read from its sidecar when it
    /// is next looked up; the frames of other lists are forgotten.
    pub fn note_all(&self, list: impl IntoIterator<Item = (Key, bool, Iso, Option<u64>)>) {
        let wants: HashMap<Key, Held> = list
            .into_iter()
            .filter(|(_, edited, _, _)| *edited)
            .map(|(frame, _, iso, key)| (frame, Held::from_row(iso, key)))
            .collect();
        *self.wants() = wants;
    }

    /// One frame saved, or found changed on disk: held as edited or not,
    /// its key read again when next looked up. `key` is the key its line
    /// is at now, when the caller knows it (a save computes it from the
    /// edit it wrote); none when the line changed under the window. It
    /// is the frame's hint until its key is read: what the frame may
    /// show, which the guard on another frame's pictures asks, so a
    /// line whose edit moved (a swap, a paste onto it) never keeps the
    /// key it was at before. Whether the frame was or is edited, which
    /// is when its pictures may have changed.
    pub fn note(&self, frame: &Key, edited: bool, iso: Iso, key: Option<u64>) -> bool {
        let mut wants = self.wants();
        let was = if edited {
            wants.insert(
                frame.clone(),
                Held {
                    hint: key,
                    ..Held::read(iso)
                },
            )
        } else {
            wants.remove(frame)
        };
        edited || was.is_some()
    }

    /// What the window holds of one frame changed (its row came in, or
    /// its sidecar was read): held as edited or not from now on, a key
    /// already found kept unless the row gives another than it gave
    /// before, when the sidecar is read again (the row's key is the
    /// guess for when it cannot be). Whether that changed what it
    /// shows, which is when its pictures change.
    pub fn hold(&self, frame: &Key, edited: bool, iso: Iso, key: Option<u64>) -> bool {
        let mut wants = self.wants();
        match (edited, wants.get_mut(frame)) {
            (true, None) => {
                wants.insert(frame.clone(), Held::from_row(iso, key));
                true
            }
            (true, Some(held)) => {
                held.iso = iso;
                // Another key from the row: the sidecar moved under it,
                // and its own key is read again on the next lookup.
                match key {
                    Some(k) if held.row != Some(k) => {
                        held.row = Some(k);
                        held.want = Want::Read;
                        true
                    }
                    _ => false,
                }
            }
            (false, Some(_)) => {
                wants.remove(frame);
                true
            }
            (false, None) => false,
        }
    }

    /// Whether the frame is held as edited.
    pub fn holds(&self, frame: &Key) -> bool {
        self.wants().contains_key(frame)
    }

    /// The key this frame was last shown under, when one was found.
    fn shown_key(&self, frame: &Key) -> Option<u64> {
        match self.wants().get(frame) {
            Some(Held {
                want: Want::Key(k), ..
            }) => Some(*k),
            _ => None,
        }
    }

    /// The frame's pictures are kept under `key`, or its sidecar was
    /// found to make `key`: it shows them from now on, and the pictures
    /// of the key it showed before are taken out of the cache. Only that
    /// key's, a key it was found to show and not its row's guess:
    /// another copy of the same content may show its own. Whether that
    /// changed what it shows.
    pub fn settle_key(&self, frame: &Key, hash: &str, key: u64, iso: Iso) -> bool {
        let was = self.shown_key(frame);
        if let Some(old) = was.filter(|&old| old != key && !self.wanted_elsewhere(frame, old)) {
            self.remove_key(hash, old);
        }
        let row = self.wants().get(frame).and_then(|h| h.row);
        self.wants().insert(
            frame.clone(),
            Held {
                want: Want::Key(key),
                iso,
                row,
                hint: Some(key),
            },
        );
        was != Some(key)
    }

    /// Whether another frame of the same file than `frame` shows `key`,
    /// or may (`Held::may_show`): one still to read its own key whose key
    /// last found was `key`, or that never found one. A version at the
    /// same edit as its file's own (a New version's first state, the two
    /// sides of a swap) is the case: its pictures under that key are not
    /// this frame's to take out until it has found a key of its own. A
    /// frame that last showed another key, and another file at the same
    /// key (its pictures are under its own hash), are no reason to keep
    /// ours.
    fn wanted_elsewhere(&self, frame: &Key, key: u64) -> bool {
        self.wants()
            .iter()
            .any(|(other, held)| other != frame && other.path == frame.path && held.may_show(key))
    }

    /// Every picture of `key` for content `hash` taken out, under both
    /// recipes; how many.
    fn remove_key(&self, hash: &str, key: u64) -> usize {
        match self.cache.lock().expect("thumbnail cache").as_mut() {
            Some(cache) => EDIT_RECIPES
                .iter()
                .map(|&recipe| cache.remove_tag(hash, Tag { recipe, stamp: key }))
                .sum(),
            None => 0,
        }
    }

    /// What `frame` shows, its sidecar read here for its key when that
    /// is not known yet: on a thumbnail's thread or a loupe's, never the
    /// window's.
    pub fn shows(&self, frame: &Key) -> Shows {
        let held = self.wants().get(frame).copied();
        let Some(held) = held else {
            return Shows::Camera;
        };
        match held.want {
            Want::Key(k) => Shows::Edit(k),
            Want::Unread => held.unread(),
            Want::Read => {
                let read = read_key(frame, held.iso, |e| self.key_of(e));
                let mut wants = self.wants();
                // A save or a keep since the read began has the last word.
                if wants.get(frame) != Some(&held) {
                    drop(wants);
                    return self.shows(frame);
                }
                let want = match read {
                    Read::Key(k) => Want::Key(k),
                    Read::Default => {
                        wants.remove(frame);
                        return Shows::Camera;
                    }
                    Read::Unread => Want::Unread,
                };
                let now = Held { want, ..held };
                wants.insert(frame.clone(), now);
                now.unread()
            }
        }
    }

    /// What a frame whose file and sidecar are out of reach shows: the
    /// pictures of the key found when one was, else of the key its row
    /// gave, else the edit's picture kept last for its content, when the
    /// window holds it as edited. Nothing of it is read.
    pub fn shows_offline(&self, frame: &Key) -> Shows {
        match self.wants().get(frame) {
            Some(held) => held.unread(),
            None => Shows::Camera,
        }
    }

    /// The picture of the edit kept for content `hash` at `size`, as
    /// `shows` says, or `None`: the camera's then stands in. The open
    /// frame's develop's picture first, then the reduced develop's. The
    /// JPEG is decoded outside the cache's lock.
    pub fn lookup(&self, hash: &str, size: u32, shows: Shows) -> Option<Thumb> {
        let (tag, bytes) = {
            let cache = self.cache.lock().expect("thumbnail cache");
            let cache = cache.as_ref().filter(|c| c.cap() > 0)?;
            let tags: Vec<Tag> = match shows {
                Shows::Camera => return None,
                Shows::Edit(k) => EDIT_RECIPES
                    .iter()
                    .map(|&recipe| Tag { recipe, stamp: k })
                    .collect(),
                Shows::Newest => vec![cache.newest(hash, &EDIT_RECIPES)?],
            };
            tags.into_iter()
                .find_map(|tag| Some((tag, cache.read(hash, size, tag)?)))?
        };
        let thumb = greycard_library::thumbs::decode(&bytes, tag);
        if let Some(cache) = self.cache.lock().expect("thumbnail cache").as_mut() {
            cache.settle(hash, size, tag, thumb.is_some(), bytes.len() as u64);
        }
        thumb
    }

    /// Whether every picture of the edit is kept for `hash` under `key`,
    /// by either develop: a save that changed nothing of the picture (a
    /// rating) makes none.
    pub fn all_kept(&self, hash: &str, key: u64) -> bool {
        EDIT_RECIPES
            .iter()
            .any(|&recipe| self.all_kept_by(hash, key, recipe))
    }

    /// Whether every picture of the edit is kept for `hash` under `key`
    /// and `recipe`: the open frame's develop asks after its own alone,
    /// so it takes the reduced develop's pictures' place.
    pub fn all_kept_by(&self, hash: &str, key: u64, recipe: u16) -> bool {
        let tag = Tag { recipe, stamp: key };
        let cache = self.cache.lock().expect("thumbnail cache");
        cache.as_ref().is_some_and(|c| {
            c.cap() > 0
                && crate::made::MADE
                    .iter()
                    .chain([BIG].iter())
                    .all(|&size| c.has(hash, size, tag))
        })
    }

    /// The picture of content `hash` under `key` by `recipe` taken in
    /// hand, or `None` when it is in hand already. Each develop marks
    /// its own: a frame opened while the reduced develop of it is under
    /// way has its pictures made from its own develop all the same.
    pub fn mark(&self, hash: &str, key: u64, recipe: u16) -> Option<Marked> {
        let which = (hash.to_string(), key, recipe);
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
    pub fn cache_on(&self) -> bool {
        self.cache
            .lock()
            .expect("thumbnail cache")
            .as_ref()
            .is_some_and(|c| c.cap() > 0)
    }

    /// The frame's edit was reset: it shows the camera's, and the
    /// pictures of the key it showed are taken out (another copy's of
    /// the same content are left). Whether it showed anything else.
    pub fn reset(&self, frame: &Key, hash: Option<&str>) -> bool {
        let old = self.shown_key(frame);
        let held = self.wants().remove(frame).is_some();
        let removed = match (old, hash) {
            (Some(old), Some(hash)) if !self.wanted_elsewhere(frame, old) => {
                self.remove_key(hash, old)
            }
            _ => 0,
        };
        held || removed > 0
    }

    /// Keep `made`, from the open frame's develop, for content `hash`
    /// under `key` as `frame`'s, and take the pictures of the key it
    /// showed before out of the cache, and the reduced develop's of
    /// this key, which these replace. The encoding is done outside the
    /// cache's lock. Answers the bytes kept.
    pub fn store(
        &self,
        frame: &Key,
        hash: &str,
        key: u64,
        iso: Iso,
        made: &Made,
    ) -> std::io::Result<u64> {
        let bytes = self.put_made(
            hash,
            Tag {
                recipe: EDITED_RECIPE,
                stamp: key,
            },
            made,
        )?;
        if let Some(cache) = self.cache.lock().expect("thumbnail cache").as_mut() {
            cache.remove_tag(
                hash,
                Tag {
                    recipe: REDUCED_RECIPE,
                    stamp: key,
                },
            );
        }
        self.settle_key(frame, hash, key, iso);
        Ok(bytes)
    }

    /// Keep `made`, from the reduced develop, for content `hash` under
    /// `key`, unless the open frame's develop kept its own meanwhile:
    /// `None` then. The frame was settled on the key before its develop
    /// began (`reduced`), so nothing of what it shows moves here.
    pub fn store_reduced(&self, hash: &str, key: u64, made: &Made) -> std::io::Result<Option<u64>> {
        if self.all_kept_by(hash, key, EDITED_RECIPE) {
            return Ok(None);
        }
        let tag = Tag {
            recipe: REDUCED_RECIPE,
            stamp: key,
        };
        self.put_made(hash, tag, made).map(Some)
    }

    /// The pictures of `made` encoded and written under `tag`; the bytes
    /// written.
    fn put_made(&self, hash: &str, tag: Tag, made: &Made) -> std::io::Result<u64> {
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
        let mut cache = self.cache.lock().expect("thumbnail cache");
        let Some(cache) = cache.as_mut().filter(|c| c.cap() > 0) else {
            return Ok(0);
        };
        for (size, entry) in &encoded {
            cache.put_bytes(hash, *size, tag, entry)?;
            bytes += entry.len() as u64;
        }
        Ok(bytes)
    }

    /// Run `task` on the keeping thread, in place of one still waiting:
    /// the newest save's pictures are the ones wanted.
    pub fn keep_later(&self, task: Task) {
        self.keeper.put(task);
    }

    /// Whether the keeping thread has nothing running or waiting, so it
    /// writes nothing more to the cache until it is given more. For a
    /// test's stop (`Worker::stop_all`).
    #[cfg(any(test, feature = "testing"))]
    pub fn keeping_idle(&self) -> bool {
        self.keeper.idle()
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
/// XMP beside it included), or why there is none: the line of the
/// frame's version, which read as no version at all (deleted since the
/// frame was listed) is no edit.
fn read_key(frame: &Key, iso: Iso, key_of: impl Fn(&Edit) -> u64) -> Read {
    let path = frame.path.as_path();
    let (sidecar, trouble) = greycard_edit_io::on_disk::read_sidecar_said(path);
    if trouble.is_some() {
        return Read::Unread;
    }
    let Some(line) = sidecar.line(frame.version()) else {
        return Read::Default;
    };
    if !is_edited(path, &line.current, iso) {
        return Read::Default;
    }
    Read::Key(key_of(&line.current))
}

/// Whether `edit` is an edit of the frame at `path`, by the rule the
/// cell's edited mark and the index's `edited` go by
/// (`greycard_library::is_default_edit`), the learned blend judged
/// against `iso`: a frame only opened, or reset to its original, shows
/// the camera's JPEG.
pub fn is_edited(path: &Path, edit: &Edit, iso: Iso) -> bool {
    !greycard_library::is_default_edit(edit, path, iso)
}

/// A job for the keeping thread.
pub type Task = Box<dyn FnOnce() + Send>;

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
    /// A task is running.
    busy: bool,
    /// The keeper was dropped: the thread ends at its next look.
    closed: bool,
}

/// The keeper dropped (with its `Edited`, once the worker and the pool
/// are gone) ends its thread. Nothing can be waiting, since a waiting
/// task holds the `Edited`. Not joined: the thread ends on its own.
impl Drop for Keeper {
    fn drop(&mut self) {
        let (lock, cv) = &*self.inner;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        cv.notify_all();
    }
}

impl Keeper {
    /// Nothing running and nothing waiting.
    #[cfg(any(test, feature = "testing"))]
    fn idle(&self) -> bool {
        let state = self.inner.0.lock().expect("keeper");
        !state.busy && state.waiting.is_none()
    }

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
                                if state.closed {
                                    return;
                                }
                                if let Some(task) = state.waiting.take() {
                                    state.busy = true;
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
                        lock.lock().expect("keeper").busy = false;
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
pub struct Finish {
    pub edit: Edit,
    /// The frame's own quarter turns, the sidecar's.
    pub turn: u8,
    /// The developed picture's size before the geometry, which the
    /// masks and the guide plane are placed by.
    pub source: (u32, u32),
    /// The full develop's size before the geometry: `source` for the
    /// open frame's develop, twice it for the reduced develop's. The
    /// picture is made at the size an export at [`BIG`] of the full
    /// develop has ([`fitted_size`]).
    pub full: (u32, u32),
    pub rasters: HashMap<(u64, usize), Arc<Raster>>,
    pub clip_level: f32,
    pub guide: Option<Arc<greycard_render::finish::Guide>>,
    pub kind: greycard_render::finish::Source,
    pub white: Option<greycard_edit::WhiteShift>,
    /// The masks' bands, of the developed picture `source` is the size
    /// of, when a mask asks for them.
    pub detail: Option<Arc<greycard_render::detail::DetailPlanes>>,
}

/// The pictures of an edit, as they are kept: the larger one and each
/// made size, all turned as the camera's tag turns the frame and no
/// further.
pub struct Made {
    pub big: Thumb,
    pub thumbs: Vec<(u32, Thumb)>,
}

/// The whole factor a developed picture of `source` is reduced by before
/// its geometry, so its frame is still at least [`BIG`] on its long edge
/// and is brought to it by one resize that is less than a halving.
pub fn factor_for(source: (u32, u32), geometry: &Geometry) -> usize {
    let frame = geometry.frame(source.0 as f32, source.1 as f32);
    let long = frame.size.0.max(frame.size.1);
    ((long / BIG as f32).floor() as usize).max(1)
}

/// The viewport's texture read back `factor` times smaller each way, a
/// band of rows at a time so the whole picture is never held at full
/// size on the CPU.
pub fn read_back_reduced(
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
pub fn make(reduced: &WorkingImage, finish: &Finish) -> Made {
    let edit = &finish.edit;
    let framed;
    // Whether the geometry frames anything is asked at the full
    // develop's size, as `fitted_size` asks it, so the two agree.
    let whole = edit
        .geometry
        .is_identity_on(finish.full.0 as f32, finish.full.1 as f32);
    let image = if whole {
        reduced
    } else {
        framed = greycard_render::geometry::apply(reduced, &edit.geometry);
        &framed
    };
    // At the size an export at BIG of the full develop has, to the
    // pixel, so the two agree and so does the grain laid on each, which
    // is placed by the output's own pixels: a crop rounded at another
    // scale would put the picture a pixel off it.
    let (w, h) = fitted_size(finish.full, &edit.geometry);
    let fitted = greycard_render::export::fit_exact(image, w, h);
    let image = fitted.as_ref().unwrap_or(image);
    // The masks' bands by the same steps: reduced by the whole factor
    // the picture was, framed, and fitted.
    let detail = finish.detail.as_ref().map(|planes| {
        let mut bands = if (planes.width, planes.height) == (reduced.width, reduced.height) {
            planes.to_image()
        } else {
            // Averaged straight from the half floats, with no
            // full-size copy of three f32 channels in between.
            let bands = planes.to_image_reduced(factor_for(finish.full, &edit.geometry));
            let (rw, rh) = (reduced.width as u32, reduced.height as u32);
            bands.fit_exact(rw, rh).unwrap_or(bands)
        };
        if !whole {
            bands = bands.framed(&edit.geometry);
        }
        bands.fit_exact(w, h).unwrap_or(bands)
    });
    let (sw, sh) = (finish.source.0 as f32, finish.source.1 as f32);
    let frame_width = edit.geometry.frame(sw, sh).size.0;
    let (fw, fh) = (finish.full.0 as f32, finish.full.1 as f32);
    let full_width = edit.geometry.frame(fw, fh).size.0;
    let settings = greycard_render::export::Settings {
        format: greycard_render::export::Format::Jpeg,
        long_edge: None,
        space: greycard_render::export::Space::Srgb,
        sharpen: greycard_render::export::Sharpen::Off,
        watermark: None,
        ..greycard_render::export::Settings::default()
    };
    let rendered = greycard_render::export::render_framed(
        image,
        frame_width,
        full_width,
        edit,
        &greycard_edit_io::Stores::user(),
        finish.source,
        &settings,
        &finish.rasters,
        finish.clip_level,
        finish.guide.as_deref(),
        finish.kind,
        finish.white.as_ref(),
        detail.as_ref().zip(finish.detail.as_ref().map(|p| &p.meta)),
    );
    let greycard_render::export::Pixels::Eight(rgb) = rendered.pixels else {
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
    let thumbs = crate::made::MADE
        .iter()
        .map(|&size| (size, shrunk(&big, size)))
        .collect();
    Made { big, thumbs }
}

/// The size an export at [`BIG`] of a full develop of `full` under
/// `geometry` has: the geometry's frame as `geometry::apply` cuts it, then
/// brought to [`BIG`] on its long edge as `export::fit` brings it, or left
/// when it is no larger.
pub fn fitted_size(full: (u32, u32), geometry: &Geometry) -> (u32, u32) {
    let (w, h) = framed_size(full, geometry);
    let long = w.max(h);
    if long <= BIG {
        return (w.max(1), h.max(1));
    }
    let scale = BIG as f64 / long as f64;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

pub use crate::stages::framed_size;

/// A finished picture turned back by the frame's shown turns, so the
/// strip's and the loupe's own turn at draw time puts it back: a turn is
/// undone by the turn the other way, and a mirror with a turn is its own
/// undoing.
pub fn unturned(thumb: Thumb, (turns, flip): (u8, bool)) -> Thumb {
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
/// when it is no larger. How every made size is made from the 2048.
pub fn shrunk(picture: &Thumb, size: u32) -> Thumb {
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
pub enum Developed {
    /// The develop's own picture on the CPU, at full size.
    Full(Arc<WorkingImage>),
    /// Already reduced, read back from the viewport's texture.
    Reduced(WorkingImage),
}

/// One keeping: the frame, its content hash and key, the picture and
/// what finishes it.
pub struct Keeping {
    pub frame: Key,
    pub hash: String,
    pub key: u64,
    pub developed: Developed,
    /// The whole factor the developed picture is reduced by: here for a
    /// full-size one, already for one read back reduced.
    pub factor: usize,
    /// How long the read back took, when there was one.
    pub read_back: Option<f64>,
    /// The frame's ISO from its row, as the frame is held.
    pub iso: Iso,
    pub finish: Finish,
    /// The picture's place in hand, let go with the keeping.
    pub marked: Marked,
}

/// Make and keep the edit's pictures of one frame, then say so to
/// `done` with the frame: on the keeping thread.
pub fn keep(edited: &Edited, job: Keeping, done: impl FnOnce(Key)) {
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
    let stored = edited.store(&job.frame, &job.hash, job.key, job.iso, &made);
    // Kept, or not: a save of the same picture from now on looks for it
    // in the cache rather than waiting on this one.
    drop(job.marked);
    let total = started.elapsed().as_secs_f64();
    let name = job.frame.said();
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
            done(job.frame);
        }
        Err(e) => tracing::warn!("the edit's pictures of {name}: not kept: {e}"),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::testing::edited::{Heard, rig};

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

    /// The reduced develop's pictures read the color mean over the
    /// span the full develop's would: a stripe of teal six full-size
    /// pixels wide, three in the reduced picture, under a color window
    /// that takes the stripe and not four fifths of its chroma. Told
    /// the full develop is twice the picture, the window takes the
    /// stripe's middle and brightens it; told it is the picture, the
    /// 5x5 reads grey in and leaves it.
    #[test]
    fn the_kept_pictures_color_mean_spans_the_full_develops_pixels() {
        use greycard_edit::mask::{Component, Mask, Mode, Shape};
        let (w, h) = (BIG as usize, 16usize);
        let ok = greycard_core::color::Oklab::for_working_space();
        let from_lms = greycard_core::color::invert3(ok.to_lms).unwrap();
        let of_lab = |lab: [f32; 3]| {
            let lms = greycard_core::color::apply3(&greycard_core::color::LAB_TO_LMS, lab)
                .map(|v| v * v * v);
            greycard_core::color::apply3(&from_lms, lms)
        };
        let (grey, teal) = (of_lab([0.5, 0.0, 0.0]), of_lab([0.5, -0.07, -0.07]));
        let mut data = Vec::with_capacity(w * h * 3);
        for _ in 0..h {
            for x in 0..w {
                data.extend(if (1000..1003).contains(&x) {
                    teal
                } else {
                    grey
                });
            }
        }
        let reduced = WorkingImage {
            width: w,
            height: h,
            data,
        };
        let mut edit = Edit::default();
        let mut adjustment = greycard_edit::Adjustment {
            mask: Mask {
                components: vec![Component {
                    shape: Shape::Color {
                        hue: 225.0,
                        width: 30.0,
                        hue_feather: 0.0,
                        chroma: 0.9 * 0.07 * 2f32.sqrt(),
                        chroma_feather: 0.0,
                    },
                    mode: Mode::Add,
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        adjustment.look.light.exposure = 2.0;
        edit.adjustments.push(adjustment);
        let middle = |full: (u32, u32)| {
            let finish = Finish {
                edit: edit.clone(),
                turn: 0,
                source: (w as u32, h as u32),
                full,
                rasters: Default::default(),
                clip_level: 1.0,
                guide: None,
                kind: greycard_render::finish::Source::SCENE,
                white: None,
                detail: None,
            };
            let big = make(&reduced, &finish).big;
            assert_eq!((big.width, big.height), (w as u32, h as u32));
            let i = ((h / 2) * w + 1001) * 3;
            big.rgb[i..i + 3].iter().map(|v| u32::from(*v)).sum::<u32>()
        };
        let (at_full, at_own) = (
            middle((2 * w as u32, 2 * h as u32)),
            middle((w as u32, h as u32)),
        );
        assert!(at_full > at_own + 60, "{at_full} against {at_own}");
    }

    /// A frame edited without being opened: its sidecar saved on disk
    /// with a brightening, the window holding it as edited, its
    /// thumbnail asked for. The camera's comes first, then the pool's
    /// reduced develop makes the edit's pictures behind it and says so,
    /// and the next ask is the edit's, brighter, at the made size and
    /// at 2048; asked again, nothing is made again. The open frame is
    /// never the pool's.
    #[test]
    fn a_frame_edited_without_being_opened_is_developed_reduced_by_the_pool() {
        let rig = rig("reduced");
        let other = rig.dir.join("other.png");
        let picture = image::RgbImage::from_fn(600, 400, |x, y| {
            image::Rgb([(30 + x / 12) as u8, (40 + y / 10) as u8, 50])
        });
        picture.save(&other).unwrap();
        let mut edit = Edit::for_picture();
        edit.light.exposure = 1.5;
        let mut sidecar = greycard_edit::Sidecar {
            line: greycard_edit::Line::of(edit.clone()),
            ..greycard_edit::Sidecar::default()
        };
        sidecar.save(&other).unwrap();
        let edited = rig.worker.edited();
        let other_frame = Key::file(other.clone());
        edited.note_all([
            (Key::file(rig.png.clone()), false, None, None),
            (other_frame.clone(), true, None, None),
        ]);
        assert!(
            edited.wants_develop(&Key::file(&rig.png)).is_none(),
            "the open frame is the worker's"
        );
        assert!(edited.wants_develop(&other_frame).is_some());
        // Its cell on screen: the pool makes the edit's pictures of the
        // frames near the rows shown.
        rig.worker.near_thumbnails([1].into());
        let ask = || {
            rig.worker.send(crate::worker::Job::Thumbnail {
                index: 1,
                frame: other_frame.clone(),
            });
        };
        ask();
        let camera = loop {
            if let Heard::Thumb { mean, .. } = rig.next() {
                break mean;
            }
        };
        // The pool develops it behind the thumbnail and says so.
        loop {
            if let Heard::Kept(path) = rig.next() {
                assert_eq!(path, other);
                break;
            }
        }
        let hash = greycard_library::hash_file(&other).unwrap();
        let key = edited.key_of(&edit);
        assert!(edited.all_kept(&hash, key), "every size kept under the key");
        assert!(
            edited.wants_develop(&other_frame).is_none(),
            "nothing more wanted"
        );
        ask();
        let brightened = loop {
            if let Heard::Thumb { mean, .. } = rig.next() {
                break mean;
            }
        };
        assert!(
            brightened > camera + 20.0,
            "the edit's picture is the brighter: {camera} then {brightened}"
        );
        let big = edited
            .lookup(&hash, BIG, Shows::Edit(key))
            .expect("the 2048 picture");
        assert_eq!(big.width.max(big.height), 600, "no larger than the frame");
        // Nothing else is said: no second making.
        assert!(
            rig.heard
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "quiet"
        );
        rig.quiet();
    }

    /// The reduced develop's pictures of a key stand for the frame until
    /// it is opened: its own develop's are then made in their place,
    /// under their own recipe, and the reduced develop's go. One that
    /// lands after them keeps nothing.
    #[test]
    fn the_open_frame_s_pictures_take_the_reduced_develop_s_place() {
        let rig = rig("replace");
        let edited = rig.worker.edited();
        let mut bright = Edit::for_picture();
        bright.light.exposure = 1.0;
        let hash = greycard_library::hash_file(&rig.png).unwrap();
        let key = edited.key_of(&bright);
        // A flat dark picture standing for the reduced develop's.
        let big = Thumb {
            width: 600,
            height: 400,
            rgb: vec![12; 600 * 400 * 3],
        };
        let made = Made {
            thumbs: crate::made::MADE
                .iter()
                .map(|&size| (size, shrunk(&big, size)))
                .collect(),
            big,
        };
        assert!(edited.store_reduced(&hash, key, &made).unwrap().is_some());
        edited.settle_key(&Key::file(&rig.png), &hash, key, None);
        assert!(edited.all_kept(&hash, key));
        assert!(!edited.all_kept_by(&hash, key, EDITED_RECIPE));
        let (_, _, reduced) = rig.thumb();
        assert!(
            reduced < 20.0,
            "the reduced develop's is shown: {reduced:.1}"
        );
        rig.save(&bright, 0);
        rig.kept();
        assert!(edited.all_kept_by(&hash, key, EDITED_RECIPE));
        assert!(
            !crate::made::MADE.iter().chain([BIG].iter()).any(|&size| {
                edited.cache.lock().unwrap().as_ref().unwrap().has(
                    &hash,
                    size,
                    Tag {
                        recipe: REDUCED_RECIPE,
                        stamp: key,
                    },
                )
            }),
            "the reduced develop's are gone"
        );
        let (_, _, own) = rig.thumb();
        assert!(own > reduced + 20.0, "the develop's own: {own:.1}");
        assert!(
            edited.store_reduced(&hash, key, &made).unwrap().is_none(),
            "a reduced develop landing after keeps nothing"
        );
        rig.quiet();
    }

    /// A picture in hand is not taken in hand again until its making is
    /// done or dropped; another key, another frame's content, or the
    /// other develop's making of it is.
    #[test]
    fn a_picture_in_hand_is_made_once() {
        let edited = Edited::new(Arc::new(Mutex::new(None)));
        let first = edited.mark("ab", 7, EDITED_RECIPE).expect("free");
        assert!(edited.mark("ab", 7, EDITED_RECIPE).is_none(), "in hand");
        let other = edited.mark("ab", 8, EDITED_RECIPE).expect("another key");
        let elsewhere = edited
            .mark("cd", 7, EDITED_RECIPE)
            .expect("another content");
        let reduced = edited
            .mark("ab", 7, REDUCED_RECIPE)
            .expect("the reduced develop's own");
        assert!(edited.mark("ab", 7, REDUCED_RECIPE).is_none());
        drop(first);
        assert!(edited.mark("ab", 7, EDITED_RECIPE).is_some(), "let go");
        drop((other, elsewhere, reduced));
    }

    /// A frame held as edited from its row or its sidecar, and let go,
    /// says when that turned; a key kept and found again is no change.
    #[test]
    fn holding_a_frame_says_when_it_turned() {
        let edited = Edited::new(Arc::new(Mutex::new(None)));
        let path = &Key::file("/x/IMG_0001.CR3");
        assert!(!edited.holds(path));
        assert!(edited.hold(path, true, None, None));
        assert!(!edited.hold(path, true, None, None));
        assert!(edited.settle_key(path, "ab", 5, None));
        assert!(!edited.settle_key(path, "ab", 5, None));
        assert!(!edited.hold(path, true, None, None), "a key found stays");
        assert_eq!(edited.shows(path), Shows::Edit(5));
        // A key known serves a frame out of reach too; one not known
        // falls to the picture kept last for the content.
        assert_eq!(edited.shows_offline(path), Shows::Edit(5));
        // The row's key moves the frame to it, and the same key again
        // is no change.
        assert!(edited.hold(path, true, None, Some(9)));
        assert!(!edited.hold(path, true, None, Some(9)));
        assert_eq!(edited.shows(path), Shows::Edit(9));
        assert_eq!(edited.shows_offline(path), Shows::Edit(9));
        assert!(edited.hold(path, false, None, None));
        assert_eq!(edited.shows_offline(path), Shows::Camera);
        assert!(edited.note(path, true, None, None));
        assert_eq!(
            edited.shows_offline(path),
            Shows::Newest,
            "a key to be read"
        );
        assert!(edited.hold(path, false, None, None));
        assert_eq!(edited.shows(path), Shows::Camera);
        assert!(!edited.note(path, false, None, None), "neither was nor is");
        assert!(edited.note(path, true, None, None));
        edited.settle_key(path, "ab", 6, None);
        // The lens database moved on: the key is read again.
        assert!(edited.set_lenses(Some("another".into())));
        assert!(!edited.set_lenses(Some("another".into())));
        assert_eq!(
            edited.shows(path),
            Shows::Newest,
            "read again, from a sidecar that is not there"
        );
        edited.note_all([(Key::file("/x/IMG_0002.CR3"), true, None, None)]);
        assert!(!edited.holds(path), "another list's frames go");
        // A version of a file is a frame of its own beside the file's.
        let warm = Key::of("/x/IMG_0002.CR3", Some("abcd"));
        assert!(!edited.holds(&warm));
        assert!(edited.note(&warm, true, None, None));
        assert!(edited.holds(&Key::file("/x/IMG_0002.CR3")));
        assert!(edited.settle_key(&warm, "cd", 11, None));
        assert_eq!(edited.shows_offline(&warm), Shows::Edit(11));
        assert_eq!(
            edited.shows_offline(&Key::file("/x/IMG_0002.CR3")),
            Shows::Newest,
            "the file's own is held apart"
        );
    }

    #[test]
    fn the_made_sizes_are_shrunk_from_the_big_one() {
        let big = picture(300, 200);
        assert_eq!(shrunk(&big, 128).width, 128);
        assert_eq!(shrunk(&big, 128).height, 85);
        assert_eq!(shrunk(&big, 360), big);
    }

    /// Two frames of one content showing the same key (a New version's
    /// first state beside its file's own, the two sides of a swap): one
    /// moving to another key leaves the pictures kept under the old key,
    /// which the other still shows; once no frame wants it, they go.
    #[test]
    fn a_key_another_frame_shows_keeps_its_pictures() {
        let dir = crate::testing::scratch_dir("edited-shared-key");
        std::fs::create_dir_all(&dir).unwrap();
        let cache: ThumbCache = Arc::new(Mutex::new(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = Edited::new(cache.clone());
        let hash = "ab".repeat(32);
        let kept = Thumb {
            width: 2,
            height: 1,
            rgb: vec![0; 6],
        };
        let tag = |key: u64| Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        let has = |key: u64| {
            cache
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .has(&hash, 128, tag(key))
        };
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put(&hash, 128, tag(5), &kept)
            .unwrap();
        let own = Key::file("/x/IMG_0001.CR3");
        let warm = Key::of("/x/IMG_0001.CR3", Some("abcd"));
        edited.note_all([
            (own.clone(), true, None, None),
            (warm.clone(), true, None, None),
        ]);
        edited.settle_key(&own, &hash, 5, None);
        edited.settle_key(&warm, &hash, 5, None);
        // Warm moves on: the pictures under 5 are version 0's still.
        assert!(edited.settle_key(&warm, &hash, 9, None));
        assert!(has(5), "version 0 still shows the key");
        // Version 0 moves too: nobody shows 5 any more, and it goes.
        assert!(edited.settle_key(&own, &hash, 7, None));
        assert!(!has(5));
        // A reset of a frame whose key another frame shows leaves the
        // pictures as well.
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put(&hash, 128, tag(9), &kept)
            .unwrap();
        edited.settle_key(&own, &hash, 9, None);
        assert!(edited.reset(&warm, Some(&hash)));
        assert!(has(9), "the file's own shows 9");
        assert!(edited.reset(&own, Some(&hash)));
        assert!(!has(9));
        crate::testing::remove_dir_retry(&dir);
    }

    /// A save says `note` for every cell of the file but the open one,
    /// which leaves that line's key to be read again: the open cell's
    /// move to a new key leaves the pictures the noted line may still
    /// show (a New version's first edit, a swap on screen).
    #[test]
    fn a_noted_line_keeps_its_pictures_when_the_open_one_moves() {
        let dir = crate::testing::scratch_dir("edited-probe-noted");
        std::fs::create_dir_all(&dir).unwrap();
        let cache: ThumbCache = Arc::new(Mutex::new(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = Edited::new(cache.clone());
        let hash = "cd".repeat(32);
        let kept = Thumb {
            width: 2,
            height: 1,
            rgb: vec![0; 6],
        };
        let tag = |key: u64| Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put(&hash, 128, tag(5), &kept)
            .unwrap();
        let own = Key::file("/x/IMG_0001.CR3");
        let warm = Key::of("/x/IMG_0001.CR3", Some("abcd"));
        edited.note_all([
            (own.clone(), true, None, None),
            (warm.clone(), true, None, None),
        ]);
        // A New version: both lines at 5, both shown.
        edited.settle_key(&own, &hash, 5, None);
        edited.settle_key(&warm, &hash, 5, None);
        // Warm is open and edited; the save says `saved` for each cell:
        // the file's own (not open) is noted at its key, Warm's Keep
        // settles at 9.
        edited.note(&own, true, None, Some(5));
        edited.settle_key(&warm, &hash, 9, None);
        let has = cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .has(&hash, 128, tag(5));
        crate::testing::remove_dir_retry(&dir);
        assert!(has, "the file's own line is still at 5: its pictures go");
    }

    /// The file's own line sits at 5, found; a version open is edited
    /// from 9 to 11. The save notes the file's own line, whose key last
    /// found was 5: it may show 5 still, and it never showed 9, so 9 goes
    /// with the version's move and 5 stays.
    #[test]
    fn an_edited_version_s_old_key_is_reclaimed() {
        let dir = crate::testing::scratch_dir("edited-reclaimed-key");
        std::fs::create_dir_all(&dir).unwrap();
        let cache: ThumbCache = Arc::new(Mutex::new(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = Edited::new(cache.clone());
        let hash = "ef".repeat(32);
        let kept = Thumb {
            width: 2,
            height: 1,
            rgb: vec![0; 6],
        };
        let tag = |key: u64| Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        for k in [5, 9] {
            cache
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .put(&hash, 128, tag(k), &kept)
                .unwrap();
        }
        let own = Key::file("/x/IMG_0001.CR3");
        let warm = Key::of("/x/IMG_0001.CR3", Some("abcd"));
        edited.note_all([
            (own.clone(), true, None, None),
            (warm.clone(), true, None, None),
        ]);
        edited.settle_key(&own, &hash, 5, None);
        edited.settle_key(&warm, &hash, 9, None);
        edited.note(&own, true, None, Some(5));
        edited.settle_key(&warm, &hash, 11, None);
        let has9 = cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .has(&hash, 128, tag(9));
        let has5 = cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .has(&hash, 128, tag(5));
        crate::testing::remove_dir_retry(&dir);
        assert!(has5, "the file's own pictures stay");
        assert!(!has9, "9 was only Warm's, and Warm moved on: kept anyway");
    }

    /// A swap on screen. The two lines trade edits, so the file's own
    /// goes from 5 to 9 and Warm from 9 to 5. The save notes Warm at
    /// the key its line is at now, 5, and the open own cell's Keep
    /// settles it at 9: Warm shows 5 now, so its pictures under 5 stay.
    /// The same when the open line ends at the defaults and is reset.
    #[test]
    fn a_swap_keeps_the_pictures_the_other_line_now_shows() {
        let dir = crate::testing::scratch_dir("edited-swap-hint");
        std::fs::create_dir_all(&dir).unwrap();
        let cache: ThumbCache = Arc::new(Mutex::new(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = Edited::new(cache.clone());
        let hash = "a1".repeat(32);
        let kept = Thumb {
            width: 2,
            height: 1,
            rgb: vec![0; 6],
        };
        let tag = |key: u64| Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        for k in [5, 9] {
            cache
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .put(&hash, 128, tag(k), &kept)
                .unwrap();
        }
        let own = Key::file("/x/IMG_0001.CR3");
        let warm = Key::of("/x/IMG_0001.CR3", Some("abcd"));
        edited.note_all([
            (own.clone(), true, None, None),
            (warm.clone(), true, None, None),
        ]);
        edited.settle_key(&own, &hash, 5, None);
        edited.settle_key(&warm, &hash, 9, None);
        // The swap's save: `saved(own)` sends the Keep, `saved(warm)`
        // notes it at the key the swap put its line at.
        edited.note(&warm, true, None, Some(5));
        edited.settle_key(&own, &hash, 9, None);
        let has5 = cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .has(&hash, 128, tag(5));
        // And a reset of the open line (swapped with a version at the
        // defaults) asks the same guard.
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put(&hash, 128, tag(5), &kept)
            .unwrap();
        edited.settle_key(&own, &hash, 5, None);
        edited.settle_key(&warm, &hash, 9, None);
        edited.note(&warm, true, None, Some(5));
        let _ = edited.reset(&own, Some(&hash));
        let has5_reset = cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .has(&hash, 128, tag(5));
        crate::testing::remove_dir_retry(&dir);
        assert!(
            has5 && has5_reset,
            "Warm now shows 5: kept after the Keep {has5}, after a reset {has5_reset}"
        );
    }

    /// Another file at the same develop key (two frames synced to one
    /// edit) keeps its own pictures under its own hash and is no reason
    /// to keep this file's old ones; nor is a frame of this file whose
    /// row guessed the key once it has found its real one.
    #[test]
    fn another_file_at_the_same_key_keeps_only_its_own_pictures() {
        let dir = crate::testing::scratch_dir("edited-cross-file");
        std::fs::create_dir_all(&dir).unwrap();
        let cache: ThumbCache = Arc::new(Mutex::new(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = Edited::new(cache.clone());
        let (ha, hb) = ("aa".repeat(32), "bb".repeat(32));
        let kept = Thumb {
            width: 2,
            height: 1,
            rgb: vec![0; 6],
        };
        let tag = |key: u64| Tag {
            recipe: EDITED_RECIPE,
            stamp: key,
        };
        let has = |hash: &str, key: u64| {
            cache
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .has(hash, 128, tag(key))
        };
        for h in [&ha, &hb] {
            cache
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .put(h, 128, tag(5), &kept)
                .unwrap();
        }
        let a = Key::file("/x/A.CR3");
        let b = Key::file("/x/B.CR3");
        edited.note_all([(a.clone(), true, None, None), (b.clone(), true, None, None)]);
        edited.settle_key(&a, &ha, 5, None);
        edited.settle_key(&b, &hb, 5, None);
        // A moves on: its pictures under 5 go, B's stay under B's hash.
        assert!(edited.settle_key(&a, &ha, 7, None));
        assert!(!has(&ha, 5), "A's old pictures go");
        assert!(has(&hb, 5), "B's are B's");
        // A version of A whose row guessed 7 keeps A's pictures under 7
        // only while it has not read its own key.
        let warm = Key::of("/x/A.CR3", Some("abcd"));
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put(&ha, 128, tag(7), &kept)
            .unwrap();
        edited.hold(&warm, true, None, Some(7));
        assert!(edited.settle_key(&a, &ha, 8, None));
        assert!(has(&ha, 7), "the version may still be at 7, by its row");
        edited.settle_key(&warm, &ha, 9, None);
        edited.settle_key(&a, &ha, 7, None);
        assert!(edited.settle_key(&a, &ha, 8, None));
        assert!(!has(&ha, 7), "the version found its own key: 7 is nobody's");
        crate::testing::remove_dir_retry(&dir);
    }
}
