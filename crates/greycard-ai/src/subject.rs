//! Subject: the salient object of the image as a matte, from BiRefNet.
//!
//! Two files of the same model: the onnx-community export, which the
//! WebGPU provider fails on, and greycard's rewrite of it, which the
//! provider runs whole and the CPU runs to the same answer (notes,
//! "BiRefNet on WebGPU"). [`model_for`] picks between them.

use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::image::{IMAGENET_MEAN, IMAGENET_STD, Mask, Rgb8};
use crate::registry::{Model, SUBJECT, SUBJECT_WEBGPU};
use crate::runtime::{self, Error, Loaded, Provider, Result};
use crate::store::Store;

/// The model's square.
pub const SIZE: usize = 1024;

pub struct Subject {
    loaded: Loaded,
    model: &'static Model,
}

/// The Subject model to use with these providers and this store: the
/// rewrite when WebGPU is on offer, since only it runs there, else the
/// original — unless the other one is already in the store and the
/// preferred one is not, when that one is used rather than asking for
/// a second download of the same weights. Either runs on either
/// provider list: the rewrite answers on the CPU exactly as the
/// original does, and the original on WebGPU fails over to the CPU.
pub fn model_for(store: Option<&Store>, providers: &[Provider]) -> &'static Model {
    choose(providers.contains(&Provider::WebGpu), |m| {
        store.is_some_and(|s| s.have(m))
    })
}

fn choose(webgpu: bool, have: impl Fn(&Model) -> bool) -> &'static Model {
    let (preferred, other) = if webgpu {
        (&SUBJECT_WEBGPU, &SUBJECT)
    } else {
        (&SUBJECT, &SUBJECT_WEBGPU)
    };
    if !have(preferred) && have(other) {
        other
    } else {
        preferred
    }
}

impl Subject {
    /// Load from the store, on the first provider that runs it: the
    /// file [`model_for`] picks.
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        Self::load_model(store, model_for(Some(store), providers), providers)
    }

    /// Load this one of the two Subject files, whichever the
    /// providers.
    pub fn load_model(
        store: &Store,
        model: &'static Model,
        providers: &[Provider],
    ) -> Result<Self> {
        if !store.have(model) {
            return Err(Error::Missing(model.name));
        }
        let path = store.path(model, &model.files[0]);
        let remember = runtime::Remembered {
            store_root: store.root(),
            model: model.id,
            hash: model.files[0].sha256,
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
        Ok(Self { loaded, model })
    }

    pub fn provider(&self) -> Provider {
        self.loaded.provider
    }

    /// Which of the two files is loaded.
    pub fn model(&self) -> &'static Model {
        self.model
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webgpu_prefers_the_rewrite_and_the_cpu_the_original() {
        let none = |_: &Model| false;
        assert_eq!(choose(true, none).id, SUBJECT_WEBGPU.id);
        assert_eq!(choose(false, none).id, SUBJECT.id);
        let both = |_: &Model| true;
        assert_eq!(choose(true, both).id, SUBJECT_WEBGPU.id);
        assert_eq!(choose(false, both).id, SUBJECT.id);
    }

    #[test]
    fn the_file_in_the_store_is_used_rather_than_fetching_the_other() {
        let only_original = |m: &Model| m.id == SUBJECT.id;
        assert_eq!(choose(true, only_original).id, SUBJECT.id);
        let only_rewrite = |m: &Model| m.id == SUBJECT_WEBGPU.id;
        assert_eq!(choose(false, only_rewrite).id, SUBJECT_WEBGPU.id);
    }

    #[test]
    fn with_no_store_the_preference_stands() {
        assert_eq!(
            model_for(None, &[Provider::WebGpu, Provider::Cpu]).id,
            SUBJECT_WEBGPU.id
        );
        assert_eq!(model_for(None, &[Provider::Cpu]).id, SUBJECT.id);
    }
}
