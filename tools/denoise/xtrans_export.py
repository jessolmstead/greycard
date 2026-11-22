"""Export an X-Trans checkpoint to ONNX, and check it against PyTorch.

    .venv/bin/python xtrans_export.py runs/xtrans/fast/last.pt xtrans-fast.onnx --version xtrans-fast-1
    .venv/bin/python xtrans_export.py --fixture replicate-xtrans.onnx

The contract the Rust side relies on (greycard-ai's ``denoise``): input
``mosaic`` [1, 1, h, w] float32, the mosaic at full resolution, each
sample stabilized by its own color's model, sample (0, 0) at the
canonical phase (``xtrans.XTRANS``); ``h`` and ``w`` multiples of 48.
Output ``rgb`` [1, 3, h, w] float32, stabilized RGB. The network packs
the plane itself. The metadata names the contract and the version.
"""

import argparse
import hashlib

import numpy as np
import onnx
import onnxruntime as ort
import torch
import torch.nn as nn
import torch.nn.functional as F

import xtrans


class XReplicate(nn.Module):
    """A fixed network whose answer is known, for the runtime's tests:
    each sample's R, G and B are the first sample of that color, in
    reading order, in the canonical 3x3 block it sits in (every 3x3
    block of X-Trans holds all three). Wrong at any other phase."""

    def __init__(self):
        super().__init__()
        shift = torch.zeros(25, 1, 5, 5)
        for k in range(25):
            shift[k, 0, k // 5, k % 5] = 1
        select = torch.zeros(3 * 25, 6, 6)
        for y in range(6):
            for x in range(6):
                by, bx = 3 * (y // 3), 3 * (x // 3)
                block = [(by + i, bx + j) for i in range(3) for j in range(3)]
                for c in range(3):
                    sy, sx = next(p for p in block if xtrans.XTRANS[p[0]][p[1]] == c)
                    select[c * 25 + (sy - y + 2) * 5 + (sx - x + 2), y, x] = 1
        self.register_buffer("shift", shift)
        self.register_buffer("select", select)

    def forward(self, x):
        shifted = F.conv2d(x, self.shift, padding=2)
        m = xtrans.periodic(x, self.select)
        b, _, h, w = x.shape
        return (m.reshape(b, 3, 25, h, w) * shifted[:, None]).sum(2)


def export(model, path, version):
    model.eval()
    x = torch.zeros(1, 1, 96, 144)
    torch.onnx.export(
        model,
        (x,),
        path,
        input_names=["mosaic"],
        output_names=["rgb"],
        dynamic_axes={"mosaic": {2: "h", 3: "w"}, "rgb": {2: "h", 3: "w"}},
        opset_version=17,
        dynamo=False,
    )
    m = onnx.load(path)
    for key, value in (("greycard.model", "denoise-xtrans"), ("greycard.version", version)):
        entry = m.metadata_props.add()
        entry.key, entry.value = key, value
    onnx.save(m, path)

    session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    for h, w in ((96, 144), (240, 192), (432, 528)):
        x = torch.randn(1, 1, h, w) + 4
        with torch.no_grad():
            want = model(x).numpy()
        got = session.run(["rgb"], {"mosaic": x.numpy()})[0]
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
    p.add_argument("--fixture", action="store_true", help="write the fixed XReplicate net instead")
    p.add_argument("--version", default="0")
    args = p.parse_args()
    if args.fixture:
        export(XReplicate(), args.out, "fixture-xtrans")
        return
    ck = torch.load(args.checkpoint, map_location="cpu")
    model = xtrans.XNet(ck["packing"], tuple(ck["widths"]), ck["post"])
    model.load_state_dict(ck["ema"])
    export(model, args.out, args.version)


if __name__ == "__main__":
    main()
