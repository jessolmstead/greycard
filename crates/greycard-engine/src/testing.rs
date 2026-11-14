//! The helpers the engine's tests share with the window's: a scratch
//! folder, a linear DNG of our own writing, and the render crate's.
//! Behind `cfg(test)` or the `testing` feature, which the window's
//! dev-dependency turns on, so they never reach a release build.

pub use greycard_render::testing::{GFX, remove_dir_retry};

/// A folder under the system's temporary directory no test has used,
/// named for `what`, made, and in its canonical form: on macOS the
/// temporary directory is a symlink, and the index keys folders by
/// their canonical path, so a test that lists a view from the index
/// would find nothing under the spelling `temp_dir` gives. A test
/// removes it with [`remove_dir_retry`].
pub fn scratch_dir(what: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(&dir).unwrap()
}

/// A linear DNG of our own writing from a body of `make` and `model`, a
/// textured scene of `width` by `height` with a ramp across it: a raw
/// the engine opens and names by its body, which no sample raw in the
/// repository may stand in for.
pub fn linear_dng(path: &std::path::Path, width: usize, height: usize, make: &str, model: &str) {
    use greycard_core::raw::{
        Calibration, CfaPattern, LevelPattern, Levels, Orientation, Samples, SensorLayout,
    };
    let m = greycard_core::color::WORKING_SPACE
        .from_xyz_matrix()
        .expect("the working space has a matrix");
    let frame = greycard_core::RawFrame {
        make: make.into(),
        model: model.into(),
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
            let (fx, fy) = (x as f32, y as f32);
            let v =
                0.5 + 0.2 * (fx * 0.13).sin() * (fy * 0.09).cos() + 0.1 * ((fx + fy) * 0.031).sin();
            let ramp = 0.02 + 0.6 * fx / width as f32;
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

/// A monochrome sensor's DNG of our own writing, `width` by `height`,
/// with no preview: one plane of LinearRaw, a ramp from left to right
/// with a texture on it, as a Leica Monochrom's or a Ricoh GR IV
/// Monochrome's file is laid out.
pub fn monochrome_dng(path: &std::path::Path, width: usize, height: usize) {
    use greycard_core::raw::{LevelPattern, Levels, Orientation, Samples, SensorLayout};
    let frame = greycard_core::RawFrame {
        make: "Test".into(),
        model: "Mono".into(),
        other_names: Vec::new(),
        width,
        height,
        channels: 1,
        layout: SensorLayout::Monochrome,
        samples: Samples::U16(Vec::new()),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(65535.0, 1),
            black_deltas: None,
        },
        as_shot_coefficients: Some([1.0; 3]),
        calibrations: Vec::new(),
        crop: None,
        orientation: Orientation::Normal,
        shot: Default::default(),
        dng: Default::default(),
    };
    let mut data = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32, y as f32);
            let v = 0.5 + 0.2 * (fx * 0.13).sin() * (fy * 0.09).cos();
            data.extend_from_slice(&[(0.02 + 0.6 * fx / width as f32) * v; 3]);
        }
    }
    let image = greycard_core::CameraImage::from_data(width, height, data).unwrap();
    let mut file = std::fs::File::create(path).unwrap();
    greycard_core::dng::write_linear_dng(&mut file, &frame, &image, [1.0; 3], &Default::default())
        .unwrap();
}

/// Models whose learned denoiser is there and whose network is the
/// plain demosaic: a walk or a merge asked for a tier runs it with no
/// model on the machine, and says it ran.
pub struct PlainDenoiser;

impl crate::walk::Models for PlainDenoiser {
    fn can_denoise(
        &mut self,
        _: &'static greycard_ai::Model,
    ) -> Result<(), crate::walk::NoLearned> {
        Ok(())
    }

    fn denoise(
        &mut self,
        _: &'static greycard_ai::Model,
        _: &greycard_core::raw::RawFrame,
        prepared: &greycard_core::develop::Prepared,
    ) -> Result<Vec<f32>, crate::walk::NoLearned> {
        greycard_core::develop::demosaic_prepared(
            prepared,
            &greycard_core::develop::DevelopSettings::default(),
        )
        .map(|(rgb, _, _)| rgb)
        .map_err(|e| crate::walk::NoLearned::Failed(e.to_string()))
    }

    fn fill(
        &mut self,
        _: &greycard_edit::retouch::Patch,
        _: &greycard_core::develop::retouch::Region,
        _: &greycard_core::image::WorkingImage,
    ) -> Option<Vec<f32>> {
        None
    }

    fn could_fill_more(
        &mut self,
        _: &greycard_edit::retouch::Retouch,
        _: &greycard_core::image::WorkingImage,
    ) -> bool {
        false
    }

    fn rasters(
        &mut self,
        _: crate::stages::Masks,
        _: &std::sync::Arc<greycard_core::image::WorkingImage>,
        _: Option<crate::walk::Patched<'_>>,
        _: &crate::walk::Meta,
    ) -> crate::walk::Rasters {
        crate::walk::Rasters::new()
    }
}

/// A worker with a cache of its own and a frame opened on it, for the
/// tests of the edit's pictures, the engine's and the window's.
pub mod edited {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use greycard_edit::Edit;

    use crate::edited_pictures::BIG;
    use crate::version_key::Key;

    /// What the worker said, as the test reads it.
    #[derive(Debug)]
    pub enum Heard {
        Developed,
        Kept(PathBuf),
        Thumb {
            size: u32,
            w: u32,
            h: u32,
            mean: f64,
            slope: (f64, f64),
        },
    }

    pub fn mean(rgb: &[u8]) -> f64 {
        rgb.iter().map(|&v| v as f64).sum::<f64>() / rgb.len().max(1) as f64
    }

    /// Which way the test picture runs: red from the left column to the
    /// right, green from the top row to the bottom. Both rise in the
    /// picture as the camera's tag turns it.
    pub fn slope(w: usize, h: usize, rgb: &[u8]) -> (f64, f64) {
        let at = |x: usize, y: usize, c: usize| rgb[(y * w + x) * 3 + c] as f64;
        (
            at(w - 1, h / 2, 0) - at(0, h / 2, 0),
            at(w / 2, h - 1, 1) - at(w / 2, 0, 1),
        )
    }

    pub fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-edited-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub struct Rig {
        pub worker: crate::worker::Worker,
        pub heard: std::sync::mpsc::Receiver<Heard>,
        pub dir: PathBuf,
        pub png: PathBuf,
        /// The last thumbnail's [`slope`].
        pub slope: std::cell::Cell<(f64, f64)>,
    }

    /// A worker with a cache of its own and a PNG of a dim gradient,
    /// 600 by 400, opened and developed on the CPU.
    pub fn rig(name: &str) -> Rig {
        use crate::worker::{Job, Outcome, Worker};
        let dir = scratch(name);
        let png = dir.join("frame.png");
        let picture = image::RgbImage::from_fn(600, 400, |x, y| {
            image::Rgb([(30 + x / 12) as u8, (40 + y / 10) as u8, 50])
        });
        picture.save(&png).unwrap();
        let (tx, heard) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = Worker::new(move |o| {
            let said = match o {
                Outcome::Developed { .. } => Heard::Developed,
                Outcome::EditedKept { frame } => Heard::Kept(frame.path),
                Outcome::Thumbnail {
                    size,
                    width,
                    height,
                    rgb,
                    ..
                } => Heard::Thumb {
                    size,
                    w: width,
                    h: height,
                    mean: mean(&rgb),
                    slope: slope(width as usize, height as usize, &rgb),
                },
                _ => return,
            };
            let _ = tx.lock().unwrap().send(said);
        });
        worker.set_thumb_cache(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        ));
        worker.send(Job::Open {
            path: png.clone(),
            version: None,
            edit: Edit::for_picture(),
            generation: 1,
            seed_blend: false,
            turn: 0,
            stance_known: false,
            learned: Vec::new(),
        });
        let rig = Rig {
            worker,
            heard,
            dir,
            png,
            slope: std::cell::Cell::new((0.0, 0.0)),
        };
        assert!(matches!(rig.next(), Heard::Developed));
        rig
    }

    impl Rig {
        pub fn next(&self) -> Heard {
            self.heard
                .recv_timeout(std::time::Duration::from_secs(120))
                .expect("the worker answers")
        }

        /// The strip's picture of the frame, asked for as a cell asks.
        pub fn thumb(&self) -> (u32, u32, f64) {
            self.worker.send(crate::worker::Job::Thumbnail {
                index: 0,
                frame: Key::file(self.png.clone()),
            });
            loop {
                if let Heard::Thumb {
                    size,
                    w,
                    h,
                    mean,
                    slope,
                } = self.next()
                {
                    assert_eq!(size, crate::made::made_size(crate::worker::THUMB_WIDTH));
                    self.slope.set(slope);
                    return (w, h, mean);
                }
            }
        }

        pub fn save(&self, edit: &Edit, turn: u8) {
            self.worker.send(crate::worker::Job::Keep {
                frame: Key::file(self.png.clone()),
                edit: edit.clone(),
                turn,
                iso: None,
            });
        }

        pub fn kept(&self) {
            loop {
                match self.next() {
                    Heard::Kept(path) => {
                        assert_eq!(path, self.png);
                        return;
                    }
                    Heard::Thumb { .. } | Heard::Developed => {}
                }
            }
        }

        /// Nothing said of the frame for a second.
        pub fn quiet(&self) {
            let said = self.heard.recv_timeout(std::time::Duration::from_secs(1));
            assert!(
                !matches!(said, Ok(Heard::Kept(_))),
                "a save that changed nothing asked the cells again"
            );
        }

        pub fn entries(&self) -> usize {
            greycard_library::thumbs::usage_at(&self.dir.join("thumbs")).entries
        }
    }
}
