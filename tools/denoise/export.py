"""Export a checkpoint to ONNX, and check it against PyTorch.

    .venv/bin/python export.py runs/first/last.pt denoise.onnx
    .venv/bin/python export.py --fixture replicate.onnx

The contract the Rust side relies on: input ``packed`` [1, 4, h, w]
float32, the stabilized mosaic packed R, G1, G2, B at half resolution;
output ``rgb`` [1, 3, 2h, 2w] float32, stabilized RGB at the mosaic's
resolution. ``h`` and ``w`` are free. The metadata carries the name and
version greycard shows.
"""

import argparse
import hashlib

import numpy as np
import onnx
import onnxruntime as ort
import torch

from model import Replicate, UNet


def export(model, path, version):
    model.eval()
    x = torch.zeros(1, 4, 64, 96)
    torch.onnx.export(
        model,
        (x,),
        path,
        input_names=["packed"],
        output_names=["rgb"],
        dynamic_axes={"packed": {2: "h", 3: "w"}, "rgb": {2: "h2", 3: "w2"}},
        opset_version=17,
        dynamo=False,
    )
    m = onnx.load(path)
    for key, value in (("greycard.model", "denoise"), ("greycard.version", version)):
        entry = m.metadata_props.add()
        entry.key, entry.value = key, value
    onnx.save(m, path)

    session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    x = torch.randn(1, 4, 40, 72)
    with torch.no_grad():
        want = model(x).numpy()
    got = session.run(["rgb"], {"packed": x.numpy()})[0]
    assert got.shape == want.shape, (got.shape, want.shape)
    err = float(np.abs(got - want).max())
    assert err < 1e-3, err
    with open(path, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    print(f"{path}: max abs error vs torch {err:.2e}, sha256 {digest}")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("checkpoint", nargs="?")
    p.add_argument("out")
    p.add_argument("--fixture", action="store_true", help="write the fixed Replicate net instead")
    p.add_argument("--version", default="0")
    args = p.parse_args()
    if args.fixture:
        export(Replicate(), args.out, "fixture")
        return
    ck = torch.load(args.checkpoint, map_location="cpu")
    model = UNet(tuple(ck["widths"]), post=ck.get("post", 8), head=ck.get("head", "shuffle"))
    model.load_state_dict(ck["ema"])
    export(model, args.out, args.version)


if __name__ == "__main__":
    main()
