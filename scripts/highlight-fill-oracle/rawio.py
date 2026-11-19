# SPDX-License-Identifier: GPL-3.0-or-later
"""Raw arrays for the Rust side: one little-endian file per array, C order,
and `manifest.txt` with a line per array, `<name> <dtype> <dim> ...`
(dtype f32, i32, or u8; a scalar is stored with shape 1), in the order
written."""
import os

import numpy as np

DTYPES = {'f32': '<f4', 'i32': '<i4', 'u8': 'u1'}


def write_dir(out, arrays):
    os.makedirs(out, exist_ok=True)
    lines = []
    for name, a in arrays.items():
        a = np.ascontiguousarray(a)
        kind = {np.dtype('float32'): 'f32', np.dtype('int32'): 'i32', np.dtype('uint8'): 'u8'}[a.dtype]
        a.astype(DTYPES[kind]).tofile(os.path.join(out, f'{name}.bin'))
        lines.append(' '.join([name, kind] + [str(d) for d in a.shape]))
    with open(os.path.join(out, 'manifest.txt'), 'w') as fh:
        fh.write('\n'.join(lines) + '\n')


def read_dir(path):
    out = {}
    for line in open(os.path.join(path, 'manifest.txt')):
        name, kind, *dims = line.split()
        a = np.fromfile(os.path.join(path, f'{name}.bin'), DTYPES[kind])
        out[name] = a.reshape([int(d) for d in dims])
    return out
