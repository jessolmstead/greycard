//! The sidecar: what sits beside a frame, where it is kept, and how it
//! is read, written, and settled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::migration::{migrate_line, refused};
use crate::{
    Edit, Exported, Line, Meta, Result, Version, mask, meta, retouch, sync, versions, xmp,
};

/// What sits beside a file: the current edit, the ones before it,
/// the states given a name, and the meta.
///
/// The meta is in the file and outside the edit, both on purpose.
/// In the file, because a rating belongs to the frame and one file
/// beside a frame is the whole library (§72); outside the edit,
/// because the history is the edit's and a star is not a step in
/// developing a picture. Nothing in [`Sidecar::record`],
/// [`Sidecar::undo`], [`Sidecar::redo`] or a snapshot touches it,
/// and setting it is not a state to undo.
///
/// The edit and its history are a [`Line`]: the file's own, version 0,
/// is the `line` field, and each further version in [`Self::versions`]
/// holds one of its own. The meta, the turn, the XMP mark, the save
/// count and the revisions are the file's and shared by every version.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidecar {
    /// Version 0's line, the file's own: its keys at the top level of
    /// the file, first. The sidecar derefs to it, so `sidecar.current`
    /// and the line's methods read and act on version 0.
    #[serde(flatten)]
    pub line: Line,
    /// The rating, the flag, the label, the keywords and the words:
    /// see [`meta`]. Absent from every sidecar written before it
    /// existed, and the default there; left out again when it says
    /// nothing, so developing a frame does not grow an empty block.
    /// Read through [`meta::loose_meta`], so that nothing written
    /// under it can cost the edit beside it.
    #[serde(
        skip_serializing_if = "Meta::is_empty",
        deserialize_with = "meta::loose_meta"
    )]
    pub meta: Meta,
    /// When each field of [`Self::meta`] was last set: see
    /// [`meta::Times`]. Stamped by the save against the meta the file
    /// held when it was read. Left out while nothing has been stamped,
    /// and read loosely.
    #[serde(
        rename = "meta_at",
        default,
        skip_serializing_if = "meta::Times::is_empty",
        deserialize_with = "meta::loose"
    )]
    pub meta_at: meta::Times,
    /// The meta and the turn as the file had them when it was read or
    /// last saved, which is what the save stamps [`Self::meta_at`]
    /// against; none for a sidecar that was never read from a file,
    /// whose every set field is then new. Not written, not part of
    /// equality, and public only so a struct update from
    /// `Sidecar::default()` works outside this crate; nothing else
    /// should set it.
    #[serde(skip)]
    pub baseline: Option<meta::Facts>,
    /// When the file this was read from was last written, seconds
    /// since the Unix epoch, as [`Self::read`] found it: the time a
    /// copy from before revisions is taken to have been saved at
    /// ([`Self::written_at`], [`Self::pin_times`]). Not written, not
    /// part of equality, public as [`Self::baseline`] is.
    #[serde(skip)]
    pub modified: Option<u64>,
    /// Quarter turns clockwise on top of the camera's orientation
    /// tag, 0 to 3: the frames a camera got wrong (shot straight
    /// down, a body with no sensor for it, a scan).
    ///
    /// Beside the edit for the same reason the meta is (§117): it is
    /// a fact about the picture the file got wrong, not a step in
    /// developing one, so undo never takes it back, a preset never
    /// carries one frame's turn onto the next, and a snapshot
    /// restores an exposure without restoring which way up the
    /// picture was. Read loosely and left out when it is 0, so a
    /// sidecar written before it existed loads unchanged and
    /// developing a frame nobody has turned does not grow a field.
    #[serde(
        default,
        skip_serializing_if = "is_no_turn",
        deserialize_with = "meta::loose_turn"
    )]
    pub turn: u8,
    /// What was last taken from an XMP beside the frame, so a load
    /// can ask whether that file has changed rather than compare two
    /// files' clocks: see [`xmp::Adopted`]. Read loosely, as the
    /// meta is, and left out when there is nothing to say.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "meta::loose"
    )]
    pub xmp: Option<xmp::Adopted>,
    /// What was last taken from a culling tool's `IMG.xmp` that the
    /// raw shares with its JPEG and reads but never writes: see
    /// [`xmp::Culled`]. Kept apart from [`Sidecar::xmp`] so the two
    /// files' clocks are never compared. Read loosely, and left out
    /// when there is nothing to say.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "meta::loose"
    )]
    pub xmp_shared: Option<xmp::Culled>,
    /// The meta a Lightroom import laid, at the values it laid: see
    /// [`meta::Laid`]. Left out when empty, and read loosely.
    #[serde(
        default,
        skip_serializing_if = "meta::Laid::is_empty",
        deserialize_with = "meta::loose"
    )]
    pub lightroom: meta::Laid,
    /// How many times this sidecar, or the one it grew out of, has
    /// been written: [`Self::save`] and [`Self::save_in`] increment
    /// it before every write, by a saturating add so a field this
    /// large is never the one thing that fails a save. [`Self::find`]
    /// reads it from both copies when a frame has one in each place,
    /// and takes the higher *only when both are counted* — both
    /// nonzero — since a copy tool, a backup restore or two clocks
    /// can make one file's mtime lie about which save is newer but
    /// cannot make it lie about how many saves it has seen. A copy
    /// with no count, or an equal count, is not decided by the
    /// counter: mtime is the fallback, exactly as before this field
    /// existed. That "no count" case is not only a sidecar from
    /// before this field existed: 0.1.1 does not know this field
    /// either, so a sidecar it saves — even one that started as a
    /// counted copy this build wrote — comes back out with no
    /// `saved` at all, serde dropping what that build does not know,
    /// and must lose to nothing just for having gone through a build
    /// that cannot count. Read loosely, as [`Self::turn`] and
    /// [`Self::meta`] are: anything that is not a plain non-negative
    /// integer reads as 0 rather than failing the whole sidecar.
    #[serde(default, deserialize_with = "meta::loose")]
    pub saved: u64,
    /// Every save of this sidecar, oldest first, the last
    /// [`sync::REVISIONS`] of them: see [`sync::Revision`]. What tells
    /// two copies of one frame's sidecar apart when one has only
    /// undone, or only changed a rating, since neither makes a state
    /// (§233). Each save makes one ([`Self::save_in`]); left out while
    /// there are none, and read loosely, a line at a time.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "sync::loose_revisions"
    )]
    pub revisions: Vec<sync::Revision>,
    /// The latest revisions of the copies a join took in
    /// ([`sync::join`]), the hashes alone, the last [`sync::ABSORBED`]
    /// of them. A copy left stale, its latest revision long since
    /// dropped from [`Self::revisions`] by the cap, is still known to
    /// be behind by this; a save never ages it. Left out while empty,
    /// and read loosely.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "sync::loose_hashes"
    )]
    pub absorbed: Vec<String>,
    /// The frame's further versions, each a line of its own under a
    /// name, in the order they were made. Left out while empty; a list
    /// that is not a list is none, and a version with no id is left out
    /// ([`versions::loose`]).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "versions::loose"
    )]
    pub versions: Vec<Version>,
}

fn is_no_turn(turn: &u8) -> bool {
    turn.rem_euclid(4) == 0
}

/// Everything the file holds, and the redo stack: not the baseline
/// the save stamps the meta's times against, nor the file's time,
/// which are notes about when the struct was read rather than a part
/// of what it says. Every field is named, so a field added later has
/// to be placed here on purpose.
impl PartialEq for Sidecar {
    fn eq(&self, other: &Self) -> bool {
        let Sidecar {
            line,
            meta,
            meta_at,
            baseline: _,
            modified: _,
            turn,
            xmp,
            xmp_shared,
            lightroom,
            saved,
            revisions,
            absorbed,
            versions,
        } = self;
        *line == other.line
            && *meta == other.meta
            && *meta_at == other.meta_at
            && *turn == other.turn
            && *xmp == other.xmp
            && *xmp_shared == other.xmp_shared
            && *lightroom == other.lightroom
            && *saved == other.saved
            && *revisions == other.revisions
            && *absorbed == other.absorbed
            && *versions == other.versions
    }
}

/// Version 0's line is the sidecar's: `sidecar.current`, `.history`,
/// `.record_as(..)` and the rest read and act on it, as they did
/// before there were versions.
impl std::ops::Deref for Sidecar {
    type Target = Line;

    fn deref(&self) -> &Line {
        &self.line
    }
}

impl std::ops::DerefMut for Sidecar {
    fn deref_mut(&mut self) -> &mut Line {
        &mut self.line
    }
}

/// Where a frame's sidecar is kept: beside the frame, which is the
/// default and the rule of §72 and §117, or in a hidden folder
/// inside the frame's own folder, for a tester who finds a `.gcd`
/// between every pair of raws cluttering. Either way the sidecar
/// travels with the shoot's folder; a parallel tree elsewhere would
/// not, and a copied shoot would arrive without its edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// `IMG_0001.CR3.gcd` beside `IMG_0001.CR3`.
    #[default]
    Beside,
    /// `.greycard/IMG_0001.CR3.gcd` under the frame's folder.
    Folder,
}

impl Placement {
    /// The other place.
    pub fn other(self) -> Self {
        match self {
            Self::Beside => Self::Folder,
            Self::Folder => Self::Beside,
        }
    }

    /// The placement `raw`'s sidecar has now, beside when it has
    /// none: for a tool without the setting, which writes a sidecar
    /// back where it found it.
    pub fn of(raw: &Path) -> Self {
        match Sidecar::find(raw) {
            Some(p) => Self::at(&p),
            None => Self::Beside,
        }
    }

    /// The placement of the sidecar at `sidecar`, a path
    /// [`Sidecar::find`] gave.
    pub fn at(sidecar: &Path) -> Self {
        if under_folder(sidecar) {
            Self::Folder
        } else {
            Self::Beside
        }
    }
}

/// Whether a sidecar's path is under the hidden folder.
fn under_folder(sidecar: &Path) -> bool {
    sidecar.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(SIDECAR_FOLDER))
}

/// Just [`Sidecar::saved`], read without building the rest of the
/// file: a struct of one field, so [`Sidecar::find`] does not build
/// the edit, the history and the snapshots — walk them into `Edit`s
/// and `Vec`s, intern the strings, and so on — twice over just to
/// compare two counters. `serde_json` still has to scan every byte
/// of the file to know what to skip, so this is not a second read of
/// less data; it is a read of the same data that stops at parsing
/// instead of going on to build. Anything that keeps it from reading
/// as a number — the file gone, not JSON, an object with no such
/// field, a sidecar from before the field existed — reads as 0, which
/// is exactly what [`Sidecar::find`] wants: an uncounted copy that
/// mtime, not the counter, decides between.
#[derive(Deserialize, Default)]
struct SavedCount {
    #[serde(default)]
    saved: u64,
}

fn saved_count(path: &Path) -> u64 {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SavedCount>(&bytes).ok())
        .map_or(0, |s| s.saved)
}

/// The hidden folder's name under a shoot's folder.
pub const SIDECAR_FOLDER: &str = ".greycard";

impl Sidecar {
    /// The sidecar's path for a RAW: the file's name with `.gcd` on the end,
    /// so `IMG_0001.CR3` keeps its edit in `IMG_0001.CR3.gcd` (JSON inside).
    /// The `Beside` placement; `path_in` is the general form.
    pub fn path_for(raw: &Path) -> PathBuf {
        Self::path_in(raw, Placement::Beside)
    }

    /// Where `raw`'s sidecar goes under `placement`: beside it, or
    /// under the `.greycard` folder in its own folder, with the same
    /// name either way.
    pub fn path_in(raw: &Path, placement: Placement) -> PathBuf {
        let mut name = raw
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".gcd");
        match placement {
            Placement::Beside => raw.with_file_name(name),
            Placement::Folder => raw.with_file_name(SIDECAR_FOLDER).join(name),
        }
    }

    /// The sidecar `raw` has, wherever it is: both places are read
    /// whatever the setting says, so flipping the setting never
    /// loses an edit. When both are there and *both* carry a nonzero
    /// [`Self::saved`], the higher one wins; a copy tool, a backup
    /// restore or two clocks can leave the older save with the newer
    /// mtime, but not with the higher count. A pair where either side
    /// reads 0 is not decided by the counter at all, even when the
    /// other side counts higher: 0 means either a sidecar from before
    /// this field existed, or one a build that does not know the
    /// field — 0.1.1, say — saved and so dropped it from, and neither
    /// is "no saves", so the count cannot be trusted to mean anything
    /// for that pair. mtime is the fallback for every such pair,
    /// beside on a tie there too, exactly the rule before this field
    /// existed. The counters are read only when both places have a
    /// file, so the common one-copy case costs nothing beyond the
    /// metadata call it already made.
    pub fn find(raw: &Path) -> Option<PathBuf> {
        let modified = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
        let (beside, under) = (
            Self::path_in(raw, Placement::Beside),
            Self::path_in(raw, Placement::Folder),
        );
        match (modified(&beside), modified(&under)) {
            (None, None) => None,
            (Some(_), None) => Some(beside),
            (None, Some(_)) => Some(under),
            (Some(b), Some(u)) => {
                let (bc, uc) = (saved_count(&beside), saved_count(&under));
                if bc > 0 && uc > 0 && bc != uc {
                    Some(if bc > uc { beside } else { under })
                } else if u > b {
                    Some(under)
                } else {
                    Some(beside)
                }
            }
        }
    }

    /// Which of two copies of one frame's sidecar was saved later, by
    /// the rule [`Self::find`] uses between the two places: the counter
    /// when both carry one and they differ, the mtime otherwise.
    /// `Greater` when `a` is the newer, `Equal` when neither rule can
    /// tell them apart (equal counts or none, and the same mtime). For
    /// two copies in different places altogether: a frame here and its
    /// copy on an archive (notes §197, §216).
    pub fn compare_copies(a: &Path, b: &Path) -> std::cmp::Ordering {
        let (ac, bc) = (saved_count(a), saved_count(b));
        if ac > 0 && bc > 0 && ac != bc {
            return ac.cmp(&bc);
        }
        let modified = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
        modified(a).cmp(&modified(b))
    }

    /// The sidecar `raw` has, if there is one, wherever it is.
    pub fn load(raw: &Path) -> Result<Option<Self>> {
        match Self::find(raw) {
            Some(path) => Self::read(&path).map(Some),
            None => Ok(None),
        }
    }

    /// The sidecar at `path`, the file itself and not the frame's:
    /// for a copy found elsewhere, on an archive. Every state of every
    /// version is brought up to the current schema; a state with no id
    /// gets one, the first read of a sidecar from before ids being the
    /// one time an id is computed from a file; a meta field with no
    /// time gets the file's ([`Self::pin_times`]); and the meta and the
    /// turn as read are kept to stamp their times against at the next
    /// save.
    pub fn read(path: &Path) -> Result<Self> {
        let modified = path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let json = std::fs::read_to_string(path)?;
        let mut sidecar = Self::from_json(&json)?;
        sidecar.modified = modified;
        sidecar.fill_ids();
        sidecar.pin_times();
        sidecar.baseline = Some(sidecar.facts());
        Ok(sidecar)
    }

    /// The sidecar `json` spells, every edit of every line brought up
    /// to the current schema first, and the whole refused
    /// ([`Error::Newer`]) when any edit is from a schema past it.
    ///
    /// [`Error::Newer`]: crate::Error::Newer
    pub fn from_json(json: &str) -> Result<Self> {
        let mut value: serde_json::Value = serde_json::from_str(json)?;
        if let Some(obj) = value.as_object_mut() {
            migrate_line(obj)?;
            if let Some(serde_json::Value::Array(versions)) = obj.get_mut("versions") {
                for version in versions.iter_mut() {
                    // A line that is not an object would be read as a
                    // list of its fields, its edits unmigrated (see
                    // `migrate`). An entry with no id, an entry that is
                    // not an object among them, is left out by
                    // `versions::loose` and is passed over here too.
                    let has_id = version
                        .get("id")
                        .and_then(|i| i.as_str())
                        .is_some_and(|i| !i.trim().is_empty());
                    if !has_id {
                        continue;
                    }
                    match version.get_mut("line") {
                        Some(serde_json::Value::Object(line)) => migrate_line(line)?,
                        Some(_) => return Err(refused("a version's line is an object")),
                        None => {}
                    }
                }
            }
        }
        Ok(serde_json::from_value(value)?)
    }
}

/// What holds for the whole file, every version's line at once: a turn,
/// a look renamed, the frame's shape, the repair sources. A method here
/// that a [`Line`] also has (`take_sources`, `rename_look`, `fill_ids`)
/// is the line's applied to every line, and is what a call on a
/// sidecar reaches.
impl Sidecar {
    // Version 0's steps, said again here so that a call on a sidecar
    // borrows the sidecar and not its line: `s.record_export(&s.current
    // .clone(), ..)` reads the sidecar while the call's borrow waits,
    // which a borrow taken through the deref does not allow.

    /// [`Line::record`] on version 0.
    pub fn record(&mut self, edit: Edit) -> bool {
        self.line.record(edit)
    }

    /// [`Line::record_as`] on version 0.
    pub fn record_as(&mut self, edit: Edit, label: Option<String>) -> bool {
        self.line.record_as(edit, label)
    }

    /// [`Line::record_export`] on version 0.
    pub fn record_export(&mut self, edit: &Edit, exported: Exported) -> bool {
        self.line.record_export(edit, exported)
    }

    /// [`Line::go_to`] on version 0.
    pub fn go_to(&mut self, index: usize) -> bool {
        self.line.go_to(index)
    }

    /// [`Line::take_snapshot`] on version 0.
    pub fn take_snapshot(&mut self, name: impl Into<String>, taken: u64) -> usize {
        self.line.take_snapshot(name, taken)
    }

    /// [`Line::restore_snapshot`] on version 0.
    pub fn restore_snapshot(&mut self, index: usize) -> bool {
        self.line.restore_snapshot(index)
    }

    /// [`Line::undo`] on version 0.
    pub fn undo(&mut self) -> bool {
        self.line.undo()
    }

    /// [`Line::redo`] on version 0.
    pub fn redo(&mut self) -> bool {
        self.line.redo()
    }

    /// Version 0's line and every version's, version 0 first.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        std::iter::once(&self.line).chain(self.versions.iter().map(|v| &v.line))
    }

    /// [`Self::lines`], to change in place.
    pub fn lines_mut(&mut self) -> impl Iterator<Item = &mut Line> {
        std::iter::once(&mut self.line).chain(self.versions.iter_mut().map(|v| &mut v.line))
    }

    /// Every edit of every line: see [`Line::edits`].
    pub fn edits(&self) -> impl Iterator<Item = &Edit> {
        self.lines().flat_map(Line::edits)
    }

    fn edits_mut(&mut self) -> impl Iterator<Item = &mut Edit> {
        self.lines_mut().flat_map(Line::edits_mut)
    }

    /// Whether any state of any version is still waiting on the
    /// frame's shape: see [`Edit::needs_frame`]. A sidecar that answers
    /// yes reads and renders, and saves back as the version 3 it is,
    /// but its crop must not be refitted until the frame is known.
    pub fn needs_frame(&self) -> bool {
        self.edits().any(Edit::needs_frame)
    }

    /// Finish version 4's step on every state of every version, now the
    /// frame's shape is known. `source_portrait` is as
    /// [`Edit::migrate_with_frame`] has it, and includes this
    /// sidecar's own [`Self::turn`], since that is already in the
    /// developed picture. A no-op when nothing was waiting.
    pub fn migrate_with_frame(&mut self, source_portrait: bool) {
        for edit in self.edits_mut() {
            edit.migrate_with_frame(source_portrait);
        }
    }

    /// [`Line::take_sources`] on every version's line; true when any
    /// changed.
    pub fn take_sources(&mut self, done: &[retouch::Patch]) -> bool {
        let mut took = false;
        for line in self.lines_mut() {
            took |= line.take_sources(done);
        }
        took
    }

    /// Turn the frame `quarters` more quarter turns clockwise, and
    /// carry every state here round with it: the crop, the keystone
    /// and a held aspect, and the masks and the repair patches when
    /// `aspect` says what shape the picture is (see [`Edit::turn`]).
    ///
    /// *Every* state, not only the current one, and of every version,
    /// the turn being the file's. A turn is not a step in developing
    /// the picture — no state is recorded for it, and an undo that
    /// took the crop back but left the frame on its side would be
    /// worse than one that does neither — and that promise only holds
    /// if what undo, redo and a snapshot hand back means the same
    /// pixels the current state does. So the history, the redo stack
    /// and the snapshots take the same map.
    ///
    /// `aspect` is the picture's width over its height *before* the
    /// turn, and is `None` when the caller cannot say. The geometry
    /// turns either way; anything placed on the picture is left
    /// where it is, which [`Sidecar::placed`] warns the caller
    /// about.
    pub fn turn_by(&mut self, quarters: i32, aspect: Option<f32>) {
        if quarters.rem_euclid(4) == 0 {
            return;
        }
        self.turn = (i32::from(self.turn) + quarters).rem_euclid(4) as u8;
        let positions = aspect.map(|a| mask::Turned::new(quarters, a));
        for edit in self.edits_mut() {
            edit.turn(quarters, positions);
        }
    }

    /// Whether any state of any version has work placed on the picture
    /// itself; see [`Edit::placed`].
    pub fn placed(&self) -> bool {
        self.edits().any(Edit::placed)
    }

    /// Whether any state of any version names the look `name`: the
    /// current one, the history, what was undone, or a snapshot.
    pub fn names_look(&self, name: &str) -> bool {
        self.edits().any(|e| e.look_lut.names(name))
    }

    /// Every look any state of any version names, `none` aside.
    pub fn looks_named(&self) -> std::collections::BTreeSet<String> {
        self.edits()
            .filter(|e| !e.look_lut.lut.is_none())
            .map(|e| e.look_lut.lut.name().to_string())
            .collect()
    }

    /// [`Line::rename_look`] on every version's line; true when any
    /// state changed.
    pub fn rename_look(&mut self, from: &str, to: &str) -> bool {
        let mut renamed = false;
        for line in self.lines_mut() {
            renamed |= line.rename_look(from, to);
        }
        renamed
    }

    /// The hidden folder under `shoot`, made and hidden: every time
    /// and not only the first, so a folder unhidden by hand, or
    /// copied to a disk that dropped the attribute, goes hidden
    /// again. The edit matters more than the attribute, so a failure
    /// to hide is said and not returned.
    pub fn folder_under(shoot: &Path) -> std::io::Result<PathBuf> {
        let folder = shoot.join(SIDECAR_FOLDER);
        std::fs::create_dir_all(&folder)?;
        if let Err(e) = hide_folder(&folder) {
            log::warn!("{}: not hidden: {e}", folder.display());
        }
        Ok(folder)
    }

    /// Write the sidecar beside `raw`, whole, through a temporary file.
    pub fn save(&mut self, raw: &Path) -> Result<()> {
        self.save_in(raw, Placement::Beside)
    }

    /// Write the sidecar where `placement` says, whole, through a
    /// temporary file; the hidden folder made, and hidden, when it
    /// is not there yet. A copy in the other place goes: what was
    /// loaded was the newer of the two and what is written came from
    /// it, so the other is superseded either way, and a frame keeps
    /// one sidecar after a save. That is how a folder migrates when
    /// the setting flips, a frame at its next edit; the rest wait for
    /// a move asked for by name.
    ///
    /// [`Self::saved`] is incremented before the write, so the copy
    /// that lands is always the one [`Self::find`] will prefer over
    /// whatever it replaces. The save is stamped with this machine
    /// and the time: see [`Self::save_in_stamped`].
    pub fn save_in(&mut self, raw: &Path, placement: Placement) -> Result<()> {
        self.save_in_stamped(raw, placement, &sync::Stamp::now())
    }

    /// [`Self::save_in`] as made at `stamp`'s time on `stamp`'s host:
    /// every state has its id, the save is a revision
    /// ([`Self::revisions`]) of that time and host, and every field of
    /// the meta and the turn that differs from the file as read is
    /// stamped with the time and that revision ([`Self::meta_at`]), so
    /// the copy written can be told from the one it replaces and from
    /// a copy elsewhere (§233).
    pub fn save_in_stamped(
        &mut self,
        raw: &Path,
        placement: Placement,
        stamp: &sync::Stamp,
    ) -> Result<()> {
        self.advance(stamp);
        self.stage_in(raw, placement)?.commit()?;
        Self::drop_other(raw, placement);
        Ok(())
    }

    /// The first half of a save already advanced ([`Self::advance`]):
    /// the file written and synced under a temporary name where
    /// `placement` puts it, the hidden folder made when it is not there
    /// yet, and nothing renamed. A batch stages every frame's before it
    /// commits any; [`Self::drop_other`] follows each commit.
    pub fn stage_in(&self, raw: &Path, placement: Placement) -> Result<Staged> {
        let path = Self::path_in(raw, placement);
        if placement == Placement::Folder
            && let Some(folder) = path.parent()
        {
            Self::folder_under(folder.parent().unwrap_or(Path::new("")))?;
        }
        Ok(Staged::write(&path, self.to_json().as_bytes())?)
    }

    /// The copy in the place `placement` does not name removed, after a
    /// save to the one it does: superseded either way (see
    /// [`Self::save_in`]). A failure is said and not returned.
    pub fn drop_other(raw: &Path, placement: Placement) {
        let other = Self::path_in(raw, placement.other());
        if other.exists()
            && let Err(e) = std::fs::remove_file(&other)
        {
            log::warn!("{}: superseded but not removed: {e}", other.display());
        }
    }

    /// What a save does to the struct before the file is written: the
    /// save count up by one, every state given its id, and this save's
    /// revision made at `stamp`, the meta's fields that changed stamped
    /// with it. For a save whose file is written elsewhere, on another
    /// thread or to another place ([`Self::write_to`]): the editor's
    /// write to an archive's copy (§233).
    pub fn advance(&mut self, stamp: &sync::Stamp) {
        self.saved = self.saved.saturating_add(1);
        self.fill_ids();
        self.revise(stamp);
    }

    /// The sidecar as it is written: pretty-printed JSON, a list of
    /// numbers kept on one line ([`crate::layout`]). Only the file
    /// takes this form; an edit's own JSON, which keys develop, does
    /// not.
    pub fn to_json(&self) -> String {
        crate::layout::lay_out(&serde_json::to_string(self).expect("a sidecar serializes"))
    }

    /// Write the sidecar as it is to `path`, through a temporary file
    /// beside it, with no new revision and nothing stamped: the same
    /// file again in another place, a copy of one frame on an archive
    /// (§233). [`Self::save_in`] is the save; this is only the write.
    pub fn write_to(&self, path: &Path) -> Result<()> {
        // A copy's hidden folder may not be there yet: the archive's
        // shoot was copied before the folder was, or made by another
        // tool. Made and hidden as a save makes its own.
        if under_folder(path)
            && let Some(shoot) = path.parent().and_then(Path::parent)
        {
            Self::folder_under(shoot)?;
        }
        Ok(replace_synced(path, self.to_json().as_bytes())?)
    }

    /// Whether `raw` has a sidecar in the place `placement` does not
    /// name: one a [`Sidecar::settle`] would move or remove.
    pub fn misplaced(raw: &Path, placement: Placement) -> bool {
        Self::path_in(raw, placement.other()).exists()
    }

    /// Put `raw`'s sidecar where `placement` says, without reading or
    /// rewriting it: the move a folder asks for by name when the
    /// setting has flipped, rather than waiting for each frame's next
    /// save. The copy in the other place is renamed into this one
    /// when it is the one [`Sidecar::find`] would pick — by
    /// [`Sidecar::saved`], falling back to mtime the same way — and
    /// removed when it is not, since a save would have superseded it
    /// the same way. The hidden folder is made, and hidden, first.
    pub fn settle(raw: &Path, placement: Placement) -> std::io::Result<Settled> {
        let (here, other) = (
            Self::path_in(raw, placement),
            Self::path_in(raw, placement.other()),
        );
        if !other.exists() {
            return Ok(Settled::InPlace);
        }
        if Self::find(raw).as_deref() == Some(other.as_path()) {
            if placement == Placement::Folder {
                let shoot = raw.parent().unwrap_or(Path::new(""));
                Self::folder_under(shoot)?;
            }
            greycard_core::fs::rename_into_place(&other, &here)?;
            Ok(Settled::Moved)
        } else {
            std::fs::remove_file(&other)?;
            Ok(Settled::Dropped)
        }
    }
}

/// What [`Sidecar::settle`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// Nothing was in the other place.
    InPlace,
    /// The copy in the other place, the one read, was renamed into
    /// this one.
    Moved,
    /// The copy in the other place was the older of two and was
    /// removed; the one read was already in place.
    Dropped,
}

/// Replace the file at `path` with `bytes`, whole: written and synced
/// under a temporary name beside it, then renamed over it
/// ([`Staged`]). A crash leaves the old file or the new one, never an
/// empty one, and a failure leaves the old one and no temporary file.
/// The sidecars are written so, and the rejects queue.
pub fn replace_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    Staged::write(path, bytes)?.commit()
}

/// A file written whole and synced under a temporary name beside the
/// one it is to replace, and not yet renamed over it: the first half
/// of [`replace_synced`], for a batch that writes and syncs every file
/// before it renames any. Dropped uncommitted, the temporary file goes.
#[derive(Debug)]
pub struct Staged {
    temp: PathBuf,
    path: PathBuf,
    done: bool,
}

impl Staged {
    /// `bytes` written to a temporary file beside `path` and synced
    /// ([`write_synced`]). The temporary name is this write's own, so
    /// two writes to one path at once (a write given up and going on
    /// alone, and the next one) never share a file and rename a torn
    /// one into place.
    pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<Self> {
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temp = path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id()));
        write_synced(&temp, bytes)?;
        Ok(Self {
            temp,
            path: path.to_path_buf(),
            done: false,
        })
    }

    /// The file this replaces.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The temporary file, until it is renamed.
    pub fn temp(&self) -> &Path {
        &self.temp
    }

    /// Rename the temporary file over the one it replaces, waiting out
    /// a moment's hold on it ([`greycard_core::fs::rename_into_place`]).
    /// On a failure the old file is as it was and the temporary one
    /// goes.
    pub fn commit(mut self) -> std::io::Result<()> {
        greycard_core::fs::rename_into_place(&self.temp, &self.path)?;
        self.done = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

#[cfg(feature = "testing")]
thread_local! {
    static SYNCED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many writes [`write_synced`] has made on this thread: a test's
/// proof that a file went through it.
#[cfg(feature = "testing")]
pub fn synced_writes() -> u64 {
    SYNCED.with(std::cell::Cell::get)
}

/// Write `bytes` to a new file at `path` and wait until they are on
/// the disk: the temporary half of a write renamed into place, which
/// must be whole before it takes the old file's name, or a crash
/// before the filesystem flushes could keep the rename and lose the
/// data. The contents are what must land (`sync_data`); the directory
/// entry goes with the rename. A volume that cannot sync at all, as
/// some network shares on macOS cannot, is written without it
/// ([`sync_is_unsupported`]); any other failure is the write's, and the
/// file is removed.
pub fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    static SAID: std::sync::Once = std::sync::Once::new();
    #[cfg(feature = "testing")]
    SYNCED.with(|n| n.set(n.get() + 1));
    let mut f = std::fs::File::create(path)?;
    let written = f.write_all(bytes).and_then(|()| match f.sync_data() {
        Err(e) if sync_is_unsupported(&e) => {
            SAID.call_once(|| {
                log::debug!(
                    "{}: no sync on this volume ({e}); written without",
                    path.display()
                )
            });
            Ok(())
        }
        other => other,
    });
    drop(f);
    if written.is_err() {
        let _ = std::fs::remove_file(path);
    }
    written
}

/// Whether a sync failed because the volume or the file cannot be
/// synced, rather than because the data did not land: `Unsupported`,
/// or on unix `ENOTSUP`, `ENOTTY`, or `EINVAL`. On Apple's systems a
/// sync is `F_FULLFSYNC`, with no fallback, which a volume without it
/// (a network share, often) refuses with one of these.
pub fn sync_is_unsupported(e: &std::io::Error) -> bool {
    let unsupported = e.kind() == std::io::ErrorKind::Unsupported;
    #[cfg(unix)]
    let unsupported = unsupported
        || matches!(
            e.raw_os_error(),
            Some(code) if code == libc::ENOTSUP || code == libc::ENOTTY || code == libc::EINVAL
        );
    unsupported
}

/// Mark a folder hidden where a leading dot does not do it. Windows
/// keeps that in a file attribute rather than in the name, and the
/// attribute set is written whole, so the ones the folder has are
/// read first and kept. Nothing to do elsewhere.
fn hide_folder(dir: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_HIDDEN, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
        };
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is a null-terminated UTF-16 path that outlives
        // both calls, which read it and nothing else.
        unsafe {
            let have = GetFileAttributesW(wide.as_ptr());
            if have == INVALID_FILE_ATTRIBUTES {
                return Err(std::io::Error::last_os_error());
            }
            if have & FILE_ATTRIBUTE_HIDDEN == 0
                && SetFileAttributesW(wide.as_ptr(), have | FILE_ATTRIBUTE_HIDDEN) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;
    use crate::{Adjustment, Snapshot, geometry, preset};

    #[test]
    fn a_sidecar_with_the_old_tone_curve_switch_off_still_reads() {
        // The curve had a switch once; a file saved with it off keeps
        // its sliders and loses only the switch, which is gone.
        let old = r#"{"version": 3, "light": {"exposure": 0.5,
            "tone": {"enabled": false, "contrast": 1.2, "shadows": 0.3}}}"#;
        let e = Edit::from_json(old).unwrap();
        assert_eq!(e.light.exposure, 0.5);
        assert_eq!(e.light.tone.contrast, 1.2);
        assert_eq!(e.light.tone.shadows, 0.3);
        let written: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
        let tone = written["light"]["tone"].as_object().unwrap();
        assert!(!tone.contains_key("enabled"), "{tone:?}");
        assert!(tone.contains_key("contrast"));

        // The same through a sidecar on disk, current and history.
        let dir = scratch("old-tone-switch");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(
            Sidecar::path_for(&raw),
            format!(r#"{{"current": {old}, "history": [{old}]}}"#),
        )
        .unwrap();
        let sidecar = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(sidecar.current, e);
        greycard_core::testing::remove_dir_retry(&dir);

        // And through a preset.
        let preset = format!(r#"{{"name": "Old", "sections": ["light"], "edit": {old}}}"#);
        let p = preset::Preset::from_json(&preset).unwrap();
        assert_eq!(p.edit.light.tone, e.light.tone);
    }

    /// A sync the volume cannot do is best effort; one that failed
    /// for any other reason fails the write.
    #[test]
    fn a_sync_the_volume_cannot_do_is_not_a_failed_write() {
        use std::io::{Error, ErrorKind};
        assert!(sync_is_unsupported(&Error::from(ErrorKind::Unsupported)));
        assert!(!sync_is_unsupported(&Error::from(ErrorKind::Other)));
        assert!(!sync_is_unsupported(&Error::from(
            ErrorKind::PermissionDenied
        )));
        #[cfg(unix)]
        {
            for code in [libc::ENOTSUP, libc::ENOTTY, libc::EINVAL] {
                assert!(
                    sync_is_unsupported(&Error::from_raw_os_error(code)),
                    "{code}"
                );
            }
            for code in [libc::EIO, libc::ENOSPC, libc::EDQUOT, libc::EROFS] {
                assert!(
                    !sync_is_unsupported(&Error::from_raw_os_error(code)),
                    "{code}"
                );
            }
        }
        // And a write that goes through is whole.
        let dir = scratch("synced");
        let path = dir.join("a.json");
        write_synced(&path, b"{}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        // One that cannot begin leaves nothing behind.
        let nowhere = dir.join("no such folder").join("b.json");
        assert!(write_synced(&nowhere, b"{}").is_err());
        assert!(!nowhere.exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    fn set_modified(path: &Path, secs: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("the file opens")
            .set_modified(t)
            .expect("the time is set");
    }

    #[test]
    fn the_sidecar_has_one_name_in_two_places() {
        let raw = Path::new("/shoot/IMG_0001.CR3");
        assert_eq!(
            Sidecar::path_in(raw, Placement::Beside),
            Path::new("/shoot/IMG_0001.CR3.gcd")
        );
        assert_eq!(
            Sidecar::path_in(raw, Placement::Beside),
            Sidecar::path_for(raw)
        );
        assert_eq!(
            Sidecar::path_in(raw, Placement::Folder),
            Path::new("/shoot/.greycard/IMG_0001.CR3.gcd")
        );
        // A bare name has a folder too: the current one.
        assert_eq!(
            Sidecar::path_in(Path::new("IMG_0001.CR3"), Placement::Folder),
            Path::new(".greycard/IMG_0001.CR3.gcd")
        );
    }

    #[test]
    fn find_reads_both_places_and_takes_the_newer() {
        let dir = scratch("find");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        assert_eq!(Sidecar::find(&raw), None);

        let beside = Sidecar::path_in(&raw, Placement::Beside);
        let under = Sidecar::path_in(&raw, Placement::Folder);
        std::fs::write(&beside, b"{}").unwrap();
        assert_eq!(Sidecar::find(&raw), Some(beside.clone()));

        std::fs::remove_file(&beside).unwrap();
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        std::fs::write(&under, b"{}").unwrap();
        assert_eq!(Sidecar::find(&raw), Some(under.clone()));

        // Both there, neither with a `saved` count (both read as 0,
        // an equal tie): mtime decides, the one written last,
        // whichever place it is in.
        std::fs::write(&beside, b"{}").unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(under.clone()));
        set_modified(&beside, 3_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside.clone()));
        // A tie is beside: the older rule.
        set_modified(&under, 3_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside));
    }

    #[test]
    fn saving_three_times_counts_three() {
        let dir = scratch("counter");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let mut sidecar = Sidecar::default();
        for n in 0..3 {
            let mut e = Edit::default();
            e.light.exposure = n as f32;
            sidecar.record(e);
            sidecar.save(&raw).unwrap();
        }
        assert_eq!(sidecar.saved, 3);
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap().saved, 3);
    }

    #[test]
    fn find_prefers_the_higher_count_over_a_newer_mtime() {
        let dir = scratch("count-over-mtime");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let (beside, under) = (
            Sidecar::path_in(&raw, Placement::Beside),
            Sidecar::path_in(&raw, Placement::Folder),
        );
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        // The beside copy has the higher count but the older mtime —
        // a copy tool or a restore could produce exactly this. It
        // still wins: the count says it is the later save.
        std::fs::write(&beside, r#"{"saved":5}"#).unwrap();
        std::fs::write(&under, r#"{"saved":2}"#).unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside));
    }

    /// Two copies in two places altogether, a frame's here and its
    /// copy's on an archive, by the same rule.
    #[test]
    fn two_copies_compare_by_count_then_mtime() {
        use std::cmp::Ordering;
        let dir = scratch("compare-copies");
        let (a, b) = (dir.join("a.gcd"), dir.join("b.gcd"));
        std::fs::write(&a, r#"{"saved":5}"#).unwrap();
        std::fs::write(&b, r#"{"saved":2}"#).unwrap();
        set_modified(&a, 1_000_000_000);
        set_modified(&b, 2_000_000_000);
        assert_eq!(Sidecar::compare_copies(&a, &b), Ordering::Greater);
        assert_eq!(Sidecar::compare_copies(&b, &a), Ordering::Less);
        // One uncounted: the mtime decides.
        std::fs::write(&a, r#"{}"#).unwrap();
        set_modified(&a, 1_000_000_000);
        assert_eq!(Sidecar::compare_copies(&a, &b), Ordering::Less);
        // Equal counts and times: no telling them apart.
        std::fs::write(&a, r#"{"saved":2}"#).unwrap();
        set_modified(&a, 2_000_000_000);
        assert_eq!(Sidecar::compare_copies(&a, &b), Ordering::Equal);
    }

    #[test]
    fn find_falls_back_to_mtime_when_counts_are_equal() {
        let dir = scratch("count-tie");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let (beside, under) = (
            Sidecar::path_in(&raw, Placement::Beside),
            Sidecar::path_in(&raw, Placement::Folder),
        );
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        // Equal, non-zero counts: not before the field existed, a
        // genuine tie (a `settle` that renamed rather than wrote,
        // for instance). mtime is what is left to decide with.
        std::fs::write(&beside, r#"{"saved":4}"#).unwrap();
        std::fs::write(&under, r#"{"saved":4}"#).unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(under.clone()));
        set_modified(&beside, 3_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside));
    }

    #[test]
    fn a_sidecar_without_the_saved_field_loads_as_zero() {
        let dir = scratch("no-field");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let beside = Sidecar::path_in(&raw, Placement::Beside);
        // What this build's own sidecars looked like before this
        // field existed, and what an older build still writes.
        std::fs::write(&beside, r#"{"current":{"version":4}}"#).unwrap();
        let loaded = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(loaded.saved, 0);
    }

    #[test]
    fn a_saved_field_that_will_not_parse_costs_that_field_and_nothing_else() {
        let dir = scratch("bad-saved");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let beside = Sidecar::path_in(&raw, Placement::Beside);
        // `saved` is read loosely, the way `turn`, `meta` and `xmp`
        // are: a value that is not a plain non-negative integer must
        // cost only this field, not the whole sidecar, the way
        // `"turn":"x"` already does not fail a load.
        std::fs::write(
            &beside,
            r#"{"current":{"version":4,"light":{"exposure":2.0}},"saved":-1}"#,
        )
        .unwrap();
        let loaded = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(loaded.saved, 0);
        assert_eq!(
            loaded.current.light.exposure, 2.0,
            "the edit beside the bad field survives"
        );
    }

    #[test]
    fn find_treats_an_uncounted_copy_as_mtime_only_however_high_the_other_counts() {
        let dir = scratch("uncounted");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let (beside, under) = (
            Sidecar::path_in(&raw, Placement::Beside),
            Sidecar::path_in(&raw, Placement::Folder),
        );
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        // The shape that loses an edit under a plain "higher count
        // wins" rule: this build saved under the folder three times
        // (`saved: 3`); 0.1.1, which does not know the field and
        // reads beside only, then starts from the default and saves
        // beside with no `saved` key at all and a newer mtime. The
        // beside copy is not "uncounted and behind" — it is the
        // sidecar with the actual latest edit — so mtime, not the
        // counter, must decide.
        std::fs::write(&under, r#"{"saved":3}"#).unwrap();
        set_modified(&under, 1_000_000_000);
        std::fs::write(&beside, r#"{"current":{"version":4}}"#).unwrap();
        set_modified(&beside, 2_000_000_000);
        assert_eq!(Sidecar::find(&raw), Some(beside));
    }

    #[test]
    fn settle_agrees_with_find_when_the_count_beats_mtime() {
        let dir = scratch("settle-count");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let (beside, under) = (
            Sidecar::path_in(&raw, Placement::Beside),
            Sidecar::path_in(&raw, Placement::Folder),
        );
        std::fs::create_dir_all(under.parent().unwrap()).unwrap();
        // Same shape as `find_prefers_the_higher_count_over_a_newer_mtime`:
        // the beside copy is the higher count but the older mtime.
        std::fs::write(&beside, r#"{"saved":5}"#).unwrap();
        std::fs::write(&under, r#"{"saved":2}"#).unwrap();
        set_modified(&beside, 1_000_000_000);
        set_modified(&under, 2_000_000_000);

        assert_eq!(Sidecar::find(&raw), Some(beside.clone()));
        // Settling into `Folder` should keep the copy `find` picked,
        // beside, and drop the other: they must never disagree.
        assert_eq!(
            Sidecar::settle(&raw, Placement::Folder).unwrap(),
            Settled::Moved
        );
        assert!(under.exists());
        assert!(!beside.exists());
        assert_eq!(std::fs::read_to_string(&under).unwrap(), r#"{"saved":5}"#);
    }

    #[test]
    fn a_sidecar_under_the_folder_loads_and_saves() {
        let dir = scratch("folder");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        sidecar.record(e);

        // Saved under the folder: the folder is made, nothing lands
        // beside the raw, and a load finds it without being told.
        sidecar.save_in(&raw, Placement::Folder).unwrap();
        let under = dir.join(SIDECAR_FOLDER).join("IMG_0001.CR3.gcd");
        assert!(under.exists(), "{}", under.display());
        assert!(!Sidecar::path_for(&raw).exists());
        // The write's temporary is gone with the rename.
        assert!(
            std::fs::read_dir(dir.join(SIDECAR_FOLDER))
                .unwrap()
                .all(|e| e.unwrap().path().extension() != Some(std::ffi::OsStr::new("tmp")))
        );
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), sidecar);

        // Saved beside afterwards: the folder's copy is superseded
        // and goes, so the frame has one sidecar, and it loads.
        let mut later = sidecar.clone();
        let mut e = Edit::default();
        e.light.exposure = 2.0;
        later.record(e);
        later.save(&raw).unwrap();
        assert!(!under.exists(), "the folder's copy is superseded");
        assert!(Sidecar::path_for(&raw).exists());
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), later);

        // And back under the folder: the beside copy goes the same way.
        later.save_in(&raw, Placement::Folder).unwrap();
        assert!(under.exists());
        assert!(
            !Sidecar::path_for(&raw).exists(),
            "the beside copy is superseded"
        );
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), later);
    }

    #[test]
    fn a_write_into_a_missing_hidden_folder_makes_it() {
        let dir = scratch("write-into-folder");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        sidecar.record(e);

        // A copy's sidecar written under a folder that is not there:
        // the folder is made, and the file lands.
        let under = Sidecar::path_in(&raw, Placement::Folder);
        assert!(!under.parent().unwrap().exists());
        sidecar.write_to(&under).unwrap();
        assert_eq!(Sidecar::read(&under).unwrap(), sidecar);

        // Beside, the shoot folder is what there is; nothing made.
        let beside = Sidecar::path_in(&raw, Placement::Beside);
        sidecar.write_to(&beside).unwrap();
        assert_eq!(Sidecar::read(&beside).unwrap(), sidecar);
    }

    #[test]
    fn settle_moves_the_newer_copy_and_drops_the_stale_one() {
        let dir = scratch("settle");
        let (a, b, c, d) = (
            dir.join("A.CR3"),
            dir.join("B.CR3"),
            dir.join("C.CR3"),
            dir.join("D.CR3"),
        );
        let place = |raw: &Path, placement, text: &str| {
            let path = Sidecar::path_in(raw, placement);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        };
        // A beside only; B under the folder only; C in both, the one
        // beside newer; D with none.
        place(&a, Placement::Beside, "a");
        place(&b, Placement::Folder, "b");
        let c_beside = place(&c, Placement::Beside, "c new");
        let c_under = place(&c, Placement::Folder, "c old");
        set_modified(&c_under, 1_000_000_000);
        set_modified(&c_beside, 2_000_000_000);

        let to = Placement::Folder;
        assert!(Sidecar::misplaced(&a, to));
        assert!(!Sidecar::misplaced(&b, to));
        assert!(Sidecar::misplaced(&c, to));
        assert!(!Sidecar::misplaced(&d, to));

        assert_eq!(Sidecar::settle(&a, to).unwrap(), Settled::Moved);
        assert_eq!(Sidecar::settle(&b, to).unwrap(), Settled::InPlace);
        assert_eq!(Sidecar::settle(&c, to).unwrap(), Settled::Moved);
        assert_eq!(Sidecar::settle(&d, to).unwrap(), Settled::InPlace);

        let read =
            |raw: &Path, placement| std::fs::read_to_string(Sidecar::path_in(raw, placement)).ok();
        assert_eq!(read(&a, to).as_deref(), Some("a"));
        assert_eq!(read(&a, Placement::Beside), None);
        assert_eq!(read(&b, to).as_deref(), Some("b"));
        // The newer copy won, over the one that was in place.
        assert_eq!(read(&c, to).as_deref(), Some("c new"));
        assert_eq!(read(&c, Placement::Beside), None);
        assert!(!Sidecar::path_in(&d, to).exists());

        // And back: a stale copy in the other place is dropped, not
        // moved over the one that is read.
        let stale = place(&a, Placement::Beside, "a stale");
        set_modified(&stale, 1_000_000_000);
        assert_eq!(Sidecar::settle(&a, to).unwrap(), Settled::Dropped);
        assert_eq!(read(&a, to).as_deref(), Some("a"));
        assert!(!stale.exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_sidecar_keeps_history_and_survives_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("greycard-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0001.CR3");
        assert!(Sidecar::load(&raw).unwrap().is_none());
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        assert!(sidecar.record(e.clone()));
        assert!(!sidecar.record(e.clone())); // the same again is not a new state
        e.light.exposure = 2.0;
        sidecar.record(e.clone());
        assert_eq!(sidecar.history.len(), 2);
        sidecar.save(&raw).unwrap();
        assert!(Sidecar::path_for(&raw).ends_with("IMG_0001.CR3.gcd"));
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back, sidecar);
        assert_eq!(back.current.light.exposure, 2.0);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_sidecar_carries_the_meta_beside_the_edit_and_the_history_leaves_it_alone() {
        use meta::{Flag, Label};
        let dir = std::env::temp_dir().join(format!("greycard-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0002.CR3");
        let mut sidecar = Sidecar::default();
        // An edit, then a rating: the rating is not a state.
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        assert!(sidecar.record(e.clone()));
        let states = sidecar.states();
        sidecar.meta.set_rating(4);
        sidecar.meta.flag = Flag::Pick;
        sidecar.meta.label = Label::Green;
        sidecar.meta.add_keyword("harbor");
        sidecar.meta.title = "Low tide".into();
        assert_eq!(sidecar.states(), states);
        assert!(!sidecar.history.is_empty());
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back, sidecar);
        assert_eq!(back.meta.rating, 4);
        assert_eq!(back.meta.flag, Flag::Pick);
        assert_eq!(back.meta.label, Label::Green);
        assert_eq!(back.meta.keywords, ["harbor"]);
        assert_eq!(back.meta.title, "Low tide");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_meta_that_says_nothing_is_not_written_and_a_bad_one_costs_no_edit() {
        // A developed frame nobody has rated: no meta block at all.
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = 1.0;
        let json = serde_json::to_string(&sidecar).unwrap();
        assert!(!json.contains("meta"), "{json}");
        assert_eq!(serde_json::from_str::<Sidecar>(&json).unwrap(), sidecar);
        sidecar.meta.set_rating(1);
        assert!(serde_json::to_string(&sidecar).unwrap().contains("meta"));

        // A meta this build cannot read at all does not take the edit
        // and the history down with it: that would open the frame as
        // a fresh one and let the next slider write over both.
        let json = r#"{"current":{"version":3,"light":{"exposure":0.5}},"meta":"picked"}"#;
        let back: Sidecar = serde_json::from_str(json).unwrap();
        assert_eq!(back.current.light.exposure, 0.5);
        assert!(back.meta.is_empty());
        // Nor does one field of it: a label from a later build leaves
        // the rating, and the edit, where they are.
        let json = r#"{"current":{"version":3,"light":{"exposure":0.5}},
                       "meta":{"rating":3,"label":"grey"}}"#;
        let back: Sidecar = serde_json::from_str(json).unwrap();
        assert_eq!(back.current.light.exposure, 0.5);
        assert_eq!(back.meta.rating, 3);
        assert_eq!(back.meta.label, meta::Label::None);
    }

    #[test]
    fn undo_after_a_rating_takes_back_the_edit_and_not_the_rating() {
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        sidecar.record(e.clone());
        sidecar.meta.set_rating(3);
        e.light.exposure = 2.0;
        sidecar.record(e);
        sidecar.meta.set_rating(5);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 1.0);
        assert_eq!(sidecar.meta.rating, 5);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
        assert_eq!(sidecar.meta.rating, 5);
        // And a redo, and a snapshot restored, leave it where it is.
        assert!(sidecar.redo());
        assert_eq!(sidecar.meta.rating, 5);
        sidecar.take_snapshot("one", 0);
        assert!(sidecar.undo());
        assert!(sidecar.restore_snapshot(0));
        assert_eq!(sidecar.meta.rating, 5);
    }

    #[test]
    fn the_turn_round_trips_beside_the_edit_and_is_absent_at_none() {
        let dir = std::env::temp_dir().join(format!("greycard-turn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0002.CR3");
        let mut sidecar = Sidecar::default();
        sidecar.current.light.exposure = 0.25;

        // Nothing to say: no field in the file at all, so a sidecar
        // written before the turn existed and one written now for an
        // unturned frame are the same bytes.
        let json = serde_json::to_string(&sidecar).unwrap();
        assert!(!json.contains(r#""turn":"#), "{json}");

        sidecar.turn_by(-1, Some(1.5));
        assert_eq!(sidecar.turn, 3);
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back.turn, 3);
        assert_eq!(back.current.light.exposure, 0.25);
        greycard_core::testing::remove_dir_retry(&dir);

        // Four turns are none, and the field goes again.
        let mut sidecar = Sidecar::default();
        for _ in 0..4 {
            sidecar.turn_by(1, Some(1.5));
        }
        assert_eq!(sidecar.turn, 0);
        assert!(
            !serde_json::to_string(&sidecar)
                .unwrap()
                .contains(r#""turn":"#)
        );

        // A turn is no part of an edit: a preset made from this one
        // cannot carry it onto the next frame.
        let mut sidecar = Sidecar::default();
        sidecar.turn_by(1, Some(1.5));
        assert!(!sidecar.current.to_json().contains(r#""turn":"#));
    }

    #[test]
    fn a_turn_this_build_cannot_read_is_no_turn_and_costs_no_edit() {
        // Loose, as the meta is (§117): a word, a number past three,
        // a list. None of them takes the edit down with it.
        for value in ["\"left\"", "[1]", "null", "-2", "true", "1.5"] {
            let json = format!(
                r#"{{"current":{{"version":3,"light":{{"exposure":0.5}}}},"turn":{value}}}"#
            );
            let back: Sidecar = serde_json::from_str(&json).unwrap();
            assert_eq!(back.turn, 0, "{value}");
            assert_eq!(back.current.light.exposure, 0.5, "{value}");
        }
        // A turn past three is the turn it comes to, not a refusal.
        let back: Sidecar = serde_json::from_str(r#"{"turn":6}"#).unwrap();
        assert_eq!(back.turn, 2);
        let back: Sidecar = serde_json::from_str(r#"{"turn":7}"#).unwrap();
        assert_eq!(back.turn, 3);
    }

    #[test]
    fn undo_after_a_turn_takes_back_the_edit_and_not_the_turn() {
        let mut sidecar = Sidecar::default();
        // A crop on the state that undo will hand back: every state
        // has to mean the same pixels as the current one, or undo
        // puts a pre-turn rectangle onto a turned frame.
        sidecar.current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        let mut edit = sidecar.current.clone();
        edit.light.exposure = 1.0;
        sidecar.record(edit);
        sidecar.turn_by(1, Some(1.5));
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 0.0);
        assert_eq!(sidecar.turn, 1, "the frame is still the way up it was");
        assert_eq!(
            sidecar.current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            }),
            "the state undo handed back is the turned frame's"
        );
        // And a snapshot restored leaves it alone too.
        sidecar.take_snapshot("before", 0);
        sidecar.turn_by(1, Some(1.5));
        let mut later = Edit::default();
        later.light.exposure = 2.0;
        sidecar.record(later);
        assert!(sidecar.restore_snapshot(0));
        assert_eq!(sidecar.current.light.exposure, 0.0);
        assert_eq!(sidecar.turn, 2);
    }

    /// A mask and a repair patch cover the same pixels through every
    /// turn, on a landscape frame and on a portrait one, and in every
    /// state the sidecar holds. Positions are in units of the
    /// picture's *width*, both ways, so leaving them alone would
    /// slide them down the frame and change their size.
    #[test]
    fn a_mask_and_a_patch_cover_the_same_pixels_through_a_turn() {
        use mask::{Component, Mask, Shape};
        use retouch::{Method, Patch};

        // The reference: a pixel of a `w` by `h` picture, turned `q`
        // quarters clockwise about the picture, and the size it is
        // then in.
        fn turn_pixel(p: (f32, f32), w: f32, h: f32, q: i32) -> ((f32, f32), (f32, f32)) {
            let (mut p, mut w, mut h) = (p, w, h);
            for _ in 0..q.rem_euclid(4) {
                p = (h - p.1, p.0);
                std::mem::swap(&mut w, &mut h);
            }
            (p, (w, h))
        }

        let face = [0.48, 0.42];
        let spot = [0.31, 0.77];
        let make = || {
            let mut edit = Edit::default();
            edit.adjustments.push(Adjustment {
                id: 1,
                name: "Face".into(),
                mask: Mask {
                    components: vec![Component {
                        shape: Shape::Radial {
                            center: face,
                            radius: [0.12, 0.09],
                            angle: 20.0,
                            feather: 0.5,
                        },
                        ..Component::default()
                    }],
                    ..Mask::default()
                },
                ..Adjustment::default()
            });
            edit.retouch.patches.push(Patch {
                id: 1,
                method: Method::Heal,
                points: vec![spot],
                radius: 0.02,
                source: Some([0.05, -0.03]),
                ..Patch::default()
            });
            edit
        };

        for (w, h) in [(6000.0f32, 4000.0f32), (4000.0f32, 6000.0f32)] {
            for q in 1..4i32 {
                let mut sidecar = Sidecar {
                    line: Line::of(make()),
                    ..Sidecar::default()
                };
                // One state behind it, one undone in front of it and
                // one kept by name: every state `edits_mut` covers
                // has to land on the same pixels.
                sidecar.history.push(make().into());
                sidecar.redo.push(make().into());
                sidecar.snapshots.push(Snapshot {
                    name: "before".into(),
                    taken: 0,
                    edit: make(),
                });
                sidecar.turn_by(q, Some(w / h));

                let (_, (nw, _)) = turn_pixel((0.0, 0.0), w, h, q);
                let want = |p: [f32; 2]| {
                    let (px, _) = turn_pixel((p[0] * w, p[1] * w), w, h, q);
                    [px.0 / nw, px.1 / nw]
                };
                let (wanted_face, wanted_spot) = (want(face), want(spot));
                let scale = w / nw;

                for edit in std::iter::once(&sidecar.current)
                    .chain(sidecar.history.iter().map(|s| &s.edit))
                    .chain(sidecar.redo.iter().map(|s| &s.edit))
                    .chain(sidecar.snapshots.iter().map(|s| &s.edit))
                {
                    let Shape::Radial {
                        center,
                        radius,
                        angle,
                        ..
                    } = &edit.adjustments[0].mask.components[0].shape
                    else {
                        panic!("the shape is still a radial");
                    };
                    assert!(
                        (center[0] - wanted_face[0]).abs() < 1e-4
                            && (center[1] - wanted_face[1]).abs() < 1e-4,
                        "{w}x{h} + {q}: {center:?} vs {wanted_face:?}"
                    );
                    assert!(
                        (radius[0] - 0.12 * scale).abs() < 1e-4
                            && (radius[1] - 0.09 * scale).abs() < 1e-4,
                        "{w}x{h} + {q}: {radius:?}"
                    );
                    // The ellipse turns with the picture: the angle
                    // grows clockwise on the screen, as `Shape::at`
                    // reads it, and stays within a half turn either
                    // way of level.
                    let wanted = {
                        let a = (20.0 + 90.0 * q as f32).rem_euclid(360.0);
                        if a > 180.0 { a - 360.0 } else { a }
                    };
                    assert!(
                        (angle - wanted).abs() < 1e-4,
                        "{w}x{h} + {q}: {angle} vs {wanted}"
                    );
                    let patch = &edit.retouch.patches[0];
                    assert!(
                        (patch.points[0][0] - wanted_spot[0]).abs() < 1e-4
                            && (patch.points[0][1] - wanted_spot[1]).abs() < 1e-4,
                        "{w}x{h} + {q}: {:?} vs {wanted_spot:?}",
                        patch.points[0]
                    );
                    assert!((patch.radius - 0.02 * scale).abs() < 1e-5);
                    // The source is an offset, so it turns without
                    // being moved: the healed pixels keep their
                    // source pixels.
                    let d = patch.source.expect("the source is kept");
                    let turned = mask::Turned::new(q, w / h).offset([0.05, -0.03]);
                    assert!((d[0] - turned[0]).abs() < 1e-5 && (d[1] - turned[1]).abs() < 1e-5);
                }
            }
        }

        // And all the way round is where it started.
        let mut sidecar = Sidecar {
            line: Line::of(make()),
            ..Sidecar::default()
        };
        let (w, h) = (6000.0f32, 4000.0f32);
        let mut aspect = w / h;
        for _ in 0..4 {
            sidecar.turn_by(1, Some(aspect));
            aspect = 1.0 / aspect;
        }
        assert_eq!(sidecar.turn, 0);
        let Shape::Radial {
            center,
            radius,
            angle,
            ..
        } = &sidecar.current.adjustments[0].mask.components[0].shape
        else {
            panic!("a radial");
        };
        assert!((center[0] - face[0]).abs() < 1e-4 && (center[1] - face[1]).abs() < 1e-4);
        assert!((radius[0] - 0.12).abs() < 1e-5 && (radius[1] - 0.09).abs() < 1e-5);
        assert!((angle - 20.0).abs() < 1e-4);
    }

    /// With no picture to measure by, the geometry still turns — a
    /// crop is in fractions of the plane and needs no aspect — and
    /// anything placed on the picture stays where it is, which the
    /// caller is told about rather than left to find out.
    #[test]
    fn a_turn_with_no_aspect_moves_the_crop_and_says_what_it_left() {
        use mask::{Component, Mask};
        let mut sidecar = Sidecar::default();
        sidecar.current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        assert!(!sidecar.placed(), "a crop is not placed on the picture");
        sidecar.current.adjustments.push(Adjustment {
            id: 1,
            mask: Mask {
                components: vec![Component::default()],
                ..Mask::default()
            },
            ..Adjustment::default()
        });
        assert!(sidecar.placed());
        let before = sidecar.current.adjustments[0].mask.clone();
        sidecar.turn_by(1, None);
        assert_eq!(sidecar.turn, 1);
        assert_eq!(
            sidecar.current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        assert_eq!(sidecar.current.adjustments[0].mask, before);
    }

    #[test]
    fn a_turn_carries_the_crop_with_the_frame() {
        let mut sidecar = Sidecar::default();
        sidecar.current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        sidecar.turn_by(1, Some(1.5));
        assert_eq!(
            sidecar.current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        assert_eq!(sidecar.current.geometry.turns, 0);
    }

    #[test]
    fn a_sidecar_can_hold_meta_with_no_edit_and_an_edit_with_no_meta() {
        // Rated before it was ever developed: the frame opens at the
        // default edit.
        let only_meta: Sidecar = serde_json::from_str(r#"{"meta":{"rating":2}}"#).unwrap();
        assert_eq!(only_meta.meta.rating, 2);
        assert_eq!(only_meta.current, Edit::default());
        assert!(only_meta.history.is_empty());
        // Written before there was a meta section at all.
        let only_edit: Sidecar =
            serde_json::from_str(r#"{"current":{"version":3,"light":{"exposure":0.5}}}"#).unwrap();
        assert_eq!(only_edit.current.light.exposure, 0.5);
        assert!(only_edit.meta.is_empty());
        // And the meta is no part of an edit: a preset or a look
        // never sees one, since the field is not on `Edit`.
        let json = Edit::default().to_json();
        assert!(!json.contains("meta"), "{json}");
    }
}

#[cfg(test)]
mod before_versions {
    use super::*;
    use crate::{VERSION, baseline};

    pub(crate) fn pinned() -> Sidecar {
        let mut s = Sidecar::default();
        let exposed = |stops: f32| {
            let mut e = Edit::default();
            e.light.exposure = stops;
            e
        };
        s.record(exposed(0.5));
        s.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        s.take_snapshot("Snapshot 1", 1_700_000_100);
        s.record_export(
            &exposed(1.0),
            Exported {
                file: "/shoot/IMG_0001.jpg".into(),
                preset: Some("Web".into()),
                at: 1_700_000_200,
            },
        );
        s.record_as(exposed(1.5), Some("Sync from IMG_0002.CR3".into()));
        s.record_export(
            &exposed(1.5),
            Exported {
                file: "/shoot/IMG_0001-1.jpg".into(),
                preset: None,
                at: 1_700_000_300,
            },
        );
        s.meta.rating = 3;
        s.meta.keywords = vec!["harbor".into(), "dusk".into()];
        s.turn_by(1, None);
        s.xmp = Some(xmp::Adopted {
            file: "IMG_0001.xmp".into(),
            len: 1234,
            hash: "0123456789abcdef".into(),
        });
        s.absorbed = vec!["00112233445566778899aabbccddeeff".into()];
        s.advance(&sync::Stamp {
            at: 1_700_000_400,
            host: "desk".into(),
        });
        s.meta.title = "Harbor".into();
        s.advance(&sync::Stamp {
            at: 1_700_000_500,
            host: "laptop".into(),
        });
        s
    }

    const BEFORE_VERSIONS: &str = include_str!("../fixtures/sidecar-before-versions.gcd");

    /// The states of a line as (edit, label, exports), the current last.
    fn states(l: &Line) -> Vec<(Edit, Option<String>, Vec<Exported>)> {
        l.history
            .iter()
            .map(|h| (h.edit.clone(), h.label.clone(), h.exports.clone()))
            .chain([(
                l.current.clone(),
                l.current_label.clone(),
                l.current_exports.clone(),
            )])
            .collect()
    }

    #[test]
    fn a_schema_four_sidecar_reads_whole_and_writes_at_five() {
        let dir = std::env::temp_dir().join(format!("gc-before-versions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("IMG_0001.CR3.gcd");
        std::fs::write(&path, BEFORE_VERSIONS).unwrap();
        let read = Sidecar::read(&path).unwrap();
        greycard_core::testing::remove_dir_retry(&dir);

        // What a build at schema 4 wrote, the same today: the states and
        // their words and records, the snapshots, the file's own facts.
        let made = pinned();
        assert!(read.edits().all(|e| e.version == VERSION));
        // Every state that develops something keeps the zero it was
        // made against; the default state takes its body's.
        let kept_zero = |mut e: Edit| {
            if e != Edit::default() {
                e.light.baseline = Some(baseline::BEFORE_BODIES);
            }
            e
        };
        let made_states: Vec<_> = states(&made.line)
            .into_iter()
            .map(|(e, l, x)| (kept_zero(e), l, x))
            .collect();
        assert_eq!(states(&read.line), made_states);
        let mut made_snapshots = made.snapshots.clone();
        for s in &mut made_snapshots {
            s.edit = kept_zero(s.edit.clone());
        }
        assert_eq!(read.snapshots, made_snapshots);
        assert_eq!(read.last_export, made.last_export);
        assert_eq!(read.meta, made.meta);
        let raw: Sidecar = serde_json::from_str(BEFORE_VERSIONS).unwrap();
        assert_eq!(read.meta_at, raw.meta_at);
        assert_eq!(read.turn, made.turn);
        assert_eq!(read.xmp, made.xmp);
        assert_eq!(read.saved, made.saved);
        assert_eq!(read.revisions, raw.revisions);
        assert_eq!(read.revisions.len(), 2);
        assert_eq!(read.absorbed, made.absorbed);
        assert!(read.versions.is_empty());
        // The stored ids are kept as they were, never made again.
        let value: serde_json::Value = serde_json::from_str(BEFORE_VERSIONS).unwrap();
        assert_eq!(read.current_id.as_deref(), value["id"].as_str());
        for (h, v) in read
            .history
            .iter()
            .zip(value["history"].as_array().unwrap())
        {
            assert_eq!(h.id.as_deref(), v["id"].as_str());
        }

        // Written, every edit says the current version; read again and
        // written again, the same bytes.
        let written = read.to_json();
        assert!(written.contains(&format!("\"version\": {VERSION}")));
        assert!(!written.contains("\"version\": 4"));
        let again = Sidecar::from_json(&written).unwrap();
        assert_eq!(again, read);
        assert_eq!(again.to_json(), written);
    }

    /// The file's text against `serde_json`'s pretty form of the same
    /// sidecar: the same document read back, the compact form's bytes
    /// once the whitespace is taken out, and no line a lone number.
    fn laid_out_as_pretty(s: &Sidecar) {
        let ours = s.to_json();
        let pretty = serde_json::to_string_pretty(s).unwrap();
        assert_eq!(
            Sidecar::from_json(&ours).unwrap(),
            Sidecar::from_json(&pretty).unwrap()
        );
        let tree = |t: &str| serde_json::from_str::<serde_json::Value>(t).unwrap();
        assert_eq!(tree(&ours), tree(&pretty));
        let squeezed: String = ours.split('\n').map(str::trim_start).collect();
        assert_eq!(
            squeezed.replace("\": ", "\":"),
            serde_json::to_string(s).unwrap()
        );
        assert!(ours.lines().count() <= pretty.lines().count());
        assert!(
            !ours
                .lines()
                .any(|l| l.trim_end_matches(',').trim().parse::<f64>().is_ok()),
            "a number on a line of its own"
        );
    }

    #[test]
    fn the_file_keeps_a_list_of_numbers_on_one_line_and_reads_the_same() {
        use crate::Adjustment;
        use crate::brush::{Op, Stroke};
        use crate::mask::{Component, Mask, Pick, Shape};
        use crate::retouch::{Method, Patch};

        // The fixture, from before versions, as read.
        let old = Sidecar::from_json(BEFORE_VERSIONS).unwrap();
        laid_out_as_pretty(&old);

        let stroke = |n: usize, op| Stroke {
            points: (0..n)
                .map(|i| [0.05054598 + i as f32 * 1e-3, 0.1175683 - i as f32 * 7e-4])
                .collect(),
            radius: 0.031,
            feather: 0.5,
            flow: 1.0,
            op,
        };
        let pick = |x, positive| Pick {
            pos: [x, 0.25],
            positive,
        };
        let shapes = vec![
            Shape::Linear {
                from: [0.1, 0.2],
                to: [0.3, 0.9],
            },
            Shape::Radial {
                center: [0.48, 0.42],
                radius: [0.12, 0.09],
                angle: 20.0,
                feather: 0.5,
            },
            Shape::Brush {
                strokes: vec![stroke(138, Op::Add), stroke(1, Op::Add), stroke(0, Op::Add)],
            },
            Shape::Subject {},
            Shape::Background {},
            Shape::Sky {
                picks: vec![pick(0.2, true), pick(0.7, false)],
            },
            Shape::Object {
                picks: vec![pick(0.5, true)],
                boxes: vec![[[0.1, 0.1], [0.4, 0.6]]],
            },
            Shape::LUMINANCE,
        ];
        let mut edit = Edit::default();
        edit.light.exposure = 0.3;
        edit.adjustments.push(Adjustment {
            id: 1,
            name: "Every shape".into(),
            mask: Mask {
                components: shapes
                    .into_iter()
                    .map(|shape| Component {
                        shape,
                        ..Component::default()
                    })
                    .collect(),
                ..Mask::default()
            },
            ..Adjustment::default()
        });
        for (id, method, source) in [
            (1, Method::Heal, Some([0.05, -0.03])),
            (2, Method::Clone, Some([-0.1, 0.2])),
            (3, Method::Fill, None),
        ] {
            edit.retouch.patches.push(Patch {
                id,
                method,
                points: vec![[0.31, 0.77], [0.32, 0.78], [0.33, 0.8]],
                radius: 0.02,
                source,
                ..Patch::default()
            });
        }
        let mut s = old.clone();
        s.record(edit.clone());
        s.take_snapshot("Masked", 1_700_000_500);
        edit.light.exposure = 0.6;
        s.record(edit);
        s.new_version(None, None).unwrap();
        laid_out_as_pretty(&s);

        laid_out_as_pretty(&Sidecar::default());

        // A stroke's points on one line, as the compact form writes them.
        let points = serde_json::to_string(&stroke(138, Op::Add).points).unwrap();
        assert!(s.to_json().contains(&format!("\"points\": {points}")));
    }
}
