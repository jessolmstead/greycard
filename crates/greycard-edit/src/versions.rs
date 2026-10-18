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

use crate::{Edit, Line, Sidecar, history, meta};

/// A version of the frame beside the file's own: a line of development
/// under a name.
///
/// Written as `{"id", "name", "made", "line": {...}}`, the line's keys
/// and shapes those of the sidecar's own (`current`, `step`,
/// `exported`, `last_export`, `id`, `history`, `snapshots`), nested
/// under `line` because its `id` is the current state's and the
/// version's own id is the version's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Its line: the edit, the history, the snapshots, the exports.
    pub line: Line,
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
        let version = serde_json::from_value::<Version>(item)
            .map_err(|e| serde::de::Error::custom(format!("version {id}: {e}")))?;
        versions.push(version);
    }
    Ok(versions)
}
/// Sixteen hex digits at random: a hash keyed by the standard
/// library's per-process random keys, over a count and the clock, so
/// two made in one process, or on two machines in one second, differ.
fn random_id() -> String {
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
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

    /// [`fn@Self::line`], to change.
    pub fn line_mut(&mut self, v: Option<&str>) -> Option<&mut Line> {
        match v {
            None => Some(&mut self.line),
            Some(id) => self.version_mut(id).map(|v| &mut v.line),
        }
    }

    /// A new version copying the current edit of version `from` (`None`
    /// for the file's own) as its first and only state, labeled "From
    /// <the source's name>" or "From the original", with no history,
    /// no snapshots and no export records: a history is fifty whole
    /// edits, and the new line's story starts here. Named `name`, or
    /// "Version N" when that is blank or none ([`Self::next_version_name`]).
    /// Its id, or none when `from` names a version the sidecar does
    /// not have.
    pub fn new_version(&mut self, from: Option<&str>, name: Option<&str>) -> Option<String> {
        let (edit, source) = match from {
            None => (self.line.current.clone(), None),
            Some(id) => {
                let v = self.version(id)?;
                (v.line.current.clone(), Some(v.name.clone()))
            }
        };
        let label = history::version_from_label(source.as_deref());
        Some(self.push_version(edit, label, name))
    }

    /// A new version starting from `default_edit`, the edit a frame
    /// nobody has touched gets ([`Edit::for_picture`] for a picture
    /// file, the default with the raw's blend otherwise), labeled "From
    /// the defaults". Named as [`Self::new_version`] names; its id.
    pub fn new_version_from_original(&mut self, default_edit: Edit, name: Option<&str>) -> String {
        self.push_version(default_edit, history::version_from_defaults_label(), name)
    }

    fn push_version(&mut self, edit: Edit, label: String, name: Option<&str>) -> String {
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
        let mut line = Line {
            current: edit,
            current_label: Some(label),
            ..Line::default()
        };
        line.fill_ids();
        self.versions.push(Version {
            id: id.clone(),
            name,
            made: now(),
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
    pub fn swap_with_original(&mut self, id: &str) -> bool {
        match self.versions.iter_mut().find(|v| v.id == id) {
            Some(v) => {
                std::mem::swap(&mut self.line, &mut v.line);
                true
            }
            None => false,
        }
    }

    /// Take version `id` out of the list; the version taken, or none
    /// when there is no such version. Not a state to undo: the editor
    /// asks first and saves.
    pub fn delete_version(&mut self, id: &str) -> Option<Version> {
        let at = self.versions.iter().position(|v| v.id == id)?;
        Some(self.versions.remove(at))
    }
}

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
    fn reread(s: &Sidecar) -> Sidecar {
        Sidecar::from_json(&s.to_json()).unwrap()
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
        let id = s.new_version_from_original(Edit::for_picture(), Some("Fresh"));
        let v = s.version(&id).unwrap();
        assert_eq!(v.name, "Fresh");
        assert_eq!(v.line.current, Edit::for_picture());
        assert_eq!(v.line.current_label.as_deref(), Some("From the defaults"));
        assert!(v.line.history.is_empty() && v.line.snapshots.is_empty());
        assert!(v.line.current_id.is_some());
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
        let line = s.line_mut(Some(&id)).unwrap();
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
        let line = s.line_mut(Some(&id)).unwrap();
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
        let plain: Sidecar = serde_json::from_str(&json).unwrap();
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
        // With the last version deleted it writes what it wrote before.
        s.delete_version(&id).unwrap();
        assert_eq!(s.to_json(), developed().to_json());
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
    fn an_edit_waiting_on_the_frame_keeps_its_three() {
        let mut waiting = Edit {
            version: 3,
            ..Edit::default()
        };
        waiting.geometry.aspect = crate::geometry::Aspect::Original;
        let mut s = Sidecar {
            line: Line::of(waiting),
            ..Sidecar::default()
        };
        s.record(exposed(1.0));
        s.new_version(None, None).unwrap();
        let back = reread(&s);
        assert_eq!(back.history[0].edit.version, 3);
        assert!(back.needs_frame());
        let value: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(value["history"][0]["version"], 3);
        assert_eq!(value["current"]["version"], VERSION);
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
        let err = Sidecar::from_json(&future.to_string()).unwrap_err();
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
        let back = Sidecar::read(&path).unwrap();
        assert_eq!(back.versions, s.versions);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["versions"][0]["line"]["history"] = serde_json::json!([{ "version": 9 }]);
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(matches!(Sidecar::read(&path), Err(Error::Newer(9))));
        assert!(matches!(Sidecar::load(&raw), Err(Error::Newer(9))));
        std::fs::remove_dir_all(&dir).unwrap();
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
        let back = Sidecar::from_json(&value.to_string()).unwrap();
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
            Sidecar::from_json(&v.to_string()).unwrap_err()
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
        assert!(Sidecar::from_json(&zero.to_string()).is_err());

        // A list that is not a list is none, and the rest reads.
        value["versions"] = serde_json::json!({ "id": good });
        let back = Sidecar::from_json(&value.to_string()).unwrap();
        assert!(back.versions.is_empty());
        assert_eq!(back.line, s.line);
        // So is a null.
        value["versions"] = serde_json::Value::Null;
        let back: Sidecar = serde_json::from_str(&value.to_string()).unwrap();
        assert!(back.versions.is_empty());
        // No versions and no line's keys at all: the defaults.
        let empty: Sidecar = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Sidecar::default());
        let bare: Sidecar = serde_json::from_str(r#"{"meta": {"rating": 2}}"#).unwrap();
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
        s.turn_by(1, None);
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
        s.migrate_with_frame(false);
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
                        s.new_version_from_original(Edit::for_picture(), Some("Fresh"));
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
                // The redo stack is not written; take it off to compare.
                let mut written = s.clone();
                for line in written.lines_mut() {
                    line.redo.clear();
                }
                let back = reread(&s);
                assert_eq!(back, written, "seed {seed} step {step}");
            }
        }
    }
}
