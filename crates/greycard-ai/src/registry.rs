//! The models this build knows: where each comes from, what it weighs,
//! its hash, and its license. Nothing here is bundled; the store
//! fetches a model the first time it is wanted, after the license has
//! been shown.

/// One file of a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File {
    /// The name it keeps in the store. ONNX external data must keep
    /// the name the graph refers to.
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct License {
    pub name: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// Stable, also the store directory: changes when the files do.
    pub id: &'static str,
    pub name: &'static str,
    /// What it is for, in a few words, for a listing to print.
    pub purpose: &'static str,
    /// Who made it and where the weights were published.
    pub source: &'static str,
    pub license: License,
    /// For a file greycard publishes itself, changed from the
    /// upstream one: what changed, and the upstream license's notice,
    /// both written into the note beside the files.
    pub modified: Option<Modified>,
    pub files: &'static [File],
}

/// A model file greycard changed and publishes under the upstream
/// license.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modified {
    /// What greycard changed, in a sentence.
    pub what: &'static str,
    /// The upstream license's text as its copyright holders give it:
    /// the copyright line and the permission notice, which a license
    /// such as MIT requires to go with every copy.
    pub notice: &'static str,
}

impl Model {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    /// The note written beside the files.
    pub fn license_note(&self) -> String {
        let files: Vec<String> = self
            .files
            .iter()
            .map(|f| {
                format!(
                    "  {} ({} bytes, sha256 {})\n    from {}",
                    f.name, f.bytes, f.sha256, f.url
                )
            })
            .collect();
        let provenance = match self.modified {
            Some(m) => format!(
                "Downloaded by greycard on first use. greycard publishes these files, modified from the original under its license: {what}\n\nThe original's license:\n\n{notice}",
                what = m.what,
                notice = m.notice,
            ),
            None => {
                "Downloaded by greycard on first use. greycard does not redistribute these files."
                    .to_string()
            }
        };
        format!(
            "{name}\n\nLicense: {lic} <{licurl}>\nSource: {source}\n\nFiles:\n{files}\n\n{provenance}\n",
            name = self.name,
            lic = self.license.name,
            licurl = self.license.url,
            source = self.source,
            files = files.join("\n"),
        )
    }
}

/// BiRefNet lite: salient-object matting, for Subject and Background.
pub const SUBJECT: Model = Model {
    id: "birefnet-lite-2024-fp16",
    name: "BiRefNet lite (Zheng et al.), fp16, ONNX export by onnx-community",
    purpose: "the Subject and Background masks",
    source: "https://github.com/ZhengPeng7/BiRefNet and https://huggingface.co/onnx-community/BiRefNet_lite-ONNX",
    license: License {
        name: "MIT",
        url: "https://github.com/ZhengPeng7/BiRefNet/blob/main/LICENSE",
    },
    modified: None,
    files: &[File {
        name: "model_fp16.onnx",
        url: "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/main/onnx/model_fp16.onnx",
        bytes: 114_538_221,
        sha256: "d39b897ceb16ae654c1731f3dba0cf9b368d9cae74b5a57459b455cc8bfec402",
    }],
};

/// BiRefNet's license, as its repository gives it
/// (https://github.com/ZhengPeng7/BiRefNet/blob/main/LICENSE), for the
/// note beside the copy greycard publishes.
const BIREFNET_LICENSE: &str = "MIT License

Copyright (c) 2024 ZhengPeng

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
";

/// BiRefNet lite rewritten so ONNX Runtime's WebGPU provider runs all
/// of it (notes, "BiRefNet on WebGPU"): the same weights and, on the
/// CPU, the same answer to the bit. Made from `SUBJECT`'s file by
/// `tools/ai/birefnet_webgpu.py rewrite`, which gives these exact
/// bytes with the versions in `tools/ai/requirements.txt`. What the
/// Subject mask prefers where WebGPU is on offer; see
/// `subject::model_for`.
pub const SUBJECT_WEBGPU: Model = Model {
    id: "birefnet-lite-2024-fp16-webgpu",
    name: "BiRefNet lite (Zheng et al.), fp16, ONNX export by onnx-community, rewritten by greycard for WebGPU",
    purpose: "the Subject and Background masks, on the GPU",
    source: "https://github.com/ZhengPeng7/BiRefNet and https://huggingface.co/onnx-community/BiRefNet_lite-ONNX, rewritten by https://github.com/jessolmstead/greycard (tools/ai/birefnet_webgpu.py)",
    license: License {
        name: "MIT",
        url: "https://github.com/ZhengPeng7/BiRefNet/blob/main/LICENSE",
    },
    modified: Some(Modified {
        what: "the decoder's Splits of more than fifteen outputs are Slices, the Sums are Adds, and the deformable convolutions' int64 coordinate arithmetic is fp16; the weights are untouched.",
        notice: BIREFNET_LICENSE,
    }),
    files: &[File {
        name: "model_fp16_webgpu.onnx",
        url: "https://huggingface.co/jessolmstead/BiRefNet_lite-ONNX-webgpu/resolve/main/model_fp16_webgpu.onnx",
        bytes: 113_778_088,
        sha256: "0a019d6ba73c9cedc9a251f8c9390b196ff6399acd281a2872692861abbd78c2",
    }],
};

/// SAM 2.1 Hiera small: segment anything from points and boxes, for
/// Objects. The encoder runs once an image, the decoder once a prompt.
pub const SAM: Model = Model {
    id: "sam2.1-hiera-small-fp32",
    name: "Segment Anything 2.1, Hiera small (Meta), ONNX export by onnx-community",
    purpose: "the Object mask, from points and boxes",
    source: "https://github.com/facebookresearch/sam2 and https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX",
    license: License {
        name: "Apache-2.0",
        url: "https://github.com/facebookresearch/sam2/blob/main/LICENSE",
    },
    modified: None,
    files: &[
        File {
            name: "vision_encoder.onnx",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/main/onnx/vision_encoder.onnx",
            bytes: 467_440,
            sha256: "aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119",
        },
        File {
            name: "vision_encoder.onnx_data",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/main/onnx/vision_encoder.onnx_data",
            bytes: 162_476_288,
            sha256: "260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456",
        },
        File {
            name: "prompt_encoder_mask_decoder.onnx",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/main/onnx/prompt_encoder_mask_decoder.onnx",
            bytes: 213_114,
            sha256: "079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f",
        },
        File {
            name: "prompt_encoder_mask_decoder.onnx_data",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/main/onnx/prompt_encoder_mask_decoder.onnx_data",
            bytes: 20_958_208,
            sha256: "f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864",
        },
    ],
};

/// EoMT's license, as its repository gives it
/// (https://github.com/tue-mps/eomt/blob/master/LICENSE), for the note
/// beside the export greycard publishes.
const EOMT_LICENSE: &str = "MIT License

Copyright (c) 2025 Mobile Perception Systems Lab at TU/e

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
";

/// EoMT-S (Kerssies et al.), COCO panoptic at 640: every pixel's class
/// among COCO's 133, of which the Sky shape reads the sky
/// (`sky::SKY_CLASS`). greycard's ONNX export of the published weights,
/// made by `tools/ai/eomt_export.py`, which gives these exact bytes
/// with the versions in `tools/ai/requirements.txt`. Its DINOv2
/// backbone is Meta's, under Apache-2.0.
pub const SKY: Model = Model {
    id: "eomt-s-coco-panoptic-640",
    name: "EoMT-S (Kerssies et al.), COCO panoptic 640, ONNX export by greycard",
    purpose: "the Sky mask: where the picture's sky is",
    source: "https://github.com/tue-mps/eomt and https://huggingface.co/tue-mps/coco_panoptic_eomt_small_640_2x (backbone: DINOv2, https://github.com/facebookresearch/dinov2, Apache-2.0), exported by https://github.com/jessolmstead/greycard (tools/ai/eomt_export.py)",
    license: License {
        name: "MIT",
        url: "https://github.com/tue-mps/eomt/blob/master/LICENSE",
    },
    modified: Some(Modified {
        what: "an ONNX export of the published weights (revision 10f5326) at a fixed 640 by 640 input, traced from the transformers port; the weights are untouched.",
        notice: EOMT_LICENSE,
    }),
    files: &[File {
        name: "eomt-s-coco-640.onnx",
        url: "https://huggingface.co/jessolmstead/greycard-sky/resolve/main/eomt-s-coco-640.onnx",
        bytes: 96_025_182,
        sha256: "805ed0fb363784811a7f3979335c2c64a7d40f91ae943326f79a56ff477f372d",
    }],
};

/// LaMa (big-lama): fills a hole from what is around it, for the
/// eraser. A fixed 512×512 square in and out.
pub const FILL: Model = Model {
    id: "big-lama-carve-fp32",
    name: "LaMa (Suvorov et al.), big-lama, ONNX export by Carve",
    purpose: "the eraser: what was behind what it removes",
    source: "https://github.com/advimman/lama and https://huggingface.co/Carve/LaMa-ONNX",
    license: License {
        name: "Apache-2.0",
        url: "https://github.com/advimman/lama/blob/main/LICENSE",
    },
    modified: None,
    files: &[File {
        name: "lama_fp32.onnx",
        url: "https://huggingface.co/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx",
        bytes: 208_044_816,
        sha256: "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6",
    }],
};

const DENOISE_LICENSE: License = License {
    name: "GPL-3.0-or-later",
    url: "https://www.gnu.org/licenses/gpl-3.0.html",
};
const DENOISE_SOURCE: &str = "https://github.com/jessolmstead/greycard (tools/denoise) and https://huggingface.co/jessolmstead/greycard-denoise";

/// greycard's raw-domain denoiser, the speed tier: v19, 1.2 M
/// parameters, about a second of network on a 24 MP frame.
pub const DENOISE_FAST: Model = Model {
    id: "greycard-denoise-fast-v19",
    name: "greycard denoise, fast (v19)",
    purpose: "learned denoise and demosaic, the speed tier",
    source: DENOISE_SOURCE,
    license: DENOISE_LICENSE,
    modified: None,
    files: &[File {
        name: "denoise-v19.onnx",
        url: "https://huggingface.co/jessolmstead/greycard-denoise/resolve/main/denoise-v19.onnx",
        bytes: 4_932_834,
        sha256: "319d024a39d3e31c95e99c91bb5c089691707cbf13f4f077c1c6e49d10235c49",
    }],
};

/// The middle tier: v15, 2.3 M parameters, about two seconds.
pub const DENOISE_BALANCED: Model = Model {
    id: "greycard-denoise-balanced-v15",
    name: "greycard denoise, balanced (v15)",
    purpose: "learned denoise and demosaic, the middle tier",
    source: DENOISE_SOURCE,
    license: DENOISE_LICENSE,
    modified: None,
    files: &[File {
        name: "denoise-v15.onnx",
        url: "https://huggingface.co/jessolmstead/greycard-denoise/resolve/main/denoise-v15.onnx",
        bytes: 9_037_296,
        sha256: "f5845119e0b0ee63c57aad38129aed1c8925e60ec9b7838c6b7a0fa23a84a546",
    }],
};

/// The quality tier: v20, 5.3 M parameters, about three seconds.
pub const DENOISE_BEST: Model = Model {
    id: "greycard-denoise-best-v20",
    name: "greycard denoise, best (v20)",
    purpose: "learned denoise and demosaic, the quality tier",
    source: DENOISE_SOURCE,
    license: DENOISE_LICENSE,
    modified: None,
    files: &[File {
        name: "denoise-v20.onnx",
        url: "https://huggingface.co/jessolmstead/greycard-denoise/resolve/main/denoise-v20.onnx",
        bytes: 20_297_204,
        sha256: "c20aed71a45b2a984570e09d8456c54ce8eff43a334b54c7ebb480c21518f9be",
    }],
};

pub const MODELS: &[Model] = &[
    SUBJECT,
    SUBJECT_WEBGPU,
    SAM,
    SKY,
    FILL,
    DENOISE_FAST,
    DENOISE_BALANCED,
    DENOISE_BEST,
];

/// The denoiser's tiers by the names an edit and the CLI use.
pub const DENOISERS: &[(&str, &Model)] = &[
    ("fast", &DENOISE_FAST),
    ("balanced", &DENOISE_BALANCED),
    ("best", &DENOISE_BEST),
];

/// The denoiser tier called `name`, if there is one.
pub fn denoiser(name: &str) -> Option<&'static Model> {
    DENOISERS.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
}

/// The registered model with this id, if there is one.
pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

/// The denoiser tier a model is, if it is one.
pub fn tier_of(model: &Model) -> Option<&'static str> {
    DENOISERS
        .iter()
        .find(|(_, m)| m.id == model.id)
        .map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HF: &str = "https://huggingface.co";

    #[test]
    fn the_registry_is_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for m in MODELS {
            assert!(ids.insert(m.id), "duplicate id {}", m.id);
            assert!(!m.files.is_empty());
            for f in m.files {
                assert!(f.url.starts_with(HF), "{} is not on the model host", f.url);
                assert!(
                    f.url.ends_with(f.name),
                    "{} must keep its name {}",
                    f.url,
                    f.name
                );
                assert_eq!(f.sha256.len(), 64);
                assert!(
                    f.sha256
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                );
                assert!(f.bytes > 0);
            }
            assert!(m.license.url.starts_with("https://"));
            if let Some(modified) = m.modified {
                assert!(
                    !modified.what.is_empty(),
                    "{} says nothing of what changed",
                    m.id
                );
                assert!(
                    modified.notice.contains("Copyright"),
                    "{} carries no copyright notice",
                    m.id
                );
            }
            assert!(!m.purpose.is_empty(), "{} says nothing it is for", m.id);
            assert_eq!(model(m.id), Some(m), "{} is not found by its id", m.id);
        }
        assert!(model("no-such-model").is_none());
    }

    #[test]
    fn the_denoiser_tiers_are_in_the_registry() {
        for (name, model) in DENOISERS {
            assert_eq!(denoiser(name).map(|m| m.id), Some(model.id));
            assert!(
                MODELS.iter().any(|m| m.id == model.id),
                "{name} not in MODELS"
            );
            assert_eq!(model.files.len(), 1, "a denoiser is one file");
            assert_eq!(tier_of(model), Some(*name));
        }
        assert!(tier_of(&SUBJECT).is_none());
        assert!(denoiser("off").is_none());
    }

    #[test]
    fn the_note_names_the_license_and_every_file() {
        let note = SAM.license_note();
        assert!(note.contains("Apache-2.0"));
        for f in SAM.files {
            assert!(note.contains(f.name));
            assert!(note.contains(f.sha256));
        }
        assert!(note.contains("does not redistribute"));

        let note = SUBJECT_WEBGPU.license_note();
        assert!(note.contains("MIT"));
        assert!(note.contains("ZhengPeng7/BiRefNet"));
        assert!(note.contains("onnx-community"));
        assert!(note.contains("modified from the original"));
        // MIT wants the copyright line and the permission notice with
        // every copy, not a link to them.
        assert!(note.contains("Copyright (c) 2024 ZhengPeng"));
        assert!(note.contains("The above copyright notice and this permission notice"));
        assert!(!note.contains("does not redistribute"));

        let note = SKY.license_note();
        assert!(note.contains("tue-mps/eomt"));
        assert!(note.contains("Copyright (c) 2025 Mobile Perception Systems Lab at TU/e"));
        assert!(note.contains("The above copyright notice and this permission notice"));
        assert!(note.contains("eomt_export.py"));
    }
}
