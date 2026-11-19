//! The lens profile an edit develops with: the one the database finds
//! for what the file names, or the one picked by hand, a lens of the
//! database's for the body or an Adobe profile in the profile
//! directory; and the list a picker offers.

use std::path::Path;

use greycard_core::develop::lens::LensCorrection;
use greycard_core::raw::{Orientation, Shot};
use greycard_edit::LensChoice;
use greycard_lens::{Database, Pick, lcp};

/// What a file is, for its lens: the body's make and model, its shot,
/// and whether it is a raw.
pub type Identity<'a> = (&'a str, &'a str, &'a Shot, bool);

/// A lens the picker offers.
#[derive(Debug, Clone, PartialEq)]
pub struct LensOption {
    pub choice: LensChoice,
    /// The lens's name.
    pub label: String,
    /// Who made the lens, one spelling a maker (see [`options_in`]).
    pub maker: String,
    /// A lens of the database's on none of the body's mounts, for a
    /// lens on an adapter: its label names its mounts, and the picker
    /// lists it only when words typed match it.
    pub off_mount: bool,
    /// Which corrections its profile has for the frame.
    pub carries: Carries,
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
/// profile directory, one a lens name.
pub fn options(db: Option<&Database>, identity: Identity) -> Vec<LensOption> {
    options_in(
        db,
        identity,
        greycard_edit_io::profiles::store_dir().as_deref(),
    )
}

/// [`options`] with the profile directory named.
///
/// Two Adobe profiles of one lens (one made on raws and one on the
/// camera's JPEGs, or one made on this body and one on another) are
/// one row: the one made on the kind of picture this is, then the one
/// made on this body, then the first by file name.
///
/// A maker is spelled one way across the rows: the database writes
/// "Nikon" and "Nikon Corporation", "Olympus" and "Olympus Imaging
/// Corp.", and the shortest of those sharing a first word stands for
/// them all. An Adobe profile is filed as `Makers::of_profile` says.
pub fn options_in(
    db: Option<&Database>,
    (make, model, shot, raw): Identity,
    profiles: Option<&Path>,
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
        .map(|l| LensOption {
            choice: LensChoice::Database(l.name().to_string()),
            label: l.name().to_string(),
            maker: makers.spelled(&l.maker),
            off_mount: false,
            carries: carries(l),
        })
        .collect();
    // The lenses on other mounts, for one on an adapter: labeled with
    // the mounts they were calibrated on, the choice the name alone,
    // so a sidecar holds the name and not the label.
    out.extend(
        body.map(|(db, c)| db.lenses_off_mount(c))
            .unwrap_or_default()
            .into_iter()
            .map(|l| LensOption {
                choice: LensChoice::Database(l.name().to_string()),
                label: format!("{} ({})", l.name(), l.mounts.join(", ")),
                maker: makers.spelled(&l.maker),
                off_mount: true,
                carries: carries(l),
            }),
    );
    // A profile made on another body is read at this body's sensor
    // size, which only the database knows: without it, only this
    // body's own are offered.
    let sized = body.is_some();
    // The body's sensor size, looked up once for every profile's row.
    let body_crop = body.map(|(_, c)| c.crop_factor);
    let made_here = |l: &lcp::Listed| lcp::load(&l.path).is_some_and(|p| p.made_on(make, model));
    let rank = |l: &lcp::Listed| (l.raw != raw, !made_here(l));
    let mut adobe: Vec<lcp::Listed> = Vec::new();
    let listed = profiles.map(lcp::list_in).unwrap_or_default();
    for l in listed.into_iter().filter(|l| sized || made_here(l)) {
        match adobe.iter_mut().find(|o| o.lens == l.lens) {
            Some(o) => {
                if rank(&l) < rank(o) {
                    *o = l;
                }
            }
            None => adobe.push(l),
        }
    }
    adobe.sort_by_key(|l| l.lens.to_lowercase());
    out.extend(adobe.into_iter().map(|l| LensOption {
        maker: makers.of_profile(&l),
        carries: lcp_carries(&l, body_crop, (make, model, shot)),
        choice: LensChoice::Lcp(l.file),
        label: l.lens,
        off_mount: false,
    }));
    out
}

/// What an Adobe profile's row has for the frame: its correction at
/// the body's sensor size `body_crop`, as a pick of it develops, or
/// at its own for one made on this body the database does not know.
/// The body is the caller's, looked up once for every row.
fn lcp_carries(
    l: &lcp::Listed,
    body_crop: Option<f32>,
    (make, model, shot): (&str, &str, &Shot),
) -> Carries {
    let Some(p) = lcp::load(&l.path) else {
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
        let options = options_in(Some(&db), ("Canon", "EOS R6", &shot, true), Some(&dir));
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
        let options = options_in(Some(&db), id, None);
        assert_eq!(
            options,
            [
                LensOption {
                    choice: LensChoice::Database("Sony FE 50mm F1.8".into()),
                    label: "Sony FE 50mm F1.8".into(),
                    maker: "Sony".into(),
                    off_mount: false,
                    carries: DISTORTION_ONLY,
                },
                LensOption {
                    choice: LensChoice::Database(name.into()),
                    label: format!("{name} (Canon EF, Nikon F AF)"),
                    maker: "Sigma".into(),
                    off_mount: true,
                    carries: DISTORTION_ONLY,
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
        std::fs::write(dir.join("a.lcp"), lcp("Other Body", false)).unwrap();
        std::fs::write(dir.join("b.lcp"), lcp("Other Body", true)).unwrap();
        std::fs::write(dir.join("c.lcp"), lcp("Canon EOS R6", true)).unwrap();
        let shot = Shot {
            focal_length: Some(35.0),
            ..Default::default()
        };
        let id = ("Canon", "EOS R6", &shot, true);
        let options = options_in(None, id, Some(&dir));
        // One row for the lens: the raw profile made on this body.
        assert_eq!(
            options,
            [LensOption {
                choice: LensChoice::Lcp("c.lcp".into()),
                label: "Hand 35mm".into(),
                maker: "Hand".into(),
                off_mount: false,
                carries: DISTORTION_ONLY,
            }]
        );
        // A rendered picture takes the one made on JPEGs, once the
        // database knows the body's size; until then only this body's
        // own are offered.
        let jpeg = ("Canon", "EOS R6", &shot, false);
        let own = options_in(None, jpeg, Some(&dir));
        assert_eq!(own[0].choice, LensChoice::Lcp("c.lcp".into()));
        let mut db = Database::default();
        db.read_xml(
            r#"<lensdatabase version="2"><camera><maker>Canon</maker>
            <model>Canon EOS R6</model><mount>Canon RF</mount>
            <cropfactor>1</cropfactor></camera></lensdatabase>"#,
        )
        .unwrap();
        let sized = options_in(Some(&db), jpeg, Some(&dir));
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
            );
            assert_eq!(
                greycard_lens::matching::camera_lookups() - before,
                1,
                "the body looked up once, not once a row"
            );
            options
                .into_iter()
                .map(|o| (o.label, o.carries))
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
}
