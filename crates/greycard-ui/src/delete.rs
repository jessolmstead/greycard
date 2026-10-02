//! Delete from disk: the frames chosen, or the rejects folder's, each
//! with its sidecars, to the system trash where there is one and for
//! good where there is not.
//!
//! What goes with a frame is what belongs to it and nothing else: the
//! raw or picture itself, its `.gcd` in either place a sidecar is kept
//! (beside it, or under the shoot's hidden folder), and the XMPs whose
//! names are this frame's, as [`greycard_edit::xmp::paths_of`] names
//! them for the move of the rejects. The short `IMG.xmp` is only this
//! frame's when no other picture in the folder answers to `IMG`, so a
//! raw deleted beside its JPEG leaves the XMP the two share.
//!
//! Nothing outside the folders the caller allows is touched, and no
//! link is followed: a frame, a sidecar or the hidden folder that is
//! a symbolic link keeps the whole frame where it is, since deleting
//! a link would delete nothing the user meant, and deleting through
//! one could delete what they did not.
//!
//! Everything here is pure over the disk and the paths handed in, so
//! it is tested on a scratch folder; the sheet and the window are
//! `panel::delete`'s. The trash itself is handed in as a function, so
//! no test ever puts anything in the user's trash.

use std::path::{Path, PathBuf};

use greycard_edit::{Placement, SIDECAR_FOLDER, Sidecar};

/// Whether this build has a system trash to move files to: the
/// freedesktop trash on Linux and the BSDs, the Finder's on macOS,
/// the Recycle Bin on Windows. The `trash` crate has no other
/// platforms. Whether the trash takes a given file is only known by
/// asking it: on Linux a file on another disk or a network mount may
/// have none, and that failure is remembered for the folder
/// ([`offer`]).
pub(crate) const TRASH_SUPPORTED: bool = cfg!(any(
    windows,
    target_os = "macos",
    all(unix, not(target_os = "android"), not(target_os = "ios"))
));

/// What the sheet offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Offer {
    /// "Move to the trash": the default button, and what Enter does.
    pub(crate) trash: bool,
    /// "Delete permanently": never the default, never Enter's.
    pub(crate) permanent: bool,
}

/// What the sheet offers: the trash wherever the platform has one;
/// permanent removal where it has none, and beside the trash, as a
/// choice the user has to make with the pointer, where the trash has
/// already refused files from this folder this session. A trash that
/// fails never becomes a permanent delete of its own accord.
pub(crate) fn offer(supported: bool, refused_here: bool) -> Offer {
    Offer {
        trash: supported,
        permanent: !supported || refused_here,
    }
}

/// How the files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum How {
    Trash,
    Permanent,
}

/// One frame to delete, and what goes with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Doomed {
    pub(crate) frame: PathBuf,
    pub(crate) sidecars: Vec<PathBuf>,
}

/// What a delete would touch, and what it refuses.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) frames: Vec<Doomed>,
    /// The frames left alone, and why.
    pub(crate) refused: Vec<(PathBuf, String)>,
}

impl Plan {
    pub(crate) fn sidecars(&self) -> usize {
        self.frames.iter().map(|d| d.sidecars.len()).sum()
    }
}

/// What a delete did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Deleted {
    /// The frames no longer on disk: deleted now, or found gone
    /// already between the sheet and the confirm.
    pub(crate) frames: Vec<PathBuf>,
    /// How many of them were gone already.
    pub(crate) vanished: usize,
    /// How many sidecars went with them.
    pub(crate) sidecars: usize,
    /// The frames that could not be deleted, and why; each is whole
    /// where it was, its sidecars with it.
    pub(crate) failed: Vec<(PathBuf, String)>,
    /// The trash refused something, and the delete stopped there.
    pub(crate) trash_refused: Option<Refusal>,
    /// The frames after a refusal, not tried.
    pub(crate) untried: usize,
}

/// What the trash refused, as found on disk afterwards: `trash`
/// stops at its first error, so part of a frame may have gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) frame: PathBuf,
    pub(crate) why: String,
    /// Whether the frame itself went (to the trash, or before).
    pub(crate) frame_gone: bool,
    /// Whether it had gone before the confirm, by another hand: the
    /// trash never had it.
    pub(crate) was_gone: bool,
    /// Its sidecars still where they were.
    pub(crate) left: Vec<PathBuf>,
    /// How many of its sidecars went to the trash.
    pub(crate) sidecars_gone: usize,
}

/// The folders a delete may reach, as the disk spells them: each of
/// `dirs` canonical. A folder that will not canonicalize (gone, or
/// not the user's to read) is left out, and so is everything in it.
pub(crate) fn allowed(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if let Ok(c) = std::fs::canonicalize(d)
            && !out.contains(&c)
        {
            out.push(c);
        }
    }
    out
}

/// Whether `path` may be deleted: a regular file, not a link, whose
/// folder is one of `allowed` (canonical) or the hidden sidecar
/// folder under one. `Ok(false)` when there is nothing there.
fn deletable(path: &Path, allowed: &[PathBuf]) -> Result<bool, String> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    // The folder as the disk has it: a link anywhere above resolves
    // here, and the answer is checked against the allowed folders'
    // own canonical spellings, so a hidden folder that is a link to
    // elsewhere is refused.
    let Ok(folder) = std::fs::canonicalize(parent) else {
        return Err(format!("{} cannot be read", parent.display()));
    };
    let hidden = std::ffi::OsStr::new(SIDECAR_FOLDER);
    let inside = allowed
        .iter()
        .any(|a| *a == folder || a.join(hidden) == folder);
    if !inside {
        return Err(format!(
            "{} is not in the folder open",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(format!(
            "{} is a link",
            path.file_name().unwrap_or_default().to_string_lossy()
        )),
        Ok(m) if !m.is_file() => Err(format!(
            "{} is not a file",
            path.file_name().unwrap_or_default().to_string_lossy()
        )),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Every sidecar of `frame` that is there: its `.gcd` in both places
/// (a folder whose setting was flipped can hold both), and its XMPs.
pub(crate) fn sidecars_of(frame: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [Placement::Beside, Placement::Folder]
        .into_iter()
        .map(|p| Sidecar::path_in(frame, p))
        .chain(greycard_edit::xmp::paths_of(frame))
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .collect();
    out.dedup();
    out
}

/// What deleting `frames` would touch, each checked against `allowed`
/// (as [`allowed`] makes it). A frame that is refused, or any of
/// whose sidecars is, stays whole: its raw never goes without its
/// sidecars or the other way round. A frame already gone is planned
/// with whatever sidecars it left behind.
pub(crate) fn plan(frames: &[PathBuf], allowed: &[PathBuf]) -> Plan {
    let mut plan = Plan::default();
    for frame in frames {
        if let Err(why) = deletable(frame, allowed) {
            plan.refused.push((frame.clone(), why));
            continue;
        }
        let sidecars = sidecars_of(frame);
        if let Some(why) = sidecars.iter().find_map(|s| deletable(s, allowed).err()) {
            plan.refused.push((frame.clone(), why));
            continue;
        }
        plan.frames.push(Doomed {
            frame: frame.clone(),
            sidecars,
        });
    }
    plan
}

/// The system trash, for the window: the `trash` crate's. Called
/// only on the delete's own thread (`panel::delete`), never the
/// window's: the crate may copy a file into the home trash, and on
/// Windows it sets up COM for the thread it is first called on.
#[cfg(not(test))]
pub(crate) fn system_trash(paths: &[PathBuf]) -> Result<(), String> {
    trash::delete_all(paths).map_err(|e| e.to_string())
}

/// A test build has no way to the real trash: whatever calls this
/// in a test is a bug, and fails the test rather than putting a file
/// in the user's trash.
#[cfg(test)]
pub(crate) fn system_trash(paths: &[PathBuf]) -> Result<(), String> {
    panic!("a test reached the system trash with {paths:?}");
}

/// Delete what `plan` names, `how`. `trash` is the system trash for
/// [`How::Trash`] ([`system_trash`], or a stand-in under test) and is
/// never called for [`How::Permanent`].
///
/// Each frame is checked again at the moment it goes: one gone since
/// the sheet counts as gone and its sidecars still go; one that has
/// become a link, or whose folder has, is refused. The trash takes a
/// frame's files in one call, the frame first. It stops at its first
/// error, so after a refusal what went is read back from the disk: a
/// frame whose raw went is counted gone even when a sidecar stayed.
/// The first refusal stops the delete, the frames after it untried,
/// and the permanent path is never taken in its place.
pub(crate) fn delete(
    plan: &Plan,
    how: How,
    allowed: &[PathBuf],
    trash: impl Fn(&[PathBuf]) -> Result<(), String>,
) -> Deleted {
    let mut done = Deleted::default();
    for (n, d) in plan.frames.iter().enumerate() {
        let there = match deletable(&d.frame, allowed) {
            Ok(there) => there,
            Err(why) => {
                done.failed.push((d.frame.clone(), why));
                continue;
            }
        };
        let mut sidecars = Vec::new();
        let mut refused = None;
        for s in &d.sidecars {
            match deletable(s, allowed) {
                Ok(true) => sidecars.push(s.clone()),
                Ok(false) => {}
                Err(why) => refused = Some(why),
            }
        }
        if let Some(why) = refused {
            done.failed.push((d.frame.clone(), why));
            continue;
        }
        match how {
            How::Trash => {
                let all: Vec<PathBuf> = there
                    .then(|| d.frame.clone())
                    .into_iter()
                    .chain(sidecars.iter().cloned())
                    .collect();
                if !all.is_empty()
                    && let Err(why) = trash(&all)
                {
                    let still = |p: &PathBuf| std::fs::symlink_metadata(p).is_ok();
                    let left: Vec<PathBuf> =
                        sidecars.iter().filter(|s| still(s)).cloned().collect();
                    let sidecars_gone = sidecars.len() - left.len();
                    let frame_gone = !still(&d.frame);
                    if frame_gone {
                        done.frames.push(d.frame.clone());
                        done.sidecars += sidecars_gone;
                        if !there {
                            done.vanished += 1;
                        }
                    }
                    done.trash_refused = Some(Refusal {
                        frame: d.frame.clone(),
                        why,
                        frame_gone,
                        was_gone: !there,
                        left,
                        sidecars_gone,
                    });
                    done.untried = plan.frames.len() - n - 1;
                    break;
                }
                done.sidecars += sidecars.len();
            }
            How::Permanent => {
                if there && let Err(e) = std::fs::remove_file(&d.frame) {
                    done.failed.push((d.frame.clone(), e.to_string()));
                    continue;
                }
                for s in &sidecars {
                    match std::fs::remove_file(s) {
                        Ok(()) => done.sidecars += 1,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => tracing::warn!("{}: not deleted: {e}", s.display()),
                    }
                }
            }
        }
        if !there {
            done.vanished += 1;
        }
        done.frames.push(d.frame.clone());
    }
    done
}

/// A count and its noun: "1 frame", "3 frames".
pub(crate) fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a delete did, said as the status line says it: what went and
/// how, then what the trash refused, worded from what is on disk
/// after, then what was left and why. `refused` are the frames the
/// confirm's own check left alone.
pub(crate) fn summary(done: &Deleted, how: How, refused: &[(PathBuf, String)]) -> String {
    let mut out = if done.frames.is_empty() {
        "nothing was deleted".to_string()
    } else {
        format!(
            "{} {} and {}{}",
            match how {
                How::Trash => "moved to the trash:",
                How::Permanent => "deleted permanently:",
            },
            count(done.frames.len(), "frame"),
            count(done.sidecars, "sidecar"),
            if done.vanished > 0 {
                format!(" ({} gone already)", done.vanished)
            } else {
                String::new()
            }
        )
    };
    if let Some(r) = &done.trash_refused {
        // Each sidecar left, and where: under the hidden folder, or in
        // the frame's own.
        let hidden = std::ffi::OsStr::new(SIDECAR_FOLDER);
        let left: Vec<String> = r
            .left
            .iter()
            .map(|p| {
                let place = if p.parent().and_then(Path::file_name) == Some(hidden) {
                    format!("left in {SIDECAR_FOLDER}")
                } else {
                    "left in the folder".to_string()
                };
                format!("{} ({place})", name(p))
            })
            .collect();
        let what = if r.frame_gone && left.is_empty() {
            format!("the trash stopped at {}: {}", name(&r.frame), r.why)
        } else if r.was_gone {
            format!(
                "{} was gone already, and the trash refused {}: {}",
                name(&r.frame),
                left.join(", "),
                r.why
            )
        } else if r.frame_gone {
            format!(
                "the trash took {} but refused {}: {}",
                name(&r.frame),
                left.join(", "),
                r.why
            )
        } else if r.sidecars_gone > 0 {
            format!(
                "the trash refused {} ({}) after taking {} of it",
                name(&r.frame),
                r.why,
                count(r.sidecars_gone, "sidecar")
            )
        } else {
            format!("the trash refused {} ({})", name(&r.frame), r.why)
        };
        out.push_str(&format!("; {what}"));
        let stayed = usize::from(!r.frame_gone) + done.untried;
        if stayed > 0 {
            out.push_str(&format!(
                "; {} still where {}: ask again to delete permanently",
                count(stayed, "frame"),
                if stayed == 1 { "it was" } else { "they were" }
            ));
        }
    }
    let not: Vec<&(PathBuf, String)> = refused.iter().chain(&done.failed).collect();
    if let Some((frame, why)) = not.first() {
        out.push_str(&format!(
            "; {} not deleted ({}: {why}{})",
            count(not.len(), "frame"),
            name(frame),
            if not.len() > 1 {
                "; the log has each"
            } else {
                ""
            }
        ));
    }
    out
}

/// Take away a folder a delete emptied: its hidden sidecar folder,
/// then the folder itself, each only when there is nothing left in
/// it. For the rejects folder, which the move of the rejects made.
pub(crate) fn remove_if_empty(dir: &Path) {
    // Fails, as it is meant to, on a folder with anything in it, and
    // never follows a link: a link is not a directory to remove_dir.
    let _ = std::fs::remove_dir(dir.join(SIDECAR_FOLDER));
    let _ = std::fs::remove_dir(dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::meta::Flag;

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-delete-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every file under `dir`, by its path below it, sorted.
    fn all(dir: &Path) -> Vec<String> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if e.file_type().unwrap().is_dir() {
                    walk(root, &p, out);
                } else {
                    out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// A shoot of five: A with a `.gcd` beside and an XMP of each name,
    /// B with its `.gcd` under the hidden folder, C with nothing, D a
    /// JPEG beside a raw of the same stem sharing an XMP, and a note
    /// that is no one's.
    fn shoot(dir: &Path) -> Vec<PathBuf> {
        let names = ["A.CR3", "B.CR3", "C.CR3", "D.CR3", "D.JPG"];
        let files: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        for f in &files {
            std::fs::write(f, b"raw").unwrap();
        }
        let mut s = Sidecar::default();
        s.meta.flag = Flag::Reject;
        s.save(&files[0]).unwrap();
        greycard_edit::xmp::save(&files[0], &s.meta, None).unwrap();
        std::fs::write(
            greycard_edit::xmp::long_path(&files[0]),
            greycard_edit::xmp::fresh(&s.meta, None),
        )
        .unwrap();
        s.save_in(&files[1], Placement::Folder).unwrap();
        std::fs::write(dir.join("D.xmp"), b"shared").unwrap();
        std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
        files
    }

    fn never(_: &[PathBuf]) -> Result<(), String> {
        panic!("the trash is not for a permanent delete");
    }

    #[test]
    fn the_sheet_offers_the_trash_where_there_is_one_and_never_both_unasked() {
        // A platform with a trash: the trash alone.
        assert_eq!(
            offer(true, false),
            Offer {
                trash: true,
                permanent: false
            }
        );
        // The trash refused this folder before: both, the trash still
        // the default and the permanent one a choice.
        assert_eq!(
            offer(true, true),
            Offer {
                trash: true,
                permanent: true
            }
        );
        // No trash at all: the permanent one alone.
        for refused in [false, true] {
            assert_eq!(
                offer(false, refused),
                Offer {
                    trash: false,
                    permanent: true
                }
            );
        }
        // Every platform this is built for has one.
        const { assert!(TRASH_SUPPORTED) };
    }

    #[test]
    fn a_permanent_delete_takes_the_frames_and_their_sidecars_and_nothing_else() {
        let dir = scratch("selection");
        let files = shoot(&dir);
        let ok = allowed(std::slice::from_ref(&dir));
        // A, B and the JPEG of D.
        let chosen = [files[0].clone(), files[1].clone(), files[4].clone()];
        let p = plan(&chosen, &ok);
        assert!(p.refused.is_empty(), "{:?}", p.refused);
        assert_eq!(p.frames.len(), 3);
        // A's .gcd and both XMPs, B's .gcd under the hidden folder;
        // D's XMP is the raw's and the JPEG's both, so neither's.
        assert_eq!(p.sidecars(), 4);
        let done = delete(&p, How::Permanent, &ok, never);
        assert_eq!(done.frames, chosen);
        assert_eq!(done.sidecars, 4);
        assert_eq!(done.vanished, 0);
        assert!(done.failed.is_empty() && done.trash_refused.is_none());
        assert_eq!(all(&dir), ["C.CR3", "D.CR3", "D.xmp", "notes.txt"]);
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_path_outside_the_folder_or_a_link_is_refused_whole() {
        let dir = scratch("outside");
        let shoot_dir = dir.join("shoot");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let files = shoot(&shoot_dir);
        let away = elsewhere.join("E.CR3");
        std::fs::write(&away, b"raw").unwrap();
        let ok = allowed(std::slice::from_ref(&shoot_dir));
        // Outside the folder; and through `..` back out of it.
        let sneaky = shoot_dir.join("..").join("elsewhere").join("E.CR3");
        let p = plan(&[away.clone(), sneaky, files[2].clone()], &ok);
        assert_eq!(p.frames.len(), 1);
        assert_eq!(p.refused.len(), 2);
        assert!(p.refused[0].1.contains("not in the folder open"));
        let done = delete(&p, How::Permanent, &ok, never);
        assert_eq!(done.frames, [files[2].clone()]);
        assert!(away.exists());
        #[cfg(unix)]
        {
            // A frame that is a link, and a frame whose sidecar is:
            // each stays, whole, and the file linked to is untouched.
            let link = shoot_dir.join("L.CR3");
            std::os::unix::fs::symlink(&away, &link).unwrap();
            let linked_gcd = Sidecar::path_for(&files[3]);
            std::os::unix::fs::symlink(&away, &linked_gcd).unwrap();
            let p = plan(&[link.clone(), files[3].clone()], &ok);
            assert!(p.frames.is_empty(), "{p:?}");
            assert!(p.refused[0].1.contains("is a link"));
            assert!(p.refused[1].1.contains("is a link"));
            // A hidden folder that is a link to elsewhere: B's sidecar
            // is outside, and B stays.
            let hidden = shoot_dir.join(SIDECAR_FOLDER);
            std::fs::rename(&hidden, dir.join("hidden")).unwrap();
            std::os::unix::fs::symlink(dir.join("hidden"), &hidden).unwrap();
            let p = plan(std::slice::from_ref(&files[1]), &ok);
            assert!(p.frames.is_empty(), "{p:?}");
            assert!(link.exists() && away.exists() && files[3].exists());
            assert!(files[1].exists());
        }
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_frame_gone_between_the_sheet_and_the_confirm_is_counted_gone() {
        let dir = scratch("vanished");
        let files = shoot(&dir);
        let ok = allowed(std::slice::from_ref(&dir));
        let p = plan(&[files[0].clone(), files[2].clone()], &ok);
        // Both gone by another hand, one of them with its sidecar.
        std::fs::remove_file(&files[0]).unwrap();
        std::fs::remove_file(&files[2]).unwrap();
        std::fs::remove_file(Sidecar::path_for(&files[0])).unwrap();
        let done = delete(&p, How::Permanent, &ok, never);
        assert_eq!(done.frames, [files[0].clone(), files[2].clone()]);
        assert_eq!(done.vanished, 2);
        // A's two XMPs were still there, and went.
        assert_eq!(done.sidecars, 2);
        assert!(done.failed.is_empty());
        let hidden_b = Path::new(SIDECAR_FOLDER).join("B.CR3.gcd");
        assert_eq!(
            all(&dir),
            [
                hidden_b.to_string_lossy().as_ref(),
                "B.CR3",
                "D.CR3",
                "D.JPG",
                "D.xmp",
                "notes.txt"
            ]
        );
        // And a whole folder gone: nothing to do, nothing panics.
        crate::testing::remove_dir_retry(&dir);
        let done = delete(&p, How::Permanent, &ok, never);
        assert!(done.frames.is_empty());
        assert_eq!(done.failed.len(), 2);
    }

    /// The trash's side, with a stand-in for the trash: a frame and its
    /// sidecars go in one call, and a refusal stops there with the
    /// rest where they were and nothing deleted for good in its place.
    #[test]
    fn a_trash_that_refuses_stops_the_delete_and_deletes_nothing_for_good() {
        let dir = scratch("trash");
        let bin = dir.join("bin");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let files = shoot(&shoot_dir);
        let ok = allowed(std::slice::from_ref(&shoot_dir));
        let calls = std::cell::RefCell::new(Vec::new());
        // Takes A's, refuses B's.
        let stand_in = |paths: &[PathBuf]| {
            calls.borrow_mut().push(paths.len());
            if paths.iter().any(|p| p.ends_with("B.CR3")) {
                return Err("no trash on this disk".to_string());
            }
            for p in paths {
                std::fs::rename(p, bin.join(p.file_name().unwrap())).unwrap();
            }
            Ok(())
        };
        let chosen = [files[0].clone(), files[1].clone(), files[2].clone()];
        let p = plan(&chosen, &ok);
        let done = delete(&p, How::Trash, &ok, stand_in);
        assert_eq!(*calls.borrow(), [4, 2]);
        assert_eq!(done.frames, [files[0].clone()]);
        assert_eq!(done.sidecars, 3);
        let r = done.trash_refused.clone().unwrap();
        assert_eq!(r.frame, files[1]);
        assert_eq!(r.why, "no trash on this disk");
        assert!(!r.frame_gone && r.sidecars_gone == 0);
        assert_eq!(done.untried, 1);
        assert!(files[1].exists() && files[2].exists());
        assert!(Sidecar::path_in(&files[1], Placement::Folder).exists());
        assert_eq!(all(&bin), ["A.CR3", "A.CR3.gcd", "A.CR3.xmp", "A.xmp"]);
        let said = summary(&done, How::Trash, &[]);
        assert_eq!(
            said,
            "moved to the trash: 1 frame and 3 sidecars; the trash refused B.CR3 \
             (no trash on this disk); 2 frames still where they were: ask again to \
             delete permanently"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The trash stops at its first error: here it takes the raw and
    /// refuses the sidecar. The frame is gone and counted so, and the
    /// status says the sidecar stayed rather than that the raw was
    /// refused.
    #[test]
    fn a_trash_that_takes_the_raw_and_refuses_the_sidecar_is_said_as_it_was() {
        let dir = scratch("half");
        let bin = dir.join("bin");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let files = shoot(&shoot_dir);
        let ok = allowed(std::slice::from_ref(&shoot_dir));
        let stand_in = |paths: &[PathBuf]| {
            std::fs::rename(&paths[0], bin.join(paths[0].file_name().unwrap())).unwrap();
            Err("the sidecar would not go".to_string())
        };
        // B, whose one sidecar is under the hidden folder, then C.
        let p = plan(&[files[1].clone(), files[2].clone()], &ok);
        let done = delete(&p, How::Trash, &ok, stand_in);
        assert_eq!(done.frames, [files[1].clone()]);
        assert_eq!(done.sidecars, 0);
        assert_eq!(done.untried, 1);
        let r = done.trash_refused.clone().unwrap();
        assert!(r.frame_gone);
        assert_eq!(r.left, [Sidecar::path_in(&files[1], Placement::Folder)]);
        assert!(!files[1].exists() && files[2].exists());
        assert_eq!(all(&bin), ["B.CR3"]);
        assert_eq!(
            summary(&done, How::Trash, &[]),
            "moved to the trash: 1 frame and 0 sidecars; the trash took B.CR3 but \
             refused B.CR3.gcd (left in .greycard): the sidecar would not go; 1 frame \
             still where it was: ask again to delete permanently"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A raw gone before the confirm, and the trash refusing the
    /// sidecar it left: the frame is counted gone already, and the
    /// status does not say the trash took it.
    #[test]
    fn a_raw_gone_before_and_its_sidecar_refused_is_not_said_as_trashed() {
        let dir = scratch("gone-refused");
        let files = shoot(&dir);
        let ok = allowed(std::slice::from_ref(&dir));
        let p = plan(std::slice::from_ref(&files[0]), &ok);
        std::fs::remove_file(&files[0]).unwrap();
        let done = delete(&p, How::Trash, &ok, |_: &[PathBuf]| {
            Err("refused".to_string())
        });
        assert_eq!(done.frames, [files[0].clone()]);
        assert_eq!(done.vanished, 1);
        let r = done.trash_refused.clone().unwrap();
        assert!(r.frame_gone && r.was_gone);
        assert_eq!(r.left.len(), 3);
        assert_eq!(
            summary(&done, How::Trash, &[]),
            "moved to the trash: 1 frame and 0 sidecars (1 gone already); A.CR3 was \
             gone already, and the trash refused A.CR3.gcd (left in the folder), \
             A.CR3.xmp (left in the folder), A.xmp (left in the folder): refused"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// `delete` checks each path again whatever the plan says: a plan
    /// made by hand with a frame outside the folder deletes nothing
    /// of it, and a folder swapped for a link after the plan was made
    /// is refused at the confirm.
    #[test]
    fn delete_refuses_what_the_plan_should_not_have_held() {
        let dir = scratch("handmade");
        let shoot_dir = dir.join("shoot");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        // The symlink swap below is unix only; the files are the
        // shoot's either way.
        #[cfg_attr(not(unix), allow(unused_variables))]
        let files = shoot(&shoot_dir);
        let away = elsewhere.join("A.CR3");
        std::fs::write(&away, b"raw").unwrap();
        let ok = allowed(std::slice::from_ref(&shoot_dir));
        let handmade = Plan {
            frames: vec![Doomed {
                frame: away.clone(),
                sidecars: Vec::new(),
            }],
            refused: Vec::new(),
        };
        let done = delete(&handmade, How::Permanent, &ok, never);
        assert!(done.frames.is_empty());
        assert_eq!(done.failed.len(), 1);
        assert!(away.exists());
        let said = summary(&done, How::Permanent, &[]);
        assert!(said.ends_with("; 1 frame not deleted (A.CR3: A.CR3 is not in the folder open)"));
        #[cfg(unix)]
        {
            // Planned, then the folder swapped for a link to one
            // holding files of the same names.
            let p = plan(&files[..2], &ok);
            assert_eq!(p.frames.len(), 2);
            std::fs::write(elsewhere.join("B.CR3"), b"raw").unwrap();
            std::fs::rename(&shoot_dir, dir.join("moved")).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &shoot_dir).unwrap();
            let done = delete(&p, How::Permanent, &ok, never);
            assert!(done.frames.is_empty());
            assert_eq!(done.failed.len(), 2);
            assert!(away.exists() && elsewhere.join("B.CR3").exists());
            assert!(dir.join("moved").join("A.CR3").exists());
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// No test can reach the real trash: the test build's is a panic.
    #[test]
    #[should_panic(expected = "a test reached the system trash")]
    fn a_test_build_has_no_way_to_the_system_trash() {
        let _ = system_trash(&[PathBuf::from("/nowhere/at/all")]);
    }

    #[test]
    fn an_emptied_rejects_folder_goes_and_one_with_anything_left_stays() {
        let dir = scratch("rejects");
        let rejects = dir.join("rejects");
        std::fs::create_dir_all(rejects.join(SIDECAR_FOLDER)).unwrap();
        std::fs::write(rejects.join("notes.txt"), b"mine").unwrap();
        remove_if_empty(&rejects);
        assert!(rejects.join("notes.txt").exists());
        assert!(!rejects.join(SIDECAR_FOLDER).exists());
        std::fs::remove_file(rejects.join("notes.txt")).unwrap();
        remove_if_empty(&rejects);
        assert!(!rejects.exists() && dir.exists());
        crate::testing::remove_dir_retry(&dir);
    }
}
