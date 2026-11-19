//! Finding a file's camera and lens in the database. Names are
//! compared as bags of tokens, letters and numbers apart and case
//! aside, so "RF24-105mm F4 L IS USM" as a camera writes it meets
//! "Canon RF 24-105mm F4L IS USM" as the database has it.

use crate::db::{Camera, Database, Lens, LensType};

/// A name as tokens: runs of letters and runs of digits (a decimal
/// point kept inside a number), lower case, with a few tokens dropped
/// that say nothing.
pub fn tokens(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut digits = false;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            let t = cur.trim_end_matches('.').to_string();
            if !t.is_empty() && !NOISE.contains(&t.as_str()) {
                out.push(t);
            }
            cur.clear();
        }
    };
    for ch in name.chars() {
        if ch.is_ascii_digit() {
            if !digits {
                flush(&mut cur, &mut out);
            }
            digits = true;
            cur.push(ch);
        } else if ch == '.' && digits && !cur.is_empty() {
            cur.push(ch);
        } else if ch.is_alphabetic() {
            if digits {
                flush(&mut cur, &mut out);
            }
            digits = false;
            cur.extend(ch.to_lowercase());
        } else {
            flush(&mut cur, &mut out);
            digits = false;
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Tokens that appear or not at a maker's whim.
const NOISE: &[&str] = &["mm", "f", "lens", "the", "asph", "aspherical"];

/// The maker's own tokens, dropped from a name before comparison.
fn without_maker(mut tokens: Vec<String>, maker: &str) -> Vec<String> {
    let maker = self::tokens(maker);
    tokens.retain(|t| !maker.contains(t));
    tokens
}

/// How well a database name fits a name a file gives: one when the
/// tokens are the same set, less as they differ, none when a number
/// on either side is missing from the other (a 24-70 is not a
/// 24-105, whatever else they share). A number in the file's name
/// that is a series code, three digits with a leading zero as Sigma
/// writes its year ("Art 019"), is not held against the database.
pub fn score(file: &[String], db: &[String]) -> Option<f32> {
    let numbers = |t: &[String]| {
        t.iter()
            .filter(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .cloned()
            .collect::<Vec<_>>()
    };
    let (fn_, dn) = (numbers(file), numbers(db));
    if fn_.is_empty() || dn.is_empty() {
        return None;
    }
    let same_number = |a: &str, b: &str| {
        a == b
            || matches!((a.parse::<f32>(), b.parse::<f32>()), (Ok(x), Ok(y)) if (x - y).abs() < 0.05)
    };
    let series_code = |n: &str| n.len() == 3 && n.starts_with('0') && !n.contains('.');
    if !fn_
        .iter()
        .all(|n| series_code(n) || dn.iter().any(|m| same_number(n, m)))
        || !dn.iter().all(|n| fn_.iter().any(|m| same_number(n, m)))
    {
        return None;
    }
    let file: Vec<String> = file.iter().filter(|t| !series_code(t)).cloned().collect();
    let file = &file[..];
    let common = file.iter().filter(|t| db.contains(t)).count();
    let union = file.len() + db.len() - common;
    if union == 0 {
        return None;
    }
    Some(common as f32 / union as f32)
}

#[cfg(feature = "testing")]
thread_local! {
    static CAMERA_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this thread has looked a body up with
/// [`Database::camera`], a walk of every camera: for a test that a
/// caller does it once, not once a row.
#[cfg(feature = "testing")]
pub fn camera_lookups() -> usize {
    CAMERA_LOOKUPS.with(std::cell::Cell::get)
}

impl Database {
    /// The camera a file's make and model name, if one does.
    pub fn camera(&self, make: &str, model: &str) -> Option<&Camera> {
        #[cfg(feature = "testing")]
        CAMERA_LOOKUPS.with(|n| n.set(n.get() + 1));
        let file = without_maker(tokens(model), make);
        let mut best: Option<(f32, &Camera)> = None;
        for c in &self.cameras {
            if !same_maker(&c.maker, make) {
                continue;
            }
            for m in &c.models {
                let db = without_maker(tokens(m), &c.maker);
                if db.is_empty() {
                    continue;
                }
                // The same name, or one that has a word more (a
                // camera's own name has a maker's flourish or two the
                // database left out); a mark or a generation more is
                // another camera.
                let s = if db == file {
                    1.0
                } else if db.iter().all(|t| file.contains(t)) {
                    db.len() as f32 / file.len().max(1) as f32
                } else {
                    continue;
                };
                if best.is_none_or(|(b, _)| s > b) {
                    best = Some((s, c));
                }
            }
        }
        best.filter(|(s, _)| *s >= 0.75).map(|(_, c)| c)
    }

    /// The lens a file's lens name names, among those that fit the
    /// camera's mount when the camera is known, that cover `focal`
    /// when it is, and that draw straight lines. When nothing fits
    /// the mount the name is tried across every mount: a third-party
    /// lens is the same glass in each, and the database lists the
    /// mounts its calibrators had.
    pub fn lens(
        &self,
        camera: Option<&Camera>,
        lens_make: Option<&str>,
        lens_model: &str,
        focal: Option<f32>,
    ) -> Option<&Lens> {
        let mounts = camera.map(|c| self.mounts_taken(&c.mount));
        self.lens_for(mounts.as_deref(), lens_make, lens_model, focal)
            .or_else(|| {
                mounts
                    .is_some()
                    .then(|| self.lens_for(None, lens_make, lens_model, focal))
                    .flatten()
            })
    }

    fn lens_for(
        &self,
        mounts: Option<&[&str]>,
        lens_make: Option<&str>,
        lens_model: &str,
        focal: Option<f32>,
    ) -> Option<&Lens> {
        let mut best: Option<(f32, &Lens)> = None;
        for l in &self.lenses {
            if l.kind != LensType::Rectilinear {
                continue;
            }
            if let Some(mounts) = mounts
                && !l.mounts.iter().any(|m| mounts.contains(&m.as_str()))
            {
                continue;
            }
            if let Some(f) = focal
                && let Some((lo, hi)) = l.focal_range()
                && (f < lo * 0.9 || f > hi * 1.1)
            {
                continue;
            }
            let file = without_maker(tokens(lens_model), &l.maker);
            for m in &l.models {
                let db = without_maker(tokens(m), &l.maker);
                let Some(mut s) = score(&file, &db) else {
                    continue;
                };
                if lens_make.is_some_and(|mk| same_maker(&l.maker, mk)) {
                    s += 0.05;
                }
                if best.is_none_or(|(b, _)| s > b) {
                    best = Some((s, l));
                }
            }
        }
        best.filter(|(s, _)| *s >= 0.4).map(|(_, l)| l)
    }
}

impl Database {
    /// The lenses a body takes, to be picked from by hand: the ones on
    /// its mount or a mount it takes that draw straight lines, one a
    /// name (the entry calibrated nearest the body's sensor size),
    /// sorted by name.
    pub fn lenses_for(&self, camera: &Camera) -> Vec<&Lens> {
        let mounts = self.mounts_taken(&camera.mount);
        self.one_a_name(camera.crop_factor, |l| {
            l.mounts.iter().any(|m| mounts.contains(&m.as_str()))
        })
    }

    /// The lenses on none of the body's mounts that draw straight
    /// lines, for a lens on an adapter: one a name, nearest the body's
    /// sensor size, sorted by name. The glass is the same on any
    /// mount; the database lists the mounts its calibrators had.
    pub fn lenses_off_mount(&self, camera: &Camera) -> Vec<&Lens> {
        let on_mount = self.names_on_mount(camera);
        self.one_a_name(camera.crop_factor, |l| !on_mount.contains(l.name()))
    }

    /// The names of the straight-line lenses the body's mounts take.
    fn names_on_mount(&self, camera: &Camera) -> std::collections::HashSet<&str> {
        let mounts = self.mounts_taken(&camera.mount);
        self.lenses
            .iter()
            .filter(|l| {
                l.kind == LensType::Rectilinear
                    && l.mounts.iter().any(|m| mounts.contains(&m.as_str()))
            })
            .map(Lens::name)
            .collect()
    }

    /// The straight-line lenses `keep` keeps, one a name (the entry
    /// calibrated nearest `crop`), sorted by name.
    fn one_a_name(&self, crop: f32, keep: impl Fn(&Lens) -> bool) -> Vec<&Lens> {
        let mut out: Vec<&Lens> = Vec::new();
        for l in self
            .lenses
            .iter()
            .filter(|l| l.kind == LensType::Rectilinear && keep(l))
        {
            match out.iter_mut().find(|o| o.name() == l.name()) {
                Some(o) => {
                    if nearer(l, o, crop) {
                        *o = l;
                    }
                }
                None => out.push(l),
            }
        }
        out.sort_by_key(|l| l.name().to_lowercase());
        out
    }

    /// The lens of this name for `camera`: as [`Database::lenses_for`]
    /// gives it when it is among them, else as
    /// [`Database::lenses_off_mount`] does, a lens on an adapter. For a
    /// body the database does not know, the first straight-line entry
    /// of the name.
    pub fn lens_named(&self, camera: Option<&Camera>, name: &str) -> Option<&Lens> {
        match camera {
            Some(c) => self
                .lenses_for(c)
                .into_iter()
                .find(|l| l.name() == name)
                .or_else(|| {
                    // Off the mount: the entry of the name nearest the
                    // body's sensor size, as `lenses_off_mount` has it,
                    // in one pass. A name on the mount was found above.
                    self.lenses
                        .iter()
                        .filter(|l| l.kind == LensType::Rectilinear && l.name() == name)
                        .reduce(|best, l| {
                            if nearer(l, best, c.crop_factor) {
                                l
                            } else {
                                best
                            }
                        })
                }),
            None => self
                .lenses
                .iter()
                .find(|l| l.kind == LensType::Rectilinear && l.name() == name),
        }
    }
}

/// Whether `a` was calibrated nearer the sensor size `crop` than `b`,
/// by ratio.
fn nearer(a: &Lens, b: &Lens, crop: f32) -> bool {
    let off = |l: &Lens| (l.crop_factor.max(0.01) / crop.max(0.01)).ln().abs();
    off(a) < off(b)
}

impl Database {
    /// The fixed lens of a body whose file names no lens: a lens on
    /// the body's own mount when that mount is a compact's (see
    /// [`Database::is_fixed_mount`]), drawing straight lines, and
    /// covering `focal` when it is known.
    ///
    /// lensfun calibrates a compact twice where it can: on the camera's
    /// own JPEGs, which the camera has already corrected, and on CHDK's
    /// DNGs, the sensor as it is. Such a body shoots raw only through
    /// CHDK, so for a `raw` the CHDK entry is taken when there is one,
    /// and for a picture that is not a raw it never is: it would
    /// correct a corrected picture twice.
    ///
    /// When several are still left, the one that is the plain body (no
    /// converter, no macro distance) is taken if there is exactly one;
    /// otherwise none, since a wrong profile is worse than no profile.
    pub fn fixed_lens(&self, camera: &Camera, focal: Option<f32>, raw: bool) -> Option<&Lens> {
        if !self.is_fixed_mount(&camera.mount) {
            return None;
        }
        let fits: Vec<&Lens> = self
            .lenses
            .iter()
            .filter(|l| l.kind == LensType::Rectilinear && l.mounts.contains(&camera.mount))
            .filter(|l| match (focal, l.focal_range()) {
                (Some(f), Some((lo, hi))) => f >= lo * 0.9 && f <= hi * 1.1,
                _ => true,
            })
            .collect();
        let take_chdk = raw && fits.iter().any(|l| is_chdk(l.name()));
        let fits: Vec<&Lens> = fits
            .into_iter()
            .filter(|l| is_chdk(l.name()) == take_chdk)
            .collect();
        match fits[..] {
            [] => None,
            [one] => Some(one),
            _ => {
                let mut plain = fits.into_iter().filter(|l| is_plain(l.name()));
                match (plain.next(), plain.next()) {
                    (Some(one), None) => Some(one),
                    _ => None,
                }
            }
        }
    }

    /// Whether a mount is a fixed lens's: lensfun gives a compact (or
    /// a phone, an action camera, a bridge camera) a mount of its own
    /// that it never defines, where every interchangeable mount has a
    /// `<mount>` entry with the mounts it takes. So a mount that has
    /// no entry, that no entry takes, and whose lenses sit on no
    /// defined mount besides.
    pub fn is_fixed_mount(&self, mount: &str) -> bool {
        let defined = |m: &str| self.mounts.iter().any(|d| d.name == m);
        !defined(mount)
            && !self
                .mounts
                .iter()
                .any(|d| d.compatible.iter().any(|c| c == mount))
            && !self
                .lenses
                .iter()
                .filter(|l| l.mounts.iter().any(|m| m == mount))
                .any(|l| l.mounts.iter().any(|m| defined(m)))
    }
}

/// Whether a compact's lens entry is calibrated on CHDK's DNGs.
fn is_chdk(name: &str) -> bool {
    name.to_lowercase().contains("chdk")
}

/// Whether a compact's lens entry is the body as it comes: "X &
/// compatibles" or "X & compatibles (Standard)", where its siblings
/// add a converter or a macro distance ("X & compatibles, with
/// WC-E63").
fn is_plain(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    let n = n.strip_suffix("(standard)").unwrap_or(&n).trim_end();
    n.ends_with("& compatibles")
}

/// Whether two maker names are the same maker: the first token
/// agrees ("Canon" and "Canon Inc.", "NIKON CORPORATION" and "Nikon").
fn same_maker(a: &str, b: &str) -> bool {
    match (tokens(a).first(), tokens(b).first()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SAMPLE;

    fn db() -> Database {
        let mut db = Database::default();
        db.read_xml(SAMPLE).unwrap();
        db
    }

    #[test]
    fn names_become_tokens_the_makers_way_and_the_cameras_way() {
        assert_eq!(
            tokens("RF24-105mm F4 L IS USM"),
            ["rf", "24", "105", "4", "l", "is", "usm"]
        );
        assert_eq!(
            tokens("Canon RF 24-105mm F4L IS USM"),
            ["canon", "rf", "24", "105", "4", "l", "is", "usm"]
        );
        assert_eq!(
            tokens("EF-S 10-18mm f/4.5-5.6 IS STM"),
            ["ef", "s", "10", "18", "4.5", "5.6", "is", "stm"]
        );
        assert_eq!(tokens("50mm."), ["50"]);
        assert!(same_maker("NIKON CORPORATION", "Nikon"));
        assert!(!same_maker("Canon", "Sigma"));
    }

    #[test]
    fn the_camera_is_found_by_make_and_model() {
        let db = db();
        let r6 = db.camera("Canon", "EOS R6").unwrap();
        assert_eq!(r6.crop_factor, 1.0);
        assert_eq!(
            db.camera("Canon", "Canon EOS R6").unwrap().models[1],
            "EOS R6"
        );
        assert_eq!(db.camera("Canon", "EOS R7").unwrap().crop_factor, 1.6);
        assert!(db.camera("Canon", "EOS R6 Mark II").is_none());
        assert!(db.camera("Nikon", "EOS R6").is_none());
    }

    #[test]
    fn the_lens_is_found_by_its_name_and_told_from_its_siblings() {
        let db = db();
        let r6 = db.camera("Canon", "EOS R6");
        let l = db
            .lens(r6, None, "RF24-105mm F4 L IS USM", Some(50.0))
            .unwrap();
        assert_eq!(l.name(), "Canon RF 24-105mm F4L IS USM");
        // The slower zoom has more numbers in its name; the file's
        // name has none of them.
        let l = db
            .lens(r6, Some("Canon"), "RF24-105mm F4-7.1 IS STM", Some(24.0))
            .unwrap();
        assert_eq!(l.name(), "Canon RF 24-105mm F4-7.1 IS STM");
        let l = db.lens(r6, None, "RF50mm F1.8 STM", Some(50.0)).unwrap();
        assert_eq!(l.name(), "Canon RF 50mm F1.8 STM");
        // A focal length the lens does not reach rules it out.
        assert!(db.lens(r6, None, "RF50mm F1.8 STM", Some(85.0)).is_none());
        // A lens whose listed mounts the body does not take is still
        // found by name when nothing else is; a fisheye never.
        assert_eq!(
            db.lens(r6, None, "EF-S10-18mm f/4.5-5.6 IS STM", Some(10.0))
                .unwrap()
                .name(),
            "Canon EF-S 10-18mm f/4.5-5.6 IS STM"
        );
        // Sigma's year code in the name a Canon body writes is not a
        // number the database has to match, and the Sigma's mounts
        // (as calibrated: Sony, Nikon, L) are not the body's.
        let sigma = db
            .lens(r6, None, "28mm F1.4 DG HSM | Art 019", Some(28.0))
            .unwrap();
        assert_eq!(sigma.name(), "Sigma 28mm F1.4 DG HSM | A");
        assert_eq!(
            db.lens(r6, Some("SIGMA"), "SIGMA 28mm F1.4 DG HSM | Art 019", None)
                .unwrap()
                .name(),
            "Sigma 28mm F1.4 DG HSM | A"
        );
        assert!(
            db.lens(None, None, "8mm f/3.5 Fish-eye", Some(8.0))
                .is_none()
        );
        // With no camera the mount is not checked.
        assert!(
            db.lens(None, None, "EF-S10-18mm f/4.5-5.6 IS STM", None)
                .is_some()
        );
        // Nothing alike is nothing.
        assert!(
            db.lens(r6, None, "RF100mm F2.8 L MACRO IS USM", Some(100.0))
                .is_none()
        );
    }

    /// Two Sony E bodies, full frame and crop, and a Sigma calibrated
    /// on Canon EF (full frame) and Nikon F (on a crop body), for a
    /// lens on an adapter. (The R7 would not do for the crop case: it
    /// takes Canon EF, so the Sigma is on its mount.)
    const ADAPTED: &str = r#"<lensdatabase version="2">
        <camera>
            <maker>Sony</maker>
            <model>Sony ILCE-7M4</model>
            <mount>Sony E</mount>
            <cropfactor>1</cropfactor>
        </camera>
        <camera>
            <maker>Sony</maker>
            <model>Sony ILCE-6400</model>
            <mount>Sony E</mount>
            <cropfactor>1.5</cropfactor>
        </camera>
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 35mm f/1.4 DG HSM | A</model>
            <mount>Nikon F AF</mount>
            <cropfactor>1.5</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 35mm f/1.4 DG HSM | A</model>
            <mount>Canon EF</mount>
            <mount>Nikon F AF</mount>
            <cropfactor>1</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.02"/></calibration>
        </lens>
    </lensdatabase>"#;

    #[test]
    fn a_lens_off_the_bodys_mount_is_offered_apart_and_named_back() {
        let mut db = db();
        db.read_xml(ADAPTED).unwrap();
        let sony = db.camera("Sony", "ILCE-7M4").unwrap();
        let name = "Sigma 35mm f/1.4 DG HSM | A";
        let names = |v: Vec<&Lens>| v.iter().map(|l| l.name().to_string()).collect::<Vec<_>>();
        let on = names(db.lenses_for(sony));
        assert_eq!(on, ["Sigma 28mm F1.4 DG HSM | A"]);
        let off = db.lenses_off_mount(sony);
        // One a name, the full-frame entry for a full-frame body, and
        // never a fisheye or a lens the body already takes.
        let sigma: Vec<&&Lens> = off.iter().filter(|l| l.name() == name).collect();
        assert_eq!(sigma.len(), 1);
        assert_eq!(sigma[0].crop_factor, 1.0);
        assert!(!off.iter().any(|l| l.kind != LensType::Rectilinear));
        assert!(!off.iter().any(|l| on.contains(&l.name().to_string())));
        let off = names(off);
        let mut sorted = off.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(off, sorted);
        // Picked by name, it holds; a name on the mount is the mount's.
        let picked = db.lens_named(Some(sony), name).unwrap();
        assert_eq!(picked.crop_factor, 1.0);
        assert_eq!(picked.mounts, ["Canon EF", "Nikon F AF"]);
        let own = db
            .lens_named(Some(sony), "Sigma 28mm F1.4 DG HSM | A")
            .unwrap();
        assert!(own.mounts.iter().any(|m| m == "Sony E"));
        let r6 = db.camera("Canon", "EOS R6").unwrap();
        let fifty = db.lens_named(Some(r6), "Canon RF 50mm F1.8 STM").unwrap();
        assert_eq!(fifty.mounts, ["Canon RF"]);
        // A crop body takes the entry calibrated nearest its size,
        // in the list and by name.
        let crop = db.camera("Sony", "ILCE-6400").unwrap();
        let listed = db.lenses_off_mount(crop);
        let listed = listed.iter().find(|l| l.name() == name).unwrap();
        assert_eq!(listed.crop_factor, 1.5);
        assert_eq!(db.lens_named(Some(crop), name).unwrap().crop_factor, 1.5);
        // A fisheye is not picked by name off the mount either.
        assert!(
            db.lens_named(Some(sony), "Samyang 8mm f/3.5 Fish-eye")
                .is_none()
        );
    }
}
