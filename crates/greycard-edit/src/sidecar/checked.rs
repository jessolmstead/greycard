//! The save that asks the disk first: §233's check, made on the local
//! copy. Before a save goes over a frame's sidecar, a stat asks whether
//! the file is still the one this sidecar read or last wrote; when it
//! is not, another writer saved it since. Its file is then taken whole,
//! its meta joined, or kept beside the frame before this one goes over
//! it: never a state that neither side made, and nothing of the other
//! writer's destroyed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Placement, Sidecar};
use crate::{Error, Result, sync};

/// What [`Sidecar::save_checked`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checked {
    /// This sidecar written: the file was the one it knew, or was behind
    /// it.
    Written,
    /// The file went on from what this sidecar knew, and this had
    /// nothing of its own since: the file taken whole, nothing written.
    Took,
    /// One side changed only the meta: the other's lines kept, the meta
    /// of both joined field by field, and the result written.
    Joined,
    /// Both changed the lines, or what changed cannot be told: the other
    /// writer's file kept beside the frame ([`keep_other`]), and this
    /// sidecar written.
    Kept,
}

/// The files a checked save put aside on its way, each where it went:
/// those that would not parse ([`set_aside`]), and other writers' files
/// kept before this one went over them ([`keep_other`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asides {
    pub bad: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
}

/// What a checked save did, and the files it put aside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub checked: Checked,
    pub asides: Asides,
}

/// A checked save that failed: why, and the files it had put aside
/// before it did, which are kept beside their frames all the same.
#[derive(Debug)]
pub struct NotSaved {
    pub error: Error,
    pub asides: Asides,
}

impl From<Error> for NotSaved {
    fn from(error: Error) -> Self {
        Self {
            error,
            asides: Asides::default(),
        }
    }
}

/// A sidecar's file as one stat found it: its path, length, mtime and,
/// on Unix, inode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<std::time::SystemTime>,
    /// The inode, where the platform's metadata has one to hand.
    pub inode: Option<u64>,
}

impl FileStat {
    /// The file at `path`, by a stat of it.
    pub fn of(path: &Path) -> std::io::Result<Self> {
        Ok(Self::from_meta(path, &path.metadata()?))
    }

    /// The file at `path`, from a stat of it already made.
    pub fn from_meta(path: &Path, meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        let inode = Some(std::os::unix::fs::MetadataExt::ino(meta));
        #[cfg(not(unix))]
        let inode = None;
        Self {
            path: path.to_path_buf(),
            len: meta.len(),
            modified: meta.modified().ok(),
            inode,
        }
    }

    /// Whether `now` is taken for the same file: the same path, length
    /// and mtime, and the same inode where both stats have one. Where
    /// there are none (Windows' metadata gives none without opening the
    /// file), a rewrite to the same length within one tick of the
    /// file system's clock is not seen: on NTFS a tick is 100 ns, on FAT
    /// two seconds.
    pub fn same(&self, now: &FileStat) -> bool {
        self.path == now.path
            && self.len == now.len
            && self.modified == now.modified
            && match (self.inode, now.inode) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

/// The bytes of a sidecar's file as read or written: their length and
/// blake3, which tell a file that is exactly the one known, and the
/// lines' print ([`lines_print`]), all made from the bytes in hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bytes {
    len: u64,
    hash: blake3::Hash,
    lines: Option<blake3::Hash>,
}

impl Bytes {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            len: bytes.len() as u64,
            hash: blake3::hash(bytes),
            lines: lines_print(bytes),
        }
    }
}

/// The fields of a sidecar's file that are not its lines: the meta and
/// what goes with it, the turn and the frame's shape written beside it.
/// Every other field is the lines' (the states, the snapshots, the
/// export records, the versions), a field this build does not know
/// among them.
const NOT_LINES: [&str; 12] = [
    "meta",
    "meta_at",
    "turn",
    "shape",
    "xmp",
    "xmp_shared",
    "turn_owed",
    "lightroom",
    "saved",
    "revisions",
    "absorbed",
    "set_aside",
];

/// The hash of a sidecar file's lines, as its bytes spell them: every
/// top-level field but [`NOT_LINES`], key and value byte for byte, in
/// the order written. One pass over the bytes, nothing parsed. Two
/// files with the same print hold the same lines; a file this build
/// would write differently for the same lines (an older build's) has
/// another print, which only ever sends a save to keep the other's
/// file. None when the bytes are not one JSON object.
pub(crate) fn lines_print(bytes: &[u8]) -> Option<blake3::Hash> {
    let mut h = blake3::Hasher::new();
    let ws = |b: &u8| b.is_ascii_whitespace();
    let mut i = bytes.iter().position(|b| !ws(b))?;
    if bytes[i] != b'{' {
        return None;
    }
    i += 1;
    loop {
        i += bytes[i..].iter().position(|b| !ws(b))?;
        match bytes[i] {
            b'}' => return Some(h.finalize()),
            b',' => {
                i += 1;
                continue;
            }
            b'"' => {}
            _ => return None,
        }
        let key_end = i + 1 + string_len(&bytes[i + 1..])?;
        let key = &bytes[i + 1..key_end];
        let value_start = key_end + 1 + bytes[key_end + 1..].iter().position(|&b| b == b':')? + 1;
        let value_end = value_start + value_len(&bytes[value_start..])?;
        if !NOT_LINES.iter().any(|n| n.as_bytes() == key) {
            h.update(&bytes[i..value_end]);
            h.update(b"\n");
        }
        i = value_end;
    }
}

/// The length of a JSON string's body, up to its closing quote, with
/// `bytes` starting after the opening one.
fn string_len(bytes: &[u8]) -> Option<usize> {
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            b'"' => return Some(i),
            _ => {}
        }
    }
    None
}

/// The length of the JSON value `bytes` starts with (whitespace before
/// it included), up to the comma or brace that ends it at its depth.
fn value_len(bytes: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => i += string_len(&bytes[i + 1..])? + 1,
            b'{' | b'[' => depth += 1,
            b'}' | b']' if depth == 0 => return Some(i),
            b'}' | b']' => depth -= 1,
            b',' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// What a sidecar last read or wrote, in a few words: the file as a
/// stat found it (none when there was no file), its bytes' length and
/// hash, and of what it held only what a save that finds the file
/// changed asks: the lines' hash, the meta's, the turn, and the latest
/// revision.
#[derive(Clone, Debug)]
pub struct OnDisk {
    pub stat: Option<FileStat>,
    bytes: Option<Bytes>,
    known: Arc<Known>,
}

#[derive(Debug)]
struct Known {
    /// [`lines_print`] of the file, none when there is none to tell.
    lines: Option<blake3::Hash>,
    /// [`Sidecar::facts_hash`].
    facts: blake3::Hash,
    turn: u8,
    revision: Option<String>,
    /// No file: the start a frame with none was given.
    made: bool,
}

/// The largest sidecar a save parses to compare with: past anything a
/// real one reaches (the largest seen is 5.4 MB), so a file at the
/// sidecar's path that is not one cannot hold the window at a slider's
/// rest. A file exactly as this sidecar last knew it is told by its
/// bytes first, whatever its size.
pub const READ_MOST: u64 = 256 << 20;

/// How many other writers' files are kept beside one frame's sidecar
/// ([`keep_other`]): the newest, so two machines saving the same frame
/// in turn keep a few of each other's copies and no more.
pub const KEEP_OTHERS: usize = 4;

#[cfg(test)]
thread_local! {
    /// [`READ_MOST`] for a test, which makes a file past it in bytes
    /// rather than hundreds of megabytes.
    pub(crate) static TEST_READ_MOST: std::cell::Cell<u64> = const { std::cell::Cell::new(READ_MOST) };
    /// What a test does right after a checked save's rename and before
    /// the save remembers its file: another writer's save in between.
    pub(crate) static AFTER_RENAME: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many changed sidecars checked saves have read on this thread: a
/// test's proof that a save found its file as it knew it.
#[cfg(any(test, feature = "testing"))]
pub fn changed_reads() -> u64 {
    READS.with(std::cell::Cell::get)
}

/// The bound a save reads to: [`READ_MOST`], or a test's own.
fn read_most() -> u64 {
    #[cfg(test)]
    return TEST_READ_MOST.get();
    #[cfg(not(test))]
    READ_MOST
}

impl Sidecar {
    /// [`lines_print`] of this sidecar as it would be written: its
    /// lines as they are now, by their content. One serialization, made
    /// only by a save that found the file changed.
    fn lines_now(&self) -> Option<blake3::Hash> {
        lines_print(self.to_json().as_bytes())
    }

    /// The hash of the meta and the turn.
    fn facts_hash(&self) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        serde_json::to_writer(&mut h, &self.meta).expect("the meta serializes");
        h.update(&[self.turn]);
        h.finalize()
    }

    /// This sidecar is what the file at `stat` holds, as just read or
    /// written, whose bytes were `bytes`.
    pub(crate) fn remember_bytes(
        &mut self,
        stat: Option<FileStat>,
        bytes: Option<Bytes>,
        made: bool,
    ) {
        self.on_disk = Some(OnDisk {
            stat,
            bytes,
            known: Arc::new(Known {
                lines: bytes.and_then(|b| b.lines),
                facts: self.facts_hash(),
                turn: self.turn,
                revision: self.revisions.last().map(|r| r.hash.clone()),
                made,
            }),
        });
    }

    /// This sidecar, written as `bytes` to the file `stat` found: what
    /// the next save asks the disk about. The stat is the staged file's,
    /// taken before the rename ([`super::Staged::stat`]), so a write
    /// another writer makes right after ours is never taken for ours.
    pub fn remember_written(&mut self, stat: Option<FileStat>, bytes: Bytes) {
        self.remember_bytes(stat, Some(bytes), false);
    }

    /// This sidecar is what a frame with no file starts from: a save that
    /// finds a file another writer made meanwhile takes it when nothing
    /// was done here, rather than write a start over it.
    pub fn remember_fresh(&mut self) {
        // Its lines as the save writes them, ids given, which they are
        // compared with then; this sidecar is left as it is.
        let mut filled = self.clone();
        filled.fill_ids();
        let lines = lines_print(filled.to_json().as_bytes());
        self.remember_bytes(None, None, true);
        if let Some(k) = &mut self.on_disk {
            Arc::get_mut(&mut k.known).expect("just made").lines = lines;
        }
    }

    /// Whether the file at `path` holds, byte for byte, what this
    /// sidecar last read or wrote (a sync tool copied it back, a stat
    /// was taken wrong): its length first, then its hash.
    pub(crate) fn as_known(&self, path: &Path) -> bool {
        let Some(known) = self.on_disk.as_ref().and_then(|k| k.bytes) else {
            return false;
        };
        path.metadata().is_ok_and(|m| m.len() == known.len)
            && std::fs::read(path).is_ok_and(|bytes| Bytes::of(&bytes) == known)
    }

    /// [`Self::save_in_stamped`] that first asks the disk
    /// ([`Self::changed_on_disk`]: a stat or two, nothing read while the
    /// file is the one this sidecar knows). When another writer saved the
    /// frame's sidecar since (a second window or machine on a shared
    /// folder, a sync tool, the command line), the file is read and
    /// weighed against this one ([`Self::checked_against`]): taken whole,
    /// its meta joined, or kept beside the frame and this written. This
    /// sidecar is what was saved or taken. A file that will not parse
    /// (torn, not JSON, not UTF-8, or a shape this build refuses as no
    /// sidecar of its: [`Error::Json`]) is set aside as `<name>.bad` and
    /// this written; one from a newer build, or past [`READ_MOST`] and not
    /// as this knew it, is never written over, and the save fails.
    ///
    /// What the stat cannot see: another writer's rewrite in place (no
    /// new inode) to exactly the same length within one tick of the
    /// file's mtime, or anything where there is no inode to compare; and
    /// a share that caches attributes (CIFS for a second by default, NFS
    /// for three to sixty) can answer with the attributes of before
    /// another machine's save. The window between the stat and the
    /// rename (milliseconds) is open as the archive write's is (§244).
    pub fn save_checked(
        &mut self,
        raw: &Path,
        placement: Placement,
        stamp: &sync::Stamp,
    ) -> Result<Checked> {
        self.save_checked_said(raw, placement, stamp)
            .map(|s| s.checked)
            .map_err(|e| e.error)
    }

    /// [`Self::save_checked`], with the files it put aside, whether it
    /// saved or not.
    pub fn save_checked_said(
        &mut self,
        raw: &Path,
        placement: Placement,
        stamp: &sync::Stamp,
    ) -> std::result::Result<Saved, NotSaved> {
        self.advance(stamp);
        let (reconciled, asides) = self.reconciled(raw, placement, stamp);
        let (checked, kept) = match reconciled {
            Ok(r) => r,
            Err(error) => {
                // A newer build's file is the frame's now: this copy is
                // read-only too, its edit kept aside (§724).
                if let crate::Error::ReadOnly(floor) = error {
                    self.newer = Some(floor);
                }
                return Err(NotSaved { error, asides });
            }
        };
        if let (Checked::Took, Some(theirs)) = (checked, &kept) {
            *self = theirs.clone();
            return Ok(Saved { checked, asides });
        }
        let mut write = kept.unwrap_or_else(|| self.clone());
        if let Err(error) = write.write_known(raw, placement) {
            return Err(NotSaved { error, asides });
        }
        *self = write;
        Ok(Saved { checked, asides })
    }

    /// The write of a save already advanced and checked: staged, renamed
    /// into place, the copy in the other place removed, and what was
    /// written remembered.
    pub(crate) fn write_known(&mut self, raw: &Path, placement: Placement) -> Result<()> {
        let staged = self.stage_in(raw, placement)?;
        let (bytes, stat) = (staged.bytes(), staged.stat().ok());
        staged.commit()?;
        #[cfg(test)]
        if let Some(then) = AFTER_RENAME.take() {
            then(&Self::path_in(raw, placement));
        }
        Self::drop_other(raw, placement);
        if let Some(bytes) = bytes {
            self.remember_written(stat, bytes);
        }
        Ok(())
    }

    /// This sidecar, advanced for a save to `raw` under `placement`,
    /// weighed against whatever another writer left on disk since it
    /// last read or wrote: what to write (or take), none when this is to
    /// be written as it is; and beside it, the files put aside on the
    /// way, said even when the save then fails. A file in the other
    /// place as well is weighed against what the first made: behind it,
    /// it goes with the save; anything else is kept beside the frame. A
    /// file that will not parse is set aside ([`set_aside`]); one from a
    /// newer build, past [`READ_MOST`], or that will not read, or one
    /// that cannot be kept, is the error.
    #[allow(clippy::type_complexity)]
    pub fn reconciled(
        &self,
        raw: &Path,
        placement: Placement,
        stamp: &sync::Stamp,
    ) -> (Result<(Checked, Option<Sidecar>)>, Asides) {
        use sync::{Compared, Side};
        let mut kept: Option<Sidecar> = None;
        let mut checked = Checked::Written;
        let mut asides = Asides::default();
        for path in self.changed_on_disk(raw, placement) {
            if self.as_known(&path) {
                continue;
            }
            // Started at this copy's start (§724): a line nobody
            // developed on disk is the same frame's.
            let disk = match read_changed(&path, &self.start) {
                // One a newer build made, or with states this build
                // cannot read, is never written over (§724). This
                // copy's edit is kept beside it, as another writer's is
                // ([`keep_ours`]), so it is not lost with the window;
                // unless this copy is read-only too, all it would keep
                // being a newer file as this build reads it.
                Ok(Read::Sidecar(disk)) if disk.read_only() => {
                    let e = disk.writable().expect_err("read-only");
                    if !self.read_only() {
                        match keep_ours(&path, self.to_json().as_bytes()) {
                            Ok(to) => asides.kept.push(to),
                            Err(k) => {
                                log::warn!("{}: this copy not kept aside: {k}", path.display())
                            }
                        }
                    }
                    return (Err(e), asides);
                }
                Ok(Read::Sidecar(disk)) => {
                    // Each line nobody developed as this copy holds it,
                    // its seed kept.
                    let mut disk = *disk;
                    disk.keep_starts_of(self);
                    disk
                }
                Ok(Read::SetAside(to)) => {
                    asides.bad.push(to);
                    continue;
                }
                Err(e) => return (Err(e), asides),
            };
            let (c, k, keep) = match kept.take() {
                None => {
                    let (c, k) = self.checked_against(disk);
                    (c, k, c == Checked::Kept)
                }
                Some(k) => match sync::compare(&k, &disk) {
                    Compared::Same | Compared::Behind(Side::Other) => (checked, k, false),
                    // Kept, and what came of the first file written
                    // (taken, too), so this one goes.
                    _ => (Checked::Kept, k, true),
                },
            };
            if keep {
                match keep_other(&path) {
                    Ok(to) => asides.kept.push(to),
                    Err(e) => return (Err(e.into()), asides),
                }
            }
            checked = c;
            kept = Some(k);
        }
        if checked == Checked::Joined
            && let Some(k) = &mut kept
        {
            k.advance(stamp);
        }
        (Ok((checked, kept)), asides)
    }

    /// This sidecar, advanced for a save, against `disk`, the file a
    /// stat found changed: what to do, and the sidecar to write (not yet
    /// advanced) or to take (the file's). Whatever comes back knows the
    /// file as `disk` was read, so the write that follows is not taken
    /// for another change. What this sidecar last read or wrote (its
    /// lines' hash, its meta's, its turn, its latest revision) is what
    /// both sides are measured from, by content. In order:
    ///
    /// 1. The file is behind this sidecar, or the same: this written.
    /// 2. The file's lines and turn are as known: the other writer
    ///    changed only the meta. This kept, its redo stack with it, the
    ///    meta of both joined field by field by their times, and
    ///    written; unless the join would change this sidecar's turn.
    /// 3. This sidecar's lines and turn are as known, and the file went
    ///    on from what was known (its lineage holds the revision known,
    ///    or it was written over a copy of this one it kept aside, or
    ///    there was no file): nothing of this sidecar's lines is lost
    ///    by taking the file whole, with its own turn and states. Taken,
    ///    with the meta joined into it when the meta here changed too;
    ///    unless that join would change the file's turn.
    /// 4. Anything else: the other writer's file kept beside the frame,
    ///    and this sidecar written, the file's latest revision noted in
    ///    [`Sidecar::set_aside`] (never in the lineage, which the archive
    ///    sync reads: the two are still apart, as they are): the other
    ///    writer, saving next with nothing new, takes this one (3) rather
    ///    than keep it in turn, and the two settle.
    pub fn checked_against(&self, disk: Sidecar) -> (Checked, Sidecar) {
        use sync::{Compared, Side};
        // The file is this frame's, so its states are shown by the
        // shape this sidecar knows (§724).
        let mut disk = disk;
        disk.take_shape(self.shape);
        let read = disk.on_disk.clone();
        let disk_revision = disk.revisions.last().map(|r| r.hash.clone());
        let ours = || {
            let mut ours = self.clone();
            ours.on_disk = read.clone();
            ours
        };
        let kept_over = || {
            let mut kept = ours();
            if let Some(r) = disk_revision
                .clone()
                .filter(|r| !kept.knows(r) && !kept.set_aside.contains(r))
            {
                kept.set_aside.push(r);
                let over = kept.set_aside.len().saturating_sub(sync::ABSORBED);
                kept.set_aside.drain(..over);
            }
            (Checked::Kept, kept)
        };
        if let Compared::Same | Compared::Behind(Side::Other) = sync::compare(self, &disk) {
            return (Checked::Written, ours());
        }
        let Some(known) = self.on_disk.as_ref().map(|k| Arc::clone(&k.known)) else {
            return kept_over();
        };
        let disk_lines = disk
            .on_disk
            .as_ref()
            .and_then(|k| k.bytes)
            .and_then(|b| b.lines);
        if disk.turn == known.turn && disk_lines.is_some() && disk_lines == known.lines {
            if disk.facts_hash() == known.facts {
                return (Checked::Written, ours());
            }
            let mut kept = ours();
            if take_meta(&mut kept, &disk) {
                return (Checked::Joined, kept);
            }
        } else if self.turn == known.turn
            && (known.made
                || known
                    .revision
                    .as_ref()
                    .is_some_and(|r| went_on_from(&disk, r)))
            && known.lines.is_some()
            && self.lines_now() == known.lines
        {
            if self.facts_hash() == known.facts {
                return (Checked::Took, disk);
            }
            let mut theirs = disk;
            if take_meta(&mut theirs, self) {
                return (Checked::Joined, theirs);
            }
        }
        kept_over()
    }

    /// The files a save of this sidecar for `raw` under `placement`
    /// would go over or remove that are not the one this read or last
    /// wrote, as a stat found it then ([`OnDisk::stat`]): the known
    /// file changed, or gone and found in the other place; and a file in
    /// either place that is not the known one (another writer's, which
    /// the save's removal of the copy in the other place would
    /// otherwise delete unread). One stat of the known file, and one of
    /// each place it is not. Empty while there is nothing but the known
    /// file, as known. A known file that is in neither of `raw`'s places
    /// (the frame moved since) is no file known. A stat that fails other
    /// than by the file's absence names the file, whose read then fails
    /// the save rather than let it write blind.
    pub fn changed_on_disk(&self, raw: &Path, placement: Placement) -> Vec<PathBuf> {
        let places = [placement, placement.other()].map(|p| Self::path_in(raw, p));
        let known = self
            .on_disk
            .as_ref()
            .and_then(|k| k.stat.as_ref())
            .filter(|k| places.contains(&k.path));
        let mut changed = Vec::new();
        if let Some(known) = known {
            match FileStat::of(&known.path) {
                Ok(now) if known.same(&now) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => changed.push(known.path.clone()),
            }
        }
        for path in places {
            if known.is_some_and(|k| k.path == path) || changed.contains(&path) {
                continue;
            }
            if path.symlink_metadata().is_ok() {
                changed.push(path);
            }
        }
        changed
    }
}

/// Whether `disk` went on from the revision `known`: its lineage holds
/// it, or it was written over a file of that revision, kept aside
/// ([`Sidecar::set_aside`]). Rule 3's question alone; nothing else reads
/// the marks of files kept aside.
fn went_on_from(disk: &Sidecar, known: &str) -> bool {
    (!disk.revisions.is_empty() && disk.knows(known)) || disk.set_aside.iter().any(|r| r == known)
}

/// `kept` with its meta joined with `other`'s, field by field by the
/// revision and time that set each ([`sync::join`] of the two with their
/// lines left out, so nothing of the lines is put together): the meta,
/// the turn, their times, the lineage and the marks of what was taken
/// from other files (the XMPs, a Lightroom import). Its baseline is the
/// joined meta, so the save that follows stamps nothing the join did
/// not. False, and `kept` as it was, when the join would change its
/// turn: its states are turned for the turn it has.
fn take_meta(kept: &mut Sidecar, other: &Sidecar) -> bool {
    let meta_of = |s: &Sidecar| Sidecar {
        meta: s.meta.clone(),
        turn: s.turn,
        meta_at: s.meta_at.clone(),
        revisions: s.revisions.clone(),
        absorbed: s.absorbed.clone(),
        modified: s.modified,
        saved: s.saved,
        xmp: s.xmp.clone(),
        xmp_shared: s.xmp_shared.clone(),
        lightroom: s.lightroom.clone(),
        ..Sidecar::default()
    };
    let joined = sync::join(&meta_of(kept), &meta_of(other));
    if joined.turn != kept.turn {
        return false;
    }
    kept.meta = joined.meta;
    kept.meta_at = joined.meta_at;
    kept.revisions = joined.revisions;
    kept.absorbed = joined.absorbed;
    kept.xmp = joined.xmp;
    kept.xmp_shared = joined.xmp_shared;
    kept.lightroom = joined.lightroom;
    kept.baseline = Some(kept.facts());
    true
}

/// What reading a changed sidecar gave.
pub enum Read {
    Sidecar(Box<Sidecar>),
    /// It would not parse, and was set aside here.
    SetAside(PathBuf),
}

/// The sidecar at `path`, read to be weighed against a save, or set
/// aside when it will not parse (torn, not JSON, not UTF-8, or refused as
/// a shape no sidecar has, which migration's `refused` says as a JSON
/// error: [`set_aside`]). A file past [`READ_MOST`] is not read, and one
/// from a newer build is not ours to write over: both are the error, as
/// is a read that fails. A line nobody developed is started at `start`,
/// the copy's it is weighed against (§724).
pub fn read_changed(path: &Path, start: &crate::Edit) -> Result<Read> {
    #[cfg(any(test, feature = "testing"))]
    READS.with(|n| n.set(n.get() + 1));
    let len = path.metadata()?.len();
    let most = read_most();
    if len > most {
        return Err(Error::Io(std::io::Error::other(format!(
            "{} is {len} bytes, past the {most} a save reads",
            path.display()
        ))));
    }
    let torn = match Sidecar::read(path, start) {
        Ok(s) => return Ok(Read::Sidecar(Box::new(s))),
        Err(Error::Json(e)) => e.to_string(),
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::InvalidData => e.to_string(),
        Err(e) => return Err(e),
    };
    let aside = set_aside(path)?;
    log::warn!(
        "{}: will not parse ({torn}); kept as {}, and the edit in hand written",
        path.display(),
        aside.display()
    );
    Ok(Read::SetAside(aside))
}

/// The name `<name>.<n>.<ext>` beside `path`, `<name>.<ext>` for the
/// first.
fn beside(path: &Path, n: u32, ext: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if n == 1 {
        path.with_file_name(format!("{name}.{ext}"))
    } else {
        path.with_file_name(format!("{name}.{n}.{ext}"))
    }
}

/// Move a sidecar that will not parse out of the way, beside itself as
/// `<name>.bad`, or `<name>.2.bad` and on when that is taken, so no
/// earlier one is written over: the name it went to.
pub fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    let mut n = 1;
    loop {
        let aside = beside(path, n, "bad");
        if aside.symlink_metadata().is_err() {
            std::fs::rename(path, &aside)?;
            return Ok(aside);
        }
        n += 1;
    }
}

/// The copies of other writers' sidecars kept beside `path`
/// ([`keep_other`]), oldest first: one listing of its folder, each name
/// `<name>.other` (the first) or `<name>.<n>.other` exactly, by number.
pub fn kept_others(path: &Path) -> std::io::Result<Vec<(u32, PathBuf)>> {
    Ok(numbered(path, "other", &names_beside(path)?))
}

/// The names in `path`'s folder.
fn names_beside(path: &Path) -> std::io::Result<Vec<OsString>> {
    std::fs::read_dir(path.parent().unwrap_or(Path::new(".")))?
        .map(|e| e.map(|e| e.file_name()))
        .collect()
}

/// Of `names`, those beside `path` named `<name>.<ext>` or
/// `<name>.<n>.<ext>` exactly ([`beside`]'s names), by number.
fn numbered(path: &Path, ext: &str, names: &[OsString]) -> Vec<(u32, PathBuf)> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dotted = format!(".{ext}");
    let mut found = Vec::new();
    for file in names {
        let Some(rest) = file
            .to_str()
            .and_then(|f| f.strip_prefix(name.as_str()))
            .and_then(|r| r.strip_prefix('.'))
        else {
            continue;
        };
        let n = match rest.strip_suffix(dotted.as_str()) {
            None if rest == ext => 1,
            Some(n) if n.bytes().all(|b| b.is_ascii_digit()) => match n.parse() {
                Ok(n) => n,
                Err(_) => continue,
            },
            _ => continue,
        };
        // Only the names `beside` makes: `.1.other` or `.02.other` are
        // none of ours.
        let at = beside(path, n, ext);
        if at.file_name() == Some(file.as_os_str()) {
            found.push((n, at));
        }
    }
    found.sort();
    found
}

/// What saves put aside beside the sidecar at `path`, of the `names`
/// its folder holds: the other writers' files kept ([`keep_other`]) and
/// the files that would not parse ([`set_aside`]), by their exact
/// names. They are the frame's, and go where its sidecar goes on a
/// move, a rename, or a delete; a copy of the frame elsewhere (the
/// archive) leaves them.
pub fn asides_among(path: &Path, names: &[OsString]) -> Vec<PathBuf> {
    ["other", "bad"]
        .into_iter()
        .flat_map(|ext| numbered(path, ext, names))
        .map(|(_, p)| p)
        .collect()
}

/// [`asides_among`] its folder's names, listed here: none when the
/// folder will not list.
pub fn asides_of(path: &Path) -> Vec<PathBuf> {
    asides_among(path, &names_beside(path).unwrap_or_default())
}

/// Where `aside`, one of the sidecar `from`'s ([`asides_of`]), goes for
/// that sidecar moved to `to`: the same ending after the new name.
pub fn aside_at(aside: &Path, from: &Path, to: &Path) -> PathBuf {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let (aside_name, from_name) = (name(aside), name(from));
    let rest = aside_name.strip_prefix(&from_name).unwrap_or(&aside_name);
    to.with_file_name(format!("{}{rest}", name(to)))
}

/// Keep another writer's sidecar at `path` before a save goes over it:
/// written beside it as `<name>.other`, or `<name>.<n>.other` numbered on
/// from the highest kept ([`kept_others`]), so none is written over and
/// the highest is the newest; written whole and synced before it takes
/// its name, as a sidecar is. A copy already kept with the same bytes is
/// the one named, and nothing is written. Past [`KEEP_OTHERS`], the
/// lowest go, each said in the log. The name it is kept under.
pub fn keep_other(path: &Path) -> std::io::Result<PathBuf> {
    let bytes = std::fs::read(path)?;
    keep_bytes(
        path,
        &bytes,
        "saved elsewhere since it was read; that copy kept as",
    )
}

/// This copy's sidecar, `bytes`, kept beside `path` as another writer's
/// is ([`keep_other`]), the same names, so the user finds both: for a
/// save a newer build's file at `path` refuses (§724).
pub fn keep_ours(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    keep_bytes(
        path,
        bytes,
        "a newer greycard saved it since it was read; this copy's edit kept as",
    )
}

/// [`keep_other`] of `bytes`, `said` in the log before the name.
fn keep_bytes(path: &Path, bytes: &[u8], said: &str) -> std::io::Result<PathBuf> {
    let kept = kept_others(path)?;
    if let Some((_, same)) = kept
        .iter()
        .find(|(_, p)| std::fs::read(p).is_ok_and(|b| b == bytes))
    {
        return Ok(same.clone());
    }
    let next = kept.last().map_or(1, |(n, _)| n.saturating_add(1));
    let to = beside(path, next, "other");
    super::Staged::write(&to, bytes)?.commit()?;
    log::warn!("{}: {said} {}", path.display(), to.display());
    let excess = (kept.len() + 1).saturating_sub(KEEP_OTHERS);
    for (_, old) in kept.into_iter().take(excess) {
        match std::fs::remove_file(&old) {
            Ok(()) => log::info!(
                "{}: the oldest kept copy past {KEEP_OTHERS}, removed",
                old.display()
            ),
            Err(e) => log::warn!("{}: not removed: {e}", old.display()),
        }
    }
    Ok(to)
}
