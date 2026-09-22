//! Subject: the salient object of the image as a matte, from BiRefNet.

use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::image::{IMAGENET_MEAN, IMAGENET_STD, Mask, Rgb8};
use crate::registry::SUBJECT;
use crate::runtime::{self, Error, Loaded, Provider, Result};
use crate::store::Store;

/// The model's square.
pub const SIZE: usize = 1024;

pub struct Subject {
    loaded: Loaded,
}

impl Subject {
    /// Load from the store, on the first provider that runs it.
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        if !store.have(&SUBJECT) {
            return Err(Error::Missing(SUBJECT.name));
        }
        let path = store.path(&SUBJECT, &SUBJECT.files[0]);
        let remember = runtime::Remembered {
            store_root: store.root(),
            model: SUBJECT.id,
            hash: SUBJECT.files[0].sha256,
        };
        let loaded = runtime::open(
            &path,
            GraphOptimizationLevel::Level3,
            providers,
            Some(remember),
            |s| {
                let zeros =
                    Tensor::from_array(([1usize, 3, SIZE, SIZE], vec![0.0f32; 3 * SIZE * SIZE]))?;
                s.run(ort::inputs!["input_image" => zeros])?;
                Ok(())
            },
        )?;
        Ok(Self { loaded })
    }

    pub fn provider(&self) -> Provider {
        self.loaded.provider
    }

    /// The subject's matte, `SIZE`×`SIZE` in the model's stretched
    /// square; resample it to the image's shape.
    pub fn mask(&mut self, image: &Rgb8) -> Result<Mask> {
        let planes = image.to_planes(SIZE, IMAGENET_MEAN, IMAGENET_STD);
        let input = Tensor::from_array(([1usize, 3, SIZE, SIZE], planes))?;
        let outputs = self
            .loaded
            .session
            .run(ort::inputs!["input_image" => input])?;
        let (shape, logits) = outputs["output_image"].try_extract_tensor::<f32>()?;
        if logits.len() != SIZE * SIZE {
            return Err(Error::Shape(shape.to_string()));
        }
        Ok(Mask::from_logits(SIZE, SIZE, logits))
    }
}
