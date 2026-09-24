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
   cast moves to just before the GatherND that uses it, where that is
   provably exact in fp16 (the reasoning is at `float_indices`).

On ONNX Runtime's CPU provider the rewritten graph answers exactly as
the original does; that is by construction, since that provider runs
fp16 arithmetic in fp32. On the card, step 2's Adds round in fp16 where
the Sum did not, which moves the matte slightly (see the notes).
The notes, section "BiRefNet on WebGPU", have the measurements.

Usage (a venv with `onnx`, `onnxruntime`, `numpy` and `pillow`):

    python birefnet_webgpu.py rewrite model_fp16.onnx model_fp16_webgpu.onnx

The output is byte for byte the same on every run with the versions in
requirements.txt, so its sha256 can go in greycard's model registry.
Other commands:

    python birefnet_webgpu.py check model_fp16_webgpu.onnx
    python birefnet_webgpu.py selftest
    python birefnet_webgpu.py compare model_fp16.onnx model_fp16_webgpu.onnx picture.png
    python birefnet_webgpu.py placements verbose.log
    python birefnet_webgpu.py mattes a.f32 b.f32

`rewrite --splits-only` does step 1 alone. `selftest` runs step 3 on
small graphs it must refuse (chains it cannot prove exact in fp16) and
some it must take, checking each against the original and against
true fp16 arithmetic. `check` runs the selftest, then exits non-zero
when a Split still binds more storage buffers than the bound allows,
or a Sum or an int64 coordinate chain is left. `compare` runs both graphs on the CPU provider of the
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
# GatherND that uses it, by the input that carries it (None: any).
INDEX_PATH = {
    "Slice": {0},
    "Reshape": {0},
    "Unsqueeze": {0},
    "Squeeze": {0},
    "Concat": None,
    "Add": None,
    "Sub": None,
    "Clip": {0},
}
# Integers up to this size are all exact in fp16.
FP16_EXACT = 2048
INF = float("inf")


def index_states(graph, cast, path, chain, values):
    """Follow a coordinate chain in graph order and say, for each of
    its tensors, what fp16 would hold there against what int64 holds:
    ("exact", lo, hi), the same integer, known to lie in [lo, hi]; or
    ("rounded",), fp16's rounding of the exact integer, which only a
    Clip may take. None when the chain is not one this pass can prove:
    an Add or Sub of a rounded value, a constant that is not a small
    integer, a Concat with an input off the chain."""
    state = {cast.output[0]: ("exact", -INF, INF)}
    for n in graph.node:
        if id(n) not in path:
            continue
        on = [x in chain for x in n.input]
        if n.op_type in ("Slice", "Reshape", "Unsqueeze", "Squeeze"):
            if not on[0] or any(on[1:]):
                return None
            state[n.output[0]] = state[n.input[0]]
        elif n.op_type == "Concat":
            if not all(on):
                return None
            ins = [state[x] for x in n.input]
            if any(s[0] == "rounded" for s in ins):
                state[n.output[0]] = ("rounded",)
            else:
                state[n.output[0]] = ("exact", min(s[1] for s in ins), max(s[2] for s in ins))
        elif n.op_type in ("Add", "Sub"):
            if sum(on) != 1 or len(n.input) != 2:
                return None
            i = on.index(True)
            c = values.get(n.input[1 - i])
            if c is None or c.dtype != np.int64 or c.size == 0 or np.abs(c).max() > FP16_EXACT:
                return None
            s = state[n.input[i]]
            if s[0] != "exact":
                return None
            cmin, cmax = int(c.min()), int(c.max())
            if n.op_type == "Add":
                lo, hi = s[1] + cmin, s[2] + cmax
            elif i == 0:
                lo, hi = s[1] - cmax, s[2] - cmin
            else:
                lo, hi = cmin - s[2], cmax - s[1]
            if -FP16_EXACT <= lo and hi <= FP16_EXACT:
                state[n.output[0]] = ("exact", lo, hi)
            else:
                state[n.output[0]] = ("rounded",)
        elif n.op_type == "Clip":
            if not on[0] or any(on[1:]) or len(n.input) < 3 or not n.input[1] or not n.input[2]:
                return None
            bounds = [values.get(x) for x in n.input[1:3]]
            if any(b is None or b.dtype != np.int64 or b.size != 1 for b in bounds):
                return None
            lo, hi = int(bounds[0]), int(bounds[1])
            if not (-FP16_EXACT <= lo <= hi <= FP16_EXACT):
                return None
            s = state[n.input[0]]
            if s[0] == "exact":
                lo, hi = max(lo, s[1]), min(hi, s[2])
            state[n.output[0]] = ("exact", lo, hi)
        else:
            return None
    return state


def float_indices(model):
    """The deformable convolutions compute their sampling coordinates
    as floats, Floor them, Cast to int64, then Slice, Add, Clip, Reshape
    and Concat them as int64 before a GatherND. The WebGPU provider
    has none of those kernels for int64, so the whole chain runs on the
    CPU with a copy out and back each. Here the chain stays in the float
    type and the Cast to int64 moves to just before each GatherND.

    Only where it is provably the same function in true fp16 (not only
    on ORT's CPU provider, which computes fp16 arithmetic in fp32):
    the Cast's input is a Floor, so every value is an integer; every
    constant is an integer of at most 2048, which fp16 holds exactly;
    an interval is carried through the chain, and an Add or Sub whose
    result may leave +-2048, where fp16 rounds, must be followed by a
    Clip to bounds within +-2048 before anything else touches it
    (rounding is monotone and leaves representable values alone, so
    Clip(round(v)) = Clip(v) for an integer v); and every index that
    reaches a GatherND is exact. Every Concat on the chain takes only
    chain inputs. BiRefNet's twenty chains are all Floor(fp16) -> Cast
    -> Slice -> [Add 1] -> Clip(0, B <= 261) -> Reshape -> Concat ->
    GatherND. A chain that does not pass is left alone. Returns how
    many chains were moved."""
    graph = model.graph
    inferred = onnx.shape_inference.infer_shapes(model)
    dtype = {vi.name: vi.type.tensor_type.elem_type for vi in list(inferred.graph.value_info) + list(graph.input)}
    values = constant_values(graph)
    producer = {x: n for n in graph.node for x in n.output}
    consumers = collections.defaultdict(list)
    for n in graph.node:
        for i, x in enumerate(n.input):
            consumers[x].append((n, i))

    moved = 0
    before = collections.defaultdict(list)  # id(GatherND) -> Casts to insert before it
    new_inits = []
    replaced = {}  # id(node) -> replacement node
    for cast in graph.node:
        attrs = {a.name: helper.get_attribute_value(a) for a in cast.attribute}
        if cast.op_type != "Cast" or attrs.get("to") != onnx.TensorProto.INT64:
            continue
        ftype = dtype.get(cast.input[0])
        if ftype not in (onnx.TensorProto.FLOAT16, onnx.TensorProto.FLOAT):
            continue
        if getattr(producer.get(cast.input[0]), "op_type", None) != "Floor":
            continue
        # Walk the int64 tensors forward to the GatherNDs.
        path, chain, gathers, ok = {}, {cast.output[0]}, [], True
        stack = [cast.output[0]]
        while stack and ok:
            x = stack.pop()
            for n, i in consumers[x]:
                allowed = INDEX_PATH.get(n.op_type, ())
                if n.op_type == "GatherND" and i == 1:
                    gathers.append((n, x))
                elif n.op_type in INDEX_PATH and (allowed is None or i in allowed):
                    if id(n) not in path:
                        path[id(n)] = n
                        chain.update(n.output)
                        stack.extend(n.output)
                else:
                    ok = False
                    break
        if not ok or not gathers:
            continue
        state = index_states(graph, cast, path, chain, values)
        if state is None or any(state[x][0] != "exact" for _, x in gathers):
            continue
        # The chain's integer constants, converted; each gets its own
        # copy, since a constant may be shared with a node off the chain.
        np_ftype = np.float16 if ftype == onnx.TensorProto.FLOAT16 else np.float32
        for n in path.values():
            if n.op_type not in ("Add", "Sub", "Clip"):
                continue
            for i, x in enumerate(n.input):
                if x and x not in chain:
                    init = numpy_helper.from_array(values[x].astype(np_ftype), f"{n.name or id(n)}/{i}_as_float")
                    n.input[i] = init.name
                    new_inits.append(init)
        replaced[id(cast)] = helper.make_node("Identity", [cast.input[0]], [cast.output[0]], name=cast.name)
        for g, x in gathers:
            as_int = f"{x}/as_int64"
            if as_int not in {c.output[0] for c in before[id(g)]}:
                before[id(g)].append(
                    helper.make_node("Cast", [x], [as_int], name=f"{x}/cast_int64", to=onnx.TensorProto.INT64)
                )
            g.input[1] = as_int
        moved += 1

    nodes = []
    for n in graph.node:
        nodes.extend(before.get(id(n), []))
        nodes.append(replaced.get(id(n), n))
    del graph.node[:]
    graph.node.extend(nodes)
    graph.initializer.extend(new_inits)
    # Shape inference recorded int64 for tensors that are now floats.
    del graph.value_info[:]
    return moved


def selftest_graph(chain, floor=True, outside_concat=False, names=True, n=4):
    """A Floor -> Cast -> `chain` -> Reshape -> [Concat] -> GatherND
    graph over data = 0..8191, so its output is the index itself."""
    T = onnx.TensorProto
    nodes, inits, cur = [], [], "x"
    if floor:
        nodes.append(helper.make_node("Floor", ["x"], ["fl"]))
        cur = "fl"
    nodes.append(helper.make_node("Cast", [cur], ["ci"], to=T.INT64))
    cur = "ci"
    for k, (op, consts) in enumerate(chain):
        names_k = []
        for j, c in enumerate(consts):
            inits.append(numpy_helper.from_array(np.array(c, np.int64), f"c{k}_{j}"))
            names_k.append(f"c{k}_{j}")
        nodes.append(helper.make_node(op, [cur] + names_k, [f"v{k}"]))
        cur = f"v{k}"
    inits.append(numpy_helper.from_array(np.array([n, 1], np.int64), "shape"))
    nodes.append(helper.make_node("Reshape", [cur, "shape"], ["idx"]))
    cur = "idx"
    if outside_concat:
        inits.append(numpy_helper.from_array(np.zeros((n, 1), np.int64), "zero"))
        inits.append(numpy_helper.from_array(np.arange(8192, dtype=np.float32).reshape(1, 8192), "data"))
        nodes.append(helper.make_node("Concat", ["zero", cur], ["idx2"], axis=1))
        cur = "idx2"
    else:
        inits.append(numpy_helper.from_array(np.arange(8192, dtype=np.float32), "data"))
    nodes.append(helper.make_node("GatherND", ["data", cur], ["y"]))
    g = helper.make_graph(
        nodes, "t", [helper.make_tensor_value_info("x", T.FLOAT16, [n])], [helper.make_tensor_value_info("y", T.FLOAT, [n])], inits
    )
    for i, node in enumerate(g.node):
        node.name = f"n{i}_{node.op_type}" if names else ""
    return helper.make_model(g, opset_imports=[helper.make_opsetid("", 17)], ir_version=8)


def fp16_chain(chain, x, floor=True):
    """The chain as the card computes it: every step rounded to fp16."""
    v = (np.floor(x) if floor else x).astype(np.float16)
    for op, consts in chain:
        if op == "Add":
            v = (v + np.float16(consts[0])).astype(np.float16)
        elif op == "Sub":
            v = (v - np.float16(consts[0])).astype(np.float16)
        elif op == "Clip":
            v = np.clip(v, np.float16(consts[0]), np.float16(consts[1]))
    return v.astype(np.int64)


# The cases a review found or the shipped file has, with whether the
# pass may move each: (name, chain, inputs, keyword arguments, moves).
SELFTEST_CASES = [
    ("Mul is not on the path", [("Mul", [3]), ("Clip", [0, 2000])], [1, 2, 3, 700], {}, False),
    ("a constant past 2048", [("Add", [3000]), ("Clip", [0, 2000])], [1, 2, 3, 4], {}, False),
    ("two Adds before the Clip", [("Add", [1000]), ("Add", [1000]), ("Sub", [1500]), ("Clip", [0, 2000])], [1001, 1003, 5, 7], {}, False),
    ("an Add after the Clip past 2048", [("Clip", [0, 2048]), ("Add", [1])], [2048, 3000, 5, 7], {}, False),
    ("no Clip", [("Add", [1])], [2048, 2050, 5, 7], {}, False),
    ("no Floor", [("Add", [1]), ("Clip", [0, 261])], [-0.5, -1.5, 0.5, 2.5], {"floor": False}, False),
    ("a Concat with an input off the chain", [("Clip", [0, 100])], [1, 2, 3, 4], {"outside_concat": True}, False),
    ("BiRefNet's shape", [("Add", [1]), ("Clip", [0, 261])], [-3000, -1, 260, 60000], {}, True),
    ("BiRefNet's shape, no Add", [("Clip", [0, 261])], [-3000, 0.5, 261.5, 60000], {}, True),
    ("an Add after the Clip within 2048", [("Clip", [0, 261]), ("Add", [1])], [-5, 3, 261, 9000], {}, True),
    ("unnamed nodes", [("Add", [1]), ("Clip", [0, 261])], [-3000, -1, 260, 60000], {"names": False}, True),
]


def selftest():
    """Run each case through `float_indices`: a case that must not move
    is left alone; one that moves still loads and runs, and its fp16
    arithmetic gives the original's indices. Returns the failures."""
    import onnxruntime as ort

    options = ort.SessionOptions()
    options.log_severity_level = 3

    def run(m, x):
        s = ort.InferenceSession(m.SerializeToString(), options, providers=["CPUExecutionProvider"])
        return s.run(None, {"x": x})[0]

    failures = []
    for name, chain, xs, kw, moves in SELFTEST_CASES:
        x = np.array(xs, np.float16)
        model = selftest_graph(chain, **kw)
        original = run(model, x)
        rewritten = onnx.load_from_string(model.SerializeToString())
        moved = float_indices(rewritten)
        if bool(moved) != moves:
            failures.append(f"{name}: {'moved' if moved else 'left'}, expected {'moved' if moves else 'left'}")
            continue
        if not moved:
            continue
        try:
            onnx.checker.check_model(rewritten)
            got = run(rewritten, x)
        except Exception as e:  # noqa: BLE001 - any refusal is a failure
            failures.append(f"{name}: the rewrite does not load: {e}")
            continue
        card = fp16_chain(chain, x, kw.get("floor", True))
        if not np.array_equal(got, original) or not np.array_equal(card.astype(np.float32), original.reshape(-1)):
            failures.append(f"{name}: original {original}, rewrite {got}, in fp16 {card}")
    return failures


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


def cmd_selftest(args):
    failures = selftest()
    for f in failures:
        print(f"selftest: {f}")
    if failures:
        sys.exit(1)
    print(f"selftest: {len(SELFTEST_CASES)} cases pass")


def cmd_check(args):
    cmd_selftest(args)
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
    c = sub.add_parser("selftest", help="the coordinate pass's own cases")
    c.set_defaults(func=cmd_selftest)
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
