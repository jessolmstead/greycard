//! Time SAM 3 on one picture, on one provider, warm:
//! `sam3_bench MODELS_DIR cpu|webgpu IMAGE PHRASE.bin [RUNS] [ENCODER.onnx]`.
//!
//! The trial in `tools/ai/sam3_trial` judges SAM 3's masks; this says
//! how long the editor would wait for them. MODELS_DIR holds our
//! export (`sam3_image_encoder.onnx`, `sam3_decoder.onnx`); ENCODER
//! names another image encoder in it (the fp16 file). PHRASE.bin is
//! one preset as `presets.py` would store it, written flat by the
//! trial: 32 bytes of language mask (0 or 1) then 32 × 256 f32 of
//! language features, little-endian.
//!
//! The picture is squashed to 1008² (bilinear) and handed over as
//! uint8 planes, as the trial does. Both sessions are built and run
//! once before timing; then RUNS (default 5) encodes and RUNS decodes
//! are timed apart, a decode being one phrase on the encoded picture,
//! the 16 best queries' masks read back. The presence and the best score
//! are printed so two providers can be checked against each other.

use std::time::Instant;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};

const SIDE: usize = 1008;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: sam3_bench MODELS_DIR cpu|webgpu IMAGE PHRASE.bin [RUNS] [ENCODER.onnx]");
        std::process::exit(2);
    }
    let (dir, provider, picture, phrase) = (&a[1], a[2].as_str(), &a[3], &a[4]);
    let runs: usize = a.get(5).map(|r| r.parse().expect("RUNS")).unwrap_or(5);
    let encoder = a
        .get(6)
        .map(String::as_str)
        .unwrap_or("sam3_image_encoder.onnx");

    let rgb = image::open(picture)
        .expect("the picture")
        .resize_exact(
            SIDE as u32,
            SIDE as u32,
            image::imageops::FilterType::Triangle,
        )
        .to_rgb8();
    let mut planes = vec![0u8; 3 * SIDE * SIDE];
    for (i, p) in rgb.pixels().enumerate() {
        for c in 0..3 {
            planes[c * SIDE * SIDE + i] = p[c];
        }
    }

    let bytes = std::fs::read(phrase).expect("the phrase file");
    assert_eq!(
        bytes.len(),
        32 + 32 * 256 * 4,
        "a phrase file is 32 mask bytes and 32 × 256 f32"
    );
    let lang_mask: Vec<bool> = bytes[..32].iter().map(|&b| b != 0).collect();
    let lang_feat: Vec<f32> = bytes[32..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&c| f32::from_le_bytes(c))
        .collect();

    ort::init().with_name("sam3_bench").commit();
    let build = |file: &str| {
        let mut b = Session::builder()
            .unwrap()
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .unwrap();
        match provider {
            "cpu" => {}
            "webgpu" => {
                b = b
                    .with_execution_providers([ort::ep::WebGPU::default()
                        .build()
                        .error_on_failure()])
                    .unwrap()
            }
            other => panic!("no provider {other}"),
        }
        b.commit_from_file(format!("{dir}/{file}"))
            .expect("session")
    };
    let t = Instant::now();
    let mut enc = build(encoder);
    let mut dec = build("sam3_decoder.onnx");
    println!(
        "{encoder} + decoder on {provider}: sessions {:.2}s",
        t.elapsed().as_secs_f64()
    );

    let encode = |enc: &mut Session| -> Vec<(String, DynValue)> {
        let input = Tensor::from_array(([3usize, SIDE, SIDE], planes.clone())).unwrap();
        let out = enc.run(ort::inputs!["image" => input]).expect("encode");
        // Copied out of the run's outputs, as the editor would keep the
        // embedding for the next phrase.
        [
            "vision_pos_enc_2",
            "backbone_fpn_0",
            "backbone_fpn_1",
            "backbone_fpn_2",
        ]
        .iter()
        .map(|&n| {
            let (shape, data) = out[n].try_extract_tensor::<f32>().unwrap();
            let v = Tensor::from_array((shape.to_vec(), data.to_vec())).unwrap();
            (n.to_string(), v.into_dyn())
        })
        .collect()
    };
    let decode = |dec: &mut Session, feats: &[(String, DynValue)]| -> (f32, f32) {
        let mut inputs: Vec<(String, SessionInputValue)> =
            feats.iter().map(|(n, v)| (n.clone(), v.into())).collect();
        let owned: [(&str, DynValue); 5] = [
            (
                "language_mask",
                Tensor::from_array(([1usize, 32], lang_mask.clone()))
                    .unwrap()
                    .into_dyn(),
            ),
            (
                "language_features",
                Tensor::from_array(([32usize, 1, 256], lang_feat.clone()))
                    .unwrap()
                    .into_dyn(),
            ),
            (
                "box_coords",
                Tensor::from_array(([1usize, 1, 4], vec![0f32; 4]))
                    .unwrap()
                    .into_dyn(),
            ),
            (
                "box_labels",
                Tensor::from_array(([1usize, 1], vec![1i64]))
                    .unwrap()
                    .into_dyn(),
            ),
            (
                "box_masks",
                Tensor::from_array(([1usize, 1], vec![true]))
                    .unwrap()
                    .into_dyn(),
            ),
        ];
        inputs.extend(owned.into_iter().map(|(n, v)| (n.to_string(), v.into())));
        let out = dec.run(inputs).expect("decode");
        let (_, presence) = out["presence"].try_extract_tensor::<f32>().unwrap();
        let (_, scores) = out["scores"].try_extract_tensor::<f32>().unwrap();
        let (_, masks) = out["masks"].try_extract_tensor::<f32>().unwrap();
        assert_eq!(masks.len(), scores.len() * 288 * 288);
        (presence[0], scores[0])
    };

    let t = Instant::now();
    let feats = encode(&mut enc);
    let first_enc = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let (presence, best) = decode(&mut dec, &feats);
    println!(
        "first: encode {first_enc:.2}s, decode {:.2}s; presence {presence:.4}, best {best:.4}",
        t.elapsed().as_secs_f64()
    );

    let mut e = Vec::new();
    let mut d = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        let f = encode(&mut enc);
        e.push(t.elapsed().as_secs_f64());
        let t = Instant::now();
        decode(&mut dec, &f);
        d.push(t.elapsed().as_secs_f64());
    }
    println!("warm encode: {}", summary(&mut e));
    println!("warm decode: {}", summary(&mut d));
}

fn summary(t: &mut [f64]) -> String {
    let all = t
        .iter()
        .map(|t| format!("{t:.3}"))
        .collect::<Vec<_>>()
        .join(" ");
    t.sort_by(f64::total_cmp);
    format!("{all} ; median {:.3}s, min {:.3}s", t[t.len() / 2], t[0])
}
