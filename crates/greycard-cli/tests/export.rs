//! `greycard export`, the editor's `--export` on the command line:
//! the process writes the file and exits 0, a frame without what its
//! edit names exits 2, and a frame that will not decode exits 1 with
//! the reason on stderr. On a linear DNG of the test's own writing, so
//! it needs no sample raw. The editor's own tests run the same through
//! its `--export`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// A folder of the test's own, gone when the test is, however it ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        greycard_core::testing::remove_dir_retry(&self.0);
    }
}

/// A linear DNG of a textured ramp, `width` by `height`.
fn dng(path: &Path, width: usize, height: usize) {
    use greycard_core::raw::{
        Calibration, CfaPattern, LevelPattern, Levels, Orientation, RawFrame, Samples, SensorLayout,
    };
    let m = greycard_core::color::WORKING_SPACE
        .from_xyz_matrix()
        .expect("the working space has a matrix");
    let frame = RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        other_names: Vec::new(),
        width,
        height,
        channels: 1,
        layout: SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(Vec::new()),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(65535.0, 1),
            black_deltas: None,
            saturation: None,
        },
        as_shot_coefficients: Some([2.0, 1.0, 1.6]),
        calibrations: vec![Calibration {
            illuminant: 21,
            color_matrix: m.rows.iter().flatten().map(|v| *v as f32).collect(),
            forward_matrix: None,
        }],
        crop: None,
        orientation: Orientation::Normal,
        shot: Default::default(),
        dng: Default::default(),
    };
    let mut data = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let v = 0.5 + 0.2 * (x as f32 * 0.13).sin() * (y as f32 * 0.09).cos();
            let ramp = 0.02 + 0.9 * x as f32 / width as f32;
            data.extend_from_slice(&[ramp * v * 0.5, ramp * v, ramp * v * 0.6]);
        }
    }
    let image = greycard_core::CameraImage::from_data(width, height, data).unwrap();
    let mut file = std::fs::File::create(path).unwrap();
    greycard_core::dng::write_linear_dng(
        &mut file,
        &frame,
        &image,
        [2.0, 1.0, 1.6],
        &Default::default(),
    )
    .unwrap();
}

/// `greycard export` on `args`, its settings, caches and log in
/// `home`, waited on for at most two minutes.
fn export(home: &Path, args: &[&std::ffi::OsStr]) -> Output {
    export_with(home, args, &[])
}

/// `export` with more of the environment set.
fn export_with(home: &Path, args: &[&std::ffi::OsStr], env: &[(&str, &std::ffi::OsStr)]) -> Output {
    let started = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_greycard"))
        .arg("export")
        .args(args)
        .envs(env.iter().copied())
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_STATE_HOME", home.join("state"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("greycard starts");
    let id = child.id();
    let waited = std::thread::spawn(move || child.wait_with_output().expect("waited on"));
    while !waited.is_finished() {
        if started.elapsed() > Duration::from_secs(120) {
            let _ = Command::new("kill").arg(id.to_string()).status();
            panic!("greycard export ran past two minutes");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    waited.join().unwrap()
}

#[test]
fn an_export_s_exit_code_says_how_it_went() {
    let home = Scratch(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("greycard-headless-export-{}", std::process::id())),
    );
    greycard_core::testing::remove_dir_retry(&home.0);
    std::fs::create_dir_all(&home.0).unwrap();
    let source = home.0.join("frame.dng");
    dng(&source, 240, 160);
    let out = home.0.join("out.jpg");
    let ran = export(
        &home.0,
        &[
            source.as_os_str(),
            "--no-sidecars".as_ref(),
            "--long-edge".as_ref(),
            "120".as_ref(),
            out.as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{said}");
    assert!(said.contains("exported"), "{said}");
    let picture = image::open(&out).unwrap();
    assert_eq!((picture.width(), picture.height()), (120, 80));

    // An edit that asks for a Subject mask, with no model downloaded
    // (the run's cache is the test's own, and empty): the file is
    // written without it, the frame and the shape named on stderr,
    // and exit 2.
    let masked = home.0.join("masked.dng");
    std::fs::copy(&source, &masked).unwrap();
    let mut sidecar = greycard_edit::Sidecar::default();
    sidecar.current.adjustments.push(greycard_edit::Adjustment {
        name: "Person".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![greycard_edit::mask::Component {
                shape: greycard_edit::mask::Shape::Subject {},
                ..Default::default()
            }],
            invert: false,
            ..Default::default()
        },
        ..Default::default()
    });
    sidecar
        .save_in(&masked, greycard_edit::Placement::Beside)
        .unwrap();
    let without = home.0.join("without.jpg");
    let ran = export(&home.0, &[masked.as_os_str(), without.as_os_str()]);
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(2), "{said}");
    assert!(
        said.contains("masked.dng") && said.contains("Person's Subject shape"),
        "{said}"
    );
    assert!(
        said.contains("without something its edit names that this machine has not got"),
        "{said}"
    );
    assert!(without.is_file());

    // A frame that will not decode: exit 1, and why on stderr.
    let broken = home.0.join("broken.CR3");
    std::fs::write(&broken, b"not a raw").unwrap();
    let ran = export(
        &home.0,
        &[
            broken.as_os_str(),
            "--no-sidecars".as_ref(),
            home.0.join("broken.jpg").as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(1), "{said}");
    assert!(said.contains("failed"), "{said}");
    assert!(!home.0.join("broken.jpg").exists());

    // A set asked of one frame's file: refused, nothing written.
    let ran = export(
        &home.0,
        &[
            source.as_os_str(),
            masked.as_os_str(),
            home.0.join("again.jpg").as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(1), "{said}");
    assert!(said.contains("names a set"), "{said}");
    assert!(!home.0.join("again.jpg").exists());

    // Both into a folder: each under its own edit, the Subject frame
    // without its shape, so exit 2.
    let mut folder = home.0.join("set").into_os_string();
    folder.push(std::path::MAIN_SEPARATOR_STR);
    let ran = export(&home.0, &[source.as_os_str(), masked.as_os_str(), &folder]);
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(2), "{said}");
    assert!(home.0.join("set").join("frame.jpg").is_file(), "{said}");
    assert!(home.0.join("set").join("masked.jpg").is_file(), "{said}");
}

/// With no ONNX Runtime to be had (here `GREYCARD_ORT_DYLIB` names a file
/// that is not there, which is decided the same way as a processor
/// without AVX2 or no library beside the executable), an export with
/// no learned mask runs and says nothing of it; an edit with a learned
/// mask goes without it, the shape named and the reason given, exit 2.
/// Nothing of the runtime is loaded on the way: the line comes from the
/// decision alone.
#[test]
fn without_onnx_runtime_an_export_runs_and_says_why() {
    let home = Scratch(Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "greycard-headless-no-runtime-{}",
        std::process::id()
    )));
    greycard_core::testing::remove_dir_retry(&home.0);
    std::fs::create_dir_all(&home.0).unwrap();
    let nowhere = home.0.join("nowhere").join("libonnxruntime.so");
    let env = [("GREYCARD_ORT_DYLIB", nowhere.as_os_str())];
    let source = home.0.join("frame.dng");
    dng(&source, 240, 160);
    let out = home.0.join("out.jpg");
    let ran = export_with(
        &home.0,
        &[
            source.as_os_str(),
            "--no-sidecars".as_ref(),
            "--long-edge".as_ref(),
            "120".as_ref(),
            out.as_os_str(),
        ],
        &env,
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{said}");
    let line = format!(
        "the AI features are off: GREYCARD_ORT_DYLIB names {}, which is not there",
        nowhere.display()
    );
    assert!(!said.contains(&line), "{said}");
    assert!(said.contains("exported"), "{said}");
    assert!(out.is_file());

    let masked = home.0.join("masked.dng");
    std::fs::copy(&source, &masked).unwrap();
    let mut sidecar = greycard_edit::Sidecar::default();
    sidecar.current.adjustments.push(greycard_edit::Adjustment {
        name: "Sky".into(),
        mask: greycard_edit::mask::Mask {
            components: vec![greycard_edit::mask::Component {
                shape: greycard_edit::mask::Shape::Subject {},
                ..Default::default()
            }],
            invert: false,
            ..Default::default()
        },
        ..Default::default()
    });
    sidecar
        .save_in(&masked, greycard_edit::Placement::Beside)
        .unwrap();
    let without = home.0.join("without.jpg");
    let ran = export_with(&home.0, &[masked.as_os_str(), without.as_os_str()], &env);
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(2), "{said}");
    assert!(
        said.contains(&format!("Sky's Subject shape: {line}")),
        "{said}"
    );
    assert!(without.is_file());

    // `ort`'s own variable is anybody's and not read: naming nothing
    // with it, the Subject mask goes without for the reason it would
    // anyway (no model in the run's cache, or no library beside the
    // test's binary), and no line names the variable.
    let foreign = [("ORT_DYLIB_PATH", nowhere.as_os_str())];
    let ran = export_with(
        &home.0,
        &[masked.as_os_str(), home.0.join("again.jpg").as_os_str()],
        &foreign,
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(2), "{said}");
    assert!(said.contains("Sky's Subject shape"), "{said}");
    assert!(!said.contains("ORT_DYLIB_PATH"), "{said}");
}

#[test]
fn an_export_preset_with_a_border_writes_a_file_larger_by_the_band() {
    let home = Scratch(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("greycard-headless-border-{}", std::process::id())),
    );
    greycard_core::testing::remove_dir_retry(&home.0);
    std::fs::create_dir_all(home.0.join("config").join("greycard")).unwrap();
    let source = home.0.join("frame.dng");
    dng(&source, 240, 160);
    // A preset at 120 px on the long edge with a black border a tenth
    // of it: 12 px each side, the file 144 by 104.
    let settings = r#"{
        "export_presets": [{
            "name": "Framed",
            "export_format": "JPEG",
            "export_quality": 95,
            "export_size": "Custom",
            "export_custom": "120",
            "export_border": true,
            "export_border_width": 10,
            "export_border_color": "Black"
        }]
    }"#;
    std::fs::write(
        home.0.join("config").join("greycard").join("settings.json"),
        settings,
    )
    .unwrap();
    let out = home.0.join("framed.jpg");
    let ran = export(
        &home.0,
        &[
            source.as_os_str(),
            "--no-sidecars".as_ref(),
            "--export-preset".as_ref(),
            "Framed".as_ref(),
            out.as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{said}");
    let picture = image::open(&out).unwrap().into_rgb8();
    assert_eq!(picture.dimensions(), (144, 104));
    // The band is black at the corners and the middle of each edge,
    // and the picture's lit right side is inside it.
    for (x, y) in [
        (0, 0),
        (143, 0),
        (0, 103),
        (143, 103),
        (72, 2),
        (72, 101),
        (2, 52),
        (141, 52),
    ] {
        let p = picture.get_pixel(x, y).0;
        assert!(p.iter().all(|c| *c < 12), "({x}, {y}) is {p:?}");
    }
    let lit = picture.get_pixel(12 + 110, 52).0;
    assert!(lit.iter().any(|c| *c > 60), "{lit:?}");
}

/// A WebP's canvas, from its VP8X chunk: an export with a profile or
/// metadata has one.
fn webp_canvas(bytes: &[u8]) -> (u32, u32) {
    assert!(bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP");
    assert_eq!(&bytes[12..16], b"VP8X");
    let side = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], 0]) + 1;
    (side(24), side(27))
}

#[test]
fn a_webp_is_written_by_its_extension_and_by_a_preset() {
    let home = Scratch(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("greycard-headless-webp-{}", std::process::id())),
    );
    greycard_core::testing::remove_dir_retry(&home.0);
    std::fs::create_dir_all(home.0.join("config").join("greycard")).unwrap();
    let source = home.0.join("frame.dng");
    dng(&source, 240, 160);
    let out = home.0.join("out.webp");
    let ran = export(
        &home.0,
        &[
            source.as_os_str(),
            "--no-sidecars".as_ref(),
            "--long-edge".as_ref(),
            "120".as_ref(),
            out.as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{said}");
    assert_eq!(webp_canvas(&std::fs::read(&out).unwrap()), (120, 80));
    // Into a folder, a preset's format names the file.
    let settings = r#"{
        "export_presets": [{
            "name": "Lossless",
            "export_format": "WebP",
            "export_size": "Custom",
            "export_custom": "96"
        }]
    }"#;
    std::fs::write(
        home.0.join("config").join("greycard").join("settings.json"),
        settings,
    )
    .unwrap();
    let set = home.0.join("set");
    std::fs::create_dir_all(&set).unwrap();
    let ran = export(
        &home.0,
        &[
            source.as_os_str(),
            "--no-sidecars".as_ref(),
            "--export-preset".as_ref(),
            "Lossless".as_ref(),
            set.as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{said}");
    let file = set.join("frame.webp");
    assert_eq!(webp_canvas(&std::fs::read(&file).unwrap()), (96, 64));
}
