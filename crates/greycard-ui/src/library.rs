//! The library index as the editor keeps it (notes §160): the open
//! folder indexed on a thread of its own, each sidecar write's row
//! brought up to date after it, and the filter bar's EXIF questions
//! answered from it.
//!
//! Two connections. The indexer's thread holds the one that writes:
//! `index_folder` when a folder opens and `index_file` after a save,
//! neither on the UI thread, so the first frame never waits for a
//! pass. The UI thread holds one opened read-only once the indexer
//! has made the file, and asks it the filter's questions — which
//! frames pass the EXIF tests, and each facet's `GROUP BY` — which
//! under write-ahead logging never waits for the writer. A folder of
//! a few thousand frames is milliseconds a question.
//!
//! The database is the user's, `Library::user_path()`, unless
//! `--library` names another. A test never starts an indexer: it
//! opens a library of its own in its own directory, and the state it
//! builds has none.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use greycard_library::{FacetCount, Library, Report};

use crate::filter::{self, Facet};
use crate::*;

/// How often a long pass tells the window how far it has got, so a
/// facet fills in and a frame the filter does not want goes, while
/// the pass is still running.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// The most chips a facet's row offers; the chips on are kept past
/// it. A zoom's focal lengths or a day of ISOs can run to dozens,
/// and the rest are a term away in the text field (`focal:70`).
pub(crate) const CHIPS_A_FACET: usize = 12;

/// What the window asks of the indexer.
enum Ask {
    /// Index these folders, the generation saying which open asked.
    Folders { dirs: Vec<PathBuf>, generation: u64 },
    /// Bring this file's row up to date after its sidecar was saved.
    File(PathBuf),
}

/// What the indexer tells the window.
#[derive(Debug)]
pub(crate) enum Told {
    /// The library is open, at this path; the window may read it.
    Opened(PathBuf),
    /// It could not be opened, and nothing will be indexed.
    Failed(String),
    /// A pass is `done` files of `total` into the folders asked for.
    Progress {
        generation: u64,
        done: usize,
        total: usize,
    },
    /// The folders asked for are indexed.
    Indexed {
        generation: u64,
        report: Report,
        seconds: f64,
    },
    /// Rows after saves are up to date.
    FilesIndexed,
}

/// The indexer's thread, and the way to ask it things.
pub(crate) struct Indexer {
    asks: mpsc::Sender<Ask>,
}

impl Indexer {
    /// Open the library at `path` on a thread of its own and wait
    /// there for folders and files. `told` is called on that thread.
    pub(crate) fn start(path: PathBuf, told: impl Fn(Told) + Send + 'static) -> Indexer {
        let (asks, waiting) = mpsc::channel();
        std::thread::Builder::new()
            .name("greycard index".into())
            .spawn(move || run(path, waiting, told))
            .expect("spawning the indexer");
        Indexer { asks }
    }

    pub(crate) fn folders(&self, dirs: Vec<PathBuf>, generation: u64) {
        let _ = self.asks.send(Ask::Folders { dirs, generation });
    }

    pub(crate) fn file(&self, path: PathBuf) {
        let _ = self.asks.send(Ask::File(path));
    }
}

fn run(path: PathBuf, waiting: mpsc::Receiver<Ask>, told: impl Fn(Told)) {
    let mut lib = match Library::open(&path) {
        Ok(lib) => lib,
        Err(e) => {
            told(Told::Failed(format!("{}: {e}", path.display())));
            return;
        }
    };
    told(Told::Opened(path));
    while let Ok(first) = waiting.recv() {
        // Everything waiting, at once: the files deduplicated, and
        // only the newest folders asked for, since a folder the
        // window has already left is nobody's question.
        let mut files: Vec<PathBuf> = Vec::new();
        let mut folders = None;
        for ask in std::iter::once(first).chain(waiting.try_iter()) {
            match ask {
                Ask::File(p) => {
                    if !files.contains(&p) {
                        files.push(p);
                    }
                }
                Ask::Folders { dirs, generation } => folders = Some((dirs, generation)),
            }
        }
        if !files.is_empty() {
            for f in &files {
                if let Err(e) = lib.index_file(f) {
                    tracing::warn!("index: {}: {e}", f.display());
                }
            }
            told(Told::FilesIndexed);
        }
        let Some((dirs, generation)) = folders else {
            continue;
        };
        let started = Instant::now();
        let mut report = Report::default();
        let mut last = Instant::now();
        let mut before = 0;
        for dir in &dirs {
            let passed = lib.index_folder(dir, &mut |p| {
                if last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    told(Told::Progress {
                        generation,
                        done: before + p.done,
                        total: before + p.total,
                    });
                }
            });
            match passed {
                Ok(r) => {
                    before += r.seen();
                    report.added += r.added;
                    report.moved += r.moved;
                    report.changed += r.changed;
                    report.meta_refreshed += r.meta_refreshed;
                    report.unchanged += r.unchanged;
                    report.returned += r.returned;
                    report.missing += r.missing;
                    report.unavailable.extend(r.unavailable);
                    report.errors.extend(r.errors);
                }
                Err(e) => tracing::warn!("index: {}: {e}", dir.display()),
            }
        }
        told(Told::Indexed {
            generation,
            report,
            seconds: started.elapsed().as_secs_f64(),
        });
    }
}

/// The folders a list of files is in, each once, in the order first
/// met.
pub(crate) fn folders_of(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for f in files {
        let dir = match f.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

/// Ask the indexer, if there is one, for the folders of the files
/// now open. The files' rows as the index already holds them are
/// read at once: a folder indexed last week has its facets before
/// this pass has looked at a file.
pub(crate) fn index_open_folder(st: &mut State) {
    refresh_ids(st);
    let Some(indexer) = &st.index else {
        return;
    };
    st.index_generation += 1;
    indexer.folders(folders_of(&st.files), st.index_generation);
}

/// A frame's sidecar was written: its row is brought up to date on
/// the indexer's thread, and the window hears when it is.
pub(crate) fn sidecar_written(st: &State, file: usize) {
    if let (Some(indexer), Some(path)) = (&st.index, st.files.get(file)) {
        indexer.file(path.clone());
    }
}

/// Each file's row id, as the index holds it now; `None` for a file
/// it has no row for yet, and for every file while there is no index.
pub(crate) fn refresh_ids(st: &mut State) {
    let started = Instant::now();
    let Some(lib) = &st.index_reader else {
        st.index_ids = vec![None; st.files.len()];
        return;
    };
    st.index_ids = lib.ids_of(&st.files).unwrap_or_else(|e| {
        tracing::warn!("index: {e}");
        vec![None; st.files.len()]
    });
    tracing::debug!(
        "index: {} of {} frames have rows, read in {:.1} ms",
        st.index_ids.iter().flatten().count(),
        st.files.len(),
        started.elapsed().as_secs_f64() * 1e3
    );
}

/// Which frames the index's tests pass, as [`filter::Frame::index`]
/// reads them: every frame when nothing is asked of the index, or
/// there is none; else a frame with a row passes when the row does,
/// and a frame with none passes until it has one.
pub(crate) fn index_pass(st: &State) -> Vec<bool> {
    let typed = st.filter.typed();
    let everyone = vec![true; st.files.len()];
    let Some(lib) = &st.index_reader else {
        return everyone;
    };
    if !st.filter.asks_index(&typed) || st.index_ids.len() != st.files.len() {
        return everyone;
    }
    let started = Instant::now();
    let ids: Vec<i64> = st.index_ids.iter().flatten().copied().collect();
    match lib.ids_passing(&ids, &st.filter.index_filter(&typed, None)) {
        Ok(passing) => {
            tracing::debug!(
                "index: {} of {} rows pass in {:.1} ms",
                passing.len(),
                ids.len(),
                started.elapsed().as_secs_f64() * 1e3
            );
            st.index_ids
                .iter()
                .map(|id| id.is_none_or(|id| passing.contains(&id)))
                .collect()
        }
        Err(e) => {
            tracing::warn!("index: {e}");
            everyone
        }
    }
}

/// Every facet's chips: each value among the frames the filter's
/// other tests leave, with its count, one `GROUP BY` a facet — the
/// rule the meta rows keep (`filter::Counts`), asked of the index.
/// The tests the sidecars answer pick the rows (`within`); the
/// index's own tests, less the facet's, are the query's filter.
pub(crate) fn facet_counts(st: &State) -> Vec<(Facet, Vec<FacetCount>)> {
    let Some(lib) = &st.index_reader else {
        return Vec::new();
    };
    if st.index_ids.len() != st.files.len() {
        return Vec::new();
    }
    let typed = st.filter.typed();
    let within = |keyword: bool| -> Vec<i64> {
        st.files
            .iter()
            .zip(&st.sidecars)
            .zip(&st.index_ids)
            .filter_map(|((path, s), id)| {
                let frame = filter::Frame {
                    path,
                    meta: &s.meta,
                    index: true,
                };
                id.filter(|_| st.filter.shows_meta(&typed, frame, keyword))
            })
            .collect()
    };
    let all_meta = within(true);
    let mut rows = Vec::new();
    for facet in Facet::ALL {
        let (ids, asked) = if facet == Facet::Keyword {
            (within(false), st.filter.index_filter(&typed, None))
        } else {
            (
                all_meta.clone(),
                st.filter.index_filter(&typed, Some(facet)),
            )
        };
        match lib.facet_counts(facet, Some(&ids), &asked) {
            Ok(counts) => rows.push((facet, counts)),
            Err(e) => tracing::warn!("index: {facet:?}: {e}"),
        }
    }
    rows
}

/// A facet's chips as the bar shows them: at most
/// [`CHIPS_A_FACET`], the most held first, the chips on kept
/// whatever their count — a chip on with nothing left under it is
/// how the filter is undone — and then in the facet's own order.
pub(crate) fn chips_of(facet: Facet, counts: &[FacetCount], on: &[String]) -> Vec<FacetChip> {
    let mut kept: Vec<&FacetCount> = counts.iter().collect();
    if kept.len() > CHIPS_A_FACET {
        let mut by_count = kept.clone();
        by_count.sort_by_key(|c| std::cmp::Reverse(c.count));
        let cut = by_count[CHIPS_A_FACET - 1].count;
        let mut room = CHIPS_A_FACET;
        kept.retain(|c| {
            let keep = on.contains(&c.value) || (c.count >= cut && room > 0);
            if keep && !on.contains(&c.value) {
                room -= 1;
            }
            keep
        });
    }
    let mut chips: Vec<FacetChip> = kept
        .iter()
        .map(|c| FacetChip {
            text: filter::facet_chip_text(facet, &c.label).into(),
            key: c.value.clone().into(),
            count: c.count as i32,
            on: on.contains(&c.value),
        })
        .collect();
    for value in on {
        if !counts.iter().any(|c| c.value == *value) {
            chips.push(FacetChip {
                text: filter::facet_chip_text(facet, value).into(),
                key: value.clone().into(),
                count: 0,
                on: true,
            });
        }
    }
    chips
}

/// The rows the bar shows: a facet with nothing to offer and nothing
/// on is left out.
pub(crate) fn facet_rows(st: &State) -> Vec<FacetRow> {
    let started = Instant::now();
    let counts = facet_counts(st);
    if st.index_reader.is_some() {
        tracing::debug!(
            "index: six facets counted over {} frames in {:.1} ms",
            st.files.len(),
            started.elapsed().as_secs_f64() * 1e3
        );
    }
    counts
        .into_iter()
        .filter_map(|(facet, counts)| {
            let on = st.filter.chosen(facet);
            let chips = chips_of(facet, &counts, on);
            (!chips.is_empty()).then(|| FacetRow {
                name: filter::facet_caption(facet).into(),
                code: filter::facet_slot(facet) as i32,
                chips: ModelRc::new(VecModel::from(chips)),
            })
        })
        .collect()
}

/// What the facets' row says while it has no chips.
pub(crate) fn facet_note(st: &State) -> String {
    match (&st.index_reader, st.index_progress) {
        (_, Some((done, total))) => format!("Indexing the folder: {done} of {total}"),
        (Some(_), None) => "No camera, lens or date in these files' EXIF".to_string(),
        (None, None) if st.index.is_some() => "Opening the library index".to_string(),
        (None, None) => String::new(),
    }
}

/// The facets `--filter` named, chosen once the index has rows to
/// choose them from: a text facet's value by what it contains, a
/// number's by what it equals, among the values the open frames
/// hold. True when something was chosen.
pub(crate) fn choose_wanted(st: &mut State) -> bool {
    let wanted = std::mem::take(&mut st.facets_wanted);
    let Some(lib) = &st.index_reader else {
        return false;
    };
    let ids: Vec<i64> = st.index_ids.iter().flatten().copied().collect();
    let mut chose = false;
    for (facet, needle) in wanted {
        let counts = match lib.facet_counts(facet, Some(&ids), &greycard_library::Filter::default())
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("index: {e}");
                continue;
            }
        };
        let needle_l = needle.to_lowercase();
        let number = |s: &str| s.trim().trim_end_matches("mm").parse::<f64>().ok();
        let hits: Vec<String> = counts
            .into_iter()
            .filter(|c| match facet {
                Facet::Iso | Facet::Focal => {
                    number(&c.value).is_some() && number(&c.value) == number(&needle)
                }
                _ => c.value.to_lowercase().contains(&needle_l),
            })
            .map(|c| c.value)
            .collect();
        if hits.is_empty() {
            tracing::warn!(
                "--filter {}:{needle}: no frame here holds it",
                facet.field()
            );
        }
        for value in hits {
            if !st.filter.chosen(facet).contains(&value) {
                st.filter.toggle_facet(facet, &value);
                chose = true;
            }
        }
    }
    chose
}

/// What the indexer said, on the UI thread.
pub(crate) fn told(app: &App, told: Told) {
    let Some(state) = crate::STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    match told {
        Told::Opened(path) => {
            tracing::info!("library index {}", path.display());
            let reader = Library::open_read_only(&path);
            let mut st = state.borrow_mut();
            match reader {
                Ok(lib) => st.index_reader = Some(lib),
                Err(e) => {
                    tracing::warn!("library index {}: {e}", path.display());
                    st.awaiting_index = false;
                }
            }
            drop(st);
            reread(&state, app, false);
        }
        Told::Failed(message) => {
            tracing::warn!("no library index: {message}");
            let mut st = state.borrow_mut();
            st.index = None;
            st.awaiting_index = false;
            crate::panel::cull::show_filter(&st, app);
            app.window().request_redraw();
        }
        Told::Progress {
            generation,
            done,
            total,
        } => {
            if generation != state.borrow().index_generation {
                return;
            }
            state.borrow_mut().index_progress = Some((done, total));
            reread(&state, app, false);
        }
        Told::Indexed {
            generation,
            report,
            seconds,
        } => {
            if generation != state.borrow().index_generation {
                return;
            }
            tracing::info!(
                "indexed the folder in {seconds:.2} s: {} added, {} moved, {} changed, \
                 {} meta refreshed, {} unchanged, {} missing{}",
                report.added,
                report.moved,
                report.changed,
                report.meta_refreshed,
                report.unchanged,
                report.missing,
                if report.errors.is_empty() {
                    String::new()
                } else {
                    format!(", {} unreadable", report.errors.len())
                }
            );
            for (path, e) in &report.errors {
                tracing::debug!("index: {}: {e}", path.display());
            }
            let mut st = state.borrow_mut();
            st.index_progress = None;
            refresh_ids(&mut st);
            let chose = !st.facets_wanted.is_empty() && choose_wanted(&mut st);
            st.awaiting_index = false;
            drop(st);
            reread(&state, app, chose);
            app.window().request_redraw();
        }
        Told::FilesIndexed => reread(&state, app, false),
    }
}

/// The index moved under the window: the rows read again, and the
/// browser's list rebuilt when the filter asks the index something
/// and the answer changed; otherwise only the chips.
fn reread(state: &Rc<RefCell<State>>, app: &App, changed: bool) {
    let mut st = state.borrow_mut();
    refresh_ids(&mut st);
    let pass = index_pass(&st);
    if changed || pass != st.index_passed {
        drop(st);
        crate::panel::cull::refilter(state, app, changed);
    } else {
        crate::panel::cull::show_filter(&st, app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_library::fixture::{A7, R5, R6, write_frame};

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-library-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A folder of five frames on a library of the test's own: two
    /// R6s, an R5, an A7, and a JPEG-like frame the index has not
    /// reached. The index is written through a connection of its
    /// own, as the indexer's thread would, and read through a
    /// read-only one, as the window does.
    fn folder(dir: &Path) -> (Vec<PathBuf>, Vec<Sidecar>, Library) {
        let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif", "d.tif", "e.tif"]
            .iter()
            .map(|n| dir.join(n))
            .collect();
        write_frame(&files[0], &R6, 1);
        write_frame(&files[1], &R6, 2);
        write_frame(&files[2], &R5, 3);
        write_frame(&files[3], &A7, 4);
        let mut sidecars = vec![Sidecar::default(); files.len()];
        sidecars[0].meta.rating = 3;
        sidecars[0].meta.set_keywords(vec!["Harbor".into()]);
        sidecars[2].meta.rating = 4;
        sidecars[2]
            .meta
            .set_keywords(vec!["harbor".into(), "dusk".into()]);
        sidecars[3].meta.flag = meta::Flag::Pick;
        for (f, s) in files.iter().zip(&mut sidecars).take(4) {
            s.save(f).unwrap();
        }
        let db = dir.join("index").join("library.sqlite");
        let mut writer = Library::open(&db).unwrap();
        writer.index_folder(dir, &mut |_| {}).unwrap();
        // The fifth arrives after the pass: no row yet.
        write_frame(&files[4], &R6, 5);
        drop(writer);
        let reader = Library::open_read_only(&db).unwrap();
        (files, sidecars, reader)
    }

    /// A state with the folder's files, sidecars and index, as the
    /// window would have them after the first pass.
    fn state(app: &App, dir: &Path) -> State {
        let (files, sidecars, reader) = folder(dir);
        let mut st = State::empty(files, app);
        st.sidecars = sidecars;
        st.index_reader = Some(reader);
        refresh_ids(&mut st);
        st
    }

    fn shown(st: &State) -> Vec<usize> {
        let pass = index_pass(st);
        let frames: Vec<filter::Frame> = st
            .files
            .iter()
            .zip(&st.sidecars)
            .zip(&pass)
            .map(|((path, s), &index)| filter::Frame {
                path,
                meta: &s.meta,
                index,
            })
            .collect();
        st.filter.apply(&frames)
    }

    fn counts(st: &State, facet: Facet) -> Vec<(String, usize)> {
        facet_counts(st)
            .into_iter()
            .find(|(f, _)| *f == facet)
            .map(|(_, c)| c.into_iter().map(|c| (c.value, c.count)).collect())
            .unwrap_or_default()
    }

    fn pairs(want: &[(&str, usize)]) -> Vec<(String, usize)> {
        want.iter().map(|(v, n)| (v.to_string(), *n)).collect()
    }

    /// A camera chip is answered from the index, and a frame the
    /// index has not reached yet stays until it has.
    #[test]
    fn a_facet_chip_filters_by_the_index_and_a_frame_without_a_row_stays() {
        let dir = scratch("chip");
        let app = crate::testing::window(5);
        let mut st = state(&app, &dir);
        assert_eq!(st.index_ids.iter().filter(|i| i.is_some()).count(), 4);
        assert_eq!(st.index_ids[4], None, "the fifth has no row");
        assert_eq!(shown(&st), [0, 1, 2, 3, 4]);

        st.filter.toggle_facet(Facet::Camera, "Canon EOS R6m2");
        // The two R6s, and the fifth, which the index cannot yet say
        // is an R6 or not.
        assert_eq!(shown(&st), [0, 1, 4]);
        // Two chips of a facet are either.
        st.filter.toggle_facet(Facet::Camera, "Canon EOS R5");
        assert_eq!(shown(&st), [0, 1, 2, 4]);
        // A second facet narrows.
        st.filter.toggle_facet(Facet::Iso, "100");
        assert_eq!(shown(&st), [2, 4]);

        // Once the fifth has a row the index's word stands.
        let db = st.index_reader.as_ref().unwrap().path().to_path_buf();
        Library::open(&db)
            .unwrap()
            .index_file(&st.files[4])
            .unwrap();
        refresh_ids(&mut st);
        assert!(st.index_ids[4].is_some());
        assert_eq!(shown(&st), [2]);
        st.filter.toggle_facet(Facet::Iso, "100");
        assert_eq!(shown(&st), [0, 1, 2, 4]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A facet's count is how many frames hold each value among the
    /// frames the other groups leave, its own set aside: the meta
    /// rows' rule, and what one chip pressed alone would list.
    #[test]
    fn a_facet_counts_what_the_other_tests_leave_and_its_own_is_set_aside() {
        let dir = scratch("counts");
        let app = crate::testing::window(5);
        let mut st = state(&app, &dir);
        let sony = st
            .index_reader
            .as_ref()
            .unwrap()
            .by_path(&st.files[3])
            .unwrap()
            .unwrap()
            .exif
            .camera;
        assert_eq!(
            counts(&st, Facet::Camera),
            pairs(&[("Canon EOS R6m2", 2), ("Canon EOS R5", 1), (&sony, 1)])
        );
        assert_eq!(
            counts(&st, Facet::Iso),
            pairs(&[("100", 1), ("3200", 1), ("6400", 2)])
        );
        assert_eq!(
            counts(&st, Facet::Keyword),
            pairs(&[("harbor", 2), ("dusk", 1)])
        );

        // Three stars or more: the meta test narrows every facet.
        st.filter.stars = filter::Stars::AtLeast(3);
        assert_eq!(
            counts(&st, Facet::Camera),
            pairs(&[("Canon EOS R5", 1), ("Canon EOS R6m2", 1)])
        );
        st.filter.stars = filter::Stars::default();

        // An R6 chip on: the camera row is unmoved, since its own
        // group is set aside; the others narrow to the R6s.
        st.filter.toggle_facet(Facet::Camera, "Canon EOS R6m2");
        assert_eq!(
            counts(&st, Facet::Camera),
            pairs(&[("Canon EOS R6m2", 2), ("Canon EOS R5", 1), (&sony, 1)])
        );
        assert_eq!(counts(&st, Facet::Iso), pairs(&[("6400", 2)]));
        assert_eq!(counts(&st, Facet::Keyword), pairs(&[("harbor", 1)]));

        // And the rule itself, chip by chip: a count is what that
        // chip alone, the other groups as they stand, would list of
        // the frames the index has rows for.
        st.filter.stars = filter::Stars::AtLeast(1);
        for facet in Facet::ALL {
            let rows = facet_counts(&st);
            let (_, chips) = rows.iter().find(|(f, _)| *f == facet).unwrap();
            for chip in chips {
                let mut alone = st.filter.clone();
                alone.facets[filter::facet_slot(facet)] = vec![chip.value.clone()];
                let saved = std::mem::replace(&mut st.filter, alone);
                let listed = shown(&st)
                    .into_iter()
                    .filter(|&i| st.index_ids[i].is_some())
                    .count();
                st.filter = saved;
                assert_eq!(listed, chip.count, "{facet:?} {}", chip.value);
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The text field takes the §160 grammar: an EXIF term goes to
    /// the index, a meta term is answered from the sidecar, and a
    /// word is the word it always was.
    #[test]
    fn a_typed_term_is_answered_where_it_can_be() {
        let dir = scratch("typed");
        let app = crate::testing::window(5);
        let mut st = state(&app, &dir);
        st.filter.text = "camera:R6".into();
        assert_eq!(shown(&st), [0, 1, 4]);
        st.filter.text = "camera:R6 rating>=3".into();
        assert_eq!(shown(&st), [0]);
        st.filter.text = "iso>=3200 harbor".into();
        assert_eq!(shown(&st), [0]);
        // The sidecar's word, not the row's: a rating set in memory
        // counts before any index_file has run.
        st.sidecars[1].meta.rating = 5;
        st.filter.text = "rating=5".into();
        assert_eq!(shown(&st), [1]);
        // A term that does not parse is the word it was, and says so.
        st.filter.text = "rating>=9".into();
        assert_eq!(st.filter.typed().errors.len(), 1);
        assert_eq!(shown(&st), Vec::<usize>::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The indexer's thread: it makes the library, indexes the folders
    /// asked for, and brings a row up to date after a save, saying
    /// each as it goes — on a database of the test's own.
    #[test]
    fn the_indexer_indexes_a_folder_and_a_saved_sidecar() {
        let dir = scratch("indexer");
        let shoot = dir.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
        write_frame(&a, &R6, 1);
        write_frame(&b, &R5, 2);
        let db = dir.join("data").join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db.clone(), move |told| {
            let _ = tx.send(told);
        });
        let wait = |want: &dyn Fn(&Told) -> bool| loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return told;
            }
        };
        assert!(matches!(wait(&|t| matches!(t, Told::Opened(_))), Told::Opened(p) if p == db));
        indexer.folders(folders_of(&[a.clone(), b.clone()]), 7);
        match wait(&|t| matches!(t, Told::Indexed { .. })) {
            Told::Indexed {
                generation, report, ..
            } => {
                assert_eq!(generation, 7);
                assert_eq!(report.added, 2, "{report:?}");
            }
            other => panic!("{other:?}"),
        }
        let reader = Library::open_read_only(&db).unwrap();
        assert_eq!(reader.by_path(&a).unwrap().unwrap().meta.rating, 0);

        // A rating saved, and the row follows once the indexer says so.
        let mut s = Sidecar::default();
        s.meta.rating = 4;
        s.save(&a).unwrap();
        indexer.file(a.clone());
        wait(&|t| matches!(t, Told::FilesIndexed));
        assert_eq!(reader.by_path(&a).unwrap().unwrap().meta.rating, 4);
        drop(indexer);
        drop(reader);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_facet_row_keeps_its_chips_on_and_caps_the_rest() {
        let counts: Vec<FacetCount> = (0..20)
            .map(|i| FacetCount {
                value: (100 * (i + 1)).to_string(),
                label: (100 * (i + 1)).to_string(),
                count: if i == 3 { 1 } else { 20 - i },
            })
            .collect();
        let on = vec!["400".to_string(), "99999".to_string()];
        let chips = chips_of(Facet::Iso, &counts, &on);
        // Twelve by count, the one on kept past the cut, and a value
        // on that no frame holds any more, so it can be let go.
        assert_eq!(chips.len(), CHIPS_A_FACET + 2);
        assert!(chips.iter().any(|c| c.key == "400" && c.on));
        assert!(
            chips
                .iter()
                .any(|c| c.key == "99999" && c.on && c.count == 0)
        );
        // In the facet's own order, not the count's.
        let keys: Vec<i32> = chips
            .iter()
            .filter(|c| c.count > 0)
            .map(|c| c.key.parse().unwrap())
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(chips_of(Facet::Focal, &counts[..1], &[])[0].text, "100 mm");
    }
}
