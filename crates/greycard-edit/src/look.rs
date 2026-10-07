//! The look an edit finishes through: a 3D LUT from the user's look
//! directory, at a strength.
//!
//! A look is the output end of what notes §78 calls the two ends of the
//! pipeline — the camera profile being the input end ([`crate::camera`],
//! which this module is built in the shape of). It goes on after the
//! tone curve and the point curves, in the table's own encoding, and
//! before the output transform.
//!
//! The choice lives in the edit because it changes the picture and a
//! preset should carry it: a film preset is exactly a look with a
//! strength. The engine takes the resolved table, not a name: reading
//! the file is this crate's job, and it is done once per file and kept,
//! so the viewport's finish and the export's are handed the same table.
//!
//! **On the name.** [`crate::Look`] is already the part of an edit a
//! mask can carry — light, curves, mixer, color, grading, tint. This is
//! the other thing the word means, a LUT, and to keep the two apart the
//! type here is [`LookLut`] and the field on [`crate::Edit`] is
//! `look_lut`. The sidecar's key is `look`, which is what the roadmap
//! and the panel call it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use greycard_core::lut::{self, Encoding, Kind, Lut3d, Primaries};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::DisplayCurve;

/// The word for no look at all, in the sidecar and on the panel.
pub const NONE: &str = "none";

/// Which look table the picture finishes through.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LutChoice {
    /// The picture as the pipeline leaves it.
    #[default]
    None,
    /// A table in the look directory, by file name without the
    /// extension.
    Named(String),
}

impl LutChoice {
    /// The name as the sidecar writes it.
    pub fn name(&self) -> &str {
        match self {
            LutChoice::None => NONE,
            LutChoice::Named(name) => name,
        }
    }

    /// A name from the panel or a sidecar. Anything empty, or the word
    /// for no look, is [`LutChoice::None`].
    pub fn from_name(name: &str) -> Self {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case(NONE) {
            LutChoice::None
        } else {
            LutChoice::Named(name.to_string())
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, LutChoice::None)
    }
}

impl Serialize for LutChoice {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for LutChoice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(LutChoice::from_name(&String::deserialize(d)?))
    }
}

/// The LOOK section of an edit: a table by name and how much of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookLut {
    pub lut: LutChoice,
    /// 0 for none of the table, 1 for the whole of it.
    pub strength: f32,
}

impl Default for LookLut {
    fn default() -> Self {
        LookLut {
            lut: LutChoice::None,
            // A look chosen is a look wanted; the slider is there to
            // take it back, not to have to be found first.
            strength: 1.0,
        }
    }
}

impl LookLut {
    /// Whether this would change any pixel.
    pub fn is_off(&self) -> bool {
        self.lut.is_none() || self.strength <= 0.0
    }

    /// Whether this names the look `name`, exactly: `Neon` is not
    /// `Neon Film`, `neon` or `Neon.agx`.
    pub fn names(&self, name: &str) -> bool {
        matches!(&self.lut, LutChoice::Named(n) if n == name)
    }

    /// Name the look `to` where this names `from`; true when it did.
    /// The strength stays.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        if !self.names(from) {
            return false;
        }
        self.lut = LutChoice::Named(to.to_string());
        true
    }

    /// The engine's look for this choice: the table read and kept with
    /// its matrices solved, or `None` when no table is named. A table
    /// that will not read warns once and comes back `None`, which is
    /// no look.
    ///
    /// A named table at a strength of zero still comes back, carrying
    /// that strength: `None` means "there is no table here", which is
    /// what tells a consumer it may let go of one. Whether the look
    /// does anything is [`lut::Look::is_off`].
    pub fn look(&self) -> Option<lut::Look> {
        let LutChoice::Named(name) = &self.lut else {
            return None;
        };
        let path = path_for(name)?;
        let table = load(&path)?;
        match lut::Look::new(table, self.strength) {
            Ok(look) => Some(look),
            Err(e) => {
                log::warn!("look {}: {e}", path.display());
                None
            }
        }
    }

    /// The look a picture under `curve` finishes through: the look's
    /// table for that curve. That is its variant for the curve
    /// ([`variant_stem`], `<name>.agx.cube`) when there is one that
    /// declares the curve, else its own file ([`look`]) through
    /// [`gate`], so a table fitted under another display curve is no
    /// look. This is what the viewport and the export both resolve, so
    /// neither can apply a table the other skips.
    ///
    /// [`look`]: LookLut::look
    pub fn look_under(&self, curve: DisplayCurve) -> Option<lut::Look> {
        self.look_under_in(&store_dir()?, curve)
    }

    /// [`LookLut::look_under`] in the look directory `dir`.
    pub fn look_under_in(&self, dir: &Path, curve: DisplayCurve) -> Option<lut::Look> {
        let LutChoice::Named(name) = &self.lut else {
            return None;
        };
        if !is_a_name(name) {
            return None;
        }
        let with = |path: &Path, table: Arc<Lut3d>| match lut::Look::new(table, self.strength) {
            Ok(look) => Some(look),
            Err(e) => {
                log::warn!("look {}: {e}", path.display());
                None
            }
        };
        let variant = variant_in(dir, name, curve);
        if variant.is_file()
            && let Some(table) = load(&variant)
            && MadeFor::read(&table.comments) == MadeFor::Curve(curve)
        {
            return with(&variant, table);
        }
        let own = path_in(dir, name);
        gate(load(&own).and_then(|t| with(&own, t)), curve)
    }

    /// Whether the look directory has any table of this look's name,
    /// its own file or a variant for a curve: what tells a look that
    /// is missing from one that is off for a picture's curve.
    ///
    /// A variant counts only when its header declares its curve, the
    /// test the listing ([`entry_of`]) and [`LookLut::look_under`]
    /// apply: `Ghost.agx.cube` declaring per channel is not a table of
    /// "Ghost", so a picture naming "Ghost" with no `Ghost.cube` is a
    /// look that is not there.
    pub fn is_there(&self) -> bool {
        store_dir().is_some_and(|dir| self.is_there_in(&dir))
    }

    /// [`LookLut::is_there`] in the look directory `dir`.
    pub fn is_there_in(&self, dir: &Path) -> bool {
        let LutChoice::Named(name) = &self.lut else {
            return false;
        };
        if !is_a_name(name) {
            return false;
        }
        path_in(dir, name).is_file()
            || DisplayCurve::ALL.into_iter().any(|c| {
                let path = variant_in(dir, name, c);
                path.is_file()
                    && Lut3d::info(&path)
                        .is_ok_and(|i| MadeFor::read(&i.comments) == MadeFor::Curve(c))
            })
    }
}

/// The file stem a look's table for `curve` is kept under: the look's
/// own name for per channel, which is where every table fitted before
/// the curve was declared already sits and what a sidecar names, and
/// `<name>.<curve>` for another curve (`Canon EOS R6 Mark II
/// Faithful.agx`), beside it. One look name, one table per curve.
pub fn variant_stem(name: &str, curve: DisplayCurve) -> String {
    match curve {
        DisplayCurve::Channels => name.to_string(),
        other => format!("{name}.{}", other.key()),
    }
}

/// Where a look's variant for `curve` would be in `dir`:
/// `<name>.<curve>.cube`, whichever curve, so a per-channel variant
/// written by hand is read too.
fn variant_in(dir: &Path, name: &str, curve: DisplayCurve) -> PathBuf {
    dir.join(format!("{name}.{}.cube", curve.key()))
}

/// The comment line every table the camera match writes carries
/// (`greycard_match::cube::FITTED`, which this crate does not depend
/// on; the editor's tests hold the two to the same text). It is what
/// tells a fitted table from a film preset or anyone else's `.cube`.
pub const FITTED: &str = "fitted from the camera's embedded JPEG by greycard-match";

/// The keys a fitted table declares in its header as `# key: value`
/// lines, read with [`lut::declared`].
pub mod key {
    /// The display curve the frames were developed under, as the
    /// sidecar names it ([`crate::DisplayCurve::key`]).
    pub const DISPLAY_CURVE: &str = "display_curve";
    /// The body the table is for: make and model as the raw names
    /// them. For a borrowed table, the body it was borrowed for.
    pub const MAKE: &str = "make";
    pub const MODEL: &str = "model";
    /// The picture style's group key, "Canon Faithful".
    pub const STYLE: &str = "style";
    /// The body whose frames the table was fitted on, as the panel
    /// names it; another body's for a borrowed table.
    pub const FITTED_ON: &str = "fitted_on";
    /// How many frames the fit used.
    pub const FRAMES: &str = "frames";
}

/// The display transform a table may be applied under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MadeFor {
    /// Any: a table that was not fitted by the camera match and
    /// declares no display curve, which is a film preset or someone
    /// else's `.cube`. It is a preference laid over whatever the
    /// picture is, and stays on under every transform.
    Any,
    /// The display curve the table was fitted under, or declares it
    /// was made for. It corrects that curve's rendering toward the
    /// camera's, so under another it is off.
    Curve(DisplayCurve),
    /// A display curve this build has not got, by the word the table
    /// gives: off under every one this build has.
    Unknown(String),
}

impl MadeFor {
    /// What a table's comments say it was made for: the curve it
    /// declares when it declares one, whoever wrote it; per channel
    /// for a table the match fitted before tables declared it, since
    /// every one of those was fitted under per channel; any for the
    /// rest.
    pub fn read(comments: &[String]) -> Self {
        match lut::declared(comments, key::DISPLAY_CURVE) {
            Some(word) => match DisplayCurve::from_key(word) {
                Some(curve) => MadeFor::Curve(curve),
                None => MadeFor::Unknown(word.to_string()),
            },
            None if is_fitted(comments) => MadeFor::Curve(DisplayCurve::Channels),
            None => MadeFor::Any,
        }
    }

    /// Whether a table made for this applies to a picture under
    /// `curve`.
    pub fn applies(&self, curve: DisplayCurve) -> bool {
        match self {
            MadeFor::Any => true,
            MadeFor::Curve(c) => *c == curve,
            MadeFor::Unknown(_) => false,
        }
    }

    /// The transform's name in a sentence, or none for any.
    pub fn phrase(&self) -> Option<String> {
        match self {
            MadeFor::Any => None,
            MadeFor::Curve(c) => Some(c.phrase().to_string()),
            MadeFor::Unknown(word) => Some(word.clone()),
        }
    }
}

/// Whether a table's comments carry the camera match's line.
pub fn is_fitted(comments: &[String]) -> bool {
    comments.iter().any(|c| c == FITTED)
}

/// A resolved look kept only when its table applies under `curve`:
/// the gate between a fitted table and a picture on another display
/// transform. The one place the decision is made, before either the
/// CPU finish or the shader is handed a table, which is what keeps
/// the two in agreement.
pub fn gate(look: Option<lut::Look>, curve: DisplayCurve) -> Option<lut::Look> {
    look.filter(|l| MadeFor::read(&l.lut.comments).applies(curve))
}

/// The declarations a fitted table's header carries, in the order they
/// are written: the display curve the run developed under, the body
/// the table is for (make and model as the raw names them), its style,
/// the body whose frames fitted it, and how many.
pub fn fitted_declarations(
    curve: DisplayCurve,
    make: &str,
    model: &str,
    style: &str,
    fitted_on: &str,
    frames: usize,
) -> Vec<(&'static str, String)> {
    let mut out = vec![(key::DISPLAY_CURVE, curve.key())];
    for (k, v) in [
        (key::MAKE, make),
        (key::MODEL, model),
        (key::STYLE, style),
        (key::FITTED_ON, fitted_on),
    ] {
        if !v.trim().is_empty() {
            out.push((k, v.trim().to_string()));
        }
    }
    out.push((key::FRAMES, frames.to_string()));
    out
}

/// Where the look tables live: `$XDG_DATA_HOME/greycard/looks` when
/// that is set, else `greycard/looks` under the platform's data
/// directory — `~/.local/share` on Linux, `~/Library/Application
/// Support` on macOS, `%APPDATA%` on Windows (notes §106). The same
/// logic the profile directory uses, one name along.
pub fn store_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::data_dir)?;
    Some(base.join("greycard").join("looks"))
}

/// Where a table of this name is kept. A name that is a path, or tries
/// to climb out of the directory, is refused.
///
/// Two extensions are read, so the one that is there wins; with
/// neither there the `.cube` path comes back, to be named in the
/// warning that follows.
pub fn path_for(name: &str) -> Option<PathBuf> {
    if !is_a_name(name) {
        log::warn!("look {name:?} is not a name in the look directory");
        return None;
    }
    Some(path_in(&store_dir()?, name))
}

/// Whether a name is a name in the directory, not a path.
fn is_a_name(name: &str) -> bool {
    !(name.is_empty()
        || name.contains(['/', '\\'])
        || name.contains("..")
        || Path::new(name).is_absolute())
}

/// The longest a look's name may be, in bytes: a file name is at most
/// 255 on every system the editor runs on, and the longest file a look
/// has, with the name a rename's copy is made under before it is put in
/// place, is `<name>.channels.cube.part`.
pub const NAME_BYTES: usize = 255 - ".channels.cube.part".len();

/// Why `name` cannot be a look's name, in the rename sheet's words, or
/// none when it can. The name is a file's on Windows, macOS and Linux
/// alike, since a look folder is copied between them: no character any
/// of them refuses, no name Windows keeps for a device, no ending in a
/// dot. Nothing [`path_for`] would refuse, and nothing the list would
/// read as something else: the word for no look, or a name ending in a
/// display curve's word (`Neon.agx` would be the AgX table of a look
/// called Neon). `name` is taken as given; the sheet trims it first.
pub fn name_refusal(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("A look needs a name.".into());
    }
    if name.eq_ignore_ascii_case(NONE) {
        return Some(format!("\"{NONE}\" is the word for no look."));
    }
    if let Some(c) = name.chars().find(|c| c.is_control()) {
        return Some(format!(
            "A look's name cannot hold a control character ({:?}).",
            c
        ));
    }
    if let Some(c) = name.chars().find(|c| is_format(*c)) {
        return Some(format!(
            "A look's name cannot hold an invisible character (U+{:04X}).",
            u32::from(c)
        ));
    }
    if let Some(c) = name
        .chars()
        .find(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        let systems = match c {
            '/' => "any system",
            ':' => "Windows or macOS",
            _ => "Windows",
        };
        return Some(format!("A file name cannot hold \"{c}\" on {systems}."));
    }
    if name.starts_with('.') {
        return Some("A name that starts with a dot is a hidden file on macOS and Linux.".into());
    }
    if name.ends_with('.') {
        return Some("A file name cannot end in a dot on Windows.".into());
    }
    if name.contains("..") {
        return Some("A look's name cannot hold two dots in a row.".into());
    }
    // Windows keeps these for devices with any extension after them,
    // so `CON.cube` is no file there either.
    let device = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_uppercase();
    let port = device
        .strip_prefix("COM")
        .or_else(|| device.strip_prefix("LPT"))
        .is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    let reserved = port
        || matches!(
            device.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        );
    if reserved {
        return Some(format!(
            "\"{}\" is a device's name on Windows, not a file's.",
            name.split('.').next().unwrap_or(name).trim_end()
        ));
    }
    if let Some((stem, word)) = name.rsplit_once('.')
        && let Some(curve) = DisplayCurve::ALL
            .into_iter()
            .find(|c| c.key().eq_ignore_ascii_case(word))
    {
        return Some(format!(
            "A name ending in \".{word}\" is read as the {} table of a look called \"{stem}\".",
            curve.phrase()
        ));
    }
    if name.len() > NAME_BYTES {
        return Some("That name is too long for a file name.".into());
    }
    None
}

/// Whether `c` is a format character (Unicode's Cf): zero-width and
/// direction marks and the like, which a name shows nothing of and two
/// names that look alike can differ by.
fn is_format(c: char) -> bool {
    matches!(
        u32::from(c),
        0xAD | 0x600..=0x605
            | 0x61C
            | 0x6DD
            | 0x70F
            | 0x890..=0x891
            | 0x8E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// [`path_for`] in the directory `dir`, the name already checked.
fn path_in(dir: &Path, name: &str) -> PathBuf {
    let mut first = None;
    for extension in lut::EXTENSIONS {
        let path = dir.join(format!("{name}.{extension}"));
        if path.is_file() {
            return path;
        }
        first.get_or_insert(path);
    }
    first.expect("at least one extension")
}

/// A table in the directory, as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The look's name: what the edit stores. The file name without
    /// its extension, less the curve for a variant.
    pub name: String,
    /// The display curve this file is the look's table for, when it is
    /// a variant (`<name>.agx.cube` declaring AgX); None for the look's
    /// own file.
    pub variant: Option<DisplayCurve>,
    pub path: PathBuf,
    /// The table's own name, when it carries one.
    pub title: Option<String>,
    /// Nodes an axis.
    pub size: usize,
    pub kind: Kind,
    pub encoding: Encoding,
    pub primaries: Primaries,
    /// The comment lines above the table, which say whether the
    /// camera match fitted it and under what, and for which body.
    pub comments: Vec<String>,
}

impl Entry {
    /// What the panel shows: the table's own name when it has one and
    /// it is not simply the file's name again. A fitted look's title
    /// that only says what its name already does, the body it was
    /// fitted on being the one it is named for, is the name.
    pub fn label(&self) -> &str {
        match &self.title {
            Some(title) if !title.is_empty() && title != &self.name => match parse_fitted(title) {
                Some((camera, frames))
                    if self.name.starts_with(camera)
                        && *title == fitted_title(&self.name, camera, frames) =>
                {
                    &self.name
                }
                _ => title,
            },
            _ => &self.name,
        }
    }

    /// The body a fitted look was fitted on: what its header
    /// declares, or for a table from before the header said, what its
    /// title says.
    pub fn fitted_on(&self) -> Option<&str> {
        if is_fitted(&self.comments)
            && let Some(on) = lut::declared(&self.comments, key::FITTED_ON)
        {
            return Some(on);
        }
        self.title.as_deref().and_then(fitted_on)
    }

    /// What display transform the table may be applied under.
    pub fn made_for(&self) -> MadeFor {
        MadeFor::read(&self.comments)
    }

    /// Whether the camera match fitted this table.
    pub fn is_fitted(&self) -> bool {
        is_fitted(&self.comments)
    }

    /// The body a fitted table declares it is for, as the panel names
    /// a camera (`greycard_core::raw::camera_name`). None for a table
    /// the match did not fit or one from before the header said.
    pub fn declared_body(&self) -> Option<String> {
        if !self.is_fitted() {
            return None;
        }
        let make = lut::declared(&self.comments, key::MAKE).unwrap_or_default();
        let model = lut::declared(&self.comments, key::MODEL).unwrap_or_default();
        (!(make.is_empty() && model.is_empty()))
            .then(|| greycard_core::raw::camera_name(make, model))
    }

    /// The body a fitted table is for: what it declares, or for a table
    /// from before the header said, read from its name and title.
    ///
    /// An old title names the body the table was fitted on, which for
    /// a borrowed table is the donor's ("Canon EOS R7 Faithful (fitted
    /// on Canon EOS R6 Mark II, 39 frames)"), so that body is taken
    /// only when the look's name starts with it. Otherwise the longest
    /// of the `known` bodies the name starts with, and failing that
    /// none: the list puts such a table with the general looks rather
    /// than under a guessed body.
    pub fn body(&self, known: &[String]) -> Option<String> {
        if !self.is_fitted() {
            return None;
        }
        if let Some(body) = self.declared_body() {
            return Some(body);
        }
        // The name is the body, or the body and then a word that is not
        // more of a model's name: "Canon EOS R6 Mark III Faithful" does
        // not start with the body "Canon EOS R6".
        let starts = |body: &str| {
            let (n, b) = (self.name.to_lowercase(), body.trim().to_lowercase());
            if b.is_empty() {
                return false;
            }
            n == b
                || n.strip_prefix(&format!("{b} "))
                    .is_some_and(|rest| rest.split_whitespace().next() != Some("mark"))
        };
        if let Some(on) = self.title.as_deref().and_then(fitted_on)
            && starts(on)
        {
            return Some(on.to_string());
        }
        known
            .iter()
            .filter(|k| starts(k))
            .max_by_key(|k| k.len())
            .cloned()
    }

    /// The picker's line for this look on a frame from `camera`
    /// (make and model as the panel names them): "fitted on" the body
    /// when the look was fitted on another, and nothing when it was
    /// fitted on this one, names no body, or its row already says the
    /// body (a borrowed look's label is its whole title). The look
    /// applies either way; this only says so.
    pub fn fitted_note(&self, camera: &str) -> Option<String> {
        let on = self.fitted_on()?;
        if on.eq_ignore_ascii_case(camera.trim()) || self.label() != self.name {
            return None;
        }
        Some(format!("fitted on {on}"))
    }
}

/// How a fitted look's title ends: the body it was fitted on.
const FITTED_ON: &str = " (fitted on ";

/// The title the camera match gives a look it wrote: its name, the
/// body the table was fitted on, which is not the named body's own
/// when the look was borrowed, and how many frames the fit used.
pub fn fitted_title(name: &str, camera: &str, frames: Option<usize>) -> String {
    match frames {
        Some(n) => format!("{name}{FITTED_ON}{camera}, {n} frames)"),
        None => format!("{name}{FITTED_ON}{camera})"),
    }
}

/// The body a title from [`fitted_title`] names, and the frames it
/// says the fit used.
pub fn parse_fitted(title: &str) -> Option<(&str, Option<usize>)> {
    let at = title.rfind(FITTED_ON)?;
    let inner = title[at + FITTED_ON.len()..].strip_suffix(')')?;
    let (camera, frames) = match inner.rsplit_once(", ") {
        Some((camera, count)) => match count
            .strip_suffix(" frames")
            .or_else(|| count.strip_suffix(" frame"))
            .and_then(|n| n.parse::<usize>().ok())
        {
            Some(n) => (camera, Some(n)),
            None => (inner, None),
        },
        None => (inner, None),
    };
    (!camera.is_empty()).then_some((camera, frames))
}

/// The body a title from [`fitted_title`] names.
pub fn fitted_on(title: &str) -> Option<&str> {
    parse_fitted(title).map(|(camera, _)| camera)
}

impl Entry {
    /// The line under the list when this one is chosen: what it is and
    /// what it was made for, since neither is on the row.
    pub fn described(&self) -> String {
        let mut out = format!("{}, {} nodes", self.kind.name(), self.size);
        if self.encoding != Encoding::default() || self.primaries != Primaries::default() {
            out.push_str(&format!(
                ", {} in {}",
                self.encoding.name(),
                self.primaries.name()
            ));
        }
        out
    }
}

/// Every look table in the directory, by name.
///
/// Only each file's header is read — its size and what it says it is —
/// not its table: a directory of film stocks at 64 nodes an axis is
/// megabytes of floats nobody has asked for yet, and this runs on the
/// thread that draws the panel. The table that is chosen is read in
/// full by [`load`], once. A file that will not read is passed over
/// with a word in the log.
pub fn list() -> Vec<Entry> {
    // Nothing is forgotten here. [`load`] reads a file again when its
    // size or its clock moved, so a table replaced in place is picked
    // up anyway, and dropping the cache on every listing would re-read
    // the open picture's table and re-upload it to the GPU each time
    // the section is opened.
    let Some(dir) = store_dir() else {
        return Vec::new();
    };
    lut::list_dir(&dir)
        .into_iter()
        .filter_map(|(stem, path, info)| match info {
            Ok(info) => Some(entry_of(&stem, path, info)),
            Err(e) => {
                log::warn!("look {}: {e}", path.display());
                None
            }
        })
        .collect()
}

/// A file in the directory as the list holds it: a variant of a look
/// when its stem ends in a curve's word and its header declares that
/// curve (what [`LookLut::look_under`] asks of one too), else a look
/// of its own under its whole stem, so a film preset that happens to
/// be called `Neon.agx` is not taken for anything.
pub fn entry_of(stem: &str, path: PathBuf, info: lut::Info) -> Entry {
    let made = MadeFor::read(&info.comments);
    let variant = stem.rsplit_once('.').and_then(|(name, word)| {
        DisplayCurve::ALL
            .into_iter()
            .find(|c| c.key() == word && made == MadeFor::Curve(*c) && !name.is_empty())
            .map(|c| (name.to_string(), c))
    });
    let (name, variant) = match variant {
        Some((name, c)) => (name, Some(c)),
        None => (stem.to_string(), None),
    };
    Entry {
        name,
        variant,
        path,
        title: info.title,
        size: info.size,
        kind: info.kind,
        encoding: info.encoding,
        primaries: info.primaries,
        comments: info.comments,
    }
}

/// The files of the look named, its own file first.
pub fn tables_of<'a>(looks: &'a [Entry], name: &str) -> Vec<&'a Entry> {
    let mut out: Vec<&Entry> = looks.iter().filter(|e| e.name == name).collect();
    out.sort_by_key(|e| e.variant.is_some());
    out
}

/// The file a picture under `curve` takes the look named from, as
/// [`LookLut::look_under`] resolves it: the variant for the curve, else
/// the look's own file when it applies under the curve.
pub fn table_for<'a>(looks: &'a [Entry], name: &str, curve: DisplayCurve) -> Option<&'a Entry> {
    let tables = tables_of(looks, name);
    tables
        .iter()
        .find(|e| e.variant == Some(curve))
        .or_else(|| {
            tables
                .iter()
                .find(|e| e.variant.is_none() && e.made_for().applies(curve))
        })
        .copied()
}

/// The display curves the look named has a table for, when it is a
/// look made per curve; empty for one that applies under any.
pub fn curves_of(looks: &[Entry], name: &str) -> Vec<DisplayCurve> {
    let tables = tables_of(looks, name);
    if tables.iter().all(|e| e.made_for() == MadeFor::Any) {
        return Vec::new();
    }
    DisplayCurve::ALL
        .into_iter()
        .filter(|c| table_for(looks, name, *c).is_some())
        .collect()
}

/// What a path read to last time.
enum Cached {
    /// There was no file to read, and it has been said once.
    Missing,
    /// A file of this age and size read to this, or to nothing.
    Read {
        stamp: Option<std::time::SystemTime>,
        len: u64,
        table: Option<Arc<Lut3d>>,
    },
}

/// How many tables are kept at once. One edit names one, and a
/// filmstrip of files naming different ones is the only way to reach
/// even a handful; past this the lot is dropped rather than grown.
const KEEP: usize = 4;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The table in a file, read once and kept until the file changes.
///
/// A file that is missing, or that will not read, is remembered as
/// such, so the warning is said once rather than on every develop.
pub fn load(path: &Path) -> Option<Arc<Lut3d>> {
    let stamped = match std::fs::metadata(path) {
        Ok(m) => Some((m.modified().ok(), m.len())),
        Err(_) => None,
    };
    let mut cache = CACHE.lock().ok()?;
    match (cache.get(path), &stamped) {
        (Some(Cached::Missing), None) => return None,
        (Some(Cached::Read { stamp, len, table }), Some((now, size)))
            if stamp == now && len == size =>
        {
            return table.clone();
        }
        _ => {}
    }
    if cache.len() >= KEEP {
        cache.clear();
    }
    let Some((stamp, len)) = stamped else {
        log::warn!("look {}: it is not there", path.display());
        cache.insert(path.to_path_buf(), Cached::Missing);
        return None;
    };
    let table = match Lut3d::load(path) {
        Ok(table) => Some(Arc::new(table)),
        Err(e) => {
            log::warn!("look {}: {e}", path.display());
            None
        }
    };
    cache.insert(
        path.to_path_buf(),
        Cached::Read {
            stamp,
            len,
            table: table.clone(),
        },
    );
    table
}

/// Forget what has been read, so the next develop reads it again.
pub fn forget() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.clear();
    }
}

/// One row of the grouped look list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A group's heading: a body, or the general looks.
    Heading {
        /// The group's key, which a click opens or folds.
        key: String,
        label: String,
        /// The looks under it.
        count: usize,
        folded: bool,
        /// The open frame's own body.
        own: bool,
        /// The group holds the chosen look and is open whatever it was
        /// set to, so a click on it would change nothing shown.
        locked: bool,
    },
    /// A look, or None, or the chosen one the directory has not got.
    Look {
        /// What the edit stores.
        name: String,
        label: String,
        /// The curves the look has a table for, "per channel · AgX";
        /// empty for one that applies under any.
        curves: String,
        /// The look has no table for the picture's curve and is off.
        off: bool,
        /// Under a heading.
        indented: bool,
    },
}

/// The key of the general looks' group: no body's name can be it,
/// since a body's key is its name and this is not a camera.
pub const GENERAL: &str = "";

/// What the grouped list is drawn for.
#[derive(Debug, Clone, Copy)]
pub struct Listing<'a> {
    /// The look the edit names.
    pub chosen: &'a str,
    /// The open frame's body as the panel names it; empty with none.
    pub camera: &'a str,
    /// The picture's display curve.
    pub curve: DisplayCurve,
    /// The groups the user has opened (true) or folded (false), by
    /// key; a group not in it is at its default.
    pub open: &'a BTreeMap<String, bool>,
}

/// The bodies the list can name an old table's body from: the open
/// frame's, and every body a table declares or its name and title
/// agree on.
pub fn known_bodies(looks: &[Entry], camera: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |b: String| {
        if !b.trim().is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&b)) {
            out.push(b);
        }
    };
    add(camera.trim().to_string());
    for e in looks {
        if let Some(b) = e.body(&[]) {
            add(b);
        }
    }
    out
}

/// The look list grouped by body, one row a look whatever tables it
/// has: None first; then the open frame's own body, marked and open;
/// then every other body by name, folded with its count; then the
/// general looks (film presets, anyone's `.cube`, a fitted table whose
/// body cannot be read), open; then the chosen look when the directory
/// has not got it.
///
/// A group the user opened or folded stays so ([`Listing::open`]),
/// whichever frame is open; a group holding the chosen look is open
/// whatever it was, so the list shows what the edit says, as it always
/// has. A look with no table for the picture's curve is listed, off,
/// with the curves it has, not hidden: the list is where the user
/// finds a look to refit.
///
/// With no fitted table to group, there are no headings: the list is
/// the plain one it was before any look was fitted.
pub fn grouped(looks: &[Entry], at: Listing) -> Vec<Row> {
    let curve = at.curve;
    let known = known_bodies(looks, at.camera);
    let look_row = |name: &str, indented: bool| {
        let tables = tables_of(looks, name);
        let label = tables.first().map_or(name, |e| e.label()).to_string();
        Row::Look {
            name: name.to_string(),
            label,
            curves: curves_of(looks, name)
                .iter()
                .map(|c| c.phrase())
                .collect::<Vec<_>>()
                .join(" · "),
            off: table_for(looks, name, curve).is_none(),
            indented,
        }
    };
    let mut rows = vec![Row::Look {
        name: NONE.to_string(),
        label: "None".to_string(),
        curves: String::new(),
        off: false,
        indented: false,
    }];
    // The looks by name, in the directory's order; then by body, case
    // aside, the body read from whichever of a look's tables says it,
    // its own file first; the general looks apart.
    let mut names: Vec<&str> = Vec::new();
    for e in looks {
        if !names.contains(&e.name.as_str()) {
            names.push(&e.name);
        }
    }
    let mut bodies: BTreeMap<String, (String, Vec<&str>)> = BTreeMap::new();
    let mut general: Vec<&str> = Vec::new();
    for name in names {
        let tables = tables_of(looks, name);
        let body = tables
            .iter()
            .find_map(|e| e.declared_body())
            .or_else(|| tables.iter().find_map(|e| e.body(&known)));
        match body {
            Some(body) => bodies
                .entry(body.to_lowercase())
                .or_insert_with(|| (body.clone(), Vec::new()))
                .1
                .push(name),
            None => general.push(name),
        }
    }
    if bodies.is_empty() {
        rows.extend(general.into_iter().map(|n| look_row(n, false)));
    } else {
        let own_key = at.camera.trim().to_lowercase();
        let mut order: Vec<(String, Vec<&str>, bool)> = Vec::new();
        if let Some((label, members)) = bodies.remove(&own_key).filter(|_| !own_key.is_empty()) {
            order.push((label, members, true));
        }
        order.extend(bodies.into_values().map(|(l, m)| (l, m, false)));
        let groups = order
            .into_iter()
            .map(|(label, members, own)| (label.clone(), label, members, own, !own))
            .chain((!general.is_empty()).then(|| {
                (
                    GENERAL.to_string(),
                    "General".to_string(),
                    general,
                    false,
                    false,
                )
            }));
        for (key, label, members, own, folded_by_default) in groups {
            let locked = members.contains(&at.chosen);
            let set = at.open.get(&key).map(|open| !open);
            let folded = set.unwrap_or(folded_by_default) && !locked;
            rows.push(Row::Heading {
                key,
                label,
                count: members.len(),
                folded,
                own,
                locked,
            });
            if !folded {
                rows.extend(members.into_iter().map(|n| look_row(n, true)));
            }
        }
    }
    if at.chosen != NONE && !looks.iter().any(|e| e.name == at.chosen) {
        rows.push(Row::Look {
            name: at.chosen.to_string(),
            label: format!("{} (missing)", at.chosen),
            curves: String::new(),
            off: false,
            indented: false,
        });
    }
    rows
}

/// What the Look section says when the chosen look has no table for
/// the picture's curve, and whether a refit is the answer (a look the
/// camera match fitted). None when it has one, or no look is chosen
/// or listed.
pub fn mismatch(looks: &[Entry], chosen: &str, curve: DisplayCurve) -> Option<(String, bool)> {
    let tables = tables_of(looks, chosen);
    if tables.is_empty() || table_for(looks, chosen, curve).is_some() {
        return None;
    }
    let has: Vec<String> =
        tables
            .iter()
            .filter_map(|e| e.made_for().phrase())
            .fold(Vec::new(), |mut v, p| {
                if !v.contains(&p) {
                    v.push(p);
                }
                v
            });
    let has = has.join(" and ");
    let fitted = tables.iter().any(|e| e.is_fitted());
    let text = if fitted {
        format!(
            "No table fitted under {} for this look yet, so it is off for this picture; it \
             has one fitted under {has}.",
            curve.phrase()
        )
    } else {
        format!(
            "Made for {has}, so it is off for this picture under {}.",
            curve.phrase()
        )
    };
    Some((text, fitted))
}

/// What is wrong with the chosen look, if anything: that the
/// directory has not got it. A look that will not read says so in the
/// log and the picture goes out without it.
pub fn warning(looks: &[Entry], chosen: &str) -> String {
    if chosen == NONE {
        return String::new();
    }
    if looks.iter().any(|e| e.name == chosen) {
        return String::new();
    }
    format!("{chosen} is not in the look directory; the picture goes out without it.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::lut::{Encoding, Kind, Primaries};

    fn entry(name: &str, title: Option<&str>) -> Entry {
        Entry {
            name: name.into(),
            variant: None,
            path: std::path::PathBuf::new(),
            title: title.map(str::to_string),
            size: 33,
            kind: Kind::Cube3d,
            encoding: Encoding::default(),
            primaries: Primaries::default(),
            comments: Vec::new(),
        }
    }

    /// The grouped list with no fitted table to group: the names and
    /// labels of its rows, which are all looks.
    fn rows(looks: &[Entry], chosen: &str) -> Vec<(String, String)> {
        let open = BTreeMap::new();
        plain(grouped(
            looks,
            Listing {
                chosen,
                camera: "Canon EOS R6m2",
                curve: DisplayCurve::Channels,
                open: &open,
            },
        ))
    }

    fn plain(rows: Vec<Row>) -> Vec<(String, String)> {
        rows.into_iter()
            .map(|r| match r {
                Row::Look { name, label, .. } => (name, label),
                Row::Heading { .. } => panic!("a heading with nothing fitted"),
            })
            .collect()
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    /// A table the match fitted, as its header now reads: the look's
    /// own file under per channel, its variant under another curve.
    fn fitted(name: &str, curve: DisplayCurve, make: &str, model: &str) -> Entry {
        let camera = greycard_core::raw::camera_name(make, model);
        let mut e = entry(name, Some(&fitted_title(name, &camera, Some(40))));
        e.comments = std::iter::once(FITTED.to_string())
            .chain(
                fitted_declarations(curve, make, model, "Canon Faithful", &camera, 40)
                    .into_iter()
                    .map(|(k, v)| format!("{k}: {v}")),
            )
            .collect();
        e.variant = (curve != DisplayCurve::Channels).then_some(curve);
        e
    }

    /// A table the match fitted before the header said anything but
    /// that the match wrote it.
    fn fitted_before(name: &str, camera: &str) -> Entry {
        let mut e = entry(name, Some(&fitted_title(name, camera, Some(40))));
        e.comments = lines(&["encoding: srgb", "primaries: srgb", FITTED]);
        e
    }

    /// The listing's reading of a file: a stem ending in a curve's
    /// word is that look's variant only when its header declares the
    /// curve, so a film preset called `Neon.agx` is a look of its own.
    #[test]
    fn a_variant_is_told_by_its_stem_and_its_header_together() {
        let info = |comments: &[&str]| lut::Info {
            title: None,
            size: 33,
            encoding: Encoding::default(),
            primaries: Primaries::default(),
            kind: Kind::Cube3d,
            comments: lines(comments),
        };
        let agx = entry_of(
            "R6 Faithful.agx",
            PathBuf::new(),
            info(&[FITTED, "display_curve: agx"]),
        );
        assert_eq!(
            (agx.name.as_str(), agx.variant),
            ("R6 Faithful", Some(DisplayCurve::Agx))
        );
        let neon = entry_of("Neon.agx", PathBuf::new(), info(&["made by hand"]));
        assert_eq!((neon.name.as_str(), neon.variant), ("Neon.agx", None));
        let wrong = entry_of(
            "R6 Faithful.agx",
            PathBuf::new(),
            info(&[FITTED, "display_curve: channels"]),
        );
        assert_eq!(wrong.variant, None);
        let own = entry_of("R6 Faithful", PathBuf::new(), info(&[FITTED]));
        assert_eq!((own.name.as_str(), own.variant), ("R6 Faithful", None));
        assert_eq!(
            variant_stem("R6 Faithful", DisplayCurve::Channels),
            "R6 Faithful"
        );
        assert_eq!(
            variant_stem("R6 Faithful", DisplayCurve::Agx),
            "R6 Faithful.agx"
        );
    }

    /// What a table may be applied under: the curve a fitted one
    /// declares; per channel for one fitted before tables declared it,
    /// which is what every one of those was fitted under; any for a
    /// film preset or someone's `.cube`; the curve declared for a
    /// table anyone declares one on, in any of the words the editor
    /// shows; none for a curve this build has not got.
    #[test]
    fn a_table_is_made_for_the_curve_it_was_fitted_under() {
        use DisplayCurve::*;
        let agx = fitted("R6 Faithful", Agx, "Canon", "Canon EOS R6m2");
        assert_eq!(agx.made_for(), MadeFor::Curve(Agx));
        let old = fitted_before("R6 Faithful", "Canon EOS R6m2");
        assert_eq!(old.made_for(), MadeFor::Curve(Channels));
        let preset = entry("Slide Warm", Some("Slide Warm"));
        assert_eq!(preset.made_for(), MadeFor::Any);
        // A title in the match's shape does not make a table fitted:
        // the match's line does.
        let lookalike = entry("x", Some(&fitted_title("x", "Canon EOS R6m2", None)));
        assert_eq!(lookalike.made_for(), MadeFor::Any);
        let mut declared = entry("graded", None);
        for word in ["agx", "AgX", " AGX "] {
            declared.comments = lines(&[&format!("display_curve: {word}")]);
            assert_eq!(declared.made_for(), MadeFor::Curve(Agx), "{word}");
        }
        for word in ["channels", "per channel", "Per channel", "norm"] {
            declared.comments = lines(&[&format!("display_curve: {word}")]);
            assert_eq!(declared.made_for(), MadeFor::Curve(Channels), "{word}");
        }
        let mut future = entry("future", None);
        future.comments = lines(&[FITTED, "display_curve: aces2"]);
        assert_eq!(future.made_for(), MadeFor::Unknown("aces2".into()));
        for c in DisplayCurve::ALL {
            assert!(MadeFor::Any.applies(c));
            assert!(!future.made_for().applies(c));
            assert_eq!(agx.made_for().applies(c), c == Agx);
            assert_eq!(old.made_for().applies(c), c == Channels);
        }
    }

    /// The gate keeps a look whose table applies under the picture's
    /// curve and drops one that does not, and leaves a general table
    /// on under both.
    #[test]
    fn the_gate_drops_a_table_fitted_under_another_curve() {
        use DisplayCurve::*;
        let look = |comments: &[&str]| {
            let mut t = Lut3d::identity(2);
            t.comments = lines(comments);
            Some(lut::Look::new(Arc::new(t), 1.0).unwrap())
        };
        let under_agx = [FITTED, "display_curve: agx"];
        assert!(gate(look(&under_agx), Agx).is_some());
        assert!(gate(look(&under_agx), Channels).is_none());
        assert!(gate(look(&[FITTED]), Channels).is_some());
        assert!(gate(look(&[FITTED]), Agx).is_none());
        assert!(gate(look(&["Created by a grading application"]), Agx).is_some());
        assert!(gate(look(&[]), Channels).is_some());
        assert!(gate(None, Agx).is_none());
    }

    /// One look, a table per curve: the picture takes the one for its
    /// curve, the look's own file being the per-channel one; with no
    /// table for its curve the look is off, and a general table is on
    /// under every curve.
    #[test]
    fn a_picture_takes_the_looks_table_for_its_curve() {
        use DisplayCurve::*;
        let looks = vec![
            fitted_before("R6 Faithful", "Canon EOS R6m2"),
            fitted("R6 Faithful", Agx, "Canon", "Canon EOS R6m2"),
            fitted_before("R5 Faithful", "Canon EOS R5m2"),
            entry("Slide Warm", None),
        ];
        assert_eq!(
            table_for(&looks, "R6 Faithful", Channels).unwrap().variant,
            None
        );
        assert_eq!(
            table_for(&looks, "R6 Faithful", Agx).unwrap().variant,
            Some(Agx)
        );
        assert!(table_for(&looks, "R5 Faithful", Channels).is_some());
        assert!(table_for(&looks, "R5 Faithful", Agx).is_none());
        assert!(table_for(&looks, "Slide Warm", Agx).is_some());
        assert_eq!(curves_of(&looks, "R6 Faithful"), [Channels, Agx]);
        assert_eq!(curves_of(&looks, "R5 Faithful"), [Channels]);
        assert!(curves_of(&looks, "Slide Warm").is_empty());
        // An AgX table alone is a look too, off under per channel.
        let only = vec![fitted("R7 Faithful", Agx, "Canon", "Canon EOS R7")];
        assert!(table_for(&only, "R7 Faithful", Channels).is_none());
        assert_eq!(curves_of(&only, "R7 Faithful"), [Agx]);
    }

    /// The header a fit writes reads back as the body, the curve and
    /// the fitting body, with no title parsed. A table from before the
    /// header names the body its title names only when its name starts
    /// with it, so an old borrowed table is not put under its donor:
    /// it goes under a known body its name starts with, else nowhere.
    #[test]
    fn a_fitted_tables_body_comes_from_its_header_then_its_name() {
        let tagged = fitted(
            "Canon EOS R6m2 Faithful",
            DisplayCurve::Agx,
            "Canon",
            "Canon EOS R6m2",
        );
        let mut retitled = tagged.clone();
        retitled.title = Some("My R6".into());
        assert_eq!(retitled.body(&[]).as_deref(), Some("Canon EOS R6m2"));
        assert_eq!(retitled.fitted_on(), Some("Canon EOS R6m2"));
        // Make and model as the raw names them, joined as the panel
        // joins them, the make not said twice.
        let mut split = tagged.clone();
        split.comments = lines(&[FITTED, "make: NIKON CORPORATION", "model: NIKON Z 6_3"]);
        assert_eq!(split.body(&[]).as_deref(), Some("NIKON Z 6_3"));
        split.comments = lines(&[FITTED, "make: FUJIFILM", "model: GFX100S II"]);
        assert_eq!(split.body(&[]).as_deref(), Some("FUJIFILM GFX100S II"));
        let old = fitted_before("Canon EOS R6 Mark II Faithful", "Canon EOS R6 Mark II");
        assert_eq!(old.body(&[]).as_deref(), Some("Canon EOS R6 Mark II"));
        // An old borrowed table: its title names the donor.
        let borrowed = fitted_before("Canon EOS R7 Faithful", "Canon EOS R6 Mark II");
        assert_eq!(borrowed.fitted_on(), Some("Canon EOS R6 Mark II"));
        assert_eq!(borrowed.body(&[]), None);
        let known = [
            "Canon EOS R6 Mark II".to_string(),
            "Canon EOS R7".to_string(),
        ];
        assert_eq!(borrowed.body(&known).as_deref(), Some("Canon EOS R7"));
        // A known body is a whole word of the name, not any prefix.
        let r5 = fitted_before("Canon EOS R5 Mark II Faithful", "Canon EOS R6 Mark II");
        let known = [
            "Canon EOS R5".to_string(),
            "Canon EOS R5 Mark II".to_string(),
        ];
        assert_eq!(r5.body(&known).as_deref(), Some("Canon EOS R5 Mark II"));
        // A known body whose name another body's starts with: the R6
        // is not the R6 Mark III, and an old borrow for the R6 Mark III
        // with no R6 Mark III known goes nowhere.
        let r6iii = fitted_before("Canon EOS R6 Mark III Faithful", "Canon EOS R6 Mark II");
        let known = [
            "Canon EOS R6".to_string(),
            "Canon EOS R6 Mark II".to_string(),
        ];
        assert_eq!(r6iii.body(&known), None);
        let known = [
            "Canon EOS R6".to_string(),
            "Canon EOS R6 Mark III".to_string(),
        ];
        assert_eq!(r6iii.body(&known).as_deref(), Some("Canon EOS R6 Mark III"));
        let r6 = fitted_before("Canon EOS R6 Faithful", "Canon EOS R5");
        assert_eq!(r6.body(&known).as_deref(), Some("Canon EOS R6"));
        let mut unreadable = fitted_before("odd", "x");
        unreadable.title = Some("odd".into());
        assert_eq!(unreadable.body(&known), None);
        assert_eq!(entry("Slide Warm", None).body(&known), None);
        // A borrowed table with a header is for the body it was
        // borrowed for, and says the body it was fitted on.
        let mut borrowed = fitted(
            "Canon EOS R5m2 Faithful",
            DisplayCurve::Agx,
            "Canon",
            "Canon EOS R5m2",
        );
        borrowed.comments.retain(|c| !c.starts_with("fitted_on"));
        borrowed.comments.push("fitted_on: Canon EOS R6m2".into());
        assert_eq!(borrowed.body(&[]).as_deref(), Some("Canon EOS R5m2"));
        assert_eq!(borrowed.fitted_on(), Some("Canon EOS R6m2"));
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Heading {
                    label,
                    count,
                    folded,
                    own,
                    locked,
                    ..
                } => format!(
                    "# {label} {count}{}{}{}",
                    if *folded { " folded" } else { "" },
                    if *own { " own" } else { "" },
                    if *locked { " locked" } else { "" }
                ),
                Row::Look {
                    name,
                    curves,
                    off,
                    indented,
                    ..
                } => format!(
                    "{}{name}{}{}",
                    if *indented { "  " } else { "" },
                    if curves.is_empty() {
                        String::new()
                    } else {
                        format!(" [{curves}]")
                    },
                    if *off { " off" } else { "" }
                ),
            })
            .collect()
    }

    fn listing<'a>(
        chosen: &'a str,
        camera: &'a str,
        curve: DisplayCurve,
        open: &'a BTreeMap<String, bool>,
    ) -> Listing<'a> {
        Listing {
            chosen,
            camera,
            curve,
            open,
        }
    }

    /// The list by body, one row a look: the open frame's first,
    /// marked and open; the others folded with their counts; the
    /// general looks in their own group; a look with no table for the
    /// picture's curve off in its group, with the curves it has.
    #[test]
    fn the_list_groups_by_body_with_the_frames_own_first() {
        use DisplayCurve::*;
        let looks = vec![
            fitted("Canon EOS R5m2 Faithful", Agx, "Canon", "Canon EOS R5m2"),
            fitted_before("Canon EOS R6m2 Faithful", "Canon EOS R6m2"),
            fitted("Canon EOS R6m2 Faithful", Agx, "Canon", "Canon EOS R6m2"),
            fitted_before("Canon EOS R6m2 Standard", "Canon EOS R6m2"),
            fitted("FUJIFILM GFX100S II Reala", Agx, "FUJIFILM", "GFX100S II"),
            entry("Slide Warm", None),
        ];
        let none = BTreeMap::new();
        let listed = grouped(&looks, listing("none", "Canon EOS R6m2", Agx, &none));
        assert_eq!(
            shape(&listed),
            [
                "none",
                "# Canon EOS R6m2 2 own",
                "  Canon EOS R6m2 Faithful [per channel · AgX]",
                "  Canon EOS R6m2 Standard [per channel] off",
                "# Canon EOS R5m2 1 folded",
                "# FUJIFILM GFX100S II 1 folded",
                "# General 1",
                "  Slide Warm",
            ]
        );
        // Under per channel the old fit is the one that applies.
        let listed = grouped(&looks, listing("none", "Canon EOS R6m2", Channels, &none));
        assert_eq!(shape(&listed)[3], "  Canon EOS R6m2 Standard [per channel]");
        // A group opened or folded stays so; the group holding the
        // chosen look is open whatever it was set to, and says it is.
        let open: BTreeMap<String, bool> = [
            ("Canon EOS R5m2".to_string(), true),
            (GENERAL.to_string(), false),
            ("FUJIFILM GFX100S II".to_string(), false),
        ]
        .into();
        let listed = grouped(
            &looks,
            listing("FUJIFILM GFX100S II Reala", "Canon EOS R6m2", Agx, &open),
        );
        assert_eq!(
            shape(&listed)[4..],
            [
                "# Canon EOS R5m2 1",
                "  Canon EOS R5m2 Faithful [AgX]",
                "# FUJIFILM GFX100S II 1 locked",
                "  FUJIFILM GFX100S II Reala [AgX]",
                "# General 1 folded",
            ]
        );
        // The R5's group, opened on an R6 frame, is still open on an
        // R5 frame, and is the own group there.
        let listed = grouped(&looks, listing("none", "Canon EOS R5m2", Agx, &open));
        assert_eq!(shape(&listed)[1], "# Canon EOS R5m2 1 own");
        // And the frame's own group folded by the user stays folded.
        let folded: BTreeMap<String, bool> = [("Canon EOS R6m2".to_string(), false)].into();
        let listed = grouped(&looks, listing("none", "Canon EOS R6m2", Agx, &folded));
        assert_eq!(shape(&listed)[1], "# Canon EOS R6m2 2 folded own");
        // No frame open: every body folded, none marked.
        let listed = grouped(&looks, listing("none", "", Agx, &none));
        assert_eq!(
            shape(&listed),
            [
                "none",
                "# Canon EOS R5m2 1 folded",
                "# Canon EOS R6m2 2 folded",
                "# FUJIFILM GFX100S II 1 folded",
                "# General 1",
                "  Slide Warm",
            ]
        );
        // A body with no table of its own: no group for it, the rest
        // as they were.
        let listed = grouped(&looks, listing("none", "Sony ILCE-7M4", Agx, &none));
        assert_eq!(shape(&listed)[1], "# Canon EOS R5m2 1 folded");
        // And a chosen look the directory has not got is last.
        let listed = grouped(&looks, listing("Gone", "Canon EOS R6m2", Agx, &none));
        assert_eq!(shape(&listed).last().unwrap(), "Gone");
    }

    /// The store as it is in use: old tables, two of them borrowed from
    /// another body. On the borrower's frame its table is in its own
    /// group; on the donor's it is not counted as the donor's, and
    /// with nothing naming its body it goes with the general looks.
    #[test]
    fn an_old_borrowed_table_is_not_put_under_its_donor() {
        let looks = vec![
            fitted_before("Canon EOS R6 Mark II Faithful", "Canon EOS R6 Mark II"),
            fitted_before("Canon EOS R7 Faithful", "Canon EOS R6 Mark II"),
            fitted_before("Canon EOS R8 Faithful", "Canon EOS R6 Mark II"),
        ];
        let none = BTreeMap::new();
        let on_r7 = grouped(
            &looks,
            listing("none", "Canon EOS R7", DisplayCurve::Channels, &none),
        );
        assert_eq!(
            shape(&on_r7),
            [
                "none",
                "# Canon EOS R7 1 own",
                "  Canon EOS R7 Faithful [per channel]",
                "# Canon EOS R6 Mark II 1 folded",
                "# General 1",
                "  Canon EOS R8 Faithful [per channel]",
            ]
        );
        let on_r6 = grouped(
            &looks,
            listing(
                "none",
                "Canon EOS R6 Mark II",
                DisplayCurve::Channels,
                &none,
            ),
        );
        assert_eq!(shape(&on_r6)[1], "# Canon EOS R6 Mark II 1 own");
        assert_eq!(shape(&on_r6)[3], "# General 2");
    }

    /// With nothing fitted the list is the plain one it always was.
    #[test]
    fn with_no_fitted_table_the_list_has_no_headings() {
        let looks = vec![
            entry("slide-warm", Some("Slide Warm")),
            entry("faded", None),
        ];
        let open = BTreeMap::new();
        let listed = grouped(
            &looks,
            listing("none", "Canon EOS R6m2", DisplayCurve::Agx, &open),
        );
        assert_eq!(shape(&listed), ["none", "slide-warm", "faded"]);
    }

    /// The Look section's line when the chosen look has no table for
    /// the picture's curve: what it has, and a refit offered for a
    /// fitted look only.
    #[test]
    fn a_mismatch_says_what_the_look_has() {
        use DisplayCurve::*;
        let mut declared = entry("graded", None);
        declared.comments = lines(&["display_curve: channels"]);
        let mut by_hand = entry("by hand", None);
        by_hand.comments = lines(&["display_curve: per channel"]);
        let looks = vec![
            fitted_before("R6 Faithful", "Canon EOS R6m2"),
            fitted_before("R5 Faithful", "Canon EOS R5m2"),
            fitted("R5 Faithful", Agx, "Canon", "Canon EOS R5m2"),
            entry("Slide Warm", None),
            declared,
            by_hand,
        ];
        let (text, refit) = mismatch(&looks, "R6 Faithful", Agx).unwrap();
        assert!(text.starts_with("No table fitted under AgX"), "{text}");
        assert!(text.ends_with("fitted under per channel."), "{text}");
        assert!(refit);
        assert_eq!(mismatch(&looks, "R6 Faithful", Channels), None);
        assert_eq!(mismatch(&looks, "R5 Faithful", Agx), None);
        assert_eq!(mismatch(&looks, "R5 Faithful", Channels), None);
        assert_eq!(mismatch(&looks, "Slide Warm", Agx), None);
        assert_eq!(mismatch(&looks, "none", Agx), None);
        let (text, refit) = mismatch(&looks, "graded", Agx).unwrap();
        assert_eq!(
            text,
            "Made for per channel, so it is off for this picture under AgX."
        );
        assert!(!refit);
        let (text, _) = mismatch(&looks, "by hand", Agx).unwrap();
        assert!(text.starts_with("Made for per channel"), "{text}");
    }

    /// The picker lists None first, then the directory, with the
    /// table's own name where it has one.
    #[test]
    fn the_picker_lists_none_first_then_the_directory() {
        let looks = vec![
            entry("slide-warm", Some("Slide Warm")),
            entry("faded", None),
        ];
        let listed = rows(&looks, "none");
        assert_eq!(
            listed,
            vec![
                ("none".to_string(), "None".to_string()),
                ("slide-warm".to_string(), "Slide Warm".to_string()),
                ("faded".to_string(), "faded".to_string()),
            ]
        );
        assert!(warning(&looks, "none").is_empty());
        assert!(warning(&looks, "faded").is_empty());
    }

    /// A look the edit names and the directory has not got is still a
    /// row, still the chosen one, and says so: the panel says what
    /// the edit says, and a sidecar or a preset from another machine
    /// is how that happens.
    #[test]
    fn a_look_that_is_not_there_is_listed_and_warned_about() {
        let looks = vec![entry("faded", None)];
        let listed = rows(&looks, "Kodachrome-ish");
        assert_eq!(listed.len(), 3);
        assert_eq!(
            listed[2],
            (
                "Kodachrome-ish".to_string(),
                "Kodachrome-ish (missing)".to_string()
            )
        );
        let warning = warning(&looks, "Kodachrome-ish");
        assert!(warning.contains("not in the look directory"), "{warning}");
        // An empty directory is not an error, only an empty list.
        assert_eq!(rows(&[], "none").len(), 1);
    }

    /// A fitted look names the body it came from in its title, shows
    /// its own name when that says it already, and the picker says
    /// "fitted on" only for a frame from another body.
    #[test]
    fn a_fitted_look_says_its_body_on_another_bodys_frame() {
        let own = entry(
            "Canon EOS R6m2 Faithful",
            Some(&fitted_title(
                "Canon EOS R6m2 Faithful",
                "Canon EOS R6m2",
                Some(40),
            )),
        );
        assert_eq!(own.label(), "Canon EOS R6m2 Faithful");
        assert_eq!(own.fitted_on(), Some("Canon EOS R6m2"));
        assert_eq!(
            parse_fitted(own.title.as_deref().unwrap()),
            Some(("Canon EOS R6m2", Some(40)))
        );
        assert_eq!(own.fitted_note("Canon EOS R6m2"), None);
        assert_eq!(
            own.fitted_note("Canon EOS R5m2").as_deref(),
            Some("fitted on Canon EOS R6m2")
        );
        // A borrowed look's row is its whole title, which names the
        // body already: no note under it saying so again.
        let borrowed = entry(
            "Canon EOS R5m2 Faithful",
            Some(&fitted_title(
                "Canon EOS R5m2 Faithful",
                "Canon EOS R6m2",
                Some(40),
            )),
        );
        assert_eq!(
            borrowed.label(),
            "Canon EOS R5m2 Faithful (fitted on Canon EOS R6m2, 40 frames)"
        );
        assert_eq!(borrowed.fitted_note("Canon EOS R5m2"), None);
        // A title from before the frame count still reads.
        assert_eq!(
            parse_fitted("X Faithful (fitted on Canon EOS R6m2)"),
            Some(("Canon EOS R6m2", None))
        );
        // A body with a comma in its name is not taken for a count.
        assert_eq!(
            parse_fitted("X (fitted on Odd, Cam)"),
            Some(("Odd, Cam", None))
        );
        // A look with no body in its title says nothing anywhere.
        let plain = entry("slide-warm", Some("Slide Warm"));
        assert_eq!(plain.fitted_note("Canon EOS R6m2"), None);
        assert_eq!(entry("faded", None).fitted_note("X"), None);
        assert_eq!(fitted_on("Something (fitted on )"), None);
    }

    /// A `.cube`'s text: a table that takes everything to one color,
    /// so a strength is easy to read off.
    fn flat_cube(to: [f32; 3]) -> String {
        let mut text = String::from("TITLE \"Flat\"\nLUT_3D_SIZE 2\n");
        for _ in 0..8 {
            text.push_str(&format!("{} {} {}\n", to[0], to[1], to[2]));
        }
        text
    }

    #[test]
    fn a_choice_reads_and_writes_as_a_name() {
        assert_eq!(LutChoice::default(), LutChoice::None);
        assert_eq!(LutChoice::None.name(), "none");
        assert_eq!(LutChoice::from_name(""), LutChoice::None);
        assert_eq!(LutChoice::from_name("None"), LutChoice::None);
        assert_eq!(
            LutChoice::from_name(" Slide Warm "),
            LutChoice::Named("Slide Warm".into())
        );
        let json = serde_json::to_string(&LutChoice::Named("Slide Warm".into())).unwrap();
        assert_eq!(json, "\"Slide Warm\"");
        let back: LutChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LutChoice::Named("Slide Warm".into()));
    }

    /// The default is no look at a full strength, so choosing one from
    /// the panel shows it whole, and a sidecar written before the
    /// section existed reads as the default.
    #[test]
    fn the_section_defaults_to_no_look_at_full_strength() {
        let look = LookLut::default();
        assert!(look.lut.is_none());
        assert_eq!(look.strength, 1.0);
        assert!(look.is_off());
        assert!(look.look().is_none());
        let from_nothing: LookLut = serde_json::from_str("{}").unwrap();
        assert_eq!(from_nothing, LookLut::default());
        // And half of the section written out reads with the rest at
        // its default.
        let half: LookLut = serde_json::from_str("{\"strength\": 0.5}").unwrap();
        assert_eq!(half.strength, 0.5);
        assert!(half.lut.is_none());
    }

    /// The section arrived with a default, so no schema bump: a
    /// sidecar written before it reads unchanged and says no look.
    #[test]
    fn a_sidecar_without_the_section_reads_as_no_look() {
        let old = r#"{"version": 3, "light": {"exposure": 0.5}}"#;
        let edit = crate::Edit::from_json(old).unwrap();
        assert_eq!(edit.look_lut, LookLut::default());
        assert_eq!(edit.light.exposure, 0.5);
        // A version 1 sidecar, migrated, likewise.
        let ancient = r#"{"light": {"exposure": 0.5}}"#;
        assert_eq!(
            crate::Edit::from_json(ancient).unwrap().look_lut,
            LookLut::default()
        );
    }

    #[test]
    fn the_section_round_trips_in_a_sidecar_under_the_key_look() {
        let edit = crate::Edit {
            look_lut: LookLut {
                lut: LutChoice::Named("Slide Warm".into()),
                strength: 0.6,
            },
            ..crate::Edit::default()
        };
        let json = edit.to_json();
        assert!(json.contains("\"look\""), "{json}");
        assert!(json.contains("\"Slide Warm\""), "{json}");
        let back = crate::Edit::from_json(&json).unwrap();
        assert_eq!(back.look_lut, edit.look_lut);
        // A look is a finish, not a develop: choosing one does not
        // send the picture back through the engine.
        assert!(back.same_develop(&crate::Edit::default()));
    }

    /// A film preset is a look and a strength, so a preset carries the
    /// section, and carries it without being asked.
    #[test]
    fn a_preset_carries_the_look() {
        use crate::preset::Section;
        assert!(Section::Look.by_default());
        let from = crate::Edit {
            look_lut: LookLut {
                lut: LutChoice::Named("Slide Warm".into()),
                strength: 0.4,
            },
            ..crate::Edit::default()
        };
        let mut onto = crate::Edit::default();
        onto.light.exposure = 1.0;
        Section::Look.copy(&from, &mut onto);
        assert_eq!(onto.look_lut, from.look_lut);
        // And nothing else came with it.
        assert_eq!(onto.light.exposure, 1.0);
        assert!(!Section::Look.same(&from, &crate::Edit::default()));
    }

    #[test]
    fn a_name_that_is_a_path_is_refused() {
        assert!(path_for("../../etc/passwd").is_none());
        assert!(path_for("sub/dir").is_none());
        assert!(path_for("/etc/passwd").is_none());
        assert!(path_for("").is_none());
    }

    /// A name for a look is a file's name on every system: each refusal
    /// says why, and a name all of them take is taken.
    #[test]
    fn a_look_name_is_a_file_name_on_every_system() {
        let refused = |n: &str| name_refusal(n).unwrap_or_else(|| panic!("{n:?} taken"));
        assert_eq!(refused(""), "A look needs a name.");
        assert!(refused("None").contains("no look"));
        assert!(refused("a/b").contains("on any system"));
        assert!(refused("a:b").contains("Windows or macOS"));
        for c in ['\\', '*', '?', '"', '<', '>', '|'] {
            assert!(refused(&format!("a{c}b")).ends_with("on Windows."), "{c}");
        }
        assert!(refused("a\tb").contains("control character"));
        for c in ['\u{200B}', '\u{200E}', '\u{FEFF}', '\u{AD}', '\u{2066}'] {
            assert!(refused(&format!("Ne{c}on")).contains("invisible"), "{c:?}");
        }
        assert!(refused(".hidden").contains("hidden file"));
        assert!(refused("Neon.").contains("end in a dot"));
        assert!(refused("A..B").contains("two dots"));
        for device in [
            "CON", "nul", "Com1", "LPT9", "aux.film", "COM¹", "lpt³", "CONIN$", "conout$",
        ] {
            assert!(refused(device).contains("device"), "{device}");
        }
        assert!(refused("Neon.agx").contains("AgX table of a look called \"Neon\""));
        assert!(refused("Neon.channels").contains("per channel table"));
        assert!(refused(&"x".repeat(NAME_BYTES + 1)).contains("too long"));
        for fine in [
            "Neon Film",
            "Canon EOS R6 Mark II Faithful",
            "Kodak 400 (warm)",
            "COM0",
            "Console",
            "Neon.v2",
            "Été",
            &"x".repeat(NAME_BYTES),
        ] {
            assert_eq!(name_refusal(fine), None, "{fine}");
            assert!(is_a_name(fine), "{fine}");
        }
    }

    #[test]
    fn a_label_prefers_the_tables_own_name() {
        let mut entry = Entry {
            name: "slide-warm".into(),
            variant: None,
            path: PathBuf::new(),
            title: Some("Slide Warm".into()),
            size: 33,
            kind: Kind::Cube3d,
            encoding: Encoding::default(),
            primaries: Primaries::default(),
            comments: Vec::new(),
        };
        assert_eq!(entry.label(), "Slide Warm");
        assert_eq!(entry.described(), "3D .cube, 33 nodes");
        entry.title = None;
        assert_eq!(entry.label(), "slide-warm");
        // A table that is not the usual sRGB says so.
        entry.encoding = Encoding::Linear;
        assert_eq!(entry.described(), "3D .cube, 33 nodes, linear in sRGB");
    }

    /// The cache is one static for the whole process, and these tests
    /// say what is in it, so they take their turn rather than run
    /// alongside each other.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-look-{what}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_table_file_reads_once_and_is_kept() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("ok");
        let path = dir.join("Flat.cube");
        std::fs::write(&path, flat_cube([1.0, 0.0, 0.0])).unwrap();
        let table = load(&path).expect("it reads");
        assert_eq!(table.title.as_deref(), Some("Flat"));
        assert!(Arc::ptr_eq(&table, &load(&path).unwrap()));
        // Rewritten, it is read afresh: the length alone says so.
        std::fs::write(&path, flat_cube([0.125, 0.0, 0.0])).unwrap();
        let again = load(&path).expect("the new one reads");
        assert!(!Arc::ptr_eq(&table, &again));
        forget();
        assert!(!Arc::ptr_eq(&again, &load(&path).unwrap()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_is_not_a_table_resolves_to_nothing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("junk");
        let path = dir.join("not-a-lut.cube");
        std::fs::write(&path, b"this is not a lookup table").unwrap();
        assert!(load(&path).is_none());
        // Twice: the second answer comes from the cache.
        assert!(load(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_table_is_remembered_as_missing() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("missing");
        let path = dir.join("Not Here Yet.cube");
        assert!(load(&path).is_none());
        assert!(matches!(
            CACHE.lock().unwrap().get(&path),
            Some(Cached::Missing)
        ));
        assert!(load(&path).is_none());
        std::fs::write(&path, flat_cube([0.5, 0.5, 0.5])).unwrap();
        assert!(load(&path).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// On disk: a look's own file and its AgX variant beside it, each
    /// taking everything to one color. A picture on each curve takes
    /// its own table; a look with only a per-channel table is no look
    /// under AgX; a film preset applies under both; a stray `.agx`
    /// file that does not declare AgX is not taken as the variant.
    #[test]
    fn a_look_resolves_to_its_table_for_the_pictures_curve() {
        let _turn = ONE_AT_A_TIME.lock();
        use DisplayCurve::*;
        let dir = scratch("curves");
        let table = |file: &str, header: &str, to: [f32; 3]| {
            let text = flat_cube(to).replacen("LUT_3D_SIZE", &format!("{header}LUT_3D_SIZE"), 1);
            std::fs::write(dir.join(file), text).unwrap();
        };
        let fit = format!("# {FITTED}\n");
        table("R6.cube", &fit, [1.0, 0.0, 0.0]);
        table(
            "R6.agx.cube",
            &format!("{fit}# display_curve: agx\n"),
            [0.0, 1.0, 0.0],
        );
        table("R5.cube", &fit, [0.0, 0.0, 1.0]);
        table("R5.agx.cube", "# made by hand\n", [0.5, 0.5, 0.5]);
        table("Film.cube", "", [0.25, 0.25, 0.25]);
        table(
            "Ghost.agx.cube",
            &format!("{fit}# display_curve: channels\n"),
            [0.1; 3],
        );
        let under = |name: &str, curve| {
            LookLut {
                lut: LutChoice::Named(name.into()),
                strength: 1.0,
            }
            .look_under_in(&dir, curve)
            .map(|l| l.lut.data[0])
        };
        assert_eq!(under("R6", Channels), Some([1.0, 0.0, 0.0]));
        assert_eq!(under("R6", Agx), Some([0.0, 1.0, 0.0]));
        assert_eq!(under("R5", Channels), Some([0.0, 0.0, 1.0]));
        assert_eq!(under("R5", Agx), None);
        assert_eq!(under("Film", Agx), Some([0.25; 3]));
        assert_eq!(under("Film", Channels), Some([0.25; 3]));
        assert_eq!(under("Gone", Agx), None);
        // Whether a look is there at all agrees with the listing: a
        // variant counts only when it declares its curve.
        let named = |name: &str| LookLut {
            lut: LutChoice::Named(name.into()),
            strength: 1.0,
        };
        assert!(named("R6").is_there_in(&dir));
        assert!(named("Film").is_there_in(&dir));
        assert!(!named("Ghost").is_there_in(&dir));
        assert!(!named("Gone").is_there_in(&dir));
        assert_eq!(under("Ghost", Agx), None);
        assert_eq!(under("Ghost", Channels), None);
        // And the listing reads the same directory the same way.
        let entries: Vec<Entry> = lut::list_dir(&dir)
            .into_iter()
            .map(|(stem, path, info)| entry_of(&stem, path, info.unwrap()))
            .collect();
        assert_eq!(table_for(&entries, "R6", Agx).unwrap().variant, Some(Agx));
        assert!(table_for(&entries, "R5", Agx).is_none());
        assert!(entries.iter().any(|e| e.name == "R5.agx"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The directory the panel lists, and what a header read gives it.
    #[test]
    fn the_directory_lists_what_it_holds() {
        let _turn = ONE_AT_A_TIME.lock();
        let dir = scratch("list");
        std::fs::write(dir.join("Flat.cube"), flat_cube([1.0, 0.0, 0.0])).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a look").unwrap();
        let listed = lut::list_dir(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "Flat");
        assert_eq!(listed[0].2.as_ref().unwrap().size, 2);
        // A header read keeps nothing: the table is only built for the
        // one that is chosen.
        assert!(!CACHE.lock().unwrap().contains_key(&listed[0].1));
        std::fs::remove_dir_all(&dir).ok();
    }
}
