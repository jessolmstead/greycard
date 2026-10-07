# 256. The Parts mask's model side, as built (2026-10-06)

Part 1 of §253's build: `greycard-ai`'s `sam3` module, the registry
entry, the phrase table and their tests. Nothing in the editor uses it
yet; part 2 (the shape, the worker job, the menu and the picking) does.
Where this differs from §253, this is what was built, and the person
match in particular is not §253's.

### The files and the module

The registry entry `SAM3`, id `sam3-fp16-top16-1`, holds three files
hosted at `huggingface.co/jessolmstead/greycard-sam3` under the SAM
License, whose text is `sam_license.txt` beside the module:

| File | Size | Sha256 |
|---|---|---|
| `sam3_image_encoder_fp16.onnx` | 908,762,234 bytes | `dd4733a6…` |
| `sam3_decoder.onnx` | 129,854,990 bytes | `7c205232…` |
| `sam3_presets.bin` | 524,962 bytes | `a1c205ef…` |

`tools/ai/sam3_trial/table.py` writes the phrase table from
`prompts.py`'s `SHIPPED`: the menu's phrases and those the routes ask
on their own (`face`, `eyes`, `mouth`, `person`). `reference.py`
writes the Python side's scores for the decode test.

`Sam3::load` opens both graphs through `runtime::open`, WebGPU first
and then the CPU. `encode` squashes the picture to 1008² and `decode`
returns every one of the sixteen queries, so the cut (`CUT = 0.5`) is
the caller's. The squash is a port of Pillow's bilinear resample, so
the encoder sees the bytes the trial's Python fed it. Its MIT-CMU
notice is in the module's header.

The routes are §253's: `Whole`, `Eye` and `Mouth`. The cascades crop
the full-resolution picture through a callback the caller gives, and
each crop's mask is refined at its own resolution (radius a 128th of
the crop's side, ε 1e-3) and handed back as a `Piece` for `draw` to put
into the 2048 raster. The Eye route keeps the two largest eyes a face
has, left to right.

Encodings are cached in `Encodings`, keyed by the caller's `key` (part
2 passes the base stamp): the preview's, and each crop's under its
rect. Everything is dropped when the key changes, so a second phrase
on the same person costs only decodes.

### People

A person is every distinct face over the cut on the preview. Two face
boxes overlapping at IoU above 0.5 are one face found twice. Each face
is paired with the untaken `person` instance highest at the face's
center, and within 0.01 of that, the smallest box: a baby in an adult's
arms takes the baby's body, not the adult's. Two faces never share a
body. A face left with no body is still a person, measured from its
face box alone.

That was the review's main finding. Counting only paired faces made a
couple whose bodies the model merged into one look like one person,
and the match then decided alone on whoever was paired.

**Signature.** The kind `colors-1` holds the mean and spread in CIELAB
and the masked fraction for three bands down the body (head, upper,
lower), plus the face's size over the picture's diagonal. It is taken
on the preview, which has no adjustments in it. It is stored as
`{"kind": "colors-1", "values": [...]}`.

A kind this build doesn't know loads and saves back unchanged but is
never compared. So a later build can write a different kind, such as a
face's place for face matching, without breaking a sidecar synced to
an older build. Two signatures compare only if they share the upper
band, or two of the three bands. A bodiless face has the head band
only, so it compares with no one.

### The match: decide only where colors can be trusted

§253 had the match pick the nearest person when it was clearly ahead.
On §252's frames that failed on group pictures: people dressed alike
for a shoot are as near each other as one person across two frames
(strangers in one group as near as 3.65, the same woman across her
frames 7.44). `choose` now decides alone in two cases only:

- **On the shape's own picture.** A same render is the same known kind,
  a band shown, and every value within 0.25. The drift between CPU and
  WebGPU is at most 0.008, and the nearest two of 19 people differ by
  4.78 in some value. The face must also be within about a face's width
  of `at`, and the nearest wins a tie.
- **On a picture with exactly one face.** That person is chosen if
  their distance is within `BOUND` (7.5). Over the bound, nobody. A lone
  person who can't be compared is asked about, rather than giving a
  silently empty mask.

Everywhere else with two or more faces it asks (`Choice::Ask`). The
ask carries a guess only when the nearest is within `BOUND`, everyone
else is comparable, and the next is at least 1.5 times as far
(`MARGIN`). A highlighted coin flip helps no one.

On the 13 frames, every person was checked against every other frame,
228 checks:

| Outcome | Count |
|---|---|
| Right person, decided alone | 4 |
| Wrong person, decided alone | 0 |
| Same woman missed | 0 |
| Nobody (one-person frames) | 176 |
| Ask, no guess | 48 |
| Ask, with a guess | 0 |

Each of the five women on the group picture resolves to herself on
her own picture. The one-person bound has little room on the frames
it was set on (the same woman at 7.44 against 7.5, the nearest
strangers at 7.93). That is the only automatic decision away from the
source picture.

A face embedding would tell people apart where colors cannot; the
search for one is recorded with the roadmap's item. An embedding is
biometric data, so it would be computed in memory at paste time and
never stored in a sidecar. The shape would keep only the picture it
was made on and where the face was.

### A person's Whole-route parts

With a body, a person's Whole-route parts are kept to that body, grown
by three mask cells. A bodiless person keeps each whole instance of
the phrase that reaches into a body-shaped region below the face (a
quarter face height above it to seven below, three face widths across)
and no further into anyone else's body. A body that holds their own
face doesn't count as someone else's. So their Top or long hair comes
back whole rather than cut at the jaw.

Left for later: when a merged body holds two faces, the face that took
it keeps that whole body, so its Top includes the other person's.
That is the "All people" extent, shown without saying so. Sending every
face a merged body holds through the bodiless rule would fix it.

### Tests

- 28 unit tests, with a stand-in model for the routes and the people:
  - the pure functions: `square`, `components`, `union`, `draw`, the
    resample, the table round trip;
  - the signature, `distance`, `same_render` and `choose`, including
    unknown kinds, blanks and non-finite values;
  - the pairing with a baby, a merged body and 30 bodies.
- The ignored tests need `GREYCARD_MODELS`:
  - the decode against the Python reference, at most 2.7e-4 off on the
    CPU and 8.7e-3 on WebGPU;
  - the Eye route finding two irises: 3.3 s on WebGPU, 37 s on the CPU
    in a debug build;
  - the 13 frames above.
- No picture enters the repo.
