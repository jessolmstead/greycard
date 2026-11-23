//! The lens profile an edit develops with: the one the database finds
//! for what the file names, or the one picked by hand, a lens of the
//! database's for the body or an Adobe profile in the profile
//! directory; and the list a picker offers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use greycard_core::develop::lens::LensCorrection;
use greycard_core::raw::{Orientation, Shot};
use greycard_edit::LensChoice;
use greycard_lens::{Database, Pick, lcp};

/// What a file is, for its lens: the body's make and model, its shot,
/// and whether it is a raw.
pub type Identity<'a> = (&'a str, &'a str, &'a Shot, bool);

/// A lens the picker offers.
#[derive(Debug, Clone)]
pub struct LensOption {
    pub choice: LensChoice,
    /// The lens's name.
    pub label: String,
    /// Who made the lens, one spelling a maker (see [`options_in`]).
    pub maker: String,
    /// A lens for another mount than the body's, for one on an
    /// adapter: a lens of the database's on none of the body's mounts,
    /// its label naming its mounts, or an Adobe profile made on
    /// another maker's body. The picker lists it only when words
    /// typed match it.
    pub off_mount: bool,
    /// Which corrections its profile has for the frame: see
    /// [`LensOption::carries`].
    carries: Carried,
}

/// A row's corrections: known when it is listed, or an Adobe
/// profile's, read from its file the first time they are asked.
#[derive(Debug, Clone)]
enum Carried {
    Known(Carries),
    Adobe(Arc<AdobeRow>),
}

impl From<Carries> for Carried {
    fn from(c: Carries) -> Self {
        Carried::Known(c)
    }
}

/// What an Adobe profile's row needs to say what it carries: its file,
/// and the body and shot it is for.
#[derive(Debug)]
struct AdobeRow {
    path: PathBuf,
    body: Arc<Body>,
    read: OnceLock<Carries>,
    /// A read of its file aside under way ([`LensOption::claim_read`]).
    reading: AtomicBool,
}

/// The frame a list of rows is for: the body's sensor size when the
/// database knows it, the body, and the shot.
#[derive(Debug)]
struct Body {
    crop: Option<f32>,
    make: String,
    model: String,
    shot: Shot,
}

impl LensOption {
    /// A row whose corrections are known.
    pub fn new(
        choice: LensChoice,
        label: String,
        maker: String,
        off_mount: bool,
        carries: Carries,
    ) -> Self {
        Self {
            choice,
            label,
            maker,
            off_mount,
            carries: Carried::Known(carries),
        }
    }

    /// Which corrections its profile has for the frame. An Adobe
    /// profile's are read from its whole file the first time they are
    /// asked, once a row: only a row chosen (on the worker, before the
    /// list is handed over) or picked is, never the list, and never on
    /// the window's thread (see [`LensOption::carries_known`]).
    pub fn carries(&self) -> Carries {
        match &self.carries {
            Carried::Known(c) => *c,
            Carried::Adobe(row) => *row.read.get_or_init(|| {
                let b = &row.body;
                lcp_carries(&row.path, b.crop, (&b.make, &b.model, &b.shot))
            }),
        }
    }
}

impl LensOption {
    /// What [`LensOption::carries`] says, when it is known without a
    /// read: always for a lens of the database's, for an Adobe profile
    /// once its file has been read. For the window, which reads no file.
    pub fn carries_known(&self) -> Option<Carries> {
        match &self.carries {
            Carried::Known(c) => Some(*c),
            Carried::Adobe(row) => row.read.get().copied(),
        }
    }

    /// Whether the caller is the one to read this row's file aside (on
    /// a thread of its own, through [`LensOption::carries`]): an Adobe
    /// profile not yet read whose read nobody has claimed. One read at
    /// most is in flight a row, however often the row is shown.
    pub fn claim_read(&self) -> bool {
        match &self.carries {
            Carried::Adobe(row) => {
                row.read.get().is_none() && !row.reading.swap(true, Ordering::AcqRel)
            }
            Carried::Known(_) => false,
        }
    }
}

/// Two rows are one when all they show is: what an Adobe row carries
/// is compared as far as it is known, so a comparison reads no file.
impl PartialEq for LensOption {
    fn eq(&self, other: &Self) -> bool {
        let file = |o: &Self| match &o.carries {
            Carried::Adobe(row) => Some(row.path.clone()),
            Carried::Known(_) => None,
        };
        self.choice == other.choice
            && self.label == other.label
            && self.maker == other.maker
            && self.off_mount == other.off_mount
            && self.carries_known() == other.carries_known()
            && (self.carries_known().is_some() || file(self) == file(other))
    }
}

/// Which of the three corrections a profile has something to apply
/// of for the frame: the switches of those it has not stand down, the
/// edit's own setting kept for a lens that has them. A kind with data
/// at any setting is extended from its nearest calibration, so it is
/// had at every focal length and aperture unless what it comes to
/// there moves and shades nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carries {
    pub distortion: bool,
    pub chromatic_aberration: bool,
    pub vignetting: bool,
}

impl Carries {
    /// All three: what a switch shows when nothing says otherwise.
    pub const ALL: Self = Self {
        distortion: true,
        chromatic_aberration: true,
        vignetting: true,
    };

    /// None of the three.
    pub const NONE: Self = Self {
        distortion: false,
        chromatic_aberration: false,
        vignetting: false,
    };

    /// What a profile's correction, every part of it asked for, has
    /// that the develop applies: a part that does nothing (all-zero
    /// distortion or vignetting, an aberration of exactly one) is
    /// dropped there, and counts as none here.
    pub fn of(c: &LensCorrection) -> Self {
        Self {
            distortion: c.distortion.as_ref().is_some_and(|d| !d.is_identity()),
            chromatic_aberration: c
                .chromatic_aberration
                .as_ref()
                .is_some_and(|a| !a.is_identity()),
            vignetting: c.vignetting.as_ref().is_some_and(|v| !v.is_identity()),
        }
    }

    /// What a lens of the database's has for a frame shot at `shot`.
    pub(crate) fn of_profile(p: &greycard_lens::Profile, shot: &Shot) -> Self {
        let (w, h) = ANY_SIZE;
        Self::of(&p.correction(w, h, shot, greycard_lens::Wanted::ALL))
    }

    /// What a profile has, from its correction; nothing when there is
    /// no correction to be had.
    fn of_any(c: Option<LensCorrection>) -> Self {
        c.map_or(Self::NONE, |c| Self::of(&c))
    }
}

/// The size a correction is asked at when only what it has matters.
const ANY_SIZE: (usize, usize) = (3, 2);

/// The profile's correction for a picture `width` by `height` of the
/// frame, standing at `orientation` from the sensor, for the lens
/// `lens` chooses; none when it wants no profile or there is none to
/// be had.
pub fn profile_correction(
    size: (usize, usize),
    orientation: Orientation,
    identity: Identity,
    lens: &greycard_edit::Lens,
    db: Option<&Database>,
) -> Option<LensCorrection> {
    profile_correction_in(
        size,
        orientation,
        identity,
        lens,
        db,
        greycard_edit_io::profiles::store_dir().as_deref(),
    )
}

/// What an export says it left out of the lens: a lens picked by hand
/// that this machine has not got for this body (an Adobe profile not
/// in the profile directory, or made on another body the database
/// does not know; a lens of the database's not offered for the body,
/// or no database). None for the file's own lens, found or not, as
/// before there was a choice.
pub fn left_out(
    identity: Identity,
    lens: &greycard_edit::Lens,
    db: Option<&Database>,
) -> Option<String> {
    left_out_in(
        identity,
        lens,
        db,
        greycard_edit_io::profiles::store_dir().as_deref(),
    )
}

/// [`left_out`] with the profile directory named.
pub fn left_out_in(
    identity: Identity,
    lens: &greycard_edit::Lens,
    db: Option<&Database>,
    profiles: Option<&Path>,
) -> Option<String> {
    let name = match &lens.chosen {
        LensChoice::FromFile => return None,
        LensChoice::Database(name) | LensChoice::Lcp(name) => name,
    };
    let none = profile_correction_in(ANY_SIZE, Orientation::Normal, identity, lens, db, profiles)
        .is_none();
    (lens.wants_profile() && none)
        .then(|| format!("the lens {name}: no profile for it here; written without one"))
}

/// The lens an export names where its source names none
/// ([`greycard_core::exif::name_lens`]): the one picked by hand, a
/// lens of the database's (with its maker) or an Adobe profile's (by
/// the lens it names), or else the one the file's own tags gave the
/// decoder. None when there is none to name; a compact's lens taken
/// from its body is not named, the body says it. Named whether the
/// correction is on or not: the glass is the same.
pub fn named(
    identity: Identity,
    lens: &greycard_edit::Lens,
    db: Option<&Database>,
) -> Option<greycard_core::exif::LensName> {
    named_in(
        identity,
        lens,
        db,
        greycard_edit_io::profiles::store_dir().as_deref(),
    )
}

/// [`named`] with the profile directory named.
pub fn named_in(
    (make, model, shot, _): Identity,
    lens: &greycard_edit::Lens,
    db: Option<&Database>,
    profiles: Option<&Path>,
) -> Option<greycard_core::exif::LensName> {
    use greycard_core::exif::LensName;
    match &lens.chosen {
        LensChoice::FromFile => shot.lens_model.clone().map(|model| LensName {
            make: shot.lens_make.clone(),
            model,
        }),
        LensChoice::Database(name) => Some(LensName {
            make: db
                .and_then(|db| db.lens_named(db.camera(make, model), name))
                .and_then(|l| l.maker_name().map(String::from)),
            model: name.clone(),
        }),
        LensChoice::Lcp(file) => lcp::row_in(profiles?, file).map(|row| LensName {
            make: None,
            model: row.lens,
        }),
    }
}

/// [`profile_correction`] with the profile directory named.
pub fn profile_correction_in(
    (width, height): (usize, usize),
    orientation: Orientation,
    (make, model, shot, raw): Identity,
    lens: &greycard_edit::Lens,
    db: Option<&Database>,
    profiles: Option<&Path>,
) -> Option<LensCorrection> {
    if !lens.wants_profile() {
        return None;
    }
    let path;
    let pick = match &lens.chosen {
        LensChoice::FromFile => Pick::FromFile,
        LensChoice::Database(name) => Pick::Named(name),
        LensChoice::Lcp(file) => {
            path = lcp::path_in(profiles?, file)?;
            Pick::Adobe(&path)
        }
    };
    greycard_lens::picked_correction(
        db,
        (make, model, shot, raw),
        pick,
        (width, height),
        orientation,
    )
}

/// What the picker offers for a file: the database's lenses for its
/// body, then those on other mounts, then the Adobe profiles in the
/// profile directory, one a lens name, from the listing held since the
/// last look ([`lcp::held_in`]), so an open never waits on a walk; the
/// lens `chosen` is listed whether the listing holds it or not. Its
/// generation beside, none for no listing held: a look ending with
/// another ([`lcp::look_in`]) means another list.
pub fn options(
    db: Option<&Database>,
    identity: Identity,
    chosen: &LensChoice,
) -> (Vec<LensOption>, Option<u64>) {
    let dir = greycard_edit_io::profiles::store_dir();
    let held = dir.as_deref().and_then(lcp::held_in);
    let listed = held.as_ref().map_or(&[][..], |l| l.entries.as_slice());
    (
        options_from(db, identity, dir.as_deref(), listed, chosen),
        held.map(|l| l.generation),
    )
}

/// [`options`] with the profile directory named and looked at now.
pub fn options_in(
    db: Option<&Database>,
    identity: Identity,
    profiles: Option<&Path>,
    chosen: &LensChoice,
) -> Vec<LensOption> {
    let listing = profiles.map(|dir| lcp::refresh_in(dir, false));
    let listed = listing.as_ref().map_or(&[][..], |l| l.entries.as_slice());
    options_from(db, identity, profiles, listed, chosen)
}

/// [`options`] over the Adobe profiles `listed` in `profiles`.
///
/// An Adobe profile is for this body's mount when it was made on this
/// body, or on a body the database puts on a mount this one takes, or,
/// where the database cannot say, on a body of this one's maker.
/// Another is a lens for another mount, listed only for words typed, as
/// a lens of the database's on another mount is. Two Adobe profiles of
/// one lens (one made on raws and one on the camera's JPEGs, or one
/// made on this body and one on another) are one row: the chosen one,
/// then one for this mount, then the one made on the kind of picture
/// this is, then the one made on this body, then one made on a body on
/// this body's mount, then the first by file name. Each profile's body
/// is looked up in the database once a call.
///
/// A maker is spelled one way across the rows: the database writes
/// "Nikon" and "Nikon Corporation", "Olympus" and "Olympus Imaging
/// Corp.", and the shortest of those sharing a first word stands for
/// them all. An Adobe profile is filed as `Makers::of_profile` says.
///
/// Only the profiles' heads are read here; the chosen row's whole file
/// is read before it is returned, and another's when it is picked
/// ([`LensOption::carries`]).
pub fn options_from(
    db: Option<&Database>,
    (make, model, shot, raw): Identity,
    profiles: Option<&Path>,
    listed: &[lcp::Listed],
    chosen: &LensChoice,
) -> Vec<LensOption> {
    let makers = Makers::of(db);
    let body = db.and_then(|db| db.camera(make, model).map(|c| (db, c)));
    // A lens of the database's, as a pick by name finds it for the
    // body: the same entry, so the same corrections.
    let carries = |lens: &greycard_lens::Lens| {
        let p = greycard_lens::Profile {
            lens,
            camera: body.map(|(_, c)| c),
            from_body: false,
        };
        Carries::of_profile(&p, shot)
    };
    let mut out: Vec<LensOption> = body
        .map(|(db, c)| db.lenses_for(c))
        .unwrap_or_default()
        .into_iter()
        .map(|l| {
            LensOption::new(
                LensChoice::Database(l.name().to_string()),
                l.name().to_string(),
                makers.spelled(&l.maker),
                false,
                carries(l),
            )
        })
        .collect();
    // The lenses on other mounts, for one on an adapter: labeled with
    // the mounts they were calibrated on, the choice the name alone,
    // so a sidecar holds the name and not the label.
    out.extend(
        body.map(|(db, c)| db.lenses_off_mount(c))
            .unwrap_or_default()
            .into_iter()
            .map(|l| {
                LensOption::new(
                    LensChoice::Database(l.name().to_string()),
                    format!("{} ({})", l.name(), l.mounts.join(", ")),
                    makers.spelled(&l.maker),
                    true,
                    carries(l),
                )
            }),
    );
    // A profile made on another body is read at this body's sensor
    // size, which only the database knows: without it, only this
    // body's own are offered.
    let sized = body.is_some();
    // The mounts the frame's body takes, and each profile's body looked
    // up once a call, however many profiles were made on it.
    let taken: Vec<&str> = body.map_or_else(Vec::new, |(db, c)| db.mounts_taken(&c.mount));
    let mut cameras: HashMap<(&str, &str), Option<&greycard_lens::Camera>> = HashMap::new();
    // The chosen profile, listed whether the listing has it yet or not.
    let chosen_row = match (chosen, profiles) {
        (LensChoice::Lcp(file), Some(dir)) if !listed.iter().any(|l| l.file == *file) => {
            lcp::row_in(dir, file)
        }
        _ => None,
    };
    struct Fact<'a> {
        l: &'a lcp::Listed,
        chosen: bool,
        made_here: bool,
        /// Whether its body is on a mount the frame's body takes (its
        /// own, or one the database says it takes on an adapter), as
        /// the database knows both; none when it cannot say.
        on_mount: Option<bool>,
        /// Whether its body is on the frame body's own mount.
        same_mount: bool,
        off_mount: bool,
    }
    let mut facts: Vec<Fact> = Vec::new();
    for l in listed.iter().chain(chosen_row.as_ref()) {
        let made_here = l.made_on(make, model);
        if !(sized || made_here) {
            continue;
        }
        let mount = match (body, l.make.as_deref(), l.model.as_deref()) {
            // Made on this body: on its mount, without a look.
            _ if made_here => Some((true, true)),
            (Some((db, c)), Some(m), Some(n)) => {
                let camera = *cameras.entry((m, n)).or_insert_with(|| db.camera(m, n));
                camera.map(|o| (taken.contains(&o.mount.as_str()), o.mount == c.mount))
            }
            _ => None,
        };
        let on_mount = mount.map(|(taken, _)| taken);
        // For this mount: made on this body, or on a body the database
        // puts on a mount it takes; where the database cannot say, made
        // on a body of its maker.
        let same_make = || {
            l.make
                .as_deref()
                .is_some_and(|m| greycard_lens::matching::same_maker(m, make))
        };
        let off_mount = !(made_here || on_mount.unwrap_or_else(same_make));
        facts.push(Fact {
            l,
            chosen: LensChoice::Lcp(l.file.clone()) == *chosen,
            made_here,
            on_mount,
            same_mount: mount.is_some_and(|(_, same)| same),
            off_mount,
        });
    }
    // Of two profiles of one lens: the chosen one; then one for this
    // mount, so a lens is never listed only for words typed while a
    // profile of it for this mount is there; then the one made on the
    // kind of picture this is; then on this body; then on a body on
    // this body's own mount; then on one that takes it; then the first
    // by path.
    let rank = |f: &Fact| {
        (
            !f.chosen,
            f.off_mount,
            f.l.raw != raw,
            !f.made_here,
            !f.same_mount,
            f.on_mount != Some(true),
        )
    };

    let mut group: HashMap<&str, usize> = HashMap::new();
    let mut adobe: Vec<Fact> = Vec::new();
    for f in facts {
        match group.get(f.l.lens.as_str()) {
            Some(&at) => {
                if rank(&f) < rank(&adobe[at]) {
                    adobe[at] = f;
                }
            }
            None => {
                group.insert(f.l.lens.as_str(), adobe.len());
                adobe.push(f);
            }
        }
    }
    adobe.sort_by_cached_key(|f| (f.off_mount, f.l.lens.to_lowercase()));
    let frame = Arc::new(Body {
        crop: body.map(|(_, c)| c.crop_factor),
        make: make.to_string(),
        model: model.to_string(),
        shot: shot.clone(),
    });
    for f in adobe {
        let row = LensOption {
            maker: makers.of_profile(f.l),
            choice: LensChoice::Lcp(f.l.file.clone()),
            label: f.l.lens.clone(),
            off_mount: f.off_mount,
            carries: Carried::Adobe(Arc::new(AdobeRow {
                path: f.l.path.clone(),
                body: frame.clone(),
                read: OnceLock::new(),
                reading: AtomicBool::new(false),
            })),
        };
        // The chosen row's file read here, on the worker, so the
        // window has its corrections without reading it.
        if f.chosen {
            row.carries();
        }
        out.push(row);
    }
    out
}

/// What an Adobe profile has for the frame: its correction at the
/// body's sensor size `body_crop`, as a pick of it develops, or at its
/// own for one made on this body the database does not know.
fn lcp_carries(
    path: &Path,
    body_crop: Option<f32>,
    (make, model, shot): (&str, &str, &Shot),
) -> Carries {
    let Some(p) = lcp::load(path) else {
        return Carries::of_any(None);
    };
    let crop = body_crop.or_else(|| p.made_on(make, model).then_some(p.crop_factor));
    Carries::of_any(crop.map(|crop| {
        p.correction(
            ANY_SIZE.0,
            ANY_SIZE.1,
            crop,
            shot,
            greycard_lens::Wanted::ALL,
        )
    }))
}

/// The database's lens makers by first word, each the shortest
/// spelling of those sharing it; and each maker's series words, the
/// word after the maker's in its lenses' names ("EF", "RF", "FE",
/// "XF", "NIKKOR"), by which a profile names a lens without its maker.
struct Makers {
    spelling: std::collections::HashMap<String, String>,
    series: std::collections::HashSet<(String, String)>,
}

impl Makers {
    fn of(db: Option<&Database>) -> Self {
        let mut spelling = std::collections::HashMap::<String, String>::new();
        let mut series = std::collections::HashSet::new();
        for lens in db.into_iter().flat_map(|db| &db.lenses) {
            let maker = &lens.maker;
            let Some(word) = first_word(maker) else {
                continue;
            };
            for name in &lens.models {
                if let Some(second) = series_word(name) {
                    series.insert((word.clone(), second));
                }
            }
            let kept = spelling.entry(word).or_insert_with(|| maker.clone());
            if rank(maker) < rank(kept) {
                *kept = maker.clone();
            }
        }
        Self { spelling, series }
    }

    /// `maker` as the rows spell it.
    fn spelled(&self, maker: &str) -> String {
        first_word(maker)
            .and_then(|w| self.spelling.get(&w))
            .cloned()
            .unwrap_or_else(|| maker.to_string())
    }

    /// The maker an Adobe profile's row is filed under: the one its
    /// lens's name starts with; the camera's maker when the name
    /// starts with one of that maker's series words, by its leading
    /// letters so "EF24-105mm" counts as EF (a profile's make is the
    /// camera's, not the lens's); else the name's first word as
    /// written.
    fn of_profile(&self, l: &lcp::Listed) -> String {
        let first = first_word(&l.lens);
        if let Some(maker) = first.as_ref().and_then(|w| self.spelling.get(w)) {
            return maker.clone();
        }
        let make = l.make.as_deref().map(str::trim).filter(|m| !m.is_empty());
        let letters: Option<String> = first
            .as_ref()
            .map(|w| w.chars().take_while(|c| c.is_alphabetic()).collect())
            .filter(|w: &String| !w.is_empty());
        if let (Some(make), Some(letters)) = (make, letters)
            && let Some(maker) = first_word(make)
            && self.series.contains(&(maker, letters))
        {
            return self.spelled(make);
        }
        l.lens
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// A name's first word, lower case, up to a hyphen, letters and
/// digits alone: what two spellings of a maker share ("Nikon" and
/// "Nikon Corporation", "Schneider" and "Schneider-Kreuznach"), a
/// leading number kept with it ("7Artisans").
fn first_word(name: &str) -> Option<String> {
    let word = name.split_whitespace().next()?.split('-').next()?;
    let word: String = word
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    (!word.is_empty()).then_some(word)
}

/// A lens name's second word, lower case, when it names a series and
/// not a focal length.
fn series_word(name: &str) -> Option<String> {
    first_word(name.split_whitespace().nth(1)?).filter(|w| !w.chars().any(|c| c.is_ascii_digit()))
}

/// The spelling preferred: the shortest, then not all capitals, then
/// the first alphabetically.
fn rank(maker: &str) -> (usize, bool, &str) {
    let all_capitals = !maker.chars().any(char::is_lowercase);
    (maker.chars().count(), all_capitals, maker)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LCP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
   xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile"
   xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/">
  <rdf:Description rdf:about="">
   <photoshop:CameraProfiles><rdf:Seq>
    <rdf:li><rdf:Description stCamera:LensPrettyName="Hand 35mm"
      stCamera:Model="MODEL" stCamera:CameraRawProfile="RAW"
      stCamera:FocalLength="35" stCamera:ApertureValue="4">
     <stCamera:PerspectiveModel><rdf:Description stCamera:FocalLengthX="1"
       stCamera:RadialDistortParam1="-0.1"/></stCamera:PerspectiveModel>
    </rdf:Description></rdf:li>
   </rdf:Seq></photoshop:CameraProfiles>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    fn lcp(model: &str, raw: bool) -> String {
        LCP.replace("MODEL", model)
            .replace("RAW", if raw { "True" } else { "False" })
    }

    /// What the test lenses and profiles carry, but for one that says.
    const DISTORTION_ONLY: Carries = Carries {
        distortion: true,
        chromatic_aberration: false,
        vignetting: false,
    };

    /// A lens of `maker` on `mount`, for a database in a test.
    fn db_lens(maker: &str, model: &str, mount: &str) -> String {
        format!(
            "<lens><maker>{maker}</maker><model>{model}</model><mount>{mount}</mount>\
             <cropfactor>1</cropfactor><calibration>\
             <distortion model=\"poly3\" focal=\"50\" k1=\"0.01\"/>\
             </calibration></lens>"
        )
    }

    /// Each row's maker: the database's spellings of one maker are
    /// one, and an Adobe profile made on a Canon is filed under the
    /// maker its lens's name starts with, under Canon only when the
    /// name starts with one of Canon's series.
    #[test]
    fn the_picker_rows_carry_one_maker_spelling() {
        let dir = crate::testing::scratch_dir("lens-makers");
        let made = |lens: &str| {
            lcp("Canon EOS R6", true)
                .replace("Hand 35mm", lens)
                .replace("stCamera:Model=", "stCamera:Make=\"Canon\" stCamera:Model=")
        };
        std::fs::write(dir.join("a.lcp"), made("SIGMA 35mm F1.4 DG HSM Art")).unwrap();
        std::fs::write(dir.join("b.lcp"), made("Hand 35mm")).unwrap();
        std::fs::write(dir.join("c.lcp"), made("RF 100mm F2.8 L Macro")).unwrap();
        std::fs::write(dir.join("d.lcp"), made("7Artisans 35mm F1.4")).unwrap();
        std::fs::write(dir.join("e.lcp"), made("EF24-105mm f/4L IS USM")).unwrap();
        let mut db = Database::default();
        db.read_xml(&format!(
            "<lensdatabase version=\"2\"><camera><maker>Canon</maker>\
             <model>Canon EOS R6</model><mount>Canon RF</mount>\
             <cropfactor>1</cropfactor></camera>{}{}{}{}{}{}</lensdatabase>",
            db_lens("Canon", "Canon RF 50mm F1.8 STM", "Canon RF"),
            db_lens("Canon", "Canon EF 50mm f/1.8 STM", "Canon EF"),
            db_lens("Canon Inc.", "Canon RF 85mm F2 Macro IS STM", "Canon RF"),
            db_lens("SIGMA", "Sigma 24-70mm F2.8 DG DN Art", "Canon RF"),
            db_lens("Sigma", "Sigma 18-35mm F1.8 DC HSM", "Nikon F AF"),
            db_lens("7Artisans", "7Artisans 25mm f/1.8", "Fujifilm X"),
        ))
        .unwrap();
        let shot = Shot::default();
        let options = options_in(
            Some(&db),
            ("Canon", "EOS R6", &shot, true),
            Some(&dir),
            &LensChoice::FromFile,
        );
        let makers: Vec<(&str, &str)> = options
            .iter()
            .map(|o| (o.label.as_str(), o.maker.as_str()))
            .collect();
        assert_eq!(
            makers,
            [
                ("Canon RF 50mm F1.8 STM", "Canon"),
                ("Canon RF 85mm F2 Macro IS STM", "Canon"),
                ("Sigma 24-70mm F2.8 DG DN Art", "Sigma"),
                // Off the body's mounts, after its own, with the
                // mounts named.
                ("7Artisans 25mm f/1.8 (Fujifilm X)", "7Artisans"),
                ("Canon EF 50mm f/1.8 STM (Canon EF)", "Canon"),
                ("Sigma 18-35mm F1.8 DC HSM (Nikon F AF)", "Sigma"),
                // Profiles: a third party's under its own name, not
                // the camera's; one named by the camera maker's series
                // under the camera's maker, by its leading letters
                // when the name runs the series into the focal length;
                // a maker that starts with a number under its own name.
                ("7Artisans 35mm F1.4", "7Artisans"),
                ("EF24-105mm f/4L IS USM", "Canon"),
                ("Hand 35mm", "Hand"),
                ("RF 100mm F2.8 L Macro", "Canon"),
                ("SIGMA 35mm F1.4 DG HSM Art", "Sigma"),
            ]
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A lens on another mount, for one on an adapter, comes after the
    /// body's, labeled with its mounts in the database's order, and the
    /// choice is its name alone; picked, it corrects.
    #[test]
    fn the_picker_offers_a_lens_off_the_mount_with_its_mounts_named() {
        let name = "Sigma 35mm f/1.4 DG HSM | A";
        let mut db = Database::default();
        db.read_xml(&format!(
            "<lensdatabase version=\"2\"><camera><maker>Sony</maker>\
             <model>Sony ILCE-7M4</model><mount>Sony E</mount>\
             <cropfactor>1</cropfactor></camera>{}{}</lensdatabase>",
            db_lens("Sony", "Sony FE 50mm F1.8", "Sony E"),
            db_lens("Sigma", name, "Canon EF</mount><mount>Nikon F AF"),
        ))
        .unwrap();
        let shot = Shot {
            focal_length: Some(35.0),
            ..Default::default()
        };
        let id = ("Sony", "ILCE-7M4", &shot, true);
        let options = options_in(Some(&db), id, None, &LensChoice::FromFile);
        assert_eq!(
            options,
            [
                LensOption {
                    choice: LensChoice::Database("Sony FE 50mm F1.8".into()),
                    label: "Sony FE 50mm F1.8".into(),
                    maker: "Sony".into(),
                    off_mount: false,
                    carries: DISTORTION_ONLY.into(),
                },
                LensOption {
                    choice: LensChoice::Database(name.into()),
                    label: format!("{name} (Canon EF, Nikon F AF)"),
                    maker: "Sigma".into(),
                    off_mount: true,
                    carries: DISTORTION_ONLY.into(),
                },
            ]
        );
        let picked = greycard_edit::Lens {
            chosen: options[1].choice.clone(),
            ..Default::default()
        };
        let c = profile_correction_in(
            (600, 400),
            Orientation::Normal,
            id,
            &picked,
            Some(&db),
            None,
        );
        assert!(c.is_some_and(|c| c.distortion.is_some()));
        assert!(left_out_in(id, &picked, Some(&db), None).is_none());
    }

    #[test]
    fn the_picker_offers_one_adobe_row_a_lens_and_the_choice_corrects() {
        let dir = crate::testing::scratch_dir("lens-options");
        // Made on another body of the frame body's maker, on JPEGs.
        let canon = |text: String| {
            text.replace("stCamera:Model=", "stCamera:Make=\"Canon\" stCamera:Model=")
        };
        std::fs::write(dir.join("a.lcp"), canon(lcp("Other Body", false))).unwrap();
        std::fs::write(dir.join("b.lcp"), lcp("Other Body", true)).unwrap();
        std::fs::write(dir.join("c.lcp"), lcp("Canon EOS R6", true)).unwrap();
        let shot = Shot {
            focal_length: Some(35.0),
            ..Default::default()
        };
        let id = ("Canon", "EOS R6", &shot, true);
        let options = options_in(None, id, Some(&dir), &LensChoice::FromFile);
        // Each row's file read, as a pick reads it, to be compared.
        for o in &options {
            o.carries();
        }
        // One row for the lens: the raw profile made on this body.
        assert_eq!(
            options,
            [LensOption {
                choice: LensChoice::Lcp("c.lcp".into()),
                label: "Hand 35mm".into(),
                maker: "Hand".into(),
                off_mount: false,
                carries: DISTORTION_ONLY.into(),
            }]
        );
        // A rendered picture takes the one made on JPEGs, once the
        // database knows the body's size; until then only this body's
        // own are offered.
        let jpeg = ("Canon", "EOS R6", &shot, false);
        let own = options_in(None, jpeg, Some(&dir), &LensChoice::FromFile);
        assert_eq!(own[0].choice, LensChoice::Lcp("c.lcp".into()));
        let mut db = Database::default();
        db.read_xml(
            r#"<lensdatabase version="2"><camera><maker>Canon</maker>
            <model>Canon EOS R6</model><mount>Canon RF</mount>
            <cropfactor>1</cropfactor></camera></lensdatabase>"#,
        )
        .unwrap();
        let sized = options_in(Some(&db), jpeg, Some(&dir), &LensChoice::FromFile);
        assert_eq!(sized[0].choice, LensChoice::Lcp("a.lcp".into()));

        let picked = greycard_edit::Lens {
            chosen: LensChoice::Lcp("c.lcp".into()),
            ..Default::default()
        };
        let c = profile_correction_in(
            (600, 400),
            Orientation::Normal,
            id,
            &picked,
            None,
            Some(&dir),
        )
        .unwrap();
        assert!(c.distortion.is_some() && c.vignetting.is_none());
        // From the file, with no database: nothing, as before.
        let plain = greycard_edit::Lens::default();
        assert!(
            profile_correction_in(
                (600, 400),
                Orientation::Normal,
                id,
                &plain,
                None,
                Some(&dir)
            )
            .is_none()
        );
        // The profile switched off, or the file gone: nothing.
        let off = greycard_edit::Lens {
            profile: false,
            ..picked.clone()
        };
        assert!(
            profile_correction_in((600, 400), Orientation::Normal, id, &off, None, Some(&dir))
                .is_none()
        );
        let gone = greycard_edit::Lens {
            chosen: LensChoice::Lcp("gone.lcp".into()),
            ..Default::default()
        };
        assert!(
            profile_correction_in((600, 400), Orientation::Normal, id, &gone, None, Some(&dir))
                .is_none()
        );
        // An export says so of the missing one, and of no other.
        let said = left_out_in(id, &gone, None, Some(&dir)).unwrap();
        assert!(said.contains("gone.lcp"), "{said}");
        assert!(left_out_in(id, &picked, None, Some(&dir)).is_none());
        assert!(left_out_in(id, &plain, None, Some(&dir)).is_none());
        assert!(left_out_in(id, &off, None, Some(&dir)).is_none());
        // Made on another body, for a body no database knows: not used.
        let other = ("Canon", "EOS R5", &shot, true);
        std::fs::remove_file(dir.join("c.lcp")).unwrap();
        let b = greycard_edit::Lens {
            chosen: LensChoice::Lcp("b.lcp".into()),
            ..Default::default()
        };
        assert!(left_out_in(other, &b, None, Some(&dir)).is_some());
        crate::testing::remove_dir_retry(&dir);
    }

    /// The lens an export names: the file's, as the decoder gave it;
    /// a lens picked by hand from the database, with its maker; an
    /// Adobe profile's lens by its name, its maker unknown. Nothing for
    /// a file that names none, or a profile that is not there.
    #[test]
    fn an_export_names_the_lens_found_or_picked() {
        use greycard_core::exif::LensName;
        let dir = crate::testing::scratch_dir("lens-named");
        std::fs::write(dir.join("c.lcp"), lcp("Canon EOS R6", true)).unwrap();
        let mut db = Database::default();
        db.read_xml(&format!(
            "<lensdatabase version=\"2\"><camera><maker>Canon</maker>\
             <model>Canon EOS R6</model><mount>Canon RF</mount>\
             <cropfactor>1</cropfactor></camera>{}{}{}{}</lensdatabase>",
            db_lens("Canon", "Canon RF 50mm F1.8 STM", "Canon RF"),
            db_lens(
                "Meke</maker><maker lang=\"de\">Meke GmbH</maker><maker lang=\"en\">Meike",
                "MEKE 85mm F1.8",
                "Canon RF"
            ),
            db_lens(
                "Ricoh Imaging Company</maker><maker lang=\"en\">Pentax",
                "Pentax 645 55mm",
                "Pentax 645AF2"
            ),
            db_lens(
                "Generic</maker><maker lang=\"de\">Generisch",
                "Generic 50mm",
                "Generic"
            ),
        ))
        .unwrap();
        let named = |shot: &Shot, chosen: LensChoice, profile: bool| {
            let lens = greycard_edit::Lens {
                chosen,
                profile,
                ..Default::default()
            };
            named_in(
                ("Canon", "EOS R6", shot, true),
                &lens,
                Some(&db),
                Some(&dir),
            )
        };
        let bare = Shot::default();
        let own = Shot {
            lens_make: Some("Canon".into()),
            lens_model: Some("RF50mm F1.8 STM".into()),
            ..Default::default()
        };
        let name = |make: Option<&str>, model: &str| {
            Some(LensName {
                make: make.map(String::from),
                model: model.into(),
            })
        };
        assert_eq!(named(&bare, LensChoice::FromFile, true), None);
        assert_eq!(
            named(&own, LensChoice::FromFile, true),
            name(Some("Canon"), "RF50mm F1.8 STM")
        );
        let pick = LensChoice::Database("Canon RF 50mm F1.8 STM".into());
        for profile in [true, false] {
            assert_eq!(
                named(&bare, pick.clone(), profile),
                name(Some("Canon"), "Canon RF 50mm F1.8 STM")
            );
        }
        // The maker as a reader expects it: the English name where the
        // database gives one, and none for lensfun's "Generic".
        for (model, make) in [
            ("MEKE 85mm F1.8", Some("Meike")),
            ("Pentax 645 55mm", Some("Pentax")),
            ("Generic 50mm", None),
        ] {
            assert_eq!(
                named(&bare, LensChoice::Database(model.into()), true),
                name(make, model)
            );
        }
        assert_eq!(
            named(&bare, LensChoice::Database("Not Here 1mm".into()), true),
            name(None, "Not Here 1mm")
        );
        assert_eq!(
            named(&bare, LensChoice::Lcp("c.lcp".into()), true),
            name(None, "Hand 35mm")
        );
        assert_eq!(named(&bare, LensChoice::Lcp("gone.lcp".into()), true), None);
        crate::testing::remove_dir_retry(&dir);
    }

    /// An Adobe profile of the lens `name`, made on a Nikon Z 50, with
    /// `inner` in its one entry's model at 24mm f/4.
    fn adobe(name: &str, inner: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
   xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile"
   xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/">
  <rdf:Description rdf:about="">
   <photoshop:CameraProfiles><rdf:Seq>
    <rdf:li><rdf:Description stCamera:LensPrettyName="{name}"
      stCamera:Make="Nikon Corporation" stCamera:Model="Nikon Z 50"
      stCamera:CameraRawProfile="True" stCamera:SensorFormatFactor="1.5"
      stCamera:FocalLength="24" stCamera:ApertureValue="4" stCamera:FocusDistance="10">
     <stCamera:PerspectiveModel><rdf:Description stCamera:FocalLengthX="1">{inner}</rdf:Description></stCamera:PerspectiveModel>
    </rdf:Description></rdf:li>
   </rdf:Seq></photoshop:CameraProfiles>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
        )
    }

    /// An Adobe profile's vignetting model of `params`.
    fn adobe_vignette(params: &str) -> String {
        format!(
            r#"<stCamera:VignetteModel><rdf:Description stCamera:FocalLengthX="1" {params}/></stCamera:VignetteModel>"#
        )
    }

    /// What each row's profile has that the develop applies, by row:
    /// a lens of the database's with distortion and TCA and no
    /// vignetting has those two (the Nikkor Z DX 16-50mm's case), one
    /// with all three has all three, one without TCA has the other two,
    /// and one whose data is all zero or one moves nothing and has
    /// nothing; an Adobe profile with vignetting alone has that alone,
    /// and one with distortion and an all-zero vignetting has the
    /// distortion alone. A frame that names no focal length is read at
    /// the middle of the lens's range, where a distortion running from
    /// barrel to pincushion comes to nothing. The body is looked up
    /// once for every row.
    #[test]
    fn each_row_says_which_corrections_its_profile_has() {
        let dir = crate::testing::scratch_dir("lens-carries");
        let distortion = r#"<stCamera:RadialDistortParam1>-0.1</stCamera:RadialDistortParam1>"#;
        std::fs::write(
            dir.join("vignette.lcp"),
            adobe(
                "Adobe vignetting only",
                &adobe_vignette(r#"stCamera:VignetteModelParam1="-0.8""#),
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("zeros.lcp"),
            adobe(
                "Adobe vignetting zeros",
                &format!(
                    "{distortion}{}",
                    adobe_vignette(
                        r#"stCamera:VignetteModelParam1="0" stCamera:VignetteModelParam2="0" stCamera:VignetteModelParam3="0""#
                    )
                ),
            ),
        )
        .unwrap();
        let lens = |model: &str, calibration: &str| {
            format!(
                r#"<lens><maker>Nikon</maker><model>{model}</model><mount>Nikon Z</mount>
                <cropfactor>1.5</cropfactor><calibration>{calibration}</calibration></lens>"#
            )
        };
        let distortion_tca = r#"<distortion model="poly3" focal="16" k1="-0.02"/>
            <distortion model="poly3" focal="50" k1="0.01"/>
            <tca model="poly3" focal="16" vr="1.0002" vb="0.9998"/>"#;
        let vignetting = r#"<vignetting model="pa" focal="18" aperture="3.5" distance="10" k1="-0.5" k2="0.1" k3="0"/>"#;
        let mut db = Database::default();
        db.read_xml(&format!(
            r#"<lensdatabase version="2"><camera><maker>Nikon Corporation</maker>
            <model>Nikon Z 50</model><mount>Nikon Z</mount>
            <cropfactor>1.5</cropfactor></camera>{}{}{}{}{}</lensdatabase>"#,
            lens("Nikkor Z DX 16-50mm f/3.5-6.3 VR", distortion_tca),
            lens(
                "Nikkor Z DX 18-140mm f/3.5-6.3 VR",
                &format!("{distortion_tca}{vignetting}")
            ),
            lens(
                "Nikkor Z DX 50-250mm f/4.5-6.3 VR",
                &format!(r#"<distortion model="poly3" focal="50" k1="0.01"/>{vignetting}"#)
            ),
            lens(
                "Nikkor Z 50mm f/1.8 S",
                r#"<distortion model="poly3" focal="50" k1="0"/>
                <tca model="linear" focal="50" kr="1" kb="1"/>
                <vignetting model="pa" focal="50" aperture="1.8" distance="10" k1="0" k2="0" k3="0"/>"#
            ),
            lens(
                "Nikkor Z 24-120mm f/4 S",
                &format!(
                    r#"<distortion model="poly3" focal="18" k1="-0.02"/>
                    <distortion model="poly3" focal="140" k1="0.02"/>
                    <tca model="poly3" focal="18" vr="1.0004" vb="0.9996"/>{vignetting}"#
                )
            ),
        ))
        .unwrap();
        let rows = |shot: &Shot| {
            let before = greycard_lens::matching::camera_lookups();
            let options = options_in(
                Some(&db),
                ("Nikon Corporation", "Nikon Z 50", shot, true),
                Some(&dir),
                &LensChoice::FromFile,
            );
            assert_eq!(
                greycard_lens::matching::camera_lookups() - before,
                1,
                "the body looked up once, not once a row"
            );
            options
                .into_iter()
                .map(|o| (o.label.clone(), o.carries()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let has = |distortion, chromatic_aberration, vignetting| Carries {
            distortion,
            chromatic_aberration,
            vignetting,
        };
        let expected = |middle: Carries| {
            [
                ("Nikkor Z DX 16-50mm f/3.5-6.3 VR", has(true, true, false)),
                ("Nikkor Z DX 18-140mm f/3.5-6.3 VR", Carries::ALL),
                ("Nikkor Z DX 50-250mm f/4.5-6.3 VR", has(true, false, true)),
                ("Nikkor Z 50mm f/1.8 S", Carries::NONE),
                ("Nikkor Z 24-120mm f/4 S", middle),
                ("Adobe vignetting only", has(false, false, true)),
                ("Adobe vignetting zeros", DISTORTION_ONLY),
            ]
            .into_iter()
            .map(|(l, c)| (l.to_string(), c))
            .collect::<std::collections::BTreeMap<_, _>>()
        };
        let at_24 = Shot {
            focal_length: Some(24.0),
            f_number: Some(5.6),
            ..Default::default()
        };
        assert_eq!(rows(&at_24), expected(Carries::ALL));
        assert_eq!(
            rows(&Shot::default()),
            expected(has(false, true, true)),
            "no focal length: the middle of the range"
        );
        assert_eq!(Carries::of_any(None), Carries::NONE);
        crate::testing::remove_dir_retry(&dir);
    }

    /// An Adobe profile made on a body of another maker is a lens for
    /// another mount, listed only for words typed; of two profiles of
    /// one lens, the one made on a body of the frame body's maker is
    /// taken over the first by path, and of two made on its maker's
    /// bodies, the one on its mount.
    #[test]
    fn a_profile_made_on_another_makers_body_is_for_another_mount() {
        let dir = crate::testing::scratch_dir("lens-makes");
        let made = |folder: &str, make: &str, model: &str, lens: &str| {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            let text = lcp(model, true).replace("Hand 35mm", lens).replace(
                "stCamera:Model=",
                &format!("stCamera:Make=\"{make}\" stCamera:Model="),
            );
            std::fs::write(dir.join(folder).join("p.lcp"), text).unwrap();
        };
        made("1", "NIKON CORPORATION", "NIKON D810", "Sigma 35mm Art");
        made("2", "Canon", "Canon EOS 5D Mark III", "Sigma 35mm Art");
        made("3", "NIKON CORPORATION", "NIKON D810", "Nikkor 35mm");
        made("4", "Canon", "Canon EOS 5D Mark III", "Canon 50mm");
        made("5", "Canon", "Canon EOS R5", "Canon 50mm");
        // An RF body takes EF lenses on an adapter, as lensfun has it.
        let mut db = Database::default();
        db.read_xml(
            r#"<lensdatabase version="2">
            <mount><name>Canon RF</name><compat>Canon EF</compat></mount>
            <camera><maker>Canon</maker><model>Canon EOS R6</model>
            <mount>Canon RF</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Canon</maker><model>Canon EOS R5</model>
            <mount>Canon RF</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Canon</maker><model>Canon EOS 5D Mark III</model>
            <mount>Canon EF</mount><cropfactor>1</cropfactor></camera>
            </lensdatabase>"#,
        )
        .unwrap();
        let shot = Shot {
            focal_length: Some(35.0),
            ..Default::default()
        };
        let id = ("Canon", "EOS R6", &shot, true);
        let options = options_in(Some(&db), id, Some(&dir), &LensChoice::FromFile);
        let rows: Vec<(&str, String, bool)> = options
            .iter()
            .map(|o| (o.label.as_str(), o.choice.key(), o.off_mount))
            .collect();
        let key = |file: &str| LensChoice::Lcp(file.into()).key();
        assert_eq!(
            rows,
            [
                ("Canon 50mm", key("5/p.lcp"), false),
                ("Sigma 35mm Art", key("2/p.lcp"), false),
                ("Nikkor 35mm", key("3/p.lcp"), true),
            ]
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A profile's text as made on the body `make` `model` for `lens`.
    fn made_on(make: &str, model: &str, lens: &str) -> String {
        lcp(model, true).replace("Hand 35mm", lens).replace(
            "stCamera:Model=",
            &format!("stCamera:Make=\"{make}\" stCamera:Model="),
        )
    }

    /// Profiles in `dir`, each `(folder, make, model, lens)`, a file a
    /// folder.
    fn profiles_in(dir: &Path, made: &[(&str, &str, &str, &str)]) {
        for (folder, make, model, lens) in made {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            std::fs::write(dir.join(folder).join("p.lcp"), made_on(make, model, lens)).unwrap();
        }
    }

    /// The rows' labels and whether each is for another mount.
    fn mounts(options: &[LensOption]) -> Vec<(&str, bool)> {
        options
            .iter()
            .map(|o| (o.label.as_str(), o.off_mount))
            .collect()
    }

    /// Each profile's body is looked up in the database once a list,
    /// however many profiles were made on it and however many of one
    /// lens there are.
    #[test]
    fn each_body_is_looked_up_once_a_list() {
        let dir = crate::testing::scratch_dir("lens-lookups");
        let bodies = [
            ("Canon", "Canon EOS 5D Mark III"),
            ("Canon", "Canon EOS R5"),
            ("NIKON CORPORATION", "NIKON D810"),
        ];
        let mut made = Vec::new();
        let folders: Vec<String> = (0..12).map(|i| i.to_string()).collect();
        for (i, folder) in folders.iter().enumerate() {
            let (make, model) = bodies[i % 3];
            let lens = ["Lens A", "Lens B", "Lens C", "Lens D"][i / 3];
            made.push((folder.as_str(), make, model, lens));
        }
        profiles_in(&dir, &made);
        let mut db = Database::default();
        db.read_xml(
            r#"<lensdatabase version="2">
            <camera><maker>Canon</maker><model>Canon EOS R6</model>
            <mount>Canon RF</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Canon</maker><model>Canon EOS R5</model>
            <mount>Canon RF</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Canon</maker><model>Canon EOS 5D Mark III</model>
            <mount>Canon EF</mount><cropfactor>1</cropfactor></camera>
            </lensdatabase>"#,
        )
        .unwrap();
        let shot = Shot::default();
        let id = ("Canon", "EOS R6", &shot, true);
        let listed = lcp::list_in(&dir);
        assert_eq!(listed.len(), 12);
        let before = greycard_lens::matching::camera_lookups();
        let options = options_from(Some(&db), id, Some(&dir), &listed, &LensChoice::FromFile);
        assert_eq!(
            greycard_lens::matching::camera_lookups() - before,
            1 + bodies.len(),
            "the frame's body, then each profile's once"
        );
        // One row a lens: the one made on the R5, on the body's mount
        // (folder 3k + 1 for the k-th lens).
        assert_eq!(options.len(), 4);
        for (k, lens) in ["Lens A", "Lens B", "Lens C", "Lens D"].iter().enumerate() {
            let o = options.iter().find(|o| o.label == *lens).unwrap();
            let r5 = LensChoice::Lcp(format!("{}/p.lcp", 3 * k + 1));
            assert_eq!(o.choice, r5, "{lens}");
            assert!(!o.off_mount, "{lens}");
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// A profile is for the body's mount when the database puts both
    /// bodies on one, whatever the makers are called: an OM System
    /// body takes Olympus's profiles, a Pentax made by Ricoh takes
    /// Pentax's, and one L-mount body takes another maker's. One whose
    /// mount is known to differ is for another mount, and so is one of
    /// another maker the database cannot place; one of the same maker
    /// it cannot place is taken.
    #[test]
    fn a_profile_on_the_bodys_mount_is_listed_whatever_its_maker() {
        let dir = crate::testing::scratch_dir("lens-mounts");
        profiles_in(
            &dir,
            &[
                (
                    "1",
                    "OLYMPUS IMAGING CORP.",
                    "E-M1MarkII",
                    "M.Zuiko 12-40mm",
                ),
                ("2", "Panasonic", "DC-G9", "Leica DG 12-60mm"),
                ("3", "PENTAX", "PENTAX K-3", "HD Pentax-D FA 24-70mm"),
                ("4", "SIGMA", "fp", "Sigma 45mm F2.8 DG DN"),
                ("5", "LEICA CAMERA AG", "LEICA SL2", "Summicron-SL 50mm"),
                ("6", "Canon", "Canon EOS R5", "RF 50mm F1.8"),
                ("7", "FUJIFILM", "X-T5", "XF 33mm F1.4"),
                ("8", "OM Digital Solutions", "OM-5", "M.Zuiko 40-150mm"),
            ],
        );
        let mut db = Database::default();
        db.read_xml(
            r#"<lensdatabase version="2">
            <camera><maker>OM Digital Solutions</maker><model>OM-1</model>
            <mount>Micro 4/3 System</mount><cropfactor>2</cropfactor></camera>
            <camera><maker>Olympus</maker><model>E-M1MarkII</model>
            <mount>Micro 4/3 System</mount><cropfactor>2</cropfactor></camera>
            <camera><maker>Panasonic</maker><model>DC-G9</model>
            <mount>Micro 4/3 System</mount><cropfactor>2</cropfactor></camera>
            <camera><maker>Ricoh Imaging Company, Ltd.</maker><model>PENTAX K-1</model>
            <mount>Pentax KAF2</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Pentax</maker><model>PENTAX K-3</model>
            <mount>Pentax KAF2</mount><cropfactor>1.5</cropfactor></camera>
            <camera><maker>Panasonic</maker><model>DC-S5</model>
            <mount>L</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Sigma</maker><model>fp</model>
            <mount>L</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Leica</maker><model>SL2</model>
            <mount>L</mount><cropfactor>1</cropfactor></camera>
            <camera><maker>Canon</maker><model>Canon EOS R5</model>
            <mount>Canon RF</mount><cropfactor>1</cropfactor></camera>
            </lensdatabase>"#,
        )
        .unwrap();
        let shot = Shot::default();
        let on = |make: &str, model: &str| -> Vec<String> {
            let id = (make, model, &shot, true);
            let options = options_in(Some(&db), id, Some(&dir), &LensChoice::FromFile);
            mounts(&options)
                .into_iter()
                .filter(|(_, off)| !off)
                .map(|(l, _)| l.to_string())
                .collect()
        };
        assert_eq!(
            on("OM Digital Solutions", "OM-1"),
            ["Leica DG 12-60mm", "M.Zuiko 12-40mm", "M.Zuiko 40-150mm"],
            "Olympus and Panasonic on its mount; an OM body it cannot place, of its maker"
        );
        assert_eq!(
            on("RICOH IMAGING COMPANY, LTD.", "PENTAX K-1"),
            ["HD Pentax-D FA 24-70mm"]
        );
        assert_eq!(
            on("Panasonic", "DC-S5"),
            ["Sigma 45mm F2.8 DG DN", "Summicron-SL 50mm"],
            "L-mount across its makers; Panasonic's own G9 is on another mount"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The chosen profile wins its lens's row, though another of the
    /// lens ranks before it, so the picker shows it as the one in use.
    #[test]
    fn the_chosen_profile_wins_its_lens() {
        let dir = crate::testing::scratch_dir("lens-chosen-wins");
        profiles_in(
            &dir,
            &[
                ("1", "Canon", "Canon EOS R6", "Lens A"),
                ("2", "Canon", "Canon EOS 5D Mark III", "Lens A"),
            ],
        );
        let shot = Shot::default();
        let id = ("Canon", "EOS R6", &shot, true);
        let mut db = Database::default();
        db.read_xml(
            r#"<lensdatabase version="2"><camera><maker>Canon</maker>
            <model>Canon EOS R6</model><mount>Canon RF</mount>
            <cropfactor>1</cropfactor></camera></lensdatabase>"#,
        )
        .unwrap();
        let own = options_in(Some(&db), id, Some(&dir), &LensChoice::FromFile);
        assert_eq!(own.len(), 1);
        assert_eq!(
            own[0].choice,
            LensChoice::Lcp("1/p.lcp".into()),
            "made here"
        );
        let chosen = LensChoice::Lcp("2/p.lcp".into());
        let picked = options_in(Some(&db), id, Some(&dir), &chosen);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].choice, chosen);
        assert!(picked[0].carries_known().is_some(), "read on the worker");
        crate::testing::remove_dir_retry(&dir);
    }

    /// Only the chosen row's profile is read whole for the list, on the
    /// worker; another's when it is asked (a pick). A chosen profile
    /// the listing does not hold yet is listed all the same.
    #[test]
    fn only_the_chosen_profile_is_read_for_the_list() {
        let dir = crate::testing::scratch_dir("lens-chosen");
        std::fs::create_dir_all(dir.join("Canon")).unwrap();
        std::fs::write(dir.join("Canon/a.lcp"), lcp("Canon EOS R6", true)).unwrap();
        let other = lcp("Canon EOS R6", true).replace("Hand 35mm", "Hand 50mm");
        std::fs::write(dir.join("Canon/b.lcp"), other).unwrap();
        let shot = Shot {
            focal_length: Some(35.0),
            ..Default::default()
        };
        let id = ("Canon", "EOS R6", &shot, true);
        let chosen = LensChoice::Lcp("Canon/b.lcp".into());
        let read = |o: &LensOption| match &o.carries {
            Carried::Adobe(row) => row.read.get().is_some(),
            Carried::Known(_) => true,
        };
        let options = options_in(None, id, Some(&dir), &chosen);
        assert_eq!(options.len(), 2);
        for o in &options {
            assert_eq!(read(o), o.choice == chosen, "{}", o.label);
        }
        // A pick reads it then, and says what it carries.
        assert_eq!(options[0].carries(), DISTORTION_ONLY);
        assert!(read(&options[0]));
        // Not in the listing handed in (none held yet): listed anyway.
        let alone = options_from(None, id, Some(&dir), &[], &chosen);
        assert_eq!(alone.len(), 1);
        assert_eq!(alone[0].choice, chosen);
        assert_eq!(alone[0].label, "Hand 50mm");
        crate::testing::remove_dir_retry(&dir);
    }

    /// The picker's list over a tree of Adobe profiles, timed: the
    /// first open and later ones. Run by hand against a tree made by a
    /// script, `GREYCARD_LCP_TREE` naming it.
    #[test]
    #[ignore = "a timing, against a tree on disk"]
    fn adobe_tree_timed() {
        let Some(dir) = std::env::var_os("GREYCARD_LCP_TREE").map(std::path::PathBuf::from) else {
            return;
        };
        // The lensfun database `GREYCARD_LENSFUN_DIR` names, the whole
        // of it, else one camera.
        let db = match std::env::var_os("GREYCARD_LENSFUN_DIR") {
            Some(lensfun) => Database::load_dir(std::path::Path::new(&lensfun)).unwrap(),
            None => {
                let mut db = Database::default();
                db.read_xml(
                    r#"<lensdatabase version="2"><camera><maker>Canon</maker>
                    <model>Canon EOS 5D Mark III</model><mount>Canon EF</mount>
                    <cropfactor>1</cropfactor></camera></lensdatabase>"#,
                )
                .unwrap();
                db
            }
        };
        eprintln!("{} cameras in the database", db.cameras.len());
        let shot = Shot {
            focal_length: Some(35.0),
            ..Default::default()
        };
        let id = ("Canon", "Canon EOS 5D Mark III", &shot, true);
        // As an open lists: from the listing held, never a walk, the
        // chosen profile read whole.
        let chosen = LensChoice::Lcp(
            "Canon/Canon Canon EOS 5D Mark III (Canon 10-70mm F2.8 Series 0) - RAW.lcp".into(),
        );
        let open = |what: &str| {
            let t = std::time::Instant::now();
            let lookups = greycard_lens::matching::camera_lookups();
            let held = lcp::held_in(&dir);
            let listed = held.as_ref().map_or(&[][..], |l| l.entries.as_slice());
            let rows = options_from(Some(&db), id, Some(&dir), listed, &chosen);
            let shown = rows.iter().filter(|o| !o.off_mount).count();
            eprintln!(
                "{what} open: {} rows ({shown} on the mount) in {:?}, {} camera lookups",
                rows.len(),
                t.elapsed(),
                greycard_lens::matching::camera_lookups() - lookups
            );
        };
        open("first");
        for what in ["first walk, off the open", "later look", "later look"] {
            let t = std::time::Instant::now();
            let listing = lcp::refresh_in(&dir, false);
            eprintln!(
                "{what}: {} profiles in {:?}",
                listing.entries.len(),
                t.elapsed()
            );
        }
        open("later");
        open("later");
    }
}
