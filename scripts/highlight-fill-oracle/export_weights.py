# SPDX-License-Identifier: GPL-3.0-or-later
"""Write a checkpoint's weights as the single blob greycard builds in
(greycard-ai's `highlight` module), and optionally the encoder and MLP's
outputs on one fixture scene, for the test that the built-in weights run
as the oracle does.

Usage:
    export_weights.py CHECKPOINT.pt OUT.bin [SCENE_DIR CHECK_DIR]

CHECKPOINT.pt is a trial checkpoint (a dict whose 'model' is the state
dict) or a bare state dict, loaded on the CPU. Only the 16 tensors of
`oracle.RegionNet6` are written, checked by a strict load, so nothing
else in the checkpoint (its arguments, paths, or optimizer state)
reaches the blob.

The blob (greycard-core's `FillWeights::to_bytes`), all little-endian:
- b'GCHLFILL', the format version (u32, 1), the tensor count (u32);
- per tensor, in the state dict's order: the name's length (u16) and its
  UTF-8 bytes, the number of dimensions (u8), each dimension (u32), then
  the values (f32, C order).

With SCENE_DIR (a fixture scene that stores `thumb`, `grid`, and
`feats`, such as lamp_f3) the model's encoder map over the thumbnail,
the map sampled at each row's grid point, and the MLP's outputs (before
the outside row is zeroed) go to CHECK_DIR as `cmap`, `ctx`, and
`outputs` (rawio's format)."""
import os
import struct
import sys

import numpy as np
import torch
import torch.nn.functional as F

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import oracle  # noqa: E402
import rawio  # noqa: E402

MAGIC = b'GCHLFILL'
VERSION = 1


def blob(state_dict):
    out = [MAGIC, struct.pack('<II', VERSION, len(state_dict))]
    for name, t in state_dict.items():
        a = np.ascontiguousarray(t.detach().cpu().numpy().astype('<f4'))
        nb = name.encode('utf-8')
        out.append(struct.pack('<H', len(nb)) + nb + struct.pack('<B', a.ndim))
        out.append(struct.pack(f'<{a.ndim}I', *a.shape))
        out.append(a.tobytes())
    return b''.join(out)


def model_of(path):
    ck = torch.load(path, map_location='cpu', weights_only=False)
    sd = ck['model'] if isinstance(ck, dict) and 'model' in ck else ck
    # A strict load: exactly RegionNet6's tensors, nothing else kept.
    return oracle.load({k: v.float() for k, v in sd.items()})


def check(model, scene, out):
    a = rawio.read_dir(scene)
    with torch.no_grad():
        thumb = torch.from_numpy(a['thumb']).permute(2, 0, 1)[None]
        cmap = model.ctx(thumb)[0]
        grid = torch.from_numpy(a['grid']).view(1, 1, -1, 2)
        ctx = F.grid_sample(cmap, grid, mode='bilinear', padding_mode='border', align_corners=False)[0, :, 0].T
        r = model.outputs(torch.from_numpy(a['feats']), ctx)
    rawio.write_dir(out, {'cmap': cmap[0].numpy(), 'ctx': ctx.numpy(), 'outputs': r.numpy()})


def main(args):
    torch.set_num_threads(4)
    model = model_of(args[0])
    with open(args[1], 'wb') as fh:
        fh.write(blob(model.state_dict()))
    if len(args) > 2:
        check(model, args[2], args[3])


if __name__ == '__main__':
    main(sys.argv[1:])
