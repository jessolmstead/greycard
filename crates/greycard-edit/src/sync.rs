//! One frame in two places: what tells two copies of a sidecar apart,
//! and how two that went their own ways are joined (§233).
//!
//! Three things in the file carry it. Every state of the history has
//! an id, the blake3 hash of its parent's id, the edit as compact JSON
//! and the step's label, so the same state reached on two machines is
//! the same state and a copy that stopped earlier is seen to be
//! behind ([`state_id`]). Every save is a [`Revision`], the hash of
//! the one before it and the file's content, with the time and the
//! host, so a copy that only undid, or only changed a rating, is still
//! ordered against the other. And the meta's fields and the turn each
//! carry the time they were set and the revision that set them
//! ([`crate::meta::Times`]), so a rating and a keyword changed on
//! different machines both survive, and a clock that is wrong does
//! not decide between them where the revisions can.
//!
//! [`compare`] says which of §233's cases two copies are, and
//! [`join`] makes one sidecar of two. Both are plain functions over
//! two [`Sidecar`]s; the editor reads the copies, decides, and writes
//! what comes back.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::meta::Facts;
use crate::{Edit, Exported, HISTORY, Sidecar, Snapshot, Step};

/// The parent id of a history's first state: a hash of nothing.
pub const ZERO_ID: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// How many revisions a sidecar keeps, oldest dropped first.
pub const REVISIONS: usize = 200;

/// How a join labels the newer copy's current recorded again on top
/// of both branches: "Reconciled with the copy on <host>", or "the
/// other copy" when the host is not known.
pub const RECONCILED: &str = "Reconciled with ";

/// Whether a state is one a join recorded again as reconciled, by its
/// label.
pub fn is_reconciled(step: &Step) -> bool {
    step.label
        .as_deref()
        .is_some_and(|l| l.starts_with(RECONCILED))
}

/// Whether the entry at `i` of a history is a repeat the cap takes
/// before any state: its edit stands again later under the same label,
/// so that the newest of each content is what stays; or it is a state
/// a join recorded again (its label says so) and its edit stands
/// anywhere else. `entries` is the whole history with the current
/// last, as (edit, label) pairs. The first entry and the last never
/// are. The join's cap and a record's cap ([`Sidecar::cap_history`])
/// ask this one question.
fn repeat_at(entries: &[(&Edit, Option<&str>)], i: usize) -> bool {
    if i == 0 || i + 1 >= entries.len() {
        return false;
    }
    let (edit, label) = entries[i];
    let later_twin = entries[i + 1..]
        .iter()
        .any(|(e, l)| **e == *edit && *l == label);
    let reconciled_twin = label.is_some_and(|l| l.starts_with(RECONCILED))
        && entries
            .iter()
            .enumerate()
            .any(|(j, (e, _))| j != i && **e == *edit);
    later_twin || reconciled_twin
}

/// How many absorbed copies' latest revisions a sidecar keeps
/// ([`Sidecar::absorbed`]), oldest dropped first: enough for a copy
/// joined in turn with every other copy of a shoot to keep each of
/// them behind, at about two kilobytes.
pub const ABSORBED: usize = 64;

/// The id of the state `edit` recorded with `label` makes after the
/// state `parent`: blake3 over the parent's id, the edit as compact
/// JSON and the label, each ended by a newline the first two cannot
/// contain. Computed once, when the state is recorded, and stored
/// (`id` beside the edit's fields); never from a loaded file, since a
/// field added with a default or a serde alias changes the bytes.
pub fn state_id(parent: &str, edit: &Edit, label: Option<&str>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(parent.as_bytes());
    hasher.update(b"\n");
    hasher.update(
        serde_json::to_string(edit)
            .expect("an edit serializes")
            .as_bytes(),
    );
    hasher.update(b"\n");
    hasher.update(label.unwrap_or("").as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// The hash of a revision: blake3 over the previous revision's hash
/// ([`ZERO_ID`] for the first) and the file's content, the first
/// sixteen bytes as hex. The content is the sidecar as compact JSON,
/// in the order it is written, with the revision list and the meta's
/// times taken out, and nothing else: the edit, the labels, the ids,
/// the exports, the meta, the turn, the XMP mark, the save count, the
/// history and the snapshots. The list cannot be in it, and the times
/// are not because they name this very revision: the hash is made
/// first and the fields that changed are stamped with it. The two are
/// taken out and put back rather than copied, and the JSON goes
/// straight into the hasher, so a sidecar of some megabytes is not
/// built twice on the window's thread for every save.
pub(crate) fn revision_hash(previous: Option<&str>, sidecar: &mut Sidecar) -> String {
    let revisions = std::mem::take(&mut sidecar.revisions);
    let meta_at = std::mem::take(&mut sidecar.meta_at);
    let mut hasher = blake3::Hasher::new();
    hasher.update(previous.unwrap_or(ZERO_ID).as_bytes());
    hasher.update(b"\n");
    serde_json::to_writer(&mut hasher, sidecar).expect("a sidecar serializes");
    sidecar.revisions = revisions;
    sidecar.meta_at = meta_at;
    hasher.finalize().to_hex()[..32].to_string()
}

/// When and where a save was made: the time, seconds since the Unix
/// epoch, and the host's name. [`Stamp::now`] is this machine now;
/// a test makes its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub at: u64,
    pub host: String,
}

impl Stamp {
    /// This machine, now.
    pub fn now() -> Self {
        Stamp {
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            host: host(),
        }
    }
}

/// This machine's name, as the system has it; empty when it has none
/// to give.
pub fn host() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_string()
}

/// One save of a sidecar: its hash, when, and on which machine.
/// Written as one line, `"<hash> <seconds> <host>"`, so two hundred
/// of them pretty-printed are a few kilobytes and not a block of
/// objects; the host is left off when it is not known.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Revision {
    /// [`revision_hash`]: 32 hex characters.
    pub hash: String,
    /// Seconds since the Unix epoch; 0 when the save did not know.
    pub at: u64,
    /// The host that made it; empty when it did not say.
    pub host: String,
}

impl Serialize for Revision {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut line = format!("{} {}", self.hash, self.at);
        if !self.host.is_empty() {
            line.push(' ');
            line.push_str(&self.host);
        }
        s.serialize_str(&line)
    }
}

impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let line = String::deserialize(d)?;
        Revision::parse(&line).ok_or_else(|| serde::de::Error::custom("not a revision"))
    }
}

impl Revision {
    fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        let hash = words.next()?;
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        // A time that will not parse is none, not a refused revision:
        // the hash is what orders two copies, the time only which of
        // two diverged branches goes on top.
        let at = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
        let host = words.collect::<Vec<_>>().join(" ");
        Some(Revision {
            hash: hash.to_string(),
            at,
            host,
        })
    }
}

/// The revision list under a key, each line read on its own: one
/// that will not read is left out, the rest kept, and a key that is
/// not a list reads as none.
pub(crate) fn loose_revisions<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<Revision>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|i| serde_json::from_value(i).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// A list of revision hashes under a key, each read on its own: one
/// that is not a hash is left out, and a key that is not a list reads
/// as none.
pub(crate) fn loose_hashes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|i| match i {
                serde_json::Value::String(s)
                    if !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit()) =>
                {
                    Some(s)
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// Which of two copies: the names of [`compare`]'s and [`join`]'s two
/// arguments, in that order. Nothing is decided by which is which;
/// every rule here comes out the same with the two swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The first argument, the copy on this machine.
    Local,
    /// The second argument, the other copy.
    Other,
}

/// What [`compare`] found, §233's five cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compared {
    /// The same sidecar. Nothing to do.
    Same,
    /// The same edit and history; the meta, the snapshots and the
    /// export records may differ. [`join`] puts them together without
    /// adding a state, and the result goes to both copies.
    SameEdit,
    /// This side stopped where the other went on, and has nothing the
    /// other lacks: the other copy goes over it whole.
    Behind(Side),
    /// Each has something the other has not: [`join`], and the result
    /// goes to both copies.
    Diverged,
}

/// Which case two copies of one frame's sidecar are, in §233's order:
/// equal; the same latest revision; one's latest revision among the
/// other's, or among the copies the other absorbed in a join
/// ([`Sidecar::absorbed`]); both with revisions and neither in the
/// other's list; and,
/// for a copy from before revisions, by state ids. There a copy whose
/// states all stand in order among the other's is behind only when it
/// also carries nothing the other lacks: the same meta and turn, its
/// snapshots among the other's, its export records among the other's
/// on the same states. States are the only thing the ids see, and a
/// rating given on the copy that stopped editing is not the kind of
/// thing to lose for being behind on states. The same states are the
/// same edit, and anything else diverged.
pub fn compare(local: &Sidecar, other: &Sidecar) -> Compared {
    if local == other {
        return Compared::Same;
    }
    let (l, o) = (local.revisions.last(), other.revisions.last());
    if let (Some(l), Some(o)) = (l, o) {
        if l.hash == o.hash {
            return Compared::SameEdit;
        }
        if other.knows(&l.hash) {
            return Compared::Behind(Side::Local);
        }
        if local.knows(&o.hash) {
            return Compared::Behind(Side::Other);
        }
        return Compared::Diverged;
    }
    let (li, oi) = (state_ids(local), state_ids(other));
    if li == oi {
        return Compared::SameEdit;
    }
    if in_order(&li, &oi) && lacks_nothing(local, other) {
        return Compared::Behind(Side::Local);
    }
    if in_order(&oi, &li) && lacks_nothing(other, local) {
        return Compared::Behind(Side::Other);
    }
    Compared::Diverged
}

/// Whether every id of `shorter` stands among `longer` in the same
/// order.
fn in_order(shorter: &[String], longer: &[String]) -> bool {
    let mut rest = longer.iter();
    shorter.iter().all(|id| rest.any(|l| l == id))
}

/// Whether `a` carries nothing outside its states that `b` has not:
/// the same meta and turn, every snapshot of `a` in `b`, and every
/// export record of `a` on a state `b` has, with the record.
fn lacks_nothing(a: &Sidecar, b: &Sidecar) -> bool {
    if a.meta != b.meta || a.turn != b.turn {
        return false;
    }
    if !a.snapshots.iter().all(|s| b.snapshots.contains(s)) {
        return false;
    }
    let b_exports: HashMap<String, &[Exported]> =
        state_ids(b).into_iter().zip(exports_of(b)).collect();
    state_ids(a)
        .into_iter()
        .zip(exports_of(a))
        .all(|(id, records)| {
            records.is_empty()
                || b_exports
                    .get(&id)
                    .is_some_and(|theirs| records.iter().all(|r| theirs.contains(r)))
        })
}

/// The export records of every state, oldest first, the current last.
fn exports_of(sidecar: &Sidecar) -> impl Iterator<Item = &[Exported]> {
    sidecar
        .history
        .iter()
        .map(|s| s.exports.as_slice())
        .chain(std::iter::once(sidecar.current_exports.as_slice()))
}

/// The ids of a sidecar's states, oldest first, the current state
/// last: as stored, and computed from the first state forward where
/// a state has none.
pub(crate) fn state_ids(sidecar: &Sidecar) -> Vec<String> {
    let mut ids = Vec::with_capacity(sidecar.history.len() + 1);
    let mut parent = ZERO_ID.to_string();
    for (edit, label, id) in sidecar
        .history
        .iter()
        .map(|s| (&s.edit, s.label.as_deref(), s.id.as_deref()))
        .chain(std::iter::once((
            &sidecar.current,
            sidecar.current_label.as_deref(),
            sidecar.current_id.as_deref(),
        )))
    {
        let id = match id {
            Some(id) => id.to_string(),
            None => state_id(&parent, edit, label),
        };
        parent = id.clone();
        ids.push(id);
    }
    ids
}

/// One sidecar of two copies that differ (§233's "Joining two
/// histories"). The newer copy is the one whose latest revision is
/// later by its time ([`Sidecar::written_at`]); on a tie the greater
/// revision hash, then the greater host name, then the greater id of
/// the current state, so two machines joining the same pair make the
/// same sidecar whichever side each calls local. Its branch goes on
/// top. The joined history is the newer copy's states, then the older
/// copy's own states and its current state, then the newer copy's
/// current state recorded again as "Reconciled with the copy on
/// <host>", so one undo brings back what the other machine had. Which
/// of the older copy's states are its own, and which of the newer
/// copy's are shared, is a union of four rules written out in the
/// body: by id, by a verified chain to a shared state, by a confirmed
/// content match over a re-chained prefix, and the rest. When the
/// older copy has nothing of its own and its current stands where the
/// common past reaches, or its latest revision is in the newer copy's
/// lineage, nothing is recorded again: the join is the newer copy's
/// history. Both redo stacks go. The cap is the join's own, in the
/// order written out in the body: repeated content first, then the
/// shared states after the first, then the older branch, and the newer
/// branch only when it is itself longer than the cap.
///
/// The meta and the turn join field by field, by the revision that
/// set each and then by time ([`crate::meta::join`]); the snapshots by
/// name, a name on both with different edits keeping both, the older
/// copy's renamed "<name> (on <host>)"; the export records by state id.
/// A snapshot taken off on one copy and still on the other comes
/// back, since the file does not say it was taken off: a join keeps
/// rather than loses. The XMP mark is the newer's. The revision lists
/// are put together, each hash once, by time, and both copies' latest
/// revisions are absorbed ([`Sidecar::absorbed`]), so that a copy left
/// stale reads as behind the join however many saves follow, rather
/// than diverged from it again. The result has no new revision:
/// saving it makes one, and from then on both inputs read as behind
/// it.
pub fn join(local: &Sidecar, other: &Sidecar) -> Sidecar {
    let (mut newer, mut older) = match newer_of(local, other) {
        Side::Local => (local.clone(), other.clone()),
        Side::Other => (other.clone(), local.clone()),
    };
    newer.fill_ids();
    older.fill_ids();
    newer.pin_times();
    older.pin_times();
    newer.redo.clear();
    older.redo.clear();
    let host = older
        .revisions
        .last()
        .map(|r| r.host.trim())
        .filter(|h| !h.is_empty())
        .map(str::to_string);

    // The states of each, oldest first, the current last.
    let newer_states = states_of(&newer);
    let older_states = states_of(&older);

    // The export records of every state on either side, by id.
    let mut exports: HashMap<&str, Vec<Exported>> = HashMap::new();
    for state in newer_states.iter().chain(older_states.iter()) {
        if let Some(id) = state.id.as_deref() {
            let records = exports.entry(id).or_default();
            for e in &state.exports {
                if !records.contains(e) {
                    records.push(e.clone());
                }
            }
        }
    }
    let with_exports = |mut s: Step| {
        if let Some(records) = s.id.as_deref().and_then(|id| exports.get(id)) {
            s.exports = records.clone();
        }
        s
    };

    // The joined states, oldest first, each with what it is to the
    // cap below: the newer copy's states first, all of them, which of
    // them are shared settled once the older copy's common past is
    // known.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        First,
        Shared,
        NewerOwn,
        OlderOwn,
        OlderCurrent,
        /// The older copy's current recorded again, where it stood.
        Stood,
        Reconciled,
        /// History the newer copy once had and capped away (rule ii),
        /// kept while there is room and the first to go after repeats
        /// when there is not.
        CappedAway,
    }
    let mut list: Vec<(Kind, Step)> = newer_states
        .iter()
        .map(|s| (Kind::NewerOwn, with_exports(s.clone())))
        .collect();
    let position_by_id = |list: &[(Kind, Step)], s: &Step| {
        s.id.as_deref()
            .and_then(|id| list.iter().position(|(_, t)| t.id.as_deref() == Some(id)))
    };
    let position_by_content = |list: &[(Kind, Step)], s: &Step| {
        list.iter()
            .position(|(_, t)| t.edit == s.edit && t.label == s.label)
    };

    // Every state of the older copy is exactly one of four things, a
    // union and not a line, since a joined history is not one chain:
    //
    // (i) present in the newer copy's list by id: shared, its records
    //     merged by id, wherever it stands in either order;
    // (ii) capped away: a state the newer copy lacks whose successor
    //     in the older copy's order the newer has by id, or is itself
    //     such a state, and whose successor's id confirms it as the
    //     parent. Each state's id is the hash of the one before it,
    //     so a chain the ids confirm is history the newer copy once
    //     had and has dropped: by its cap from the front, or as a
    //     repeat from the middle of a run. Kept, in its own order, as
    //     the lowest kind of state: when the join is under the cap
    //     nothing is lost for no reason, and when it is not these go
    //     right after the repeats, before any shared state, so they
    //     never take a slot from a state that is one copy's own. Once
    //     kept, both copies have them and they are shared states from
    //     then on. A branch a join transplanted does not verify, since
    //     its states hang off their original parents, and is the older
    //     copy's own;
    // (iii) in the re-chained common prefix: a chain this build
    //     re-hashed over a drain a build from before ids made, the
    //     same edit under the same label as a state already here,
    //     reading as new by id. Matched by content, its records merged
    //     onto the match. Only while `fork` is still the first state
    //     (the drain shape) may a match skip newer states; after that
    //     a match counts only at the state right after `fork`, and a
    //     match at or behind `fork`, or none, ends the walk;
    // (iv) everything else: the older copy's own branch, pushed in its
    //     own order and never merged, whatever it looks like. A step
    //     back to an earlier edit is a step of its own, a state that
    //     happens to equal one of the newer copy's own is not that
    //     state, and a first state the newer copy does not have is
    //     the older copy's own too.
    //
    // `fork` is the furthest the common past (i and iii) reaches in
    // the list and drives the walk; which list states are shared is
    // kept apart from it, since after a join the states two copies
    // have by id stand in different orders: the ones the older copy
    // has, by id or by a match, and the verified ancestors of those,
    // which the older copy once had and capped away.
    // Records of an entry that goes, to the first entry with its edit
    // when there is one: an export is a record of a look, and the look
    // is still there.
    fn hand_records(list: &mut [(Kind, Step)], records: Vec<Exported>, edit: &Edit) {
        if let Some((_, twin)) = list.iter_mut().find(|(_, t)| t.edit == *edit) {
            for e in records {
                if !twin.exports.contains(&e) {
                    twin.exports.push(e);
                }
            }
        }
    }
    let verified_child = |parent: &Step, child: &Step| {
        parent.id.as_deref().is_some_and(|p| {
            child.id.as_deref() == Some(state_id(p, &child.edit, child.label.as_deref()).as_str())
        })
    };
    let mut shared_at: HashSet<usize> = HashSet::new();
    // Rule (ii)'s states to bring back, in the older copy's order,
    // each with the list position of the shared state it led to.
    let mut capped: Vec<(usize, &Step)> = Vec::new();
    // The records of states the walk lets go, handed to an entry with
    // their edit once the list is whole: the entry may be one of the
    // older copy's own, pushed after the walk.
    let mut orphans: Vec<(Edit, Vec<Exported>)> = Vec::new();
    let mut own: Vec<&Step> = Vec::new();
    let first = &older_states[0];
    let mut fork = match position_by_id(&list, first).or_else(|| position_by_content(&list, first))
    {
        Some(i) => {
            for e in with_exports(first.clone()).exports {
                if !list[i].1.exports.contains(&e) {
                    list[i].1.exports.push(e);
                }
            }
            shared_at.insert(i);
            Some(i)
        }
        None => {
            own.push(first);
            None
        }
    };
    let mut walking = fork.is_some();
    // (ii), found from the end: a state the newer copy lacks whose
    // successor it has by id, or is itself such a state, and whose
    // successor's id confirms it as the parent. Wherever it stands:
    // the cap takes a repeat from the middle of a run too.
    let mut capped_away: HashSet<usize> = HashSet::new();
    let mut reaches_shared = false;
    for k in (1..older_states.len()).rev() {
        if position_by_id(&list, &older_states[k]).is_some() {
            reaches_shared = true;
        } else if reaches_shared && verified_child(&older_states[k], &older_states[k + 1]) {
            capped_away.insert(k);
        } else {
            reaches_shared = false;
        }
    }
    for (k, s) in older_states.iter().enumerate().skip(1) {
        if let Some(i) = position_by_id(&list, s) {
            shared_at.insert(i);
            fork = fork.max(Some(i));
            continue;
        }
        if capped_away.contains(&k) {
            // One whose edit stands here already is a repeat the cap
            // took, and comes back as a repeat the cap would take
            // again: its records go to that entry and it stays gone.
            // The rest come back before the shared state they led to.
            if list.iter().any(|(_, t)| t.edit == s.edit) {
                orphans.push((s.edit.clone(), with_exports(s.clone()).exports));
            } else {
                let anchor = older_states[k + 1..]
                    .iter()
                    .find_map(|t| position_by_id(&list, t))
                    .expect("a capped-away state leads to a shared one");
                capped.push((anchor, s));
            }
            continue;
        }
        // A state recorded again by an earlier join, whose edit is
        // here already, is a repeat the newer copy's cap took, or will
        // take first: not the older copy's own, and its records go to
        // the entry it repeats. The older copy's current is never a
        // repeat to drop: it stays one undo away.
        if k + 1 < older_states.len()
            && is_reconciled(s)
            && list.iter().any(|(_, t)| t.edit == s.edit)
        {
            orphans.push((s.edit.clone(), with_exports(s.clone()).exports));
            continue;
        }
        if walking {
            let at = position_by_content(&list, s);
            // A match that skips newer states (the drain shape) is
            // confirmed by the state after it matching the state
            // after that: one state the same by chance, two copies
            // recording the same look apart, is not a chain.
            let chained = |i: usize| {
                older_states
                    .get(k + 1)
                    .is_some_and(|next| position_by_content(&list, next) == Some(i + 1))
            };
            let counts = match (fork, at) {
                (Some(f), Some(i)) => i > f && (i == f + 1 || (f == 0 && chained(i))),
                _ => false,
            };
            if counts {
                let i = at.expect("a match");
                for e in with_exports(s.clone()).exports {
                    if !list[i].1.exports.contains(&e) {
                        list[i].1.exports.push(e);
                    }
                }
                shared_at.insert(i);
                fork = Some(i);
                continue;
            }
            walking = false;
        }
        own.push(s);
    }
    // The newer copy's states before a shared one that the ids confirm
    // led to it: the older copy had them and capped them away.
    for i in shared_at.clone() {
        let mut j = i;
        while j > 1 && verified_child(&list[j - 1].1, &list[j].1) {
            j -= 1;
            shared_at.insert(j);
        }
    }
    for (i, (kind, _)) in list.iter_mut().enumerate() {
        *kind = if i == 0 {
            Kind::First
        } else if shared_at.contains(&i) {
            Kind::Shared
        } else {
            Kind::NewerOwn
        };
    }

    // The older copy's current state goes on the end of its branch:
    // as itself when it is the branch's own, and recorded again when
    // it stood in the common past before the fork, so the log says
    // where that machine stood; then the newer copy's current,
    // recorded again as reconciled. A state recorded again is a new
    // state with a new id and no records of its own; the records made
    // from the state it repeats stay on that one. As any record,
    // neither repeats the state before it. An older copy that stood at
    // the fork with nothing of its own says nothing the newer copy's
    // states do not, and nothing is recorded again.
    let recorded_again = |list: &[(Kind, Step)], edit: &Edit, label: Option<String>| {
        let parent = list
            .last()
            .and_then(|(_, s)| s.id.clone())
            .unwrap_or_else(|| ZERO_ID.to_string());
        Step {
            edit: edit.clone(),
            id: Some(state_id(&parent, edit, label.as_deref())),
            label,
            exports: Vec::new(),
        }
    };
    let last_edit = |list: &[(Kind, Step)]| list.last().map(|(_, s)| s.edit.clone());
    let older_current = older_states.last().expect("a current state");
    // When the older copy's latest revision is in the newer copy's
    // lineage, the newer copy has seen the whole of it: every state it
    // lacks it dropped on purpose, by its cap or as a repeat, and a
    // joined history's order cannot always confirm that by the ids.
    // Nothing of the older copy's is its own then; its records, meta
    // and snapshots still join. (The editor copies such a pair over
    // whole; a join met again with its input is this case.)
    let seen = older.revisions.last().is_some_and(|r| newer.knows(&r.hash));
    if seen {
        for s in own.drain(..).chain(capped.drain(..).map(|(_, s)| s)) {
            orphans.push((s.edit.clone(), with_exports(s.clone()).exports));
        }
    }
    let current_is_own = own.last().is_some_and(|s| std::ptr::eq(*s, older_current));
    let stood =
        position_by_id(&list, older_current).or_else(|| position_by_content(&list, older_current));
    // Where it stood is said already when it stands in the list, by id
    // or by content, or an entry has its edit under its label or a
    // reconciled one: an earlier join recorded it there, under an id
    // of that record's own, so a join met again with the same copy
    // records nothing twice. (In one chain the older copy's current
    // is the last of its states and the furthest the common past
    // reaches; in joined histories the two copies' shared states stand
    // in different orders, which says nothing about where it stood.)
    let said = stood.is_some()
        || list.iter().any(|(_, t)| {
            t.edit == older_current.edit && (t.label == older_current.label || is_reconciled(t))
        });
    // Rule (ii)'s states come back only as far as there is room for
    // them once the older branch and the states this join will record
    // again are counted, the ones nearest the shared states first: the
    // cap would take them again at once otherwise, and a current
    // recorded again for nothing would be all that was left of the
    // join, which is what keeps a full join the same when met again by
    // a copy with no revisions. The states recorded again are counted
    // only when they will be pushed: the older copy's current where it
    // stood, when it is not here and not said; the newer copy's
    // current reconciled, when the last entry will not be it already.
    let will_stand = !current_is_own && !said && {
        let last = own
            .last()
            .map(|s| &s.edit)
            .unwrap_or(&list.last().expect("a state").1.edit);
        *last != older_current.edit
    };
    let will_reconcile = {
        let last = if will_stand {
            &older_current.edit
        } else {
            own.last()
                .map(|s| &s.edit)
                .unwrap_or(&list.last().expect("a state").1.edit)
        };
        *last != newer.current
    };
    let extra = usize::from(will_stand) + usize::from(will_reconcile);
    let room = (HISTORY + 1).saturating_sub(list.len() + own.len() + extra);
    if capped.len() > room {
        let drop = capped.len() - room;
        for (_, s) in capped.drain(..drop) {
            orphans.push((s.edit.clone(), with_exports(s.clone()).exports));
        }
    }
    let older_adds_nothing = seen || (own.is_empty() && capped.is_empty() && said);
    let _ = fork;
    // The older copy's current, when it is neither here by id nor
    // pushed as its own, still hands its records to the entry with its
    // edit.
    if !current_is_own && position_by_id(&list, older_current).is_none() {
        orphans.push((
            older_current.edit.clone(),
            with_exports(older_current.clone()).exports,
        ));
    }
    if !older_adds_nothing {
        // The states brought back stand right before the shared state
        // they led to, in the older copy's order: that is where they
        // sit in the chain the ids verify, so an undo reads in time
        // order and both caps take them first. For history capped from
        // the front that is right after the first state. The runs go
        // in from the back, so the positions ahead of each hold.
        let mut anchors: Vec<usize> = capped.iter().map(|(a, _)| *a).collect();
        anchors.sort_unstable();
        anchors.dedup();
        for anchor in anchors.into_iter().rev() {
            let run: Vec<(Kind, Step)> = capped
                .iter()
                .filter(|(a, _)| *a == anchor)
                .map(|(_, s)| (Kind::CappedAway, with_exports((*s).clone())))
                .collect();
            debug_assert!(
                run.iter()
                    .all(|(_, r)| !list.iter().any(|(_, t)| t.edit == r.edit)),
                "a state brought back repeats an edit the list holds"
            );
            list.splice(anchor..anchor, run);
        }
        let branch = if current_is_own {
            &own[..own.len() - 1]
        } else {
            &own[..]
        };
        for s in branch {
            list.push((Kind::OlderOwn, with_exports((*s).clone())));
        }
        if current_is_own {
            list.push((Kind::OlderCurrent, with_exports(older_current.clone())));
        } else if !said && last_edit(&list).as_ref() != Some(&older_current.edit) {
            // Labeled with the same words as the reconciled state, so
            // that it reads as what it is and a later join knows it
            // for a repeat.
            let label = match &host {
                Some(host) => format!("{RECONCILED}the copy on {host}, where it stood"),
                None => format!("{RECONCILED}the other copy, where it stood"),
            };
            let step = recorded_again(&list, &older_current.edit, Some(label));
            list.push((Kind::Stood, step));
        }
        if last_edit(&list).as_ref() != Some(&newer.current) {
            let label = match &host {
                Some(host) => format!("{RECONCILED}the copy on {host}"),
                None => format!("{RECONCILED}the other copy"),
            };
            let step = recorded_again(&list, &newer.current, Some(label));
            list.push((Kind::Reconciled, step));
        }
    }
    for (edit, records) in orphans {
        hand_records(&mut list, records, &edit);
    }
    // The cap, in an order that loses the least. First the entries
    // that are not content of their own: any entry whose edit stands
    // again, under the same label, later in the list, so that the
    // newest of each is what stays. That takes the states a join
    // recorded again, by this join or an earlier one, and the
    // unlabeled doubles a copy brings back after an older build
    // re-saved it without ids, each re-chained under a new id. A
    // repeat drained under the cap loses no content, where merging
    // such doubles at join time could fold a real step back and forth
    // into one state. Then the shared states after the first, since
    // both copies have them; then the older branch's own states,
    // oldest first, its current kept one undo away under the
    // reconciled state; and only when a newer branch is itself longer
    // than the cap, its oldest after all of those. The first state,
    // the current, and the entry that carries the older copy's
    // current never go.
    if list.len() > HISTORY + 1 {
        let mut excess = list.len() - (HISTORY + 1);
        // The entry that is the older copy's current, when it stands
        // among the newer copy's states: by id, or when it was matched
        // by content, the newest entry with its edit.
        let protect = position_by_id(&list, older_current)
            .or_else(|| list.iter().rposition(|(_, t)| t.edit == older_current.edit))
            .and_then(|i| list[i].1.id.clone());
        let kept = |step: &Step| step.id.is_some() && step.id == protect;
        let mut i = 1;
        while excess > 0 && i + 1 < list.len() {
            let entries: Vec<(&Edit, Option<&str>)> = list
                .iter()
                .map(|(_, t)| (&t.edit, t.label.as_deref()))
                .collect();
            if !kept(&list[i].1) && repeat_at(&entries, i) {
                // Its records go to an entry with its edit.
                let (_, gone) = list.remove(i);
                hand_records(&mut list, gone.exports, &gone.edit);
                excess -= 1;
            } else {
                i += 1;
            }
        }
        for kind in [
            Kind::CappedAway,
            Kind::Shared,
            Kind::OlderOwn,
            Kind::NewerOwn,
        ] {
            while excess > 0 {
                match list.iter().position(|(k, s)| *k == kind && !kept(s)) {
                    Some(i) => {
                        let (_, gone) = list.remove(i);
                        hand_records(&mut list, gone.exports, &gone.edit);
                        excess -= 1;
                    }
                    None => break,
                }
            }
        }
    }
    let mut joined = Sidecar {
        xmp: newer.xmp.clone().or(older.xmp.clone()),
        saved: newer.saved.max(older.saved),
        ..Sidecar::default()
    };
    // The first state of the list is the current; the rest follow it
    // as records, ids and all.
    let mut states = list.into_iter().map(|(_, s)| s);
    if let Some(first) = states.next() {
        joined.current = first.edit;
        joined.current_label = first.label;
        joined.current_exports = first.exports;
        joined.current_id = first.id;
    }
    for state in states {
        joined.push_state(state);
    }

    // The meta and the turn, by field.
    fn lineage(s: &Sidecar) -> Vec<&str> {
        s.revisions
            .iter()
            .map(|r| r.hash.as_str())
            .chain(s.absorbed.iter().map(String::as_str))
            .collect()
    }
    let (newer_knows, older_knows) = (lineage(&newer), lineage(&older));
    let (newer_facts, older_facts) = (newer.facts(), older.facts());
    // A copy with no revisions was last written by a build from
    // before §233, and its file's time is all it has to say when its
    // fields were set, the cleared ones among them: every field that
    // differs from the other copy's takes it, for this join only.
    let times = |s: &Sidecar, facts: &Facts, other: &Sidecar, other_facts: &Facts| match s.modified
    {
        Some(at) if s.revisions.is_empty() => {
            s.meta_at.as_of(facts, other_facts, &other.meta_at, at)
        }
        _ => s.meta_at.clone(),
    };
    let (newer_times, older_times) = (
        times(&newer, &newer_facts, &older, &older_facts),
        times(&older, &older_facts, &newer, &newer_facts),
    );
    let (facts, meta_at) = crate::meta::join(
        crate::meta::Side {
            facts: &newer_facts,
            times: &newer_times,
            knows: &newer_knows,
        },
        crate::meta::Side {
            facts: &older_facts,
            times: &older_times,
            knows: &older_knows,
        },
    );
    joined.meta = facts.meta;
    joined.turn = facts.turn;
    joined.meta_at = meta_at;
    joined.baseline = Some(joined.facts());

    // The snapshots, by name.
    joined.snapshots = newer.snapshots.clone();
    for s in &older.snapshots {
        if joined.snapshots.contains(s) {
            continue;
        }
        let snapshot = match joined.snapshots.iter().find(|n| n.name == s.name) {
            Some(n) if n.edit == s.edit => continue,
            Some(_) => Snapshot {
                name: match &host {
                    Some(host) => format!("{} (on {host})", s.name),
                    None => format!("{} (on the other copy)", s.name),
                },
                taken: s.taken,
                edit: s.edit.clone(),
            },
            None => s.clone(),
        };
        if !joined.snapshots.contains(&snapshot) {
            joined.snapshots.push(snapshot);
        }
    }

    // The revisions of both, each once, by time; and both copies'
    // latest absorbed, the lines a copy left stale will be looked for
    // by after the cap has had them.
    let mut revisions = newer.revisions.clone();
    for r in &older.revisions {
        if !revisions.iter().any(|n| n.hash == r.hash) {
            revisions.push(r.clone());
        }
    }
    revisions.sort_by_key(|r| r.at);
    if revisions.len() > REVISIONS {
        revisions.drain(..revisions.len() - REVISIONS);
    }
    joined.revisions = revisions;
    let mut absorbed = newer.absorbed.clone();
    for hash in older
        .absorbed
        .iter()
        .chain(older.revisions.last().map(|r| &r.hash))
        .chain(newer.revisions.last().map(|r| &r.hash))
    {
        if !absorbed.contains(hash) {
            absorbed.push(hash.clone());
        }
    }
    if absorbed.len() > ABSORBED {
        absorbed.drain(..absorbed.len() - ABSORBED);
    }
    joined.absorbed = absorbed;
    joined
}

/// Which copy is the newer: the one written later
/// ([`Sidecar::written_at`]); on a tie the greater latest revision
/// hash, then host, then the greater id of the current state; and the
/// first when nothing tells them apart.
fn newer_of(local: &Sidecar, other: &Sidecar) -> Side {
    let key = |s: &Sidecar| {
        let latest = s.revisions.last();
        (
            s.written_at(),
            latest.map(|r| r.hash.clone()).unwrap_or_default(),
            latest.map(|r| r.host.clone()).unwrap_or_default(),
            state_ids(s).pop().unwrap_or_default(),
        )
    };
    if key(other) > key(local) {
        Side::Other
    } else {
        Side::Local
    }
}

/// A sidecar's states oldest first, the current one last, as steps.
fn states_of(sidecar: &Sidecar) -> Vec<Step> {
    let mut states = sidecar.history.clone();
    states.push(Step {
        edit: sidecar.current.clone(),
        label: sidecar.current_label.clone(),
        exports: sidecar.current_exports.clone(),
        id: sidecar.current_id.clone(),
    });
    states
}

impl Sidecar {
    /// Whether this copy has the revision `hash` in its lineage: in
    /// its list, or absorbed from a copy a join took in.
    fn knows(&self, hash: &str) -> bool {
        self.revisions.iter().any(|r| r.hash == hash) || self.absorbed.iter().any(|a| a == hash)
    }

    /// When this copy was last written, seconds since the Unix epoch:
    /// its latest revision's time, or for a copy from before
    /// revisions the time its file had when it was read
    /// ([`Self::modified`]); 0 when neither is known.
    pub fn written_at(&self) -> u64 {
        self.revisions
            .last()
            .map(|r| r.at)
            .filter(|at| *at > 0)
            .or(self.modified)
            .unwrap_or(0)
    }

    /// The meta and the turn together: what the times are kept for.
    pub(crate) fn facts(&self) -> Facts {
        Facts {
            meta: self.meta.clone(),
            turn: self.turn,
        }
    }

    /// Give every field of the meta and the turn that says something
    /// and was never stamped a time: the file's first revision's, or
    /// for a copy from before revisions the time its file had when it
    /// was read. For a sidecar from before the times, once, on its
    /// first read; a copy never read from a file and never saved by
    /// this build gets 0, and loses to anything. True when anything
    /// was pinned.
    pub fn pin_times(&mut self) -> bool {
        let at = self
            .revisions
            .first()
            .map(|r| r.at)
            .filter(|at| *at > 0)
            .or(self.modified)
            .unwrap_or(0);
        let facts = self.facts();
        self.meta_at.pin(&facts, at)
    }

    /// Give every state here that has none its id, from the first
    /// state forward: the one time an id is computed from a loaded
    /// file, for a sidecar written before ids existed. True when any
    /// was filled.
    pub fn fill_ids(&mut self) -> bool {
        let mut filled = false;
        let mut parent = ZERO_ID.to_string();
        for step in &mut self.history {
            if step.id.is_none() {
                step.id = Some(state_id(&parent, &step.edit, step.label.as_deref()));
                filled = true;
            }
            parent = step.id.clone().expect("just filled");
        }
        if self.current_id.is_none() {
            self.current_id = Some(state_id(
                &parent,
                &self.current,
                self.current_label.as_deref(),
            ));
            filled = true;
        }
        // The redo stack is newest first; its last is the current
        // state's child.
        parent = self.current_id.clone().expect("just filled");
        for step in self.redo.iter_mut().rev() {
            if step.id.is_none() {
                step.id = Some(state_id(&parent, &step.edit, step.label.as_deref()));
                filled = true;
            }
            parent = step.id.clone().expect("just filled");
        }
        filled
    }

    /// Put `state` after the current one as a recorded state, its id
    /// and records as they are: a join carrying a branch over, where
    /// [`Self::record_as`] would make a new id. The cap is the join's
    /// to apply, in its own order, before it gets here.
    fn push_state(&mut self, state: Step) {
        let previous = Step {
            edit: std::mem::replace(&mut self.current, state.edit),
            label: std::mem::replace(&mut self.current_label, state.label),
            exports: std::mem::replace(&mut self.current_exports, state.exports),
            id: std::mem::replace(&mut self.current_id, state.id),
        };
        self.history.push(previous);
        self.redo.clear();
    }

    /// The history's cap: the first state stays, the oldest after it
    /// go.
    pub(crate) fn cap_history(&mut self) {
        if self.history.len() <= HISTORY {
            return;
        }
        // Repeats first, as the join's cap has it ([`repeat_at`]): the
        // doubles a copy brings back after an older build re-saved it
        // would otherwise push its oldest states of their own out.
        let mut i = 1;
        while self.history.len() > HISTORY && i < self.history.len() {
            let entries: Vec<(&Edit, Option<&str>)> = self
                .history
                .iter()
                .map(|h| (&h.edit, h.label.as_deref()))
                .chain(std::iter::once((
                    &self.current,
                    self.current_label.as_deref(),
                )))
                .collect();
            if repeat_at(&entries, i) {
                let gone = self.history.remove(i);
                self.hand_records(gone.exports, &gone.edit);
            } else {
                i += 1;
            }
        }
        // Then the oldest after the first.
        if self.history.len() > HISTORY {
            let extra = self.history.len() - HISTORY;
            for gone in self.history.drain(1..1 + extra).collect::<Vec<_>>() {
                self.hand_records(gone.exports, &gone.edit);
            }
        }
    }

    /// The records of a state that went, to the first state here with
    /// its edit when there is one.
    fn hand_records(&mut self, records: Vec<Exported>, edit: &Edit) {
        let twin = self
            .history
            .iter_mut()
            .map(|h| (&h.edit, &mut h.exports))
            .chain(std::iter::once((&self.current, &mut self.current_exports)))
            .find(|(e, _)| **e == *edit);
        if let Some((_, exports)) = twin {
            for e in records {
                if !exports.contains(&e) {
                    exports.push(e);
                }
            }
        }
    }

    /// Make this save's revision: hash the file as it is about to be
    /// written against the last revision, keep the last
    /// [`REVISIONS`], and stamp every field of the meta and the turn
    /// that differs from the file as read with the time and the new
    /// revision's hash.
    pub(crate) fn revise(&mut self, stamp: &Stamp) {
        let previous = self.revisions.last().map(|r| r.hash.clone());
        let hash = revision_hash(previous.as_deref(), self);
        let before = self.baseline.take().unwrap_or_default();
        let after = self.facts();
        self.meta_at.stamp(&before, &after, stamp.at, &hash);
        self.baseline = Some(after);
        self.revisions.push(Revision {
            hash,
            at: stamp.at,
            host: stamp.host.clone(),
        });
        if self.revisions.len() > REVISIONS {
            let extra = self.revisions.len() - REVISIONS;
            self.revisions.drain(..extra);
        }
    }
}

/// Several places and an archive, worked at random and synced the
/// way part 2 will sync them, with what must hold checked after every
/// sync: see [`simulate`].
#[cfg(test)]
mod simulation {
    use super::*;
    use crate::meta::Flag;
    use std::collections::HashSet;

    /// A small deterministic generator, so a failing seed can be run
    /// again.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            // SplitMix64.
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn chance(&mut self, out_of: u64) -> bool {
            self.below(out_of) == 0
        }
    }

    /// What a run is made of.
    #[derive(Clone, Copy, Debug, Default)]
    struct Mode {
        /// Enough edits a round to press on the cap.
        heavy: bool,
        /// Some places are on a build from before §233 now and then:
        /// ids, revisions, field times and the absorbed list gone, a
        /// few states recorded with no ids and that build's cap, then
        /// read back by this build as a file with a time.
        old_builds: bool,
        /// Each place's clock off by up to a few minutes.
        skew: bool,
    }

    fn exposed(stops: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = stops;
        e
    }

    /// What a save does to the struct, without the file.
    fn commit(s: &mut Sidecar, at: u64, host: &str) {
        s.saved = s.saved.saturating_add(1);
        s.fill_ids();
        s.revise(&Stamp {
            at,
            host: host.into(),
        });
    }

    fn ids(s: &Sidecar) -> Vec<String> {
        state_ids(s)
    }

    /// Every export record with the id of the state it is on.
    fn records(s: &Sidecar) -> Vec<(String, Option<String>)> {
        s.history
            .iter()
            .flat_map(|h| h.exports.iter().map(|e| (e.file.clone(), h.id.clone())))
            .chain(
                s.current_exports
                    .iter()
                    .map(|e| (e.file.clone(), s.current_id.clone())),
            )
            .collect()
    }

    fn edits(s: &Sidecar) -> Vec<&Edit> {
        s.history
            .iter()
            .map(|h| &h.edit)
            .chain(std::iter::once(&s.current))
            .collect()
    }

    fn summary(s: &Sidecar) -> Vec<(f32, Option<String>)> {
        s.history
            .iter()
            .map(|h| (h.edit.light.exposure, h.label.clone()))
            .chain(std::iter::once((
                s.current.light.exposure,
                s.current_label.clone(),
            )))
            .collect()
    }

    /// The states of `a` that `b` once had too: the ones `b` has by
    /// id, and the ones before each of those that the ids confirm led
    /// to it (which `b` capped away); with `content`, also the ones
    /// whose edit `b` has under some state, which the join may have
    /// matched by content over a re-chained prefix, and their verified
    /// ancestors. The join's own shared set lies between the two: a
    /// confirmed match counts there, a chance one does not.
    fn once_shared(a: &Sidecar, b: &Sidecar, content: bool) -> HashSet<String> {
        let (ai, bi) = (ids(a), ids(b));
        let b_has: HashSet<&String> = bi.iter().collect();
        let b_edits = edits(b);
        let a_states: Vec<Step> = a
            .history
            .iter()
            .cloned()
            .chain(std::iter::once(Step {
                edit: a.current.clone(),
                label: a.current_label.clone(),
                exports: Vec::new(),
                id: a.current_id.clone(),
            }))
            .collect();
        let mut shared = HashSet::new();
        for (i, id) in ai.iter().enumerate() {
            let anchor = b_has.contains(id) || (content && b_edits.contains(&&a_states[i].edit));
            if !anchor {
                continue;
            }
            shared.insert(id.clone());
            let mut j = i;
            while j > 1 {
                let child = &a_states[j];
                let expect = state_id(&ai[j - 1], &child.edit, child.label.as_deref());
                if child.id.as_deref() != Some(expect.as_str()) {
                    break;
                }
                j -= 1;
                shared.insert(ai[j].clone());
            }
        }
        shared
    }

    /// How many entries of `s` the cap would take before any state
    /// ([`repeat_at`]): not the first, not the current, and not the
    /// entry that carries the older copy's current, by id or, when it
    /// is not here by id, the newest with its edit.
    fn drainable_repeats(s: &Sidecar, older: &Sidecar) -> usize {
        let entries: Vec<(&Edit, Option<&str>)> = s
            .history
            .iter()
            .map(|h| (&h.edit, h.label.as_deref()))
            .chain(std::iter::once((&s.current, s.current_label.as_deref())))
            .collect();
        let by_id = s
            .history
            .iter()
            .position(|h| h.id.is_some() && h.id == older.current_id);
        let protected = by_id.or_else(|| s.history.iter().rposition(|h| h.edit == older.current));
        (1..s.history.len())
            .filter(|i| Some(*i) != protected && repeat_at(&entries, *i))
            .count()
    }

    /// What must hold of a join of `a` and `b` that came out as `j`.
    fn check_join(seed: u64, a: &Sidecar, b: &Sidecar, j: &Sidecar) {
        let why = format!("seed {seed}");
        // (c) The same from either side: `j` is `join(a, b)`.
        assert_eq!(*j, join(b, a), "{why}: order");
        // (d) Joined again with either input, once saved, the same,
        // through the lineage: the input's latest revision is in the
        // join's.
        let mut saved = j.clone();
        commit(&mut saved, 1 << 40, "check");
        for input in [a, b] {
            // An input with no revisions (a copy an older build saved
            // last) has no lineage to be known by, so only the union
            // rules below apply to it.
            if input.revisions.is_empty() {
                continue;
            }
            let again = join(&saved, input);
            assert_eq!(
                summary(&again),
                summary(&saved),
                "{why}: rejoin with input {:?}",
                summary(input)
            );
            // The states, with their ids; the records as a set, since
            // a record on a re-chained double may sit on its own entry
            // in the join and on the entry that stays in the rejoin.
            let brief = |h: &Step| {
                (
                    h.edit.light.exposure,
                    h.label.clone(),
                    h.id.as_deref().map(|id| id[..8].to_string()),
                )
            };
            let differ: Vec<String> = again
                .history
                .iter()
                .zip(saved.history.iter())
                .enumerate()
                .filter(|(_, (x, y))| brief(x) != brief(y))
                .map(|(i, (x, y))| format!("{i}: {:?} vs {:?}", brief(x), brief(y)))
                .collect();
            assert!(differ.is_empty(), "{why}: rejoin differs at {differ:#?}");
            // The set of files: a record handed to a twin may stand on
            // more entries after a join than before, which is not a
            // record more or less.
            let files = |s: &Sidecar| {
                records(s)
                    .into_iter()
                    .map(|(f, _)| f)
                    .collect::<std::collections::BTreeSet<String>>()
            };
            let where_ = |s: &Sidecar| {
                s.history
                    .iter()
                    .flat_map(|h| {
                        h.exports.iter().map(move |e| {
                            (
                                e.file.clone(),
                                h.edit.light.exposure,
                                h.label.clone(),
                                h.id.as_deref().map(|i| i[..8].to_string()),
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                files(&again),
                files(&saved),
                "{why}: rejoin records; again {:?}; saved {:?}; input records {:?}; input {:?}",
                where_(&again),
                where_(&saved),
                where_(input),
                summary(input)
            );
            // And through the union rules, with the lineage stripped
            // from the input: not the same again, since a joined
            // history's order cannot always confirm what was dropped
            // on purpose (which is what the lineage is for), but the
            // same from either side, nothing of the join gone by
            // content below the cap, and every record and snapshot
            // kept.
            let mut bare = input.clone();
            bare.revisions.clear();
            bare.absorbed.clear();
            let union = join(&saved, &bare);
            assert_eq!(union, join(&bare, &saved), "{why}: union rejoin order");
            let kept = edits(&union);
            let gone: Vec<f32> = edits(&saved)
                .into_iter()
                .filter(|e| !kept.contains(e))
                .map(|e| e.light.exposure)
                .collect();
            assert!(
                gone.is_empty() || union.history.len() == HISTORY,
                "{why}: union rejoin lost {gone:?} below the cap"
            );
            let files: Vec<String> = records(&union).into_iter().map(|(f, _)| f).collect();
            for (edit, exports) in saved
                .history
                .iter()
                .map(|h| (&h.edit, &h.exports))
                .chain(std::iter::once((&saved.current, &saved.current_exports)))
            {
                for e in exports {
                    assert!(
                        files.contains(&e.file) || !kept.contains(&edit),
                        "{why}: union rejoin lost record {}",
                        e.file
                    );
                }
            }
            for s in &saved.snapshots {
                assert!(
                    union.snapshots.iter().any(|n| n.edit == s.edit),
                    "{why}: union rejoin lost snapshot {}",
                    s.name
                );
            }
        }
        let jedits = edits(j);
        // Snapshots: every one of either input, by name and by edit.
        for s in a.snapshots.iter().chain(b.snapshots.iter()) {
            assert!(
                j.snapshots.iter().any(|n| n.name == s.name),
                "{why}: snapshot {} lost by name",
                s.name
            );
            assert!(
                j.snapshots.iter().any(|n| n.edit == s.edit),
                "{why}: snapshot {} lost by edit",
                s.name
            );
        }
        // The meta is one input's or the other's, field by field.
        assert!(
            [a.meta.rating, b.meta.rating].contains(&j.meta.rating),
            "{why}: rating from nowhere"
        );
        assert!(
            [a.turn, b.turn].contains(&j.turn),
            "{why}: turn from nowhere"
        );
        // Independent of any rule about ids: an edit of either input is
        // gone from the join by content only when the join is at the
        // cap.
        let gone: Vec<f32> = edits(a)
            .into_iter()
            .chain(edits(b))
            .filter(|e| !jedits.contains(e))
            .map(|e| e.light.exposure)
            .collect();
        if !gone.is_empty() {
            assert_eq!(
                j.history.len(),
                HISTORY,
                "{why}: edits {gone:?} gone from the join below the cap"
            );
        }
        // (a) A record is kept while any entry with its edit stands.
        let jfiles: Vec<String> = records(j).into_iter().map(|(f, _)| f).collect();
        for s in [a, b] {
            for (edit, exports) in s
                .history
                .iter()
                .map(|h| (&h.edit, &h.exports))
                .chain(std::iter::once((&s.current, &s.current_exports)))
            {
                for e in exports {
                    assert!(
                        jfiles.contains(&e.file) || !jedits.contains(&edit),
                        "{why}: record {} lost while an entry with its edit stands",
                        e.file
                    );
                }
            }
        }
        // (b) No state lost except by the cap, and then in the cap's
        // order: no state of a branch goes while a repeat, a shared
        // state (but the first) or, for the newer branch, an older
        // branch state remains, the older copy's current always kept.
        let jids: HashSet<String> = ids(j).into_iter().collect();
        let (newer, older) = match newer_of(a, b) {
            Side::Local => (a, b),
            Side::Other => (b, a),
        };
        // A copy whose latest revision the other carries in its
        // lineage has been seen whole by it; what the other lacks of
        // it was dropped on purpose, and is not this join's loss.
        if older.revisions.last().is_some_and(|r| newer.knows(&r.hash)) {
            return;
        }
        // Strict: by id and the chains to them; wide: content anchors
        // too. A state is "own and lost" only outside the wide set, and
        // "shared and left" only inside the strict one.
        let strict: HashSet<String> = once_shared(newer, older, false)
            .union(&once_shared(older, newer, false))
            .cloned()
            .collect();
        let shared: HashSet<String> = once_shared(newer, older, true)
            .union(&once_shared(older, newer, true))
            .cloned()
            .collect();
        let missing = |s: &Sidecar| {
            ids(s)
                .into_iter()
                .filter(|id| !jids.contains(id))
                .collect::<Vec<_>>()
        };
        let (mn, mo) = (missing(newer), missing(older));
        // A state only the older copy has that the newer once had and
        // capped away (rule ii) is not a loss of this join.
        let newer_has: HashSet<String> = ids(newer).into_iter().collect();
        let mo: Vec<String> = mo
            .into_iter()
            .filter(|id| newer_has.contains(id) || !shared.contains(id))
            .collect();
        // A missing entry whose edit stands elsewhere in its input is
        // a repeat, the cap's first choice; others may rightly remain.
        let is_repeat = |s: &Sidecar, id: &String| {
            let all = edits(s);
            s.history
                .iter()
                .map(|h| (&h.edit, h.id.as_ref()))
                .chain(std::iter::once((&s.current, s.current_id.as_ref())))
                .find(|(_, i)| *i == Some(id))
                .is_some_and(|(e, _)| all.iter().filter(|x| **x == e).count() > 1)
        };
        // And a missing state whose edit is still in the join under
        // another entry (a repeat on either side drained, the newest
        // of the content kept) is not a loss.
        let content_kept = |s: &Sidecar, id: &String| {
            s.history
                .iter()
                .map(|h| (&h.edit, h.id.as_ref()))
                .chain(std::iter::once((&s.current, s.current_id.as_ref())))
                .find(|(_, i)| *i == Some(id))
                .is_some_and(|(e, _)| jedits.contains(&e))
        };
        let mn: Vec<String> = mn
            .into_iter()
            .filter(|id| !is_repeat(newer, id) && !content_kept(newer, id))
            .collect();
        let mo: Vec<String> = mo
            .into_iter()
            .filter(|id| !is_repeat(older, id) && !content_kept(older, id))
            .collect();
        if !mn.is_empty() || !mo.is_empty() {
            assert_eq!(
                j.history.len(),
                HISTORY,
                "{why}: lost without the cap; newer {:?}, older {:?}, join {:?}",
                summary(newer),
                summary(older),
                summary(j)
            );
            let lost_edits = |s: &Sidecar, lost: &[String]| {
                s.history
                    .iter()
                    .filter(|h| h.id.as_ref().is_some_and(|id| lost.contains(id)))
                    .map(|h| (h.edit.light.exposure, h.label.clone()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                drainable_repeats(j, older),
                0,
                "{why}: a repeat remained; lost newer {:?} older {:?}; newer {:?}; older {:?}; join {:?}",
                lost_edits(newer, &mn),
                lost_edits(older, &mo),
                summary(newer),
                summary(older),
                summary(j)
            );
            let first = ids(newer)[0].clone();
            let real_in_j = |id: &String| {
                j.history
                    .iter()
                    .any(|h| h.id.as_ref() == Some(id) && !is_reconciled(h))
            };
            let not_current = |id: &String| Some(id) != older.current_id.as_ref();
            // A state whose edit the other copy also has, under some
            // state of its own, is shared or not by rules this check
            // does not repeat (a confirmed match over a re-chained
            // prefix is, a chance match is not); it is set aside here
            // rather than guessed.
            let edit_of = |s: &Sidecar, id: &String| {
                s.history
                    .iter()
                    .find(|h| h.id.as_ref() == Some(id))
                    .map(|h| h.edit.clone())
                    .or_else(|| (s.current_id.as_ref() == Some(id)).then(|| s.current.clone()))
            };
            let ambiguous = |id: &String| {
                edit_of(newer, id).is_some_and(|e| edits(older).contains(&&e))
                    || edit_of(older, id).is_some_and(|e| edits(newer).contains(&&e))
            };
            let shared_left = jids
                .iter()
                .filter(|id| strict.contains(*id) && **id != first && real_in_j(id))
                .filter(|id| not_current(id) && !ambiguous(id))
                .count();
            let o_own_left = ids(older)
                .iter()
                .filter(|id| !shared.contains(*id) && real_in_j(id))
                .filter(|id| not_current(id) && !ambiguous(id))
                .count();
            let n_own_lost = mn.iter().any(|id| !shared.contains(id) && !ambiguous(id));
            let o_own_lost = mo
                .iter()
                .any(|id| !shared.contains(id) && not_current(id) && !ambiguous(id));
            if n_own_lost {
                let left: Vec<_> = ids(older)
                    .iter()
                    .filter(|id| !shared.contains(*id) && real_in_j(id))
                    .filter(|id| not_current(id) && !ambiguous(id))
                    .map(|id| {
                        let h = j
                            .history
                            .iter()
                            .find(|h| h.id.as_ref() == Some(id))
                            .unwrap();
                        (h.edit.light.exposure, h.label.clone(), id[..8].to_string())
                    })
                    .collect();
                assert_eq!(shared_left, 0, "{why}: newer branch lost over shared");
                assert_eq!(
                    o_own_left,
                    0,
                    "{why}: newer branch lost over older branch: lost {:?}, left {left:?}; newer {:?}; older {:?}; join {:?}",
                    lost_edits(newer, &mn),
                    summary(newer),
                    summary(older),
                    summary(j)
                );
            }
            if o_own_lost {
                assert_eq!(shared_left, 0, "{why}: older branch lost over shared");
            }
            assert!(
                older
                    .current_id
                    .as_ref()
                    .is_some_and(|id| jids.contains(id))
                    || jedits.contains(&&older.current),
                "{why}: the older copy's current went"
            );
        }
    }

    /// An older build loading a copy and saving it: ids, revisions,
    /// field times and the absorbed list gone, `records` states
    /// recorded with no ids and that build's cap, a rating changed
    /// now and then; then this build reading the file back, with the
    /// time `at` on it.
    fn old_build_roundtrip(s: &Sidecar, at: u64, records: usize, rng: &mut Rng) -> Sidecar {
        let mut v = serde_json::to_value(s).unwrap();
        let o = v.as_object_mut().unwrap();
        for k in ["id", "revisions", "meta_at", "absorbed"] {
            o.remove(k);
        }
        for h in o.get_mut("history").unwrap().as_array_mut().unwrap() {
            h.as_object_mut().unwrap().remove("id");
        }
        let mut old: Sidecar = serde_json::from_value(v).unwrap();
        for _ in 0..records {
            let e = exposed(rng.below(10_000) as f32 / 100.0 + 200.0);
            let previous = Step {
                edit: std::mem::replace(&mut old.current, e),
                label: old.current_label.take(),
                exports: std::mem::take(&mut old.current_exports),
                id: None,
            };
            old.history.push(previous);
            if old.history.len() > HISTORY {
                let extra = old.history.len() - HISTORY;
                old.history.drain(1..1 + extra);
            }
        }
        if rng.chance(3) {
            old.meta.rating = rng.below(6) as u8;
        }
        if rng.chance(4) {
            old.turn = rng.below(4) as u8;
        }
        old.current_id = None;
        old.modified = Some(at);
        old.fill_ids();
        old.pin_times();
        old.baseline = Some(old.facts());
        old
    }

    /// One run: `places` machines and an archive, `rounds` of random
    /// work, each round each machine doing a few things and then
    /// syncing with the archive as part 2 will: a copy that is behind
    /// is copied over, otherwise the two are joined and the join goes
    /// to both. At the end every machine syncs until all are the same,
    /// and, where the cap was never reached, every edit the archive
    /// ever held is still in it unless it was undone.
    fn simulate(seed: u64, mode: Mode) {
        let mut rng = Rng(seed ^ if mode.old_builds { 0xA5A5_5A5A } else { 0 });
        let places = 3 + rng.below(3) as usize;
        let rounds = 6 + rng.below(3) as usize;
        let offsets: Vec<i64> = (0..places)
            .map(|_| {
                if mode.skew {
                    rng.below(400) as i64 - 200
                } else {
                    0
                }
            })
            .collect();
        let mut t: u64 = 10_000;
        let mut base = Sidecar::default();
        base.record(exposed(0.5));
        commit(&mut base, t, "archive");
        let mut archive = base.clone();
        let mut machines: Vec<Sidecar> = (0..places).map(|_| base.clone()).collect();
        let mut files = 0u64;
        let mut snaps = 0u64;
        let mut ever: Vec<Edit> = edits(&archive).into_iter().cloned().collect();
        let mut undone: Vec<Edit> = Vec::new();
        let mut capped = false;
        let hold = |archive: &Sidecar, ever: &mut Vec<Edit>, capped: &mut bool| {
            if archive.history.len() >= HISTORY {
                *capped = true;
            }
            for e in edits(archive) {
                if !ever.contains(e) {
                    ever.push(e.clone());
                }
            }
        };
        for _round in 0..rounds {
            for (m, s) in machines.iter_mut().enumerate() {
                t += 10;
                let at = (t as i64 + offsets[m]) as u64;
                let host = format!("m{m}");
                if mode.old_builds && rng.chance(3) {
                    *s = old_build_roundtrip(s, at, rng.below(3) as usize, &mut rng);
                } else {
                    let edits = if mode.heavy {
                        8 + rng.below(16)
                    } else {
                        1 + rng.below(3)
                    };
                    for _ in 0..edits {
                        match rng.below(12) {
                            0 | 1 if s.history.len() > 1 => {
                                s.undo();
                                if let Some(r) = s.redo.last() {
                                    undone.push(r.edit.clone());
                                }
                            }
                            2 => s.meta.rating = rng.below(6) as u8,
                            3 => {
                                s.meta.flag = if rng.chance(2) {
                                    Flag::Pick
                                } else {
                                    Flag::None
                                }
                            }
                            4 => {
                                files += 1;
                                let current = s.current.clone();
                                s.record_export(
                                    &current,
                                    Exported {
                                        file: format!("/out/{host}-{files}.jpg"),
                                        preset: None,
                                        at,
                                    },
                                );
                            }
                            5 if rng.chance(3) => {
                                snaps += 1;
                                s.take_snapshot(format!("{host} snapshot {snaps}"), at);
                            }
                            6 if rng.chance(4) => s.turn = rng.below(4) as u8,
                            _ => {
                                s.record(exposed(rng.below(10_000) as f32 / 100.0));
                                // A copy at the cap has dropped states
                                // by its own record, which no join and
                                // no copy is answerable for.
                                if s.history.len() >= HISTORY {
                                    capped = true;
                                }
                            }
                        }
                    }
                    commit(s, at, &host);
                }
                if rng.chance(4) {
                    // Offline this round.
                    continue;
                }
                sync(seed, s, &mut archive, at, &host, &undone, capped);
                hold(&archive, &mut ever, &mut capped);
            }
        }
        // (e) Syncing until nothing moves: every copy the same.
        for _ in 0..places + 3 {
            for (m, s) in machines.iter_mut().enumerate() {
                t += 10;
                let at = (t as i64 + offsets[m]) as u64;
                sync(seed, s, &mut archive, at, &format!("m{m}"), &undone, capped);
                hold(&archive, &mut ever, &mut capped);
            }
        }
        for (m, s) in machines.iter().enumerate() {
            assert_eq!(
                compare(s, &archive),
                Compared::Same,
                "seed {seed}: machine {m} not converged"
            );
        }
        // Ground truth, where the cap never came into it.
        if !capped {
            let now = edits(&archive);
            let lost: Vec<f32> = ever
                .iter()
                .filter(|e| !now.contains(e) && !undone.contains(e))
                .map(|e| e.light.exposure)
                .collect();
            assert!(
                lost.is_empty(),
                "seed {seed}: the archive once held {lost:?}, gone with no cap ({mode:?})"
            );
        }
    }

    fn sync(
        seed: u64,
        local: &mut Sidecar,
        archive: &mut Sidecar,
        t: u64,
        host: &str,
        undone: &[Edit],
        capped: bool,
    ) {
        let compared = compare(local, archive);
        if let Compared::Behind(side) = compared {
            // The copy that goes has nothing the one that stays lacks,
            // but what the cap took (now, or before an undo brought the
            // history back under it) or an undo put aside.
            let (gone, keep) = match side {
                Side::Local => (&*local, &*archive),
                Side::Other => (&*archive, &*local),
            };
            let kept = edits(keep);
            let lost: Vec<f32> = edits(gone)
                .into_iter()
                .filter(|e| !kept.contains(e) && !undone.contains(e))
                .map(|e| e.light.exposure)
                .collect();
            assert!(
                lost.is_empty() || capped || keep.history.len() == HISTORY,
                "seed {seed}: behind ({side:?}) would lose {lost:?}; local {:?} revs {} absorbed {}; archive {:?} revs {} absorbed {}; undone {:?}",
                summary(local),
                local.revisions.len(),
                local.absorbed.len(),
                summary(archive),
                archive.revisions.len(),
                archive.absorbed.len(),
                undone.iter().map(|e| e.light.exposure).collect::<Vec<_>>()
            );
        }
        match compared {
            Compared::Same => {}
            Compared::Behind(Side::Local) => *local = archive.clone(),
            Compared::Behind(Side::Other) => *archive = local.clone(),
            Compared::SameEdit | Compared::Diverged => {
                let mut j = join(local, archive);
                check_join(seed, local, archive, &j);
                commit(&mut j, t, host);
                *archive = j.clone();
                *local = j;
            }
        }
    }

    // The checks after each join run several more joins over
    // histories near the cap, which is where the time goes; the counts
    // here keep the three under half a minute in a debug build, and
    // `many_seeds_by_hand` runs the rest.
    #[test]
    fn places_and_an_archive_worked_at_random_lose_nothing_and_converge() {
        for seed in 0..12 {
            simulate(seed, Mode::default());
        }
    }

    #[test]
    fn places_and_an_archive_under_the_cap_lose_only_to_it_in_order_and_converge() {
        for seed in 1_000..1_012 {
            simulate(
                seed,
                Mode {
                    heavy: true,
                    ..Mode::default()
                },
            );
        }
    }

    #[test]
    fn places_on_older_builds_with_skewed_clocks_lose_nothing_and_converge() {
        for seed in 2_000..2_012 {
            simulate(
                seed,
                Mode {
                    old_builds: true,
                    skew: true,
                    ..Mode::default()
                },
            );
        }
    }

    /// `SIM_SEEDS=from..to` runs more by hand, `SIM_MODE` naming any of
    /// `heavy`, `old` and `skew`; with no mode, light and heavy
    /// alternate by seed.
    #[test]
    #[ignore]
    fn many_seeds_by_hand() {
        let Ok(range) = std::env::var("SIM_SEEDS") else {
            return;
        };
        let (from, to) = range.split_once("..").expect("from..to");
        let named = std::env::var("SIM_MODE").unwrap_or_default();
        for seed in from.parse::<u64>().unwrap()..to.parse::<u64>().unwrap() {
            let mode = if named.is_empty() {
                Mode {
                    heavy: seed % 2 == 1,
                    ..Mode::default()
                }
            } else {
                Mode {
                    heavy: named.contains("heavy"),
                    old_builds: named.contains("old"),
                    skew: named.contains("skew"),
                }
            };
            simulate(seed, mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{Field, Flag, Label, Set};
    use crate::{Placement, Snapshot};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-sync-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn exposed(stops: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = stops;
        e
    }

    fn stamp(at: u64, host: &str) -> Stamp {
        Stamp {
            at,
            host: host.into(),
        }
    }

    /// A frame's sidecar saved at `at` on `host`, beside the frame.
    fn save(sidecar: &mut Sidecar, raw: &Path, at: u64, host: &str) {
        sidecar
            .save_in_stamped(raw, Placement::Beside, &stamp(at, host))
            .unwrap();
    }

    /// The ids of every state, the current last.
    fn ids(s: &Sidecar) -> Vec<String> {
        state_ids(s)
    }

    /// The exposures of every state, the current last: the shape of
    /// a history in one line.
    fn exposures(s: &Sidecar) -> Vec<f32> {
        s.history
            .iter()
            .map(|s| s.edit.light.exposure)
            .chain(std::iter::once(s.current.light.exposure))
            .collect()
    }

    /// Two copies of one frame, as a backup makes them: three states
    /// recorded here and saved, then the file copied to the archive.
    /// Both are read back, so each is what the editor would hold.
    fn backed_up(dir: &Path) -> (Sidecar, Sidecar, PathBuf, PathBuf) {
        let local = dir.join("local").join("IMG_0001.CR3");
        let archive = dir.join("archive").join("IMG_0001.CR3");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        s.record(exposed(1.0));
        save(&mut s, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let a = Sidecar::read(&Sidecar::path_for(&local)).unwrap();
        let b = Sidecar::read(&Sidecar::path_for(&archive)).unwrap();
        (a, b, local, archive)
    }

    #[test]
    fn ids_are_stable_across_save_and_load() {
        let dir = scratch("stable");
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        s.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        let before = ids(&s);
        assert!(s.current_id.is_some() && s.history.iter().all(|h| h.id.is_some()));
        assert_eq!(before.len(), 3);
        assert_eq!(before.iter().collect::<HashSet<_>>().len(), 3);
        save(&mut s, &raw, 10, "desk");
        let back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(ids(&back), before);
        assert_eq!(back, s);
        // Where they sit in the file: `id` beside each state's fields.
        let tree: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(Sidecar::path_for(&raw)).unwrap())
                .unwrap();
        assert_eq!(tree["id"], before[2].as_str());
        assert_eq!(tree["history"][1]["id"], before[1].as_str());
        assert_eq!(tree["step"], "Preset: Faded film");
        // Undo and redo move the ids with their states.
        let mut back = back;
        back.undo();
        assert_eq!(back.current_id.as_deref(), Some(before[1].as_str()));
        assert_eq!(back.redo[0].id.as_deref(), Some(before[2].as_str()));
        back.redo();
        assert_eq!(ids(&back), before);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_machines_reaching_the_same_state_get_the_same_id() {
        let mut a = Sidecar::default();
        let mut b = Sidecar::default();
        for s in [&mut a, &mut b] {
            s.record(exposed(0.5));
            s.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        }
        assert_eq!(ids(&a), ids(&b));
        assert_eq!(
            a.history[0].id.as_deref(),
            Some(state_id(ZERO_ID, &Edit::default(), None).as_str())
        );
        // The same edit under another label, or after another parent,
        // is another state.
        let mut c = Sidecar::default();
        c.record(exposed(0.5));
        c.record(exposed(1.0));
        assert_ne!(c.current_id, a.current_id);
        let mut d = Sidecar::default();
        d.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        assert_ne!(d.current_id, a.current_id);
    }

    #[test]
    fn a_stored_id_is_kept_whatever_the_file_spelled_the_edit_as() {
        // `norm` reads as the per-channel curve (§232) and writes back
        // as nothing, so recomputing this state's id would give the
        // id of a different file; the stored one stands.
        let stored = "ab".repeat(32);
        let parent = "cd".repeat(32);
        let json = format!(
            r#"{{"current":{{"version":4,"display_curve":"norm"}},"id":"{stored}",
                "history":[{{"version":4,"id":"{parent}"}}]}}"#
        );
        let mut s: Sidecar = serde_json::from_str(&json).unwrap();
        assert!(!s.fill_ids());
        assert_eq!(s.current_id.as_deref(), Some(stored.as_str()));
        assert_eq!(s.history[0].id.as_deref(), Some(parent.as_str()));
        assert_ne!(
            state_id(&parent, &s.current, None),
            stored,
            "the stored id is not what this build would compute"
        );
        assert_eq!(ids(&s), [parent, stored]);
    }

    #[test]
    fn a_sidecar_from_before_ids_gets_them_on_its_first_read_and_keeps_them() {
        let dir = scratch("pre-id");
        let raw = dir.join("IMG_0001.CR3");
        // What this build records, and what an earlier one wrote for
        // the same states, must come out with the same ids.
        let mut now = Sidecar::default();
        now.record(exposed(0.5));
        now.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        let mut tree: serde_json::Value = serde_json::from_str(&now.to_json()).unwrap();
        tree.as_object_mut().unwrap().remove("id");
        for state in tree["history"].as_array_mut().unwrap() {
            state.as_object_mut().unwrap().remove("id");
        }
        tree.as_object_mut().unwrap().remove("revisions");
        std::fs::write(Sidecar::path_for(&raw), tree.to_string()).unwrap();

        let mut old = Sidecar::load(&raw).unwrap().unwrap();
        assert!(old.history.iter().all(|h| h.id.is_some()));
        assert_eq!(ids(&old), ids(&now));
        assert!(old.revisions.is_empty());
        // Kept: a save writes them, and the next read computes none.
        save(&mut old, &raw, 5, "desk");
        let mut again = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(ids(&again), ids(&now));
        assert!(!again.fill_ids());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_older_format_sidecar_loads_and_a_bad_new_field_costs_only_itself() {
        let old: Sidecar = serde_json::from_str(
            r#"{"current":{"version":4,"light":{"exposure":1.0}},"meta":{"rating":3},
                "saved":2,"history":[{"version":4}],"snapshots":[]}"#,
        )
        .unwrap();
        assert_eq!(old.current.light.exposure, 1.0);
        assert_eq!(old.meta.rating, 3);
        assert!(old.revisions.is_empty());
        assert!(old.meta_at.is_empty());
        assert!(old.current_id.is_none(), "a plain read computes nothing");
        // The new fields read loosely: a wrong id, a revision list
        // that is not a list, a line that is not a revision, a time
        // that is not a number.
        let odd: Sidecar = serde_json::from_str(
            r#"{"current":{"version":4},"id":5,"revisions":"soon",
                "meta_at":{"rating":"x","flag":7},
                "history":[{"version":4,"id":[1]}]}"#,
        )
        .unwrap();
        assert!(odd.current_id.is_none());
        assert!(odd.history[0].id.is_none());
        assert!(odd.revisions.is_empty());
        assert_eq!(odd.meta_at.rating.at, 0);
        assert_eq!(odd.meta_at.flag.at, 7);
        let lines: Sidecar = serde_json::from_str(
            r#"{"current":{"version":4},
                "revisions":[7,"zz 12","abc 12 desk","0123 soon","4567 13 two words"]}"#,
        )
        .unwrap();
        let revision = |hash: &str, at, host: &str| Revision {
            hash: hash.into(),
            at,
            host: host.into(),
        };
        assert_eq!(
            lines.revisions,
            [
                revision("abc", 12, "desk"),
                revision("0123", 0, ""),
                revision("4567", 13, "two words"),
            ]
        );
    }

    /// What a build from before §233 does with a sidecar this one
    /// wrote: `Sidecar` and `Edit` are `#[serde(default)]` structs
    /// without `deny_unknown_fields`, so the keys it does not know are
    /// skipped, on the sidecar (`id`, `meta_at`, `revisions`) and
    /// inside each state, whose `id` lands in the edit's object the
    /// way `step` and `exported` already did before that build knew
    /// them. This is the old shape read against the new file.
    #[test]
    fn a_build_before_ids_and_revisions_reads_a_new_sidecar_whole() {
        let dir = scratch("old-build");
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        s.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        s.meta.rating = 4;
        s.take_snapshot("Warm", 7);
        save(&mut s, &raw, 10, "desk");
        let json = std::fs::read_to_string(Sidecar::path_for(&raw)).unwrap();
        assert!(json.contains("\"revisions\"") && json.contains("\"meta_at\""));
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Before {
            current: Edit,
            meta: crate::Meta,
            saved: u64,
            history: Vec<Edit>,
            snapshots: Vec<Snapshot>,
        }
        let old: Before = serde_json::from_str(&json).unwrap();
        assert_eq!(old.current, s.current);
        assert_eq!(old.meta.rating, 4);
        assert_eq!(old.saved, 1);
        let edits: Vec<Edit> = s.history.iter().map(|h| h.edit.clone()).collect();
        assert_eq!(old.history, edits);
        assert_eq!(old.snapshots, s.snapshots);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_save_is_a_revision_chained_on_the_last_and_the_list_is_capped() {
        let dir = scratch("revisions");
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        save(&mut s, &raw, 100, "desk");
        assert_eq!(s.revisions.len(), 1);
        assert_eq!(s.revisions[0].at, 100);
        assert_eq!(s.revisions[0].host, "desk");
        assert_eq!(s.revisions[0].hash.len(), 32);
        // A save that only changes the meta is a revision too, and a
        // save that changes nothing at all still is: the chain says
        // when the file was written, not only what.
        s.meta.rating = 3;
        save(&mut s, &raw, 101, "desk");
        save(&mut s, &raw, 102, "laptop");
        assert_eq!(s.revisions.len(), 3);
        let hashes: HashSet<&str> = s.revisions.iter().map(|r| r.hash.as_str()).collect();
        assert_eq!(hashes.len(), 3);
        // The file reads back with them, as lines.
        let mut back = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(back.revisions, s.revisions);
        let json = std::fs::read_to_string(Sidecar::path_for(&raw)).unwrap();
        assert!(
            json.contains(&format!("\"{} 102 laptop\"", s.revisions[2].hash)),
            "{json}"
        );
        // The content hash leaves the list itself out, so it can be
        // recomputed from the file as written.
        let previous = Some(s.revisions[1].hash.as_str());
        assert_eq!(revision_hash(previous, &mut back), s.revisions[2].hash);
        for i in 0..REVISIONS + 5 {
            save(&mut s, &raw, 200 + i as u64, "desk");
        }
        assert_eq!(s.revisions.len(), REVISIONS);
        assert_eq!(s.revisions[0].at, 205, "the oldest went first");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_meta_fields_are_stamped_against_the_file_as_read() {
        let dir = scratch("meta-at");
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar::default();
        s.meta.rating = 3;
        s.meta.add_keyword("sea");
        save(&mut s, &raw, 100, "desk");
        assert_eq!((s.meta_at.rating.at, s.meta_at.keywords.at), (100, 100));
        assert_eq!(s.meta_at.flag.at, 0, "never set, never stamped");
        let mut back = Sidecar::load(&raw).unwrap().unwrap();
        back.meta.flag = Flag::Pick;
        back.meta.add_keyword("rocks");
        save(&mut back, &raw, 200, "desk");
        assert_eq!(back.meta_at.rating.at, 100, "unchanged, unstamped");
        assert_eq!(back.meta_at.flag.at, 200);
        assert_eq!(back.meta_at.keywords.at, 200, "the keywords are one field");
        // Cleared is a change too.
        let mut back = Sidecar::load(&raw).unwrap().unwrap();
        back.meta.rating = 0;
        save(&mut back, &raw, 300, "desk");
        assert_eq!(back.meta_at.rating.at, 300);
        let tree: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(Sidecar::path_for(&raw)).unwrap())
                .unwrap();
        assert!(
            tree["meta_at"]["rating"]
                .as_str()
                .unwrap()
                .starts_with("300 "),
            "{tree}"
        );
        assert!(tree["meta_at"].get("title").is_none(), "a 0 is left out");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_copies_of_one_file_are_the_same_and_a_save_puts_the_other_behind() {
        let dir = scratch("behind");
        let (mut a, b, local, _) = backed_up(&dir);
        assert_eq!(compare(&a, &b), Compared::Same);
        a.record(exposed(2.0));
        save(&mut a, &local, 200, "desk");
        assert_eq!(compare(&a, &b), Compared::Behind(Side::Other));
        assert_eq!(compare(&b, &a), Compared::Behind(Side::Local));
        // A save that only undid, or only rated, orders the same way.
        let (mut a, b, local, _) = backed_up(&dir);
        a.undo();
        save(&mut a, &local, 200, "desk");
        assert_eq!(compare(&b, &a), Compared::Behind(Side::Local));
        let (mut a, b, local, _) = backed_up(&dir);
        a.meta.rating = 5;
        save(&mut a, &local, 200, "desk");
        assert_eq!(compare(&b, &a), Compared::Behind(Side::Local));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_same_latest_revision_is_the_same_edit_with_the_meta_to_join() {
        let dir = scratch("same-edit");
        let (a, mut b, _, _) = backed_up(&dir);
        // The same revision on both, the copies no longer equal.
        b.meta.rating = 2;
        assert_eq!(compare(&a, &b), Compared::SameEdit);
        let joined = join(&a, &b);
        assert_eq!(joined.history, a.history, "no state added");
        assert_eq!(joined.current, a.current);
        assert_eq!(joined.current_id, a.current_id);
        assert_eq!(joined.current_label, a.current_label);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn both_saved_since_the_copy_is_diverged_and_the_join_keeps_both() {
        let dir = scratch("diverged");
        let (mut a, mut b, local, archive) = backed_up(&dir);
        let shared = ids(&a);
        a.record(exposed(2.0));
        save(&mut a, &local, 200, "desk");
        b.record_as(exposed(-1.0), Some("Preset: Night".into()));
        save(&mut b, &archive, 300, "laptop");
        assert_eq!(compare(&a, &b), Compared::Diverged);
        assert_eq!(compare(&b, &a), Compared::Diverged);

        // The laptop saved later: its branch goes on top, the desk's
        // under it, then the laptop's current again, reconciled.
        let joined = join(&a, &b);
        assert_eq!(exposures(&joined), [0.0, 0.5, 1.0, -1.0, 2.0, -1.0]);
        assert_eq!(
            joined.current_label.as_deref(),
            Some("Reconciled with the copy on desk")
        );
        assert_eq!(&ids(&joined)[..3], &shared[..]);
        assert_eq!(joined.history[3].id, b.current_id);
        assert_eq!(joined.history[3].label.as_deref(), Some("Preset: Night"));
        assert_eq!(joined.history[4].id, a.current_id);
        assert!(joined.redo.is_empty());
        assert_eq!(joined.saved, 2);
        // One undo brings back what the other machine had.
        let mut walk = joined.clone();
        walk.undo();
        assert_eq!(walk.current, a.current);
        // Saved, the join is ahead of both.
        let mut joined = joined;
        save(&mut joined, &local, 400, "desk");
        assert_eq!(compare(&joined, &a), Compared::Behind(Side::Other));
        assert_eq!(compare(&joined, &b), Compared::Behind(Side::Other));
        assert_eq!(compare(&a, &joined), Compared::Behind(Side::Local));
        assert_eq!(compare(&b, &joined), Compared::Behind(Side::Local));
        // And the same file on both copies is the same.
        joined.write_to(&Sidecar::path_for(&archive)).unwrap();
        let (l, r) = (
            Sidecar::read(&Sidecar::path_for(&local)).unwrap(),
            Sidecar::read(&Sidecar::path_for(&archive)).unwrap(),
        );
        assert_eq!(compare(&l, &r), Compared::Same);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_newer_copy_is_by_time_and_a_tie_is_broken_the_same_way_from_either_side() {
        let dir = scratch("newer");
        let (mut a, mut b, local, archive) = backed_up(&dir);
        a.record(exposed(2.0));
        b.record(exposed(-1.0));
        // The local saved later: its current on top.
        save(&mut a, &local, 300, "desk");
        save(&mut b, &archive, 200, "laptop");
        let j = join(&a, &b);
        assert_eq!(j.current.light.exposure, 2.0);
        assert_eq!(j.history.last().unwrap().edit.light.exposure, -1.0);
        assert_eq!(
            j.current_label.as_deref(),
            Some("Reconciled with the copy on laptop")
        );
        assert_eq!(join(&b, &a), j, "the same join from either side");
        // A tie by time is broken by the revision hash, the same way
        // on both machines, so neither side's name decides.
        let mut a2 = a.clone();
        a2.revisions.last_mut().unwrap().at = 200;
        let (x, y) = (join(&a2, &b), join(&b, &a2));
        assert_eq!(x, y);
        let on_top = if a2.revisions.last().unwrap().hash > b.revisions.last().unwrap().hash {
            2.0
        } else {
            -1.0
        };
        assert_eq!(x.current.light.exposure, on_top);
        // A copy whose time is not known, neither from a revision nor
        // from its file, is older than one whose is.
        let mut b0 = b.clone();
        b0.revisions.last_mut().unwrap().at = 0;
        b0.modified = None;
        assert_eq!(join(&a2, &b0).current.light.exposure, 2.0);
        let mut a0 = a.clone();
        a0.revisions.last_mut().unwrap().at = 0;
        a0.modified = None;
        assert_eq!(join(&a0, &b).current.light.exposure, -1.0);
        // Known from the file, it counts as the copy's time.
        b0.modified = Some(250);
        assert_eq!(join(&a2, &b0).current.light.exposure, -1.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A copy at state 4 and a copy at state 5 with 4 to undo are
    /// either the first having undone or the second having redone.
    /// With revisions each save says so: both saved since the copy,
    /// neither in the other's list, so they diverged. The join holds
    /// every state, but the files cannot tell an undo from a copy
    /// that only saved again at the state it stood on, and that copy
    /// adds nothing: so the undone position is not recorded again
    /// and the join is the longer copy's history, the undone state
    /// in it. Without revisions the shorter copy's states all stand
    /// in the longer's, and it reads as behind. Nothing is lost
    /// either way.
    #[test]
    fn the_undo_ambiguity_is_diverged_with_revisions_and_behind_without() {
        let dir = scratch("undo");
        let (mut a, mut b, local, archive) = backed_up(&dir);
        a.undo();
        save(&mut a, &local, 200, "desk");
        b.record(exposed(2.0));
        save(&mut b, &archive, 300, "laptop");
        assert_eq!(compare(&a, &b), Compared::Diverged);
        let j = join(&a, &b);
        assert_eq!(exposures(&j), [0.0, 0.5, 1.0, 2.0]);
        assert_eq!(j.current_label, None);
        assert_eq!(ids(&j), ids(&b));
        assert_eq!(join(&b, &a), j);

        a.revisions.clear();
        b.revisions.clear();
        assert_eq!(compare(&a, &b), Compared::Behind(Side::Local));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_pair_from_before_revisions_is_compared_by_its_state_ids() {
        let mut a = Sidecar::default();
        a.record(exposed(0.5));
        a.record(exposed(1.0));
        let mut b = a.clone();
        assert_eq!(compare(&a, &b), Compared::Same);
        b.record(exposed(2.0));
        assert_eq!(compare(&a, &b), Compared::Behind(Side::Local));
        assert_eq!(compare(&b, &a), Compared::Behind(Side::Other));
        // A rating on either copy that the other does not have is not
        // behind: the join settles it, and adds no state for it.
        b.meta.rating = 1;
        assert_eq!(compare(&a, &b), Compared::Diverged);
        let j = join(&a, &b);
        assert_eq!(exposures(&j), [0.0, 0.5, 1.0, 2.0]);
        assert_eq!(j.meta.rating, 1);
        // Both gone on: diverged, the join the same from either side,
        // with no host to name.
        a.record(exposed(3.0));
        assert_eq!(compare(&a, &b), Compared::Diverged);
        let j = join(&a, &b);
        assert_eq!(j, join(&b, &a));
        assert_eq!(
            j.current_label.as_deref(),
            Some("Reconciled with the other copy")
        );
        assert_eq!(j.history.len(), 5);
        assert!([2.0, 3.0].contains(&j.current.light.exposure));
        assert!(j.revisions.is_empty());
    }

    #[test]
    fn a_pair_from_before_ids_is_compared_once_the_ids_are_filled() {
        let dir = scratch("pre-id-pair");
        let strip = |s: &Sidecar| {
            let mut tree: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
            tree.as_object_mut().unwrap().remove("id");
            for state in tree["history"].as_array_mut().unwrap() {
                state.as_object_mut().unwrap().remove("id");
            }
            tree.to_string()
        };
        let mut a = Sidecar::default();
        a.record(exposed(0.5));
        let b = a.clone();
        a.record(exposed(1.0));
        let (pa, pb) = (dir.join("a.gcd"), dir.join("b.gcd"));
        std::fs::write(&pa, strip(&a)).unwrap();
        std::fs::write(&pb, strip(&b)).unwrap();
        let (ra, rb) = (Sidecar::read(&pa).unwrap(), Sidecar::read(&pb).unwrap());
        assert!(ra.current_id.is_some() && rb.current_id.is_some());
        assert_eq!(compare(&ra, &rb), Compared::Behind(Side::Other));
        // And a sidecar not read from a file is compared the same
        // way, its ids computed for the comparison and not kept.
        let plain: Sidecar = serde_json::from_str(&strip(&b)).unwrap();
        assert!(plain.current_id.is_none());
        assert_eq!(compare(&plain, &ra), Compared::Behind(Side::Local));
        assert!(plain.current_id.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_cap_leaves_the_first_state_shared_and_the_join_starts_there() {
        let dir = scratch("cap");
        let (mut a, mut b, local, archive) = backed_up(&dir);
        let first = a.history[0].id.clone();
        for i in 0..HISTORY + 10 {
            a.record(exposed(10.0 + i as f32));
        }
        assert_eq!(a.history.len(), HISTORY);
        assert_eq!(a.history[0].id, first);
        assert!(!ids(&a).contains(b.history[1].id.as_ref().unwrap()));
        save(&mut a, &local, 300, "desk");
        b.record(exposed(-1.0));
        save(&mut b, &archive, 200, "laptop");
        assert_eq!(compare(&a, &b), Compared::Diverged);
        let j = join(&a, &b);
        assert_eq!(j.history[0].id, first);
        assert_eq!(j.history.len(), HISTORY);
        // The laptop's own intermediate states go to the cap before
        // the desk's; its current stays, one undo from the desk's.
        let last = 10.0 + (HISTORY + 9) as f32;
        let tail: Vec<f32> = j.history[HISTORY - 4..]
            .iter()
            .map(|s| s.edit.light.exposure)
            .collect();
        assert_eq!(tail, [last - 2.0, last - 1.0, last, -1.0]);
        assert_eq!(j.current.light.exposure, last);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_meta_joins_field_by_field_by_time() {
        let dir = scratch("meta-join");
        let (mut a, mut b, local, archive) = backed_up(&dir);
        a.meta.rating = 4;
        a.meta.set_keywords(vec!["sea".into(), "rocks".into()]);
        save(&mut a, &local, 200, "desk");
        b.meta.label = Label::Red;
        b.meta.set_keywords(vec!["sea".into()]);
        save(&mut b, &archive, 300, "laptop");
        // The laptop is newer, but each field goes by its own time:
        // the rating set on the desk stands, the label set on the
        // laptop stands, and the keywords, one field, are the later
        // write's, so the one taken off stays off.
        let j = join(&a, &b);
        assert_eq!(j.meta.rating, 4);
        assert_eq!(j.meta.label, Label::Red);
        assert_eq!(j.meta.keywords, ["sea"]);
        assert_eq!(
            (
                j.meta_at.rating.at,
                j.meta_at.label.at,
                j.meta_at.keywords.at
            ),
            (200, 300, 300)
        );
        // Each field changed on each side: the later one wins.
        let mut a2 = a.clone();
        a2.meta.rating = 1;
        save(&mut a2, &local, 400, "desk");
        assert_eq!(join(&a2, &b).meta.rating, 1);
        assert_eq!(join(&b, &a2).meta.rating, 1);
        // Saving the join stamps nothing it did not change.
        let mut j = join(&a2, &b);
        save(&mut j, &local, 500, "desk");
        assert_eq!((j.meta_at.rating.at, j.meta_at.label.at), (400, 300));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_meta_from_before_the_times_is_as_old_as_its_file() {
        let dir = scratch("meta-old");
        let (mut a, _, local, archive) = backed_up(&dir);
        a.meta.rating = 4;
        save(&mut a, &local, 200, "desk");
        // The laptop's copy rated and flagged by a build with no
        // times, and no revisions: its file's time says 300, and so
        // does every field it set. The fields it did not set are not
        // pinned, so the desk's rating stands against nothing.
        old_build_rewrite(&archive, 300, |v| {
            v["meta"] = serde_json::json!({"rating": 2, "flag": "pick"});
        });
        let b = Sidecar::read(&Sidecar::path_for(&archive)).unwrap();
        assert_eq!(b.meta_at.rating.at, 300);
        assert_eq!(b.meta_at.flag.at, 300);
        assert!(b.meta_at.label.is_unset());
        let j = join(&a, &b);
        assert_eq!(j.meta.rating, 2);
        assert_eq!(j.meta.flag, Flag::Pick);
        assert_eq!(j.meta_at.rating.at, 300);
        assert_eq!(join(&b, &a), j);
        // Pinned once: a save keeps the time, and a later read does
        // not move it.
        let mut b = b;
        save(&mut b, &archive, 900, "laptop");
        let again = Sidecar::read(&Sidecar::path_for(&archive)).unwrap();
        assert_eq!(again.meta_at.rating.at, 300);
        // And a copy from before, with an earlier file, loses.
        let (mut a, _, local, archive) = backed_up(&dir);
        a.meta.rating = 4;
        save(&mut a, &local, 200, "desk");
        old_build_rewrite(&archive, 150, |v| {
            v["meta"] = serde_json::json!({"rating": 2});
        });
        let b = Sidecar::read(&Sidecar::path_for(&archive)).unwrap();
        assert_eq!(join(&a, &b).meta.rating, 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn snapshots_join_by_name_and_a_clash_keeps_both() {
        let dir = scratch("snapshots");
        let (mut a, mut b, local, archive) = backed_up(&dir);
        a.take_snapshot("Warm", 1);
        a.record(exposed(2.0));
        a.take_snapshot("Bright", 2);
        save(&mut a, &local, 200, "desk");
        b.record(exposed(-1.0));
        b.take_snapshot("Warm", 3);
        b.take_snapshot("Shared", 4);
        b.undo();
        b.take_snapshot("Shared", 5);
        save(&mut b, &archive, 300, "laptop");
        let j = join(&a, &b);
        let names: Vec<&str> = j.snapshots.iter().map(|s| s.name.as_str()).collect();
        // The laptop's first, the desk's that are new, and the desk's
        // "Warm" under a name that says where it came from; a name
        // the same copy used twice is left as it was.
        assert_eq!(
            names,
            ["Warm", "Shared", "Shared", "Warm (on desk)", "Bright"]
        );
        assert_eq!(j.snapshots[0].edit.light.exposure, -1.0);
        assert_eq!(j.snapshots[3].edit.light.exposure, 1.0);
        // The same snapshot on both is one.
        let (mut a, mut b, local, archive) = backed_up(&dir);
        a.take_snapshot("Warm", 1);
        b.take_snapshot("Warm", 1);
        save(&mut a, &local, 200, "desk");
        save(&mut b, &archive, 300, "laptop");
        assert_eq!(join(&a, &b).snapshots.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn export_records_join_by_state_id() {
        let dir = scratch("exports");
        let (mut a, mut b, local, archive) = backed_up(&dir);
        let shared = a.current.clone();
        let record = |file: &str, at: u64| Exported {
            file: file.into(),
            preset: None,
            at,
        };
        a.record_export(&shared, record("/out/desk.jpg", 1));
        a.record(exposed(2.0));
        a.record_export(&a.current.clone(), record("/out/desk2.jpg", 2));
        save(&mut a, &local, 200, "desk");
        b.record_export(&shared, record("/out/laptop.jpg", 3));
        b.record_export(&shared, record("/out/desk.jpg", 1));
        save(&mut b, &archive, 300, "laptop");
        let j = join(&a, &b);
        // The shared state, third of the history, has both machines'
        // records, the same one once; the desk's own state keeps its
        // own; the reconciled current has none of its own.
        let files = |e: &[Exported]| e.iter().map(|e| e.file.clone()).collect::<Vec<_>>();
        assert_eq!(
            files(&j.history[2].exports),
            ["/out/laptop.jpg", "/out/desk.jpg"]
        );
        assert_eq!(files(&j.history[3].exports), ["/out/desk2.jpg"]);
        assert!(j.current_exports.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_revision_line_round_trips_and_the_host_is_this_machines() {
        let r = Revision {
            hash: "0123abcd".into(),
            at: 1_700_000_000,
            host: "desk".into(),
        };
        let line = serde_json::to_string(&r).unwrap();
        assert_eq!(line, r#""0123abcd 1700000000 desk""#);
        assert_eq!(serde_json::from_str::<Revision>(&line).unwrap(), r);
        let bare = Revision {
            host: String::new(),
            ..r.clone()
        };
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#""0123abcd 1700000000""#
        );
        let now = Stamp::now();
        assert!(now.at > 1_700_000_000);
        assert_eq!(now.host, host());
    }
    /// What a build before §233 leaves after it loads a sidecar and
    /// saves it: no ids, no revisions, no times; `change` is what it
    /// did, and `modified` the time its save left on the file.
    fn old_build_rewrite(raw: &Path, modified: u64, change: impl FnOnce(&mut serde_json::Value)) {
        let p = Sidecar::path_for(raw);
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("id");
        o.remove("revisions");
        o.remove("meta_at");
        for s in o.get_mut("history").unwrap().as_array_mut().unwrap() {
            s.as_object_mut().unwrap().remove("id");
        }
        change(&mut v);
        std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(modified))
            .unwrap();
    }

    /// A state added the way an older build writes one.
    fn add_state(v: &mut serde_json::Value, exposure: f32) {
        let current = v["current"].clone();
        v["history"].as_array_mut().unwrap().push(current);
        v["current"]["light"]["exposure"] = serde_json::json!(exposure);
    }

    fn reread(raw: &Path) -> Sidecar {
        Sidecar::read(&Sidecar::path_for(raw)).unwrap()
    }

    /// A copy whose states all stand in the other's is still not
    /// behind when it carries something else the other lacks: a
    /// rating, a snapshot, a turn. The states are all the ids see.
    #[test]
    fn a_prefix_copy_with_something_of_its_own_is_not_behind() {
        let dir = scratch("prefix-own");
        // Both copies last written by an older build: the archive
        // added a state, the desk only rated, later. Not behind; the
        // join keeps the state and the rating.
        let (_, _, local, archive) = backed_up(&dir);
        old_build_rewrite(&archive, 300, |v| add_state(v, 2.0));
        old_build_rewrite(&local, 400, |v| {
            v["meta"] = serde_json::json!({"rating": 5})
        });
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(compare(&l, &a), Compared::Diverged);
        let j = join(&l, &a);
        assert_eq!(j.meta.rating, 5);
        // The desk's file is the later one, so its current goes on top
        // of the archive's branch, as §233 has it.
        assert_eq!(exposures(&j), [0.0, 0.5, 1.0, 2.0, 1.0]);
        assert_eq!(
            j.current_label.as_deref(),
            Some("Reconciled with the other copy")
        );
        assert_eq!(join(&a, &l), j);
        // The other way about, the archive written later without the
        // rating: between two copies with no revisions a value
        // survives over none, since nothing says the field was ever
        // cleared rather than never set. Still not behind.
        old_build_rewrite(&archive, 500, |_| {});
        let a = reread(&archive);
        assert_eq!(compare(&l, &a), Compared::Diverged);
        assert_eq!(join(&l, &a).meta.rating, 5);
        assert_eq!(join(&a, &l).meta.rating, 5);
        // And so a rating cleared by an older build, against another
        // copy with no revisions that still has it, comes back: the
        // lossless choice, and the one thing that can.
        let (_, _, local, archive) = backed_up(&dir);
        old_build_rewrite(&local, 300, |v| {
            v["meta"] = serde_json::json!({"rating": 4})
        });
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        old_build_rewrite(&archive, 600, |v| v["meta"] = serde_json::json!({}));
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(join(&l, &a).meta.rating, 4);
        assert_eq!(join(&a, &l).meta.rating, 4);

        // The desk on this build: a rating, a snapshot and a turn,
        // the archive's copy an older build's with a state added.
        let (_, _, local, archive) = backed_up(&dir);
        old_build_rewrite(&archive, 400, |v| add_state(v, 2.0));
        let mut l = reread(&local);
        l.meta.rating = 5;
        l.take_snapshot("Mine", 3);
        l.turn = 1;
        save(&mut l, &local, 200, "desk");
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(compare(&l, &a), Compared::Diverged);
        assert_eq!(compare(&a, &l), Compared::Diverged);
        let j = join(&l, &a);
        assert_eq!((j.meta.rating, j.snapshots.len(), j.turn), (5, 1, 1));
        assert_eq!(exposures(&j), [0.0, 0.5, 1.0, 2.0]);

        // An export record on the shorter copy is something of its
        // own too; the same record on both is not.
        let (mut l, mut a, local, archive) = backed_up(&dir);
        let record = Exported {
            file: "/out/a.jpg".into(),
            preset: None,
            at: 1,
        };
        l.record_export(&l.current.clone(), record.clone());
        a.record(exposed(2.0));
        l.revisions.clear();
        a.revisions.clear();
        assert_eq!(compare(&l, &a), Compared::Diverged);
        a.history.last_mut().unwrap().exports.push(record);
        assert_eq!(compare(&l, &a), Compared::Behind(Side::Local));
        drop((local, archive));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A meta from before the times is pinned to its file's time once
    /// and does not move with the saves that follow, so a rating set
    /// on the other copy after it wins; and a field that copy never
    /// set is not pinned, so the other copy's flag wins too.
    #[test]
    fn a_meta_from_before_the_times_does_not_drift_with_later_saves() {
        let dir = scratch("drift");
        let (_, _, local, archive) = backed_up(&dir);
        old_build_rewrite(&local, 300, |v| {
            v["meta"] = serde_json::json!({"rating": 3})
        });
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        // The laptop rates 5 and flags at 500.
        let mut a = reread(&archive);
        a.meta.rating = 5;
        a.meta.flag = Flag::Pick;
        save(&mut a, &archive, 500, "laptop");
        // The desk only edits, saving at 400, 600 and 1000.
        let mut l = reread(&local);
        for (i, t) in [400u64, 600, 1000].into_iter().enumerate() {
            l.record(exposed(3.0 + i as f32));
            save(&mut l, &local, t, "desk");
        }
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(l.meta_at.rating.at, 300, "pinned at the file's time");
        assert!(l.meta_at.flag.is_unset());
        assert_eq!(compare(&l, &a), Compared::Diverged);
        let j = join(&l, &a);
        assert_eq!(j.meta.rating, 5);
        assert_eq!(j.meta.flag, Flag::Pick);
        assert_eq!(join(&a, &l).meta, j.meta);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A change made by an older build has its file's time, so it is
    /// ordered against this build's by when it was made, not lost for
    /// having no time of its own.
    #[test]
    fn a_change_by_an_older_build_is_as_old_as_its_file() {
        let dir = scratch("old-rating");
        let (_, _, local, archive) = backed_up(&dir);
        let mut l = reread(&local);
        l.meta.rating = 3;
        save(&mut l, &local, 150, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        // The laptop, on an older build, rates 5 afterwards.
        old_build_rewrite(&archive, 160, |v| {
            v["meta"]["rating"] = serde_json::json!(5)
        });
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(join(&l, &a).meta.rating, 5);
        assert_eq!(join(&a, &l).meta.rating, 5);
        // And one made before this build's loses to it.
        old_build_rewrite(&archive, 140, |v| {
            v["meta"]["rating"] = serde_json::json!(5)
        });
        let a = reread(&archive);
        assert_eq!(join(&l, &a).meta.rating, 3);
        assert_eq!(join(&a, &l).meta.rating, 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_turn_on_the_other_copy_survives_the_join() {
        let dir = scratch("turn");
        let (mut l, mut a, local, archive) = backed_up(&dir);
        l.turn = 1;
        save(&mut l, &local, 200, "desk");
        assert_eq!(l.meta_at.turn.at, 200);
        a.record(exposed(-1.0));
        save(&mut a, &archive, 300, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(compare(&l, &a), Compared::Diverged);
        assert_eq!(join(&l, &a).turn, 1);
        assert_eq!(join(&a, &l).turn, 1);
        // Turned on both: the later turn.
        let mut a = a;
        a.turn = 3;
        save(&mut a, &archive, 400, "laptop");
        assert_eq!(join(&l, &a).turn, 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A write the other copy already carries, by the revision that
    /// made it, is older than one it does not, whatever the clocks
    /// say: the desk's clock is behind the laptop's, and the desk's
    /// rating still wins because the desk had the laptop's.
    #[test]
    fn a_write_the_other_copy_knows_loses_whatever_the_clocks_say() {
        let dir = scratch("causal");
        let (_, _, local, archive) = backed_up(&dir);
        let mut a = reread(&archive);
        a.meta.rating = 5;
        save(&mut a, &archive, 500, "laptop");
        // Synced to the desk, which then rates 2 on a clock reading
        // 400.
        std::fs::copy(Sidecar::path_for(&archive), Sidecar::path_for(&local)).unwrap();
        let mut l = reread(&local);
        l.meta.rating = 2;
        save(&mut l, &local, 400, "desk");
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(compare(&l, &a), Compared::Behind(Side::Other));
        assert_eq!(join(&l, &a).meta.rating, 2);
        assert_eq!(join(&a, &l).meta.rating, 2);
        // The laptop goes on to rate 4 at 600 without the desk's
        // write: both changed since they were the same, and only
        // then does the clock decide.
        let mut a = a;
        a.meta.rating = 4;
        save(&mut a, &archive, 600, "laptop");
        let a = reread(&archive);
        assert_eq!(compare(&l, &a), Compared::Diverged);
        assert_eq!(join(&l, &a).meta.rating, 4);
        assert_eq!(join(&a, &l).meta.rating, 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A copy that only saved again where it stood has nothing the
    /// other's states do not say: the join is the other's history,
    /// with no state recorded again and no reconciled state.
    #[test]
    fn a_copy_that_only_saved_again_adds_no_states_to_the_join() {
        let dir = scratch("noop");
        let (mut l, mut a, local, archive) = backed_up(&dir);
        l.record(exposed(2.0));
        save(&mut l, &local, 200, "desk");
        save(&mut a, &archive, 150, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(compare(&l, &a), Compared::Diverged);
        let j = join(&l, &a);
        assert_eq!(exposures(&j), exposures(&l));
        assert_eq!(ids(&j), ids(&l));
        assert_eq!(j.current_label, None);
        assert_eq!(join(&a, &l), j);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A copy left behind by more saves than the revision list holds
    /// has its latest revision no longer in the other's list, and
    /// reads as diverged. The join then adds nothing, and keeps that
    /// copy's latest revision past the cap, so once the join is saved
    /// the stale copy reads as behind it and not diverged again.
    #[test]
    fn a_copy_past_the_revision_cap_converges_after_one_join() {
        let dir = scratch("cap-revisions");
        let (mut l, _, local, archive) = backed_up(&dir);
        for i in 0..REVISIONS as u64 + 5 {
            if i % 50 == 0 {
                l.record(exposed(10.0 + i as f32));
            } else {
                l.meta.rating = (i % 5) as u8;
            }
            save(&mut l, &local, 200 + i, "desk");
        }
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(compare(&l, &a), Compared::Diverged);
        let mut j = join(&l, &a);
        assert_eq!(exposures(&j), exposures(&l));
        assert_eq!(j.current_label, None);
        assert!(j.absorbed.contains(&a.revisions.last().unwrap().hash));
        save(&mut j, &local, 1000, "desk");
        assert_eq!(compare(&j, &a), Compared::Behind(Side::Other));
        assert_eq!(compare(&j, &l), Compared::Behind(Side::Other));
        // And still after the list has turned over again.
        for i in 0..REVISIONS as u64 + 5 {
            save(&mut j, &local, 2000 + i, "desk");
        }
        assert_eq!(compare(&j, &a), Compared::Behind(Side::Other));
        let back = reread(&local);
        assert_eq!(back.absorbed, j.absorbed);
        assert_eq!(compare(&back, &a), Compared::Behind(Side::Other));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two machines joining the same pair in the same second each
    /// make the same sidecar, and once each has saved its own the two
    /// read as the same edit.
    #[test]
    fn two_machines_joining_in_the_same_second_agree() {
        let dir = scratch("tie");
        let (mut l, mut a, local, archive) = backed_up(&dir);
        l.record(exposed(2.0));
        save(&mut l, &local, 300, "desk");
        a.record(exposed(-1.0));
        a.meta.rating = 2;
        save(&mut a, &archive, 300, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        let (mut x, mut y) = (join(&l, &a), join(&a, &l));
        assert_eq!(x, y);
        save(&mut x, &local, 400, "desk");
        save(&mut y, &archive, 400, "laptop");
        assert_eq!(compare(&x, &y), Compared::SameEdit);
        let z = join(&x, &y);
        assert_eq!(exposures(&z), exposures(&x));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_time_line_round_trips_and_reads_loosely() {
        let set = Set {
            at: 300,
            rev: "abcd".into(),
            was: "0123".into(),
        };
        let line = serde_json::to_string(&set).unwrap();
        assert_eq!(line, r#""300 abcd 0123""#);
        assert_eq!(serde_json::from_str::<Set>(&line).unwrap(), set);
        let without_was = Set {
            was: String::new(),
            ..set.clone()
        };
        assert_eq!(
            serde_json::to_string(&without_was).unwrap(),
            r#""300 abcd""#
        );
        assert_eq!(
            serde_json::from_str::<Set>(r#""300 abcd""#).unwrap(),
            without_was
        );
        let bare = Set {
            at: 300,
            ..Set::default()
        };
        assert_eq!(serde_json::to_string(&bare).unwrap(), r#""300""#);
        assert_eq!(serde_json::from_str::<Set>("300").unwrap(), bare);
        assert_eq!(serde_json::from_str::<Set>(r#""300""#).unwrap(), bare);
        assert!(serde_json::from_str::<Set>(r#""soon""#).unwrap().is_unset());
        assert!(serde_json::from_str::<Set>("[1]").unwrap().is_unset());
        assert_eq!(serde_json::from_str::<Set>(r#""300 zz""#).unwrap(), bare);
    }

    /// A build from before §233 writes no times, so a copy it saved
    /// last says only, by its file's time, when something was last
    /// written. A field it changed to a new value is a change made
    /// then, and wins or loses by that time; a field it only carried
    /// across, still the value this build's stamp replaced, is not a
    /// change at all, whatever the file's time, and the stamped value
    /// wins. The one thing the files cannot tell apart is an older
    /// build changing a field back to exactly the value it had before
    /// this build set it, which reads as carrying the old value and
    /// loses; older builds are 0.3.x and earlier, and the limit goes
    /// as copies move to this build.
    #[test]
    fn an_older_builds_copy_carrying_the_old_value_is_not_a_change() {
        let dir = scratch("carry");
        // Case 2 of the review: the desk rates 3, copies, rates 4;
        // the laptop on an older build only edits, later.
        let (_, _, local, archive) = backed_up(&dir);
        let mut d = reread(&local);
        d.meta.rating = 3;
        save(&mut d, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        d.meta.rating = 4;
        save(&mut d, &local, 600, "desk");
        old_build_rewrite(&archive, 900, |v| add_state(v, 2.0));
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(a.meta.rating, 3, "carried, pinned at the file's time");
        assert_eq!(a.meta_at.rating.at, 900);
        let j = join(&l, &a);
        assert_eq!(j.meta.rating, 4);
        assert_eq!(join(&a, &l).meta.rating, 4);
        assert_eq!(exposures(&j), [0.0, 0.5, 1.0, 2.0], "the edit is kept");

        // Case 3: both copies rated 3 before the times; the desk rates
        // 5; the archive's file time is refreshed by a copy that kept
        // nothing else.
        let (_, _, local, archive) = backed_up(&dir);
        old_build_rewrite(&local, 100, |v| {
            v["meta"] = serde_json::json!({"rating": 3})
        });
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        d.meta.rating = 5;
        save(&mut d, &local, 500, "desk");
        old_build_rewrite(&archive, 2000, |_| {});
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(a.meta_at.rating.at, 2000);
        assert_eq!(join(&l, &a).meta.rating, 5);
        assert_eq!(join(&a, &l).meta.rating, 5);

        // A real change by the older build to a new value is a change
        // at its file's time: later than the stamp it wins, earlier it
        // loses (the rule of `a_change_by_an_older_build_is_as_old_as_its_file`).
        let (_, _, local, archive) = backed_up(&dir);
        let mut d = reread(&local);
        d.meta.rating = 3;
        save(&mut d, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        d.meta.rating = 4;
        save(&mut d, &local, 600, "desk");
        old_build_rewrite(&archive, 900, |v| {
            v["meta"]["rating"] = serde_json::json!(1)
        });
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(join(&l, &a).meta.rating, 1);
        old_build_rewrite(&archive, 500, |v| {
            v["meta"]["rating"] = serde_json::json!(1)
        });
        let a = reread(&archive);
        assert_eq!(join(&l, &a).meta.rating, 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A field an older build cleared is a change at its file's time
    /// like any other, though a cleared field is never pinned: the
    /// join gives it the time for its own purposes. Cleared back to
    /// exactly the value this build's stamp replaced, it reads as the
    /// old value carried, and loses: the limit named above.
    #[test]
    fn a_clear_by_an_older_build_is_a_change_unless_it_restores_the_old_value() {
        let dir = scratch("clear-old");
        // Rated 2 before this build rated 3; the older build clears:
        // a change, at 900, and it wins.
        let (_, _, local, archive) = backed_up(&dir);
        old_build_rewrite(&local, 50, |v| v["meta"] = serde_json::json!({"rating": 2}));
        let mut d = reread(&local);
        d.meta.rating = 3;
        save(&mut d, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        old_build_rewrite(&archive, 900, |v| v["meta"] = serde_json::json!({}));
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(a.meta.rating, 0);
        assert!(a.meta_at.rating.is_unset(), "a default is never pinned");
        assert_eq!(join(&l, &a).meta.rating, 0);
        assert_eq!(join(&a, &l).meta.rating, 0);
        // Cleared before the stamp: the stamp is later, and wins.
        old_build_rewrite(&archive, 80, |v| v["meta"] = serde_json::json!({}));
        let a = reread(&archive);
        assert_eq!(join(&l, &a).meta.rating, 3);
        // Unrated before this build rated 3 (case 1 of the review):
        // the clear restores exactly the value the stamp replaced, and
        // cannot be told from a copy that carried it. The 3 stands.
        let (_, _, local, archive) = backed_up(&dir);
        let mut d = reread(&local);
        d.meta.rating = 3;
        save(&mut d, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        old_build_rewrite(&archive, 900, |v| v["meta"] = serde_json::json!({}));
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(
            l.meta_at.rating.was,
            Field::Rating.value_hash(&Facts::default())
        );
        assert_eq!(join(&l, &a).meta.rating, 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A copy joined in turn with every other copy of a shoot, many
    /// saves between, keeps each of them behind by what it absorbed,
    /// long after the revision list has turned over.
    #[test]
    fn a_hub_joined_with_six_copies_keeps_each_behind() {
        let dir = scratch("hub");
        let hub = dir.join("hub").join("IMG_0001.CR3");
        std::fs::create_dir_all(hub.parent().unwrap()).unwrap();
        let mut h = Sidecar::default();
        h.record(exposed(1.0));
        save(&mut h, &hub, 100, "hub");
        let spokes: Vec<PathBuf> = (0..6)
            .map(|i| {
                let p = dir.join(format!("s{i}")).join("IMG_0001.CR3");
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::copy(Sidecar::path_for(&hub), Sidecar::path_for(&p)).unwrap();
                p
            })
            .collect();
        let mut t = 200;
        let mut edited = Vec::new();
        for (i, spoke) in spokes.iter().enumerate() {
            let mut s = reread(spoke);
            s.meta.rating = (i as u8 % 5) + 1;
            s.record(exposed(10.0 + i as f32));
            save(&mut s, spoke, t, &format!("s{i}"));
            edited.push(reread(spoke));
            t += 1;
            // Enough saves on the hub between joins to turn the
            // revision list over.
            let mut hh = reread(&hub);
            for k in 0..60 {
                hh.meta.flag = if k % 2 == 0 { Flag::Pick } else { Flag::None };
                save(&mut hh, &hub, t, "hub");
                t += 1;
            }
            let hs = reread(&hub);
            assert_eq!(compare(&hs, &s), Compared::Diverged);
            let mut j = join(&hs, &s);
            save(&mut j, &hub, t, "hub");
            t += 1;
        }
        let h = reread(&hub);
        assert_eq!(h.absorbed.len(), 12);
        for s in &edited {
            assert_eq!(compare(s, &h), Compared::Behind(Side::Local));
            assert_eq!(compare(&h, s), Compared::Behind(Side::Other));
            assert_eq!(join(&h, s).history.len(), h.history.len());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A pre-revision sidecar already at the cap, backed up; the desk
    /// on this build records several states with an export record on
    /// one; the archive's copy, on an older build, records one. That
    /// build drained a state before ids existed, so the ids this build
    /// fills on the archive's copy hash a shortened chain and the two
    /// copies share only their first state by id, each "own" branch
    /// being fifty states of the same content. Every state the desk
    /// made, and its record, and the archive's new state, survive the
    /// cap.
    #[test]
    fn a_capped_history_edited_by_an_older_build_loses_none_of_the_desks_states() {
        let dir = scratch("cap-old-build");
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..60 {
            s.record(exposed(100.0 + i as f32));
        }
        assert_eq!(s.history.len(), HISTORY);
        save(&mut s, &local, 100, "desk");
        old_build_rewrite(&local, 100, |_| {});
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        // The desk, on this build.
        let mut d = reread(&local);
        for i in 0..5 {
            d.record(exposed(200.0 + i as f32));
            if i == 2 {
                d.record_export(
                    &d.current.clone(),
                    Exported {
                        file: "/out/desk.jpg".into(),
                        preset: None,
                        at: 1,
                    },
                );
            }
        }
        save(&mut d, &local, 300, "desk");
        // The archive, on an older build: one state recorded, the
        // second state drained as that build's cap does.
        old_build_rewrite(&archive, 200, |v| {
            add_state(v, 300.0);
            let history = v["history"].as_array_mut().unwrap();
            if history.len() > HISTORY {
                history.remove(1);
            }
        });
        let (l, a) = (reread(&local), reread(&archive));
        assert_eq!(compare(&l, &a), Compared::Diverged);
        let j = join(&l, &a);
        let got = exposures(&j);
        for e in [200.0, 201.0, 202.0, 203.0, 204.0, 300.0] {
            assert!(got.contains(&e), "{e} missing from {got:?}");
        }
        assert_eq!(got[0], 0.0, "the first state stays");
        assert_eq!(j.history.len(), HISTORY);
        let records: Vec<&str> = j
            .history
            .iter()
            .flat_map(|h| h.exports.iter())
            .chain(j.current_exports.iter())
            .map(|e| e.file.as_str())
            .collect();
        assert_eq!(records, ["/out/desk.jpg"]);
        assert_eq!(join(&a, &l), j);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two long branches off one base, both on this build: the cap
    /// takes the shared states after the first, then the older
    /// branch's oldest, and the newer branch comes through whole with
    /// its records, the older copy's current one undo away.
    #[test]
    fn the_cap_drains_the_shared_states_and_the_older_branch_before_the_newer() {
        let dir = scratch("cap-order");
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..20 {
            s.record(exposed(0.01 * (i + 1) as f32));
        }
        save(&mut s, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        for i in 0..30 {
            d.record(exposed(1.0 + i as f32));
            if i == 15 {
                d.record_export(
                    &d.current.clone(),
                    Exported {
                        file: "/out/desk-mid.jpg".into(),
                        preset: None,
                        at: 1,
                    },
                );
            }
        }
        save(&mut d, &local, 300, "desk");
        let mut lp = reread(&archive);
        for i in 0..30 {
            lp.record(exposed(-1.0 - i as f32));
        }
        save(&mut lp, &archive, 200, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        let j = join(&l, &a);
        let got = exposures(&j);
        assert_eq!(j.history.len(), HISTORY);
        for i in 0..30 {
            assert!(got.contains(&(1.0 + i as f32)), "desk state {i} missing");
        }
        assert_eq!(got[0], 0.0);
        assert_eq!(*got.last().unwrap(), 30.0, "the desk's current, reconciled");
        assert_eq!(
            got[got.len() - 2],
            -30.0,
            "the laptop's current one undo away"
        );
        // Shared 20 and the laptop's oldest 11 went; its later 18 stay.
        assert!(
            !got.contains(&(-1.0)),
            "the laptop's oldest own states go first"
        );
        assert!(!got.contains(&(-11.0)));
        assert!(got.contains(&(-12.0)));
        assert!(!got.contains(&0.01));
        assert_eq!(
            j.history.iter().filter(|h| !h.exports.is_empty()).count(),
            1,
            "the record survives"
        );
        assert_eq!(join(&a, &l), j);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Nothing in the older copy's own branch is merged, whatever it
    /// looks like: a state that happens to be a shared state's edit
    /// again, with an export record on it, is a step of its own and
    /// keeps its record, and a step back and forth keeps its sequence.
    #[test]
    fn the_older_branchs_own_states_are_never_merged_by_content() {
        let dir = scratch("own-branch");
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..30 {
            s.record(exposed(0.01 * (i + 1) as f32));
        }
        save(&mut s, &local, 100, "desk");
        let state_8 = s.history[8].edit.clone();
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        for i in 0..25 {
            d.record(exposed(1.0 + i as f32));
        }
        save(&mut d, &local, 300, "desk");
        let mut lp = reread(&archive);
        lp.record(exposed(-1.0));
        lp.record(state_8.clone());
        lp.record_export(
            &state_8,
            Exported {
                file: "/out/laptop-e8.jpg".into(),
                preset: None,
                at: 1,
            },
        );
        lp.record(exposed(-3.0));
        save(&mut lp, &archive, 200, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        let j = join(&l, &a);
        let got = exposures(&j);
        assert_eq!(j.history.len(), HISTORY);
        let n = got.len();
        // The laptop's branch, whole and in order, under the desk's
        // reconciled current.
        assert_eq!(
            &got[n - 4..],
            &[-1.0, state_8.light.exposure, -3.0, 25.0],
            "{got:?}"
        );
        let records: Vec<&str> = j
            .history
            .iter()
            .flat_map(|h| h.exports.iter())
            .map(|e| e.file.as_str())
            .collect();
        assert_eq!(records, ["/out/laptop-e8.jpg"]);
        assert_eq!(
            j.history[n - 3].exports.len(),
            1,
            "on the laptop's own state"
        );
        assert_eq!(join(&a, &l), j);

        // A step back and forth on the older branch keeps its sequence.
        let (mut l, mut a, local, archive) = backed_up(&dir);
        l.record(exposed(5.0));
        save(&mut l, &local, 300, "desk");
        a.record(exposed(7.0));
        a.record(exposed(8.0));
        a.record(exposed(7.0));
        save(&mut a, &archive, 200, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        let j = join(&l, &a);
        assert_eq!(exposures(&j), [0.0, 0.5, 1.0, 5.0, 7.0, 8.0, 7.0, 5.0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A join saved and then compared with the same input again, the
    /// archive's write having failed: the states the join capped away
    /// are not brought back, nothing is recorded again, and the join
    /// is the join.
    #[test]
    fn rejoining_a_saved_join_with_its_input_changes_nothing() {
        let dir = scratch("rejoin");
        // Two long branches, so the first join capped shared states.
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..20 {
            s.record(exposed(0.01 * (i + 1) as f32));
        }
        save(&mut s, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        for i in 0..30 {
            d.record(exposed(1.0 + i as f32));
        }
        save(&mut d, &local, 300, "desk");
        // The archive's copy, a build from before revisions.
        old_build_rewrite(&archive, 200, |v| {
            for i in 0..10 {
                add_state(v, -1.0 - i as f32);
            }
        });
        let (l, a) = (reread(&local), reread(&archive));
        let mut j = join(&l, &a);
        save(&mut j, &local, 400, "desk");
        let j = reread(&local);
        assert_eq!(
            compare(&j, &a),
            Compared::Diverged,
            "the input has no revisions"
        );
        let again = join(&j, &a);
        assert_eq!(again.history, j.history);
        assert_eq!(again.current, j.current);
        assert_eq!(again.current_label, j.current_label);
        assert_eq!(again.current_id, j.current_id);
        assert_eq!(join(&a, &j).history, j.history);

        // The re-chained shape: the archive at the cap, edited by an
        // older build, the join saved, then compared with it again.
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..60 {
            s.record(exposed(100.0 + i as f32));
        }
        save(&mut s, &local, 100, "desk");
        old_build_rewrite(&local, 100, |_| {});
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        for i in 0..5 {
            d.record(exposed(200.0 + i as f32));
        }
        save(&mut d, &local, 300, "desk");
        old_build_rewrite(&archive, 200, |v| {
            add_state(v, 300.0);
            let history = v["history"].as_array_mut().unwrap();
            if history.len() > HISTORY {
                history.remove(1);
            }
        });
        let (l, a) = (reread(&local), reread(&archive));
        let mut j = join(&l, &a);
        assert_eq!(
            j.history
                .iter()
                .filter(|h| h
                    .label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Reconciled")))
                .count()
                + usize::from(
                    j.current_label
                        .as_deref()
                        .is_some_and(|l| l.starts_with("Reconciled"))
                ),
            1
        );
        save(&mut j, &local, 400, "desk");
        let j = reread(&local);
        let again = join(&j, &a);
        assert_eq!(again.history, j.history);
        assert_eq!(again.current, j.current);
        assert_eq!(again.current_label, j.current_label);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Three copies of one frame: the desk records three states with
    /// a record on the first; the laptop records one and writes it
    /// through to the archive; the desk joins with the archive and the
    /// join goes to both; the laptop, offline, records one more and
    /// then meets the join. A joined history is not one chain, so the
    /// desk's states, which the laptop's copy never had, stand between
    /// states the laptop does have by id; they are the join's own and
    /// come through with their record, from either side.
    #[test]
    fn a_joined_history_met_by_a_third_copy_keeps_the_states_between_shared_ones() {
        let dir = scratch("three-places");
        let (_, _, local, archive) = backed_up(&dir);
        let laptop = dir.join("laptop").join("IMG_0001.CR3");
        std::fs::create_dir_all(laptop.parent().unwrap()).unwrap();
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&laptop)).unwrap();
        let mut d = reread(&local);
        for i in 0..3 {
            d.record(exposed(1.1 + i as f32));
            if i == 0 {
                d.record_export(
                    &d.current.clone(),
                    Exported {
                        file: "/out/desk-n1.jpg".into(),
                        preset: None,
                        at: 1,
                    },
                );
            }
        }
        save(&mut d, &local, 300, "desk");
        let mut lp = reread(&laptop);
        lp.record(exposed(-0.9));
        save(&mut lp, &laptop, 200, "laptop");
        std::fs::copy(Sidecar::path_for(&laptop), Sidecar::path_for(&archive)).unwrap();
        let mut j1 = join(&reread(&local), &reread(&archive));
        save(&mut j1, &local, 400, "desk");
        j1.write_to(&Sidecar::path_for(&archive)).unwrap();
        let mut lp = reread(&laptop);
        lp.record(exposed(-1.9));
        save(&mut lp, &laptop, 500, "laptop");
        let (l, j1) = (reread(&laptop), reread(&archive));
        for (a, b) in [(&l, &j1), (&j1, &l)] {
            let j = join(a, b);
            let got = exposures(&j);
            for e in [1.1, 2.1, 3.1, -0.9, -1.9] {
                assert!(got.contains(&e), "{e} missing from {got:?}");
            }
            let records: Vec<&str> = j
                .history
                .iter()
                .flat_map(|h| h.exports.iter())
                .chain(j.current_exports.iter())
                .map(|e| e.file.as_str())
                .collect();
            assert_eq!(records, ["/out/desk-n1.jpg"]);
        }
        assert_eq!(join(&l, &j1), join(&j1, &l));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The older copy's first own state happens to be a shared state's
    /// edit again, with a record on it: a match at or behind the fork
    /// ends the common past rather than merging, so the state and its
    /// record survive the cap, with twenty-seven desk states and with
    /// twenty-nine.
    #[test]
    fn a_first_step_back_to_a_shared_edit_is_the_older_copys_own() {
        let dir = scratch("step-back");
        for desk_states in [27, 29] {
            let (_, _, local, archive) = backed_up(&dir);
            let mut s = reread(&local);
            for i in 0..30 {
                s.record(exposed(0.01 * (i + 1) as f32));
            }
            save(&mut s, &local, 100, "desk");
            let state_8 = s.history[8].edit.clone();
            std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
            let mut d = reread(&local);
            for i in 0..desk_states {
                d.record(exposed(1.0 + i as f32));
            }
            save(&mut d, &local, 300, "desk");
            let mut lp = reread(&archive);
            lp.record(state_8.clone());
            lp.record_export(
                &state_8,
                Exported {
                    file: "/out/laptop-e8.jpg".into(),
                    preset: None,
                    at: 1,
                },
            );
            lp.record(exposed(-3.0));
            save(&mut lp, &archive, 200, "laptop");
            let (l, a) = (reread(&local), reread(&archive));
            let j = join(&l, &a);
            assert_eq!(j.history.len(), HISTORY);
            let n = j.history.len();
            assert_eq!(j.history[n - 2].edit, state_8, "{desk_states}");
            assert_eq!(
                j.history[n - 2].exports[0].file,
                "/out/laptop-e8.jpg",
                "{desk_states}"
            );
            assert_eq!(j.history[n - 1].edit.light.exposure, -3.0);
            assert_eq!(join(&a, &l), j);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A state of the older copy's branch that happens to equal one of
    /// the newer copy's own states is not that state: the newer
    /// copy's branch stays its own, and the older's state is pushed as
    /// its own.
    #[test]
    fn a_coincidence_with_a_newer_own_state_does_not_move_the_fork() {
        let dir = scratch("coincidence");
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..20 {
            s.record(exposed(0.01 * (i + 1) as f32));
        }
        save(&mut s, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        for i in 0..5 {
            d.record(exposed(1.0 + i as f32));
        }
        save(&mut d, &local, 300, "desk");
        let mut lp = reread(&archive);
        lp.record(exposed(3.0));
        lp.record(exposed(-1.0));
        save(&mut lp, &archive, 200, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        let j = join(&l, &a);
        let got = exposures(&j);
        let n = got.len();
        assert_eq!(&got[n - 8..], &[1.0, 2.0, 3.0, 4.0, 5.0, 3.0, -1.0, 5.0]);
        assert_eq!(got.iter().filter(|e| **e == 3.0).count(), 2);
        // Under the cap the desk's branch is its own and goes last:
        // the same shape with the base long enough to need it.
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..45 {
            s.record(exposed(0.01 * (i + 1) as f32));
        }
        save(&mut s, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        let mut d = reread(&local);
        for i in 0..5 {
            d.record(exposed(1.0 + i as f32));
        }
        save(&mut d, &local, 300, "desk");
        let mut lp = reread(&archive);
        lp.record(exposed(3.0));
        lp.record(exposed(-1.0));
        save(&mut lp, &archive, 200, "laptop");
        let (l, a) = (reread(&local), reread(&archive));
        let j = join(&l, &a);
        let got = exposures(&j);
        let n = got.len();
        assert_eq!(j.history.len(), HISTORY);
        // The desk's 3.0 is the same look as the laptop's later 3.0,
        // so under the cap it is the first thing to go, the look still
        // there once; the rest of the desk's branch is whole.
        assert_eq!(
            &got[n - 7..],
            &[1.0, 2.0, 4.0, 5.0, 3.0, -1.0, 5.0],
            "{got:?}"
        );
        assert_eq!(got.iter().filter(|e| **e == 3.0).count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A reconciled entry from an earlier join, whose edit still stands
    /// elsewhere, goes to the cap before any state of a copy's own: the
    /// desk and the laptop join, the desk goes on twenty states, and a
    /// tablet's branch of thirty-five from the base is joined in. Every
    /// tablet state survives; the old reconciled repeat does not.
    #[test]
    fn an_old_reconciled_repeat_goes_before_a_copys_own_states() {
        let dir = scratch("old-repeat");
        let (_, _, local, archive) = backed_up(&dir);
        let tablet = dir.join("tablet").join("IMG_0001.CR3");
        std::fs::create_dir_all(tablet.parent().unwrap()).unwrap();
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&tablet)).unwrap();
        let mut desk = reread(&local);
        for i in 0..3 {
            desk.record(exposed(1.0 + i as f32));
        }
        save(&mut desk, &local, 200, "desk");
        let mut lap = reread(&archive);
        lap.record(exposed(-1.0));
        save(&mut lap, &archive, 150, "laptop");
        let mut j1 = join(&desk, &lap);
        save(&mut j1, &local, 300, "desk");
        assert!(j1.history.iter().any(is_reconciled) || is_reconciled_step(&j1));
        let mut d2 = reread(&local);
        for i in 0..20 {
            d2.record(exposed(10.0 + i as f32));
        }
        save(&mut d2, &local, 400, "desk");
        let mut tab = reread(&tablet);
        for i in 0..35 {
            tab.record(exposed(-100.0 - i as f32));
        }
        save(&mut tab, &tablet, 350, "tablet");
        let j2 = join(&d2, &tab);
        assert_eq!(j2.history.len(), HISTORY);
        let got = exposures(&j2);
        // The cap's order still takes the tablet's oldest own states
        // before the desk's; what the repeat going first buys is one
        // of them. The list is the desk's states, the tablet's
        // thirty-five and the reconciled current; of the excess, the
        // repeat goes, the two shared base states (0.5 and 1.0, which
        // `backed_up` records) go, and the rest comes off the tablet's
        // oldest.
        let excess = (d2.history.len() + 1 + 35 + 1) - (HISTORY + 1);
        let lost = (0..35)
            .filter(|i| !got.contains(&(-100.0 - *i as f32)))
            .count();
        assert_eq!(lost, excess - 3, "{got:?}");
        // The reconciled repeat of 3.0 is gone, 3.0 itself still there.
        assert!(
            !j2.history
                .iter()
                .any(|h| is_reconciled(h) && h.edit.light.exposure == 3.0)
        );
        assert!(got.contains(&3.0));
        assert_eq!(join(&tab, &d2), j2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn is_reconciled_step(s: &Sidecar) -> bool {
        s.current_label
            .as_deref()
            .is_some_and(|l| l.starts_with(RECONCILED))
    }

    /// History one copy capped away, confirmed by the ids to have led
    /// to a shared state, comes back from a copy that still has it when
    /// there is room: the laptop's branch of eleven stands between
    /// 29.89 and its reconciled repeat; the desk, which took the join,
    /// recorded past the cap and lost the branch to it, then undid
    /// below the cap; meeting the laptop's copy again, the join keeps
    /// the eleven, and a third pass changes nothing.
    #[test]
    fn capped_away_history_comes_back_when_there_is_room() {
        let dir = scratch("capped-away");
        let (_, _, local, archive) = backed_up(&dir);
        let mut lap = reread(&archive);
        lap.record(exposed(29.89));
        for i in 0..11 {
            lap.record(exposed(300.0 + i as f32));
        }
        save(&mut lap, &archive, 200, "laptop");
        let mut desk = reread(&local);
        desk.record(exposed(29.89));
        save(&mut desk, &local, 300, "desk");
        // The desk takes the join: the laptop's branch, then its own
        // current reconciled.
        let mut j1 = join(&desk, &lap);
        save(&mut j1, &local, 400, "desk");
        let branch = |s: &Sidecar| {
            (0..11)
                .filter(|i| exposures(s).contains(&(300.0 + *i as f32)))
                .count()
        };
        assert_eq!(branch(&j1), 11);
        // The desk goes on past the cap, which takes the branch, then
        // undoes below it.
        let mut d2 = reread(&local);
        for i in 0..70 {
            d2.record(exposed(500.0 + i as f32));
        }
        assert_eq!(branch(&d2), 0);
        for _ in 0..20 {
            d2.undo();
        }
        save(&mut d2, &local, 500, "desk");
        // The laptop's copy is as it was: no revision the desk knows
        // through its own list any more is needed for this, only the
        // ids.
        let mut lap2 = reread(&archive);
        lap2.revisions.clear();
        lap2.absorbed.clear();
        let j2 = join(&d2, &lap2);
        assert!(j2.history.len() < HISTORY);
        assert_eq!(branch(&j2), 11, "{:?}", exposures(&j2));
        assert_eq!(join(&lap2, &d2), j2);
        // Kept, they are shared states: a third pass adds nothing.
        let mut j2s = j2.clone();
        save(&mut j2s, &local, 600, "desk");
        let j3 = join(&j2s, &lap2);
        assert_eq!(exposures(&j3), exposures(&j2s));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The room for history brought back counts only the states this
    /// join will record again: the newer copy records forty-five past a
    /// ten-state base, so its cap takes 1..5, then undoes three (or
    /// five); the older copy has the base and a two-state branch whose
    /// current is the newer copy's current, so nothing is reconciled.
    /// Every capped-away state that has room comes back, the join ends
    /// at the cap, the restored states stand right after the first
    /// state in time order, and a later record's cap takes them first.
    #[test]
    fn restored_history_fills_the_room_the_join_really_has() {
        for undo in [3usize, 5] {
            let dir = scratch("capped-room");
            let (_, _, local, archive) = backed_up(&dir);
            let mut base = reread(&local);
            for i in 0..10 {
                base.record(exposed(1.0 + i as f32));
            }
            save(&mut base, &local, 100, "desk");
            std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
            let mut n = reread(&local);
            for i in 0..45 {
                n.record(exposed(100.0 + i as f32));
            }
            for _ in 0..undo {
                n.undo();
            }
            save(&mut n, &local, 300, "desk");
            let mut o = reread(&archive);
            o.record(exposed(-1.0));
            o.record(n.current.clone());
            save(&mut o, &archive, 200, "laptop");
            let (n, o) = (reread(&local), reread(&archive));
            let j = join(&n, &o);
            let got = exposures(&j);
            // The newer copy's cap took 1..=k of the base; with `undo`
            // states undone its history is 50 - undo + 1 long, and the
            // join has room for all but the ones that will not fit.
            let taken: Vec<f32> = (1..=10)
                .map(|i| i as f32)
                .filter(|x| !exposures(&n).contains(x))
                .collect();
            assert!(!taken.is_empty());
            // The older branch is two states and nothing is recorded
            // again, since its current is the newer copy's.
            let room = (HISTORY + 1) - (n.history.len() + 1 + 2);
            let back: Vec<f32> = taken.iter().copied().filter(|x| got.contains(x)).collect();
            assert_eq!(back.len(), taken.len().min(room), "undo {undo}: {got:?}");
            assert_eq!(j.history.len(), HISTORY, "undo {undo}");
            // Right after the first state, oldest first, before the base.
            assert_eq!(got[0], 0.0);
            assert_eq!(&got[1..1 + back.len()], &back[..], "undo {undo}: {got:?}");
            assert!(
                !j.history.iter().any(is_reconciled),
                "nothing recorded again"
            );
            assert_eq!(j.current, n.current);
            assert_eq!(join(&o, &n), j);
            // A later record's cap takes the restored states first.
            let mut k = j.clone();
            for i in 0..5 {
                k.record(exposed(900.0 + i as f32));
            }
            let kept = exposures(&k);
            // Of five slots the first record's cap takes the repeat (the
            // older copy's current, the same edit as the newer's) and
            // the rest come off the restored states, oldest first.
            let dropped: Vec<f32> = got.iter().copied().filter(|x| !kept.contains(x)).collect();
            assert_eq!(dropped, got[1..5].to_vec(), "undo {undo}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// A repeat in the middle of the history (a step back to an
    /// earlier look) that a record's cap took does not come back from a
    /// copy that still has it: its edit stands in the list already, so
    /// it is a repeat the cap would take again. The records on it go
    /// to that entry, and a second round changes nothing. A state of
    /// its own that the cap took still comes back, right before the
    /// state it led to.
    #[test]
    fn a_capped_mid_history_repeat_is_not_brought_back() {
        let dir = scratch("mid-repeat");
        let (_, _, local, archive) = backed_up(&dir);
        let mut s = reread(&local);
        for i in 0..6 {
            s.record(exposed(10.0 + i as f32));
        }
        // X, Y, X: the first X is a repeat; it carries a record.
        s.record(exposed(77.0));
        s.record_export(
            &s.current.clone(),
            Exported {
                file: "/out/on-x.jpg".into(),
                preset: None,
                at: 1,
            },
        );
        s.record(exposed(78.0));
        s.record(exposed(77.0));
        for i in 0..4 {
            s.record(exposed(20.0 + i as f32));
        }
        save(&mut s, &local, 100, "desk");
        std::fs::copy(Sidecar::path_for(&local), Sidecar::path_for(&archive)).unwrap();
        // The desk records past the cap: the record's cap takes the
        // first X first, then the oldest states (0.5, 1.0, 10.0, ...).
        let mut d = reread(&local);
        for i in 0..40 {
            d.record(exposed(100.0 + i as f32));
        }
        assert_eq!(exposures(&d).iter().filter(|x| **x == 77.0).count(), 1);
        assert!(!exposures(&d).contains(&10.0));
        for _ in 0..8 {
            d.undo();
        }
        save(&mut d, &local, 300, "desk");
        // The archive's copy still has both X and the oldest states.
        let mut a = reread(&archive);
        a.revisions.clear();
        a.absorbed.clear();
        // With no revisions its time is its file's; set earlier than
        // the desk's save, so the desk is the newer copy.
        a.modified = Some(200);
        let j = join(&d, &a);
        let got = exposures(&j);
        assert!(j.history.len() < HISTORY);
        assert_eq!(got.iter().filter(|x| **x == 77.0).count(), 1, "{got:?}");
        // The record on the dropped X is on the X that stands.
        let on_x: Vec<&str> = j
            .history
            .iter()
            .filter(|h| h.edit.light.exposure == 77.0)
            .flat_map(|h| h.exports.iter().map(|e| e.file.as_str()))
            .collect();
        assert_eq!(on_x, ["/out/on-x.jpg"]);
        // The states of their own that the cap took come back, right
        // before the state they led to, in time order.
        let i10 = got.iter().position(|x| *x == 10.0).unwrap();
        assert_eq!(got[i10 + 1], 11.0, "{got:?}");
        assert_eq!(got[0], 0.0);
        assert_eq!(join(&a, &d), j);
        // A second round changes nothing.
        let mut js = j.clone();
        save(&mut js, &local, 400, "desk");
        let j2 = join(&js, &a);
        assert_eq!(exposures(&j2), exposures(&js));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The states of a sidecar as (look, exposure, label) — what a
    /// state is, without its id — oldest first, the current last.
    fn looks(s: &Sidecar) -> Vec<(String, f32, Option<String>)> {
        states_of(s)
            .iter()
            .map(|t| {
                (
                    t.edit.look_lut.lut.name().to_string(),
                    t.edit.light.exposure,
                    t.label.clone(),
                )
            })
            .collect()
    }

    /// A look renamed on one copy and not on the other joins with no
    /// state twice and none lost, whichever side the join is asked
    /// from: the renamed states are new states (new ids made from their
    /// content), so the rule that checks a chain by its content never
    /// takes them for the other copy's, and the join keeps both.
    #[test]
    fn a_look_renamed_on_one_copy_joins_with_nothing_doubled_or_lost() {
        use crate::look::{LookLut, LutChoice};
        let dir = scratch("renamed-look");
        let raw = dir.join("IMG_0001.CR3");
        let named = |look: &str, stops: f32| Edit {
            look_lut: LookLut {
                lut: LutChoice::Named(look.into()),
                strength: 1.0,
            },
            ..exposed(stops)
        };
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        s.record(named("Neon", 0.5));
        s.record(named("Neon", 1.0));
        save(&mut s, &raw, 100, "desk");
        let plain = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
        let mut renamed = plain.clone();
        assert!(renamed.rename_look("Neon", "Glow"));
        save(&mut renamed, &raw, 200, "desk");
        // The states before the first renamed one are the same states.
        let (before, after) = (ids(&plain), ids(&renamed));
        assert_eq!(before[..2], after[..2]);
        assert!(after[2..].iter().all(|id| !before.contains(id)));
        let no_doubles = |j: &Sidecar| {
            let ids = ids(j);
            let unique: HashSet<&String> = ids.iter().collect();
            assert_eq!(unique.len(), ids.len(), "{:?}", looks(j));
        };

        // The copy that was not renamed is behind: the join is the
        // renamed copy, from either side.
        let j = join(&renamed, &plain);
        assert_eq!(j, join(&plain, &renamed));
        no_doubles(&j);
        assert_eq!(looks(&j), looks(&renamed));
        assert_eq!(j.current.look_lut.lut.name(), "Glow");

        // The copy that was not renamed went on, later, elsewhere: both
        // branches are kept, the shared past once, and every state of
        // either copy is in the join.
        let mut went_on = plain.clone();
        went_on.record(named("Neon", 1.5));
        let elsewhere = dir.join("elsewhere").join("IMG_0001.CR3");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        save(&mut went_on, &elsewhere, 300, "laptop");
        let j = join(&renamed, &went_on);
        assert_eq!(j, join(&went_on, &renamed));
        no_doubles(&j);
        let joined = looks(&j);
        for state in looks(&renamed).iter().chain(looks(&went_on).iter()) {
            assert!(
                joined
                    .iter()
                    .any(|(l, e, _)| (l, e) == (&state.0, &state.1)),
                "{state:?} lost from {joined:?}"
            );
        }
        let shared = joined
            .iter()
            .filter(|(l, e, _)| l == "none" && *e == 0.5)
            .count();
        assert_eq!(shared, 1, "{joined:?}");
        assert!(joined.iter().any(|(l, e, _)| l == "Glow" && *e == 1.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A state after the first renamed one that does not name the look
    /// takes a new id too, the ids being a chain: joined with a copy
    /// that was not renamed and went on, it stands twice, once under
    /// each id. Nothing is lost; the double is the price of the chain,
    /// and only a renamed history joined with an unrenamed one pays it.
    #[test]
    fn a_renamed_state_after_which_the_look_changed_joins_with_nothing_lost() {
        use crate::look::{LookLut, LutChoice};
        let dir = scratch("renamed-then-other");
        let raw = dir.join("IMG_0001.CR3");
        let named = |look: &str, stops: f32| Edit {
            look_lut: LookLut {
                lut: LutChoice::Named(look.into()),
                strength: 1.0,
            },
            ..exposed(stops)
        };
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        s.record(named("Neon", 1.0));
        s.record(named("Mono", 2.0));
        save(&mut s, &raw, 100, "desk");
        let plain = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
        let mut renamed = plain.clone();
        assert!(renamed.rename_look("Neon", "Glow"));
        save(&mut renamed, &raw, 200, "desk");
        let mut went_on = plain.clone();
        went_on.record(named("Mono", 3.0));
        let elsewhere = dir.join("elsewhere").join("IMG_0001.CR3");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        save(&mut went_on, &elsewhere, 300, "laptop");
        let j = join(&renamed, &went_on);
        assert_eq!(j, join(&went_on, &renamed));
        let joined = looks(&j);
        for state in looks(&renamed).iter().chain(looks(&went_on).iter()) {
            assert!(
                joined
                    .iter()
                    .any(|(l, e, _)| (l, e) == (&state.0, &state.1)),
                "{state:?} lost from {joined:?}"
            );
        }
        let ids = ids(&j);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        let mono_2 = joined
            .iter()
            .filter(|(l, e, _)| l == "Mono" && *e == 2.0)
            .count();
        assert_eq!(mono_2, 2, "{joined:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
