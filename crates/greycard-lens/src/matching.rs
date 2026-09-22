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

impl Database {
    /// The camera a file's make and model name, if one does.
    pub fn camera(&self, make: &str, model: &str) -> Option<&Camera> {
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
}
