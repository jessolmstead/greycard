//! The index's rows standing in for sidecars, as the window holds a
//! frame's sidecar: read from disk, or a stand-in made from the frame's
//! row with nothing read. The window's rules for which it may write and
//! when it reads the rest are its own (the editor's `rows` module).

use std::path::Path;

use greycard_edit::Sidecar;
use greycard_edit_io::on_disk::{FrameShape, Fresh, Trouble, load_sidecar_said};

use crate::RowMeta;

/// What a frame's row stands in for while its sidecar has not been
/// read, kept beside the sidecar in `State::from_row`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FromRow {
    /// The sidecar was read from disk, or there is none to read
    /// (`--no-sidecars`): the row stands in for nothing, and the
    /// sidecar in memory is the whole of it.
    pub read: bool,
    /// The quarter turns and the mirror the row says the picture is
    /// shown at, used while the sidecar stands in; once read, the
    /// sidecar's own edit says.
    pub turns: (u8, bool),
    /// The file's content hash and its mtime stamp, from the row: the
    /// key its thumbnails are kept under in the cache, for a frame
    /// whose file cannot be read (its root offline).
    pub key: Option<(String, u64)>,
    /// The row says the sidecar holds a develop: a read that finds no
    /// sidecar then is not believed over it.
    pub edited: bool,
    /// The row says the sidecar records an export, for the mark on the
    /// frame's cell while the sidecar stands in.
    pub exported: bool,
    /// The frame's ISO as the row had it (`Some(None)`: the camera
    /// gave none), kept past the read; `None` for a frame never
    /// stood in for. What a learned blend is judged against.
    pub iso: Option<Option<u32>>,
    /// The hash of the sidecar the row was read from, `None` for none:
    /// a row that comes in with another says the sidecar changed under
    /// the frame (`edited::changed`).
    pub sidecar: Option<String>,
    /// The last read of the sidecar did not take, and why: it could not
    /// be read, or it was not there though the row says it holds a
    /// develop. The frame stands in still, and nothing is written over
    /// the file; the next request for it reads it again.
    pub unread: Option<String>,
    /// The develop key of the frame's edit as the row has it, so its
    /// edit's pictures are found without the sidecar being read, its
    /// root offline included; `None` for a row not yet read for one, or
    /// one whose edit could not be read.
    pub develop_key: Option<u64>,
    /// The versions the row says the sidecar keeps, each with its own
    /// marks and key: what a version's cell stands in with.
    pub versions: Vec<crate::VersionRow>,
}

impl FromRow {
    /// A frame whose sidecar has been read, or has nothing to read.
    pub fn read() -> Self {
        FromRow {
            read: true,
            turns: (0, false),
            key: None,
            edited: false,
            exported: false,
            iso: None,
            sidecar: None,
            unread: None,
            develop_key: None,
            versions: Vec::new(),
        }
    }

    /// A frame standing in from `row`.
    pub fn of(row: &RowMeta) -> Self {
        FromRow {
            read: false,
            turns: row.shown_turns(),
            key: Some((row.hash.clone(), row.mtime.max(0) as u64)),
            edited: row.edited,
            exported: row.exported,
            iso: Some(row.iso),
            sidecar: row.sidecar_hash.clone(),
            unread: None,
            develop_key: row.develop_key.filter(|&k| k != 0),
            versions: row.versions.clone(),
        }
    }

    /// The marks of version `version` of the frame as the row has
    /// them, the file's own for none: whether it shows as edited and
    /// as exported.
    pub fn marks(&self, version: Option<&str>) -> (bool, bool) {
        match version {
            None => (self.edited, self.exported),
            Some(id) => self
                .versions
                .iter()
                .find(|v| v.id == id)
                .map_or((false, false), |v| (v.edited, v.exported)),
        }
    }
}

/// The versions a row lists, as a stand-in sidecar keeps them: each
/// with the default edit `default` makes, so a version's cell stands
/// where it will once the sidecar is read.
pub fn versions_of_row(
    row: &RowMeta,
    default: impl Fn() -> greycard_edit::Line,
) -> Vec<greycard_edit::Version> {
    row.versions
        .iter()
        .map(|v| greycard_edit::Version {
            id: v.id.clone(),
            name: v.name.clone(),
            made: v.made.max(0) as u64,
            named: 0,
            line: default(),
        })
        .collect()
}

impl Default for FromRow {
    fn default() -> Self {
        Self::read()
    }
}

/// A frame's sidecar as the browser first holds it, with what a merge
/// carries beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub sidecar: Sidecar,
    pub from_row: FromRow,
    /// The frame's row in the index, when the read that brought this
    /// asked the index and it had one.
    pub id: Option<i64>,
    /// Read from disk, and the `.gcd` was not there or could not be
    /// read: the default stands in the sidecar. Over a frame standing
    /// in from its row, that is not taken as the frame's sidecar
    /// (the editor's `rows::take`).
    pub trouble: Option<Trouble>,
}

/// The frames whose sidecars were read from disk since the process
/// started, for the tests that say a view of the roots reads none:
/// by path, since the tests run beside each other.
#[cfg(any(test, feature = "fixtures"))]
pub static DISK_READS: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// A stand-in for `path`'s sidecar from its row: the row's meta, the
/// frame's default edit, nothing read. With sidecars off there is
/// nothing to stand in for, and the frame is held as read. `fresh` is
/// what a raw's new edit starts with, as the sidecar read will start it
/// when there is none.
pub fn from_row(path: &Path, row: &RowMeta, write_sidecars: bool, fresh: Fresh) -> Held {
    if !write_sidecars {
        return from_nothing(path, fresh);
    }
    let default = || greycard_edit_io::on_disk::fresh_sidecar(path, fresh).line;
    Held {
        sidecar: Sidecar {
            line: default(),
            meta: row.meta.clone(),
            // The versions the row lists, standing in as the file's
            // own line does, so each has its cell.
            versions: versions_of_row(row, default),
            ..Sidecar::default()
        },
        from_row: FromRow::of(row),
        id: Some(row.id),
        trouble: None,
    }
}

/// `path`'s sidecar read from disk, as a folder's open always read it,
/// with `sidecars` where the editor's setting puts a sidecar (none with
/// sidecars off, when nothing is read): a Lightroom develop beside a
/// raw with no edit of ours is taken and written there. `fresh` is
/// what a raw with no sidecar starts with; `shape` says the frame's
/// shape where a state waits on it (`on_disk::frame_shape` off the
/// window's thread).
pub fn from_disk(
    path: &Path,
    sidecars: Option<greycard_edit::Placement>,
    fresh: Fresh,
    shape: FrameShape,
) -> Held {
    let Some(placement) = sidecars else {
        return from_nothing(path, fresh);
    };
    #[cfg(any(test, feature = "fixtures"))]
    DISK_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(path.to_path_buf());
    let (sidecar, trouble) = load_sidecar_said(path, Some(placement), fresh, shape);
    Held {
        sidecar,
        from_row: FromRow::read(),
        id: None,
        trouble,
    }
}

/// A frame with sidecars off: its default (a picture's own, as a
/// sidecar read would start it), a raw's lens section as `fresh`
/// says, and nothing to read.
pub fn from_nothing(path: &Path, fresh: Fresh) -> Held {
    Held {
        sidecar: greycard_edit_io::on_disk::fresh_sidecar(path, fresh),
        from_row: FromRow::read(),
        id: None,
        trouble: None,
    }
}
