//! Indexing: a folder's files against the rows the index holds for
//! it, as little read as the rules allow.
//!
//! The rules, in the order they are tried for each file on disk:
//!
//! 1. A row at this path with the same size and mtime: the file is
//!    not read. Its sidecar is looked for, and when that is not the
//!    one the row knows (a different place, size or mtime, or none
//!    where there was one) the meta is read again and only the meta.
//! 2. A row at this path with another size or mtime: the file
//!    changed on disk, and is hashed and probed again.
//! 3. No row at this path: the file is hashed. A row with that hash
//!    whose own path is gone from disk is this file moved, and
//!    keeps its id and its EXIF; only its path and its meta are
//!    written. Otherwise the file is probed and a row is added.
//!
//! What was in the folder's rows and not on disk is marked missing,
//! with the time, and kept: a move to a folder not yet indexed is
//! found when that folder is, from the missing row's hash.
//!
//! A file the probe cannot read is still a row, with no EXIF, so it
//! is listed by name and rating with the rest; the error is in the
//! report. It is not retried until the file changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use greycard_edit::Sidecar;
use greycard_edit::meta::Meta;
use rusqlite::{Transaction, params};

use crate::{Exif, Library, Result, filter, hash, mtime_of, now_secs, path_text};

/// Where an index run is, for a progress bar: `done` of `total`
/// files in this folder, the one about to be looked at.
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    pub done: usize,
    pub total: usize,
    pub path: &'a Path,
}

/// What an index run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Files not in the index before.
    pub added: usize,
    /// Files found under a new path by their hash.
    pub moved: usize,
    /// Files whose size or mtime changed, read again whole.
    pub changed: usize,
    /// Files unchanged whose sidecar changed: the meta read again.
    pub meta_refreshed: usize,
    /// Files whose row was right already.
    pub unchanged: usize,
    /// Files back at a path the index had marked missing.
    pub returned: usize,
    /// Rows whose file is gone from its path.
    pub missing: usize,
    /// Files that could not be read, and why; each has a row anyway
    /// when it could be hashed.
    pub errors: Vec<(PathBuf, String)>,
}

impl Report {
    /// Files looked at.
    pub fn seen(&self) -> usize {
        self.added + self.moved + self.changed + self.meta_refreshed + self.unchanged
    }

    fn add(&mut self, other: Report) {
        self.added += other.added;
        self.moved += other.moved;
        self.changed += other.changed;
        self.meta_refreshed += other.meta_refreshed;
        self.unchanged += other.unchanged;
        self.returned += other.returned;
        self.missing += other.missing;
        self.errors.extend(other.errors);
    }
}

/// A folder's row as the pass needs it.
struct Row {
    id: i64,
    size: u64,
    mtime: i64,
    sidecar: Option<String>,
    sidecar_size: Option<i64>,
    sidecar_mtime: Option<i64>,
    missing: bool,
}

/// Whether a path is one the index holds: a raw or a picture, as
/// the browser lists a folder.
pub fn is_indexed_path(path: &Path) -> bool {
    greycard_core::decode::is_raw_path(path) || greycard_core::picture::is_picture_path(path)
}

/// The files of one folder, sorted, not descending.
fn list_folder(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.path())
        .filter(|p| is_indexed_path(p))
        .collect();
    files.sort();
    Ok(files)
}

impl Library {
    /// Index one folder's files, not its subfolders: see the module
    /// for the rules. `progress` is called before each file.
    pub fn index_folder(
        &mut self,
        dir: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
    ) -> Result<Report> {
        let dir = std::fs::canonicalize(dir)?;
        let files = list_folder(&dir)?;
        let folder = path_text(&dir);
        let tx = self.conn_mut().transaction()?;
        let existing = rows_in_folder(&tx, &folder)?;
        let report = index_paths(&tx, &folder, &files, existing, progress)?;
        tx.commit()?;
        Ok(report)
    }

    /// Index a folder and every folder under it, hidden ones (a
    /// leading dot, the sidecar folder among them) left alone.
    pub fn index_tree(
        &mut self,
        root: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
    ) -> Result<Report> {
        let mut dirs = vec![std::fs::canonicalize(root)?];
        let mut report = Report::default();
        while let Some(dir) = dirs.pop() {
            report.add(self.index_folder(&dir, progress)?);
            let mut under: Vec<PathBuf> = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.path())
                .collect();
            // Popped from the end, so reversed to walk in name order.
            under.sort();
            under.reverse();
            dirs.extend(under);
        }
        Ok(report)
    }

    /// Bring one file's row up to date, under the same rules as its
    /// folder: after a sidecar is saved, for the row to say what the
    /// sidecar says without a pass over the folder. A file gone from
    /// its path is marked missing.
    pub fn index_file(&mut self, path: &Path) -> Result<Report> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let folder = path_text(path.parent().unwrap_or(Path::new("")));
        let tx = self.conn_mut().transaction()?;
        let mut existing = HashMap::new();
        if let Some(row) = row_at(&tx, &path_text(&path))? {
            existing.insert(path_text(&path), row);
        }
        let files: Vec<PathBuf> = if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
        let report = index_paths(&tx, &folder, &files, existing, &mut |_| {})?;
        tx.commit()?;
        Ok(report)
    }
}

fn rows_in_folder(tx: &Transaction<'_>, folder: &str) -> Result<HashMap<String, Row>> {
    let mut stmt = tx.prepare_cached(
        "SELECT id, path, size, mtime, sidecar, sidecar_size, sidecar_mtime, missing_since \
         FROM files WHERE folder = ?",
    )?;
    let rows = stmt.query_map(params![folder], |r| {
        Ok((
            r.get::<_, String>(1)?,
            Row {
                id: r.get(0)?,
                size: r.get::<_, i64>(2)? as u64,
                mtime: r.get(3)?,
                sidecar: r.get(4)?,
                sidecar_size: r.get(5)?,
                sidecar_mtime: r.get(6)?,
                missing: r.get::<_, Option<i64>>(7)?.is_some(),
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

fn row_at(tx: &Transaction<'_>, path: &str) -> Result<Option<Row>> {
    use rusqlite::OptionalExtension;
    Ok(tx
        .prepare_cached(
            "SELECT id, size, mtime, sidecar, sidecar_size, sidecar_mtime, missing_since \
             FROM files WHERE path = ?",
        )?
        .query_row(params![path], |r| {
            Ok(Row {
                id: r.get(0)?,
                size: r.get::<_, i64>(1)? as u64,
                mtime: r.get(2)?,
                sidecar: r.get(3)?,
                sidecar_size: r.get(4)?,
                sidecar_mtime: r.get(5)?,
                missing: r.get::<_, Option<i64>>(6)?.is_some(),
            })
        })
        .optional()?)
}

/// The sidecar a file has now: its path, size and mtime, or `None`.
struct SidecarStat {
    path: String,
    size: i64,
    mtime: i64,
}

fn sidecar_of(raw: &Path) -> Option<SidecarStat> {
    let path = Sidecar::find(raw)?;
    let metadata = std::fs::metadata(&path).ok()?;
    Some(SidecarStat {
        path: path_text(&path),
        size: metadata.len() as i64,
        mtime: mtime_of(&metadata),
    })
}

impl Row {
    fn same_sidecar(&self, now: Option<&SidecarStat>) -> bool {
        match now {
            None => self.sidecar.is_none(),
            Some(s) => {
                self.sidecar.as_deref() == Some(s.path.as_str())
                    && self.sidecar_size == Some(s.size)
                    && self.sidecar_mtime == Some(s.mtime)
            }
        }
    }
}

/// The pass over `files`, which are all in `folder`, against
/// `existing`, the folder's rows by path; what is left of `existing`
/// at the end is marked missing.
fn index_paths(
    tx: &Transaction<'_>,
    folder: &str,
    files: &[PathBuf],
    mut existing: HashMap<String, Row>,
    progress: &mut dyn FnMut(Progress<'_>),
) -> Result<Report> {
    let mut report = Report::default();
    let total = files.len();
    for (done, path) in files.iter().enumerate() {
        progress(Progress { done, total, path });
        let key = path_text(path);
        let stat = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                report.errors.push((path.clone(), e.to_string()));
                continue;
            }
        };
        let (size, mtime) = (stat.len(), mtime_of(&stat));
        let sidecar = sidecar_of(path);
        match existing.remove(&key) {
            Some(row) if row.size == size && row.mtime == mtime => {
                if row.missing {
                    tx.prepare_cached("UPDATE files SET missing_since = NULL WHERE id = ?")?
                        .execute(params![row.id])?;
                    report.returned += 1;
                }
                if row.same_sidecar(sidecar.as_ref()) {
                    report.unchanged += 1;
                } else {
                    write_meta(tx, row.id, sidecar.as_ref())?;
                    report.meta_refreshed += 1;
                }
            }
            Some(row) => {
                let hash = match hash::hash_file(path) {
                    Ok(h) => h,
                    Err(e) => {
                        report.errors.push((path.clone(), e.to_string()));
                        continue;
                    }
                };
                let exif = probe(path, &mut report);
                tx.prepare_cached(
                    "UPDATE files SET size = ?, mtime = ?, hash = ?, make = ?, model = ?, \
                     camera = ?, lens = ?, iso = ?, focal = ?, aperture = ?, shutter = ?, \
                     taken = ?, missing_since = NULL WHERE id = ?",
                )?
                .execute(params![
                    size as i64,
                    mtime,
                    hash,
                    exif.make,
                    exif.model,
                    exif.camera,
                    exif.lens,
                    exif.iso,
                    exif.focal,
                    exif.aperture,
                    exif.shutter,
                    exif.taken,
                    row.id
                ])?;
                write_meta(tx, row.id, sidecar.as_ref())?;
                report.changed += 1;
            }
            None => {
                let hash = match hash::hash_file(path) {
                    Ok(h) => h,
                    Err(e) => {
                        report.errors.push((path.clone(), e.to_string()));
                        continue;
                    }
                };
                if let Some((id, old_path)) = gone_by_hash(tx, &hash)? {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    tx.prepare_cached(
                        "UPDATE files SET path = ?, folder = ?, name = ?, size = ?, mtime = ?, \
                         missing_since = NULL WHERE id = ?",
                    )?
                    .execute(params![
                        key,
                        folder,
                        name,
                        size as i64,
                        mtime,
                        id
                    ])?;
                    write_meta(tx, id, sidecar.as_ref())?;
                    // A rename within the folder: the old row is this
                    // one, and is not to be marked missing.
                    existing.remove(&old_path);
                    log::info!("{}: moved from {old_path}", path.display());
                    report.moved += 1;
                } else {
                    let exif = probe(path, &mut report);
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    tx.prepare_cached(
                        "INSERT INTO files (path, folder, name, size, mtime, hash, make, model, \
                         camera, lens, iso, focal, aperture, shutter, taken) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    )?
                    .execute(params![
                        key,
                        folder,
                        name,
                        size as i64,
                        mtime,
                        hash,
                        exif.make,
                        exif.model,
                        exif.camera,
                        exif.lens,
                        exif.iso,
                        exif.focal,
                        exif.aperture,
                        exif.shutter,
                        exif.taken,
                    ])?;
                    let id = tx.last_insert_rowid();
                    write_meta(tx, id, sidecar.as_ref())?;
                    report.added += 1;
                }
            }
        }
    }
    // What was not on disk.
    let now = now_secs();
    for row in existing.values().filter(|r| !r.missing) {
        tx.prepare_cached("UPDATE files SET missing_since = ? WHERE id = ?")?
            .execute(params![now, row.id])?;
        report.missing += 1;
    }
    Ok(report)
}

/// A row with this hash whose file is not at its path any more: a
/// move's other end. Its id and its old path.
fn gone_by_hash(tx: &Transaction<'_>, hash: &str) -> Result<Option<(i64, String)>> {
    let mut stmt = tx.prepare_cached("SELECT id, path FROM files WHERE hash = ? ORDER BY id")?;
    let candidates = stmt.query_map(params![hash], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for candidate in candidates {
        let (id, old_path) = candidate?;
        if !Path::new(&old_path).exists() {
            return Ok(Some((id, old_path)));
        }
    }
    Ok(None)
}

/// The file's EXIF, a raw's through its decoder and a picture's
/// from its chunk; a file that will not say is an empty EXIF and a
/// line in the report.
fn probe(path: &Path, report: &mut Report) -> Exif {
    let probed = if greycard_core::picture::is_picture_path(path) {
        greycard_core::picture::probe_path(path)
    } else {
        greycard_core::decode::probe_path(path)
    };
    match probed {
        Ok(p) => Exif::from_probe(&p),
        Err(e) => {
            log::warn!("{}: {e}", path.display());
            report.errors.push((path.to_path_buf(), e.to_string()));
            Exif::default()
        }
    }
}

/// The sidecar's meta section and nothing else of it: the edit and
/// its history are not read, and a sidecar whose edit this build
/// cannot read still gives up its stars.
pub fn read_meta(sidecar: &Path) -> Meta {
    let json = match std::fs::read_to_string(sidecar) {
        Ok(j) => j,
        Err(e) => {
            log::warn!("{}: {e}", sidecar.display());
            return Meta::default();
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("{}: {e}", sidecar.display());
            return Meta::default();
        }
    };
    value
        .get("meta")
        .cloned()
        .and_then(|m| serde_json::from_value(m).ok())
        .unwrap_or_default()
}

/// The row's meta from its sidecar, or the empty meta when it has
/// none, and the sidecar's stat so the next pass can tell.
fn write_meta(tx: &Transaction<'_>, id: i64, sidecar: Option<&SidecarStat>) -> Result<()> {
    let meta = sidecar
        .map(|s| read_meta(Path::new(&s.path)))
        .unwrap_or_default();
    tx.prepare_cached(
        "UPDATE files SET sidecar = ?, sidecar_size = ?, sidecar_mtime = ?, rating = ?, \
         flag = ?, label = ?, keywords = ? WHERE id = ?",
    )?
    .execute(params![
        sidecar.map(|s| s.path.as_str()),
        sidecar.map(|s| s.size),
        sidecar.map(|s| s.mtime),
        i64::from(meta.rating),
        filter::flag_name(meta.flag),
        filter::label_name(meta.label),
        serde_json::to_string(&meta.keywords).unwrap_or_else(|_| "[]".into()),
        id
    ])?;
    tx.prepare_cached("DELETE FROM keywords WHERE file = ?")?
        .execute(params![id])?;
    let mut insert =
        tx.prepare_cached("INSERT OR IGNORE INTO keywords (file, word) VALUES (?, ?)")?;
    for word in &meta.keywords {
        insert.execute(params![id, word])?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Entry, Filter, Library};
    use greycard_core::exif::{Provenance, write_rgb16_tiff};
    use greycard_edit::Placement;
    use greycard_edit::meta::{Flag, Label};
    use rawler::decoders::RawMetadata;
    use rawler::exif::Exif as RawExif;
    use rawler::formats::tiff::Rational;
    use std::time::{Duration, SystemTime};

    /// A scratch folder of this run's own.
    pub(crate) fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-library-{what}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A frame's worth of EXIF.
    pub(crate) struct Frame<'a> {
        pub make: &'a str,
        pub model: &'a str,
        pub lens: &'a str,
        pub iso: u16,
        pub focal: u32,
        pub aperture: (u32, u32),
        pub shutter: (u32, u32),
        pub taken: &'a str,
    }

    /// A small TIFF carrying real EXIF, as an export writes one:
    /// a picture the probe reads the way it reads any JPEG's.
    pub(crate) fn write_frame(path: &Path, frame: &Frame<'_>, seed: u16) {
        let metadata = RawMetadata {
            exif: RawExif {
                exposure_time: Some(Rational::new(frame.shutter.0, frame.shutter.1)),
                fnumber: Some(Rational::new(frame.aperture.0, frame.aperture.1)),
                iso_speed_ratings: Some(frame.iso),
                date_time_original: Some(frame.taken.into()),
                focal_length: Some(Rational::new(frame.focal, 1)),
                lens_model: Some(frame.lens.into()),
                ..RawExif::default()
            },
            model: frame.model.into(),
            make: frame.make.into(),
            lens: None,
            unique_image_id: None,
            rating: None,
        };
        let (w, h) = (4u32, 3u32);
        let data: Vec<u16> = (0..w * h * 3)
            .map(|i| (i as u16).wrapping_mul(2654).wrapping_add(seed))
            .collect();
        let mut file = std::io::Cursor::new(Vec::new());
        write_rgb16_tiff(
            &mut file,
            w,
            h,
            &data,
            None,
            Some(&Provenance {
                metadata: &metadata,
                software: "greycard-library test",
                width: w,
                height: h,
                srgb: true,
                written: None,
                source_name: None,
                output: None,
                edit: None,
            }),
        )
        .unwrap();
        std::fs::write(path, file.into_inner()).unwrap();
    }

    const R5: Frame<'static> = Frame {
        make: "Canon",
        model: "Canon EOS R5",
        lens: "RF35mm F1.4 L VCM",
        iso: 100,
        focal: 35,
        aperture: (2, 1),
        shutter: (1, 2500),
        taken: "2024:08:24 15:32:39",
    };
    const R6: Frame<'static> = Frame {
        make: "Canon",
        model: "Canon EOS R6m2",
        lens: "RF50mm F1.8 STM",
        iso: 6400,
        focal: 50,
        aperture: (18, 10),
        shutter: (1, 60),
        taken: "2026:09:21 20:05:00",
    };
    const A7: Frame<'static> = Frame {
        make: "SONY",
        model: "ILCE-7M4",
        lens: "FE 24-70mm F2.8 GM II",
        iso: 3200,
        focal: 24,
        aperture: (28, 10),
        shutter: (1, 320),
        taken: "2026:09:02 08:00:00",
    };

    /// Three frames and their sidecars: the R5 rated and picked
    /// beside the file, the R6 labelled under the hidden folder,
    /// the A7 bare.
    fn shoot(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let (r5, r6, a7) = (dir.join("r5.tif"), dir.join("r6.tif"), dir.join("a7.tif"));
        write_frame(&r5, &R5, 1);
        write_frame(&r6, &R6, 2);
        write_frame(&a7, &A7, 3);
        let mut s = Sidecar::default();
        s.meta.rating = 4;
        s.meta.flag = Flag::Pick;
        s.meta.set_keywords(vec!["Wedding".into(), "Harbor".into()]);
        s.save(&r5).unwrap();
        let mut s = Sidecar::default();
        s.meta.rating = 2;
        s.meta.label = Label::Red;
        s.save_in(&r6, Placement::Folder).unwrap();
        (r5, r6, a7)
    }

    fn names(lib: &Library, filter: &str) -> Vec<String> {
        lib.query(&Filter::parse(filter).unwrap())
            .unwrap()
            .iter()
            .map(Entry::name)
            .collect()
    }

    #[test]
    fn a_folder_is_indexed_and_a_second_pass_reads_nothing() {
        let dir = scratch("index");
        let (r5, r6, _a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        let mut seen = Vec::new();
        let report = lib
            .index_folder(&dir, &mut |p| {
                seen.push((p.done, p.total, p.path.to_path_buf()))
            })
            .unwrap();
        assert_eq!(report.added, 3, "{report:?}");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].0, 0);
        assert_eq!(seen[2], (2, 3, std::fs::canonicalize(&r6).unwrap()));
        assert_eq!(lib.len().unwrap(), 3);

        // The EXIF came through the probe.
        let e = lib.by_path(&r5).unwrap().expect("the R5's row");
        assert_eq!(e.exif.camera, "Canon EOS R5");
        assert_eq!(e.exif.lens.as_deref(), Some("RF35mm F1.4 L VCM"));
        assert_eq!(e.exif.iso, Some(100));
        assert_eq!(e.exif.focal, Some(35.0));
        assert_eq!(e.exif.aperture, Some(2.0));
        assert!((e.exif.shutter.unwrap() - 1.0 / 2500.0).abs() < 1e-9);
        assert_eq!(e.exif.taken.as_deref(), Some("2024-08-24 15:32:39"));
        assert_eq!(e.hash, hash::hash_file(&r5).unwrap());
        assert_eq!(e.size, std::fs::metadata(&r5).unwrap().len());
        // And the meta, from wherever the sidecar was.
        assert_eq!(e.meta.rating, 4);
        assert_eq!(e.meta.flag, Flag::Pick);
        assert_eq!(e.meta.keywords, ["Wedding", "Harbor"]);
        assert_eq!(
            e.sidecar,
            Some(
                std::fs::canonicalize(&r5)
                    .unwrap()
                    .with_extension("tif.gcd")
            )
        );
        let e = lib.by_path(&r6).unwrap().unwrap();
        assert_eq!(e.meta.label, Label::Red);
        assert!(e.sidecar.unwrap().to_string_lossy().contains(".greycard"));

        // The filters, against the seeded rows.
        assert_eq!(names(&lib, ""), ["a7.tif", "r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "camera:R6"), ["r6.tif"]);
        assert_eq!(names(&lib, "make=canon"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "make!=canon"), ["a7.tif"]);
        assert_eq!(names(&lib, "lens:\"24-70\""), ["a7.tif"]);
        assert_eq!(names(&lib, "iso>=3200"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "iso>=3200 camera:sony"), ["a7.tif"]);
        assert_eq!(names(&lib, "focal:50mm"), ["r6.tif"]);
        assert_eq!(names(&lib, "focal<=35"), ["a7.tif", "r5.tif"]);
        assert_eq!(names(&lib, "aperture:f/1.8"), ["r6.tif"]);
        assert_eq!(names(&lib, "aperture<2.8"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "aperture<=2.8"), ["a7.tif", "r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "aperture>2"), ["a7.tif"]);
        assert_eq!(names(&lib, "aperture>=2"), ["a7.tif", "r5.tif"]);
        assert_eq!(names(&lib, "aperture:2.8"), ["a7.tif"]);
        assert_eq!(names(&lib, "aperture!=2.8"), ["r5.tif", "r6.tif"]);
        assert_eq!(names(&lib, "shutter:1/60"), ["r6.tif"]);
        assert_eq!(names(&lib, "shutter<1/1000"), ["r5.tif"]);
        assert_eq!(names(&lib, "date:2026-09"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "date:2026-09-21"), ["r6.tif"]);
        assert_eq!(names(&lib, "date<=2026-09-02"), ["a7.tif", "r5.tif"]);
        assert_eq!(names(&lib, "date>2024"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "date:2024"), ["r5.tif"]);
        assert_eq!(names(&lib, "rating>=3"), ["r5.tif"]);
        assert_eq!(names(&lib, "rating:0"), ["a7.tif"]);
        assert_eq!(names(&lib, "rating<=2 flag!=pick"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "flag:pick"), ["r5.tif"]);
        assert_eq!(names(&lib, "flag:none"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "label:red"), ["r6.tif"]);
        assert_eq!(names(&lib, "label!=none"), ["r6.tif"]);
        assert_eq!(names(&lib, "keyword:wedding"), ["r5.tif"]);
        assert_eq!(names(&lib, "keyword=harbor"), ["r5.tif"]);
        assert_eq!(names(&lib, "keyword=harb"), Vec::<String>::new());
        assert_eq!(names(&lib, "keyword!=wedding"), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "harbor"), ["r5.tif"]);
        assert_eq!(names(&lib, "R6"), ["r6.tif"]);
        // Two words both have to land, in the name or a keyword.
        assert_eq!(names(&lib, "r 6"), ["r6.tif"]);
        assert_eq!(names(&lib, "r6 harbor"), Vec::<String>::new());
        assert_eq!(names(&lib, "r5 harbor"), ["r5.tif"]);
        assert_eq!(names(&lib, "name:.tif"), ["a7.tif", "r5.tif", "r6.tif"]);
        assert_eq!(
            names(
                &lib,
                &format!(
                    "folder=\"{}\"",
                    std::fs::canonicalize(&dir).unwrap().display()
                )
            ),
            ["a7.tif", "r5.tif", "r6.tif"]
        );
        assert_eq!(lib.count(&Filter::parse("iso>=3200").unwrap()).unwrap(), 2);
        assert_eq!(
            lib.folders().unwrap(),
            vec![std::fs::canonicalize(&dir).unwrap()]
        );

        // Again: nothing added, nothing read.
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(
            report,
            Report {
                unchanged: 3,
                ..Report::default()
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_changed_sidecar_refreshes_the_meta_and_nothing_else() {
        let dir = scratch("meta");
        let (r5, _r6, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut |_| {}).unwrap();
        let before = lib.by_path(&r5).unwrap().unwrap();

        // The R5 loses a star; the A7 gains a sidecar. The sidecar's
        // mtime is moved on by hand, since a save within the same
        // filesystem tick would look like the same sidecar.
        let mut s = Sidecar::load(&r5).unwrap().unwrap();
        s.meta.rating = 3;
        s.save(&r5).unwrap();
        let gcd = Sidecar::find(&r5).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&gcd)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let mut s = Sidecar::default();
        s.meta.set_keywords(vec!["sea".into()]);
        s.save(&a7).unwrap();

        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(
            report,
            Report {
                meta_refreshed: 2,
                unchanged: 1,
                ..Report::default()
            }
        );
        let after = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(after.meta.rating, 3);
        assert_eq!(after.meta.flag, Flag::Pick);
        assert_eq!(after.id, before.id);
        assert_eq!(after.hash, before.hash);
        assert_eq!(after.exif, before.exif);
        assert_ne!(after.sidecar_mtime, before.sidecar_mtime);
        assert_eq!(names(&lib, "keyword:sea"), ["a7.tif"]);

        // The sidecar taken away: the meta goes with it.
        std::fs::remove_file(Sidecar::find(&a7).unwrap()).unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert!(names(&lib, "keyword:sea").is_empty());
        assert_eq!(lib.by_path(&a7).unwrap().unwrap().sidecar, None);

        // One file on its own, after a save: the same rules.
        let mut s = Sidecar::load(&r5).unwrap().unwrap();
        s.meta.rating = 5;
        s.save(&r5).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&gcd)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(10))
            .unwrap();
        let report = lib.index_file(&r5).unwrap();
        assert_eq!(report.meta_refreshed, 1, "{report:?}");
        assert_eq!(lib.by_path(&r5).unwrap().unwrap().meta.rating, 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_changed_on_disk_is_read_again() {
        let dir = scratch("changed");
        let (r5, _, _) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut |_| {}).unwrap();
        let before = lib.by_path(&r5).unwrap().unwrap();
        // Overwritten by another frame, a second later.
        write_frame(&r5, &A7, 9);
        std::fs::File::options()
            .write(true)
            .open(&r5)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.changed, 1, "{report:?}");
        let after = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(after.id, before.id);
        assert_ne!(after.hash, before.hash);
        assert_eq!(after.exif.camera, "SONY ILCE-7M4");
        // The sidecar beside it still counts.
        assert_eq!(after.meta.rating, 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_renamed_file_is_a_move_and_keeps_its_row() {
        let dir = scratch("move");
        let (r5, _r6, a7) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut |_| {}).unwrap();
        let before = lib.by_path(&r5).unwrap().unwrap();

        // Renamed within the folder, the sidecar with it.
        let renamed = dir.join("z_renamed.tif");
        std::fs::rename(&r5, &renamed).unwrap();
        std::fs::rename(Sidecar::path_for(&r5), Sidecar::path_for(&renamed)).unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(
            report,
            Report {
                moved: 1,
                unchanged: 2,
                ..Report::default()
            }
        );
        assert_eq!(lib.len().unwrap(), 3);
        let after = lib
            .by_path(&renamed)
            .unwrap()
            .expect("the row at the new path");
        assert_eq!(after.id, before.id);
        assert_eq!(after.hash, before.hash);
        assert_eq!(after.exif, before.exif);
        assert_eq!(after.meta.rating, 4);
        assert!(lib.by_path(&r5).unwrap().is_none());
        let found = lib.by_hash(&before.hash).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, std::fs::canonicalize(&renamed).unwrap());
        assert!(!found[0].missing);

        // Moved to a folder indexed later: the old folder marks it
        // missing, the new one claims it.
        let sub = dir.join("picks");
        std::fs::create_dir(&sub).unwrap();
        let moved = sub.join("a7.tif");
        std::fs::rename(&a7, &moved).unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        let gone = lib.by_path(&moved).unwrap();
        assert!(gone.is_none());
        assert!(names(&lib, "").iter().all(|n| n != "a7.tif"));
        assert_eq!(names(&lib, "missing:yes"), ["a7.tif"]);
        let report = lib.index_folder(&sub, &mut |_| {}).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
        assert_eq!(report.added, 0);
        assert_eq!(lib.len().unwrap(), 3);
        let e = lib.by_path(&moved).unwrap().unwrap();
        assert_eq!(e.exif.camera, "SONY ILCE-7M4");
        assert!(!e.missing);
        assert!(names(&lib, "missing:yes").is_empty());
        // And the old folder, indexed again, has nothing to say.
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        assert_eq!(report.unchanged, 2);

        // Moved the other way round: the new folder indexed first
        // finds the old path gone and claims the row before the old
        // folder ever notices.
        let back = dir.join("a7.tif");
        std::fs::rename(&moved, &back).unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
        let report = lib.index_folder(&sub, &mut |_| {}).unwrap();
        assert_eq!(report, Report::default());
        assert_eq!(lib.len().unwrap(), 3);

        // A copy is not a move: both files are there, so both rows.
        let copy = dir.join("copy.tif");
        std::fs::copy(&back, &copy).unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.added, 1, "{report:?}");
        assert_eq!(lib.by_hash(&e.hash).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_gone_is_missing_until_pruned_or_back() {
        let dir = scratch("missing");
        let (r5, _, _) = shoot(&dir);
        let mut lib = Library::open_in_memory().unwrap();
        lib.index_folder(&dir, &mut |_| {}).unwrap();
        let bytes = std::fs::read(&r5).unwrap();
        std::fs::remove_file(&r5).unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.missing, 1, "{report:?}");
        assert_eq!(names(&lib, ""), ["a7.tif", "r6.tif"]);
        assert_eq!(names(&lib, "missing:yes"), ["r5.tif"]);
        assert_eq!(names(&lib, "missing:no"), ["a7.tif", "r6.tif"]);
        assert!(lib.by_path(&r5).unwrap().unwrap().missing);
        // Still missing next time, and not counted twice.
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.missing, 0, "{report:?}");
        // Back where it was: found, and its row is the same row.
        let id = lib.by_path(&r5).unwrap().unwrap().id;
        std::fs::write(&r5, &bytes).unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert!(report.returned == 1 || report.changed == 1, "{report:?}");
        assert_eq!(report.added, 0);
        let e = lib.by_path(&r5).unwrap().unwrap();
        assert_eq!(e.id, id);
        assert!(!e.missing);
        // Gone for good, and pruned.
        std::fs::remove_file(&r5).unwrap();
        lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(lib.prune_missing().unwrap(), 1);
        assert_eq!(lib.len().unwrap(), 2);
        assert!(lib.by_path(&r5).unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_will_not_probe_is_still_a_row() {
        let dir = scratch("bad");
        let bad = dir.join("IMG_0001.CR3");
        std::fs::write(&bad, b"not a raw at all").unwrap();
        let mut s = Sidecar::default();
        s.meta.rating = 1;
        s.save(&bad).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(report.errors[0].0, std::fs::canonicalize(&bad).unwrap());
        let e = lib.by_path(&bad).unwrap().unwrap();
        assert_eq!(e.exif, Exif::default());
        assert_eq!(e.meta.rating, 1);
        assert_eq!(names(&lib, "rating:1"), ["IMG_0001.CR3"]);
        // Not retried while the file stands.
        let report = lib.index_folder(&dir, &mut |_| {}).unwrap();
        assert!(report.errors.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tree_is_indexed_folder_by_folder_and_hidden_folders_are_not() {
        let dir = scratch("tree");
        shoot(&dir);
        let sub = dir.join("day2");
        std::fs::create_dir(&sub).unwrap();
        write_frame(&sub.join("x.tif"), &A7, 7);
        // The hidden sidecar folder holds a .gcd, and a hidden
        // folder with a picture in it is somebody else's business.
        let hidden = dir.join(".cache");
        std::fs::create_dir(&hidden).unwrap();
        write_frame(&hidden.join("thumb.tif"), &A7, 8);
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_tree(&dir, &mut |_| {}).unwrap();
        assert_eq!(report.added, 4, "{report:?}");
        assert_eq!(lib.folders().unwrap().len(), 2);
        assert_eq!(names(&lib, "camera:sony"), ["a7.tif", "x.tif"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_library_on_disk_keeps_what_it_indexed() {
        let dir = scratch("disk");
        shoot(&dir);
        let db = dir.join("lib").join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_folder(&dir, &mut |_| {}).unwrap();
            lib.checkpoint().unwrap();
            assert!(lib.size_on_disk().unwrap() > 0);
        }
        let lib = Library::open(&db).unwrap();
        assert_eq!(lib.len().unwrap(), 3);
        assert_eq!(names(&lib, "flag:pick"), ["r5.tif"]);
        assert_eq!(lib.path(), db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The sample raws, when they are to hand: `GREYCARD_SAMPLES`
    /// or the workspace's `target/work/samples`. Real cameras' EXIF
    /// through the raw probe. Skipped, and says so, when neither is
    /// there.
    #[test]
    fn the_sample_raws_index_with_their_exif() {
        let samples = std::env::var_os("GREYCARD_SAMPLES")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/work/samples")
            });
        let Ok(files) = list_folder(&samples) else {
            eprintln!(
                "no sample raws at {}; set GREYCARD_SAMPLES to a folder of them",
                samples.display()
            );
            return;
        };
        let raws: Vec<&PathBuf> = files
            .iter()
            .filter(|p| greycard_core::decode::is_raw_path(p))
            .collect();
        if raws.is_empty() {
            eprintln!("no raws under {}", samples.display());
            return;
        }
        let mut lib = Library::open_in_memory().unwrap();
        let report = lib.index_folder(&samples, &mut |_| {}).unwrap();
        assert_eq!(report.added, files.len(), "{report:?}");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        for raw in &raws {
            let e = lib.by_path(raw).unwrap().unwrap();
            assert!(!e.exif.camera.is_empty(), "{}: no camera", raw.display());
            assert!(e.exif.iso.is_some(), "{}: no ISO", raw.display());
            assert!(e.exif.shutter.is_some(), "{}: no shutter", raw.display());
            assert!(e.exif.aperture.is_some(), "{}: no aperture", raw.display());
            assert!(e.exif.focal.is_some(), "{}: no focal length", raw.display());
            let taken = e
                .exif
                .taken
                .as_deref()
                .unwrap_or_else(|| panic!("{}: no date", raw.display()));
            assert!(
                taken.len() >= 10 && &taken[4..5] == "-",
                "{}: {taken}",
                raw.display()
            );
        }
        // A lens is not every body's to say (a Panasonic RW2 in the
        // samples names none), but most do.
        let with_lens = lib
            .query(&Filter::default())
            .unwrap()
            .iter()
            .filter(|e| e.exif.lens.is_some())
            .count();
        assert!(
            with_lens * 2 > raws.len(),
            "{with_lens} of {} lenses",
            raws.len()
        );
        // Every raw answers some camera filter, and the filters
        // partition the folder.
        let all = lib.count(&Filter::default()).unwrap();
        let canon = lib.count(&Filter::parse("make:canon").unwrap()).unwrap();
        let not_canon = lib.count(&Filter::parse("make!=canon").unwrap()).unwrap();
        assert_eq!(canon + not_canon, all);
        // A picture without EXIF has no ISO and answers neither side.
        let low = lib.count(&Filter::parse("iso<800").unwrap()).unwrap();
        let high = lib.count(&Filter::parse("iso>=800").unwrap()).unwrap();
        let with_iso = lib.count(&Filter::parse("iso>=0").unwrap()).unwrap();
        assert_eq!(low + high, with_iso);
        assert!(with_iso >= raws.len());
        let report = lib.index_folder(&samples, &mut |_| {}).unwrap();
        assert_eq!(report.unchanged, files.len(), "{report:?}");
    }
}
