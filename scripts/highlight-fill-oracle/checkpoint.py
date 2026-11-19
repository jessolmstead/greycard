# SPDX-License-Identifier: GPL-3.0-or-later
"""The fixtures' random-seeded checkpoint of the shipped architecture (no
trained weights), and the raw weights format the Rust loader reads.

Weights format: a directory with `manifest.txt` and one file per tensor,
`<name>.bin`, raw little-endian float32 in C (row-major) order. Each
manifest line is `<name> f32 <dim> <dim> ...`; names are the PyTorch
state-dict keys (`ctx.net.0.weight` is the first convolution, (out, in,
3, 3); `mlp.0.weight` the first linear, (out, in), applied as x W^T + b)."""
import numpy as np
import torch

import oracle
import rawio

SEED = 20261119


def random_model():
    """Default PyTorch initialization under a fixed seed, then the last
    layer drawn too (the trial's init zeroes it, which would make every
    region's outputs equal) and its biases set near the trial's init, with
    a lift on H and H2 so the fixtures show a gain."""
    torch.manual_seed(SEED)
    m = oracle.RegionNet6().eval()
    with torch.no_grad():
        # Default init shrinks activations layer on layer; scaled up so the
        # encoder's map and the hidden layers carry values of order one.
        for i in (0, 2, 4, 6, 8):
            m.ctx.net[i].weight.mul_(2.5)
        for i in (0, 2):
            m.mlp[i].weight.mul_(2.0)
        last = m.mlp[-1]
        last.weight.normal_(0.0, 0.15)
        bias = torch.zeros(oracle.NOUT)
        bias[0], bias[1], bias[2], bias[3], bias[8] = 0.8, float(np.log(0.3)), -1.0, -1.0, -1.0
        bias[10], bias[11] = 0.6, float(np.log(0.3))
        last.bias.copy_(bias + 0.2 * torch.randn(oracle.NOUT))
    return m


def save(model, out):
    rawio.write_dir(out, {k: v.detach().cpu().numpy().astype(np.float32) for k, v in model.state_dict().items()})


def load(path):
    return oracle.load({k: torch.from_numpy(v) for k, v in rawio.read_dir(path).items()})
