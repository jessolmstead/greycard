"""Export Meta's SAM 3 checkpoint to the three ONNX graphs the trial
(and later the editor) runs: image encoder, language encoder, decoder.

    python export.py SAM3_PT BPE_VOCAB_GZ CHECK_IMAGE OUT_DIR [CLIP_TOKENIZER_JSON]

SAM 3 by Meta (Carion et al., 2025), the SAM License; the weights are
`facebook/sam3` on Hugging Face, `sam3.pt`. The model code is Meta's
`sam3` package at Kentaro Wada's ONNX branch (wkentaro/sam3, commit
812de8a), which swaps the complex rotary encoding for cos and sin and
removes a few calls the exporter cannot trace; the graph split and the
two encoder wrappers follow his sam3-onnx (MIT). What is ours is the
decoder: the published exports apply SAM 3's 0.5 inside the graph and
drop the rest; ours returns the K best queries whatever their score,
ranked, with the presence score on its own, so the cut is ours to
choose. K is 16, more than any phrase in the trial finds; reading
back all 200 queries' masks was most of a decode's time on WebGPU.

    decoder outputs  boxes     K x 4, normalized x0 y0 x1 y1
                     scores    K, descending: each query's sigmoid
                               times the presence score, SAM 3's own
                               score for an instance
                     presence  1, the presence head's sigmoid
                     masks     K x 1 x 288 x 288, sigmoid

Each graph is checked against the PyTorch model it came from on
CHECK_IMAGE and the phrase "hair", and SAM 3's tokenizer is checked
against the Hugging Face CLIP tokenizer the trial uses, when its
tokenizer.json is given, over every phrase in prompts.py.
"""

import pathlib
import sys

import numpy as np
import onnxruntime as ort
import torch
from PIL import Image
from torchvision.transforms import v2

from sam3.model.sam3_image_processor import Sam3Processor
from sam3.model.tokenizer_ve import SimpleTokenizer
from sam3.model_builder import build_sam3_image_model

ckpt, bpe, check_image, out = sys.argv[1:5]
out = pathlib.Path(out)
out.mkdir(parents=True, exist_ok=True)
torch.manual_seed(0)
TOP_K = 16


def cos_sin_rope(module):
    """Replace each complex freqs_cis buffer with real cos and sin."""
    if hasattr(module, "freqs_cis"):
        module.register_buffer("freqs_cos", module.freqs_cis.real.float())
        module.register_buffer("freqs_sin", module.freqs_cis.imag.float())
        del module.freqs_cis
    for child in module.children():
        cos_sin_rope(child)


model = build_sam3_image_model(bpe_path=bpe, device="cpu", checkpoint_path=ckpt, load_from_HF=False)
with torch.no_grad():
    cos_sin_rope(model)
model.requires_grad_(False)
processor = Sam3Processor(model, device="cpu")


class ImageEncoder(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.model = model
        self.norm = v2.Compose([
            v2.ToDtype(torch.float32, scale=True),
            v2.Normalize(mean=[0.5, 0.5, 0.5], std=[0.5, 0.5, 0.5]),
        ])

    def forward(self, image):
        x = self.norm(image).unsqueeze(0)
        o = self.model.backbone._forward_image_no_act_ckpt(x)
        return *o["vision_pos_enc"], *o["backbone_fpn"]


class LanguageEncoder(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.model = model

    def forward(self, tokens):
        lb = self.model.backbone.language_backbone
        embeds = lb.encoder.token_embedding(tokens)
        _, memory = lb.encoder(tokens)
        mask = (tokens != 0).ne(1)
        memory = lb.resizer(memory.transpose(0, 1))
        return mask, memory, embeds.transpose(0, 1)


class Decoder(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.model = model

    def forward(self, pos2, fpn0, fpn1, fpn2, language_mask, language_features,
                box_coords, box_labels, box_masks):
        prompt = self.model._get_dummy_prompt()
        prompt.box_embeddings = box_coords
        prompt.box_labels = box_labels
        prompt.box_mask = box_masks
        backbone_out = {
            "vision_pos_enc": [pos2, pos2, pos2],
            "backbone_fpn": [fpn0, fpn1, fpn2],
            "language_mask": language_mask,
            "language_features": language_features,
        }
        o = self.model.forward_grounding(backbone_out=backbone_out, find_input=processor.find_stage,
                                    geometric_prompt=prompt, find_target=None)
        cx, cy, w, h = o["pred_boxes"][0].unbind(-1)
        boxes = torch.stack([cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2], dim=-1)
        presence = o["presence_logit_dec"].reshape(1).sigmoid()
        scores, best = (o["pred_logits"][0, :, 0].sigmoid() * presence).topk(TOP_K)
        masks = o["pred_masks"][0][best].unsqueeze(1).sigmoid()
        return boxes[best], scores, presence, masks


def export(module, args, name, inputs, outputs):
    path = out / f"{name}.onnx"
    torch.onnx.export(module, args, path, input_names=inputs, output_names=outputs,
                      opset_version=21, dynamo=False, external_data=True)
    return path


def check(name, path, feed, expected):
    sess = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    got = sess.run(None, feed)
    for (oname, e), g in zip(expected.items(), got):
        e = e.detach().numpy() if isinstance(e, torch.Tensor) else e
        diff = np.abs(e.astype(np.float64) - g.astype(np.float64)).max() if e.size else 0.0
        print(f"  {name}.{oname}: max |torch - onnx| = {diff:.2e}")
    return got


image = np.asarray(Image.open(check_image).convert("RGB").resize((1008, 1008), Image.BILINEAR))
image = np.ascontiguousarray(image.transpose(2, 0, 1))
tok = SimpleTokenizer(bpe_path=bpe)
tokens = tok(["hair"], context_length=32)

with torch.no_grad():
    enc = ImageEncoder().eval()
    img_t = torch.from_numpy(image)
    img_ref = enc(img_t)
    enc_names = ["vision_pos_enc_0", "vision_pos_enc_1", "vision_pos_enc_2",
                 "backbone_fpn_0", "backbone_fpn_1", "backbone_fpn_2"]
    p = export(enc, (img_t,), "sam3_image_encoder", ["image"], enc_names)
    print("image encoder")
    check("image", p, {"image": image}, dict(zip(enc_names, img_ref)))

    lang = LanguageEncoder().eval()
    lang_ref = lang(tokens)
    lang_names = ["text_attention_mask", "text_memory", "text_embeds"]
    p = export(lang, (tokens,), "sam3_language_encoder", ["tokens"], lang_names)
    print("language encoder")
    check("language", p, {"tokens": tokens.numpy()}, dict(zip(lang_names, lang_ref)))

    dec = Decoder().eval()
    box_coords = torch.zeros(1, 1, 4)
    box_labels = torch.ones(1, 1, dtype=torch.int64)
    box_masks = torch.ones(1, 1, dtype=torch.bool)
    dec_args = (img_ref[2], img_ref[3], img_ref[4], img_ref[5], lang_ref[0], lang_ref[1],
                box_coords, box_labels, box_masks)
    dec_in = ["vision_pos_enc_2", "backbone_fpn_0", "backbone_fpn_1", "backbone_fpn_2",
              "language_mask", "language_features", "box_coords", "box_labels", "box_masks"]
    dec_out = ["boxes", "scores", "presence", "masks"]
    dec_ref = dec(*dec_args)
    p = export(dec, dec_args, "sam3_decoder", dec_in, dec_out)
    print("decoder")
    got = check("decoder", p, {n: a.numpy() for n, a in zip(dec_in, dec_args)}, dict(zip(dec_out, dec_ref)))
    s = got[1]
    print(f"  'hair': presence {got[2][0]:.3f}, {int((s > 0.5).sum())} queries over 0.5, top {s[0]:.3f}")

# The trial tokenizes with Hugging Face's CLIP tokenizer; it must agree
# with SAM 3's own on every phrase the presets hold.
if len(sys.argv) > 5:
    from tokenizers import Tokenizer

    from prompts import EYE, FREE, PRESETS, SHIPPED
    from sam3onnx import tokenize

    hf = Tokenizer.from_file(sys.argv[5])
    phrases = list(dict.fromkeys(PRESETS + FREE + EYE + SHIPPED))
    bad = [ph for ph in phrases if not np.array_equal(tok([ph], context_length=32).numpy(), tokenize(hf, ph))]
    print(f"tokenizers agree on {len(phrases) - len(bad)} of {len(phrases)} phrases; differ on {bad}")
