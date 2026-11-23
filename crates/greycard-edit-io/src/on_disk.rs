//! A frame's sidecar as an open reads it from disk: the `.gcd`, or the
//! default when there is none or it will not read, what an `.xmp`
//! beside the frame says that the sidecar has not taken yet, a
//! Lightroom develop in it taken and written when the reader knows
//! where sidecars go and the frame has no edit of ours
//! ([`crate::lightroom::beside`]), and a sidecar from an older build
//! brought up to date from the frame's shape. Here, below the window,
//! since the reduced develop, the masks, and the headless export read
//! a sidecar as the window's open does.

use std::path::Path;

use greycard_edit::mask::Shape;
use greycard_edit::{Edit, Line, Placement, Sidecar};

use crate::xmp;

pub fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A turn in words, for the log: which way and how far.
fn turn_words(quarters: i32) -> &'static str {
    match quarters.rem_euclid(4) {
        1 => "a quarter turn to the right of its tag",
        2 => "a half turn from its tag",
        3 => "a quarter turn to the left of its tag",
        _ => "as its tag says",
    }
}

/// How a reader of a sidecar says its frame's shape when a state waits
/// on it (§724): the frame's path and the frame's turn in, the developed
/// picture's width over its height out, none when it cannot say now.
/// The edit crate reads no file; the reader holding the frame does,
/// with [`frame_shape`] off the window's thread, or the window's mapped
/// header read, or says nothing (`|_, _| None`).
pub type FrameShape = fn(&Path, u8) -> Option<f32>;

/// The shape of the frame at `frame` under the frame's turn `turn`, as
/// its file's header says it (no pixel decoded): width over height, the
/// camera's tag and the turn applied. None when the file will not say.
/// Never through a memory map: this is reached from a folder's open,
/// the library's pass, the headless export, and the command line, over
/// volumes that can go away under a mapping. A raw is read from its ends
/// where its format allows and whole otherwise, so never on the
/// window's thread.
pub fn frame_shape(frame: &Path, turn: u8) -> Option<f32> {
    #[cfg(test)]
    HEADERS_READ.with(|n| n.set(n.get() + 1));
    let (w, h) = greycard_core::decode::stance_path_unmapped(frame)
        .ok()?
        .shown_size(turn % 4)?;
    (w > 0 && h > 0).then(|| w as f32 / h as f32)
}

#[cfg(test)]
thread_local! {
    /// How many times this thread asked a frame's header for its shape.
    pub(crate) static HEADERS_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What went wrong reading a sidecar from disk, the default edit
/// standing in for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trouble {
    /// No `.gcd` beside the frame.
    Missing,
    /// There is one, and it could not be read: the error.
    Unreadable(String),
}

/// One file's sidecar, read from disk: what the library's `held::from_disk`
/// reads for a frame the index has no row for, and for a frame whose
/// row stood in until now. Beside it, what went wrong when the `.gcd`
/// was not there or could not be read and the default stands in for it,
/// so a frame whose row says it has a develop is not given the default
/// in its place. Whether a line of it still waits for its ISO's blend
/// is the line's to say (`rows::wants_seed`).
///
/// `sidecars` is where the editor's setting puts a sidecar, none with
/// sidecars off, when nothing is read and the default stands in. A
/// Lightroom develop beside a raw with no edit of ours is taken and the
/// sidecar written there ([`crate::lightroom::beside`]).
///
/// `fresh` is what a raw with no sidecar starts from, by the editor's
/// settings; `shape` says the frame's shape when a state waits on it.
pub fn load_sidecar_said(
    f: &Path,
    sidecars: Option<Placement>,
    fresh: Fresh,
    shape: FrameShape,
) -> (Sidecar, Option<Trouble>) {
    match sidecars {
        Some(placement) => load(f, Some(placement), fresh, true, shape),
        None => (fresh_sidecar(f, fresh), None),
    }
}

/// What the editor's settings say a raw's fresh edit starts with: the
/// part of a new sidecar that is a preference rather than the
/// schema's default. A picture that is not a raw starts from its own
/// default whatever this says, and a sidecar already written keeps
/// what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fresh {
    /// The lens section on: the profile's corrections and the rest of
    /// the section as the default has them. Off, the section's switch
    /// is off and nothing else of it changes.
    pub lens: bool,
}

impl Default for Fresh {
    fn default() -> Self {
        Fresh { lens: true }
    }
}

/// The sidecar of a frame nothing has been written for: a raw's
/// default, its lens section as `fresh` says, and for a picture that
/// is not a raw its own ([`Edit::for_picture`]).
pub fn fresh_sidecar(f: &Path, fresh: Fresh) -> Sidecar {
    // Known as the frame's start, so a save that finds a sidecar some
    // other writer made meanwhile takes it rather than joining a default
    // edit over it (`Sidecar::save_checked`). There are three starts,
    // each made and remembered once: a folder's open holds thousands,
    // and remembering one prints it.
    static STARTS: std::sync::OnceLock<[Sidecar; 3]> = std::sync::OnceLock::new();
    let starts = STARTS.get_or_init(|| {
        let of = |edit: Edit| Sidecar::starting_at(&edit);
        [
            of(Edit::for_picture()),
            Sidecar::default(),
            of(Edit::raw_without_lens()),
        ]
        .map(|mut s| {
            s.remember_fresh();
            s
        })
    });
    let start = if greycard_core::picture::is_picture_path(f) {
        0
    } else if fresh.lens {
        1
    } else {
        2
    };
    starts[start].clone()
}

/// [`load_sidecar_said`] for a reader that does not know where
/// sidecars go (the reduced develop, the masks): the sidecar as it is,
/// nothing taken and nothing written, a raw with none at the default.
/// Each of those readers gives up on a sidecar that is not there. Off
/// the window's thread: what waits on the frame's shape asks the
/// frame's header ([`frame_shape`]).
pub fn read_sidecar_said(f: &Path) -> (Sidecar, Option<Trouble>) {
    load(f, None, Fresh::default(), true, frame_shape)
}

/// [`read_sidecar_said`] with a raw with no sidecar starting as
/// `fresh` says, and the frame's shape said by `shape`: for a reader
/// that holds the editor's settings (a merge's frame the window has not
/// read).
pub fn read_sidecar_fresh(f: &Path, fresh: Fresh, shape: FrameShape) -> (Sidecar, Option<Trouble>) {
    load(f, None, fresh, true, shape)
}

/// [`read_sidecar_said`] for a reader that only looks ahead (the
/// prefetch of a neighbor), which says for itself what it does about a
/// sidecar that will not read, and says nothing else of it: nothing
/// starts from the default edit and nothing is turned or left out, so
/// the reader says none of what it says for an open. The window's own
/// read says it when the frame is opened.
pub fn peek_sidecar(f: &Path) -> (Sidecar, Option<Trouble>) {
    load(f, None, Fresh::default(), false, frame_shape)
}

/// The sidecar read, and with `takes` a Lightroom develop beside it
/// taken and written there. `say` is whether what the read found (a
/// sidecar that will not read, shapes this build does not know, a turn
/// taken from an XMP, an orientation that could not be read) is said in
/// the log ([`said`]). `shape` says the frame's shape where a state, or a
/// turn the XMP asks for, waits on it.
fn load(
    f: &Path,
    takes: Option<Placement>,
    fresh: Fresh,
    say: bool,
    shape: FrameShape,
) -> (Sidecar, Option<Trouble>) {
    let mut trouble = None;
    // A picture that is not a raw starts from its own default.
    let fresh = || fresh_sidecar(f, fresh);
    // A line nobody developed holds no state (§724): its start is the
    // frame's, by the settings.
    let mut sidecar = match Sidecar::load(f, fresh().line.current()) {
        Ok(Some(s)) => {
            // A shape from a later build: loaded as nothing, the
            // rest of the edit kept. One of a kind this build has
            // not heard of is gone on the next save; a known kind
            // with a route or a group it has not (a Part, a
            // Landscape) is written back as it came.
            let unknown: usize = s
                .current()
                .adjustments
                .iter()
                .map(|a| a.mask.unknown())
                .sum();
            let dropped: usize = s
                .current()
                .adjustments
                .iter()
                .flat_map(|a| &a.mask.components)
                .filter(|c| c.shape == Shape::Unknown)
                .count();
            if unknown > 0 {
                said(
                    say,
                    module_path!(),
                    log::Level::Warn,
                    format!(
                        "{}: sidecar: {unknown} mask shape(s) this build does not know, left \
                         out of their masks; {dropped} of a kind it has not heard of, dropped \
                         when the edit is saved, and {} kept as they came",
                        file_name(f),
                        unknown - dropped
                    ),
                );
            }
            s
        }
        Ok(None) => {
            trouble = Some(Trouble::Missing);
            fresh()
        }
        Err(e) => {
            said(
                say,
                module_path!(),
                log::Level::Warn,
                format!(
                    "{}: sidecar: {e}; starting from the default edit",
                    file_name(f)
                ),
            );
            trouble = Some(Trouble::Unreadable(format!("{e}")));
            fresh()
        }
    };
    // What waits on the frame's shape, shown by what the reader says
    // of it (§724), before the XMP's turn is made over it.
    sidecar.settle_shape(|turn| shape(f, turn));
    // What an XMP beside the frame has to say, when it has
    // changed since the sidecar last took its word. Reading
    // one is not behind the setting: writing an XMP is a
    // choice about somebody else's folder, believing one
    // that is already there is not. Nothing is written
    // here — the meta and the mark ride in memory until the
    // frame's next real save — so opening a folder of
    // somebody else's raws deposits nothing in it.
    // The camera's tag, for the one field of a packet that
    // is measured against it. The closure is called only for
    // a packet that carries `tiff:Orientation` at all, so a
    // folder whose XMPs are silent about it opens without
    // reading a byte of any raw.
    let took = xmp::adopt_said(
        f,
        &mut sidecar,
        || {
            greycard_core::decode::orientation_path(f)
                .inspect_err(|e| {
                    said(
                        say,
                        module_path!(),
                        log::Level::Warn,
                        format!("{}: orientation: {e}", file_name(f)),
                    )
                })
                .ok()
        },
        |turn| shape(f, turn),
        say,
    );
    // A turn taken from somebody else's file turns the
    // picture under the user without their having pressed
    // anything, and records no state to undo it with, so it
    // is said out loud and by name.
    if took.turned.is_some() {
        said(
            say,
            module_path!(),
            log::Level::Info,
            format!(
                "{}: the XMP beside it says the frame stands {}; turned to match",
                file_name(f),
                // Where it now stands, not how far it moved to
                // get there: a reader of the log wants the frame,
                // not the delta.
                turn_words(i32::from(sidecar.turn))
            ),
        );
    }
    // A state placed under another turn than the file's, with
    // something placed on the picture, is shown by the shape the file
    // writes beside its turn, or failing that by what `shape` says
    // (§724). One that still waits has a frame nothing here measured,
    // and is read-only until something does.
    if sidecar.current_waits() {
        said(
            say,
            module_path!(),
            log::Level::Warn,
            format!(
                "{}: nothing can measure the frame, so its masks and repair patches \
                 wait where they were placed, and no step is recorded on it",
                file_name(f)
            ),
        );
    }
    // A Lightroom develop in the XMP beside a raw with no edit of ours,
    // taken as a catalog's is and written, so every reader after this
    // one finds it. Never over a `.gcd` that will not read.
    if !matches!(trouble, Some(Trouble::Unreadable(_)))
        && let Some(placement) = takes
    {
        use crate::lightroom::beside;
        beside::take_and_save(f, &mut sidecar, placement);
        // Taken here, or by another reader while this one waited.
        if trouble == Some(Trouble::Missing)
            && !beside::no_edit_of_ours(&sidecar)
            && Sidecar::find(f).is_some()
        {
            trouble = None;
        }
    }
    (sidecar, trouble)
}

/// `line` in the log at `level` under `target` (the caller's module, as
/// its own `log::warn!` would have it) when `say`; in the tests, also
/// kept for them to read ([`SAID`]).
pub(crate) fn said(say: bool, target: &str, level: log::Level, line: String) {
    if !say {
        return;
    }
    log::log!(target: target, level, "{line}");
    #[cfg(test)]
    SAID.with(|s| s.borrow_mut().push(format!("{target} {line}")));
}

#[cfg(test)]
thread_local! {
    /// What [`said`] has logged on this thread.
    static SAID: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A raw's starting edit, its learned blend at its ISO, and the body
/// that shot it as the library names one, from its probe (none for a
/// file that would not probe): what an import lays a develop over.
pub fn starting_of(probe: Option<&greycard_core::decode::Probe>) -> (Edit, Option<String>) {
    let mut edit = Edit::default();
    edit.noise.learned_strength = greycard_edit::Noise::blend_for_iso(probe.and_then(|p| p.iso));
    let camera = probe.map(|p| greycard_core::raw::camera_name(&p.make, &p.model));
    (edit, camera)
}

/// Whether a raw's sidecar has nothing in it that a develop put
/// there: the edit is the default, and there is no history and no
/// snapshot behind it. A file rated but never developed has a
/// sidecar, and that is not the same as having been developed —
/// its learned blend is still waiting for its ISO.
pub fn never_developed(sidecar: &Sidecar) -> bool {
    line_never_developed(&sidecar.line)
}

/// [`never_developed`] of one line of a sidecar: a version made from the
/// original before the raw was opened is at the default too, and waits
/// for its ISO on its own; one made from an edited version, or edited
/// since, is not.
///
/// A raw's default is either of [`Edit::raw_defaults`]: one with the
/// lens section off is how a fresh edit starts with lens corrections
/// off for new edits.
pub fn line_never_developed(line: &Line) -> bool {
    // No state recorded (§724), and its start not seeded yet.
    !line.recorded()
        && Edit::raw_defaults()
            .iter()
            .any(|d| line.current().develops_as(d))
}

/// A sidecar read with no window holding the frame (`Sidecar::from_json`,
/// a copy), told it is `frame`'s: what still waits on the frame's shape
/// (a state placed under another turn the file has no shape for, or one
/// from before version 4) shown by the shape `frame`'s header says
/// ([`Sidecar::settle_shape`]), as the window does with the frame it
/// opens (`panel::browser::migrate_frame`). Left as it is when the file
/// will not say.
pub fn migrate_from_file(sidecar: &mut Sidecar, frame: &Path, shape: FrameShape) {
    sidecar.settle_shape(|turn| shape(frame, turn));
    if sidecar.needs_frame() {
        log::warn!(
            "{}: nothing can say which way up it is, so what waits on its \
             shape waits on",
            frame.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With sidecars off nothing is read, and a picture still starts
    /// from its own default, lens corrections and capture sharpen off;
    /// a raw from the raw's.
    #[test]
    fn with_sidecars_off_a_picture_starts_from_its_own_default() {
        let (picture, _) = load_sidecar_said(
            Path::new("/nowhere/IMG_0001.JPG"),
            None,
            Fresh::default(),
            frame_shape,
        );
        assert_eq!(*picture.line.current(), Edit::for_picture());
        assert!(!picture.line.current().lens.wants_profile());
        let (raw, _) = load_sidecar_said(
            Path::new("/nowhere/IMG_0001.CR3"),
            None,
            Fresh::default(),
            frame_shape,
        );
        assert_eq!(raw, Sidecar::default());
    }

    /// The setting decides a fresh raw edit's lens section and nothing
    /// else: on, the default; off, the default with the section's
    /// switch off, still never developed. A picture starts from its
    /// own default either way.
    #[test]
    fn a_fresh_raw_edit_takes_its_lens_section_from_the_setting() {
        let raw = Path::new("/nowhere/IMG_0001.CR3");
        let off = Fresh { lens: false };
        assert_eq!(fresh_sidecar(raw, Fresh::default()), Sidecar::default());
        let without = fresh_sidecar(raw, off);
        assert!(!without.line.current().lens.enabled);
        let mut back = without.line.current().clone();
        back.lens.enabled = true;
        assert_eq!(back, Edit::default(), "only the switch moves");
        assert!(never_developed(&without));
        let (read, _) = load_sidecar_said(raw, None, off, frame_shape);
        assert_eq!(read, without);
        let picture = Path::new("/nowhere/IMG_0001.JPG");
        assert_eq!(
            fresh_sidecar(picture, off),
            fresh_sidecar(picture, Fresh::default())
        );
    }

    /// A sidecar with an edit recorded keeps its lens section whatever
    /// the setting says; one that is not there, or that holds a rating
    /// and no state (§724), starts as it says.
    #[test]
    fn an_existing_sidecar_keeps_its_lens_section() {
        let dir = std::env::temp_dir().join(format!("greycard-fresh-lens-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0002.CR3");
        let off = Fresh { lens: false };
        let (missing, trouble) = load_sidecar_said(&raw, Some(Placement::Beside), off, frame_shape);
        assert_eq!(trouble, Some(Trouble::Missing));
        assert!(!missing.line.current().lens.enabled);
        let mut written = Sidecar::default();
        written.meta.rating = 3;
        written.save(&raw).unwrap();
        let (read, trouble) = load_sidecar_said(&raw, Some(Placement::Beside), off, frame_shape);
        assert_eq!(trouble, None);
        assert!(!read.line.recorded());
        assert!(
            !read.line.current().lens.enabled,
            "no state: the setting's start"
        );
        assert_eq!(read.meta.rating, 3);
        let mut brighter = Edit::default();
        brighter.light.exposure = 0.5;
        written.record(brighter);
        written.save(&raw).unwrap();
        let (read, _) = load_sidecar_said(&raw, Some(Placement::Beside), off, frame_shape);
        assert!(read.line.current().lens.enabled, "the file's own");
        let (read, _) = read_sidecar_fresh(&raw, off, frame_shape);
        assert!(read.line.current().lens.enabled);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A star is not a develop: the sidecar a rating makes still has
    /// nothing of a develop in it, so the frame's learned blend is
    /// still waiting for its ISO.
    #[test]
    fn a_rated_sidecar_is_still_undeveloped() {
        let mut sidecar = Sidecar::default();
        assert!(never_developed(&sidecar));
        sidecar.meta.rating = 3;
        assert!(never_developed(&sidecar));
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        assert!(sidecar.record(edit));
        assert!(!never_developed(&sidecar));
    }

    /// A peek says nothing of what it found, where the window's read of
    /// the same files says each thing: a sidecar that will not read and
    /// an orientation an XMP asks to be measured against a raw that will
    /// not say it.
    #[test]
    fn a_peek_is_quiet_where_a_read_says_what_it_found() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-peek-quiet-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"not a raw").unwrap();
        std::fs::write(Sidecar::path_in(&raw, Placement::Beside), b"{ not json").unwrap();
        std::fs::write(
            xmp::short_path(&raw),
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/" tiff:Orientation="6"/>
 </rdf:RDF>
</x:xmpmeta>"#,
        )
        .unwrap();
        let said_now = || SAID.with(|s| std::mem::take(&mut *s.borrow_mut()));
        said_now();
        let (_, trouble) = peek_sidecar(&raw);
        assert!(matches!(trouble, Some(Trouble::Unreadable(_))));
        assert_eq!(said_now(), Vec::<String>::new(), "the peek says nothing");
        let (_, trouble) = read_sidecar_said(&raw);
        assert!(matches!(trouble, Some(Trouble::Unreadable(_))));
        let lines = said_now();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("greycard_edit_io::on_disk ")
                    && l.ends_with("; starting from the default edit")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains(": orientation: ")),
            "{lines:?}"
        );
        // An XMP beside it that will not parse.
        let broken = dir.join("IMG_0002.CR3");
        std::fs::write(&broken, b"not a raw").unwrap();
        std::fs::write(xmp::short_path(&broken), b"<x:xmpmeta <<< not xml").unwrap();
        peek_sidecar(&broken);
        assert_eq!(said_now(), Vec::<String>::new(), "the peek says nothing");
        read_sidecar_said(&broken);
        let lines = said_now();
        assert!(
            lines.iter().any(|l| l.starts_with("greycard_edit_io::xmp ")
                && l.ends_with("; the sidecar's meta stands")),
            "{lines:?}"
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An uncompressed grey TIFF `w` by `h`, its pixels black: a frame
    /// whose header says its shape, made here rather than kept as a file.
    fn tiny_tiff(w: u16, h: u16) -> Vec<u8> {
        let entries: [(u16, u16, u32); 9] = [
            (256, 3, u32::from(w)),
            (257, 3, u32::from(h)),
            (258, 3, 8),
            (259, 3, 1),
            (262, 3, 1),
            (273, 4, 8 + 2 + 9 * 12 + 4),
            (277, 3, 1),
            (278, 3, u32::from(h)),
            (279, 4, u32::from(w) * u32::from(h)),
        ];
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, kind, value) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(1u32.to_le_bytes());
            if kind == 3 {
                out.extend((value as u16).to_le_bytes());
                out.extend([0, 0]);
            } else {
                out.extend(value.to_le_bytes());
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, usize::from(w) * usize::from(h)));
        out
    }

    /// A file whose turned states it has no shape for (a copy turned by
    /// a build before version 10 joined with one placed under the old
    /// turn here, or a file cut by hand): read by a reader off the
    /// window's thread, the frame's own header says the shape, and every
    /// state shows right. Read by one that measures later (the window),
    /// nothing of the frame is read and the states wait. A turned file
    /// with nothing waiting reads no header at all.
    #[test]
    fn a_state_waiting_on_the_shape_is_shown_by_its_frame_s_header() {
        let dir = std::env::temp_dir().join(format!("greycard-header-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("IMG_0001.tif");
        std::fs::write(&raw, tiny_tiff(6, 4)).unwrap();
        let mut s = Sidecar::default();
        let mut placed = Edit::default();
        placed
            .retouch
            .patches
            .push(greycard_edit::retouch::Patch::default());
        s.record(placed);
        assert!(s.turn_by(1, Some(1.5)).is_some());
        let mut cut = s.clone();
        cut.shape = None;
        std::fs::write(Sidecar::path_for(&raw), cut.to_json()).unwrap();
        let headers = || HEADERS_READ.with(std::cell::Cell::get);
        let was = headers();
        let (read, _) = read_sidecar_said(&raw);
        assert!(!read.needs_frame());
        assert_eq!(read.line.current(), s.line.current());
        assert_eq!(headers(), was + 1);
        let (later, _) =
            load_sidecar_said(&raw, Some(Placement::Beside), Fresh::default(), |_, _| None);
        assert!(later.current_waits(), "measured later, by its reader");
        assert_eq!(headers(), was + 1, "no header read");
        // Nothing waiting: nothing read.
        let mut cropped = Sidecar::default();
        let mut crop = Edit::default();
        crop.geometry.crop = Some(greycard_edit::geometry::Crop::of([0.1, 0.2, 0.5, 0.6]));
        cropped.record(crop);
        assert!(cropped.turn_by(1, None).is_some());
        cropped.save(&raw).unwrap();
        let (read, _) = read_sidecar_said(&raw);
        assert_eq!(read.turn, 1);
        assert_eq!(headers(), was + 1, "no header read");
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
