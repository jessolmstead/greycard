"""Write the phrase table the editor ships: sam3_presets.bin, and
sam3_presets.json beside it for reading by eye.

    python table.py MODELS_DIR OUT_DIR

Runs the language encoder (MODELS_DIR/sam3_language_encoder.onnx,
tokenized by sam3onnx.py with MODELS_DIR/clip_tokenizer.json) over
every phrase in prompts.SHIPPED, writes the decoder's two text inputs
per phrase, reads the file back and checks every value is the
encoder's to the bit. The editor feeds the decoder from this table
and never loads the 1.4 GB language encoder.

The file is flat and little-endian:

    u32                 the phrase count
    per phrase, in SHIPPED's order:
      u16               the phrase's length in bytes
      bytes             the phrase, UTF-8
      32 x u8           language_mask, 0 or 1 (1: a padding token)
      32 x 256 x f32    language_features (32 x 1 x 256)

The JSON maps each phrase to its index and says what the file holds.
"""

import hashlib
import json
import pathlib
import struct
import sys

import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

from prompts import SHIPPED
from sam3onnx import CONTEXT, session, tokenize

WIDTH = 256


def write_table(path: pathlib.Path, table: dict):
    with open(path, "wb") as f:
        f.write(struct.pack("<I", len(table)))
        for phrase, (mask, features) in table.items():
            name = phrase.encode("utf-8")
            f.write(struct.pack("<H", len(name)))
            f.write(name)
            f.write(np.asarray(mask, np.uint8).reshape(CONTEXT).tobytes())
            f.write(np.asarray(features, "<f4").reshape(CONTEXT * WIDTH).tobytes())


def read_table(path: pathlib.Path) -> dict:
    """phrase -> {"language_mask": bool 1 x 32, "language_features": f32
    32 x 1 x 256}, the decoder's two inputs as sam3onnx.Sam3.decode
    takes them."""
    data = pathlib.Path(path).read_bytes()
    (count,), at = struct.unpack_from("<I", data, 0), 4
    out = {}
    for _ in range(count):
        (n,) = struct.unpack_from("<H", data, at)
        at += 2
        phrase = data[at : at + n].decode("utf-8")
        at += n
        mask = np.frombuffer(data, np.uint8, CONTEXT, at).astype(np.bool_).reshape(1, CONTEXT)
        at += CONTEXT
        features = np.frombuffer(data, "<f4", CONTEXT * WIDTH, at).astype(np.float32).reshape(CONTEXT, 1, WIDTH)
        at += CONTEXT * WIDTH * 4
        out[phrase] = {"language_mask": mask, "language_features": features}
    assert at == len(data), f"{len(data) - at} bytes left over"
    return out


if __name__ == "__main__":
    ort.set_default_logger_severity(3)
    models, out = map(pathlib.Path, sys.argv[1:3])
    out.mkdir(parents=True, exist_ok=True)
    assert len(SHIPPED) == len(set(SHIPPED)), "a phrase twice in SHIPPED"
    language = session(models / "sam3_language_encoder.onnx")
    tok = Tokenizer.from_file(str(models / "clip_tokenizer.json"))
    table = {}
    for phrase in SHIPPED:
        mask, memory, _ = language.run(None, {"tokens": tokenize(tok, phrase)})
        assert mask.shape == (1, CONTEXT) and memory.shape == (CONTEXT, 1, WIDTH), phrase
        table[phrase] = (mask, memory)

    bin_path = out / "sam3_presets.bin"
    write_table(bin_path, table)
    back = read_table(bin_path)
    assert list(back) == SHIPPED
    for phrase, (mask, memory) in table.items():
        assert np.array_equal(back[phrase]["language_mask"], mask), phrase
        assert np.array_equal(back[phrase]["language_features"], memory), phrase

    digest = hashlib.sha256(bin_path.read_bytes()).hexdigest()
    (out / "sam3_presets.json").write_text(json.dumps({
        "file": bin_path.name,
        "bytes": bin_path.stat().st_size,
        "sha256": digest,
        "phrases": {p: i for i, p in enumerate(SHIPPED)},
        "tokens": {p: int((~table[p][0]).sum()) for p in SHIPPED},
    }, indent=1) + "\n")
    print(f"{len(table)} phrases, {bin_path.stat().st_size} bytes, sha256 {digest}")
