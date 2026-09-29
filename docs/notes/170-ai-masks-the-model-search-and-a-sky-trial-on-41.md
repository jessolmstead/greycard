# 170. AI masks: the model search, and a sky trial on 41 frames (2026-09-25)

Roadmap v0.5.0's AI masks line, reworded on 2026-09-24 to say what is
wanted: Sky first, and it must never paint where there is no sky; then
the landscape's other features; then the body's parts and clothing as
shapes of their own. Two opus agents did the legwork, a model search
over the face, body and sky models with their licenses, and a trial
of six sky priors on a test set. Nothing landed; this section records
what was found so the Sky shape can be built from it.

**The rule the search ran under.** No non-commercial terms on the
weights or on the training data. Read literally that rules out two
models the editor already ships: SAM 2.1 was trained on SA-1B, which
is licensed for research only, and BiRefNet, like nearly everything,
starts from an ImageNet-pretrained backbone whose terms are research
only. The reading used from here on, which is the user's call to
confirm: clean when the weights allow any use and either the data
does too or the data's owner published the weights under that license
(Meta with SA-1B, Google with its own captures); clean with the
ImageNet caveat when the only non-commercial link is a pretrained
backbone, which is where BiRefNet and SAM already sit; ruled out when
a third party trained on someone else's non-commercial data or the
weights' own license forbids commercial use.

**Faces and bodies.** Every CelebAMask-HQ model (the dataset is for
non-commercial research only), every LaPa and LIP and ATR model, the
DeepFashion2 and ModaNet garment parsers, the FaceSynthetics heads
(Microsoft's research-use agreement counts trained models as
results) and NVIDIA's SegFormer lineage (its license says
non-commercial) are out. Sapiens2 gives upper and lower garments,
shoes and socks under a restrictive license and is out. What is left:
EasyPortrait's FPN-ResNet50 face parser at 1024 (SberDevices; its own
attribution license, a reworked CC BY-SA that says a format
conversion is not an adaptation, so an ONNX file greycard hosted
would owe attribution and not share-alike; classes skin, brows, eyes,
lips, teeth; 114 MB as ONNX, rebuilt in plain PyTorch since the
publisher ships a checkpoint only; the fetch bucket is on a sanctioned
Russian cloud, which is the second call for the user: convert and
host it ourselves as BiRefNet was, or not use it); MediaPipe's Face
Landmarker and Selfie Multiclass (Apache-2.0 from the data's owner;
the iris as a circle fitted to the landmarks and clipped to the eye's
mask, body skin and clothes as one class each, coarse at 256 px);
and SAM 2 seeded from landmarks for hair and for a garment by click,
which the Object shape already runs. Teeth are a class in
EasyPortrait and untested, since no trial frame showed them. Two
WebGPU findings from the timing runs: a grouped convolution with a
channel multiplier comes out wrong on the WebGPU provider (the
BiSeNet-V2 export), and the small models are slower on the card than
on the CPU. All of this waits on the user's two calls and on the sky.

**The sky set.** Forty-one raws of the user's, picked for the
purpose: 36 with sky, from clear skylines to sky in the gaps of a
blossom canopy, and five without, which the user named before the
trial and the agent was not told: two conifers against defocused
snow slopes, backlit sheer curtains, a cream building facade, a forest
path. The agent's own judgment of which frames had no sky matched
that list exactly, five for five, so the scores below are against the
user's ground truth. Two frames show sky through a window and were
scored as having sky; a Sky shape that leaves windows alone would
turn those two misses into correct answers. The frames were
developed by the CLI to 2048 px previews and, for the edge crops, at
full size. The frames and every overlay stay in the session's scratch
and out of the repo.

**Six priors, and the rule that matters.** A frame counts as a false
sky if the mask touches anything that is not sky, a speck on a wall
included, because a look on a sky mask that leaks is worse than no
mask. Under that rule:

| Prior | False skies of 41 | Missed of 36 |
|---|---|---|
| SAM 2.1 seeded from the top band (the earlier baseline) | 11 | 5 |
| Grounding DINO box, then SAM | 8 | 0 |
| Florence-2 grounding box, then SAM | 11 | 0 |
| Florence-2 referring segmentation | 10 | 0 |
| CLIPSeg, prompt "sky" | 4 | 0 |
| OWL-ViT box, then SAM | 1 | 25 |
| DETR-R50 panoptic's sky class, alone | 0 | 3 |
| EoMT-S panoptic's sky class, alone | 1 | 3 |
| DETR's sky seeding SAM, clipped to DETR's sky | 0 | 3 (2 partial) |
| EoMT's sky seeding SAM, clipped to EoMT's sky | 0 | 5 |

The text-prompted detectors fail the rule by construction: Grounding
DINO returns a "sky" box on all 41 frames, at scores up to 0.78 on
frames with none; Florence-2's grounding always returns a box;
OWL-ViT does not detect sky at all (its best score was 0.16). CLIPSeg
paints a facade, a snow slope and glass that reflects sky. The
learned panoptic priors are the answer: a model that was taught what
sky is, against a hundred other classes, does not see it in a
curtain. DETR-R50 panoptic (Apache-2.0, COCO panoptic, 800 px short
side) painted no false sky on any frame and missed three, the two
windows and the blossom canopy. EoMT-S (MIT weights, DINOv2 backbone
under Apache-2.0, 640 px, 96 MB) painted one false sky alone, a
bluish defocused snow slope, and none once SAM was seeded from its
confident core, since the erosion left no seed there.

The labeling is not the transformers panoptic post-processor: when
only one query clears its threshold that post-processor gives the
whole frame to it, and DETR painted all of one snow frame as sky that
way. Instead each query is kept at class probability 0.5 or more,
each pixel takes the kept query with the highest class times mask
probability, and the soft sky map is the sky-weighted sum over the
queries. SAM is seeded with up to eight positive points over the
prior's confident sky (probability above 0.8, eroded 20 px at 2048)
and up to eight negatives over its confident non-sky, one decode per
positive, unioned, then clipped to the prior's sky dilated 12 px. No
confident core means no seeds, which means no sky.

**The gate.** DETR's hard label is already a gate on this set: the
labeled sky area is zero on all five no-sky frames, with a peak sky
probability of 0.00 to 0.74 (the 0.74 is the snow slope), while every
real sky had a peak of 0.92 or more and the smallest real sky was 5.3
percent of the frame. The gate to build: at least one confident core
(probability above 0.8 after an erosion of one percent of the long
side), at least half a percent of the frame labeled sky, and as
cheap insurance the sky touching the top or a side of the frame;
otherwise the shape offers nothing. With DETR that admits every sky
frame but the two windows and the canopy and rejects all five no-sky
frames; with EoMT it also rejects the blossom frame. The fallback
when the gate passes but the eroded core gives SAM no seed is the
prior's own mask, which turns the canopy from a miss into a partial.
The set has only five hard negatives; sky-blue walls, a lake
reflecting sky with none above it, a snowfield filling the frame are
what the gate still wants testing against.

**Speed, and one crash.** On the 9950X3D, PyTorch on the CPU: DETR
2.3 s a frame, EoMT-S 0.12 s, CLIPSeg 0.09 s, Grounding DINO 1.8 s,
Florence-2 3.2 s for the grounding pass alone; the SAM image
encoding 1.1 to 2.1 s through ONNX Runtime as the editor runs it,
the eight decodes under a second. Exported to ONNX and run through
the §159 bench on Dawn: EoMT-S 0.028 s on WebGPU against 0.145 s on
the CPU, matching to 6e-5 on the class logits; CLIPSeg 0.010 s.
DETR's export crashes the WebGPU provider outright (`munmap_chunk():
invalid pointer`, at optimization levels 1 and 3, not bisected) and
takes 1.2 to 1.4 s on the CPU through ONNX Runtime. So the pick for
the editor is EoMT-S with SAM behind the gate, DETR kept as the
reference while EoMT's two extra misses among branches and blossoms
are studied, and bisecting DETR's crash the alternative if those
frames matter more than a second.

**Edges at 100 percent do not pass.** Crops at 100 percent from the
full-size develops, the mask over the frame and the sky taken down
1.5 EV through it, which is the job a sky mask does. DETR's raw mask
is blocky: its mask head works at a quarter of an 800 px input, about
a fortieth of a 100 MP frame's width, and on one frame it covers half
a pole. DETR clipped to SAM has a smooth outline that stops 50 to 80
px short of curly hair all the way round, because SAM's mask is a
256-square logit map upsampled 45 times; darkened, that is a bright
rim around the head. The editor's guided filter at the Subject rule
(radius width/256, eps 1e-3) feathers the outline and cannot move it
50 px, so the rim becomes a glow; at radius 8 and eps 1e-4 it keeps
the rim. Sky among bare branches is not in the mask at all, since the
prior called it tree. One test beyond the brief: a color-line matte
on a band around the boundary (known sky the mask eroded by 4 percent
of the long side, known not-sky the complement eroded the same, each
pixel between projected onto the line from the local not-sky color to
the local sky color, both from normalized Gaussian blurs of the known
regions, then the tight guided filter) makes the hair a real matte
with no rim at 1.5 EV, the one flaw about 20 percent alpha on the
ear. It changes nothing among branches, which are farther than the
band from any known sky, and nothing where the preview clipped the
sky to white, since there is no color line to project onto. The
verdict by use: darkening a sky is fine at fit-to-screen on clean
silhouettes (skylines, ridges) and with the matte on hair against
clear sky; a color shift of the sky is fine on skylines and wrong
near hair without the matte; a sky replacement is out of reach on
every route, wanting the branch gaps, decontaminated edge colors and
the wires and lanterns in the sky handled.

**The landscape classes, from the same two models.** The label maps
give the roadmap's other features at no extra inference. Water: sea,
river and water-other merged into one, usable, and the horizon
between sea and sky clean on every open-water frame in both models;
EoMT finds a lake and the sea through a ferry window that DETR leaves
unassigned. Mountain: usable for distant ridges; on a near wooded
snowy slope the mountain, snow and tree boundary is a model's opinion.
Foliage: tree is usable, blossoms come out as tree with some flower,
but grass against tree is confused (DETR calls gravel grass and both
mislabel lavender), so one Vegetation shape (tree, grass, flower)
would hold and separate Grass and Tree shapes would not. Ground: road,
pavement and dirt merged, usable; separately arbitrary. Buildings:
building, house, roof and the wall classes merged, usable; a glass
tower's reflection of sky is not called sky. Snow: unreliable in both,
and not confused with sky anywhere but EoMT's one false frame.

**Training a sky model of our own: later.** The false-sky problem,
which was the priority, is solved on this set by an off-the-shelf
MIT prior plus a gate. What training would buy is the edge and the
branch gaps, and that is a matting problem, sky alpha at full
resolution, which a segmenter trained on polygon labels would not
solve either; the data for it would have to be built. Revisit if the
matte plus a sky-inside-tree test still fail on branch frames after
they exist, or if the COCO images' mixed Flickr licenses come to
matter.

**What the Sky shape is, then.** EoMT-S on the preview, the gate, SAM
2.1 seeded from the confident core with negatives in the confident
non-sky, the mask clipped to the dilated prior, the prior's own mask
when there is no seed; then a matte stage on a band around the
boundary, with a CPU reference and a test like any other op, in
linear scene data rather than the clipped preview; then a
sky-inside-tree test with its own trial. Water, Mountain, Vegetation,
Ground and Building shapes come from the same label map once Sky is
in. The two license calls, the ImageNet reading and hosting the
EasyPortrait conversion, stay with the user, and the face and body
parts wait on them.

**Addendum, the same night: sky inside the trees.** The user looked at
the recommended sheet and said it "totally gives up around tree
branches", which it did: the prior labels a canopy tree, gaps
included, at a fortieth of the frame's width; SAM is only allowed
inside the prior's sky; the matte's band reaches 4 percent past the
boundary and the gaps sit deeper. So a fourth stage was built and
run on the five branch and canopy frames, two clean-tree controls and
the five no-sky frames. It starts from the prior-and-SAM mask shrunk
to confident sky; the pixels in question are the prior's tree, flower
and unlabeled pixels within reach of that sky (20 percent of the long
side) plus the band around the mask's edge; the local sky color is a
wide normalized blur of the confident sky; each pixel is scored by its
brightness relative to the local sky and its color distance from it,
which keeps out snow, walls and blossoms; then the tight guided
filter, nothing clipped after. The sky grows in three passes, pixels
scored as clearly sky joining the known sky after each, and that is
what made it work: without it the sky color for one canopy came from
the blue sky far to its right and the warmer sunset gaps were
rejected. Of four settings the one to keep is "unmix", each pixel's
share of sky on a brightness ramp rather than a threshold, which
darkens most evenly with the faintest halos; a threshold leaves a
halo on every twig.

The data was the CLI's preview at two stops under, decoded to linear
and multiplied back, since the linear TIFF clips the sky too (6.8
percent of one frame at 1.0 where the sensor itself clips 0.8
percent); unclipped data barely changes the mask but matters for
judging it, since darkening a clipped sky turns it flat grey and draws
a white rim wherever the mask is partial, which made the earlier
crops look worse than the masks were. Results: bare branches against
an overcast sky are filled and darken evenly at 100 percent with a
faint rim on the twigs, a pass; the defocused white tree top's 50 to
80 px gap closes to the blurred edge without painting the tree, a
pass; branches against a sunset two stops over white gain the larger
gaps and not the fine-twig haze, a partial; defocused blossoms
against a grey sky gain the grey-blue gaps upper right with hard
edges and none of the pink, a partial; the blossom canopy gains
nothing, since the mask it grows from is empty there, though growing
from the prior's own labels instead finds gaps across the canopy top
that look like sky at 2048, a fallback worth having behind the gate.
The controls gain about 2 percent along their tree edges and the snow
stays unpainted. The five no-sky frames gain zero pixels under every
setting and both priors, because the stage only grows from known sky
and there is none. EoMT's mask is too small to survive the shrink on
two of the frames, where DETR's gains; that is the second count
against it. The remaining flaw is the guided filter spilling 1 to 5
percent of the added area onto building and ridge edges beside the
sky, invisible at fit-to-screen and a problem for a replacement. In
unoptimized Python: 0.55 s at 2048, 3 to 4 s at 24 MP, 13 to 17 s at
100 MP, so the editor runs it at preview size and brings the result
up with the guided filter, as the other masks are.

**Second addendum: a real alpha matte, tried classically.** The user's
verdict on the sky-inside-trees sheet was "pretty rough, none of these
are good enough", so the question became what makes a true alpha
through branches and hair under the license rule. Every matting
network in reach (ViTMatte, MatteFormer, FBA, IndexNet, GCA, MODNet's
matting stage) is trained on Adobe's Composition-1k, Distinctions-646
or AIM-500, all research-only, so the trial ran the training-free
solvers: closed-form matting (Levin, Lischinski and Weiss, 2006) and
KNN matting (Chen, Li and Tang, 2013) through pymatting (MIT, no
weights), closed-form again on full-resolution tiles of the boundary
band, SAM 2 on 1024 px tiles of the band, and the color-line matte and
the unmix stage as the baselines, each fed one trimap from the prior:
known sky the SAM-clipped mask shrunk by half a percent of the long
side, known not-sky everything outside the prior's sky and the mask
grown by two percent less the prior's tree, flower and unlabeled
pixels within 20 percent of the known sky, unknown the rest, 10 to 33
percent of a frame. Judged at 100 percent on the unclipped data with
the sky taken down 1.5 EV and, since a color shift shows a bad alpha
more than a darkening does, with a saturation and hue shift through
the matte. The solvers were the disappointment: where clear sky is
near they are clean but soft after the upsample from 2048, and where
the known sky is far away and a different color they spread a haze of
0.3 to 0.5 alpha over whole tree regions (mean alpha over the
controls' unknown region 0.25 to 0.39 against 0.11 to 0.20 for the
color-line matte); closed-form on full-size tiles gives hair as good
as the color-line matte and branches worse, with tile seams, at 41 to
457 s a frame; SAM on tiles is a sharper binary outline and not a
matte. The color-line matte at full size is the one that passes: clean
strands on the hair frame, crisp branches against overcast sky, a
clean edge on the defocused tree top, the five no-sky frames untouched
by every candidate, and its one flaw a yellow sunglass lens taking
about 25 percent alpha, which keeping the prior's person and object
classes out of the matte would fix. What no training-free method
fixes is the dense canopy whose nearest clear sky is far off and a
different color, twigs against sunset glare and the two blossom
frames: those want better sky samples inside the canopy, which is a
learned model's job, trained on data with a clean license that would
have to be assembled. So the Sky shape's edge stage is the color-line
matte at full resolution with unmix's growth passes to sample the sky
color close to each gap and the person and object classes excluded:
two blurred color estimates made at 512 px, a per-pixel projection
and one guided filter, linear in pixels and no sparse solver, 1.3 to
6 s a frame in Python at full size. The sky-inside-trees stage as a
mask grower is dropped.
