//! Writing a frame's sidecar, and its meta to an XMP beside it, from
//! the window's thread: one file at a time, or handed to the batch
//! when there are two or more, which is `panel::saves`'s.

use greycard_edit_io::xmp;

use crate::State;
use crate::panel::browser::{camera_tag, file_name};

/// Write cell `c`'s file's sidecar, if sidecars are written, and its
/// meta to an XMP beside it when that is asked for: [`write_file`] of
/// the cell's file. The sidecar is the file's whole, every line of it,
/// written once whichever of its cells moved.
pub(crate) fn write_sidecar(st: &mut State, c: usize) {
    if let Some(file) = st.file_at(c) {
        write_file(st, file);
    }
}

/// Write file `file`'s sidecar, if sidecars are written, and its meta
/// to an XMP beside it when that is asked for.
///
/// The XMP goes first so that the mark for what it now holds is in
/// the sidecar before the sidecar is written; otherwise the next
/// load would read back what this build just wrote. An XMP that
/// will not parse costs the interop and nothing else: the `.gcd` is
/// written either way.
pub(crate) fn write_file(st: &mut State, file: usize) {
    if !file_writable(st, file) {
        return;
    }
    // Saved here, it is no longer a batch's to save.
    crate::panel::saves::take_back(st, file);
    // A frame followed to an archive's copy has its XMP written there
    // by the archive write, from the sidecar, off the window's thread.
    if st.window.xmp_sidecars && !crate::sync::followed(st, file) {
        // `tiff:Orientation` is the camera's tag and the frame's
        // quarter turns composed; with no tag to compose onto,
        // whatever the file says about the orientation is left
        // alone and the rest of the meta still goes.
        let turn = st
            .file_cell(file)
            .and_then(|c| camera_tag(st, c))
            .map(|o| xmp::Turn::new(o, st.sidecars[file].turn));
        // The capture time's dates, from the camera's own time, asked
        // only when a shift moves them.
        let (path, meta) = (st.files[file].clone(), st.sidecars[file].meta.clone());
        let taken = || crate::panel::time_shift::camera_time(st, file);
        match xmp::save_dated(&path, &meta, turn, taken) {
            Ok(Some(mark)) => st.sidecars[file].xmp = Some(mark),
            Ok(None) => {}
            Err(e) => tracing::warn!("{}: xmp not written: {e}", file_name(&st.files[file])),
        }
    }
    save_file(st, file);
}

/// [`write_file`] of each of `files`: one alone on the window's
/// thread, two or more as a batch off it ([`crate::panel::saves`]),
/// marked saved when the batch lands.
pub(crate) fn write_files(st: &mut State, files: &[usize]) {
    match files {
        [] => {}
        [file] => write_file(st, *file),
        _ => crate::panel::saves::save(st, files),
    }
}

/// [`save_file`] of cell `c`'s file.
pub(crate) fn save_sidecar(st: &mut State, c: usize) -> bool {
    st.file_at(c).is_some_and(|file| save_file(st, file))
}

/// Write file `file`'s sidecar where it lives: for a frame the window
/// follows on an archive's copy (§233), the save is made in memory and
/// the file written off the window's thread by the archive write, with
/// its check against the copy; for every other frame, written here
/// beside it, the index told and the archive's copy queued. One save,
/// one revision, however many of its lines moved. True when the save
/// was made.
pub(crate) fn save_file(st: &mut State, file: usize) -> bool {
    crate::panel::saves::take_back(st, file);
    if crate::sync::followed(st, file) {
        st.sidecars[file].advance(&greycard_edit::sync::Stamp::now());
        crate::sync::after_save(st, file);
        for c in st.cells_of_file(file) {
            crate::edited::saved(st, c);
        }
        return true;
    }
    let (path, placement) = (st.files[file].clone(), st.window.placement);
    match st.sidecars[file].save_in(&path, placement) {
        Ok(_) => {
            crate::library::sidecar_written(st, file);
            true
        }
        Err(e) => {
            tracing::warn!("{}: sidecar not saved: {e}", file_name(&path));
            false
        }
    }
}

/// Whether cell `c`'s sidecar may be written now: sidecars are on,
/// the frame is not being deleted, and the sidecar in memory is the
/// frame's own and not its row standing in (`rows`), which would put
/// the default edit over whatever the file holds.
pub(crate) fn writable(st: &State, c: usize) -> bool {
    st.file_at(c).is_some_and(|file| file_writable(st, file))
}

/// [`writable`] by the file.
pub(crate) fn file_writable(st: &State, file: usize) -> bool {
    st.window.write_sidecars
        && !crate::panel::delete::held_file(st, file)
        && st.from_row.get(file).is_none_or(|r| r.read)
}
