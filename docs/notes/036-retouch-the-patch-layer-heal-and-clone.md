# 36. Retouch: the patch layer, heal and clone (2026-09-07)

The eraser's first tier, and the layer the learned fill will sit
behind. A patch is a spot or a stroke on the developed picture,
replaced from elsewhere in it, before the look: so every adjustment
after it sees the repaired pixels, on the viewport and in the export.

**In the edit** (`retouch.rs`): `Edit.retouch.patches`, each a
`Patch { id, method: Heal | Clone, points, radius, feather, opacity,
source: Option<Pos> }` in the masks' units. `Patch::region(w, h)`
gives the engine a window with each pixel's coverage (distance to the
polyline, the outer `feather` of the radius smoothstepped) and a ring
just outside it (1 to 1.7 radii). The source is an offset, chosen by
the engine when a patch has none and written back into the edit and
so the sidecar, so it stays put. `Edit::same_patched` sits between
`same_base` and `same_develop`.

**In the engine** (`develop::retouch`): `find_source` scores 32
candidates on two rings round the patch (2.2 and 3.2 radii, 16
angles) by the mean squared difference over the ring between the
picture and the picture shifted, and takes the least: a source whose
surroundings match. `apply` copies the source under the coverage
(clone) or transfers its texture (heal): the source minus its own low
frequencies plus the destination's, where the destination's low
frequencies come from a blur normalized by (1 − coverage), so the
blemish being removed does not color its replacement. The blur is
three box passes (σ = radius/2). Tests: a clone copies, a heal takes
a dark spot out of a gradient to within 0.03, opacity halves the
change, the finder prefers a source along the gradient, a weighted
blur ignores what is weighted out.

**In the worker:** `Base.patched` caches the base with the retouch
applied, for the retouch it was; a change copies the base (24 MP,
tens of ms) and applies every patch. The sharpen runs on that. Exports
that need a develop go through `develop_job` now, so they carry the
retouch too.

**In the editor:** a RETOUCH section with Heal and Clone tools, kept
in hand like the brush: a click is a spot, a drag a stroke, the wheel
sizes it, Esc or the button puts it down. Size, Feather and Opacity
edit the chosen patch and set the next. The patches list by name with
a bin. The chosen patch shows two pins on the view, its center and its
source, draggable; a drop develops. Sources chosen by the engine
arrive with the develop and go into the edit.

Checked on the bridge frame: a heal spot over the couple takes rock
texture from the left with the local light, as a spot that small over
a person should. What is not here yet: the patch outline on the view
(the pins and the cursor circle stand in), a source chosen by texture
rather than surroundings, and updating only the changed window rather
than the whole picture. LaMa is next, as a third method filling the
same region from a model.

### Fill: the eraser, LaMa behind the same patches (2026-09-07)

`Method::Fill` is the third method on a patch, with no source: the
region is made up by a model from what is around it. The model is
big-lama through Carve's ONNX export (`registry::FILL`, Apache-2.0,
208 MB, a fixed 512×512 in and out), which runs on WebGPU here in
91 ms and on the CPU in 1.2 s, and `greycard-ai::Fill` wraps it. Two
things the export leaves to the caller: the picture under the mask
must be blanked before the network (LaMa's own forward pass does it),
and the answer is a square.

The worker cuts a square window round the patch's region, twice its
size for context, from the picture as it is by then (earlier patches
applied), takes it to sRGB with a gain that puts the window's mean
outside the hole at middle grey, resamples it to 512, asks, resamples
back, undoes the gain and the matrix, and hands `Retouch::apply_with`
the region's window; the blend under the coverage is the same as a
clone's. Fills are kept by patch, so a slider elsewhere does not ask
again. Choosing the Fill tool with the model not in the store opens
the model sheet first.

Two checks. Stripes with a grey hole come back continued at full
amplitude (bright 0.845, dark 0.244 against 0.85 and 0.25): the model
is not blending with its input. And on the bridge frame, a first hole
that half covered the couple gave what looked like a ghost of the
shirt, which was the model continuing the visible shirt into the hole,
as `the_fill_does_not_show_its_input_through` measures (correlation
0.57 with the input there); a hole over the whole couple takes them
out cleanly, the rail, the rock and the post carried through. So the
tool wants a generous stroke, which is how Lightroom's works too.

### SAM 3's license, read (2026-09-07)

§34 left Sky and People parts waiting on a reading of SAM 3's license,
since its concept prompting ("sky", "hair", "lips") would cover both.
Read: the "SAM License" of 19 November 2025 is Meta's own, not
Apache. It grants a royalty-free, worldwide, non-exclusive right to
use, copy, modify and redistribute, commercial use included; asks that
any redistribution carry the license; forbids reverse engineering and
trade-controlled uses; and lets Meta amend it. Not GPL-compatible in
the FSF sense, but that was never the question: under this repo's
rule the models are data fetched by the user on first use under their
own terms, never shipped, so a license that permits use is enough.
This one does.

The actual obstacles are elsewhere. The official weights on Hugging
Face are gated behind a manual approval, so a first-use download by
the app cannot fetch them anonymously; the user would have to accept
Meta's terms on the site and hand the app a token. And the community
ONNX export of the concept path needs a text encoder of 1.4 GB (364 MB
at int4) beside a 96 MB decoder, with no vision encoder exported yet:
more than every other model here together, for two masks. So SAM 3 is
not the route for now. Sky wants a small segmentation model with a
clean license, or SAM 2 seeded by a sky heuristic; People parts want
MediaPipe's landmarks (Apache-2.0) driving SAM 2 point prompts, as §34
said. SAM 3.1 has since appeared; same license, same gate.
