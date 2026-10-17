//! RawTherapee's processing profiles read: the `.pp3` beside a raw,
//! its rating, label and trash mark carried into the meta exactly,
//! and the parts of its edit that mean something here mapped onto an
//! edit.
//!
//! A `.pp3` is a GLib key file: `[Section]` lines, `Key=value` under
//! them, whole-line comments. RawTherapee writes every tool it has,
//! most of them switched off, so a profile is read for what it sets
//! rather than for what it names. The `[General]` section is exact:
//! `Rank` is the stars, 0 to 5; `ColorLabel` is RawTherapee's 0 to 5,
//! none, red, yellow, green, blue and purple (its file browser's
//! `FILEBROWSER_POPUPCOLORLABEL0..5`, "Label: None" to "Label:
//! Purple"), the same five as [`Label`]; `InTrash=true` is a reject.
//!
//! The edit is a starting point, as a Lightroom preset's is
//! ([`crate::lightroom`]), and is said to be. Two things are the same
//! here as there and come across as they are: the geometry's quarter
//! turns, mirrors and fine angle, since RawTherapee's rotation samples
//! the source through the same matrix [`Geometry::to_source`] does,
//! positive counter-clockwise on the screen; and the lateral
//! chromatic aberration set by hand, a scale on the red's and the
//! blue's radius in both. The rest is approximate: the exposure in
//! stops lands on a different tone pipeline; the white balance's
//! kelvin is put where RawTherapee puts it (on the black body's locus
//! to 4000 K, on daylight's above) and its green, a multiplier on the
//! white's green channel, is taken as Duv from there, but the white
//! reaches the camera through RawTherapee's matrices there and this
//! engine's here; the crop is in RawTherapee's pixels of its own
//! decode of the frame, which can differ from this one's by a few at
//! the edges, and on its picture as zoomed, which is undone here only
//! for the zoom a fine angle's fill or the transforms' scales make; the
//! manual distortion is a linear term in half diagonals there and a
//! poly3 in half shorter sides here, fitted to agree at the center
//! and the corner; and a lens profile there is looked up from
//! lensfun here, whatever RawTherapee took it from.
//!
//! RawTherapee's tone curves are not carried. Its curve does the tone
//! mapping a raw gets from the base curve here, so laid over that
//! curve it would apply the contrast twice; its auto-matched curve,
//! an imitation of the camera's JPEG, is the base curve's job here
//! outright. What has no counterpart, or none clear enough to guess
//! at, is passed over and named: a section by RawTherapee's name for
//! it when it is switched on ("Local Contrast", "Wavelet"), a key by
//! its section's and its own when the section is one read here
//! ("Exposure Contrast").
//!
//! What is carried is what the profile sets off RawTherapee's neutral,
//! as a preset carries what it sets ([`Carry::Set`]): over a frame
//! with no history here nothing else could differ. An import forced
//! over a frame developed here carries every setting a section read
//! here speaks to, neutral ones too ([`Carry::Every`]), so the frame
//! comes out as RawTherapee has it. Either way two things are written
//! that the profile may not set: the camera's white balance, which is
//! carried whenever the profile says "Camera", and the hand-set
//! chromatic aberration, which shares its part with the profile's
//! switch for it and so is written, at zero when the profile has none,
//! whenever a lens profile is carried.

use crate::geometry::{Aspect, Crop, Geometry, MIN_CROP};
use crate::meta::{Flag, Label, Meta, STARS};
use crate::preset::{Choice, Part, Preset, Section};
use crate::{Edit, Error, Result, Sidecar, WhiteBalance, history};

/// The profile versions this reader was written against: from the
/// one that stored a histogram-matched curve's origin (RawTherapee
/// 5.5's era) to the newest at the time of writing. A profile outside
/// them is read all the same, and the report says so.
pub const KNOWN_VERSIONS: std::ops::RangeInclusive<u32> = 340..=353;

/// The name the history and the report give the import's source.
pub const SOURCE: &str = "RawTherapee";

/// A key file, parsed: its sections in the order the file has them,
/// each with its keys in order. A section named twice is one section,
/// and a key given twice in one is its last value. Looked up by a walk
/// of the list: a profile has some sixty sections of a few dozen keys,
/// and is read once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Profile {
    /// The value of `key` in `section`, as written, unescaped.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.keys(section)?
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The keys of `section`, if the file has it.
    pub fn keys(&self, section: &str) -> Option<&[(String, String)]> {
        self.sections
            .iter()
            .find(|(name, _)| name == section)
            .map(|(_, keys)| keys.as_slice())
    }

    /// The sections' names, in the file's order.
    pub fn sections(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().map(|(name, _)| name.as_str())
    }

    /// A number, finite: `nan` and `inf` parse as floats, and either
    /// carried into an edit would write a sidecar that does not read.
    fn num(&self, section: &str, key: &str) -> Option<f64> {
        self.get(section, key)?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }

    fn int(&self, section: &str, key: &str) -> Option<i64> {
        self.get(section, key)?.trim().parse::<i64>().ok()
    }

    /// A key file's boolean: `true` or `1`, `false` or `0`.
    fn flag(&self, section: &str, key: &str) -> Option<bool> {
        match self.get(section, key)?.trim() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }
}

/// Read a key file. Lines are trimmed; a line starting `#` or `;` is
/// a comment; a value is unescaped as GLib escapes it (`\s`, `\n`,
/// `\t`, `\r`, `\\`) and a pair of double quotes around it dropped,
/// for a profile written by hand. A line before the first section, or
/// without an `=`, is passed over. Refused only when there is no
/// section at all, which is not a profile.
pub fn parse(text: &str) -> Result<Profile> {
    let mut profile = Profile::default();
    let mut current: Option<usize> = None;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = name.trim();
            current = Some(match profile.sections.iter().position(|(n, _)| n == name) {
                Some(i) => i,
                None => {
                    profile.sections.push((name.to_string(), Vec::new()));
                    profile.sections.len() - 1
                }
            });
            continue;
        }
        let (Some(i), Some((key, value))) = (current, line.split_once('=')) else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        profile.sections[i]
            .1
            .push((key.trim().to_string(), unescape(value)));
    }
    if profile.sections.is_empty() {
        return Err(Error::Pp3("no sections in the file".into()));
    }
    Ok(profile)
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// What the import knows of the frame a profile is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Frame {
    /// The frame's size the way up the camera's tag stands it, which
    /// is the frame RawTherapee measures its crop on; none when the
    /// file will not say. Without it the crop is passed over.
    pub size: Option<(u32, u32)>,
    /// The frame carries a quarter turn of its own here
    /// ([`Sidecar::turn`]), which RawTherapee never saw: its crop,
    /// rotation and turns would land on a different picture, so they
    /// are passed over.
    pub turned: bool,
}

/// How much of a profile's edit is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Carry {
    /// What the profile sets off RawTherapee's neutral, as a preset
    /// carries what it sets: for a frame with no history here, whose
    /// edit is this engine's default and has nothing to take back.
    #[default]
    Set,
    /// Every setting a section read here speaks to, the neutral ones
    /// too: for an import forced over a frame developed here, which
    /// should come out as RawTherapee has it, an exposure set back to
    /// nothing there set back to nothing here.
    Every,
}

/// The `[General]` section: what comes into the meta, exactly. A key
/// the profile does not have leaves its field as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rated {
    pub rating: Option<u8>,
    pub label: Option<Label>,
    /// `InTrash`: true a reject. False clears a reject, and leaves a
    /// pick, since RawTherapee has no pick to say otherwise.
    pub trash: Option<bool>,
}

impl Rated {
    /// Nothing to carry.
    pub fn is_empty(&self) -> bool {
        *self == Rated::default()
    }

    /// Put it into `meta`; whether anything changed.
    pub fn apply(&self, meta: &mut Meta) -> bool {
        let before = meta.clone();
        if let Some(stars) = self.rating {
            meta.set_rating(stars);
        }
        if let Some(label) = self.label {
            meta.label = label;
        }
        match self.trash {
            Some(true) => meta.flag = Flag::Reject,
            Some(false) if meta.flag == Flag::Reject => meta.flag = Flag::None,
            _ => {}
        }
        *meta != before
    }
}

/// A profile read, with what came across and what could not.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// `[Version] AppVersion`, as written.
    pub app_version: Option<String>,
    /// `[Version] Version`, the profile's own version.
    pub version: Option<u32>,
    pub rated: Rated,
    /// The edit's settings, as the parts and sections of a preset, so
    /// that each lands on a frame as a preset's would and nothing else
    /// of the frame's edit moves.
    pub preset: Preset,
    /// The crop, the angle and the quarter turns, whole, when the
    /// profile set any of them, or under [`Carry::Every`] has their
    /// sections at all.
    pub geometry: Option<Geometry>,
    /// The profile's crop could not be placed (the frame's size is not
    /// known), so the crop the frame has, with its aspect and way up,
    /// is kept under the geometry carried rather than cleared by it.
    pub keeps_crop: bool,
    /// What came across as it is, in a few words each.
    pub exact: Vec<String>,
    /// What came across as a likeness.
    pub approximated: Vec<String>,
    /// What came across at RawTherapee's neutral, carried only under
    /// [`Carry::Every`].
    pub neutral: Vec<String>,
    /// What was set and has no place here: a section's name, or a
    /// section's and a key's, with a reason after a section read here
    /// that could not be carried.
    pub skipped: Vec<String>,
}

impl Imported {
    /// `onto` with the profile's edit laid over it. The geometry is
    /// replaced but for what a profile has nothing to say about: the
    /// keystone, which RawTherapee's Perspective does by a model of its
    /// own (named, not carried), and a crop the profile could not place
    /// ([`Self::keeps_crop`]).
    pub fn applied(&self, onto: &Edit) -> Edit {
        let mut edit = self.preset.applied(onto);
        if let Some(g) = &self.geometry {
            let old = &onto.geometry;
            let mut g = g.clone();
            g.vertical = old.vertical;
            g.horizontal = old.horizontal;
            if self.keeps_crop {
                g.crop = old.crop;
                g.aspect = old.aspect;
                g.portrait = old.portrait;
            }
            edit.geometry = g;
        }
        edit
    }

    /// Whether the profile carries anything into the edit.
    pub fn has_edit(&self) -> bool {
        !self.preset.is_empty() || self.geometry.is_some()
    }

    /// A word on the profile's version when it is not one this reader
    /// knows; none when it is.
    pub fn version_note(&self) -> Option<String> {
        match self.version {
            None => Some("no profile version; read as a current one".into()),
            Some(v) if v > *KNOWN_VERSIONS.end() => Some(format!(
                "profile version {v} is newer than this reader knows ({}); read as that",
                KNOWN_VERSIONS.end()
            )),
            Some(v) if v < *KNOWN_VERSIONS.start() => Some(format!(
                "profile version {v} is older than this reader knows ({}); read as that",
                KNOWN_VERSIONS.start()
            )),
            Some(_) => None,
        }
    }

    /// Where the profile came from, in words: "RawTherapee 5.10,
    /// profile version 351".
    pub fn source(&self) -> String {
        let app = match self.app_version.as_deref().map(str::trim) {
            Some(v) if !v.is_empty() => format!("{SOURCE} {v}"),
            _ => SOURCE.to_string(),
        };
        match self.version {
            Some(v) => format!("{app}, profile version {v}"),
            None => app,
        }
    }

    /// The report, a line a kind that has something to say: the kind
    /// in a word for a column, then the words.
    pub fn report(&self) -> Vec<(&'static str, String)> {
        let mut lines = Vec::new();
        if let Some(note) = self.version_note() {
            lines.push(("version", note));
        }
        if !self.exact.is_empty() {
            lines.push(("exact", self.exact.join(", ")));
        }
        if !self.approximated.is_empty() {
            lines.push(("approximate", self.approximated.join(", ")));
        }
        if !self.neutral.is_empty() {
            lines.push(("neutral", self.neutral.join(", ")));
        }
        if !self.skipped.is_empty() {
            lines.push(("no place", self.skipped.join(", ")));
        }
        lines
    }
}

/// Parse a profile and map it for `frame`, carrying what it sets.
pub fn read(text: &str, frame: Frame) -> Result<Imported> {
    read_carrying(text, frame, Carry::Set)
}

/// Parse a profile and map it for `frame`, carrying as `carry` says.
pub fn read_carrying(text: &str, frame: Frame, carry: Carry) -> Result<Imported> {
    Ok(map(&parse(text)?, frame, carry))
}

/// The transforms' shared settings: AutoFill and the scales, read
/// with the crop.
const COMMON: &str = "Common Properties for Transformations";

/// Sections that hold no edit of their own: the version and the
/// meta, read above; the transforms' shared settings, read with the
/// crop (their scales named there when they do something that cannot
/// be carried); the raw's own processing (the demosaic, the raw-level
/// corrections and white balance, the camera profile and the output
/// space), which is this engine's choice for a raw and not an edit to
/// carry; and the metadata an export writes.
const PLUMBING: &[&str] = &[
    "Version",
    "General",
    COMMON,
    "Color Management",
    "RAW",
    "RAW Bayer",
    "RAW X-Trans",
    "RAW Preprocess WB",
    "MetaData",
    "Exif",
    "IPTC",
];

/// Sections read here, whose keys are reported one by one.
const READ: &[&str] = &[
    "Exposure",
    "White Balance",
    "Crop",
    "Coarse Transformation",
    "Rotation",
    "Distortion",
    "LensProfile",
    "CACorrection",
];

/// The profile, mapped, carrying as `carry` says.
pub fn map(p: &Profile, frame: Frame, carry: Carry) -> Imported {
    let every = carry == Carry::Every;
    let mut edit = Edit::default();
    let mut sections = Vec::new();
    let mut parts = Vec::new();
    let mut exact = Vec::new();
    let mut approximated = Vec::new();
    let mut neutral = Vec::new();
    let mut skipped = Vec::new();

    // The meta, exactly; a value out of RawTherapee's range is named
    // rather than held to it.
    let mut rated = Rated::default();
    if let Some(rank) = p.int("General", "Rank") {
        match u8::try_from(rank).ok().filter(|r| *r <= STARS) {
            Some(r) => {
                rated.rating = Some(r);
                exact.push(match r {
                    1 => "1 star".to_string(),
                    r => format!("{r} stars"),
                });
            }
            None => skipped.push(format!("General Rank {rank}")),
        }
    }
    if let Some(n) = p.int("General", "ColorLabel") {
        match usize::try_from(n).ok().and_then(|i| Label::ALL.get(i)) {
            Some(label) => {
                rated.label = Some(*label);
                exact.push(match label {
                    Label::None => "no label".to_string(),
                    l => format!("label {}", l.name().to_lowercase()),
                });
            }
            None => skipped.push(format!("General ColorLabel {n}")),
        }
    }
    if let Some(trash) = p.flag("General", "InTrash") {
        rated.trash = Some(trash);
        if trash {
            exact.push("reject".into());
        }
    }

    // Exposure: the compensation in stops is the same number on a
    // different tone pipeline.
    let wanted = |v: &f64| every || *v != 0.0;
    if let Some(ev) = p.num("Exposure", "Compensation").filter(wanted) {
        edit.light.exposure = (ev as f32).clamp(-5.0, 5.0);
        parts.push(Part::LightExposure);
        if ev == 0.0 {
            neutral.push("exposure 0".into());
        } else {
            approximated.push(format!("exposure {:+.2} EV", edit.light.exposure));
        }
    }
    // The black point, on RawTherapee's 0..65535 after the exposure:
    // a positive one takes that much of white to black, which here is
    // a crush by a fraction of mid grey. A negative one lifts by a
    // rule of its own, and is named.
    if let Some(black) = p.num("Exposure", "Black").filter(wanted) {
        if black > 0.0 {
            let of_grey = black / 65535.0 / f64::from(greycard_core::develop::noise::MID_GREY);
            edit.light.tone.blacks = (-of_grey as f32).clamp(-0.3, 0.0);
            parts.push(Part::LightBlacks);
            approximated.push("black point".into());
        } else if black == 0.0 {
            parts.push(Part::LightBlacks);
            neutral.push("no black point".into());
        } else {
            skipped.push("Exposure Black (a lift)".into());
        }
    }
    // Saturation, ±100 with -100 grey, onto the global chroma scale
    // whose -1 is grey too.
    if let Some(s) = p.num("Exposure", "Saturation").filter(wanted) {
        edit.color.saturation = (s as f32 / 100.0).clamp(-1.0, 1.0);
        parts.push(Part::ColorSaturation);
        if s == 0.0 {
            neutral.push("saturation 0".into());
        } else {
            approximated.push("saturation".into());
        }
    }
    for key in ["Brightness", "Contrast", "HighlightCompr"] {
        if p.num("Exposure", key).is_some_and(|v| v != 0.0) {
            skipped.push(format!("Exposure {key}"));
        }
    }
    for key in ["Curve", "Curve2"] {
        if p.get("Exposure", key).is_some_and(|c| !is_linear_curve(c)) {
            skipped.push(format!("Exposure {key}"));
        }
    }

    // White balance: the camera's as it is; a temperature as the white
    // RawTherapee puts there ([`rt_white`]), and its green as a tint
    // on that. RawTherapee's green divides the white's green channel,
    // so above one the picture goes greener; near daylight the white
    // moves about 0.05 Duv for each unit of `1 - 1 / green`, and a
    // greener picture is a magenta white point, negative Duv.
    if p.keys("White Balance").is_some() {
        let method = p.get("White Balance", "Setting").unwrap_or("Camera").trim();
        if p.flag("White Balance", "Enabled") == Some(false) {
            skipped.push("White Balance (switched off)".into());
        } else if method.eq_ignore_ascii_case("Camera") {
            // The default either way; carried, so that a frame given
            // a white here takes the camera's back.
            edit.white_balance = WhiteBalance::AsShot;
            sections.push(Section::WhiteBalance);
            exact.push("white balance as shot".into());
        } else if method.to_ascii_lowercase().starts_with("auto") {
            skipped.push(format!("White Balance {method}"));
        } else if let Some(t) = p.num("White Balance", "Temperature").filter(|t| *t > 0.0) {
            let green = p
                .num("White Balance", "Green")
                .filter(|g| *g > 0.0)
                .unwrap_or(1.0);
            let (cct, duv) = rt_white(t);
            let tint = (duv - 0.05 * (1.0 - 1.0 / green)).clamp(-0.05, 0.05);
            edit.white_balance = WhiteBalance::Custom {
                temperature: cct.clamp(2000.0, 12000.0),
                tint,
            };
            sections.push(Section::WhiteBalance);
            approximated.push(format!("white balance {t:.0} K, green {green:.3}"));
        } else {
            skipped.push(format!("White Balance {method}"));
        }
        if p.num("White Balance", "Equal").is_some_and(|e| e != 1.0) {
            skipped.push("White Balance Equal".into());
        }
    }

    // The geometry: the quarter turns and the mirrors, the angle, the
    // crop. RawTherapee turns its picture clockwise by `Rotate` and
    // mirrors the turned picture; here the source is mirrored and
    // then turned counter-clockwise, so a mirror after the turn is a
    // mirror before it with the turn reversed.
    let quarters = p
        .int("Coarse Transformation", "Rotate")
        .map(|r| (r.rem_euclid(360) / 90) as i32)
        .unwrap_or(0);
    let hflip = p.flag("Coarse Transformation", "HorizontalFlip") == Some(true);
    let vflip = p.flag("Coarse Transformation", "VerticalFlip") == Some(true);
    let angle = p
        .num("Rotation", "Degree")
        .filter(|d| *d != 0.0)
        .map(|d| (d as f32).clamp(-crate::geometry::MAX_ANGLE, crate::geometry::MAX_ANGLE));
    let crop = (p.flag("Crop", "Enabled") == Some(true)).then(|| {
        (
            p.num("Crop", "X").unwrap_or(0.0) as f32,
            p.num("Crop", "Y").unwrap_or(0.0) as f32,
            p.num("Crop", "W").unwrap_or(0.0) as f32,
            p.num("Crop", "H").unwrap_or(0.0) as f32,
        )
    });
    let turns_set = quarters != 0 || hflip || vflip;
    // Forced, a profile that has the sections at all says the
    // geometry, nothing set in them a geometry with nothing in it.
    let spoken = every
        && ["Coarse Transformation", "Rotation", "Crop"]
            .iter()
            .any(|s| p.keys(s).is_some());
    // AutoFill off, RawTherapee zooms by `Scale` and stretches by the
    // other two (the zoom one over them, as `ascale / scale_h` in its
    // transform); on, all three are one. Each is held to its slider's
    // 0.1 to 10 there, so that one written by hand as nothing at all
    // (1e-300) cannot make the crop's zoom infinite.
    let autofill = p.flag(COMMON, "AutoFill") != Some(false);
    let scale_of = |key: &str| {
        if autofill {
            1.0
        } else {
            p.num(COMMON, key).map_or(1.0, |s| s.clamp(0.1, 10.0)) as f32
        }
    };
    let (zoom, stretch) = (
        scale_of("Scale"),
        (scale_of("Scale horizontally"), scale_of("Scale vertically")),
    );
    let mut crop_carried = false;
    let mut keeps_crop = false;
    let mut geometry = None;
    if frame.turned && (turns_set || angle.is_some() || crop.is_some()) {
        for (set, name) in [
            (turns_set, "Coarse Transformation"),
            (angle.is_some(), "Rotation"),
            (crop.is_some(), "Crop"),
        ] {
            if set {
                skipped.push(format!("{name} (the frame is turned here)"));
            }
        }
    } else if turns_set || angle.is_some() || crop.is_some() || (spoken && !frame.turned) {
        let mut g = Geometry::default().turned(-quarters);
        if quarters != 0 {
            exact.push(format!("turned {}° clockwise", quarters * 90));
        }
        if hflip {
            g = g.flipped(false);
            exact.push("mirrored left to right".into());
        }
        if vflip {
            g = g.flipped(true);
            exact.push("mirrored top to bottom".into());
        }
        if let Some(a) = angle {
            g.angle = a;
            exact.push(format!("straightened {a:+.2}°"));
        }
        if let Some((x, y, w, h)) = crop {
            match frame.size {
                Some((fw, fh)) if w >= 1.0 && h >= 1.0 => {
                    let (pw, ph) = g.plane_size(fw as f32, fh as f32);
                    // RawTherapee's crop is on its picture as zoomed:
                    // by AutoFill's fill under an angle, or by the
                    // scales without it. This plane is not zoomed.
                    let scales = if autofill {
                        let fill = angle.map_or(1.0, |a| fill_scale(pw, ph, a));
                        (fill, fill)
                    } else {
                        let unzoom = |s: f32| Some(1.0 / zoom / s).filter(|u| u.is_finite());
                        (
                            unzoom(stretch.0).unwrap_or(1.0),
                            unzoom(stretch.1).unwrap_or(1.0),
                        )
                    };
                    g.crop = Some(crop_fractions((x, y, w, h), (pw, ph), scales));
                    if p.flag("Crop", "FixedRatio") == Some(true) {
                        g.aspect = ratio(p.get("Crop", "Ratio").unwrap_or(""));
                    }
                    // A named ratio's portrait is the crop's own way
                    // up; the image's own ratio's is against the
                    // plane's ([`Aspect::ratio`]): a tall crop of a
                    // tall frame is not turned.
                    g.portrait = if g.aspect == Aspect::Original {
                        (h > w) != (ph > pw)
                    } else {
                        h > w
                    };
                    approximated.push("crop".into());
                    crop_carried = true;
                }
                Some(_) => {
                    skipped.push("Crop (no size)".into());
                    keeps_crop = true;
                }
                None => {
                    skipped.push("Crop (the frame's size is not known)".into());
                    keeps_crop = true;
                }
            }
        }
        if g != Geometry::default() {
            geometry = Some(g);
        } else if spoken {
            geometry = Some(g);
            neutral.push("no crop or turn".into());
        }
    }
    // A zoom with no crop to carry it is a crop this engine was not
    // given; a stretch has no counterpart at all.
    if !crop_carried && zoom != 1.0 {
        skipped.push(format!("{COMMON} Scale"));
    }
    for (s, key) in [
        (stretch.0, "Scale horizontally"),
        (stretch.1, "Scale vertically"),
    ] {
        if s != 1.0 {
            skipped.push(format!("{COMMON} {key}"));
        }
    }

    // The lens: a profile is lensfun's here whichever RawTherapee
    // took, its three corrections switched as there. An LCP mode with
    // no file named is no profile there (`useLcp`).
    let mode = p
        .get("LensProfile", "LcMode")
        .map(str::trim)
        .unwrap_or("none");
    let no_lcp = mode == "lcp"
        && p.get("LensProfile", "LCPFile")
            .is_none_or(|f| f.trim().is_empty());
    let profiled = match mode {
        "none" | "" => false,
        "lcp" if no_lcp => false,
        "lfauto" => {
            exact.push("lens profile (lensfun)".into());
            true
        }
        "lfmanual" => {
            approximated.push("lens profile (lensfun, by the file's lens)".into());
            true
        }
        "lcp" => {
            approximated.push("lens profile (lensfun for the LCP)".into());
            true
        }
        "metadata" => {
            approximated.push("lens profile (lensfun for the file's own)".into());
            true
        }
        other => {
            skipped.push(format!("LensProfile LcMode {other}"));
            false
        }
    };
    if profiled {
        let on = |key: &str, default: bool| p.flag("LensProfile", key).unwrap_or(default);
        edit.lens.profile = true;
        edit.lens.distortion = on("UseDistortion", true);
        edit.lens.vignetting = on("UseVignette", true);
        edit.lens.chromatic_aberration = on("UseCA", false);
        parts.extend([
            Part::LensProfile,
            Part::LensVignetting,
            Part::LensChromaticAberration,
        ]);
    } else if every && p.keys("LensProfile").is_some() && (matches!(mode, "none" | "") || no_lcp) {
        edit.lens.profile = false;
        parts.push(Part::LensProfile);
        neutral.push("no lens profile".into());
    }
    if let Some(a) = p.num("Distortion", "Amount").filter(wanted) {
        let (w, h) = frame.size.unwrap_or((3, 2));
        edit.lens.manual = poly3_for(a, w as f64, h as f64).clamp(
            -crate::lens::MAX_MANUAL as f64,
            crate::lens::MAX_MANUAL as f64,
        ) as f32;
        parts.push(Part::LensDistortion);
        if a == 0.0 {
            neutral.push("no manual distortion".into());
        } else {
            approximated.push("manual distortion".into());
        }
    }
    // The hand-set aberration rides in one part with the profile's
    // switch for it, so a profile carried writes these too, at zero
    // when the profile has none.
    let red = p.num("CACorrection", "Red").unwrap_or(0.0);
    let blue = p.num("CACorrection", "Blue").unwrap_or(0.0);
    if red != 0.0 || blue != 0.0 || (every && p.keys("CACorrection").is_some()) {
        let m = crate::lens::MAX_MANUAL_CA;
        edit.lens.ca_red = (red as f32).clamp(-m, m);
        edit.lens.ca_blue = (blue as f32).clamp(-m, m);
        if !parts.contains(&Part::LensChromaticAberration) {
            parts.push(Part::LensChromaticAberration);
        }
        if red != 0.0 || blue != 0.0 {
            exact.push("chromatic aberration by hand".into());
        } else {
            neutral.push("no chromatic aberration by hand".into());
        }
    }

    // What else was set: a section switched on, or one with no switch
    // whose values say something.
    for name in p.sections() {
        if PLUMBING.contains(&name) || READ.contains(&name) {
            continue;
        }
        let on = match p.flag(name, "Enabled") {
            Some(on) => on,
            None => match name {
                // Every number at zero is no correction at all.
                "Perspective" => p
                    .keys(name)
                    .unwrap_or(&[])
                    .iter()
                    .any(|(_, v)| v.trim().parse::<f64>().is_ok_and(|n| n != 0.0)),
                "Vignetting Correction" => p.num(name, "Amount").is_some_and(|a| a != 0.0),
                // A section this reader does not know, with no switch
                // to say it is off: named, since nothing says it does
                // nothing.
                _ => true,
            },
        };
        if on && !skipped.iter().any(|s| s == name) {
            skipped.push(name.to_string());
        }
    }

    let choice = Choice {
        sections,
        parts,
        ..Choice::default()
    };
    Imported {
        app_version: p.get("Version", "AppVersion").map(str::to_string),
        version: p
            .int("Version", "Version")
            .and_then(|v| u32::try_from(v).ok()),
        rated,
        preset: Preset::from_choice(SOURCE, &edit, &choice),
        geometry,
        keeps_crop,
        exact,
        approximated,
        neutral,
        skipped,
    }
}

/// The white RawTherapee puts at `kelvin`, as this engine's kelvin and
/// Duv. Up to 4000 K it is the black body's, on the locus; above, the
/// CIE daylight locus's, which sits above the black body's by about
/// 0.003 Duv between 5000 and 6500 K (`ColorTemp::temp2mulxyz`, from
/// the D illuminant's chromaticity at that temperature). A named
/// preset with a spectrum of its own there (a fluorescent, a lamp) is
/// taken at its kelvin all the same.
fn rt_white(kelvin: f64) -> (f64, f64) {
    if kelvin <= 4000.0 {
        return (kelvin, 0.0);
    }
    let t = kelvin.min(25000.0);
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 7000.0 {
        -4.6070e9 / t3 + 2.9678e6 / t2 + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / t3 + 1.9018e6 / t2 + 0.24748e3 / t + 0.237040
    };
    let y = -3.0 * x * x + 2.87 * x - 0.275;
    match rawcolor::cct::chromaticity_to_temp_tint(rawcolor::Chromaticity::new(x, y)) {
        Some(tt) => (tt.cct, tt.duv),
        None => (kelvin, 0.0),
    }
}

/// Whether a curve's text is one that does nothing: empty, linear
/// (type 0), or a spline (type 1) or Catmull-Rom (type 4) whose
/// points all lie on the diagonal.
fn is_linear_curve(text: &str) -> bool {
    let values: Vec<f64> = text
        .split(';')
        .filter(|v| !v.trim().is_empty())
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    match values.split_first() {
        None => true,
        Some((kind, _)) if *kind == 0.0 => true,
        Some((kind, points)) if *kind == 1.0 || *kind == 4.0 => points
            .chunks(2)
            .all(|xy| xy.len() == 2 && (xy[0] - xy[1]).abs() < 1e-6),
        Some(_) => false,
    }
}

/// How far RawTherapee's AutoFill zooms a `w` by `h` frame turned by
/// `degrees` so no corner is empty: the largest rectangle of the
/// frame's shape inside it turned. The distortion and the lens's own
/// fill are left out, which makes the crop under them approximate.
fn fill_scale(w: f32, h: f32, degrees: f32) -> f32 {
    let (s, c) = degrees.abs().to_radians().sin_cos();
    (w / (w * c + h * s)).min(h / (w * s + h * c)).min(1.0)
}

/// RawTherapee's crop in pixels of a `pw` by `ph` plane, as fractions
/// of this one's: scaled about the center by `scales`, across and
/// down, since its crop is on the picture zoomed by that much and this
/// one's on the plane unzoomed, then held to the plane. Never less
/// than [`MIN_CROP`] across where the plane allows it, and never a
/// panic where it does not: a crop at the very edge is held to the
/// edge.
fn crop_fractions(
    (x, y, w, h): (f32, f32, f32, f32),
    (pw, ph): (f32, f32),
    scales: (f32, f32),
) -> Crop {
    let span = |at: f32, len: f32, of: f32, scale: f32| {
        let start = (0.5 + (at / of - 0.5) * scale).clamp(0.0, 1.0 - MIN_CROP);
        let end = (0.5 + ((at + len) / of - 0.5) * scale)
            .max(start + MIN_CROP)
            .min(1.0);
        (start, end - start)
    };
    let (x, w) = span(x, w, pw, scales.0);
    let (y, h) = span(y, h, ph, scales.1);
    Crop { x, y, w, h }
}

/// A crop ratio as RawTherapee names it: "3:2", "As Image", or one
/// with words after it ("1.414 - DIN EN ISO 216"), the first word
/// read. Free when none of that reads.
fn ratio(name: &str) -> Aspect {
    let name = name.trim();
    if name.eq_ignore_ascii_case("As Image") {
        return Aspect::Original;
    }
    name.split_whitespace()
        .next()
        .and_then(Aspect::parse_ratio)
        .unwrap_or(Aspect::Free)
}

/// The poly3 `k1` for RawTherapee's distortion `amount`. There the
/// source's radius is the output's times `1 - a + a r`, `r` in half
/// diagonals; here it is times `1 - k + k ρ²`, `ρ` in half shorter
/// sides. Both scale the center by their constant and hold their unit
/// radius, so `k` is the one whose corner stands to its center as
/// RawTherapee's does: `(1 - k + k / c²) / (1 - k) = 1 / (1 - a)`,
/// `c` the shorter side over the diagonal.
fn poly3_for(amount: f64, w: f64, h: f64) -> f64 {
    let a = amount.clamp(-0.5, 0.5);
    let c2 = (w.min(h) / w.hypot(h)).powi(2);
    let ratio = 1.0 / (1.0 - a) - 1.0;
    ratio / (ratio + 1.0 / c2)
}

/// What an import did to a sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Landed {
    /// The meta changed.
    pub meta: bool,
    /// A state was recorded.
    pub edit: bool,
}

/// Lay `imported` into `sidecar`: the meta exactly, and the edit over
/// the original's current one as one state of its history, named for
/// the import ([`history::import_label`]). Nothing is written; the
/// caller saves.
pub fn import_into(sidecar: &mut Sidecar, imported: &Imported) -> Landed {
    let meta = imported.rated.apply(&mut sidecar.meta);
    let edit = imported.has_edit()
        && sidecar.record_as(
            imported.applied(&sidecar.current),
            Some(history::import_label(SOURCE)),
        );
    Landed { meta, edit }
}

/// Whether a sidecar has a history of its own already: a state before
/// the current one on any of its lines. Such a frame was developed
/// here, and an import over it waits to be asked for.
pub fn has_history(sidecar: &Sidecar) -> bool {
    sidecar.lines().any(|line| !line.history.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A profile as RawTherapee 5.10 writes one, cut to what the tests
    /// read, with most tools present and switched off as they are in a
    /// real one.
    const SAMPLE: &str = "\
[Version]
AppVersion=5.10
Version=351

[General]
Rank=3
ColorLabel=1
InTrash=false

[Exposure]
Auto=false
Clip=0.02
Compensation=0.5
Brightness=0
Contrast=12
Saturation=-20
Black=150
HighlightCompr=0
ShadowCompr=50
HistogramMatching=false
CurveMode=Standard
CurveMode2=Standard
Curve=0;
Curve2=0;

[HLRecovery]
Enabled=false
Method=Blend

[Local Contrast]
Enabled=true
Radius=80
Amount=0.2

[Wavelet]
Enabled=false

[White Balance]
Enabled=true
Setting=Custom
Temperature=5200
Green=1.04
Equal=1
TemperatureBias=0

[Crop]
Enabled=true
X=600
Y=400
W=4800
H=3200
FixedRatio=true
Ratio=3:2
Orientation=As Image
Guide=Frame

[Coarse Transformation]
Rotate=0
HorizontalFlip=false
VerticalFlip=false

[Common Properties for Transformations]
Method=log
AutoFill=true

[Rotation]
Degree=0

[Distortion]
Amount=0

[LensProfile]
LcMode=lfauto
LCPFile=
UseDistortion=true
UseVignette=true
UseCA=false

[Perspective]
Method=simple
Horizontal=0
Vertical=0

[CACorrection]
Red=0
Blue=0

[Vignetting Correction]
Amount=0
Radius=50
Strength=1

[Color Management]
InputProfile=(cameraICC)

[RAW Bayer]
Method=amaze
";

    fn frame(w: u32, h: u32) -> Frame {
        Frame {
            size: Some((w, h)),
            turned: false,
        }
    }

    #[test]
    fn a_key_file_reads_with_its_comments_and_quotes() {
        let p = parse(
            "\u{feff}# a comment\n; another\n stray=before any section\n\
             [General]\r\nRank = 2 \r\n\n[Exposure]\nCurve=1;0;0;1;1;\nnoequals\n\
             [LensProfile]\nLCPFile=\"C:\\\\lenses\\\\a\\sb.lcp\"\n\
             [General]\nRank=4\n",
        )
        .unwrap();
        // The second [General] joins the first, its Rank the last one.
        assert_eq!(
            p.sections().collect::<Vec<_>>(),
            ["General", "Exposure", "LensProfile"]
        );
        assert_eq!(p.get("General", "Rank"), Some("4"));
        assert_eq!(p.get("Exposure", "Curve"), Some("1;0;0;1;1;"));
        assert_eq!(p.get("LensProfile", "LCPFile"), Some(r"C:\lenses\a b.lcp"));
        assert_eq!(p.get("Exposure", "noequals"), None);
        assert_eq!(p.get("Missing", "Rank"), None);
        assert!(parse("no sections here\nRank=3").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn the_meta_comes_across_exactly() {
        let i = read(SAMPLE, frame(6000, 4000)).unwrap();
        assert_eq!(
            i.rated,
            Rated {
                rating: Some(3),
                label: Some(Label::Red),
                trash: Some(false),
            }
        );
        // RawTherapee's labels, 0 to 5.
        for (n, label) in Label::ALL.iter().enumerate() {
            let i = read(&format!("[General]\nColorLabel={n}\n"), Frame::default()).unwrap();
            assert_eq!(i.rated.label, Some(*label), "{n}");
        }
        // The trash is a reject; out of it clears a reject and leaves
        // a pick; out of range is named, not held.
        let i = read(
            "[General]\nInTrash=true\nRank=7\nColorLabel=9\n",
            Frame::default(),
        )
        .unwrap();
        assert_eq!(i.rated.trash, Some(true));
        assert_eq!(i.rated.rating, None);
        assert_eq!(i.rated.label, None);
        assert_eq!(i.skipped, ["General Rank 7", "General ColorLabel 9"]);
        let mut meta = Meta {
            rating: 5,
            flag: Flag::Pick,
            ..Meta::default()
        };
        assert!(i.rated.apply(&mut meta));
        assert_eq!(meta.flag, Flag::Reject);
        assert_eq!(meta.rating, 5, "a rank out of range leaves the stars");
        let out = Rated {
            trash: Some(false),
            ..Rated::default()
        };
        assert!(out.apply(&mut meta));
        assert_eq!(meta.flag, Flag::None);
        meta.flag = Flag::Pick;
        assert!(!out.apply(&mut meta));
        assert_eq!(meta.flag, Flag::Pick);
        // A profile with no [General] carries none.
        assert!(
            read("[Exposure]\nCompensation=1\n", Frame::default())
                .unwrap()
                .rated
                .is_empty()
        );
    }

    #[test]
    fn the_sample_maps_and_names_what_it_cannot_carry() {
        let i = read(SAMPLE, frame(6000, 4000)).unwrap();
        let e = i.applied(&Edit::default());
        assert_eq!(e.light.exposure, 0.5);
        // 150 of 65535 is about 1.3% of mid grey to black.
        assert!(
            (e.light.tone.blacks + 0.0127).abs() < 1e-3,
            "{}",
            e.light.tone.blacks
        );
        assert!((e.color.saturation + 0.2).abs() < 1e-6);
        match e.white_balance {
            WhiteBalance::Custom { temperature, tint } => {
                // The daylight locus's white at 5200 K, and greener
                // there a magenta shift of it here.
                let (cct, duv) = rt_white(5200.0);
                assert_eq!(temperature, cct);
                assert!((temperature - 5200.0).abs() < 30.0, "{temperature}");
                let shift = tint - duv;
                assert!(shift < 0.0 && shift > -0.005, "{shift}");
            }
            _ => panic!("custom"),
        }
        assert!(e.lens.profile && e.lens.distortion && e.lens.vignetting);
        assert!(!e.lens.chromatic_aberration);
        let c = e.geometry.crop.expect("a crop");
        assert!((c.x - 0.1).abs() < 1e-6 && (c.y - 0.1).abs() < 1e-6);
        assert!((c.w - 0.8).abs() < 1e-6 && (c.h - 0.8).abs() < 1e-6);
        assert_eq!(e.geometry.aspect, Aspect::Ratio { w: 3.0, h: 2.0 });
        assert!(!e.geometry.portrait);
        // The base curve and everything not read stay as they were.
        assert_eq!(e.curves, crate::Curves::default());
        assert_eq!(e.light.tone.contrast, 1.0);
        assert_eq!(e.detail, crate::Detail::default());
        assert_eq!(
            i.skipped,
            ["Exposure Contrast", "Local Contrast"],
            "switched-off tools, plumbing and neutral sections are not named"
        );
        assert_eq!(i.exact, ["3 stars", "label red", "lens profile (lensfun)"]);
        assert_eq!(
            i.approximated,
            [
                "exposure +0.50 EV",
                "black point",
                "saturation",
                "white balance 5200 K, green 1.040",
                "crop"
            ]
        );
        assert_eq!(i.version_note(), None);
        assert_eq!(i.source(), "RawTherapee 5.10, profile version 351");
    }

    #[test]
    fn the_report_says_each_kind_on_a_line() {
        let i = read(SAMPLE, frame(6000, 4000)).unwrap();
        assert_eq!(
            i.report(),
            [
                (
                    "exact",
                    "3 stars, label red, lens profile (lensfun)".to_string()
                ),
                (
                    "approximate",
                    "exposure +0.50 EV, black point, saturation, \
                     white balance 5200 K, green 1.040, crop"
                        .to_string()
                ),
                ("no place", "Exposure Contrast, Local Contrast".to_string()),
            ]
        );
        // A version this reader does not know is read, and said.
        let newer = SAMPLE.replace("Version=351", "Version=360");
        let i = read(&newer, frame(6000, 4000)).unwrap();
        assert_eq!(i.rated.rating, Some(3));
        assert_eq!(
            i.report()[0],
            (
                "version",
                "profile version 360 is newer than this reader knows (353); read as that"
                    .to_string()
            )
        );
        let i = read("[General]\nRank=1\n", Frame::default()).unwrap();
        assert_eq!(
            i.report(),
            [
                (
                    "version",
                    "no profile version; read as a current one".to_string()
                ),
                ("exact", "1 star".to_string()),
            ]
        );
        assert_eq!(i.source(), "RawTherapee");
        assert!(!i.has_edit());
    }

    #[test]
    fn white_balance_as_shot_auto_and_off() {
        let wb = |body: &str| read(&format!("[White Balance]\n{body}"), Frame::default()).unwrap();
        let i = wb("Setting=Camera\nTemperature=6504\nGreen=1\n");
        assert_eq!(i.preset.sections, [Section::WhiteBalance]);
        assert_eq!(i.preset.edit.white_balance, WhiteBalance::AsShot);
        assert_eq!(i.exact, ["white balance as shot"]);
        let i = wb("Setting=autold\nTemperature=5000\n");
        assert!(i.preset.is_empty());
        assert_eq!(i.skipped, ["White Balance autold"]);
        let i = wb("Enabled=false\nSetting=Custom\nTemperature=5000\nEqual=1.2\n");
        assert!(i.preset.is_empty());
        assert_eq!(
            i.skipped,
            ["White Balance (switched off)", "White Balance Equal"]
        );
        // A named one is a kelvin there, and a green below one a
        // green white point, positive Duv.
        let i = wb("Setting=Daylight\nTemperature=1000\nGreen=0.9\n");
        match i.preset.edit.white_balance {
            WhiteBalance::Custom { temperature, tint } => {
                assert_eq!(temperature, 2000.0, "held to the slider's range");
                assert!((tint - 0.05 * (1.0 / 0.9 - 1.0)).abs() < 1e-9, "{tint}");
            }
            _ => panic!("custom"),
        }
    }

    #[test]
    fn the_turns_and_mirrors_land_the_way_up_rawtherapee_shows() {
        let g = |body: &str| {
            read(
                &format!("[Coarse Transformation]\n{body}"),
                Frame::default(),
            )
            .unwrap()
            .geometry
            .expect("a geometry")
        };
        // Clockwise there, counter-clockwise turns here.
        assert_eq!(g("Rotate=90\n").turns, 3);
        assert_eq!(g("Rotate=270\n").turns, 1);
        assert!(!g("Rotate=90\n").flip);
        // A mirror alone is the source mirrored.
        let h = g("HorizontalFlip=true\n");
        assert_eq!((h.turns, h.flip), (0, true));
        let v = g("VerticalFlip=true\n");
        assert_eq!((v.turns, v.flip), (2, true));
        // Turned clockwise and then mirrored left to right: a point
        // at the source's top left lands where RawTherapee puts it.
        // Turned clockwise a quarter, the top left goes to the top
        // right; mirrored, back to the top left: the transpose.
        let t = g("Rotate=90\nHorizontalFlip=true\n");
        let (w, h) = (300.0, 200.0);
        let at = |px: (f32, f32)| t.to_source(px, w, h);
        let (sx, sy) = at((0.5, 0.5));
        assert!(sx < 1.0 && sy < 1.0, "({sx}, {sy})");
        // The plane's bottom left is the source's top right.
        let (sx, sy) = at((0.5, 299.5));
        assert!((sx - 299.5).abs() < 1.0 && sy < 1.0, "({sx}, {sy})");
        // Both mirrors are a half turn.
        let both = g("HorizontalFlip=true\nVerticalFlip=true\n");
        assert_eq!((both.turns, both.flip), (2, false));
        let i = read(
            "[Coarse Transformation]\nRotate=90\nHorizontalFlip=true\n[Rotation]\nDegree=-1.5\n",
            Frame::default(),
        )
        .unwrap();
        assert_eq!(i.geometry.as_ref().unwrap().angle, -1.5);
        assert_eq!(
            i.exact,
            [
                "turned 90° clockwise",
                "mirrored left to right",
                "straightened -1.50°"
            ]
        );
    }

    #[test]
    fn the_crop_is_held_on_the_plane_and_zoomed_under_an_angle() {
        let body = "[Crop]\nEnabled=true\nX=0\nY=0\nW=6000\nH=4000\nFixedRatio=false\n";
        let i = read(body, frame(6000, 4000)).unwrap();
        let c = i.geometry.unwrap().crop.unwrap();
        assert_eq!((c.x, c.y, c.w, c.h), (0.0, 0.0, 1.0, 1.0));
        // RawTherapee's decode is a few pixels larger than this one's:
        // held to the plane.
        let body2 = "[Crop]\nEnabled=true\nX=10\nY=10\nW=6040\nH=4030\n";
        let c = read(body2, frame(6000, 4000))
            .unwrap()
            .geometry
            .unwrap()
            .crop
            .unwrap();
        assert!(c.x + c.w <= 1.0 + 1e-6 && c.y + c.h <= 1.0 + 1e-6, "{c:?}");
        // Turned a quarter, the plane is the frame on end.
        let turned = format!("{body}[Coarse Transformation]\nRotate=90\n")
            .replace("W=6000\nH=4000", "W=4000\nH=6000");
        let g = read(&turned, frame(6000, 4000)).unwrap().geometry.unwrap();
        assert_eq!(g.crop.unwrap().w, 1.0);
        assert!(g.portrait);
        // Under an angle the whole frame is RawTherapee's fill: the
        // largest of its shape inside the frame turned, which fits.
        let angled = format!("{body}[Rotation]\nDegree=5\n");
        let g = read(&angled, frame(6000, 4000)).unwrap().geometry.unwrap();
        let c = g.crop.unwrap();
        assert!(c.w < 1.0 && (c.x + c.w / 2.0 - 0.5).abs() < 1e-6, "{c:?}");
        assert!(g.fits(&c, 6000.0, 4000.0), "{c:?}");
        let fit = g.largest_fit((0.5, 0.5), 1.5, 6000.0, 4000.0);
        assert!((fit.w - c.w).abs() < 1e-3, "{fit:?} {c:?}");
        // Without AutoFill nothing is zoomed.
        let unfilled = format!("{angled}[Common Properties for Transformations]\nAutoFill=false\n");
        let c = read(&unfilled, frame(6000, 4000))
            .unwrap()
            .geometry
            .unwrap()
            .crop
            .unwrap();
        assert_eq!(c.w, 1.0);
        // A ratio with words after it, and the image's own.
        assert_eq!(
            ratio("1.414 - DIN EN ISO 216"),
            Aspect::Ratio { w: 1.414, h: 1.0 }
        );
        assert_eq!(ratio("As Image"), Aspect::Original);
        assert_eq!(ratio("Current"), Aspect::Free);
    }

    #[test]
    fn the_geometry_waits_on_the_frame() {
        let body = "[Crop]\nEnabled=true\nX=0\nY=0\nW=600\nH=400\n[Rotation]\nDegree=2\n";
        // No size: the crop is named, the angle still comes.
        let i = read(body, Frame::default()).unwrap();
        assert_eq!(i.skipped, ["Crop (the frame's size is not known)"]);
        let g = i.geometry.unwrap();
        assert_eq!((g.angle, g.crop), (2.0, None));
        // A frame turned here: none of it.
        let i = read(
            body,
            Frame {
                size: Some((600, 400)),
                turned: true,
            },
        )
        .unwrap();
        assert_eq!(i.geometry, None);
        assert_eq!(
            i.skipped,
            [
                "Rotation (the frame is turned here)",
                "Crop (the frame is turned here)"
            ]
        );
        // A crop switched off is no crop.
        let off = read(
            "[Crop]\nEnabled=false\nX=10\nY=10\nW=100\nH=100\n",
            frame(600, 400),
        )
        .unwrap();
        assert_eq!(off.geometry, None);
    }

    #[test]
    fn the_lens_settings_land_where_this_engine_has_them() {
        let i = read(
            "[LensProfile]\nLcMode=lcp\nLCPFile=lens.lcp\nUseDistortion=false\nUseVignette=true\nUseCA=true\n\
             [Distortion]\nAmount=-0.1\n[CACorrection]\nRed=0.002\nBlue=-0.001\n\
             [Vignetting Correction]\nAmount=30\n",
            frame(6000, 4000),
        )
        .unwrap();
        let e = i.applied(&Edit::default());
        assert!(e.lens.profile && !e.lens.distortion && e.lens.vignetting);
        assert!(e.lens.chromatic_aberration);
        assert_eq!((e.lens.ca_red, e.lens.ca_blue), (0.002, -0.001));
        // Barrel there is barrel here, a fraction of it for the
        // longer unit radius.
        assert!(
            e.lens.manual < 0.0 && e.lens.manual > -0.1,
            "{}",
            e.lens.manual
        );
        assert_eq!(
            i.approximated,
            ["lens profile (lensfun for the LCP)", "manual distortion"]
        );
        assert_eq!(i.exact, ["chromatic aberration by hand"]);
        assert_eq!(i.skipped, ["Vignetting Correction"]);
        // No profile there leaves this engine's own lookup alone.
        let i = read("[LensProfile]\nLcMode=none\n", Frame::default()).unwrap();
        assert!(i.preset.is_empty());
        let i = read("[LensProfile]\nLcMode=later\n", Frame::default()).unwrap();
        assert_eq!(i.skipped, ["LensProfile LcMode later"]);
    }

    #[test]
    fn the_distortion_fit_agrees_at_the_center_and_the_corner() {
        let (w, h) = (6000.0f64, 4000.0f64);
        let a = 0.2;
        let k = poly3_for(a, w, h);
        let c = w.min(h) / w.hypot(h);
        let rt = |r: f64| 1.0 - a + a * r;
        let here = |rho: f64| 1.0 - k + k * rho * rho;
        let corner = 1.0 / c;
        assert!(
            ((here(corner) / here(0.0)) - (rt(1.0) / rt(0.0))).abs() < 1e-9,
            "{k}"
        );
        assert_eq!(poly3_for(0.0, w, h), 0.0);
    }

    #[test]
    fn a_tone_curve_is_named_not_carried() {
        assert!(is_linear_curve("0;"));
        assert!(is_linear_curve(""));
        assert!(is_linear_curve("1;0;0;0.5;0.5;1;1;"));
        assert!(!is_linear_curve("1;0;0;0.5;0.6;1;1;"));
        assert!(!is_linear_curve("2;0.25;0.5;0.75;10;0;0;0;"));
        let i = read(
            "[Exposure]\nCurve=1;0;0;0.4;0.5;1;1;\nCurve2=0;\nHistogramMatching=true\n",
            Frame::default(),
        )
        .unwrap();
        assert!(i.preset.is_empty());
        assert_eq!(i.skipped, ["Exposure Curve"]);
        // A lift is not this engine's black point.
        let i = read("[Exposure]\nBlack=-200\n", Frame::default()).unwrap();
        assert_eq!(i.skipped, ["Exposure Black (a lift)"]);
    }

    #[test]
    fn sections_with_no_switch_are_named_when_they_say_something() {
        let i = read(
            "[Perspective]\nMethod=simple\nHorizontal=0\nVertical=12\n\
             [Some Later Tool]\nStrength=3\n[Color Management]\nInputProfile=x\n\
             [Retinex]\nEnabled=true\n[Wavelet]\nEnabled=false\n",
            Frame::default(),
        )
        .unwrap();
        assert_eq!(i.skipped, ["Perspective", "Some Later Tool", "Retinex"]);
    }

    /// A fresh folder a test can write into, its own by name and pid.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-edit-pp3-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_import_is_the_meta_and_one_named_state_through_the_sidecar() {
        let dir = scratch("import");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, b"").unwrap();
        let i = read(SAMPLE, frame(6000, 4000)).unwrap();
        let mut sidecar = Sidecar::default();
        assert!(!has_history(&sidecar));
        let landed = import_into(&mut sidecar, &i);
        assert_eq!(
            landed,
            Landed {
                meta: true,
                edit: true
            }
        );
        sidecar.save(&raw).unwrap();
        let back = Sidecar::load(&raw).unwrap().expect("a sidecar");
        assert_eq!(back.meta.rating, 3);
        assert_eq!(back.meta.label, Label::Red);
        assert_eq!(back.meta.flag, Flag::None);
        assert_eq!(back.history.len(), 1, "one state before the import's");
        assert_eq!(back.history[0].edit, Edit::default());
        assert_eq!(
            back.current_label.as_deref(),
            Some("Imported from RawTherapee")
        );
        assert_eq!(back.current.light.exposure, 0.5);
        assert!(has_history(&back));
        // Again: nothing more to record, and the meta is as it was.
        let mut again = back.clone();
        assert_eq!(import_into(&mut again, &i), Landed::default());
        assert_eq!(again.history.len(), 1);
        remove_dir_retry(&dir);
    }

    /// A test's folder removed, tried again for a while when the
    /// system refuses, as the window's tests do: on Windows a file let
    /// go of a moment ago can still be held.
    fn remove_dir_retry(dir: &std::path::Path) {
        let start = std::time::Instant::now();
        loop {
            match std::fs::remove_dir_all(dir) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) if start.elapsed() < std::time::Duration::from_secs(20) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => panic!("{} could not be removed: {e}", dir.display()),
            }
        }
    }

    #[test]
    fn a_crop_at_the_very_edge_is_held_to_it() {
        // Held to 0.98 of the way across, 1 - 0.98 is a hair under
        // the smallest crop in f32: no panic, and on the plane.
        for body in [
            "[Crop]\nEnabled=true\nX=590\nY=0\nW=100\nH=400\n",
            "[Crop]\nEnabled=true\nX=600\nY=400\nW=1\nH=1\n",
            "[Crop]\nEnabled=true\nX=-50\nY=-50\nW=5000\nH=5000\n",
        ] {
            let c = read(body, frame(600, 400))
                .unwrap()
                .geometry
                .unwrap()
                .crop
                .unwrap();
            assert!(c.x >= 0.0 && c.y >= 0.0, "{body}: {c:?}");
            assert!(
                c.x + c.w <= 1.0 + 1e-6 && c.y + c.h <= 1.0 + 1e-6,
                "{body}: {c:?}"
            );
            assert!(c.w > 0.0 && c.h > 0.0, "{body}: {c:?}");
        }
    }

    #[test]
    fn a_number_that_is_not_finite_is_no_number() {
        for v in ["nan", "NaN", "inf", "-inf", "infinity"] {
            let i = read(
                &format!(
                    "[Exposure]\nCompensation={v}\nBlack={v}\nSaturation={v}\n\
                     [Rotation]\nDegree={v}\n[Distortion]\nAmount={v}\n"
                ),
                Frame::default(),
            )
            .unwrap();
            assert!(i.preset.is_empty(), "{v}");
            assert_eq!(i.geometry, None, "{v}");
            // And what is carried writes a sidecar that reads back.
            let edit = i.applied(&Edit::default());
            assert_eq!(Edit::from_json(&edit.to_json()).unwrap(), edit);
        }
    }

    #[test]
    fn the_image_s_own_ratio_keeps_a_tall_frame_tall() {
        // RawTherapee's default crop: the ratio fixed, as the image.
        let crop = |w: u32, h: u32| {
            format!(
                "[Crop]\nEnabled=true\nX=0\nY=0\nW={w}\nH={h}\n\
                 FixedRatio=true\nRatio=As Image\nOrientation=As Image\n"
            )
        };
        let tall = read(&crop(4000, 6000), frame(4000, 6000))
            .unwrap()
            .geometry
            .unwrap();
        assert_eq!(tall.aspect, Aspect::Original);
        assert!(!tall.portrait, "a tall crop of a tall frame is not turned");
        let ratio = tall.aspect.ratio(4000.0, 6000.0, tall.portrait).unwrap();
        assert!((ratio - 4000.0 / 6000.0).abs() < 1e-6, "{ratio}");
        // A wide crop of the image's ratio on a tall frame is turned.
        let wide = read(&crop(4000, 2667), frame(4000, 6000))
            .unwrap()
            .geometry
            .unwrap();
        assert!(wide.portrait);
        // A named ratio's way up is the crop's own.
        let named = read(
            &crop(4000, 6000).replace("Ratio=As Image", "Ratio=2:3"),
            frame(4000, 6000),
        )
        .unwrap()
        .geometry
        .unwrap();
        assert_eq!(named.aspect, Aspect::Ratio { w: 2.0, h: 3.0 });
        assert!(named.portrait);
    }

    #[test]
    fn the_scales_zoom_the_crop_and_a_stretch_is_named() {
        let common = "[Common Properties for Transformations]\nAutoFill=false\nScale=1.25\n\
                      Scale horizontally=1\nScale vertically=1\n";
        let whole = "[Crop]\nEnabled=true\nX=0\nY=0\nW=6000\nH=4000\n";
        let i = read(&format!("{common}{whole}"), frame(6000, 4000)).unwrap();
        let c = i.geometry.unwrap().crop.unwrap();
        assert!(
            (c.w - 0.8).abs() < 1e-6 && (c.x - 0.1).abs() < 1e-6,
            "{c:?}"
        );
        assert!(i.skipped.is_empty(), "{:?}", i.skipped);
        // With no crop to carry it, the zoom is named; a stretch is
        // named always.
        let i = read(
            &common.replace("Scale horizontally=1", "Scale horizontally=1.1"),
            frame(6000, 4000),
        )
        .unwrap();
        assert_eq!(
            i.skipped,
            [
                "Common Properties for Transformations Scale",
                "Common Properties for Transformations Scale horizontally"
            ]
        );
        // AutoFill on, the scales do nothing there.
        let i = read(
            &format!(
                "{}{whole}",
                common.replace("AutoFill=false", "AutoFill=true")
            ),
            frame(6000, 4000),
        )
        .unwrap();
        assert_eq!(i.geometry.unwrap().crop.unwrap().w, 1.0);
        assert!(i.skipped.is_empty());
    }

    #[test]
    fn a_scale_of_nothing_is_held_to_the_slider_s_range() {
        for scale in ["1e-300", "1e-45", "1e-39", "0", "-2", "1e300"] {
            let body = format!(
                "[Common Properties for Transformations]\nAutoFill=false\nScale={scale}\n\
                 Scale horizontally={scale}\nScale vertically=1\n\
                 [Crop]\nEnabled=true\nX=300\nY=200\nW=1\nH=1\n"
            );
            let i = read(&body, frame(600, 400)).unwrap();
            let c = i.geometry.as_ref().unwrap().crop.unwrap();
            for v in [c.x, c.y, c.w, c.h] {
                assert!(v.is_finite() && (0.0..=1.0).contains(&v), "{scale}: {c:?}");
            }
            let edit = i.applied(&Edit::default());
            assert_eq!(
                Edit::from_json(&edit.to_json()).unwrap(),
                edit,
                "{scale}: the sidecar reads back"
            );
        }
    }

    #[test]
    fn a_forced_geometry_keeps_what_a_profile_cannot_say() {
        let mut developed = Edit::default();
        developed.geometry.vertical = 6.0;
        developed.geometry.horizontal = -2.0;
        developed.geometry.crop = Some(Crop {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        });
        developed.geometry.aspect = Aspect::Ratio { w: 1.0, h: 1.0 };
        let body = "[Coarse Transformation]\nRotate=0\n[Rotation]\nDegree=1\n\
                    [Crop]\nEnabled=true\nX=0\nY=0\nW=300\nH=200\n";
        // The keystone has no counterpart in a profile: it stays.
        let i = read_carrying(body, frame(600, 400), Carry::Every).unwrap();
        assert!(!i.keeps_crop);
        let g = i.applied(&developed).geometry;
        assert_eq!((g.vertical, g.horizontal), (6.0, -2.0));
        assert_eq!(g.angle, 1.0);
        assert_ne!(g.crop, developed.geometry.crop, "the profile's crop placed");
        // With the frame's size unknown the profile's crop cannot be
        // placed, and the frame's own is kept rather than cleared.
        let i = read_carrying(body, Frame::default(), Carry::Every).unwrap();
        assert!(i.keeps_crop);
        assert_eq!(i.skipped, ["Crop (the frame's size is not known)"]);
        let g = i.applied(&developed).geometry;
        assert_eq!(g.crop, developed.geometry.crop);
        assert_eq!(g.aspect, developed.geometry.aspect);
        assert_eq!((g.angle, g.vertical), (1.0, 6.0));
    }

    #[test]
    fn rawtherapee_s_daylight_sits_above_the_black_body() {
        let (cct, duv) = rt_white(6504.0);
        assert!((cct - 6504.0).abs() < 15.0, "{cct}");
        assert!((0.0025..0.004).contains(&duv), "{duv}");
        let (cct, duv) = rt_white(5000.0);
        assert!((cct - 5000.0).abs() < 15.0, "{cct}");
        assert!((0.002..0.004).contains(&duv), "{duv}");
        // Up to 4000 K it is the black body's own.
        assert_eq!(rt_white(3200.0), (3200.0, 0.0));
    }

    #[test]
    fn an_lcp_with_no_file_is_no_profile() {
        let i = read("[LensProfile]\nLcMode=lcp\nLCPFile=\n", Frame::default()).unwrap();
        assert!(i.preset.is_empty());
        assert!(i.skipped.is_empty() && i.approximated.is_empty());
    }

    #[test]
    fn the_raw_s_own_sections_are_never_named() {
        let i = read(
            "[RAW]\nCA=true\n[RAW Bayer]\nMethod=amaze\n[RAW Preprocess WB]\nMode=1\n\
             [Common Properties for Transformations]\nMethod=log\nAutoFill=true\n",
            Frame::default(),
        )
        .unwrap();
        assert!(i.skipped.is_empty(), "{:?}", i.skipped);
    }

    #[test]
    fn forced_every_setting_a_read_section_speaks_to_comes_across() {
        let profile = "[Exposure]\nCompensation=0\nBlack=0\nSaturation=0\nContrast=0\n\
                       [Coarse Transformation]\nRotate=0\n[Rotation]\nDegree=0\n\
                       [Crop]\nEnabled=false\n[LensProfile]\nLcMode=none\n\
                       [Distortion]\nAmount=0\n[CACorrection]\nRed=0\nBlue=0\n";
        // Carrying what is set, a profile at its neutral carries nothing.
        let set = read(profile, frame(6000, 4000)).unwrap();
        assert!(!set.has_edit());
        assert!(set.neutral.is_empty());
        // Forced over a frame developed here, it takes it back.
        let mut developed = Edit::default();
        developed.light.exposure = 0.3;
        developed.light.tone.blacks = -0.1;
        developed.color.saturation = 0.4;
        developed.geometry.angle = 3.0;
        developed.lens.manual = -0.05;
        developed.lens.ca_red = 0.001;
        developed.light.tone.contrast = 1.3;
        let every = read_carrying(profile, frame(6000, 4000), Carry::Every).unwrap();
        let e = every.applied(&developed);
        assert_eq!(e.light.exposure, 0.0);
        assert_eq!(e.light.tone.blacks, 0.0);
        assert_eq!(e.color.saturation, 0.0);
        assert_eq!(e.geometry, crate::Geometry::default());
        assert!(!e.lens.profile);
        assert_eq!((e.lens.manual, e.lens.ca_red), (0.0, 0.0));
        // What it does not read is not touched.
        assert_eq!(e.light.tone.contrast, 1.3);
        assert_eq!(
            every.report(),
            [
                (
                    "version",
                    "no profile version; read as a current one".to_string()
                ),
                (
                    "neutral",
                    "exposure 0, no black point, saturation 0, no crop or turn, \
                     no lens profile, no manual distortion, no chromatic aberration by hand"
                        .to_string()
                ),
            ]
        );
    }
}
