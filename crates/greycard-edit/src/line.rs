//! One line of development: the current edit, the history before it,
//! the snapshots, and the exports made along it.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Edit, history, mask, meta, retouch, sync};

/// The shape of a frame's developed picture, as something that had it
/// said: its width over its height under the frame's turn `turn`. What
/// a state placed under another turn is shown by when it has something
/// placed on the picture ([`mask::Turned`]), and what version 4's step
/// reads ([`Edit::migrate_with_frame`]).
///
/// Written beside the turn while any state of the file is placed under
/// another turn or waits on version 4's step ([`crate::Sidecar::shape`]):
/// the writer that turned the frame measured it, so every reader after
/// shows those states as that writer did, with no file of its own to
/// measure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    /// The frame's turn the aspect was measured under.
    pub turn: u8,
    /// Width over height under `turn`.
    pub aspect: f32,
}

impl Measured {
    /// `aspect` measured under the frame's turn `turn`, when it is a
    /// shape at all: a positive, finite number.
    pub fn new(turn: u8, aspect: f32) -> Option<Self> {
        (aspect.is_finite() && aspect > 0.0).then_some(Self {
            turn: turn % 4,
            aspect,
        })
    }

    /// The picture's width over its height under the frame's turn
    /// `turn`: the measured aspect, or its inverse a quarter turn
    /// either way, so the turn it was measured under gives it back to
    /// the bit.
    pub fn aspect_at(self, turn: u8) -> f32 {
        if (turn.wrapping_sub(self.turn)) % 2 == 1 {
            1.0 / self.aspect
        } else {
            self.aspect
        }
    }
}

/// What the file holds of a state, beside the copy of it shown (§724).
///
/// A state is placed under the frame's turn it was recorded at and is
/// shown under the sidecar's turn now. For a frame not turned since,
/// the two are one edit and nothing more is kept. For the rest the
/// state as written is kept here, with the shown copy as it was made
/// from it: a change made to the shown copy in place (a look renamed, a
/// repair source taken, the seed of a frame nobody developed) is then
/// told apart, and the state is written as the shown copy under the
/// turn it is shown at, as every state was before §724. Not while the
/// shown copy waits on the frame's shape: what is placed on it is not
/// where the picture has it, so a change made to it in place is not
/// kept, and the state stays as written, and waiting, until it is shown
/// again with a shape ([`Kept::holds`]).
#[derive(Debug, Clone, Default)]
pub struct Written {
    /// The frame's turn the shown copy is in.
    shown_for: u8,
    kept: Option<Box<Kept>>,
    /// Read from a state a build before version 10 wrote: one with no id
    /// is given the one such a build gave it ([`sync::filled_id`]).
    before_placed: bool,
    /// The state as written, made once ([`crate::sparse::Cache`]).
    cache: crate::sparse::Cache,
}

#[derive(Debug, Clone)]
struct Kept {
    /// The turn the state was placed under.
    under: u8,
    /// The state as written, its `placed_under` at 0 (it is `under`).
    edit: Edit,
    /// The shown copy as it was made from `edit`.
    made: Edit,
    /// The shown copy waits on the frame's shape: positions to turn,
    /// or version 4's step, with no [`Measured`] to do it by.
    waiting: bool,
    /// Of that, positions left where they were placed under another
    /// turn: the shown copy is not the picture, and nothing may be
    /// made from it ([`Line::current_waits`]).
    unturned: bool,
    /// What turned the positions of `edit` into the shown copy's, when
    /// any were turned: a chosen source is kept in the frame the state
    /// was placed under and shown turned by it (§724).
    turned: Option<mask::Turned>,
}

impl Kept {
    /// Whether the state as written is the word on the shown copy
    /// `shown`: the copy was made from it and not changed since, or it
    /// waits on the frame's shape. A wait fails closed: a copy that
    /// waits, changed in place, is shown again from the state as
    /// written, and the change is not kept.
    fn holds(&self, shown: &Edit) -> bool {
        self.waiting || self.made == *shown
    }
}

impl Written {
    /// A state placed under the turn it is shown at, `turn`.
    pub(crate) fn at(turn: u8) -> Self {
        Self {
            shown_for: turn,
            kept: None,
            before_placed: false,
            cache: Default::default(),
        }
    }

    /// A state just read, `edit` as the file has it: placed under the
    /// turn its `placed_under` says, and shown there until the sidecar
    /// shows it under its own ([`show_state`]). The edit's
    /// `placed_under` is taken out into this, so a copy shown never
    /// carries one. `before_placed` when a build before version 10 wrote
    /// it ([`ReadEdit`]).
    pub(crate) fn read(edit: &mut Edit, before_placed: bool) -> Self {
        let mut written = Self::at(edit.placed_under % 4);
        written.before_placed = before_placed;
        edit.placed_under = 0;
        written
    }

    /// The id of the state whose shown copy is `shown`, with `label`,
    /// after the state `parent`, for a state that has none: as a build
    /// before version 10 gave it for one that build wrote, and as it is
    /// recorded now for the rest.
    pub(crate) fn filled_id(&self, shown: &Edit, parent: &str, label: Option<&str>) -> String {
        if self.before_placed {
            sync::filled_id(parent, &self.edit(shown), label)
        } else {
            sync::state_id(parent, &self.edit(shown), label)
        }
    }

    /// The frame's turn the shown copy is in.
    pub(crate) fn shown_for(&self) -> u8 {
        self.shown_for
    }

    /// The turn the state whose shown copy is `shown` was placed under,
    /// and the edit written but for that (its `placed_under` is 0 but
    /// on a state not shown yet, which carries the turn there).
    fn parts<'a>(&'a self, shown: &'a Edit) -> (u8, &'a Edit) {
        match &self.kept {
            Some(k) if k.holds(shown) => (k.under, &k.edit),
            _ if shown.placed_under != 0 => (shown.placed_under, shown),
            _ => (self.shown_for, shown),
        }
    }

    /// The state as the file has it, `placed_under` and all.
    /// The state as the file writes it, of the shown copy `shown`:
    /// [`Self::edit`] less its defaults ([`crate::sparse`]), made once.
    pub(crate) fn sparse(&self, shown: &Edit) -> std::sync::Arc<(Edit, serde_json::Value)> {
        self.cache.written(&self.edit(shown))
    }

    pub(crate) fn edit<'a>(&'a self, shown: &'a Edit) -> Cow<'a, Edit> {
        let (under, edit) = self.parts(shown);
        if edit.placed_under == under {
            Cow::Borrowed(edit)
        } else {
            let mut edit = edit.clone();
            edit.placed_under = under;
            Cow::Owned(edit)
        }
    }

    /// The turn the state was placed under.
    pub fn placed_under(&self, shown: &Edit) -> u8 {
        self.parts(shown).0
    }

    /// Whether the shown copy waits on the frame's shape.
    pub(crate) fn waiting(&self, shown: &Edit) -> bool {
        match &self.kept {
            Some(k) if k.holds(shown) => k.waiting,
            // Recorded since the last show, or changed in place: as it
            // stands.
            _ => shown.needs_frame(),
        }
    }

    /// Whether the shown copy has positions left where they were placed
    /// under another turn, for want of the frame's shape.
    pub(crate) fn unturned(&self, shown: &Edit) -> bool {
        match &self.kept {
            Some(k) if k.holds(shown) => k.unturned,
            // Recorded since the last show, or changed in place: placed
            // under the turn it is shown at.
            _ => false,
        }
    }

    /// Whether showing the state takes the frame's shape: it has work
    /// placed on the picture ([`Edit::placed`]) under another turn than
    /// it is shown at, or wants version 4's step. A crop or a keystone
    /// turns with no shape. What has the file write the shape
    /// ([`crate::Sidecar::shape`]).
    fn needs_shape(&self, shown: &Edit) -> bool {
        let (under, edit) = self.parts(shown);
        (under % 4 != self.shown_for && edit.placed()) || edit.needs_frame()
    }

    /// Whether two states are written the same.
    fn same(&self, shown: &Edit, other: &Self, other_shown: &Edit) -> bool {
        let (a, b) = (self.parts(shown), other.parts(other_shown));
        a.0 % 4 == b.0 % 4 && {
            if a.1.placed_under == b.1.placed_under {
                a.1 == b.1
            } else {
                self.edit(shown) == other.edit(other_shown)
            }
        }
    }

    /// The shown copy `shown` given the sources `sources` keeps for its
    /// patches, the state as written left as it was: a chosen source is
    /// no change to the state (§724). A patch the state holds a source
    /// for keeps its own; one it holds none for shows the one `sources`
    /// keeps, over one shown before. Not while the copy waits on the
    /// frame's shape, whose patches are not where the picture has them.
    /// True when any patch's source moved.
    fn fill_sources(&mut self, shown: &mut Edit, sources: &Sources) -> bool {
        if sources.is_empty() || self.waiting(shown) {
            return false;
        }
        // Each patch's source as kept for it in the frame the state was
        // placed under, turned to the shown copy's; none for one the
        // state as written holds a source for. The shown copy's patches
        // are the written state's, in the same order.
        let (under, written) = self.parts(shown);
        let Some(turned) = self.turned(shown) else {
            return false;
        };
        let found: Vec<Option<mask::Pos>> = written
            .retouch
            .patches
            .iter()
            .map(|w| {
                if w.source.is_some() || !w.method.sourced() {
                    return None;
                }
                sources
                    .get(&source_key(w, under))
                    .map(|o| turned.offset(*o))
            })
            .collect();
        let mut filled = shown.clone();
        let mut moved = false;
        for (patch, s) in filled.retouch.patches.iter_mut().zip(&found) {
            if let Some(s) = s
                && patch.source != Some(*s)
            {
                patch.source = Some(*s);
                moved = true;
            }
        }
        if !moved {
            return false;
        }
        match self.kept.as_deref_mut() {
            Some(k) if k.holds(shown) => k.made = filled.clone(),
            _ => {
                let (under, edit) = self.parts(shown);
                let mut edit = edit.clone();
                edit.placed_under = 0;
                self.kept = Some(Box::new(Kept {
                    under,
                    edit,
                    made: filled.clone(),
                    waiting: false,
                    unturned: false,
                    turned: None,
                }));
            }
        }
        *shown = filled;
        true
    }

    /// What turns a position of the state as written into the shown
    /// copy `shown`'s: none needed under the turn it was placed under,
    /// the show's when it was turned, and None when it was placed
    /// under another turn and nothing turned it.
    fn turned(&self, shown: &Edit) -> Option<mask::Turned> {
        let identity = mask::Turned::new(0, 1.0);
        if self.parts(shown).0 % 4 == self.shown_for {
            return Some(identity);
        }
        match &self.kept {
            Some(k) if k.holds(shown) => k.turned,
            _ => None,
        }
    }

    /// The key and the offset, in the frame the state was placed
    /// under, of a source the engine chose for `chosen` (its source
    /// left out) as it saw it on the shown copy `shown`: None when the
    /// copy holds no such patch, holds it with a source placed by hand,
    /// or waits on the frame's shape.
    fn chosen_for(
        &self,
        shown: &Edit,
        chosen: &retouch::Patch,
        source: mask::Pos,
    ) -> Option<(String, mask::Pos)> {
        if self.waiting(shown) {
            return None;
        }
        let i = shown.retouch.patches.iter().position(|p| {
            let mut p = p.clone();
            p.source = None;
            p == *chosen
        })?;
        let (under, written) = self.parts(shown);
        let w = written.retouch.patches.get(i)?;
        if w.source.is_some() {
            return None;
        }
        let back = self.turned(shown)?.back();
        Some((source_key(w, under), back.offset(source)))
    }

    /// The look `from` named `to`, in the shown copy and in the state
    /// as written alike: a look's name is no position, and a state
    /// renamed in both is still the one it was made from.
    fn rename_look(&mut self, shown: &mut Edit, from: &str, to: &str) -> bool {
        match self.kept.as_deref_mut() {
            Some(k) if k.holds(shown) => {
                k.edit.look_lut.rename(from, to);
                k.made.look_lut.rename(from, to);
            }
            _ => {}
        }
        shown.look_lut.rename(from, to)
    }
}

/// The state `edit` with what the file holds of it, `written`, shown
/// under the frame's turn `turn`: made again from the state as written,
/// never from the copy shown before, so a turn and its way back give
/// back the same bits. `shape` is the picture's, when known.
pub(crate) fn show_state(
    edit: &mut Edit,
    written: &mut Written,
    turn: u8,
    shape: Option<Measured>,
) {
    let shown = std::mem::take(edit);
    let before_placed = written.before_placed;
    let (under, mut body) = match written.kept.take() {
        Some(k) if k.holds(&shown) => (k.under, k.edit),
        _ if shown.placed_under != 0 => (shown.placed_under, shown),
        _ => (written.shown_for, shown),
    };
    body.placed_under = 0;
    body.unturned = false;
    let under = under % 4;
    if under == turn && !body.needs_frame() {
        *edit = body;
        *written = Written::at(turn);
        written.before_placed = before_placed;
        return;
    }
    let (shown, waits, turned) = shown_from(&body, under, turn, shape);
    *written = Written {
        cache: Default::default(),
        shown_for: turn,
        kept: Some(Box::new(Kept {
            under,
            edit: body,
            made: shown.clone(),
            waiting: waits.step || waits.unturned,
            unturned: waits.unturned,
            turned,
        })),
        before_placed,
    };
    *edit = shown;
}

/// What a shown copy waits on the frame's shape for ([`shown_from`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Waits {
    /// Version 4's step, left undone.
    pub(crate) step: bool,
    /// Positions on the picture, left where they were placed.
    pub(crate) unturned: bool,
}

/// `written`, placed under the turn `under`, as shown under `turn`:
/// version 4's step done in the frame it was placed in, then carried
/// round by the turn between them ([`Edit::turn`]). Beside it, what it
/// waits on the frame's shape for, which `shape` does not say: version
/// 4's step is left, and positions stay where they are while the crop
/// and the keystone, which need no shape, turn.
pub(crate) fn shown_from(
    written: &Edit,
    under: u8,
    turn: u8,
    shape: Option<Measured>,
) -> (Edit, Waits, Option<mask::Turned>) {
    let mut edit = written.clone();
    edit.placed_under = 0;
    edit.unturned = false;
    let mut waits = Waits::default();
    let mut turned = None;
    if edit.needs_frame() {
        match shape {
            Some(m) => edit.migrate_with_frame(m.aspect_at(under) < 1.0),
            None => waits.step = true,
        }
    }
    let quarters = (i32::from(turn) - i32::from(under)).rem_euclid(4);
    if quarters != 0 {
        let positions = if edit.placed() {
            match shape {
                Some(m) => Some(mask::Turned::new(quarters, m.aspect_at(under))),
                None => {
                    waits.unturned = true;
                    None
                }
            }
        } else {
            None
        };
        edit.turn(quarters, positions);
        turned = positions;
    }
    edit.unturned = waits.unturned;
    (edit, waits, turned)
}

/// One line of development: the current edit and the steps that led
/// to it, the states kept by name, and the exports made along it.
///
/// The sidecar's own line is version 0, the file's own version
/// (the sidecar's `line`); each further version holds a line of its own
/// ([`Version::line`]). Undo, redo, the snapshots, the export records
/// and the history's cap are all a line's, so a version walks its own
/// history and nothing it does touches another's.
///
/// Each state is held twice (§724): as shown, in the sidecar's turn,
/// which is what [`Self::current`], [`Step::edit`] and
/// [`Snapshot::edit`] are and what everything that reads a state reads;
/// and as written, placed under the turn it was recorded at, which is
/// what the file holds, what an id is made from, and what two lines are
/// equal by.
///
/// [`Version::line`]: crate::Version::line
// Read only as a sidecar's ([`read_line`]): no `Deserialize`, so no
// reader can make a line nobody developed without the sidecar's reader
// giving it a start (§724).
#[derive(Debug, Clone, Default)]
pub struct Line {
    /// The current state, as shown: read through [`Self::current`],
    /// changed through [`Self::record`] and the rest, never in place
    /// under its id (§724). Not public, so no copy marked as
    /// waiting on the frame's shape ([`Edit::unturned`]) can be put in
    /// its place from outside (§724).
    pub(crate) current: Edit,
    /// What the file holds of [`Self::current`], read through
    /// [`Self::current_written`]'s method. The crate's own, as the
    /// current state is.
    pub(crate) current_written: Written,
    /// The words the step that made [`Self::current`] was recorded
    /// with, when it was one with a name of its own (a preset, a
    /// sync, a snapshot restored): see [`Step::label`]. Written as
    /// `step` just after `current`, left out when there is none, and
    /// read loosely.
    pub(crate) current_label: Option<String>,
    /// The exports made from [`Self::current`]: see [`Step::exports`].
    /// Written as `exported` after `step`, left out when there are
    /// none, and read loosely, a record at a time.
    pub current_exports: Vec<Exported>,
    /// The last export made of the frame, whatever state it was made
    /// from: a fact about what went out, not the state, so it outlives
    /// an undo past that state, a history that replaces it and the
    /// history's cap, none of which a record on a state does. What went
    /// out is a version's picture, so each line has its own. Written
    /// as `last_export` after `exported`, left out when the frame has
    /// never been exported (or its sidecar is from before the field,
    /// which gets it on its next export) and read loosely.
    pub last_export: Option<Exported>,
    /// The id of the current state: see [`Step::id`]. Written as `id`
    /// after `exported`, left out when the state has none yet, and
    /// read loosely.
    pub(crate) current_id: Option<String>,
    /// The id of the state the current one was made from: see
    /// [`Step::parent`]. Written as `parent` after `id`, left out for a
    /// root, and read loosely.
    pub(crate) current_parent: Option<String>,
    /// Earlier states, oldest first, each one a whole edit and the
    /// words it was recorded with, if any.
    pub history: Vec<Step>,
    /// States undone, newest first; kept for the session, not written.
    pub redo: Vec<Step>,
    /// The ids of the states undone and then left by a new step, or
    /// read from the file's `undone` (§724): kept so a copy can tell a
    /// state it undid from one it never had. Only the ids: a file that
    /// travels keeps nothing of what the user took back. Written as
    /// `undone`, with the redo stack's after them, less any the line
    /// holds, the last [`HISTORY`] of them.
    pub undone: Vec<String>,
    /// When this session made [`Self::current`] current, by a record
    /// or a redo: see [`Step::order`].
    pub current_order: Order,
    /// The line's own id (§724), kept wherever it goes: a swap with the
    /// original moves lines between versions, each under its id, and a
    /// join joins lines by it. A version's line is given its version's
    /// id when made; the original's is made from its root state
    /// ([`crate::sync::line_id_of_root`]), so two copies migrated apart
    /// agree. Given at the save ([`crate::Sidecar::fill_ids`]); empty,
    /// and not written, in a line nobody developed.
    ///
    /// Unique within one frame's file and its copies only: two frames
    /// whose originals start alike (a default root) have one id. A line
    /// id is compared between copies of one file, never across files
    /// (a join is of one frame's copies; a virtual copy is a version of
    /// its frame's file).
    pub line_id: String,
    /// When and by which install the current state was last made
    /// current (§724): a record, an undo, a redo, a go-to. Written as
    /// `head_at`, none in a line nobody developed. The later of two copies' is the join's;
    /// which head a join keeps by it is the new join's (step 7). Not in
    /// two lines' equality: it is when a line moved, not what it holds.
    pub head_at: Option<HeadAt>,
    /// States kept by name, in the order they were taken, outside
    /// the history's cap.
    pub snapshots: Vec<Snapshot>,
    /// The snapshots removed from this line, by id, each at the time
    /// it went (§724), this machine's clock: written as
    /// `snapshots_removed`, left out when none, so a join can tell a
    /// snapshot removed on one copy from one taken on the other. A join
    /// that keeps the snapshot drops its tombstone. At most
    /// [`TOMBSTONES`], the oldest going first.
    pub snapshots_removed: BTreeMap<String, u64>,
    /// Whether [`Self::current`] is a recorded state. A line nobody
    /// developed holds none (§724): its current is the frame's start,
    /// made by whoever reads the file from the settings and the frame
    /// (the lens for new edits, the ISO's blend), with no id, and
    /// nothing of it is written. The first thing that needs a state (a
    /// step, an export record, a snapshot) records the start whole as
    /// the root; until then a seed changes the start ([`Self::seed`])
    /// and rewrites nothing.
    pub(crate) recorded: bool,
    /// Whether the start of a line nobody developed has taken its seed
    /// (§724): the ISO's blend from an open or an export, or a version 9
    /// line's seeded blend kept by the migration. A start with its seed is
    /// the more specific of two, which a join or a save's read keeps
    /// ([`Self::keep_start_of`]). Not written: nothing of such a line is.
    pub(crate) seeded: bool,
    /// The repair sources the engine chose for this line's patches, by
    /// [`source_key`] (§724). A chosen source is derived, not placed:
    /// the same patch on the same picture gets it again. So it is kept
    /// here, beside the states and never in one, and every shown copy
    /// takes its patches' from here ([`Self::take_sources`]); a source in
    /// a state is one placed by hand. Written as `sources`, left out
    /// when there are none, and read loosely.
    pub(crate) sources: Sources,
    /// How many of the file's states could not be read (§724): a
    /// later build's difference this reader does not wholly understand,
    /// or one whose base is gone. Not written; while there are any, the
    /// sidecar is not written either ([`crate::Sidecar::unread`]).
    pub(crate) unread: usize,
}

/// The repair sources chosen for a line's patches, by [`source_key`].
pub type Sources = std::collections::BTreeMap<String, mask::Pos>;

/// The key a chosen source is kept under: the patch as the state
/// holding it was written, placed under the frame's turn `under`, by
/// what places its pixels (the method, its strokes' points and radii,
/// the feather and opacity) and that turn. The offset kept is in that
/// frame, and turned as the patch is when shown under another (§724):
/// a quarter turn maps the engine's search onto itself, so the choice
/// is the same picked over again. Made from the named fields, so a new
/// field of a patch, or a field renamed, keys no source anew; the
/// golden file pins the recipe.
pub(crate) fn source_key(patch: &retouch::Patch, under: u8) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"greycard source key 1");
    hasher.update(&[under % 4]);
    hasher.update(&[match patch.method {
        retouch::Method::Heal => 0,
        retouch::Method::Clone => 1,
        retouch::Method::Fill => 2,
    }]);
    let mut f = |x: f32| {
        hasher.update(&x.to_bits().to_le_bytes());
    };
    let mut stroke = |points: &[mask::Pos], radius: f32| {
        f(points.len() as f32);
        for p in points {
            f(p[0]);
            f(p[1]);
        }
        f(radius);
    };
    stroke(&patch.points, patch.radius);
    for s in &patch.more {
        stroke(&s.points, s.radius);
    }
    f(patch.feather);
    f(patch.opacity);
    hasher.finalize().to_hex()[..32].to_string()
}

/// An edit as a file has it, read: the edit, and whether a build before
/// version 10 wrote it, which the migration marks
/// ([`crate::migration::BEFORE_PLACED_MARK`]) and this takes out.
#[derive(Default)]
struct ReadEdit {
    edit: Edit,
    before_placed: bool,
}

impl ReadEdit {
    /// The edit, its `placed_under` taken into what the file holds of
    /// it ([`Written::read`]).
    fn into_parts(self) -> (Edit, Written) {
        let mut edit = self.edit;
        let written = Written::read(&mut edit, self.before_placed);
        (edit, written)
    }

    /// `value` read, the mark taken out first.
    fn of(mut value: serde_json::Value) -> serde_json::Result<Self> {
        let before_placed = value
            .as_object_mut()
            .and_then(|o| o.remove(crate::migration::BEFORE_PLACED_MARK))
            .is_some();
        Ok(Self {
            edit: Edit::deserialize(value)?,
            before_placed,
        })
    }
}

impl<'de> Deserialize<'de> for ReadEdit {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::of(serde_json::Value::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// A line as the file has it, read: [`Line`] but for what is not
/// written, each state's `placed_under` taken into what the file holds
/// of it ([`Written::read`]).
#[derive(Deserialize, Default)]
#[serde(default)]
struct LineIn {
    current: Option<ReadEdit>,
    #[serde(rename = "step", deserialize_with = "meta::loose")]
    current_label: Option<String>,
    #[serde(rename = "exported", deserialize_with = "loose_exports")]
    current_exports: Vec<Exported>,
    #[serde(deserialize_with = "meta::loose")]
    last_export: Option<Exported>,
    #[serde(rename = "id", deserialize_with = "meta::loose")]
    current_id: Option<String>,
    #[serde(rename = "parent", deserialize_with = "meta::loose")]
    current_parent: Option<String>,
    history: Vec<Step>,
    /// Read loosely, each id that is a string kept: a later build's
    /// other entries are none of this one's.
    undone: Vec<serde_json::Value>,
    #[serde(deserialize_with = "meta::loose")]
    line_id: String,
    #[serde(deserialize_with = "meta::loose")]
    head_at: Option<HeadAt>,
    snapshots: Vec<Snapshot>,
    #[serde(deserialize_with = "meta::loose")]
    snapshots_removed: BTreeMap<String, u64>,
    #[serde(deserialize_with = "meta::loose")]
    sources: Sources,
    #[serde(rename = "#unread")]
    unread: usize,
}

/// A line as a sidecar's reader reads it, for its fields' `deserialize_with`:
/// the crate's own way in, the start given after by the sidecar's reader.
pub(crate) fn read_line<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Line, D::Error> {
    LineIn::deserialize(d).map(Line::from)
}

impl From<LineIn> for Line {
    fn from(read: LineIn) -> Self {
        let LineIn {
            current,
            current_label,
            current_exports,
            last_export,
            current_id,
            current_parent,
            history,
            undone,
            line_id,
            head_at,
            snapshots,
            snapshots_removed,
            mut sources,
            unread,
        } = read;
        // An offset too large for an f32 reads as infinite, which writes
        // as null and does not read back: no source at all, so the engine
        // chooses again.
        sources.retain(|_, o| o.iter().all(|v| v.is_finite()));
        // No current state written: a line nobody developed (§724).
        let recorded = current.is_some();
        // Such a line writes none of what follows a state, so none of it
        // is read: a file that has it with no current state (no build
        // writes one) would otherwise be held here as it is not written,
        // its history asking for a shape its next read does not.
        let (history, undone, line_id, head_at, mut snapshots) = if recorded {
            (history, undone, line_id, head_at, snapshots)
        } else {
            Default::default()
        };
        fill_snapshot_ids(&mut snapshots);
        let (current, current_written) = current.unwrap_or_default().into_parts();
        let mut line = Line {
            current_written,
            current,
            current_label,
            current_exports,
            last_export,
            current_id,
            current_parent,
            history,
            redo: Vec::new(),
            undone: undone
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|i| !i.is_empty())
                .map(str::to_string)
                .collect(),
            current_order: Order::BEFORE,
            line_id: line_id.trim().to_string(),
            head_at,
            snapshots,
            snapshots_removed,
            recorded,
            seeded: false,
            sources,
            unread,
        };
        line.parents_read_at_nine();
        // An undone id the line holds stands for no undone state.
        let mut undone = std::mem::take(&mut line.undone);
        cap_undone(&mut undone, &line.held_ids());
        line.undone = undone;
        line
    }
}

/// A line as the file has it, written: every state as written, under
/// the keys and in the order a line has always been written with.
#[derive(Serialize)]
struct LineOut<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<&'a serde_json::Value>,
    #[serde(rename = "step", skip_serializing_if = "Option::is_none")]
    current_label: &'a Option<String>,
    #[serde(rename = "exported", skip_serializing_if = "Vec::is_empty")]
    current_exports: &'a Vec<Exported>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_export: &'a Option<Exported>,
    #[serde(rename = "id", skip_serializing_if = "Option::is_none")]
    current_id: &'a Option<String>,
    #[serde(rename = "parent", skip_serializing_if = "Option::is_none")]
    current_parent: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history: Option<&'a Vec<Step>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    undone: Vec<String>,
    #[serde(skip_serializing_if = "str::is_empty")]
    line_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_at: Option<&'a HeadAt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshots: Option<&'a Vec<Snapshot>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    snapshots_removed: &'a BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Sources::is_empty")]
    sources: &'a Sources,
}

impl Serialize for Line {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        // A line nobody developed writes no state (§724): only what
        // went out of it, which outlives a state.
        let recorded = self.recorded;
        let current = recorded.then(|| self.current_written.sparse(&self.current));
        LineOut {
            current: current.as_ref().map(|c| &c.1),
            current_label: &self.current_label,
            current_exports: &self.current_exports,
            last_export: &self.last_export,
            current_id: &self.current_id,
            current_parent: &self.current_parent,
            history: recorded.then_some(&self.history),
            undone: if recorded {
                self.undone_written()
            } else {
                Vec::new()
            },
            line_id: if recorded { &self.line_id } else { "" },
            head_at: self.head_at.as_ref().filter(|_| recorded),
            snapshots: recorded.then_some(&self.snapshots),
            snapshots_removed: &self.snapshots_removed,
            sources: &self.sources,
        }
        .serialize(serializer)
    }
}

/// Two lines are equal when the file holds the same of them and the
/// same was undone: each state as written (§724), whatever turn it is
/// shown under, which the sidecar's own equality adds.
impl PartialEq for Line {
    fn eq(&self, other: &Self) -> bool {
        let Line {
            current,
            current_written,
            current_label,
            current_exports,
            last_export,
            current_id,
            current_parent,
            history,
            redo: _,
            undone: _,
            current_order,
            line_id,
            head_at: _,
            snapshots,
            snapshots_removed,
            recorded,
            seeded: _,
            sources,
            unread: _,
        } = self;
        *recorded == other.recorded
            && *sources == other.sources
            && current_written.same(current, &other.current_written, &other.current)
            && *current_order == other.current_order
            && *current_label == other.current_label
            && *current_exports == other.current_exports
            && *last_export == other.last_export
            && *current_id == other.current_id
            && *current_parent == other.current_parent
            && *history == other.history
            // What was undone as the file has it: the redo stack is
            // written as undone (§724).
            && self.undone_written() == other.undone_written()
            && *snapshots == other.snapshots
            && *snapshots_removed == other.snapshots_removed
            // None yet is the one the save gives it, made from the line
            // ([`crate::Sidecar::fill_ids`]): equal to any.
            && (line_id.is_empty() || other.line_id.is_empty() || *line_id == other.line_id)
    }
}

/// One state of the history: a whole edit, and the words the step
/// that made it was recorded with when it had a name of its own.
///
/// The label goes with the state it produced, so an undo carries it
/// to the redo stack and a redo brings it back, and a new step after
/// an undo drops it along with the rest of the redo stack. Most steps
/// have none, and the panel names those by what moved
/// ([`history::describe`]); a preset, a sync and a snapshot restored
/// are recorded with theirs ([`Sidecar::record_as`]), and the panel
/// reads it in place of the list of sections that moved
/// ([`history::describe_step`]).
///
/// Written as the edit's own object with a `step` key beside its
/// fields, and that key left out when there is no label: a sidecar
/// with no labels writes exactly as one did before labels existed,
/// and a build from before them reads each state as the edit it is
/// (an edit ignores a field it does not know), labels dropped. Read
/// loosely, as the meta is: a `step` that is not a string reads as
/// no label rather than costing the state.
///
/// The exports made from the state ride on it the same way, under an
/// `exported` key beside `step`: see [`Step::exports`].
///
/// [`Sidecar::record_as`]: crate::Sidecar::record_as
#[derive(Debug, Clone, Default)]
pub struct Step {
    /// The state as shown, in the sidecar's turn: read through
    /// [`Self::edit`], never changed from outside the crate, since a
    /// change to a shown copy is written (§724).
    pub(crate) edit: Edit,
    /// Read through [`Self::label`]: set where the state is recorded,
    /// and part of its id (§724).
    pub(crate) label: Option<String>,
    /// The files written from this state, oldest first. A record of
    /// what went out, not a change: it adds no state to walk, so undo
    /// and redo step over it and carry it with its state, and a
    /// state that goes (undone and then replaced, or past the cap)
    /// takes its records with it.
    pub exports: Vec<Exported>,
    /// The state's id: blake3 of its parent state's id, the edit as
    /// compact JSON and the label ([`sync::state_id`]), computed once
    /// when the state is recorded and carried with it through undo
    /// and redo. The same state reached on two machines from the same
    /// parent has the same id, which is how two copies of one frame's
    /// sidecar find the states they share (§233). None only on a
    /// state from a sidecar written before ids, until its first read
    /// fills it ([`Sidecar::fill_ids`]). Written as `id` beside
    /// `step` and `exported`, and read loosely.
    ///
    /// [`Sidecar::fill_ids`]: crate::Sidecar::fill_ids
    pub(crate) id: Option<String>,
    /// The id of the state this one was made from (§724): the current
    /// state when it was recorded, whatever stands before it in a list
    /// since (a join's branch carried over, a repeat the cap took).
    /// What a join asks to tell a chain from a branch, so no id is ever
    /// made again from content to answer it. None for a root, and for
    /// a state a build before version 10 wrote that does not hang off
    /// the state before it ([`Line::parents_read_at_nine`]). Written as
    /// `parent` beside `id`, and read loosely.
    pub(crate) parent: Option<String>,
    /// When this session last acted on the state, never written: for
    /// one in the history, when it was made current (recorded, or
    /// redone); for one waiting to be redone, when it was undone. So
    /// an undo takes back the later of two kinds of step, and a redo
    /// makes again the one undone last. [`Order::BEFORE`] for one read
    /// from the sidecar or carried over by a join.
    pub order: Order,
    /// What the file holds of the state ([`Written`]).
    pub written: Written,
}

/// Two states are equal when the file holds the same of them: as
/// written, with the same words, records and id, and the same order
/// (which every order is: see [`Order`]).
impl PartialEq for Step {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
            && self.exports == other.exports
            && self.id == other.id
            && self.parent == other.parent
            && self.order == other.order
            && self.written.same(&self.edit, &other.written, &other.edit)
    }
}

/// Where a step stands in the session's order of things done: a tick
/// of one clock the process keeps for every state a line records and
/// every rating, flag, or label change the window records, so that
/// undo can take back whichever of the two was done last (§117's
/// marks have a history of their own, across frames, beside each
/// line's). Not part of what a state is: two states equal in all else
/// are equal whatever their order, so a state read back from disk
/// equals the one written.
#[derive(Debug, Clone, Copy, Default, Eq)]
pub struct Order(u64);

impl PartialEq for Order {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How far apart two acts' ticks are: room for the ticks of the work
/// one act's request does when it runs later ([`Order::during`]).
const STRIDE: u64 = 1 << 20;

thread_local! {
    /// The act whose work is running ([`Order::during`]): its tick,
    /// and how many ticks its work has taken so far.
    static AS_OF: std::cell::Cell<Option<(u64, u64)>> = const { std::cell::Cell::new(None) };
}

impl Order {
    /// A state from before the session: older than anything done in it.
    pub const BEFORE: Self = Self(0);

    /// The next tick: later than every one handed out before it. Inside
    /// [`Order::during`], the next of the act's own, which stay before
    /// every act after it however late its work runs.
    pub fn next() -> Self {
        if let Some((at, taken)) = AS_OF.get() {
            let taken = (taken + 1).min(STRIDE - 1);
            AS_OF.set(Some((at, taken)));
            return Self(at + taken);
        }
        Self(CLOCK.fetch_add(STRIDE, std::sync::atomic::Ordering::Relaxed) + STRIDE)
    }

    /// The tick of an act being made now, to carry to where its work
    /// is done: a new one, or inside [`Order::during`] the running
    /// act's, whose work this is part of.
    pub fn act() -> Self {
        match AS_OF.get() {
            Some((at, _)) => Self(at),
            None => Self::next(),
        }
    }

    /// `work` done in the name of the act ticked `at`: a request queued
    /// at a key's press and run when its read lands. Every tick it
    /// takes is the act's, so what it records stands where the press
    /// did, before anything done after the press and before the work.
    pub fn during<R>(at: Self, work: impl FnOnce() -> R) -> R {
        struct Restore(Option<(u64, u64)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                AS_OF.set(self.0);
            }
        }
        let _restore = Restore(AS_OF.replace(Some((at.0, 0))));
        work()
    }

    /// The tick, for comparing: 0 for a state from before the session.
    pub fn tick(self) -> u64 {
        self.0
    }
}

impl Step {
    /// A state with no words of its own.
    pub fn plain(edit: Edit) -> Self {
        let mut edit = edit;
        let written = Written::read(&mut edit, false);
        Self {
            edit,
            label: None,
            exports: Vec::new(),
            id: None,
            parent: None,
            order: Order::BEFORE,
            written,
        }
    }

    /// The state as the file has it, `placed_under` and all.
    pub fn written(&self) -> Cow<'_, Edit> {
        self.written.edit(&self.edit)
    }

    /// The state as shown, in the sidecar's turn.
    pub fn edit(&self) -> &Edit {
        &self.edit
    }

    /// The words the state was recorded with: see the type's notes.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// The state's id, none until filled: see the field's notes.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The id of the state this one was made from, none for a root:
    /// see the field's notes.
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    fn parts(&self) -> (&Edit, Option<&str>) {
        (&self.edit, self.label.as_deref())
    }
}

/// An export made from a state of the history: the file written, the
/// export preset the sheet was when it was one, and when it finished.
///
/// Kept on the state it was exported from ([`Step::exports`],
/// [`Sidecar::current_exports`]) rather than as a state of its own:
/// an export changes nothing in the edit, so a step for it would be
/// an undo that does nothing, and an export that finished after the
/// panel moved on would have had to land between two states. On the
/// state, the history panel shows it as a row of its own above that
/// state's, and a click on it goes to that state
/// ([`Sidecar::rows`]).
///
/// A build from before it reads a sidecar with records as though
/// they were not there, and drops them at its next save.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Exported {
    /// The file written, as the path it was written to.
    pub file: String,
    /// The export preset's name, when the sheet was one as saved.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "meta::loose"
    )]
    pub preset: Option<String>,
    /// When it was written, seconds since the Unix epoch.
    #[serde(default, deserialize_with = "meta::loose")]
    pub at: u64,
}

impl Exported {
    /// The file's name, without its folder.
    pub fn file_name(&self) -> &str {
        Path::new(&self.file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.file)
    }

    /// What the history panel's row says: [`history::export_label`].
    pub fn label(&self) -> String {
        history::export_label(self.file_name(), self.preset.as_deref())
    }
}

/// The export records under a key, each read on its own: one that
/// will not read is left out, the rest kept, and a key that is not a
/// list reads as none.
fn loose_exports<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<Exported>, D::Error> {
    Ok(exports_from(serde_json::Value::deserialize(d)?))
}

fn exports_from(value: serde_json::Value) -> Vec<Exported> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|i| serde_json::from_value(i).ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// A row of the history panel, newest first: a state, or an export
/// made from one ([`Sidecar::rows`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The state at this index among [`Sidecar::states`].
    State(usize),
    /// Export `export` (oldest first) of the state at `state`.
    Exported { state: usize, export: usize },
}

impl Row {
    /// The state the row goes to when clicked: its own, or the one it
    /// was exported from.
    pub fn state(self) -> usize {
        match self {
            Self::State(i) | Self::Exported { state: i, .. } => i,
        }
    }
}

impl From<Edit> for Step {
    fn from(edit: Edit) -> Self {
        Self::plain(edit)
    }
}

impl PartialEq<Edit> for Step {
    fn eq(&self, other: &Edit) -> bool {
        self.edit == *other
    }
}

impl Serialize for Step {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a> {
            #[serde(flatten)]
            edit: &'a serde_json::Value,
            #[serde(skip_serializing_if = "Option::is_none")]
            step: &'a Option<String>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            exported: &'a Vec<Exported>,
            #[serde(skip_serializing_if = "Option::is_none")]
            id: &'a Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            parent: &'a Option<String>,
        }
        let written = self.written.sparse(&self.edit);
        Out {
            edit: &written.1,
            step: &self.label,
            exported: &self.exports,
            id: &self.id,
            parent: &self.parent,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let label = value
            .as_object_mut()
            .and_then(|o| o.remove("step"))
            .and_then(|s| match s {
                serde_json::Value::String(s) => Some(s),
                _ => None,
            });
        let exports = value
            .as_object_mut()
            .and_then(|o| o.remove("exported"))
            .map(exports_from)
            .unwrap_or_default();
        let mut take = |key: &str| {
            value
                .as_object_mut()
                .and_then(|o| o.remove(key))
                .and_then(|s| match s {
                    serde_json::Value::String(s) => Some(s),
                    _ => None,
                })
        };
        let id = take("id");
        let parent = take("parent");
        let (edit, written) = ReadEdit::of(value)
            .map_err(serde::de::Error::custom)?
            .into_parts();
        Ok(Self {
            edit,
            label,
            exports,
            id,
            parent,
            order: Order::BEFORE,
            written,
        })
    }
}

/// A whole edit kept under a name.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "SnapshotIn")]
pub struct Snapshot {
    /// Its own, made when it is taken from the state and the time
    /// ([`snapshot_id`]) and kept through a rename; one read with none
    /// is given it then, alike in every copy.
    pub id: String,
    pub name: String,
    /// When it was last renamed, seconds since the Unix epoch; 0 while
    /// it has the name it was taken with, and then not written.
    pub named: u64,
    /// When it was taken, seconds since the Unix epoch.
    pub taken: u64,
    /// The state as shown, in the sidecar's turn.
    pub edit: Edit,
    /// What the file holds of it ([`Written`]).
    pub written: Written,
}

impl Snapshot {
    /// `edit` kept under `name`, taken at `taken`, placed under the
    /// turn its `placed_under` says.
    pub fn new(name: impl Into<String>, taken: u64, edit: Edit) -> Self {
        let mut edit = edit;
        let written = Written::read(&mut edit, false);
        let id = snapshot_id(&written.edit(&edit), taken, 0);
        Self {
            id,
            name: name.into(),
            named: 0,
            taken,
            edit,
            written,
        }
    }

    /// The state as the file has it, `placed_under` and all.
    pub fn written(&self) -> Cow<'_, Edit> {
        self.written.edit(&self.edit)
    }
}

#[derive(Deserialize)]
struct SnapshotIn {
    #[serde(default, deserialize_with = "meta::loose")]
    id: String,
    name: String,
    #[serde(default, deserialize_with = "meta::loose")]
    named: u64,
    taken: u64,
    edit: ReadEdit,
}

impl From<SnapshotIn> for Snapshot {
    fn from(read: SnapshotIn) -> Self {
        let (edit, written) = read.edit.into_parts();
        Snapshot {
            id: read.id.trim().to_string(),
            name: read.name,
            named: read.named,
            taken: read.taken,
            edit,
            written,
        }
    }
}

/// When and by which install a line's head last moved
/// ([`Line::head_at`]): seconds since the Unix epoch on that machine's
/// clock, and the install's opaque id ([`sync::install`]), never a
/// machine's or a user's name: a sidecar travels.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct HeadAt {
    pub at: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub install: String,
}

impl HeadAt {
    /// This install, now.
    pub fn now() -> Self {
        HeadAt {
            at: crate::versions::now(),
            install: sync::install(),
        }
    }

    /// The later of two, by time then install.
    pub fn later(a: &Option<Self>, b: &Option<Self>) -> Option<Self> {
        a.clone().max(b.clone())
    }
}

/// `undone` each id once (the later kept), less those `held` (an undone
/// id never stands for a state the line still holds: the same state
/// made again after an undo has the same id), the last [`HISTORY`].
pub(crate) fn cap_undone(undone: &mut Vec<String>, held: &std::collections::HashSet<&str>) {
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<String> = Vec::with_capacity(undone.len().min(HISTORY));
    for id in std::mem::take(undone).into_iter().rev() {
        if !held.contains(id.as_str()) && seen.insert(id.clone()) {
            kept.push(id);
        }
        if kept.len() == HISTORY {
            break;
        }
    }
    kept.reverse();
    *undone = kept;
}

/// How many tombstones a map keeps (§724): the oldest go past it. A
/// tombstone outlives its use once every copy has joined past it; a
/// few hundred is years of deletes, some 25 KB at most.
pub const TOMBSTONES: usize = 500;

/// `tombstones` past [`TOMBSTONES`], the oldest (then the least id)
/// dropped.
pub(crate) fn cap_tombstones(tombstones: &mut BTreeMap<String, u64>) {
    while tombstones.len() > TOMBSTONES {
        let oldest = tombstones
            .iter()
            .min_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)))
            .map(|(id, _)| id.clone())
            .expect("not empty");
        tombstones.remove(&oldest);
    }
}

/// Both copies' tombstones, each at the later time, less those of what
/// `holds` says the join kept, and capped (§724).
pub(crate) fn joined_tombstones(
    a: &BTreeMap<String, u64>,
    b: &BTreeMap<String, u64>,
    holds: impl Fn(&str) -> bool,
) -> BTreeMap<String, u64> {
    let mut out = a.clone();
    for (id, at) in b {
        let t = out.entry(id.clone()).or_insert(*at);
        *t = (*t).max(*at);
    }
    out.retain(|id, _| !holds(id));
    cap_tombstones(&mut out);
    out
}

/// For `skip_serializing_if`: a time not set.
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A snapshot's id: a hash of the state as written and the time it was
/// taken, and `n`, counted up past one another snapshot of the line has
/// (the same state kept twice in one second). Made alike by every copy
/// and build that reads a file whose snapshots have none (§724), so
/// the two copies of one agree on it.
pub(crate) fn snapshot_id(written: &Edit, taken: u64, n: u32) -> String {
    let label = match n {
        0 => format!("snapshot {taken}"),
        n => format!("snapshot {taken} {n}"),
    };
    crate::sync::id_of(
        crate::sync::ZERO_ID,
        &crate::sparse::written_text(written),
        Some(&label),
    )
}

/// The id of a snapshot of `written` taken at `taken`, none of `ids`.
pub(crate) fn fresh_snapshot_id<'a>(
    written: &Edit,
    taken: u64,
    ids: impl Iterator<Item = &'a str> + Clone,
) -> String {
    (0..)
        .map(|n| snapshot_id(written, taken, n))
        .find(|id| !ids.clone().any(|o| o == id))
        .expect("an id")
}

/// Each snapshot read with no id, or with one an earlier snapshot has,
/// given its own ([`snapshot_id`]), in order.
fn fill_snapshot_ids(snapshots: &mut [Snapshot]) {
    for i in 0..snapshots.len() {
        let (before, rest) = snapshots.split_at_mut(i);
        let s = &mut rest[0];
        if s.id.is_empty() || before.iter().any(|o| o.id == s.id) {
            if !s.id.is_empty() {
                log::warn!(
                    "sidecar: a second snapshot with the id {} given its own",
                    s.id
                );
            }
            let written = s.written.edit(&s.edit).into_owned();
            s.id = fresh_snapshot_id(&written, s.taken, before.iter().map(|o| o.id.as_str()));
        }
    }
}

impl Serialize for Snapshot {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a> {
            id: &'a str,
            name: &'a str,
            #[serde(skip_serializing_if = "is_zero")]
            named: u64,
            taken: u64,
            edit: &'a serde_json::Value,
        }
        let written = self.written.sparse(&self.edit);
        Out {
            id: &self.id,
            name: &self.name,
            named: self.named,
            taken: self.taken,
            edit: &written.1,
        }
        .serialize(serializer)
    }
}

/// Two snapshots are equal when the file holds the same of them.
impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.named == other.named
            && self.taken == other.taken
            && self.written.same(&self.edit, &other.written, &other.edit)
    }
}

/// How many earlier states a sidecar keeps. The earliest is kept
/// whatever the count, so the state the file was opened in stays.
pub const HISTORY: usize = 50;

impl Line {
    /// A line whose one state is `edit`, with nothing before it: what
    /// a frame nobody has developed starts from. `edit` is as shown
    /// under the frame's turn `turn`, and placed under it; a line put
    /// on a sidecar under another turn is shown under that one before
    /// anything acts on it ([`crate::Sidecar::show`]). Its id is made
    /// when first asked for ([`Self::fill_ids`]).
    ///
    /// None for a copy marked as waiting on the frame's shape
    /// ([`Edit::unturned`]): its masks or patches are where they were
    /// placed under another turn, and a line started from it would
    /// place them under this one, a quarter turn off (§724).
    ///
    /// The state is recorded, the line's root; [`Self::start`] is a line
    /// nobody developed, which holds none.
    pub fn of(edit: Edit, turn: u8) -> Option<Self> {
        let mut line = Self::start(edit, turn)?;
        line.recorded = true;
        Some(line)
    }

    /// A line nobody developed (§724), starting at `edit`, as shown
    /// under the frame's turn `turn`: no state recorded, nothing of it
    /// written, until something needs one. None for a copy marked as
    /// waiting on the frame's shape, as [`Self::of`].
    pub fn start(edit: Edit, turn: u8) -> Option<Self> {
        if edit.unturned {
            log::warn!("a line is not started from a state whose masks wait on the frame's shape");
            return None;
        }
        let mut current = edit;
        current.placed_under = 0;
        Some(Self {
            current,
            current_written: Written::at(turn % 4),
            ..Self::default()
        })
    }

    /// Whether the current state is a recorded state; false for a line
    /// nobody developed (§724), whose current is the frame's start.
    pub fn recorded(&self) -> bool {
        self.recorded
    }

    /// The start of a line nobody developed changed by `change`: a seed
    /// from the frame (the ISO's blend, the lens as an export rendered
    /// it), or an import's start. Nothing is recorded or rewritten:
    /// there is no state yet (§724). False, and nothing changed, for a
    /// line with a recorded state, or when `change` changed nothing.
    pub fn seed(&mut self, change: impl FnOnce(&mut Edit)) -> bool {
        if self.recorded {
            return false;
        }
        let was = self.current.clone();
        change(&mut self.current);
        self.current.unturned = false;
        self.seeded = true;
        self.current != was
    }

    /// The start of a line nobody developed (§724), as its reader makes
    /// it from the settings and the frame; one with a recorded state is
    /// left as it is, unless a build before version 10 wrote it untouched
    /// at this very start ([`Self::untouched_at`]): then it holds no
    /// state from here on, its learned blend kept, so the picture is
    /// the one it was.
    pub(crate) fn set_start(&mut self, start: &Edit) {
        let mut start = start.clone();
        start.placed_under = 0;
        start.unturned = false;
        if self.recorded {
            if !self.untouched_at(&start) {
                return;
            }
            start.noise.learned_strength = self.current.noise.learned_strength;
            self.recorded = false;
            self.current_id = None;
            self.current_parent = None;
            // Nothing of the state read is kept: the root a step records
            // later is this build's, its id made as this build makes one.
            self.current_written = Written::at(self.current_written.shown_for());
            // Opened before, its blend was seeded: kept, and so the start
            // with its seed.
            self.seeded = start.noise.learned_strength != Edit::default().noise.learned_strength;
        } else {
            self.seeded = false;
        }
        self.current = start;
    }

    /// The start `local` holds, seed and all, when neither line holds a
    /// state (§724), unless this one has its seed and `local`'s has none:
    /// the more specific start is kept. See
    /// [`crate::Sidecar::keep_starts_of`].
    pub(crate) fn keep_start_of(&mut self, local: &Line) {
        if !self.recorded && !local.recorded && (local.seeded || !self.seeded) {
            self.current = local.current.clone();
            self.current_written = local.current_written.clone();
            self.seeded = local.seeded;
        }
    }

    /// Each state a build before version 10 wrote given its parent
    /// (§724), once, as it is read: the state before it in the file
    /// when its id is that state's child's ([`sync::verifies`], in
    /// either of the two forms such a build made ids in), and none
    /// when not (a branch an earlier join carried over, a repeat the
    /// cap took from before it). The one place an id is checked
    /// against content; from here on a join asks the parent written.
    /// A state with no id gets its parent with its id
    /// ([`Self::fill_ids`]).
    fn parents_read_at_nine(&mut self) {
        if !self.recorded {
            return;
        }
        let ids = sync::state_ids(self);
        let states = self
            .history
            .iter_mut()
            .map(|s| {
                (
                    &s.edit,
                    &s.written,
                    s.label.as_deref(),
                    &s.id,
                    &mut s.parent,
                )
            })
            .chain(std::iter::once((
                &self.current,
                &self.current_written,
                self.current_label.as_deref(),
                &self.current_id,
                &mut self.current_parent,
            )));
        for (i, (edit, written, label, id, parent)) in states.enumerate() {
            let Some(id) = id else {
                continue;
            };
            if i == 0 || parent.is_some() || !written.before_placed {
                continue;
            }
            let before = &ids[i - 1];
            if sync::verifies(before, &written.edit(edit), label, id) {
                *parent = Some(before.clone());
            }
        }
    }

    /// The start recorded whole as the line's root, when there is no
    /// state yet (§724): what a step, an export record, or a snapshot
    /// does first. Its id is made from it as written,
    /// after no state.
    pub(crate) fn record_root(&mut self) {
        if self.recorded {
            return;
        }
        self.recorded = true;
        self.current_parent = None;
        self.current_id = Some(sync::state_id(
            sync::ZERO_ID,
            &self.current_written.edit(&self.current),
            self.current_label.as_deref(),
        ));
    }

    /// Whether a build before version 10 wrote this line's one state as
    /// `start` nobody developed, its learned blend seeded or not, with
    /// nothing else (no history, snapshot, export, or words: a copy made
    /// "From the defaults", or an import that came out at the start, says
    /// what it is). Such a line holds no state (§724). The upgrade's,
    /// once: a line this build wrote reads as it was written.
    ///
    /// A seeded line (opened under a build before version 10) counts at
    /// the reader's own start only: a raw's default on a picture, or the
    /// lens section the other way from the setting, is a picture the
    /// start would not give, and stays. One never opened, its blend never
    /// seeded, was never seen developed: either lens section is the frame
    /// untouched, and it reads as a frame with no sidecar does.
    fn untouched_at(&self, start: &Edit) -> bool {
        if !self.recorded
            || !self.current_written.before_placed
            || !self.history.is_empty()
            || !self.redo.is_empty()
            || !self.snapshots.is_empty()
            || !self.current_exports.is_empty()
            || self.current_label.is_some()
        {
            return false;
        }
        let mut was = self.current_written.edit(&self.current).into_owned();
        was.placed_under = 0;
        let unseeded = was.noise.learned_strength == Edit::default().noise.learned_strength;
        was.noise.learned_strength = start.noise.learned_strength;
        // The process a state was made under is no develop.
        was.develops_as(start)
            || (unseeded
                && Edit::raw_defaults().iter().any(|d| was.develops_as(d))
                && Edit::raw_defaults().iter().any(|d| start.develops_as(d)))
    }

    /// The current state, as shown under the frame's turn: what the
    /// develop, the panel, and a copy read. While it waits on the
    /// frame's shape it is a copy marked so ([`Edit::unturned`]); see
    /// [`Self::ready`] for one that never is.
    pub fn current(&self) -> &Edit {
        &self.current
    }

    /// The words the current state was recorded with, if any.
    pub fn current_label(&self) -> Option<&str> {
        self.current_label.as_deref()
    }

    /// The current state's id, none until filled.
    pub fn current_id(&self) -> Option<&str> {
        self.current_id.as_deref()
    }

    /// The id of the state the current one was made from; none for a
    /// root.
    pub fn current_parent(&self) -> Option<&str> {
        self.current_parent.as_deref()
    }

    /// Whether any state here is shown under another turn than `turn`:
    /// a line made for one turn and put on a sidecar under another.
    pub(crate) fn shown_elsewhere(&self, turn: u8) -> bool {
        let turn = turn % 4;
        self.current_written.shown_for() != turn
            || self
                .history
                .iter()
                .chain(self.redo.iter())
                .any(|s| s.written.shown_for() != turn)
            || self.snapshots.iter().any(|s| s.written.shown_for() != turn)
    }

    /// Whether any state here takes the frame's shape to be shown: one
    /// placed under another turn, or one wanting version 4's step.
    pub(crate) fn needs_shape(&self) -> bool {
        self.current_written.needs_shape(&self.current)
            || self
                .history
                .iter()
                .chain(self.redo.iter())
                .any(|s| s.written.needs_shape(&s.edit))
            || self
                .snapshots
                .iter()
                .any(|s| s.written.needs_shape(&s.edit))
    }

    /// The current state when it is shown as it stands, and none while
    /// it waits on the frame's shape ([`Self::waits`]): what is handed
    /// to a develop, an export, or a copy taken to another frame, none
    /// of which may take a mask or a patch still where it was placed
    /// under another turn.
    pub fn ready(&self) -> Option<&Edit> {
        (!self.current_waits()).then_some(&self.current)
    }

    /// Whether the current state waits on the frame's shape with its
    /// masks or patches still where they were placed under another
    /// turn: nothing is recorded from it, or given a source, until the
    /// shape is known. (One waiting on version 4's step alone is the
    /// picture still, its crop to be settled, as it always was.)
    pub fn current_waits(&self) -> bool {
        self.current_written.unturned(&self.current)
    }

    /// Every state of this line shown under the frame's turn `turn`,
    /// made again from the state as written ([`show_state`]): the
    /// current one, the history, what was undone, and the snapshots.
    pub(crate) fn show(&mut self, turn: u8, shape: Option<Measured>) {
        show_state(&mut self.current, &mut self.current_written, turn, shape);
        for s in self.history.iter_mut().chain(self.redo.iter_mut()) {
            show_state(&mut s.edit, &mut s.written, turn, shape);
        }
        for s in &mut self.snapshots {
            show_state(&mut s.edit, &mut s.written, turn, shape);
        }
        self.fill_sources();
    }

    /// `a` and `b` with the sources each chose kept by both, the lower
    /// on a tie (by its across, then its down), and every shown copy
    /// given them: what a join of the two starts from (§724).
    pub(crate) fn joined_sources(a: &Self, b: &Self) -> (Self, Self) {
        let mut sources = a.sources.clone();
        for (key, s) in &b.sources {
            sources
                .entry(key.clone())
                .and_modify(|t| {
                    if s[0].total_cmp(&t[0]).then(s[1].total_cmp(&t[1])).is_lt() {
                        *t = *s;
                    }
                })
                .or_insert(*s);
        }
        let with = |line: &Self| {
            let mut line = line.clone();
            line.sources = sources.clone();
            line.fill_sources();
            line
        };
        (with(a), with(b))
    }

    /// `edit`, about to be recorded with `written` as what the file
    /// holds of it, without the sources chosen for its patches: the one
    /// the map keeps for a patch as it will be written, and one the
    /// current state shows as chosen (a turn since puts the patch under
    /// another key), which the map keeps under this key too. A source
    /// placed by hand, or one that moved with its spot (a move or a
    /// resize), is the state's. A copy shown from a state as written (a
    /// snapshot restored) holds none, and is left as it is.
    fn strip_sources(&mut self, edit: &mut Edit, written: &Written) {
        if matches!(&written.kept, Some(k) if k.holds(edit)) {
            return;
        }
        let under = written.parts(edit).0;
        for i in 0..edit.retouch.patches.len() {
            let patch = &edit.retouch.patches[i];
            let Some(s) = patch.source else {
                continue;
            };
            if !patch.method.sourced() {
                continue;
            }
            let key = source_key(patch, under);
            if self.sources.get(&key) == Some(&s) {
                edit.retouch.patches[i].source = None;
            } else if self.shows_chosen(patch) {
                self.sources.entry(key).or_insert(s);
                edit.retouch.patches[i].source = None;
            }
        }
    }

    /// Whether the current state shows `patch`, source and all, with a
    /// source the state as written does not hold: one the map gave it.
    fn shows_chosen(&self, patch: &retouch::Patch) -> bool {
        let shown = &self.current;
        let Some(i) = shown.retouch.patches.iter().position(|p| p == patch) else {
            return false;
        };
        let (_, written) = self.current_written.parts(shown);
        written
            .retouch
            .patches
            .get(i)
            .is_some_and(|w| w.source.is_none())
    }

    /// Two copies of a line, each state with no parent given the one
    /// the other copy holds for its id (§724). A state read at version
    /// 9 gets its parent only where it follows its parent in that file
    /// ([`Self::parents_read_at_nine`]), which one copy may show and the
    /// other not. An id is the hash of its parent's, so two parents
    /// written for one id are the same one. Where neither copy knows,
    /// the state stays with none, and a join takes it as no chain's.
    pub(crate) fn joined_parents(a: &Self, b: &Self) -> (Self, Self) {
        let mut known: std::collections::HashMap<String, String> = Default::default();
        for line in [a, b] {
            let states = line
                .history
                .iter()
                .chain(line.redo.iter())
                .map(|s| (&s.id, &s.parent))
                .chain(std::iter::once((&line.current_id, &line.current_parent)));
            // The first copy's, then the first in its order, where two
            // say different parents for one id (a corrupt file): the
            // newer copy's, as the join orders them, so both ways agree.
            for (id, parent) in states {
                if let (Some(id), Some(parent)) = (id, parent) {
                    known.entry(id.clone()).or_insert_with(|| parent.clone());
                }
            }
        }
        let with = |line: &Self| {
            let mut line = line.clone();
            let fill = |id: &Option<String>, parent: &mut Option<String>| {
                if parent.is_none()
                    && let Some(p) = id.as_ref().and_then(|id| known.get(id))
                {
                    *parent = Some(p.clone());
                }
            };
            for s in line.history.iter_mut().chain(line.redo.iter_mut()) {
                fill(&s.id, &mut s.parent);
            }
            fill(&line.current_id, &mut line.current_parent);
            line
        };
        (with(a), with(b))
    }

    /// Every shown copy here given the sources [`Self::sources`] keeps
    /// for its patches ([`Written::fill_sources`]); true when any took
    /// one.
    fn fill_sources(&mut self) -> bool {
        if self.sources.is_empty() {
            return false;
        }
        let sources = &self.sources;
        let mut took = self
            .current_written
            .fill_sources(&mut self.current, sources);
        for s in self.history.iter_mut().chain(self.redo.iter_mut()) {
            took |= s.written.fill_sources(&mut s.edit, sources);
        }
        for s in &mut self.snapshots {
            took |= s.written.fill_sources(&mut s.edit, sources);
        }
        took
    }

    /// Whether any state here waits on the frame's shape to be shown:
    /// placed under another turn with something placed on the picture,
    /// or still wanting version 4's step ([`Edit::needs_frame`]).
    pub fn waits(&self) -> bool {
        self.current_written.waiting(&self.current)
            || self
                .history
                .iter()
                .chain(self.redo.iter())
                .any(|s| s.written.waiting(&s.edit))
            || self.snapshots.iter().any(|s| s.written.waiting(&s.edit))
    }

    /// The current state as the file has it, `placed_under` and all.
    pub fn current_written(&self) -> Cow<'_, Edit> {
        self.current_written.edit(&self.current)
    }

    /// Every state of this line as the file has it, each in place of
    /// its shown copy: what two copies of a line are joined by, since
    /// two copies under two turns show the same state two ways.
    pub(crate) fn into_written(self) -> Line {
        let mut line = self;
        let to_written = |edit: &mut Edit, written: &mut Written| {
            let w = written.edit(edit).into_owned();
            let before_placed = written.before_placed;
            *written = Written::at(w.placed_under);
            written.before_placed = before_placed;
            *edit = w;
        };
        to_written(&mut line.current, &mut line.current_written);
        for s in line.history.iter_mut().chain(line.redo.iter_mut()) {
            to_written(&mut s.edit, &mut s.written);
        }
        for s in &mut line.snapshots {
            to_written(&mut s.edit, &mut s.written);
        }
        line
    }

    /// Record `edit` as the current state, keeping the one it replaces;
    /// false when it was the current state already. The step has no
    /// words of its own: the panel names it by what moved. `turn` is the
    /// frame's turn `edit` is in, which the state is placed under: the
    /// sidecar's ([`crate::Sidecar::record`], [`crate::LineMut::record`]).
    pub fn record(&mut self, edit: Edit, turn: u8) -> bool {
        self.record_as(edit, None, turn)
    }

    /// Record `edit` as the current state with `label` as the step's
    /// words ("Preset: Faded film"), which the history panel shows in
    /// place of the sections that moved; `None` is [`Self::record`].
    /// A blank label is none. False, and the label not kept, when
    /// `edit` was the current state already, or when the current state
    /// waits on the frame's shape ([`Self::current_waits`]): an edit made
    /// from a copy that shows its masks where they were placed under
    /// another turn would keep them there for good. What was undone
    /// goes, labels and all.
    ///
    /// The state is placed under `turn`, the frame's turn `edit` is in
    /// (§724); an edit handed in says nothing of where it was placed. A
    /// line shown under another turn is shown under this one first, so
    /// what undo hands back is in the frame `edit` is.
    pub fn record_as(&mut self, edit: Edit, label: Option<String>, turn: u8) -> bool {
        let turn = turn % 4;
        if self.shown_elsewhere(turn) {
            self.show(turn, None);
        }
        if self.current_waits() {
            return false;
        }
        let mut edit = edit;
        edit.placed_under = 0;
        // A step is a state the user makes: this build's process, though
        // the edit came from a state made under another. Only the
        // process differing is no step.
        edit.process = self.current.process;
        if edit == self.current {
            return false;
        }
        edit.process = crate::process_now();
        // A copy of a state that waits, or an edit made from one (an
        // undone row clicked over a moved panel, a step or a snapshot
        // copied out, whether or not that state is still here): its
        // masks are where they were placed under another turn, and
        // recorded under this one they would stay there for good. Go to
        // the state ([`Self::go_to`], a snapshot's restore) instead,
        // which moves it as written.
        if edit.unturned {
            log::warn!(
                "a state whose masks wait on the frame's shape is not recorded again: \
                 nothing has measured the frame yet"
            );
            return false;
        }
        self.push_recorded(edit, Written::at(turn), label)
    }

    /// Whether the state at `index` among [`Self::states`] has its masks
    /// or patches waiting where they were placed under another turn, for
    /// want of the frame's shape: nothing is recorded from its copy
    /// ([`Self::record_as`]), though it can be gone to.
    pub fn waits_at(&self, index: usize) -> bool {
        let position = self.position();
        let written = if index < position {
            self.history.get(index).map(|s| (&s.edit, &s.written))
        } else if index == position {
            Some((&self.current, &self.current_written))
        } else {
            let past = index - position - 1;
            self.redo
                .len()
                .checked_sub(past + 1)
                .map(|i| (&self.redo[i].edit, &self.redo[i].written))
        };
        written.is_some_and(|(shown, w)| w.unturned(shown))
    }

    /// `edit`, shown, with what the file holds of it, `written`, as the
    /// new current state under `label`: [`Self::record_as`]'s step, and
    /// a snapshot's restore, which keeps the turn the snapshot was
    /// placed under rather than place it again.
    fn push_recorded(&mut self, mut edit: Edit, written: Written, label: Option<String>) -> bool {
        // Blank words are none, so nothing writes an empty `step`.
        let label = label.filter(|l| !l.trim().is_empty());
        // A chosen source is the line's, not the state's (§724): the
        // state is recorded waiting on it, and shown with it below.
        self.strip_sources(&mut edit, &written);
        // A line nobody developed records its start as the root first.
        self.record_root();
        // The new state's id hangs off the current state's, which a
        // sidecar from before ids gets here if nothing filled it yet.
        self.fill_ids();
        // Its parent is the state it is made from, written (§724).
        let parent = self.current_id.clone();
        let id = sync::state_id(
            parent.as_deref().unwrap_or(sync::ZERO_ID),
            &written.edit(&edit),
            label.as_deref(),
        );
        let previous = Step {
            edit: std::mem::replace(&mut self.current, edit),
            label: std::mem::replace(&mut self.current_label, label),
            exports: std::mem::take(&mut self.current_exports),
            id: self.current_id.replace(id),
            parent: std::mem::replace(&mut self.current_parent, parent),
            order: std::mem::replace(&mut self.current_order, Order::next()),
            written: std::mem::replace(&mut self.current_written, written),
        };
        self.history.push(previous);
        // The earliest stays; what follows it goes.
        self.cap_history();
        self.leave_redo();
        self.head_at = Some(HeadAt::now());
        self.current_written
            .fill_sources(&mut self.current, &self.sources);
        true
    }

    /// The redo stack left, as a new step leaves it: its states' ids
    /// kept as undone (§724), less those the line holds, the last
    /// [`HISTORY`].
    pub(crate) fn leave_redo(&mut self) {
        let left: Vec<String> = self.redo.drain(..).filter_map(|s| s.id).collect();
        self.undone.extend(left);
        let mut undone = std::mem::take(&mut self.undone);
        cap_undone(&mut undone, &self.held_ids());
        self.undone = undone;
    }

    /// The ids of the states the line holds: its history's and its
    /// current one's.
    pub(crate) fn held_ids(&self) -> std::collections::HashSet<&str> {
        self.history
            .iter()
            .filter_map(|s| s.id.as_deref())
            .chain(self.current_id.as_deref())
            .collect()
    }

    /// The undone ids as written: those kept, then the redo stack's,
    /// each once, less those the line holds, the last [`HISTORY`].
    fn undone_written(&self) -> Vec<String> {
        let mut out = self.undone.clone();
        out.extend(self.redo.iter().filter_map(|s| s.id.clone()));
        cap_undone(&mut out, &self.held_ids());
        out
    }

    /// How many states there are to walk: the history, the current
    /// one, and what was undone.
    pub fn states(&self) -> usize {
        self.history.len() + 1 + self.redo.len()
    }

    /// Where the current state sits among [`Self::states`], oldest
    /// first.
    pub fn position(&self) -> usize {
        self.history.len()
    }

    /// The index among [`Self::states`] of the state the history
    /// panel's row `row` goes to ([`Self::rows`]): its own, or for an
    /// export's row the state it was exported from. None for a row
    /// past the end, or a negative one, which is what a list with
    /// nothing chosen hands back.
    pub fn state_at_row(&self, row: i32) -> Option<usize> {
        self.row(row).map(Row::state)
    }

    /// The history panel's row `row` of [`Self::rows`]; None past the
    /// end or for a negative one.
    pub fn row(&self, row: i32) -> Option<Row> {
        usize::try_from(row)
            .ok()
            .and_then(|r| self.rows().get(r).copied())
    }

    /// The history panel's rows, newest first: every state, each
    /// under the exports made from it, newest of those first. An
    /// export's row sits between its state and the next, since that
    /// is where the look it sent out stands in the history, whenever
    /// the file itself finished.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.states());
        for state in (0..self.states()).rev() {
            let n = self.exports(state).len();
            rows.extend((0..n).rev().map(|export| Row::Exported { state, export }));
            rows.push(Row::State(state));
        }
        rows
    }

    /// Where among [`Self::rows`] the state at `index` is.
    pub fn row_of_state(&self, index: usize) -> Option<usize> {
        self.rows().iter().position(|r| *r == Row::State(index))
    }

    /// The exports made from the state at `index` among
    /// [`Self::states`], oldest first.
    pub fn exports(&self, index: usize) -> &[Exported] {
        let position = self.position();
        if index < position {
            self.history.get(index).map_or(&[], |s| &s.exports)
        } else if index == position {
            &self.current_exports
        } else {
            let past = index - position - 1;
            self.redo
                .len()
                .checked_sub(past + 1)
                .map_or(&[], |i| &self.redo[i].exports)
        }
    }

    /// Note that `exported` was written from `edit`: on the state that
    /// is `edit`, the current one when it is, and otherwise the
    /// nearest to it, back through the history and then forward
    /// through what was undone. Nothing is recorded as a state, and
    /// undo and redo are as they were. False, and nothing noted, when
    /// no state here is `edit` any more: the history moved on past it
    /// while the file was written (undone and then replaced), and a
    /// record with no state to go back to would not do what its row
    /// promises.
    ///
    /// A record a state took is also kept as [`Self::last_export`]. One
    /// no state took (the command line's overrides, which are never the
    /// frame's) leaves the sidecar as it was.
    pub fn record_export(&mut self, edit: &Edit, exported: Exported) -> bool {
        let last = exported.clone();
        if self.current == *edit {
            // Made from the start of a line nobody developed: the
            // start is recorded as the root the export is noted on.
            self.record_root();
            self.current_exports.push(exported);
            self.keep_last_export(last);
            return true;
        }
        // The redo stack is newest first: its nearest is its last.
        let found = self
            .history
            .iter_mut()
            .rev()
            .chain(self.redo.iter_mut().rev())
            .find(|s| s.edit == *edit);
        match found {
            Some(step) => {
                step.exports.push(exported);
                self.keep_last_export(last);
                true
            }
            None => false,
        }
    }

    fn keep_last_export(&mut self, exported: Exported) {
        if self
            .last_export
            .as_ref()
            .is_none_or(|l| l.at <= exported.at)
        {
            self.last_export = Some(exported);
        }
    }

    /// The state at `index` among [`Self::states`], oldest first.
    pub fn state(&self, index: usize) -> Option<&Edit> {
        self.state_and_label(index).map(|(edit, _)| edit)
    }

    /// The words the step that made the state at `index` was recorded
    /// with, when it had any; see [`Step::label`].
    pub fn label(&self, index: usize) -> Option<&str> {
        self.state_and_label(index).and_then(|(_, label)| label)
    }

    /// The state at `index` among [`Self::states`] and its words.
    fn state_and_label(&self, index: usize) -> Option<(&Edit, Option<&str>)> {
        let position = self.position();
        if index < position {
            self.history.get(index).map(Step::parts)
        } else if index == position {
            Some((&self.current, self.current_label.as_deref()))
        } else {
            // The redo stack is newest first.
            let past = index - position - 1;
            self.redo
                .len()
                .checked_sub(past + 1)
                .map(|i| self.redo[i].parts())
        }
    }

    /// What the history panel calls the state at `index`: the words
    /// it was recorded with, or what moved from the state before it
    /// ([`history::describe_step`]). None for the earliest state,
    /// which has nothing before it to be named against, and for an
    /// index past the end.
    pub fn describe(&self, index: usize) -> Option<String> {
        let before = self.state(index.checked_sub(1)?)?;
        let (after, label) = self.state_and_label(index)?;
        Some(history::describe_step(
            before,
            after,
            label,
            &self.snapshots,
        ))
    }

    /// Make the state at `index` current by undoing or redoing up to
    /// it, so what lies past it is still there to redo; false when
    /// nothing moved.
    pub fn go_to(&mut self, index: usize) -> bool {
        let from = self.position();
        while self.position() > index && self.undo() {}
        while self.position() < index && self.redo() {}
        self.position() != from
    }

    /// Keep the current state under `name`; its index.
    pub fn take_snapshot(&mut self, name: impl Into<String>, taken: u64) -> usize {
        self.record_root();
        // Made from what it keeps and when, not at random: the same
        // state kept in the same second is the same snapshot, and two
        // copies that read one file with none agree on it.
        let id = fresh_snapshot_id(
            &self.current_written.edit(&self.current),
            taken,
            self.snapshots.iter().map(|s| s.id.as_str()),
        );
        self.snapshots.push(Snapshot {
            id,
            name: name.into(),
            named: 0,
            taken,
            edit: self.current.clone(),
            written: self.current_written.clone(),
        });
        self.snapshots.len() - 1
    }

    /// Name snapshot `index` `name`, trimmed, the time of it kept
    /// ([`Snapshot::named`]), always later than the last; false when
    /// there is no such snapshot, the name is empty, or it is its name.
    pub fn rename_snapshot(&mut self, index: usize, name: &str) -> bool {
        let name = name.trim();
        match self.snapshots.get_mut(index) {
            Some(s) if !name.is_empty() && s.name != name => {
                s.name = name.to_string();
                s.named = crate::versions::now().max(s.taken).max(s.named + 1);
                true
            }
            _ => false,
        }
    }

    /// Take snapshot `index` out, leaving its tombstone
    /// ([`Self::snapshots_removed`]); the snapshot, or none when there
    /// is no such snapshot.
    pub fn remove_snapshot(&mut self, index: usize) -> Option<Snapshot> {
        if index >= self.snapshots.len() {
            return None;
        }
        let gone = self.snapshots.remove(index);
        self.snapshots_removed
            .insert(gone.id.clone(), crate::versions::now());
        cap_tombstones(&mut self.snapshots_removed);
        Some(gone)
    }

    /// Make snapshot `index` the current state as a new step, so the
    /// history stays a line and undo goes back to before; false when
    /// it is the current state already, or there is no such snapshot.
    ///
    /// The step is recorded as "Snapshot: " and the name the snapshot
    /// had then, so the row keeps saying where it came from after the
    /// snapshot is renamed or removed.
    ///
    /// The state comes back as the snapshot holds it, placed under the
    /// turn it was placed under (§724): restoring places nothing again,
    /// so a snapshot that waits on the frame's shape comes back waiting
    /// and is shown right once the shape is known.
    pub fn restore_snapshot(&mut self, index: usize) -> bool {
        let Some(s) = self.snapshots.get(index) else {
            return false;
        };
        if s.edit == self.current {
            return false;
        }
        let (edit, written, label) = (
            s.edit.clone(),
            s.written.clone(),
            history::snapshot_label(&s.name),
        );
        self.push_recorded(edit, written, Some(label))
    }

    /// Back to the previous state; false at the beginning. The state
    /// undone keeps its words on the redo stack.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.history.pop() else {
            return false;
        };
        let undone = Step {
            edit: std::mem::replace(&mut self.current, previous.edit),
            label: std::mem::replace(&mut self.current_label, previous.label),
            exports: std::mem::replace(&mut self.current_exports, previous.exports),
            id: std::mem::replace(&mut self.current_id, previous.id),
            parent: std::mem::replace(&mut self.current_parent, previous.parent),
            order: Order::next(),
            written: std::mem::replace(&mut self.current_written, previous.written),
        };
        self.current_order = previous.order;
        self.redo.push(undone);
        self.head_at = Some(HeadAt::now());
        true
    }

    /// Keep the repair sources the engine chose, `done` being the
    /// patches it chose them for as it saw them, shown under the frame's
    /// turn: each goes into [`Self::sources`] under its patch
    /// ([`source_key`]), and every shown copy with the same patch still
    /// waiting on one takes it, the undone ones and the snapshots too
    /// (§724). No state changes as written: a chosen source is derived,
    /// not placed, so nothing is recorded, no id moves, and nothing
    /// undone is dropped. True when a source was kept that was not.
    ///
    /// A copy that waits on the frame's shape takes none: its patches
    /// are shown where they were placed under another turn, which is
    /// not where the engine chose for.
    pub fn take_sources(&mut self, done: &[retouch::Patch]) -> bool {
        // Chosen on a copy that is not the picture: kept under no key.
        if self.current_waits() {
            return false;
        }
        // Under the key of each state that shows the patch, as written,
        // in the frame it was placed under.
        let mut chosen = Vec::new();
        for patch in done {
            let Some(s) = patch.source else {
                continue;
            };
            if !patch.method.sourced() {
                continue;
            }
            let bare = retouch::Patch {
                source: None,
                ..patch.clone()
            };
            let copies = std::iter::once((&self.current, &self.current_written))
                .chain(
                    self.history
                        .iter()
                        .chain(self.redo.iter())
                        .map(|s| (&s.edit, &s.written)),
                )
                .chain(self.snapshots.iter().map(|s| (&s.edit, &s.written)));
            for (shown, written) in copies {
                chosen.extend(written.chosen_for(shown, &bare, s));
            }
        }
        let mut kept = false;
        for (key, s) in chosen {
            if let std::collections::btree_map::Entry::Vacant(entry) = self.sources.entry(key) {
                entry.insert(s);
                kept = true;
            }
        }
        if kept {
            self.fill_sources();
        }
        kept
    }

    /// Forward again; false when nothing was undone.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = Step {
            edit: std::mem::replace(&mut self.current, next.edit),
            label: std::mem::replace(&mut self.current_label, next.label),
            exports: std::mem::replace(&mut self.current_exports, next.exports),
            id: std::mem::replace(&mut self.current_id, next.id),
            parent: std::mem::replace(&mut self.current_parent, next.parent),
            order: std::mem::replace(&mut self.current_order, Order::next()),
            written: std::mem::replace(&mut self.current_written, next.written),
        };
        self.history.push(current);
        self.head_at = Some(HeadAt::now());
        true
    }

    /// Every edit of this line: the current one, the history, what was
    /// undone, and the snapshots.
    pub fn edits(&self) -> impl Iterator<Item = &Edit> {
        std::iter::once(&self.current)
            .chain(self.history.iter().map(|s| &s.edit))
            .chain(self.redo.iter().map(|s| &s.edit))
            .chain(self.snapshots.iter().map(|s| &s.edit))
    }

    /// Every state of this line that names the look `from` names `to`
    /// instead, in place: nothing is recorded, nothing undone is
    /// dropped, and undo never goes back to a name that is no longer
    /// there.
    ///
    /// A renamed state is a new state to the ids, though, rewritten on
    /// purpose (§724): it, and every state made from it, down its
    /// parents, gets a new id made from its new content and its
    /// parent's new id ([`sync::state_id`]), the redo stack's too. The
    /// rest keep theirs and stay shared, a branch an earlier join
    /// carried over among them. Under old ids, a copy elsewhere that
    /// still has the old name would take the renamed states for its
    /// own; under new ones the renamed run is a branch of its own, and
    /// a join keeps both. True when any state changed.
    pub fn rename_look(&mut self, from: &str, to: &str) -> bool {
        let mut renamed = false;
        for snapshot in &mut self.snapshots {
            renamed |= snapshot.written.rename_look(&mut snapshot.edit, from, to);
        }
        // Ids made for a state from before ids first, so each has its
        // parent to follow.
        self.fill_ids();
        // The states oldest first, each after its parent: the history,
        // the current one, then the redo stack from its end. The old id
        // of each state given a new one, to the new.
        let mut moved: std::collections::HashMap<String, String> = Default::default();
        let recorded = self.recorded;
        let mut step = |edit: &mut Edit,
                        written: &mut Written,
                        label: Option<&str>,
                        id: &mut Option<String>,
                        parent: &mut Option<String>| {
            let changed = written.rename_look(edit, from, to);
            let made_from = parent.as_ref().and_then(|p| moved.get(p)).cloned();
            if !changed && made_from.is_none() {
                return changed;
            }
            if made_from.is_some() {
                *parent = made_from;
            }
            let new = sync::state_id(
                parent.as_deref().unwrap_or(sync::ZERO_ID),
                &written.edit(edit),
                label,
            );
            if let Some(old) = id.replace(new.clone()) {
                moved.insert(old, new);
            }
            changed
        };
        for s in &mut self.history {
            renamed |= step(
                &mut s.edit,
                &mut s.written,
                s.label.as_deref(),
                &mut s.id,
                &mut s.parent,
            );
        }
        if recorded {
            renamed |= step(
                &mut self.current,
                &mut self.current_written,
                self.current_label.as_deref(),
                &mut self.current_id,
                &mut self.current_parent,
            );
        } else {
            // A start nobody developed has no id to make again.
            renamed |= self
                .current_written
                .rename_look(&mut self.current, from, to);
        }
        for s in self.redo.iter_mut().rev() {
            renamed |= step(
                &mut s.edit,
                &mut s.written,
                s.label.as_deref(),
                &mut s.id,
                &mut s.parent,
            );
        }
        renamed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{exposed, scratch};
    use crate::{
        Adjustment, Preset, Section, Sidecar, WhiteBalance, apply_preset_into, baseline, describe,
        geometry, sync_into,
    };

    /// The history list shows the newest step first, and undo leaves
    /// the undone ones below the current one rather than dropping
    /// them, so the mapping is over the whole line and not over what
    /// is still in force.
    #[test]
    fn a_row_of_the_newest_first_list_names_a_state() {
        let mut sidecar = Sidecar::default();
        for exposure in [1.0, 2.0, 3.0] {
            let mut edit = Edit::default();
            edit.light.exposure = exposure;
            assert!(sidecar.record(edit));
        }
        assert_eq!(sidecar.states(), 4);
        // Row 0 is the newest state, the last row the oldest.
        assert_eq!(sidecar.state_at_row(0), Some(3));
        assert_eq!(sidecar.state_at_row(3), Some(0));
        assert_eq!(sidecar.state_at_row(4), None);
        // A list with nothing chosen reports -1.
        assert_eq!(sidecar.state_at_row(-1), None);
        // Undone steps stay in the list, so the rows do not move.
        assert!(sidecar.undo());
        assert_eq!(sidecar.states(), 4);
        assert_eq!(sidecar.state_at_row(0), Some(3));
        assert_eq!(
            sidecar.state(sidecar.state_at_row(1).unwrap()).unwrap(),
            sidecar.state(sidecar.position()).unwrap()
        );
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut sidecar = Sidecar::default();
        for exposure in [1.0, 2.0, 3.0] {
            let mut e = Edit::default();
            e.light.exposure = exposure;
            sidecar.record(e);
        }
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 2.0);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, 1.0);
        assert!(sidecar.redo());
        assert_eq!(sidecar.current.light.exposure, 2.0);
        // A new state after an undo forgets what was undone.
        let mut e = Edit::default();
        e.light.exposure = 5.0;
        sidecar.record(e);
        assert!(!sidecar.redo());
        assert_eq!(sidecar.history.len(), 3);
        for _ in 0..3 {
            assert!(sidecar.undo());
        }
        assert!(!sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
    }

    #[test]
    fn go_to_walks_either_way_and_keeps_what_lies_past() {
        let mut sidecar = Sidecar::default();
        for exposure in [1.0, 2.0, 3.0] {
            let mut e = Edit::default();
            e.light.exposure = exposure;
            sidecar.record(e);
        }
        // Four states: the default, then 1, 2, 3; the current is last.
        assert_eq!(sidecar.states(), 4);
        assert_eq!(sidecar.position(), 3);
        assert!(sidecar.go_to(1));
        assert_eq!(sidecar.current.light.exposure, 1.0);
        assert_eq!(sidecar.position(), 1);
        assert_eq!(sidecar.states(), 4);
        // Every state is still there to read, in order.
        let exposures: Vec<f32> = (0..4)
            .map(|i| sidecar.state(i).unwrap().light.exposure)
            .collect();
        assert_eq!(exposures, [0.0, 1.0, 2.0, 3.0]);
        assert!(sidecar.state(4).is_none());
        assert!(sidecar.go_to(3));
        assert_eq!(sidecar.current.light.exposure, 3.0);
        assert!(!sidecar.go_to(3));
        // Past the end stops at the end.
        assert!(sidecar.go_to(0));
        assert!(sidecar.go_to(9));
        assert_eq!(sidecar.position(), 3);
    }

    #[test]
    fn the_cap_keeps_the_earliest_state() {
        let mut sidecar = Sidecar::default();
        let mut first = Edit::default();
        first.light.exposure = -9.0;
        sidecar.record(first.clone());
        for i in 0..(HISTORY + 20) {
            let mut e = Edit::default();
            e.light.exposure = i as f32;
            sidecar.record(e);
        }
        assert_eq!(sidecar.history.len(), HISTORY);
        assert_eq!(sidecar.history[0], Edit::default());
        assert_eq!(sidecar.history[1].edit.light.exposure, 20.0);
    }

    /// A snapshot renamed on one copy is kept under both names, and
    /// each name has one id whichever copy is newer and in whatever order
    /// three copies join (§724): a tombstone names one snapshot
    /// everywhere.
    #[test]
    fn a_renamed_snapshot_s_two_ids_do_not_hang_on_the_join_s_order() {
        let mut base = Sidecar::default();
        base.record(crate::testing::exposed(1.0));
        base.take_snapshot("Snapshot 1", 1_700_000_000);
        let stamp = |s: &mut Sidecar, at: u64, host: &str| {
            s.revisions.push(crate::sync::Revision {
                hash: format!("{:032x}", at),
                at,
                host: host.into(),
            });
        };
        let (mut a, mut b, mut c) = (base.clone(), base.clone(), base.clone());
        assert!(a.rename_snapshot(0, "Renamed"));
        stamp(&mut a, 200, "a");
        stamp(&mut b, 100, "b");
        stamp(&mut c, 300, "c");
        let ids = |s: &Sidecar| {
            let mut v: Vec<(String, String)> = s
                .snapshots
                .iter()
                .map(|x| (x.name.clone(), x.id.clone()))
                .collect();
            v.sort();
            v
        };
        let ab = crate::sync::join(&a, &b);
        let ac = crate::sync::join(&a, &c);
        assert_eq!(ab.snapshots.len(), 2);
        assert_eq!(ids(&ab), ids(&ac), "a newer than b, c newer than a: alike");
        assert_eq!(ids(&crate::sync::join(&b, &a)), ids(&ab));
        let three = [
            crate::sync::join(&ab, &c),
            crate::sync::join(&c, &ab),
            crate::sync::join(&crate::sync::join(&b, &c), &a),
        ];
        for j in &three {
            assert_eq!(ids(j), ids(&ab));
        }
        // The one not renamed keeps the id it was taken with.
        let kept = ab
            .snapshots
            .iter()
            .find(|s| s.name == "Snapshot 1")
            .unwrap();
        assert_eq!(kept.id, base.snapshots[0].id);
    }

    /// What was undone is written (§724): the redo stack as it stands,
    /// and what a new step left of it; read back as undone, never to
    /// redo; each once by id, the last [`HISTORY`]; a join keeps both
    /// copies' less what it holds.
    #[test]
    fn undone_states_are_written_and_never_redone_across_a_read() {
        let mut s = Sidecar::default();
        for x in [1.0, 2.0, 3.0] {
            s.record(crate::testing::exposed(x));
        }
        assert!(s.undo());
        let undone_id = s.redo[0].id.clone().unwrap();
        // On the redo stack: written as an undone id, read back as one.
        let json = s.to_json();
        let back = Sidecar::from_json(&json, &Edit::default()).unwrap();
        assert!(back.redo.is_empty());
        assert_eq!(back.undone, vec![undone_id.clone()]);
        assert_eq!(back, s, "the redo stack is what the file holds as undone");
        // Only the id: nothing of the state undone is in the file.
        let file: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(file["undone"], serde_json::json!([undone_id]));
        assert!(!json.contains("3.0"), "{json}");
        // A new step leaves it undone.
        s.record(crate::testing::exposed(4.0));
        assert!(s.redo.is_empty());
        assert_eq!(s.undone, vec![undone_id.clone()]);
        // At most the cap, each once.
        let mut many = s.clone();
        for n in 0..(HISTORY + 5) {
            many.record(crate::testing::exposed(10.0 + n as f32));
            assert!(many.undo());
            many.record(crate::testing::exposed(500.0 + n as f32));
        }
        assert_eq!(many.undone.len(), HISTORY);
        // A join: both copies' undone, less what the join holds.
        let mut there = s.clone();
        there.undone.clear();
        assert!(there.redo.is_empty());
        let j = crate::sync::join(&s, &there);
        assert!(j.undone.contains(&undone_id));
        let held = j.held_ids();
        assert!(j.undone.iter().all(|u| !held.contains(u.as_str())));
        // An entry that is no id, from a later build, is passed over.
        let mut later: serde_json::Value = serde_json::from_str(&json).unwrap();
        later["undone"] = serde_json::json!([{"delta": 1}, undone_id, 7]);
        let read = Sidecar::from_json(&later.to_string(), &Edit::default()).unwrap();
        assert_eq!(read.undone, vec![undone_id]);
    }

    /// Record 1, 2, 3; undo; record 3 again: the same state, made from
    /// the same one, has the same id. It is live, so no undone id names
    /// it, at the step, in the file, or read back from one that has it.
    #[test]
    fn an_undone_id_never_names_a_state_the_line_holds() {
        let mut s = Sidecar::default();
        for x in [1.0, 2.0, 3.0] {
            s.record(crate::testing::exposed(x));
        }
        assert!(s.undo());
        let three = s.redo[0].id.clone().unwrap();
        assert!(s.record(crate::testing::exposed(3.0)));
        assert_eq!(s.line.current_id.as_deref(), Some(three.as_str()));
        assert!(s.undone.is_empty(), "{:?}", s.undone);
        let json = s.to_json();
        let file: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(file.get("undone").is_none(), "{json}");
        // A file that names it anyway reads it as none.
        let mut named = file.clone();
        named["undone"] = serde_json::json!([three]);
        let read = Sidecar::from_json(&named.to_string(), &Edit::default()).unwrap();
        assert!(read.undone.is_empty());
    }

    /// A line's head says when and where it last moved (§724): a
    /// record, an undo, a redo, each. Written and read back; a join
    /// keeps the later of two; a line nobody developed writes none.
    #[test]
    fn a_line_s_head_says_when_and_where_it_last_moved() {
        let mut s = Sidecar::default();
        assert!(s.head_at.is_none());
        // This thread's install id, cleared at the end: nothing
        // process-wide is set.
        crate::testing::INSTALL.set(Some("00c0ffee00c0ffee".into()));
        s.record(crate::testing::exposed(1.0));
        let h = s.head_at.clone().expect("a record moves it");
        assert_eq!(h.install, "00c0ffee00c0ffee");
        assert!(h.at > 1_700_000_000);
        // Nothing of the machine's name is written.
        assert!(!s.to_json().contains(&crate::sync::host()) || crate::sync::host().is_empty());
        s.line.head_at = None;
        assert!(s.undo());
        assert!(s.head_at.is_some(), "an undo moves it");
        s.line.head_at = None;
        assert!(s.redo());
        assert!(s.head_at.is_some(), "a redo moves it");
        assert!(s.to_json().contains("\"head_at\""));
        let back = Sidecar::from_json(&s.to_json(), &Edit::default()).unwrap();
        assert_eq!(back.head_at, s.head_at);
        let at = |at: u64, install: &str| {
            Some(HeadAt {
                at,
                install: install.into(),
            })
        };
        let (mut older, mut newer) = (back.clone(), back.clone());
        older.line.head_at = at(5, "there");
        newer.line.head_at = at(9, "here");
        for j in [
            crate::sync::join(&older, &newer),
            crate::sync::join(&newer, &older),
        ] {
            assert_eq!(j.head_at, at(9, "here"));
        }
        let mut nobody = Sidecar::default();
        nobody.line.head_at = at(1, "x");
        crate::testing::INSTALL.set(None);
        assert!(!nobody.to_json().contains("head_at"));
    }

    /// A snapshot's id is its own from when it is taken, kept through a
    /// rename, whose time is kept; a removal leaves a tombstone; and a
    /// file with none is given the same ids by every read (§724).
    #[test]
    fn a_snapshot_keeps_its_id_through_a_rename_and_leaves_a_tombstone() {
        let mut s = Sidecar::default();
        s.record(crate::testing::exposed(1.0));
        // The same state kept twice in one second: two ids.
        s.take_snapshot("A", 1_700_000_000);
        s.take_snapshot("B", 1_700_000_000);
        let (a, b) = (s.snapshots[0].id.clone(), s.snapshots[1].id.clone());
        assert_ne!(a, b);
        assert_eq!(s.snapshots[0].named, 0);
        assert!(s.rename_snapshot(0, " Warm "));
        assert!(!s.rename_snapshot(0, "Warm"));
        assert!(!s.rename_snapshot(0, "  "));
        assert!(!s.rename_snapshot(5, "x"));
        assert_eq!(
            (s.snapshots[0].id.as_str(), s.snapshots[0].name.as_str()),
            (a.as_str(), "Warm")
        );
        assert!(s.snapshots[0].named > 1_700_000_000);
        let gone = s.remove_snapshot(1).unwrap();
        assert_eq!(gone.id, b);
        assert!(s.remove_snapshot(1).is_none());
        assert!(s.snapshots_removed.contains_key(&b));
        // Written and read back: the id, the rename's time, the tombstone.
        let json = s.to_json();
        assert!(json.contains("\"snapshots_removed\""));
        let back = Sidecar::from_json(&json, &Edit::default()).unwrap();
        assert_eq!(back.snapshots, s.snapshots);
        assert_eq!(back.snapshots_removed, s.snapshots_removed);
        // A file whose snapshots have no ids: each read gives the same,
        // the same state twice in one second two of them.
        let old = r#"{"current":{"version":10,"light":{"exposure":1.0}},
            "snapshots":[{"name":"x","taken":5,"edit":{"version":10}},
                         {"name":"y","taken":5,"edit":{"version":10}}]}"#;
        let (one, two) = (
            Sidecar::from_json(old, &Edit::default()).unwrap(),
            Sidecar::from_json(old, &Edit::default()).unwrap(),
        );
        assert_eq!(one.snapshots[0].id, two.snapshots[0].id);
        assert_ne!(one.snapshots[0].id, one.snapshots[1].id);
        assert!(!one.snapshots[1].id.is_empty());
        // A join: both copies' tombstones, at the later time, but for a
        // snapshot the join keeps (§243: the other copy has it), whose
        // tombstone goes; one renamed on one copy kept under both names,
        // by ids of their own.
        let mut here = one.clone();
        let mut there = one.clone();
        here.remove_snapshot(1);
        there.rename_snapshot(0, "z");
        there.snapshots_removed.insert("gone".into(), 9);
        let j = crate::sync::join(&here, &there);
        assert!(j.snapshots.iter().any(|s| s.id == one.snapshots[1].id));
        assert!(!j.snapshots_removed.contains_key(&one.snapshots[1].id));
        assert_eq!(j.snapshots_removed.get("gone"), Some(&9));
        let names: Vec<_> = j.snapshots.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"x") && names.contains(&"z"), "{names:?}");
        let mut ids: Vec<_> = j.snapshots.iter().map(|s| &s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), j.snapshots.len());
    }

    #[test]
    fn snapshots_are_kept_named_restored_as_a_step_and_written() {
        let mut sidecar = Sidecar::default();
        let mut e = Edit::default();
        e.light.exposure = 1.0;
        sidecar.record(e.clone());
        assert_eq!(sidecar.take_snapshot("Bright", 1_700_000_000), 0);
        let mut e2 = Edit::default();
        e2.light.exposure = -1.0;
        sidecar.record(e2);
        // Restoring is a step forward, not a step back.
        assert!(sidecar.restore_snapshot(0));
        assert_eq!(sidecar.current, e);
        assert_eq!(sidecar.history.len(), 3);
        assert!(!sidecar.restore_snapshot(0));
        assert!(!sidecar.restore_snapshot(7));
        assert!(sidecar.undo());
        assert_eq!(sidecar.current.light.exposure, -1.0);
        // The snapshot is written and read back; an old sidecar
        // without any reads as none.
        let dir = std::env::temp_dir().join(format!("greycard-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0002.CR3");
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert_eq!(back.snapshots, sidecar.snapshots);
        assert_eq!(back.snapshots[0].name, "Bright");
        std::fs::write(
            Sidecar::path_for(&raw),
            r#"{"current":{"version":1},"history":[]}"#,
        )
        .unwrap();
        let old = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert!(old.snapshots.is_empty());
        // A snapshot's edit is migrated as the current one is.
        std::fs::write(
            Sidecar::path_for(&raw),
            r#"{"current":{"version":1},"snapshots":[{"name":"x","taken":0,"edit":{"version":1,"noise":{"enabled":true}}}]}"#,
        )
        .unwrap();
        let migrated = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert!(migrated.snapshots[0].edit.noise.profiled);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A labeled step is written with its words and read back with
    /// them, the current state's and a history state's alike; a step
    /// without words writes no `step` key, so a sidecar with no
    /// labels writes as one did before labels existed.
    #[test]
    fn a_labelled_step_round_trips_and_an_unlabelled_one_writes_as_before() {
        let mut sidecar = Sidecar::default();
        sidecar.record(exposed(0.5));
        assert!(sidecar.record_as(exposed(1.0), Some("Preset: Faded film".into())));
        sidecar.record(exposed(1.5));
        assert!(sidecar.record_as(exposed(2.0), Some("Sync from IMG_0001".into())));
        assert_eq!(sidecar.current_label.as_deref(), Some("Sync from IMG_0001"));
        assert_eq!(sidecar.label(2), Some("Preset: Faded film"));
        assert_eq!(sidecar.label(1), None);

        let dir = scratch("labels");
        let raw = dir.join("IMG_0003.CR3");
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert_eq!(back.current_label, sidecar.current_label);
        assert_eq!(back.history, sidecar.history);
        assert_eq!(back.label(2), Some("Preset: Faded film"));
        assert_eq!(back.describe(2).as_deref(), Some("Preset: Faded film"));
        assert_eq!(back.describe(4).as_deref(), Some("Sync from IMG_0001"));
        // The rows without words read as they always have.
        assert_eq!(back.describe(3).as_deref(), Some("Exposure +1.50"));

        // Where the words sit in the file: beside the edit's own
        // fields, and nowhere on a step that has none.
        let tree: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(Sidecar::path_for(&raw)).unwrap())
                .unwrap();
        assert_eq!(tree["step"], "Sync from IMG_0001");
        assert_eq!(tree["history"][2]["step"], "Preset: Faded film");
        assert_eq!(tree["history"][2]["light"]["exposure"], 1.0);
        assert!(tree["history"][1].get("step").is_none());

        let mut plain = Sidecar::default();
        plain.record(exposed(0.5));
        plain.record(exposed(1.0));
        let json = serde_json::to_string_pretty(&plain).unwrap();
        assert!(!json.contains("\"step\""), "{json}");
        // Byte for byte what a history of bare edits wrote, but for
        // each state's id beside it (§233).
        #[derive(Serialize)]
        struct State<'a> {
            #[serde(flatten)]
            edit: serde_json::Value,
            id: &'a Option<String>,
            // Version 10's, beside the id (§724).
            #[serde(skip_serializing_if = "Option::is_none")]
            parent: &'a Option<String>,
        }
        #[derive(Serialize)]
        struct Before<'a> {
            // The file's floors, first (§724).
            floors: crate::Floors,
            current: serde_json::Value,
            id: &'a Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            parent: &'a Option<String>,
            history: Vec<State<'a>>,
            // When and where the head last moved (§724).
            head_at: &'a Option<HeadAt>,
            snapshots: &'a [Snapshot],
            saved: u64,
        }
        let before = Before {
            floors: crate::Floors::OURS,
            current: crate::sparse::written_value(&plain.current),
            id: &plain.current_id,
            parent: &plain.current_parent,
            saved: plain.saved,
            history: plain
                .history
                .iter()
                .map(|s| State {
                    edit: crate::sparse::written_value(&s.edit),
                    id: &s.id,
                    parent: &s.parent,
                })
                .collect(),
            head_at: &plain.head_at,
            snapshots: &plain.snapshots,
        };
        assert_eq!(json, serde_json::to_string_pretty(&before).unwrap());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A build from before labels reads each state of a labeled
    /// sidecar as the plain edit it is: what an edit does not know it
    /// ignores, so the history loads whole and the words are dropped.
    #[test]
    fn a_build_before_labels_reads_a_labelled_history_as_plain_edits() {
        let mut sidecar = Sidecar::default();
        sidecar.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        sidecar.record_as(exposed(2.0), Some("Snapshot: Warm".into()));
        let json = serde_json::to_string(&sidecar).unwrap();
        #[derive(Deserialize)]
        struct Before {
            current: Edit,
            history: Vec<Edit>,
        }
        let old: Before = serde_json::from_str(&json).unwrap();
        assert_eq!(old.current, sidecar.current);
        let edits: Vec<Edit> = sidecar.history.iter().map(|s| s.edit.clone()).collect();
        assert_eq!(old.history, edits);
    }

    /// A sidecar written before labels, of every schema version,
    /// loads with none and its rows read as they did.
    #[test]
    fn an_old_sidecar_loads_with_no_labels_and_its_rows_read_as_before() {
        let dir = scratch("old-labels");
        let raw = dir.join("IMG_0004.CR3");
        std::fs::write(
            Sidecar::path_for(&raw),
            r#"{"current":{"version":4,"light":{"exposure":1.0,"tone":{"contrast":0.2}}},
               "history":[{"version":1},
                          {"version":2,"light":{"exposure":0.5}},
                          {"version":3,"light":{"exposure":1.0}}],
               "snapshots":[{"name":"a","taken":0,"edit":{"version":4}}]}"#,
        )
        .unwrap();
        let old = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert_eq!(old.current_label, None);
        assert!(old.history.iter().all(|s| s.label.is_none()));
        for i in 1..old.states() {
            let (before, after) = (old.state(i - 1).unwrap(), old.state(i).unwrap());
            assert_eq!(
                old.describe(i),
                Some(describe(before, after, &old.snapshots)),
                "row {i}"
            );
        }
        assert_eq!(old.describe(1).as_deref(), Some("Exposure +0.50"));
        assert_eq!(old.describe(0), None);
        // A `step` that is not words costs the label, not the state.
        std::fs::write(
            Sidecar::path_for(&raw),
            r#"{"current":{"version":4},"step":7,
               "history":[{"version":4,"light":{"exposure":1.0},"step":["x"]}]}"#,
        )
        .unwrap();
        let odd = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert_eq!(odd.current_label, None);
        // Written before the baselines per body, it keeps the zero it
        // was made against.
        let mut want = exposed(1.0);
        want.light.baseline = Some(baseline::BEFORE_BODIES);
        // And the first process, which it was made under.
        want.process = 1;
        assert_eq!(odd.history[0], want);
        assert_eq!(odd.history[0].label, None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The words go with the state they made: undo takes them to the
    /// redo stack and redo brings them back; a new step after an
    /// undo drops what was undone, words and all.
    #[test]
    fn undo_carries_a_label_and_a_new_step_drops_the_redos() {
        let mut sidecar = Sidecar::default();
        sidecar.record(exposed(1.0));
        sidecar.record_as(exposed(2.0), Some("Preset: Faded film".into()));
        assert!(sidecar.undo());
        assert_eq!(sidecar.current_label, None);
        assert_eq!(sidecar.redo[0].label.as_deref(), Some("Preset: Faded film"));
        // The undone row still reads by its words.
        assert_eq!(sidecar.describe(2).as_deref(), Some("Preset: Faded film"));
        assert!(sidecar.redo());
        assert_eq!(sidecar.current_label.as_deref(), Some("Preset: Faded film"));
        assert!(sidecar.undo());
        // A new step: the preset's state and its words are gone.
        sidecar.record(exposed(3.0));
        assert!(sidecar.redo.is_empty());
        assert_eq!(sidecar.current_label, None);
        assert!(
            (0..sidecar.states()).all(|i| sidecar.label(i).is_none()),
            "no state keeps the dropped words"
        );
        assert_eq!(sidecar.describe(2).as_deref(), Some("Exposure +3.00"));
        // A step that changes nothing records nothing, words included.
        assert!(!sidecar.record_as(exposed(3.0), Some("Preset: Faded film".into())));
        assert_eq!(sidecar.current_label, None);
        // go_to walks the words with the states.
        sidecar.record_as(exposed(4.0), Some("Sync from IMG_0001".into()));
        assert!(sidecar.go_to(0));
        assert_eq!(sidecar.label(3), Some("Sync from IMG_0001"));
        assert!(sidecar.go_to(3));
        assert_eq!(sidecar.current_label.as_deref(), Some("Sync from IMG_0001"));
    }

    /// A snapshot restored is a step named for the snapshot, and the
    /// row keeps its name after the snapshot is renamed, even when a
    /// single control is all that moved.
    #[test]
    fn a_snapshot_restored_is_named_for_it() {
        let mut sidecar = Sidecar::default();
        sidecar.record(exposed(1.0));
        sidecar.take_snapshot("Bright", 0);
        sidecar.record(exposed(-1.0));
        assert!(sidecar.restore_snapshot(0));
        assert_eq!(sidecar.current_label.as_deref(), Some("Snapshot: Bright"));
        sidecar.snapshots[0].name = "Renamed".into();
        assert_eq!(sidecar.describe(3).as_deref(), Some("Snapshot: Bright"));
    }

    /// A preset over a set and a sync record their words on each
    /// frame that took them, and none on a frame left alone.
    #[test]
    fn a_preset_and_a_sync_over_frames_record_their_words() {
        let mut sidecars = vec![Sidecar::default(), Sidecar::default(), Sidecar::default()];
        sidecars[2].record(exposed(1.0));
        let preset = Preset::from_edit("Bright", &exposed(1.0), &[Section::Light]);
        let moved = apply_preset_into(
            &mut sidecars,
            &preset,
            &[0, 1, 2],
            None,
            Some("Preset ×3: Bright"),
        );
        assert_eq!(moved, [0, 1]);
        for s in &sidecars[..2] {
            assert_eq!(s.current_label.as_deref(), Some("Preset ×3: Bright"));
            assert_eq!(s.describe(1).as_deref(), Some("Preset ×3: Bright"));
        }
        assert_eq!(sidecars[2].current_label, None);

        let moved = sync_into(
            &mut sidecars,
            &exposed(2.0),
            &[1, 2],
            &[Section::Light],
            Some("Sync from IMG_0001"),
        );
        assert_eq!(moved, [1, 2]);
        for s in &sidecars[1..] {
            assert_eq!(s.current_label.as_deref(), Some("Sync from IMG_0001"));
        }
        assert_eq!(sidecars[1].label(1), Some("Preset ×3: Bright"));
    }

    /// `Step` takes a `step` key off each state's object before the
    /// edit reads the rest, so no field of the edit may be called
    /// that: a fully populated edit, every section and list filled,
    /// writes no such key among its own.
    #[test]
    fn no_field_of_an_edit_is_called_step() {
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        edit.white_balance = WhiteBalance::Custom {
            temperature: 5000.0,
            tint: 0.01,
        };
        edit.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        edit.adjustments.push(Adjustment::default());
        let tree: serde_json::Value = serde_json::from_str(&edit.to_json()).unwrap();
        let keys: Vec<&String> = tree.as_object().unwrap().keys().collect();
        assert!(!keys.iter().any(|k| *k == "step"), "{keys:?}");
        // And a step with no words, blank or none, writes none.
        let mut sidecar = Sidecar::default();
        sidecar.record_as(exposed(1.0), Some("  ".into()));
        assert_eq!(sidecar.current_label, None);
        sidecar.record(exposed(2.0));
        let json = serde_json::to_string(&sidecar).unwrap();
        assert!(!json.contains("\"step\""), "{json}");
        // The key sits just after `current` when there is one.
        sidecar.record_as(exposed(3.0), Some("Preset: Faded film".into()));
        let json = serde_json::to_string(&sidecar).unwrap();
        let (current, step, history) = (
            json.find("\"current\"").unwrap(),
            json.find("\"step\"").unwrap(),
            json.find("\"history\"").unwrap(),
        );
        assert!(current < step && step < history, "{json}");
    }

    /// The export records are written beside the states they ride on,
    /// read back whole, read one at a time when some will not read,
    /// and are nothing at all to a build that does not know them.
    #[test]
    fn export_records_round_trip_and_an_older_build_reads_past_them() {
        let exposed = |ev: f32| {
            let mut e = Edit::default();
            e.light.exposure = ev;
            e
        };
        let web = Exported {
            file: "/out/IMG_0001.jpg".into(),
            preset: Some("Web".into()),
            at: 1_790_000_000,
        };
        let mut sidecar = Sidecar::default();
        sidecar.record(exposed(1.0));
        assert!(sidecar.record_export(&exposed(1.0), web.clone()));
        sidecar.record(exposed(2.0));
        assert!(sidecar.record_export(
            &exposed(2.0),
            Exported {
                preset: None,
                ..web.clone()
            }
        ));
        let json = serde_json::to_string(&sidecar).unwrap();
        let back: Sidecar = crate::sidecar::plain(&json).unwrap();
        assert_eq!(back, sidecar);
        assert_eq!(back.history[1].exports, vec![web.clone()]);
        // No preset, no key; and none at all where there were none.
        assert_eq!(json.matches("\"preset\"").count(), 1, "{json}");
        assert_eq!(json.matches("\"exported\"").count(), 2, "{json}");

        // A build from before: the sidecar and each state read as they
        // did, the records unknown fields.
        #[derive(Deserialize)]
        struct Before {
            current: Edit,
            history: Vec<Edit>,
        }
        let before: Before = serde_json::from_str(&json).unwrap();
        assert_eq!(before.current, exposed(2.0));
        assert_eq!(before.history, vec![Edit::default(), exposed(1.0)]);

        // A record that will not read costs itself, not the others,
        // and a key that is not a list costs the records only.
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["history"][1]["exported"] =
            serde_json::json!([{"file": 3}, {"file": "a.jpg", "at": "noon"}]);
        value["exported"] = serde_json::json!("lots");
        let back: Sidecar = crate::sidecar::plain_value(value).unwrap();
        assert_eq!(back.current, exposed(2.0));
        assert!(back.current_exports.is_empty());
        assert_eq!(
            back.history[1].exports,
            vec![Exported {
                file: "a.jpg".into(),
                preset: None,
                at: 0,
            }]
        );
    }

    /// A chosen source changes no state as written (§724): states that
    /// show the same once it is taken stay two, each with its id, its
    /// words, and its exports, and the redo stack is kept.
    #[test]
    fn a_chosen_source_merges_no_states_and_moves_no_id() {
        let (placed, done) = a_placed_spot();
        let record = |file: &str| Exported {
            file: file.into(),
            ..Exported::default()
        };
        let mut sidecar = Sidecar::default();
        assert!(sidecar.record_as(placed.clone(), Some("Clone".into())));
        assert!(sidecar.record_export(&placed, record("first.jpg")));
        assert!(sidecar.record(done.clone()));
        assert!(sidecar.record_export(&done, record("second.jpg")));
        assert!(sidecar.undo());
        let before = sidecar.line.clone();
        let ids: Vec<_> = sidecar.history.iter().map(|s| s.id.clone()).collect();
        assert!(sidecar.take_sources(&done.retouch.patches));
        assert_eq!(sidecar.states(), 3);
        assert_eq!(sidecar.current, done, "shown with the source");
        assert_eq!(
            sidecar.current_written().retouch,
            placed.retouch,
            "written without"
        );
        assert_eq!(sidecar.current_label.as_deref(), Some("Clone"));
        assert_eq!(sidecar.current_exports, [record("first.jpg")]);
        assert_eq!(sidecar.redo[0].exports, [record("second.jpg")]);
        assert_eq!(sidecar.current_id, before.current_id);
        let after: Vec<_> = sidecar.history.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, after);
        // The file holds the states as they were, and the source beside.
        let json = sidecar.to_json();
        let file: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            file["current"]["retouch"]["patches"][0]
                .get("source")
                .is_none_or(serde_json::Value::is_null),
            "{json}"
        );
        assert_eq!(file["sources"].as_object().unwrap().len(), 1, "{json}");
        let back = Sidecar::from_json(&json, &crate::Edit::default()).unwrap();
        assert_eq!(back.current, done, "shown with it once read");
    }

    /// A step recorded over a state shown with a chosen source holds no
    /// source: the source is the line's, and shows with the new state
    /// too. One placed by hand is the state's own.
    #[test]
    fn a_step_over_a_chosen_source_records_none() {
        let (placed, done) = a_placed_spot();
        let mut s = Sidecar::default();
        assert!(s.record(placed.clone()));
        assert!(s.take_sources(&done.retouch.patches));
        let mut brighter = s.current.clone();
        brighter.light.exposure = 0.5;
        assert!(s.record(brighter.clone()));
        assert_eq!(s.current, brighter, "shown with it");
        assert_eq!(
            s.current_written().retouch,
            placed.retouch,
            "written without"
        );
        let mut dragged = brighter;
        dragged.retouch.patches[0].source = Some([0.3, 0.3]);
        assert!(s.record(dragged.clone()));
        assert_eq!(*s.current_written(), dragged, "placed by hand: its own");
    }

    /// Two copies that each chose a source for one patch keep both
    /// sources, the lower on a tie, so a join either way shows every
    /// state the same (§724).
    #[test]
    fn a_join_keeps_both_copies_sources_the_lower_on_a_tie() {
        let (placed, done) = a_placed_spot();
        let mut a = Sidecar::default();
        assert!(a.record(placed));
        let mut b = Sidecar::from_json(&a.to_json(), &crate::Edit::default()).unwrap();
        let mut lower = done.clone();
        lower.retouch.patches[0].source = Some([0.01, 0.5]);
        assert!(a.take_sources(&done.retouch.patches));
        assert!(b.take_sources(&lower.retouch.patches));
        for j in [crate::sync::join(&a, &b), crate::sync::join(&b, &a)] {
            assert_eq!(j.states(), 2);
            assert_eq!(j.current, lower, "the lower");
            assert_eq!(j.line.sources.len(), 1);
        }
    }

    /// A source is kept in the frame its state was placed under and
    /// shown turned with its spot: under another turn it is the same
    /// source, chosen once, and back at the first turn the first.
    #[test]
    fn a_source_chosen_under_one_turn_turns_with_its_spot() {
        let (placed, done) = a_placed_spot();
        let chosen = done.retouch.patches[0].source.unwrap();
        let mut s = Sidecar::default();
        assert!(s.record(placed));
        assert!(s.take_sources(&done.retouch.patches));
        s.frame_known(1.5);
        s.turn_by(1, None).unwrap();
        let turned = mask::Turned::new(1, 1.5).offset(chosen);
        assert_eq!(s.current.retouch.patches[0].source, Some(turned));
        assert_eq!(s.line.sources.len(), 1, "chosen once");
        // The engine choosing the same source again under the turn
        // keeps nothing new.
        let again = s.current.clone();
        assert!(!s.take_sources(&again.retouch.patches));
        s.turn_by(3, None).unwrap();
        assert_eq!(s.current, done);
    }

    /// A step after a turn writes no chosen source into its state: the
    /// one shown is the map's, under the new state's key too, and it
    /// shows once read again.
    #[test]
    fn a_step_after_a_turn_keeps_the_chosen_source_out_of_the_state() {
        let (placed, done) = a_placed_spot();
        let mut s = Sidecar::default();
        assert!(s.record(placed));
        assert!(s.take_sources(&done.retouch.patches));
        s.frame_known(1.5);
        s.turn_by(1, None).unwrap();
        let shown = s.current.retouch.patches[0].source;
        let mut e = s.current().clone();
        e.light.exposure = 0.5;
        assert!(s.record(e));
        assert_eq!(s.current_written().retouch.patches[0].source, None);
        assert_eq!(s.current.retouch.patches[0].source, shown);
        assert_eq!(s.line.sources.len(), 2);
        let back = Sidecar::from_json(&s.to_json(), &Edit::default()).unwrap();
        assert_eq!(back.current().retouch.patches[0].source, shown);
    }

    /// A spot moved or resized after its source was chosen keeps that
    /// source, and the state recorded writes it as placed by hand: what
    /// the person saw stay with the spot (§724).
    #[test]
    fn a_moved_or_resized_spot_writes_the_source_it_shows() {
        let (placed, done) = a_placed_spot();
        let chosen = done.retouch.patches[0].source;
        let mut s = Sidecar::default();
        assert!(s.record(placed));
        assert!(s.take_sources(&done.retouch.patches));
        let mut e = s.current().clone();
        e.retouch.patches[0] = e.retouch.patches[0].moved_to([0.5, 0.5]);
        assert!(s.record(e));
        assert_eq!(s.current_written().retouch.patches[0].source, chosen);
        let mut e = s.current().clone();
        e.retouch.patches[0].resize(0.05);
        assert!(s.record(e));
        assert_eq!(s.current_written().retouch.patches[0].source, chosen);
    }

    /// A version made from a line takes the sources chosen for its
    /// patches: read again, it shows them, and what an export of it
    /// takes ([`Line::ready`]) has them.
    #[test]
    fn a_version_keeps_the_sources_chosen_for_its_patches() {
        let (placed, done) = a_placed_spot();
        let chosen = done.retouch.patches[0].source;
        let mut s = Sidecar::default();
        assert!(s.record(placed));
        assert!(s.take_sources(&done.retouch.patches));
        let id = s.new_version(None, None).unwrap();
        let back = Sidecar::from_json(&s.to_json(), &Edit::default()).unwrap();
        let line = back.line(Some(&id)).unwrap();
        assert_eq!(line.current().retouch.patches[0].source, chosen);
        assert_eq!(
            line.ready().map(|e| e.retouch.patches[0].source),
            Some(chosen)
        );
    }

    /// Two sources for one key, the same but for the sign of a zero, are
    /// settled alike whichever copy is first.
    #[test]
    fn a_tie_between_two_sources_is_settled_alike_both_ways() {
        let (mut a, mut b) = (Line::default(), Line::default());
        a.sources.insert("k".into(), [0.0, 0.1]);
        b.sources.insert("k".into(), [-0.0, 0.1]);
        let (x, _) = Line::joined_sources(&a, &b);
        let (y, _) = Line::joined_sources(&b, &a);
        assert_eq!(x.sources["k"][0].to_bits(), y.sources["k"][0].to_bits());
    }

    /// A chosen source past an f32's range (found by the `sidecar_json`
    /// fuzz target) is no source: kept, it reads as infinite and writes
    /// as null, which the next read drops.
    #[test]
    fn a_source_past_f32_is_not_kept() {
        let json = r#"{"sources":{"a":[0,1e308],"b":[0.25,-0.5]}}"#;
        let s = Sidecar::from_json(json, &Edit::default()).unwrap();
        assert_eq!(s.line.sources.len(), 1);
        assert_eq!(s.line.sources["b"], [0.25, -0.5]);
        let written = s.to_json();
        let back = Sidecar::from_json(&written, &Edit::default()).unwrap();
        assert_eq!(back.to_json(), written, "the round trip is stable");
    }

    /// The key is the patch's placing alone: its id, seed, and fill
    /// switches key nothing anew; its points, radius, and the turn it
    /// was placed under do.
    #[test]
    fn a_source_key_is_the_patch_s_placing_and_turn() {
        let (placed, _) = a_placed_spot();
        let p = &placed.retouch.patches[0];
        let key = source_key(p, 0);
        let other = retouch::Patch {
            id: p.id + 7,
            seed: 3,
            fast: true,
            source: Some([0.1, 0.1]),
            ..p.clone()
        };
        assert_eq!(source_key(&other, 0), key);
        assert_ne!(source_key(p, 1), key);
        assert_ne!(source_key(&p.moved_to([0.1, 0.1]), 0), key);
        let mut bigger = p.clone();
        bigger.resize(0.05);
        assert_ne!(source_key(&bigger, 0), key);
    }

    /// A turn's way back gives back an offset turned by it.
    #[test]
    fn a_turn_and_its_way_back_give_back_an_offset() {
        let d = [0.05, -0.02];
        for q in 0..4 {
            let t = mask::Turned::new(q, 1.5);
            let back = t.back().offset(t.offset(d));
            assert!((back[0] - d[0]).abs() < 1e-6 && (back[1] - d[1]).abs() < 1e-6);
        }
    }

    /// Work done in an act's name stands where the act did: before an
    /// act made after it, however late the work runs.
    #[test]
    fn work_in_an_act_s_name_takes_the_act_s_ticks() {
        let pressed = Order::act();
        let after = Order::next();
        let (first, second) = Order::during(pressed, || (Order::next(), Order::next()));
        assert!(pressed.tick() < first.tick());
        assert!(first.tick() < second.tick());
        assert!(second.tick() < after.tick());
        // Out of the act, the clock goes on past every tick before.
        assert!(Order::next().tick() > after.tick());
        // A request queued inside the act is the act's too.
        assert_eq!(Order::during(pressed, Order::act).tick(), pressed.tick());
    }

    /// A clone spot as placed, waiting on its source, and as the
    /// engine completed it.
    fn a_placed_spot() -> (Edit, Edit) {
        let mut placed = Edit::default();
        placed.retouch.patches.push(retouch::Patch {
            id: placed.retouch.next_id(),
            method: retouch::Method::Clone,
            points: vec![[0.4, 0.6]],
            radius: 0.03,
            ..Default::default()
        });
        let mut done = placed.clone();
        done.retouch.patches[0].source = Some([0.05, -0.02]);
        (placed, done)
    }

    /// Issue 7: when the develop is slow, the panel's save records
    /// the spot before the engine has chosen its source. The source
    /// is kept beside the state and shown with it (§724); it is not a
    /// step, so one undo takes the spot away and a redo brings it back,
    /// source and all.
    #[test]
    fn a_chosen_source_completes_the_state_rather_than_adding_one() {
        let mut sidecar = Sidecar::default();
        let (placed, done) = a_placed_spot();
        // The save timer fires first.
        assert!(sidecar.record(placed));
        // Then the develop lands with the source.
        assert!(sidecar.take_sources(&done.retouch.patches));
        assert_eq!(sidecar.states(), 2);
        assert_eq!(sidecar.current, done);
        // Two states that show the same once it is taken stay two,
        // under their ids.
        let mut stepped = Sidecar::default();
        stepped.record(a_placed_spot().0);
        stepped.record(done.clone());
        let later = stepped.current_id.clone();
        assert!(stepped.take_sources(&done.retouch.patches));
        assert_eq!(stepped.states(), 3);
        assert_eq!(stepped.current_id, later);
        // The save the develop's arrival would have made finds
        // nothing new.
        assert!(!sidecar.record(done.clone()));
        assert!(sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
        // The undone state re-develops; it has no spot, so nothing
        // is chosen, and the redo stays.
        assert!(!sidecar.take_sources(&sidecar.current.retouch.patches.clone()));
        assert!(sidecar.redo());
        assert_eq!(sidecar.current, done);
        // The redone state re-develops too, and has its source.
        assert!(!sidecar.take_sources(&done.retouch.patches));
        assert_eq!(sidecar.states(), 2);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
    }

    /// A develop that lands before the save: the state is recorded
    /// with its source as its own, and keeping the source beside it
    /// changes nothing.
    #[test]
    fn a_source_chosen_before_the_save_is_recorded_with_the_spot() {
        let mut sidecar = Sidecar::default();
        let (_, done) = a_placed_spot();
        assert!(sidecar.record(done.clone()));
        let id = sidecar.current_id.clone();
        sidecar.take_sources(&done.retouch.patches);
        assert_eq!(sidecar.states(), 2);
        assert_eq!(sidecar.current_id, id);
        assert_eq!(*sidecar.current_written(), done);
        assert!(sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
    }

    /// Every state still waiting on the same patch's source takes it,
    /// the undone ones too, so walking the line never re-develops a
    /// state that would want one.
    #[test]
    fn every_state_waiting_on_the_patch_takes_its_source() {
        let mut sidecar = Sidecar::default();
        let (placed, done) = a_placed_spot();
        assert!(sidecar.record(placed.clone()));
        let mut brighter = placed.clone();
        brighter.light.exposure = 0.5;
        assert!(sidecar.record(brighter));
        assert!(sidecar.undo());
        // The develop of the spot alone lands after the undo.
        assert!(sidecar.take_sources(&done.retouch.patches));
        assert_eq!(sidecar.current, done);
        assert_eq!(sidecar.states(), 3);
        assert!(sidecar.redo());
        assert_eq!(sidecar.current.retouch, done.retouch);
        assert_eq!(sidecar.current.light.exposure, 0.5);
    }

    /// A spot that took a removed spot's id, somewhere else, is not
    /// handed the removed one's source.
    #[test]
    fn a_reused_patch_id_elsewhere_takes_no_source() {
        let mut sidecar = Sidecar::default();
        let (placed, done) = a_placed_spot();
        let mut elsewhere = placed.clone();
        elsewhere.retouch.patches[0].points = vec![[0.1, 0.1]];
        assert!(sidecar.record(elsewhere.clone()));
        sidecar.take_sources(&done.retouch.patches);
        assert_eq!(sidecar.current, elsewhere);
    }

    /// A line a build before the fix left, the spot recorded without
    /// its source and then with it: the sourceless state is shown with
    /// the source once it is chosen, and both states stay as written,
    /// under their ids (§724). The undo between them changes nothing
    /// shown, which is the price of never rewriting a state.
    #[test]
    fn a_line_with_the_source_as_its_own_step_keeps_both_states() {
        let mut sidecar = Sidecar::default();
        let (placed, done) = a_placed_spot();
        assert!(sidecar.record_as(placed.clone(), Some("Clone".into())));
        assert!(sidecar.record(done.clone()));
        let ids: Vec<_> = sidecar.history.iter().map(|s| s.id.clone()).collect();
        let current = sidecar.current_id.clone();
        assert!(sidecar.undo());
        assert!(sidecar.take_sources(&done.retouch.patches));
        assert_eq!(sidecar.states(), 3);
        assert_eq!(sidecar.current, done);
        assert_eq!(sidecar.current_label.as_deref(), Some("Clone"));
        assert!(sidecar.redo());
        assert_eq!(sidecar.current, done);
        assert_eq!(sidecar.current_id, current);
        let after: Vec<_> = sidecar.history.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, after);
        assert!(sidecar.undo());
        assert!(sidecar.undo());
        assert_eq!(sidecar.current, Edit::default());
    }

    /// A rename reaches every state that names the look, the undone
    /// and the snapshots too; a state naming another look, one whose
    /// name starts with this one's included, is left as it was. The
    /// states before the first renamed one keep their ids, and from it
    /// on every id is made again from the state's content, as a join
    /// checks a chain.
    #[test]
    fn a_look_renamed_reaches_every_state_and_chains_new_ids() {
        use crate::look::{LookLut, LutChoice};
        let named = |n: &str| LookLut {
            lut: LutChoice::Named(n.into()),
            strength: 0.6,
        };
        let mut sidecar = Sidecar::default();
        let mut e = Edit {
            look_lut: named("Neon"),
            ..Edit::default()
        };
        sidecar.record(e.clone());
        sidecar.take_snapshot("Glow", 1_700_000_000);
        e.look_lut = named("Neon Film");
        sidecar.record(e.clone());
        e.look_lut = named("Neon.agx");
        sidecar.record(e.clone());
        e.look_lut = named("Neon");
        e.light.exposure = 0.5;
        sidecar.record(e.clone());
        e.light.exposure = 1.0;
        sidecar.record(e);
        assert!(sidecar.undo());
        sidecar.fill_ids();
        let ids = sync::state_ids(&sidecar);
        assert!(sidecar.names_look("Neon"));
        assert!(!sidecar.names_look("neon"));

        assert!(sidecar.rename_look("Neon", "Glow"));
        assert!(!sidecar.names_look("Neon"));
        let names: Vec<&str> = sidecar
            .history
            .iter()
            .map(|s| s.edit.look_lut.lut.name())
            .chain(std::iter::once(sidecar.current.look_lut.lut.name()))
            .collect();
        assert_eq!(names, ["none", "Glow", "Neon Film", "Neon.agx", "Glow"]);
        assert_eq!(sidecar.redo[0].edit.look_lut.lut.name(), "Glow");
        assert_eq!(sidecar.snapshots[0].edit.look_lut.lut.name(), "Glow");
        assert_eq!(sidecar.current.look_lut.strength, 0.6);
        let renamed = sync::state_ids(&sidecar);
        assert_eq!(renamed[0], ids[0]);
        assert!(renamed[1..].iter().all(|id| !ids.contains(id)));
        // Every id is what its content and its parent make, the redo
        // stack's chained on from the current state.
        let mut fresh = sidecar.clone();
        for s in &mut fresh.history {
            s.id = None;
        }
        fresh.current_id = None;
        assert_eq!(sync::state_ids(&fresh), renamed);
        let redo = &sidecar.redo[0];
        assert_eq!(
            redo.id.as_deref(),
            Some(sync::state_id(&renamed[4], &redo.edit, redo.label.as_deref()).as_str())
        );
        // Nothing to rename is said so, and changes nothing.
        let before = sidecar.clone();
        assert!(!sidecar.rename_look("Neon", "Other"));
        assert_eq!(sidecar, before);
    }
}
