//! The learned denoiser's answers kept on disk, so a frame pays the
//! network once and not once a session. Each answer is a linear DNG
//! through `greycard_core::dng`: camera RGB, the color tags of the
//! frame it came from, so it is also a file any raw editor opens. The
//! design is in the notes, §37.
//!
//! The key is what the network was given: the prepared samples
//! themselves, their size and pattern, the gains and ceiling they
//! carry, the noise model, the model's identity and the tiling. Any
//! change upstream (the white balance, the highlights, a hot pixel
//! setting) changes the samples and so the key; nothing has to
//! enumerate the settings. Hashing 24 MP costs tens of milliseconds.
//!
//! The samples in the file are camera native, the gains divided out,
//! scaled to fit under the white level with `BaselineExposure` saying
//! by how much; reading back undoes both with the gains and ceiling of
//! the prepared frame the key was made from, which the key guarantees
//! are the ones the file was written under.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use greycard_core::CameraImage;
use greycard_core::develop::Prepared;
use greycard_core::dng::{LinearDngOptions, write_linear_dng};
use greycard_core::raw::{RawFrame, SensorLayout};

use crate::denoise::Tiling;
use crate::registry::Model;
use crate::store::{self, Store};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Core(#[from] greycard_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What the disk holds at most, before the oldest answers go. A 24 MP
/// answer is about 70 MB compressed.
pub const DEFAULT_BUDGET: u64 = 8 << 30;

/// The identity of an answer: a hash of everything the network saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key(u64);

impl Key {
    /// The file's stem.
    pub fn name(self) -> String {
        format!("{:016x}", self.0)
    }
}

/// The answers on disk.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    budget: u64,
}

impl Cache {
    /// The user's cache: `denoise` beside the model store, so
    /// `~/.cache/greycard/denoise` on Linux.
    pub fn user() -> Result<Self> {
        let store = Store::user()?;
        let base = store
            .root()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| store.root().to_path_buf());
        Ok(Self::at(base.join("denoise")))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            budget: DEFAULT_BUDGET,
        }
    }

    pub fn with_budget(self, budget: u64) -> Self {
        Self { budget, ..self }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The key of `model`'s answer for `prepared`, cut by `tiling`.
    pub fn key(model: &Model, tiling: Tiling, prepared: &Prepared) -> Key {
        let mut h = Hasher::default();
        h.str(model.id);
        for f in model.files {
            h.str(f.sha256);
        }
        h.word(tiling.tile as u64);
        h.word(tiling.margin as u64);
        h.word(prepared.width as u64);
        h.word(prepared.height as u64);
        match &prepared.pattern {
            Some(p) => h.str(&format!("{p:?}")),
            None => h.word(0),
        }
        for g in prepared.gains {
            h.word(g.to_bits().into());
        }
        h.word(prepared.ceiling.to_bits().into());
        match prepared.noise_model() {
            Some(m) => {
                for v in m.a.iter().chain(&m.b) {
                    h.word(v.to_bits().into());
                }
            }
            None => h.word(0),
        }
        h.floats(&prepared.samples);
        Key(h.finish())
    }

    pub fn path(&self, key: Key) -> PathBuf {
        self.dir.join(format!("{}.dng", key.name()))
    }

    /// The answer for `key`, in `prepared`'s units and size, if it is
    /// here. A file that will not decode or does not fit is removed.
    pub fn read(&self, key: Key, prepared: &Prepared) -> Option<Vec<f32>> {
        let path = self.path(key);
        if !path.is_file() {
            return None;
        }
        match answer(&path, prepared) {
            Some(rgb) => {
                // Read now, so the budget keeps it over one not read.
                if let Ok(f) = std::fs::File::open(&path) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Some(rgb)
            }
            None => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Keep `rgb`, the network's answer for `prepared` (interleaved
    /// camera RGB in its units), under `key`. Written beside and
    /// renamed, so a crash leaves nothing that reads as an answer; then
    /// the oldest answers over the budget go.
    pub fn write(
        &self,
        key: Key,
        frame: &RawFrame,
        prepared: &Prepared,
        rgb: &[f32],
    ) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let headroom = headroom(prepared);
        let inverse = prepared.gains.map(|g| 1.0 / g);
        let native: Vec<f32> = rgb
            .iter()
            .enumerate()
            .map(|(i, v)| v * inverse[i % 3])
            .collect();
        let image = CameraImage::from_data(prepared.width, prepared.height, native)?;
        let options = LinearDngOptions {
            headroom: (headroom > 1.0).then_some(headroom),
            software: Some(concat!(
                "greycard ",
                env!("CARGO_PKG_VERSION"),
                " denoise cache"
            )),
            ..Default::default()
        };
        let path = self.path(key);
        let partial = path.with_extension("dng.partial");
        let result = (|| -> Result<()> {
            let file = std::fs::File::create(&partial)?;
            let mut out = std::io::BufWriter::new(file);
            write_linear_dng(&mut out, frame, &image, prepared.gains, &options)?;
            std::io::Write::flush(&mut out)?;
            drop(out);
            std::fs::rename(&partial, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        result?;
        self.trim();
        Ok(())
    }

    /// Remove the least recently used answers until the rest fit the
    /// budget. Best effort; the cache is never worth an error.
    fn trim(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
            .filter_map(|e| {
                let e = e.ok()?;
                let path = e.path();
                if path.extension().is_none_or(|x| x != "dng") {
                    return None;
                }
                let meta = e.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), path))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort();
        for (_, len, path) in files {
            if total <= self.budget {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

/// What the file's white level stands for in native units: the ceiling
/// through the smallest gain, so the brightest reconstructed highlight
/// fits.
fn headroom(prepared: &Prepared) -> f32 {
    let smallest = prepared.gains.iter().copied().fold(f32::INFINITY, f32::min);
    (prepared.ceiling / smallest).max(1.0)
}

/// The file's samples back in `prepared`'s units, if the file is an
/// answer of its size.
fn answer(path: &Path, prepared: &Prepared) -> Option<Vec<f32>> {
    let frame = greycard_core::decode::decode_path(path).ok()?;
    if frame.layout != SensorLayout::Linear
        || frame.channels != 3
        || frame.width != prepared.width
        || frame.height != prepared.height
    {
        return None;
    }
    // The writer's levels are one value per channel.
    let scale: [f32; 3] = std::array::from_fn(|c| {
        let black = frame.levels.black.at(0, 0, c);
        let white = frame.levels.white.at(0, 0, c);
        headroom(prepared) * prepared.gains[c] / (white - black).max(f32::EPSILON)
    });
    let black: [f32; 3] = std::array::from_fn(|c| frame.levels.black.at(0, 0, c));
    let mut rgb = frame.samples.to_f32();
    for (i, v) in rgb.iter_mut().enumerate() {
        let c = i % 3;
        *v = (*v - black[c]) * scale[c];
    }
    Some(rgb)
}

/// A hash over words that is the same on every build: what a key
/// needs, since the standard hasher promises nothing across releases.
/// The mix is a multiply and a rotate a word, closed by a finalizer
/// that spreads every bit; this is not a defense against anything,
/// only a name for the input.
#[derive(Default)]
struct Hasher {
    state: u64,
    length: u64,
}

impl Hasher {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;

    fn word(&mut self, w: u64) {
        self.state = (self.state ^ w).wrapping_mul(Self::K).rotate_left(29);
        self.length += 1;
    }

    fn str(&mut self, s: &str) {
        let bytes = s.as_bytes();
        for chunk in bytes.chunks(8) {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            self.word(u64::from_le_bytes(w));
        }
        self.word(bytes.len() as u64);
    }

    fn floats(&mut self, v: &[f32]) {
        let (pairs, rest) = v.as_chunks::<2>();
        for [a, b] in pairs {
            self.word(u64::from(a.to_bits()) | (u64::from(b.to_bits()) << 32));
        }
        for r in rest {
            self.word(r.to_bits().into());
        }
        self.word(v.len() as u64);
    }

    fn finish(self) -> u64 {
        // MurmurHash3's finalizer.
        let mut h = self.state ^ self.length;
        h ^= h >> 33;
        h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        h ^= h >> 33;
        h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
        h ^= h >> 33;
        h
    }
}

#[cfg(test)]
mod tests {
    use greycard_core::develop::{DevelopSettings, prepare};
    use greycard_core::raw::{
        Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Orientation, Samples,
    };

    use super::*;

    /// A flat RGGB frame of `side` square, gains (2, 1, 4), the camera
    /// Rec.2020 itself (the matrix rows are XYZ to Rec.2020).
    fn frame(side: usize) -> RawFrame {
        let pattern = CfaPattern::rggb();
        let mut samples = Vec::new();
        for y in 0..side {
            for x in 0..side {
                samples.push(match pattern.color_at(y, x) {
                    CfaColor::Red => 600u16,
                    CfaColor::Green => 900,
                    CfaColor::Blue => 350,
                    CfaColor::Other(_) => unreachable!(),
                });
            }
        }
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            width: side,
            height: side,
            channels: 1,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(100.0, 1),
                white: LevelPattern::uniform(1100.0, 1),
            },
            as_shot_coefficients: Some([2.0, 1.0, 4.0]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: vec![
                    1.7167, -0.3557, -0.2534, -0.6667, 1.6165, 0.0158, 0.0176, -0.0428, 0.9421,
                ],
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Rotate90,
            shot: Default::default(),
        }
    }

    fn model() -> Model {
        crate::registry::DENOISE_FAST
    }

    fn temp_cache() -> Cache {
        let dir = std::env::temp_dir().join(format!(
            "greycard-denoise-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Cache::at(dir)
    }

    #[test]
    fn an_answer_comes_back_in_the_prepared_units() {
        let frame = frame(8);
        let settings = DevelopSettings::default();
        let prepared = prepare(&frame, &settings, false).unwrap();
        // An answer with highlights above one in every channel, and a
        // deep shadow.
        let n = prepared.width * prepared.height;
        let rgb: Vec<f32> = (0..n * 3)
            .map(|i| match i % 3 {
                0 => 1.5,
                1 => prepared.ceiling.min(3.0),
                _ => 0.01,
            })
            .collect();
        let cache = temp_cache();
        let key = Cache::key(&model(), Tiling::default(), &prepared);
        assert!(cache.read(key, &prepared).is_none());
        cache.write(key, &frame, &prepared, &rgb).unwrap();
        assert!(cache.path(key).is_file());
        let back = cache.read(key, &prepared).unwrap();
        assert_eq!(back.len(), rgb.len());
        let step = headroom(&prepared) * 4.0 / 65535.0;
        for (a, b) in back.iter().zip(&rgb) {
            assert!((a - b).abs() <= step, "{a} vs {b}");
        }
        let _ = std::fs::remove_dir_all(cache.dir());
    }

    #[test]
    fn the_key_names_what_the_network_saw() {
        let frame = frame(8);
        let settings = DevelopSettings::default();
        let prepared = prepare(&frame, &settings, false).unwrap();
        let key = Cache::key(&model(), Tiling::default(), &prepared);
        assert_eq!(key, Cache::key(&model(), Tiling::default(), &prepared));
        assert_ne!(
            key,
            Cache::key(&crate::registry::DENOISE_BEST, Tiling::default(), &prepared)
        );
        assert_ne!(
            key,
            Cache::key(
                &model(),
                Tiling {
                    tile: 512,
                    margin: 96
                },
                &prepared
            )
        );
        let mut other = prepared.clone();
        other.samples[5] += 1e-3;
        assert_ne!(key, Cache::key(&model(), Tiling::default(), &other));
        let mut other = prepared.clone();
        other.ceiling += 1.0;
        assert_ne!(key, Cache::key(&model(), Tiling::default(), &other));
        assert_eq!(key.name().len(), 16);
    }

    #[test]
    fn a_wrong_file_is_dropped_and_the_budget_keeps_the_newest() {
        let frame = frame(8);
        let settings = DevelopSettings::default();
        let prepared = prepare(&frame, &settings, false).unwrap();
        let cache = temp_cache();
        let key = Cache::key(&model(), Tiling::default(), &prepared);
        std::fs::create_dir_all(cache.dir()).unwrap();
        std::fs::write(cache.path(key), b"not a dng").unwrap();
        assert!(cache.read(key, &prepared).is_none());
        assert!(!cache.path(key).is_file());

        let rgb = vec![0.5; prepared.width * prepared.height * 3];
        let other = Key(key.0 ^ 1);
        let roomy = cache.clone().with_budget(DEFAULT_BUDGET);
        roomy.write(other, &frame, &prepared, &rgb).unwrap();
        let size = std::fs::metadata(cache.path(other)).unwrap().len();
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Room for one and a half answers: the older goes.
        let tight = cache.clone().with_budget(size * 3 / 2);
        tight.write(key, &frame, &prepared, &rgb).unwrap();
        assert!(cache.path(key).is_file());
        assert!(!cache.path(other).is_file());
        let _ = std::fs::remove_dir_all(cache.dir());
    }

    #[test]
    fn the_hash_is_the_same_every_time() {
        let mut h = Hasher::default();
        h.str("greycard");
        h.floats(&[1.0, 2.0, 3.0]);
        assert_eq!(h.finish(), 0x8f09_b722_7ad8_d7ea);
    }
}
