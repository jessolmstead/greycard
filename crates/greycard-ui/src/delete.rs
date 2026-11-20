//! Delete from disk: the frames chosen, or the rejects folder's, each
//! with its sidecars, to the system trash where there is one and for
//! good where there is not.
//!
//! What goes with a frame is what belongs to it and nothing else: the
//! raw or picture itself, its `.gcd` in either place a sidecar is kept
//! (beside it, or under the shoot's hidden folder), and the XMPs whose
//! names are this frame's, as [`greycard_edit_io::xmp::paths_of`] names
//! them for the move of the rejects. The short `IMG.xmp` is only this
//! frame's when no other picture in the folder answers to `IMG`, so a
//! raw deleted beside a JPEG listed apart leaves the XMP the two share.
//! The camera's JPEG beside a raw (`greycard_library::pairs`) is the
//! raw's and goes with it, its own sidecars with it, unless the JPEGs
//! are listed apart; a raw and its JPEG with one short XMP between them
//! take the XMP too, since nothing of its stem is left to answer to it.
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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::tr;

use greycard_edit::{Placement, SIDECAR_FOLDER, Sidecar};

/// Whether this build has a system trash to move files to: the
/// freedesktop trash on Linux and the BSDs, the Finder's on macOS,
/// the Recycle Bin on Windows. The `trash` crate has no other
/// platforms. Whether the trash takes a given file is only known by
/// asking it: on Linux a file on another disk or a network mount may
/// have none, and that failure is remembered for the folder
/// ([`offer`]).
pub const TRASH_SUPPORTED: bool = cfg!(any(
    windows,
    target_os = "macos",
    all(unix, not(target_os = "android"), not(target_os = "ios"))
));

/// What the sheet offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    /// "Move to the trash": the default button, and what Enter does.
    pub trash: bool,
    /// "Delete permanently": never the default, never Enter's.
    pub permanent: bool,
}

/// What the sheet offers: the trash wherever the platform has one;
/// permanent removal where it has none, and beside the trash, as a
/// choice the user has to make with the pointer, where the trash has
/// already refused files from this folder this session. A trash that
/// fails never becomes a permanent delete of its own accord.
pub fn offer(supported: bool, refused_here: bool) -> Offer {
    Offer {
        trash: supported,
        permanent: !supported || refused_here,
    }
}

/// How the files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Trash,
    Permanent,
}

/// One frame to delete, and what goes with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doomed {
    pub frame: PathBuf,
    /// The camera's JPEGs beside the raw, which go with it.
    pub companions: Vec<PathBuf>,
    pub sidecars: Vec<PathBuf>,
}

/// What a delete would touch, and what it refuses.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    pub frames: Vec<Doomed>,
    /// The frames left alone, and why.
    pub refused: Vec<(PathBuf, String)>,
}

impl Plan {
    pub fn sidecars(&self) -> usize {
        self.frames.iter().map(|d| d.sidecars.len()).sum()
    }

    pub fn companions(&self) -> usize {
        self.frames.iter().map(|d| d.companions.len()).sum()
    }
}

/// What a delete did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Deleted {
    /// The frames no longer on disk: deleted now, or found gone
    /// already between the sheet and the confirm.
    pub frames: Vec<PathBuf>,
    /// How many of them were gone already.
    pub vanished: usize,
    /// How many sidecars went with them.
    pub sidecars: usize,
    /// The camera's JPEGs beside them that went with them: the index
    /// forgets their rows with the frames'.
    pub companions: Vec<PathBuf>,
    /// The camera's JPEGs left behind when their raw went, and why: half
    /// a pair, said so.
    pub companions_left: Vec<(PathBuf, String)>,
    /// The frames that could not be deleted, and why; each is whole
    /// where it was, its sidecars with it.
    pub failed: Vec<(PathBuf, String)>,
    /// The trash refused something, and the delete stopped there.
    pub trash_refused: Option<Refusal>,
    /// The frames after a refusal, not tried.
    pub untried: usize,
}

/// What the trash refused, as found on disk afterwards: `trash`
/// stops at its first error, so part of a frame may have gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub frame: PathBuf,
    pub why: String,
    /// Whether the frame itself went (to the trash, or before).
    pub frame_gone: bool,
    /// Whether it had gone before the confirm, by another hand: the
    /// trash never had it.
    pub was_gone: bool,
    /// Its sidecars still where they were.
    pub left: Vec<PathBuf>,
    /// How many of its sidecars went to the trash.
    pub sidecars_gone: usize,
}

/// The folders a delete may reach, as the disk spells them: each of
/// `dirs` canonical. A folder that will not canonicalize (gone, or
/// not the user's to read) is left out, and so is everything in it.
pub fn allowed(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if let Ok(c) = dunce::canonicalize(d)
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
    let Ok(folder) = dunce::canonicalize(parent) else {
        return Err(tr!("{} cannot be read", parent.display()));
    };
    let hidden = std::ffi::OsStr::new(SIDECAR_FOLDER);
    let inside = allowed
        .iter()
        .any(|a| *a == folder || a.join(hidden) == folder);
    if !inside {
        return Err(tr!(
            "{} is not in the folder open",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(tr!(
            "{} is a link",
            path.file_name().unwrap_or_default().to_string_lossy()
        )),
        Ok(m) if !m.is_file() => Err(tr!(
            "{} is not a file",
            path.file_name().unwrap_or_default().to_string_lossy()
        )),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Every sidecar of `frame` that is there: its `.gcd` in both places
/// (a folder whose setting was flipped can hold both), with what saves
/// put aside beside each ([`greycard_edit::asides_among`], of `folders`'
/// names), and its XMPs.
pub fn sidecars_of(frame: &Path, folders: &mut Folders) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [Placement::Beside, Placement::Folder]
        .into_iter()
        .map(|p| Sidecar::path_in(frame, p))
        .flat_map(|gcd| {
            let names = gcd.parent().map_or(&[][..], |dir| folders.names(dir));
            let asides = greycard_edit::asides_among(&gcd, names);
            std::iter::once(gcd).chain(asides)
        })
        .chain(greycard_edit_io::xmp::paths_of(frame))
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .collect();
    out.dedup();
    out
}

/// What deleting `frames` would touch, each checked against `allowed`
/// (as [`allowed`] makes it). A frame that is refused, or any of
/// whose sidecars or companions is, stays whole: its raw never goes
/// without its sidecars or its JPEG, or the other way round. A frame
/// already gone is planned with whatever sidecars it left behind.
/// Unless `apart`, each raw takes the camera's JPEGs beside it as its
/// folder holds them now, but for a JPEG `listed` (the window's files),
/// which is a frame of its own there.
pub fn plan(frames: &[PathBuf], listed: &[PathBuf], allowed: &[PathBuf], apart: bool) -> Plan {
    let mut plan = Plan::default();
    let mut pairs = companions_of(frames, listed, apart);
    let mut folders = Folders::default();
    for frame in frames {
        if let Err(why) = deletable(frame, allowed) {
            plan.refused.push((frame.clone(), why));
            continue;
        }
        let companions = pairs.remove(frame).unwrap_or_default();
        let mut sidecars = sidecars_of(frame, &mut folders);
        for c in &companions {
            sidecars.extend(sidecars_of(c, &mut folders));
        }
        if !companions.is_empty()
            && let Some(shared) = shared_xmp(frame, &companions, &mut folders)
            && !sidecars.contains(&shared)
        {
            sidecars.push(shared);
        }
        let refused = companions
            .iter()
            .chain(&sidecars)
            .find_map(|s| deletable(s, allowed).err());
        if let Some(why) = refused {
            plan.refused.push((frame.clone(), why));
            continue;
        }
        plan.frames.push(Doomed {
            frame: frame.clone(),
            companions,
            sidecars,
        });
    }
    plan
}

/// The camera's JPEGs beside each of `raws` that go where it goes, as
/// their folders hold them now, each folder read once: none when the
/// JPEGs are listed `apart`, and none that `listed` names, which is a
/// frame of its own.
pub fn companions_of(
    raws: &[PathBuf],
    listed: &[PathBuf],
    apart: bool,
) -> HashMap<PathBuf, Vec<PathBuf>> {
    if apart {
        return HashMap::new();
    }
    let listed: HashSet<&PathBuf> = listed.iter().collect();
    greycard_library::pairs::companions_of(raws)
        .into_iter()
        .map(|(raw, with)| {
            let with: Vec<PathBuf> = with.into_iter().filter(|c| !listed.contains(c)).collect();
            (raw, with)
        })
        .filter(|(_, with)| !with.is_empty())
        .collect()
}

/// The names in each folder asked about, each folder read once.
#[derive(Default)]
pub struct Folders(HashMap<PathBuf, Vec<std::ffi::OsString>>);

impl Folders {
    pub(crate) fn names(&mut self, dir: &Path) -> &[std::ffi::OsString] {
        self.0.entry(dir.to_path_buf()).or_insert_with(|| {
            let read = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir
            };
            std::fs::read_dir(read)
                .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
                .unwrap_or_default()
        })
    }
}

/// The short XMP (`IMG.xmp`) a raw and its JPEGs share, when it is
/// there, the raw owns its stem's short names among what is in the
/// folder (`greycard_core::decode::owns_short_names`: alone, or an
/// `.ORF` there beside its `.ORI`), and nothing else of its stem is
/// there but raws it owns them over: the pair's, which goes where the
/// pair goes, though it is neither file's alone
/// ([`greycard_edit_io::xmp::paths_of`]).
pub fn shared_xmp(raw: &Path, companions: &[PathBuf], folders: &mut Folders) -> Option<PathBuf> {
    let short = greycard_edit_io::xmp::short_path(raw);
    std::fs::symlink_metadata(&short).ok()?;
    let (dir, stem) = (
        raw.parent()?,
        raw.file_stem()?.to_string_lossy().to_lowercase(),
    );
    let names = folders.names(dir);
    if !greycard_core::decode::owns_short_names(raw, names.iter().map(Path::new)) {
        return None;
    }
    let others = names.iter().any(|name| {
        let p = raw.with_file_name(name);
        let ours = p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "xmp" | "gcd" | "tmp"));
        let same = p
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem);
        same && !ours
            && p != raw
            && !companions.contains(&p)
            && !greycard_core::decode::is_raw_path(&p)
    });
    (!others).then_some(short)
}

/// Delete what `plan` names, `how`. `trash` is the system trash for
/// [`How::Trash`] (the window's `panel::delete::system_trash`, or a
/// stand-in under test) and is never called for [`How::Permanent`].
///
/// Each frame is checked again at the moment it goes: one gone since
/// the sheet counts as gone and its sidecars still go; one that has
/// become a link, or whose folder has, is refused. The trash takes a
/// frame's files in one call, the frame first. It stops at its first
/// error, so after a refusal what went is read back from the disk: a
/// frame whose raw went is counted gone even when a sidecar stayed.
/// The first refusal stops the delete, the frames after it untried,
/// and the permanent path is never taken in its place.
pub fn delete(
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
        let mut companions = Vec::new();
        for c in &d.companions {
            match deletable(c, allowed) {
                Ok(true) => companions.push(c.clone()),
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
                    .chain(companions.iter().cloned())
                    .chain(sidecars.iter().cloned())
                    .collect();
                if !all.is_empty()
                    && let Err(why) = trash(&all)
                {
                    let still = |p: &PathBuf| std::fs::symlink_metadata(p).is_ok();
                    let left_sidecars = sidecars.iter().filter(|s| still(s)).count();
                    let sidecars_gone = sidecars.len() - left_sidecars;
                    let companions_gone: Vec<PathBuf> =
                        companions.iter().filter(|c| !still(c)).cloned().collect();
                    let left: Vec<PathBuf> = companions
                        .iter()
                        .chain(&sidecars)
                        .filter(|s| still(s))
                        .cloned()
                        .collect();
                    let frame_gone = !still(&d.frame);
                    if frame_gone {
                        done.frames.push(d.frame.clone());
                        done.sidecars += sidecars_gone;
                        done.companions.extend(companions_gone);
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
                done.companions.extend(companions);
            }
            How::Permanent => {
                if there && let Err(e) = std::fs::remove_file(&d.frame) {
                    done.failed.push((d.frame.clone(), e.to_string()));
                    continue;
                }
                for c in companions {
                    match std::fs::remove_file(&c) {
                        Ok(()) => done.companions.push(c),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            tracing::warn!("{}: not deleted with its raw: {e}", c.display());
                            done.companions_left.push((c, e.to_string()));
                        }
                    }
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

/// The camera's JPEGs that go with the frames, as a list's middle
/// item: ", 2 camera JPEGs"; empty for none.
pub fn companion_words(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        tr!(", {n} camera JPEG" | ", {n} camera JPEGs" % n)
    }
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
pub fn summary(done: &Deleted, how: How, refused: &[(PathBuf, String)]) -> String {
    let mut out = if done.frames.is_empty() {
        tr!("nothing was deleted")
    } else {
        tr!(
            "{how} {frames}{companions} and {sidecars}{gone}",
            how = match how {
                How::Trash => tr!("moved to the trash:"),
                How::Permanent => tr!("deleted permanently:"),
            },
            frames = tr!("{n} frame" | "{n} frames" % done.frames.len()),
            companions = companion_words(done.companions.len()),
            sidecars = tr!("{n} sidecar" | "{n} sidecars" % done.sidecars),
            gone = if done.vanished > 0 {
                tr!(" ({} gone already)", done.vanished)
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
                    tr!("left in {folder}", folder = SIDECAR_FOLDER)
                } else {
                    tr!("left in the folder")
                };
                format!("{} ({place})", name(p))
            })
            .collect();
        let what = if r.frame_gone && left.is_empty() {
            tr!("the trash stopped at {}: {}", name(&r.frame), r.why)
        } else if r.was_gone {
            tr!(
                "{} was gone already, and the trash refused {}: {}",
                name(&r.frame),
                left.join(", "),
                r.why
            )
        } else if r.frame_gone {
            tr!(
                "the trash took {} but refused {}: {}",
                name(&r.frame),
                left.join(", "),
                r.why
            )
        } else if r.sidecars_gone > 0 {
            tr!(
                "the trash refused {} ({}) after taking {} of it",
                name(&r.frame),
                r.why,
                tr!("{n} sidecar" | "{n} sidecars" % r.sidecars_gone)
            )
        } else {
            tr!("the trash refused {} ({})", name(&r.frame), r.why)
        };
        out.push_str(&format!("; {what}"));
        let stayed = usize::from(!r.frame_gone) + done.untried;
        if stayed > 0 {
            out.push_str(&tr!(
                "; {n} frame still where it was: ask again to delete permanently"
                    | "; {n} frames still where they were: ask again to delete permanently"
                        % stayed
            ));
        }
    }
    let not: Vec<&(PathBuf, String)> = refused.iter().chain(&done.failed).collect();
    if let Some((frame, why)) = not.first() {
        out.push_str(&tr!(
            "; {n} frame not deleted ({name}: {why}{each})"
                | "; {n} frames not deleted ({name}: {why}{each})" % not.len(),
            name = name(frame),
            why = why,
            each = if not.len() > 1 {
                tr!("; the log has each")
            } else {
                String::new()
            }
        ));
    }
    if let Some((jpeg, why)) = done.companions_left.first() {
        out.push_str(&tr!(
            "; {n} camera JPEG left where it was when its raw went ({name}: {why})"
                | "; {n} camera JPEGs left where they were when their raws went ({name}: {why})"
                    % done.companions_left.len(),
            name = name(jpeg),
            why = why
        ));
    }
    out
}

/// Take away a folder a delete emptied: its hidden sidecar folder,
/// then the folder itself, each only when there is nothing left in
/// it. For the rejects folder, which the move of the rejects made.
pub fn remove_if_empty(dir: &Path) {
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
        greycard_edit_io::xmp::save(&files[0], &s.meta, None).unwrap();
        std::fs::write(
            greycard_edit_io::xmp::long_path(&files[0]),
            greycard_edit_io::xmp::fresh(&s.meta, None),
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
        let p = plan(&chosen, &[], &ok, false);
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
        greycard_render::testing::remove_dir_retry(&dir);
    }

    /// What saves put aside beside a frame's `.gcd`, in either place
    /// (other writers' files kept, files that would not parse), goes to
    /// the trash with it; names of the shape that are another frame's,
    /// or none of ours, stay.
    #[test]
    fn what_saves_put_aside_goes_to_the_trash_with_the_frame() {
        let dir = scratch("asides");
        let bin = dir.join("bin");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(shoot_dir.join(greycard_edit::SIDECAR_FOLDER)).unwrap();
        let files = shoot(&shoot_dir);
        for name in [
            "A.CR3.gcd.other",
            "A.CR3.gcd.3.other",
            "A.CR3.gcd.bad",
            ".greycard/A.CR3.gcd.2.other",
            "A.CR3.gcd.1.other",
            "AA.CR3.gcd.other",
            "C.CR3.gcd.other",
        ] {
            std::fs::write(shoot_dir.join(name), b"x").unwrap();
        }
        let ok = allowed(std::slice::from_ref(&shoot_dir));
        let p = plan(&[files[0].clone()], &[], &ok, false);
        assert_eq!(
            p.sidecars(),
            3 + 4,
            "the .gcd, two XMPs, and four set aside"
        );
        let trashed = std::cell::RefCell::new(Vec::new());
        let done = delete(&p, How::Trash, &ok, |paths: &[PathBuf]| {
            for p in paths {
                let to = bin.join(trashed.borrow().len().to_string());
                greycard_core::testing::rename_retry(p, &to);
                trashed
                    .borrow_mut()
                    .push(p.strip_prefix(&shoot_dir).unwrap().to_path_buf());
            }
            Ok(())
        });
        assert_eq!(done.sidecars, 7);
        let mut trashed = trashed.into_inner();
        trashed.sort();
        let want: Vec<PathBuf> = [
            ".greycard/A.CR3.gcd.2.other",
            "A.CR3",
            "A.CR3.gcd",
            "A.CR3.gcd.3.other",
            "A.CR3.gcd.bad",
            "A.CR3.gcd.other",
            "A.CR3.xmp",
            "A.xmp",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(trashed, want);
        for stays in ["A.CR3.gcd.1.other", "AA.CR3.gcd.other", "C.CR3.gcd.other"] {
            assert!(shoot_dir.join(stays).exists(), "{stays}");
        }
        greycard_render::testing::remove_dir_retry(&dir);
    }

    /// A raw takes the camera's JPEG beside it, the JPEG's own sidecar
    /// and the short XMP the two share, said in the summary; apart, or
    /// with the JPEG chosen as a frame of its own, the raw goes alone.
    #[test]
    fn a_raw_takes_its_jpeg_and_their_shared_xmp_unless_apart() {
        let dir = scratch("pair");
        for name in [
            "P.CR3",
            "P.JPG",
            "P.xmp",
            "P.JPG.gcd",
            "Q.CR3",
            "Q.JPG",
            "R.CR3",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let ok = allowed(std::slice::from_ref(&dir));
        let p = plan(&[dir.join("P.CR3"), dir.join("R.CR3")], &[], &ok, false);
        assert!(p.refused.is_empty(), "{:?}", p.refused);
        assert_eq!(p.frames[0].companions, [dir.join("P.JPG")]);
        assert!(p.frames[1].companions.is_empty());
        assert_eq!(p.companions(), 1);
        assert_eq!(p.sidecars(), 2, "the JPEG's .gcd and the pair's XMP");
        let done = delete(&p, How::Permanent, &ok, never);
        assert_eq!(
            (done.frames.len(), done.companions.len(), done.sidecars),
            (2, 1, 2)
        );
        assert_eq!(all(&dir), ["Q.CR3", "Q.JPG"]);
        assert_eq!(
            summary(&done, How::Permanent, &[]),
            "deleted permanently: 2 frames, 1 camera JPEG and 2 sidecars"
        );
        // Half a pair, a JPEG that would not go with its raw, is said.
        let half = Deleted {
            companions_left: vec![(dir.join("P.JPG"), "denied".into())],
            ..done.clone()
        };
        assert!(
            summary(&half, How::Permanent, &[])
                .ends_with("; 1 camera JPEG left where it was when its raw went (P.JPG: denied)"),
            "{}",
            summary(&half, How::Permanent, &[])
        );
        // Apart, the raw is planned alone; and a JPEG chosen beside its
        // raw is a frame of its own, not the raw's.
        let p = plan(&[dir.join("Q.CR3")], &[], &ok, true);
        assert!(p.frames[0].companions.is_empty());
        let both = [dir.join("Q.CR3"), dir.join("Q.JPG")];
        let p = plan(&both, &both, &ok, false);
        assert_eq!(p.frames.len(), 2);
        assert_eq!(p.companions(), 0);
        greycard_render::testing::remove_dir_retry(&dir);
    }

    /// The first `.ORF` in `GREYCARD_SAMPLES` with its `.ORI` beside
    /// it, copied to a scratch folder with a stem XMP: deleted with the
    /// ORI there, the XMP goes with the ORF; with the ORF gone first,
    /// it stays, nobody's. Nothing is touched beside the samples.
    #[test]
    #[ignore = "reads the folder named by GREYCARD_SAMPLES"]
    fn the_samples_orf_deleted_beside_its_ori() {
        let Some(samples) = std::env::var_os("GREYCARD_SAMPLES").map(PathBuf::from) else {
            eprintln!("set GREYCARD_SAMPLES to a folder with an ORF and its ORI");
            return;
        };
        let mut orfs: Vec<PathBuf> = std::fs::read_dir(&samples)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("orf")))
            .filter(|p| p.with_extension("ORI").is_file())
            .collect();
        orfs.sort();
        let orf = orfs.first().expect("an ORF with its ORI");
        for gone_first in [false, true] {
            let dir = scratch("samples-ori");
            let (here, ori) = (
                dir.join(orf.file_name().unwrap()),
                dir.join(orf.with_extension("ORI").file_name().unwrap()),
            );
            std::fs::copy(orf, &here).unwrap();
            std::fs::copy(orf.with_extension("ORI"), &ori).unwrap();
            let xmp = greycard_edit_io::xmp::short_path(&here);
            std::fs::write(&xmp, b"<x/>").unwrap();
            if gone_first {
                std::fs::remove_file(&here).unwrap();
            }
            let ok = allowed(std::slice::from_ref(&dir));
            let p = plan(std::slice::from_ref(&here), &[], &ok, false);
            let done = delete(&p, How::Permanent, &ok, never);
            println!(
                "ORF {}: {} sidecars went; left {:?}",
                if gone_first { "gone first" } else { "there" },
                done.sidecars,
                all(&dir)
            );
            assert_eq!(xmp.exists(), gone_first);
            assert!(ori.exists());
            greycard_render::testing::remove_dir_retry(&dir);
        }
    }

    /// A High Res Shot's merge goes with its JPEG and the stem's XMP;
    /// its single frame stays, with its own sidecars, and takes none of
    /// the merge's.
    #[test]
    fn an_orf_takes_the_stems_xmp_and_leaves_its_ori_whole() {
        let dir = scratch("ori");
        for name in [
            "P.ORF",
            "P.ORI",
            "P.JPG",
            "P.xmp",
            "P.ORF.gcd",
            "P.ORI.gcd",
            "P.ORI.xmp",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let ok = allowed(std::slice::from_ref(&dir));
        let ori = plan(&[dir.join("P.ORI")], &[], &ok, false);
        assert!(ori.frames[0].companions.is_empty());
        assert_eq!(ori.sidecars(), 2, "its .gcd and its own XMP");
        let p = plan(&[dir.join("P.ORF")], &[], &ok, false);
        assert_eq!(p.frames[0].companions, [dir.join("P.JPG")]);
        assert_eq!(p.sidecars(), 2, "its .gcd and the stem's XMP");
        delete(&p, How::Permanent, &ok, never);
        assert_eq!(all(&dir), ["P.ORI", "P.ORI.gcd", "P.ORI.xmp"]);
        greycard_render::testing::remove_dir_retry(&dir);

        // The ORF gone from the disk already: a delete of the ORF
        // takes its `.gcd` alone, and the ORI's takes its own only;
        // the stem's XMP is neither's and stays.
        let dir = scratch("ori-gone");
        for name in ["P.ORI", "P.ORI.gcd", "P.xmp", "P.ORF.gcd"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let ok = allowed(std::slice::from_ref(&dir));
        let p = plan(&[dir.join("P.ORF")], &[], &ok, false);
        assert_eq!(p.sidecars(), 1, "its .gcd, not the stem's XMP");
        delete(&p, How::Permanent, &ok, never);
        assert_eq!(all(&dir), ["P.ORI", "P.ORI.gcd", "P.xmp"]);
        let ori = plan(&[dir.join("P.ORI")], &[], &ok, false);
        assert_eq!(ori.sidecars(), 1, "its .gcd alone");
        delete(&ori, How::Permanent, &ok, never);
        assert_eq!(all(&dir), ["P.xmp"]);
        greycard_render::testing::remove_dir_retry(&dir);
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
        let p = plan(&[away.clone(), sneaky, files[2].clone()], &[], &ok, false);
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
            let p = plan(&[link.clone(), files[3].clone()], &[], &ok, false);
            assert!(p.frames.is_empty(), "{p:?}");
            assert!(p.refused[0].1.contains("is a link"));
            assert!(p.refused[1].1.contains("is a link"));
            // A hidden folder that is a link to elsewhere: B's sidecar
            // is outside, and B stays.
            let hidden = shoot_dir.join(SIDECAR_FOLDER);
            greycard_core::testing::rename_retry(&hidden, dir.join("hidden"));
            std::os::unix::fs::symlink(dir.join("hidden"), &hidden).unwrap();
            let p = plan(std::slice::from_ref(&files[1]), &[], &ok, false);
            assert!(p.frames.is_empty(), "{p:?}");
            assert!(link.exists() && away.exists() && files[3].exists());
            assert!(files[1].exists());
        }
        greycard_render::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_frame_gone_between_the_sheet_and_the_confirm_is_counted_gone() {
        let dir = scratch("vanished");
        let files = shoot(&dir);
        let ok = allowed(std::slice::from_ref(&dir));
        let p = plan(&[files[0].clone(), files[2].clone()], &[], &ok, false);
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
        greycard_render::testing::remove_dir_retry(&dir);
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
                greycard_core::testing::rename_retry(p, bin.join(p.file_name().unwrap()));
            }
            Ok(())
        };
        let chosen = [files[0].clone(), files[1].clone(), files[2].clone()];
        let p = plan(&chosen, &[], &ok, false);
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
        greycard_render::testing::remove_dir_retry(&dir);
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
            greycard_core::testing::rename_retry(
                &paths[0],
                bin.join(paths[0].file_name().unwrap()),
            );
            Err("the sidecar would not go".to_string())
        };
        // B, whose one sidecar is under the hidden folder, then C.
        let p = plan(&[files[1].clone(), files[2].clone()], &[], &ok, false);
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
        greycard_render::testing::remove_dir_retry(&dir);
    }

    /// A raw gone before the confirm, and the trash refusing the
    /// sidecar it left: the frame is counted gone already, and the
    /// status does not say the trash took it.
    #[test]
    fn a_raw_gone_before_and_its_sidecar_refused_is_not_said_as_trashed() {
        let dir = scratch("gone-refused");
        let files = shoot(&dir);
        let ok = allowed(std::slice::from_ref(&dir));
        let p = plan(std::slice::from_ref(&files[0]), &[], &ok, false);
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
        greycard_render::testing::remove_dir_retry(&dir);
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
                companions: Vec::new(),
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
            let p = plan(&files[..2], &[], &ok, false);
            assert_eq!(p.frames.len(), 2);
            std::fs::write(elsewhere.join("B.CR3"), b"raw").unwrap();
            greycard_core::testing::rename_retry(&shoot_dir, dir.join("moved"));
            std::os::unix::fs::symlink(&elsewhere, &shoot_dir).unwrap();
            let done = delete(&p, How::Permanent, &ok, never);
            assert!(done.frames.is_empty());
            assert_eq!(done.failed.len(), 2);
            assert!(away.exists() && elsewhere.join("B.CR3").exists());
            assert!(dir.join("moved").join("A.CR3").exists());
        }
        greycard_render::testing::remove_dir_retry(&dir);
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
        greycard_render::testing::remove_dir_retry(&dir);
    }
}
