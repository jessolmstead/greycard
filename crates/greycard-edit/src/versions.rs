//! Versions of one frame: further lines of development kept in the
//! frame's one sidecar beside its own, each with a name, an edit, a
//! history, snapshots and export records of its own, and the file's
//! meta and turn shared.
//!
//! Version 0 is the file's own line, the sidecar's top level
//! (the sidecar's `line`), and has no id; it is the file. Every further
//! version is a [`Version`] in [`Sidecar::versions`], named by an id
//! made once at random and kept through a rename, a swap and a join.
//! A caller names a line by `Option<&str>`: `None` for version 0, the
//! version's id for the rest ([`fn@Sidecar::line`], [`Sidecar::line_mut`]).

use std::hash::{BuildHasher, Hasher};

use serde::{Deserialize, Serialize};

use crate::{Edit, Line, Sidecar, Written, history, meta, sync};

/// A version of the frame beside the file's own: a line of development
/// under a name.
///
/// Written as `{"id", "name", "made", "line": {...}}`, the line's keys
/// and shapes those of the sidecar's own (`current`, `step`,
/// `exported`, `last_export`, `id`, `history`, `snapshots`), nested
/// under `line` because its `id` is the current state's and the
/// version's own id is the version's.
// Read only as a sidecar's ([`VersionIn`]): no `Deserialize`, as a line
// has none (§724).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Version {
    /// Sixteen hex digits, random, made when the version is: not the
    /// hash of anything, since a name changes, the edit changes, and
    /// two machines making a version each must get two ids. Kept
    /// through a rename, a swap and a join.
    pub id: String,
    /// What the user called it ("Warm"), or "Version N" as made. A
    /// label, not a key: two versions may share one.
    #[serde(default, deserialize_with = "meta::loose")]
    pub name: String,
    /// When it was made, seconds since the Unix epoch: the order the
    /// list is kept in after a join.
    #[serde(default, deserialize_with = "meta::loose")]
    pub made: u64,
    /// When it was last renamed, seconds since the Unix epoch, none (0)
    /// while it has the name it was made with: of two copies' names, a
    /// join keeps the later (§724).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub named: u64,
    /// Its line: the edit, the history, the snapshots, the exports.
    pub line: Line,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A version as [`loose`] reads it: [`Version`], its line read as a
/// sidecar's ([`crate::line::read_line`]).
#[derive(Deserialize)]
struct VersionIn {
    id: String,
    #[serde(default, deserialize_with = "meta::loose")]
    name: String,
    #[serde(default, deserialize_with = "meta::loose")]
    made: u64,
    #[serde(default, deserialize_with = "meta::loose")]
    named: u64,
    #[serde(deserialize_with = "crate::line::read_line")]
    line: Line,
}

/// The versions under a key. A key that is not a list reads as none,
/// and an entry that is not a version at all (no id, an empty one, or
/// an id another version already has) is left out and said in the
/// log. A version with an id of its own that will not read refuses the
/// whole sidecar, as a fault in version 0's line does: left out, it
/// would be gone at the next save, and a sidecar that cannot be read is
/// never written over.
pub(crate) fn loose<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<Version>, D::Error> {
    let items = match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Null => return Ok(Vec::new()),
        _ => {
            log::warn!("sidecar: versions that are not a list are read as none");
            return Ok(Vec::new());
        }
    };
    let mut versions: Vec<Version> = Vec::with_capacity(items.len());
    for item in items {
        let id = match item.get("id").and_then(|i| i.as_str()) {
            Some(id) if !id.trim().is_empty() => id.to_string(),
            _ => {
                log::warn!("sidecar: a version with no id is left out");
                continue;
            }
        };
        if versions.iter().any(|o| o.id == id) {
            log::warn!("sidecar: a second version with the id {id} is left out");
            continue;
        }
        let VersionIn {
            id: read,
            name,
            made,
            named,
            line,
        } = VersionIn::deserialize(item)
            .map_err(|e| serde::de::Error::custom(format!("version {id}: {e}")))?;
        versions.push(Version {
            id: read,
            name,
            made,
            named,
            line,
        });
    }
    Ok(versions)
}
/// Sixteen hex digits at random: a hash keyed by the standard
/// library's per-process random keys, over a count and the clock, so
/// two made in one process, or on two machines in one second, differ.
pub(crate) fn random_id() -> String {
    #[cfg(test)]
    if let Some(n) = crate::testing::IDS.get() {
        crate::testing::IDS.set(Some(n + 1));
        return format!("{:016x}", n.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    }
    static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}

/// Now, seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    #[cfg(test)]
    if let Some(at) = crate::testing::CLOCK.get() {
        return at;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A line of a sidecar, to change ([`Sidecar::line_mut`]): what it records
/// is placed under the sidecar's turn as it stands at the call, never a
/// turn the line kept from before it was put on the sidecar (§724).
pub struct LineMut<'a> {
    line: &'a mut Line,
    turn: u8,
}

impl<'a> LineMut<'a> {
    /// [`Line::record`] under the sidecar's turn.
    pub fn record(&mut self, edit: Edit) -> bool {
        self.line.record(edit, self.turn)
    }

    /// [`Line::record_as`] under the sidecar's turn.
    pub fn record_as(&mut self, edit: Edit, label: Option<String>) -> bool {
        self.line.record_as(edit, label, self.turn)
    }

    /// The line, for as long as the sidecar is borrowed: for a change to
    /// something of it that is no state (a snapshot's name). What it
    /// records takes the turn by hand ([`Line::record`]).
    pub fn into_line(self) -> &'a mut Line {
        self.line
    }
}

impl std::ops::Deref for LineMut<'_> {
    type Target = Line;

    fn deref(&self) -> &Line {
        self.line
    }
}

impl std::ops::DerefMut for LineMut<'_> {
    fn deref_mut(&mut self) -> &mut Line {
        self.line
    }
}

impl Sidecar {
    /// The version `id`, when the sidecar has it.
    pub fn version(&self, id: &str) -> Option<&Version> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// The version `id`, to change, when the sidecar has it.
    pub fn version_mut(&mut self, id: &str) -> Option<&mut Version> {
        self.versions.iter_mut().find(|v| v.id == id)
    }

    /// The line of version `v`: the file's own for `None`, the
    /// version's for its id, and none for an id the sidecar does not
    /// have (deleted here, or on another machine).
    pub fn line(&self, v: Option<&str>) -> Option<&Line> {
        match v {
            None => Some(&self.line),
            Some(id) => self.version(id).map(|v| &v.line),
        }
    }

    /// [`fn@Self::line`], to change: shown under the sidecar's turn
    /// first ([`Self::show_if_stale`]), and recording under it
    /// ([`LineMut::record`]).
    pub fn line_mut(&mut self, v: Option<&str>) -> Option<LineMut<'_>> {
        self.show_if_stale();
        let turn = self.turn;
        let line = match v {
            None => &mut self.line,
            Some(id) => &mut self.version_mut(id)?.line,
        };
        Some(LineMut { line, turn })
    }

    /// A new version copying the current edit of version `from` (`None`
    /// for the file's own) as its first and only state, labeled "From
    /// <the source's name>" or "From the original", with no history,
    /// no snapshots and no export records: a history is fifty whole
    /// edits, and the new line's story starts here. Named `name`, or
    /// "Version N" when that is blank or none ([`Self::next_version_name`]).
    /// Its id, or none when `from` names a version the sidecar does
    /// not have.
    ///
    /// The state is copied as the file holds it, placed under the turn
    /// it was placed under (§724), with its shown copy: a copy places
    /// nothing again, so one that waits on the frame's shape waits in
    /// the new version too, and shows right once the shape is known.
    pub fn new_version(&mut self, from: Option<&str>, name: Option<&str>) -> Option<String> {
        self.show_if_stale();
        let (line, source) = match from {
            None => (&self.line, None),
            Some(id) => {
                let v = self.version(id)?;
                (&v.line, Some(v.name.clone()))
            }
        };
        let (edit, written) = (line.current.clone(), line.current_written.clone());
        let label = history::version_from_label(source.as_deref());
        // Made from a line nobody developed: one too (§724), its start
        // with the seed it took, if it took one. The sources chosen for
        // its patches go with it, so it shows them once read again.
        let (recorded, seeded) = (line.recorded, line.seeded);
        let sources = line.sources.clone();
        Some(self.push_version(edit, written, label, name, (recorded, seeded), sources))
    }

    /// A new version starting from `default_edit`, the edit a frame
    /// nobody has touched gets ([`Edit::for_picture`] for a picture
    /// file, the default with the raw's blend otherwise), labeled "From
    /// the defaults". Named as [`Self::new_version`] names; its id.
    /// `default_edit` is as shown under the sidecar's turn, and placed
    /// under it. None for a copy marked as waiting on the frame's shape
    /// ([`Edit::unturned`]): it would place its masks a quarter turn off
    /// (§724).
    pub fn new_version_from_original(
        &mut self,
        default_edit: Edit,
        name: Option<&str>,
    ) -> Option<String> {
        if default_edit.unturned {
            log::warn!("a version is not made from a state whose masks wait on the frame's shape");
            return None;
        }
        self.show_if_stale();
        let mut edit = default_edit;
        edit.placed_under = 0;
        let written = Written::at(self.turn % 4);
        // Nobody developed it (§724): no state until something needs
        // one, the start seeded on its own as the original's is.
        Some(self.push_version(
            edit,
            written,
            history::version_from_defaults_label(),
            name,
            (false, false),
            Default::default(),
        ))
    }

    fn push_version(
        &mut self,
        mut edit: Edit,
        written: Written,
        label: String,
        name: Option<&str>,
        (recorded, seeded): (bool, bool),
        sources: crate::line::Sources,
    ) -> String {
        let id = loop {
            let id = random_id();
            if self.version(&id).is_none() {
                break id;
            }
        };
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => name.to_string(),
            None => self.next_version_name(),
        };
        // A new version's first state is one the user makes: this
        // build's process, whatever the state it copies was made under.
        edit.process = crate::process_now();
        // A new state: its id made from it as written, as a recorded
        // state's is. A line nobody developed has none, and no label.
        let line = if recorded {
            let made = sync::state_id(sync::ZERO_ID, &written.edit(&edit), Some(&label));
            Line {
                current: edit,
                current_written: written,
                current_label: Some(label),
                current_id: Some(made),
                line_id: id.clone(),
                head_at: Some(crate::line::HeadAt::now()),
                recorded: true,
                sources,
                ..Line::default()
            }
        } else {
            Line {
                current: edit,
                current_written: written,
                seeded,
                sources,
                ..Line::default()
            }
        };
        self.versions.push(Version {
            id: id.clone(),
            name,
            made: now(),
            named: 0,
            line,
        });
        id
    }

    /// The name a new version takes when given none: "Version N", N
    /// the count of versions plus two, so the first made is "Version
    /// 2" (the original being the first), and on past any N a version
    /// already has, so a delete does not make the next name a twin.
    pub fn next_version_name(&self) -> String {
        let mut n = self.versions.len() + 2;
        loop {
            let name = format!("Version {n}");
            if !self.versions.iter().any(|v| v.name == name) {
                return name;
            }
            n += 1;
        }
    }

    /// Call version `id` `name`, trimmed; false when there is no such
    /// version, the name is blank, or it is the name already.
    pub fn rename_version(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        match self.version_mut(id) {
            Some(v) if !name.is_empty() && v.name != name => {
                v.name = name.to_string();
                v.named = now().max(v.made).max(v.named + 1);
                true
            }
            _ => false,
        }
    }

    /// Exchange version `id`'s line with the file's own, whole: the
    /// edit, the history, the snapshots, the export records, the last
    /// export and what was undone. The version keeps its id and its
    /// name, and nothing else in the file moves. False when there is
    /// no such version.
    ///
    /// Each line keeps its id ([`Line::line_id`]), so the swap moves no
    /// line's identity: it changes which line is the original, at a
    /// time ([`Self::original_at`]) a join compares (§724).
    pub fn swap_with_original(&mut self, id: &str) -> bool {
        self.fill_line_ids();
        match self.versions.iter_mut().find(|v| v.id == id) {
            Some(v) => {
                std::mem::swap(&mut self.line, &mut v.line);
                self.original_at = now().max(self.original_at + 1);
                true
            }
            None => false,
        }
    }

    /// Take version `id` out of the list; the version taken, or none
    /// when there is no such version. Not a state to undo: the editor
    /// asks first and saves.
    ///
    /// The delete leaves a tombstone ([`Sidecar::removed`]), carried
    /// through a join; one that keeps the version, because the other
    /// copy edited it (§243), drops it.
    pub fn delete_version(&mut self, id: &str) -> Option<Version> {
        let at = self.versions.iter().position(|v| v.id == id)?;
        let gone = self.versions.remove(at);
        self.removed.insert(gone.id.clone(), now());
        crate::line::cap_tombstones(&mut self.removed);
        Some(gone)
    }
}

#[cfg(test)]
mod join_matrix_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::{LookLut, LutChoice};
    use crate::{Error, Exported, HISTORY, VERSION, sync};

    fn exposed(stops: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = stops;
        e
    }

    fn is_id(id: &str) -> bool {
        id.len() == 16 && id.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// A delete leaves a tombstone, and a rename its time (§724): both
    /// written and read back, and a join carries every tombstone of
    /// either copy, at the later time.
    #[test]
    fn a_delete_s_tombstone_and_a_rename_s_time_are_written_and_joined() {
        let mut a = developed();
        let id = a.new_version(None, Some("Warm")).unwrap();
        let kept = a.new_version(None, Some("Kept")).unwrap();
        let b = a.clone();
        assert!(a.rename_version(&kept, "Cool"));
        let named = a.version(&kept).unwrap().named;
        assert!(named > 0);
        assert!(a.delete_version(&id).is_some());
        let at = a.removed[&id];
        let a = Sidecar::from_json(&a.to_json(), &Edit::default()).unwrap();
        assert_eq!(a.removed.get(&id), Some(&at), "written and read back");
        assert_eq!(a.version(&kept).unwrap().named, named);
        for joined in [sync::join(&a, &b), sync::join(&b, &a)] {
            // Kept by the join (§243), its tombstone dropped; or gone,
            // and its tombstone kept: never both.
            assert_ne!(
                joined.version(&id).is_some(),
                joined.removed.get(&id) == Some(&at)
            );
        }
        // Edited on the other copy: kept, and no tombstone for it.
        let mut b = b;
        b.line_mut(Some(&id)).unwrap().record(exposed(9.0));
        for joined in [sync::join(&a, &b), sync::join(&b, &a)] {
            assert!(joined.version(&id).is_some());
            assert!(!joined.removed.contains_key(&id));
        }
        // A snapshot removed here and kept there: kept, no tombstone.
        let mut here = developed();
        let there = here.clone();
        let gone = here.remove_snapshot(0).unwrap();
        assert!(here.snapshots_removed.contains_key(&gone.id));
        for joined in [sync::join(&here, &there), sync::join(&there, &here)] {
            assert!(joined.snapshots.iter().any(|s| s.id == gone.id));
            assert!(!joined.snapshots_removed.contains_key(&gone.id));
        }
        // The maps are capped, the oldest going first.
        let mut s = developed();
        for n in 0..(crate::line::TOMBSTONES as u64 + 10) {
            s.removed.insert(format!("{n:016x}"), n);
        }
        let id = s.new_version(None, None).unwrap();
        s.delete_version(&id);
        assert_eq!(s.removed.len(), crate::line::TOMBSTONES);
        assert!(s.removed.contains_key(&id) && !s.removed.contains_key(&format!("{:016x}", 0)));
    }

    /// A frame with a history, a snapshot and an export on its own line.
    fn developed() -> Sidecar {
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        s.take_snapshot("Snapshot 1", 1_700_000_000);
        s.record_as(exposed(1.0), Some("Preset: Faded film".into()));
        s.record_export(
            &exposed(1.0),
            Exported {
                file: "/out/IMG_0001.jpg".into(),
                preset: None,
                at: 1_700_000_100,
            },
        );
        s
    }

    /// Written and read back as the editor reads it, short of the file.
    /// `s` through its file, a picture's: a line nobody developed has
    /// a picture's start.
    fn reread(s: &Sidecar) -> Sidecar {
        let mut back = Sidecar::from_json(&s.to_json(), &crate::Edit::default()).unwrap();
        back.set_start(&Edit::for_picture());
        back
    }

    /// The highlight fill rides with the whole edit wherever one is
    /// kept: a step of the history and its undo and redo, a snapshot,
    /// a version made from the frame, and the sidecar's file, a name
    /// this build lacks included.
    #[test]
    fn the_highlight_fill_goes_wherever_a_whole_edit_goes() {
        let filled = |name: &str| {
            let mut e = exposed(0.5);
            e.highlight_fill = Some(name.into());
            e
        };
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        assert!(s.record(filled("hl-1")), "the fill alone is a step");
        s.take_snapshot("Filled", 1_700_000_000);
        s.record(filled("hl-9"));
        assert!(s.undo());
        assert_eq!(s.line.current, filled("hl-1"));
        assert!(s.undo());
        assert_eq!(s.line.current.highlight_fill, None);
        assert!(s.redo() && s.redo());
        assert_eq!(s.line.current, filled("hl-9"));
        assert!(s.restore_snapshot(0));
        assert_eq!(s.line.current, filled("hl-1"));
        let id = s.new_version(None, None).unwrap();
        assert_eq!(s.version(&id).unwrap().line.current, filled("hl-1"));
        let read = reread(&s);
        assert_eq!(read.line, s.line);
        assert_eq!(read.versions, s.versions);
        assert_eq!(read.line.snapshots[0].edit, filled("hl-1"));
    }

    #[test]
    fn a_new_version_copies_the_sources_current_edit_and_nothing_else() {
        let mut s = developed();
        let id = s.new_version(None, None).unwrap();
        assert!(is_id(&id));
        let v = s.version(&id).unwrap();
        assert_eq!(v.name, "Version 2");
        assert!(v.made > 0);
        let line = &v.line;
        assert_eq!(line.current, exposed(1.0));
        assert_eq!(line.current_label.as_deref(), Some("From the original"));
        assert!(line.history.is_empty() && line.redo.is_empty());
        assert!(line.snapshots.is_empty());
        assert!(line.current_exports.is_empty() && line.last_export.is_none());
        assert_eq!(
            line.current_id.as_deref(),
            Some(sync::state_id(sync::ZERO_ID, &exposed(1.0), Some("From the original")).as_str())
        );
        // The original is as it was.
        assert_eq!(s.line, developed().line);

        // A copy of a version names it, and takes its current edit.
        s.line_mut(Some(&id)).unwrap().record(exposed(2.0));
        let warm = s.new_version(Some(&id), Some("  Warm ")).unwrap();
        let w = s.version(&warm).unwrap();
        assert_eq!(w.name, "Warm");
        assert_eq!(w.line.current, exposed(2.0));
        assert_eq!(w.line.current_label.as_deref(), Some("From Version 2"));
        assert_ne!(warm, id);
        // A source that is not there makes nothing.
        assert_eq!(s.new_version(Some("0000000000000000"), None), None);
        assert_eq!(s.versions.len(), 2);
    }

    #[test]
    fn a_new_version_from_the_original_starts_from_the_edit_it_is_given() {
        let mut s = developed();
        let id = s
            .new_version_from_original(Edit::for_picture(), Some("Fresh"))
            .unwrap();
        let v = s.version(&id).unwrap();
        assert_eq!(v.name, "Fresh");
        assert_eq!(v.line.current, Edit::for_picture());
        // Nobody developed it (§724): no state, no id, until a step.
        assert!(!v.line.recorded());
        assert!(v.line.history.is_empty() && v.line.snapshots.is_empty());
        assert!(v.line.current_id.is_none());
    }

    #[test]
    fn the_default_name_counts_from_two_and_steps_past_a_name_taken() {
        let mut s = Sidecar::default();
        let a = s.new_version(None, None).unwrap();
        let b = s.new_version(None, Some("")).unwrap();
        assert_eq!(s.version(&a).unwrap().name, "Version 2");
        assert_eq!(s.version(&b).unwrap().name, "Version 3");
        // With Version 2 gone the count says 3, which is taken.
        s.delete_version(&a).unwrap();
        let c = s.new_version(None, None).unwrap();
        assert_eq!(s.version(&c).unwrap().name, "Version 4");
    }

    #[test]
    fn a_rename_keeps_the_id_and_refuses_a_blank_name() {
        let mut s = developed();
        let id = s.new_version(None, None).unwrap();
        assert!(s.rename_version(&id, " Warm "));
        assert_eq!(s.version(&id).unwrap().name, "Warm");
        assert!(!s.rename_version(&id, "Warm"));
        assert!(!s.rename_version(&id, "   "));
        assert!(!s.rename_version("ffffffffffffffff", "Cool"));
        assert_eq!(s.version(&id).unwrap().name, "Warm");
    }

    /// `json` as a build at schema 9 wrote it: no floors, no line ids,
    /// no swap time, every state at 9.
    fn at_nine(json: &str) -> String {
        fn walk(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(o) => {
                    for k in [
                        "floors",
                        "line_id",
                        "original_at",
                        "placed_under",
                        "process",
                    ] {
                        o.remove(k);
                    }
                    if o.get("version") == Some(&serde_json::json!(VERSION)) {
                        o.insert("version".into(), serde_json::json!(9));
                    }
                    o.values_mut().for_each(walk);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
                _ => {}
            }
        }
        let mut v: serde_json::Value = serde_json::from_str(json).unwrap();
        walk(&mut v);
        v.to_string()
    }

    /// A swap on one copy (§724): the join's original is the one the
    /// copy that swapped later names, and each line is joined with the
    /// line of its id on the other copy, whichever way round the join
    /// goes. So too for a schema 9 file two copies migrated apart, one
    /// of them swapping: both derive each line's id alike (the
    /// original's from its root state, a version's from its id).
    #[test]
    fn a_swap_on_one_copy_joins_each_line_with_its_own() {
        for nine in [false, true] {
            let mut base = developed();
            let id = base.new_version(None, Some("Warm")).unwrap();
            base.line_mut(Some(&id)).unwrap().record(exposed(3.0));
            base.fill_ids();
            let text = if nine {
                at_nine(&base.to_json())
            } else {
                base.to_json()
            };
            let read = || Sidecar::from_json(&text, &Edit::default()).unwrap();
            let (mut here, mut there) = (read(), read());
            assert!(here.swap_with_original(&id));
            assert!(here.original_at > 0);
            // Here: Warm is the original, the old original a version.
            here.record(exposed(5.0));
            here.line_mut(Some(&id)).unwrap().record(exposed(6.0));
            // There, unswapped.
            there.record(exposed(7.0));
            there.line_mut(Some(&id)).unwrap().record(exposed(8.0));
            let has = |l: &Line, x: f32| {
                l.history
                    .iter()
                    .map(|h| &h.edit)
                    .chain([&l.current])
                    .any(|e| e.light.exposure == x)
            };
            for j in [sync::join(&here, &there), sync::join(&there, &here)] {
                let v = &j.version(&id).unwrap().line;
                // Warm's line, made from the original at 1.0: 3, 5 and 8.
                for x in [3.0, 5.0, 8.0] {
                    assert!(has(&j.line, x), "nine {nine}: the original lost {x}");
                    assert!(!has(v, x), "nine {nine}: {x} in the version");
                }
                for x in [0.5, 1.0, 6.0, 7.0] {
                    assert!(has(v, x), "nine {nine}: the version lost {x}");
                }
                for x in [0.5, 6.0, 7.0] {
                    assert!(!has(&j.line, x), "nine {nine}: {x} in the original");
                }
                assert_eq!(j.original_at, here.original_at);
                assert_ne!(j.line.line_id, v.line_id);
            }
        }
    }

    #[test]
    fn a_swap_exchanges_the_lines_whole_and_the_version_keeps_its_id_and_name() {
        let mut s = developed();
        s.meta.rating = 4;
        s.turn = 1;
        let id = s.new_version(None, Some("Warm")).unwrap();
        s.line_mut(Some(&id)).unwrap().record(exposed(3.0));
        s.line_mut(Some(&id)).unwrap().record(exposed(4.0));
        s.line_mut(Some(&id)).unwrap().undo();
        let (original, version) = (s.line.clone(), s.version(&id).unwrap().line.clone());
        assert!(s.swap_with_original(&id));
        assert_eq!(s.line, version);
        assert_eq!(s.current, exposed(3.0));
        assert_eq!(s.redo.len(), 1);
        let v = s.version(&id).unwrap();
        assert_eq!((v.id.as_str(), v.name.as_str()), (id.as_str(), "Warm"));
        assert_eq!(v.line, original);
        assert_eq!(v.line.last_export, original.last_export);
        // The file's own facts do not move.
        assert_eq!((s.meta.rating, s.turn), (4, 1));
        // Swapped back, it is as it was.
        assert!(s.swap_with_original(&id));
        assert_eq!(s.line, original);
        assert!(!s.swap_with_original("ffffffffffffffff"));
    }

    #[test]
    fn a_delete_takes_the_version_alone() {
        let mut s = developed();
        let a = s.new_version(None, Some("A")).unwrap();
        let b = s.new_version(None, Some("B")).unwrap();
        let gone = s.delete_version(&a).unwrap();
        assert_eq!(gone.name, "A");
        assert_eq!(s.versions.len(), 1);
        assert_eq!(s.versions[0].id, b);
        assert!(s.delete_version(&a).is_none());
        assert_eq!(s.line, developed().line);
    }

    #[test]
    fn a_line_is_named_by_its_versions_id_and_walks_its_own_history() {
        let mut s = developed();
        let id = s.new_version(None, None).unwrap();
        assert_eq!(s.line(None).unwrap().current, exposed(1.0));
        assert!(s.line(Some("ffffffffffffffff")).is_none());
        assert!(s.line_mut(Some("ffffffffffffffff")).is_none());
        let mut line = s.line_mut(Some(&id)).unwrap();
        assert!(line.record(exposed(2.0)));
        assert!(line.record_as(exposed(2.5), Some("Preset: Night".into())));
        line.take_snapshot("Night", 1_700_000_200);
        assert!(line.record_export(
            &exposed(2.5),
            Exported {
                file: "/out/IMG_0001-Version 2.jpg".into(),
                preset: None,
                at: 1_700_000_300,
            },
        ));
        assert!(line.undo());
        assert_eq!(line.current, exposed(2.0));
        assert!(line.go_to(2));
        assert_eq!(line.current, exposed(2.5));
        assert_eq!(line.describe(2).as_deref(), Some("Preset: Night"));
        assert!(line.undo());
        assert!(line.restore_snapshot(0));
        assert_eq!(line.current, exposed(2.5));
        assert_eq!(line.current_label.as_deref(), Some("Snapshot: Night"));
        assert_eq!(line.states(), 3);
        // Version 0 saw none of it.
        assert_eq!(s.line, developed().line);
        // Each line's history has its own cap.
        let mut line = s.line_mut(Some(&id)).unwrap();
        for i in 0..HISTORY + 10 {
            line.record(exposed(10.0 + i as f32));
        }
        assert_eq!(line.history.len(), HISTORY);
        assert_eq!(s.history.len(), 2);
    }

    #[test]
    fn a_sidecar_with_versions_round_trips() {
        let mut s = developed();
        let before_ids = sync::state_ids(&s);
        let id = s.new_version(None, Some("Warm")).unwrap();
        s.line_mut(Some(&id)).unwrap().record(exposed(2.0));
        s.line_mut(Some(&id))
            .unwrap()
            .take_snapshot("Kept", 1_700_000_400);
        // Making a version moves no id of the original's.
        assert_eq!(sync::state_ids(&s), before_ids);
        let json = s.to_json();
        assert!(json.contains("\"versions\": ["));
        let back = reread(&s);
        assert_eq!(back, s);
        // Read by serde alone too, not through the migration.
        let plain: Sidecar = crate::sidecar::plain(&json).unwrap();
        assert_eq!(plain, s);
        assert_eq!(plain.to_json(), json);
        // The version's line keeps its keys, nested under `line`.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let v = &value["versions"][0];
        assert_eq!(v["id"], id.as_str());
        assert_eq!(v["name"], "Warm");
        for key in ["current", "id", "history", "snapshots"] {
            assert!(v["line"].get(key).is_some(), "{key}");
        }
        assert_eq!(v["line"]["history"][0]["step"], "From the original");
        // With the last version deleted it writes what it wrote before,
        // and the delete's tombstone (§724).
        s.delete_version(&id).unwrap();
        let mut before = developed();
        // The head's time is the clock's, and a second may have turned.
        before.line.head_at = s.line.head_at.clone();
        before.removed = s.removed.clone();
        assert!(before.removed.contains_key(&id));
        assert_eq!(s.to_json(), before.to_json());
    }

    #[test]
    fn every_edit_is_written_at_five_so_a_build_before_versions_refuses_it() {
        let mut s = developed();
        s.new_version(None, Some("Warm")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        // A build before versions refuses an edit past its 4, and its
        // read migrates version 0's current first.
        assert_eq!(value["current"]["version"], VERSION);
        const { assert!(VERSION >= 5) };
        for h in value["history"].as_array().unwrap() {
            assert_eq!(h["version"], VERSION);
        }
        for snap in value["snapshots"].as_array().unwrap() {
            assert_eq!(snap["edit"]["version"], VERSION);
        }
        assert_eq!(value["versions"][0]["line"]["current"]["version"], VERSION);
        // A sidecar without versions is written at it all the same.
        let value: serde_json::Value = serde_json::from_str(&developed().to_json()).unwrap();
        assert_eq!(value["current"]["version"], VERSION);
    }

    #[test]
    fn an_edit_waiting_on_the_frame_keeps_its_mark() {
        let mut waiting = Edit {
            version: 3,
            ..Edit::default()
        };
        waiting.geometry.aspect = crate::geometry::Aspect::Original;
        let mut s = Sidecar {
            line: Line::of(waiting, 0).unwrap(),
            ..Sidecar::default()
        };
        s.record(exposed(1.0));
        s.new_version(None, None).unwrap();
        let back = reread(&s);
        assert!(back.history[0].edit.frame_pending);
        assert!(back.needs_frame());
        // Read and written back, it keeps the mark at this build's
        // version, and reads the same again.
        let value: serde_json::Value = serde_json::from_str(&back.to_json()).unwrap();
        assert_eq!(value["history"][0]["version"], VERSION);
        assert_eq!(value["history"][0]["frame_pending"], true);
        assert_eq!(value["current"]["version"], VERSION);
        assert!(value["current"].get("frame_pending").is_none());
        assert_eq!(reread(&back), back);
        // A version-4 edit is not waiting on anything.
        let four = Edit {
            version: 4,
            ..Edit::default()
        };
        assert!(!four.needs_frame());
    }

    #[test]
    fn a_version_from_a_newer_schema_refuses_the_whole_sidecar() {
        let mut s = developed();
        s.new_version(None, Some("Warm")).unwrap();
        let mut future: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        future["versions"][0]["line"]["current"]["version"] = (VERSION + 1).into();
        let err = Sidecar::from_json(&future.to_string(), &crate::Edit::default()).unwrap_err();
        assert!(matches!(err, Error::Newer(v) if v == VERSION + 1));
        assert_eq!(
            err.to_string(),
            format!(
                "sidecar is schema version {}, this build reads up to {VERSION}",
                VERSION + 1
            )
        );
    }

    #[test]
    fn the_file_read_refuses_a_future_version_rather_than_dropping_it() {
        let dir = std::env::temp_dir().join(format!("gc-versions-future-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0001.CR3");
        let mut s = developed();
        s.new_version(None, None).unwrap();
        s.save(&raw).unwrap();
        let path = Sidecar::path_for(&raw);
        let back = Sidecar::read_as_file(&path).unwrap();
        assert_eq!(back.versions, s.versions);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["versions"][0]["line"]["history"] = serde_json::json!([{ "version": VERSION + 1 }]);
        std::fs::write(&path, value.to_string()).unwrap();
        let newer = |r: crate::Result<_>| matches!(r, Err(Error::Newer(v)) if v == VERSION + 1);
        assert!(newer(Sidecar::read_as_file(&path).map(|_| ())));
        assert!(newer(Sidecar::load_as_file(&raw).map(|_| ())));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn junk_in_the_versions_is_left_out_and_a_version_that_will_not_read_refuses() {
        let mut s = developed();
        let good = s.new_version(None, Some("Good")).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        let list = value["versions"].as_array_mut().unwrap();
        let copy = list[0].clone();
        // Not versions at all: no id, an empty one, a repeated one.
        list.push(serde_json::json!({ "name": "No id", "line": copy["line"].clone() }));
        list.push(serde_json::json!({ "id": "", "line": copy["line"].clone() }));
        list.push(serde_json::json!({ "id": 7, "line": copy["line"].clone() }));
        list.push(copy.clone());
        // A name or a time that is not one reads as none, the version kept.
        let mut odd = copy.clone();
        odd["id"] = "3333333333333333".into();
        odd["name"] = 7.into();
        odd["made"] = "yesterday".into();
        list.push(odd);
        let back = Sidecar::from_json(&value.to_string(), &crate::Edit::default()).unwrap();
        let ids: Vec<&str> = back.versions.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, [good.as_str(), "3333333333333333"]);
        assert_eq!(back.versions[1].name, "");
        assert_eq!(back.versions[1].made, 0);
        assert_eq!(back.line, s.line);

        // A version of its own that will not read is not dropped: the
        // sidecar is refused, as a fault in version 0 refuses it.
        let refused = |broken: serde_json::Value| {
            let mut v = value.clone();
            v["versions"].as_array_mut().unwrap().push(broken);
            Sidecar::from_json(&v.to_string(), &crate::Edit::default()).unwrap_err()
        };
        let mut bad_step = copy.clone();
        bad_step["id"] = "1111111111111111".into();
        bad_step["line"]["current"]["light"] = serde_json::json!({ "exposure": "x" });
        let err = refused(bad_step);
        assert!(err.to_string().contains("1111111111111111"), "{err}");
        refused(serde_json::json!({ "id": "2222222222222222", "line": "not a line" }));
        refused(serde_json::json!({ "id": "4444444444444444" }));
        // As in version 0.
        let mut zero = value.clone();
        zero["history"][0]["light"] = serde_json::json!({ "exposure": "x" });
        assert!(Sidecar::from_json(&zero.to_string(), &crate::Edit::default()).is_err());

        // A list that is not a list is none, and the rest reads.
        value["versions"] = serde_json::json!({ "id": good });
        let back = Sidecar::from_json(&value.to_string(), &crate::Edit::default()).unwrap();
        assert!(back.versions.is_empty());
        assert_eq!(back.line, s.line);
        // So is a null.
        value["versions"] = serde_json::Value::Null;
        let back: Sidecar = crate::sidecar::plain(&value.to_string()).unwrap();
        assert!(back.versions.is_empty());
        // No versions and no line's keys at all: the defaults.
        let empty: Sidecar = crate::sidecar::plain("{}").unwrap();
        assert_eq!(empty, Sidecar::default());
        let bare: Sidecar = crate::sidecar::plain(r#"{"meta": {"rating": 2}}"#).unwrap();
        assert_eq!(bare.line, Line::default());
        assert_eq!(bare.meta.rating, 2);
    }

    #[test]
    fn a_turn_a_rename_and_the_frames_shape_reach_every_version() {
        let named = |n: &str| LookLut {
            lut: LutChoice::Named(n.into()),
            strength: 0.6,
        };
        let mut s = Sidecar::default();
        s.record(Edit {
            look_lut: named("Neon"),
            ..Edit::default()
        });
        let id = s.new_version(None, None).unwrap();
        assert!(s.names_look("Neon"));
        assert!(s.rename_look("Neon", "Glow"));
        assert!(!s.names_look("Neon"));
        assert_eq!(s.version(&id).unwrap().line.current.look_lut, named("Glow"));
        // The renamed version's ids are chained again from its content.
        let line = &s.version(&id).unwrap().line;
        assert_eq!(
            line.current_id.as_deref(),
            Some(
                sync::state_id(sync::ZERO_ID, &line.current, line.current_label.as_deref())
                    .as_str()
            )
        );
        assert_eq!(s.looks_named().into_iter().collect::<Vec<_>>(), ["Glow"]);

        let mut turned = s.version(&id).unwrap().line.current.clone();
        turned.turn(1, None);
        s.turn_by(1, Some(1.5));
        assert_eq!(s.turn, 1);
        assert_eq!(s.version(&id).unwrap().line.current, turned);
        assert_eq!(s.current, turned);

        let mut waiting = Edit {
            version: 3,
            ..Edit::default()
        };
        waiting.geometry.aspect = crate::geometry::Aspect::Original;
        s.line_mut(Some(&id)).unwrap().record(waiting);
        assert!(s.needs_frame());
        s.frame_known(1.5);
        assert!(!s.needs_frame());
    }

    /// A small generator of its own, so the walk below is the same on
    /// every run.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    #[test]
    fn makers_at_random_keep_every_line_and_round_trip() {
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
            let mut s = developed();
            for step in 0..60 {
                let ids: Vec<String> = s.versions.iter().map(|v| v.id.clone()).collect();
                let pick = |rng: &mut Rng| -> Option<String> {
                    match rng.below(ids.len() + 1) {
                        0 => None,
                        i => Some(ids[i - 1].clone()),
                    }
                };
                let before = s.clone();
                match rng.below(7) {
                    0 => {
                        let from = pick(&mut rng);
                        let id = s.new_version(from.as_deref(), None).unwrap();
                        let source = before.line(from.as_deref()).unwrap();
                        assert_eq!(s.version(&id).unwrap().line.current, source.current);
                    }
                    1 => {
                        s.new_version_from_original(Edit::for_picture(), Some("Fresh"))
                            .unwrap();
                    }
                    2 => {
                        if let Some(id) = pick(&mut rng) {
                            s.rename_version(&id, &format!("Name {step}"));
                        }
                    }
                    3 => {
                        if let Some(id) = pick(&mut rng) {
                            assert!(s.swap_with_original(&id));
                            assert_eq!(s.line, before.version(&id).unwrap().line);
                            assert_eq!(s.version(&id).unwrap().line, before.line);
                        }
                    }
                    4 => {
                        if let Some(id) = pick(&mut rng) {
                            s.delete_version(&id).unwrap();
                            assert!(s.version(&id).is_none());
                        }
                    }
                    5 => {
                        let v = pick(&mut rng);
                        s.line_mut(v.as_deref())
                            .unwrap()
                            .record(exposed(step as f32 / 8.0));
                    }
                    _ => {
                        let v = pick(&mut rng);
                        s.line_mut(v.as_deref()).unwrap().undo();
                    }
                }
                // Every id is sixteen hex digits and its own.
                let mut seen = std::collections::HashSet::new();
                assert!(
                    s.versions
                        .iter()
                        .all(|v| is_id(&v.id) && seen.insert(&v.id))
                );
                // The redo stack is written as undone (§724).
                let back = reread(&s);
                assert_eq!(back, s, "seed {seed} step {step}");
            }
        }
    }

    /// A file holding one line id twice (found by fuzzing a join; no
    /// build writes one) has the later line's made anew, so a join
    /// places each line once and keeps both edits.
    #[test]
    fn a_line_id_held_twice_is_made_anew() {
        let json = r#"{"current":{},"line_id":"9e3779b97f4a7c15",
            "versions":[
                {"id":"9e3779b97f4a7c15","line":{"current":{"light":{"exposure":0.5}}}},
                {"id":"3c6ef372fe94f82a","line":{"current":{"light":{"exposure":0.7}},
                    "line_id":"9e3779b97f4a7c15"}}]}"#;
        let mut s = Sidecar::from_json(json, &Edit::default()).unwrap();
        s.fill_ids();
        let ids: std::collections::HashSet<&str> = std::iter::once(&s.line)
            .chain(s.versions.iter().map(|v| &v.line))
            .map(|l| l.line_id.as_str())
            .collect();
        assert_eq!(ids.len(), 3, "{ids:?}");
        let again = reread(&s);
        for joined in [sync::join(&s, &again), sync::join(&again, &s)] {
            assert_eq!(joined.versions.len(), 2);
            for (id, exposure) in [("9e3779b97f4a7c15", 0.5), ("3c6ef372fe94f82a", 0.7)] {
                let line = joined.line(Some(id)).unwrap();
                assert_eq!(line.current().light.exposure, exposure);
            }
        }
    }
}
