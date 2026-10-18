//! The order a list of frames is read in: by when each was taken, by
//! name, by rating, or by when its develop was last saved, either way
//! up. One rule, in two spellings that agree: the `ORDER BY` a query
//! of the index is listed by ([`Order::sql`]), and the comparison a
//! list made elsewhere (a folder read from the disk, a collection's
//! references found one by one) is sorted by ([`Order::compare`]).
//!
//! Ties break by the file's name, then its path, whichever way the
//! order runs, so a burst taken within one second reads in the
//! camera's numbering and every pass over the same rows gives the same
//! list. A frame with no value for the order (no capture time in its
//! EXIF, no develop) goes last both ways up: it is neither the oldest
//! nor the newest.

use std::cmp::Ordering;
use std::path::Path;

/// What a list is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortBy {
    /// When the frame was taken, as [`capture_time`] reads it.
    #[default]
    Taken,
    /// The file's name.
    Name,
    /// The stars the sidecar gives it, none counted as nought.
    Rating,
    /// When its develop was last saved: the sidecar's last write, as
    /// the index last saw it, for a frame whose sidecar holds a
    /// develop (the row's `edited` mark); none for one that holds none.
    Edited,
}

impl SortBy {
    pub const ALL: [SortBy; 4] = [SortBy::Taken, SortBy::Name, SortBy::Rating, SortBy::Edited];

    /// The name a settings file, a command line and the editor's menu
    /// share.
    pub fn key(self) -> &'static str {
        match self {
            SortBy::Taken => "capture",
            SortBy::Name => "name",
            SortBy::Rating => "rating",
            SortBy::Edited => "edited",
        }
    }

    pub fn from_key(key: &str) -> Option<SortBy> {
        SortBy::ALL.into_iter().find(|s| s.key() == key)
    }

    /// The words for it on a menu.
    pub fn words(self) -> &'static str {
        match self {
            SortBy::Taken => "Capture time",
            SortBy::Name => "Name",
            SortBy::Rating => "Rating",
            SortBy::Edited => "Date edited",
        }
    }
}

/// An order: what by, and which way up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Order {
    pub by: SortBy,
    pub descending: bool,
}

/// What a frame is sorted by, borrowed from wherever the list holds
/// it: an index row, or a frame the list has no row for (every value
/// none but its path).
#[derive(Debug, Clone, Copy)]
pub struct SortKey<'a> {
    pub path: &'a Path,
    /// The index's `taken`, as [`capture_time`] reads it.
    pub taken: Option<&'a str>,
    pub rating: u8,
    /// See [`SortBy::Edited`]: none for a frame with no develop.
    pub edited_at: Option<i64>,
}

impl<'a> SortKey<'a> {
    /// A frame the list knows nothing of but its path: it goes after
    /// every frame with a value, by name.
    pub fn bare(path: &'a Path) -> SortKey<'a> {
        SortKey {
            path,
            taken: None,
            rating: 0,
            edited_at: None,
        }
    }
}

/// When a frame was taken, from the index's `taken` column (the EXIF's
/// date, normalized so it sorts as text). Every reader of a capture
/// time for an order goes through here and through
/// [`CAPTURE_TIME_SQL`], so a shift of the capture time kept in the
/// sidecar's meta (a camera's clock set wrong, or a second body
/// brought into line with the first) can later be laid over it in this
/// one place, on both spellings, rather than in every list.
pub fn capture_time(taken: Option<&str>) -> Option<&str> {
    taken
}

/// [`capture_time`] in SQL, over a `files` row.
pub const CAPTURE_TIME_SQL: &str = "files.taken";

/// [`SortBy::Edited`]'s value in SQL, over a `files` row.
pub const EDITED_AT_SQL: &str = "CASE WHEN files.edited <> 0 THEN files.sidecar_mtime END";

impl Order {
    pub const fn new(by: SortBy, descending: bool) -> Order {
        Order { by, descending }
    }

    /// By name, A to Z: within one folder, the camera's numbering.
    pub const NAME: Order = Order::new(SortBy::Name, false);
    /// By capture time, the oldest first.
    pub const TAKEN: Order = Order::new(SortBy::Taken, false);

    /// The `ORDER BY` body over the `files` table: the order's value
    /// that way up (nulls last either way), then the name and the path
    /// A to Z. The name's text and the path's bytes compare as
    /// [`Order::compare`] compares them.
    pub fn sql(self) -> String {
        let dir = if self.descending { "DESC" } else { "ASC" };
        match self.by {
            SortBy::Taken => {
                format!("{CAPTURE_TIME_SQL} {dir} NULLS LAST, files.name, files.path")
            }
            SortBy::Name => format!("files.name {dir}, files.path"),
            SortBy::Rating => format!("files.rating {dir}, files.name, files.path"),
            SortBy::Edited => format!("{EDITED_AT_SQL} {dir} NULLS LAST, files.name, files.path"),
        }
    }

    /// The same order as [`Order::sql`], for two frames in hand.
    pub fn compare(self, a: &SortKey<'_>, b: &SortKey<'_>) -> Ordering {
        let first = match self.by {
            SortBy::Taken => nulls_last(
                capture_time(a.taken),
                capture_time(b.taken),
                self.descending,
            ),
            SortBy::Name => way(name_of(a.path).cmp(&name_of(b.path)), self.descending),
            SortBy::Rating => way(a.rating.cmp(&b.rating), self.descending),
            SortBy::Edited => nulls_last(a.edited_at, b.edited_at, self.descending),
        };
        first
            .then_with(|| name_of(a.path).cmp(&name_of(b.path)))
            .then_with(|| crate::path_bytes(a.path).cmp(&crate::path_bytes(b.path)))
    }

    /// The name a settings file and a command line know it by:
    /// `capture`, or `capture-desc` the other way up.
    pub fn key(self) -> String {
        if self.descending {
            format!("{}-desc", self.by.key())
        } else {
            self.by.key().to_string()
        }
    }

    /// [`Order::key`] read back.
    pub fn from_key(key: &str) -> Option<Order> {
        let (by, descending) = match key.strip_suffix("-desc") {
            Some(by) => (by, true),
            None => (key, false),
        };
        Some(Order::new(SortBy::from_key(by)?, descending))
    }
}

/// A file's name as the index's `name` column holds it.
fn name_of(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default()
}

fn way(o: Ordering, descending: bool) -> Ordering {
    if descending { o.reverse() } else { o }
}

/// Two values that may be missing, the missing after every value
/// whichever way the values run.
fn nulls_last<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => way(x.cmp(&y), descending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_reads_back() {
        for by in SortBy::ALL {
            for descending in [false, true] {
                let o = Order::new(by, descending);
                assert_eq!(Order::from_key(&o.key()), Some(o));
            }
        }
        assert_eq!(
            Order::from_key("capture-desc"),
            Some(Order::new(SortBy::Taken, true))
        );
        assert_eq!(Order::from_key("size"), None);
        assert_eq!(Order::from_key("-desc"), None);
    }

    #[test]
    fn the_missing_go_last_both_ways_up() {
        let (a, b, c) = (Path::new("/x/a"), Path::new("/x/b"), Path::new("/x/c"));
        let keys = [
            SortKey::bare(c),
            SortKey {
                taken: Some("2026-01-02 10:00:00"),
                ..SortKey::bare(b)
            },
            SortKey {
                taken: Some("2026-01-01 10:00:00"),
                ..SortKey::bare(a)
            },
        ];
        let sorted = |o: Order| {
            let mut k = keys.to_vec();
            k.sort_by(|x, y| o.compare(x, y));
            k.iter().map(|k| k.path).collect::<Vec<_>>()
        };
        assert_eq!(sorted(Order::TAKEN), [a, b, c]);
        assert_eq!(sorted(Order::new(SortBy::Taken, true)), [b, a, c]);
    }

    /// A row put straight into an index, as a pass would have left it.
    struct Row {
        path: &'static str,
        taken: Option<&'static str>,
        rating: i64,
        edited: bool,
        sidecar_mtime: Option<i64>,
    }

    /// A test path spelled with the platform's separator, so the
    /// folder prefix the query appends matches the rows on Windows too.
    fn sep(path: &str) -> String {
        path.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    /// A row's path back in the test's spelling.
    fn unsep(path: &Path) -> String {
        path.display()
            .to_string()
            .replace(std::path::MAIN_SEPARATOR, "/")
    }

    fn put(lib: &crate::Library, rows: &[Row]) {
        let mut stmt = lib
            .conn
            .prepare(
                "INSERT INTO files (path, folder, folder_text, name, size, mtime, hash, taken, \
                 rating, edited, sidecar_mtime) VALUES (?, ?, ?, ?, 0, 0, ?, ?, ?, ?, ?)",
            )
            .unwrap();
        for r in rows {
            let spelled = sep(r.path);
            let path = Path::new(&spelled);
            let folder = path.parent().unwrap();
            stmt.execute(rusqlite::params![
                crate::path_bytes(path),
                crate::path_bytes(folder),
                folder.to_string_lossy(),
                path.file_name().unwrap().to_string_lossy(),
                r.path,
                r.taken,
                r.rating,
                r.edited as i64,
                r.sidecar_mtime,
            ])
            .unwrap();
        }
    }

    /// Two bodies' files in two folders, their letter prefixes apart
    /// and their capture times interleaved; a burst within one second,
    /// one name in both folders, a frame with no date, frames rated and
    /// never developed, and developed ones whose sidecars were written
    /// at different times.
    fn fixture() -> crate::Library {
        let lib = crate::Library::open_in_memory().unwrap();
        let row = |path, taken, rating, edited, sidecar_mtime| Row {
            path,
            taken,
            rating,
            edited,
            sidecar_mtime,
        };
        put(
            &lib,
            &[
                row(
                    "/p/b/_DSC0002.ARW",
                    Some("2026-05-01 10:00:02"),
                    3,
                    true,
                    Some(50),
                ),
                row(
                    "/p/a/IMG_0001.CR3",
                    Some("2026-05-01 10:00:01"),
                    0,
                    false,
                    Some(90),
                ),
                row(
                    "/p/a/IMG_0003.CR3",
                    Some("2026-05-01 10:00:03"),
                    5,
                    true,
                    Some(70),
                ),
                row(
                    "/p/b/_DSC0001.ARW",
                    Some("2026-05-01 10:00:00"),
                    3,
                    false,
                    None,
                ),
                // A burst: one second, three frames, the camera's
                // numbering their order.
                row(
                    "/p/a/IMG_0005.CR3",
                    Some("2026-05-01 10:00:05"),
                    1,
                    false,
                    None,
                ),
                row(
                    "/p/a/IMG_0004.CR3",
                    Some("2026-05-01 10:00:05"),
                    1,
                    false,
                    None,
                ),
                row(
                    "/p/b/_DSC0003.ARW",
                    Some("2026-05-01 10:00:05"),
                    1,
                    true,
                    Some(10),
                ),
                // No date in its EXIF.
                row("/p/b/scan.tif", None, 2, false, None),
                // One name in both folders.
                row(
                    "/p/b/IMG_0001.CR3",
                    Some("2026-05-02 09:00:00"),
                    0,
                    true,
                    Some(30),
                ),
            ],
        );
        lib
    }

    fn names(rows: &[(std::path::PathBuf, crate::RowMeta)]) -> Vec<String> {
        rows.iter().map(|(p, _)| unsep(p)).collect()
    }

    #[test]
    fn each_order_lists_the_index_as_its_comparison_sorts() {
        let lib = fixture();
        let roots = [std::path::PathBuf::from(sep("/p"))];
        for by in SortBy::ALL {
            for descending in [false, true] {
                let order = Order::new(by, descending);
                let listed = lib.rows_under_canonical(&roots, order).unwrap();
                let mut sorted = listed.clone();
                sorted.reverse();
                sorted.sort_by(|a, b| order.compare(&a.1.sort_key(&a.0), &b.1.sort_key(&b.0)));
                assert_eq!(names(&listed), names(&sorted), "{order:?}");
                let entries = lib.query_sorted(&crate::Filter::default(), order).unwrap();
                let mut by_entry = entries.clone();
                by_entry.reverse();
                by_entry.sort_by(|a, b| order.compare(&a.sort_key(), &b.sort_key()));
                let paths =
                    |e: &[crate::Entry]| e.iter().map(|e| unsep(&e.path)).collect::<Vec<_>>();
                assert_eq!(
                    paths(&entries),
                    names(&listed),
                    "{order:?}: the query and the rows"
                );
                assert_eq!(
                    paths(&by_entry),
                    names(&listed),
                    "{order:?}: an entry's key"
                );
            }
        }
    }

    #[test]
    fn capture_time_runs_across_folders_with_the_burst_by_name_and_the_undated_last() {
        let lib = fixture();
        let roots = [std::path::PathBuf::from(sep("/p"))];
        let listed = lib.rows_under_canonical(&roots, Order::TAKEN).unwrap();
        assert_eq!(
            names(&listed),
            [
                "/p/b/_DSC0001.ARW",
                "/p/a/IMG_0001.CR3",
                "/p/b/_DSC0002.ARW",
                "/p/a/IMG_0003.CR3",
                "/p/a/IMG_0004.CR3",
                "/p/a/IMG_0005.CR3",
                "/p/b/_DSC0003.ARW",
                "/p/b/IMG_0001.CR3",
                "/p/b/scan.tif",
            ]
        );
        let newest = lib
            .rows_under_canonical(&roots, Order::new(SortBy::Taken, true))
            .unwrap();
        // Newest first, the burst still in the camera's numbering, the
        // undated still last.
        assert_eq!(
            names(&newest),
            [
                "/p/b/IMG_0001.CR3",
                "/p/a/IMG_0004.CR3",
                "/p/a/IMG_0005.CR3",
                "/p/b/_DSC0003.ARW",
                "/p/a/IMG_0003.CR3",
                "/p/b/_DSC0002.ARW",
                "/p/a/IMG_0001.CR3",
                "/p/b/_DSC0001.ARW",
                "/p/b/scan.tif",
            ]
        );
    }

    #[test]
    fn name_rating_and_date_edited_break_ties_by_name_then_path() {
        let lib = fixture();
        let roots = [std::path::PathBuf::from(sep("/p"))];
        // One name in two folders: the path decides.
        let by_name = names(&lib.rows_under_canonical(&roots, Order::NAME).unwrap());
        assert_eq!(&by_name[..2], ["/p/a/IMG_0001.CR3", "/p/b/IMG_0001.CR3"]);
        assert_eq!(by_name.last().unwrap(), "/p/b/scan.tif");
        let by_rating = names(
            &lib.rows_under_canonical(&roots, Order::new(SortBy::Rating, true))
                .unwrap(),
        );
        assert_eq!(
            by_rating,
            [
                "/p/a/IMG_0003.CR3",
                "/p/b/_DSC0001.ARW",
                "/p/b/_DSC0002.ARW",
                "/p/b/scan.tif",
                "/p/a/IMG_0004.CR3",
                "/p/a/IMG_0005.CR3",
                "/p/b/_DSC0003.ARW",
                "/p/a/IMG_0001.CR3",
                "/p/b/IMG_0001.CR3",
            ]
        );
        // Only a develop has a date edited: a sidecar written for a
        // rating alone is no edit, and those go last, by name.
        let edited = names(
            &lib.rows_under_canonical(&roots, Order::new(SortBy::Edited, true))
                .unwrap(),
        );
        assert_eq!(
            edited,
            [
                "/p/a/IMG_0003.CR3",
                "/p/b/_DSC0002.ARW",
                "/p/b/IMG_0001.CR3",
                "/p/b/_DSC0003.ARW",
                "/p/a/IMG_0001.CR3",
                "/p/a/IMG_0004.CR3",
                "/p/a/IMG_0005.CR3",
                "/p/b/_DSC0001.ARW",
                "/p/b/scan.tif",
            ]
        );
        // One folder alone by name is the camera's numbering.
        let one = names(
            &lib.rows_in_canonical(Path::new("/p/a"), Order::NAME)
                .unwrap(),
        );
        assert_eq!(
            one,
            [
                "/p/a/IMG_0001.CR3",
                "/p/a/IMG_0003.CR3",
                "/p/a/IMG_0004.CR3",
                "/p/a/IMG_0005.CR3",
            ]
        );
    }

    /// The all-roots query by capture time against the same by name,
    /// over 100,000 rows in 200 folders: within the same order of time,
    /// with the index on `taken` and without it. Printed for the notes;
    /// asserted loosely, since a loaded machine moves both.
    #[test]
    fn capture_time_over_a_hundred_thousand_rows_costs_about_what_name_does() {
        let lib = crate::Library::open_in_memory().unwrap();
        {
            let tx = lib.conn.unchecked_transaction().unwrap();
            let mut stmt = tx
                .prepare(
                    "INSERT INTO files (path, folder, folder_text, name, size, mtime, hash, \
                     taken, rating) VALUES (?, ?, ?, ?, 0, 0, ?, ?, ?)",
                )
                .unwrap();
            for i in 0..100_000u32 {
                let folder = sep(&format!(
                    "/archive/{:04}/shoot{:03}",
                    2000 + i % 25,
                    i % 200
                ));
                let name = format!("{}_{:05}.CR3", if i % 2 == 0 { "IMG" } else { "_DSC" }, i);
                let path = format!("{folder}{}{name}", std::path::MAIN_SEPARATOR);
                // Scattered, so the taken order is no other order; one
                // in fifty undated.
                let s = i.wrapping_mul(2_654_435_761) % 31_536_000;
                let taken = (i % 50 != 0).then(|| {
                    format!(
                        "2025-{:02}-{:02} {:02}:{:02}:{:02}",
                        1 + s / 2_628_000 % 12,
                        1 + s / 86_400 % 28,
                        s / 3600 % 24,
                        s / 60 % 60,
                        s % 60
                    )
                });
                stmt.execute(rusqlite::params![
                    crate::path_bytes(Path::new(&path)),
                    crate::path_bytes(Path::new(&folder)),
                    folder,
                    name,
                    path,
                    taken,
                    i % 6
                ])
                .unwrap();
            }
            drop(stmt);
            tx.commit().unwrap();
        }
        lib.conn.execute_batch("ANALYZE").unwrap();
        let roots = [std::path::PathBuf::from(sep("/archive"))];
        let time = |order: Order| {
            // The first read warms the statement's cache; the best of
            // three after it is what is kept.
            let n = lib.rows_under_canonical(&roots, order).unwrap().len();
            assert_eq!(n, 100_000);
            (0..3)
                .map(|_| {
                    let t = std::time::Instant::now();
                    lib.rows_under_canonical(&roots, order).unwrap();
                    t.elapsed().as_secs_f64() * 1e3
                })
                .fold(f64::INFINITY, f64::min)
        };
        // The sort alone: the paths in each order, nothing else read.
        let sorted = |order: Order| {
            let sql = format!(
                "SELECT files.path FROM files WHERE folder >= ? AND folder < ? ORDER BY {}",
                order.sql()
            );
            (0..3)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let n = lib
                        .conn
                        .prepare_cached(&sql)
                        .unwrap()
                        .query_map(
                            rusqlite::params![&b"/archive/"[..], &b"/archive0"[..]],
                            |r| r.get::<_, Vec<u8>>(0),
                        )
                        .unwrap()
                        .count();
                    assert_eq!(n, 100_000);
                    t.elapsed().as_secs_f64() * 1e3
                })
                .fold(f64::INFINITY, f64::min)
        };
        // How SQLite answers each, for the notes.
        let plan = |order: Order| -> String {
            let sql = format!(
                "EXPLAIN QUERY PLAN SELECT files.path FROM files WHERE folder >= ? AND \
                 folder < ? ORDER BY {}",
                order.sql()
            );
            let mut stmt = lib.conn.prepare(&sql).unwrap();
            let rows = stmt
                .query_map(
                    rusqlite::params![&b"/archive/"[..], &b"/archive0"[..]],
                    |r| r.get::<_, String>(3),
                )
                .unwrap();
            rows.map(|r| r.unwrap()).collect::<Vec<_>>().join("; ")
        };
        eprintln!(
            "plan by name: {}; by capture time: {}",
            plan(Order::NAME),
            plan(Order::TAKEN)
        );
        let name = time(Order::NAME);
        let taken = time(Order::TAKEN);
        let (name_alone, taken_alone) = (sorted(Order::NAME), sorted(Order::TAKEN));
        let folder = {
            let t = std::time::Instant::now();
            lib.paths_under_canonical(&roots).unwrap();
            t.elapsed().as_secs_f64() * 1e3
        };
        lib.conn.execute_batch("DROP INDEX files_taken").unwrap();
        let bare = time(Order::TAKEN);
        let bare_alone = sorted(Order::TAKEN);
        eprintln!(
            "100,000 rows, all roots, with each row's meta: by name {name:.1} ms, by capture \
             time {taken:.1} ms ({bare:.1} ms without the index on taken). The paths alone: \
             by name {name_alone:.1} ms, by capture time {taken_alone:.1} ms ({bare_alone:.1} \
             ms without the index); by folder then name, as before, {folder:.1} ms"
        );
        assert!(taken < name * 10.0 + 50.0, "{taken} ms against {name} ms");
        assert!(bare < name * 10.0 + 50.0, "{bare} ms against {name} ms");
    }
}
