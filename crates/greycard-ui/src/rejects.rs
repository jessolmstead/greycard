//! The move of the rejects to a folder beside the shoot, and back:
//! each frame with its sidecars, its XMPs, and the camera's JPEG beside
//! a raw, never a delete and never a file of another frame's written
//! over. Pure over the disk and the paths handed in, so the culling
//! window and the archive's rejects run it alike.

use std::path::{Path, PathBuf};

use crate::{delete, tr};

/// The folder the rejects go to: beside the frames, named for what
/// they are.
pub fn rejects_dir(shoot: &Path) -> PathBuf {
    shoot.join(REJECTS)
}

/// The rejects folder's name.
pub const REJECTS: &str = "rejects";

/// What a move of the rejects did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// The files moved, by their index in the list given.
    pub files: Vec<usize>,
    /// Where each of `files` went, in the same order.
    pub to: Vec<PathBuf>,
    /// How many of them had a sidecar that went with them.
    pub sidecars: usize,
    /// The camera's JPEGs that went with their raws, each from where
    /// to where.
    pub companions: Vec<(PathBuf, PathBuf)>,
    /// The camera's JPEGs that stayed when their raws went, and why:
    /// half a pair, said so.
    pub companions_left: Vec<(PathBuf, String)>,
    /// The files left where they were, and why.
    pub skipped: Vec<(usize, String)>,
}

/// What a move does with a sidecar already in the rejects folder under
/// one of the frame's names, with no raw of the name beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orphans {
    /// The frame's own sidecar goes over it: the shoot's folder is the
    /// user's working folder, and the orphan is an earlier cull's.
    WriteOver,
    /// The frame stays whole where it is and the clash is named: on an
    /// archive nothing is ever written over.
    Keep,
}

/// Move the files at `rejected` (indices into `files`), each with its
/// sidecar when it has one and the camera's JPEG beside a raw, into
/// the rejects folder beside them, with what to do about an orphaned
/// sidecar in the rejects folder said, and whether the camera's JPEG
/// beside a raw is listed `apart`, a frame of its own that stays
/// unless chosen. Never a delete: a file of the same name already
/// there is left alone and the frame stays, said so in `skipped`. The
/// folder is made if it is not there.
pub fn move_rejects_as(
    files: &[PathBuf],
    rejected: &[usize],
    orphans: Orphans,
    apart: bool,
) -> anyhow::Result<Moved> {
    let mut moved = Moved::default();
    // Each frame into the rejects folder of its own folder: a list
    // from more than one (the library's all-roots view) sends each
    // shoot's rejects to that shoot's, never all to the first one's.
    let mut by_shoot: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for &i in rejected {
        let Some(raw) = files.get(i) else {
            continue;
        };
        let shoot = raw
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // Already out: a frame in a rejects folder (the all-roots view
        // lists those too) stays there, rather than going one folder
        // deeper each time the rejects are moved.
        if above_rejects(raw).is_some() {
            moved
                .skipped
                .push((i, tr!("it is in a rejects folder already")));
            continue;
        }
        match by_shoot.iter_mut().find(|(s, _)| *s == shoot) {
            Some((_, group)) => group.push(i),
            None => by_shoot.push((shoot, vec![i])),
        }
    }
    for (shoot, group) in by_shoot {
        let dir = rejects_dir(&shoot);
        // A shoot whose rejects folder cannot be made (a folder that
        // is not the user's to write) keeps its rejects where they are,
        // said; the other shoots' go on.
        if let Err(e) = std::fs::create_dir_all(&dir) {
            let why = format!("{}: {e}", dir.display());
            moved
                .skipped
                .extend(group.iter().map(|&i| (i, why.clone())));
            continue;
        }
        move_into(files, &group, &dir, orphans, apart, &mut moved);
    }
    Ok(moved)
}

/// Move the frames at `chosen` (indices into `files`) that are in a
/// rejects folder back into the folder above it, each with its
/// sidecar and its XMPs: the inverse of [`move_rejects_as`]. A sidecar
/// under the rejects folder's hidden folder goes under the folder
/// above's own, one beside stays beside. Never a delete and never a
/// file of another frame's written over: a raw of the name in the
/// folder above, or a sidecar of one of its names there with a raw it
/// belongs to beside it, leaves the frame whole where it is, said in
/// `skipped`. A sidecar there of its names with no raw beside it is
/// this frame's own, left when it was culled, and the frame's sidecar
/// goes over it, as [`Orphans::WriteOver`] has it. A frame not in a
/// rejects folder is left and said. The camera's JPEG beside a raw goes
/// back with it unless the JPEGs are listed `apart`.
pub fn move_back(files: &[PathBuf], chosen: &[usize], apart: bool) -> Moved {
    let mut moved = Moved::default();
    let mut by_folder: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for &i in chosen {
        let Some(raw) = files.get(i) else {
            continue;
        };
        let Some(above) = above_rejects(raw) else {
            moved
                .skipped
                .push((i, tr!("it is not in a rejects folder")));
            continue;
        };
        match by_folder.iter_mut().find(|(d, _)| *d == above) {
            Some((_, group)) => group.push(i),
            None => by_folder.push((above, vec![i])),
        }
    }
    for (above, group) in by_folder {
        move_into(files, &group, &above, Orphans::WriteOver, apart, &mut moved);
    }
    moved
}

/// The folder above the rejects folder `raw` is in, where Move back
/// takes it; None when `raw` is not in a rejects folder.
pub fn above_rejects(raw: &Path) -> Option<PathBuf> {
    let dir = raw.parent()?;
    if dir.file_name() != Some(std::ffi::OsStr::new(REJECTS)) {
        return None;
    }
    Some(
        dir.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
    )
}

/// The file in `dir` that answers to the stem of the short XMP name
/// `xmp` (`IMG.xmp`), by its name: a picture there of that stem, the
/// camera's `IMG.JPG` beside a raw, which the XMP is then that file's
/// or nobody's. Stems are matched without regard to case, as the disks
/// of two of the three platforms match them, and sidecars of ours and
/// files part-written (`.tmp`) are no picture. A folder that cannot be
/// read is taken to hold one. Only the short name has this question:
/// a `.gcd` and the long XMP name (`IMG.CR3.xmp`) carry their raw's
/// whole name, and no other file answers to it.
///
/// The files this same move brought in (`arrived`) are passed over:
/// a raw and the camera's JPEG of one shot moved together share the
/// short XMP as they did where they were, and the one that went first
/// is no other frame's claim on it.
///
/// `raw` is where the frame the XMP goes with lands. When it owns its
/// stem's short names among what will be there
/// (`greycard_core::decode::owns_short_names`: an `.ORF` arriving
/// beside its `.ORI`) and no short XMP is there yet, the other raws
/// there answer to nothing. One that is there already beside a raw of
/// the stem is answered to all the same: what wrote it while the frame
/// was away cannot be told from the names, so it is never written over.
fn stem_owner(xmp: &Path, dir: &Path, arrived: &[PathBuf], raw: &Path) -> Option<String> {
    let stem = xmp.file_stem()?.to_string_lossy().to_lowercase();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Some(tr!("{} (it cannot be read)", dir.display()));
    };
    let names: Vec<std::ffi::OsString> = entries.flatten().map(|e| e.file_name()).collect();
    let landed = raw.file_name().map(std::ffi::OsStr::to_os_string);
    let owns = !xmp.exists()
        && greycard_core::decode::owns_short_names(
            raw,
            names.iter().chain(landed.as_ref()).map(Path::new),
        );
    names.into_iter().find_map(|name| {
        if arrived
            .iter()
            .any(|a| a.file_name() == Some(name.as_os_str()))
        {
            return None;
        }
        let path = Path::new(&name);
        if owns && greycard_core::decode::is_raw_path(path) {
            return None;
        }
        let ours = path
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "xmp" | "gcd" | "tmp"));
        let same = path
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem);
        (same && !ours).then(|| name.to_string_lossy().into_owned())
    })
}

/// Whether `from`, one of `raw`'s XMPs, is its short name (`IMG.xmp`)
/// rather than its long one (`IMG.CR3.xmp`).
fn is_short_xmp(from: &Path, raw: &Path) -> bool {
    from.extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("xmp"))
        && from.file_stem() == raw.file_stem()
}

/// The frames at `group`, all in one folder, into the folder `dir`:
/// its rejects folder, or for Move back the folder above. A raw takes
/// the camera's JPEGs beside it, their sidecars and the short XMP the
/// pair shares, unless `apart` or the JPEG is in `files` itself, a
/// frame of its own there.
fn move_into(
    files: &[PathBuf],
    group: &[usize],
    dir: &Path,
    orphans: Orphans,
    apart: bool,
    moved: &mut Moved,
) {
    // The raws this call has moved into `dir` so far.
    let mut arrived: Vec<PathBuf> = Vec::new();
    // Their camera's JPEGs, and their folder's names, read once.
    let raws: Vec<PathBuf> = group
        .iter()
        .filter_map(|&i| files.get(i).cloned())
        .collect();
    let mut pairs = delete::companions_of(&raws, files, apart);
    let mut folders = delete::Folders::default();
    for &i in group {
        let Some(raw) = files.get(i) else {
            continue;
        };
        let Some(name) = raw.file_name() else {
            moved.skipped.push((i, tr!("no file name")));
            continue;
        };
        let dest = dir.join(name);
        // Everything that belongs to this frame and has to travel
        // with it: the `.gcd`, wherever the frame keeps it, and the
        // XMPs whose names are this frame's (both, when a folder has
        // been through two tools). A sidecar under the hidden folder
        // goes under the rejects' own, so the placement survives the
        // cull; the XMPs are always beside.
        let sidecar = greycard_edit::Sidecar::find(raw);
        let hidden = std::ffi::OsStr::new(greycard_edit::SIDECAR_FOLDER);
        // The camera's JPEGs that go with it, and what is theirs: their
        // own sidecars, and the short XMP the pair shares, which is
        // neither file's alone where they are and stays the pair's.
        let companions = pairs.remove(raw).unwrap_or_default();
        // What saves put aside beside each `.gcd` (other writers' files
        // kept, files that would not parse) goes with it, under the
        // same name; left behind, a later frame of the name would take
        // them for its own.
        let mut asides = Vec::new();
        for gcd in sidecar.iter().cloned().chain(
            companions
                .iter()
                .filter_map(|c| greycard_edit::Sidecar::find(c)),
        ) {
            let names = gcd.parent().map_or(&[][..], |d| folders.names(d));
            asides.extend(greycard_edit::asides_among(&gcd, names));
        }
        let theirs: Vec<PathBuf> = companions
            .iter()
            .flat_map(|c| {
                greycard_edit::Sidecar::find(c)
                    .into_iter()
                    .chain(greycard_edit_io::xmp::paths_of(c))
            })
            .chain(
                (!companions.is_empty())
                    .then(|| delete::shared_xmp(raw, &companions, &mut folders))
                    .flatten(),
            )
            .collect();
        let beside: Vec<(PathBuf, PathBuf)> = sidecar
            .iter()
            .cloned()
            .chain(greycard_edit_io::xmp::paths_of(raw))
            .chain(theirs)
            .chain(asides)
            .filter_map(|from| {
                let file = from.file_name()?;
                let to = if from.parent().and_then(Path::file_name) == Some(hidden) {
                    dir.join(hidden).join(file)
                } else {
                    dir.join(file)
                };
                Some((from, to))
            })
            .collect();
        // The frame's `.gcd` names there, under either placement: a
        // reader takes the newer of the two (`Sidecar::find`), so one
        // left under the other placement would shadow the frame's own.
        let gcd = greycard_edit::Sidecar::path_for(&dest);
        let gcds = [
            gcd.clone(),
            dir.join(hidden).join(gcd.file_name().unwrap_or_default()),
        ];
        // The raw's name there already: the frame stays whole where
        // it is, rather than its raw going and one of the files beside
        // it not. A sidecar of its names there with no raw of the name
        // beside it is what an earlier move left when the frame was
        // dragged out by hand (the sidecar under the hidden folder is
        // easy to miss): an orphan of this very frame, which its own
        // sidecar, the one that carries the flag now, writes over. The
        // folder is the truth, and a sidecar with no frame is nothing.
        // The short XMP name is the one a file there can answer to (the
        // camera's JPEG of the stem): written over it would be that
        // file's lost, and moved in beside it with none there it would
        // be nobody's, the frame's ratings lost to every reader; either
        // way the frame stays whole and the file is named.
        let companion_there = companions
            .iter()
            .filter_map(|c| c.file_name())
            .find(|n| dir.join(n).exists());
        let taken = if dest.exists() {
            Some(tr!(
                "{} is in {} already",
                name.to_string_lossy(),
                dir.display()
            ))
        } else if let Some(n) = companion_there {
            Some(tr!(
                "{} is in {} already",
                n.to_string_lossy(),
                dir.display()
            ))
        } else {
            let keep = orphans == Orphans::Keep;
            let there = |to: &Path| {
                tr!(
                    "{} is in {} already",
                    to.file_name().unwrap_or_default().to_string_lossy(),
                    dir.display()
                )
            };
            gcds.iter()
                .find(|to| keep && to.exists())
                .map(|to| there(to))
                .or_else(|| {
                    beside.iter().find_map(|(from, to)| {
                        if is_short_xmp(from, raw)
                            && let Some(owner) = stem_owner(to, dir, &arrived, &dest)
                        {
                            return Some(if to.exists() {
                                there(to)
                            } else {
                                tr!(
                                    "{owner} there answers to {} too",
                                    to.file_name().unwrap_or_default().to_string_lossy(),
                                    owner = owner
                                )
                            });
                        }
                        (keep && to.exists()).then(|| there(to))
                    })
                })
        };
        if let Some(why) = taken {
            moved.skipped.push((i, why));
            continue;
        }
        if let Err(e) = std::fs::rename(raw, &dest) {
            moved.skipped.push((i, e.to_string()));
            continue;
        }
        moved.files.push(i);
        moved.to.push(dest.clone());
        arrived.push(dest.clone());
        for c in &companions {
            let Some(n) = c.file_name() else {
                continue;
            };
            let to = dir.join(n);
            match std::fs::rename(c, &to) {
                Ok(()) => {
                    arrived.push(to.clone());
                    moved.companions.push((c.clone(), to));
                }
                Err(e) => {
                    tracing::warn!("{}: not moved with its raw: {e}", c.display());
                    moved.companions_left.push((c.clone(), e.to_string()));
                }
            }
        }
        // An orphan under the placement the frame's own sidecar does
        // not take goes, so it cannot shadow it; one under the same
        // placement is written over below. A frame with no sidecar of
        // its own leaves an orphan there alone, and takes it up: it is
        // this frame's, and the only edit it has.
        if let Some(own) = sidecar.as_ref() {
            let lands = beside
                .iter()
                .find(|(from, _)| from == own)
                .map(|(_, to)| to);
            for other in gcds.iter().filter(|g| Some(*g) != lands && g.exists()) {
                match std::fs::remove_file(other) {
                    Ok(()) => tracing::info!(
                        "{}: an orphan of this frame under the other placement, taken away",
                        other.display()
                    ),
                    Err(e) => tracing::warn!("{}: orphan not taken away: {e}", other.display()),
                }
            }
        }
        for (from, to) in &beside {
            if to.parent().is_some_and(|p| !p.exists())
                && let Err(e) = greycard_edit::Sidecar::folder_under(dir)
            {
                tracing::warn!("{}: not moved: {e}", from.display());
                continue;
            }
            if to.exists() {
                tracing::info!("{}: an orphan of this frame, written over", to.display());
            }
            match std::fs::rename(from, to) {
                Ok(()) if Some(from) == sidecar.as_ref() => moved.sidecars += 1,
                Ok(()) => {}
                Err(e) => tracing::warn!("{}: not moved: {e}", from.display()),
            }
        }
    }
}
