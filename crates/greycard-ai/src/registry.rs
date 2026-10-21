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

/// The SAM License, which goes with every copy of SAM 3 and of what is
/// made from it: Meta's copyright line, as its source files carry it,
/// and the agreement as `facebookresearch/sam3` gives it.
const SAM_LICENSE: &str = concat!(
    "Copyright (c) Meta Platforms, Inc. and affiliates. All Rights Reserved\n\n",
    include_str!("sam_license.txt"),
);

/// SAM 3 (Meta): segment by a phrase, for the People mask. Our export
/// of Meta's `sam3.pt` (`tools/ai/sam3_trial/export.py` and
/// `fp16.py`): the image encoder in fp16, once a picture or a crop;
/// the decoder once a phrase; and the phrases' decoder inputs as a
/// table (`table.py`), so no text encoder ships. See `sam3`.
pub const SAM3: Model = Model {
    id: "sam3-fp16-top16-1",
    name: "SAM 3 (Meta), fp16 image encoder, ONNX export by greycard",
    purpose: "the People mask: a person, or a part of one, asked for by name",
    source: "https://github.com/facebookresearch/sam3 and https://huggingface.co/facebook/sam3, exported by https://github.com/jessolmstead/greycard (tools/ai/sam3_trial/export.py, fp16.py, table.py)",
    license: License {
        name: "SAM License",
        url: "https://github.com/facebookresearch/sam3/blob/main/LICENSE",
    },
    modified: Some(Modified {
        what: "converted to ONNX (with Kentaro Wada's traceable rotary encoding, wkentaro/sam3 at 812de8a), the image encoder converted to fp16, the decoder made to return the sixteen best queries with the presence score apart, and a fixed set of phrases run through the language encoder once and stored as a table; the weights are otherwise untouched.",
        notice: SAM_LICENSE,
    }),
    files: &[
        File {
            name: "sam3_image_encoder_fp16.onnx",
            url: "https://huggingface.co/jessolmstead/greycard-sam3/resolve/main/sam3_image_encoder_fp16.onnx",
            bytes: 908_762_234,
            sha256: "dd4733a6882264d0ab00599acd6ece3dd700f9b61b7976369b956ce41d88c23d",
        },
        File {
            name: "sam3_decoder.onnx",
            url: "https://huggingface.co/jessolmstead/greycard-sam3/resolve/main/sam3_decoder.onnx",
            bytes: 129_854_990,
            sha256: "7c205232b0698d532731fd5fa206cdea694717de69c0d8e88997a14027425564",
        },
        File {
            name: "sam3_presets.bin",
            url: "https://huggingface.co/jessolmstead/greycard-sam3/resolve/main/sam3_presets.bin",
            bytes: 524_962,
            sha256: "a1c205ef7e4374a6a9072928339d957eb78f78efa01b5cf170fe0f6ff92d6917",
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

/// LaMa's license, as its repository gives it
/// (https://github.com/advimman/lama/blob/main/LICENSE), which
/// Apache-2.0 wants with every copy of what is made from it.
const LAMA_LICENSE: &str = include_str!("lama_license.txt");

/// LaMa (big-lama): fills a hole from what is around it, for the
/// eraser. greycard's export of the published checkpoint at any height
/// and width that are multiples of 8, its Fourier transforms written
/// as matrix products so it runs whole on WebGPU. It replaced Carve's
/// fixed 512 export (`big-lama-carve-fp32`), whose file a store that
/// has it keeps in its own directory, unused.
pub const FILL: Model = Model {
    id: "big-lama-dynamic-fp32",
    name: "LaMa (Suvorov et al.), big-lama, ONNX export by greycard",
    purpose: "the eraser: what was behind what it removes",
    source: "https://github.com/advimman/lama (big-lama), exported by https://github.com/jessolmstead/greycard (tools/ai/lama_export.py) and published at https://huggingface.co/jessolmstead/greycard-lama",
    license: License {
        name: "Apache-2.0",
        url: "https://github.com/advimman/lama/blob/main/LICENSE",
    },
    modified: Some(Modified {
        what: "an ONNX export (opset 21, fp32) of the generator in big-lama's checkpoint with a dynamic height and width, its rfftn and irfftn written as the same transforms by matrix products, and LaMa's own masking and compositing around it; the weights are untouched.",
        notice: LAMA_LICENSE,
    }),
    files: &[File {
        name: "big-lama-dynamic-fp32.onnx",
        url: "https://huggingface.co/jessolmstead/greycard-lama/resolve/main/big-lama-dynamic-fp32.onnx",
        bytes: 209_024_270,
        sha256: "5f39f5e80b91596fd50f3227858c97912dabc0ffbd4796f21ca688529abb8771",
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
    SAM3,
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

/// ONNX Runtime as Microsoft publishes it, for a processor the one
/// greycard ships cannot run on, or a platform greycard ships none
/// for: an archive fetched on first use, as a model is, from which the
/// store keeps the library and the files that go with it. CPU only
/// (Microsoft's releases carry no WebGPU), with the faster kernels
/// chosen at run time, so the x86-64 builds run on any 64-bit Intel or
/// AMD processor. The version is the one `ort` binds and greycard's
/// own library is made from (pyke's `ms@1.28.0`), so either answers
/// the same API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    /// Stable, also the store directory: changes when the files do.
    pub id: &'static str,
    pub name: &'static str,
    /// What it is for, for a listing and the note beside the files:
    /// "the models on ...".
    pub purpose: &'static str,
    /// The release the archive is from.
    pub source: &'static str,
    pub license: License,
    /// The license's text as the archive carries it, for the note.
    pub notice: &'static str,
    /// The archive as published: its name, URL, size and hash.
    pub archive: File,
    pub packing: Packing,
    /// The library, kept as [`crate::runtime::LIBRARY`] is named.
    pub library: Member,
    /// The files kept beside it: the license and the third-party
    /// notices, which go with every copy of the library.
    pub kept: &'static [Member],
}

/// How a runtime's archive is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packing {
    /// A gzipped tar, as the Linux release is.
    Tgz,
    /// A zip of deflated members, as the Windows releases are.
    Zip,
}

/// One file inside a runtime's archive that the store keeps: its path
/// in the archive, the name it keeps, and its size and hash, checked
/// as it is taken out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    pub path: &'static str,
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

impl Runtime {
    /// What is kept once it is unpacked: the library and the files
    /// beside it.
    pub fn bytes(&self) -> u64 {
        self.members().map(|m| m.bytes).sum()
    }

    /// Every member taken out of the archive, the library first.
    pub fn members(&self) -> impl Iterator<Item = &Member> {
        std::iter::once(&self.library).chain(self.kept)
    }

    /// The note written beside the files.
    pub fn license_note(&self) -> String {
        let members: Vec<String> = self
            .members()
            .map(|m| {
                format!(
                    "  {} ({} bytes, sha256 {})\n    {} in the archive",
                    m.name, m.bytes, m.sha256, m.path
                )
            })
            .collect();
        format!(
            "{name}\n\nLicense: {lic} <{licurl}>\nSource: {source}\n\nFrom {archive} ({bytes} bytes, sha256 {sha})\n  {url}\n\nFiles:\n{members}\n\nDownloaded by greycard and kept as Microsoft published it, to run {purpose}. greycard does not redistribute these files. The license:\n\n{notice}",
            name = self.name,
            purpose = self.purpose,
            lic = self.license.name,
            licurl = self.license.url,
            source = self.source,
            archive = self.archive.name,
            bytes = self.archive.bytes,
            sha = self.archive.sha256,
            url = self.archive.url,
            members = members.join("\n"),
            notice = self.notice,
        )
    }
}

/// ONNX Runtime's license, as Microsoft's archives carry it (`LICENSE`
/// at the top of each; the Windows one has CRLF line ends).
const ONNXRUNTIME_LICENSE: &str = "MIT License

Copyright (c) Microsoft Corporation

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

const ONNXRUNTIME_SOURCE: &str = "https://github.com/microsoft/onnxruntime/releases/tag/v1.28.0";
const ONNXRUNTIME_MIT: License = License {
    name: "MIT",
    url: "https://github.com/microsoft/onnxruntime/blob/v1.28.0/LICENSE",
};

/// Microsoft's ONNX Runtime 1.28.0 for Linux on x86-64, glibc 2.27 or
/// later. The library is `libonnxruntime.so.1.28.0` (the `.so` and
/// `.so.1` beside it in the archive are links to it). Nothing else in
/// the archive is wanted: the providers' shared library is for the
/// CUDA and TensorRT plugins, which this build does not use.
pub const ONNXRUNTIME_CPU_LINUX_X64: Runtime = Runtime {
    id: "onnxruntime-1.28.0-cpu-linux-x64",
    name: "ONNX Runtime 1.28.0 by Microsoft, for the CPU",
    purpose: "the models on a processor greycard's own ONNX Runtime cannot run on (one without AVX2)",
    source: ONNXRUNTIME_SOURCE,
    license: ONNXRUNTIME_MIT,
    notice: ONNXRUNTIME_LICENSE,
    archive: File {
        name: "onnxruntime-linux-x64-1.28.0.tgz",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-1.28.0.tgz",
        bytes: 9_125_960,
        sha256: "a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407",
    },
    packing: Packing::Tgz,
    library: Member {
        path: "onnxruntime-linux-x64-1.28.0/lib/libonnxruntime.so.1.28.0",
        name: "libonnxruntime.so",
        bytes: 24_268_848,
        sha256: "1461ef7cc3d9e49982591721683cc3e3a55580aeca9a5254e7aac47b75ee4bab",
    },
    kept: &[
        Member {
            path: "onnxruntime-linux-x64-1.28.0/LICENSE",
            name: "LICENSE",
            bytes: 1_073,
            sha256: "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
        },
        Member {
            path: "onnxruntime-linux-x64-1.28.0/ThirdPartyNotices.txt",
            name: "ThirdPartyNotices.txt",
            bytes: 325_054,
            sha256: "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
        },
    ],
};

/// The same for Windows on x86-64. The zip is mostly the library's
/// debug symbols (`onnxruntime.pdb`, 408 MB unpacked), which are left
/// in it; `onnxruntime.dll` needs nothing but the system's own DLLs
/// and the C++ runtime greycard's package already ships.
pub const ONNXRUNTIME_CPU_WINDOWS_X64: Runtime = Runtime {
    id: "onnxruntime-1.28.0-cpu-windows-x64",
    name: "ONNX Runtime 1.28.0 by Microsoft, for the CPU",
    purpose: "the models on a processor greycard's own ONNX Runtime cannot run on (one without AVX2)",
    source: ONNXRUNTIME_SOURCE,
    license: ONNXRUNTIME_MIT,
    notice: ONNXRUNTIME_LICENSE,
    archive: File {
        name: "onnxruntime-win-x64-1.28.0.zip",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-win-x64-1.28.0.zip",
        bytes: 78_796_801,
        sha256: "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc",
    },
    packing: Packing::Zip,
    library: Member {
        path: "onnxruntime-win-x64-1.28.0/lib/onnxruntime.dll",
        name: "onnxruntime.dll",
        bytes: 15_809_848,
        sha256: "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257",
    },
    kept: &[
        Member {
            path: "onnxruntime-win-x64-1.28.0/LICENSE",
            name: "LICENSE",
            bytes: 1_094,
            sha256: "c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674",
        },
        Member {
            path: "onnxruntime-win-x64-1.28.0/ThirdPartyNotices.txt",
            name: "ThirdPartyNotices.txt",
            bytes: 331_175,
            sha256: "fb0af774b4d7cffc5b9d046f2aaeade2f37df2f80abf8033c95dfffcc77a8866",
        },
    ],
};

/// The same for Windows on ARM64, where greycard ships no ONNX
/// Runtime at all: pyke's one build for that target carries DirectML,
/// which greycard does not use, and none carries WebGPU, so there is
/// nothing to make greycard's own from, and this is the only one the
/// models run on there. The zip is laid out as the x86-64 one is,
/// debug symbols and all, and its license and notices are the same
/// files byte for byte.
pub const ONNXRUNTIME_CPU_WINDOWS_ARM64: Runtime = Runtime {
    id: "onnxruntime-1.28.0-cpu-windows-arm64",
    name: "ONNX Runtime 1.28.0 by Microsoft, for the CPU",
    purpose: "the models on Windows on ARM, which greycard ships no ONNX Runtime of its own for",
    source: ONNXRUNTIME_SOURCE,
    license: ONNXRUNTIME_MIT,
    notice: ONNXRUNTIME_LICENSE,
    archive: File {
        name: "onnxruntime-win-arm64-1.28.0.zip",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-win-arm64-1.28.0.zip",
        bytes: 79_812_663,
        sha256: "cbe4547463ece092b505c3581376ed5896d22b5429f39d5e645e425ecdd369ad",
    },
    packing: Packing::Zip,
    library: Member {
        path: "onnxruntime-win-arm64-1.28.0/lib/onnxruntime.dll",
        name: "onnxruntime.dll",
        bytes: 15_895_904,
        sha256: "102342326311ad62879b808840b4e3fd1372c41d8c871b59536d89a3c64d53f2",
    },
    kept: &[
        Member {
            path: "onnxruntime-win-arm64-1.28.0/LICENSE",
            name: "LICENSE",
            bytes: 1_094,
            sha256: "c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674",
        },
        Member {
            path: "onnxruntime-win-arm64-1.28.0/ThirdPartyNotices.txt",
            name: "ThirdPartyNotices.txt",
            bytes: 331_175,
            sha256: "fb0af774b4d7cffc5b9d046f2aaeade2f37df2f80abf8033c95dfffcc77a8866",
        },
    ],
};

/// Every CPU-only runtime the registry knows, whatever this build is.
pub const RUNTIMES: &[Runtime] = &[
    ONNXRUNTIME_CPU_LINUX_X64,
    ONNXRUNTIME_CPU_WINDOWS_X64,
    ONNXRUNTIME_CPU_WINDOWS_ARM64,
];

/// The CPU-only runtime for this build's platform: on Linux and
/// Windows on x86-64, where the one greycard ships may not run, and on
/// Windows on ARM64, where greycard ships none. None on a Mac, whose
/// package is for Apple silicon alone (which lacks nothing greycard's
/// library needs; Microsoft publishes no Intel Mac build of 1.28.0
/// either), and none on Linux on ARM, which has no package.
pub fn cpu_runtime() -> Option<&'static Runtime> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(&ONNXRUNTIME_CPU_LINUX_X64)
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some(&ONNXRUNTIME_CPU_WINDOWS_X64)
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        Some(&ONNXRUNTIME_CPU_WINDOWS_ARM64)
    } else {
        None
    }
}

/// The runtime with this id, if the registry has one.
pub fn runtime(id: &str) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|r| r.id == id)
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

        // Apache-2.0 wants the license with every copy.
        let note = FILL.license_note();
        assert!(note.contains("advimman/lama"));
        assert!(note.contains("Copyright [2021] Samsung Research"));
        assert!(note.contains("TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION"));
        assert!(note.contains("matrix products"));
        assert!(note.contains("jessolmstead/greycard-lama"));

        let note = SKY.license_note();
        assert!(note.contains("tue-mps/eomt"));
        assert!(note.contains("Copyright (c) 2025 Mobile Perception Systems Lab at TU/e"));
        assert!(note.contains("The above copyright notice and this permission notice"));
        assert!(note.contains("eomt_export.py"));

        // The SAM License wants a copy of itself with every copy, not
        // a link to it.
        let note = SAM3.license_note();
        assert!(note.contains("SAM License"));
        assert!(note.contains("Copyright (c) Meta Platforms, Inc. and affiliates."));
        assert!(note.contains("you shall provide a copy of this Agreement"));
        assert!(note.contains("Modifications and Amendments"));
        assert!(note.contains("sixteen best queries"));
        for f in SAM3.files {
            assert!(note.contains(f.sha256));
        }
    }

    /// The CPU-only runtimes are Microsoft's own releases of the
    /// version greycard's library is made from, each keeping the
    /// library under the name the decision looks for and the license
    /// that goes with it.
    #[test]
    fn the_runtimes_are_microsofts_and_keep_the_library_and_its_license() {
        let release = "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/";
        let mut ids = std::collections::HashSet::new();
        for r in RUNTIMES {
            assert!(ids.insert(r.id), "duplicate id {}", r.id);
            assert!(model(r.id).is_none(), "{} is also a model's id", r.id);
            assert_eq!(runtime(r.id), Some(r));
            assert!(r.archive.url.starts_with(release), "{}", r.archive.url);
            assert!(r.archive.url.ends_with(r.archive.name));
            assert!(r.archive.bytes > 0);
            assert!(r.name.contains("1.28.0") && r.id.contains("1.28.0"));
            assert_eq!(r.license.name, "MIT");
            assert!(r.license.url.contains("microsoft/onnxruntime"));
            assert!(r.notice.contains("Copyright (c) Microsoft Corporation"));
            assert!(
                r.notice
                    .contains("The above copyright notice and this permission notice")
            );
            match r.packing {
                Packing::Tgz => assert!(r.archive.name.ends_with(".tgz")),
                Packing::Zip => assert!(r.archive.name.ends_with(".zip")),
            }
            let top = r.archive.name.rsplit_once('.').unwrap().0;
            let mut names = std::collections::HashSet::new();
            for m in r.members() {
                assert!(names.insert(m.name), "{} kept twice", m.name);
                assert!(m.path.starts_with(&format!("{top}/")), "{}", m.path);
                assert!(!m.name.contains('/') && !m.name.contains('\\'));
                assert!(m.bytes > 0);
                assert_eq!(m.sha256.len(), 64);
                assert!(
                    m.sha256
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                );
            }
            assert!(r.kept.iter().any(|m| m.name == "LICENSE"));
            assert!(r.bytes() > r.library.bytes);
            let note = r.license_note();
            assert!(note.contains(r.archive.sha256) && note.contains(r.library.sha256));
            assert!(note.contains("Copyright (c) Microsoft Corporation"));
            assert!(note.contains("does not redistribute"));
        }
        assert_eq!(ONNXRUNTIME_CPU_LINUX_X64.library.name, "libonnxruntime.so");
        assert_eq!(ONNXRUNTIME_CPU_WINDOWS_X64.library.name, "onnxruntime.dll");
        assert_eq!(
            ONNXRUNTIME_CPU_WINDOWS_ARM64.library.name,
            "onnxruntime.dll"
        );
        // Each says what it is for in the note beside it, and the two
        // reasons differ: a processor short of AVX2, or a platform
        // greycard ships nothing for.
        assert!(
            ONNXRUNTIME_CPU_LINUX_X64
                .license_note()
                .contains("to run the models on a processor greycard's own ONNX Runtime cannot run on (one without AVX2).")
        );
        assert!(
            ONNXRUNTIME_CPU_WINDOWS_ARM64
                .license_note()
                .contains("to run the models on Windows on ARM, which greycard ships no ONNX Runtime of its own for.")
        );
        assert!(!ONNXRUNTIME_CPU_WINDOWS_ARM64.purpose.contains("AVX2"));
        // Kept under the name greycard's own library has, on the
        // platform it is offered on; none offered on a Mac.
        if let Some(r) = cpu_runtime() {
            assert_eq!(r.library.name, crate::runtime::LIBRARY);
        }
        assert_eq!(
            cpu_runtime().is_some(),
            cfg!(any(
                all(any(target_os = "linux", windows), target_arch = "x86_64"),
                all(windows, target_arch = "aarch64")
            ))
        );
    }
}
