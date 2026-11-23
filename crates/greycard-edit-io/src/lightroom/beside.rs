//! A Lightroom XMP beside a raw, its develop taken as a catalog's is:
//! the `crs:` settings mapped for the frame's body ([`super::read_for`]'s
//! mapping, scales and offset) over its starting edit, its crop and its
//! turn in Lightroom with them, recorded as one state, "Imported from
//! Lightroom"; and the packet's rating, label, keywords (with their
//! paths, when Lightroom keeps them), title and caption on each of
//! them the frame's `.gcd` leaves empty, recorded as Lightroom's for an
//! import to replace. Lightroom Classic writes such a file with
//! its metadata saved to the files; Lightroom's export of the originals
//! with their settings writes one beside every raw.
//!
//! **Which file.** `IMG.CR3.xmp`, and `IMG.xmp`, the name Lightroom
//! gives it, when it is the raw's: no other raw in the folder has the
//! stem, or the raw is an `.ORF` and the other its `.ORI`, and never
//! for an `.ORI` ([`short_is_its`]). Of those,
//! the one that holds `crs:` settings, the newer if both do. A JPEG of
//! the same stem is the raw's companion and claims none: Lightroom
//! keeps a JPEG's settings inside it.
//!
//! **When.** Only for a raw with no edit of ours: no `.gcd`, or one
//! holding no develop, history, snapshot, or version (a frame rated
//! here still takes the develop, and keeps its own meta, as an open's
//! adoption leaves it). Once taken,
//! the history holds the state, so nothing takes it twice, and a `.gcd`
//! that will not read is never written over. Settings that sit where
//! Lightroom starts a frame ([`untouched`]) are not taken, as a
//! catalog's unedited image is not, whatever `HasSettings` says.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use greycard_edit::curve::Point;
use greycard_edit::meta::Meta;
use greycard_edit::xmp::Adopted;
use greycard_edit::{Edit, Placement, Sidecar};

use super::crop::{self, Cropped, Turned};
use super::{Scales, Settings};
use crate::{on_disk, xmp};

/// What a take did.
#[derive(Debug, Clone, PartialEq)]
pub struct Taken {
    /// The XMP the settings came from.
    pub xmp: PathBuf,
    /// What the mapper had no place for, by Lightroom's names; the crop
    /// among them when it was not carried.
    pub unmapped: Vec<String>,
    /// The body as the library names it, none when the raw would not
    /// probe; and the stops its offset added to the Exposure slider.
    pub camera: Option<String>,
    pub offset: f32,
    pub cropped: Cropped,
    pub turned: Turned,
    /// The meta moved: the packet's laid where the frame had none.
    pub meta: bool,
    /// The sidecar was written ([`take_and_save`]).
    pub saved: bool,
}

/// The XMPs beside `raw` that could be Lightroom's for it and are
/// there: its long name, and its stem's when that is the raw's
/// ([`short_is_its`]).
fn candidates(raw: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let long = xmp::long_path(raw);
    if long.is_file() {
        out.push(long);
    }
    let short = xmp::short_path(raw);
    if short.is_file() && short_is_its(raw) {
        out.push(short);
    }
    out
}

/// Whether the stem's `IMG.xmp` is `raw`'s: when the raw owns its
/// stem's short names among what is in its folder
/// ([`greycard_core::decode::short_names`]: alone, or an `.ORF` beside
/// its `.ORI`); never an `.ORI`'s, and never when another raw of the
/// stem has them or may.
fn short_is_its(raw: &Path) -> bool {
    use greycard_core::decode::ShortNames;
    !greycard_core::decode::always_yields(raw)
        && matches!(
            greycard_core::decode::short_names_on_disk(raw),
            ShortNames::Alone | ShortNames::Owns
        )
}

/// An XMP beside a raw that holds `crs:` settings, read.
struct Beside {
    path: PathBuf,
    bytes: Vec<u8>,
    settings: Settings,
}

/// The XMP beside `raw` that holds Lightroom's settings for it, the
/// newer of two that both do; none when none does.
fn read_beside(raw: &Path) -> Option<Beside> {
    let mut found: Vec<(Option<std::time::SystemTime>, Beside)> = Vec::new();
    for path in candidates(raw) {
        let bytes = match crate::xmp::bytes_of(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::FileTooLarge => {
                log::warn!("{}: {e}", path.display());
                continue;
            }
            Err(e) => {
                log::debug!("{}: {e}", path.display());
                continue;
            }
        };
        let Ok(settings) = super::settings_in(&String::from_utf8_lossy(&bytes)) else {
            continue;
        };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        found.push((
            modified,
            Beside {
                path,
                bytes,
                settings,
            },
        ));
    }
    found.sort_by_key(|(t, _)| *t);
    found.pop().map(|(_, b)| b)
}

/// The XMP beside `raw` a take would read: the one with Lightroom's
/// settings for it, none when there is none.
pub fn path_for(raw: &Path) -> Option<PathBuf> {
    read_beside(raw).map(|b| b.path)
}

/// Whether `sidecar` holds no edit of ours: no history, snapshot, or
/// version, and the edit a raw's default (either of
/// `Edit::raw_defaults`) but for the learned blend an open seeds from
/// the ISO. Its meta is not an edit.
pub fn no_edit_of_ours(sidecar: &Sidecar) -> bool {
    let mut current = sidecar.line.current().clone();
    current.noise.learned_strength = Edit::default().noise.learned_strength;
    sidecar.versions.is_empty()
        && sidecar.line.history.is_empty()
        && sidecar.line.snapshots.is_empty()
        && Edit::raw_defaults().iter().any(|d| current.develops_as(d))
}

/// The sliders the mapper reads at the values Lightroom starts a frame
/// at: the 2012 process's, and the older processes' for a file from
/// before it. The lens profile's switch is left out: it is a default
/// Lightroom lets a user change, and either way is a frame untouched.
const AS_IT_CAME: &[(&str, f32)] = &[
    ("Exposure2012", 0.0),
    ("Contrast2012", 0.0),
    ("Highlights2012", 0.0),
    ("Shadows2012", 0.0),
    ("Whites2012", 0.0),
    ("Blacks2012", 0.0),
    ("Texture", 0.0),
    ("Clarity2012", 0.0),
    ("Dehaze", 0.0),
    ("Vibrance", 0.0),
    ("Saturation", 0.0),
    ("Sharpness", 40.0),
    ("LuminanceSmoothing", 0.0),
    ("ColorNoiseReduction", 25.0),
    ("ParametricShadows", 0.0),
    ("ParametricDarks", 0.0),
    ("ParametricLights", 0.0),
    ("ParametricHighlights", 0.0),
    ("SplitToningShadowSaturation", 0.0),
    ("SplitToningHighlightSaturation", 0.0),
    ("SplitToningBalance", 0.0),
    ("ColorGradeShadowSat", 0.0),
    ("ColorGradeMidtoneSat", 0.0),
    ("ColorGradeHighlightSat", 0.0),
    ("ColorGradeGlobalSat", 0.0),
    ("PostCropVignetteAmount", 0.0),
    ("GrainAmount", 0.0),
    ("LensManualDistortionAmount", 0.0),
    ("DefringePurpleAmount", 0.0),
    ("DefringeGreenAmount", 0.0),
    ("PerspectiveUpright", 0.0),
    ("PerspectiveVertical", 0.0),
    ("PerspectiveHorizontal", 0.0),
    ("PerspectiveRotate", 0.0),
    ("RedHue", 0.0),
    ("RedSaturation", 0.0),
    ("GreenHue", 0.0),
    ("GreenSaturation", 0.0),
    ("BlueHue", 0.0),
    ("BlueSaturation", 0.0),
    ("ShadowTint", 0.0),
    ("Exposure", 0.0),
    ("Contrast", 25.0),
    ("Brightness", 50.0),
    ("Shadows", 5.0),
    ("FillLight", 0.0),
    ("HighlightRecovery", 0.0),
    ("Clarity", 0.0),
];

/// Whether the settings sit where Lightroom starts a frame: every
/// slider at its start ([`AS_IT_CAME`], the mixer's bands at zero), the
/// white balance as shot, the 2012 curves straight, no black and white,
/// no crop, and no masks, spots, or filters.
fn untouched(keys: &BTreeMap<String, String>, curves: &BTreeMap<String, Vec<Point>>) -> bool {
    let num = |v: &str| v.trim().parse::<f32>().ok().filter(|v| v.is_finite());
    let at = |k: &str, start: f32| {
        keys.get(k)
            .and_then(|v| num(v))
            .is_none_or(|v| (v - start).abs() < 1e-4)
    };
    let word = |k: &str| keys.get(k).map(|v| v.trim());
    let mixer_still = keys.iter().all(|(k, v)| {
        let band = [
            "HueAdjustment",
            "SaturationAdjustment",
            "LuminanceAdjustment",
        ]
        .iter()
        .any(|p| k.starts_with(p));
        !band || num(v).is_none_or(|v| v.abs() < 1e-4)
    });
    let straight = curves
        .iter()
        .filter(|(k, _)| k.starts_with("ToneCurvePV2012"))
        .all(|(_, points)| points.iter().all(|[x, y]| (x - y).abs() < 1e-3));
    let structures = [
        "MaskGroupBasedCorrections",
        "CircularGradientBasedCorrections",
        "GradientBasedCorrections",
        "PaintBasedCorrections",
        "RetouchAreas",
        "FilterList",
    ];
    AS_IT_CAME.iter().all(|(k, start)| at(k, *start))
        && mixer_still
        && straight
        && word("WhiteBalance").is_none_or(|w| w.eq_ignore_ascii_case("As Shot"))
        && word("ToneCurveName2012").is_none_or(|w| w.eq_ignore_ascii_case("Linear"))
        && !word("ConvertToGrayscale").is_some_and(|w| w.eq_ignore_ascii_case("true"))
        && !word("Treatment").is_some_and(|w| w.eq_ignore_ascii_case("Black & White"))
        && crop::Crop::of(keys).is_none()
        && !crop::Crop::unreadable(keys)
        && !keys
            .keys()
            .any(|k| structures.iter().any(|s| k.starts_with(s)))
}

/// Whether a Lightroom develop waits beside `raw` for a take: a raw
/// with an XMP of its own whose settings Lightroom moved from where it
/// starts a frame. Says nothing of the sidecar.
pub fn waits(raw: &Path) -> bool {
    greycard_core::decode::is_raw_path(raw)
        && read_beside(raw).is_some_and(|b| !untouched(&b.settings.keys, &b.settings.curves))
}

/// The develop in the XMP beside `raw` taken into `sidecar` when it
/// holds no edit of ours: recorded on its own line as one state over
/// the frame's starting edit, and the packet's meta laid on each field
/// the sidecar leaves empty, what it laid recorded
/// ([`greycard_edit::meta::Laid`]). Nothing written. None when nothing
/// was taken.
pub fn take(raw: &Path, sidecar: &mut Sidecar) -> Option<Taken> {
    let disk = sidecar.meta.clone();
    take_over(raw, sidecar, &disk)
}

/// [`take`], the packet's meta laid on each field `disk` (the meta the
/// `.gcd` holds) leaves empty; `sidecar`'s own kept on the rest, as an
/// open's adoption left it.
fn take_over(raw: &Path, sidecar: &mut Sidecar, disk: &Meta) -> Option<Taken> {
    if !greycard_core::decode::is_raw_path(raw) || !no_edit_of_ours(sidecar) {
        return None;
    }
    let Beside {
        path,
        bytes,
        settings,
    } = read_beside(raw)?;
    if untouched(&settings.keys, &settings.curves) {
        return None;
    }
    let (probe, stance) = match greycard_core::decode::probe_and_stance_counted(raw) {
        Ok((probe, stance, _)) => (Some(probe), Some(stance)),
        Err(e) => {
            log::debug!("{}: {e}", raw.display());
            (None, None)
        }
    };
    let (starting, camera) = on_disk::starting_of(probe.as_ref());
    let offset = super::body_offset(camera.as_deref());
    let imported = super::map(
        String::new(),
        &settings.keys,
        &settings.curves,
        &Scales::DEFAULT,
        offset,
    );
    let packet = xmp::read(&String::from_utf8_lossy(&bytes)).ok();
    let lightroom = packet.as_ref().and_then(xmp::Packet::orientation);
    let mut edit = imported.preset.applied(&starting);
    let stance = stance.as_ref();
    let turned = match lightroom {
        Some(o) => crop::turn_onto(&mut edit, stance, o, sidecar.turn),
        None => Turned::Same,
    };
    let cropped = crop::crop_onto(
        &mut edit,
        stance,
        &settings.keys,
        None,
        lightroom,
        sidecar.turn,
    );
    // Settings moved from Lightroom's start that carry nothing here (an
    // old process's sliders, a stray old key in a later file): nothing
    // taken, so nothing written, and the next open finds it the same.
    // The packet's meta is an open's adoption's, as for any XMP.
    if edit == starting {
        log::debug!(
            "{}: the Lightroom develop in {} carries nothing here{}",
            on_disk::file_name(raw),
            on_disk::file_name(&path),
            if imported.unmapped.is_empty() {
                String::new()
            } else {
                format!(" ({})", imported.unmapped.join(", "))
            }
        );
        return None;
    }
    let mut unmapped = imported.unmapped;
    if let Cropped::Carried { .. } = cropped {
        unmapped.retain(|n| n != "crop");
    }
    // A line nobody developed (`no_edit_of_ours`) holds no state
    // (§724): the import's start is its start, recorded as the root
    // under the import's step. One with its untouched start recorded
    // keeps it as the root, never changed under its id, and the import
    // goes on top.
    sidecar.line.seed(|e| *e = starting);
    sidecar.record_as(
        edit,
        Some(greycard_edit::history::import_label("Lightroom")),
    );
    let mut meta = false;
    if let Some(packet) = &packet {
        let mut theirs = packet.over(&Meta::default());
        if let Some(paths) = packet.keyword_paths().filter(|p| !p.is_empty()) {
            theirs.set_keywords(paths.to_vec());
        }
        let mut out = sidecar.meta.clone();
        let empty = Meta::default();
        if disk.rating == empty.rating && disk.flag == empty.flag {
            // One field in an XMP: a -1 rating is a reject.
            out.rating = theirs.rating;
            out.flag = theirs.flag;
        }
        if disk.label == empty.label {
            out.label = theirs.label;
        }
        if disk.keywords.is_empty() {
            out.keywords = theirs.keywords;
        }
        if disk.title.is_empty() {
            out.title = theirs.title;
        }
        if disk.caption.is_empty() {
            out.caption = theirs.caption;
        }
        // What came from the XMP on this read, the adoption's and the
        // take's, is Lightroom's for an import to replace.
        sidecar.lightroom = greycard_edit::meta::Laid::between(disk, &out).then(&sidecar.lightroom);
        meta = out != sidecar.meta;
        sidecar.meta = out;
    }
    // Its word taken, as an open's adoption marks it, when it is the
    // file an open adopts from: the next open lays nothing of it over
    // what the frame holds now.
    if xmp::path_of(raw).as_deref() == Some(path.as_path()) {
        sidecar.xmp = Some(Adopted::of(&path, &bytes));
    }
    Some(Taken {
        xmp: path,
        unmapped,
        camera,
        offset: imported.offset,
        cropped,
        turned,
        meta,
        saved: false,
    })
}

/// The frames being taken in the process, so two readers of one frame
/// (the window and the index) do not both take it, while readers of
/// others go on.
static TAKING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static TAKEN: Condvar = Condvar::new();

/// One frame held for a take, let go when dropped.
struct Holding(PathBuf);

impl Holding {
    fn of(raw: &Path) -> Holding {
        let mut held = TAKING.lock().unwrap_or_else(|e| e.into_inner());
        while held.iter().any(|p| p == raw) {
            held = TAKEN.wait(held).unwrap_or_else(|e| e.into_inner());
        }
        held.push(raw.to_path_buf());
        Holding(raw.to_path_buf())
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        let mut held = TAKING.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|p| *p != self.0);
        TAKEN.notify_all();
    }
}

/// Where a take writes `raw`'s sidecar: where it is, when it has one;
/// else under the folder's `.greycard` when `setting` says so or the
/// folder has one; else beside the frame.
fn placement_for(raw: &Path, setting: Placement) -> Placement {
    match Sidecar::find(raw) {
        Some(there) => Placement::at(&there),
        None if setting == Placement::Folder
            || raw.with_file_name(greycard_edit::SIDECAR_FOLDER).is_dir() =>
        {
            Placement::Folder
        }
        None => Placement::Beside,
    }
}

/// [`take`], the packet's meta laid where the `.gcd` on disk has none
/// and `sidecar`'s (an open's adoption over the file's, the XMP's word winning
/// when it changed since the file took it)
/// kept elsewhere, and the sidecar written where `setting` (the
/// editor's) puts it ([`placement_for`]). A reader that finds the frame
/// taken while it waited gets the sidecar the other wrote, and None.
pub fn take_and_save(raw: &Path, sidecar: &mut Sidecar, setting: Placement) -> Option<Taken> {
    if !greycard_core::decode::is_raw_path(raw)
        || !no_edit_of_ours(sidecar)
        || candidates(raw).is_empty()
    {
        return None;
    }
    let _held = Holding::of(raw);
    // Started at the reader's start, as the copy in hand was (§724).
    let ours = match Sidecar::load(raw, &sidecar.start) {
        // A newer build's is read and never written over (§724).
        Ok(Some(there)) if !no_edit_of_ours(&there) || there.read_only() => {
            // The frame's shape is the frame's, whichever copy said it.
            let shape = sidecar.shape;
            *sidecar = there;
            sidecar.take_shape(shape);
            return None;
        }
        Ok(Some(there)) => there.meta,
        Ok(None) => Meta::default(),
        // A `.gcd` that will not read is never written over.
        Err(_) => return None,
    };
    let mut taking = sidecar.clone();
    let mut taken = take_over(raw, &mut taking, &ours)?;
    *sidecar = taking;
    let name = on_disk::file_name(raw);
    match sidecar.save_in(raw, placement_for(raw, setting)) {
        Ok(()) => {
            taken.saved = true;
            log::info!(
                "{name}: the Lightroom develop in {} taken{}{}",
                on_disk::file_name(&taken.xmp),
                taken
                    .camera
                    .as_deref()
                    .filter(|_| taken.offset != 0.0)
                    .map(|c| format!(", {:+.2} stops for the {c}", taken.offset))
                    .unwrap_or_default(),
                if taken.unmapped.is_empty() {
                    String::new()
                } else {
                    format!("; no place here for {}", taken.unmapped.join(", "))
                }
            );
        }
        Err(e) => log::warn!("{name}: the Lightroom develop taken but not saved: {e}"),
    }
    Some(taken)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::on_disk::{Fresh, read_sidecar_said};

    fn load_sidecar_said(
        f: &Path,
        sidecars: Option<Placement>,
    ) -> (Sidecar, Option<crate::on_disk::Trouble>) {
        crate::on_disk::load_sidecar_said(
            f,
            sidecars,
            Fresh::default(),
            crate::on_disk::frame_shape,
        )
    }

    /// Sidecars written beside the frame, as the editor's setting
    /// says by default.
    const BESIDE: Option<Placement> = Some(Placement::Beside);

    /// A Lightroom sidecar as Lightroom writes it for a frame nobody
    /// developed: every slider where Lightroom starts one, the lens
    /// profile's switch as `lens` says, and `HasSettings` true.
    fn as_it_came(lens: u8) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:ProcessVersion="15.4" crs:WhiteBalance="As Shot"
   crs:Exposure2012="0.00" crs:Contrast2012="0" crs:Highlights2012="0" crs:Shadows2012="0"
   crs:Whites2012="0" crs:Blacks2012="0" crs:Texture="0" crs:Clarity2012="0" crs:Dehaze="0"
   crs:Vibrance="0" crs:Saturation="0" crs:ParametricShadows="0"
   crs:ParametricShadowSplit="25" crs:Sharpness="40" crs:LuminanceSmoothing="0"
   crs:ColorNoiseReduction="25" crs:HueAdjustmentRed="0" crs:SplitToningShadowHue="0"
   crs:SplitToningShadowSaturation="0" crs:PostCropVignetteAmount="0" crs:GrainAmount="0"
   crs:LensProfileEnable="{lens}" crs:LensManualDistortionAmount="0"
   crs:ConvertToGrayscale="False" crs:ToneCurveName2012="Linear"
   crs:CameraProfile="Adobe Standard" crs:HasCrop="False" crs:HasSettings="True">
   <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
        )
    }

    /// A Lightroom sidecar written by hand, nothing in it from a real
    /// file: a stop brighter, a little contrast, three stars, a red
    /// label, a title and a keyword under two others.
    pub(crate) const PACKET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="3"
   xmp:Label="Red"
   crs:ProcessVersion="11.0"
   crs:WhiteBalance="As Shot"
   crs:Exposure2012="+1.00"
   crs:Contrast2012="+20"
   crs:HasCrop="False"
   crs:HasSettings="True">
   <dc:title><rdf:Alt><rdf:li xml:lang="x-default">Harbor</rdf:li></rdf:Alt></dc:title>
   <dc:subject><rdf:Bag><rdf:li>Skye</rdf:li></rdf:Bag></dc:subject>
   <lr:hierarchicalSubject><rdf:Bag><rdf:li>Places|Scotland|Skye</rdf:li></rdf:Bag></lr:hierarchicalSubject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("greycard-beside-{name}-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A raw that will not probe, as the catalog's tests have: the
    /// develop still comes across, the crop and the body do not.
    fn raw_in(dir: &Path, name: &str) -> PathBuf {
        let raw = dir.join(name);
        std::fs::write(&raw, b"not a raw").unwrap();
        raw
    }

    fn label(s: &Sidecar) -> Option<&str> {
        s.line.current_label()
    }

    #[test]
    fn a_first_open_takes_the_develop_and_the_meta_once() {
        let dir = folder("open");
        let raw = raw_in(&dir, "IMG_0001.CR3");
        std::fs::write(xmp::short_path(&raw), PACKET).unwrap();
        let (sidecar, trouble) = load_sidecar_said(&raw, BESIDE);
        assert_eq!(trouble, None, "written, so not missing");
        assert_eq!(sidecar.line.history.len(), 1);
        assert_eq!(label(&sidecar), Some("Imported from Lightroom"));
        assert!((sidecar.line.current().light.exposure - 1.0).abs() < 1e-6);
        // The import's start is the root, recorded whole under an id made
        // from it, never changed under one (§724).
        let root = &sidecar.line.history[0];
        assert_eq!(
            root.id(),
            Some(
                greycard_edit::sync::state_id(
                    greycard_edit::sync::ZERO_ID,
                    &root.written(),
                    root.label()
                )
                .as_str()
            )
        );
        assert_eq!(sidecar.meta.rating, 3);
        assert_eq!(sidecar.meta.label, greycard_edit::meta::Label::Red);
        assert_eq!(sidecar.meta.title, "Harbor");
        assert_eq!(sidecar.meta.keywords, ["Places/Scotland/Skye"]);
        assert_eq!(sidecar.lightroom.rating, Some(3), "recorded as laid");
        assert_eq!(
            sidecar.lightroom.keywords.as_deref(),
            Some(&["Places/Scotland/Skye".to_string()][..])
        );
        // On disk, and taken once: a second open finds our edit.
        let saved = Sidecar::load_as_file(&raw)
            .unwrap()
            .expect("a .gcd written");
        assert_eq!(saved.line.history.len(), 1);
        let (again, _) = load_sidecar_said(&raw, BESIDE);
        assert_eq!(again.line.history.len(), 1);
        assert_eq!(again.line.current(), sidecar.line.current());
        // Sidecars off: nothing read, nothing taken.
        let other = raw_in(&dir, "IMG_0002.CR3");
        std::fs::write(xmp::short_path(&other), PACKET).unwrap();
        let (off, _) = load_sidecar_said(&other, None);
        assert!(off.line.history.is_empty());
        assert!(Sidecar::find(&other).is_none());
        // A reader that does not know where sidecars go takes nothing.
        let (read, _) = read_sidecar_said(&other);
        assert!(read.line.history.is_empty());
        assert_eq!(read.meta.rating, 3, "the meta adopted as before");
        assert!(Sidecar::find(&other).is_none());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn an_edit_of_ours_is_left_alone() {
        let dir = folder("ours");
        let raw = raw_in(&dir, "IMG_0001.CR3");
        let mut ours = Sidecar::default();
        let mut edit = Edit::default();
        edit.light.exposure = -0.5;
        assert!(ours.record(edit));
        ours.save(&raw).unwrap();
        std::fs::write(xmp::long_path(&raw), PACKET).unwrap();
        let (sidecar, _) = load_sidecar_said(&raw, BESIDE);
        assert_eq!(sidecar.line.history.len(), 1);
        assert_eq!(sidecar.line.current().light.exposure, -0.5);
        assert_eq!(label(&sidecar), None);
        // A frame only rated here, which took this XMP's word before
        // (its mark), still takes the develop, and keeps its own meta;
        // the XMP's goes only where ours says nothing.
        let rated = raw_in(&dir, "IMG_0002.CR3");
        std::fs::write(xmp::long_path(&rated), PACKET).unwrap();
        let mut stars = Sidecar::default();
        stars.meta.rating = 1;
        stars.meta.set_keywords(vec!["Mine".into()]);
        stars.xmp = Some(Adopted::of(&xmp::long_path(&rated), PACKET.as_bytes()));
        stars.save(&rated).unwrap();
        let (sidecar, _) = load_sidecar_said(&rated, BESIDE);
        assert_eq!(label(&sidecar), Some("Imported from Lightroom"));
        assert!((sidecar.line.current().light.exposure - 1.0).abs() < 1e-6);
        assert_eq!(
            (sidecar.meta.rating, &sidecar.meta.keywords[..]),
            (1, &["Mine".to_string()][..]),
            "ours kept"
        );
        assert_eq!(
            sidecar.meta.title, "Harbor",
            "an empty field takes the XMP's"
        );
        assert_eq!(
            sidecar.lightroom,
            greycard_edit::meta::Laid {
                label: Some(greycard_edit::meta::Label::Red),
                title: Some("Harbor".into()),
                ..Default::default()
            },
            "what came from the XMP, recorded"
        );
        let saved = Sidecar::load_as_file(&rated).unwrap().unwrap();
        assert_eq!(saved.meta, sidecar.meta);
        assert_eq!(saved.lightroom, sidecar.lightroom);
        // And a later open does not lay the XMP's over it.
        let (again, _) = load_sidecar_said(&rated, BESIDE);
        assert_eq!(again.meta, sidecar.meta);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A newer greycard's `.gcd` (§724), with no state of ours a take
    /// would keep: read, and never written over by the XMP's develop.
    #[test]
    fn a_newer_build_s_sidecar_is_not_taken_over() {
        let dir = folder("newer");
        let raw = raw_in(&dir, "IMG_0001.CR3");
        let newer = r#"{"floors": {"read": 10, "write": 11}, "meta": {"rating": 2}}"#;
        std::fs::write(Sidecar::path_for(&raw), newer).unwrap();
        std::fs::write(xmp::long_path(&raw), PACKET).unwrap();
        let (sidecar, _) = load_sidecar_said(&raw, BESIDE);
        assert!(sidecar.read_only());
        assert_eq!(label(&sidecar), None, "the develop not taken");
        assert_eq!(sidecar.meta.rating, 2);
        assert_eq!(
            std::fs::read_to_string(Sidecar::path_for(&raw)).unwrap(),
            newer
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A meta-only `.gcd` from a catalog import of a frame never
    /// developed there: the take records what the XMP changes and keeps
    /// the catalog's record for the rest.
    #[test]
    fn a_catalog_s_record_is_kept_where_the_xmp_changes_nothing() {
        let dir = folder("catalog-record");
        let raw = raw_in(&dir, "IMG_0001.CR3");
        std::fs::write(xmp::short_path(&raw), PACKET).unwrap();
        let mut catalog = Sidecar::default();
        catalog.meta.rating = 3;
        catalog.meta.set_caption("From the catalog");
        catalog.lightroom.rating = Some(3);
        catalog.lightroom.caption = Some("From the catalog".into());
        // The XMP's word taken already, as the catalog's sidecar was.
        catalog.xmp = Some(Adopted::of(&xmp::short_path(&raw), PACKET.as_bytes()));
        catalog.save(&raw).unwrap();
        let (sidecar, _) = load_sidecar_said(&raw, BESIDE);
        assert_eq!(label(&sidecar), Some("Imported from Lightroom"));
        let laid = &sidecar.lightroom;
        assert_eq!(laid.rating, Some(3), "the catalog's, kept");
        assert_eq!(laid.caption.as_deref(), Some("From the catalog"));
        assert_eq!(laid.title.as_deref(), Some("Harbor"), "the XMP's, laid");
        assert_eq!(laid.label, Some(greycard_edit::meta::Label::Red));
        assert_eq!(
            Sidecar::load_as_file(&raw).unwrap().unwrap().lightroom,
            *laid
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A `.gcd` with a rating of ours that never took this XMP's word:
    /// the XMP's wins, as an open's adoption has it, the take keeps
    /// what the adoption laid, and it is recorded as Lightroom's.
    #[test]
    fn an_xmp_changed_since_the_gcd_took_it_wins_its_fields() {
        let dir = folder("changed");
        let raw = raw_in(&dir, "IMG_0001.CR3");
        let mut one = Sidecar::default();
        one.meta.rating = 1;
        one.save(&raw).unwrap();
        std::fs::write(xmp::short_path(&raw), PACKET).unwrap();
        let (sidecar, _) = load_sidecar_said(&raw, BESIDE);
        assert_eq!(label(&sidecar), Some("Imported from Lightroom"));
        assert_eq!(sidecar.meta.rating, 3, "the XMP's word");
        assert_eq!(sidecar.lightroom.rating, Some(3));
        assert_eq!(sidecar.meta.keywords, ["Places/Scotland/Skye"]);
        let saved = Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert_eq!(saved.meta.rating, 3);
        assert!(saved.xmp.is_some(), "marked as taken");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_raw_without_a_develop_beside_it_is_unchanged() {
        let dir = folder("none");
        let bare = raw_in(&dir, "IMG_0001.CR3");
        let (sidecar, trouble) = load_sidecar_said(&bare, BESIDE);
        assert_eq!(trouble, Some(on_disk::Trouble::Missing));
        assert!(sidecar.line.history.is_empty());
        // Lightroom's own start, the lens profile off or on, whatever
        // `HasSettings` says.
        let plain = raw_in(&dir, "IMG_0002.CR3");
        std::fs::write(xmp::short_path(&plain), as_it_came(0)).unwrap();
        assert!(!waits(&plain));
        let lens = raw_in(&dir, "IMG_0005.CR3");
        std::fs::write(xmp::short_path(&lens), as_it_came(1)).unwrap();
        assert!(!waits(&lens));
        // One slider moved from it is a develop.
        let moved = dir.join("IMG_0006.CR3");
        std::fs::write(&moved, b"not a raw").unwrap();
        std::fs::write(
            xmp::short_path(&moved),
            as_it_came(0).replace(r#"crs:Vibrance="0""#, r#"crs:Vibrance="+5""#),
        )
        .unwrap();
        assert!(waits(&moved));
        // Settings that change nothing.
        let neutral = raw_in(&dir, "IMG_0003.CR3");
        std::fs::write(
            xmp::short_path(&neutral),
            PACKET
                .replace(r#"crs:Exposure2012="+1.00""#, r#"crs:Exposure2012="0""#)
                .replace(r#"crs:Contrast2012="+20""#, r#"crs:Contrast2012="0""#),
        )
        .unwrap();
        assert!(!waits(&neutral));
        // A picture is not a raw.
        let picture = dir.join("IMG_0004.jpg");
        std::fs::write(&picture, b"jpeg").unwrap();
        std::fs::write(xmp::short_path(&picture), PACKET).unwrap();
        for f in [&bare, &plain, &lens, &neutral, &picture] {
            let (s, _) = load_sidecar_said(f, BESIDE);
            assert!(s.line.history.is_empty(), "{}", f.display());
            assert!(Sidecar::find(f).is_none(), "{}", f.display());
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Settings moved from Lightroom's start that carry nothing here
    /// (an old process's sliders, a stray old key in a later file) are
    /// no take: nothing written, open after open, and not counted;
    /// the packet's rating still comes as an open adopts it. A real
    /// develop still takes.
    #[test]
    fn a_develop_that_carries_nothing_is_not_taken() {
        let dir = folder("carries-nothing");
        let packet = |attrs: &str| {
            format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
                  xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmp:Rating="2" {attrs}/>
                </rdf:RDF></x:xmpmeta>"#
            )
        };
        for (name, attrs) in [
            ("shadows.CR2", r#"crs:ProcessVersion="5.7" crs:Shadows="9""#),
            (
                "brightness.CR2",
                r#"crs:ProcessVersion="5.7" crs:Brightness="70""#,
            ),
            (
                "stray.CR2",
                r#"crs:ProcessVersion="11.0" crs:Exposure2012="0" crs:Brightness="70""#,
            ),
        ] {
            let raw = raw_in(&dir, name);
            std::fs::write(xmp::long_path(&raw), packet(attrs)).unwrap();
            assert!(waits(&raw), "{name}: moved from Lightroom's start");
            for _ in 0..2 {
                let (sidecar, _) = load_sidecar_said(&raw, BESIDE);
                assert!(sidecar.line.history.is_empty(), "{name}");
                assert_eq!(sidecar.meta.rating, 2, "{name}: the meta adopted");
                assert!(Sidecar::find(&raw).is_none(), "{name}: nothing written");
                let mut fresh = Sidecar::default();
                assert_eq!(take_and_save(&raw, &mut fresh, Placement::Beside), None);
                assert!(Sidecar::find(&raw).is_none(), "{name}: not counted");
            }
        }
        // A develop that moves something still takes.
        let real = raw_in(&dir, "real.CR2");
        std::fs::write(
            xmp::long_path(&real),
            packet(r#"crs:ProcessVersion="5.7" crs:Shadows="9" crs:Exposure="+0.5""#),
        )
        .unwrap();
        let mut fresh = Sidecar::default();
        let taken = take_and_save(&real, &mut fresh, Placement::Beside).expect("taken");
        assert!(taken.saved);
        assert_eq!(taken.unmapped, ["Shadows"]);
        assert!(Sidecar::find(&real).is_some());

        // An XMP past the most read beside a raw is not read: no take,
        // no panic.
        let long = raw_in(&dir, "long.CR2");
        let file = std::fs::File::create(xmp::long_path(&long)).unwrap();
        file.set_len(xmp::BESIDE_MOST as u64 + 1).unwrap();
        drop(file);
        assert!(!waits(&long));
        assert_eq!(path_for(&long), None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn the_stems_xmp_is_a_raws_unless_another_raw_shares_the_stem() {
        let dir = folder("stem");
        let raw = raw_in(&dir, "IMG_0001.CR3");
        std::fs::write(dir.join("IMG_0001.JPG"), b"jpeg").unwrap();
        std::fs::write(xmp::short_path(&raw), PACKET).unwrap();
        // The camera's JPEG beside it claims none.
        assert_eq!(path_for(&raw), Some(xmp::short_path(&raw)));
        assert!(waits(&raw));
        let (s, _) = load_sidecar_said(&raw, BESIDE);
        assert_eq!(label(&s), Some("Imported from Lightroom"));
        assert_eq!(s.meta.rating, 3, "the meta from the pair's XMP too");
        // A second raw of the stem: neither is sure it is its.
        let twin = raw_in(&dir, "IMG_0002.CR3");
        raw_in(&dir, "IMG_0002.DNG");
        std::fs::write(xmp::short_path(&twin), PACKET).unwrap();
        assert_eq!(path_for(&twin), None);
        // Its long name is always its own.
        std::fs::write(xmp::long_path(&twin), PACKET).unwrap();
        assert_eq!(path_for(&twin), Some(xmp::long_path(&twin)));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A High Res Shot's `P1.xmp` is the merge's (`.ORF`): the ORF
    /// takes Lightroom's develop and meta from it, the single frame
    /// (`.ORI`) beside it nothing, and nothing once the ORF is gone or
    /// with no ORF at all: its own is its long name. A RAF and its DNG
    /// still take it neither.
    #[test]
    fn the_stems_xmp_is_the_orfs_never_its_oris() {
        let dir = folder("ori");
        let orf = raw_in(&dir, "P1.ORF");
        let ori = raw_in(&dir, "P1.ORI");
        std::fs::write(xmp::short_path(&orf), PACKET).unwrap();
        assert_eq!(path_for(&orf), Some(xmp::short_path(&orf)));
        assert!(waits(&orf));
        assert!(!waits(&ori));
        assert_eq!(path_for(&ori), None);
        let mut s = Sidecar::default();
        assert!(take(&ori, &mut s).is_none());
        assert_eq!(s.meta.rating, 0);
        let (s, _) = load_sidecar_said(&orf, BESIDE);
        assert_eq!(label(&s), Some("Imported from Lightroom"));
        assert_eq!(s.meta.rating, 3);
        assert_eq!(s.meta.keywords, ["Places/Scotland/Skye"]);
        let (s, _) = load_sidecar_said(&ori, BESIDE);
        assert!(s.line.history.is_empty());
        assert_eq!(s.meta.rating, 0);
        assert!(Sidecar::find(&ori).is_none());
        // The ORF gone: the XMP is nobody's, the ORI's still none.
        std::fs::remove_file(&orf).unwrap();
        assert_eq!(path_for(&orf), None);
        assert!(!waits(&ori));

        let alone = folder("ori-alone");
        let ori = raw_in(&alone, "P2.ORI");
        std::fs::write(xmp::short_path(&ori), PACKET).unwrap();
        assert!(!waits(&ori));
        let (s, _) = load_sidecar_said(&ori, BESIDE);
        assert!(s.line.history.is_empty());
        assert_eq!(s.meta.rating, 0);
        // Its long name is its own.
        std::fs::write(xmp::long_path(&ori), PACKET).unwrap();
        assert!(waits(&ori));
        assert_eq!(path_for(&ori), Some(xmp::long_path(&ori)));

        let pair = folder("raf-dng");
        let raf = raw_in(&pair, "D1.RAF");
        let dng = raw_in(&pair, "D1.DNG");
        std::fs::write(xmp::short_path(&raf), PACKET).unwrap();
        assert_eq!((path_for(&raf), path_for(&dng)), (None, None));
        for d in [dir, alone, pair] {
            greycard_core::testing::remove_dir_retry(&d);
        }
    }

    #[test]
    fn of_two_names_the_one_with_lightroom_s_settings_is_read() {
        let dir = folder("names");
        let raw = raw_in(&dir, "IMG_0001.CR3");
        // Ours, meta only, under the long name; Lightroom's under the
        // stem's.
        let ours = Meta {
            rating: 2,
            ..Meta::default()
        };
        std::fs::write(xmp::long_path(&raw), xmp::fresh(&ours, None)).unwrap();
        std::fs::write(xmp::short_path(&raw), PACKET).unwrap();
        assert_eq!(path_for(&raw), Some(xmp::short_path(&raw)));
        // Both Lightroom's: the newer.
        std::fs::write(xmp::long_path(&raw), PACKET).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(xmp::short_path(&raw))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(path_for(&raw), Some(xmp::long_path(&raw)));
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
