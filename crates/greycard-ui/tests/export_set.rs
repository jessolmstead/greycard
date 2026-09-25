//! The editor's `--export DIR --also ROWS` from the command line: a
//! set of three frames written into a folder, each with its own
//! source's EXIF, and `--export FILE` beside it on one frame as it
//! always was.
//!
//! Ignored, as the tests that want raws are: run it with
//! `cargo test --release -p greycard-ui --test export_set -- --ignored`
//! (a debug build develops a frame in tens of seconds). It wants
//! `GREYCARD_SAMPLES` (a folder of raws; `5M0A3976.CR3` when it is
//! there, else the first raw in it), a display for the window, and
//! `exiv2` on the path to read the files back; without any of them it
//! says SKIPPED and passes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const RAWS: [&str; 8] = ["cr3", "cr2", "nef", "arw", "raf", "dng", "rw2", "orf"];

fn skipped(why: &str) {
    eprintln!("SKIPPED: the command-line set export: {why}");
    println!("SKIPPED: the command-line set export: {why}");
}

/// The sample to copy: the one the other sample tests use when it is
/// there, else the first raw in the folder.
fn sample() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("GREYCARD_SAMPLES")?);
    let preferred = dir.join("5M0A3976.CR3");
    if preferred.is_file() {
        return Some(preferred);
    }
    let mut raws: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| RAWS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    raws.sort();
    raws.into_iter().next()
}

fn has_display() -> bool {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// One tag's value, as exiv2 prints it; empty when it is not there.
fn tag(file: &Path, key: &str) -> String {
    let out = Command::new("exiv2")
        .args(["-q", "-g", key, "-Pv"])
        .arg(file)
        .output()
        .expect("exiv2 runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The editor on `args`, its settings, models, caches and log in
/// `home`, waited on for at most two minutes: whether it succeeded,
/// and how long it took.
fn editor(home: &Path, args: &[&std::ffi::OsStr]) -> (bool, Duration) {
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_greycard-ui"))
        .args(args)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_STATE_HOME", home.join("state"))
        .spawn()
        .expect("the editor starts");
    loop {
        if let Some(status) = child.try_wait().expect("the editor is waited on") {
            return (status.success(), started.elapsed());
        }
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            panic!("the editor ran past two minutes");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "wants GREYCARD_SAMPLES, a display and exiv2"]
fn a_set_exported_from_the_command_line_carries_each_frame_s_exif() {
    let Some(sample) = sample() else {
        skipped("GREYCARD_SAMPLES is not set, or holds no raw");
        return;
    };
    if !has_display() {
        skipped("no display for the window");
        return;
    }
    if Command::new("exiv2").arg("--version").output().is_err() {
        skipped("exiv2 is not on the path");
        return;
    }
    let home = std::env::temp_dir().join(format!("greycard-export-set-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let shoot = home.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let ext = sample.extension().unwrap().to_string_lossy().into_owned();
    let names: Vec<String> = (1..=3).map(|i| format!("copy-{i}")).collect();
    for n in &names {
        std::fs::copy(&sample, shoot.join(format!("{n}.{ext}"))).unwrap();
    }
    let out = home.join("out");
    // A trailing separator asks for a folder that is not there yet.
    let mut folder = out.clone().into_os_string();
    folder.push(std::path::MAIN_SEPARATOR_STR);
    let (ok, set_time) = editor(
        &home,
        &[
            shoot.as_os_str(),
            "--export".as_ref(),
            &folder,
            "--also".as_ref(),
            "1,2".as_ref(),
            "--long-edge".as_ref(),
            "1024".as_ref(),
            "--no-sidecars".as_ref(),
        ],
    );
    assert!(ok, "the set export failed");
    let mut written: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        names.iter().map(|n| format!("{n}.jpg")).collect::<Vec<_>>()
    );
    let shot = tag(&sample, "Exif.Photo.DateTimeOriginal");
    let exposure = tag(&sample, "Exif.Photo.ExposureTime");
    assert!(!shot.is_empty(), "exiv2 reads the sample's EXIF");
    for n in &names {
        let file = out.join(format!("{n}.jpg"));
        assert_eq!(tag(&file, "Exif.Photo.DateTimeOriginal"), shot, "{n}");
        assert_eq!(tag(&file, "Exif.Photo.ExposureTime"), exposure, "{n}");
        assert!(
            tag(&file, "Exif.Image.Software").starts_with("greycard"),
            "{n}"
        );
        // Each file names its own source.
        assert_eq!(
            tag(&file, "Xmp.greycard.Source"),
            format!("{n}.{ext}"),
            "{n}"
        );
        let side = |key: &str| tag(&file, key).parse::<u32>().unwrap_or(0);
        let long = side("Exif.Photo.PixelXDimension").max(side("Exif.Photo.PixelYDimension"));
        assert_eq!(long, 1024, "{n}");
    }

    // `--export FILE` is one frame, whatever else is selected.
    let single = home.join("single.jpg");
    let (ok, single_time) = editor(
        &home,
        &[
            shoot.join(format!("{}.{ext}", names[0])).as_os_str(),
            "--export".as_ref(),
            single.as_os_str(),
            "--long-edge".as_ref(),
            "1024".as_ref(),
            "--no-sidecars".as_ref(),
        ],
    );
    assert!(ok, "the single export failed");
    assert_eq!(tag(&single, "Exif.Photo.DateTimeOriginal"), shot);
    println!(
        "three frames as a set in {:.2} s; one frame alone in {:.2} s",
        set_time.as_secs_f64(),
        single_time.as_secs_f64()
    );
    std::fs::remove_dir_all(&home).unwrap();
}
