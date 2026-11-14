//! `greycard export --author --copyright`, and the settings' author and
//! copyright: what exiv2 reads of a JPEG and a TIFF export. The source
//! is a small TIFF of the test's own writing that carries an author and
//! a notice of its own, the camera's in a raw's place.
//!
//! Wants `exiv2` on the path; without it it says SKIPPED and passes.
//! With `--nocapture` it prints `exiv2 -pa` and `exiv2 -px` of each.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use greycard_core::exif::{Provenance, Rights, Tags, write_rgb16_tiff_tagged};

/// A folder of the test's own, gone when the test is, however it ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        greycard_core::testing::remove_dir_retry(&self.0);
    }
}

/// A 32 by 24 TIFF whose EXIF names `Camera Owner` and `camera notice`.
fn source(path: &Path) {
    let (w, h) = (32u32, 24u32);
    let data: Vec<u16> = (0..w * h * 3)
        .map(|i| (i as u16).wrapping_mul(2654))
        .collect();
    let metadata = greycard_core::decode::RawMetadata {
        exif: Default::default(),
        model: "Test Body".into(),
        make: "Test".into(),
        lens: None,
        unique_image_id: None,
        rating: None,
    };
    let theirs = Rights::new("Camera Owner", "camera notice");
    let mut file = std::io::Cursor::new(Vec::new());
    write_rgb16_tiff_tagged(
        &mut file,
        w,
        h,
        &data,
        None,
        Tags::Provenance(&Provenance {
            metadata: &metadata,
            software: "greycard test",
            width: w,
            height: h,
            srgb: true,
            written: None,
            source_name: None,
            output: None,
            edit: None,
            rights: &theirs,
        }),
    )
    .unwrap();
    std::fs::write(path, file.into_inner()).unwrap();
}

/// `greycard export` on `args`, its settings, caches and log in
/// `home`, waited on for at most two minutes.
fn export(home: &Path, args: &[&std::ffi::OsStr]) -> Output {
    let started = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_greycard"))
        .arg("export")
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

/// What `exiv2 FLAG` prints of `file`.
fn exiv2(flag: &str, file: &Path) -> String {
    let out = Command::new("exiv2")
        .args(["-q", flag])
        .arg(file)
        .output()
        .expect("exiv2 runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The line of `printed` for `key`, empty when there is none.
fn line<'a>(printed: &'a str, key: &str) -> &'a str {
    printed
        .lines()
        .find(|l| l.split_whitespace().next() == Some(key))
        .unwrap_or_default()
}

#[test]
fn exiv2_reads_the_users_author_and_copyright() {
    if Command::new("exiv2").arg("--version").output().is_err() {
        println!("SKIPPED: the author and copyright: exiv2 is not on the path");
        return;
    }
    let scratch = Scratch(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("greycard-cli-export-rights-{}", std::process::id())),
    );
    let home = &scratch.0;
    greycard_core::testing::remove_dir_retry(home);
    std::fs::create_dir_all(home.join("config/greycard")).unwrap();
    let shot = home.join("shot.tif");
    source(&shot);
    // The settings' copyright; the flag's author over the settings'.
    std::fs::write(
        home.join("config/greycard/settings.json"),
        r#"{"author": "Settings Author", "copyright": "© 2026 Zoë Ångström"}"#,
    )
    .unwrap();

    for name in ["mine.jpg", "mine.tif"] {
        let out = home.join(name);
        let done = export(
            home,
            &[
                shot.as_os_str(),
                out.as_os_str(),
                "--author".as_ref(),
                "Zoë Ångström; Jo Doe".as_ref(),
                "--no-sidecars".as_ref(),
                "--cpu".as_ref(),
            ],
        );
        assert!(
            done.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&done.stderr)
        );
        let (pa, px) = (exiv2("-pa", &out), exiv2("-px", &out));
        println!("exiv2 -pa {name}, the rights:");
        for key in ["Exif.Image.Artist", "Exif.Image.Copyright"] {
            println!("{}", line(&pa, key));
        }
        println!("exiv2 -px {name}, the rights:");
        for key in ["Xmp.dc.creator", "Xmp.dc.rights"] {
            println!("{}", line(&px, key));
        }
        assert!(
            line(&pa, "Exif.Image.Artist").ends_with("Zoë Ångström; Jo Doe"),
            "{name}: {pa}"
        );
        assert!(
            line(&pa, "Exif.Image.Copyright").contains("© 2026 Zoë Ångström"),
            "{name}: {pa}"
        );
        assert!(!pa.contains("Camera Owner") && !pa.contains("camera notice"));
        assert!(
            line(&px, "Xmp.dc.creator").ends_with("Zoë Ångström, Jo Doe"),
            "{name}: {px}"
        );
        assert!(
            line(&px, "Xmp.dc.rights").ends_with("lang=\"x-default\" © 2026 Zoë Ångström"),
            "{name}: {px}"
        );
    }

    // Nothing said: the source's own travel, and the XMP names neither.
    std::fs::remove_file(home.join("config/greycard/settings.json")).unwrap();
    let theirs = home.join("theirs.jpg");
    let done = export(
        home,
        &[
            shot.as_os_str(),
            theirs.as_os_str(),
            "--no-sidecars".as_ref(),
            "--cpu".as_ref(),
        ],
    );
    assert!(done.status.success());
    let (pa, px) = (exiv2("-pa", &theirs), exiv2("-px", &theirs));
    assert!(
        line(&pa, "Exif.Image.Artist").ends_with("Camera Owner"),
        "{pa}"
    );
    assert!(line(&pa, "Exif.Image.Copyright").contains("camera notice"));
    assert!(!px.contains("Xmp.dc."), "{px}");
}

/// Metadata None writes nothing: not the settings' author and
/// copyright, not the flags', not the source's, in any format; the
/// run says the flags went unwritten.
#[test]
fn metadata_none_writes_no_author_or_copyright() {
    if Command::new("exiv2").arg("--version").output().is_err() {
        println!("SKIPPED: metadata None: exiv2 is not on the path");
        return;
    }
    let scratch = Scratch(
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("greycard-cli-export-none-{}", std::process::id())),
    );
    let home = &scratch.0;
    greycard_core::testing::remove_dir_retry(home);
    std::fs::create_dir_all(home.join("config/greycard")).unwrap();
    let shot = home.join("shot.tif");
    source(&shot);
    std::fs::write(
        home.join("config/greycard/settings.json"),
        r#"{"author": "Settings Author", "copyright": "© 2026 Zoë Ångström", "export_metadata": "None"}"#,
    )
    .unwrap();

    for name in ["none.jpg", "none.png", "none.tif", "none.webp"] {
        for flagged in [false, true] {
            let out = home.join(format!("{flagged}-{name}"));
            let mut args = vec![
                shot.as_os_str(),
                out.as_os_str(),
                "--no-sidecars".as_ref(),
                "--cpu".as_ref(),
            ];
            if flagged {
                args.extend::<[&std::ffi::OsStr; 4]>([
                    "--author".as_ref(),
                    "Flag Author".as_ref(),
                    "--copyright".as_ref(),
                    "flag notice".as_ref(),
                ]);
            }
            let done = export(home, &args);
            let stderr = String::from_utf8_lossy(&done.stderr);
            assert!(done.status.success(), "{name}: {stderr}");
            assert_eq!(
                stderr.contains("are not written: the export's metadata is None"),
                flagged,
                "{name}: {stderr}"
            );
            let (pa, px) = (exiv2("-pa", &out), exiv2("-px", &out));
            for key in ["Exif.Image.Artist", "Exif.Image.Copyright"] {
                assert_eq!(line(&pa, key), "", "{name} {flagged}: {pa}");
            }
            assert!(!px.contains("Xmp.dc."), "{name} {flagged}: {px}");
            let bytes = std::fs::read(&out).unwrap();
            for word in ["Author", "notice", "Ångström", "Camera Owner"] {
                assert!(
                    !bytes.windows(word.len()).any(|w| w == word.as_bytes()),
                    "{name} {flagged}: {word}"
                );
            }
        }
    }
}
