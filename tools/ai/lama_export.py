"""Export big-lama to ONNX with a dynamic height and width for
greycard's eraser.

The weights are LaMa's `big-lama` checkpoint (Suvorov et al.,
"Resolution-robust Large Mask Inpainting with Fourier Convolutions",
Samsung Research, Apache-2.0), from https://github.com/advimman/lama,
whose README links the zip. This script builds the generator from
the code in a checkout of that repository, loads the checkpoint's
generator weights, and writes one fp32 ONNX file, opset 21, with the
weights inside:

    input   image    1 x 3 x H x W, float32, RGB in 0 to 1
    input   mask     1 x 1 x H x W, float32, 1 inside the hole
    output  output   1 x 3 x H x W, float32 in 0 to 1: the model's
                     answer inside the hole and the image outside

H and W are any multiples of 8 from 32. Around the generator is LaMa's
own forward pass: the picture blanked under the mask, the two
concatenated, and the answer composited over the picture.

The Fourier units' torch.fft.rfftn and irfftn are written as the same
transforms by real matrix products (`fourier_forward`, a rewrite of
`FourierUnit.forward` in LaMa's saicinpainting/training/modules/ffc.py,
Apache-2.0, Copyright 2021 Samsung Research), the matrices made from
the tensor's size inside the graph, so every op runs on ONNX Runtime's
WebGPU provider and the shape stays free. Exported as is
(--native-fft), the ONNX DFT nodes fail in ONNX Runtime on a shape
mismatch. Opset 21 is the highest that runs whole on ONNX Runtime
1.28's WebGPU provider.

Usage, in a venv with the versions in requirements.txt, with the
repository unpacked and big-lama.zip unzipped beside this script:

    python lama_export.py --lama lama --ckpt big-lama/models/best.ckpt \
        --out big-lama-dynamic-fp32.onnx

The output's bytes differ from run to run (the exporter's node names),
its answers do not; greycard's model registry (`FILL`) holds the hash
of the file published at https://huggingface.co/jessolmstead/greycard-lama.

Part of greycard, GPL-3.0-or-later.
"""

import argparse
import math
import pickle
import sys
import types

import torch
import torch.nn as nn


def stub_lightning():
    # saicinpainting.utils imports seed_everything, for training only.
    pl = types.ModuleType("pytorch_lightning")
    pl.seed_everything = lambda *a, **k: None
    sys.modules.setdefault("pytorch_lightning", pl)


class _Stub:
    def __init__(self, *a, **k):
        pass

    def __setstate__(self, state):
        pass


class _Unpickler(pickle.Unpickler):
    # The checkpoint also pickles Lightning's and OmegaConf's training
    # state; only the tensors are wanted, so anything else is a stub.
    def find_class(self, module, name):
        if module.split(".")[0] in ("torch", "collections", "numpy", "_codecs", "builtins"):
            return super().find_class(module, name)
        return _Stub


_Weights = types.SimpleNamespace(Unpickler=_Unpickler, load=pickle.load, __name__="pickle")


def _scale(h, w, like):
    return 1.0 / torch.sqrt(torch.ones((), dtype=like.dtype) * h * w)


def rfft2_ortho(x):
    """torch.fft.rfftn(x, dim=(-2, -1), norm='ortho'), as (re, im)."""
    h, w = x.shape[-2], x.shape[-1]
    k = w // 2 + 1
    dt = x.dtype
    tw = torch.arange(w, dtype=dt)
    tk = torch.arange(k, dtype=dt)
    ang = tw[:, None] * tk[None, :] * (2 * math.pi) / w  # (w, k)
    re1 = x @ torch.cos(ang)
    im1 = -(x @ torch.sin(ang))
    th = torch.arange(h, dtype=dt)
    angh = th[:, None] * th[None, :] * (2 * math.pi) / h  # (h, h)
    ch, sh = torch.cos(angh), torch.sin(angh)
    s = _scale(h, w, x)
    re = (ch @ re1 + sh @ im1) * s
    im = (ch @ im1 - sh @ re1) * s
    return re, im


def irfft2_ortho(re, im, h, w):
    """torch.fft.irfftn(complex(re, im), s=(h, w), dim=(-2, -1), norm='ortho')."""
    dt = re.dtype
    th = torch.arange(h, dtype=dt)
    angh = th[:, None] * th[None, :] * (2 * math.pi) / h
    ch, sh = torch.cos(angh), torch.sin(angh)
    zre = ch @ re - sh @ im
    zim = ch @ im + sh @ re
    k = re.shape[-1]
    tk = torch.arange(k, dtype=dt)
    tw = torch.arange(w, dtype=dt)
    # Every bin but DC and, for an even width, Nyquist stands for two.
    a = 2.0 - (tk == 0).to(dt) - (tk * 2 == w).to(dt)
    ang = tk[:, None] * tw[None, :] * (2 * math.pi) / w  # (k, w)
    cw = torch.cos(ang) * a[:, None]
    sw = torch.sin(ang) * a[:, None]
    return (zre @ cw - zim @ sw) * _scale(h, w, re)


def fourier_forward(self, x):
    # FourierUnit.forward for big-lama's settings: no spatial scale, no
    # spectral position encoding, no SE, 2-D, ortho.
    batch = x.shape[0]
    h, w = x.shape[-2], x.shape[-1]
    re, im = rfft2_ortho(x)
    k = re.shape[-1]
    ffted = torch.stack((re, im), dim=2)  # (b, c, 2, h, k)
    ffted = ffted.reshape(batch, -1, h, k)
    ffted = self.relu(self.bn(self.conv_layer(ffted)))
    ffted = ffted.reshape(batch, -1, 2, h, k)
    return irfft2_ortho(ffted[:, :, 0], ffted[:, :, 1], h, w)


class Inpaint(nn.Module):
    """LaMa's forward pass: blank the hole, ask, keep the outside."""

    def __init__(self, generator):
        super().__init__()
        self.generator = generator

    def forward(self, image, mask):
        masked = image * (1 - mask)
        pred = self.generator(torch.cat([masked, mask], dim=1))
        return mask * pred + (1 - mask) * image


def build(lama, ckpt, native_fft=False):
    sys.path.insert(0, lama)
    stub_lightning()
    from saicinpainting.training.modules import ffc

    if not native_fft:
        ffc.FourierUnit.forward = fourier_forward
    # big-lama/config.yaml's generator.
    gen = ffc.FFCResNetGenerator(
        input_nc=4,
        output_nc=3,
        ngf=64,
        n_downsampling=3,
        n_blocks=18,
        add_out_act="sigmoid",
        init_conv_kwargs=dict(ratio_gin=0, ratio_gout=0, enable_lfu=False),
        downsample_conv_kwargs=dict(ratio_gin=0, ratio_gout=0, enable_lfu=False),
        resnet_conv_kwargs=dict(ratio_gin=0.75, ratio_gout=0.75, enable_lfu=False),
    )
    state = torch.load(ckpt, map_location="cpu", pickle_module=_Weights)["state_dict"]
    pre = "generator."
    gen.load_state_dict({k[len(pre):]: v for k, v in state.items() if k.startswith(pre)})
    for m in gen.modules():
        if isinstance(m, ffc.FourierUnit):
            assert m.spatial_scale_factor is None and not m.spectral_pos_encoding
            assert not m.use_se and not m.ffc3d and m.fft_norm == "ortho"
    return Inpaint(gen).eval()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--lama", required=True)
    p.add_argument("--ckpt", required=True)
    p.add_argument("--out", required=True)
    # 21 is the highest that runs whole on ONNX Runtime 1.28's WebGPU
    # provider: at 22 and 23 some ops fall back to the CPU, 2.3 times
    # slower at 2048 and eight times the memory.
    p.add_argument("--opset", type=int, default=21)
    p.add_argument("--native-fft", action="store_true")
    a = p.parse_args()
    torch.manual_seed(0)
    model = build(a.lama, a.ckpt, a.native_fft)
    image = torch.rand(1, 3, 512, 384)
    mask = torch.zeros(1, 1, 512, 384)
    mask[:, :, 160:320, 96:256] = 1
    # Height and width are any multiple of 8 (three halvings) from 32.
    hk = torch.export.Dim("hk", min=4, max=2048)
    wk = torch.export.Dim("wk", min=4, max=2048)
    shape = {2: 8 * hk, 3: 8 * wk}
    with torch.no_grad():
        prog = torch.onnx.export(
            model,
            (image, mask),
            input_names=["image", "mask"],
            output_names=["output"],
            dynamic_shapes={"image": shape, "mask": shape},
            opset_version=a.opset,
            dynamo=True,
        )
    prog.optimize()
    prog.save(a.out, external_data=False)


if __name__ == "__main__":
    main()
