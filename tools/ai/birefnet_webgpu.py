"""Rewrite BiRefNet lite's ONNX export so ONNX Runtime's WebGPU provider
runs all of it.

The onnx-community export of BiRefNet lite (model_fp16.onnx; BiRefNet
by Zheng Peng et al., MIT; the export by onnx-community) has three
things the WebGPU provider will not run on the card:

1. Its decoder cuts its input into a 32 x 32 grid of patches with
   Splits of 16 and 32 outputs. A shader binds at most 16 storage
   buffers per stage under Dawn, and ONNX Runtime's WebGPU Split binds
   its input and every output in one shader, so such a Split fails at
   run time ("Too many storage buffers in shader. Current: 17, Max is
   16"). Each becomes one Slice per output: same axis, same offsets,
   same output names.
2. Its twenty deformable convolutions sum four fp16 terms with a Sum,
   which the provider has no kernel for. Each becomes a chain of Adds.
3. The same convolutions floor their sampling coordinates, cast them
   to int64, and Slice, Add, Clip, Reshape and Concat them as int64,
   none of which the provider takes. The chain stays in fp16 and the
   cast moves to just before the GatherND that uses it (the reasoning
   for why that is exact is at `float_indices`).

On the CPU the rewritten graph answers exactly as the original does.
The notes, section "BiRefNet on WebGPU", have the measurements.

Usage (a venv with `onnx`, `onnxruntime`, `numpy` and `pillow`):

    python birefnet_webgpu.py rewrite model_fp16.onnx model_fp16_webgpu.onnx

The output is byte for byte the same on every run with the versions in
requirements.txt, so its sha256 can go in greycard's model registry.
Other commands:

    python birefnet_webgpu.py check model_fp16_webgpu.onnx
    python birefnet_webgpu.py compare model_fp16.onnx model_fp16_webgpu.onnx picture.png
    python birefnet_webgpu.py placements verbose.log
    python birefnet_webgpu.py mattes a.f32 b.f32

`rewrite --splits-only` does step 1 alone. `check` exits non-zero when
a Split still binds more storage buffers than the bound allows, or a
Sum or an int64 coordinate chain is left. `compare` runs both graphs on the CPU provider of the
`onnxruntime` package on one picture and prints how far apart the
mattes are. `placements` reads the "Node placements" block of a session
run with verbose logging (`subject_bench` with `BENCH_VERBOSE=1`) and
counts the nodes each provider took, by op type. `mattes` compares two
raw f32 mattes written by `subject_bench`.

Part of greycard, GPL-3.0-or-later.
"""

import argparse
import collections
import hashlib
import os
import re
import sys

import numpy as np
import onnx
from onnx import helper, numpy_helper

# Dawn's maxStorageBuffersPerShaderStage on every adapter seen so far;
# WebGPU's own default limit is 8, but ONNX Runtime asks for the
# adapter's.
STORAGE_BUFFERS = 16


def constant_values(graph):
    """The value of every Constant node's output and every initializer."""
    values = {i.name: numpy_helper.to_array(i) for i in graph.initializer}
    for n in graph.node:
        if n.op_type == "Constant" and n.attribute and n.attribute[0].name == "value":
            values[n.output[0]] = numpy_helper.to_array(n.attribute[0].t)
    return values


def split_sizes(node, values, shapes):
    """The sizes a Split cuts its input into along its axis."""
    attrs = {a.name: helper.get_attribute_value(a) for a in node.attribute}
    if len(node.input) > 1 and node.input[1]:
        if node.input[1] not in values:
            raise SystemExit(f"{node.name}: split sizes are not a constant")
        return [int(s) for s in values[node.input[1]]]
    if "split" in attrs:
        return [int(s) for s in attrs["split"]]
    # Equal parts: the axis's length is needed.
    shape = shapes.get(node.input[0])
    axis = attrs.get("axis", 0)
    if shape is None or not isinstance(shape[axis], int) or shape[axis] <= 0:
        raise SystemExit(f"{node.name}: equal split of an axis of unknown length")
    n = len(node.output)
    if shape[axis] % n:
        raise SystemExit(f"{node.name}: {shape[axis]} does not split evenly {n} ways")
    return [shape[axis] // n] * n


def inferred_shapes(model):
    inferred = onnx.shape_inference.infer_shapes(model)
    shapes = {}
    for vi in list(inferred.graph.value_info) + list(inferred.graph.input):
        dims = vi.type.tensor_type.shape.dim
        shapes[vi.name] = [d.dim_value if d.HasField("dim_value") else None for d in dims]
    return shapes


def too_wide(node, bound):
    """Whether a Split binds more storage buffers than `bound`: its
    input and each output are one buffer each."""
    return node.op_type == "Split" and 1 + len(node.output) > bound


def split_to_slices(model, bound):
    """Every Split with more outputs than `bound` allows becomes one
    Slice per output. Returns how many Splits were rewritten."""
    graph = model.graph
    values = constant_values(graph)
    shapes = None
    nodes = []
    new_inits = []
    rewritten = 0
    for node in graph.node:
        if not too_wide(node, bound):
            nodes.append(node)
            continue
        if shapes is None and not (len(node.input) > 1 and node.input[1]):
            shapes = inferred_shapes(model)
        sizes = split_sizes(node, values, shapes or {})
        attrs = {a.name: helper.get_attribute_value(a) for a in node.attribute}
        axis = attrs.get("axis", 0)
        base = node.name or node.output[0]
        axes = f"{base}/slice_axes"
        new_inits.append(numpy_helper.from_array(np.array([axis], np.int64), axes))
        start = 0
        for i, (out, size) in enumerate(zip(node.output, sizes)):
            starts, ends = f"{base}/slice_{i}_starts", f"{base}/slice_{i}_ends"
            new_inits.append(numpy_helper.from_array(np.array([start], np.int64), starts))
            new_inits.append(numpy_helper.from_array(np.array([start + size], np.int64), ends))
            start += size
            if out:
                nodes.append(
                    helper.make_node("Slice", [node.input[0], starts, ends, axes], [out], name=f"{base}/slice_{i}")
                )
        rewritten += 1
    del graph.node[:]
    graph.node.extend(nodes)
    graph.initializer.extend(new_inits)
    return rewritten


def wide_splits(model, bound):
    return [n for n in model.graph.node if too_wide(n, bound)]


def sum_to_add(model):
    """Every Sum of more than one input becomes a chain of Adds. The
    WebGPU provider has no Sum kernel, and the deformable convolutions'
    four-way fp16 Sums otherwise run on the CPU with a cast to fp32 and
    back around each. Returns how many Sums were rewritten."""
    nodes = []
    rewritten = 0
    for node in model.graph.node:
        if node.op_type != "Sum" or len(node.input) < 2:
            nodes.append(node)
            continue
        base = node.name or node.output[0]
        acc = node.input[0]
        for i, x in enumerate(node.input[1:], 1):
            out = node.output[0] if i == len(node.input) - 1 else f"{base}/add_{i}_output"
            nodes.append(helper.make_node("Add", [acc, x], [out], name=f"{base}/add_{i}"))
            acc = out
        rewritten += 1
    del model.graph.node[:]
    model.graph.node.extend(nodes)
    return rewritten


# What an index may pass through on its way from the float Cast to the
# GatherND that uses it, with the inputs that carry it; the other
# inputs stay as they are, except the Add, Sub and Clip constants, which
# are converted.
INDEX_PATH = {
    "Slice": {0},
    "Reshape": {0},
    "Unsqueeze": {0},
    "Squeeze": {0},
    "Concat": None,  # every input
    "Add": None,
    "Sub": None,
    "Clip": {0},
}
ARITHMETIC_CONSTANTS = {"Add": None, "Sub": None, "Clip": {1, 2}}
# Integers above this are not all exact in fp16.
FP16_EXACT = 2048


def float_indices(model):
    """The deformable convolutions compute their sampling coordinates
    as floats, Floor them, Cast to int64, then Slice, Add, Clip, Reshape
    and Concat them as int64 before a GatherND. The WebGPU provider
    has none of those kernels for int64, so the whole chain runs on the
    CPU with a copy out and back each. Here the chain stays in the float
    type and the Cast to int64 moves to just before each GatherND.

    The same function: every value on the path is an integer (a Floor,
    plus integer constants) and the Clip bounds it by at most
    `FP16_EXACT`, where fp16 still holds every integer; a value rounded
    on its way to the Clip was outside the bounds and is clipped the
    same either way. Refused (left alone) when a chain passes anything
    else, or its constants are not small integers. Returns how many
    chains were moved."""
    graph = model.graph
    inferred = onnx.shape_inference.infer_shapes(model)
    dtype = {vi.name: vi.type.tensor_type.elem_type for vi in list(inferred.graph.value_info) + list(graph.input)}
    values = constant_values(graph)
    consumers = collections.defaultdict(list)
    for n in graph.node:
        for i, x in enumerate(n.input):
            consumers[x].append((n, i))

    moved = 0
    new_nodes_before = collections.defaultdict(list)  # GatherND name -> Casts to insert before it
    new_inits = []
    replaced = {}  # id(node) -> replacement node
    for cast in graph.node:
        attrs = {a.name: helper.get_attribute_value(a) for a in cast.attribute}
        if cast.op_type != "Cast" or attrs.get("to") != onnx.TensorProto.INT64:
            continue
        ftype = dtype.get(cast.input[0])
        if ftype not in (onnx.TensorProto.FLOAT16, onnx.TensorProto.FLOAT):
            continue
        # Walk the int64 tensors forward to the GatherNDs.
        path, gathers, ok = [], [], True
        stack, seen = [cast.output[0]], set()
        while stack and ok:
            x = stack.pop()
            if x in seen:
                continue
            seen.add(x)
            for n, i in consumers[x]:
                if n.op_type == "GatherND" and i == 1:
                    gathers.append((n, x))
                elif n.op_type in INDEX_PATH and (INDEX_PATH[n.op_type] is None or i in INDEX_PATH[n.op_type]):
                    if n not in path:
                        path.append(n)
                    stack.extend(n.output)
                else:
                    ok = False
                    break
        if not ok or not gathers:
            continue
        # The path's integer constants, converted; each gets its own
        # copy, since a constant may be shared with a node off the path.
        np_ftype = np.float16 if ftype == onnx.TensorProto.FLOAT16 else np.float32
        rewrites = []
        for n in path:
            which = ARITHMETIC_CONSTANTS.get(n.op_type, set())
            for i, x in enumerate(n.input):
                if x in seen or (which is not None and i not in which) or not x:
                    continue
                if x not in values:
                    ok = False
                    break
                v = values[x]
                if v.dtype != np.int64 or np.abs(v).max(initial=0) > FP16_EXACT:
                    ok = False
                    break
                name = f"{n.name}/{i}_as_float"
                rewrites.append((n, i, numpy_helper.from_array(v.astype(np_ftype), name)))
            if n.op_type == "Clip" and ok:
                bounds = [values.get(x) for x in n.input[1:3] if x]
                if len(bounds) != 2 or any(b is None or np.abs(b).max() > FP16_EXACT for b in bounds):
                    ok = False
        if not ok:
            continue
        for n, i, init in rewrites:
            n.input[i] = init.name
            new_inits.append(init)
        replaced[id(cast)] = helper.make_node("Identity", [cast.input[0]], [cast.output[0]], name=cast.name)
        for g, x in gathers:
            as_int = f"{x}/as_int64"
            if as_int not in {c.output[0] for c in new_nodes_before[g.name]}:
                new_nodes_before[g.name].append(
                    helper.make_node("Cast", [x], [as_int], name=f"{x}/cast_int64", to=onnx.TensorProto.INT64)
                )
            g.input[1] = as_int
        moved += 1

    nodes = []
    for n in graph.node:
        nodes.extend(new_nodes_before.get(n.name, []))
        nodes.append(replaced.get(id(n), n))
    del graph.node[:]
    graph.node.extend(nodes)
    graph.initializer.extend(new_inits)
    # Shape inference recorded int64 for tensors that are now floats.
    del graph.value_info[:]
    return moved


def declined(model, bound):
    """What the WebGPU provider would fail on or leave to the CPU, of
    what this tool rewrites: Splits past the bound, Sums, and floored
    coordinates cast to int64 (the head of an int64 index chain)."""
    wide = wide_splits(model, bound)
    sums = [n for n in model.graph.node if n.op_type == "Sum"]
    producer = {x: n for n in model.graph.node for x in n.output}
    casts = [
        n
        for n in model.graph.node
        if n.op_type == "Cast"
        and any(a.name == "to" and a.i == onnx.TensorProto.INT64 for a in n.attribute)
        and getattr(producer.get(n.input[0]), "op_type", None) == "Floor"
    ]
    return wide, sums, casts


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def cmd_rewrite(args):
    model = onnx.load(args.input)
    before = collections.Counter(len(n.output) for n in model.graph.node if n.op_type == "Split")
    n = split_to_slices(model, args.bound)
    sums = 0 if args.splits_only else sum_to_add(model)
    chains = 0 if args.splits_only else float_indices(model)
    onnx.checker.check_model(model)
    # Deterministic bytes: the same input and script give the same hash.
    model.producer_name = "greycard tools/ai/birefnet_webgpu.py"
    model.doc_string = (
        "BiRefNet lite (Zheng Peng et al., MIT, https://github.com/ZhengPeng7/BiRefNet), "
        "ONNX export by onnx-community (https://huggingface.co/onnx-community/BiRefNet_lite-ONNX), "
        f"rewritten so ONNX Runtime's WebGPU provider runs all of it: Splits of more than "
        f"{args.bound - 1} outputs as Slices, Sums as Adds, the deformable convolutions' int64 "
        "coordinate math in fp16 (greycard, tools/ai/birefnet_webgpu.py)."
    )
    onnx.save(model, args.output)
    after = collections.Counter(len(n.output) for n in model.graph.node if n.op_type == "Split")
    print(f"Splits by outputs before: {dict(sorted(before.items()))}")
    print(f"Splits by outputs after:  {dict(sorted(after.items()))}")
    print(f"rewrote {n} Splits into Slices, {sums} Sums into Adds, {chains} int64 index chains into floats")
    print(f"{args.input}: sha256 {sha256(args.input)}")
    print(f"{args.output}: {os.path.getsize(args.output)} bytes, sha256 {sha256(args.output)}")


def cmd_check(args):
    model = onnx.load(args.model)
    wide, sums, casts = declined(model, args.bound)
    for n in wide:
        print(f"{n.name}: {len(n.output)} outputs, {1 + len(n.output)} storage buffers > {args.bound}")
    for n in sums:
        print(f"{n.name}: a Sum, which the WebGPU provider leaves to the CPU")
    for n in casts:
        print(f"{n.name}: a floored coordinate cast to int64, whose index math the WebGPU provider leaves to the CPU")
    if wide or sums or casts:
        print(f"{len(wide)} Splits bind more than {args.bound} storage buffers; {len(sums)} Sums; {len(casts)} int64 index chains")
        sys.exit(1)
    print(f"no Split binds more than {args.bound} storage buffers; no Sum; no int64 index chain")


def preprocess(picture, size=1024):
    from PIL import Image

    im = Image.open(picture).convert("RGB").resize((size, size), Image.BILINEAR)
    x = np.asarray(im, np.float32) / 255.0
    x = (x - np.array([0.485, 0.456, 0.406], np.float32)) / np.array([0.229, 0.224, 0.225], np.float32)
    return x.transpose(2, 0, 1)[None].copy()


def matte(path, x):
    import onnxruntime as ort

    options = ort.SessionOptions()
    options.log_severity_level = 3
    s = ort.InferenceSession(path, options, providers=["CPUExecutionProvider"])
    (logits,) = s.run(["output_image"], {"input_image": x})
    return 1.0 / (1.0 + np.exp(-logits.astype(np.float64)))


def report(a, b):
    d = np.abs(a - b)
    print(f"matte |a - b|: max {d.max():.3g}, mean {d.mean():.3g}, above 1/255: {(d > 1 / 255).mean() * 100:.4f}% of pixels")


def cmd_compare(args):
    x = preprocess(args.picture)
    report(matte(args.a, x), matte(args.b, x))


def cmd_mattes(args):
    report(np.fromfile(args.a, "<f4").astype(np.float64), np.fromfile(args.b, "<f4").astype(np.float64))


def cmd_placements(args):
    block = None
    counts = {}
    for line in open(args.log, errors="replace"):
        line = re.sub(r"^\[\w+\]\s?", "", line.rstrip())
        m = re.match(r"\s*(?:All nodes|Node\(s\)) placed on \[(\w+)\]\. Number of nodes: (\d+)", line)
        if m:
            block = m.group(1)
            counts[block] = [int(m.group(2)), collections.Counter()]
            continue
        m = re.match(r"\s+(\w+) \((.*)\)$", line)
        if block and m:
            counts[block][1][m.group(1)] += 1
            if args.names:
                print(f"{block}\t{m.group(1)}\t{m.group(2)}")
            continue
        if block and not line.startswith(" "):
            block = None
    for provider, (n, ops) in counts.items():
        print(f"{provider}: {n} nodes; {', '.join(f'{op} {k}' for op, k in ops.most_common())}")


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--bound", type=int, default=STORAGE_BUFFERS, help="storage buffers a shader may bind")
    sub = p.add_subparsers(required=True)
    r = sub.add_parser("rewrite")
    r.add_argument("input")
    r.add_argument("output")
    r.add_argument("--splits-only", action="store_true", help="only rewrite the wide Splits")
    r.set_defaults(func=cmd_rewrite)
    c = sub.add_parser("check")
    c.add_argument("model")
    c.set_defaults(func=cmd_check)
    c = sub.add_parser("compare")
    c.add_argument("a")
    c.add_argument("b")
    c.add_argument("picture")
    c.set_defaults(func=cmd_compare)
    c = sub.add_parser("mattes")
    c.add_argument("a")
    c.add_argument("b")
    c.set_defaults(func=cmd_mattes)
    c = sub.add_parser("placements")
    c.add_argument("log")
    c.add_argument("--names", action="store_true", help="print every node, not just the counts")
    c.set_defaults(func=cmd_placements)
    args = p.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
