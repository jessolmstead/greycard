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

/// The Subject model to use, or to offer to fetch, with these
/// providers and this store. What is in the store comes first: the
/// rewrite when WebGPU is on offer and the store has it, else the
/// original when the store has it and either WebGPU is off or the
/// original is not on record as failing on it — an install from
/// before the rewrite shipped, still on a card the original runs on
/// fine, fetches nothing a second time. Where the store has the
/// original alone, WebGPU is on offer, and the original *is* on
/// record as having failed on this adapter (`providers.json`, written
/// by `runtime::open`), the rewrite is offered instead, so an install
/// from before it existed comes off the CPU's three seconds a mask —
/// unless its id is in `unavailable` (its fetch failed, or it was
/// declined, this session), when the original stands. With neither
/// file in the store, the one to offer: the rewrite where WebGPU is on
/// offer, unless `unavailable`, then the original. So a rewrite that
/// cannot be had never stands between a shape and the original.
pub fn model_for(
    store: Option<&Store>,
    providers: &[Provider],
    unavailable: &[&str],
) -> &'static Model {
    pick(
        providers.contains(&Provider::WebGpu),
        |m| store.is_some_and(|s| s.have(m)),
        |m| unavailable.contains(&m.id),
        |_| store.is_some_and(|s| original_failed_on_webgpu(s, providers)),
    )
}

/// Whether the original (`SUBJECT`) is on record as having failed on
/// WebGPU, for the adapter and build this launch would use: what
/// turns [`model_for`] (and the offer sheet, `greycard-ui`) to the
/// rewrite for a store that has only the original.
pub fn original_failed_on_webgpu(store: &Store, providers: &[Provider]) -> bool {
    runtime::remembered_failure(
        store.root(),
        SUBJECT.files[0].sha256,
        Provider::WebGpu,
        providers,
    )
}

/// [`model_for`]'s choice, given whether WebGPU is on offer, what the
/// store has, what cannot be fetched this session, and whether the
/// original is on record as having failed on WebGPU (only ever asked
/// of `SUBJECT`; a store with neither file, or a CPU-only launch,
/// never calls it).
pub fn pick(
    webgpu: bool,
    have: impl Fn(&Model) -> bool,
    unavailable: impl Fn(&Model) -> bool,
    original_failed_on_webgpu: impl Fn(&Model) -> bool,
) -> &'static Model {
    if webgpu && have(&SUBJECT_WEBGPU) {
        &SUBJECT_WEBGPU
    } else if have(&SUBJECT) {
        if webgpu && original_failed_on_webgpu(&SUBJECT) && !unavailable(&SUBJECT_WEBGPU) {
            &SUBJECT_WEBGPU
        } else {
            &SUBJECT
        }
    } else if have(&SUBJECT_WEBGPU) || (webgpu && !unavailable(&SUBJECT_WEBGPU)) {
        &SUBJECT_WEBGPU
    } else {
        &SUBJECT
    }
}

impl Subject {
    /// Load from the store, on the first provider that runs it: the
    /// file [`model_for`] picks.
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        Self::load_model(store, model_for(Some(store), providers, &[]), providers)
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

    fn id(webgpu: bool, have: &[&str], unavailable: &[&str]) -> &'static str {
        id_with(webgpu, have, unavailable, false)
    }

    /// `id`, with whether the original is on record as having failed
    /// on WebGPU.
    fn id_with(webgpu: bool, have: &[&str], unavailable: &[&str], failed: bool) -> &'static str {
        pick(
            webgpu,
            |m| have.contains(&m.id),
            |m| unavailable.contains(&m.id),
            |_| failed,
        )
        .id
    }

    #[test]
    fn what_the_store_has_comes_first() {
        let (rw, orig) = (SUBJECT_WEBGPU.id, SUBJECT.id);
        assert_eq!(id(true, &[rw, orig], &[]), rw);
        assert_eq!(id(false, &[rw, orig], &[]), orig);
        // One of the two in the store: that one, on either machine,
        // rather than a second download of the same weights.
        assert_eq!(id(true, &[orig], &[]), orig);
        assert_eq!(id(false, &[rw], &[]), rw);
    }

    #[test]
    fn with_neither_the_rewrite_is_offered_only_where_webgpu_runs() {
        assert_eq!(id(true, &[], &[]), SUBJECT_WEBGPU.id);
        assert_eq!(id(false, &[], &[]), SUBJECT.id);
    }

    #[test]
    fn a_rewrite_that_cannot_be_had_gives_way_to_the_original() {
        assert_eq!(id(true, &[], &[SUBJECT_WEBGPU.id]), SUBJECT.id);
        // Its being unavailable to fetch does not matter once it is here.
        assert_eq!(
            id(true, &[SUBJECT_WEBGPU.id], &[SUBJECT_WEBGPU.id]),
            SUBJECT_WEBGPU.id
        );
    }

    /// An install from before the rewrite shipped: the store has only
    /// the original. On WebGPU, with the original on record as having
    /// failed there, the rewrite is offered instead — the same weights
    /// are not fetched twice for a card that runs the original fine,
    /// but a card that cannot is not left at three seconds a mask
    /// forever.
    #[test]
    fn an_original_on_record_as_failing_on_webgpu_offers_the_rewrite() {
        let orig = SUBJECT.id;
        assert_eq!(
            id_with(true, &[orig], &[], true),
            SUBJECT_WEBGPU.id,
            "the original fails here: the rewrite is worth fetching"
        );
    }

    /// The same store, with nothing on record: the original stands,
    /// since a card that runs it needs no second download.
    #[test]
    fn an_original_with_no_record_of_failing_keeps_the_original() {
        let orig = SUBJECT.id;
        assert_eq!(id_with(true, &[orig], &[], false), orig);
        // Off WebGPU the record does not even apply.
        assert_eq!(id_with(false, &[orig], &[], true), orig);
    }

    /// The rewrite declined (or its fetch failed) this session: the
    /// original stands even though it is on record as failing here,
    /// same as when there was nothing in the store at all.
    #[test]
    fn a_declined_rewrite_falls_back_to_the_original() {
        let orig = SUBJECT.id;
        assert_eq!(id_with(true, &[orig], &[SUBJECT_WEBGPU.id], true), orig);
    }

    #[test]
    fn with_no_store_the_preference_stands() {
        assert_eq!(
            model_for(None, &[Provider::WebGpu, Provider::Cpu], &[]).id,
            SUBJECT_WEBGPU.id
        );
        assert_eq!(model_for(None, &[Provider::Cpu], &[]).id, SUBJECT.id);
    }
}
