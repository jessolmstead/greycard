"""Export EoMT-S (COCO panoptic, 640) to ONNX for greycard's Sky mask.

The weights are `tue-mps/coco_panoptic_eomt_small_640_2x` (EoMT by
Kerssies et al., MIT; its DINOv2 ViT-S backbone by Meta, Apache-2.0),
published on Hugging Face. This script traces the transformers port
once, at the fixed input the editor feeds it, and writes one ONNX file
with the weights inside:

    input   pixel_values           1 x 3 x 640 x 640, float32: the
            picture resized to 640 on its long side (bilinear, with
            antialiasing), padded with black on the bottom or right to
            the square, then ImageNet-normalized ((v / 255 - mean) / std)
    output  class_queries_logits   1 x 200 x 134: each query's class
            logits, the 133 COCO panoptic categories in the order of the
            model's `id2label` (sky-other-merged is 119) and "no object"
            last
    output  masks_queries_logits   1 x 200 x 160 x 160: each query's
            mask logits over the padded square, a quarter of its side

Nothing in the graph is changed after the export; the file is the
published weights in another container. greycard's Sky shape reads it
through ONNX Runtime (crates/greycard-ai/src/sky.rs), on WebGPU where
the card takes it and on the CPU otherwise.

Usage, in a venv with the versions in requirements.txt:

    python eomt_export.py OUT.onnx

With those versions the output is byte for byte the same on every
run, so its sha256 goes in greycard's model registry (`SKY`). The
revision is pinned below so a later push to the model repository does
not change the file.

Part of greycard, GPL-3.0-or-later.
"""

import argparse
import hashlib

import torch
from transformers import EomtForUniversalSegmentation

REPO = "tue-mps/coco_panoptic_eomt_small_640_2x"
REVISION = "10f5326"
SIZE = 640


class Wrapped(torch.nn.Module):
    """The two outputs the editor reads, under names of their own."""

    def __init__(self, model):
        super().__init__()
        # The attribute's name is in every node and weight name, so in the
        # file's bytes: keep it.
        self.m = model

    def forward(self, pixel_values):
        out = self.m(pixel_values=pixel_values)
        return out.class_queries_logits, out.masks_queries_logits


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("out", help="the ONNX file to write")
    args = parser.parse_args()

    model = EomtForUniversalSegmentation.from_pretrained(REPO, revision=REVISION).eval()
    # The trace depends on the input's shape, not its values.
    x = torch.zeros(1, 3, SIZE, SIZE)
    torch.onnx.export(
        Wrapped(model).eval(),
        (x,),
        args.out,
        input_names=["pixel_values"],
        output_names=["class_queries_logits", "masks_queries_logits"],
        opset_version=17,
        dynamo=False,
    )
    with open(args.out, "rb") as f:
        data = f.read()
    print(f"{args.out}: {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main()
