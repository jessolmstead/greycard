//! An export noted in a frame's sidecar on disk, for a frame no window
//! holds: the headless export's every frame, and one the window has let
//! go of since its export began.

use std::path::Path;

use greycard_edit::{Edit, Exported, Placement, Sidecar};
use greycard_edit_io::on_disk::{
    FrameShape, Fresh, Trouble, file_name, line_never_developed, migrate_from_file,
    read_sidecar_fresh,
};
use greycard_edit_io::xmp;

pub fn not_kept(source: &Path) {
    log::info!(
        "{}: exported under an edit that is no state of its history \
         (undone and replaced since, or the command line's); not recorded",
        file_name(source)
    );
}

/// The version the export was of is no longer in the sidecar (deleted,
/// or taken out by a join since the set was queued): nothing to record
/// it on.
pub fn no_version(source: &Path, version: Option<&str>) {
    log::warn!(
        "{}: no version {}; export not recorded",
        file_name(source),
        version.unwrap_or("(the file's own)")
    );
}

/// The window's `record_export` for a frame no window holds: its sidecar read
/// from where it is and brought up to date as an open brings it, the
/// export noted on the state that is `edit`, and written back where
/// `placement` says, the frame's `.xmp` with it when `xmp` asks, as
/// every sidecar write of the window's does (`write_sidecar`); true
/// when it was written. The window's for a frame it has let go of,
/// and the headless `--export`'s for every frame. `shape` says the
/// frame's shape where a state waits on it: the window's mapped header
/// read on its own thread, the unmapped one off it (§724).
pub fn record_export_on_disk(
    source: &Path,
    edit: &Edit,
    version: Option<&str>,
    exported: Exported,
    placement: Placement,
    xmp: bool,
    shape: FrameShape,
) -> bool {
    // Read as an open reads it (`read_sidecar_said`): the `.gcd`, a
    // picture's own default where there is none, and what an `.xmp`
    // beside the frame says that the sidecar has not taken yet. Read
    // without the `.xmp`, the write below would put the `.gcd`'s old
    // rating and turn over whatever another tool wrote there since.
    let (mut sidecar, trouble) = read_sidecar_fresh(source, Fresh::default(), shape);
    if let Some(Trouble::Unreadable(e)) = trouble {
        log::warn!("{}: sidecar: {e}; export not recorded", file_name(source));
        return false;
    }
    // A newer build's (§724): read, never written, nor its XMP.
    if let Err(e) = sidecar.writable() {
        log::warn!("{}: sidecar: {e}; export not recorded", file_name(source));
        return false;
    }
    // The edit was rendered from the sidecar as an open leaves it, so
    // an older build's is brought up to date the same way before the
    // two are compared; otherwise a version 3 Original crop is never
    // the state its own export was made from.
    migrate_from_file(&mut sidecar, source, shape);
    // The same seed for a frame the window has let go of, by the rule
    // a load decides it by.
    take_seed(&mut sidecar, source, version, edit);
    let Some(mut line) = sidecar.line_mut(version) else {
        no_version(source, version);
        return false;
    };
    if !line.record_export(edit, exported) {
        not_kept(source);
        return false;
    }
    if xmp {
        // As `write_sidecar` writes it: the camera's tag and the
        // frame's turns composed, or no orientation when the file
        // will not say its tag.
        let turn = greycard_core::decode::stance_path(source)
            .ok()
            .map(|s| xmp::Turn::new(s.orientation, sidecar.xmp_turn()));
        let taken = || camera_time_of(source);
        match xmp::save_dated(source, &sidecar.meta, turn, taken) {
            Ok(Some(mark)) => sidecar.xmp = Some(mark),
            Ok(None) => {}
            Err(e) => log::warn!("{}: xmp not written: {e}", file_name(source)),
        }
    }
    match sidecar.save_in(source, placement) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("{}: sidecar not saved: {e}", file_name(source));
            false
        }
    }
}

/// Give a frame still waiting on its ISO seed the learned blend its
/// export was rendered with (the worker seeds a set's frame as its
/// first open would), as its start before it holds any state (§724), as
/// the open's own seed is taken (`Outcome::Opened`), so the export's
/// record makes the seeded start its root; true when it took it. Nothing
/// when the two differ in anything else: then the export was not of
/// this state, and the seed waits for the open.
///
/// A raw's default is either of `Edit::raw_defaults`, so a frame read
/// at one and rendered at the other (no sidecar on disk, and lens
/// corrections off for new edits in the window) takes the rendered one
/// with its seed: both are the frame untouched.
pub fn take_seed(
    sidecar: &mut Sidecar,
    source: &Path,
    version: Option<&str>,
    rendered: &Edit,
) -> bool {
    if greycard_core::picture::is_picture_path(source) {
        return false;
    }
    let Some(mut line) = sidecar.line_mut(version) else {
        return false;
    };
    if !line_never_developed(&line) {
        return false;
    }
    let mut seeded = line.current().clone();
    seeded.noise.learned_strength = rendered.noise.learned_strength;
    if seeded != *rendered {
        let lens = seeded.lens.enabled;
        seeded.lens.enabled = rendered.lens.enabled;
        // Never developed, it was one of the two; it is the other now.
        if seeded != *rendered {
            return false;
        }
        log::info!(
            "{}: untouched, its lens section turned {} as the export was rendered",
            file_name(source),
            if lens { "off" } else { "on" }
        );
    }
    // No state to rewrite (§724): the start takes the seed, and the
    // export's record makes it the root.
    line.seed(|e| *e = seeded);
    true
}

/// The file at `path`'s capture time as its camera wrote it, read from
/// the file itself (the EXIF, no samples), in the index's form. For a
/// caller with no window state: an XMP written off the window's thread.
pub fn camera_time_of(path: &Path) -> Option<String> {
    let probe = if greycard_core::picture::is_picture_path(path) {
        greycard_core::picture::probe_path(path)
    } else {
        greycard_core::decode::probe_path(path)
    };
    crate::normalize_taken(probe.ok()?.taken.as_deref()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ISO's seed an export took for a raw's line never opened goes
    /// to that line alone: not to a version at the same default (which
    /// waits for its own open or export), not to one edited apart, and
    /// never to a picture file's.
    #[test]
    fn an_export_s_seed_goes_to_the_line_exported_alone() {
        let raw = Path::new("/shoot/IMG_0001.CR3");
        let mut sidecar = Sidecar::default();
        let copy = sidecar.new_version(None, Some("Copy")).unwrap();
        let apart = sidecar.new_version(None, Some("Apart")).unwrap();
        let mut moved = Edit::default();
        moved.light.exposure = 0.3;
        sidecar.line_mut(Some(&apart)).unwrap().record(moved);
        let mut rendered = Edit::default();
        rendered.noise.learned_strength = 0.42;
        assert!(take_seed(&mut sidecar, raw, None, &rendered));
        assert_eq!(sidecar.current().noise.learned_strength, 0.42);
        let strength =
            |s: &Sidecar, v: &str| s.line(Some(v)).unwrap().current().noise.learned_strength;
        assert_eq!(
            strength(&sidecar, &copy),
            Edit::default().noise.learned_strength,
            "the copy waits for its own"
        );
        assert_ne!(
            strength(&sidecar, &apart),
            0.42,
            "a line edited apart keeps its own"
        );
        // The copy's own export seeds its line, and the file's own stays.
        assert!(take_seed(&mut sidecar, raw, Some(&copy), &rendered));
        assert_eq!(strength(&sidecar, &copy), 0.42);
        // A line edited apart is never seeded, whatever was rendered.
        let mut seeded = sidecar.line(Some(&apart)).unwrap().current().clone();
        seeded.noise.learned_strength = 0.9;
        assert!(
            !take_seed(&mut sidecar, raw, Some(&apart), &seeded),
            "developed already"
        );
        assert_eq!(
            sidecar.current().noise.learned_strength,
            0.42,
            "the file's own as it was"
        );
        assert!(
            !take_seed(&mut sidecar, raw, Some("none"), &rendered),
            "no such version"
        );
        let mut picture = Sidecar::default();
        assert!(
            !take_seed(&mut picture, Path::new("/shoot/scan.tif"), None, &rendered),
            "a picture has no ISO to seed from"
        );
    }

    /// A raw read at the default and exported from the window at the
    /// default with the lens section off, which a new edit starts at
    /// with lens corrections off for new edits, is the frame untouched
    /// both ways: it takes the rendered state, and is still not edited.
    /// Anything else apart, it is not the state exported.
    #[test]
    fn an_export_at_a_new_edit_without_lens_is_recorded_on_the_default() {
        let raw = Path::new("/shoot/IMG_0001.CR3");
        let mut rendered = Edit::raw_without_lens();
        rendered.noise.learned_strength = 0.42;
        let mut sidecar = Sidecar::default();
        assert!(take_seed(&mut sidecar, raw, None, &rendered));
        assert_eq!(*sidecar.current(), rendered);
        assert!(crate::is_default_edit(sidecar.current(), raw, None));
        assert!(!crate::develop_marks(&sidecar, raw, None).0);
        let mut moved = rendered.clone();
        moved.light.exposure = 0.3;
        let mut sidecar = Sidecar::default();
        assert!(!take_seed(&mut sidecar, raw, None, &moved));
        assert_eq!(*sidecar.current(), Edit::default());
    }

    /// The export record of a frame the window let go of reads its
    /// sidecar with the reader's own shape (§724): the window's mapped
    /// An export of a frame a newer greycard made (§724) is not recorded:
    /// its sidecar is read-only, and neither it nor an XMP is written.
    #[test]
    fn an_export_of_a_newer_build_s_frame_is_not_recorded() {
        let dir =
            std::env::temp_dir().join(format!("greycard-record-newer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0000.CR3");
        std::fs::write(&raw, b"not a raw").unwrap();
        let path = Sidecar::path_for(&raw);
        let newer = r#"{"floors": {"read": 10, "write": 11}, "current": {"version": 11, "light": {"exposure": 0.5}}}"#;
        std::fs::write(&path, newer).unwrap();
        let shown = Sidecar::load_as_file(&raw)
            .unwrap()
            .unwrap()
            .current()
            .clone();
        let exported = Exported {
            file: "/out/IMG_0000.jpg".into(),
            preset: None,
            at: 1_700_000_000,
        };
        assert!(!record_export_on_disk(
            &raw,
            &shown,
            None,
            exported,
            Placement::Beside,
            true,
            |_, _| None,
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        assert!(greycard_edit_io::xmp::paths_of(&raw).is_empty(), "no XMP");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// header read on its thread, never the unmapped one, which takes a
    /// raw whole. A waiting state is measured by it, and the export is
    /// noted on the state as measured.
    #[test]
    fn an_export_record_measures_with_the_reader_s_shape() {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        static ASKED: AtomicUsize = AtomicUsize::new(0);
        fn counted(frame: &Path, turn: u8) -> Option<f32> {
            ASKED.fetch_add(1, Relaxed);
            let (w, h) = greycard_core::decode::stance_path(frame)
                .ok()?
                .shown_size(turn % 4)?;
            Some(w as f32 / h as f32)
        }
        let dir =
            std::env::temp_dir().join(format!("greycard-record-shape-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0000.tif");
        crate::fixture::write_frame_of(&raw, &crate::fixture::R5, 6, 4, &[0; 6 * 4 * 3]);
        let mut placed = Edit::default();
        placed
            .retouch
            .patches
            .push(greycard_edit::retouch::Patch::default());
        let mut s = Sidecar::default();
        s.record(placed);
        assert!(s.turn_by(1, Some(1.5)).is_some());
        let shown = s.current().clone();
        s.shape = None;
        s.save(&raw).unwrap();
        let exported = Exported {
            file: "/out/IMG_0000.jpg".into(),
            preset: None,
            at: 1_700_000_000,
        };
        assert!(record_export_on_disk(
            &raw,
            &shown,
            None,
            exported,
            Placement::Beside,
            false,
            counted,
        ));
        assert!(ASKED.load(Relaxed) > 0, "the reader's shape asked");
        let back = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert_eq!(back.current_exports.len(), 1, "noted on the state");
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
