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
//! and by what it was made under: the steps of the frame's list to it,
//! which say the edit's base and the frame's quarter turns. The frame
//! keeps the retouch's patched picture when it has one, so the fills
//! are not made again, and the learned denoiser's pair when the base
//! was blended from it, so a move of the strength after the return is
//! a blend again and not a run of the network ([`crate::walk::Kept::shelve`]). It
//! lets go of everything on the GPU (the picture before the sharpen,
//! the patched picture's upload, and the dehaze's reduced copy), which
//! the device would have to hold for frames not on screen, and of the
//! masks' detail bands, which are made again beside the develop and
//! would cost the shelf as much as the base.
//!
//! What is held is held under a budget of bytes ([`Budget::user`]),
//! the least recently left going first. The base of the open frame is
//! not on the shelf and is never counted against it: a base is taken
//! off before the shelf is brought back under its budget. The budget is
//! worked out again at each open ([`Shelf::size_for`]): what the open
//! frame holds and what making a base costs come off the machine's
//! memory first ([`crate::memory`]), so a large frame leaves the shelf less.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use greycard_core::raw::Orientation;

use super::*;
pub(super) use crate::memory::{COST, Gpu, Held, Machine, Uses};
use crate::stages::Facts;

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
    /// The most the budget is on this machine ([`Budget::from_setting`]),
    /// with `GREYCARD_SHELF_MB` read from the environment here, the one
    /// place it is read: once a session, when the worker's first open
    /// makes the shelf. With it, the machine's memory to size the
    /// shelf on at each open ([`Shelf::size_for`]): none when a setting
    /// gave the budget or the memory is not known.
    pub(super) fn user() -> (Self, Option<u64>) {
        let setting = std::env::var("GREYCARD_SHELF_MB").ok();
        let ram = greycard_ai::ram::physical();
        let set = setting
            .as_deref()
            .is_some_and(|v| v.trim().parse::<u64>().is_ok());
        (
            Self::from_setting(setting.as_deref(), ram),
            ram.filter(|_| !set),
        )
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

/// What a base made for the shelf needs that is not there, for the
/// log: the bytes it needs and the room left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Short {
    pub need: u64,
    pub room: u64,
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

/// What a frame left behind kept.
pub(super) struct Shelved {
    file: FileKey,
    held: FrameKept,
    bytes: u64,
}

/// What a file holds now, as the window has it after its edit changed
/// without an open of it: every edit its sidecar holds, its versions'
/// among them, and its quarter turns.
#[derive(Clone, Debug)]
pub struct FileNow {
    pub edits: Vec<Edit>,
    pub turn: u8,
}

impl FileNow {
    /// Whether a base made under `edit` at `turn` of the frame `facts`
    /// describes is still one an open of the file would take: the same
    /// turn, and one of its edits with the same steps to the base. A
    /// base made under no learned tier does not depend on the learned
    /// strength, which a frame never developed seeds at its open and
    /// not in its sidecar, so a strength alone keeps it there; a base
    /// made under a tier is kept only at the strength the sidecar says,
    /// so one shelved from a seeded frame never saved goes on the next
    /// save of the file (a base made again, never a wrong one).
    pub(super) fn serves(
        &self,
        edit: &Edit,
        turn: u8,
        facts: &Facts,
        lenses: Option<&greycard_lens::Database>,
    ) -> bool {
        let facts = Facts {
            turn: turn % 4,
            ..facts.clone()
        };
        self.turn % 4 == turn % 4
            && self.edits.iter().any(|now| {
                let mut as_made = now.clone();
                if now.noise.tier().is_none() && edit.noise.tier().is_none() {
                    as_made.noise.learned_strength = edit.noise.learned_strength;
                    as_made.noise.learned_chroma_strength = edit.noise.learned_chroma_strength;
                }
                crate::stages::same_base(edit, &as_made, &facts, lenses)
            })
    }
}

/// The bases left behind, the most recently left first.
pub(super) struct Shelf {
    /// The budget now, within `cap`.
    budget: Budget,
    /// The most the budget is, whatever the open frame: the machine's
    /// ([`Budget::of`]), or the one a setting or a test gave.
    cap: Budget,
    /// The machine the budget is worked out on at each open; none when
    /// its memory is not known or a setting gave the budget, which is
    /// then the cap.
    machine: Option<Machine>,
    /// What the open frame holds at the last sizing, as the machine's
    /// memory counts it.
    open: u64,
    held: VecDeque<Shelved>,
}

impl Shelf {
    pub(super) fn new(budget: Budget) -> Self {
        Self {
            budget,
            cap: budget,
            machine: None,
            open: 0,
            held: VecDeque::new(),
        }
    }

    /// A shelf under `cap`, sized on `machine` at each open.
    pub(super) fn on(cap: Budget, machine: Machine) -> Self {
        Self {
            machine: Some(machine),
            ..Self::new(cap)
        }
    }

    /// The shelf for this machine's budget ([`Budget::user`]), said in
    /// the log as it is made: sized on the machine's memory at each
    /// open, unless `GREYCARD_SHELF_MB` set the budget.
    pub(super) fn user() -> Self {
        let (cap, ram) = Budget::user();
        let shelf = match ram {
            Some(ram) => Self::on(cap, Machine::of(ram)),
            None => Self::new(cap),
        };
        tracing::info!("the shelf of frames left: {}", shelf.said());
        shelf
    }

    /// The budget worked out again for an open frame of `pixels` whose
    /// develop runs where `gpu` says, under an edit that `uses` so much:
    /// the machine's allowance less what the frame holds and the room
    /// kept for what comes and goes beside it ([`COST`]), within the
    /// cap. The shelf is brought under it, the least recently left going
    /// first; how many went. A shelf with no machine keeps its cap.
    pub(super) fn size_for(&mut self, pixels: u64, gpu: Gpu, uses: Uses) -> usize {
        let Some(machine) = self.machine else {
            return 0;
        };
        let (held, making) = COST.charged(pixels, gpu, uses);
        self.open = held;
        let room = machine
            .allowance
            .saturating_sub(held.saturating_add(making));
        let budget = match self.cap {
            Budget::Bytes(most) => Budget::Bytes(most.min(room)),
            Budget::One => Budget::One,
        };
        if budget == self.budget {
            return 0;
        }
        self.budget = budget;
        let gone = self.fit();
        tracing::info!(
            "the shelf of frames left: {} beside a {:.1} MP frame {}{}",
            self.said(),
            pixels as f64 / 1e6,
            gpu.said(),
            if gone > 0 {
                format!(", {gone} let go")
            } else {
                String::new()
            }
        );
        gone
    }

    /// Room on the machine for a base of `bytes` of the file `file`,
    /// made at a cost of `making`, beside the open frame (as the last
    /// sizing counted it) and the shelf. Where there is not, the bases
    /// a put and a fit would let go anyway are let go now, before the
    /// making, the least recently left first, unless the next to go is
    /// a base of a file in `keep` other than `file`'s. How many went,
    /// or the bytes needed and the room there was. A shelf with no
    /// machine always has room.
    pub(super) fn room_to_make(
        &mut self,
        file: &FileKey,
        (bytes, making): (u64, u64),
        keep: &[PathBuf],
    ) -> Result<usize, Short> {
        let Some(machine) = self.machine else {
            return Ok(0);
        };
        let need = bytes.saturating_add(making);
        let open = self.open;
        let room = |on: u64| machine.allowance.saturating_sub(open.saturating_add(on));
        let mut on = self.bytes();
        let short = Short {
            need,
            room: room(on),
        };
        let mut going = 0;
        for s in self.held.iter().rev() {
            if need <= room(on) {
                break;
            }
            if s.file.path != file.path && keep.contains(&s.file.path) {
                return Err(short);
            }
            on -= s.bytes;
            going += 1;
        }
        if need > room(on) {
            return Err(short);
        }
        self.held.truncate(self.held.len() - going);
        Ok(going)
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
    /// its decode, which an open makes again whatever is kept). One
    /// the engine's own path made standing in for a learned denoiser
    /// is, while the model is still not to be had (`stands_in`), and is
    /// taken back only while that lasts ([`Shelf::take`]). One that
    /// stood in with the model there (the card short, a run failed) is
    /// not: the walk back runs the model, so it would never be taken,
    /// and would hold the shelf's budget against the bases that are.
    fn takes(held: &FrameKept, stands_in: bool) -> bool {
        held.meta.source != greycard_render::finish::Source::Display
            && (held.meta.stood_in.is_none() || stands_in)
            && held.kept.picture(crate::stages::Keep::Base).is_some()
    }

    /// Put what the frame `file` kept on the shelf ([`crate::walk::Kept::shelve`]);
    /// the shelf is not brought under its budget until [`Shelf::fit`].
    /// The caller lets the ops' working textures go. A frame over the
    /// whole budget on its own is not put at all, so it cannot push out
    /// the bases that fit. A base already there for the same file and
    /// the same steps at the same turn is the older of two and goes, as
    /// does one of the same path made from what the file was before.
    /// `stands_in` says whether the learned denoiser the frame's edit
    /// asks for is still not to be had ([`Shelf::takes`]). Whether the
    /// base was put.
    pub(super) fn put(&mut self, file: FileKey, mut held: FrameKept, stands_in: bool) -> bool {
        if !Self::takes(&held, stands_in) {
            return false;
        }
        held.kept.shelve();
        held.planes = None;
        held.from_shelf = false;
        let bytes = held.bytes();
        if !self.budget.holds(1, bytes) {
            return false;
        }
        self.held.retain(|s| !Self::replaced(s, &file, &held));
        self.held.push_front(Shelved { file, held, bytes });
        true
    }

    /// What the frame `file` kept, taken off the shelf, when a walk of
    /// `edit` at `turn` starts from its base: with `has_gpu` false (no
    /// device), only a base whose CA and lens were not the GPU's; with
    /// `stands_in` (the learned denoiser the edit asks for is not to be
    /// had now), only one the engine's own path made in its place, and
    /// without, never one ([`FrameKept::serves`]). A base of the same
    /// path from before the file changed is dropped on the way.
    pub(super) fn take(
        &mut self,
        file: &FileKey,
        edit: &Edit,
        turn: u8,
        (has_gpu, stands_in): (bool, bool),
        lenses: Option<&greycard_lens::Database>,
    ) -> Option<FrameKept> {
        self.held
            .retain(|s| s.file.path != file.path || s.file == *file);
        let at = self.held.iter().position(|s| {
            s.file == *file && s.held.serves(edit, turn, lenses, has_gpu, stands_in)
        })?;
        let Shelved { mut held, .. } = self.held.remove(at)?;
        held.from_shelf = true;
        Some(held)
    }

    /// Whether a base of the file at `path`, as the file system now
    /// says it is (`stat`), that a walk of `edit` at `turn` starts
    /// from, is on the shelf for an open to take: [`Shelf::take`]'s test
    /// without the decode's word on the frame, which a prefetch asks
    /// before it decodes. With `seeded`, the edit's learned blend is
    /// still to be seeded from the frame's ISO, so the strength of the
    /// base found is taken for the edit's: the edits are otherwise the
    /// same, and a base made without a learned tier does not depend on
    /// it. A base standing in for the learned denoiser is not counted:
    /// a prefetch makes no learned frame's base.
    pub(super) fn holds(
        &self,
        (path, stat): (&Path, &Stat),
        edit: &Edit,
        seeded: bool,
        turn: u8,
        has_gpu: bool,
        lenses: Option<&greycard_lens::Database>,
    ) -> bool {
        self.held.iter().any(|s| {
            let edit_as_made = seeded.then(|| {
                let mut e = edit.clone();
                e.noise.learned_strength = s.held.edit.noise.learned_strength;
                e
            });
            s.file.path == path
                && s.file.stat == *stat
                && s.held.serves(
                    edit_as_made.as_ref().unwrap_or(edit),
                    turn,
                    lenses,
                    has_gpu,
                    false,
                )
        })
    }

    /// The bases of the file at `path` that no open will take any more,
    /// let go: its edit changed under them without an open of it (a
    /// sync, a paste, a sidecar written by another hand), and what it
    /// holds `now` serves none of them ([`FileNow::serves`]). With
    /// nothing at hand, every base of the file goes. How many went.
    pub(super) fn drop_changed(
        &mut self,
        path: &Path,
        now: Option<&FileNow>,
        lenses: Option<&greycard_lens::Database>,
    ) -> usize {
        let before = self.held.len();
        self.held.retain(|s| {
            s.file.path != path
                || now.is_some_and(|n| n.serves(&s.held.edit, s.held.turn, &s.held.facts, lenses))
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

    /// Whether the put of `held` for `file` replaces the shelved `s`:
    /// one of the same path made from what the file was before, or the
    /// same file's base at the same turn for the same steps, or, where
    /// one of the two stood in for the learned denoiser and the other
    /// did not, under the same noise section and the same steps but for
    /// the learned denoiser, the lens's as walked among them: the
    /// learned denoiser came or went between them, and the newer is the
    /// one an open would take now.
    fn replaced(s: &Shelved, file: &FileKey, held: &FrameKept) -> bool {
        let stood_in = |h: &FrameKept| h.meta.stood_in.is_some();
        s.file.path == file.path
            && (s.file != *file
                || (s.held.turn == held.turn
                    && (s.held.kept.same_base(&held.kept)
                        || (stood_in(&s.held) != stood_in(held)
                            && s.held.edit.noise == held.edit.noise
                            && s.held.kept.same_base_but_learned(&held.kept)))))
    }

    /// The file of a base in `keep` that putting the base `held` for
    /// `file` and bringing the shelf under its budget would let go, the
    /// least recently left first, as [`Shelf::put`] and [`Shelf::fit`]
    /// would.
    pub(super) fn pushes_out_of(
        &self,
        file: &FileKey,
        held: &FrameKept,
        keep: &[PathBuf],
    ) -> Option<PathBuf> {
        self.pushes_out_where(
            |s| Self::replaced(s, file, held),
            &file.path,
            held.bytes(),
            keep,
        )
    }

    /// [`Shelf::pushes_out_of`] before the base is made, for one of
    /// `bytes` of the file at `file` as the file system says it is now:
    /// the bases of the file as it was before go with the put; those of
    /// the file as it is, at another turn or under another base, are
    /// counted as staying, which they may.
    pub(super) fn pushes_out_before(
        &self,
        file: &FileKey,
        bytes: u64,
        keep: &[PathBuf],
    ) -> Option<PathBuf> {
        self.pushes_out_where(
            |s| s.file.path == file.path && s.file.stat != file.stat,
            &file.path,
            bytes,
            keep,
        )
    }

    /// The file of a base in `keep`, other than one of `own`, that a put
    /// of `bytes` replacing the bases `replaced` says, then a fit, would
    /// let go; a base over the whole budget is not put, and pushes
    /// nothing out.
    fn pushes_out_where(
        &self,
        replaced: impl Fn(&Shelved) -> bool,
        own: &Path,
        bytes: u64,
        keep: &[PathBuf],
    ) -> Option<PathBuf> {
        if !self.budget.holds(1, bytes) {
            return None;
        }
        let mut staying: Vec<&Shelved> = self.held.iter().filter(|s| !replaced(s)).collect();
        let mut total = bytes + staying.iter().map(|s| s.bytes).sum::<u64>();
        while !self.budget.holds(staying.len() + 1, total) {
            let out = staying.pop()?;
            if out.file.path != own && keep.contains(&out.file.path) {
                return Some(out.file.path.clone());
            }
            total -= out.bytes;
        }
        None
    }

    /// The bases of the files in `named` brought to the front, as if
    /// just left, their order among themselves kept: what a list of
    /// neighbors names is not the first to go when another is made.
    pub(super) fn bring_forward(&mut self, named: &[PathBuf]) {
        let (mut wanted, rest): (VecDeque<Shelved>, VecDeque<Shelved>) =
            std::mem::take(&mut self.held)
                .into_iter()
                .partition(|s| named.contains(&s.file.path));
        wanted.extend(rest);
        self.held = wanted;
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

#[cfg(test)]
mod tests {
    use super::super::engine::{forget_bases, swap_base};
    use super::*;
    use crate::stages::{Keep, Masks, Stage};
    use crate::walk::{Meta, Models, NoLearned, Patched, Rasters};

    /// A raw of `width` by `height`.
    fn raw(width: usize, height: usize) -> Arc<RawFrame> {
        Arc::new(super::super::tests::aberrated_frame(width, height))
    }

    /// What the window's walk of a raw of `width` by `height` keeps under
    /// `edit` at `turn`.
    fn base(edit: &Edit, turn: u8, width: usize, height: usize) -> FrameKept {
        cpu_points(FrameKept::walked(&raw(width, height), edit, turn))
    }

    /// `held` without the developed picture a walk on the CPU keeps,
    /// which no frame left behind takes to the shelf: the sizes the
    /// tests count are the shelf's.
    fn cpu_points(mut held: FrameKept) -> FrameKept {
        held.kept.take_working(Keep::Developed);
        held
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

    fn warmer() -> Edit {
        Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature: 6500.0,
                tint: 0.0,
            },
            ..Edit::default()
        }
    }

    /// The learned denoiser as the plain demosaic, for a pair to be had
    /// without a model.
    struct Network;

    impl Models for Network {
        fn can_denoise(&mut self, _: &'static greycard_ai::Model) -> Result<(), NoLearned> {
            Ok(())
        }

        fn denoise(
            &mut self,
            _: &'static greycard_ai::Model,
            _: &RawFrame,
            prepared: &Prepared,
        ) -> Result<Vec<f32>, NoLearned> {
            greycard_core::develop::demosaic_prepared(prepared, &DevelopSettings::default())
                .map(|(rgb, _, _)| rgb)
                .map_err(|e| NoLearned::Failed(e.to_string()))
        }

        fn fill(
            &mut self,
            _: &greycard_edit::retouch::Patch,
            _: &greycard_core::develop::retouch::Region,
            _: &WorkingImage,
        ) -> Option<Vec<f32>> {
            None
        }

        fn could_fill_more(&mut self, _: &Retouch, _: &WorkingImage) -> bool {
            false
        }

        fn rasters(
            &mut self,
            _: Masks,
            _: &Arc<WorkingImage>,
            _: Option<Patched<'_>>,
            _: &Meta,
        ) -> Rasters {
            Rasters::new()
        }
    }

    /// An edit with the learned denoiser at `strength`.
    fn learned(strength: f32) -> Edit {
        let mut edit = Edit::default();
        edit.noise.enabled = true;
        edit.noise.learned = greycard_edit::Learned::Fast;
        edit.noise.learned_strength = strength;
        edit
    }

    /// A raw's develop under the learned denoiser at `strength`, its
    /// pair kept.
    fn paired(strength: f32) -> FrameKept {
        cpu_points(FrameKept::walked_with(
            &crate::walk::Input::Raw(raw(32, 24)),
            &learned(strength),
            0,
            &mut Network,
        ))
    }

    /// A base shelved is taken back for its file under an edit with
    /// the same base at the same turn, the very picture it was, said
    /// to be from the shelf; the masks' bands are not on the shelf with
    /// it, and it is off the shelf once taken.
    #[test]
    fn a_base_shelved_is_taken_back_for_the_same_edit_and_turn() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        let b = base(&edit, 1, 40, 30);
        let image = b.image.clone();
        let bytes = b.bytes();
        shelf.put(file("a.CR3"), b, false);
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf.bytes(), bytes);

        // A move after the base (the sharpen and the light) is the same base.
        let mut later = edit.clone();
        later.sharpen.enabled = !later.sharpen.enabled;
        later.light.exposure += 1.0;
        let b = shelf
            .take(&file("a.CR3"), &later, 1, (true, false), None)
            .expect("taken back");
        assert!(Arc::ptr_eq(b.kept.picture(Keep::Base).unwrap(), &image));
        assert!(b.from_shelf);
        assert!(!b.kept.holds(Keep::Pair));
        assert!(b.planes.is_none());
        assert_eq!(shelf.len(), 0);
        assert!(
            shelf
                .take(&file("a.CR3"), &edit, 1, (true, false), None)
                .is_none()
        );
    }

    /// Not taken back under an edit whose base differs, at another
    /// turn, for another file, for the same path written since, or,
    /// with no GPU, for a base whose CA was the GPU's; the one written
    /// since is let go on the way.
    #[test]
    fn a_base_is_not_taken_back_when_anything_it_was_made_from_differs() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a.CR3"), base(&edit, 0, 32, 24), false);
        assert!(
            shelf
                .take(&file("a.CR3"), &warmer(), 0, (true, false), None)
                .is_none()
        );
        assert!(
            shelf
                .take(&file("a.CR3"), &edit, 1, (true, false), None)
                .is_none()
        );
        assert!(
            shelf
                .take(&file("b.CR3"), &edit, 0, (true, false), None)
                .is_none()
        );
        assert_eq!(shelf.len(), 1, "still there for its own edit and turn");

        let mut gpu_ca = base(&edit, 0, 32, 24);
        gpu_ca.kept.as_if_on_device(|s| matches!(s, Stage::Ca(_)));
        shelf.put(file("c.CR3"), gpu_ca, false);
        assert!(
            shelf
                .take(&file("c.CR3"), &edit, 0, (false, false), None)
                .is_none()
        );
        assert!(
            shelf
                .take(&file("c.CR3"), &edit, 0, (true, false), None)
                .is_some()
        );

        let mut written = file("a.CR3");
        written.stat.modified += 1;
        assert!(
            shelf
                .take(&written, &edit, 0, (true, false), None)
                .is_none()
        );
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
            shelf.put(file("a.CR3"), base(&edit, 0, 32, 24), false);
            let mut rewritten = file("a.CR3");
            rewrite(&mut rewritten);
            assert_eq!(
                (rewritten.stat.len, rewritten.stat.modified),
                (file("a.CR3").stat.len, file("a.CR3").stat.modified)
            );
            assert!(
                shelf
                    .take(&rewritten, &edit, 0, (true, false), None)
                    .is_none()
            );
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
        crate::testing::remove_dir_retry(&dir);
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

    /// Two versions of one file keep a base each; the same file, steps,
    /// and turn shelved again replaces the older.
    #[test]
    fn a_file_keeps_a_base_per_edit_and_the_newer_of_two_the_same() {
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a.CR3"), base(&Edit::default(), 0, 32, 24), false);
        shelf.put(file("a.CR3"), base(&warmer(), 0, 32, 24), false);
        assert_eq!(shelf.len(), 2);
        let newer = base(&Edit::default(), 0, 32, 24);
        let image = newer.image.clone();
        shelf.put(file("a.CR3"), newer, false);
        assert_eq!(shelf.len(), 2);
        let b = shelf
            .take(&file("a.CR3"), &Edit::default(), 0, (true, false), None)
            .unwrap();
        assert!(Arc::ptr_eq(&b.image, &image));
    }

    /// Over its budget the shelf lets the least recently left go
    /// first. With the machine's memory unknown it keeps one.
    #[test]
    fn the_shelf_lets_the_least_recently_left_go_over_its_budget() {
        let edit = Edit::default();
        let one = base(&edit, 0, 32, 24).bytes();
        let mut shelf = Shelf::new(Budget::Bytes(2 * one));
        for name in ["a", "b", "c"] {
            shelf.put(file(name), base(&edit, 0, 32, 24), false);
        }
        assert_eq!(shelf.fit(), 1);
        assert_eq!(shelf.len(), 2);
        assert!(
            shelf
                .take(&file("a"), &edit, 0, (true, false), None)
                .is_none()
        );
        assert!(
            shelf
                .take(&file("b"), &edit, 0, (true, false), None)
                .is_some()
        );
        assert!(
            shelf
                .take(&file("c"), &edit, 0, (true, false), None)
                .is_some()
        );

        let mut one = Shelf::new(Budget::One);
        one.put(file("a"), base(&edit, 0, 64, 48), false);
        assert_eq!(one.fit(), 0);
        one.put(file("b"), base(&edit, 0, 32, 24), false);
        assert_eq!(one.fit(), 1);
        assert!(
            one.take(&file("b"), &edit, 0, (true, false), None)
                .is_some()
        );
    }

    /// What a put would push out is said before it is made: a base
    /// four times the open frame's size on a shelf for three of them
    /// pushes out the two before it, and names the first of them it is
    /// asked to keep; a base over the whole budget pushes nothing out.
    #[test]
    fn the_shelf_says_what_a_put_would_push_out() {
        let edit = Edit::default();
        let one = base(&edit, 0, 32, 24).bytes();
        let big = base(&edit, 0, 64, 48).bytes();
        assert!(big >= 3 * one, "{big} against {one}");
        let mut shelf = Shelf::new(Budget::Bytes(big + one));
        shelf.put(file("left"), base(&edit, 0, 32, 24), false);
        shelf.put(file("named"), base(&edit, 0, 32, 24), false);
        let keep = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<_>>();
        // A small one fits beside both.
        assert_eq!(
            shelf.pushes_out_before(&file("n"), one, &keep(&["left"])),
            None
        );
        // A big one pushes out "left", the least recently put.
        assert_eq!(
            shelf.pushes_out_before(&file("n"), big, &keep(&["left", "named"])),
            Some(PathBuf::from("left"))
        );
        assert_eq!(
            shelf.pushes_out_before(&file("n"), big, &keep(&["named"])),
            None,
            "only left goes, which is not kept"
        );
        // A file's own base going is no loss to it.
        assert_eq!(
            shelf.pushes_out_before(&file("left"), big, &keep(&["left", "named"])),
            None
        );
        assert_eq!(
            shelf.pushes_out_before(&file("n"), 2 * big, &keep(&["left"])),
            None,
            "not put at all"
        );
        // One base at most: anything put pushes out what is there.
        let mut alone = Shelf::new(Budget::One);
        alone.put(file("left"), base(&edit, 0, 32, 24), false);
        assert_eq!(
            alone.pushes_out_before(&file("n"), one, &keep(&["left"])),
            Some(PathBuf::from("left"))
        );
    }

    /// The file's own base at another turn stays on a put, as `put`
    /// keeps it, and is counted: the base it leaves no room for goes.
    /// At the same turn under the same base it is replaced, and room is
    /// left.
    #[test]
    fn a_put_counts_the_file_s_own_base_that_stays() {
        let edit = Edit::default();
        let one = base(&edit, 0, 32, 24).bytes();
        let big = base(&edit, 0, 64, 48).bytes();
        let mut shelf = Shelf::new(Budget::Bytes(big + one));
        shelf.put(file("named"), base(&edit, 0, 32, 24), false);
        shelf.put(file("own"), base(&edit, 0, 32, 24), false);
        let keep = vec![PathBuf::from("named")];
        let mut turned = base(&edit, 1, 64, 48);
        turned.kept.shelve();
        assert_eq!(
            shelf.pushes_out_of(&file("own"), &turned, &keep),
            Some(PathBuf::from("named"))
        );
        let mut same = base(&edit, 0, 64, 48);
        same.kept.shelve();
        assert_eq!(shelf.pushes_out_of(&file("own"), &same, &keep), None);
        assert_eq!(
            shelf.pushes_out_before(&file("own"), big, &keep),
            Some(PathBuf::from("named"))
        );
    }

    /// The bases a list names brought to the front go last: the others
    /// are let go before them, the order among them kept.
    #[test]
    fn the_bases_a_list_names_are_brought_forward() {
        let edit = Edit::default();
        let one = base(&edit, 0, 32, 24).bytes();
        let mut shelf = Shelf::new(Budget::Bytes(3 * one));
        for name in ["a", "b", "c"] {
            shelf.put(file(name), base(&edit, 0, 32, 24), false);
        }
        // The front is c, b, a: a goes first.
        shelf.bring_forward(&[PathBuf::from("a"), PathBuf::from("b")]);
        let order: Vec<_> = shelf.held.iter().map(|s| s.file.path.clone()).collect();
        assert_eq!(order, ["b", "a", "c"].map(PathBuf::from));
        shelf.put(file("d"), base(&edit, 0, 32, 24), false);
        assert_eq!(shelf.fit(), 1);
        assert!(
            shelf
                .take(&file("c"), &edit, 0, (true, false), None)
                .is_none()
        );
        assert!(
            shelf
                .take(&file("a"), &edit, 0, (true, false), None)
                .is_some()
        );
    }

    /// A base over the whole budget on its own is not kept, and does
    /// not push out an older base that fits on its way off.
    #[test]
    fn a_base_over_the_whole_budget_leaves_the_others_where_they_are() {
        let edit = Edit::default();
        let small = base(&edit, 0, 32, 24);
        let mut shelf = Shelf::new(Budget::Bytes(2 * small.bytes()));
        assert!(shelf.put(file("small"), small, false));
        assert!(
            !shelf.put(file("big"), base(&edit, 0, 96, 72), false),
            "said not put"
        );
        assert_eq!(shelf.fit(), 0);
        assert_eq!(shelf.len(), 1);
        assert!(
            shelf
                .take(&file("big"), &edit, 0, (true, false), None)
                .is_none()
        );
        assert!(
            shelf
                .take(&file("small"), &edit, 0, (true, false), None)
                .is_some()
        );

        // A shelf turned off keeps nothing.
        let mut off = Shelf::new(Budget::Bytes(0));
        assert!(!off.put(file("a"), base(&edit, 0, 32, 24), false));
        assert_eq!(off.len(), 0);
    }

    /// `holds` answers as `take` would before the decode has said what
    /// the frame is: the path and the stat, the steps to the base, the
    /// turn, and a GPU-CA base only with a GPU. A learned strength is
    /// part of the base only under a tier; with the seed still to come,
    /// the strength of the base found stands for the edit's.
    #[test]
    fn the_shelf_says_what_it_holds_before_a_decode() {
        let edit = Edit::default();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        let a = file("a");
        let mut strength = edit.clone();
        strength.noise.learned_strength = 0.4;
        assert!(shelf.put(a.clone(), base(&strength, 1, 32, 24), false));
        let holds = |shelf: &Shelf, edit: &Edit, seed: bool, turn: u8, has_gpu: bool| {
            shelf.holds((&a.path, &a.stat), edit, seed, turn, has_gpu, None)
        };
        assert!(holds(&shelf, &strength, false, 1, true));
        assert!(
            holds(&shelf, &strength, false, 1, false),
            "a CPU-CA base either way"
        );
        assert!(holds(&shelf, &edit, false, 1, true), "no tier reads it");
        assert!(!holds(&shelf, &strength, false, 0, true), "another turn");
        assert!(!holds(&shelf, &warmer(), true, 1, true), "another base");
        let other = Stat {
            modified: 8,
            ..a.stat.clone()
        };
        assert!(!shelf.holds((&a.path, &other), &strength, false, 1, true, None));
        assert!(!shelf.holds((Path::new("b"), &a.stat), &strength, false, 1, true, None));

        let mut gpu = base(&edit, 0, 32, 24);
        gpu.kept.as_if_on_device(|s| matches!(s, Stage::Ca(_)));
        assert!(shelf.put(file("g"), gpu, false));
        let g = file("g");
        assert!(shelf.holds((&g.path, &g.stat), &edit, false, 0, true, None));
        assert!(!shelf.holds((&g.path, &g.stat), &edit, false, 0, false, None));

        // Under a tier the strength is the base's blend.
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        assert!(shelf.put(a.clone(), paired(0.4), false));
        assert!(holds(&shelf, &learned(0.4), false, 0, true));
        assert!(!holds(&shelf, &learned(0.35), false, 0, true), "as made");
        assert!(
            holds(&shelf, &learned(0.35), true, 0, true),
            "still to seed"
        );
    }

    /// How many bases of a size the budget holds: none for a shelf
    /// turned off, one when the memory is not known.
    #[test]
    fn the_shelf_says_how_many_bases_of_a_size_it_has_room_for() {
        let one = 1200;
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
        let a = base(&edit, 0, 32, 24);
        let mut shelf = Shelf::new(Budget::Bytes(a.bytes()));
        let a_image = a.image.clone();
        shelf.put(file("a"), a, false);
        let mut held = Some(base(&edit, 0, 64, 48));
        let opened = Input::Raw(raw(32, 24));
        swap_base(
            &mut held,
            &mut shelf,
            (Some(file("b")), Some((&file("a"), &opened))),
            (&edit, 0, &|_, _| false),
            None,
            None,
        );
        let b = held.as_ref().expect("A in hand");
        assert!(Arc::ptr_eq(&b.image, &a_image) && b.from_shelf);
        assert_eq!(shelf.len(), 0, "B alone was over the budget");

        // A opened again, as another version of it would be.
        swap_base(
            &mut held,
            &mut shelf,
            (Some(file("a")), Some((&file("a"), &opened))),
            (&edit, 0, &|_, _| false),
            None,
            None,
        );
        assert!(Arc::ptr_eq(&held.as_ref().unwrap().image, &a_image));

        // A file that cannot be read for its stat is neither shelved
        // nor taken.
        swap_base(
            &mut held,
            &mut shelf,
            (None, None),
            (&edit, 0, &|_, _| false),
            None,
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
        shelf.put(file("a"), base(&edit, 0, 32, 24), false);
        shelf.put(file("b"), base(&edit, 0, 32, 24), false);
        let mut shelf = Some(shelf);
        let mut held = Some(base(&edit, 0, 32, 24));
        forget_bases(&mut held, &mut shelf, None);
        assert!(held.is_none());
        let shelf = shelf.as_mut().expect("still made");
        assert_eq!(shelf.len(), 0);
        assert!(
            shelf
                .take(&file("a"), &edit, 0, (true, false), None)
                .is_none()
        );

        let mut none = None;
        forget_bases(&mut held, &mut none, None);
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
        shelf.put(file("a"), base(&edit, 0, 32, 24), false);
        shelf.put(file("a"), base(&warmer(), 0, 32, 24), false);
        shelf.put(file("b"), base(&edit, 0, 32, 24), false);
        assert_eq!(shelf.len(), 3);

        let mut after = edit.clone();
        after.light.exposure += 1.0;
        after.noise.learned_strength += 0.25;
        let now = |edits: Vec<Edit>, turn: u8| FileNow { edits, turn };
        assert_eq!(
            shelf.drop_changed(
                Path::new("a"),
                Some(&now(vec![after.clone(), warmer()], 4)),
                None
            ),
            0,
            "the main line's base and the version's both still taken"
        );
        assert_eq!(
            shelf.drop_changed(Path::new("a"), Some(&now(vec![after.clone()], 0)), None),
            1
        );
        assert!(
            shelf
                .take(&file("a"), &warmer(), 0, (true, false), None)
                .is_none()
        );
        assert!(shelf.holds(
            (Path::new("a"), &file("a").stat),
            &edit,
            false,
            0,
            true,
            None
        ));
        // Turned by a paste: no open takes the unturned base.
        assert_eq!(
            shelf.drop_changed(Path::new("a"), Some(&now(vec![after], 1)), None),
            1
        );
        shelf.put(file("a"), base(&edit, 0, 32, 24), false);
        assert_eq!(shelf.drop_changed(Path::new("a"), None, None), 1);
        assert_eq!(shelf.len(), 1);
        assert!(
            shelf
                .take(&file("b"), &edit, 0, (true, false), None)
                .is_some(),
            "b's own"
        );
    }

    /// The learned pair goes on the shelf with a base blended from it,
    /// counted once where the base is one of the pair, and comes back
    /// with it. A base standing in for a missing denoiser is shelved,
    /// and taken back only while the denoiser is still not to be had: a
    /// walk with the model would run it. Shelved beside it, the learned
    /// base of the same file under the same edit replaces it, and the
    /// stand-in made again replaces that. One that stood in with the
    /// model there (the card short, a run failed), and a picture's, are
    /// not shelved.
    #[test]
    fn the_learned_pair_is_shelved_with_its_base_and_a_stand_in_while_it_stands_in() {
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        // At the end of the strength the base is the network's picture.
        let b = paired(1.0);
        let one = b.image.data.len() as u64 * 4;
        let guide = b.bytes() - b.kept.bytes();
        assert_eq!(b.kept.bytes(), 2 * one, "the pair, the base one of it");
        shelf.put(file("a"), b, false);
        assert_eq!(shelf.bytes(), 2 * one + guide);
        let b = shelf
            .take(&file("a"), &learned(1.0), 0, (true, false), None)
            .unwrap();
        assert!(b.kept.holds(Keep::Pair), "the pair came back");

        let stand_in = base(&learned(0.5), 0, 32, 24);
        assert!(stand_in.meta.stood_in.is_some());
        let image = stand_in.image.clone();
        assert!(shelf.put(file("s"), stand_in, true));
        let now = |shelf: &mut Shelf, stands_in| {
            shelf.take(&file("s"), &learned(0.5), 0, (true, stands_in), None)
        };
        assert!(now(&mut shelf, false).is_none(), "the model is there now");
        let b = now(&mut shelf, true).expect("still not to be had");
        assert!(Arc::ptr_eq(&b.image, &image));
        assert!(b.kept.holds(Keep::Base));

        // The model came: its base replaces the stand-in's, and the
        // stand-in made again when it went replaces that.
        assert!(shelf.put(file("s"), b, true));
        assert!(shelf.put(file("s"), paired(0.5), false));
        assert_eq!(shelf.len(), 1, "one base of the file under the edit");
        assert!(now(&mut shelf, true).is_none());
        let learned_base = now(&mut shelf, false).expect("the learned base");
        assert!(learned_base.meta.stood_in.is_none());
        assert!(shelf.put(file("s"), learned_base, false));
        assert!(shelf.put(file("s"), base(&learned(0.5), 0, 32, 24), true));
        assert_eq!(shelf.len(), 1);
        assert!(now(&mut shelf, true).is_some());

        // A stand-in with the model there (the card short, or a run
        // that failed) is not shelved: the walk back runs the model,
        // so it would only hold the budget. Nor is a missing model's
        // once the model has come.
        for why in [
            NoLearned::Short("short".into()),
            NoLearned::Failed("failed".into()),
        ] {
            let mut stood = base(&learned(0.5), 0, 32, 24);
            stood.meta.stood_in = Some(why);
            assert!(!shelf.put(file("t"), stood, false));
        }
        assert!(!shelf.put(file("t"), base(&learned(0.5), 0, 32, 24), false));
        assert_eq!(shelf.len(), 0);
        let picture = FrameKept::walked_with(
            &crate::walk::Input::Picture {
                image: Arc::new(WorkingImage::new(32, 24)),
                meta: Arc::new(greycard_core::picture::Picture {
                    image: WorkingImage::new(0, 0),
                    make: String::new(),
                    model: String::new(),
                    shot: Default::default(),
                    metadata: Default::default(),
                    space: "sRGB".into(),
                    bits: 16,
                    white: greycard_core::illuminant::D65,
                }),
            },
            &Edit::for_picture(),
            0,
            &mut crate::walk::NoModels,
        );
        assert!(!shelf.put(file("p.tif"), picture, false));
        assert_eq!(shelf.len(), 0);
    }

    /// The refined masks' picture goes on the shelf with its base,
    /// counted, and serves the frame's edit again when it comes back.
    #[test]
    fn the_refined_masks_picture_travels_with_its_base() {
        let mut edit = Edit::default();
        edit.adjustments.push(greycard_edit::Adjustment {
            id: 3,
            mask: greycard_edit::mask::Mask {
                components: vec![greycard_edit::mask::Component::default()],
                refine: 0.2,
                ..Default::default()
            },
            ..Default::default()
        });
        // The walk on the CPU tapped it; the develop keeps it beside
        // the base as well, counted once.
        let mut b = base(&edit, 0, 32, 24);
        let picture = b.kept.mask_picture().expect("tapped").clone();
        let with = b.bytes();
        let size = picture.bytes();
        assert!(size > 0);
        b.mask_picture = Some(super::super::KeptMaskPicture {
            patched: b.patched_picture(),
            picture: picture.clone(),
        });
        assert_eq!(b.bytes(), with, "counted once");
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a"), b, false);
        assert_eq!(shelf.bytes(), with, "still counted, with the base");
        let b = shelf
            .take(&file("a"), &edit, 0, (true, false), None)
            .unwrap();
        let kept = b.mask_picture.as_ref().expect("kept").picture.clone();
        assert!(Arc::ptr_eq(&kept, &picture));
    }

    /// A retouch's patched picture stays with its base and counts.
    #[test]
    fn the_patched_picture_stays_with_its_base() {
        let mut edit = Edit::default();
        edit.retouch.patches.push(greycard_edit::retouch::Patch {
            id: 3,
            points: vec![[0.5, 0.5]],
            radius: 0.1,
            ..Default::default()
        });
        let b = base(&edit, 0, 32, 24);
        let patched = b.kept.patched().expect("a retouch").image.clone();
        let one = b.image.data.len() as u64 * 4;
        let guide = b.bytes() - b.kept.bytes();
        let mut shelf = Shelf::new(Budget::Bytes(1 << 30));
        shelf.put(file("a"), b, false);
        assert_eq!(shelf.bytes(), 2 * one + guide);
        let b = shelf
            .take(&file("a"), &edit, 0, (true, false), None)
            .unwrap();
        assert!(Arc::ptr_eq(b.kept.patched().unwrap().image, &patched));
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

    const G: u64 = 1 << 30;

    /// The bases of a frame of `pixels` a shelf on a machine of `ram`
    /// keeps beside it, its develop where `gpu` says under an edit that
    /// `uses` so much; and, before this sizing, under the session's
    /// budget alone.
    fn kept(ram: u64, pixels: u64, gpu: Gpu, uses: Uses) -> (u64, u64) {
        let base = pixels * 12;
        let today = Shelf::new(Budget::of(ram)).room_for(base);
        let mut shelf = Shelf::on(Budget::of(ram), Machine::of(ram));
        shelf.size_for(pixels, gpu, uses);
        (shelf.room_for(base), today)
    }

    /// A frame that leaves the machine room keeps as many bases as the
    /// session's budget alone kept, on every kind of GPU: 24 MP on
    /// 16 GB, 45 MP on 32 GB, 102 MP on 64 GB beside a 16 GB card, and
    /// smaller frames on larger machines. 102 MP on the owner's 24 GB
    /// of unified memory keeps three under the default edit, and one
    /// with Clarity and Dehaze on; smaller unified machines keep fewer
    /// for large frames. The peak assertion is a guard over the
    /// constants, not a measurement: what a prefetch beside a full shelf
    /// holds by the account (what the editor holds besides its pictures,
    /// the open frame, the making, and as many bases as the shelf keeps,
    /// one of them the new one, the oldest let go first).
    #[test]
    fn the_budget_per_machine_and_frame() {
        const MP_24: u64 = 6000 * 4000;
        const MP_45: u64 = 8192 * 5464;
        const MP_61: u64 = 9504 * 6336;
        const MP_102: u64 = 8736 * 11648;
        let all = [Gpu::Unified, Gpu::Discrete, Gpu::None];
        let roomy = [
            (16 * G, MP_24, all.as_slice()),
            (32 * G, MP_24, &all),
            (32 * G, MP_45, &all),
            (64 * G, MP_45, &all),
            (64 * G, MP_102, &[Gpu::Discrete, Gpu::None]),
            (128 * G, MP_102, &all),
        ];
        for (ram, pixels, gpus) in roomy {
            for &gpu in gpus {
                let (now, today) = kept(ram, pixels, gpu, Uses::NONE);
                assert_eq!(now, today, "{} GiB, {pixels} px, {gpu:?}", ram / G);
                assert!(today > 0);
            }
        }
        let detail = Uses {
            local_contrast: true,
            dehaze: true,
            ..Uses::NONE
        };
        // The owner's case: 102 MP on a 24 GB Mac.
        assert_eq!(kept(24 * G, MP_102, Gpu::Unified, Uses::NONE), (3, 3));
        assert_eq!(kept(24 * G, MP_102, Gpu::Unified, detail), (1, 3));
        assert_eq!(kept(24 * G, MP_102, Gpu::Discrete, detail), (3, 3));
        // On the CPU the Detail section keeps nothing more in memory
        // ([`the_cpu_keeps_no_more_for_the_detail_section`]).
        assert_eq!(
            kept(24 * G, MP_102, Gpu::None, detail),
            kept(24 * G, MP_102, Gpu::None, Uses::NONE)
        );
        // Smaller machines on unified memory.
        assert_eq!(kept(16 * G, MP_102, Gpu::Unified, Uses::NONE), (0, 2));
        assert_eq!(kept(16 * G, MP_61, Gpu::Unified, Uses::NONE), (4, 4));
        assert_eq!(kept(16 * G, MP_61, Gpu::Unified, detail), (1, 4));
        assert_eq!(kept(8 * G, MP_24, Gpu::Unified, Uses::NONE), (3, 3));
        assert_eq!(kept(8 * G, MP_45, Gpu::Unified, Uses::NONE), (0, 1));
        let (held, room) = COST.charged(MP_102, Gpu::Unified, Uses::NONE);
        let peak = crate::memory::BESIDE + held + room + 3 * MP_102 * 12;
        assert!(peak <= 16 * G, "{:.1} GiB", peak as f64 / G as f64);
        // A machine whose memory is not known keeps one; a shelf with no
        // machine keeps its budget whatever the frame.
        let mut one = Shelf::new(Budget::One);
        assert_eq!(one.size_for(MP_102, Gpu::Unified, Uses::NONE), 0);
        assert_eq!(one.room_for(MP_102 * 12), 1);
        let mut set = Shelf::new(Budget::Bytes(4 * G));
        set.size_for(MP_102, Gpu::Unified, Uses::NONE);
        assert_eq!(set.room_for(MP_102 * 12), 3);
    }

    /// On the CPU the window keeps the base and its developed picture,
    /// three floats a pixel each ([`super::base::window_steps`] keeps no
    /// picture before the sharpen without a device): what `COST`'s CPU
    /// figure counts, the developed picture its own under the default
    /// sharpen. Clarity and Dehaze make it its own as well, with the
    /// sharpen off or on, and keep nothing more.
    #[test]
    fn the_cpu_keeps_no_more_for_the_detail_section() {
        let frame = Arc::new(super::super::tests::aberrated_frame(200, 150));
        let px = 200 * 150;
        let kept = |clarity: f32, dehaze: f32, sharpen: bool| {
            let mut e = Edit::default();
            e.detail.clarity = clarity;
            e.detail.dehaze = dehaze;
            e.sharpen.enabled = sharpen;
            FrameKept::walked(&frame, &e, 0).kept.bytes()
        };
        let two = 2 * 12 * px;
        assert_eq!(kept(0.0, 0.0, true), two, "the default edit");
        for sharpen in [true, false] {
            assert_eq!(kept(0.3, 0.0, sharpen), two, "Clarity");
            assert_eq!(kept(0.0, 0.3, sharpen), two, "Dehaze");
            assert_eq!(kept(0.3, 0.3, sharpen), two, "both");
        }
        assert_eq!(kept(0.0, 0.0, false), 12 * px, "the base alone");
    }

    /// The budget shrunk below what the shelf holds lets the least
    /// recently left go first; grown again, nothing comes back, and
    /// the next frame left is kept.
    #[test]
    fn a_budget_shrunk_at_an_open_lets_the_oldest_go() {
        let edit = Edit::default();
        // A base as the shelf holds it, its working copies let go.
        let mut measure = Shelf::new(Budget::Bytes(G));
        measure.put(file("m"), base(&edit, 0, 32, 24), false);
        let one = measure.bytes();
        let small = 32 * 24;
        let (held, making) = COST.charged(small, Gpu::None, Uses::NONE);
        let machine = Machine {
            allowance: held + making + 3 * one,
        };
        let mut shelf = Shelf::on(Budget::Bytes(G), machine);
        assert_eq!(shelf.size_for(small, Gpu::None, Uses::NONE), 0);
        for name in ["a", "b", "c"] {
            shelf.put(file(name), base(&edit, 0, 32, 24), false);
        }
        assert_eq!(shelf.fit(), 0);
        // A frame the more by two bases' worth of cost.
        let larger = small + 2 * one / (COST.held_cpu + COST.making_cpu);
        assert_eq!(shelf.size_for(larger, Gpu::None, Uses::NONE), 2);
        let order: Vec<_> = shelf.held.iter().map(|s| s.file.path.clone()).collect();
        assert_eq!(order, [PathBuf::from("c")]);
        assert_eq!(shelf.size_for(small, Gpu::None, Uses::NONE), 0);
        assert_eq!(shelf.len(), 1);
        shelf.put(file("d"), base(&edit, 0, 32, 24), false);
        assert_eq!(shelf.fit(), 0);
        assert_eq!(shelf.len(), 2);
    }

    /// A base to be made asks the machine first: it is made where the
    /// open frame, the shelf, the base, and its making fit; where they
    /// do not, the bases a put would let go anyway go first, the least
    /// recently left first, but never one of a file kept; and where
    /// even that leaves too little, it is not made. A shelf with no
    /// machine always has room.
    #[test]
    fn a_base_to_be_made_asks_for_room_first() {
        let edit = Edit::default();
        let one = base(&edit, 0, 32, 24).bytes();
        let open = 100_000;
        let (held, _) = COST.charged(open, Gpu::Unified, Uses::NONE);
        let machine = Machine {
            allowance: held + 4 * one,
        };
        let mut shelf = Shelf::on(Budget::Bytes(G), machine);
        shelf.size_for(open, Gpu::Unified, Uses::NONE);
        shelf.budget = Budget::Bytes(G);
        let keep = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<_>>();
        // Room for four: one and its making of two fit beside nothing.
        assert_eq!(shelf.room_to_make(&file("n"), (one, 2 * one), &[]), Ok(0));
        shelf.put(file("a"), base(&edit, 0, 32, 24), false);
        shelf.put(file("b"), base(&edit, 0, 32, 24), false);
        // Beside two, a let go first.
        assert_eq!(
            shelf.room_to_make(&file("n"), (one, 2 * one), &keep(&["a"])),
            Err(Short {
                need: 3 * one,
                room: 2 * one
            }),
            "a is kept"
        );
        assert_eq!(shelf.len(), 2);
        assert_eq!(
            shelf.room_to_make(&file("n"), (one, 2 * one), &keep(&["b"])),
            Ok(1)
        );
        let order: Vec<_> = shelf.held.iter().map(|s| s.file.path.clone()).collect();
        assert_eq!(order, [PathBuf::from("b")]);
        // A file's own base going is no loss to it.
        assert_eq!(
            shelf.room_to_make(&file("b"), (one, 3 * one), &keep(&["b"])),
            Ok(1)
        );
        // More than the whole room is not made, and lets nothing go.
        shelf.put(file("c"), base(&edit, 0, 32, 24), false);
        assert_eq!(
            shelf.room_to_make(&file("n"), (one, 4 * one), &[]),
            Err(Short {
                need: 5 * one,
                room: 3 * one
            })
        );
        assert_eq!(shelf.len(), 1);
        let mut roomy = Shelf::new(Budget::Bytes(G));
        assert_eq!(roomy.room_to_make(&file("n"), (G, G), &[]), Ok(0));
    }
}
