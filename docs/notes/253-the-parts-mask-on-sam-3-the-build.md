# 253. The Parts mask on SAM 3: the build (2026-10-06)

A design, not yet built: §252's trial made into a mask shape. The user
picks a part from a menu (Face, Body or Clothing), and on a picture
with more than one person, the person; the shape finds that part of
that person on any picture it lands on, including a picture it was
pasted to, where it looks for the same person rather than whoever
stands in the same place. It follows the paths Subject, Sky and Object
already take (registry, store, the worker's `Ai`, the PNG cache, the
masks panel); what is new is one model module, one shape kind, the
crop routes, and the person's signature.

Decided with the user: a person is picked, with "All people" beside
the people found; the shape stores the phrase as text, so "Describe
your own mask" reuses it later with no schema change; the first menu
is Face, Body and Clothing (no accessories); the build is an Opus
author with a fresh Opus reviewer, in two parts, the model side first.

### The model files

Three files under one registry entry, `SAM3`, id
`sam3-fp16-top16-1`, hosted at `huggingface.co/jessolmstead/greycard-sam3`
with the SAM License beside them (`modified`: converted to ONNX, the
image encoder to fp16, the decoder returning the sixteen best queries;
§252's `export.py` and `fp16.py`):

- `sam3_image_encoder_fp16.onnx`, 908,762,234 bytes: uint8
  3 × 1008 × 1008 in, fp32 out (`vision_pos_enc_0..2`,
  `backbone_fpn_0..2`; the decoder takes `vision_pos_enc_2` and the
  three `backbone_fpn`).
- `sam3_decoder.onnx`, 129,854,990 bytes: the four maps, the phrase's
  `language_mask` (bool 1 × 32) and `language_features` (f32
  32 × 1 × 256), and a box prompt masked off (`box_coords` zeros
  1 × 1 × 4, `box_labels` ones i64 1 × 1, `box_masks` true 1 × 1); out
  `boxes` 16 × 4 (normalized x0 y0 x1 y1), `scores` 16 descending,
  `presence` 1, `masks` 16 × 1 × 288 × 288 (sigmoid).
- `sam3_presets.bin`: the preset table. A flat file, little-endian: a
  u32 count, then per phrase a u16 byte length and the UTF-8 phrase,
  32 mask bytes (0 or 1) and 32 × 256 f32. Written from §252's
  `presets.py` table by a new `tools/ai/sam3_trial/table.py`, which
  also writes `sam3_presets.json` (phrase to its index) for reading by
  eye. The phrases are the menu's and the routes' (below) plus
  `person`, `face`, `eyes` and `mouth`.

The files go up when the user says so; until then the registry's
sha256 values are those of the files in `target/sam3/models/ours/`
and the table the author writes, and tests read a local store
(`GREYCARD_MODELS`). The first-use sheet says about 1.04 GB.

### `greycard-ai/src/sam3.rs`

- `Sam3::load(store, providers)` through `runtime::open`, both graphs
  at `Level3` (the bench ran so), WebGPU then the CPU. `providers()`
  as `Sam` has.
- `Presets::read(path)` → the phrase table; `phrase(&str)` →
  `Option<&Phrase>`. A phrase not in the table is an error the UI
  shows ("not a preset"), never a call into a text encoder.
- `encode(&mut self, &Rgb8) -> Result<Encoding>`: the picture
  stretched to 1008² (bilinear), uint8 planes, the four maps kept.
- `decode(&mut self, &Encoding, &Phrase) -> Result<Found>`: `Found {
  presence, instances: Vec<Instance { score, bbox, mask: Mask(288²) }>
  }` with every query returned, so the cut is the caller's (`CUT =
  0.5`).
- Pure functions, each with a CPU reference test: `square(bbox,
  scale, size, min_side)` (§252's crop rule), `components(mask, cut)`
  (the eyes split: connected parts at 0.5, each boxed, parts under
  three cells across dropped), `union(instances, cut)` (max of the
  sigmoids over the instances above the cut), and the signature and
  match below.

### The routes

A part's phrase is asked one of three ways (`Route`, stored in the
shape):

- `Whole`: the 2048 preview encoded once, the phrase decoded, the
  instances over the cut united. Face, facial skin, skin, hair,
  eyebrows, hands, and the clothing.
- `Eye`: `face` on the preview; for each chosen face, `eyes` on a
  1.3 × square crop of it; the eyes mask split into its parts; each
  eye a 2.5 × square crop; the phrase ("iris of the eye") on each.
- `Mouth`: as `Eye` with `mouth` (the best instance, one per face) and
  a 1.8 × crop; teeth and lips.

The face and eye crops are cut from the full-resolution base develop,
not the 2048 preview: at 2048 an eye in a full-length frame is a few
dozen pixels. `ai.rs` gains `region(image, edit, rect) -> Rgb8`: the
same display rendering `preview` makes, of one rectangle of the base
image at its own pixels. Each crop's mask is refined at the crop's
resolution against its own luma (`refine`, radius a 128th of the
crop's side, ε 1e-3; §252: 1e-4 cuts the catchlight out of the
iris), then drawn into the 2048-wide raster at its place, the max
where crops overlap. The raster stays 2048 wide; a finer raster for
small parts is the same question as Sky's 4096 (§175's roadmap line)
and waits with it.

Encodings are cached in `Ai` as SAM 2's is: the preview's under the
base `stamp`, and the crops' under `(stamp, rect)`, dropped with the
preview. So a second phrase on the same person costs only decodes.

### The shape

```rust
Part {
    phrase: String,          // "iris of the eye"
    route: Route,            // whole | eye | mouth
    #[serde(default)]
    person: Option<Person>,  // None: everyone
}
pub struct Person { signature: Signature, at: Pos }
```

`of_kind` takes `"Part:<phrase>"`; the menu maps its labels to
phrases and routes (Iris → `iris of the eye`, `Eye`; Teeth →
`teeth`, `Mouth`; Lips → `lips`, `Mouth`; Top → `upper body
clothing`; Bottoms → `lower body clothing`; All clothing →
`clothing`; the rest `Whole` under their own name, lowercased). `is_learned` and `is_raster` are
true; `name()` is the menu's label; `ShapeGpu::of` gives it the
raster layer (kind 2) as Object has. The PNG cache key hashes the
shape's JSON as for every learned shape, which now includes the
person, so a mask for one person is never served for another.

### People, and the same person on another picture

A person is a `face` instance over the cut on the preview, paired with
the `person` instance whose box holds the face's center (the best
such). Fewer than two faces: no picking, `person` stays `None`.

**Signature.** From the person's instance mask on the preview, split
into three bands of equal height (head, upper, lower): in each band
the mean and standard deviation of CIELAB over the masked pixels,
plus the band's masked fraction; and the face box's size over the
picture's diagonal. Eighteen numbers and one; computed on the preview
render, which has no adjustments in it (`preview` clears them), so a
paste between pictures edited differently compares like with like.
No face embedding: nothing in it identifies a face, only clothes,
hair and skin as colors.

**Match.** On any picture, a `Part` with a `person` resolves it
afresh: every person found is scored by the distance between
signatures (bands weighted 1, 1.5, 1.5 for head, upper, lower; size
weighted 0.5), and `at` (where the face was, in masks' units) breaks
ties only on the picture the shape was made on. The best wins when it
is clearly ahead: the second's distance at least 1.5 × the best's,
and the best under an absolute bound the author sets from §252's
frames and states in a test. Otherwise the mask is empty and the
status line says "which person? click one", with the panel in the
same placing state Object uses; a click picks the person under it and
writes their signature and `at` into the shape. On the picture the
shape was made on, the match is the person picked, which is the
test's first case.

This is what makes a pasted Lips mask land on the same woman in the
next frame of a shoot rather than whoever stands where she stood.
It fails, by saying so, where outfits match; face recognition would
not, and stays a separate item (below).

### The UI

The masks panel's model row gains a Parts button opening a menu in
three groups: Face (Face, Facial skin, Eyebrows, Eyes, Iris, Lips,
Teeth), Body (Skin, Hair, Hands), Clothing (Top, Bottoms, Shoes, All
clothing).

Clothing is by where it is worn, not by garment, at the user's
suggestion, checked on §252's fourteen frames: "upper body clothing"
scored 0.83 to 0.98 on all of them and takes a coat and the top
under it as one, which is what a look on clothing wants; "lower
body clothing" 0.83 to 0.97, and none on the three frames that show
no legs; "clothing" 0.78 to 0.97. The short words score lower ("top"
0.52 to 0.95), so the menu says Top and asks in the long phrasing.
Garments by name, with "dress" among them, stay for Describe. Choosing one on a picture with
two or more people arms the placing state with "click the person, or
All people" and an All people button; one person or none, the shape
is made at once, as Subject is. The first use asks for the download
through the existing model sheet (`Step::Ask`). On the CPU provider,
the status line says before the run that a part takes several seconds
and an eye or mouth part about fifteen.

### Tests

- Pure: `square`, `components`, `union`, the table reader round trip,
  the signature on a synthetic three-band figure, the match (a clear
  winner, two equal figures → ambiguous, the absolute bound), the
  shape's serde (with and without `person`, an unknown route
  rejected).
- Ignored, with `GREYCARD_MODELS`: decode a preset against a reference
  the Python side writes (`tools/ai/sam3_trial/reference.py`, a
  picture's presence and best scores for three phrases, tolerance
  1e-3) on CPU and on WebGPU; the Eye route on a picture given by
  `GREYCARD_SAM3_PICTURE` finds two irises. No picture enters the
  repo.

### In two parts

1. **The model side** (`greycard-ai`): the table writer, the registry
   entry, `sam3.rs`, the routes as functions over an `Rgb8` and a
   region callback, the signature and match, their tests.
2. **The editor side** (`greycard-edit`, `greycard-ui`): the shape,
   the cache keys, `region`, the worker job, the menu, the person
   picking, the paste behavior (nothing to do in `sync.rs`: the
   shape carries its person, and resolving happens at raster time).

Not in this build: MediaPipe's iris as the CPU fallback (§252); the
Describe field; face recognition for matching across outfits and
days, which waits on a license search (the strong face-embedding
models are mostly non-commercial, InsightFace's among them)
and on deciding whether a sidecar may hold a face embedding at all,
which is biometric data about the people photographed.
