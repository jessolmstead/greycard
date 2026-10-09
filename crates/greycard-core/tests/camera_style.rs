//! The camera style reader against exiv2, on real raws.
//!
//! Runs over every raw in `GREYCARD_SAMPLES` when that is set, and over
//! the raws listed in the files named by `GREYCARD_STYLE_FRAMES` (one
//! path a line, list files separated by `:`). Skips with a note when
//! neither is set or `exiv2` is missing; fails when frame lists were
//! named and exiv2 is missing, or when there were files and none was
//! compared. The expected values come from exiv2's own names for the
//! settings (`-Pkt`), not from the reader's rules.
//!
//! Every field compared is tallied per body, and the table of files and
//! agreements is printed before the test fails on the first
//! disagreement, so one run says how far the reader and exiv2 agree.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::process::Command;

use greycard_core::decode::{CameraStyle, Maker, is_raw_path};

fn samples() -> Vec<PathBuf> {
    let mut files = Vec::new();
    match std::env::var_os("GREYCARD_SAMPLES") {
        Some(dir) => {
            let mut here: Vec<_> = std::fs::read_dir(&dir)
                .expect("read GREYCARD_SAMPLES")
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| is_raw_path(p))
                .collect();
            here.sort();
            files.extend(here);
        }
        None => println!("SKIPPED the sample folder: GREYCARD_SAMPLES is not set"),
    }
    for list in frame_lists() {
        let text = std::fs::read_to_string(&list).expect("read a GREYCARD_STYLE_FRAMES list");
        files.extend(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(PathBuf::from),
        );
    }
    files
}

fn frame_lists() -> Vec<String> {
    std::env::var("GREYCARD_STYLE_FRAMES")
        .map(|v| {
            v.split(':')
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// exiv2's keys and values for a file: `-Pkv` for the numbers, `-Pkt`
/// for its names of them.
fn exiv2(path: &Path, mode: &str) -> HashMap<String, String> {
    let out = Command::new("exiv2")
        .arg(mode)
        .arg(path)
        .output()
        .expect("run exiv2");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        map.entry(key.to_string())
            .or_insert_with(|| value.trim().to_string());
    }
    map
}

/// A setting as (on, known): what exiv2 can say of it. Its kind is the
/// reader's, not exiv2's, and is tested beside the reader.
fn find(style: &CameraStyle, name: &str) -> Option<(bool, bool)> {
    style
        .adaptive
        .iter()
        .find(|a| a.name == name)
        .map(|a| (a.on, a.known))
}

/// The setting as expected: `Some(on)` a named value, `None` one exiv2
/// does not name or the file does not carry.
fn expect(on: Option<bool>) -> Option<(bool, bool)> {
    Some(match on {
        Some(on) => (on, true),
        None => (true, false),
    })
}

/// A film or monochrome name with exiv2's `F1b/` codes and
/// parenthesized asides dropped, lowercased, so "PROVIA (F0/Standard)"
/// and "Provia" meet.
fn film(name: &str) -> String {
    let name = name.split(" (").next().unwrap_or(name);
    let name = match name.split_once('/') {
        Some((code, rest)) if code.starts_with('F') && code.len() <= 3 => rest,
        _ => name,
    };
    name.to_lowercase()
}

/// Per body and field, the files compared and the ones that agreed,
/// and every disagreement.
#[derive(Default)]
struct Tally {
    rows: BTreeMap<(String, &'static str), (usize, usize)>,
    wrong: Vec<String>,
}

/// One file's checks: the tally, the body and the file.
struct Check<'a> {
    tally: &'a mut Tally,
    body: String,
    path: &'a Path,
}

impl Check<'_> {
    fn eq<T: PartialEq + Debug>(&mut self, field: &'static str, got: T, want: T) {
        let row = self
            .tally
            .rows
            .entry((self.body.clone(), field))
            .or_default();
        row.0 += 1;
        if got == want {
            row.1 += 1;
        } else {
            self.tally.wrong.push(format!(
                "{} ({}): {field}: read {got:?}, exiv2 says {want:?}",
                self.path.display(),
                self.body
            ));
        }
    }
}

fn check_canon(c: &mut Check, style: &CameraStyle, kt: &HashMap<String, String>) {
    let expected = match kt.get("Exif.CanonPr.PictureStyle").map(String::as_str) {
        Some("None") | None => None,
        Some(name) => Some(name.to_string()),
    };
    c.eq("style", style.style.clone(), expected);
    let alo = match kt
        .get("Exif.CanonLiOp.AutoLightingOptimizer")
        .map(String::as_str)
    {
        Some("Off") => Some(false),
        Some("Standard" | "Low" | "Strong") => Some(true),
        _ => None,
    };
    c.eq("ALO", find(style, "Auto Lighting Optimizer"), expect(alo));
    let htp = match kt
        .get("Exif.CanonLiOp.HighlightTonePriority")
        .map(String::as_str)
    {
        Some("Off") => Some(false),
        Some("On") => Some(true),
        _ => None,
    };
    c.eq("HTP", find(style, "Highlight Tone Priority"), expect(htp));
    let peripheral = kt
        .get("Exif.CanonVigCor2.PeripheralLightingSetting")
        .or_else(|| kt.get("Exif.CanonLiOp.PeripheralIlluminationCorr"));
    let expected = match peripheral.map(String::as_str) {
        Some("On") => Some(true),
        Some("Off") => Some(false),
        _ => None,
    };
    c.eq("peripheral", style.peripheral_correction, expected);
}

fn check_fujifilm(
    c: &mut Check,
    style: &CameraStyle,
    kv: &HashMap<String, String>,
    kt: &HashMap<String, String>,
) {
    let mono = kt.get("Exif.Fujifilm.Color").filter(|c| {
        let c = c.to_lowercase();
        c.starts_with("monochrome") || c.starts_with("acros") || c.starts_with("sepia")
    });
    let expected = kt.get("Exif.Fujifilm.FilmMode").or(mono).map(|n| film(n));
    c.eq("film mode", style.style.as_deref().map(film), expected);

    let dr = match (
        kt.get("Exif.Fujifilm.DynamicRangeSetting")
            .map(String::as_str),
        kt.get("Exif.Fujifilm.DevelopmentDynamicRange")
            .map(String::as_str),
    ) {
        (Some("Auto"), _) => Some(true),
        (Some("Manual") | None, Some("100")) => Some(false),
        (Some("Manual") | None, Some("200" | "400")) => Some(true),
        (Some("Standard (100%)"), _) => Some(false),
        (Some(s), _) if s.starts_with("Wide mode") => Some(true),
        _ => None,
    };
    c.eq("dynamic range", find(style, "Dynamic Range"), expect(dr));
    let drp = match kt.get("Exif.Fujifilm.DRangePriority").map(String::as_str) {
        None => Some(false),
        Some("Auto" | "Fixed") => Some(true),
        Some(_) => None,
    };
    c.eq(
        "D-Range Priority",
        find(style, "D-Range Priority"),
        expect(drp),
    );
    for (key, name) in [
        ("Exif.Fujifilm.HighlightTone", "Highlight Tone"),
        ("Exif.Fujifilm.ShadowTone", "Shadow Tone"),
    ] {
        let moved = kv.get(key).is_some_and(|v| v != "0");
        c.eq(
            name,
            find(style, name),
            moved.then(|| expect(Some(true)).unwrap()),
        );
    }
}

/// exiv2's name for a value it has no name for: the number in
/// parentheses.
fn unnamed(text: &str) -> bool {
    text.starts_with('(') && text.ends_with(')')
}

fn check_nikon(c: &mut Check, style: &CameraStyle, kt: &HashMap<String, String>) {
    // exiv2 reads every Picture Control record with the first
    // version's offsets. The third version writes its version twice and
    // the name four bytes on, so exiv2's name there starts with the
    // second copy and its base is the empty bytes before the real one.
    let version = kt.get("Exif.NikonPc.Version").cloned().unwrap_or_default();
    let third = version.starts_with("3.");
    let name = kt.get("Exif.NikonPc.Name").map(|n| match third {
        true => n.get(4..).unwrap_or("").to_string(),
        false => n.clone(),
    });
    let base = kt.get("Exif.NikonPc.Base").cloned();
    c.eq(
        "Picture Control",
        style.style.as_ref().map(|s| s.to_uppercase()),
        name.clone().filter(|n| !n.is_empty()),
    );
    if !third {
        // A custom control's base; the style's own for a preset.
        let expected = match (&name, &base) {
            (Some(n), Some(b)) if n != b && !b.is_empty() => Some(b.clone()),
            _ => None,
        };
        c.eq(
            "Picture Control base",
            style.base.as_ref().map(|s| s.to_uppercase()),
            expected,
        );
        // A preset's base is its own name, so either way the control
        // is as fixed as its base, and Auto is not.
        let fixed = match (&name, &base) {
            (Some(n), Some(b)) => !n.is_empty() && !b.is_empty() && b != "AUTO",
            _ => false,
        };
        c.eq("style fixed", style.style_fixed, fixed);
        // exiv2 reads contrast and saturation where the first version
        // keeps them; the second keeps them elsewhere.
        if version.starts_with("1.") {
            let auto = ["Exif.NikonPc.Contrast", "Exif.NikonPc.Saturation"]
                .iter()
                .any(|k| kt.get(*k).is_some_and(|v| v == "Auto"));
            c.eq(
                "auto contrast or saturation",
                find(style, "Auto Contrast or Saturation"),
                expect(Some(auto)),
            );
        }
    } else if style.base.is_none() {
        // A preset: fixed unless it is Auto.
        c.eq(
            "style fixed",
            style.style_fixed,
            name.as_deref()
                .is_some_and(|n| !n.is_empty() && n != "AUTO"),
        );
    }
    let adl = match kt.get("Exif.Nikon3.ActiveDLighting").map(String::as_str) {
        Some("Off") => Some(false),
        Some("Low" | "Normal" | "High" | "Extra High" | "Auto") => Some(true),
        _ => None,
    };
    c.eq(
        "Active D-Lighting",
        find(style, "Active D-Lighting"),
        expect(adl),
    );
    for (key, name) in [
        ("Exif.Nikon3.SceneMode", "Scene Mode"),
        ("Exif.Nikon3.VariProgram", "Auto, Scene or Effects Program"),
    ] {
        let on = kt.get(key).is_some_and(|t| !t.trim().is_empty());
        c.eq(name, find(style, name), expect(Some(on)));
    }
    let vignette = match kt.get("Exif.Nikon3.VignetteControl").map(String::as_str) {
        Some("Off") => Some(false),
        Some("Low" | "Normal" | "High") => Some(true),
        _ => None,
    };
    c.eq("vignette control", style.peripheral_correction, vignette);
}

/// A Sony tag from whichever of exiv2's two groups the note is read
/// into.
fn sony<'a>(kt: &'a HashMap<String, String>, tag: &str) -> Option<&'a str> {
    kt.get(&format!("Exif.Sony2.{tag}"))
        .or_else(|| kt.get(&format!("Exif.Sony1.{tag}")))
        .map(String::as_str)
}

fn check_sony(c: &mut Check, style: &CameraStyle, kt: &HashMap<String, String>) {
    let (expected, fixed) = match sony(kt, "CreativeStyle") {
        None | Some("None") => (None, false),
        Some(t) if unnamed(t) => (Some(format!("Unknown ({})", &t[1..t.len() - 1])), false),
        Some(t) => (Some(t.to_string()), true),
    };
    c.eq("Creative Style", style.style.clone(), expected);
    c.eq("style fixed", style.style_fixed, fixed);
    let dro = match sony(kt, "DynamicRangeOptimizer") {
        Some("Off") => Some(false),
        Some(t) if !unnamed(t) => Some(true),
        _ => None,
    };
    c.eq("DRO", find(style, "Dynamic Range Optimizer"), expect(dro));
    let hdr = match sony(kt, "AutoHDR") {
        None => Some(false),
        Some(t) if t.starts_with("Off,") => Some(false),
        Some(t) if t.starts_with("Auto,") || t.contains(" EV,") => Some(true),
        _ => None,
    };
    c.eq("Auto HDR", find(style, "Auto HDR"), expect(hdr));
    let scene = match sony(kt, "SceneMode") {
        None | Some("Standard" | "Cont. Priority AE") => Some(false),
        Some(t) if t == "n/a" || unnamed(t) => None,
        Some(_) => Some(true),
    };
    c.eq("scene mode", find(style, "Scene Mode"), expect(scene));
    let ia = match sony(kt, "IntelligentAuto") {
        None | Some("Off") => Some(false),
        Some("On" | "Advanced") => Some(true),
        _ => None,
    };
    c.eq(
        "Intelligent Auto",
        find(style, "Intelligent Auto"),
        expect(ia),
    );
    let effect = match sony(kt, "PictureEffect") {
        None | Some("Off") => Some(false),
        Some(t) if unnamed(t) => None,
        Some(_) => Some(true),
    };
    c.eq(
        "Picture Effect",
        find(style, "Picture Effect"),
        expect(effect),
    );
    let vignetting = match sony(kt, "VignettingCorrection") {
        Some("Off") => Some(false),
        Some("Auto") => Some(true),
        _ => None,
    };
    c.eq(
        "vignetting correction",
        style.peripheral_correction,
        vignetting,
    );
}

fn check_panasonic(c: &mut Check, style: &CameraStyle, kt: &HashMap<String, String>) {
    let pana = |tag: &str| kt.get(&format!("Exif.Panasonic.{tag}")).map(String::as_str);
    // exiv2 names style 0 "NoAuto"; the reader calls it Auto.
    let (expected, fixed) = match pana("PhotoStyle") {
        None => (None, false),
        Some("NoAuto") => (Some("Auto".to_string()), false),
        Some(t) if unnamed(t) => (Some(format!("Unknown ({})", &t[1..t.len() - 1])), false),
        Some(t) => (Some(t.to_string()), true),
    };
    c.eq("Photo Style", style.style.clone(), expected);
    c.eq("style fixed", style.style_fixed, fixed);
    let level = |t: Option<&str>| match t {
        Some("Off") => Some(false),
        Some("Low" | "Standard" | "High") => Some(true),
        _ => None,
    };
    let idr = pana("IntelligentDRange");
    let iex = pana("IntelligentExposure");
    if idr.is_some() || iex.is_none() {
        c.eq(
            "Intelligent D-Range",
            find(style, "Intelligent D-Range"),
            expect(level(idr)),
        );
    }
    if iex.is_some() {
        c.eq(
            "Intelligent Exposure",
            find(style, "Intelligent Exposure"),
            expect(level(iex)),
        );
    }
    let hdr = match pana("HDR") {
        None | Some("Off") => Some(false),
        Some(t) if unnamed(t) => None,
        Some(_) => Some(true),
    };
    c.eq("HDR", find(style, "HDR"), expect(hdr));
    let scene = match pana("ShootingMode") {
        Some("Program" | "Aperture priority" | "Shutter-speed priority" | "Manual") => Some(false),
        Some(t) if !unnamed(t) => Some(true),
        _ => None,
    };
    c.eq(
        "shooting mode",
        find(style, "Scene or Intelligent Auto Mode"),
        expect(scene),
    );
    let shading = match pana("ShadingCompensation") {
        Some("Off") => Some(false),
        Some("On") => Some(true),
        _ => None,
    };
    c.eq("shading compensation", style.peripheral_correction, shading);
}

#[test]
fn camera_style_agrees_with_exiv2() {
    let files = samples();
    if files.is_empty() {
        println!("SKIPPED: no sample raws to compare");
        return;
    }
    if Command::new("exiv2").arg("--version").output().is_err() {
        assert!(
            frame_lists().is_empty(),
            "GREYCARD_STYLE_FRAMES is set and exiv2 is not on PATH"
        );
        println!("SKIPPED: no exiv2 on PATH");
        return;
    }
    let mut compared = 0;
    let mut tally = Tally::default();
    let mut counts: HashMap<(Maker, Option<String>), usize> = HashMap::new();
    for path in &files {
        let kv = exiv2(path, "-Pkv");
        let kt = exiv2(path, "-Pkt");
        let make = kv.get("Exif.Image.Make").cloned().unwrap_or_default();
        let body = kv
            .get("Exif.Image.Model")
            .cloned()
            .unwrap_or_else(|| "?".into());
        let maker = Maker::from_make(&make);
        let mut c = Check {
            tally: &mut tally,
            body,
            path,
        };
        let style = match CameraStyle::read(path) {
            Ok(style) => style,
            Err(e) => {
                c.eq("read", Err::<(), _>(e.to_string()), Ok(()));
                continue;
            }
        };
        let Some(style) = style else {
            c.eq("maker", Maker::Other, maker);
            *counts.entry((Maker::Other, None)).or_default() += 1;
            compared += 1;
            continue;
        };
        c.eq("maker", style.maker, maker);
        // Whether the maker note was found at all: the reader lists the
        // adaptive settings whenever it reads one, and exiv2 shows the
        // maker's group.
        let group = match maker {
            Maker::Nikon => Some(&["Exif.Nikon3."][..]),
            Maker::Sony => Some(&["Exif.Sony1.", "Exif.Sony2."][..]),
            Maker::Panasonic => Some(&["Exif.Panasonic."][..]),
            _ => None,
        };
        if let Some(group) = group {
            let shown = kt.keys().any(|k| group.iter().any(|g| k.starts_with(g)));
            c.eq("maker note read", !style.adaptive.is_empty(), shown);
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match (maker, ext.as_str()) {
            (Maker::Canon, "cr3") => check_canon(&mut c, &style, &kt),
            (Maker::Fujifilm, "raf") => check_fujifilm(&mut c, &style, &kv, &kt),
            (Maker::Nikon, "nef" | "nrw") => check_nikon(&mut c, &style, &kt),
            (Maker::Sony, "arw") => check_sony(&mut c, &style, &kt),
            (Maker::Panasonic, "rw2") => check_panasonic(&mut c, &style, &kt),
            _ => c.eq("style", style.style.clone(), None),
        }
        compared += 1;
        *counts.entry((maker, style.group_key())).or_default() += 1;
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by_key(|((m, g), _)| (m.name(), g.clone()));
    for ((maker, group), n) in counts {
        println!("{} {group:?}: {n}", maker.name());
    }
    println!("body\tfield\tfiles\tagreed");
    for ((body, field), (files, agreed)) in &tally.rows {
        println!("{body}\t{field}\t{files}\t{agreed}");
    }
    for wrong in &tally.wrong {
        println!("DISAGREES {wrong}");
    }
    println!("compared {compared} of {}", files.len());
    assert!(compared > 0, "there were files and none was compared");
    assert!(
        tally.wrong.is_empty(),
        "{} disagreements with exiv2, the first: {}",
        tally.wrong.len(),
        tally.wrong[0]
    );
}
