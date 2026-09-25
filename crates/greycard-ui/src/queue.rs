//! The export queue: a set of frames written one at a time on the
//! worker, behind whatever develop is running, each under its own
//! edit and all under one sheet.
//!
//! The set is shared between the window, which holds it to cancel it
//! and to say how far it has got, and the worker, which takes its
//! frames off the export queue one at a time and records what came of
//! each. What is here is the part that needs no engine: the names the
//! frames are written under, the tally, the cancel and the words the
//! status line says, so it is tested without a raw or a window.

use crate::export::{Format, OnExists, Settings};
use greycard_edit::Edit;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// One frame of a set: the file, the edit it is written under, and
/// where it goes.
#[derive(Debug, Clone)]
pub struct Frame {
    pub source: PathBuf,
    pub edit: Edit,
    /// The frame's quarter turns (`Sidecar::turn`).
    pub turn: u8,
    /// A raw with no sidecar yet: its learned-denoiser blend is seeded
    /// from its ISO, as its first open would.
    pub seed_blend: bool,
    /// The name asked for; the policy may write another beside it.
    pub out: PathBuf,
}

/// What came of one frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Done {
    Exported {
        path: PathBuf,
        seconds: f64,
        /// What the policy made of a file of that name being there.
        note: Option<String>,
    },
    /// A file of that name was there and the policy said leave it.
    Skipped {
        path: PathBuf,
    },
    Failed {
        message: String,
    },
    /// The set was canceled before this frame was begun.
    Canceled,
}

/// What a set has come to so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tally {
    pub exported: usize,
    pub skipped: usize,
    pub canceled: usize,
    /// The frames that failed, by source name, and why.
    pub failed: Vec<(String, String)>,
}

/// A set on its way through the queue.
pub struct Set {
    pub total: usize,
    /// Where the frames go: the folder chosen, for the finished line.
    /// None when each goes beside its own file.
    pub folder: Option<PathBuf>,
    pub settings: Settings,
    pub on_exists: OnExists,
    pub started: std::time::Instant,
    canceled: AtomicBool,
    tally: Mutex<Tally>,
}

impl Set {
    pub fn new(
        total: usize,
        folder: Option<PathBuf>,
        settings: Settings,
        on_exists: OnExists,
    ) -> Self {
        Self {
            total,
            folder,
            settings,
            on_exists,
            started: std::time::Instant::now(),
            canceled: AtomicBool::new(false),
            tally: Mutex::new(Tally::default()),
        }
    }

    /// Stop after the frame in hand: every frame not yet begun is
    /// passed over.
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    /// Count what came of frame `source`, and say where the set stands.
    pub fn record(&self, source: &Path, done: &Done) -> Tally {
        let mut t = self.tally.lock().expect("the set's tally");
        match done {
            Done::Exported { .. } => t.exported += 1,
            Done::Skipped { .. } => t.skipped += 1,
            Done::Canceled => t.canceled += 1,
            Done::Failed { message } => t.failed.push((file_name(source), message.clone())),
        }
        t.clone()
    }

    pub fn tally(&self) -> Tally {
        self.tally.lock().expect("the set's tally").clone()
    }

    /// Whether frame `index` is the set's last, after which the worker
    /// says the set is finished.
    pub fn is_last(&self, index: usize) -> bool {
        index + 1 >= self.total
    }
}

/// Run frame `index` of `set` through `work` unless the set has been
/// canceled, record what came of it, and say whether the set is done:
/// the worker's step for a frame taken off the queue, with the engine
/// behind `work`.
pub fn step(set: &Set, index: usize, source: &Path, work: impl FnOnce() -> Done) -> (Done, bool) {
    let done = if set.is_canceled() {
        Done::Canceled
    } else {
        work()
    };
    set.record(source, &done);
    (done, set.is_last(index))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Where each frame of a set goes in `folder`: its own name with the
/// format's extension. Without a folder (no chooser to ask) each goes
/// beside its own file under the `.greycard` name the editor writes
/// there, which no camera does. Two frames of one name (a raw and its camera
/// JPEG) are told apart as the policy would tell them, ` (2)` on the
/// second, so neither writes over the other whatever the policy says;
/// the names are compared as a case-blind file system compares them.
/// A name that would be the source itself (a JPEG exported as a JPEG
/// into its own folder) takes the `.greycard` name the editor writes
/// beside a file when it has no chooser to ask.
pub fn names(sources: &[PathBuf], folder: Option<&Path>, format: Format) -> Vec<PathBuf> {
    let mut taken: Vec<String> = Vec::new();
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    sources
        .iter()
        .map(|source| {
            let stem = source
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "export".into());
            let ext = format.extension();
            let (folder, beside) = match folder {
                Some(f) => (f.to_path_buf(), false),
                None => (
                    source.parent().map(Path::to_path_buf).unwrap_or_default(),
                    true,
                ),
            };
            let mut path = folder.join(format!("{stem}.{ext}"));
            if beside || same_file(&path, source) {
                path = folder.join(format!("{stem}.greycard.{ext}"));
            }
            let base = path.clone();
            let mut n = 2;
            while taken.contains(&key(&path)) || same_file(&path, source) {
                let stem = base
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                path = folder.join(format!("{stem} ({n}).{ext}"));
                n += 1;
            }
            taken.push(key(&path));
            path
        })
        .collect()
}

/// Whether two paths name one file: the same path in any case, or, when
/// both are there, the same file by the file system's own account.
fn same_file(a: &Path, b: &Path) -> bool {
    if a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// "1 frame", "3 frames".
pub fn frames(n: usize) -> String {
    format!("{n} frame{}", if n == 1 { "" } else { "s" })
}

/// The status line while frame `index` (from zero) is being written.
pub fn progress_line(index: usize, total: usize, name: &str) -> String {
    format!("exporting {} of {total}: {name}", index + 1)
}

/// The status line once the set is done with: how many were written,
/// where, and what did not go.
pub fn finished_line(tally: &Tally, total: usize, folder: Option<&Path>, seconds: f64) -> String {
    let mut s = if tally.canceled > 0 {
        format!("export stopped: {} of {total} exported", tally.exported)
    } else {
        format!("exported {}", frames(tally.exported))
    };
    match folder {
        Some(f) => s.push_str(&format!(" to {}", f.display())),
        None => s.push_str(" beside their files"),
    }
    s.push_str(&format!(" in {seconds:.1} s"));
    if tally.skipped > 0 {
        s.push_str(&format!(", {} skipped (there already)", tally.skipped));
    }
    if !tally.failed.is_empty() {
        s.push_str(&format!(", {} failed", tally.failed.len()));
        if let [(name, why)] = tally.failed.as_slice() {
            s.push_str(&format!(" ({name}: {why})"));
        } else {
            s.push_str(" (see the log)");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(total: usize) -> Set {
        Set::new(
            total,
            Some(PathBuf::from("out")),
            Settings::default(),
            OnExists::Increment,
        )
    }

    fn sources(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("shoot/IMG_{i:04}.CR3")))
            .collect()
    }

    /// Five frames, the cancel pressed while the third is in hand: the
    /// third is finished, the last two are passed over, the set is
    /// said to be done once, at the last, and the frames run in order.
    #[test]
    fn a_cancel_finishes_the_frame_in_hand_and_passes_over_the_rest() {
        let s = set(5);
        let files = sources(5);
        let mut ran = Vec::new();
        let mut finished = Vec::new();
        let mut results = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let (done, last) = step(&s, i, f, || {
                ran.push(i);
                if i == 2 {
                    // Pressed while this one is being written.
                    s.cancel();
                }
                Done::Exported {
                    path: PathBuf::from(format!("out/{i}.jpg")),
                    seconds: 0.1,
                    note: None,
                }
            });
            results.push(done);
            if last {
                finished.push(i);
            }
        }
        assert_eq!(ran, vec![0, 1, 2]);
        assert_eq!(finished, vec![4]);
        assert!(matches!(results[2], Done::Exported { .. }));
        assert_eq!(results[3], Done::Canceled);
        assert_eq!(results[4], Done::Canceled);
        let t = s.tally();
        assert_eq!((t.exported, t.canceled, t.skipped), (3, 2, 0));
        assert!(t.failed.is_empty());
        let line = finished_line(&t, 5, s.folder.as_deref(), 3.0);
        assert!(
            line.starts_with("export stopped: 3 of 5 exported"),
            "{line}"
        );
    }

    /// A frame that fails is counted and named, and the rest go.
    #[test]
    fn a_failure_is_counted_and_the_rest_are_exported() {
        let s = set(4);
        let files = sources(4);
        for (i, f) in files.iter().enumerate() {
            step(&s, i, f, || {
                if i == 1 || i == 3 {
                    Done::Failed {
                        message: "not a raw".into(),
                    }
                } else {
                    Done::Exported {
                        path: PathBuf::from("x.jpg"),
                        seconds: 0.0,
                        note: None,
                    }
                }
            });
        }
        let t = s.tally();
        assert_eq!(t.exported, 2);
        assert_eq!(
            t.failed,
            vec![
                ("IMG_0001.CR3".to_string(), "not a raw".to_string()),
                ("IMG_0003.CR3".to_string(), "not a raw".to_string()),
            ]
        );
        let line = finished_line(&t, 4, Some(Path::new("/tmp/out")), 2.5);
        assert_eq!(
            line,
            "exported 2 frames to /tmp/out in 2.5 s, 2 failed (see the log)"
        );
        // One failure is said in full.
        let one = Tally {
            exported: 4,
            failed: vec![("IMG_0009.CR3".into(), "no such file".into())],
            ..Tally::default()
        };
        assert_eq!(
            finished_line(&one, 5, None, 1.0),
            "exported 4 frames beside their files in 1.0 s, 1 failed (IMG_0009.CR3: no such file)"
        );
        assert_eq!(
            progress_line(2, 5, "5M0A3021.jpg"),
            "exporting 3 of 5: 5M0A3021.jpg"
        );
    }

    #[test]
    fn a_set_s_names_never_meet_each_other_or_their_sources() {
        let folder = Path::new("out");
        let files = vec![
            PathBuf::from("a/IMG_1.CR3"),
            PathBuf::from("a/IMG_1.jpg"),
            PathBuf::from("b/img_1.CR3"),
            PathBuf::from("a/IMG_2.CR3"),
        ];
        let got = names(&files, Some(folder), Format::Jpeg);
        assert_eq!(
            got,
            vec![
                folder.join("IMG_1.jpg"),
                folder.join("IMG_1 (2).jpg"),
                folder.join("img_1 (3).jpg"),
                folder.join("IMG_2.jpg"),
            ]
        );
        // A JPEG into its own folder as a JPEG is not written over.
        let a = Path::new("a");
        let own = names(&[a.join("IMG_3.JPG")], Some(a), Format::Jpeg);
        assert_eq!(own, vec![a.join("IMG_3.greycard.jpg")]);
        let tiff = names(&[a.join("IMG_3.JPG")], Some(a), Format::Tiff);
        assert_eq!(tiff, vec![a.join("IMG_3.tif")]);
        // No folder: each beside its own, a raw and its JPEG apart.
        let beside = names(
            &[a.join("IMG_4.CR3"), a.join("IMG_4.JPG")],
            None,
            Format::Jpeg,
        );
        assert_eq!(
            beside,
            vec![
                a.join("IMG_4.greycard.jpg"),
                a.join("IMG_4.greycard (2).jpg")
            ]
        );
    }
}
