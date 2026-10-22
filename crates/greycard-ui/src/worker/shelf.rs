//! The bases of the frames left behind, kept for a return to them.
//!
//! A base is the develop before its detail and sharpen: the decode's
//! mosaic prepared, demosaiced, and finished, and the lens applied. It
//! is most of an open's develop (two seconds and more at 45 MP), and
//! the same every time a frame is opened under the same edit at the
//! same turn. So when another frame is opened the base in hand is
//! shelved rather than dropped, and an open of a frame whose base is
//! on the shelf takes it back: its develop is then the detail, the
//! sharpen, and the picture's upload.
//!
//! A base on the shelf is keyed by its file ([`FileKey`]: its path,
//! what the file system says of it when it was opened, and what its
//! decode said of the frame, so a file written since is another file)
//! and by what it was made under: the edit's base
//! ([`Edit::same_base`]) and the frame's quarter turns. The base keeps
//! the retouch's patched picture when it has one, so the fills are not
//! made again, and the learned denoiser's pair when the base was
//! blended from it, so a move of the strength after the return is a
//! blend again and not a run of the network. It lets go of everything
//! on the GPU (the picture before the sharpen, the patched picture's
//! upload, and the dehaze's reduced copy), which the device would have
//! to hold for frames not on screen, and of the masks' detail bands,
//! which are made again beside the develop and would cost the shelf as
//! much as the base.
//!
//! What is held is held under a budget of bytes ([`Budget::user`]),
//! the least recently left going first. The base of the open frame is
//! not on the shelf and is never counted against it: a base is taken
//! off before the shelf is brought back under its budget.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use greycard_core::raw::Orientation;

use super::*;

/// The most the shelf holds, however much memory the machine has: at
/// 45 MP seven bases, fewer with a retouch or a learned pair.
const MOST: u64 = 4 << 30;

/// The memory left out of the shelf's share for the editor itself, its
/// models, the open frame, and the system.
const RESERVE: u64 = 4 << 30;

/// How much the shelf may hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Budget {
    /// So many bytes of pictures.
    Bytes(u64),
    /// The machine's memory is not known: one base, whatever its size,
    /// which is about what holding the open one and the one before it
    /// cost when each open made a new base while the last was still in
    /// hand.
    One,
}

impl Budget {
    /// The budget on this machine ([`Budget::from_setting`]), with
    /// `GREYCARD_SHELF_MB` read from the environment here, the one
    /// place it is read: once a session, when the worker's first open
    /// makes the shelf.
    pub(super) fn user() -> Self {
        let setting = std::env::var("GREYCARD_SHELF_MB").ok();
        Self::from_setting(setting.as_deref(), crate::ram::physical())
    }

    /// The budget for a machine of `ram` bytes, unless `setting` (the
    /// value of `GREYCARD_SHELF_MB`, for measuring) is a whole number
    /// of megabytes, 0 turning the shelf off. A setting that is not a
    /// number is ignored. With no setting and the memory unknown, one
    /// base.
    pub(super) fn from_setting(setting: Option<&str>, ram: Option<u64>) -> Self {
        if let Some(mb) = setting.and_then(|v| v.trim().parse::<u64>().ok()) {
            return Self::Bytes(mb.saturating_mul(1 << 20));
        }
        ram.map_or(Self::One, Self::of)
    }

    /// The budget for a machine of `ram` bytes: a quarter of the
    /// memory past the first 4 GB, at most 4 GB. An 8 GB machine keeps
    /// a 45 MP base (540 MB), a 16 GB one five, and 32 GB and more
    /// seven.
    pub(super) fn of(ram: u64) -> Self {
        Self::Bytes((ram.saturating_sub(RESERVE) / 4).min(MOST))
    }

    fn holds(self, count: usize, bytes: u64) -> bool {
        match self {
            Self::Bytes(most) => bytes <= most,
            Self::One => count <= 1,
        }
    }
}

/// What the file system says of a file: its length and modification
/// time and, on Unix, its change time and inode, which a rewrite in
/// place that keeps the first two (a tool that restores the time it
/// found) still moves.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Stat {
    pub len: u64,
    /// Nanoseconds since the epoch.
    pub modified: u64,
    /// The change time, seconds and nanoseconds; `None` where there is
    /// none to read.
    pub changed: Option<(i64, i64)>,
    pub inode: Option<u64>,
}

impl Stat {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        #[cfg(unix)]
        let (changed, inode) = {
            use std::os::unix::fs::MetadataExt;
            (Some((meta.ctime(), meta.ctime_nsec())), Some(meta.ino()))
        };
        #[cfg(not(unix))]
        let (changed, inode) = (None, None);
        Some(Self {
            len: meta.len(),
            modified,
            changed,
            inode,
        })
    }
}

/// What a raw's decode said of its frame: the body, the sensor's size,
/// and the orientation its tag gives. An edit of the file's tags in
/// place that keeps its length and its times still changes these.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FrameId {
    pub make: String,
    pub model: String,
    pub width: usize,
    pub height: usize,
    pub orientation: Orientation,
}

/// A file as a base on the shelf knows it: where it is, what the file
/// system said of it before it was read ([`Stat`]), and what its
/// decode said of the frame ([`FrameId`]).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FileKey {
    pub path: PathBuf,
    pub stat: Stat,
    pub frame: Option<FrameId>,
}

impl FileKey {
    /// The file at `path` as it is on the disk now, before its decode;
    /// `None` when it cannot be read, and then nothing of it is shelved
    /// or taken.
    pub(super) fn of(path: &Path) -> Option<Self> {
        Some(Self {
            path: path.to_path_buf(),
            stat: Stat::of(path)?,
            frame: None,
        })
    }

    /// This key with what the decode of the file, `input`, said of its
    /// frame. A picture that is not a raw says nothing here, and its
    /// base is not shelved ([`Shelf::put`]).
    pub(super) fn decoded(self, input: &Input) -> Self {
        let frame = match input {
            Input::Raw(f) => Some(FrameId {
                make: f.make.clone(),
                model: f.model.clone(),
                width: f.width,
                height: f.height,
                orientation: f.orientation,
            }),
            Input::Picture { .. } => None,
        };
        Self { frame, ..self }
    }
}

/// A base left behind, and the learned pair it was blended from.
pub(super) struct Shelved {
    file: FileKey,
    base: Base,
    learned: Option<LearnedBase>,
    bytes: u64,
}

/// What a file holds now, as the window has it after its edit changed
/// without an open of it: every edit its sidecar holds, its versions'
/// among them, and its quarter turns.
#[derive(Clone, Debug)]
pub(crate) struct FileNow {
    pub edits: Vec<Edit>,
    pub turn: u8,
}

impl FileNow {
    /// Whether a base made under `edit` at `turn` is still one an open
    /// of the file would take: the same turn, and one of its edits with
    /// the same base. A base made under no learned tier does not depend
    /// on the learned strength, which a frame never developed seeds at
    /// its open and not in its sidecar, so a strength alone keeps it
    /// there; a base made under a tier is kept only at the strength the
    /// sidecar says, so one shelved from a seeded frame never saved goes
    /// on the next save of the file (a base made again, never a wrong
    /// one).
    pub(crate) fn serves(&self, edit: &Edit, turn: u8) -> bool {
        self.turn % 4 == turn % 4
            && self.edits.iter().any(|now| {
                let mut as_made = now.clone();
                if now.noise.tier().is_none() && edit.noise.tier().is_none() {
                    as_made.noise.learned_strength = edit.noise.learned_strength;
                }
                edit.same_base(&as_made)
            })
    }
}

/// The bases left behind, the most recently left first.
pub(super) struct Shelf {
    budget: Budget,
    held: VecDeque<Shelved>,
}

impl Shelf {
    pub(super) fn new(budget: Budget) -> Self {
        Self {
            budget,
            held: VecDeque::new(),
        }
    }

    /// The shelf for this machine's budget ([`Budget::user`]), said in
    /// the log as it is made.
    pub(super) fn user() -> Self {
        let shelf = Self::new(Budget::user());
        tracing::info!("the shelf of frames left: {}", shelf.said());
        shelf
    }

    /// The bases on the shelf.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.held.len()
    }

    /// The bytes of pictures the shelf holds.
    pub(super) fn bytes(&self) -> u64 {
        self.held.iter().map(|s| s.bytes).sum()
    }

    /// Whether `base` is worth a place: a raw's (a picture's base is
    /// its decode, which an open makes again whatever is kept), and
    /// not one standing in for a learned denoiser that was missing or
    /// failed, whose model may be there by the time the frame is
    /// opened again.
    fn takes(base: &Base) -> bool {
        base.source != crate::finish::Source::Display && base.stand_in.is_none()
    }

    /// Put `base`, the frame `file`'s, on the shelf, with `learned`
    /// when the base was blended from it; the shelf is not brought
    /// under its budget until [`Shelf::fit`]. Whatever it holds on the
    /// GPU, and the masks' bands, are let go here; the caller lets the
    /// ops' working textures go. A base over the whole budget on its
    /// own is not put at all, so it cannot push out the bases that fit.
    /// A base already there for the same file, edit, and turn is the
    /// older of two and goes, as does one of the same path made from
    /// what the file was before. Whether the base was put.
    pub(super) fn put(
        &mut self,
        file: FileKey,
        mut base: Base,
        learned: Option<LearnedBase>,
    ) -> bool {
        if !Self::takes(&base) {
            return false;
        }
        let learned = learned.filter(|l| {
            base.edit.noise.tier().is_some() && pair_serves(l, base.turn, &base.edit, true)
        });
        let bytes = bytes_of(&base, learned.as_ref());
        if !self.budget.holds(1, bytes) {
            return false;
        }
        base.pre = None;
        base.uploaded = None;
        base.haze = None;
        base.planes = None;
        base.from_shelf = false;
        self.held.retain(|s| {
            s.file.path != file.path
                || (s.file == file
                    && !(s.base.turn == base.turn && s.base.edit.same_base(&base.edit)))
        });
        self.held.push_front(Shelved {
            file,
            base,
            learned,
            bytes,
        });
        true
    }

    /// The base of `file` made under `edit`'s base at `turn`, taken off
    /// the shelf, and the learned pair kept with it. With `has_gpu`
    /// false (no device), only a base whose CA correction was not the
    /// GPU's, as `develop_job` keeps one. A base of the same path from
    /// before the file changed is dropped on the way.
    pub(super) fn take(
        &mut self,
        file: &FileKey,
        edit: &Edit,
        turn: u8,
        has_gpu: bool,
    ) -> Option<(Base, Option<LearnedBase>)> {
        self.held
            .retain(|s| s.file.path != file.path || s.file == *file);
        let at = self.held.iter().position(|s| {
            s.file == *file
                && s.base.turn == turn
                && s.base.edit.same_base(edit)
                && (has_gpu || !s.base.on_gpu)
        })?;
        let Shelved {
            mut base, learned, ..
        } = self.held.remove(at)?;
        base.from_shelf = true;
        Some((base, learned))
    }

    /// Whether a base of the file at `path`, as the file system now
    /// says it is (`stat`), made under `edit`'s base at `turn`, is on
    /// the shelf for an open to take: [`Shelf::take`]'s test without
    /// the decode's word on the frame, which a prefetch asks before it
    /// decodes. With `seeded`, the edit's learned blend is still to be
    /// seeded from the frame's ISO, so the strength of the base found is
    /// taken for the edit's: the edits are otherwise the same, and a
    /// base made without a learned tier does not depend on it.
    pub(super) fn holds(
        &self,
        path: &Path,
        stat: &Stat,
        edit: &Edit,
        seeded: bool,
        turn: u8,
        has_gpu: bool,
    ) -> bool {
        self.held.iter().any(|s| {
            let edit_as_made = seeded.then(|| {
                let mut e = edit.clone();
                e.noise.learned_strength = s.base.edit.noise.learned_strength;
                e
            });
            s.file.path == path
                && s.file.stat == *stat
                && s.base.turn == turn
                && s.base.edit.same_base(edit_as_made.as_ref().unwrap_or(edit))
                && (has_gpu || !s.base.on_gpu)
        })
    }

    /// The bases of the file at `path` that no open will take any more,
    /// let go: its edit changed under them without an open of it (a
    /// sync, a paste, a sidecar written by another hand), and what it
    /// holds `now` serves none of them ([`FileNow::serves`]). With
    /// nothing at hand, every base of the file goes. How many went.
    pub(super) fn drop_changed(&mut self, path: &Path, now: Option<&FileNow>) -> usize {
        let before = self.held.len();
        self.held.retain(|s| {
            s.file.path != path || now.is_some_and(|n| n.serves(&s.base.edit, s.base.turn))
        });
        before - self.held.len()
    }

    /// How many bases of `bytes` each the budget holds: at least one
    /// when the machine's memory is not known ([`Budget::One`]).
    pub(super) fn room_for(&self, bytes: u64) -> u64 {
        match self.budget {
            Budget::Bytes(most) => most.checked_div(bytes).unwrap_or(0),
            Budget::One => 1,
        }
    }

    /// Let the least recently left go until the shelf is within its
    /// budget; how many went.
    pub(super) fn fit(&mut self) -> usize {
        let mut gone = 0;
        while !self.held.is_empty() && !self.budget.holds(self.held.len(), self.bytes()) {
            self.held.pop_back();
            gone += 1;
        }
        gone
    }

    /// Everything on the shelf let go: what it was made under has
    /// changed (the lens database).
    pub(super) fn clear(&mut self) {
        self.held.clear();
    }

    /// The budget's line for the log.
    pub(super) fn said(&self) -> String {
        let mb = |b: u64| b.div_ceil(1 << 20);
        match self.budget {
            Budget::Bytes(most) => format!(
                "{} base(s), {} MB of {} MB",
                self.held.len(),
                mb(self.bytes()),
                mb(most)
            ),
            Budget::One => format!(
                "{} base(s), {} MB, one at most",
                self.held.len(),
                mb(self.bytes())
            ),
        }
    }
}

/// The bytes of the pictures a shelved base holds: its own, its
/// patched picture, the learned pair, and the guide plane, each picture
/// counted once however many of them share it (a blend at either end
/// of its strength is one of the pair).
pub(super) fn bytes_of(base: &Base, learned: Option<&LearnedBase>) -> u64 {
    let mut seen: Vec<*const WorkingImage> = Vec::new();
    let mut total = 0u64;
    let mut count = |image: &Arc<WorkingImage>| {
        let at = Arc::as_ptr(image);
        if !seen.contains(&at) {
            seen.push(at);
            total += (image.data.len() * std::mem::size_of::<f32>()) as u64;
        }
    };
    count(&base.image);
    if let Some((_, patched, _)) = &base.patched {
        count(patched);
    }
    if let Some(l) = learned {
        count(&l.model);
        count(&l.plain);
    }
    total + (base.guide.data.len() * std::mem::size_of::<f32>()) as u64
}

#[cfg(test)]
mod tests {
    use super::super::engine::{forget_bases, swap_base};
    use super::*;

    /// A raw's base of `width` by `height` under `edit` at `turn`.
    fn base(edit: &Edit, turn: u8, width: usize, height: usize) -> Base {
        Base {
            edit: edit.clone(),
            turn,
            image: Arc::new(WorkingImage::new(width, height)),
            guide: Arc::new(crate::finish::Guide::NONE),
            white: WhiteBase::IDENTITY,
            radius: None,
            clip_level: 1.0,
            source: crate::finish::Source::SCENE,
            stamp: 1,
            patched: None,
            pre: None,
            uploaded: None,
            haze: None,
            planes: None,
            on_gpu: false,
            stand_in: None,
            gave_way: None,
            profile: None,
            from_shelf: false,
        }
    }

    fn file(name: &str) -> FileKey {
        FileKey {
            path: PathBuf::from(name),
            stat: Stat {
                len: 1000,
                modified: 7,
                changed: Some((7, 0)),
                inode: Some(42),
            },
            frame: Some(FrameId {
                make: "Canon".into(),
                model: "Canon EOS R5".into(),
                width: 8352,
                height: 5586,
                orientation: Orientation::Normal,
            }),
        }
    }

    /// The bytes of a base of `width` by `height`, as the shelf counts
    /// them.
    fn size(width: usize, height: usize) -> u64 {
        (width * height * 3 * 4) as u64
    }

    fn warmer() -> Edit {
        Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature: 6500.0,
                tint: 0.0,
            },
            ..Edit::default()
        }
    }

    /// A base shelved is taken back for its file under an edit with
    /// the same base at the same turn, the very picture it was, said
    /// to be from the shelf; what it held on the GPU and the masks'
    /// bands are not on the shelf with it, and it is off the shelf
    /// once taken.
    #[test]
    fn a_base_shelved_is_taken_back_for_the_same_edit_and_turn() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        let b = base(&edit, 1, 40, 30);
        let image = b.image.clone();
        shelf.put(file("a.CR3"), b, None);
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf.bytes(), size(40, 30));

        // A move after the base (the sharpen and the light) is the same base.
        let mut later = edit.clone();
        later.sharpen.enabled = !later.sharpen.enabled;
        later.light.exposure += 1.0;
        let (b, learned) = shelf
            .take(&file("a.CR3"), &later, 1, true)
            .expect("taken back");
        assert!(Arc::ptr_eq(&b.image, &image));
        assert!(b.from_shelf);
        assert!(learned.is_none());
        assert!(b.pre.is_none() && b.uploaded.is_none() && b.haze.is_none());
        assert!(b.planes.is_none());
        assert_eq!(shelf.len(), 0);
        assert!(shelf.take(&file("a.CR3"), &edit, 1, true).is_none());
    }

    /// Not taken back under an edit whose base differs, at another
    /// turn, for another file, for the same path written since, or,
    /// with no GPU, for a base whose CA was the GPU's; the one written
    /// since is let go on the way.
    #[test]
    fn a_base_is_not_taken_back_when_anything_it_was_made_from_differs() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a.CR3"), base(&edit, 0, 8, 8), None);
        assert!(shelf.take(&file("a.CR3"), &warmer(), 0, true).is_none());
        assert!(shelf.take(&file("a.CR3"), &edit, 1, true).is_none());
        assert!(shelf.take(&file("b.CR3"), &edit, 0, true).is_none());
        assert_eq!(shelf.len(), 1, "still there for its own edit and turn");

        let mut gpu_ca = base(&edit, 0, 8, 8);
        gpu_ca.on_gpu = true;
        shelf.put(file("c.CR3"), gpu_ca, None);
        assert!(shelf.take(&file("c.CR3"), &edit, 0, false).is_none());
        assert!(shelf.take(&file("c.CR3"), &edit, 0, true).is_some());

        let mut written = file("a.CR3");
        written.stat.modified += 1;
        assert!(shelf.take(&written, &edit, 0, true).is_none());
        assert_eq!(shelf.len(), 0, "the base of the file as it was is gone");
    }

    /// A rewrite in place that keeps the file's length and its
    /// modification time is still another file: its orientation tag
    /// edited, or its change time or inode moved. None of them is
    /// taken back for it, and the base of the file as it was goes.
    #[test]
    fn a_file_rewritten_under_the_same_size_and_time_is_not_taken_back() {
        let edit = Edit::default();
        let rewrites: [fn(&mut FileKey); 3] = [
            |k| k.frame.as_mut().unwrap().orientation = Orientation::Rotate90,
            |k| k.stat.changed = Some((8, 0)),
            |k| k.stat.inode = Some(43),
        ];
        for rewrite in rewrites {
            let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
            shelf.put(file("a.CR3"), base(&edit, 0, 8, 8), None);
            let mut rewritten = file("a.CR3");
            rewrite(&mut rewritten);
            assert_eq!(
                (rewritten.stat.len, rewritten.stat.modified),
                (file("a.CR3").stat.len, file("a.CR3").stat.modified)
            );
            assert!(shelf.take(&rewritten, &edit, 0, true).is_none());
            assert_eq!(shelf.len(), 0);
        }
    }

    /// The key of a file on disk: read before the decode with nothing
    /// of the frame, then given the decoded frame's body, size, and
    /// orientation.
    #[test]
    fn a_key_takes_the_decoded_frame() {
        let dir = std::env::temp_dir().join(format!("greycard-shelf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.raw");
        std::fs::write(&path, b"not a raw").unwrap();
        let key = FileKey::of(&path).expect("the file is there");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(key.stat.len, 9);
        assert!(key.frame.is_none());
        #[cfg(unix)]
        assert!(key.stat.changed.is_some() && key.stat.inode.is_some());
        let mut frame = super::super::tests::aberrated_frame(16, 12);
        frame.orientation = Orientation::Rotate270;
        let key = key.decoded(&Input::Raw(Arc::new(frame)));
        let id = key.frame.expect("a raw's frame");
        assert_eq!((id.width, id.height), (16, 12));
        assert_eq!(id.orientation, Orientation::Rotate270);
        assert_eq!((id.make.as_str(), id.model.as_str()), ("Test", "Cam"));
        assert!(FileKey::of(&dir.join("gone")).is_none());
    }

    /// Two versions of one file keep a base each; the same file, edit,
    /// and turn shelved again replaces the older.
    #[test]
    fn a_file_keeps_a_base_per_edit_and_the_newer_of_two_the_same() {
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a.CR3"), base(&Edit::default(), 0, 8, 8), None);
        shelf.put(file("a.CR3"), base(&warmer(), 0, 8, 8), None);
        assert_eq!(shelf.len(), 2);
        let newer = base(&Edit::default(), 0, 8, 8);
        let image = newer.image.clone();
        shelf.put(file("a.CR3"), newer, None);
        assert_eq!(shelf.len(), 2);
        let (b, _) = shelf
            .take(&file("a.CR3"), &Edit::default(), 0, true)
            .unwrap();
        assert!(Arc::ptr_eq(&b.image, &image));
    }

    /// Over its budget the shelf lets the least recently left go
    /// first. With the machine's memory unknown it keeps one.
    #[test]
    fn the_shelf_lets_the_least_recently_left_go_over_its_budget() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(2 * size(10, 10)));
        for name in ["a", "b", "c"] {
            shelf.put(file(name), base(&edit, 0, 10, 10), None);
        }
        assert_eq!(shelf.fit(), 1);
        assert_eq!(shelf.len(), 2);
        assert!(shelf.take(&file("a"), &edit, 0, true).is_none());
        assert!(shelf.take(&file("b"), &edit, 0, true).is_some());
        assert!(shelf.take(&file("c"), &edit, 0, true).is_some());

        let mut one = Shelf::new(Budget::One);
        one.put(file("a"), base(&edit, 0, 2000, 2000), None);
        assert_eq!(one.fit(), 0);
        one.put(file("b"), base(&edit, 0, 10, 10), None);
        assert_eq!(one.fit(), 1);
        assert!(one.take(&file("b"), &edit, 0, true).is_some());
    }

    /// A base over the whole budget on its own is not kept, and does
    /// not push out an older base that fits on its way off.
    #[test]
    fn a_base_over_the_whole_budget_leaves_the_others_where_they_are() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(2 * size(10, 10)));
        assert!(shelf.put(file("small"), base(&edit, 0, 10, 10), None));
        assert!(
            !shelf.put(file("big"), base(&edit, 0, 20, 20), None),
            "said not put"
        );
        assert_eq!(shelf.fit(), 0);
        assert_eq!(shelf.len(), 1);
        assert!(shelf.take(&file("big"), &edit, 0, true).is_none());
        assert!(shelf.take(&file("small"), &edit, 0, true).is_some());

        // A shelf turned off keeps nothing.
        let mut off = Shelf::new(Budget::Bytes(0));
        assert!(!off.put(file("a"), base(&edit, 0, 1, 1), None));
        assert_eq!(off.len(), 0);
    }

    /// `holds` answers as `take` would before the decode has said what
    /// the frame is: the path and the stat, the edit's base, the turn,
    /// and a GPU-CA base only with a GPU. With the seed still to come,
    /// the strength of the base found stands for the edit's.
    #[test]
    fn the_shelf_says_what_it_holds_before_a_decode() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 20));
        let a = file("a");
        let mut seeded = edit.clone();
        seeded.noise.learned_strength = 0.4;
        assert!(shelf.put(a.clone(), base(&seeded, 1, 10, 10), None));
        let holds = |edit: &Edit, seed: bool, turn: u8, has_gpu: bool| {
            shelf.holds(&a.path, &a.stat, edit, seed, turn, has_gpu)
        };
        assert!(holds(&seeded, false, 1, true));
        assert!(holds(&seeded, false, 1, false), "a CPU-CA base either way");
        assert!(!holds(&edit, false, 1, true), "another strength, as made");
        assert!(holds(&edit, true, 1, true), "the strength still to seed");
        assert!(!holds(&seeded, false, 0, true), "another turn");
        assert!(!holds(&warmer(), true, 1, true), "another base");
        let other = Stat {
            modified: 8,
            ..a.stat.clone()
        };
        assert!(!shelf.holds(&a.path, &other, &seeded, false, 1, true));
        assert!(!shelf.holds(Path::new("b"), &a.stat, &seeded, false, 1, true));

        let mut gpu = base(&edit, 0, 10, 10);
        gpu.on_gpu = true;
        assert!(shelf.put(file("g"), gpu, None));
        let g = file("g");
        assert!(shelf.holds(&g.path, &g.stat, &edit, false, 0, true));
        assert!(!shelf.holds(&g.path, &g.stat, &edit, false, 0, false));
    }

    /// How many bases of a size the budget holds: none for a shelf
    /// turned off, one when the memory is not known.
    #[test]
    fn the_shelf_says_how_many_bases_of_a_size_it_has_room_for() {
        let one = size(10, 10);
        assert_eq!(Shelf::new(Budget::Bytes(5 * one)).room_for(one), 5);
        assert_eq!(Shelf::new(Budget::Bytes(5 * one - 1)).room_for(one), 4);
        assert_eq!(Shelf::new(Budget::Bytes(0)).room_for(one), 0);
        assert_eq!(Shelf::new(Budget::Bytes(one)).room_for(0), 0);
        assert_eq!(Shelf::new(Budget::One).room_for(one), 1);
    }

    /// The base taken back on an open is the open frame's and never the
    /// shelf's to let go: a return to A with B in hand, on a shelf
    /// that holds one base of A's size, takes A and keeps it in hand,
    /// and B, over the budget alone, is not shelved. A frame opened
    /// again finds the base it was just in hand as.
    #[test]
    fn the_open_frame_s_base_is_never_let_go_by_the_shelf() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(size(10, 10)));
        let a = base(&edit, 0, 10, 10);
        let a_image = a.image.clone();
        shelf.put(file("a"), a, None);
        let mut held = Some(base(&edit, 0, 20, 20));
        let mut learned = None;
        swap_base(
            &mut held,
            &mut learned,
            &mut shelf,
            Some(file("b")),
            Some(&file("a")),
            &edit,
            0,
            None,
        );
        let b = held.as_ref().expect("A in hand");
        assert!(Arc::ptr_eq(&b.image, &a_image) && b.from_shelf);
        assert_eq!(shelf.len(), 0, "B alone was over the budget");

        // A opened again, as another version of it would be.
        swap_base(
            &mut held,
            &mut learned,
            &mut shelf,
            Some(file("a")),
            Some(&file("a")),
            &edit,
            0,
            None,
        );
        assert!(Arc::ptr_eq(&held.as_ref().unwrap().image, &a_image));

        // A file that cannot be read for its stat is neither shelved
        // nor taken.
        swap_base(
            &mut held,
            &mut learned,
            &mut shelf,
            None,
            None,
            &edit,
            0,
            None,
        );
        assert!(held.is_none());
        assert_eq!(shelf.len(), 0);
    }

    /// A new lens database takes the base in hand and every base on
    /// the shelf with it; before any open there is no shelf to empty.
    #[test]
    fn a_new_lens_database_empties_the_shelf() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a"), base(&edit, 0, 8, 8), None);
        shelf.put(file("b"), base(&edit, 0, 8, 8), None);
        let mut shelf = Some(shelf);
        let mut held = Some(base(&edit, 0, 8, 8));
        let mut learned = None;
        forget_bases(&mut held, &mut learned, &mut shelf, None);
        assert!(held.is_none());
        let shelf = shelf.as_mut().expect("still made");
        assert_eq!(shelf.len(), 0);
        assert!(shelf.take(&file("a"), &edit, 0, true).is_none());

        let mut none = None;
        forget_bases(&mut held, &mut learned, &mut none, None);
        assert!(none.is_none(), "not made by the change");
    }

    /// A file whose edit changed without an open: its bases that none
    /// of its edits now would take go, and only its; one whose base is
    /// still one of them stays, whatever changed after the base or in a
    /// learned strength no tier uses; a base kept for a version stays
    /// beside it; a turn changed drops it; with nothing at hand, all of
    /// the file's go.
    #[test]
    fn a_base_whose_edit_changed_under_it_is_dropped() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a"), base(&edit, 0, 8, 8), None);
        shelf.put(file("a"), base(&warmer(), 0, 8, 8), None);
        shelf.put(file("b"), base(&edit, 0, 8, 8), None);
        assert_eq!(shelf.len(), 3);

        let mut after = edit.clone();
        after.light.exposure += 1.0;
        after.noise.learned_strength += 0.25;
        let now = |edits: Vec<Edit>, turn: u8| FileNow { edits, turn };
        assert_eq!(
            shelf.drop_changed(Path::new("a"), Some(&now(vec![after.clone(), warmer()], 4))),
            0,
            "the main line's base and the version's both still taken"
        );
        assert_eq!(
            shelf.drop_changed(Path::new("a"), Some(&now(vec![after.clone()], 0))),
            1
        );
        assert!(shelf.take(&file("a"), &warmer(), 0, true).is_none());
        assert!(shelf.holds(Path::new("a"), &file("a").stat, &edit, false, 0, true));
        // Turned by a paste: no open takes the unturned base.
        assert_eq!(
            shelf.drop_changed(Path::new("a"), Some(&now(vec![after], 1))),
            1
        );
        shelf.put(file("a"), base(&edit, 0, 8, 8), None);
        assert_eq!(shelf.drop_changed(Path::new("a"), None), 1);
        assert_eq!(shelf.len(), 1);
        assert!(shelf.take(&file("b"), &edit, 0, true).is_some(), "b's own");
    }

    /// The learned pair goes on the shelf with a base blended from it,
    /// counted once where the base is one of the pair, and comes back
    /// with it; a pair the base was not made from is not kept. A base
    /// standing in for a missing denoiser, and a picture's, are not
    /// shelved at all.
    #[test]
    fn the_learned_pair_is_shelved_with_its_base_and_a_stand_in_is_not() {
        let mut edit = Edit::default();
        edit.noise.enabled = true;
        edit.noise.learned = greycard_edit::Learned::Fast;
        assert!(edit.noise.tier().is_some());
        let plain = Arc::new(WorkingImage::new(10, 10));
        let pair = || LearnedBase {
            edit: edit.clone(),
            turn: 0,
            model: Arc::new(WorkingImage::new(10, 10)),
            plain: plain.clone(),
            white: WhiteBase::IDENTITY,
            radius: None,
            clip_level: 1.0,
            ca_on_gpu: false,
        };
        let mut b = base(&edit, 0, 10, 10);
        b.image = plain.clone();
        assert_eq!(bytes_of(&b, Some(&pair())), 2 * size(10, 10));
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a"), b, Some(pair()));
        assert_eq!(shelf.bytes(), 2 * size(10, 10));
        let (b, l) = shelf.take(&file("a"), &edit, 0, true).unwrap();
        let l = l.expect("the pair came back");
        assert!(Arc::ptr_eq(&b.image, &l.plain));

        // The denoiser off: the pair is not the base's.
        let off = Edit::default();
        shelf.put(file("a"), base(&off, 0, 10, 10), Some(pair()));
        assert_eq!(shelf.bytes(), size(10, 10));
        assert!(shelf.take(&file("a"), &off, 0, true).unwrap().1.is_none());

        let mut stand_in = base(&edit, 0, 10, 10);
        stand_in.stand_in = Some(LearnedReport::Missing(&greycard_ai::DENOISE_FAST));
        shelf.put(file("a"), stand_in, None);
        let mut picture = base(&off, 0, 10, 10);
        picture.source = crate::finish::Source::Display;
        shelf.put(file("p.tif"), picture, None);
        assert_eq!(shelf.len(), 0);
    }

    /// A retouch's patched picture stays with its base and counts.
    #[test]
    fn the_patched_picture_stays_with_its_base() {
        let edit = Edit::default();
        let mut b = base(&edit, 0, 10, 10);
        let patched = Arc::new(WorkingImage::new(10, 10));
        b.patched = Some((Retouch::default(), patched.clone(), true));
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a"), b, None);
        assert_eq!(shelf.bytes(), 2 * size(10, 10));
        let (b, _) = shelf.take(&file("a"), &edit, 0, true).unwrap();
        assert!(Arc::ptr_eq(&b.patched.unwrap().1, &patched));
    }

    /// A quarter of the memory past the first 4 GB, at most 4 GB.
    #[test]
    fn the_budget_is_a_quarter_of_the_memory_past_four_gigabytes() {
        const G: u64 = 1 << 30;
        assert_eq!(Budget::of(G), Budget::Bytes(0));
        assert_eq!(Budget::of(4 * G), Budget::Bytes(0));
        assert_eq!(Budget::of(8 * G), Budget::Bytes(G));
        assert_eq!(Budget::of(16 * G), Budget::Bytes(3 * G));
        assert_eq!(Budget::of(64 * G), Budget::Bytes(4 * G));
    }

    /// `GREYCARD_SHELF_MB`: a whole number of megabytes is the budget,
    /// 0 turns the shelf off, a value too large for bytes is as much as
    /// there is, and anything else is ignored for the machine's own.
    /// Without it, the machine's, or one base when that is unknown.
    #[test]
    fn the_shelf_s_variable_sets_the_budget_or_is_ignored() {
        const G: u64 = 1 << 30;
        let ram = Some(64 * G);
        let of = |v: Option<&str>| Budget::from_setting(v, ram);
        assert_eq!(of(Some("0")), Budget::Bytes(0));
        assert_eq!(of(Some(" 512 ")), Budget::Bytes(512 << 20));
        assert_eq!(of(Some(&u64::MAX.to_string())), Budget::Bytes(u64::MAX));
        for bad in ["", "lots", "-1", "1.5", "2G"] {
            assert_eq!(of(Some(bad)), Budget::of(64 * G), "{bad:?}");
        }
        assert_eq!(of(None), Budget::Bytes(4 * G));
        assert_eq!(Budget::from_setting(None, None), Budget::One);
        assert_eq!(Budget::from_setting(Some("lots"), None), Budget::One);
    }
}
