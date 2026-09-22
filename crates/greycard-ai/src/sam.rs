//! Objects: Segment Anything 2.1 from points and boxes. The image is
//! embedded once, then every prompt is a decoder call of a few
//! milliseconds, which is what makes click-to-mask feel instant.

use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::image::{IMAGENET_MEAN, IMAGENET_STD, Mask, Rgb8};
use crate::registry::SAM;
use crate::runtime::{self, Error, Loaded, Provider, Result};
use crate::store::Store;

/// The model's square.
pub const SIZE: usize = 1024;
/// The decoder's mask side.
pub const MASK: usize = 256;

/// The three feature maps the encoder gives the decoder.
pub struct Embedding {
    high: Vec<f32>,
    mid: Vec<f32>,
    low: Vec<f32>,
}

const HIGH: [usize; 4] = [1, 32, 256, 256];
const MID: [usize; 4] = [1, 64, 128, 128];
const LOW: [usize; 4] = [1, 256, 64, 64];

/// A prompt, in image fractions: (0, 0) top left, (1, 1) bottom right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prompt {
    /// A click: inside the object, or (negative) outside it.
    Point { x: f32, y: f32, positive: bool },
    /// A drag: the object's box.
    Box { x0: f32, y0: f32, x1: f32, y1: f32 },
}

pub struct Sam {
    encoder: Loaded,
    decoder: Loaded,
}

impl Sam {
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        if !store.have(&SAM) {
            return Err(Error::Missing(SAM.name));
        }
        // Level1: ORT 1.28's transpose optimizer throws on this export
        // at higher levels.
        let level = GraphOptimizationLevel::Level1;
        let encoder = runtime::open(
            &store.path(&SAM, &SAM.files[0]),
            level,
            providers,
            Some(runtime::Remembered {
                store_root: store.root(),
                model: SAM.id,
                hash: SAM.files[0].sha256,
            }),
            |s| {
                let zeros =
                    Tensor::from_array(([1usize, 3, SIZE, SIZE], vec![0.0f32; 3 * SIZE * SIZE]))?;
                s.run(ort::inputs!["pixel_values" => zeros])?;
                Ok(())
            },
        )?;
        let decoder = runtime::open(
            &store.path(&SAM, &SAM.files[2]),
            level,
            providers,
            Some(runtime::Remembered {
                store_root: store.root(),
                model: SAM.id,
                hash: SAM.files[2].sha256,
            }),
            |s| {
                let feed = decoder_inputs(
                    &Embedding {
                        high: vec![0.0; HIGH.iter().product()],
                        mid: vec![0.0; MID.iter().product()],
                        low: vec![0.0; LOW.iter().product()],
                    },
                    &[Prompt::Point {
                        x: 0.5,
                        y: 0.5,
                        positive: true,
                    }],
                )?;
                s.run(feed)?;
                Ok(())
            },
        )?;
        Ok(Self { encoder, decoder })
    }

    pub fn providers(&self) -> (Provider, Provider) {
        (self.encoder.provider, self.decoder.provider)
    }

    /// Embed the image: the slow half, once an image.
    pub fn embed(&mut self, image: &Rgb8) -> Result<Embedding> {
        let planes = image.to_planes(SIZE, IMAGENET_MEAN, IMAGENET_STD);
        let input = Tensor::from_array(([1usize, 3, SIZE, SIZE], planes))?;
        let outputs = self
            .encoder
            .session
            .run(ort::inputs!["pixel_values" => input])?;
        let take = |name: &str, dims: [usize; 4]| -> Result<Vec<f32>> {
            let (shape, data) = outputs[name].try_extract_tensor::<f32>()?;
            if data.len() != dims.iter().product::<usize>() {
                return Err(Error::Shape(format!("{name}: {shape}")));
            }
            Ok(data.to_vec())
        };
        Ok(Embedding {
            high: take("image_embeddings.0", HIGH)?,
            mid: take("image_embeddings.1", MID)?,
            low: take("image_embeddings.2", LOW)?,
        })
    }

    /// The mask for `prompts` on an embedded image, `MASK`×`MASK` in
    /// the model's stretched square, and the model's own estimate of
    /// how good it is (the worst over the objects).
    ///
    /// The decoder answers one object a call: a box, with the clicks
    /// inside it, or the clicks in no box. Several boxes are several
    /// objects, decoded one at a time and joined; passing them in one
    /// call makes the model answer with one mask a box, and with clicks
    /// as well it cannot shape its prompt at all.
    pub fn decode(&mut self, embedding: &Embedding, prompts: &[Prompt]) -> Result<(Mask, f32)> {
        let objects = objects(prompts);
        if objects.is_empty() {
            return Err(Error::Shape("no prompts".into()));
        }
        let mut logits = vec![f32::NEG_INFINITY; MASK * MASK];
        let mut score = f32::INFINITY;
        for object in &objects {
            let (l, s) = self.decode_one(embedding, object)?;
            for (a, b) in logits.iter_mut().zip(l) {
                *a = a.max(b);
            }
            score = score.min(s);
        }
        Ok((Mask::from_logits(MASK, MASK, &logits), score))
    }

    /// One object's mask as logits, and its score.
    fn decode_one(&mut self, embedding: &Embedding, prompts: &[Prompt]) -> Result<(Vec<f32>, f32)> {
        let feed = decoder_inputs(embedding, prompts)?;
        let outputs = self.decoder.session.run(feed)?;
        let (_, scores) = outputs["iou_scores"].try_extract_tensor::<f32>()?;
        let (shape, masks) = outputs["pred_masks"].try_extract_tensor::<f32>()?;
        let n = MASK * MASK;
        if masks.len() != 3 * n || scores.len() != 3 {
            return Err(Error::Shape(format!("pred_masks {shape}")));
        }
        // Three candidates for the ambiguity of a prompt; take the one
        // the model rates best.
        let best = (0..3)
            .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
            .expect("three candidates");
        Ok((masks[best * n..(best + 1) * n].to_vec(), scores[best]))
    }
}

/// The prompts as objects for the decoder: each box with the clicks
/// inside it (a click in two boxes goes to both), then the clicks in
/// no box, if a positive one is among them; negatives alone say
/// nothing.
fn objects(prompts: &[Prompt]) -> Vec<Vec<Prompt>> {
    let boxes: Vec<Prompt> = prompts
        .iter()
        .filter(|p| matches!(p, Prompt::Box { .. }))
        .copied()
        .collect();
    let mut objects: Vec<Vec<Prompt>> = boxes.iter().map(|b| vec![*b]).collect();
    let mut loose = Vec::new();
    for p in prompts {
        let Prompt::Point { x, y, .. } = *p else {
            continue;
        };
        let mut inside = false;
        for (object, b) in objects.iter_mut().zip(&boxes) {
            if let Prompt::Box { x0, y0, x1, y1 } = *b
                && (x0..=x1).contains(&x)
                && (y0..=y1).contains(&y)
            {
                object.push(*p);
                inside = true;
            }
        }
        if !inside {
            loose.push(*p);
        }
    }
    if loose
        .iter()
        .any(|p| matches!(p, Prompt::Point { positive: true, .. }))
    {
        objects.push(loose);
    }
    objects
}

type Feed = Vec<(&'static str, ort::value::DynValue)>;

fn decoder_inputs(embedding: &Embedding, prompts: &[Prompt]) -> ort::Result<Feed> {
    let mut points = Vec::new();
    let mut labels = Vec::new();
    let mut boxes = Vec::new();
    for p in prompts {
        match *p {
            Prompt::Point { x, y, positive } => {
                points.extend([x * SIZE as f32, y * SIZE as f32]);
                labels.push(if positive { 1i64 } else { 0 });
            }
            Prompt::Box { x0, y0, x1, y1 } => {
                boxes.extend([x0, y0, x1, y1].map(|v| v * SIZE as f32));
            }
        }
    }
    let np = labels.len();
    let nb = boxes.len() / 4;
    Ok(vec![
        (
            "input_points",
            Tensor::from_array(([1usize, 1, np, 2], points))?.into_dyn(),
        ),
        (
            "input_labels",
            Tensor::from_array(([1usize, 1, np], labels))?.into_dyn(),
        ),
        (
            "input_boxes",
            Tensor::from_array(([1usize, nb, 4], boxes))?.into_dyn(),
        ),
        (
            "image_embeddings.0",
            Tensor::from_array((HIGH, embedding.high.clone()))?.into_dyn(),
        ),
        (
            "image_embeddings.1",
            Tensor::from_array((MID, embedding.mid.clone()))?.into_dyn(),
        ),
        (
            "image_embeddings.2",
            Tensor::from_array((LOW, embedding.low.clone()))?.into_dyn(),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, y: f32, positive: bool) -> Prompt {
        Prompt::Point { x, y, positive }
    }

    #[test]
    fn boxes_are_objects_and_clicks_go_to_the_box_they_are_in() {
        let a = Prompt::Box {
            x0: 0.0,
            y0: 0.0,
            x1: 0.4,
            y1: 0.4,
        };
        let b = Prompt::Box {
            x0: 0.5,
            y0: 0.5,
            x1: 0.9,
            y1: 0.9,
        };
        let objects = objects(&[
            point(0.1, 0.1, true),
            a,
            point(0.7, 0.7, false),
            b,
            point(0.45, 0.95, true),
            point(0.2, 0.9, false),
        ]);
        assert_eq!(
            objects,
            vec![
                vec![a, point(0.1, 0.1, true)],
                vec![b, point(0.7, 0.7, false)],
                vec![point(0.45, 0.95, true), point(0.2, 0.9, false)],
            ]
        );
    }

    #[test]
    fn negatives_in_no_box_make_no_object() {
        let a = Prompt::Box {
            x0: 0.0,
            y0: 0.0,
            x1: 0.4,
            y1: 0.4,
        };
        assert_eq!(objects(&[a, point(0.8, 0.8, false)]), vec![vec![a]]);
        assert!(objects(&[point(0.8, 0.8, false)]).is_empty());
        assert_eq!(
            objects(&[point(0.8, 0.8, true)]),
            vec![vec![point(0.8, 0.8, true)]]
        );
    }
}
