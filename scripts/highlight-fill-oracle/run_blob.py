# SPDX-License-Identifier: GPL-3.0-or-later
"""Run the oracle with the weights of a built-in blob (greycard-ai's
`src/highlight/*.bin`, `export_weights.py`'s format) on fixture scenes,
for the test that the built-in weights fill as the oracle does.

Usage:
    run_blob.py BLOB.bin OUT_DIR SCENE_DIR...

Each scene (one that stores its x and codes) is filled whole frame at
its own look scale f; OUT_DIR/<scene>/ gets `out_clipped` (N, 3), the
filled picture at the pixels with codes & 0x7F > 0 in row-major order,
and the half-size fields `gain_h` (Hh, Wh) and `dab_h` (2, Hh, Wh)
(rawio's format)."""
import os
import struct
import sys
from collections import OrderedDict

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import oracle  # noqa: E402
import rawio  # noqa: E402

MAGIC = b'GCHLFILL'
VERSION = 1


def parse(data):
    """The blob's tensors, in order (the inverse of export_weights.blob)."""
    assert data[:8] == MAGIC, 'not a highlight fill blob'
    version, count = struct.unpack_from('<II', data, 8)
    assert version == VERSION, version
    at = 16
    sd = OrderedDict()
    for _ in range(count):
        (n,) = struct.unpack_from('<H', data, at)
        at += 2
        name = data[at:at + n].decode('utf-8')
        at += n
        ndim = data[at]
        at += 1
        shape = struct.unpack_from(f'<{ndim}I', data, at)
        at += 4 * ndim
        size = int(np.prod(shape))
        sd[name] = torch.from_numpy(np.frombuffer(data, '<f4', size, at).astype(np.float32).reshape(shape))
        at += 4 * size
    assert at == len(data), 'bytes after the last tensor'
    return sd


def main(args):
    torch.set_num_threads(4)
    with open(args[0], 'rb') as fh:
        model = oracle.load(parse(fh.read()))
    for scene in args[2:]:
        a = rawio.read_dir(scene)
        rec = {}
        with torch.no_grad():
            pic = oracle.fill(model, a['x'], a['codes'], [float(c) for c in a['clips']], f=float(a['f'][0]), rec=rec)
        sel = (a['codes'] & 0x7F) > 0
        out = os.path.join(args[1], os.path.basename(os.path.normpath(scene)))
        rawio.write_dir(out, {'out_clipped': pic[sel].astype(np.float32),
                              'gain_h': rec['gain_h'].numpy().astype(np.float32),
                              'dab_h': rec['dab_h'].numpy().astype(np.float32)})
        print(out, int(sel.sum()), 'clipped pixels', flush=True)


if __name__ == '__main__':
    main(sys.argv[1:])
