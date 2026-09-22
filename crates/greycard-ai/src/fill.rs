//! Fill: a hole made up from what is around it, by LaMa. The model
//! takes and gives a fixed 512×512 square; the caller cuts the window
//! and puts the answer back.

use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::image::Rgbf;
use crate::registry::FILL;
use crate::runtime::{self, Error, Loaded, Provider, Result};
use crate::store::Store;

/// The model's square.
pub const SIZE: usize = 512;

pub struct Fill {
    loaded: Loaded,
}

impl Fill {
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        if !store.have(&FILL) {
            return Err(Error::Missing(FILL.name));
        }
        let path = store.path(&FILL, &FILL.files[0]);
        let remember = runtime::Remembered {
            store_root: store.root(),
            model: FILL.id,
            hash: FILL.files[0].sha256,
        };
        let loaded = runtime::open(
            &path,
            GraphOptimizationLevel::Level3,
            providers,
            Some(remember),
            |s| {
                let image =
                    Tensor::from_array(([1usize, 3, SIZE, SIZE], vec![0.5f32; 3 * SIZE * SIZE]))?;
                let mask =
                    Tensor::from_array(([1usize, 1, SIZE, SIZE], vec![0.0f32; SIZE * SIZE]))?;
                s.run(ort::inputs!["image" => image, "mask" => mask])?;
                Ok(())
            },
        )?;
        Ok(Self { loaded })
    }

    pub fn provider(&self) -> Provider {
        self.loaded.provider
    }

    /// `image`, `SIZE`×`SIZE` in 0 to 1, with the pixels where `hole`
    /// is nonzero made up. The answer is in 0 to 1 too.
    pub fn fill(&mut self, image: &Rgbf, hole: &[f32]) -> Result<Rgbf> {
        if image.width != SIZE || image.height != SIZE || hole.len() != SIZE * SIZE {
            return Err(Error::Shape(format!(
                "fill wants {SIZE}x{SIZE}, got {}x{} and a hole of {}",
                image.width,
                image.height,
                hole.len()
            )));
        }
        let hole: Vec<f32> = hole
            .iter()
            .map(|&h| if h > 0.0 { 1.0 } else { 0.0 })
            .collect();
        // The model's own forward pass blanks the picture under the
        // mask before the network; the export leaves that to us.
        let n = SIZE * SIZE;
        let planes: Vec<f32> = image
            .to_planes()
            .iter()
            .enumerate()
            .map(|(i, v)| v.clamp(0.0, 1.0) * (1.0 - hole[i % n]))
            .collect();
        let image_in = Tensor::from_array(([1usize, 3, SIZE, SIZE], planes))?;
        let mask_in = Tensor::from_array(([1usize, 1, SIZE, SIZE], hole))?;
        let outputs = self
            .loaded
            .session
            .run(ort::inputs!["image" => image_in, "mask" => mask_in])?;
        let (shape, out) = outputs["output"].try_extract_tensor::<f32>()?;
        if out.len() != 3 * SIZE * SIZE {
            return Err(Error::Shape(shape.to_string()));
        }
        // Some exports answer in 0 to 255.
        let peak = out.iter().copied().fold(0.0f32, f32::max);
        let scale = if peak > 2.0 { 1.0 / 255.0 } else { 1.0 };
        let out: Vec<f32> = out.iter().map(|v| (v * scale).clamp(0.0, 1.0)).collect();
        Ok(Rgbf::from_planes(SIZE, SIZE, &out))
    }
}
