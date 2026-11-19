//! The oracle's fixtures (`scripts/highlight-fill-oracle/`): a
//! directory a scene, `manifest.txt` with a line an array (`name dtype
//! dim …`), and each array raw little-endian in C order (SPEC.md,
//! "Fixture format and layouts"). The one reader every highlight_fill
//! test uses.

use std::collections::HashMap;
use std::path::PathBuf;

use super::net::{FillWeights, TENSORS, Tensor};

/// The fixtures' random-seeded checkpoint (`weights/`), as tensors.
pub fn random_tensors() -> Vec<Tensor> {
    let a = Fixture::load("weights");
    TENSORS
        .iter()
        .map(|(name, _)| Tensor {
            name: (*name).to_string(),
            shape: a.dims(name).to_vec(),
            data: a.f32(name),
        })
        .collect()
}

/// The fixtures' random-seeded checkpoint.
pub fn random_weights() -> FillWeights {
    FillWeights::from_tensors(random_tensors()).unwrap()
}

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/highlight_fill/"
);

pub struct Fixture {
    pub name: String,
    arrays: HashMap<String, (String, Vec<usize>, Vec<u8>)>,
}

impl Fixture {
    pub fn load(name: &str) -> Self {
        let dir = PathBuf::from(ROOT).join(name);
        let manifest = std::fs::read_to_string(dir.join("manifest.txt"))
            .unwrap_or_else(|e| panic!("{name}'s manifest: {e}"));
        let mut arrays = HashMap::new();
        for line in manifest.lines().filter(|l| !l.trim().is_empty()) {
            let mut words = line.split_whitespace();
            let key = words.next().expect("a name").to_string();
            let dtype = words.next().expect("a dtype").to_string();
            let dims: Vec<usize> = words.map(|d| d.parse().expect("a dim")).collect();
            let bytes = std::fs::read(dir.join(format!("{key}.bin")))
                .unwrap_or_else(|e| panic!("{name}/{key}: {e}"));
            let size = if dtype == "u8" { 1 } else { 4 };
            assert_eq!(
                bytes.len(),
                dims.iter().product::<usize>() * size,
                "{name}/{key}"
            );
            arrays.insert(key, (dtype, dims, bytes));
        }
        Self {
            name: name.to_string(),
            arrays,
        }
    }

    /// The scenes `scenes.txt` lists, in its order.
    pub fn scenes() -> Vec<String> {
        std::fs::read_to_string(PathBuf::from(ROOT).join("scenes.txt"))
            .expect("scenes.txt")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.split_whitespace().next().unwrap().to_string())
            .collect()
    }

    pub fn has(&self, key: &str) -> bool {
        self.arrays.contains_key(key)
    }

    pub fn dims(&self, key: &str) -> &[usize] {
        &self.get(key, None).1
    }

    fn get(&self, key: &str, dtype: Option<&str>) -> &(String, Vec<usize>, Vec<u8>) {
        let a = self
            .arrays
            .get(key)
            .unwrap_or_else(|| panic!("{} has no {key}", self.name));
        if let Some(d) = dtype {
            assert_eq!(a.0, d, "{}/{key}'s dtype", self.name);
        }
        a
    }

    pub fn f32(&self, key: &str) -> Vec<f32> {
        let (words, _) = self.get(key, Some("f32")).2.as_chunks::<4>();
        words.iter().map(|&b| f32::from_le_bytes(b)).collect()
    }

    pub fn i32(&self, key: &str) -> Vec<i32> {
        let (words, _) = self.get(key, Some("i32")).2.as_chunks::<4>();
        words.iter().map(|&b| i32::from_le_bytes(b)).collect()
    }

    pub fn u8(&self, key: &str) -> Vec<u8> {
        self.get(key, Some("u8")).2.clone()
    }

    /// The scene's inputs: x (interleaved), width, height, the codes a
    /// pixel, the clips, and the look scale. `headroom` stores none of
    /// x or the codes: they are the lamp's, x divided by its headroom
    /// in f32.
    pub fn inputs(&self) -> Inputs {
        let (x, codes, dims) = if self.has("x") {
            (self.f32("x"), self.u8("codes"), self.dims("x").to_vec())
        } else {
            let lamp = Fixture::load("lamp");
            let h = self.f32("headroom")[0];
            let x = lamp.f32("x").into_iter().map(|v| v / h).collect();
            (x, lamp.u8("codes"), lamp.dims("x").to_vec())
        };
        let clips = self.f32("clips");
        Inputs {
            x,
            codes,
            width: dims[1],
            height: dims[0],
            clips: [clips[0], clips[1], clips[2]],
            f: self.f32("f")[0],
        }
    }
}

pub struct Inputs {
    pub x: Vec<f32>,
    pub codes: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub clips: [f32; 3],
    pub f: f32,
}
