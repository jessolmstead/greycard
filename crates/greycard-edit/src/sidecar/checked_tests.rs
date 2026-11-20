//! The save that asks the disk first ([`Sidecar::save_checked`]).

use super::*;
use crate::testing::{exposed, scratch};

pub(super) fn stamp(at: u64) -> sync::Stamp {
    sync::Stamp {
        at,
        host: "desk".into(),
    }
}

/// A stamp later than any file's mtime: for a pair with a file from
/// before revisions, whose time is its mtime.
pub(super) fn later() -> sync::Stamp {
    stamp(sync::Stamp::now().at + 100)
}

/// A frame with a sidecar saved at 0.25, and that sidecar as a window
/// reads it.
fn saved(name: &str) -> (PathBuf, PathBuf, Sidecar) {
    let dir = scratch(name);
    let raw = dir.join("IMG_0001.CR3");
    let mut s = Sidecar::default();
    s.record(exposed(0.25));
    s.save_in_stamped(&raw, Placement::Beside, &stamp(100))
        .unwrap();
    let read = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    (dir, raw, read)
}

/// A frame whose sidecar was last written by a build from before
/// revisions, with `states` exposures, and that sidecar as read.
fn saved_before_revisions(name: &str, states: &[f32]) -> (PathBuf, PathBuf, Sidecar) {
    let dir = scratch(name);
    let raw = dir.join("IMG_0001.CR3");
    let mut base = Sidecar::default();
    for &ev in states {
        base.record(exposed(ev));
    }
    base.write_to(&Sidecar::path_for(&raw)).unwrap();
    let read = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    assert!(read.revisions.is_empty());
    (dir, raw, read)
}

pub(super) fn has_state(s: &Sidecar, pick: impl Fn(&Edit) -> bool) -> bool {
    s.history.iter().any(|st| pick(&st.edit)) || pick(&s.current)
}

fn labels(s: &Sidecar) -> Vec<String> {
    s.history
        .iter()
        .map(|h| h.label.clone().unwrap_or_default())
        .chain(std::iter::once(s.current_label.clone().unwrap_or_default()))
        .collect()
}

fn no_changes(s: &Sidecar, raw: &Path) -> bool {
    s.changed_on_disk(raw, Placement::Beside).is_empty()
}

/// Another writer's save over `raw`'s sidecar: read, `change`d, and
/// saved at `at`. The sidecar it saved.
pub(super) fn other_saves(
    raw: &Path,
    placement: Placement,
    at: u64,
    change: impl FnOnce(&mut Sidecar),
) -> Sidecar {
    let mut other = Sidecar::load(raw).unwrap().unwrap();
    change(&mut other);
    other.save_in_stamped(raw, placement, &stamp(at)).unwrap();
    other
}

/// The other writer's preset: grain laid over the current state.
pub(super) fn preset(o: &mut Sidecar) {
    let mut grain = o.current.clone();
    grain.grain.amount = 0.4;
    o.record_as(grain, Some("Preset: Fine Grain".into()));
}

/// Rewrite the file at `path` with its own bytes, as a sync tool copying
/// it back does: a new inode and mtime, the same content.
pub(super) fn rewrite_same_bytes(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::write(path, &bytes).unwrap();
}

/// A file as it was read is written over, and is then the one known.
#[test]
fn a_file_as_it_was_read_is_written() {
    let (dir, raw, mut s) = saved("checked-plain");
    assert!(no_changes(&s, &raw));
    s.record(exposed(0.5));
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Written);
    assert!(no_changes(&s, &raw));
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), s);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Only a rating given here while another writer laid a preset: the
/// preset stays current, the history the file's, the rating joined.
#[test]
fn a_rating_here_keeps_another_writers_preset_current() {
    let (dir, raw, mut s) = saved("checked-rating");
    let other = other_saves(&raw, Placement::Beside, 150, preset);
    s.meta.rating = 3;
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Joined);
    assert_eq!(s.current.grain.amount, 0.4);
    assert_eq!(labels(&s), labels(&other));
    assert_eq!(s.meta.rating, 3);
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), s);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A save with nothing changed here (a scheduled save) while another
/// writer laid a preset: the file taken whole, nothing written.
#[test]
fn a_save_of_nothing_takes_another_writers_file() {
    let (dir, raw, mut s) = saved("checked-noop");
    let other = other_saves(&raw, Placement::Beside, 150, preset);
    let bytes = std::fs::read(Sidecar::path_for(&raw)).unwrap();
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Took);
    assert_eq!(s, other);
    assert_eq!(s.current.grain.amount, 0.4);
    let now = std::fs::read(Sidecar::path_for(&raw)).unwrap();
    assert_eq!(now, bytes, "nothing written");
    assert!(no_changes(&s, &raw));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file rewritten with the very bytes we wrote (a sync tool copying
/// it back; a stat a made-up inode got wrong): written as a plain save
/// would write it, nothing joined.
#[test]
fn a_false_change_writes_what_a_plain_save_would() {
    let (dir, raw, mut s) = saved("checked-false");
    rewrite_same_bytes(&Sidecar::path_for(&raw));
    let mut plain = s.clone();
    s.record(exposed(0.5));
    plain.record(exposed(0.5));
    assert!(!no_changes(&s, &raw));
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Written);
    plain.advance(&stamp(200));
    assert_eq!(s, plain);
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), plain);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file from before revisions rewritten with its own bytes while an
/// undo, a version deleted or a version renamed was made here: each is
/// this sidecar's own, and is what is written and kept.
#[test]
fn a_change_here_stands_against_a_same_bytes_rewrite_of_an_old_file() {
    let (dir, raw, mut s) = saved_before_revisions("checked-undo", &[0.25, 0.5]);
    assert!(s.undo());
    rewrite_same_bytes(&Sidecar::path_for(&raw));
    let checked = s.save_checked(&raw, Placement::Beside, &later());
    assert_eq!(checked.unwrap(), Checked::Written);
    assert_eq!(s.current.light.exposure, 0.25, "the undo kept");
    let back = Sidecar::load(&raw).unwrap().unwrap();
    assert_eq!(back.current.light.exposure, 0.25);
    greycard_core::testing::remove_dir_retry(&dir);

    for rename in [false, true] {
        let dir = scratch("checked-version");
        let raw = dir.join("IMG_0001.CR3");
        let path = Sidecar::path_for(&raw);
        let mut base = Sidecar::default();
        base.record(exposed(0.25));
        let id = base.new_version(None, Some("Copy")).unwrap();
        base.write_to(&path).unwrap();
        let mut s = Sidecar::read(&path).unwrap();
        if rename {
            assert!(s.rename_version(&id, "Mono"));
        } else {
            assert!(s.delete_version(&id).is_some());
        }
        rewrite_same_bytes(&path);
        let checked = s.save_checked(&raw, Placement::Beside, &later());
        assert_eq!(checked.unwrap(), Checked::Written, "rename {rename}");
        let back = Sidecar::load(&raw).unwrap().unwrap();
        if rename {
            assert_eq!(back.versions[0].name, "Mono");
        } else {
            assert!(back.versions.is_empty());
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// A file restored to an older copy of ours, a slider here since: ours
/// is ahead of it, and written over it.
#[test]
fn a_file_put_back_to_an_older_copy_is_written_over() {
    let (dir, raw, mut s) = saved("checked-rollback");
    let path = Sidecar::path_for(&raw);
    let old = std::fs::read(&path).unwrap();
    s.record(exposed(0.5));
    s.save_checked(&raw, Placement::Beside, &stamp(200))
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, &old).unwrap();
    s.meta.rating = 2;
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(300));
    assert_eq!(checked.unwrap(), Checked::Written);
    assert_eq!(s.current.light.exposure, 0.5);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file another writer deleted: put back.
#[test]
fn a_deleted_file_is_written_again() {
    let (dir, raw, mut s) = saved("checked-deleted");
    std::fs::remove_file(Sidecar::path_for(&raw)).unwrap();
    s.record(exposed(0.5));
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Written);
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), s);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file a writer with the other setting moved under `.greycard/` and
/// changed there: found, joined, and left in our place alone.
#[test]
fn a_file_moved_to_the_other_place_is_joined() {
    let (dir, raw, mut s) = saved("checked-moved");
    other_saves(&raw, Placement::Folder, 150, |o| o.meta.rating = 2);
    assert!(!Sidecar::path_for(&raw).exists());
    s.record(exposed(0.5));
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Joined);
    assert_eq!(s.meta.rating, 2);
    assert!(!Sidecar::path_in(&raw, Placement::Folder).exists());
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), s);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file from a newer build, or past the size a save reads, is never
/// written over: the save fails, the file is as it was, and the edit is
/// still in hand.
#[test]
fn a_newer_or_oversized_file_is_not_written_over() {
    let newer = "{\"current\": {\"version\": 99, \"light\": {\"exposure\": 2.0}}}";
    let (dir, raw, mut s) = saved("checked-newer");
    let path = Sidecar::path_for(&raw);
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, newer).unwrap();
    s.record(exposed(0.5));
    let failed = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert!(matches!(failed, Err(crate::Error::Newer(99))), "{failed:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    assert_eq!(s.current.light.exposure, 0.5);
    greycard_core::testing::remove_dir_retry(&dir);

    // Past the bound (a test's, in bytes): not read, not written over.
    let (dir, raw, mut s) = saved("checked-huge");
    let path = Sidecar::path_for(&raw);
    let bound = std::fs::metadata(&path).unwrap().len() + 10;
    checked::TEST_READ_MOST.set(bound);
    std::fs::remove_file(&path).unwrap();
    let big = "x".repeat(bound as usize + 1);
    std::fs::write(&path, &big).unwrap();
    s.record(exposed(0.5));
    let refused = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), big, "{refused:?}");
    assert!(refused.is_err());
    // A sidecar past it that is the one this knew is told by its bytes,
    // before the bound, and saved over as ever.
    std::fs::remove_file(&path).unwrap();
    s.write_known(&raw, Placement::Beside).unwrap();
    checked::TEST_READ_MOST.set(10);
    rewrite_same_bytes(&path);
    s.record(exposed(0.75));
    let saved = s.save_checked(&raw, Placement::Beside, &stamp(300));
    checked::TEST_READ_MOST.set(checked::READ_MOST);
    assert_eq!(saved.unwrap(), Checked::Written);
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), s);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file that will not parse (torn, or not a sidecar) is set aside as
/// `.bad` and ours written; a second one goes beside the first, which
/// stays as it was.
#[test]
fn a_torn_file_is_set_aside_and_ours_written() {
    let (dir, raw, mut s) = saved("checked-torn");
    let path = Sidecar::path_for(&raw);
    let bad = |n: &str| path.with_file_name(format!("IMG_0001.CR3.gcd{n}.bad"));
    for (round, torn) in ["{\"current\": {\"light\": ", "not json"]
        .iter()
        .enumerate()
    {
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, torn).unwrap();
        s.record(exposed(0.5 + round as f32));
        let checked = s.save_checked(&raw, Placement::Beside, &stamp(200 + round as u64));
        assert_eq!(checked.unwrap(), Checked::Written);
        assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), s);
    }
    assert_eq!(
        std::fs::read_to_string(bad("")).unwrap(),
        "{\"current\": {\"light\": "
    );
    assert_eq!(std::fs::read_to_string(bad(".2")).unwrap(), "not json");
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A frame that had no sidecar when it was read (its start known, as a
/// fresh sidecar's is), given one since by another writer: taken when
/// nothing was done here, kept beside the frame when something was.
#[test]
fn a_file_made_since_a_read_that_found_none() {
    for ours in [false, true] {
        let dir = scratch("checked-made");
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar::default();
        s.remember_fresh();
        let mut other = Sidecar::default();
        other.record(exposed(-1.0));
        other.meta.rating = 5;
        other
            .save_in_stamped(&raw, Placement::Beside, &stamp(150))
            .unwrap();
        if ours {
            s.record(exposed(0.5));
        }
        let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
        if ours {
            assert_eq!(checked.unwrap(), Checked::Kept);
            assert!(!has_state(&s, |e| e.light.exposure == -1.0));
            assert_eq!((s.current.light.exposure, s.meta.rating), (0.5, 0));
            assert_eq!(kept_beside(&Sidecar::path_for(&raw)).len(), 1);
        } else {
            assert_eq!(s.meta.rating, 5);
            assert_eq!(checked.unwrap(), Checked::Took);
            assert_eq!(s.current.light.exposure, -1.0);
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// Another writer's save dated the same as ours, as a clock with a grain
/// of two seconds dates them: still seen, by the length or the inode.
#[test]
fn a_change_within_one_tick_is_still_seen() {
    let (dir, raw, mut s) = saved("checked-tick");
    let path = Sidecar::path_for(&raw);
    let known = s.on_disk.clone().unwrap().stat.unwrap();
    other_saves(&raw, Placement::Beside, 100, |o| o.meta.rating = 3);
    if let Some(at) = known.modified {
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }
    s.record(exposed(0.5));
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(100));
    assert_eq!(checked.unwrap(), Checked::Joined);
    assert_eq!(s.meta.rating, 3);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Where the stat has no inode (Windows): a rewrite in place to the same
/// length is seen by its mtime. Not seen there: the same, within one
/// tick of the file system's clock (100 ns on NTFS, two seconds on FAT).
#[test]
fn with_no_inode_a_same_length_rewrite_is_seen_by_its_time() {
    let (dir, raw, mut s) = saved("checked-no-inode");
    let path = Sidecar::path_for(&raw);
    if let Some(k) = s.on_disk.as_mut().and_then(|k| k.stat.as_mut()) {
        k.inode = None;
    }
    let known = s.on_disk.clone().unwrap().stat.unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(known.modified.unwrap() + std::time::Duration::from_secs(5))
        .unwrap();
    drop(f);
    assert_eq!(
        s.changed_on_disk(&raw, Placement::Beside),
        vec![path.clone()]
    );
    // And with no inode on one side, the rest the same, it is the same.
    let mut now = FileStat::of(&path).unwrap();
    now.modified = known.modified;
    assert!(known.same(&now));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// An undo twice here and saved, another writer's rating, then a flag
/// here: the other changed only the meta, so the window's sidecar is
/// kept with the session's redo stack, the meta joined.
#[test]
fn a_meta_change_on_disk_keeps_the_redo_stack() {
    let (dir, raw, mut s) = saved("checked-redo");
    s.record(exposed(0.5));
    s.record(exposed(0.75));
    s.save_checked(&raw, Placement::Beside, &stamp(110))
        .unwrap();
    s.undo();
    s.undo();
    s.save_checked(&raw, Placement::Beside, &stamp(120))
        .unwrap();
    assert_eq!(s.redo.len(), 2);
    other_saves(&raw, Placement::Beside, 150, |o| o.meta.rating = 5);
    s.meta.flag = crate::meta::Flag::Pick;
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Joined);
    assert_eq!(s.redo.len(), 2, "the redo stack kept");
    assert_eq!((s.meta.rating, s.meta.flag), (5, crate::meta::Flag::Pick));
    assert!(s.redo());
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap().meta.rating, 5);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file that is not UTF-8 is torn like one that is not JSON: set
/// aside, and ours written.
#[test]
fn a_file_that_is_not_utf8_is_set_aside() {
    let (dir, raw, mut s) = saved("checked-utf8");
    let path = Sidecar::path_for(&raw);
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"{\"current\": \xff\xfe").unwrap();
    s.record(exposed(1.0));
    let saved = s
        .save_checked_said(&raw, Placement::Beside, &stamp(200))
        .unwrap();
    assert_eq!(saved.checked, Checked::Written);
    assert_eq!(
        saved.asides.bad,
        vec![path.with_file_name("IMG_0001.CR3.gcd.bad")]
    );
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap(), s);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file exactly as last known is told by its bytes, without a parse,
/// and a save leaves the file it wrote as the one known.
#[test]
fn a_file_as_known_is_told_by_its_bytes() {
    let (dir, raw, mut s) = saved("checked-bytes");
    let path = Sidecar::path_for(&raw);
    rewrite_same_bytes(&path);
    assert!(s.as_known(&path));
    std::fs::write(&path, b"{}").unwrap();
    assert!(!s.as_known(&path));
    s.record(exposed(1.0));
    s.save_checked(&raw, Placement::Beside, &stamp(200))
        .unwrap();
    assert!(s.as_known(&path));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The frame moved, its file with it, and another file made where it
/// was: the save for the new name does not read the old name's file,
/// and finds its own as known.
#[test]
fn a_known_file_of_another_name_is_not_read() {
    let (dir, raw, mut s) = saved("checked-moved-name");
    let moved = dir.join("IMG_0002.CR3");
    std::fs::rename(Sidecar::path_for(&raw), Sidecar::path_for(&moved)).unwrap();
    let mut stranger = Sidecar::default();
    stranger.record(exposed(2.0));
    stranger
        .save_in_stamped(&raw, Placement::Beside, &stamp(150))
        .unwrap();
    let theirs = std::fs::read(Sidecar::path_for(&raw)).unwrap();
    s.record(exposed(0.5));
    let checked = s.save_checked(&moved, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Written);
    assert!(!has_state(&s, |e| e.light.exposure == 2.0));
    assert_eq!(std::fs::read(Sidecar::path_for(&raw)).unwrap(), theirs);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A copy's sidecar written over the frame's own is the file the frame's
/// next save knows: found as known by a stat, nothing read.
#[test]
fn a_file_written_as_known_is_not_read_again() {
    let (dir, raw, s) = saved("checked-write-known");
    let mut copy = s;
    copy.record(exposed(0.5));
    copy.advance(&stamp(150));
    copy.write_to_known(&Sidecar::path_for(&raw)).unwrap();
    let mut s2 = copy.clone();
    s2.record(exposed(0.75));
    let reads = super::checked::changed_reads();
    let checked = s2.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Written);
    assert_eq!(super::checked::changed_reads(), reads, "nothing read");
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A state at `ev` cropped, for the turn's tests.
fn cropped_at(ev: f32) -> Edit {
    let mut e = exposed(ev);
    e.geometry.crop = Some(crate::geometry::Crop::of([0.1, 0.2, 0.5, 0.6]));
    e
}

/// Every state's crop in `s` is the one its turn makes of the crop
/// first set, on any of four turns.
fn crops_agree(s: &Sidecar) -> bool {
    let at = |q: u8| {
        let mut e = cropped_at(0.0);
        e.turn(i32::from(q), None);
        e.geometry.crop
    };
    let want = at(s.turn);
    s.edits()
        .all(|e| e.geometry.crop.is_none() || e.geometry.crop == want)
}

/// A frame that had no file, its start an edit of this machine's own
/// settings, and a file another writer made meanwhile from its own
/// start: nothing done here takes the file whole, a rating here joins
/// only the meta, and the start here is in neither.
#[test]
fn a_fresh_start_takes_a_file_made_meanwhile() {
    for rated in [false, true] {
        let dir = scratch(&format!("checked-fresh-{rated}"));
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar {
            line: Line::of(exposed(0.1)),
            ..Sidecar::default()
        };
        s.remember_fresh();
        if rated {
            s.meta.rating = 3;
        }
        let mut o = Sidecar::default();
        preset(&mut o);
        o.save_in_stamped(&raw, Placement::Beside, &stamp(150))
            .unwrap();
        let checked = s.save_checked(&raw, Placement::Beside, &later()).unwrap();
        let want = if rated {
            Checked::Joined
        } else {
            Checked::Took
        };
        assert_eq!(checked, want);
        assert_eq!(s.current.grain.amount, 0.4);
        assert!(!has_state(&s, |e| e.light.exposure == 0.1), "{rated}");
        assert_eq!(s.meta.rating, if rated { 3 } else { 0 });
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
/// The other writers' files kept beside `path`, oldest first.
pub(super) fn kept_beside(path: &Path) -> Vec<PathBuf> {
    super::checked::kept_others(path)
        .unwrap()
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

/// Both changed the lines: a slider here, another writer's preset
/// there. Ours is written as it is, and the other's file kept beside the
/// frame byte for byte.
#[test]
fn both_lines_changed_keeps_the_others_file() {
    let (dir, raw, mut s) = saved("checked-both-lines");
    let path = Sidecar::path_for(&raw);
    other_saves(&raw, Placement::Beside, 150, preset);
    let theirs = std::fs::read(&path).unwrap();
    s.record(exposed(1.0));
    let mine = s.clone();
    let saved = s
        .save_checked_said(&raw, Placement::Beside, &later())
        .unwrap();
    assert_eq!(saved.checked, Checked::Kept);
    let to = path.with_file_name("IMG_0001.CR3.gcd.other");
    assert_eq!(saved.asides.kept, vec![to.clone()]);
    assert_eq!(std::fs::read(&to).unwrap(), theirs, "theirs kept as it was");
    assert_eq!((&s.line, &s.versions), (&mine.line, &mine.versions));
    assert!(!has_state(&s, |e| e.grain.amount == 0.4));
    assert_eq!(Sidecar::load(&raw).unwrap().unwrap().to_json(), s.to_json());
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file from a build before revisions that went on from ours: whether
/// it went on cannot be shown, so it is kept, and ours written.
#[test]
fn an_old_builds_file_is_kept_not_taken() {
    let (dir, raw, mut s) = saved_before_revisions("checked-old-build", &[0.25]);
    let path = Sidecar::path_for(&raw);
    let mut ahead = s.clone();
    ahead.record(exposed(1.0));
    std::fs::remove_file(&path).unwrap();
    ahead.write_to(&path).unwrap();
    let theirs = std::fs::read(&path).unwrap();
    let checked = s.save_checked(&raw, Placement::Beside, &later());
    assert_eq!(checked.unwrap(), Checked::Kept);
    assert_eq!(s.current.light.exposure, 0.25);
    assert_eq!(std::fs::read(&kept_beside(&path)[0]).unwrap(), theirs);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Another writer turns the frame (every state's crop turned in place);
/// the window then saves after nothing (their file taken whole), a
/// rating (taken, the rating joined into it), or a slider (theirs kept,
/// ours written). Every time, the turn and the crops agree.
#[test]
fn a_turn_elsewhere_is_taken_or_kept_whole() {
    for (here, want) in [
        ("nothing", Checked::Took),
        ("rating", Checked::Joined),
        ("slider", Checked::Kept),
    ] {
        let dir = scratch(&format!("checked-turn-elsewhere-{here}"));
        let raw = dir.join("IMG_0001.CR3");
        let mut s = Sidecar::default();
        s.record(cropped_at(0.25));
        s.record(cropped_at(0.5));
        s.take_snapshot("snap", 1);
        s.save_in_stamped(&raw, Placement::Beside, &stamp(100))
            .unwrap();
        let mut s = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
        let theirs = other_saves(&raw, Placement::Beside, 150, |o| o.turn_by(1, Some(1.5)));
        match here {
            "rating" => s.meta.rating = 3,
            "slider" => {
                s.record(cropped_at(1.0));
            }
            _ => {}
        }
        let checked = s.save_checked(&raw, Placement::Beside, &later()).unwrap();
        assert_eq!(checked, want, "{here}");
        let file = Sidecar::load(&raw).unwrap().unwrap();
        for (what, sc) in [("memory", &s), ("file", &file)] {
            assert_eq!(
                sc.turn,
                if here == "slider" { 0 } else { 1 },
                "{here}: {what}"
            );
            assert!(crops_agree(sc), "{here}: the crops in {what}");
        }
        if here != "slider" {
            assert_eq!((&s.line, &s.versions), (&theirs.line, &theirs.versions));
        }
        assert_eq!(s.meta.rating, if here == "rating" { 3 } else { 0 });
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// A turn here and a rating there: the other changed only the meta, so
/// ours is kept, turned, with their rating.
#[test]
fn a_turn_here_and_a_rating_there_join_the_meta() {
    let dir = scratch("checked-turn-here");
    let raw = dir.join("IMG_0001.CR3");
    let mut s = Sidecar::default();
    s.record(cropped_at(0.25));
    s.save_in_stamped(&raw, Placement::Beside, &stamp(100))
        .unwrap();
    let mut s = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    other_saves(&raw, Placement::Beside, 150, |o| o.meta.rating = 5);
    s.turn_by(1, None);
    let checked = s.save_checked(&raw, Placement::Beside, &later());
    assert_eq!(checked.unwrap(), Checked::Joined);
    assert_eq!((s.turn, s.meta.rating), (1, 5));
    assert!(crops_agree(&s));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A value set in place here (no new state, the ids as they were) and
/// a rating there: told by content, the lines here changed and the
/// file's did not, so ours is kept with their rating. Against a slider
/// there, both changed the lines: theirs kept beside, ours written.
#[test]
fn an_in_place_change_here_is_told_by_content() {
    for (there, want) in [("rating", Checked::Joined), ("slider", Checked::Kept)] {
        let (dir, raw, mut s) = saved(&format!("checked-in-place-here-{there}"));
        s.line.current.light.tone.contrast = 0.3;
        other_saves(&raw, Placement::Beside, 150, |o| match there {
            "rating" => o.meta.rating = 5,
            _ => {
                o.record(exposed(0.9));
            }
        });
        let checked = s.save_checked(&raw, Placement::Beside, &later());
        assert_eq!(checked.unwrap(), want, "{there}");
        assert_eq!(s.current.light.tone.contrast, 0.3, "{there}");
        let file = Sidecar::load(&raw).unwrap().unwrap();
        assert_eq!(file.to_json(), s.to_json(), "{there}");
        greycard_core::testing::remove_dir_retry(&dir);
    }
}

/// A value set in place there, nothing here: their file is taken whole.
#[test]
fn an_in_place_change_there_is_taken() {
    let (dir, raw, mut s) = saved("checked-in-place-there");
    other_saves(&raw, Placement::Beside, 150, |o| {
        o.line.current.light.tone.contrast = 0.3;
    });
    let checked = s.save_checked(&raw, Placement::Beside, &later());
    assert_eq!(checked.unwrap(), Checked::Took);
    assert_eq!(s.current.light.tone.contrast, 0.3);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Another writer's save between ours and the moment ours is
/// remembered: the stat ours keeps is the staged file's, so the next
/// save finds the file changed, and keeps it rather than write over it
/// unseen.
#[test]
fn a_write_right_after_ours_is_not_taken_for_ours() {
    let (dir, raw, mut s) = saved("checked-after-rename");
    s.record(exposed(0.5));
    super::checked::AFTER_RENAME.set(Some(|path| {
        let raw = path.with_extension("");
        let mut o = Sidecar::load(&raw).unwrap().unwrap();
        o.record(exposed(9.0));
        o.save_in_stamped(&raw, Placement::Beside, &stamp(150))
            .unwrap();
    }));
    let first = s.save_checked(&raw, Placement::Beside, &stamp(140));
    assert_eq!(first.unwrap(), Checked::Written);
    s.record(exposed(0.75));
    let checked = s.save_checked(&raw, Placement::Beside, &stamp(200));
    assert_eq!(checked.unwrap(), Checked::Kept);
    let kept = kept_beside(&Sidecar::path_for(&raw));
    let theirs = Sidecar::read(&kept[0]).unwrap();
    assert!(has_state(&theirs, |e| e.light.exposure == 9.0));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Another writer's copy in the other place, made since: read, and
/// kept beside it before the save's removal of the copy there.
#[test]
fn a_copy_left_in_the_other_place_is_kept_before_it_goes() {
    let (dir, raw, mut s) = saved("checked-other-place");
    let folder = Sidecar::path_in(&raw, Placement::Folder);
    let mut o = Sidecar::load(&raw).unwrap().unwrap();
    preset(&mut o);
    o.advance(&stamp(150));
    o.write_to(&folder).unwrap();
    let theirs = std::fs::read(&folder).unwrap();
    s.record(exposed(1.0));
    let saved = s
        .save_checked_said(&raw, Placement::Beside, &later())
        .unwrap();
    assert!(!folder.exists(), "the copy there removed");
    let kept = kept_beside(&folder);
    assert_eq!(saved.asides.kept, kept);
    assert_eq!(std::fs::read(&kept[0]).unwrap(), theirs);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The other writers' files kept beside one frame: the same bytes kept
/// once, and no more than [`super::checked::KEEP_OTHERS`], the newest.
#[test]
fn kept_copies_are_bounded_and_kept_once() {
    let dir = scratch("checked-kept-bounded");
    let path = dir.join("IMG_0001.CR3.gcd");
    let mut names = Vec::new();
    for k in 0..7 {
        std::fs::write(&path, format!("copy {k}")).unwrap();
        names.push(super::checked::keep_other(&path).unwrap());
    }
    assert_eq!(
        super::checked::keep_other(&path).unwrap(),
        names[6],
        "kept once"
    );
    let kept = kept_beside(&path);
    assert_eq!(kept.len(), super::checked::KEEP_OTHERS);
    assert_eq!(kept, names[7 - super::checked::KEEP_OTHERS..]);
    for (k, p) in kept.iter().enumerate() {
        let n = 7 - super::checked::KEEP_OTHERS + k;
        assert_eq!(std::fs::read_to_string(p).unwrap(), format!("copy {n}"));
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Two windows that both changed the frame, then saving in turn for
/// forty rounds: the first save keeps the other's file, the other takes
/// the result, and from then on each takes or writes; nothing grows,
/// and the two hold the same.
#[test]
fn two_windows_saving_in_turn_settle() {
    let (dir, raw, mut s) = saved("checked-two-windows");
    let mut o = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    s.record(exposed(1.0));
    o.record(exposed(-1.0));
    o.save_checked(&raw, Placement::Beside, &later()).unwrap();
    let mut outcomes = Vec::new();
    let size = |x: &Sidecar| (x.history.len(), x.redo.len(), x.snapshots.len());
    let mut sizes = Vec::new();
    for k in 0..40u64 {
        let a = s.save_checked(
            &raw,
            Placement::Beside,
            &stamp(sync::Stamp::now().at + 200 + 2 * k),
        );
        let b = o.save_checked(
            &raw,
            Placement::Beside,
            &stamp(sync::Stamp::now().at + 201 + 2 * k),
        );
        outcomes.push((a.unwrap(), b.unwrap()));
        sizes.push((size(&s), size(&o)));
    }
    assert_eq!(outcomes[0], (Checked::Kept, Checked::Took));
    assert!(
        outcomes[1..].iter().all(|o| matches!(
            o,
            (
                Checked::Took | Checked::Written,
                Checked::Took | Checked::Written
            )
        )),
        "{outcomes:?}"
    );
    assert!(sizes.windows(2).all(|w| w[0] == w[1]), "{sizes:?}");
    assert_eq!((&s.line, &s.versions), (&o.line, &o.versions));
    assert_eq!(kept_beside(&Sidecar::path_for(&raw)).len(), 1);
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The lines' print: the same for two files that differ in the meta,
/// the turn, the saves, the lineage, and the marks of files kept aside
/// only; another for any change to
/// a state, a label (quotes and braces in it too), a snapshot, or a
/// version; none for bytes that are not one object.
#[test]
fn the_lines_print_reads_the_lines_alone() {
    use super::checked::lines_print;
    let mut s = Sidecar::default();
    s.record(exposed(0.25));
    s.take_snapshot("snap", 1);
    s.new_version(None, Some("V2"));
    let print = |s: &Sidecar| lines_print(s.to_json().as_bytes()).unwrap();
    let base = print(&s);
    let mut meta = s.clone();
    meta.meta.rating = 4;
    meta.turn = 1;
    meta.advance(&stamp(300));
    meta.saved += 3;
    meta.set_aside
        .push("00112233445566778899aabbccddeeff".into());
    assert_eq!(print(&meta), base);
    let mut label = s.clone();
    label.record_as(
        exposed(0.5),
        Some("a \"label\" with {braces}, [and] commas".into()),
    );
    let mut snap = s.clone();
    snap.take_snapshot("another", 2);
    let mut version = s.clone();
    let id = version.versions[0].id.clone();
    version.rename_version(&id, "Renamed");
    let mut in_place = s.clone();
    in_place.line.current.light.tone.contrast = 0.3;
    for (what, other) in [
        ("label", &label),
        ("snapshot", &snap),
        ("version", &version),
        ("in place", &in_place),
    ] {
        assert_ne!(print(other), base, "{what}");
    }
    let mut back = label.clone();
    back.meta.rating = 2;
    assert_eq!(print(&back), print(&label));
    for bad in ["", "[1]", "{\"current\": ", "{\"a\": \"unended}"] {
        assert_eq!(lines_print(bad.as_bytes()), None, "{bad}");
    }
}

fn has_ev(s: &Sidecar, ev: f32) -> bool {
    s.lines().any(|l| {
        l.current.light.exposure == ev || l.history.iter().any(|h| h.edit.light.exposure == ev)
    })
}

fn kept_read(raw: &Path) -> Vec<Sidecar> {
    kept_beside(&Sidecar::path_for(raw))
        .iter()
        .map(|p| Sidecar::read(p).unwrap())
        .collect()
}

/// A file kept aside is never lineage. The archive sync, which already
/// holds the other writer's file, finds ours apart from it (Diverged)
/// and joins the two, the verdict a last-writer-wins save gets; read as
/// ahead, ours would be written over the other's copy.
#[test]
fn a_kept_file_is_not_lineage_to_the_archive_sync() {
    use crate::sync::{Compared, compare, join};
    let (dir, raw, mut w) = saved("checked-kept-not-lineage");
    let b = other_saves(&raw, Placement::Beside, 150, |o| {
        o.record(exposed(-1.0));
    });
    let archive = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    assert_eq!(compare(&archive, &b), Compared::Same);
    w.record(exposed(0.9));
    let mut last_writer = w.clone();
    let at = stamp(200);
    assert_eq!(
        w.save_checked(&raw, Placement::Beside, &at).unwrap(),
        Checked::Kept
    );
    last_writer.advance(&at);
    let here = Sidecar::load(&raw).unwrap().unwrap();
    assert!(!has_ev(&here, -1.0));
    assert_eq!(here.set_aside, [b.revisions.last().unwrap().hash.clone()]);
    assert_eq!(compare(&here, &archive), compare(&last_writer, &archive));
    assert_eq!(compare(&here, &archive), Compared::Diverged);
    let joined = join(&here, &archive);
    assert!(has_ev(&joined, -1.0) && has_ev(&joined, 0.9));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Past the bound, the first kept copy leaves the pile; the archive's
/// copy of that file does not, since ours is still apart from it and
/// the sync joins it in.
#[test]
fn the_archive_keeps_what_the_pile_lets_go() {
    use crate::sync::{Compared, compare, join};
    let (dir, raw, mut w) = saved("checked-pile-archive");
    other_saves(&raw, Placement::Beside, 150, |o| {
        o.record(exposed(-1.0));
    });
    let archive = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    w.record(exposed(0.9));
    assert_eq!(
        w.save_checked(&raw, Placement::Beside, &stamp(200))
            .unwrap(),
        Checked::Kept
    );
    for k in 0..super::checked::KEEP_OTHERS as u64 {
        other_saves(&raw, Placement::Beside, 300 + 10 * k, |o| {
            o.record(exposed(2.0 + k as f32));
        });
        w.record(exposed(0.1 * k as f32));
        assert_eq!(
            w.save_checked(&raw, Placement::Beside, &stamp(305 + 10 * k))
                .unwrap(),
            Checked::Kept
        );
    }
    let kept = kept_read(&raw);
    assert_eq!(kept.len(), super::checked::KEEP_OTHERS);
    assert!(!kept.iter().any(|k| has_ev(k, -1.0)), "left the pile");
    let here = Sidecar::load(&raw).unwrap().unwrap();
    assert_eq!(compare(&here, &archive), Compared::Diverged);
    assert!(has_ev(&join(&here, &archive), -1.0));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// The window whose file was kept aside, with only a rating of its own
/// since, takes the file written over it, its rating joined in: its own
/// slider leaves the window and is in the kept copy. No worse than a
/// last-writer-wins save, whose file would hold the slider and lose the
/// other window's: here both are on disk.
#[test]
fn a_window_with_a_rating_takes_the_file_its_own_was_kept_under() {
    let (dir, raw, mut a) = saved("checked-kept-then-rating");
    let mut b = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    b.record(exposed(-1.0));
    b.save_checked(&raw, Placement::Beside, &stamp(150))
        .unwrap();
    a.record(exposed(0.9));
    assert_eq!(
        a.save_checked(&raw, Placement::Beside, &stamp(200))
            .unwrap(),
        Checked::Kept
    );
    b.meta.rating = 4;
    assert_eq!(
        b.save_checked(&raw, Placement::Beside, &stamp(250))
            .unwrap(),
        Checked::Joined
    );
    assert!(!has_ev(&b, -1.0) && has_ev(&b, 0.9) && b.meta.rating == 4);
    let file = Sidecar::load(&raw).unwrap().unwrap();
    assert!(has_ev(&file, 0.9) && file.meta.rating == 4);
    let kept = kept_read(&raw);
    assert!(kept.len() == 1 && has_ev(&kept[0], -1.0));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A build from before the marks of files kept aside drops them at its
/// next save. The window whose file was kept then cannot tell the file
/// went on from its own, and keeps it in turn rather than take it, as
/// before the marks: nothing lost, one copy more.
#[test]
fn an_older_build_drops_the_marks_and_the_file_is_kept_again() {
    let (dir, raw, mut a) = saved("checked-old-build-marks");
    let path = Sidecar::path_for(&raw);
    let mut b = Sidecar::read(&path).unwrap();
    b.record(exposed(-1.0));
    b.save_checked(&raw, Placement::Beside, &stamp(150))
        .unwrap();
    a.record(exposed(0.9));
    assert_eq!(
        a.save_checked(&raw, Placement::Beside, &stamp(200))
            .unwrap(),
        Checked::Kept
    );
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(v.as_object_mut().unwrap().remove("set_aside").is_some());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    assert_eq!(
        b.save_checked(&raw, Placement::Beside, &stamp(250))
            .unwrap(),
        Checked::Kept
    );
    let file = Sidecar::load(&raw).unwrap().unwrap();
    assert!(has_ev(&file, -1.0) && !has_ev(&file, 0.9));
    let kept = kept_read(&raw);
    assert_eq!(kept.len(), 2);
    assert!(has_ev(&kept[0], -1.0) && has_ev(&kept[1], 0.9));
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Forty conflicts on one frame: the pile never passes the bound, and
/// holds the four newest other files, numbered on.
#[test]
fn the_pile_holds_the_newest_over_many_conflicts() {
    let n = 40u64;
    let (dir, raw, mut a) = saved("checked-pile-many");
    for k in 0..n {
        other_saves(&raw, Placement::Beside, 300 + 10 * k, |o| {
            o.record(exposed(2.0 + k as f32));
        });
        a.record(exposed(-0.01 * k as f32));
        assert_eq!(
            a.save_checked(&raw, Placement::Beside, &stamp(305 + 10 * k))
                .unwrap(),
            Checked::Kept
        );
        assert!(kept_beside(&Sidecar::path_for(&raw)).len() <= super::checked::KEEP_OTHERS);
    }
    let kept = kept_read(&raw);
    let newest = (n - super::checked::KEEP_OTHERS as u64)..n;
    assert_eq!(kept.len(), super::checked::KEEP_OTHERS);
    for (s, k) in kept.iter().zip(newest) {
        assert_eq!(s.current.light.exposure, 2.0 + k as f32);
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// Four hundred files kept beside one sidecar: the four newest stay,
/// numbered on from the highest; a copy of any of them is kept once;
/// and names of the same shape that are no copy of ours are left alone.
#[test]
fn the_pile_is_listed_by_exact_name() {
    use super::checked::{KEEP_OTHERS, keep_other, kept_others};
    let dir = scratch("checked-pile-names");
    let path = dir.join("IMG_0001.CR3.gcd");
    let strays = [
        "IMG_0001.CR3.gcd.1.other",
        "IMG_0001.CR3.gcd.02.other",
        "IMG_0001.CR3.gcd.x.other",
        "IMG_0001.CR3.gcd.other.tmp",
        "IMG_0001.CR3.gcdx.other",
        "IMG_0001.CR3.gcd..other",
        "IMG_0001.CR.gcd.other",
    ];
    for s in strays {
        std::fs::write(dir.join(s), "stray").unwrap();
    }
    let mut last = PathBuf::new();
    for k in 0..400u32 {
        std::fs::write(&path, format!("copy {k}")).unwrap();
        last = keep_other(&path).unwrap();
        assert!(kept_others(&path).unwrap().len() <= KEEP_OTHERS);
    }
    let kept = kept_others(&path).unwrap();
    let numbers: Vec<u32> = kept.iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, [397, 398, 399, 400]);
    assert_eq!(kept[3].1, last);
    for (k, (_, p)) in (396..).zip(&kept) {
        assert_eq!(std::fs::read_to_string(p).unwrap(), format!("copy {k}"));
    }
    std::fs::write(&path, "copy 397").unwrap();
    assert_eq!(keep_other(&path).unwrap(), kept[1].1, "kept once");
    assert_eq!(kept_others(&path).unwrap(), kept);
    for s in strays {
        assert_eq!(
            std::fs::read_to_string(dir.join(s)).unwrap(),
            "stray",
            "{s}"
        );
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A folder that cannot take the kept copy (read-only): the save is
/// refused and the other's file is as it was.
#[cfg(unix)]
#[test]
fn a_copy_that_cannot_be_kept_refuses_the_save() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, raw, mut s) = saved("checked-read-only");
    other_saves(&raw, Placement::Beside, 150, |o| {
        o.record(exposed(-1.0));
    });
    let theirs = std::fs::read(Sidecar::path_for(&raw)).unwrap();
    s.record(exposed(0.9));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = s.save_checked_said(&raw, Placement::Beside, &stamp(200));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let now = std::fs::read(Sidecar::path_for(&raw)).unwrap();
    greycard_core::testing::remove_dir_retry(&dir);
    assert!(r.is_err());
    assert_eq!(now, theirs);
}

/// Two windows alternating real edits, eight rounds each: each one's
/// every state is on disk at the end, in the file or a kept copy.
#[test]
fn two_windows_alternating_edits_lose_nothing_latest() {
    let (dir, raw, mut a) = saved("checked-alternating");
    let mut b = Sidecar::read(&Sidecar::path_for(&raw)).unwrap();
    for k in 0..8u64 {
        a.record(exposed(10.0 + k as f32));
        a.save_checked(&raw, Placement::Beside, &stamp(200 + 4 * k))
            .unwrap();
        b.record(exposed(-10.0 - k as f32));
        b.save_checked(&raw, Placement::Beside, &stamp(202 + 4 * k))
            .unwrap();
    }
    let mut files = kept_read(&raw);
    files.push(Sidecar::load(&raw).unwrap().unwrap());
    let lost: Vec<f32> = (0..8)
        .flat_map(|k| [10.0 + k as f32, -10.0 - k as f32])
        .filter(|&ev| !files.iter().any(|f| has_ev(f, ev)))
        .collect();
    greycard_core::testing::remove_dir_retry(&dir);
    assert!(lost.is_empty(), "lost {lost:?}");
}

/// An older build's writer (no revisions, compact JSON) and a window
/// changing nothing, in turn for ten rounds: each save keeps the other's
/// file, and the pile stays at the bound.
#[test]
fn an_older_build_in_turn_stays_within_the_bound() {
    let (dir, raw, mut s) = saved("checked-old-build-turns");
    let path = Sidecar::path_for(&raw);
    for k in 0..10u64 {
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for f in ["revisions", "absorbed", "set_aside", "meta_at", "saved"] {
            v.as_object_mut().unwrap().remove(f);
        }
        v["current"]["light"]["exposure"] = serde_json::json!(-2.0 - k as f32);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(
            s.save_checked(&raw, Placement::Beside, &stamp(1000 + k))
                .unwrap(),
            Checked::Kept
        );
        assert!(kept_beside(&path).len() <= super::checked::KEEP_OTHERS);
    }
    greycard_core::testing::remove_dir_retry(&dir);
}

/// A file this build would write otherwise (compact JSON, an older
/// build's) opened, nothing changed in the window, and another window
/// lays a preset: the save cannot tell nothing was done here, its print
/// not the file's, and keeps the preset aside rather than take it.
#[test]
fn an_unchanged_window_over_an_old_shape_keeps_rather_than_takes() {
    let (dir, raw, _) = saved("checked-old-shape");
    let path = Sidecar::path_for(&raw);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let mut s = Sidecar::read(&path).unwrap();
    other_saves(&raw, Placement::Beside, 150, preset);
    assert_eq!(
        s.save_checked(&raw, Placement::Beside, &stamp(200))
            .unwrap(),
        Checked::Kept
    );
    assert!(kept_read(&raw)[0].current.grain.amount == 0.4);
    greycard_core::testing::remove_dir_retry(&dir);
}
