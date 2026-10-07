"""SAM 3 through ONNX Runtime, split the way the editor would run it.

Three graphs, as the community exports split them (and as our own
export should): an image encoder run once per picture, a language
encoder run once per phrase, and a decoder run per (picture, phrase).
A preset is the language encoder's two outputs saved to disk, so the
decoder can be fed without the language encoder present.

    image encoder     image uint8 3 x 1008 x 1008 (the picture
                      stretched to the square, RGB)
                      -> vision_pos_enc_0..2, backbone_fpn_0..2
    language encoder  tokens int64 1 x 32 (CLIP BPE, start and end
                      tokens, zero padded)
                      -> text_attention_mask bool 1 x 32,
                         text_memory f32 32 x 1 x 256, text_embeds
    decoder           the encoder outputs, language_mask,
                      language_features, and a box prompt (masked off
                      for text alone)
                      -> boxes N x 4 (normalized x0 y0 x1 y1),
                         scores N, masks N x 1 x 288 x 288 (sigmoid)
"""

import pathlib
import time

import numpy as np
import onnxruntime as ort
from PIL import Image
from tokenizers import Tokenizer

SIDE = 1008
CONTEXT = 32


def session(path, providers=("CPUExecutionProvider",)):
    opts = ort.SessionOptions()
    opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    return ort.InferenceSession(str(path), opts, providers=list(providers))


def tokenize(tok: Tokenizer, text: str) -> np.ndarray:
    # The HF CLIP tokenizer adds start (49406) and end (49407) itself;
    # SAM 3's own tokenizer pads with zeros, not with the end token.
    ids = tok.encode(text.lower()).ids
    if len(ids) > CONTEXT:
        ids = ids[: CONTEXT - 1] + [ids[-1]]
    out = np.zeros((1, CONTEXT), dtype=np.int64)
    out[0, : len(ids)] = ids
    return out


class Sam3:
    def __init__(self, image_encoder, decoder, language_encoder=None, tokenizer=None):
        self.image = session(image_encoder)
        self.decoder = session(decoder)
        self.language = session(language_encoder) if language_encoder else None
        self.tok = Tokenizer.from_file(str(tokenizer)) if tokenizer else None

    def encode_image(self, im: Image.Image):
        x = np.asarray(im.convert("RGB").resize((SIDE, SIDE), Image.BILINEAR))
        x = np.ascontiguousarray(x.transpose(2, 0, 1))
        inp = self.image.get_inputs()[0]
        if inp.type == "tensor(float16)":
            x = x.astype(np.float16)
        elif inp.type == "tensor(float)":
            x = x.astype(np.float32)
        t = time.perf_counter()
        out = self.image.run(None, {inp.name: x})
        dt = time.perf_counter() - t
        names = [o.name for o in self.image.get_outputs()]
        return {n: v.astype(np.float32) for n, v in zip(names, out)}, dt

    def encode_text(self, text: str):
        mask, memory, _ = self.language.run(None, {"tokens": tokenize(self.tok, text)})
        return {"language_mask": mask, "language_features": memory}

    def decode(self, feats, prompt, cut=0.5):
        """Boxes, scores and masks of the instances over `cut`. With our
        own export (the 16 best queries, ranked, and the presence score
        apart), `self.raw` also holds the presence and the five best
        scores, whether over the cut or not; the community exports cut
        at 0.5 inside the graph and give neither."""
        feed = {k: feats[k] for k in ("vision_pos_enc_2", "backbone_fpn_0", "backbone_fpn_1", "backbone_fpn_2")}
        feed.update(prompt)
        feed["box_coords"] = np.zeros((1, 1, 4), np.float32)
        feed["box_labels"] = np.ones((1, 1), np.int64)
        feed["box_masks"] = np.ones((1, 1), np.bool_)
        t = time.perf_counter()
        out = dict(zip((o.name for o in self.decoder.get_outputs()), self.decoder.run(None, feed)))
        dt = time.perf_counter() - t
        if "presence" not in out:
            self.raw = None
            return out["boxes"], out["scores"], out["masks"], dt
        scores = out["scores"]
        keep = scores > cut
        self.raw = {
            "presence": round(float(out["presence"][0]), 4),
            "top": [round(float(s), 4) for s in scores[:5]],
        }
        return out["boxes"][keep], scores[keep], out["masks"][keep], dt


def save_presets(path: pathlib.Path, presets: dict):
    flat = {}
    for name, p in presets.items():
        flat[f"{name}|mask"] = p["language_mask"]
        flat[f"{name}|features"] = p["language_features"]
    np.savez(path, **flat)


def load_presets(path: pathlib.Path) -> dict:
    z = np.load(path)
    out = {}
    for key in z.files:
        name, part = key.rsplit("|", 1)
        out.setdefault(name, {})["language_mask" if part == "mask" else "language_features"] = z[key]
    return out
