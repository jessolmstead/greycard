//! The export queue: sets waiting to be exported later, each with the
//! frames, the edits, the sheet and the destination it was queued with,
//! kept in `export-queue.json` beside the settings so a day's picks
//! survive a restart.
//!
//! What is here is the part with no window: the entries, the file and
//! the words. Running an entry is a set's export (`queue::Set`), started
//! by the window one entry at a time.

use crate::sheet::Sheet;
use greycard_edit::Edit;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name, beside `settings.json`.
pub const FILE: &str = "export-queue.json";

/// What the file says its shape is: a later build that changes it
/// bumps this, and this build reads only its own.
const VERSION: u32 = 1;

/// One frame of an entry: the file and the edit it was queued under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedFrame {
    pub source: PathBuf,
    /// The edit at queue time, as the sidecar writes one: the export
    /// is of this, whatever the sidecar says by the time it runs. Read
    /// through the edit's own migration, as a sidecar's is: an edit of
    /// a newer schema than this build's fails the file (which is then
    /// set aside) rather than lose the fields it does not know.
    #[serde(deserialize_with = "migrated")]
    pub edit: Edit,
    /// The frame's quarter turns.
    #[serde(default)]
    pub turn: u8,
    /// A raw with no sidecar yet: its learned blend is seeded from its
    /// ISO at export, as its first open would.
    #[serde(default)]
    pub seed_blend: bool,
}

/// One set waiting: what Export would have written, held until the
/// queue is run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// When it was queued, seconds since the epoch.
    pub queued: u64,
    /// The folder chosen; none for each beside its own file.
    pub folder: Option<PathBuf>,
    /// The export preset the sheet was, as saved, when it was queued.
    #[serde(default)]
    pub preset: Option<String>,
    /// The sheet as it was: format, size, color, mark, the on-exists
    /// choice and all, so a later change to the sheet leaves it alone.
    pub sheet: Sheet,
    pub frames: Vec<QueuedFrame>,
}

/// An edit as stored, brought up to this build's schema first.
fn migrated<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Edit, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let value = greycard_edit::migrate(value).map_err(serde::de::Error::custom)?;
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
struct OnDisk {
    version: u32,
    entries: Vec<Entry>,
}

#[derive(Serialize)]
struct ToDisk<'a> {
    version: u32,
    entries: &'a [Entry],
}

/// The queue's file in the settings' directory.
pub fn path_beside(settings: &Path) -> PathBuf {
    settings.with_file_name(FILE)
}

/// The queue in `path`. No file is an empty queue. One that will not
/// read is moved aside, with a warning, so the next write does not
/// lose it, and the queue starts empty.
pub fn load(path: &Path) -> Vec<Entry> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        // Unreadable now is not empty: set aside, so the next write
        // does not put an empty queue over it.
        Err(e) => {
            set_aside(path, &e.to_string());
            return Vec::new();
        }
    };
    match serde_json::from_slice::<OnDisk>(&bytes) {
        Ok(d) if d.version == VERSION => d.entries,
        Ok(d) => {
            set_aside(path, &format!("version {} is not this build's", d.version));
            Vec::new()
        }
        Err(e) => {
            set_aside(path, &e.to_string());
            Vec::new()
        }
    }
}

/// Move the file at `path` to `export-queue.json.unread`, or `.unread.2`
/// and on when one is there already, so no earlier one is written over.
fn set_aside(path: &Path, why: &str) {
    let mut aside = path.with_extension("json.unread");
    let mut n = 2;
    while aside.exists() {
        aside = path.with_extension(format!("json.unread.{n}"));
        n += 1;
    }
    match std::fs::rename(path, &aside) {
        Ok(()) => tracing::warn!(
            "export queue {}: {why}; moved to {}, nothing queued",
            path.display(),
            aside.display()
        ),
        Err(e) => tracing::warn!(
            "export queue {}: {why}; nothing queued ({e})",
            path.display()
        ),
    }
}

/// Write the queue to `path`, through a file beside it renamed over
/// the old one, so a crash mid-write leaves the last queue whole. An
/// empty queue removes the file. Compact, not pretty: it is written
/// after every frame of a running entry, edits and all.
pub fn save(path: &Path, entries: &[Entry]) {
    if entries.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("export queue {}: {e}", path.display()),
        }
        return;
    }
    let text = match serde_json::to_string(&ToDisk {
        version: VERSION,
        entries,
    }) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("export queue not saved: {e}");
            return;
        }
    };
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!("export queue directory {}: {e}", dir.display());
        return;
    }
    let partial = path.with_extension("json.partial");
    let written = (|| {
        use std::io::Write;
        let mut f = std::fs::File::create(&partial)?;
        f.write_all(text.as_bytes())?;
        // On the disk before it takes the old one's name.
        f.sync_all()?;
        std::fs::rename(&partial, path)
    })();
    if let Err(e) = written {
        tracing::warn!("export queue not saved to {}: {e}", path.display());
    }
}

/// How many frames the queue holds, all entries together.
pub fn frame_count(entries: &[Entry]) -> usize {
    entries.iter().map(|e| e.frames.len()).sum()
}

/// "1 set", "3 sets".
pub fn sets(n: usize) -> String {
    format!("{n} set{}", if n == 1 { "" } else { "s" })
}

/// The sheet's line for the queue: "2 sets, 7 frames queued".
pub fn count_line(entries: &[Entry]) -> String {
    format!(
        "{}, {} queued",
        sets(entries.len()),
        crate::queue::frames(frame_count(entries))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-xq-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn entry(n: usize, preset: Option<&str>) -> Entry {
        Entry {
            queued: 1_790_000_000,
            folder: Some(PathBuf::from("/out")),
            preset: preset.map(str::to_string),
            sheet: Sheet {
                size: "2048".into(),
                ..Sheet::default()
            },
            frames: (0..n)
                .map(|i| {
                    let mut edit = Edit::default();
                    edit.light.exposure = i as f32 * 0.5;
                    QueuedFrame {
                        source: PathBuf::from(format!("/shoot/IMG_{i:04}.CR3")),
                        edit,
                        turn: (i % 4) as u8,
                        seed_blend: i == 0,
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn the_queue_round_trips_and_an_empty_one_leaves_no_file() {
        let dir = scratch("round");
        let file = dir.join("greycard").join(FILE);
        assert!(load(&file).is_empty(), "no file is no queue");
        let q = vec![entry(2, Some("Web")), entry(1, None)];
        save(&file, &q);
        assert_eq!(load(&file), q);
        assert_eq!(count_line(&q), "2 sets, 3 frames queued");
        assert_eq!(count_line(&q[1..]), "1 set, 1 frame queued");
        save(&file, &[]);
        assert!(!file.exists());
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_file_that_will_not_read_is_set_aside_not_lost() {
        let dir = scratch("bad");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(FILE);
        std::fs::write(&file, "{ not json").unwrap();
        assert!(load(&file).is_empty());
        assert!(!file.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("export-queue.json.unread")).unwrap(),
            "{ not json"
        );
        // A second one does not go over the first set aside.
        std::fs::write(&file, r#"{"version": 99, "entries": []}"#).unwrap();
        assert!(load(&file).is_empty());
        assert!(!file.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("export-queue.json.unread")).unwrap(),
            "{ not json"
        );
        assert!(dir.join("export-queue.json.unread.2").exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// An entry's edit is read through the edit's migration: an older
    /// schema is brought up, a newer one fails the file, which is set
    /// aside rather than half read.
    #[test]
    fn an_edit_in_the_queue_is_migrated_and_a_newer_one_set_aside() {
        let dir = scratch("migrate");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(FILE);
        let with_edit = |edit: serde_json::Value| {
            let mut q = serde_json::to_value(ToDisk {
                version: VERSION,
                entries: &[entry(1, None)],
            })
            .unwrap();
            q["entries"][0]["frames"][0]["edit"] = edit;
            std::fs::write(&file, q.to_string()).unwrap();
        };
        // Version 1, the oldest: brought up to this build's.
        with_edit(serde_json::json!({ "version": 1, "light": { "exposure": 0.75 } }));
        let read = load(&file);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].frames[0].edit.light.exposure, 0.75);
        assert!(file.exists(), "a file that read is left where it is");
        // A schema this build does not know yet.
        let newer = greycard_edit::VERSION + 1;
        with_edit(serde_json::json!({ "version": newer, "light": { "exposure": 0.75 } }));
        assert!(load(&file).is_empty());
        assert!(!file.exists());
        assert!(dir.join("export-queue.json.unread").exists());
        crate::testing::remove_dir_retry(&dir);
    }
}
