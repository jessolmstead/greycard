//! The editor's `--export` with no display at all: the process run
//! with `DISPLAY` and `WAYLAND_DISPLAY` taken away writes the file and
//! exits 0, and a frame that will not decode exits 1 with the reason
//! on stderr. On a linear DNG of the test's own writing, so it needs no
//! sample raw.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// A folder of the test's own, gone when the test is, however it ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
        width,
        height,
        channels: 1,
        layout: SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(Vec::new()),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(65535.0, 1),
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

/// The editor on `args` with no display to open a window on, its
/// settings, caches and log in `home`, waited on for at most two
/// minutes.
fn editor(home: &Path, args: &[&std::ffi::OsStr]) -> Output {
    let started = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_greycard-ui"))
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_STATE_HOME", home.join("state"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the editor starts");
    let id = child.id();
    let waited = std::thread::spawn(move || child.wait_with_output().expect("waited on"));
    while !waited.is_finished() {
        if started.elapsed() > Duration::from_secs(120) {
            let _ = Command::new("kill").arg(id.to_string()).status();
            panic!("the editor ran past two minutes");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    waited.join().unwrap()
}

#[test]
fn an_export_runs_with_no_display_and_its_exit_code_says_how_it_went() {
    let home = Scratch(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("greycard-headless-export-{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&home.0);
    std::fs::create_dir_all(&home.0).unwrap();
    let source = home.0.join("frame.dng");
    dng(&source, 240, 160);
    let out = home.0.join("out.jpg");
    let ran = editor(
        &home.0,
        &[
            source.as_os_str(),
            "--no-sidecars".as_ref(),
            "--long-edge".as_ref(),
            "120".as_ref(),
            "--export".as_ref(),
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
        },
        ..Default::default()
    });
    sidecar
        .save_in(&masked, greycard_edit::Placement::Beside)
        .unwrap();
    let without = home.0.join("without.jpg");
    let ran = editor(
        &home.0,
        &[masked.as_os_str(), "--export".as_ref(), without.as_os_str()],
    );
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
    let ran = editor(
        &home.0,
        &[
            broken.as_os_str(),
            "--no-sidecars".as_ref(),
            "--export".as_ref(),
            home.0.join("broken.jpg").as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(1), "{said}");
    assert!(said.contains("failed"), "{said}");
    assert!(!home.0.join("broken.jpg").exists());

    // A flag the run has no window for: refused, nothing written.
    let ran = editor(
        &home.0,
        &[
            source.as_os_str(),
            "--export".as_ref(),
            home.0.join("again.jpg").as_os_str(),
            "--screenshot".as_ref(),
            home.0.join("shot.png").as_os_str(),
        ],
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(1), "{said}");
    assert!(said.contains("--screenshot"), "{said}");
    assert!(!home.0.join("again.jpg").exists());
}

#[test]
fn an_export_preset_with_a_border_writes_a_file_larger_by_the_band() {
    let home = Scratch(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("greycard-headless-border-{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&home.0);
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
    let ran = editor(
        &home.0,
        &[
            source.as_os_str(),
            "--no-sidecars".as_ref(),
            "--export-preset".as_ref(),
            "Framed".as_ref(),
            "--export".as_ref(),
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
