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
fn without_maker(tokens: Vec<String>, maker: &str) -> Vec<String> {
    without_makers(tokens, [maker])
}

/// The tokens of each of a maker's names, dropped from a name: a body
/// filed under "Ricoh Imaging Company, Ltd." and named "Pentax K-1"
/// is a K-1.
fn without_makers<'a>(
    mut tokens: Vec<String>,
    makers: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let maker: Vec<String> = makers.into_iter().flat_map(self::tokens).collect();
    tokens.retain(|t| !maker.contains(t));
    tokens
}

/// How well a database name fits a name a file gives: one when the
/// tokens are the same set, less as they differ, none when a number
/// on either side is missing from the other (a 24-70 is not a
/// 24-105, whatever else they share).
///
/// A code, three digits with a leading zero (Sigma's year, "Art 019";
/// Tamron's model, "A065"), is never a focal length: read as one, a
/// Mark II's "024" would be the 24 of a 24-70. When both names carry
/// codes they must be the same ones, which tells Tamron's A056 from its
/// A065; when only one does, its codes are left out, since a body and
/// the database each write them or not as they like, but then most of
/// the rest must agree: a name sharing only its numbers and a word or
/// two is another design of the maker's ("85mm f/1.4 EX DG HSM" against
/// "85mm F1.4 DG DN | Art 020" is 0.43; the right ones are 0.67 and up).
pub fn score(file: &[String], db: &[String]) -> Option<f32> {
    let code = |n: &String| n.len() == 3 && n.starts_with('0') && !n.contains('.');
    let codes = |t: &[String]| {
        let mut c: Vec<String> = t.iter().filter(|n| code(n)).cloned().collect();
        c.sort();
        c.dedup();
        c
    };
    let (fc, dc) = (codes(file), codes(db));
    if !fc.is_empty() && !dc.is_empty() && fc != dc {
        return None;
    }
    let keep_codes = !fc.is_empty() && !dc.is_empty();
    let file: Vec<String> = file
        .iter()
        .filter(|t| keep_codes || !code(t))
        .cloned()
        .collect();
    let db: Vec<String> = db
        .iter()
        .filter(|t| keep_codes || !code(t))
        .cloned()
        .collect();
    let (file, db) = (&file[..], &db[..]);
    let numbers = |t: &[String]| {
        t.iter()
            .filter(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && !code(t))
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
    if !fn_.iter().all(|n| dn.iter().any(|m| same_number(n, m)))
        || !dn.iter().all(|n| fn_.iter().any(|m| same_number(n, m)))
    {
        return None;
    }
    let common = file.iter().filter(|t| db.contains(t)).count();
    let union = file.len() + db.len() - common;
    if union == 0 {
        return None;
    }
    // A name the same but for a code one side leaves out is a hair
    // short of one the same outright, so the entry that is the name wins
    // a tie with one that only resembles it.
    let ignored = !keep_codes && (!fc.is_empty() || !dc.is_empty());
    let s = common as f32 / union as f32;
    if ignored && s < 0.6 {
        return None;
    }
    Some(s - if ignored { 0.001 } else { 0.0 })
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
        let named = without_maker(tokens(model), make);
        let mut best: Option<(f32, &Camera)> = None;
        for c in &self.cameras {
            if !c.makers().any(|m| same_maker(m, make)) {
                continue;
            }
            // Each of the body's maker names dropped from both sides:
            // the EXIF's "PENTAX K-1" from "RICOH IMAGING COMPANY,
            // LTD." is the database's "Pentax K-1" filed under Ricoh.
            let file = without_makers(named.clone(), c.makers());
            for m in &c.models {
                let db = without_makers(tokens(m), c.makers());
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
    /// mounts its calibrators had. Only then: a native lens spelled a
    /// little differently is nearer than another design off the mount
    /// that spells it alike. On either rung a name whose mount or
    /// format word contradicts the file's is not taken (see
    /// [`contradicts`]), nor another maker's lens for a file naming a
    /// line of its own maker's (see [`strays`]): a wrong correction is
    /// worse than none, and the picker still offers either by hand.
    pub fn lens(
        &self,
        camera: Option<&Camera>,
        lens_make: Option<&str>,
        lens_model: &str,
        focal: Option<f32>,
    ) -> Option<&Lens> {
        let mounts = camera.map(|c| self.mounts_taken(&c.mount));
        let crop = camera.map(|c| c.crop_factor);
        self.lens_for(mounts.as_deref(), crop, lens_make, lens_model, focal)
            .or_else(|| {
                let c = camera?;
                let found = self.lens_for(None, crop, lens_make, lens_model, focal);
                if let Some(l) = found {
                    log::debug!(
                        "lens {lens_model}: none on {}, {} taken off the mount",
                        c.mount,
                        l.name()
                    );
                }
                found
            })
    }

    /// The best match for the name among the lenses on `mounts` (any
    /// mount when none). Of two entries of one name, the calibration
    /// for the body at `crop` (see [`better_for`]); of two names, the
    /// first.
    fn lens_for(
        &self,
        mounts: Option<&[&str]>,
        crop: Option<f32>,
        lens_make: Option<&str>,
        lens_model: &str,
        focal: Option<f32>,
    ) -> Option<&Lens> {
        let named = tokens(lens_model);
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
            // The database's first maker name alone: another of its
            // names dropped from both sides is a word the two no longer
            // share, and a lens of another maker's then ties with the
            // right one ("Meike 85mm f/1.8" against "Meike SL 85mm F1.8
            // STM PRO", filed under "Meke").
            let file = without_maker(named.clone(), &l.maker);
            for m in &l.models {
                let db = without_maker(tokens(m), &l.maker);
                if contradicts(&file, &db, &l.maker) || strays(&named, lens_make, &l.maker, &db) {
                    continue;
                }
                let Some(mut s) = score(&file, &db) else {
                    continue;
                };
                if lens_make.is_some_and(|mk| l.makers().any(|m| same_maker(m, mk))) {
                    s += 0.05;
                }
                let better = match best {
                    None => true,
                    Some((b, o)) => {
                        s > b
                            || (s == b
                                && o.name() == l.name()
                                && crop.is_some_and(|c| better_for(l, o, c)))
                    }
                };
                if better {
                    best = Some((s, l));
                }
            }
        }
        best.filter(|(s, _)| *s >= 0.4).map(|(_, l)| l)
    }
}

/// Whether a lens's calibration covers a body at `crop`: made on a
/// sensor as large as the body's or larger, a twentieth over allowed
/// for the database's rounding. lensfun's own lookup takes no lens
/// calibrated on a smaller sensor than the camera's, "since the
/// mathematical models of the lens can be incorrect for sensor sizes
/// larger than the one used for calibration" (`lfDatabase::FindLenses`).
fn covers(lens: &Lens, crop: f32) -> bool {
    lens.crop_factor <= crop * 1.05
}

/// Whether `a` is the better of two calibrations of one lens for a
/// body at `crop`: one that [`covers`] the body over one that does
/// not, then the one nearer its sensor size. We only prefer here and
/// never refuse: the database calibrates many full-frame lenses on crop
/// bodies alone, and that calibration is the lens's on any body.
fn better_for(a: &Lens, b: &Lens, crop: f32) -> bool {
    match (covers(a, crop), covers(b, crop)) {
        (true, false) => true,
        (false, true) => false,
        _ => nearer(a, b, crop),
    }
}

/// A maker's lines of lenses, each as the words that name it.
type Lines = &'static [&'static [&'static str]];

/// The makers whose lens names say their mount or format in a word,
/// the brands a name of theirs may begin with, and the words, one slice
/// a line of lenses, as tokens: Leica's M, TL (once T), SL, R, and S;
/// Canon's RF, EF, and FD; Nikon's Z and its F lenses' AF and AI;
/// Sony's FE, E, and DT.
const LINES: &[(&str, &[&str], Lines)] = &[
    (
        "leica",
        &["leica"],
        &[&["m"], &["tl", "t"], &["sl"], &["r"], &["s"]],
    ),
    ("canon", &["canon"], &[&["rf"], &["ef"], &["fd", "fdn"]]),
    ("nikon", &["nikon", "nikkor"], &[&["z"], &["af", "ai"]]),
    ("sony", &["sony"], &[&["fe"], &["e"], &["dt"]]),
];

/// The lines of `maker`'s, when it is one of [`LINES`].
fn lines_of(maker: &str) -> Option<(&'static str, Lines)> {
    let first = tokens(maker).into_iter().next()?;
    LINES
        .iter()
        .find(|(m, _, _)| *m == first)
        .map(|(m, _, lines)| (*m, *lines))
}

/// Whether a name of `maker`'s, `db` as tokens, strays from a file
/// whose name, `named` as tokens, is of a line of another maker's: the
/// file's lens make is that maker, or the name begins with its brand
/// ("NIKKOR Z"), and the candidate does not carry the line's word, nor
/// the brand when the name begins with it (a third party's "Z" says
/// its mount, not that it is a Nikkor). With its own maker's sibling
/// refused (see [`contradicts`]) or absent, a 50 mm f/1.8 of anyone's
/// would otherwise do.
fn strays(named: &[String], lens_make: Option<&str>, maker: &str, db: &[String]) -> bool {
    let brand = named.first().and_then(|first| {
        LINES
            .iter()
            .find(|(_, brands, _)| brands.contains(&first.as_str()))
            .map(|(m, _, lines)| (*m, *lines, first.as_str()))
    });
    let own = lens_make
        .and_then(lines_of)
        .or(brand.map(|(m, lines, _)| (m, lines)));
    let Some((own, lines)) = own else {
        return false;
    };
    if lines_of(maker).is_some_and(|(m, _)| m == own) {
        return false;
    }
    let on = |name: &[String], line: &[&str]| name.iter().any(|t| line.contains(&t.as_str()));
    let fired: Vec<&&[&str]> = lines.iter().filter(|line| on(named, line)).collect();
    if fired.is_empty() {
        return false;
    }
    // A word of one letter in another maker's name is its own
    // business (Pentax's "Pentax-M"), not the line's.
    let carries = |line: &[&str]| db.iter().any(|t| t.len() > 1 && line.contains(&t.as_str()));
    fired.iter().any(|line| !carries(line))
        || brand.is_some_and(|(m, _, b)| m == own && !on(db, &[b]))
}

/// Whether a database name of `maker`'s is for another line of lenses
/// than the file's name says: each carries one line's word and not the
/// other's ("Summilux-M" against "Summilux-TL", "FE" against "E"), or
/// the file's carries another maker's line in a word of two letters or
/// more, which no maker shares ("FE" against Canon's "RF"). A name with
/// no such word contradicts nothing.
fn contradicts(file: &[String], db: &[String], maker: &str) -> bool {
    let Some((first, lines)) = lines_of(maker) else {
        return false;
    };
    let on = |name: &[String], line: &[&str]| name.iter().any(|t| line.contains(&t.as_str()));
    let own = lines.iter().enumerate().any(|(i, a)| {
        on(file, a)
            && !on(db, a)
            && lines
                .iter()
                .enumerate()
                .any(|(j, b)| i != j && on(db, b) && !on(file, b))
    });
    let foreign = || {
        LINES
            .iter()
            .filter(|(m, _, _)| *m != first)
            .flat_map(|(_, _, ls)| ls.iter().flat_map(|l| l.iter()))
            .any(|w| {
                w.len() > 1
                    && on(file, &[w])
                    && !on(db, &[w])
                    && !lines.iter().any(|l| l.contains(w))
            })
            && lines.iter().any(|l| on(db, l))
    };
    own || foreign()
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
pub fn same_maker(a: &str, b: &str) -> bool {
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

    /// lensfun files Pentax's later bodies under "Ricoh Imaging Company,
    /// Ltd." and names the maker "Pentax" in English; rawler calls them
    /// "Pentax", their EXIF "RICOH IMAGING COMPANY, LTD.". Each finds
    /// the body, a mark or a variant more is another body, and a
    /// maker's other name lends no body to a third maker. The body then
    /// decides the calibration: a full-frame K-1 takes the lens's
    /// full-frame entry, a K-3 III its APS-C one.
    #[test]
    fn a_body_is_found_under_any_of_its_makers_names() {
        let body = |model: &str, lang: &str, mount: &str, crop: &str| {
            format!(
                "<camera><maker>Ricoh Imaging Company, Ltd.</maker>\
                 <maker lang=\"en\">Pentax</maker><model>Pentax {model}</model>\
                 <model lang=\"en\">{lang}</model><mount>{mount}</mount>\
                 <cropfactor>{crop}</cropfactor></camera>"
            )
        };
        let macro_100 = |mount: &str, crop: &str| {
            format!(
                "<lens><maker>Pentax</maker>\
                 <model>smc Pentax-D FA Macro 100mm f/2.8 WR</model>\
                 <mount>{mount}</mount><cropfactor>{crop}</cropfactor><calibration>\
                 <distortion model=\"poly3\" focal=\"100\" k1=\"0.001\"/>\
                 </calibration></lens>"
            )
        };
        let mut db = Database::default();
        db.read_xml(&format!(
            "<lensdatabase version=\"2\">\
             <mount><name>Pentax KAF2</name><compat>Pentax KAF</compat>\
             <compat>Pentax KAF4</compat></mount>\
             <mount><name>Pentax KAF4</name><compat>Pentax KAF</compat>\
             <compat>Pentax KAF2</compat></mount>\
             {}{}{}\
             <camera><maker>Ricoh Imaging Company, Ltd.</maker>\
             <maker lang=\"en\">Ricoh</maker><model>GR III</model>\
             <mount>ricohGRIII</mount><cropfactor>1.52</cropfactor></camera>\
             {}{}</lensdatabase>",
            body("K-3 Mark III", "K-3 III", "Pentax KAF2", "1.546"),
            body(
                "K-3 Mark III Monochrome",
                "K-3 III Monochrome",
                "Pentax KAF2",
                "1.546"
            ),
            body("K-1", "K-1", "Pentax KAF4", "1"),
            macro_100("Pentax KAF2", "1.53"),
            macro_100("Pentax KAF", "1"),
        ))
        .unwrap();
        let k3 = &db.cameras[0];
        assert_eq!(k3.maker_aliases, ["Pentax"]);
        assert_eq!(
            k3.makers().collect::<Vec<_>>(),
            ["Ricoh Imaging Company, Ltd.", "Pentax"]
        );
        let found = |make: &str, model: &str| db.camera(make, model).map(|c| c.models[0].as_str());
        for (make, model, body) in [
            ("Pentax", "K-3 Mark III", "Pentax K-3 Mark III"),
            (
                "RICOH IMAGING COMPANY, LTD.",
                "PENTAX K-3 Mark III",
                "Pentax K-3 Mark III",
            ),
            (
                "Pentax",
                "K-3 Mark III Monochrome",
                "Pentax K-3 Mark III Monochrome",
            ),
            ("Pentax", "K-1", "Pentax K-1"),
            ("RICOH IMAGING COMPANY, LTD.", "PENTAX K-1", "Pentax K-1"),
            ("Ricoh", "GR III", "GR III"),
        ] {
            assert_eq!(found(make, model), Some(body), "{make} {model}");
        }
        assert_eq!(found("Pentax", "K-1 Mark II"), None);
        assert_eq!(found("Pentax", "GR III"), None);
        assert_eq!(found("Nikon", "K-1"), None);

        let name = "smc PENTAX-D FA Macro 100mm F2.8 WR";
        let on = |model: &str| {
            let camera = db.camera("Pentax", model);
            db.lens(camera, Some("Pentax"), name, Some(100.0))
                .map(|l| l.crop_factor)
        };
        assert_eq!(on("K-1"), Some(1.0));
        assert_eq!(on("K-3 Mark III"), Some(1.53));
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

    /// A Leica M body and an L-mount APS-C lens of its name but not its
    /// glass, a medium-format and an APS-C body for adapted full-frame
    /// glass, and two lines of Sony's and Nikon's that share numbers.
    const FORMATS: &str = r#"<lensdatabase version="2">
        <mount><name>Leica M</name><compat>Generic</compat></mount>
        <mount><name>Leica L</name><compat>Leica M</compat></mount>
        <mount><name>Nikon Z</name><compat>Nikon F AF</compat></mount>
        <camera>
            <maker>Leica Camera AG</maker>
            <model>Leica M11</model>
            <mount>Leica M</mount>
            <cropfactor>1</cropfactor>
        </camera>
        <camera>
            <maker>Leica Camera AG</maker>
            <model>Leica CL</model>
            <mount>Leica L</mount>
            <cropfactor>1.53</cropfactor>
        </camera>
        <camera>
            <maker>Fujifilm</maker>
            <model>GFX 100S</model>
            <mount>Fujifilm G</mount>
            <cropfactor>0.79</cropfactor>
        </camera>
        <camera>
            <maker>Fujifilm</maker>
            <model>X-T5</model>
            <mount>Fujifilm X</mount>
            <cropfactor>1.53</cropfactor>
        </camera>
        <camera>
            <maker>Nikon</maker>
            <model>Nikon Z 6</model>
            <mount>Nikon Z</mount>
            <cropfactor>1</cropfactor>
        </camera>
        <lens>
            <maker>Leica</maker>
            <model>Summilux-TL 1:1.4/35 Asph.</model>
            <mount>Leica L</mount>
            <cropfactor>1.53</cropfactor>
            <calibration><distortion model="ptlens" focal="35" a="0.006" b="-0.017" c="0.003"/></calibration>
        </lens>
        <lens>
            <maker>Leica</maker>
            <model>Summicron-TL 1:2/23 Asph.</model>
            <mount>Leica L</mount>
            <cropfactor>1.53</cropfactor>
            <calibration><distortion model="poly3" focal="23" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 50mm f/1.4 DG HSM [A]</model>
            <mount>Canon EF</mount>
            <cropfactor>1.005</cropfactor>
            <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Sony</maker>
            <model>E 50mm f/1.8 OSS</model>
            <mount>Sony E</mount>
            <cropfactor>1.534</cropfactor>
            <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Nikon</maker>
            <model>Nikon AF-S Nikkor 35mm f/1.8G ED</model>
            <mount>Nikon F AF</mount>
            <cropfactor>1</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Nikon</maker>
            <model>Nikkor Z 50mm f/1.8 S</model>
            <mount>Nikon Z</mount>
            <cropfactor>1</cropfactor>
            <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration>
        </lens>
    </lensdatabase>"#;

    fn formats() -> Database {
        let mut db = db();
        db.read_xml(ADAPTED).unwrap();
        db.read_xml(FORMATS).unwrap();
        db
    }

    #[test]
    fn an_m_lens_is_not_the_tl_lens_of_its_name() {
        let db = formats();
        let m11 = db.camera("Leica Camera AG", "LEICA M11");
        assert!(m11.is_some());
        // The M lens the database lacks is not the TL lens of its
        // name: no profile, not the TL's, with the lens make or not.
        for make in [Some("Leica Camera AG"), None] {
            assert!(
                db.lens(m11, make, "Summilux-M 1:1.4/35 ASPH.", Some(35.0))
                    .is_none()
            );
        }
        // A name with no line's word says nothing against the TL: it is
        // taken, as it was before the lines were read.
        assert_eq!(
            db.lens(m11, None, "Summilux 1:1.4/35 ASPH.", Some(35.0))
                .unwrap()
                .name(),
            "Summilux-TL 1:1.4/35 Asph."
        );
        // On an APS-C L body the TL lens is its own.
        let cl = db.camera("Leica Camera AG", "LEICA CL");
        assert_eq!(
            db.lens(cl, None, "Summilux-TL 1:1.4/35 ASPH.", Some(35.0))
                .unwrap()
                .name(),
            "Summilux-TL 1:1.4/35 Asph."
        );
    }

    #[test]
    fn full_frame_glass_on_an_adapter_is_still_found() {
        let db = formats();
        let name = "SIGMA 50mm F1.4 DG HSM A014";
        // On medium format, as §142 holds its vignetting at the edge.
        let gfx = db.camera("Fujifilm", "GFX 100S");
        assert_eq!(
            db.lens(gfx, Some("SIGMA"), name, Some(50.0))
                .unwrap()
                .crop_factor,
            1.005
        );
        // On APS-C: a larger image circle covers a smaller sensor.
        let xt5 = db.camera("Fujifilm", "X-T5");
        assert!(db.lens(xt5, Some("SIGMA"), name, Some(50.0)).is_some());
    }

    #[test]
    fn of_one_name_on_two_mounts_the_calibration_that_covers_is_taken() {
        let db = formats();
        let name = "35mm F1.4 DG HSM | Art 012";
        // The database lists the Nikon F entry at 1.5 first; a
        // full-frame body off both mounts takes the full-frame one.
        let a7 = db.camera("Sony", "ILCE-7M4");
        let l = db.lens(a7, Some("SIGMA"), name, Some(35.0)).unwrap();
        assert_eq!((l.crop_factor, &l.mounts[0][..]), (1.0, "Canon EF"));
        // An APS-C body takes the entry nearest its own sensor.
        let a6400 = db.camera("Sony", "ILCE-6400");
        let l = db.lens(a6400, Some("SIGMA"), name, Some(35.0)).unwrap();
        assert_eq!(l.crop_factor, 1.5);
    }

    /// An APS-H body, a Sigma calibrated on APS-C (listed first) and on
    /// full frame, and an F lens a Z body takes beside its own.
    const TIES: &str = r#"<lensdatabase version="2">
        <camera>
            <maker>Canon</maker>
            <model>Canon EOS-1D Mark II</model>
            <mount>Canon EF</mount>
            <cropfactor>1.255</cropfactor>
        </camera>
        <camera>
            <maker>Nikon</maker>
            <model>Nikon Z 30</model>
            <mount>Nikon Z</mount>
            <cropfactor>1.531</cropfactor>
        </camera>
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 24mm F1.4 DG HSM | A</model>
            <mount>Canon EF</mount>
            <cropfactor>1.534</cropfactor>
            <calibration><distortion model="poly3" focal="24" k1="-0.03"/></calibration>
        </lens>
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 24mm F1.4 DG HSM | A</model>
            <mount>Canon EF</mount>
            <cropfactor>1.0</cropfactor>
            <calibration><distortion model="poly3" focal="24" k1="-0.02"/></calibration>
        </lens>
        <lens>
            <maker>Nikon</maker>
            <model>Nikon AF Nikkor 50mm f/1.8D</model>
            <mount>Nikon F AF</mount>
            <cropfactor>1.528</cropfactor>
            <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration>
        </lens>
    </lensdatabase>"#;

    #[test]
    fn of_one_name_the_calibration_that_covers_the_body_is_taken() {
        let mut db = formats();
        db.read_xml(TIES).unwrap();
        // Nearer by ratio is the APS-C entry, listed first, whose
        // corners stop short of the APS-H frame's; the full-frame
        // entry covers it.
        let one_d = db.camera("Canon", "EOS-1D Mark II");
        let l = db
            .lens(
                one_d,
                Some("SIGMA"),
                "24mm F1.4 DG HSM | Art 015",
                Some(24.0),
            )
            .unwrap();
        assert_eq!(l.crop_factor, 1.0);
        // Two names that tie keep the database's order, whatever their
        // calibrations.
        let z30 = db.camera("Nikon", "NIKON Z 30");
        assert_eq!(
            db.lens(z30, Some("VILTROX"), "Viltrox AF 50mm f/1.8 Z", Some(50.0))
                .unwrap()
                .name(),
            "Nikkor Z 50mm f/1.8 S"
        );
    }

    /// Lenses of other makers that share a first-party lens's numbers:
    /// what a file naming one the database lacks must not take.
    const STRANGERS: &str = r#"<lensdatabase version="2">
        <mount><name>Canon RF</name><compat>M42</compat></mount>
        <camera>
            <maker>Canon</maker>
            <model>Canon EOS R</model>
            <mount>Canon RF</mount>
            <cropfactor>1</cropfactor>
        </camera>
        <camera>
            <maker>Nikon</maker>
            <model>Nikon Z 5</model>
            <mount>Nikon Z</mount>
            <cropfactor>1</cropfactor>
        </camera>
        <camera>
            <maker>Leica Camera AG</maker>
            <model>Leica T</model>
            <mount>Leica L</mount>
            <cropfactor>1.53</cropfactor>
        </camera>
        <lens>
            <maker>Pentacon</maker>
            <model>Pentacon 50mm f/1.8</model>
            <mount>M42</mount>
            <cropfactor>1</cropfactor>
            <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Sigma</maker>
            <model>70-200mm F2.8 DG OS HSM | Sports 018</model>
            <mount>Canon EF</mount>
            <mount>Nikon F AF</mount>
            <cropfactor>1</cropfactor>
            <calibration><distortion model="poly3" focal="70" k1="-0.01"/><distortion model="poly3" focal="200" k1="0.01"/></calibration>
        </lens>
        <lens>
            <maker>Pentax</maker>
            <model>smc Pentax-M 35mm 1:2</model>
            <mount>Pentax K</mount>
            <cropfactor>1.538</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Fujifilm</maker>
            <model>XF35mmF2 R WR</model>
            <mount>Fujifilm X</mount>
            <cropfactor>1.528</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Voigtländer</maker>
            <model>35mm f/2</model>
            <mount>Nikon Z</mount>
            <cropfactor>1</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>Cosina</maker>
            <model>Cosina Cosinon-S 50mm 1:2</model>
            <mount>Pentax K</mount>
            <cropfactor>1.538</cropfactor>
            <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration>
        </lens>
    </lensdatabase>"#;

    #[test]
    fn a_first_party_lens_the_database_lacks_is_no_strangers() {
        let mut db = Database::default();
        db.read_xml(FORMATS).unwrap();
        db.read_xml(STRANGERS).unwrap();
        let canon = db.camera("Canon", "Canon EOS R");
        assert!(
            db.lens(canon, Some("Canon"), "RF50mm F1.8 STM", Some(50.0))
                .is_none()
        );
        assert!(
            db.lens(canon, None, "Canon RF 50mm F1.8 STM", Some(50.0))
                .is_none()
        );
        let z5 = db.camera("Nikon", "NIKON Z 5");
        assert!(
            db.lens(z5, None, "NIKKOR Z 70-200mm f/2.8 VR S", Some(70.0))
                .is_none()
        );
        let t = db.camera("Leica Camera AG", "LEICA T");
        assert!(
            db.lens(t, Some("Leica Camera AG"), "Summicron-M 1:2/50", Some(50.0))
                .is_none()
        );
        // Pentax's "M" is not Leica's.
        assert!(
            db.lens(
                t,
                Some("Leica Camera AG"),
                "Summicron-M 1:2/35 ASPH.",
                Some(35.0)
            )
            .is_none()
        );
        // A stranger off the mount sharing a few words does not displace
        // a lens on it sharing a few too.
        let xt5 = db.camera("Fujifilm", "X-T5");
        assert_eq!(
            db.lens(xt5, Some("FUJIFILM"), "XC35mmF2", Some(35.0))
                .unwrap()
                .name(),
            "XF35mmF2 R WR"
        );
        // A third party's own name still finds its lens on any body.
        assert_eq!(
            db.lens(z5, Some("SIGMA"), "70-200mm F2.8 DG OS HSM | S", Some(70.0))
                .unwrap()
                .name(),
            "70-200mm F2.8 DG OS HSM | Sports 018"
        );
    }

    #[test]
    fn a_name_of_another_line_is_not_taken_on_the_mount() {
        let db = formats();
        // Sony's FE and E share the mount; an FE lens the database
        // lacks is not the APS-C E lens of its numbers, nor Canon's RF
        // lens of them off the mount.
        let a7 = db.camera("Sony", "ILCE-7M4");
        assert!(db.lens(a7, None, "FE 50mm F1.8", Some(50.0)).is_none());
        let a6400 = db.camera("Sony", "ILCE-6400");
        assert_eq!(
            db.lens(a6400, None, "E 50mm F1.8 OSS", Some(50.0))
                .unwrap()
                .name(),
            "E 50mm f/1.8 OSS"
        );
        // A Z lens is not an F lens the Z body takes on its adapter,
        // and a Z lens the database has is found.
        let z6 = db.camera("Nikon", "NIKON Z 6");
        assert!(
            db.lens(z6, Some("Nikon"), "NIKKOR Z 35mm f/1.8 S", Some(35.0))
                .is_none()
        );
        assert_eq!(
            db.lens(z6, Some("Nikon"), "NIKKOR Z 50mm f/1.8 S", Some(50.0))
                .unwrap()
                .name(),
            "Nikkor Z 50mm f/1.8 S"
        );
        assert_eq!(
            db.lens(z6, Some("Nikon"), "AF-S NIKKOR 35mm f/1.8G ED", Some(35.0))
                .unwrap()
                .name(),
            "Nikon AF-S Nikkor 35mm f/1.8G ED"
        );
    }

    #[test]
    fn mount_words_contradict_only_across_lines() {
        let c = |file: &str, db: &str, maker: &str| {
            contradicts(
                &without_maker(tokens(file), maker),
                &without_maker(tokens(db), maker),
                maker,
            )
        };
        assert!(c(
            "Summilux-M 1:1.4/35 ASPH.",
            "Summilux-TL 1:1.4/35 Asph.",
            "Leica"
        ));
        // T is what Leica called the TL line first.
        assert!(!c(
            "Summicron-T 1:2/23 ASPH.",
            "Summicron-TL 1:2/23 Asph.",
            "Leica"
        ));
        assert!(!c(
            "Summilux 1:1.4/35",
            "Summilux-TL 1:1.4/35 Asph.",
            "Leica"
        ));
        assert!(c(
            "RF24-105mm F4 L IS USM",
            "Canon EF 24-105mm f/4L IS USM",
            "Canon"
        ));
        assert!(!c(
            "EF-S10-18mm f/4.5-5.6 IS STM",
            "Canon EF-S 10-18mm f/4.5-5.6 IS STM",
            "Canon"
        ));
        assert!(c("FE 50mm F1.8", "E 50mm f/1.8 OSS", "Sony"));
        assert!(c("FE 50mm F1.8", "Canon RF 50mm F1.8 STM", "Canon"));
        assert!(c("EF28mm f/2.8", "Canon FDn 28mm f/2.8", "Canon"));
        // Another maker's word the candidate shares is no contradiction.
        assert!(!c(
            "AF DT 50mm F1.8 SAM",
            "Sony AF DT 50mm F1.8 SAM",
            "Sony"
        ));
        // A maker with no lines, and Canon's L, are left alone.
        assert!(!c(
            "28mm F1.4 DG HSM | Art 019",
            "Sigma 28mm F1.4 DG HSM | A",
            "Sigma"
        ));
        assert!(!c("RF50mm F1.2 L USM", "Canon RF 50mm F1.2 L USM", "Canon"));
    }

    /// Sigma's two 24-70 DG DN, their year codes in the database's
    /// names, and a 24 mm prime; Tamron's two 70-180 by model code; an
    /// L body, and Sigma's two 85 mm f/1.4 on and off its mount.
    const YEARS: &str = r#"<lensdatabase version="2">
        <lens>
            <maker>Sigma</maker>
            <model>24-70mm F2.8 DG DN | Art 019</model>
            <mount>Sony E</mount>
            <cropfactor>1.0</cropfactor>
            <calibration><distortion model="ptlens" focal="24" a="0.04" b="-0.12" c="0.064"/><distortion model="ptlens" focal="70" a="0" b="0" c="0"/></calibration>
        </lens>
        <lens>
            <maker>Sigma</maker>
            <model>24-70mm F2.8 DG DN II | Art 024</model>
            <mount>Sony E</mount>
            <cropfactor>1.0</cropfactor>
            <calibration><distortion model="ptlens" focal="24" a="0.026" b="-0.066" c="0.007"/><distortion model="ptlens" focal="70" a="0" b="0" c="0"/></calibration>
        </lens>
        <lens>
            <maker>Sony</maker>
            <model>FE 24mm f/1.4 GM</model>
            <mount>Sony E</mount>
            <cropfactor>1.0</cropfactor>
            <calibration><distortion model="poly3" focal="24" k1="-0.01"/></calibration>
        </lens>
        <camera>
            <maker>Leica Camera AG</maker>
            <model>Leica SL (Typ 601)</model>
            <mount>Leica L</mount>
            <cropfactor>1</cropfactor>
        </camera>
        <lens>
            <maker>Sigma</maker>
            <model>85mm F1.4 DG DN | Art 020</model>
            <mount>Sony E</mount>
            <mount>Leica L</mount>
            <cropfactor>1</cropfactor>
            <calibration><distortion model="poly3" focal="85" k1="0.01"/></calibration>
        </lens>
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 85mm f/1.4 EX DG HSM</model>
            <mount>Canon EF</mount>
            <cropfactor>1.534</cropfactor>
            <calibration><distortion model="poly3" focal="85" k1="0.02"/></calibration>
        </lens>
        <lens>
            <maker>Tamron</maker>
            <model>E 70-180mm F2.8 A056</model>
            <mount>Sony E</mount>
            <cropfactor>1.0</cropfactor>
            <calibration><distortion model="ptlens" focal="70" a="0.001" b="-0.008" c="0.002"/><distortion model="ptlens" focal="180" a="0" b="0" c="0"/></calibration>
        </lens>
        <lens>
            <maker>Tamron</maker>
            <model>E 70-180mm F2.8 A065</model>
            <mount>Sony E</mount>
            <cropfactor>1.534</cropfactor>
            <calibration><distortion model="ptlens" focal="70" a="0.002" b="-0.006" c="0.001"/><distortion model="ptlens" focal="180" a="0" b="0" c="0"/></calibration>
        </lens>
    </lensdatabase>"#;

    #[test]
    fn a_year_code_in_the_databases_name_is_no_focal_length() {
        let mut db = formats();
        db.read_xml(YEARS).unwrap();
        let a7 = db.camera("Sony", "ILCE-7M4");
        let name = |file: &str, focal: f32| db.lens(a7, None, file, Some(focal)).map(Lens::name);
        // The Mark I's "019" is not held against a file that leaves it
        // out, and the Mark II's "024" is not the 24 of the zoom.
        assert_eq!(
            name("24-70mm F2.8 DG DN | A", 35.0),
            Some("24-70mm F2.8 DG DN | Art 019")
        );
        assert_eq!(
            name("24-70mm F2.8 DG DN II | Art 024", 35.0),
            Some("24-70mm F2.8 DG DN II | Art 024")
        );
        // A file's code the database's name lacks is not held against it
        // either; one it has must agree.
        assert_eq!(
            name("24-70mm F2.8 DG DN | Art 019", 35.0),
            Some("24-70mm F2.8 DG DN | Art 019")
        );
        // A 24 that is a focal length is still one.
        assert_eq!(name("FE 24mm F1.4 GM", 24.0), Some("FE 24mm f/1.4 GM"));
    }

    /// The Art's code no longer lets a name sharing only its numbers
    /// and three words through: the L mount has nothing for the EX, so
    /// the EX is found off it.
    #[test]
    fn a_lens_of_its_own_name_off_the_mount_beats_a_sibling_on_it() {
        let mut db = formats();
        db.read_xml(YEARS).unwrap();
        // On an L body through an adapter, the EF lens the file names,
        // not the L-mount Art that shares its numbers and three words.
        let sl = db.camera("Leica Camera AG", "LEICA SL (Typ 601)");
        assert_eq!(
            db.lens(sl, Some("SIGMA"), "85mm F1.4 EX DG HSM", Some(85.0))
                .unwrap()
                .name(),
            "Sigma 85mm f/1.4 EX DG HSM"
        );
        // The Art itself is still the Art.
        assert_eq!(
            db.lens(sl, Some("SIGMA"), "85mm F1.4 DG DN | A", Some(85.0))
                .unwrap()
                .name(),
            "85mm F1.4 DG DN | Art 020"
        );
    }

    #[test]
    fn another_design_sharing_a_lens_numbers_is_none() {
        let mut db = formats();
        db.read_xml(YEARS).unwrap();
        // Without the EX in the database, the Art's 0.43 is not enough
        // once its code is left out: no profile.
        db.lenses
            .retain(|l| l.name() != "Sigma 85mm f/1.4 EX DG HSM");
        let sl = db.camera("Leica Camera AG", "LEICA SL (Typ 601)");
        assert!(
            db.lens(sl, Some("SIGMA"), "85mm F1.4 EX DG HSM", Some(85.0))
                .is_none()
        );
        // "| A" still finds the Art of its numbers.
        let a7 = db.camera("Sony", "ILCE-7M4");
        assert_eq!(
            db.lens(a7, Some("SIGMA"), "24-70mm F2.8 DG DN | A", Some(35.0))
                .unwrap()
                .name(),
            "24-70mm F2.8 DG DN | Art 019"
        );
    }

    /// A Micro Four Thirds body with its own 14-42 and the Four Thirds
    /// one of the same words but another design, and a Z body with a
    /// third party's 35 mm f/1.7 on its mount and a C-mount one off it.
    const NATIVE: &str = r#"<lensdatabase version="2">
        <camera>
            <maker>Olympus</maker>
            <model>Olympus E-M10</model>
            <mount>Micro 4/3 System</mount>
            <cropfactor>2</cropfactor>
        </camera>
        <lens>
            <maker>Olympus</maker>
            <model>Olympus M.Zuiko Digital ED 14-42mm f/3.5-5.6</model>
            <mount>Micro 4/3 System</mount>
            <cropfactor>2</cropfactor>
            <calibration><distortion model="poly3" focal="14" k1="-0.03"/><distortion model="poly3" focal="42" k1="0.01"/></calibration>
        </lens>
        <lens>
            <maker>Olympus</maker>
            <model>Olympus Zuiko Digital ED 14-42mm f/3.5-5.6</model>
            <mount>4/3 System</mount>
            <cropfactor>2</cropfactor>
            <calibration><distortion model="poly3" focal="14" k1="-0.02"/><distortion model="poly3" focal="42" k1="0.01"/></calibration>
        </lens>
        <lens>
            <maker>Viltrox</maker>
            <model>Viltrox 35mm F1.7 E</model>
            <mount>Sony E</mount>
            <mount>Nikon Z</mount>
            <cropfactor>1.534</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.01"/></calibration>
        </lens>
        <lens>
            <maker>7Artisans</maker>
            <model>35mm f/1.7</model>
            <mount>C</mount>
            <cropfactor>2</cropfactor>
            <calibration><distortion model="poly3" focal="35" k1="-0.02"/></calibration>
        </lens>
    </lensdatabase>"#;

    #[test]
    fn a_native_lens_spelled_otherwise_is_not_left_for_one_off_the_mount() {
        let mut db = formats();
        db.read_xml(NATIVE).unwrap();
        let em10 = db.camera("OLYMPUS IMAGING CORP.", "E-M10");
        assert_eq!(
            db.lens(
                em10,
                None,
                "Olympus Zuiko Digital ED 14-42 3.5-5.6",
                Some(14.0)
            )
            .unwrap()
            .name(),
            "Olympus M.Zuiko Digital ED 14-42mm f/3.5-5.6"
        );
        let z6 = db.camera("Nikon", "NIKON Z 6");
        assert_eq!(
            db.lens(z6, None, "35mm f/1.7", Some(35.0)).unwrap().name(),
            "Viltrox 35mm F1.7 E"
        );
    }

    /// Sigma's 17-70 of 2010, whose second name the database gave the
    /// 2013 lens's words and code, and the 2013 lens.
    const TWO_SEVENTIES: &str = r#"<lensdatabase version="2">
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 17-70mm f/2.8-4 DC Macro OS HSM</model>
            <model lang="en">Sigma 17-70mm f/2.8-4 DC Macro OS HSM | Contemporary 013</model>
            <mount>Canon EF</mount>
            <cropfactor>1.523</cropfactor>
            <calibration><distortion model="poly3" focal="17" k1="-0.03"/><distortion model="poly3" focal="70" k1="0.01"/></calibration>
        </lens>
        <lens>
            <maker>Sigma</maker>
            <model>Sigma 17-70mm f/2.8-4 DC MACRO OS HSM Contemporary</model>
            <mount>Canon EF</mount>
            <cropfactor>1.53</cropfactor>
            <calibration><distortion model="poly3" focal="17" k1="-0.02"/><distortion model="poly3" focal="70" k1="0.01"/></calibration>
        </lens>
    </lensdatabase>"#;

    #[test]
    fn a_name_the_same_outright_beats_one_the_same_but_for_a_code() {
        let mut db = db();
        db.read_xml(TWO_SEVENTIES).unwrap();
        let r7 = db.camera("Canon", "EOS R7");
        let name = "Sigma 17-70mm f/2.8-4 DC MACRO OS HSM Contemporary";
        assert_eq!(
            db.lens(r7, Some("SIGMA"), name, Some(17.0)).unwrap().name(),
            name
        );
    }

    #[test]
    fn a_model_code_both_names_carry_must_agree() {
        let mut db = formats();
        db.read_xml(YEARS).unwrap();
        let name = |body: &str, file: &str| {
            let c = db.camera("Sony", body);
            db.lens(c, Some("TAMRON"), file, Some(100.0))
                .map(Lens::name)
        };
        // Tamron's G2, calibrated on APS-C, and the first, on full
        // frame: each body takes the one its file names.
        assert_eq!(
            name("ILCE-7M4", "E 70-180mm F2.8 A065"),
            Some("E 70-180mm F2.8 A065")
        );
        assert_eq!(
            name("ILCE-6400", "E 70-180mm F2.8 A056"),
            Some("E 70-180mm F2.8 A056")
        );
    }

    #[test]
    fn a_lens_refused_automatically_is_still_offered_by_hand() {
        let db = formats();
        let m11 = db.camera("Leica Camera AG", "LEICA M11").unwrap();
        let tl = "Summilux-TL 1:1.4/35 Asph.";
        assert!(db.lenses_off_mount(m11).iter().any(|l| l.name() == tl));
        assert_eq!(db.lens_named(Some(m11), tl).unwrap().name(), tl);
    }
}
