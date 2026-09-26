//! The camera style reader against exiv2, on real raws.
//!
//! Runs over every raw in `GREYCARD_SAMPLES` when that is set, and over
//! the raws listed in the files named by `GREYCARD_STYLE_FRAMES` (one
//! path a line, list files separated by `:`). Skips with a note when
//! neither is set or `exiv2` is missing; fails when frame lists were
//! named and exiv2 is missing, or when there were files and none was
//! compared. The expected values come from exiv2's own names for the
//! settings (`-Pkt`), not from the reader's rules.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use greycard_core::decode::{Adaptive, CameraStyle, Maker, is_raw_path};

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

fn find(style: &CameraStyle, name: &str) -> Option<Adaptive> {
    style.adaptive.iter().find(|a| a.name == name).copied()
}

/// The setting as expected: `Some(on)` a named value, `None` one exiv2
/// does not name or the file does not carry.
fn expect(name: &'static str, on: Option<bool>) -> Option<Adaptive> {
    Some(match on {
        Some(on) => Adaptive {
            name,
            on,
            known: true,
        },
        None => Adaptive {
            name,
            on: true,
            known: false,
        },
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

fn check_canon(path: &Path, style: &CameraStyle, kt: &HashMap<String, String>) {
    let what = path.display();
    match kt.get("Exif.CanonPr.PictureStyle").map(String::as_str) {
        Some("None") | None => assert_eq!(style.style, None, "{what}"),
        Some(name) => assert_eq!(style.style.as_deref(), Some(name), "{what}: picture style"),
    }
    let alo = match kt
        .get("Exif.CanonLiOp.AutoLightingOptimizer")
        .map(String::as_str)
    {
        Some("Off") => Some(false),
        Some("Standard" | "Low" | "Strong") => Some(true),
        _ => None,
    };
    assert_eq!(
        find(style, "Auto Lighting Optimizer"),
        expect("Auto Lighting Optimizer", alo),
        "{what}: ALO"
    );
    let htp = match kt
        .get("Exif.CanonLiOp.HighlightTonePriority")
        .map(String::as_str)
    {
        Some("Off") => Some(false),
        Some("On") => Some(true),
        _ => None,
    };
    assert_eq!(
        find(style, "Highlight Tone Priority"),
        expect("Highlight Tone Priority", htp),
        "{what}: HTP"
    );
    let peripheral = kt
        .get("Exif.CanonVigCor2.PeripheralLightingSetting")
        .or_else(|| kt.get("Exif.CanonLiOp.PeripheralIlluminationCorr"));
    let expected = match peripheral.map(String::as_str) {
        Some("On") => Some(true),
        Some("Off") => Some(false),
        _ => None,
    };
    assert_eq!(style.peripheral_correction, expected, "{what}: peripheral");
}

fn check_fujifilm(
    path: &Path,
    style: &CameraStyle,
    kv: &HashMap<String, String>,
    kt: &HashMap<String, String>,
) {
    let what = path.display();
    let mono = kt.get("Exif.Fujifilm.Color").filter(|c| {
        let c = c.to_lowercase();
        c.starts_with("monochrome") || c.starts_with("acros") || c.starts_with("sepia")
    });
    let expected = kt.get("Exif.Fujifilm.FilmMode").or(mono).map(|n| film(n));
    assert_eq!(
        style.style.as_deref().map(film),
        expected,
        "{what}: film mode"
    );

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
    assert_eq!(
        find(style, "Dynamic Range"),
        expect("Dynamic Range", dr),
        "{what}: dynamic range"
    );
    let drp = match kt.get("Exif.Fujifilm.DRangePriority").map(String::as_str) {
        None => Some(false),
        Some("Auto" | "Fixed") => Some(true),
        Some(_) => None,
    };
    assert_eq!(
        find(style, "D-Range Priority"),
        expect("D-Range Priority", drp),
        "{what}: D-Range Priority"
    );
    for (key, name) in [
        ("Exif.Fujifilm.HighlightTone", "Highlight Tone"),
        ("Exif.Fujifilm.ShadowTone", "Shadow Tone"),
    ] {
        let moved = kv.get(key).is_some_and(|v| v != "0");
        assert_eq!(
            find(style, name),
            moved.then(|| expect(name, Some(true)).unwrap()),
            "{what}: {name}"
        );
    }
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
    let mut counts: HashMap<(Maker, Option<String>), usize> = HashMap::new();
    for path in &files {
        let kv = exiv2(path, "-Pkv");
        let kt = exiv2(path, "-Pkt");
        let make = kv.get("Exif.Image.Make").cloned().unwrap_or_default();
        let maker = Maker::from_make(&make);
        let style = CameraStyle::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let Some(style) = style else {
            assert_eq!(maker, Maker::Other, "{}", path.display());
            *counts.entry((Maker::Other, None)).or_default() += 1;
            compared += 1;
            continue;
        };
        assert_eq!(style.maker, maker, "{}", path.display());
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match (maker, ext.as_str()) {
            (Maker::Canon, "cr3") => check_canon(path, &style, &kt),
            (Maker::Fujifilm, "raf") => check_fujifilm(path, &style, &kv, &kt),
            _ => assert_eq!(style.style, None, "{}", path.display()),
        }
        compared += 1;
        *counts.entry((maker, style.group_key())).or_default() += 1;
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by_key(|((m, g), _)| (m.name(), g.clone()));
    for ((maker, group), n) in counts {
        println!("{} {group:?}: {n}", maker.name());
    }
    println!("compared {compared} of {}", files.len());
    assert!(compared > 0, "there were files and none was compared");
}
