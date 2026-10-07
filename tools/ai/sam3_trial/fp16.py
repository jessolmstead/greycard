"""Convert an fp32 ONNX graph to fp16 weights and arithmetic, inputs
and outputs left as they were.

    python fp16.py IN.onnx OUT.onnx [OP_TYPE | node:NAME ...]

The op types and nodes named stay in fp32 (onnxconverter-common's
block lists), for a mixed-precision file once the comparison says
which ops drift. The converter records some Casts' outputs as fp16
while leaving their target type at fp32, and ONNX Runtime refuses the
file; so afterwards each Cast's target is set to the type the graph
records for its output.
"""

import sys

import onnx
from onnxconverter_common import float16

src, dst, *keep = sys.argv[1:]
ops = [k for k in keep if not k.startswith("node:")]
nodes = [k[5:] for k in keep if k.startswith("node:")]
m = onnx.load(src)
m16 = float16.convert_float_to_float16(m, keep_io_types=True, op_block_list=ops or None, node_block_list=nodes or None)
recorded = {v.name: v.type.tensor_type.elem_type
            for v in list(m16.graph.value_info) + list(m16.graph.output)}
for n in m16.graph.node:
    if n.op_type != "Cast" or n.output[0] not in recorded:
        continue
    for a in n.attribute:
        if a.name == "to":
            a.i = recorded[n.output[0]]
onnx.save(m16, dst)
